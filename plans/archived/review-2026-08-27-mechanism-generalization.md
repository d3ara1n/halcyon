# 机制层 Review：特例补丁与可泛化机制

> 对象：`notes/ideas/` 全部 17 篇 + `notes/impls/` 全部 8 篇，代码佐证来自 os/kernel、os/handle_table、shared。视角：机制本身合不合理——有没有为特例打的补丁、不同结构的类似机制能否用一个更泛化机制通吃。按 [`REVIEW.md`](../REVIEW.md) 设计 Review 轴执行；未闭合承接项收拢进 [`todo-2026-08-26-review-carryover.md`](../todo-2026-08-26-review-carryover.md)。

## 总评

主干机制的正交性是高水准的：Waiting 唯一入口、终态电平统一（Mailbox/Notification/Tunnel/Job/Process 全走 ObjectSignals + WaitMany）、affine role 一次性消费、域—类—执行点三层、唤醒所有权——彼此咬合、无旁路 authority。TRANSIT/GRANT 分离、Notification vs ObjectSignals 两层，这些「看似重复实则分离」的设计在 ideas 里有独立论证，不是合并对象。

问题集中在三类：**①正确性靠枚举/穷举/纪律维持而非结构保证的点**；**②同域两制（可泛化合并候选）**；**③impls 失同步**（代码演进后文档未修订——三篇 impls 对同一机制的描述互相矛盾）。

## 一、结构靠枚举维持的点（特例补丁的温床）

共同形态：正确性前提是「当前集合里没有 X」，集合增长时靠人记得重审。

### R1 close callback fanout 的固定上界是 role 集合枚举证明

- 证据：`impls/task.md`「close callback fanout 固定上界（约束）」——「唯一级联关闭者是 MailboxOwner（≤16×8=128 条 transit 条目）……**新增可 transit 的容器 role 时必须重审此证明**」。
- 代码佐证：`Process::drain_batch` 把单个 close 回调当 **1 个 work unit** 锁外同步执行（`os/kernel/src/task/proc.rs:1002`，`scan_budget = budget - work - 1`）；`close_owner` 是同步 `loop { pop_front ... close_transit }` 一次做完，无游标/预算（`os/kernel/src/task/mailbox.rs`）。预算只约束「跨对象 fanout」，不约束「单对象内部扇出」。
- 判断：当前上界可接受，但证明建立在「owner 不可 TRANSIT ⟹ 消息内无容器 ⟹ transit close 恒叶子」这条未成文的推导链上——它其实比枚举更强，缺的是把它写成显式公理（见 M2/M3）。

### R2 锁序合法嵌套靠文档穷举，无结构兜底

- 证据：`impls/task.md`「已知合法嵌套（穷举）」段；lifecycle 锁纪律靠「锁内不出游」+ TerminationTodo 人肉维护。
- 判断：协作式内核 + 单向锁序（Job 链锁 → lifecycle → 对象锁）是运行时断言的理想场景——每锁一个 rank，获取点断言单调，一次机制消灭整类穷举负担。全部发现里性价比最高的结构改进。测绘 rank 表的前置工作本身会暴露未成文的序（如 HandleTable 锁与链锁的相对位置只存在于嵌套穷举里）。

### R3 drain 阶段间的隐式顺序契约

- 证据：AddressSpace 阶段入口 `debug_assert!(external_mappings.is_empty())`（`proc.rs:706`）——外部映射「必须已由 Handle 阶段 close 回调清空」，是两阶段间的硬依赖，但只由 close 回调自觉 + debug_assert 兜底。
- 判断：与 R1 同根——close 回调承担了「清空外部映射」这个跨模块契约，却没有机制面（类型/签名/RAII）表达它。

### R4 静默谓词的等待源枚举（已自知，归类存档）

`internals.md`「停机语义」有纪律条款，代码注释（`sched.rs:391-397`）亦有；`is_quiescent` = 全 hart idle ∧ 就绪空 ∧ 期限表空，IPC 等待者刻意不阻止静默，语义自洽。不新报。

## 二、同域两制：可合并/泛化候选

### M1 容器收束两制：ProcessDrain vs close_owner（最实质）

- 现状：同一问题域「释放容器内容」存在两套机制——ProcessDrain 是完整有界状态机（`drain_gate` try_lock 仲裁 + `drain_cursor`/`FreeScan` 双游标 + 硬预算 + 阶段机 + REAPABLE gate，`proc.rs:702-1030`）；对象 close 是同步无界循环，只受容量常量约束。
- 机制衔接点：drain 把整个 close 回调当作 1 个 work unit 锁外执行，单个大队列 mailbox 的 close 在单批次内一次做完，不跨批次。
- 合并方向：把「有界收束」视为对象层的通用语义——close 升级为「提交终止 + 可选的有界 continuation」，让 drain 模式成为唯一收束机制。触发条件：mailbox 容量参数化、或第一个级联 close role 出现。收益：R1/R3 两条枚举证明一并消灭，「新增 role 必须重审」变成结构保证。
- 反向判断（克制）：两制是**按容量分层的正确形态**——mailbox 16 条、页表十万级，小容器同步 close 是短路径的正常预算，强行 drain 化引入「半关闭」中间态反而复杂化。缺的不是统一机制，是分类判据的成文（见 M3）。

### M2 「禁令变结构」的三个机会：RAII 化收束契约

- **MappingLease**：tunnel Endpoint 持有的 RAII 对象（Drop = `unmap_external`），替代「close 回调记得调用」的纪律；`proc.rs:706` 的 debug_assert 从兜底变成结构不变量（drain 到达时 lease 必已 Drop）。
- **栈 VA 双别名无类型防护**：`mm.md`「禁止经 phys_to_virt 触碰栈内存」是禁令不是结构保证，绕过即无防护。轻量收窄：StackVA newtype 或 debug 期校验 `phys_to_virt` 落点不在栈窗口。
- **role 类型公理**：「可 TRANSIT 的 role ⟹ close 是 O(1) 叶子」写进 role 定义（ideas/object.md 公理层 + `handle.rs` 注释层），R1 的 fanout 证明从「枚举当前 role」变成「类型规则推导」。

### M3 收束分层公理（统一判据，不统一机制）

把「按收束工作量分层：小容器同步 close（上界由 role 公理保证），大容器 REAPABLE + drain（上界由预算保证）」写成显式公理；判据：收束工作量超过单 syscall 正常预算的对象必须走 drain 模式。「新增 role 必须重审」变成「新增 role 按公理分类」。

### M4 reserve/commit/rollback 三处同构、无共享骨架

- 代码佐证：job.rs 成员/子表（`gate_reserve_member`/`commit_member`/`rollback_member`，`job.rs:276-361`）、handle_table 槽（`reserve`/`commit`/`rollback`，`os/handle_table/src/lib.rs:358-406`）、sched 就绪队列（`reserve_ready`/`commit_ready`/`rollback_ready`，`sched.rs:74-100`）——四要素逐点同构：占位条目对查找/枚举不可见、单调 token 凭据、commit/rollback 按 token find + `expect(...disappeared)`、容器锁内完成。job.rs 内部两表已局部共享，跨容器无泛型。
- 判断：**不建议现在抽 trait**（容器形态差异是本质的：有序表/槽表/FIFO，公共子集会弱化各自结构性质）。建议两步：①协议纪律（token 语义、占位可见性规则、expect 的结构性论证）成文为 impls 契约；②第四处出现时再评估共享骨架。startup_block 的线性撤销（逆序 unmap，`proc.rs:458`）是第四种形态，属合理差异。

### M5 KernelRequest 概念双名（正名即可，最便宜）

- 证据：`ideas/call.md` 与 `impls/call.md` 都以「KernelRequest（等待对象+期限+完成动作）」为记账单元，但全仓 grep 该名零代码命中。实际结构是 `WaitContext`（`registrations` + `WaitPlan.deadline` + `WaitAction`），写回 TrapFrame 在 `WaitContext::deliver`（`os/kernel/src/task/wait.rs:257-306`）。
- 判断：「文档描述一个不存在的结构」——两套词汇一个机制。建议 impls/call.md 写明对应关系，或 ideas 改用 WaitContext 术语；概念债不还，第二个异步 syscall 接入时会在两个名字间横跳。

### M6 raw CAS 自旋体两份（可选，收益小）

`shared/src/sync/spin.rs:5` SimpleLock（仅用户态堆用）vs `os/kernel/src/sync.rs` RawSpinlock——关中断语义差异是硬性的、切分合理，但 CAS 自旋核心 ~30 行近似重复。方向：shared 提供 raw 原语、kernel 叠关中断壳。优先级最低。

### 正面样板（无需行动，佐证体系健康）

`os/tar` 被 init 跨 workspace path 依赖复用（`user/systems/init/Cargo.toml:14`，全仓唯一 ustar 解析）；`os/elf` → libprocess 同构复用；WaitMany/Sleep 共用 WaitContext 与期限表；五种对象终态电平统一；spinlock 两套的中断语义切分有独立理由。

## 三、impls 失同步（代码已演进，文档未修订）

按 AGENTS.md「impls 随代码演进同步修订，过时即改或删」的纪律，这些是违约实例：

### D1 `impls/internals.md`「进程表」段落整段过时

描述 `ProcessTable 封装 OnceLock<Spinlock<BTreeMap<Pid, Arc<Process>>>>`——该结构已退役（全仓零命中；现状是 Job 成员表唯一强持根，`job.rs:69/334`），「全局状态分层」表格的示例「进程表」同样过时。

### D2 `impls/startup.md` 与现状矛盾

「进程表使用 PID 单调不复用的 Vec 容器；reservation marker 对查找与回收不可见」——与 `impls/task.md`「全局进程表已退役」直接矛盾（task.md 是现状）。

### D3 `impls/mm.md` borrowed payload 表述与代码不符

文档说 opaque payload 是「BootPackage reservation 持有的 borrowed backing……地址空间销毁只清 payload PTE」；代码事实是映入时即收编为 owned `FrameTracker` 进 `frames`，销毁时 Drop **归还帧池**（`proc.rs:551-556`）。`ideas/bootstrap.md` 的「页所有权在映入 init 时即移交」是对的。若按文档理解，帧要么泄漏要么双重归还。

### D4 流程缺口

三篇文档对同一机制的三个矛盾描述说明「过时即改或删」没有兑现成例行动作——收口 checklist 应加「改动机制的 impls 涉及面 grep」。

## 四、文档已标注的已知简化（核查触发条件是否成文）

| 项 | 位置 | 触发条件成文？ |
|---|---|---|
| marker 预留不在 SchedClass trait | task.md 演进点 / carryover F2 | ✅ D64 eligibility |
| TableTree root 借用模型 | mm.md | ✅ 扩展共享分区/新 teardown 路径 |
| normalize_satp 两处归一 | execution-context.md | ✅ 新终止来源接入 |
| Boot Reservations 未收敛 | mm.md | ✅ 新占用方 |
| 空槽线性扫描 | ipc.md | ✅ 真实大 Handle 负载 |
| deadline 相对毫秒/无取消 | ipc.md | ✅ 设计审查延期项 |
| fence.i 每次 dispatch | mm/task.md | ⚠️ 只写了目标态（代码代次），未写退役触发条件——建议补「性能实测可见或 D64/多核扩展时」 |
| ASID 恒 0 | task.md | ✅ remote call TLB shootdown 消费者 |
| 帧池非 Drain 路径无界 dealloc | KNOWN_ISSUES | ✅ 已跟踪 |

## 五、微点（低优先级）

- **ProcessControl「同一对象身份」表述**：`revive_control`（`proc.rs:966-994`，weak 单槽 + 铸造点重放电平）证明身份真值是 core，shell 是可再生电平视图。设计自洽（ideas/task.md 已把 shell 表述为「非拥有关系定位 core」），impls 表述可直接说破「core 为身份、shell 为视图」，消除「同一对象」与「可铸造新 shell」的表面张力。

## 建议行动排序

1. **修 impls 失同步四处**（D1–D3 + fence.i 触发条件，纯文档）；
2. **KernelRequest 正名**（M5，一段文字）；
3. **Lock Ladder 立项**（R2，debug 期锁序断言，消灭穷举）；
4. **收束分层公理 + role 公理成文**（M2/M3，消灭 R1/R3 的枚举证明）；
5. reserve/commit/rollback 协议成文（M4），第四处出现时再议共享骨架。
