# 架构审计发现承接

> 状态：M2 已修复；本轮 A2/M3/M5 容量校准及提交后/退出预留审计已完成。正式 capacity 路线验证了 32 张 Bound ledger 的顺序与并发准入、第 33 张 `ReachLimit`、31 张各两笔 Map 的收束，以及 8 张常驻 Bound 与 Map/partial Unmap/backing/drain 混合运行。32 已确定为允许提交前分配失败的条件性准入上限，不承诺任意工作集的物理余量。其他审计发现保留在本记录，暂缓主动计量，随真实消费者、故障或明确的设计/审查需求择机处理；本记录不归档，也不占近期施工顺序。原始审视基线为 `master` 的 `f4a4d57`（当时另有执行闭包未提交改动），不能当作当前工作树状态。

2026-10-07范围校准：下方状态表与M2/A2/M3/M5的计数/容量证据属于原固定基线，不代表当前实现仍有MetadataSponsor、17类Counter或virt-capacity路线。对象链已替换为Pool双维实际成本付款；当前硬隔离边界由[预算计划](todo-2026-09-14-kernel-memory-budget.md#当前接力硬隔离完成门)拥有。其它未关闭发现仍保留原证据，复核前先核定对应机制是否存在；原发现不能自动转成新结构施工任务。旧固定Review范围由[总体入口](todo-2026-09-kernel-final-architecture-review.md#旧-review-范围归并)归并。
>
> 本计划只拥有下列发现的**判断与验证面**。发现涉及的机制改造归各自既有专题（见「归属映射」），不重复安排同一问题。

## 来源与证据边界

本清单来自一次自顶向下的架构审计：通读 `notes/ideas/*`、`notes/impls/*`、`plans/COMPASS.md`、`shared/erhino_shared/src/*` 的 ABI 定义，并定点阅读 `os/kernel/src/task/resources.rs`、`os/kernel/src/deferred_work.rs`、`os/kernel/src/sync.rs`、`user/frameworks/libsrv/{budget,wake,work_queue,runtime}.rs`、`user/frameworks/libfal/Cargo.toml` 与各 crate 依赖、`plans/archived/ref-2026-09-acceptance-timing-flake.md`。

第二轮补充了内存分配的定点通读：`notes/{ideas,impls}/mm.md` 全文、`os/memory_supply/src/lib.rs`、`os/frame_pool/src/lib.rs`、`os/funded_frame/src/lib.rs`、`os/memory_pool/src/lib.rs`、`os/kernel/src/{heap,frame,mm,rt}.rs`、`user/rinlib/src/rt.rs`（M 类由此而来）。

**未覆盖**：内核实现全量通读、`artifacts/` 下验证日志、`os/kernel/src/task/**` 的全部细节、`references/` 外部取证。因此 A1、D 两条可能是「文档未写但代码已有」；A2、C1、M2 只核对了常量取值与部分使用点，未核对全部使用点。**每条发现都标了「把握程度」，低把握的条目在动手前必须先补证据，不能据本文直接改代码。**

## 归属映射（避免与既有专题重复）

| 发现 | 本计划拥有的部分 | 归他处拥有的部分 |
|---|---|---|
| A1 退役债务不变量不可局部验证 | 债务批次数的观测面与断言要求 | 退休/请求结构收束已归档于 [`public-operation-ownership`](archived/todo-2026-09-14-public-operation-ownership.md) |
| A2 全局 metadata 配额不可归属 | 触发条件认定与常量依据审查 | 配额机制本身归 [`kernel-memory-budget`](todo-2026-09-14-kernel-memory-budget.md) |
| B1 同步 Caller 与事件循环并存 | 「阻塞必须显式」的类型要求 | typed PendingCall / Outbox 归 [`service-runtime-prerequisites`](archived/todo-2026-09-13-service-runtime-prerequisites.md) |
| C1–C3 容量/粒度/派生依据 | 全部（本计划唯一拥有） | — |
| M1 裸帧号穿越 affine 所有权边界 | 全部（本计划唯一拥有） | — |
| M2 metadata 耗尽后果是内核 panic | 事实核实（是否为真实 panic 面） | 配额与预算机制归 [`kernel-memory-budget`](todo-2026-09-14-kernel-memory-budget.md) |
| M3 region slot 全局不可归属配额 | 同 A2（同一问题的容量面） | 配额机制本身归 `kernel-memory-budget` |
| M4 固定 64 extents 限制碎片容忍度 | 全部（本计划唯一拥有） | — |
| M5 内核堆压力观测与扩展路径 | 条件性准入校准结果与未来触发条件 | 预算归属归 `kernel-memory-budget` |
| D 协作式延迟无观测面 | 「内核路径恒短是待测断言」的文档定性 | fence.i 优化的计时前提见 [task 实现](../notes/impls/task.md)；内核驻留时间观测属本项待核需求 |

## A 类：设计层实质问题

### A1 退役债务不变量不可局部验证，其失败信号已被归类为环境噪声

**现状与位置**：`notes/ideas/mm.md`「MemoryChange 事务」末段要求「预算必须覆盖完整调用链，包括查找、容器移动、最后引用析构和通知传播；不能把一次许可归还可能引出的任意宽度通知隐藏为常数工作」。实现面为 `os/kernel/src/deferred_work.rs`（四组 `WorkDebts` + 四个并行 `PENDING` 数组 + 每安全点 16 步 / 每债务 4 步）与 `os/kernel/src/task/retirement.rs`、`task/notify_work.rs`。

**判断**：该不变量约束的是「任何未来新增的字段或通知都不得引入无界宽度」，是全局属性，无法由类型系统、锁序断言或 contract test 局部证明。每次给对象加一个 Drop 副作用都可能破坏它。更关键的是它的失败信号恰好是压力测试超时——`plans/archived/ref-2026-09-acceptance-timing-flake.md` 记录的正是该形状（300 秒墙钟在 Tunnel 矩阵后截断，无 panic、无稳定依赖环），结论为「墙钟敏感的偶发验收现象」。该归档在证据上成立，但同时意味着这条核心不变量当前唯一的检测手段被判为不可靠，且内核没有 per-operation 的债务批次数观测，只有总耗时。因此该不变量事实上处于「声明存在、无人守卫」状态。

**待办**：为每个对象的 close/retire 路径提供**确定性计数**（批次次数与每批步数）并设可断言上界，使违规表现为计数越界而不是墙钟超时。完成标准是存在一个可在 host 或 fixture 中直接断言的观测面，且覆盖 Pool 归还、对象 view 退役、通知传播三条路径。

**触发条件**：下一次出现「同一 round 重复无进展」「attempts/waits 持续增长而轮次不前进」，或任何 close 路径新增 fanout。

**验证**：`ref-2026-09-acceptance-timing-flake.md`「未来重开条件」列出的判定项；本项不重开该归档，只在触发时以本条目为承接入口。

**把握程度**：中。文档层面证据充分，但未核对 `deferred_work.rs` 是否已存在未成文的计数观测。

### A2 全局 metadata 配额不可归属，与「纯 capability 授权」承诺冲突

**现状与位置**：`os/kernel/src/task/resources.rs` 顶部常量。`REGION_SLOTS_PER_ADDRESS_SPACE = 4_096`、`MAX_ADMITTED_BOUND_ADDRESS_SPACES = 32`、`REGION_SLOT_GLOBAL_LIMIT = 两者相乘`、`ADDRESS_SPACE_GLOBAL_LIMIT = 4_096`、`PROCESS_GLOBAL_LIMIT = 4_096`。每笔成功 Map 至少产生一个 Region 与一个 `ReservationGroup`，切分还会铸造新 RegionKey；但全局 region slot 在 Bind 时按整张账本预留，而非随每次 Map 单独扣减。其容量对应 32 张满密度账本的准入，不是已证明的堆驻留能力。

**核实后的判断**：`notes/ideas/mm.md` 已明确区分有界准入与跨域 DoS 隔离。当前 per-sponsor 限额挂在全局 admission 上，不能转授或按域收紧；持 `CREATE` 的 JobControl 可以创建自己的 sponsor 并与其它创建者竞争全局容量，但只持 JobControl 不足以创建。`srv_init::DELEGATED_DOMAIN_RIGHTS` 授给 `srv_pm` 的 `pm_domain` control 只有 MANAGE|READ|WAIT，明确没有 CREATE；`srv_pm` 当前没有 ProcessCreate 路径。实际 ProcessCreate 调用集中在可信 `srv_init` 的启动与验收脚本，因此「现有 pm_domain 已是受托创建域」的原判断不成立。普通 Running 进程仍可在自己的 sponsor 内创建被授权的对象/映射，不能把 JobControl 的 CREATE 限制当成未来跨域预算隔离。32 是共享全局 region 与 planner transaction 容量的政策因子，不是从 heap 字节推导的容量证明；其来源尚未找到。

**当前触发结论**：尚无证据表明已开放不可信创建域或需要跨域保证额度的真实消费者，KernelMemoryBudget 的硬触发条件目前未满足；`pm_domain` 是受托管理域，不是受托创建域。未来给域外进程授予 `CREATE`，或承诺一域不能耗尽另一域的 metadata 容量之前，必须完成独立预算，不得以当前全局政策代替。该结论同步由 [`kernel-memory-budget`](todo-2026-09-14-kernel-memory-budget.md) 承接。

**已测下界与校准结论**：目标 RV64 的 Region/transaction 条目为 96/64 字节，单张账本预留至少 393472 字节；32 张至少 12591104 字节，约为 16 MiB 物理 heap 储备的四分之三，尚未计入壳、其它对象和 allocator 开销。本轮真实顺序/并发 Bind、混合负载、准入峰值与失败出口见「本轮容量校准施工记录」。32 仅限制可同时准入的完整 ledger 数量；实际分配仍在提交前 fallible，不由该计数保证 heap 字节或未来连续空间。没有需要重新设定数值的已知故障或跨域硬保证需求。

**本轮结论**：授权触发条件已按当前授权图判断，条件性准入的负载校准已完成；不得把 32 写成系统对任意工作集保证同时驻留的数量。未来若需硬容量或跨域隔离承诺，先明确消费者与最坏工作集，再在对应计划中设计预留或独立预算。

**把握程度**：授权、Bind 路径和两种条目的 RV64 布局高；全对象成本与 32 的安全余量尚无测量。


## B 类：当前阶段的实质矛盾

### B1 同步阻塞 `Caller` 与事件循环执行模型并存，且类型上不可区分

**现状与位置**：`user/libraries/librpc/src/caller.rs` 的 `Caller` 为同步阻塞、线程私有 ReplyPort、一次一个 outstanding；`notes/impls/rpc.md` 自承「`send_blocking` 在 MailboxFull 时无限等待，因此它还不是完整调用 deadline」。同时 `notes/ideas/framework.md` 要求「下游 RPC、发送背压和设备完成作为挂起状态，不在控制循环中阻塞」，`user/libraries/libexecution/src/runtime.rs` 是异步单 actor 有界推进。

**判断**：两套执行模型都是**正式形态**而非标注的过渡。在 `no_std` 服务里一次 `Caller::call()` 会直接停住 actor 线程，而类型上看不出来。这不需要谁犯错，只需有人自然地写 `let r = caller.call(...)`。

**待办**：让 `Caller` 的等待形态返回 `PendingCall`，「阻塞」成为显式适配器（如 `block_on`），使在事件循环里误用它在类型上显得别扭。完成标准是事件循环路径上不存在可隐式阻塞的调用形态。

**边界**：typed `PendingCall` 与完整投递阶段（Unsent/Sent/Completed）归执行前置，本项只拥有「阻塞必须显式」这一类型要求。

**把握程度**：高（`notes/impls/rpc.md` 已自承该缺口）。



## C 类：容量与依据（可能过度设计，需重新论证而非直接改）

### C1 `PROCESS_MAIN_STACK_SIZE` / `PROCESS_USER_TOP` 作为 ABI 常量

**现状与位置**：`shared/erhino_shared/src/proc.rs`：`PROCESS_MAIN_STACK_SIZE = 8 << 20`、`PROCESS_USER_TOP = 1 << 38`、`PROCESS_PAGE_SIZE = 4096`。

**判断**：8 MiB 固定主栈对 no_std 服务是巨量（实际用量 KiB 级），对 drv 类负载可能不够，而它是**两侧同步改**的 ABI 值。按 `AGENTS.md`「工程限额必须有依据」，8 MiB 目前无硬件或 ABI 来源。它更应是 libprocess 的默认策略参数，由 init manifest 按服务覆盖。

**待办**：论证该常量的依据来源；若无独立依据，迁移为 libprocess 策略默认值 + manifest 覆盖，ABI 只保留页大小等真实硬件约束。

**把握程度**：中（未核对全部使用点与是否有服务依赖该精确值）。

### C2 metadata admission 的 20 类粒度

**现状与位置**：`os/kernel/src/task/resources.rs` 的 16 个全局 `Counter` + `IpcClass::COUNT = 4`，`ADMISSION_CLASSES = 16 + IpcClass::COUNT`；`deferred_work.rs` 另有 4 个并行 `PENDING` 数组。

**判断**：细粒度的唯一收益是失败归因更清楚，但很可能大部分类的实际使用量长期是个位数，而 20 类数组与 4 个并行 Pending 数组是持续维护面。不算错（可观测性是真收益），若要精简这是第一候选。

**待办**：先采集各类实际使用分布，再决定合并或保留。无数据前不改。

**把握程度**：低（未采集实际使用分布，可能是纯推测）。

### C3 锁断言栈用 hart 数封顶而非秩数派生

**现状与位置**：`os/kernel/src/sync.rs` 的 `frames: [Frame; hart::HART_NUM_LIMIT]`（`HART_NUM_LIMIT = 8`）；锁嵌套深度的语义上界是秩表中的秩数。

**判断**：当前秩数小于 8 所以巧合够用，但秩数增长时会先撞上该常量并产生误导性 panic。应由秩数派生。

**待办**：改为按秩数派生容量，或在两处之间加编译期互校断言。

**把握程度**：中（未核对当前秩数总数与是否有互校断言）。

## M 类：内存分配机制（第二轮定点审计）

内存机制整体判断：三层物理资源分离（planner / FramePool / talc）、外置 metadata、单一事务核、affine 所有权贯穿、失败原子性统一形式——**明显高于同类项目水准，不该整体改动**。以下 M1–M5 是其中的具体薄弱点，M2 是与 A2 同源的严重项。

### M1 `ClaimedUserExtent` 的物理所有权靠约定维持，有一处穿越裸帧号的窄缝

**现状与位置**：`os/frame_pool` 的 `alloc_order`/`alloc_largest`/`alloc_at` 返回裸 `FrameNumber`（`os/frame_pool/src/lib.rs`）；`os/kernel/src/frame.rs` 的 `UserInventory::claim_largest` 拿到后用 `ExtentGeometry::new(base, count)` 重新包装为 `ClaimedUserExtent`。而 `ExtentGeometry` 自身声明「可复制但只表达几何，不能复制物理所有权」。

**判断**：从库存 crate 取裸帧号再重新构造 affine owner，是把物理所有权穿过了一个**可复制的窄缝**。`notes/impls/mm.md` 自承该不变量无法单独证明：「切分后存活侧读写只验证访问范围，直映射不会随库存归还失效，因此不能单独证明独占所有权」。影响面是任何一处把 `FrameNumber` 暂存后再包装的代码都会产生可复制的物理所有权假象。这是当前内核唯一一处所有权**类型边界**薄弱点。

**待办**：让 `frame_pool` 的分配原语直接返回 affine claim token（`!Copy`），内核 adapter 只做 `Claim → ClaimedUserExtent` 的所有权转换，不再经裸帧号重建。完成标准是 `ExtentGeometry` 不再被用于从裸值构造所有权 owner。

**把握程度**：高（文档自承 + 代码形态已确认）。

### M2 metadata 耗尽的后果是内核 panic，而设计文档写的是「普通 metadata OOM」

**现状与位置**：`os/kernel/src/rt.rs` 的 `#[alloc_error_handler] handle_alloc_error` 实现为 `panic!("heap allocation error, layout = {:?}", layout)`。而 `os/kernel/src/heap.rs` 注释与 `notes/impls/mm.md` 均声明「ticket 用尽后普通 metadata 分配明确 OOM」。`notes/ideas/kernel.md` 戒律要求「用户可触发的 fault 一律杀进程绝不 panic 内核」。内核堆容量为 16 × 1 MiB heap tickets（`HEAP_CHUNK_LIMIT = 16`，`os/kernel/src/frame.rs`），不支持归还，`recovery = 0`。

**判断**：两条规则冲突。若存在用户可达路径能把内核 metadata 吃光（结合 M3 的全局不可归属配额与 20 类 admission 的累计占用，这类路径不必恶意即可构成），则「合法 syscall → 内核 panic」成立。M2 与 M3 是同一问题的两面：**不可归属的全局配额 + 耗尽即 panic = 可被合法调用触发的内核死亡**。

**核实结果（2026-09-24）**：该风险在当前代码中真实存在，不是纯文档冲突。已确认三条用户可达路径：
- `os/kernel/src/syscall.rs` 的 `Debug` syscall 原先用 `alloc::vec![0; len]` 和 `alloc::format!`；
- `os/kernel/src/task/proc.rs` 的 `Process::new` 原先构造 `ProcessHandleTable::new()`，其 `os/handle_table` 实现用 `vec![Slot { ... }]` 创建首槽。
- `os/kernel/src/task/process.rs` 的 `ProcessGrant` 原先用 `map(...).collect()` 隐式分配 grant pairs。
三处在 heap ticket 用尽时都会进入 `#[alloc_error_handler]` 的 panic，而不是返回 `SystemCallError::OutOfMemory`。消息、Wait、Map、线程、句柄、对象和 Tunnel 的已达分配点均已核对，使用 `try_reserve`/`try_reserve_exact`/`Arc::try_new` 或事务错误映射；`OrderedTable::new` 与 `VecDeque::new` 只建立空状态，不发生堆分配。
**修复与验证**：Debug 缓冲改为 `try_reserve_exact` 并移除动态 pid tag；HandleTable 首槽改为 fallible `try_new`/`try_with_limit`，Process 构造将失败映射为 `SpaceError::NoFrame`；ProcessGrant 的 grant pairs 改为 `try_reserve_exact` + `push`；测试 fixture 同步迁移。`handle_table` host 18 项测试、`just check` 和 `just clippy` 均通过，当前 M2 的三处 panic 面已闭合。M3 的全局 quota 依据和 M5 的高水位观测仍不因 M2 修复自动关闭。

**把握程度**：高——三处用户可达不可失败分配已由代码定位、修复，并由 host/target 检查验证；其余已扫描分配点均有 fallible 预留或位于启动期/自检期。

### M3 region slot 的全局不可归属配额（A2 的容量面）

**现状与位置**：`os/kernel/src/task/resources.rs`：`REGION_SLOTS_PER_ADDRESS_SPACE = 4_096`、`MAX_ADMITTED_BOUND_ADDRESS_SPACES = 32`、`REGION_SLOT_GLOBAL_LIMIT = 二者相乘`，planner transaction 全局数也同乘 32。`reserve_address_space` 在 Bind 前一次占用完整的 4096+4 槽，不是每笔 Map 各占一个全局 region slot；`os/memory_space/src/space.rs::MemorySpace::new` 在构造时以 `try_reserve_exact` 预留真实 4096 个 `Region` 和 4 个 transaction 的堆容量，失败在发布前返回 `NoFrame`/`OutOfMemory`。Region slot 的全局计数表示可接纳的整张 ledger 的上限，不等于实际 live fragment 数，也不保证相应 heap 字节已经可用。

**判断**：4096 是当前每地址空间固定 ledger 容量，同时作为本地 admission 上限；32 是全局最多准入 32 张完整 ledger 的条件性政策。RV64 已知 ledger 元素的最低预留为 12591104 字节（32 张），未计其它成本；本轮顺序、并发和混合验收给出了特定 QEMU/virt 负载的成功证据，而不是从其余 bytes 与系统储备推导出的硬容量保证。即使只有一个 live fragment，Bind 也消耗一张 ledger 的全局 slots 和对应 Vec 容量；提交前的实际 heap 分配仍可失败。

**后续条件**：已与 A2 合并完成本轮条件性准入校准。只有未来需要指定工作集的物理容量保证、出现真实分配/退休故障，或需跨域隔离时，才分别论证连续空间预留、完成余量与独立 KernelMemoryBudget；预算隔离不能由 32 这个计数代替。

**把握程度**：高（常量取值与推导链已核对）。

### M4 固定 64 extents 的推导来源是栈预算，不是内存语义

**现状与位置**：`os/kernel/src/frame.rs`：`pub(crate) const MAX_FUNDED_EXTENTS: usize = 64`；`funded_frame::fund` 以它为单次资金化事务的 extent 上限，超限返回 `FundError::ExtentLimit`。`task/proc.rs` 另设 `MAX_BACKING_SPLITS_PER_CHANGE = MAX_FUNDED_EXTENTS * 4`。

**判断**：一个 512 页（2 MiB）的 backing 在库存碎片化后可能超过 64 个 extent，此时**尽管 Pool 额度与物理页都充足**仍失败。`notes/impls/mm.md` 承认当前依托「四页单 extent 是当前两平台启动供给的 fixture 前置」；长时间起停服务后该前提不成立。更重要的是推导方向：64 的来源是「debug 帧成本由栈 guard 与 ELF audit 共同约束」，即**由栈预算倒推**，而非由内存语义推导——与 `AGENTS.md`「栈帧审计不能反过来扭曲通用机制」相悖。且 `notes/impls/memory-object.md` 已为常驻 `ObjectBacking` 采用堆化多 extent 列表并明确「定长容器只适合一次性事务结果」——那么一次性事务为何仍内联 64 槽，这个不一致需要回答。

**待办**：重审 64 的依据；或在一次性事务结果同样采用堆化存储（`alloc` 可用），使 extent 数上限由内存语义而非栈预算决定。**正确性不受影响**（失败发生在 Commit 前且可恢复），本条是可用性与依据问题。

**把握程度**：中（常量与推导来源已核对，未核实碎片化实测数据）。

### M5 内核堆压力观测与按需分级拒绝

**现状与位置**：`os/kernel/src/frame.rs` 的 `HEAP_CHUNK_LIMIT = 16`（每 chunk 1 MiB）、`RECOVERY_TICKET_LIMIT = 0`；`os/kernel/src/heap.rs` 的 `SystemSource` 只做 O(1) ticket pop，ticket 一经消费永久归堆且不支持归还。本轮已建立 Counter 成功 acquire 的高水位，以及 heap 在 trap/Bind/Reset 边界的采样极值和 source 失败布局；后者不是 allocator 内每笔分配的瞬时峰值，也不报告最大连续空闲块。

**核实后的判断**：M2 已证实并修复三处用户可达的不可失败分配；不能再以「是否真的触发 panic 待核实」作为 M5 前置。`admission_usage()` 统计 20 类 slot 的当前用量，Counter 另有成功 acquire 的峰值；talc 的 `counters` 在 Ready 和授权 reset 时输出 claimed/allocated/available/fragment 与已消费的 system ticket 数，并在 trap/Bind/Reset 累积采样极值。首轮 virt/core 的 Ready/Reset 分别为 allocated 404768/420152、available 16368224/16351680 字节；virt/stress 的 Reset 为 allocated 487288、available 16284544 字节。core Ready 与两条路线的 Reset 均已消费 16 个 ticket、claim 16776856 字节，证明「剩余 ticket 为零」不等于 allocator 已耗尽。后续运行期结果见下方容量校准记录；边界采样仍无法证明没有 syscall 内短时分配峰值或碎片导致的大块分配失败。

**本轮结论与后续条件**：已测正式顺序/并发 32 张 Bound、33rd `ReachLimit`、混合 Map/Unmap/backing/drain 及完整收束；现阶段保持 fallible 创建与已预付完成路径，不增设无依据的拒绝阈值。若未来真实 OOM、退休失败、需保证指定工作集容量或跨域隔离，再明确最大布局与完成余量，决定是否增加 allocator-native 观测、连续空间预留或预算机制；不能用剩余 ticket 数或采样极值替代保证。

**把握程度**：高（Counter 高水位、heap 边界采样与 QEMU 场景有代码及运行证据）；allocator 内瞬时峰值、最大连续块和任意工作集保证未经测量，也不属于当前条件性准入承诺。

## D 类：协作式定性的诚实质疑（文档定性，不立案实施）

**现状与位置**：`notes/ideas/kernel.md`「协作式内核」以「若某项需求看起来必须依赖内核抢占、内核睡眠或后台内核线程，首先应重新判断工作归属」论证协作式可接受性。

**判断**：该论证在方向上成立（长工作放用户态），但作为论证不可证伪——任何反例都会被重新解释为「归属放错了」。真正承重的命题是同一篇「有界路径」中的「用户可触发的单次内核调用必须有结构性工作上界」。「结构性有界」（无无界循环）确实做到了，但协作式可接受性依赖的是**延迟有界**，而系统目前没有内核驻留时间的观测面；fence.i 优化本身尚缺 dispatch 计时手段（Zicntr），更无法据此推断最坏 trap 延迟。结论是「内核路径恒短」目前是**设计意图而非已验证性质**。

**待办**：把该定性在 `notes/ideas/kernel.md` 中显式降级为待测断言，并把「内核驻留时间观测面」登记为实时负载（ECS/机器人方向）出现前的显式前置。**本项不实施任何机制，只修正文档的确定性表述。**

**把握程度**：中（判断基于文档自承的测量缺口，未实测）。

## 明确不动的部分

以下判断在本次审计中**确认成立**，未来 Review 不因本清单重开：

- **TRANSIT / GRANT 按存储拓扑而非权限区分**——正确的抽象维度。
- **Commit 前全预留 / Commit 后不回滚**——单一事务形状贯穿 Pool / Process / Memory / Tunnel，可审查性极高。
- **role 不可由 rights 伪造**——一次性消灭「从权限位反推对象关系」的一类漏洞。
- **`UserWriteLease` + release cookie 的 Map 提交承诺**——干净解决了「异步写回 vs 并发 Unmap」。
- **work debt 把「必须完成但超预算」表达为显式状态**——方向正确，问题只在可验证性（见 A1），不在机制。
- **验收以确定性终因覆盖替代概率轮次**——工程判断正确。

内存分配面（第二轮确认成立）：

- **外置 metadata + 分级 order 树**——`claim/dealloc` 步数只由地址位宽与 arena 数决定，不随运行期碎片数增长；比 buddy bitmap 或空闲链表更适合该场景。
- **planner 的 fail-closed 与固定 workspace**——`MAX_CLASSIFIED_RANGES = 1169` 的推导（`M+2MP+MB+1+H+R`）是「依据驱动的容量设定」的正面范本，可作为 M3/M4/C1 的对照标准。
- **`Funded` 的字段顺序（extents 在前、charge 在后）且不提供公共拆包入口**——跨两个账本的瞬时交接始终守恒。
- **`TableTree::Drop` 拒绝未 drain 树**——把「忘了 drain」从静默泄漏变成即时拒绝，是不变量可局部验证的正面例子。
- **Pool 四项守恒 + 外部不可伪造的 `OwnerKey`**——即使调用者重复提供相同 identity，token 也不能跨实例提交。
- **清零在 POOL 锁外、quota commit 之前**——长工作不持锁（符合协作式），且失败时两类 owner 分别 RAII 回滚。
- **容量证明覆盖整个生命周期**——`reserved_extent_growth` 统计在途事务尚未兑现的最坏增长，而非只覆盖单次创建。

## 排期与接手

M2 已修复，A2/M3/M5 的条件性准入校准已完成。其余发现不占近期施工顺序，也没有相互的默认执行顺序；M1、M4、B1、A1、C1–C3 与 D 可随实际消费者、故障或明确的设计/审查需求择机复核。启动某项前按上方归属映射核对当前代码与已有计划，发现仍成立再确定范围，不以重复计量为默认下一步。

KernelMemoryBudget 独立按不可信创建域、预算授权或跨域容量保证触发；必须在相应能力开放前完成，不因本记录暂缓而延期这一前置。

## 本轮核验与接力

`be6dae3` 的干净工作树为首轮调查基线；当时只读核对全局及本地计数、Bind 前整体预留和 fallible ledger 存储、JobControl 的 CREATE 校验、`srv_init` 对 `srv_pm` 的实际 grant。随后接入 Talc `counters`、目标布局与 Ready/Reset 两点堆观测；`just check`、`just clippy`、virt/core、virt/stress 均通过（首轮摘要保存在本地 `artifacts/heap-core-run.log` 与 `artifacts/heap-stress-run.log`）。该轮没有测 32 张账本或运行中采样；后续校准及其局限见下节。Talc `counters` 增加常数级计数更新与 allocator 状态大小，尚未单独基准测试吞吐。


## 容量校准记录（2026-09-24 至 2026-09-25）

### 范围与非目标

本轮闭合 M5 的运行中 heap/admission 峰值观测，以及 A2/M3 的真实 Bound AddressSpace 容量边界。测试必须经过正式 `ProcessCreate → ProcessBindMemory` 路径，覆盖顺序创建、并发创建、混合运行压力、`N+1` 失败和完整收束。
本轮不实现独立 `KernelMemoryBudget`，不因测试方便改变 `MAX_ADMITTED_BOUND_ADDRESS_SPACES`、Region ledger 或 planner transaction 的政策常量，不新增测试专用内核 syscall。

### 观测契约

- heap 记录 `claimed`、`allocated`、`available`、`fragments`、已消费 chunks；当前实现的极值只取 Ready 后 trap/Bind/Reset 采样点，source 取不到 ticket 时保留失败分配布局。它们不是 syscall 内连续高水位或最大连续空闲块。
- metadata admission 记录每类 Counter 当前值和峰值，至少包括 AddressSpace、Region、planner transaction、backing、MemoryChange、Wait、Remote 与 IPC。
- workload 记录同时存活的 Bound AddressSpace 数、阶段、操作结果和错误类型；创建成功不等于容量校准完成。
- 观测不得在 HEAP 锁内重入分配，不得改变锁序；失败或收束路径也必须记录。

### 压力矩阵与完成门

1. `N=1..32` 顺序 Create+Bind，保持全部 Bound 存活，测量每张完整 ledger 的实际增量。
2. `N=1..32` 并发 Create+Bind，复用正式多线程/多 hart 验收路径，观察全局 admission 竞争。
3. 在 8 张常驻 Bound 上叠加 Map、Protect、partial Unmap、backing view 收缩与正式 drain；core/stress 的其余 Tunnel、Job 和退出路线独立回归，不把它们误称为与 32 张同时驻留。
4. 在最大成功点继续尝试 `N+1`，区分 `ReachLimit`、`OutOfMemory`、`NoFrame` 与其它错误；不得以任意错误代替准入边界。
5. 达到压力后执行 kill/exit/drain，记录收束期间采样最低余量并核对 Pool、frame、metadata 和 process 账户回到基线；init 自身占用的账户不归零。
6. 重复固定顺序、并发和混合场景；本地 `artifacts/` 保留完整输出和命令身份，不能把现有 stress 竞态通过误称为 32 张账本容量证据。

本轮条件性准入的完成门是：明确采样盲区、顺序/并发 `N=1..32` 结果、`N+1` 失败出口及完整收束账户，再决定是否保留政策上限。上述证据已经取得，32 保留不变。aggregate available、碎片数量或无 source failure 不构成任意未来工作集的物理保证；最大连续块与硬容量保证不属于本轮完成门。

### 实施顺序与验证

先实现无分配峰值观测，再接通正式 workload；开发期间按影响运行 `just check`、`just clippy`、必要的 host 检查和 `just virt`/`just virt-stress`。成功结果写入 `artifacts/` 并回填本节；失败保留完整日志、退出码和未闭合责任。`COMPASS` 只在本轮状态发生变化后更新摘要。

### 首轮实施与验证结果（2026-09-24）

#### 已实施

- `metadata_admission::Counter` 增加无分配 `peak()` 和 Ready 基线 `reset_peak()`；资源表提供每类 admission 当前值/峰值快照。
- heap 在用户 trap 收尾以及 Bind/Reset 读取 allocated/available/fragments 并累积采样极值；Ready 建立基线，source 无 ticket 时记录失败布局。采样不能覆盖单个 syscall 内的瞬时极值。
- `srv_init` 增加显式 `capacity-calibration` feature 与 `just virt-capacity` 路线，沿真实 ProcessCreate → ProcessBindMemory 保留 31 个新增对象，使总 Bound AddressSpace 达到现行 32 上限；第 33 个尝试要求 `ReachLimit`，随后使用正式 abandon-to-completion 收束并核对 Pool 退款。
- `srv_init` capacity workload 追加 4 线程并发 Create+Bind；每线程最多 8 个，达到 31 个新增成功后仅允许 `ReachLimit`，主线程统一 Join 和 abandon-to-completion。
- `srv_init` capacity workload 追加 8 个常驻、已映射 Bound process，与正式 stress 的 `drain minimum-budget` 共存；256 registrations 分批完成 4165 次 drain 后再统一 abandon。
- `tools/qemu-acceptance.sh` 增加 capacity profile 和成功日志保留开关；完整日志使用 `QEMU_ACCEPTANCE_KEEP_LOG=1` 保存。

#### 结果

- capacity 完整日志：[qemu-acceptance-20260924-232158-63165.log](../artifacts/qemu-acceptance-20260924-232158-63165.log)。31 个新增 Bind 全部成功，总计 32 个 Bound AddressSpace；第 33 个返回 `ReachLimit`；测试收束后通过全部现有 acceptance 锚点。
- capacity Reset heap：`claimed=16776856`、最终 `allocated=420488`、运行 `peak_allocated=12743568`、`min_available=4007936`、`peak_fragments=109`、`source_failures=0`。这是顺序完整 ledger 场景，不是混合 stress 的容量上界。
- capacity Reset metadata peaks（数组顺序由 `admission_usage` 定义）：AddressSpace `33`、Region slots `131072`、planner transactions `128`；第 33 个失败尝试发生了可回滚的暂时 admission 峰值，最终 current usage 已恢复为基线。
- capacity 映射阶段通过：31 个新增 Bound process 各保留入口页和栈顶页两笔真实 `ProcessMap`，随后由 abandon-to-completion 清理；完整日志为 [qemu-acceptance-20260924-234213-77204.log](../artifacts/qemu-acceptance-20260924-234213-77204.log)，Reset 峰值 `peak_allocated=12791440`、`min_available=3956096`、`peak_fragments=115`、`source_failures=0`。
- capacity 并发阶段通过：4 个用户线程合计 31 个新增 Bind 成功、1 次 `ReachLimit`，并发结果完整收束；完整日志为 [qemu-acceptance-20260924-233555-74291.log](../artifacts/qemu-acceptance-20260924-233555-74291.log)，Reset 峰值 `peak_allocated=12761152`、`min_available=3989824`、`peak_fragments=123`、`source_failures=0`。
- capacity 混合阶段通过：8 个常驻 Bound AddressSpace 与完整 MemoryObject Map/Protect/partial Unmap、backing view 收缩和正式 `drain minimum-budget` 共存；drain 256 registrations 分 4165 批完成。最终日志 [qemu-acceptance-20260925-004612-5596.log](../artifacts/qemu-acceptance-20260925-004612-5596.log) 的 Reset 采样极值 `peak_allocated=12791184`、`min_available=3956352`、`peak_fragments=314`、`source_failures=0`，账户回到 init 基线。
- capacity、core、stress 均完成显式 system reset 和资源收束；stress Reset heap 为 `peak_allocated=4172632`、`min_available=12476640`、`peak_fragments=509`、`source_failures=0`。
- 最终开发检查：`just check` 通过；capacity feature 下 `just clippy` 的 shared-host、os-host、kernel-target、user-host、user-target、user-stress、user-fp 均通过；`metadata_admission` aarch64 host 8 tests 通过；capacity `just build_user` 与 ELF audit、`just virt-capacity`、`just virt`、`just virt-stress` 通过。首次 capacity fixture 把 init 自身的一个 Bound 重复计入，已修正。混合场景原有 MemoryObject 全局 Pool 退款相等断言在其它 charge 同时退休时失败；混合前后实测 `11204→11203`、`11203→11203`，故 capacity 只要求不净增加，正式 core 仍保留精确断言。局部账差不能单独证明本次 mapping 退款；最终账户及 reset 锚点已通过。相关失败日志保留在 `artifacts/failed-acceptance-20260925-000218-83080.log`。

#### 重复验证与延期边界

- 已完成固定顺序重复：清理短路修正后两次 QEMU capacity 分别见 [repeat A](../artifacts/qemu-acceptance-20260925-003347-92693.log) 与 [repeat B](../artifacts/qemu-acceptance-20260925-003539-95589.log)；两次均为 32 张成功、第 33 张 `ReachLimit`、并发 31 张成功、8 张混合与 4165 批 drain 完成，Reset `allocated=420680`、`peak_allocated=12791184`、`min_available=3956352`、`source_failures=0`，碎片峰值分别为 301/318。
- Counter acquire 高水位为精确原子峰值；heap 极值只在 trap/Bind/Reset 边界采样，可能漏掉 syscall 内短暂峰值。Talc 仅公开 aggregate available 和 fragment count，没有最大连续空闲块接口；source failure 只报告 source 无法取得 ticket 或 claim 失败的请求。这些局限已纳入条件性准入结论，不阻塞本轮交付。
- 延期项由本记录保留：若真实 OOM/退休故障、指定工作集的硬容量承诺或性能瓶颈出现，再核对消费者、最大布局、完成余量及锁序，决定是否增加 allocator 内计量或连续空间/专用 arena 预留。触发前不继续为 32 重复计量；完成条件是相关操作在定义的工作集下满足新承诺，并可在压力与退休路径验证。

### 交付前结构检查与当前判断（2026-09-25）

- 受影响的真实消费者仍是 `srv_init` 正式验收编排：capacity feature 不进入共享 ABI；default core/stress 分支未更改测试门。Bound 的 PoolBinding、Region/planner permits 和 Builder/Control 仍沿原有 owner 路径取得并通过 abandon-to-completion 收束；用户可达失败返回 `ReachLimit`，测试逐项收束不再因首次错误短路。
- `MetadataAdmission::counters()` 统一 usage、peak、reset 的 20 类顺序；Ready 前建立无并发基线，精确 peak 在每次成功 acquire 上更新，失败事务已预付但退款的临时占用也会计入峰值。观测只获取原有 HEAP/ADMISSION 锁、按顺序释放后写日志，不增加交叉持锁或 heap 内分配；在 trap 末尾采样增加一次 HEAP 锁，core/stress 回归已通过，尚未量化额外性能成本。
- `MemorySpace::new` 在 Bind 内 `try_reserve_exact(4096)` 创建 Region Vec，RV64 Region 条目 96 字节，单次请求至少 `393216` 字节；transaction Vec 为 `4×64=256` 字节。完成的顺序和并发 32 张 Bind 证明这些请求在各次创建时成功；不能由当时成功推出长寿命混合负载下未来最大连续块。
- 最后一次验证 [qemu-acceptance-20260925-005251-9127.log](../artifacts/qemu-acceptance-20260925-005251-9127.log)：32 张成功、第 33 张 `ReachLimit`、4 线程 31 张成功、8 张常驻混合 Map/Unmap/backing/drain 通过。Reset `peak_allocated=12791184`、`min_available=3956352`、`peak_fragments=319`、`source_failures=0`；mixed mapping Pool `11206→11203`，净额未增加，但其它退休同时发生，不能据此推出局部精确退款。
- 当前判断：保留现行 32 作为条件性准入上限，不提高限额，也不将其描述为任何业务工作集的物理容量保证；内核 fallible 分配路径仍决定真实 OOM 边界。M5 的严格瞬时峰值与连续空闲块并非该契约的必需前置；后续有实际需求时按上面的延期条件复核。本记录因其他审计发现保留，但容量校准本轮已完成。
- advisor（`moeflux-openai-responses/gpt-6-astra`）复核结论已采纳：32 是条件性全局准入上限；新建和普通操作在 Commit 前允许 `OutOfMemory`/`NoFrame`/`ReachLimit`，但一旦 Commit，事务必须只消费已持有的 owner、reservation、work debt 和稳定游标并最终完成；退出/drain 同理。最大连续块观测可以辅助未来硬容量方案的诊断，但不足以替代连续空间预留或专用 arena。当前授权范围内不引入 `KernelMemoryBudget`、不调整 32。
- post-Commit 责任审计：`MemoryChangeCompletion` 在启动前取得 metadata、Wait、Remote、work debt 和 completion；`MemorySpace::commit_inner` 注明不得分配；`Process::new` 取得 termination、drain waiter、request、finalization debt；`drain_batch` 只消费既有 finalization/handle/address-space owner 并按预算推进。已有 ideas/impls 契约与这些实现一致，未发现本轮必须修复的未预留责任。
其余发现开工时按归属映射核对既有专题；本计划唯一拥有的 C 类、M1、M4 等若需实施再明确范围。已完成的 M2 与 A2/M3/M5 容量校准在本记录保留结论和证据，不再次排入施工；其余发现经复核关闭或移交后，本记录再归档。
