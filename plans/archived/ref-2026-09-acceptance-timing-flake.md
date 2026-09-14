# 验收墙钟敏感性与历史 Tunnel 静默记录

> 性质：已结束的调查参考。当前无开放实施任务；若以后同一类失败复现，从本文的证据和触发条件重新立案，不长期保留活动 todo 或 KNOWN_ISSUES。

## 结论

2026-09 的 stress 失败包含两个已经分开的现象：

1. 竞态矩阵把有限轮次内两种合法终因都出现当作通过条件，合法单侧偏胜会误报 15/16 或 14/16。现已改为确定性覆盖 `Exited`/`Killed` 两种终因，真实竞速只检查允许结果、返回值与最终收束，并继续报告随机分布。
2. 一轮 `THROTTLE=100`、300 秒运行在 concurrent Tunnel close round7 完成后被外层 `timeout` 收割。原日志缺少随后 24 轮 Close/Attach 矩阵的逐轮进度，不能证明 Tunnel 卡死。后续相同 workload 多次完成 Tunnel 矩阵；诊断运行总耗时 223.446 秒，普通复跑 166.765 秒，另有历史完整 stress 约 87.554、97.356、167.232 秒。150 秒旧上限也曾在 Tunnel 已完成后截断。常规验证继续使用默认 50% 节流；100% 仅用于固定全速条件的专项诊断，不作为普通验收门。

因此该历史截断按**墙钟超时敏感的偶发验收现象**归档，而不是已确认的 Tunnel 正确性缺陷。QEMU guest 的工作量受宿主调度和负载影响，外层 `timeout` 计算真实墙钟；开发机负载较高时，同一 guest 工作会占用更长墙钟时间。guest 内 `sys_sleep` 基于虚拟平台时钟，QEMU 被宿主延迟或由 `qemu-throttle.sh` 暂停期间，guest 也无法推进；无论具体虚拟时钟补偿方式如何，外层墙钟仍会继续消耗，所以主机负载和节流会直接压缩可用执行预算。

这不是“证明 Tunnel 永远无问题”。归档含义是：现有证据不足以支持一个开放代码缺陷，且本轮已补齐再次发生时所需的身份与阶段观测；未来只有新现场满足下面触发条件才重开调查。

## 已实施的验收改进

- `tools/qemu-acceptance.sh` 为运行记录 run id、git commit、workload/profile、平台/模式/throttle、内核与 BootPackage 路径及 SHA-256；失败日志与 metadata 同名保留，并打印最后一条 `acceptance progress:`。
- `srv_init` 为竞态矩阵逐场景输出开始/结束锚点。
- `test_hammer` 为 concurrent Tunnel close、Close/Attach 每轮及其 Close/Attach/等待重试输出阶段和周期性计数。
- `last-thread-exit-vs-kill` 使用 Exit-first/Kill-first 确定性覆盖，不再依赖随机赢家分布。

验证：`just check`、stress 用户态构建、七面 `just clippy`、默认 `THROTTLE=50` 的多轮 `just virt-stress` 及一轮 `THROTTLE=100 just virt-stress` 通过；均完成 Tunnel 24 轮矩阵、竞态 16/16 与显式 reset。

## 历史证据边界

- 原截断：`artifacts/failed-acceptance-20260913-201112-17670.log`，最后锚点是 concurrent Tunnel close round7 完成，随后 SIGTERM；没有 panic、业务失败锚点或原现场 GDB。
- GDB 诊断：`artifacts/check/public-ipc-waiting-stall-diagnostic.log`、`public-ipc-stall-gdb-{1,2,3}.log`、`public-ipc-stall-timing.json`。该轮完成 Tunnel 矩阵，采样落在等待投递、MemoryMap 预检、Tunnel unmap 发布和 lifecycle 锁等待等不同路径，没有形成稳定死锁环；它不是原截断现场。
- 普通复跑：`artifacts/failed-acceptance-20260913-202204-31065.log`，完成 Tunnel 矩阵后命中旧概率判定。
- 150 秒旧上限：`artifacts/failed-acceptance-20260913-151419-55224.log`，Tunnel 与 thread suite 已完成后才被收割，说明总耗时不能归因于 Tunnel。
- 旧运行没有当前 metadata/hash，不能严格证明产物完全相同；后续通过也不反向证明原轮的具体内部状态。

## 未来重开条件

满足任一项时重新建立 `todo-<日期>-acceptance-reliability.md`：

- 在当前身份记录下，同一 Tunnel round/step 重复无进展并触发 300 秒 hard timeout；
- `attempts`/`waits` 持续增长但轮次不前进，显示可复现重试活锁或异常成本；
- 多次 GDB 样本形成稳定等待依赖环；
- 出现非法终因、错误 code、未收束 owner、退款缺失、panic 或映射残留；
- 正常 stress 在常规宿主负载下经常逼近 300 秒，需要重新校准时限或拆分 workload。

复现时固定 commit、ELF/BootPackage hash、QEMU 版本、hart 数、throttle、timeout 与宿主负载；连续运行预先约定轮数，不重跑直到绿。