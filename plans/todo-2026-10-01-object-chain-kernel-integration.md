# 对象持有与释放链：内核整体接入

状态：**施工中（合并替换阶段）**。设计接缝已收敛（advisor 一次 + LoneCrane 对抗多轮，含 capability 设计轮次 4 角色类型）。本篇是唯一工作记录与新会话直接入口：按责任链整段替换，一次重写内核对象持有与释放链与 `vm`，删除旧路径，恢复编译与验收。

## 接手前必读（按顺序）

1. [设计总纲](todo-2026-09-29-system-redesign.md)：已确认原则、“设计推进方式”、“施工方式：按责任链整段替换”。
2. [内存重构](todo-2026-10-01-memory-redesign.md)：设计轮次 3（VM 接缝：分步 ledger/PTE、端点物化、最后离开者、固定批次退休、epoch、fault 分类、运输复用）。
3. [维护执行](todo-2026-09-29-kernel-maintenance-execution.md)：执行机制、ticket 锁、`MAINTENANCE` rank、timer/remote 收敛结论。
4. [进程自动回收](todo-2026-09-29-process-automatic-reclamation.md)：设计轮次 2（Shell=Control、`Box<Body>`、出生准入、Job `try_commit_completion`）。
5. [capability](todo-2026-09-28-capability-last-reference.md)：权限模型、Tunnel/Mailbox 收敛、**设计轮次 4 角色即对象类型**（Mailbox 三类型 + ReplyObservation、Notification Owner/Signaler）。
6. [预算分案](todo-2026-09-14-kernel-memory-budget.md)：账户即 Pool、欠账转父、`ProcessCreate` 用 Pool `CREATE`（不新增 FUND）。

## 工作约定（用户确认）

- 流程与授权见总纲“设计推进方式”。已授权连续施工，不需逐步汇报；遇设计矛盾才停下讨论（LoneCrane / advisor）。
- 中间态允许不可编译；**整链替换完成前不跑验收**，多个步骤做完再统一编译修错。
- 避免重复改写：会被整体替换的旧文件（`proc.rs`、`process.rs` 等）不在旧结构上先修一遍。
- 设计推翻线稿时按 AGENTS 设计轮次规则先记计划再改 ideas。

## 交付边界

本链交付：维护执行、`vm/`、本地 CapTable 释放、角色类型拆分、已关闭 Mailbox 有界收束、进程私有 Body 退休、Job 自主完成、Pool 资金账户、Tunnel lease 删除与 Runnel 迁移。
**不交付**：TRANSFER（队列 capability 环，归 IPC 分案；OnceSender 放弃不承诺穿越环）。
施工门（非设计）：`STEP_UNITS`/qM/qU/qT 定标、每步成本证明、Pool 父链退款深度（逐级转交或计入原子成本，择一写明）。

## 已完成（目标形状，均未接通整链）

| 区块 | 内容 | 证据 |
|---|---|---|
| `sync.rs` | `RawLock` trait；CAS `RawSpinlock` 与 FIFO `RawTicketLock`/`TicketSpinlock`；`ranks::MAINTENANCE=660`（叶锁）。其余锁未迁移 | 类型检查 |
| timer（`sched.rs`） | 统一 `earliest_future`/`arm_or_pend`（已到期→本 hart `HART_TIMER_PENDING`）；`service_expired` 每次至多 `TIMER_SERVICE_BATCH=16`；有 pending 不 WFI。**未做**：expire 后续清理移交 Work | 类型检查 |
| `maintenance.rs` | `Work` trait、内嵌 `WorkRecord`、`Account`（Ledger + Ready 链，`absorb_debt`）、全局 `QUEUE`、`arm/publish/wake/kick/has_ready`、`advance_one`、每 hart `Rotor`；已接 `sched::run`、idle、`wake_idle_harts`、trap `charge_user` | `os/maintenance` host 17 项 |
| `os/funded_frame` | `Funded<_,_,1>::{into_single,from_single}` | host 12 项 |
| `os/memory_space` | `split_at_boundaries`、`commit`+`commit_step`、事务区间过渡真值、租约保留到 `complete`、`release_allocation_step`、`span_reserved`、`FaultClass::Publishing{target}`、`RegionKind: Copy` | host 23 项 |
| `os/page_table` | `PublishCursor`、`worst_case_frames`、`publish_step`、`detach_empty_table`。**旧** `prepare/publish/publish_batch/generation` 仍在（过渡残留，见债务） | host tree 32 项 |
| `vm/table.rs` | 新 `TableMem`：`adopt` 同源校验后并入 `TableCharge: Option<AllocatedCredit>`，`reclaim` 恰好一页；`FundedTableFrame::{into_adopted,from_adopted}` | 类型检查 |
| `vm/space.rs` | `AnySpace{ledger,tree,account}`：`plan_cursors`、`fund_reserve`、`prepare_and_commit`、`step`、`detach_boundary_tables`、drain 接口；常数 `LEVELS=3`、`BOUNDARY_TABLES=RETIRE_BATCH=K=4`（几何上界）、`STEP_UNITS=16`（待定标）；`RetireBatch`、`ChangeProgress` | 类型检查 |
| `vm/change.rs` | `VmShell`/`VmState{space: Option<AnySpace>}`、`ChangeWork`（单 `state` 锁；分步→摘边界表→新 epoch→`Transport` fence→synchronize→锁外放帧→complete）、`RetireSink` | 类型检查 |
| `vm/retire.rs` | `RetireWork`：出生内嵌、`activate` 只激活、逐单元 drain、固定批次 fence 复用 | 类型检查 |
| `vm/fault.rs` | R/W/X 对照承诺权限；未发布锁外让出；陈旧 sfence 重试 | 类型检查 |
| `vm/remote.rs` | Work 内嵌每目标运输槽；最终 ack = fence 完成 ∧ 槽归还；最后者锁外 wake | 类型检查（注入错误核实覆盖） |
| `MemoryPool` | 内嵌 `maintenance: Arc<Account>`、`maintenance_account()`；child Drop 欠账并入父 | 类型检查 |
| ABI `erhino_shared` | 删 `Rights::MAP`（位 7 保留）、ProcessDrain、Tunnel lease/映射字段；Endpoint 事件用 SIGNAL；`ProcessCreate` 带 Pool；删 `HandleRole`，`ObjectKind` 改名 `MailboxReceiver`/`NotificationOwner` 并追加 `MailboxOnceSender=16`/`NotificationSignaler=17`/`ReplyObservation=18`；`HandleDescription.role`→`reserved0` | host 28 项 |
| 内核对象契约 | `Cargo.toml` 依赖 `os/capability`；`task/cap.rs`（`ObjectAuthority` 实现 `Authority`、`deliver`）；`KernelObject` 新增 `holders()`/`on_last_capability()`，删 `close_handle`/`close_transit`，`allowed_*` 去 role；15 个对象内嵌 `Holders`（ProcessControl `Remintable`，其余 `Terminal`）；业务关闭动作迁 `on_last_capability`（ProcessBuilder abort、Invitation abandon）；非 selftest 的 `MAP` 用点按映射授权矩阵改写 | 部分类型检查 |
| `task/handle.rs` | 整文件重写：`ProcessCapTable=CapTable`，`mint`/`release_all`/`resolve`（kind+rights）/`install`（预留→写回→publish→提交）/`query`/`close`/`duplicate`/`map_error`；删 `CapabilityRef`/`PendingClose`/`PendingEntries`/`entry()` | **未编译** |
| `task/notification.rs` | 整文件重写：`NotificationOwner`/`NotificationSignaler` 共享 core；shut 与 signal 同 core 锁 | **未编译** |
| `task/mailbox.rs` | 整文件重写：`MailboxCore`（队列、open、no-senders 聚合含 Pending OnceSender、在途 ReceiveTxn）+ `MailboxReceiver`/`MailboxSender`/`MailboxOnceSender`/`ReplyObservation`；入队与 Pending→Sent 同 core 锁；`claim→commit_claim/restore`；最后 Pending 释放→Abandoned；armed 随 `install` publish | **未编译** |

## 当前编译状态

内核不可编译（计划内）。最后一次统计在 handle/notification/mailbox 重写之前：44 个错误，来源为 Tunnel/ProcessDrain/`ProcessBindMemory` 旧调用点、`tunnel/selftest.rs` 6 处 `MAP`、`proc.rs` 旧 vm 接口；重写后进程侧调用点（`proc.rs`、`process.rs`、`wait_set`、`tunnel`、`job` 等约 230 处 HandleTable 用法）与新 `handle.rs` 不再匹配，数量会上升，随下述替换消失。

已知待补：`handle.rs` 调用的 `uaccess::write_user` 不存在（现有 unsafe `write_user_value`/`deliver_output`），统一编译时补安全包装或改调用。

## 下一步（按顺序，连续施工）

1. **`task/process/`**（替换 `proc.rs` 5475 + `process.rs` 1260，连同 `job.rs`/`thread.rs`/`lifecycle.rs`/`resources.rs` 相关部分，≈9.7k 行）：
   - `process/shell.rs`：ProcessShell = Control（回收 WorkRecord、唯一终态、Job 成员、Body gate；`Remintable`，重铸只发新 capability）。
   - `process/body.rs`：`Box<ProcessBody>`（线程表、`ProcessCapTable`、`VmShell`、私有许可；短 lease，不跨等待/返回用户态）。
   - `process/builder.rs`：构造 lease、armed 发布、Start/Abort 状态机；骨架建立后转成员。
   - `process/reclaim.rs`：五阶段 Work（Frozen→主动摘线程→等 lease 归零→释放 capability/激活 `RetireWork`→Settle 实际退款）；`ThreadDeparture` 改短期强持 Shell。
   - `process/syscall.rs`：create（Pool `CREATE`）、map/write/grant/start/query/kill。
   - 删 drain、`bind_memory`、`revive_control`、`DeadSnapshot`；`MetadataSponsor` 计数并入 Pool。
   - Job：`try_commit_completion`，Seal 时发布 Job Work，有界传播封口；删 `CompletionCursor`/`finish_completion`。
2. Tunnel（Endpoint 事件、删 lease 与 selftest `MAP`、Attach 一 handle）与 WaitSet 改用新 `handle.rs`。
3. 删除旧路径：`deferred_work`、`work_ledger`、`notify_work`、`request`、`retirement`、`os/work_debt`、`RegionOwner::Lease`、旧 `remote_call` 全局表、`page_table` 旧接口、旧 `TableMem`。
4. 首次统一编译修错，然后用户态：`libkernel`、`libprocess`（删 Collector drain）、`librunnel`（失败内部收束、ack 后重探）、`srv_fs`（就绪改 Start）、`srv_init`（删 `drain_to_completion`）。
5. 完成门验收。

## 债务登记

| 现状 | 目标 | 删除条件 |
|---|---|---|
| `page_table` 新旧接口并存 | 只留游标/摘表接口 | `proc.rs` 删除后（下一步 3） |
| `deferred_work`/`notify_work` drain 调用点仍在 `sched` | 由 Work 承接 | 消费者迁走后（下一步 3） |
| timer expire 后续清理未移交 Work | 移交既有 Work | 进程/等待替换时 |
| `STEP_UNITS`、qT 等暂定常数 | 按真实负载定标 | 整链验收后 |

## 完成门

`just check`、`just clippy` 通过；QEMU 验收进程退出/Kill/构造失败自动回收、Map/Unmap/Protect 并发、整空间退休；`notes/impls` 回填；KNOWN_ISSUES 矫正区对应条目收缩；COMPASS 更新；按 REVIEW 手册登记独立 Review。

## 后续

[锁优化与无锁化评估](todo-2026-10-01-lock-performance.md)，本阶段完成后开始。
