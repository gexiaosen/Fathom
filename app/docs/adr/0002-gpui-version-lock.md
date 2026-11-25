# ADR-0002：GPUI / WGPUI 版本锁定策略

- 状态：Accepted
- 日期：2026-11-20

## 决策

- `wgpui` 锁定 **0.4.0**（crates.io），`wgpu` 锁定 **30.0.2**，两者必须同源同版本。
- 提交 `Cargo.lock`；CI 使用 `--locked`，禁止浮动升级。
- Rust 最低版本 1.95（wgpui 的 MSRV），开发使用 1.99.1。

## 升级规则

1. 每季度一次升级窗口，不在 Sprint 中途升级。
2. 升级前跑：`cargo test --workspace`、性能基准、2D/3D golden image。
3. 升级后 3D 帧率或启动时间回退 > 10%，回滚并记录 ADR。
4. 若必须 patch WGPUI，vendor 到 `vendor/wgpui` 并用 `[patch.crates-io]` 指向本地，patch 内容单独文档化。

## 原因

GPUI/WGPUI 均为 pre-1.0，API 破坏性变更频繁；产品开发不能被上游升级节奏拖住。锁定 + 定期升级是成本最低的策略。

## 后果

- 安全补丁需要手动 backport；每季度评估一次上游安全公告。
- 新特性（如官方 compositor）不会自动获得，需主动评估。
