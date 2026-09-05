# 批次 D-1：机制泛化改造 Review（代码轴）

## 范围与基线

目标提交：`15c7811`（契约/命名归属）、`9c03251`（Lock Ladder、per-hart 期限表、MappingLease 等）、`95deea6`（release ladder `mark_tp_ready` 空桩）。当前工作树基线为 `61490ae`；代码证据来自目标提交快照或隔离读取，不以当前树后续实现替代历史事实。全程只读，未修改或提交代码。

本轮只审尚未被 midterm design review 吸收的三个代码轴：Lock Ladder、per-hart Timeout、MappingLease；公理层和文档自洽不重复。

## 执行命令与验证边界

执行过 `git status`、目标提交 `git show`/`git diff`、`git grep`，逐项读取 `sync.rs`、`sched.rs`、`wait.rs`、`lifecycle.rs`、`proc.rs`、`tunnel.rs`、`process.rs`、`object.rs`、`job.rs`、`rt.rs`、`main.rs`、`hart.rs`、`trap.rs` 及相关 notes，并核对目标提交全部锁构造点、RawSpinlock、期限表和 `unmap_external` 调用点。

未运行目标快照的 `just check`、release check、`just virt`、`just virt-release` 或 host tests；目标提交提交信息中的测试描述未作为本次独立复验。目标快照的相关内核 host 覆盖有限，主要只有 wait_context 纯逻辑测试。

## 三个代码轴结论

### Lock Ladder

目标快照的 rank 表集中于 `os/kernel/src/sync.rs:33-65`，可见 Spinlock 构造点均带 rank；RawSpinlock 只在同步封装内部使用，talc 通过 `RankedRawSpinlock` 注入。Job 链和 HandleTable 同秩链段均有对应 key 约束。bootstrap 的 `TP_READY` 在 formal entry 首行发布，secondary 启动晚于该发布。panic 路径经 RawWriter/park，不取 heap 或 console lock；release 空桩及 `95deea6` 的 `mark_tp_ready` 修复符合诊断降级意图。

**结论：目标范围内通过；release 不提供运行时锁阶断言属于诊断覆盖缺口，不是已证实独立 bug。**

### per-hart Timeout

`95deea6` 仍使用每 hart 一个 `Spinlock<Vec<DeadlineEntry>>`，登记、arm、到期扫描只访问当前 hart 表，证明 owner-hart 分片方向；但没有稳定 token、owner slot/arena/generation 或 cancel/unregister 机制，期限表没有结构化固定容量上界。

**结论：不通过，存在两个 P1。**

### MappingLease

`MappingLease` 使用 `Weak<Process> + AtomicUsize VA`，`release` 先 swap 清零保证幂等，再 upgrade owner 并在锁外进入 AddressSpace。正常 owner 消散后由地址空间整树销毁外部 PTE，Handle close/drain 路径的锁序方向成立。创建/attach 失败路径依赖局部变量逆声明析构顺序，map 后错误和跨 hart close/attach 缺少内核正面测试。

**结论：静态正常路径成立，但验证/维护边界不闭合，见 P2。**

## Findings

### P1-D1-01：Terminating 竞态下 INSTALLING WaitContext 永不 finish

位置：目标 `9c03251`，`os/kernel/src/task/wait.rs:302-317`；`WaitCore::offer` 的 INSTALLING 行为见 `os/wait_context/src/lib.rs:84-86`。

可达前提：调度循环在 `park_publish` 安装等待时，另一条 ProcessKill/终止路径先将生命周期置为 Terminating，使 `process.lifecycle.park_waiting(&context)` 返回 false。

直接证据：false 分支只调用 `context.offer(WaitOutcome::Abandoned)` 后返回；INSTALLING 阶段的 offer 返回 Deferred，未调用 `finish_installing` 或统一 `context.finish`。

后果：WaitContext 未完成，线程不再回到可运行队列，也不执行统一 Abandoned 收尾/离场确认；成员表和 REAPABLE 屏障可能永久卡住，进程无法完成收束。

违反契约：`notes/ideas/task.md` 的 Terminating→Dead 闭包、`notes/impls/task.md` 的单一 outcome 仲裁与线程消散、`wait.rs` 自身“Terminating 直接以 Abandoned 取消”的承诺。

建议：安装拒绝也必须完成 INSTALLING→FINISHING→DONE 交接，并进入统一 `context.finish`；补 park-vs-kill 竞态测试，断言 thread=Gone、REAPABLE 发布和 WaitContext DONE。

### P1-D1-02：提前完成的 deadline 条目不注销，累积无效 timer 并污染静默判定

位置：目标 `9c03251`，注册 `os/kernel/src/sched.rs:180-190`，完成 `os/kernel/src/task/wait.rs:237-255,355-367`，到期清理 `sched.rs:223-234`，静默谓词 `sched.rs:403-414`。

可达前提：有限期限的 WaitMany/Sleep 在 deadline 前因对象信号、错误或 Abandoned 完成。

直接证据：期限表条目只保存 deadline 和 context；完成路径没有 unregister/cancel。到期时才扫描条目并对已经 DONE 的 context 尝试 Deadline outcome，该 offer 只会 Lost。期限表为普通 Vec，无固定结构化容量。

后果：已完成 WaitContext 被强持至 deadline，反复提前完成可累积无效条目并增加扫描成本；在目标快照仍由 `is_quiescent` 读取期限表的语义下，stale entry 会保守地保持“存在唤醒主人”，造成 false negative：阻止本应已无唤醒主人的系统进入静默终态或延迟终态判定。

违反契约：`plans/archived/todo-2026-08-27-mechanism-generalization-review.md:32-37` 明确要求完成即注销并核对 owner slot/arena/generation 跨取消、到期和复用闭合；同时违反 timer 仅承担确定期限唤醒所有权、不得保留已完成请求的实现边界。

建议：引入稳定 TimeoutRegistration（owner slot、arena slot、generation/token），所有 Complete/Abandoned/Timeout 路径注销；设置结构化容量和明确 OOM 边界；补提前完成、Abandoned、到期和槽位复用测试。后续 `5fbd67b` 已引入 `TimeoutRegistration`/TimerQueue，但不能替代目标提交本身的缺口证明。

### P2-D1-03：MappingLease 失败回滚与 owner 消散仅有静态/间接证据，锁序依赖局部声明顺序

位置：目标 `9c03251`，`os/kernel/src/task/tunnel.rs:350-409,411-478`，`os/kernel/src/task/proc.rs:640-672,702-706`。

可达前提：map_external 遇到 Conflict/NoFrame/OOM、map 后写回失败，或持 Endpoint 的 Process 走非标准最后释放路径。

直接证据：目标快照没有内核 tunnel/MappingLease 测试；map 后的写回/commit 使用 expect；失败安全依赖 `space`/`connection` guard 按逆声明序先析构，类型未表达该约束。

后果：本项不是已证实泄漏或死锁，但未来调整局部变量顺序或失败步骤可能把 `MappingLease::Drop → owner.space.lock()` 带入既有 AddressSpace 锁，形成死锁；map 后失败和 owner 消散缺少直接验证。

违反契约：`notes/impls/mm.md` 外部映射关闭契约、`notes/ideas/object.md` 的同步 close/有界 drain 分层、Lock Ladder 锁内不出游原则。

建议：以显式事务 guard 表达 map/commit/rollback 和析构顺序，补 Conflict/NoFrame、owner 消散、重复 close、跨 hart close/attach 测试。与既有 A-C findings 去重，不新增多线程写回 panic 条目。

## 已证实不变量

- rank 数字集中于同步模块，目标快照可见锁构造点均有 rank；RawSpinlock 未发现模块外裸用。
- Job 链、HandleTable 链和 bootstrap formal-entry `TP_READY` 发布顺序成立。
- ladder panic 路径不取 heap/console lock；release 空桩行为符合诊断设计。
- per-hart deadline 登记/到期扫描按当前 hart owner 执行，但注销闭包不足。
- MappingLease 的 swap(0) 幂等和 owner upgrade 失败后的地址空间整树销毁方向成立；正常 drain 锁序成立。

## 与前批 findings 去重

- 不重复 A 的 WritePermit 泄漏、object owner 重复 retire、post-Commit retire 分配、EXECUTE capability；不重复 B 的 DT status、FramePool arithmetic、MemoryPool/SystemSupply 错误策略和 bootstrap post-commit 窗口。
- 不重复 C-1 的 supervisor authority、必选服务降级、稀疏 raw hartid、q-only DT、ThreadControl CLOSED 和无限监督等待。
- 不重复 C-2 的 RemoteCalls token identity、epoch overflow、UserStack cleanup；本报告的 Timeout stale owner 是期限表独立位置，MappingLease 仅补充其验证边界。

## 后续行动与复核条件

本报告在 findings 未闭合期间同时作为唯一行动计划，不另建重复 todo：

1. 修复 INSTALLING WaitContext 的 Abandoned 完成闭包；
2. 将 deadline registration 的注销、固定容量和 generation/token 结构统一收口；
3. 将 MappingLease map/commit/rollback 改为显式事务 guard，并补内核压力证据；
4. 更新 `notes/impls/{task,mm,tunnel}.md`；
5. 修复后按本报告逐项复核，全部闭合后移入 `plans/archived/`。

## 最终判定

**不通过。** Lock Ladder 在目标范围内通过；per-hart Timeout 存在两个独立 P1；MappingLease 还有一个 P2 验证/维护风险。