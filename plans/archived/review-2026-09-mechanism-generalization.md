# 批次 D-1：机制泛化改造 Review（代码轴）

## 2026-09-08 最终复核（D-1，通过并归档）

固定对象 `228b6a56dd75dc31ddb349556905634b1dc7c6ff`，范围 `8987f89..228b6a5`；OliveWillow 通过正式 send_to 回报定点只读复核结论：P2-D1-03 已闭合，同批暴露的输出终止锁序 P1 也已修复，无新 finding。旧 P1-D1-01/02 已在 `9ee2791` 复核关闭，本报告全部条目完成。

- `task/tunnel/selftest.rs:141`、`:207` 生产 Create/Attach 失败覆盖 Conflict、无效输出、完整 Prepare 后的 Building gate 拒绝；失败后 Invitation/PTE/write_views 及库存恢复。`:328` 的真实 Pool reservation/HeapPressure 覆盖额度与元数据 OOM，不伪造返回值。`:267` 固定输出初检→真实 Unmap→Prepare→复检 Fault→rollback 的完整交错。
- fixture 在 `:243` 起经 departure/deferred work、每批 `max_work=1` 的重复 ProcessDrain 收束，最终释放全部 Process/Connection/Endpoint/Invitation 后，Pool、frame 和 16 类 metadata 库存回到初始值。HeapPressure 释放后，已 claim 的 system heap 容量继续归 allocator，符合既有 ticket 生命周期，不是泄漏。
- `uaccess.rs:110` 只冻结终因并向 Thread 固定槽移交待办；`proc.rs:4554`、`:4606` 保证首次交付、锁外 take 和 Drop 无遗留，`trap.rs:140` 在所有 handler guard 释放后交付并强制 Killed。全部生产 deliver_output 调用点共用该机制；并发终止的首达者责任保持唯一。
- `test_hammer/src/main.rs:723` 三组各 8 轮确定顺序/并发竞争，检查 Invitation 消费、重复 close 的 StaleHandle 及双方 VA 复用；原 8 轮 close-vs-Unmap、16 轮 Endpoint 退出仍保留。启动期确定性失败与真实多 hart 用户负载各自提供对应证据。
- 最终 `artifacts/review-fixes/acceptance.log` 七面 lint、24 轮矩阵、stress 16/16、release/sifive_u/nofd 及启动注入全部通过，`acceptance.status=0`。此前中间失败日志只作修复取证，不作为最终结果。

以下保留首审、首次复核与实施记录；其中“开放/待复核”均为对应时点状态，不构成当前待办。


## P2-D1-03 补证与同批修复（待固定提交复核）

`task/tunnel/selftest.rs` 已加入隔离 Building fixture，生产 Create/Attach 直接覆盖 Conflict、无效输出、完整 Prepare 后缺失 Running 提交资格；真实表页额度耗尽与堆耗尽分别触发 QuotaExceeded/NoFrame/OutOfMemory。失败后检查 Invitation、PTE、write_views 与库存不变。输出页在初检后由真实 MemoryUnmap 撤销，随后 Tunnel Prepare/输出复检失败/显式 rollback；fixture 经同一 Fault/离场/一 work unit ProcessDrain 收束，最终比较完整 Pool/frame 与 16 类 metadata admission 库存。

`test_hammer::tunnel_close_attach` 覆盖 Attach 先完成、close 先完成和并发竞争共 24 轮，每轮检查 Invitation 成功消费/失败保留、重复 close 为 StaleHandle、两端 VA 可以重新映射。已存在的 8 轮 close-vs-Unmap 和 16 轮退出压力继续保留。

直接验证暴露的输出终止锁序缺陷已按下节结构修复；新 kernel 失败锚点和用户态 24 轮矩阵均被 acceptance 强制检查。完整 `THROTTLE=100 just acceptance` 退出 0，日志 `artifacts/review-fixes/acceptance.log`，debug/release 内核 ELF frame audit 均通过原上限。实现与补证完成，最终关闭等待固定提交复核。

## 2026-09-08 提交后复核（D-1，仍开放）

对象 `9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`；OliveWillow 独立只读审查，WiseHare 交叉核对，统筹者复查现有 guest 负载。正式结论取自两位 reviewer 的 send_to 回报，不采用此前 peek 摘要。

| Finding | 结论与证据 |
|---|---|
| P1-D1-01 / INSTALLING 悬挂 | 闭合。`os/kernel/src/task/wait.rs:481` rejected park 走 Abandoned→finish_installing→begin_finish→完成责任交接。 |
| P1-D1-02 / deadline 不注销 | 闭合。`wait.rs:282` TimeoutRegistration 在 outcome/finish 及注册竞态中取消 token；`sched.rs:307`、`:358` 进入 owner timer queue。timer_queue 的取消、owner、generation 与堆修复 host 测试通过。 |
| P2-D1-03 / 失败回滚与析构验证 | **保持 P2 开放**。旧 MappingLease Drop 回取锁路径已由显式 MemoryChange/rollback 替换；`tunnel.rs:397` abandon_mapping 在空间锁外取消 writes，REAPABLE close_detached 在 `:1156` 只逻辑关闭并交给 ProcessDrain。结构改善不足以替代下述直接失败证据。 |

### 实施中直接验证发现的输出终止锁序缺陷

新隔离 fixture 在完整 Tunnel Prepare 后撤销输出页，直接命中 `uaccess::deliver_output` 的失败分支。该分支持 AddressSpace 锁（rank 300）调用 `run_termination_todo`，后者获取 Thread/Process 终止 reservation 的 LEAF 锁（rank 150）；debug Lock Ladder 报错，证明该用户可触发失败路径存在内核 panic。日志：`artifacts/failed-acceptance-20260908-095956-55506.log`。该 P1 作为本条验证暴露的同批修复责任，不另建计划。

冻结的交付结构：`deliver_output` 在业务锁内只冻结 lifecycle 终因，并把唯一 TerminationTodo 移入当前 Thread 的固定 `output_termination` 槽（MEMORY_COMPLETION 秩）；syscall handler 返回、全部业务 guard 释放后，trap 尾段取走待办并执行 IPI/termination debt/通知，然后按生命周期返回 Killed。执行容器在交付前强持 Thread，Drop 断言无未交付待办；不在每个 syscall 上添加锁序补丁，也不引入堆分配。后续固定提交须同时复核此交付路径与原失败验证。

### P2-D1-03 唯一后续行动

- 现状与位置：`os/kernel/src/task/tunnel.rs` 的 Create/Attach、abandon_mapping、abandon_unmap、显式 close 和 close_detached 已消费统一事务。`user/tests/test_hammer/src/main.rs:683` 已有 8 轮 Endpoint close 并发普通 Unmap；`:359` 的 tunnel_exit_target 留存 live Endpoint，stress 以 16 轮退出验证 ProcessDrain 接管。不能写成“完全没有并发/owner 消散覆盖”。
- 仍缺：Conflict/NoFrame/OOM、输出写回失败、close-vs-Attach 的精确失败/交错证据，以及这些路径的 permit、backing、页表、Pool 守恒与锁阶检查。未证实独立泄漏或死锁，但原验证门尚未满足。
- 目标：在现有最终事务接口上形成直接可重复的失败证据，失败不消费 Invitation、不残留 view/PTE/permit；已提交路径只沿原完成责任收束。不得重建旧 MappingLease 或临时 adapter。
- 自然顺序：盘点注入点及已有测试→同一验证单元补齐失败/交错/守恒断言→运行 host 与对应 guest 组合→按新固定提交复核本条。无需等待多页 Tunnel/RNL2；后者目前依赖本轮 program，转交会形成循环。
- 完成与归档门：上述失败和交错证据齐全、`notes/impls/{mm,tunnel}` 如实描述、reviewer 对固定提交确认闭合后才归档。本报告是唯一行动真值，不新建 todo、不转交已归档实施计划、不以降级 P3 消除待办。

验证边界：主线全速 stress 16/16 已通过，仍不能证明精确故障注入；本报告保持根目录开放状态。

---

以下为原目标提交首审记录，其“当前”保留历史语境。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](todo-2026-09-review-program.md) 为准；正文建议保留首审语境，不作为现行实施顺序。

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

### P1-D1-01（历史 finding，已由后续主线修复）：Terminating 竞态下 INSTALLING WaitContext 永不 finish

位置：目标 `9c03251`，`os/kernel/src/task/wait.rs:302-317`；`WaitCore::offer` 的 INSTALLING 行为见 `os/wait_context/src/lib.rs:84-86`。

可达前提：调度循环在 `park_publish` 安装等待时，另一条 ProcessKill/终止路径先将生命周期置为 Terminating，使 `process.lifecycle.park_waiting(&context)` 返回 false。

直接证据：false 分支只调用 `context.offer(WaitOutcome::Abandoned)` 后返回；INSTALLING 阶段的 offer 返回 Deferred，未调用 `finish_installing` 或统一 `context.finish`。

后果：WaitContext 未完成，线程不再回到可运行队列，也不执行统一 Abandoned 收尾/离场确认；成员表和 REAPABLE 屏障可能永久卡住，进程无法完成收束。

违反契约：`notes/ideas/task.md` 的 Terminating→Dead 闭包、`notes/impls/task.md` 的单一 outcome 仲裁与线程消散、`wait.rs` 自身“Terminating 直接以 Abandoned 取消”的承诺。

建议：安装拒绝也必须完成 INSTALLING→FINISHING→DONE 交接，并进入统一 `context.finish`；补 park-vs-kill 竞态测试，断言 thread=Gone、REAPABLE 发布和 WaitContext DONE。

### P1-D1-02（历史 finding，已由后续主线修复）：提前完成的 deadline 条目不注销，累积无效 timer 并污染静默判定

位置：目标 `9c03251`，注册 `os/kernel/src/sched.rs:180-190`，完成 `os/kernel/src/task/wait.rs:237-255,355-367`，到期清理 `sched.rs:223-234`，静默谓词 `sched.rs:403-414`。

可达前提：有限期限的 WaitMany/Sleep 在 deadline 前因对象信号、错误或 Abandoned 完成。

直接证据：期限表条目只保存 deadline 和 context；完成路径没有 unregister/cancel。到期时才扫描条目并对已经 DONE 的 context 尝试 Deadline outcome，该 offer 只会 Lost。期限表为普通 Vec，无固定结构化容量。

后果：已完成 WaitContext 被强持至 deadline，反复提前完成可累积无效条目并增加扫描成本；在目标快照仍由 `is_quiescent` 读取期限表的语义下，stale entry 会保守地保持“存在唤醒主人”，造成 false negative：阻止本应已无唤醒主人的系统进入静默终态或延迟终态判定。

违反契约：`plans/archived/todo-2026-08-27-mechanism-generalization-review.md:32-37` 明确要求完成即注销并核对 owner slot/arena/generation 跨取消、到期和复用闭合；同时违反 timer 仅承担确定期限唤醒所有权、不得保留已完成请求的实现边界。

建议：引入稳定 TimeoutRegistration（owner slot、arena slot、generation/token），所有 Complete/Abandoned/Timeout 路径注销；设置结构化容量和明确 OOM 边界；补提前完成、Abandoned、到期和槽位复用测试。后续 `5fbd67b` 已引入 `TimeoutRegistration`/TimerQueue，但不能替代目标提交本身的缺口证明。

### P2-D1-03（当前仍为验证缺口）：MappingLease 失败回滚与 owner 消散仅有静态/间接证据，锁序依赖局部声明顺序

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

以下为首审建议与复核条件；两个历史 P1 已有后续修复，当前 lease 验证随内存事务计划的 AddressSpace/Tunnel 纵向迁移，不重建历史 MappingLease 类型：

1. 修复 INSTALLING WaitContext 的 Abandoned 完成闭包；
2. 将 deadline registration 的注销、固定容量和 generation/token 结构统一收口；
3. 将 MappingLease map/commit/rollback 改为显式事务 guard，并补内核压力证据；
4. 更新 `notes/impls/{task,mm,tunnel}.md`；
5. 修复后按本报告逐项复核，全部闭合后移入 `plans/archived/`。

## 当前 HEAD 状态复核

当前 HEAD `11fde56` 已接入 `TimeoutRegistration`、`finish_installing` 和 TimerQueue cancel，P1-D1-01/P1-D1-02 作为目标提交历史缺口保留，不再作为当前债务。P2-D1-03 仍是当前验证缺口，尚无独立泄漏/死锁证据。

## 最终判定

**目标批次首审不通过；当前 HEAD 仅保留 P2 验证/维护风险。** Lock Ladder 在目标范围内通过；per-hart Timeout 的两个 P1 已由后续机制收口；MappingLease 仍需失败注入与析构顺序验证。