> 已收口：Review 报告见 [`review-2026-08-27-process-lifecycle-code-review.md`](review-2026-08-27-process-lifecycle-code-review.md)（七轴全过，PLR-1 已修、PLR-2 登记）。

# 进程生命周期 step 2–4 统一代码 Review（挂账）

> 验收已通过（设计拍板入档 + 实现完成 + 测试全绿并已提交）；按
> [REVIEW.md](REVIEW.md) 纪律，本批次的代码 Review 回头补做。本文档是
> 唯一跟踪点；Review 完成后归档至 `archived/`。

## 基线

- 审查对象即提交（任务以提交记录；基于 `fa73856` 之上）：
  - `72a35e4` feat(shared)：进程生命周期 ABI 与纯逻辑 crate 有界原语——32B CreateResult（builder+control）、48B StartDescriptor、Snapshot/FaultCode/DrainResult、REAPABLE 位、Query/Kill/Drain 调用号；frame_pool `dealloc_bounded`、HandleTable Pinned/`take_next_bounded`、TableTree `leak_root`；
  - `5684599` feat(kernel)：生命周期状态机、显式 Kill/Query/Drain 与有界收束——`task/lifecycle.rs`、Job 成员表接管强持、调度 gate（pick 吸收/trap 入口/park 线性化）、Start pin 事务、DrainStage 硬预算、发布序外部真值先行；
  - `910fca1` feat(user)：init 最小监督闭环与生命周期 ABI 用户侧同步——rinlib 封装与判别校验、libprocess 失败路径收束、init 监督/kill 剧本；
  - `840ccc2` refactor(task)：退役全局进程表（table.rs 删除），新增 srv_target kill 靶子；
  - `16b1382` docs(task)：ideas 方向入档、impls 收束现状、ref 取证归档。
- 契约来源：
  - [`notes/ideas/task.md`](../notes/ideas/task.md)、[`notes/ideas/bootstrap.md`](../notes/ideas/bootstrap.md)、[`notes/ideas/signal.md`](../notes/ideas/signal.md)；
  - [`todo-2026-08-26-process-lifecycle.md`](todo-2026-08-26-process-lifecycle.md)「已确认的 Process ABI」节。
- 实现现状记录：[`notes/impls/task.md`](../notes/impls/task.md)（生命周期、锁序契约、fanout 证明、有界收束）。

## 已发生的审查轨迹（供复核者参考，不替代本次 Review）

1. 设计阶段：8 项结构决策 + S1/S2/S3 补充项由设计方逐条确认并入计划文档。
2. 过程中两轮集中 findings 已由实施方自行修复并以测试覆盖：
   - C1 Building 操作屏障 / C2 Ready·Waiting·Gone 时点 / C3 Start pin 事务 /
     C4 root 显式回收 / C5 硬预算上界 / H1 waiter 零分配 / H2 快照一致性 /
     H3 JobCreate 事务 / M1 init 失败收尾 / M2 rinlib 判别校验；
   - P1-1 JobCreate 发布序 / P1-2 三处预算预留 / P2-1 Drain 输出前置校验 /
     P2-2 close fanout 固定上界证明（成文约束）/ P2-3 非 Drain 帧归还登记 KNOWN_ISSUES。
3. 审查方对整体机制的定性结论：「整体机制通过」。上述修复批次本身未经独立复核。

## Review 重点建议

按 REVIEW.md 代码 Review 三轴（ideas 忠实 / impls 忠实 / 代码自身健康）：

1. `lifecycle.rs` 锁序契约（顶级锁纪律）在全部调用面的落实——是否存在锁内触碰对象锁、
   期限表锁、调度类锁或 space/uaccess 的路径；
2. Drain 有界性逐臂核对：`AddressSpace::drain` 各阶段预算预留（含 Frames 登记步、
   Tables 双步预留）、`frame_pool::dealloc_bounded` 游标失效重启语义、`RootFree` 重入、
   `drain_batch` 扫描预算为 close 预留的口径；
3. Start 事务：`pin_for_start / commit_pinned_into / unpin` 在多线程调用者视角下的完整性，
   builder/grants 是否存在任何 expect 可达失败；
4. 发布序外部真值先行（shell CLOSED → core Dead → Job 摘成员）在 Query、WaitMany、
   Job 完成计数各观察面的一致性；
5. 成员记录单一真值与调度 gate（pick 边界惰性撤销、trap 入口吸收、park 线性化）的竞态闭合；
6. ABI 双侧（shared ↔ rinlib/libprocess）判别值、reserved、组合规则的对齐；
7. 「机制别扭产生大量特殊分支」的设计反馈扫描：Staging/TerminationTodo/Terminated 分类
   是否可再收敛。

## 触发条件

任何触碰以下模块的新工作启动前完成本 Review：`task/lifecycle.rs`、`task/process.rs`、
`task/job.rs`、`sched.rs`、`AddressSpace` drain、HandleTable 事务原语。亦可并入
system-audit 分片顺手执行。发现 P0/P1 时按章程立即叫停修复；结果写入对应
`review-<日期>-<主题>.md` 后本文档归档。
