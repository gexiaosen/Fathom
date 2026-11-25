# ADR-0001：3D 视口采用 WGPUI `WgpuSurface`

- 状态：Accepted（Phase 0 性能 gate 已实测通过）
- 日期：2026-11-20
- 决策者：项目负责人 + AI 架构

## 背景

GPUI 官方（gpui 0.2.2）是 2D UI 框架，没有跨平台“嵌入外部 wgpu 纹理”的稳定 API：

- `paint_image` 需要 GPU → CPU 回读，1080p 每帧约 8MB，实时漫游不可行。
- `paint_surface` 仅 macOS。
- 官方 external compositor PR 尚未合并。

产品要求 3D 实时预览、漫游和材质，必须把 wgpu 渲染结果零拷贝合成到 UI。

## 候选

| 方案 | 结论 |
|---|---|
| WGPUI（gpui-ce fork，统一 wgpu + winit） | 有 `WgpuSurface`，UI 树内直接嵌 render pass；crates.io 0.4.0 |
| 官方 GPUI + gpui_interop | 平台覆盖不全，macOS 未实现 |
| CPU 回读 + RenderImage | 仅静态预览，作 fallback |

## 决策

采用 **WGPUI 0.4.0** 作为 UI 基线，使用 `window.create_wgpu_surface(w, h, format)` 创建双缓冲 surface，`wgpu_surface(handle)` 嵌入 UI；渲染线程通过 `back_view_with_size()` 绘制，`present()` 零拷贝交换。

## 证据

- `wgpui 0.4.0` 在本机（Rust 1.99.1 / Xcode 27.2 / macOS 27.3）`cargo check` 通过，耗时 2m43s。
- 本仓库 `app/apps/desktop/src/bin/wgpu_cube.rs` 实现 1280×720 旋转立方体，编译通过，窗口内持续 60fps（vsync 上限）。
- wgpu 版本解析为 30.0.2（crates.io），与 WGPUI 内部一致。
- **离屏基准**（`bench_3d`，Apple M4 / Metal）：200 件家具代理 + 2048² 阴影 + 9-tap PCF @1920×1080，平均 **1.55 ms/帧（≈646 fps）**，p95 2.49 ms；3000 件无阴影仍 0.93 ms。基准可通过 `cargo run --release -p fp-desktop --bin bench_3d` 复现。

## 后果

- 优点：3D 零拷贝、UI 与 3D 同一窗口、跨平台统一 wgpu/winit 路径。
- 代价：依赖社区 fork，需要锁定版本并在升级时回归；WGPUI 的 API 可能落后官方 GPUI。
- 已消除的风险：Phase 0 的 3D 性能 gate（≥30fps@1080p/200 件/阴影）在 macOS 上以约 20 倍余量通过。
- 仍存在：Windows/Linux 实机未测；`WgpuSurface` 在窗口 resize、DPI 切换下的行为需在 Sprint 3 联调验证。
- 若后续实测不达标，备选顺序为：`gpui_interop` 官方互操作 → 独立 3D 子窗口 → 静态烘焙预览。
- 影响范围：`fp-render3d` 只依赖 wgpu，不依赖 WGPUI；UI 层通过 `fp-ui` 的 surface 适配器对接。

## 后续

- Sprint 1 补性能数据后更新本 ADR。
- 每季度评估上游 GPUI 是否提供官方 external compositor；若提供，评估迁回官方。
