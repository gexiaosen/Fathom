//! Recently opened plans, kept beside the autosave file.
//!
//! This is deliberately a small JSON sidecar rather than a project database:
//! paths remain user-owned files, and deleting the app data only forgets the
//! shortcuts.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentPlan {
    pub path: PathBuf,
    pub name: String,
    pub saved_at: u64,
}

fn store_path() -> PathBuf {
    let mut path = crate::scene::user_plan_path();
    path.set_file_name("recent.json");
    path
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn load() -> Vec<RecentPlan> {
    let mut entries: Vec<RecentPlan> = fs::read_to_string(store_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<RecentPlan>>(&text).ok())
        .unwrap_or_default();
    entries.retain(|entry| entry.path.is_file());
    dedupe(&mut entries);
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.saved_at));
    entries.truncate(MAX_RECENT);
    entries
}

fn write_entries(entries: &[RecentPlan]) {
    let body = serde_json::to_string_pretty(entries).unwrap_or_default();
    if let Some(parent) = store_path().parent() {
        let _ = fs::create_dir_all(parent);
    }
    let _ = fs::write(store_path(), body);
}

/// Remove a path from the shortcut list without touching the plan file.
pub fn forget(path: &Path) {
    let mut entries = load();
    entries.retain(|entry| entry.path != path);
    write_entries(&entries);
}

/// Move a plan into `Documents/Fathom/Archive` and forget its shortcut.
pub fn archive(path: &Path) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err("方案文件已不存在".into());
    }
    let mut dir = store_path();
    dir.pop();
    dir.push("Archive");
    fs::create_dir_all(&dir).map_err(|e| format!("无法创建归档目录: {e}"))?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "方案".into());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "json".into());
    let mut target = dir.join(format!("{stem}.{ext}"));
    let mut n = 2;
    while target.exists() {
        target = dir.join(format!("{stem}-{n}.{ext}"));
        n += 1;
    }
    fs::rename(path, &target).map_err(|e| format!("归档失败: {e}"))?;
    forget(path);
    Ok(target)
}

pub fn remember(path: &Path, name: &str) {
    let entry = RecentPlan {
        path: path.to_path_buf(),
        name: if name.trim().is_empty() {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "未命名户型".into())
        } else {
            name.trim().to_owned()
        },
        saved_at: now(),
    };
    let mut entries = load();
    entries.insert(0, entry);
    dedupe(&mut entries);
    entries.sort_by_key(|entry| std::cmp::Reverse(entry.saved_at));
    entries.truncate(MAX_RECENT);
    write_entries(&entries);
}

fn dedupe(entries: &mut Vec<RecentPlan>) {
    let mut seen = Vec::new();
    entries.retain(|entry| {
        if seen.iter().any(|path: &PathBuf| path == &entry.path) {
            false
        } else {
            seen.push(entry.path.clone());
            true
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, at: u64, path: &str) -> RecentPlan {
        RecentPlan {
            path: PathBuf::from(path),
            name: name.into(),
            saved_at: at,
        }
    }

    #[test]
    fn dedupe_keeps_the_first_of_each_path() {
        let mut entries = vec![
            entry("new", 3, "/tmp/a.json"),
            entry("other", 2, "/tmp/b.json"),
            entry("old", 1, "/tmp/a.json"),
        ];
        dedupe(&mut entries);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "new");
    }

    #[test]
    fn max_recent_is_bounded() {
        let mut entries: Vec<_> = (0..MAX_RECENT + 3)
            .map(|i| entry(&format!("p{i}"), i as u64, &format!("/tmp/{i}.json")))
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.saved_at));
        entries.truncate(MAX_RECENT);
        assert_eq!(entries.len(), MAX_RECENT);
        assert_eq!(entries[0].name, format!("p{}", MAX_RECENT + 2));
    }
}
