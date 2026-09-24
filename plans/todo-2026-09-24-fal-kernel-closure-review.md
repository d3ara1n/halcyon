# FAL 服务能力与内核身份统一提交审查

## 对象与状态

待独立 Review。固定对象为闭包提交 `dda8a5b6f4fd7700378f9e85804c6b1d30826601`（父提交 `ff73a9e357b7bc936062b88d09867a982a4913e3`），以及已合入 `master` 后追加的故障验证提交 `5b29915111e0f406c9ddea28b3212b180b57d593`（父提交 `7b04c8135d215d73ce73b8e3137dee36425e630d`）。前者核对完整实现，后者单独核对验证能力和预算修改；不得以后者掩盖前者原有问题。本记录只安排提交后的只读审查，不替代提交前结构检查。

`dda8a5b` 包含 FAL F3d–F3f/F4、`test_fal` 正式 A/B 消费者、RPC→FAL→Create 生命周期、Copy 双端清理、provider 退出观察、内核 Rust 组件身份 `erhino_kernel` → `kernel`、构建/QEMU/boot-failure 入口同步，以及对应文档。`5b29915` 补充正式 A/B 的资源表配额拒绝/恢复、B 已接受部分数据后的在途 Copy 退出验证，并调整验收路线预算；两次提交的范围和结果分别核对。

## 审查范围

1. FAL 的 authority、owner、生命周期、失败/取消/退出/退款责任链，以及 `CallOperation`、`ClientOperation`、Create 分类和 Copy 清理是否在提交中保持单一真值。
2. `test_fal` 是否是正式 A/B 消费者而非测试专用替代机制；init 是否只保留装配、监督和最终收束责任。
3. provider CLOSED、已投递未知 Create、完成操作终态、双端 Cancel 和账户归零证据是否与实现一致；记录的 OOM、强制 close、Gate source refusal 限制是否没有被误称为已验。
4. 内核身份改名是否完整同步 package、binary target、crate symbol、产物、Justfile、GDB/workspace、boot-failure 工具、构建手册和有效文档；是否引入 alias、双轨产物或 ABI/运行语义变化。
5. `libservice`、FAL provider/backend、srv_fs 拆分和 Cargo workspace 依赖是否保持领域归属、单向依赖和构建入口一致。
6. notes/ideas、notes/impls、唯一工作记录、COMPASS、归档 Review 和当前 Review todo 是否各自保持事实归属，历史快照是否未被重写。
7. 提交是否只包含本次任务闭包；检查是否有遗漏的旧名有效引用、临时测试机制、重复真值、未登记债务或文档链接问题。
8. 对 `5b29915` 单独核对资源耗尽/恢复及 B 中途退出的可观察性、阶段握手与失败收束，以及预算调整是否有运行证据；不把未注入的 OOM、强制 close、Gate source refusal 当作已通过。

## 已有验证证据

提交前已通过：

- `just check`
- `just clippy` 七面
- `cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin`
- `just build_kernel`
- `just virt`
- `just virt-boot-failure`
- 完整 FAL `just acceptance`，日志 `artifacts/fal-f4-acceptance-final.log`
- `git diff --check`

后续 `5b29915` 的验收证据、耗时及仍未覆盖的正式故障注入条件由 [FAL 工作记录](archived/todo-2026-09-fal-service-capabilities.md#跨会话接力断点) 保留；日志属于当时本机，不随 Git 交付。上述证据不是独立 Review 结论，复核须分别核对两次固定变更，不能用当前 HEAD 替代目标提交。

## 复核门

- 无 P1/P2/P3 finding，或每个 finding 转入唯一 `review-*.md` 报告并登记修复顺序。
- FAL 完成门、验证限制和延期项与 `notes/impls/fal.md`、FAL 工作记录及 Compass 一致。
- `kernel` 身份无有效旧名残留，历史记录中的旧名保留为历史事实。
- 两次提交的结论分别可追溯；Review 不授权后续提交、push、发布或合并。
