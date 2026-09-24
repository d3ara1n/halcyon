# 架构审计发现承接

> 状态：M2 的三处用户可达不可失败 heap 分配已修复并验证；A2/M3/M5 的触发和准入路径已核对，RV64 布局成本与正式 core/stress 两点堆采样已有开发证据。尚无运行中峰值、32 张账本饱和与其它 metadata 同时驻留的证据，不能以当前采样承诺 32 可满额准入；下一步补峰值与并发负载并据此校准全局政策，独立预算仍按授权触发。原始审视基线为 `master` 的 `f4a4d57`（当时另有执行闭包未提交改动），不能当作当前工作树状态。进入实施的发现须按 `AGENTS.md` 施工流程核对事实并闭合设计；已关闭条目从本清单删除，由对应专题和 git 历史保留证据。
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
| M5 内核堆无压力观测与扩展路径 | 观测面要求 | 预算归属归 `kernel-memory-budget` |
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

**已测下界与待办**：目标 RV64 的 Region/transaction 条目为 96/64 字节，单张账本预留至少 393472 字节；32 张至少 12591104 字节，约为 16 MiB 物理 heap 储备的四分之三，尚未计入壳、其它对象和 allocator 开销。core/stress 的终点观测见「本轮核验与接力」。仍须取得真实创建时每张 Bound 地址空间的完整 heap 成本、运行期高水位/最大连续可用块、其它对象及完成路径的同时驻留成本、准入类别峰值与接近 32 张账本时的失败出口，再决定全局政策数值或降级为有出处的部署限制；不能单凭 32 张 ledger 的 Vec 下界推断可兑现容量。

**完成标准**：触发条件已按当前授权图判断；容量项仍待真实字节成本和运行期压力数据，使常量的理由可一句话说清并指向单一真值来源。未经校准不得把 32 写成系统可保证同时驻留的数量。

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

**判断**：4096 是当前每地址空间固定 ledger 容量，同时作为本地 admission 上限；32 是全局最多完整预留 32 张 ledger 的准入政策。RV64 已知 ledger 元素的最低预留为 12591104 字节（32 张），但没有从其余 bytes、并发负载和系统储备推导出 32 的依据。即使只有一个 live fragment，Bind 也消耗一张 ledger 的全局 slots 和对应 Vec 容量。保留 32 作为待校准政策不等于保证 32 张满载账本能与其它 metadata 同时驻留，也不等于 4096 个 fragment 由任何全局字节预算担保。

**待办**：与 A2 合并按真实 `Region`/transaction 布局、allocator 开销与其它对象峰值校准。独立 KernelMemoryBudget 解决来源隔离，不自动使全局容量数值合理；全局字节容量与域内可授权额度分别论证。

**把握程度**：高（常量取值与推导链已核对）。

### M4 固定 64 extents 的推导来源是栈预算，不是内存语义

**现状与位置**：`os/kernel/src/frame.rs`：`pub(crate) const MAX_FUNDED_EXTENTS: usize = 64`；`funded_frame::fund` 以它为单次资金化事务的 extent 上限，超限返回 `FundError::ExtentLimit`。`task/proc.rs` 另设 `MAX_BACKING_SPLITS_PER_CHANGE = MAX_FUNDED_EXTENTS * 4`。

**判断**：一个 512 页（2 MiB）的 backing 在库存碎片化后可能超过 64 个 extent，此时**尽管 Pool 额度与物理页都充足**仍失败。`notes/impls/mm.md` 承认当前依托「四页单 extent 是当前两平台启动供给的 fixture 前置」；长时间起停服务后该前提不成立。更重要的是推导方向：64 的来源是「debug 帧成本由栈 guard 与 ELF audit 共同约束」，即**由栈预算倒推**，而非由内存语义推导——与 `AGENTS.md`「栈帧审计不能反过来扭曲通用机制」相悖。且 `notes/impls/memory-object.md` 已为常驻 `ObjectBacking` 采用堆化多 extent 列表并明确「定长容器只适合一次性事务结果」——那么一次性事务为何仍内联 64 槽，这个不一致需要回答。

**待办**：重审 64 的依据；或在一次性事务结果同样采用堆化存储（`alloc` 可用），使 extent 数上限由内存语义而非栈预算决定。**正确性不受影响**（失败发生在 Commit 前且可恢复），本条是可用性与依据问题。

**把握程度**：中（常量与推导来源已核对，未核实碎片化实测数据）。

### M5 内核堆没有压力观测，也没有接近上限时的分级拒绝

**现状与位置**：`os/kernel/src/frame.rs` 的 `HEAP_CHUNK_LIMIT = 16`（每 chunk 1 MiB）、`RECOVERY_TICKET_LIMIT = 0`；`os/kernel/src/heap.rs` 的 `SystemSource` 只做 O(1) ticket pop，ticket 一经消费永久归堆且不支持归还。admission/Remote 槽高水位观测曾仅以 KernelMemoryBudget 立案或容量重校为触发条件；本项要求额外考虑用户可达 metadata 耗尽现场，候选登记见 [COMPASS](COMPASS.md#尚未立案的候选)。

**核实后的判断**：M2 已证实并修复三处用户可达的不可失败分配；不能再以「是否真的触发 panic 待核实」作为 M5 前置。`admission_usage()` 统计 20 类 slot 的当前用量，主要用于启动自检；本轮启用 talc 5.0.4 的 `counters`，在 Ready 和授权 reset 时输出 claimed/allocated/available/fragment 与已消费的 system ticket 数。virt/core 的 Ready/Reset 分别为 allocated 404768/420152、available 16368224/16351680 字节；virt/stress 的 Reset 为 allocated 487288、available 16284544 字节。core Ready 与两条路线的 Reset 均已消费 16 个 ticket、claim 16776856 字节，证明「剩余 ticket 为零」不等于 allocator 已耗尽。快照并非高水位，且日志无法证明没有短时分配峰值或碎片导致的大块分配失败。

**待办**：补代表性运行中峰值、准入计数峰值、接近容量时最大连续分配能力/失败布局的证据；当前两点观测不得宣称为高水位。采集不得在 allocator 锁内重入分配或引入锁序逆转。若实际观测或故障显示特定创建类操作需要分级拒绝，再依据其最坏准备成本和必须保留的完成容量定阈值、准入点及干净错误出口；不能用剩余 ticket 数或随意高水位代替 allocator 可用量与完成责任证明。真实 metadata OOM 现场、未来不可信创建域或容量重校任一出现时重排本项；现阶段保持 fallible 创建与已预付完成路径，不增设无依据的拒绝阈值。

**把握程度**：高（计数与采样点有代码和 QEMU 证据）；运行中峰值与阈值成本仍未知。

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

## 自然顺序

M2 已修复，A2/M3 的授权触发与 M5 的两点观测已核实；下一项是运行期峰值及并发容量校准，不要求先实现独立预算。其余发现不整体占用串行位置；以下顺序仅用于接手，不表示彼此均有技术依赖：

```text
M5 运行中峰值 + A2/M3 并发容量校准（共享 heap/Region 证据）
  → M1（核实 affine claim token 的所有权窄缝）
  → M4（核实 extent 容器）
  → B1（阻塞显式化，与现有 typed PendingCall 对照）
  → A1（观测面，待触发条件或主动补证）
  → C1 / C3（容量依据，各自独立）
  → C2（需先采集数据）
  → D（文档定性）
```

KernelMemoryBudget 不按上述串行顺序自动开工；不可信创建域、独立预算授权或跨域容量保证一旦需要，须在开放前提前处理。

## 本轮核验与接力

`be6dae3` 的干净工作树为调查基线；首轮只读核对全局及本地计数、Bind 前整体预留和 fallible ledger 存储、JobControl 的 CREATE 校验、`srv_init` 对 `srv_pm` 的实际 grant。随后接入 Talc `counters`、目标布局与两点堆观测；`just check`、`just clippy`、virt/core、virt/stress 均通过（本轮运行摘要保存在 `artifacts/heap-core-run.log` 与 `artifacts/heap-stress-run.log`，成功 QEMU 路线的原始临时日志按工具惯例删除）。stress 摘要仅保留 Reset 行，故不把未留存的 Ready 行计作证据。检查重点：HEAP 计数复制后释放锁，再读取 SYSTEM_SUPPLY 并写 console；没有新增 ABI、跨组件 owner、分配准入阈值或测试专用消费者。Talc 启用 `counters` 增加常数级计数更新与 allocator 状态大小，尚未基准测试吞吐。未做同时准入 32 张账本或运行中峰值测试，本计划的容量完成标准仍未满足，保持进行中。

每项在开工时按其归属映射并入对应专题；本计划唯一拥有的 C 类、M1、M4 等若需实施则单独立案。完成一项后从本清单删除该条并回填结论去向；全部清空后本计划归档。
