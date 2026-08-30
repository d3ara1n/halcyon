# 多线程 teardown barrier Review 计划

> 【未来审查计划】核心对象是生命周期 step 7 的提交 `d741880`，并结合 ThreadSpawn 批二 `bdc83ef` 与压力收口 `004cae5` 复核屏障被真实多线程消费后的终态；Review 纪律见 [`REVIEW.md`](REVIEW.md)。设计决策见 [archived/todo-2026-08-28-thread-teardown-barrier.md](archived/todo-2026-08-28-thread-teardown-barrier.md) 与 [archived/todo-2026-09-thread-model.md](archived/todo-2026-09-thread-model.md)，实现现状见 `notes/impls/{task,call,execution-context}.md`。

## 提交对照

| 提交 | 内容 |
|---|---|
| `d741880` | feat(task)：step 7 主体——线程成员表（tid 寻址有序 fallible Vec、离场即摘、Gone 删除）、TerminationTodo 纯量化 + run_termination_todo 游标取消循环、IPI 目标 = 冻结时刻 active 位图快照（自杀排除本 hart）、trap 汇编非 Resume 出口归一（KERNEL_SATP sym 注入、调度循环两处 Rust 归一删除）、deliver_output 复检即杀 + 分发出口终止检查、15 处写回 expect 清零、TunnelAttach invitation 移除并发 close 重排、notes 三篇同步、KNOWN_ISSUES 写回条目消解 + release 不收束新条目 |
| `bdc83ef` | Running `Spawning→Ready`、ThreadDeparture、末线程终局、线程级 Map-result obligation 与 ThreadControl DONE 实际接入 |
| `004cae5` | spawn/kill、末线程 exit/kill、join/Drop、1024 成员容量恢复和多平台 16/16 压力收口；删除普通离场热路径统计但保留 Process 生命周期 guard |

## Review 轴（代码为主）

### 成员表不变量

- 单一归属与条目生命周期：Building 使用 Staging，Running spawn 提交窗使用 Spawning，执行路径在 Ready / Running / Waiting 间转换，终止所有者写 Exiting，`thread_departed` 最终摘除；审查每条转换的调用方确持线程容器或提交 reservation，无「无主写条目」。
- 唤醒过渡窗口的 stale Waiting 记录（自然完成后、再调度前）：确认 offer(Abandoned) 落败路径（单 outcome 仲裁）与 pick gate 吸收 → reap 摘除的完整链，以及该窗口内 is_reapable 不误真（条目未摘即不空表）。
- `enter_running` 的 position expect 与 `thread_departed` 的摘除 expect：论证「dispatched/departing thread must be a member」在多 hart 竞态下无例外（尤其 requeue 与游标取消交错时）。

### 游标取消循环

- `take_first_waiting` 摘取（转 Exiting）与自然完成方 finish 的竞争：双方都以 lifecycle 锁串行化，先摘先得；确认 offer 落败方不再触碰线程（finish_offered 只在 Complete 分支进入）。
- 循环体锁序：游标持锁仅做摘取，offer/finish 的对象锁、space 锁全部在无 lifecycle 持有下进行，finish 末尾的 thread_departed 重入 lifecycle 无嵌套——Lock Ladder 无新边。
- 冻结后 Waiting 单调不增论证：park_waiting 拒绝是唯一入口封口，无其他路径写入 Waiting 条目。

### 汇编出口归一

- `KERNEL_SATP` 的发布时序（mm init Release store 先于任何 hart 进入用户态）与 asm la/ld 消费的无竞态性；bootstrap 阶段（satp 未发布）不可能到达该出口的论证。
- Resume 路径仅增加一次出口编码比较；确认 `_ret_to_user` 的调度循环切换段（HL_USER_SATP）与 trap 出口归一段无交互遗漏（如 fatal 路径、S 态来源不走该出口）。
- 删除调度循环两处归一后，idle/wfi 与 pick 间无依赖用户 satp 的残留路径。

### 写回复检即杀

- deliver_output 的冻结语义：副作用已发生后杀进程（进程死亡清理兜底）与「成功即已交付」契约的一致性；等待交付路径（deliver_wait_result 返回错误）与 syscall 输出路径的语义分界是否如文档所陈独立成立。
- 分发出口终止检查只改写 Completed（Wait 交由 park_publish 终止分支）：确认无 Completed 之外的出口需要改写、无 Wait 意图泄漏（park_kind/park_arg 槽）。
- TunnelAttach 重排：remove 先于 side 翻转后，并发 close 胜者/败者两侧的 connection 状态机终态（Closed+对端通知 vs Alive(weak)）均良定义；写回失败杀进程路径上 Alive(dead-weak) 对端的可观察行为。

### 既有回归面

- 自杀路径（Exit/fault/kill-self）新增 run_termination_todo 执行：单线程下为空转，多线程下 IPI 排除本 hart 的正确性（本 hart bit 尚置位时）。
- `Lifecycle::building()` 预留主线程容量使 ProcessCreate 失败面 +1（OutOfMemory）：确认无调用方假设 building 构造不可失败。
- Running ThreadSpawn 与异 hart ProcessKill、末线程 ThreadExit 与 ProcessKill 的冻结竞争均有双方胜出记录；ThreadControl DONE、JoinHandle Drop 与结果 Acquire 在成员摘除后发布。
- 1024 并发成员命中硬上界，额外 spawn 返回 ReachLimit，join 后容量恢复且 tid 单调；common debug/release、hetero、nofd 均为 16/16，`sifive_u` 连续十轮。
- `sched::reap` 不再维护 switch/lifespan 统计；复核 `ThreadDeparture` 只 weak 引用 Process 时，局部强引用必须覆盖 `drop(Thread) → departure.request`，避免最后成员摘除后 core 提前消散。
