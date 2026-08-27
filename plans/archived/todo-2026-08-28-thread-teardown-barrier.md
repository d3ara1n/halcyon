# 多线程 teardown barrier（生命周期 step 7）计划

> 2026-08-28 实施收口：四项决策按推荐拍板（成员表离场即摘除、锁外惰性
> 游标、归一本批收敛到汇编出口、写回失败杀调用进程）；实施顺序 1–5
> 全部完成，另含 TunnelAttach invitation 移除的并发 close 重排（前提含
> 同进程并发变更，按判据一并修复）。验收：just check、host 单测、
> virt ×4、sifive_u 全绿；文档同步 impls/{task,call,execution-context}.md
> 与 KNOWN_ISSUES 条目消解均已完成。本文件留档为设计依据。

## 目标

在 ThreadSpawn 接入前，把进程终止屏障从「一进程一线程」的偶然形态泛化为多线程结构：

1. **线程成员表**：lifecycle 的成员记录从单值 `ThreadRecord` 泛化为按 Tid 寻址的有序成员表，成为多线程容器真值；
2. **等待取消集合化**：终止触达从「取消一个 WaitContext」泛化为「取消全部等待中的线程」，且零分配、无 OOM 错误面；
3. **离场确认泛化**：REAPABLE 合取改为「全部线程离场 + active 位图归零 + 无在途 Building 操作」；
4. **归一收敛**：非 Resume 出口的 kernel satp 归一从两处 Rust 调用点收敛到汇编出口边界（execution-context.md 既定触发点）；
5. **写回 panic 面消解**：IPC/生命周期对象层的用户写回 `expect` 改为优雅路径，消除 KNOWN_ISSUES「用户态多线程落地前的写回 panic 面」条目。

屏障公理不变：**active hart 必须先切回 kernel satp 并执行本地全量 SFENCE.VMA，然后才清除 active 位**；REAPABLE 只在位图归零后发布；不以「SBI 请求已发送」代替完成。本计划是既有最小正确版（决策 8）的结构泛化，不改变其正确性论证，只把「单线程」前提从论证中删除。

## 非目标

- **不实现 ThreadSpawn ABI**（入口/栈分配/TLS/join 语义）：服务化阶段另行设计；本计划只保证其接入时无需重构屏障结构。
- **不做多线程压力验证**：单线程负载下无法产生同进程多 hart 竞态，多线程压力随 ThreadSpawn 接入时的 step 9 验证矩阵执行；本计划验证目标是「泛化后现有全负载等价回归」。
- **不动 per-dispatch fence.i**：active 位图早已存在，代码代次优化按 impls/task.md 既有触发条件（开销实测可见）另行推进。
- **不动 WaitContext/期限表/对象订阅结构**：等待侧机制已随 IPC 对象层收口，本计划只消费其 offer 仲裁。

## 现状取证（单线程假设清单）

- `LifecycleInner.member: ThreadRecord` 是单值枚举（`Gone/Ready/Staging/Running{slot}/Waiting{context}/Exiting`），全部 lifecycle 方法隐式作用于「这一个线程」；
- `TerminationTodo.cancel_wait: Option<Weak<WaitContext>>` 只承载一个等待者；
- REAPABLE 合取为 `member == Gone && active == 0 && building_ops == 0`；
- `todo.ipi_slots` 由单个 `Running{slot}` 记录推导（语义上等于 active 位图快照，多线程下位图是精确目标集）；
- `mm::normalize_satp()` 分布于调度循环每轮入口与非 Resume 出口处置两处 Rust 调用点（execution-context.md「已知简化」：新终止来源各自记得补归一是记账式风险）；
- 用户写回：`copy_to_user` 本身已在**当次** space 锁内复检并返回 `Result`，但约 15 处调用点以 `.expect("validated ... must remain writable")` 把复检失败当编程错误——单线程下前提成立，多线程下同进程异 hart 线程经 `HandleClose → unmap_external` 可打破前提（KNOWN_ISSUES 既有条目）。

## 设计

### 成员表：Tid 寻址的有序 fallible Vec

```rust
struct MemberEntry { tid: Tid, state: ThreadState }
enum ThreadState { Ready, Staging, Running { slot: usize }, Waiting { context: Weak<WaitContext> }, Exiting }

struct LifecycleInner {
    reason: ProcessExitReason,
    code: i64,
    members: Vec<MemberEntry>,      // 按 tid 升序；二分定位
    next_tid: Tid,                  // 进程内单调不复用；主线程恒 0
    active: u64,                    // 语义不变：本进程线程所在 hart 的 slot 位图
    building_ops: usize,
}
```

- **`Gone` 态删除**：线程离场确认即从表内摘除条目；「无线程」由表空表达。`Gone` 作为驻留态没有消费者——枚举触达只关心未离场成员，REAPABLE 只关心表空。Building（未 Start）天然表空。
- **有序 Vec + try_reserve + 二分定位**：与 Job 成员表同一结构选型（决策 15 的理由全部继承：单批触达 O(log n + N)、插入 O(width) memmove 只在创建路径、alloc 失败可表达）。宽度使 memmove 可观测时换 fallible 有序树，同样结构私有可换。
- **容量在可失败段预留**：主线程容量 1 在 `Lifecycle::building()` 构造时 `try_reserve`（失败沿 `Process::new → ProcessCreate` 报 OutOfMemory），保证 `begin_running` 的提交区插入不可失败。ThreadSpawn 的第 2..N 个线程将在其 syscall 可失败段预留容量后线性化——预留语义与 Job/HandleTable/就绪队列的 reserve/commit 同族。
- **方法全部按 tid 寻址**：`enter_running(tid, slot)`、`on_requeue(tid, slot)`、`park_waiting(tid, &context)`、`thread_departed(tid)`；调用方（调度循环、park 发布、WaitContext finish、Start 收尾）都持有 `Thread`，取 `t.tid` 即可。tid 查找 miss 是编程错误（debug_assert）——单一归属保证转换只由容器持有方驱动。
- **Tid 进程内分配**（`Tid = u32`，shared 已有类型）：主线程 0。跨进程唯一性不是本表的需求（容器真值只在本进程内寻址）；若未来线程操作 ABI 需要全局可读身份（诊断/procfs），届时再在用户态以 (pid, tid) 组合表达，不反过来要求内核表换键。

### REAPABLE 与离场确认

```text
REAPABLE ≡ is_terminating && members.is_empty() && active == 0 && building_ops == 0
```

- `thread_departed(tid)`：摘除条目后重判合取，末离场者返回 true（调用方锁外发布）。多线程下同一逻辑自然泛化——每个线程离场都重判，最后一个返回 true。
- `request_termination` 冻结序不变：锁内冻结终因 + 组装 todo；`exiting_self` 分支将调用线程的条目转 `Exiting`（reap 收尾摘除）。
- **末线程自然离场冻结 Exited 的边**（ThreadSpawn 世界：线程退出 ABI 使最后一个线程离场时进程尚非 Terminating）是 ThreadSpawn 批的接入点——结构上就是在 `thread_departed` 摘除后加「表空 && 未终止 → 冻结 (Exited, code)」一个判定，本计划不预埋不可达代码，只在此处入档。

### 等待取消：锁外惰性游标

`TerminationTodo` 收缩为两个纯量字段：

```rust
struct TerminationTodo { ipi_slots: u64, reapable: bool }
```

等待取消不再随 todo 携带 `Weak` 集合，改为 `run_termination_todo` 的锁外游标循环：

```text
loop {
    lock lifecycle
    找到首个 state == Waiting 的条目，摘出其 Weak（条目移除）
    无 → unlock，结束
    unlock
    upgrade + offer(Abandoned)
    胜者（OfferResult::Complete）→ finish 负责线程消散、订阅清理与离场确认
    败者 → 自然完成方已负责收尾（单 outcome 仲裁，无双重处置）
}
```

- **零分配**：每次只持一个 `Weak`，无 Vec、无预留、无 OOM 错误面——kill 是管理关键路径，不应因帧耗尽而失败或部分完成。
- **对 churn 免疫**：冻结后 `park_waiting` 拒绝发布，Waiting 条目集合单调不增（自然完成方与游标竞争摘除，先摘先得），循环必然终止于表内无 Waiting。
- **竞态良定义**：游标摘除与 WaitContext 自然完成（finish → `thread_departed` 摘条目）都以 lifecycle 锁串行化；offer 的单 outcome 仲裁保证 Abandoned 与自然信号恰有一个赢家接管线程。
- **锁序**：循环体每次先释放全部锁再 offer/finish（finish 内部经对象锁、space 锁后**末尾**才进 lifecycle 做离场确认），游标重新加锁时无任何嵌套持有——与 Lock Ladder 现秩兼容，不新增锁、不新增同秩链段。
- **成本上界**：循环总工作量 = 等待线程数 × 每线程订阅数（≤ WAIT_MANY_MAX），是目标进程状态的函数、由持 MANAGE 的管理者在自身 syscall 上下文内支付——与「收束工作由管理者驱动」的分工一致；线程数洪水由 Job 域资源上限（F4 记账决策）在接入时约束，屏障本身不再加 batching 面。
- **IPI 目标集**：`todo.ipi_slots = 冻结临界区内的 inner.active 快照`。完备性：active 位在 dispatch 前置位、非 Resume 出口归一后才清除，位集合恰好覆盖「仍在用户态或 Resume 热路径循环中」的本进程 hart；冻结后 `enter_running` 拒绝，位只能清不能置。对已进内核 hart 的多余 IPI 良性（SSIP 只引发 Requeue/继续）。

### 归一收敛到汇编出口边界

把「非 Resume 出口先切 kernel satp + 本地全量 SFENCE.VMA」从调度循环的两处 Rust 调用点（每轮入口、出口处置）移入 `_ret_to_user` 汇编：outcome 编码非 Resume（Switch/Park/Killed）时，返回 Rust 前完成 `csrw satp, kernel_satp; sfence.vma`；Resume 路径零新增开销（一次出口编码比较）。

- 调度循环两处 `mm::normalize_satp()` 删除——循环体此后结构性只运行于内核页表下，「新终止来源各自记得归一」的记账式风险由出口边界一处承担；
- barrier 公理的证明因此变强：**任何**非 Resume 出口（含未来线程退出等新来源）都先于 Rust 可见地产出「内核表 + 本地全量冲刷」；
- `kernel_satp` 经既有静态值供汇编寻址（bootstrap 后恒定，无重入问题）；
- 该项是 execution-context.md「已知简化」写明的既定触发条件（新终止来源接入时顺势收敛），本批正是触发点。

### 写回 panic 面消解（KNOWN_ISSUES 条目）

机制事实：`copy_to_user`/`write_user_value` **已在写回当次持有的 space 锁内复检**并返回 `Result`——缺的只是调用点把复检失败当编程错误。修复是调用点语义，不是 uaccess 机制：

1. **统一交付辅助**：新增 `uaccess::deliver_output(thread, space, dst, value)`——复检失败时对调用进程 `request_termination(Fault, StoreAccess, exiting_self = true)` 并返回 Err；约 15 处 `.expect` 站点机械替换为该辅助。语义与「用户可触发 fault 杀进程」戒律同构：同进程线程在两次 space 锁之间拆掉自己的输出页，等价于一次 store access fault，由内核代为检出。
2. **分发出口终止检查**：`syscall::dispatch` 在 handler 返回后检查 `thread.process.lifecycle.is_terminating()`，是则 `Outcome::Killed`（线程不回用户态）。它同时覆盖三类路径：写回失败自杀（错误码写 frame 后被 Killed 覆盖，frame 随线程消散）；syscall 执行期间被异 hart kill 冻结（今天依赖 sret 边界 IPI 吸收的下一 trap 入口，检查使收束确定性提前一个 syscall 且不依赖 IPI 时序）；既有 `ProcessKill::TerminatedCaller` 特判（可顺势收敛为同一机制）。
3. **非用户竞态的 expect 保留**：与用户内存无关的结构断言（如 tunnel 邀请安装的预留 token 消费）前提不受多线程影响，保持 expect；逐点审查时以「前提是否含『同进程无并发映射变更』」为判据。
4. **等待交付路径维持现状**：`deliver_wait_result` 的 `put_user_indirect` 失败已优雅返回错误（线程醒来收 MemoryNotAccessible）。理由独立成立：syscall 输出契约是「成功即已交付」，不可交付则调用语义破裂须杀；等待交付是「尽力送达，失败告知」，已有错误通道。两条路径语义不同，不强行统一。
5. uaccess.rs 头注释「同进程无并发映射变更者」前提表述同步改写为复检 + 终止语义。

### ThreadSpawn 接入面清单（结构就位即可）

- 成员插入：可失败段预留容量 → 线性化点插入 `(tid, Staging)` → 入队后 `staging_ready(tid)`（复用 Start 的 Staging 模式）；
- 线程退出：`thread_departed(tid)` + 末线程冻结 Exited 边（上文入档）；
- 栈布局：栈区向下扩展（impls/task.md 布局既定），与本计划无耦合；
- 线程数上界：Job 域内资源上限的既有记账决策（F4）不变。

## 实施顺序

1. lifecycle 成员表落地（`ThreadRecord` 单值 → 成员表、Gone 删除、tid 寻址、`Lifecycle::building` 容量预留）；
2. TerminationTodo 收缩 + `run_termination_todo` 游标取消循环；调用点（sched.rs / wait.rs / process.rs）按 tid 改写；
3. 汇编出口归一收敛，删除调度循环两处 Rust 归一；
4. `deliver_output` 辅助 + 分发出口终止检查 + ~15 处 expect 站点替换 + uaccess 头注释修正；
5. 验证：`just check`、host 单测、virt 多核全负载 ×N、sifive_u（5s 窗口）全绿——等价回归；
6. 文档同步：impls/task.md（成员表、游标取消、归一边界）、execution-context.md（已知简化消解）、KNOWN_ISSUES 条目删除、COMPASS 收口。

每步独立可验证、可单独提交；1–2 是主体，3–4 相互独立。

## 完成标准

- 单线程全负载行为等价回归（virt/sifive_u/host 全绿，生命周期/Job/IPC 验收线不变）；
- lifecycle 无单线程假设残留：成员表、取消、REAPABLE 合取在结构上支持 N 线程，ThreadSpawn 接入面清单全部就位；
- kill 关键路径零分配；等待取消对并发自然完成良定义；
- 非 Resume 出口的归一由汇编出口边界唯一承担，调度循环不再调用 `normalize_satp`；
- 用户写回复检失败不再可能 panic 内核——统一走进程终止路径，KNOWN_ISSUES 条目消解。
