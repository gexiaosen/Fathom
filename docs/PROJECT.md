# Fathom 项目说明

## 产品边界

Fathom（方寸）是面向室内设计师、装修公司和全屋定制门店的本地户型工具。0.0.1 聚焦单机 macOS：户型绘制、2D 家具布置、3D 预览与漫游、材料报价和图纸导出。不包含账号、云同步、多人协作和移动端。

## 技术架构

- `apps/desktop`：WGPUI 窗口、UI、场景状态、命令分发、导入导出。
- `fp-model`：Document / Wall / Opening / Room / Furniture / Material / Measure。
- `fp-geom`：多边形、面积、房间识别、吸附与几何计算。
- `fp-history`：命令栈与原子撤销。
- `fp-render2d` / `fp-render3d`：2D 批处理与 wgpu 3D 渲染。
- `fp-ui`：拾取、框选、拖拽、碰撞与交互规则。

核心层不依赖窗口系统；桌面壳目前是 WGPUI/macOS。Windows / Linux 需要新增窗口与输入后端，属于后续独立工作。

## 当前状态（0.0.1）

- 2D：画墙、门窗、拆墙、测量、家具库、多选、对齐、图层、房间重命名。
- 3D：轨道视角、剖切、日照、夜景、房间飞行、第一人称漫游、门窗开洞。
- 文档：自动保存、最近方案、方案信息、撤销重做、模板。
- 导出：PNG、SVG、A4 PDF、报价 CSV、3D 截图。
- 分发：macOS 11+，arm64 与 x86_64 分开打包，DMG 拖拽安装，Apple Development 签名（未公证）。

## 打包

`app/scripts/package_macos.sh` 默认生成 0.0.1。DMG 背景和 Finder 布局是静态资产，位于 `app/packaging/dmg/`；签名自动选择 Apple Development / Developer ID Application。证书与公证凭据只通过环境变量或钥匙串提供，不入库。

## 路线

1. 稳定 macOS 0.0.1：回归测试、崩溃与性能、真实户型验收。
2. 完成自动更新与正式 Developer ID 公证。
3. 评估 Windows / Linux 桌面壳移植，不在当前版本承诺。
4. 之后再考虑模板资产、报价与施工图能力。
