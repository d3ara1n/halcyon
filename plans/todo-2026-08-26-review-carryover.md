# review 未闭合承接项收拢

> 已归档 review 的调查结论不随归档消失。本文件只保留仍未触发或未完成的承接项；完成项由对应实施档案和 notes 记录。

## 设备/中断接入重审

来源：[`archived/review-2026-08-ipc.md`](archived/review-2026-08-ipc.md)。

- [x] 设备与中断设计前重审静默谓词、IRQ 唤醒主人和对象订阅所有权；2026-10-23 设计核对见下文。代码接入与运行证明仍由设备实施验收承担，不能将本项勾选解释为 IRQ 已实现。

触发：设备/中断接入设计开始。核对[现有调度 idle](../os/kernel/src/sched.rs)、[观察源与 Hint](../os/kernel/src/task/wait_source.rs)后，将以下要求写入[设备 IRQ 合同](todo-2026-09-device-system-design.md#irq)：

- idle 在 SIE=0 下 WFI，外部唤醒不会自动进入 trap；run 安全点、睡前复检和醒后须接同一个有界外部 IRQ 领取原语，不能只加 SEIP trap 分支。
- 硬件来源按实际 hart/context 交付，IRQ use 保存 in-service/代次，WaitSet 持观察节点并负责 detach，source 仅经 Weak HintTarget 提示；不得保存线程/地址空间强根作为永久订阅。
- 领取、确认和撤销由 PLIC 同一短锁串行，孤立控制封源；一次只处理一源，通知在锁外。无 runnable 可进入 WFI，不等于硬件完成/设备静止；DMA 的静态隔离保留也不是 runnable 维护债务。

上述为源码与设计推演证据，未执行中断或热插实验。实施须验证全 idle 时插入、等待取消/线程退出、Ack 与撤销交错、控制者消散、持续 IRQ 下普通工作服务机会；唯一实施与失败处置记录仍在设备计划，本篇不另建修复队列。
