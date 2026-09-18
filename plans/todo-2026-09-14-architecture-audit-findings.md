# 架构审计发现承接

> 状态：待逐条审视的发现清单，不是当前实施任务，不阻塞任何活跃专题。审视基线为 `master` 分支 `f4a4d57`（工作树另有「通用执行与准入」闭包的未提交改动，与本清单无关）。每条发现都须先按 `AGENTS.md`「标准施工流程」完成接手、规模审计与设计闭包，再决定实施、降级为文档修正或关闭。
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
| A3 libfal→libsrv 依赖反向 | 全部（本计划唯一拥有） | — |
| B1 同步 Caller 与事件循环并存 | 「阻塞必须显式」的类型要求 | typed PendingCall / Outbox 归 [`service-runtime-prerequisites`](todo-2026-09-13-service-runtime-prerequisites.md) |
| B2 srv_fs 验证面与成本不匹配 | 全部（本计划唯一拥有） | FAL 业务操作归 [`fal-service-capabilities`](todo-2026-09-fal-service-capabilities.md) |
| B3 `shared::service::Endpoint` 空占位 | 全部（本计划唯一拥有） | 服务发现实现归 FAL 总计划 |
| C1–C3 容量/粒度/派生依据 | 全部（本计划唯一拥有） | — |
| M1 裸帧号穿越 affine 所有权边界 | 全部（本计划唯一拥有） | — |
| M2 metadata 耗尽后果是内核 panic | 事实核实（是否为真实 panic 面） | 配额与预算机制归 [`kernel-memory-budget`](todo-2026-09-14-kernel-memory-budget.md) |
| M3 region slot 全局不可归属配额 | 同 A2（同一问题的容量面） | 配额机制本身归 `kernel-memory-budget` |
| M4 固定 64 extents 限制碎片容忍度 | 全部（本计划唯一拥有） | — |
| M5 内核堆无压力观测与扩展路径 | 观测面要求 | 预算归属归 `kernel-memory-budget` |
| D 协作式延迟无观测面 | 「内核路径恒短是待测断言」的文档定性 | Zicntr 计时手段见 `COMPASS`「挂起项」 |

## A 类：设计层实质问题

### A1 退役债务不变量不可局部验证，其失败信号已被归类为环境噪声

**现状与位置**：`notes/ideas/mm.md`「MemoryChange 事务」末段要求「预算必须覆盖完整调用链，包括查找、容器移动、最后引用析构和通知传播；不能把一次许可归还可能引出的任意宽度通知隐藏为常数工作」。实现面为 `os/kernel/src/deferred_work.rs`（四组 `WorkDebts` + 四个并行 `PENDING` 数组 + 每安全点 16 步 / 每债务 4 步）与 `os/kernel/src/task/retirement.rs`、`task/notify_work.rs`。

**判断**：该不变量约束的是「任何未来新增的字段或通知都不得引入无界宽度」，是全局属性，无法由类型系统、锁序断言或 contract test 局部证明。每次给对象加一个 Drop 副作用都可能破坏它。更关键的是它的失败信号恰好是压力测试超时——`plans/archived/ref-2026-09-acceptance-timing-flake.md` 记录的正是该形状（300 秒墙钟在 Tunnel 矩阵后截断，无 panic、无稳定依赖环），结论为「墙钟敏感的偶发验收现象」。该归档在证据上成立，但同时意味着这条核心不变量当前唯一的检测手段被判为不可靠，且内核没有 per-operation 的债务批次数观测，只有总耗时。因此该不变量事实上处于「声明存在、无人守卫」状态。

**待办**：为每个对象的 close/retire 路径提供**确定性计数**（批次次数与每批步数）并设可断言上界，使违规表现为计数越界而不是墙钟超时。完成标准是存在一个可在 host 或 fixture 中直接断言的观测面，且覆盖 Pool 归还、对象 view 退役、通知传播三条路径。

**触发条件**：下一次出现「同一 round 重复无进展」「attempts/waits 持续增长而轮次不前进」，或任何 close 路径新增 fanout。

**验证**：`ref-2026-09-acceptance-timing-flake.md`「未来重开条件」列出的判定项；本项不重开该归档，只在触发时以本条目为承接入口。

**把握程度**：中。文档层面证据充分，但未核对 `deferred_work.rs` 是否已存在未成文的计数观测。

### A2 全局 metadata 配额不可归属，与「纯 capability 授权」承诺冲突

**现状与位置**：`os/kernel/src/task/resources.rs` 顶部常量。`REGION_SLOTS_PER_ADDRESS_SPACE = 4_096`、`MAX_ADMITTED_BOUND_ADDRESS_SPACES = 32`、`REGION_SLOT_GLOBAL_LIMIT = 两者相乘`、`ADDRESS_SPACE_GLOBAL_LIMIT = 4_096`、`PROCESS_GLOBAL_LIMIT = 4_096`。每笔成功 Map 至少产生一个 Region 与一个 `ReservationGroup`，切分还会铸造新 RegionKey，因此 region slot 是**一个全局共享池**，容量等于 32 个满密度地址空间。

**判断**：`notes/ideas/mm.md` 自己写明「资源有账本和硬上限只证明可归属及失败安全，不自动证明不同授权域之间的 DoS 隔离」。当前 region slot / object view / endpoint / connection 等 per-sponsor 限额都挂在全局 admission 上，per-sponsor 限额是平摊的、不能授权、不能收紧。capability 授权的是**操作权**，不授权**资源额度**，而全局 metadata 池是 ambient 的。后果是：任何持 JobControl 的受托域都可以合法耗尽全局 region slot，使关键路径 Map 返回 `OutOfMemory`，且没有任何授权机制阻止——配额不在它的 capability 图上。另外 `MAX_ADMITTED_BOUND_ADDRESS_SPACES = 32` 是 `REGION_SLOT_GLOBAL_LIMIT` 的推导依据，但真正决定容量的是物理 metadata 字节；32 目前没有可追溯来源，与 `AGENTS.md`「工程限额必须有依据」冲突。

**待办**：① 审查 `MAX_ADMITTED_BOUND_ADDRESS_SPACES` 等常量的依据，或按实际 metadata 字节从系统储备推导，或明确降级为「当前政策，见 KernelMemoryBudget」；② 认定 KernelMemoryBudget 的触发条件是否已被满足——`srv_pm` 已持 `pm_domain` JobControl 并管理子域进程，「受托创建域」在架构上已经存在，只是域内目前全是自研负载。

**完成标准**：常量的依据可一句话说清并指向单一真值来源；触发条件的认定有明确结论（已满足 / 未满足 + 判定理由），结论写回 `notes/ideas/mm.md` 或 `kernel-memory-budget` 计划。

**把握程度**：高（常量取值已核对）。未核对全部使用点，也未确认是否已有未成文的 per-sponsor 收紧路径。

### A3 `libfal` 依赖 `libsrv`，依赖方向与分层声明相反

**现状与位置**：`user/frameworks/libfal/Cargo.toml` 依赖 `libsrv`；`libfal/src/{authority,backend,data,grant,store,value}.rs` 的**公开类型签名**中出现 `libsrv::budget::{Account, Charge}` 与 `Rc<dyn libsrv::wake::Wake>`。

**判断**：`notes/ideas/framework.md` 声明「准入机制提供账户、额度、预留与真实释放后的退款；领域资源类别由领域定义」。`Taxonomy` 反转该机制已经做对（分类归 `libfal::resource`，机制归 `libsrv::budget`），但**依赖方向做反了**：协议库的公开类型里出现执行框架的具体类型。后果是 provider 实现被迫知道记账；`bytes.rs`/`header.rs`/`node.rs`/`protocol.rs` 这些本可独立 host 可测的纯逻辑被迫拖着 libsrv 才能编译；未来跨信任域复用 FAL 会带着执行框架的额度语义一起走。

**待办**：把额度与唤醒的**抽象**下移到 libsrv 之下（`shared/` 内新 crate，或 `libfal` 自己定义 trait 并由服务层注入），`libfal` 面向 `&dyn Budget` / `&dyn Wake` 编程，具体 `Account` 由服务层提供。完成标准是 `libfal` 不再依赖 `libsrv`，且 `memfs` 请求路径不再出现具体 `Charge` 类型。

**触发条件**：立即可做，成本最低。建议作为本清单首项。

**验证**：`libfal` 的 host 测试独立通过；`cargo tree -p libfal` 无 `libsrv`；`just clippy` 用户态面通过。

**把握程度**：高。

## B 类：当前阶段的实质矛盾

### B1 同步阻塞 `Caller` 与事件循环执行模型并存，且类型上不可区分

**现状与位置**：`user/frameworks/librpc/src/caller.rs` 的 `Caller` 为同步阻塞、线程私有 ReplyPort、一次一个 outstanding；`notes/impls/rpc.md` 自承「`send_blocking` 在 MailboxFull 时无限等待，因此它还不是完整调用 deadline」。同时 `notes/ideas/framework.md` 要求「下游 RPC、发送背压和设备完成作为挂起状态，不在控制循环中阻塞」，`user/frameworks/libsrv/src/runtime.rs` 是异步单 actor 有界推进。

**判断**：两套执行模型都是**正式形态**而非标注的过渡。在 `no_std` 服务里一次 `Caller::call()` 会直接停住 actor 线程，而类型上看不出来。这不需要谁犯错，只需有人自然地写 `let r = caller.call(...)`。

**待办**：让 `Caller` 的等待形态返回 `PendingCall`，「阻塞」成为显式适配器（如 `block_on`），使在事件循环里误用它在类型上显得别扭。完成标准是事件循环路径上不存在可隐式阻塞的调用形态。

**边界**：typed `PendingCall` 与完整投递阶段（Unsent/Sent/Completed）归执行前置，本项只拥有「阻塞必须显式」这一类型要求。

**把握程度**：高（`notes/impls/rpc.md` 已自承该缺口）。

### B2 `srv_fs` 的验证面与维护成本不匹配

**现状与位置**：`user/services/srv_fs/src/main.rs` 同进程 memfs provider + 客户端泵；`notes/impls/fal.md` 自列边界：slot 1 是临时 anchor 副本、无 DirectoryGrant、无 rights ceiling、provider 与 client 同进程、Lookup Delegate 只在 mock 中、Open 返回 Unsupported、Move/Copy 返回 Unsupported。

**判断**：剥掉上述边界后，该负载实际覆盖的是 Mailbox 往返 + Handle move + send-once——而这三项在公共 IPC 前置中已有更严格的 fixture（真实双接收线程、forced Full、64 条独立授权、跨进程 CLOSE 提交后 kill）。风险是它会让人误以为 FAL 已通，而泵逻辑是一笔真实的自检维护成本。

**待办**：在 FAL 业务恢复前二选一——接上真实 DirectoryGrant 使 slot 1 名实相符，或降为最小 smoke。完成标准是该负载的验证声明与实际覆盖面一致，无中间态。

**把握程度**：高（`notes/impls/fal.md` 已自列边界）。

### B3 `shared::service::Endpoint` 是空占位 struct

**现状与位置**：`shared/erhino_shared/src/service.rs` 全文为 `pub struct Endpoint {}`；`shared/erhino_shared/src/sync.rs` 全文为 `pub mod spin;`。`notes/ideas/service.md` 已是完整契约，但代码中该名字会被 import，且 `Endpoint` 当前有三重含义（Tunnel Endpoint、Mailbox 的 endpoint 概念、未来的服务 endpoint）。

**待办**：要么现在定名（`ServiceRecord` / `ServiceEndpoint`），要么删除空文件等实现落地。完成标准是不存在无实现的公开占位类型。

**把握程度**：高。

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

**待办（必须先补证）**：核实是否所有用户可达路径上的 metadata 分配都走 fallible 版本（`try_reserve` 等），使 `alloc_error_handler` 只在内核内部 bug 时触发。核实结果为「是」则本条降级为文档措辞修正；为「否」则须把对应路径改为返回 `SystemCallError::OutOfMemory`。**在核实前不改代码。**

**把握程度**：中——两条规则的冲突已确认，**未**逐条核实所有分配点的 fallibility。

### M3 region slot 的全局不可归属配额（A2 的容量面）

**现状与位置**：`os/kernel/src/task/resources.rs`：`REGION_SLOTS_PER_ADDRESS_SPACE = 4_096`、`MAX_ADMITTED_BOUND_ADDRESS_SPACES = 32`、`REGION_SLOT_GLOBAL_LIMIT = 二者相乘`。

**判断**：每笔成功 Map 至少产生 1 个 Region 与 1 个 ReservationGroup，故 region slot 是全局共享池。`notes/impls/mm.md` 给出了 `REGION_SLOT_GLOBAL_LIMIT = 131072` 的算术推导，但 **32 本身无依据**——真正决定容量的是每 fragment 的 metadata 字节数。对照同一文档中 `MAX_ARENAS = 2048` 的推导（`16 × 2 × usize::BITS`，有依据），32 属于「政策常量伪装成容量推导」，违反 `AGENTS.md`「工程限额必须有依据」。

**待办**：与 A2 合并处理——给出依据（按实际 metadata 字节从系统储备推导）或明确降级为「当前政策，见 KernelMemoryBudget」。

**把握程度**：高（常量取值与推导链已核对）。

### M4 固定 64 extents 的推导来源是栈预算，不是内存语义

**现状与位置**：`os/kernel/src/frame.rs`：`pub(crate) const MAX_FUNDED_EXTENTS: usize = 64`；`funded_frame::fund` 以它为单次资金化事务的 extent 上限，超限返回 `FundError::ExtentLimit`。`task/proc.rs` 另设 `MAX_BACKING_SPLITS_PER_CHANGE = MAX_FUNDED_EXTENTS * 4`。

**判断**：一个 512 页（2 MiB）的 backing 在库存碎片化后可能超过 64 个 extent，此时**尽管 Pool 额度与物理页都充足**仍失败。`notes/impls/mm.md` 承认当前依托「四页单 extent 是当前两平台启动供给的 fixture 前置」；长时间起停服务后该前提不成立。更重要的是推导方向：64 的来源是「debug 帧成本由栈 guard 与 ELF audit 共同约束」，即**由栈预算倒推**，而非由内存语义推导——与 `AGENTS.md`「栈帧审计不能反过来扭曲通用机制」相悖。且 `notes/impls/memory-object.md` 已为常驻 `ObjectBacking` 采用堆化多 extent 列表并明确「定长容器只适合一次性事务结果」——那么一次性事务为何仍内联 64 槽，这个不一致需要回答。

**待办**：重审 64 的依据；或在一次性事务结果同样采用堆化存储（`alloc` 可用），使 extent 数上限由内存语义而非栈预算决定。**正确性不受影响**（失败发生在 Commit 前且可恢复），本条是可用性与依据问题。

**把握程度**：中（常量与推导来源已核对，未核实碎片化实测数据）。

### M5 内核堆没有压力观测，也没有接近上限时的分级拒绝

**现状与位置**：`os/kernel/src/frame.rs` 的 `HEAP_CHUNK_LIMIT = 16`（每 chunk 1 MiB）、`RECOVERY_TICKET_LIMIT = 0`；`os/kernel/src/heap.rs` 的 `SystemSource` 只做 O(1) ticket pop，ticket 一经消费永久归堆且不支持归还。`plans/COMPASS.md`「挂起项」现有条目为「过渡 admission/Remote 槽高水位可观测性」，触发条件是「KernelMemoryBudget 立案或容量重校需求出现」。

**判断**：在 KernelMemoryBudget 落地前，48 MiB heap tickets 是**唯一的 metadata 容量真值**，且既无压力观测也无接近上限的降级路径（只能等 M2 的 panic）。现有触发条件偏窄。

**待办**：① 把触发条件放宽为「任何用户可达的 metadata 耗尽现场」（与 M2 联动）；② 提供 heap 高水位/剩余量观测，并在接近上限时对新的创建类操作返回干净错误而非等待 panic。

**把握程度**：中（常量与消费路径已核对，观测面缺失已确认；未评估实现成本）。

## D 类：协作式定性的诚实质疑（文档定性，不立案实施）

**现状与位置**：`notes/ideas/kernel.md`「协作式内核」以「若某项需求看起来必须依赖内核抢占、内核睡眠或后台内核线程，首先应重新判断工作归属」论证协作式可接受性。

**判断**：该论证在方向上成立（长工作放用户态），但作为论证不可证伪——任何反例都会被重新解释为「归属放错了」。真正承重的命题是同一篇「有界路径」中的「用户可触发的单次内核调用必须有结构性工作上界」。「结构性有界」（无无界循环）确实做到了，但协作式可接受性依赖的是**延迟有界**，而系统目前没有内核驻留时间的观测面——`plans/COMPASS.md`「挂起项」自己承认 fence.i 优化缺 dispatch 计时手段（Zicntr），连调度开销都测不了，遑论最坏 trap 延迟。结论是「内核路径恒短」目前是**设计意图而非已验证性质**。

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

本清单**不参与** `COMPASS` 的活跃计划串行位。建议按成本与独立性排序，各自独立立案：

```text
M2 核实（纯读码，零改动，且结论决定 M3/M5 的紧急度）
  → A3（纯依赖调整，立即可做）
  → B3（删占位或定名）
  → M1（affine claim token，消除所有权窄缝）
  → B2（srv_fs 验证面收口）
  → A2 + M3（同一问题的配额面，合并立案）
  → M4（一次性事务 extent 容器堆化，与 ObjectBacking 对齐）
  → M5（堆压力观测 + 分级拒绝，与 M2 结论联动）
  → B1（阻塞显式化，与执行前置的 typed PendingCall 协调）
  → A1（观测面，待触发条件或主动补证）
  → C1 / C3（容量依据，各自独立）
  → C2（需先采集数据）
  → D（纯文档修正）
```

每项在开工时按其归属映射并入对应专题，或（A3/B3/B2/C 类）单独立案。完成一项后从本清单删除该条并回填结论去向；全部清空后本计划归档。
