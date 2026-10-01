# 对象持有与释放链：内核整体接入（下阶段交接）

状态：**待开始**。本篇是新会话的直接入口：按责任链施工方式，一次重写内核的对象持有与释放链与 `vm`，删除旧路径，恢复内核编译与验收。

## 接手前必读（按顺序）

1. [设计总纲](todo-2026-09-29-system-redesign.md)：已确认原则、“设计推进方式”、“施工方式：按责任链整段替换”、责任链×就绪表。
2. [内存重构](todo-2026-10-01-memory-redesign.md)：**组件状态总表**与“内核 vm 拼装草图”。
3. [维护执行](todo-2026-09-29-kernel-maintenance-execution.md)：执行机制目标设计与内核拼装草图（ticket 锁、`MAINTENANCE` rank、事件执行点）。
4. [进程自动回收](todo-2026-09-29-process-automatic-reclamation.md)：回收结构目标设计（五阶段、终态屏障、发布顺序）。
5. [capability](todo-2026-09-28-capability-last-reference.md)：权限模型定稿与内核拼装草图（KernelObject 收窄、Work 正交）。

## 当前事实（基线为本篇所在提交）

- 已就绪、未接入内核的 crate：`os/capability`、`os/maintenance`、`os/page_table`（表帧 owner 帧承载）、`os/memory_space`（三分界事务、`OrdMap` ledger、`SegmentMap`/`RangeUse`）。host 单测全过，clippy 干净。
- **内核编译中断（用户确认 C1）**：唯一错误源为内核 `TableMem` 未实现 `adopt/reclaim`。不加临时 adapter，随本阶段重写恢复。
- 剩余设计门见各分案“剩余设计门”节；本阶段施工中遇到的未闭合项按“设计推进方式”现场收敛，写回所属分案。

## 工作约定

流程与授权由[总纲“设计推进方式”](todo-2026-09-29-system-redesign.md#设计推进方式用户确认)拥有，新会话先读该节，用户无需重复交代。

## 施工范围

1. `sync.rs` 新增 ticket 锁与 `ranks::MAINTENANCE`。
2. 内核维护执行：`MaintenanceQueue`、`WorkRecord`、每 hart 轮转器，接入 `sched::run`、`trap.rs` 返回边界与 idle；timer/remote 有界服务。
3. 内核 `vm/` 从 `task/proc.rs` 拆出：`AnySpace`、`ChangeWork`、整空间退休 Work、fault、入口握手、表帧 owner 目标形状、每表有效项计数。
4. capability 接入：`CapTable` 替换 HandleTable，KernelObject 收窄，`on_last_capability` 锁外发布。
5. 进程回收记录替换三执行根；Job/资金账户按分案接通。
6. 删除旧路径：`deferred_work/work_ledger/notify_work/request/retirement`、`os/work_debt`、`RegionOwner::Lease`、Tunnel lease、ProcessDrain、`Rights::MAP`、Tunnel 0x60–0x63。
7. ABI 同步面：`shared/erhino_shared`、`libkernel`、`libprocess` Collector、srv_init `drain_to_completion` 调用方。

## 完成门

`just check`、`just clippy` 通过；QEMU 进程退出/Kill/构造失败自动回收、Map/Unmap/Protect 并发、整空间退休验收；impls 回填；KNOWN_ISSUES 矫正区对应条目收缩；按 REVIEW 手册登记独立 Review。

## 后续

性能专题见[锁优化与无锁化评估](todo-2026-10-01-lock-performance.md)，本阶段完成后开始。
