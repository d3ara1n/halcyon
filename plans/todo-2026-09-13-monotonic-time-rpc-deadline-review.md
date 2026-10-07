# 公共时间与期限提交后代码复核

> 状态：待执行的提交后 Review；固定 `c6e0a84`，完成后归档。本文件不重复安排执行基座、FAL 或验收可靠性。

## 固定对象

- 提交：`c6e0a84`，`feat(time): 收口公共期限与运行期时钟停止`
- 分支：`task/fal-service-capabilities`
- 范围：ClockGeometry/Deadline、ClockState、CSR 采样排序、绝对 Wait/Sleep/Send、timer 消费、运行期协作停止、真实时间组合与对应文档。
- 复核固定快照：`git show --stat c6e0a84`；不得用后续修复替代该提交行为。

## 后续轻量候选（本轮仅安排）

2026-10-07文档整理将**纯时间换算与Deadline编码**选为后续局部Review候选，本轮没有执行它。原`c6e0a84`完整提交Review的范围和状态保持不变；下面的当前快照候选不能替代原提交审查。

| 项目 | 安排 |
|---|---|
| 候选固定目标 | `f174d8e`的`shared/erhino_shared/src/time.rs`及其既有host用例；实际开始前核对源码是否仍与该目标一致 |
| 需要核实的契约 | u128换算、向下/向上取整、可读/可编程末端、Infinite/At编码及非整频率边界 |
| 排除项 | CSR采样、ClockState并发、timer/Wait/Sleep/Send消费者、runtime_stop及平台/GDB组合；不由本候选宣告这些部分通过 |
| 完成产物 | 若后续启动，按REVIEW手册登记实际固定范围、独立报告、契约依据与未覆盖部分；发现的问题由唯一报告承接 |
| 本轮完成状态 | 仅完成候选选择及范围/排除项登记；未审代码、未运行测试、未形成findings或通过结论 |

## 复核闭包

1. u128 换算、可读/可编程末端、向下/向上取整、Infinite/At 编码及非整频率边界。
2. `read_time` 的 R→I→RW fence、编译器内存约束、ClockState 高水位、origin 一 tick、回退/越界不可逆失败。
3. validated tick 是否覆盖所有运行期期限消费者；不允许 raw time 绕过统一状态。
4. Wait/Sleep 初始命中、At(0)、安装延期、timer token 取消、过期 Send 不消费 moves/once、已提交交付不撤回。
5. 运行期停止的 lock-free admitted ID 发布、IPI 广播、SSIP/STIP 屏蔽、scheduler/trap/idle/启动安全点、Ready gate 不回退及不伪造在途结果。
6. 真实用户期限、跨线程因果读取、平台/release/nofd、GDB 4/4 停驻证据和并行构建失败分类。
7. 文档是否明确跨硬件 epoch、RPC/FAL、验收时间敏感归档 的边界；是否保留完整日志且不把主动收割或基础设施竞争写成 guest 成功。

## 验证

本提交已有 shared 27 项、os 140 项 host、`just check`、七面 clippy、virt core/release/nofd、sifive_u core 和 runtime-stop GDB probe（PARKED `0xf`）证据。日志位于本机 `artifacts/check/time-*`，不随 Git 提交；异机必须按当前源码重跑。后续完整 stress 的概率判定与 Tunnel 墙钟敏感现场见 [只读归档](archived/ref-2026-09-acceptance-timing-flake.md)。
