# 生命周期监督 Authority 与失败升级政策计划

> 当前 Review findings 归并后的机制计划。目标是建立可重启、可接管、可诊断的用户态监督状态机，不在 init/pm 各处逐个替换无限等待。

## 目标

监督者必须始终满足：

- authority 在目标未完成收束前不丢失；
- 必选拓扑失败不会伪装成正常启动；
- 等待、Drain、Query 都有有限预算与进度语义；
- 失败可重新枚举、重试或升级为明确的 failed/unmanaged/整树收束；
- manager 重启或单个监督步骤失败后，另一持有合法 authority 的管理者可接管；
- 内核 `ProcessControl`/`JobControl` 的 CLOSED/REAPABLE 仍只表示已定义的生命周期屏障，不由用户态日志替代。

## 当前问题簇

- `srv_init::launch_test_services` 对普通服务 spawn 失败只记录日志并继续；只有 pm 缺失才返回阶段错误。
- `srv_init::supervise_services` 在 Drain 或 Query 失败后关闭 control 并移除监督项，可能丢失最后 authority。
- `srv_init`、`srv_pm`、`libprocess::job_kill` 多处使用 `WAIT_TIMEOUT_INFINITE`，没有 deadline、重试预算或失败升级。
- pm 委托域管理失败仅降级日志，init 兜底边界没有结构化记录。

## 最终监督状态机

监督项不再只是 `(pid, control)`，而应表达策略与进度：

```text
Expected
  → Starting
  → Running
  → Terminating
  → Reapable
  → Draining(cursor/budget)
  → VerifiedDead
  → Removed
```

失败分支必须显式分类：

```text
TransientBusy / Timeout
  → retain authority
  → re-enumerate / retry with bounded backoff

PermanentFailure / AuthorityLost / InconsistentSnapshot
  → mark Failed or Unmanaged
  → hand off to parent/root supervisor or terminate containing Job
```

`Removed` 只能在 Drain Complete 且终态 Query 成功后发生。任何失败都不能以 `close(control)` 作为默认收尾。

## 拓扑启动政策

启动前声明服务集合与必选/可选属性：

- 必选服务任一 spawn、Bind、Map、Write、Grant、Attach、Start 失败，整个 stage 进入统一 failure；
- 统一 failure 关闭临时 mailbox/authority，并以持有的 JobControl 对 services 域执行有界收束；
- 可选服务失败必须登记为显式 `Degraded`，后续监督、IPC 和验收不得把它当作成功启动；
- pm_domain 目标失败必须进入域级失败状态，由 init 或 pm 的直接 authority 接管，不允许只写日志。

启动结果应携带实际拓扑与缺失/失败集合；监督循环只接受已声明集合的完整快照。

## 等待与预算

统一定义监督策略参数：

- 单阶段 deadline；
- retry/backoff 次数；
- 单次 `drain_to_completion` work budget；
- 重新枚举次数；
- 预算耗尽后的 escalation policy。

普通 `wait_many` 不再由业务代码直接传无限期限，除非该等待明确属于不可失败的协议自测并不承担系统收束职责。`libprocess::job_kill` 应接收或通过 policy 获得 deadline/budget，并在超时后返回包含进度与残留 authority 的错误。

## Authority 保留与接管

- 每个监督项在 `REAPABLE`、Drain 和 Query 期间保留 control；
- Drain/Query 返回 Busy 或暂时错误时，保留项并在下一轮重试；
- 其它错误转为带 pid/job、阶段、进度、原始错误的 `SupervisionFailure`；
- 只有确认 Dead/CLOSED 且 Drain 完成后才 close 最后一份 control；
- pm 的委托域 control 与 init 的兜底 control 分开记录，明确谁负责当前阶段，避免两个管理者同时消费同一份可变政策状态；
- manager 自身失败不触发内核隐式级联，root supervisor 通过合法 capability 接管或将域标记 unmanaged。

## 与内核生命周期的边界

内核只负责：

- ProcessControl/JobControl 状态与电平；
- ProcessKill、Drain、Query 的原子/有界单步；
- CLOSED/REAPABLE 的准确发布。

用户态负责：

- 递归 JobKill；
- 目标集合、必选服务政策、重启策略；
- deadline/retry/escalation；
- supervisor handoff 与最终 reset 意图。

不通过增加内核递归扫描、内核线程或隐藏 init 特权解决监督问题。

## 自然实施顺序

1. 在 `libprocess` 定义带 deadline/budget/progress 的收束结果与策略接口；
2. 将 `srv_init` 的服务启动改为声明式必选/可选集合和统一 stage failure；
3. 将 `supervise_services` 改为保留 authority 的状态机，Drain/Query 失败不移除；
4. 将 `srv_pm` 委托域收束迁移到同一 policy helper；
5. 将 init/pm 关键无限等待替换为有限等待与重试/升级；
6. 为 supervisor handoff、manager 重启、Job/Process 残留和预算耗尽定义协议/记录；
7. 补 OOM、ObjectBusy、Query fault、IPI/Drain 停滞、必选服务缺失和 pm_domain 失败剧本；
8. 运行 core/stress/release/sifive_u/acceptance 并验证失败状态可观察、authority 不丢失、最终 reset 只由政策提交。

## 完成标准

- 必选服务失败不继续进入正常 IPC/监督脚本；
- Drain/Query 失败不会静默 close control 或从监督集合移除；
- 所有系统收束等待都有明确 deadline、重试预算和升级结果；
- `job_kill` 返回值包含未完成阶段与仍存 authority，不以无限等待隐藏停滞；
- init/pm 可在失败后由 root/委托 authority 接管；
- supervisor 的失败、降级、重试、接管和最终终态均有可 grep 的正式英文日志；
- 设计与实现文档同步 `notes/ideas/{task,bootstrap,service}.md`、`notes/impls/{task,startup,internals}.md`；
- 原 C-1 P1/P2 findings 全部可标为机制重构闭合，并完成故障注入复核。

## 依赖与边界

- 事务 Commit/rollback/Bound rollback 依赖 `todo-2026-09-memory-transaction-state-machine.md` 的最终闭包，但监督政策不得等待内核无限收束；必须能表示 pending/failed/hand-off。
- Admission 只提供启动失败广播，不决定服务策略；遵循 `todo-2026-09-admission-fail-closed.md`。
- 不引入新的内核 shutdown 推断；显式 SystemReset 仍由用户态终态政策决定。
