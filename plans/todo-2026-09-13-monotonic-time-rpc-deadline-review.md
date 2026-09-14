# 公共时间与期限提交后代码复核

> 状态：待执行的提交后 Review；固定 `c6e0a84`，完成后归档。本文件不重复安排执行基座、FAL 或验收可靠性。

## 固定对象

- 提交：`c6e0a84`，`feat(time): 收口公共期限与运行期时钟停止`
- 分支：`task/fal-service-capabilities`
- 范围：ClockGeometry/Deadline、ClockState、CSR 采样排序、绝对 Wait/Sleep/Send、timer 消费、运行期协作停止、真实时间组合与对应文档。
- 复核固定快照：`git show --stat c6e0a84`；不得用后续修复替代该提交行为。

## 复核闭包

1. u128 换算、可读/可编程末端、向下/向上取整、Infinite/At 编码及非整频率边界。
2. `read_time` 的 R→I→RW fence、编译器内存约束、ClockState 高水位、origin 一 tick、回退/越界不可逆失败。
3. validated tick 是否覆盖所有运行期期限消费者；不允许 raw time 绕过统一状态。
4. Wait/Sleep 初始命中、At(0)、安装延期、timer token 取消、过期 Send 不消费 moves/once、已提交交付不撤回。
5. 运行期停止的 lock-free admitted ID 发布、IPI 广播、SSIP/STIP 屏蔽、scheduler/trap/idle/启动安全点、Ready gate 不回退及不伪造在途结果。
6. 真实用户期限、跨线程因果读取、平台/release/nofd、GDB 4/4 停驻证据和并行构建失败分类。
7. 文档是否明确跨硬件 epoch、RPC/FAL、验收可靠性计划 的边界；是否保留完整日志且不把主动收割或基础设施竞争写成 guest 成功。

## 验证

本提交已有 shared 27 项、os 140 项 host、`just check`、七面 clippy、virt core/release/nofd、sifive_u core 和 runtime-stop GDB probe（PARKED `0xf`）证据。日志位于本机 `artifacts/check/time-*`，不随 Git 提交；异机必须按当前源码重跑。完整 stress 的概率失败与 Tunnel 静默截断仍归 验收可靠性计划。
