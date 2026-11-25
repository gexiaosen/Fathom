# Fathom 桌面客户端

Fathom（方寸）的 Rust + WGPUI 桌面实现。产品概览见仓库根 [`README.md`](../README.md)，架构与路线见 [`docs/PROJECT.md`](../docs/PROJECT.md)。

## 环境

- Rust 1.95+（当前验证 1.99.1）
- macOS 11+，Xcode Command Line Tools（Metal / CoreText）
- 分别支持 arm64（Apple Silicon）和 x86_64（Intel）

## 构建与运行

```bash
cargo build --workspace
cargo run -p fp-desktop --bin fp-desktop
```

## 测试

```bash
cargo test -p fp-desktop
cargo test -p fp-ui
```

## 打包

```bash
./scripts/package_macos.sh
# → dist/Fathom-arm64.app   + dist/Fathom-v0.0.1-arm64.dmg
# → dist/Fathom-x86_64.app  + dist/Fathom-v0.0.1-x86_64.dmg
```

默认依次打 arm64 和 x86_64 两个包，最低系统均为 macOS 11.0。只打其中一个：

```bash
ARCHES=arm64 ./scripts/package_macos.sh
ARCHES=x86_64 ./scripts/package_macos.sh
```

x86_64 需要额外安装对应 Rust 标准库（`rustup target add x86_64-apple-darwin`）。脚本会自动选择钥匙串中的 Apple Development / Developer ID Application 身份，开启 Hardened Runtime，严格验签，然后生成带 Finder 排版的 DMG。签名与布局细节见 [`docs/packaging.md`](docs/packaging.md)。

## Workspace

| 目录 | 职责 |
|---|---|
| `apps/desktop` | 窗口、UI、场景状态、命令、导入导出、入口二进制 |
| `crates/fp-core` | 单位、ID、基础类型 |
| `crates/fp-model` | 文档模型、序列化、校验 |
| `crates/fp-geom` | 多边形、面积、房间识别、吸附 |
| `crates/fp-history` | 命令栈与 undo/redo |
| `crates/fp-render2d` | 2D 相机、墙体/家具/房间批量绘制 |
| `crates/fp-render3d` | wgpu 网格、材质、光照与相机 |
| `crates/fp-ui` | 拾取、拖拽、碰撞与交互规则 |

## 调试二进制

- `wgpu_cube`：WGPUI + wgpu 旋转立方体，验证 3D surface。
- `ime_probe`：中文输入法事件探针。
- `bench_3d`：3D 离屏基准。

```bash
cargo run -p fp-desktop --bin wgpu_cube
cargo run -p fp-desktop --bin ime_probe
cargo run --release -p fp-desktop --bin bench_3d
```

## 数据

运行数据保存在 `~/Documents/Fathom/`（方案、最近列表、收藏、偏好）。首次启动会自动加载示例户型。

## 文档

- [`docs/packaging.md`](docs/packaging.md)：macOS 签名、DMG 排版与公证。
- [`docs/adr/`](docs/adr/)：3D 视口与 WGPUI 版本锁定决策。
