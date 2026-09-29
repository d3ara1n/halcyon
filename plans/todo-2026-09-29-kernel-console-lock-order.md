# Kernel console 锁序 panic

## 状态

本轮已完成代码施工，core/stress 与完整 acceptance 已通过；具体实现与路线证据由本记录拥有。
## 现象与证据

`just acceptance`、`just virt` 和 `just virt-stress` 在本轮 runtime 改动后均能完成内核自检、初始进程启动、服务拓扑建立以及 FAL2 中段检查，随后触发 debug kernel 锁序断言：

- 断言位置：`os/kernel/src/sync.rs:172`
- 请求位置：`os/kernel/src/console.rs:37`
- 现象：在持有 `Rank(220)` 的上下文中获取 `Rank(150)` 的 console 锁
- 验收结果：脚本按 failure-without-progress 以退出码 124 收束
- 复现日志：
  - `artifacts/dev/runtime-layering-round2-just-acceptance.log`
  - `artifacts/dev/runtime-layering-round2-virt-core.log`
  - `artifacts/dev/runtime-layering-round2-virt-stress.log`

本轮 runtime 改动未修改 kernel 锁实现；日志证明用户态启动采用、服务拓扑和 FAL2 已越过原先的启动停滞点，但不能据此把 kernel panic 归因于 runtime layering 已闭合。

## 范围

### 目标

1. 固定触发该锁序违规的第一调用链、持锁集合和 rank key。
2. 判断违规来自正常日志路径、panic/fatal 路径、跨 hart 输出交错，还是验收负载的调用顺序。
3. 在不绕过 Lock Ladder 断言、不把日志失败转成静默的前提下，修正 console 输出责任链或调用方锁边界。
4. 以 core、stress 和必要的 release/platform 路线重新验证，并更新 kernel 相关实现说明。

### 非目标

- 不修改 runtime layering 的 StartupSeed、ThreadRegistry、ResultCell 或 Unmap owner 责任链。
- 不把 panic 日志改成无条件忽略锁序错误。
- 不以增大验收超时替代定位或修复。
- 不在本计划中重做全局 Lock Ladder；发现跨专题锁序缺口时另立任务。

## 前置与未知项

- 已读取 DEBUG-PLAYBOOK 与 TOOLING-PITFALLS；调查按第一坏事件区分锁序 panic 与验收超时。
- 根因已固定：`Connection.state` 的 `CONNECTION(220)` 持有期间，`prepare_shootdown` 的一次性多 hart 诊断调用 `console_write` 获取 `CONSOLE(150)`。
- tunnel create/attach/close 三条调用链均横跨 `prepare_shootdown`；attach 另有锁内 `set_signals` 可能通过 work-ledger 诊断输出，已一并移到锁外。
- `prepare_shootdown` 现在只返回 active 快照；报告函数由调用方在高阶业务锁释放后调用，Lock Ladder 未放松。
- acceptance 早期超时与 core/stress 中段锁序 panic 已分别复核：本轮 core/stress 均正常 reset；旧中段 panic 的根因是锁内诊断，已修复，不与早期 timeout 互相归因。

## 调查顺序

1. 固定 workload、QEMU 参数、kernel ELF 和完整串口日志，清理残留 QEMU。
2. 对 `console_write` 的所有调用点建立持锁上下文表，标出 `Rank(220)` 的具体锁与调用链。
3. 用低成本日志/调用探针或 panic 前后事件窗口确定第一笔违规获取；必要时使用 DEBUG-PLAYBOOK 的 A/B 或执行环窗方法。
4. 依据第一调用链决定修复归属：
   - 调整调用方锁外输出；
   - 调整 console/panic 输出的锁路径；
   - 或修正文档/测试对合法日志路径的契约。
5. 更新受影响的 `notes/impls` 唯一拥有篇；若锁序长期契约改变，同步对应 `notes/ideas`。
6. 执行静态检查和最小复现，再执行构建手册要求的完整验收组合。

## 本轮实施与证据

- 代码：`os/kernel/src/task/proc.rs` 删除锁内多 hart `log!`，新增 `PreparedShootdown::active_count` 与锁外 `report_multi_hart_shootdown`。
- 代码：`os/kernel/src/task/tunnel.rs` create/attach/close 在释放 `Connection.state` 后报告；attach 的 endpoint/peer `set_signals` 也在锁外执行。
- 静态调查：除上述路径外，doorbell `warn!` 家族调用点未发现同类高阶锁嵌套；地址空间 unbound 不变量日志已移除，避免 `ADDRESS_SPACE(300)` 内取 console。
- 开发验证：`just check` 已通过，完整日志为 `artifacts/dev/runtime-layering-round3-check-2.log`；core/stress 与完整 acceptance 验收已通过。
- 新增真实 registry 证据：`test_hammer` 打满 runtime registry，Drop 所有 receiver，释放 worker，循环 `prepare/reap` 并对 probe 执行真实 join；marker 为 `round3 test=runtime_registry_reclaims_after_receiver_drop passed`。首次运行暴露测试自身未收束剩余 probe 的 `ObjectBusy`，已修复并由第二次 stress 通过验证。

## 完成门

- 第一违规获取点、外层锁 owner、rank key 和可达调用链有源码/日志证据。
- 修复不绕过锁序断言，不遗失 console/panic 输出责任，也不引入新的反向锁序。
- 基线对照和修复后复现结果均记录，至少 core 与 stress 路线不再触发该 panic。
- `just check`、`just clippy` 及适用的 acceptance/platform 路线通过；完整日志放在 `artifacts/dev/`。
- 受影响的 `notes/impls`/`notes/ideas` 与 `COMPASS` 状态准确；本轮 runtime layering R9 已通过，允许关闭本记录。

## 关联入口

- [运行时分层第二轮复核](review-2026-09-28-runtime-layering.md)
- [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md)
- [构建与验证手册](BUILD-AND-TEST.md)

## 关闭声明

第一违规链已修复：`CONNECTION(220)` 持有期间不再执行 console 输出或 endpoint/peer signal 通知；core、stress、完整 acceptance 和入口门均通过。本记录与运行时分层复核记录共同关闭。
