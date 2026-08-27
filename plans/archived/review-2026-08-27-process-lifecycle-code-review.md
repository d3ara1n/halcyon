# 进程生命周期 step 2–4 统一代码 Review

- 基线：`755fd9a`（审查对象为提交 `72a35e4`/`5684599`/`910fca1`/`840ccc2`/`16b1382` 落地后的**当前树状态**——目的不是复刻当时 diff，而是确认那批代码对现在的影响是否依旧构成问题）
- 状态：已收口
- 审查人：pi 主审（单遍完整核对；挂账文档记载的两轮集中 findings 修复批次本身未逐条复核，按「整体机制通过 + 当前态无 P0/P1」收口）

## 结论

七重点轴全部核对，**P0/P1/P2 缺陷为零**，两项 P3（文档漂移 + 理论 ABI 边界）已随本报告收口或登记。step 5 的地基确认踏实。

| 轴 | 结论 |
|---|---|
| 1. lifecycle 锁序契约落实 | 通过；一处单向嵌套不构成环（见 PLR-1） |
| 2. Drain 有界性逐臂核对 | 通过；budget=1 边界见 PLR-2 |
| 3. Start pin 事务完整性 | 通过 |
| 4. 发布序与观察面一致性 | 通过 |
| 5. 调度 gate 竞态闭合 | 通过 |
| 6. ABI 双侧对齐 | 通过 |
| 7. Staging/TerminationTodo/Terminated 分类收敛 | 无收敛机会，分类即必要 |

## 逐轴证据

### 1. 锁序契约

lifecycle.rs 锁内只操作 `LifecycleInner`（状态/终因/成员记录/active/building_ops），副作用经 `TerminationTodo` 延迟到锁外（`run_termination_todo` 执行 offer/IPI/REAPABLE 发布）。全部调用面核对：

- `sched.rs` 调度循环：`enter_running`/`on_requeue`/`clear_active` 均在 `pick` 的类锁释放后调用；
- `wait.rs`：`park_waiting` 在 lifecycle 锁内线性化且不触 WaitContext；`install` 失败路径的 offer 与 `finish` 的 cleanup/confirm_departure 全部锁外；
- `trap.rs`：`is_terminating` 原子快读，`request_termination` 只组装待办；
- `process.rs`：`drain` Complete 分支的 publish 序在 drain_gate（try_lock）内但 lifecycle 锁外驱动 control/Job 动作；
- `job.rs`：无 lifecycle 调用；`remove_member` 摘成员在 JobState 锁内、CLOSED 发布锁外。

唯一嵌套点：`ProcessControl::snapshot`（process.rs:186-205）持 control state 锁期间调 `lifecycle.snapshot()`。lifecycle 锁内从不出游（不存在 lifecycle 锁 → 任何对象锁的路径），该嵌套是单向进入，不可能成环——死锁风险不存在，属契约表述未覆盖合法单向嵌套（PLR-1）。

### 2. Drain 有界性

- `frame_pool::dealloc_bounded`：budget 步链扫描 + 完成插入计 1 步（每调用进展 ≥1）；游标恢复前 O(1) 邻接校验，他方归还致失效时从链头重启且本次仍受 budget 约束；
- `AddressSpace::drain`：Frames（tracker 出栈 + 登记 = 1 步）→ Tables（root/L1 双游标，每槽 1 步，L1 收尾 2 步预留）→ Root（512 槽验证/剥离）→ RootFree（leak_root 后独立有界归还，预算中断重入不触 tree）——预算是硬执行上界，中断点游标全部持久化；
- `HandleTable::take_next_bounded`：max_slots 硬限制，Pinned 跳过不消费，空槽同计费；
- `Process::drain_batch`：`scan_budget = budget - work - 1` 为 close 回调预留 1 步；close_entry 在表锁释放后执行；close fanout 固定上界（MailboxOwner ≤ 128 条 transit，其余 role 均为叶子）成立；
- 边界：budget=1 且 HandleTable 未扫尽时 `scan_budget=0` → take_next_bounded 立即 Progress → 返回 `{0, More}`（PLR-2）。现实调用面（rinlib `drain_to_completion` 固定 `PROCESS_DRAIN_MAX=256` 循环）不触及。

### 3. Start pin 事务

`pin_for_start` 单临界区原子验证（builder MANAGE + grants GRANT/子集/去重/不含 builder）后翻转 Pinned，对其他线程 get/close/extract/drain 不可见，失败零副作用；`commit_pinned_into` 向已预留容量追加（不可失败）；`unpin` 无损还原。`start_staged` 提交序：可失败步骤（含 pin）全部前置 → `begin_running` 线性化（失败则 unpin + 完整回滚）→ 线性化后仅剩容量已预留的不可失败消费（commit/moved/builder_entry.expect/staging_ready/commit_ready）。`builder_entry.expect` 论证成立（pin 集含 builder 恰一份，grants 不得含 builder 已在 pin_for_start 校验）。bootstrap 路径（proc.rs launch）的 enter_building_op/begin_running expect 由「boot 路径不可能终止」前提保证。

### 4. 发布序

Complete 分支固定序：`publish_dead`（锁内置 DeadSnapshot + REAPABLE→CLOSED，外部无 Dead+REAPABLE 混合视图）→ `mark_dead`（core 内部置 Dead）→ `remove_member`（摘除强持）。观察面：

- Query：dead 已冻结走 DeadSnapshot；未冻结时 `upgrade().expect("live control core must be held by its job")` 成立——remove_member 只发生在 publish_dead 之后；
- WaitMany：publish_reapable/publish_dead 的「锁内 take completer → 锁外 finish_offered」循环交付，无分配、信号原子翻转；
- Job 完成计数：remove_member 摘除即唯一真值，Job CLOSED/计数随 step 5 接入。

### 5. 调度 gate 竞态

- pick 惰性撤销：dispatch 前 `enter_running` 失败 → reap（不进用户态）；
- trap 入口吸收：任意 trap（IPI/量子/异常）`is_terminating` 即 Killed；kill 与出口的竞态由「先归一内核 satp + 全量 SFENCE.VMA → clear_active → 处置」序闭合；
- park 线性化：`park_waiting` 在 lifecycle 锁内 Running→Waiting；已 Terminating 则不发布等待直接 Abandoned（线程不回用户态）；「可被唤醒严格晚于离开一切 hart 引用」由 HartLocal 私有槽 + 调度循环 Park 分支发布的结构性质保证；
- enqueue 无条件入队不反向触碰 lifecycle 锁，成员记录唯一真值贯穿。

### 6. ABI 双侧

shared：判别值固定（State 0–3 / Reason 0–4 / DrainStatus 0–1 / FaultCode 0–8 repr(i64)）+ 编译期尺寸断言（32/48/40/16B）齐全。rinlib：`validate_snapshot` 校验 reserved、判别范围、组合规则（终态必带终因、活态必 None+0、Abandoned code=0、Fault code∈0..=8）；drain 校验 reserved/status/work_done ≤ min(max_work, MAX) 且 More+0 拒绝。内核侧全部满足（Dead 幂等分支返回 {0, Complete} 合法通过）。

### 7. 分类收敛

Staging 是 Start 线性化与入队之间的必要所有权间隙（恰一处转换 begin_running→staging_ready）；Exiting 是自杀路径（不回用户态）的收尾标记；TerminationTodo 不入公开 ABI；ProcessState 与成员记录分离是「外部观察 vs 内部容器真值」的必要设计。特殊分支没有堆积——不存在收敛重构机会。

## Findings

### PLR-1：锁序契约未覆盖合法单向嵌套（P3，文档漂移，已收口）

- 契约：`notes/impls/task.md`「锁序契约」+ lifecycle.rs 头注释——「任何对象锁…内不得反向获取 lifecycle」。
- 实现：`ProcessControl::snapshot` 持 control state 锁调 `lifecycle.snapshot()`（对象锁内进入 lifecycle）。
- 影响：无死锁可能（lifecycle 锁内从不出游，单向嵌套不成环）；但契约字面与实现不一致，后续审查者会重复发现此疑点。
- 处理：精确化 impls/task.md 与 lifecycle.rs 头注释的契约表述——反向（lifecycle 锁内出游玩对象锁）一律禁止；正向嵌套获取 lifecycle（如 shell 快照）因 lifecycle 不出游而安全。已随本报告修复。

### PLR-2：drain budget=1 时可返回 More+0（P3，观察项，登记不修）

- 契约：rinlib drain 校验「More + 0 work = 内核违约」。
- 实现：`drain_batch` 在 budget=1、HandleTable 未扫尽时 `scan_budget=0` → 零进展返回 More。
- 影响：合法 ABI 输入 `drain(c, 1)` 会被 rinlib 误报 InternalError；现实调用面（drain_to_completion 固定 256、init/libprocess 全部走它）不可达。
- 处理方向（触发条件：Drain 调用面扩展或 ABI 文档化时）：内核保证每批至少 1 进展（scan_budget 取 `(budget - work).saturating_sub(1)` 并在 budget=1 时仍允许一次单项扫描 + close 超预算），或在 ABI 注释明确 max_work 有效下界。现登记于本报告，不单独挂账。

## 收口动作

1. 本报告归档（review 收口即入 archived/）；
2. `todo-2026-08-27-process-lifecycle-review.md` 移入 `archived/`（挂账销账）；
3. COMPASS 活跃计划表与位置节同步（Review 完成，step 5 解锁）；
4. PLR-1 契约表述修正（impls/task.md + lifecycle.rs 头注释）。
