# 验收判定与静默窗口可靠性

> 状态：待实施。用户明确安排在后续完善验收机制；不阻塞当前公共对象前置继续施工，不把未通过的整体 stress 记为通过。本文是以下两项缺口的唯一实施真值点，公共 IPC 计划仅保留历史证据和链接。没有已确认的内核缺陷可据此豁免。

本计划的调查与改动仍遵循 `AGENTS.md`「标准施工流程」；当前只记录验收可靠性专题的特有范围、前置和判定门。

## 范围与前置

范围为 srv_init/test_hammer 验收编排、QEMU 运行身份与阶段观测、结果分类和调查工具。先审视正式退出/等待/退款契约与现有负载，再实施；若取证证明内核问题，单独立案并重新安排受影响前置，不用验收修改掩盖正确性问题。

前置是当前公共对象/Native continuation/退休路径稳定、对应 binary 与源码和运行配置可追溯。无需引入测试 syscall、内核抢占或内核线程。

## 概率覆盖误失败

- 现状：`user/services/srv_init/src/race.rs` 中 memory-vs-kill、last-thread-exit-vs-kill 要求有限轮次两种终因都胜出。合法结果全部偏向同一侧也会得到 15/16 或 14/16；短暂延迟不能保证胜者。
- 证据：`artifacts/failed-acceptance-20260913-190640-99659.log`、`201738-25206.log`、`202204-31065.log`；本轮 last-thread-exit-vs-kill 均 exited 0/killed 4。同一代码另一轮 exited 1/killed 3 通过该场景，但整轮因独立 Tunnel 截断失败。历史改动前后均观察到过合法偏胜及 16/16。
- 目标：退出语义验证与竞速覆盖分离。确定性编排保证两种终因各被验证，同时保留真正竞争的允许终因/退款/最终收束检查，不要求随机赢家分布。
- 完成标准：连续合法单侧获胜不能误失败；非法终因、错误 code、不可收束、遗漏退款仍必须失败；两种终因有明确覆盖证据，不靠增加轮数或重跑直到绿。

## Tunnel 静默与截断

- 现状：test_hammer `concurrent_tunnel_close` round7 后进入 `tunnel_close_attach` 24 轮矩阵，内部无逐轮进度锚点。`artifacts/failed-acceptance-20260913-201112-17670.log` 在明确 THROTTLE=100、300s 时限下截断；原因为未知。
- 证据：`artifacts/check/public-ipc-waiting-stall-diagnostic.log` 总 223.446s 通过 24 轮后因上项 15/16 主动失败收割；普通复跑总 166.765s 同样完成矩阵后 15/16。诊断三份 `public-ipc-stall-gdb-{1,2,3}.log`/`public-ipc-stall-timing.json` 分别观察等待投递/dispatch、MemoryMap 预检、Tunnel unmap 发布和另一 hart 等 lifecycle 锁；不是原截断轮现场，不足以归因。
- 目标：记录 workload、binary 身份、throttle、运行阶段、轮次及等待/重试进度，明确区分硬 timeout、业务断言失败、预期 reset 收割与资源失败。静默时在 guest 存活期间采样，而非只保留最后一行。
- 待证假设：页表每层完整槽遍历成本；同进程 yield 变化使 execution snapshot 失效、ObjectBusy 重试的频率与成本。只作为调查候选，不能据此删除执行快照验证、迁移范式或先放宽时限。
- 完成标准：定位截断具体阶段及是否有单调进度；给出可复现/统计证据与对应解释。正常长工作按实测成本重校超时，真正卡死或活锁单独修复；后续通过不能替代原失败归因。

## 自然顺序

1. 固化运行身份、业务阶段/轮次/重试观测与收束分类，保留完整失败日志；不得丢退出码。
2. 去除概率赢家分布的通过条件，补确定性终因覆盖和错误路径负例。
3. 同版本、同 throttle 复现 Tunnel 静默；结合阶段计数/GDB 判断有界慢工作、重试活锁或等待环。
4. 按已证原因修复机制或校准基础设施；执行 debug stress、release core 和板型组合，复核既往证据边界。
5. 完成后移除 KNOWN_ISSUES 对应条目，持久性观测契约转 notes，本文归档。
