//! GitHub Releases based update check and installer.
//!
//! The update is staged from the signed DMG, verified, and swapped in by a
//! small detached shell helper after the current process exits. No Sparkle
//! framework, appcast, or elevated helper is required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const RELEASE_API: &str = "https://api.github.com/repos/gexiaosen/Fathom/releases/latest";
const RELEASE_PAGE: &str = "https://github.com/gexiaosen/Fathom/releases/latest";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub title: String,
    pub notes: String,
    pub page_url: String,
    pub download_url: String,
    pub asset_name: String,
}

#[derive(Clone, Debug)]
pub struct PreparedUpdate {
    pub release: Release,
    pub staged_app: PathBuf,
    pub destination: PathBuf,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    html_url: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Serialize, Deserialize, Default)]
struct UpdateState {
    #[serde(default)]
    last_check: u64,
}

pub fn current_version() -> String {
    // The packaging script can set a different CFBundleShortVersionString from
    // Cargo's package version, so the bundle is authoritative at runtime.
    current_app_bundle()
        .ok()
        .and_then(|bundle| bundle_version(&bundle))
        .map(|version| normalize_version(&version))
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// Returns the latest release if it is newer than the running build.
///
/// `releases/latest` already excludes drafts and prereleases. The function is
/// intentionally blocking; call it from the background executor, never from the
/// UI thread.
pub fn check_latest() -> Result<Option<Release>, String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Fathom/{}", current_version()))
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|err| compact_error(&err.to_string()))?;

    let response = client
        .get(RELEASE_API)
        .header("Accept", "application/vnd.github+json")
        .send()
        .map_err(|err| compact_error(&err.to_string()))?;

    if !response.status().is_success() {
        return Err(format!(
            "GitHub API 返回 HTTP {}",
            response.status().as_u16()
        ));
    }

    let github: GithubRelease = response
        .json()
        .map_err(|err| compact_error(&err.to_string()))?;

    let version = normalize_version(&github.tag_name);
    let current = current_version();
    if !is_newer(&current, &version) {
        return Ok(None);
    }

    let page_url = github
        .html_url
        .filter(|url| url.starts_with("https://"))
        .unwrap_or_else(|| RELEASE_PAGE.to_owned());
    let asset = pick_dmg(&github.assets);
    let (download_url, asset_name) = asset
        .map(|asset| (asset.browser_download_url.clone(), asset.name.clone()))
        .unwrap_or_else(|| (page_url.clone(), String::new()));

    Ok(Some(Release {
        version,
        title: github
            .name
            .filter(|name| !name.trim().is_empty())
            .unwrap_or_else(|| format!("Fathom {}", github.tag_name)),
        notes: plain_notes(github.body.as_deref().unwrap_or("")),
        page_url,
        download_url,
        asset_name,
    }))
}

/// Downloads the architecture-specific DMG, verifies the app inside it, and
/// copies the new bundle into the update cache. Blocking: call from the
/// background executor.
pub fn download_and_stage(release: &Release) -> Result<PreparedUpdate, String> {
    if release.asset_name.trim().is_empty() {
        return Err("这个版本没有提供 DMG 安装包，请到 Release 页面手动下载".into());
    }

    let destination = current_app_bundle()?;
    let parent = destination
        .parent()
        .ok_or_else(|| "无法确定 Fathom.app 所在目录".to_owned())?;
    if !can_write_dir(parent) {
        return Err(format!(
            "没有写入 {} 的权限，请先把 Fathom.app 拖入「应用程序」",
            parent.display()
        ));
    }

    let cache = update_cache_dir()?;
    let dmg_path = cache.join(dmg_file_name(release));
    let part_path = dmg_path.with_extension("dmg.part");
    download_file(&release.download_url, &part_path)?;
    if part_path != dmg_path {
        let _ = fs::remove_file(&dmg_path);
        fs::rename(&part_path, &dmg_path).map_err(|err| format!("无法保存下载的 DMG：{err}"))?;
    }

    run_checked(
        Command::new("hdiutil").arg("verify").arg(&dmg_path),
        "DMG 校验失败",
    )?;

    let mount_path = cache.join(format!("mount-{}", std::process::id()));
    let _ = fs::remove_dir_all(&mount_path);
    fs::create_dir_all(&mount_path).map_err(|err| format!("无法创建挂载目录：{err}"))?;
    let mount = MountGuard::attach(&dmg_path, &mount_path)?;

    let source_app = fs::read_dir(mount.path())
        .map_err(|err| format!("无法读取 DMG 内容：{err}"))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| path.extension().is_some_and(|ext| ext == "app"))
        .ok_or_else(|| "DMG 中没有找到 Fathom.app".to_owned())?;

    verify_staged_app(&source_app, &destination, &release.version)?;

    let staged_app = cache.join(format!("Fathom-v{}.app", release.version));
    if staged_app.exists() {
        fs::remove_dir_all(&staged_app).map_err(|err| format!("无法清理旧的更新缓存：{err}"))?;
    }
    run_checked(
        Command::new("ditto").arg(&source_app).arg(&staged_app),
        "复制新版 Fathom.app 失败",
    )?;
    let _ = Command::new("xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged_app)
        .status();

    drop(mount);
    let _ = fs::remove_dir_all(&mount_path);
    let _ = fs::remove_file(&dmg_path);

    Ok(PreparedUpdate {
        release: release.clone(),
        staged_app,
        destination,
    })
}

/// Spawn a detached helper that waits for this process to exit, replaces the
/// bundle, clears quarantine, and relaunches Fathom.
pub fn install_and_relaunch(staged_app: &Path, destination: &Path) -> Result<(), String> {
    if !staged_app.is_dir() {
        return Err("更新缓存已丢失，请重新检查更新".into());
    }
    if !destination.is_dir() {
        return Err("找不到当前 Fathom.app，请手动安装".into());
    }

    let cache = update_cache_dir()?;
    let script_path = cache.join("install-update.sh");
    let log_path = cache.join("install.log");
    let script = r#"#!/bin/sh
set -u
pid="$1"
src="$2"
dst="$3"
log="$4"
{
    echo "update started $(date)"
    while kill -0 "$pid" 2>/dev/null; do
        sleep 0.5
    done
    sleep 1
    rm -rf "$dst" || exit 1
    ditto "$src" "$dst" || exit 1
    xattr -dr com.apple.quarantine "$dst" 2>/dev/null || true
    open "$dst" || exit 1
    rm -rf "$src" 2>/dev/null || true
    echo "update finished $(date)"
} >>"$log" 2>&1
"#;
    fs::write(&script_path, script).map_err(|err| format!("无法写入更新脚本：{err}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755))
            .map_err(|err| format!("无法设置更新脚本权限：{err}"))?;
    }

    Command::new("/bin/sh")
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(staged_app)
        .arg(destination)
        .arg(&log_path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|err| format!("无法启动更新助手：{err}"))?;
    Ok(())
}

fn current_app_bundle() -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|err| format!("无法定位当前程序：{err}"))?;
    let macos = exe
        .parent()
        .ok_or_else(|| "当前程序路径不完整".to_owned())?;
    let contents = macos
        .parent()
        .ok_or_else(|| "当前程序不在 app bundle 中".to_owned())?;
    let bundle = contents
        .parent()
        .ok_or_else(|| "当前程序不在 app bundle 中".to_owned())?;
    if macos.file_name().and_then(|name| name.to_str()) != Some("MacOS")
        || contents.file_name().and_then(|name| name.to_str()) != Some("Contents")
        || bundle.extension().and_then(|ext| ext.to_str()) != Some("app")
    {
        return Err("自动更新只能在 Fathom.app 安装版中使用".into());
    }
    if bundle.to_string_lossy().contains("AppTranslocation") {
        return Err("当前 Fathom 正被系统隔离运行，请先拖入「应用程序」再更新".into());
    }
    Ok(bundle.to_path_buf())
}

fn update_cache_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "无法读取 HOME 目录".to_owned())?;
    let path = PathBuf::from(home).join("Library/Caches/Fathom/Updates");
    fs::create_dir_all(&path).map_err(|err| format!("无法创建更新缓存目录：{err}"))?;
    Ok(path)
}

fn can_write_dir(dir: &Path) -> bool {
    let probe = dir.join(format!(".fathom-write-{}", std::process::id()));
    match fs::File::create(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn dmg_file_name(release: &Release) -> String {
    let name = Path::new(&release.asset_name)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| name.ends_with(".dmg"))
        .map(|name| name.replace('/', "_"));
    name.unwrap_or_else(|| format!("Fathom-v{}-update.dmg", release.version))
}

fn download_file(url: &str, path: &Path) -> Result<(), String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent(format!("Fathom/{}", current_version()))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|err| compact_error(&err.to_string()))?;
    let mut response = client
        .get(url)
        .header("Accept", "application/octet-stream")
        .send()
        .map_err(|err| compact_error(&err.to_string()))?;
    if !response.status().is_success() {
        return Err(format!("下载失败：HTTP {}", response.status().as_u16()));
    }

    let mut file = fs::File::create(path).map_err(|err| format!("无法创建下载文件：{err}"))?;
    std::io::copy(&mut response, &mut file).map_err(|err| format!("下载中断：{err}"))?;
    Ok(())
}

fn verify_staged_app(app: &Path, current_app: &Path, expected_version: &str) -> Result<(), String> {
    run_checked(
        Command::new("codesign")
            .args(["--verify", "--deep", "--strict", "--verbose=2"])
            .arg(app),
        "新版 App 签名校验失败",
    )?;

    let current_bundle_id =
        bundle_identifier(current_app).ok_or_else(|| "无法读取当前 App 的 Bundle ID".to_owned())?;
    let new_bundle_id =
        bundle_identifier(app).ok_or_else(|| "无法读取新版 App 的 Bundle ID".to_owned())?;
    if current_bundle_id != new_bundle_id {
        return Err("新版 App 的 Bundle ID 与当前版本不一致".into());
    }

    let new_version = bundle_version(app).ok_or_else(|| "无法读取新版 App 的版本号".to_owned())?;
    if normalize_version(&new_version) != normalize_version(expected_version) {
        return Err(format!(
            "新版 App 版本号不匹配：期望 {}，实际 {}",
            expected_version, new_version
        ));
    }
    if !is_newer(&current_version(), &new_version) {
        return Err(format!("新版 {} 并不高于当前版本", new_version));
    }

    match (team_identifier(current_app), team_identifier(app)) {
        (Some(old_team), Some(new_team)) if old_team == new_team => {}
        (Some(_), Some(_)) => return Err("新版 App 的签名团队与当前版本不一致".into()),
        (Some(_), None) => return Err("新版 App 没有可验证的签名团队".into()),
        _ => {}
    }
    Ok(())
}

fn bundle_identifier(app: &Path) -> Option<String> {
    plist_value(app, "CFBundleIdentifier")
}

fn bundle_version(app: &Path) -> Option<String> {
    plist_value(app, "CFBundleShortVersionString")
}

fn plist_value(app: &Path, key: &str) -> Option<String> {
    let info = app.join("Contents/Info.plist");
    let output = Command::new("plutil")
        .args(["-extract", key, "raw", "-o", "-"])
        .arg(info)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!value.is_empty()).then_some(value)
}

fn team_identifier(app: &Path) -> Option<String> {
    let output = Command::new("codesign")
        .args(["-dv", "--verbose=4"])
        .arg(app)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines()
        .find_map(|line| line.trim().strip_prefix("TeamIdentifier="))
        .filter(|team| !team.is_empty() && *team != "not set")
        .map(ToOwned::to_owned)
}

fn run_checked(command: &mut Command, context: &str) -> Result<(), String> {
    let output = command
        .output()
        .map_err(|err| format!("{context}：{err}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let message = stderr
        .lines()
        .chain(stdout.lines())
        .find(|line| !line.trim().is_empty());
    Err(match message {
        Some(line) => format!("{context}：{}", compact_error(line)),
        None => context.to_owned(),
    })
}

struct MountGuard {
    mount: PathBuf,
    mounted: bool,
}

impl MountGuard {
    fn attach(dmg: &Path, mount: &Path) -> Result<Self, String> {
        run_checked(
            Command::new("hdiutil")
                .args(["attach", "-readonly", "-nobrowse", "-quiet", "-mountpoint"])
                .arg(mount)
                .arg(dmg),
            "挂载 DMG 失败",
        )?;
        Ok(Self {
            mount: mount.to_path_buf(),
            mounted: true,
        })
    }

    fn path(&self) -> &Path {
        &self.mount
    }
}

impl Drop for MountGuard {
    fn drop(&mut self) {
        if self.mounted {
            let _ = Command::new("hdiutil")
                .arg("detach")
                .arg(&self.mount)
                .arg("-quiet")
                .status();
            self.mounted = false;
        }
        let _ = fs::remove_dir_all(&self.mount);
    }
}

/// Daily throttling for the silent startup check.
pub fn should_check_daily() -> bool {
    let state = load_state();
    let now = unix_now();
    now.saturating_sub(state.last_check) >= CHECK_INTERVAL_SECS
}

pub fn mark_check_now() {
    let mut state = load_state();
    state.last_check = unix_now();
    let path = state_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(&state) {
        let _ = fs::write(path, text);
    }
}

fn state_path() -> PathBuf {
    let mut path = crate::scene::user_plan_path();
    path.set_file_name("update.json");
    path
}

fn load_state() -> UpdateState {
    fs::read_to_string(state_path())
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn normalize_version(tag: &str) -> String {
    tag.trim().trim_start_matches(['v', 'V']).trim().to_owned()
}

pub fn is_newer(current: &str, candidate: &str) -> bool {
    let Some(current) = parse_version(current) else {
        return false;
    };
    let Some(candidate) = parse_version(candidate) else {
        return false;
    };

    let len = current.len().max(candidate.len());
    for index in 0..len {
        let left = current.get(index).copied().unwrap_or(0);
        let right = candidate.get(index).copied().unwrap_or(0);
        if left != right {
            return right > left;
        }
    }
    false
}

fn parse_version(value: &str) -> Option<Vec<u64>> {
    let core = normalize_version(value)
        .split(['-', '+'])
        .next()?
        .to_owned();
    if core.is_empty() {
        return None;
    }

    let mut parts = Vec::new();
    for part in core.split('.') {
        parts.push(part.parse::<u64>().ok()?);
    }
    Some(parts)
}

fn pick_dmg(assets: &[GithubAsset]) -> Option<&GithubAsset> {
    let arch = if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else {
        ""
    };

    let is_dmg = |asset: &&GithubAsset| {
        asset.name.ends_with(".dmg") && asset.browser_download_url.starts_with("https://")
    };

    assets
        .iter()
        .filter(is_dmg)
        .find(|asset| !arch.is_empty() && asset.name.contains(arch))
        .or_else(|| assets.iter().filter(is_dmg).next())
}

/// GitHub release notes are Markdown. The update panel is a native text card,
/// so remove the most common markers instead of showing literal `###` / `**`.
fn plain_notes(markdown: &str) -> String {
    let mut lines = Vec::new();
    for raw in markdown.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with("<!--") || line.starts_with("![") {
            continue;
        }
        let line = line
            .trim_start_matches('#')
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .replace("**", "")
            .replace('`', "");
        if !line.is_empty() {
            lines.push(line);
        }
    }

    let mut notes = lines.join("\n");
    const MAX_CHARS: usize = 720;
    if notes.chars().count() > MAX_CHARS {
        notes = notes.chars().take(MAX_CHARS).collect::<String>();
        notes.push('…');
    }
    notes
}

fn compact_error(message: &str) -> String {
    let first = message.lines().next().unwrap_or(message).trim();
    if first.chars().count() > 160 {
        first.chars().take(160).collect::<String>() + "…"
    } else {
        first.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_numbers_compare_by_component() {
        assert!(is_newer("0.0.1", "0.0.2"));
        assert!(is_newer("0.0.9", "0.1.0"));
        assert!(is_newer("0.9.0", "1.0.0"));
        assert!(is_newer("v0.0.1", "v0.1.0"));
        assert!(!is_newer("0.1.0", "0.0.9"));
        assert!(!is_newer("0.0.1", "0.0.1"));
        assert!(!is_newer("1.0", "1.0.0"));
    }

    #[test]
    fn rejects_malformed_versions() {
        assert!(!is_newer("0.0.1", "nightly"));
        assert!(!is_newer("bad", "0.0.2"));
    }

    #[test]
    fn strips_markdown_markers_from_release_notes() {
        let notes = plain_notes("### 新增\n- **检查更新**\n\n`GitHub Releases`");
        assert_eq!(notes, "新增\n检查更新\nGitHub Releases");
    }
}
