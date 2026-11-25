# macOS 打包与分发

## 产物

```bash
cd app
./scripts/package_macos.sh
```

- `dist/Fathom-arm64.app` / `dist/Fathom-x86_64.app`
- `dist/Fathom-v0.0.1-arm64.dmg` / `dist/Fathom-v0.0.1-x86_64.dmg`
- 版本：`CFBundleShortVersionString=0.0.1`，`CFBundleVersion=1`
- 架构：arm64 与 x86_64 分开打包，不做 Universal 合并
- 最低系统：macOS 11.0

可通过环境变量覆盖：`ARCHES`（`all` / `arm64` / `x86_64`）、`MIN_MACOS`、`VERSION`、`BUILD_NUMBER`、`APP_NAME`、`BUNDLE_ID`、`FATHOM_SIGN_IDENTITY`、`MAKE_DMG=0`。

## 架构与工具链

- `arm64` 对应 `aarch64-apple-darwin`
- `x86_64` 对应 `x86_64-apple-darwin`
- 两个包都设置 `MACOSX_DEPLOYMENT_TARGET=11.0`，`Info.plist` 的 `LSMinimumSystemVersion` 同样是 `11.0`
- 打包脚本会检查二进制架构和 `minos`，不一致直接失败
- 使用 Homebrew Rust 时，需要为 `x86_64-apple-darwin` 补装标准库；推荐用 rustup 管理：
  ```bash
  rustup target add aarch64-apple-darwin x86_64-apple-darwin
  ```
- 如果 `cargo` 不在默认 PATH，可用 `CARGO=/path/to/cargo ./scripts/package_macos.sh`；脚本会把同目录的 `rustc` 放到 PATH 前面，避免工具链混用

只打一个架构：

```bash
ARCHES=arm64 ./scripts/package_macos.sh
ARCHES=x86_64 ./scripts/package_macos.sh
```

## 签名

脚本按 MiscKit 的规则自动选择钥匙串中的身份：

1. 优先 `Developer ID Application`
2. 否则 `Apple Development`
3. 都没有则 ad-hoc 签名并打印警告

签名开启 Hardened Runtime，不注入额外 entitlement。签完执行 `codesign --verify --deep --strict`，并打印 designated requirement。可用 `FATHOM_SIGN_IDENTITY` 强制指定。

> Apple Development 适合本机/内测分发，但不是 Developer ID，不能替代公证。公开下载建议配置 Developer ID Application。

## 公证（可选）

设置以下环境变量后，脚本会对 DMG 执行 `notarytool --wait`，成功后再 `stapler staple`：

```bash
export APPLE_ID=you@example.com
export APPLE_TEAM_ID=TEAMID
export APPLE_PASSWORD=app-specific-password
./scripts/package_macos.sh
```

证书、Apple ID 和密码不要提交到仓库；CI 使用 secrets。

## DMG 排版

DMG 采用静态 Finder 布局，打包阶段不调用 Finder：

- 窗口 600×360，图标 128px
- `Fathom.app` 位于 `(150,160)`，`Applications` 位于 `(450,160)`
- 背景图 `packaging/dmg/background.png`
- Finder 元数据 `packaging/dmg/DS_Store`
- DMG 文件和挂载卷都使用 `assets/AppIcon.icns` 作为自定义图标

脚本先生成可写 UDRW 镜像，在挂载卷根目录设置自定义图标位，再转换为 UDZO；最后给 `.dmg` 文件本身写入同样的图标资源。这样下载后 Finder 显示的是 Fathom 图标，而不是白色磁盘。

`background.png` 由 `scripts/make-dmg-background.swift` 生成。调整布局后，用 `scripts/make-dmg-layout.sh` 重新烘焙 `DS_Store`（需要一次性安装 `dmgbuild`）：

```bash
MAKE_DMG=0 ./scripts/package_macos.sh
python3 -m pip install --user dmgbuild
./scripts/make-dmg-layout.sh
```

日常打包只依赖 macOS 自带 `hdiutil` / `ditto` / `codesign`。

## 常见问题

- **DMG 里没有背景或图标位置不对**：确认 `packaging/dmg/DS_Store` 和 `background.png` 存在；重新运行布局烘焙脚本。
- **签名报 resource fork / detritus**：脚本已执行 `xattr -cr`；如仍失败，把构建目录移出 iCloud/Dropbox 同步目录后重试。
- **Gatekeeper 拦截**：未公证的包首次启动需右键打开；正式发布请使用 Developer ID + 公证。
