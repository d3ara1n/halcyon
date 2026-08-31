# MemoryPool、MemoryObject 与多页 IPC 数据面

> 【当前实施计划】方向契约由 `notes/ideas/{mm,object,task,bootstrap,tunnel,runnel,buffer-queue}.md` 拥有，架构调查归档见 [`archived/todo-2026-09-ipc-data-plane-design.md`](archived/todo-2026-09-ipc-data-plane-design.md)，系统参照见 [`ref-2026-09-ipc-data-plane-systems.md`](ref-2026-09-ipc-data-plane-systems.md)。重排基线 `a8d6382`；严格按供给闭包、Pool 状态机、资金化帧取得、进程绑定、backing、数据面顺序实施，不从公共对象 ABI 倒推资源模型。

## 目标与边界

本计划先建立可证明的 page-backed storage 资源闭包，再公开 MemoryObject 和多页 IPC 数据面。用户匿名 backing、页表帧、bootstrap payload、Tunnel backing 与公共 MemoryObject 都经过一个 funded frame broker；普通与 object-owned mapping 都经过一个 AddressSpace/MemoryChange seam；Job、页额度、内核 metadata、CPU 与设备资源保持正交。

首版范围：

- 完整平台内存供给分类、物理隔离的系统储备和 strict count-solvent root MemoryPool；
- 页额度型 MemoryPool、不可撤销 child grant、固定宽查询与 Building-only `ProcessBindMemory`；
- AddressSpace 根/中间页表、匿名 backing、bootstrap payload、Tunnel/MemoryObject backing 的 frame/charge 同寿命；
- 固定长度 eager MemoryObject 的创建、Query、对象子范围映射与 `SealExecutable`；
- 多页 ObjectView、TunnelCreate/Attach 几何和 Runnel v2；
- shared、内核、rinlib、libprocess、librunnel 与 acceptance 同步迁移。

不在本计划实现：KernelMemoryBudget 公共 ABI、resize、COW、pager、文件缓存、原始 frame capability、普通 Pool revoke/reparent、MemoryLease、BufferQueue 代码、DMA pin/IOMMU/cache maintenance、动态链接、Job 聚合配额或内核 heap 通用收费。KernelMemoryBudget 未落地期间仅允许受 capability 控制的可信资源管理器创建可放大的内核对象，并以系统储备、全局 admission slots、每容器硬上限和 Commit 后零分配收束覆盖 metadata 失败；不得把该过渡状态描述为完整多租户 DoS 隔离。

## 外部契约

平台供给解析以固定规范为依据，不从现有单区间实现外推：

- Devicetree Specification v0.4「`/memory` node」要求接纳多个 memory nodes 及单个 `reg` 中的多个 range；
- 同规范「Memory Reservation Block」中的全部 `(address, size)` 区间不得用于普通分配，且 DTB 即使未列入 reservation block，也必须在解析完成前保护；
- 同规范「`/reserved-memory`」要求排除 static `reg`，处理动态 `size`/`alignment`/`alloc-ranges`，并区分 `no-map` 与 `reusable`；本计划实现 static 与 `no-map` 的分配/映射双重排除，动态放置和 reclaim 生命周期在独立计划 [`todo-2026-09-platform-reserved-memory-lifecycle.md`](todo-2026-09-platform-reserved-memory-lifecycle.md) 中闭合，在此之前对应平台描述必须于用户供给发布前明确拒绝；
- BootPackage 只以验证后的 envelope 实际范围进入 boot-held；loader 最大窗口不是可永久扣除的物理事实；
- RISC-V PTE 发布、`SFENCE.VMA`、IPI 与 `FENCE.I` 顺序继续以 [`notes/ideas/mm.md`](../notes/ideas/mm.md) 引用的 privileged architecture 与 RVWMO 章节为准。

## 共同不变量

### 物理供给

- 系统储备与 user supply 在第一次普通分配前物理分离，内核 heap/recovery 路径不得借 root pool 页；
- 稳态 `user_supply = free_inventory + boot_held + funded_extents`，事务态 claim/return 必须由唯一 affine token 占有；
- `root.total == user_supply`，root pool 不因碎片、暂时库存不足或 metadata 失败改变总额度；
- raw runtime frame allocation 不向用户可放大路径公开；每个调用点必须归类为 funded user path、固定 system reserve 或永久 platform ownership。

### Pool 与 charge

- 每个 Pool 恒满足 `total = available + reserved + allocated + delegated`；
- `ChargeReservation`、`MemoryCharge` 与 `ParentCredit` 不可复制，遗忘最多泄漏额度，不能扩容；
- Derive 只从 parent 的 available 转移为 delegated，child 自然消散后沿有界父链归还；duplicate、TRANSIT、GRANT 与跨 Job 移动不增加或重记额度；
- 普通 Pool 不登记 child、binding、backing、进程或 view，不提供枚举、关闭电平、强制撤销或 reparent；
- quota 不足返回 `QuotaExceeded`，实际库存或 metadata 不能满足返回 `OutOfMemory`，超出结构硬上限返回 `ReachLimit`，同代 pinned Handle 冲突返回 `ObjectBusy`。

### 进程与地址空间

- ProcessCreate 只创建 Building 空壳：PID、Job 成员、lifecycle、HandleTable、Builder、Control 与 Unbound AddressSpace；
- `ProcessBindMemory(builder, pool)` 是唯一 Unbound → Bound 操作：提交前失败不消费 Pool Handle，提交后安装不可转移 PoolBinding 与 charged root page table；
- Building 操作只在精确 Building 状态登记；Start 只有在 `building_ops == 1`、地址空间已绑定、至少一个线程和其余 readiness 成立时提交；
- root 与中间页表、匿名 backing 由目标绑定池支付；对象数据 backing 由创建池支付；view 所需页表由 view 所在进程支付；
- Published TranslationTree 必须分批 drain；`Drop` 只验证树已清空并做常数释放，不递归扫描仍存页表。

### Backing 与对象

- 物理 extent 与 charge 正交但同生灭：物理失败完整回滚 reservation，backing 释放同时归还 frame 与 charge；
- 匿名 backing 部分解除可以切出 `OwnedBackingSlice { extents, charge }` 进入 retire；
- ObjectBacking 唯一持有完整 extents 与 charge，ObjectView 只强持对象、offset/length 和 WritePermit，view 切割不切数据 backing；
- object backing 逻辑连续、物理可多 extent；页数、extent 数、单次 translation 数均有硬上限，普通 close 不扫描 view 或对象图；
- MemoryObject Handle 消散不解除 view；Tunnel Endpoint close 只撤销本端完整 lease；stale translation 确认前不归还 frame、charge 或 WritePermit；
- R/RW/RX 分别要求 `MAP|READ`、`MAP|READ|WRITE`、`MAP|READ|EXECUTE`；seal 要求 `MANAGE`，W+X 永远拒绝。

## 过渡期 metadata admission 门

KernelMemoryBudget 公共能力不在本计划实现，但每个新增或改造对象在进入对应切片前必须登记并实现过渡 admission：sponsor、最大存量、permit 的唯一 owner、Commit 后是否分配、退款终点和终态壳的真实寿命。至少覆盖 Process core/Builder/Control、Pool core、charge reservation、AddressSpace/Region/ObjectView、MemoryObject/Connection、Handle reservation、MemoryChange、WaitContext 与 Remote Call slots。

过渡 permit 来自物理隔离系统储备支撑的固定全局 slots。切片 2 先建立长期结构的最小骨架：可信 ProcessCreate/bootstrap 取得一个不可转授、不可扩容的内部 `MetadataSponsor` 并附入 `ProcessResources`，当前只开放 Pool core 类型化子额度；切片 4 在同一 `ProcessResources` 中增加 PoolBinding，并按后续对象逐类扩充 sponsor，不另建临时全局准入旁路。Running Map、MemoryObject/Tunnel Create 等路径只从当前进程 sponsor 的类型化子额度预留 permits，不需要再次持资源管理 capability。独立于进程存活的对象把 permit 与 sponsor 强引用一起带到自身真实析构，不能在 creator Dead 或 capability 转移时提前退款。容器硬上限限制单对象宽度，sponsor/global slots 限制进程及全系统总量，三者不能互相替代。Process core 到 Dead 才退 core permit，Builder 随最后引用退自己的 permit，Control shell permit 随最后一个观察 capability 消散；Publish/Commit 后不得再取得新 permit。该表、实现和 metadata exhaustion-before-Commit 测试是每片准入门；自然顺序是在该片新增首个可放大对象之前完成，而不是等切片 10 补账。公共完成标准是所有用户可触达分配均可失败、permit 唯一退款且 Commit 后零分配；在此之前不得开放对应创建面，也不得把固定 heap 容量描述为 KernelMemoryBudget 或完整多租户 DoS 隔离。

## 切片 1：平台供给账本与系统储备

扩展纯逻辑 DT/区间层，接纳全部 `/memory` ranges，解析 FDT reservation block 与 `/reserved-memory`，对任意来源执行 checked end、页边界规范化、排序、合并与区间相减。永久保留、系统储备、boot-held 与 user-free 使用不同 affine 类型；重叠或矛盾只能在显式优先级规则下归一，否则启动失败。`no-map` 进入独立政策子集，板级层生成带洞的标准直映射 admitted ranges，页表组件以 eager range mapper 自动选择最大合法叶，静态中间表预算不从物理供给借用。动态 reserved-memory 与 `reusable` 在独立生命周期机制落地前由平台 admission 明确拒绝。

平台供给账本批次已由提交 `198e665` 收口，未来复审对象固定在 [`todo-2026-09-platform-memory-ledger-review.md`](todo-2026-09-platform-memory-ledger-review.md)。

系统储备采用已确认的“用户 FramePool + typed system tickets”，不建立双 FramePool，也不在共享池上增加可误传的 class 参数。启动 planner 在发布 user-free 前把 FramePool metadata、预清零 heap chunks 与 recovery tickets 交给 `SystemSupply`；这些用途不可互换，均不进入未来 root pool。metadata 依实际 RAM 几何精确计算；heap chunk 数是独立的编译期物理容量政策，规定内核 heap 最多可占多少系统页，不根据尚无全局存量上限的对象数伪造“永不耗尽”证明。普通 metadata 分配必须可失败，后续 sponsor/global slots 决定对象级 admission；平台无法完整提供配置容量时 fail closed。现有 Commit 后完成、rollback、drain 和 remote-call 路径不分配物理页，因此本批 recovery ticket 数为零；未来路径若需要 recovery 页，必须先给出并发上界、提高独立子预算并通过 exhaustion 测试，才能进入 Commit。

Talc 5.0.4 的 `Source::acquire` 在 allocator 正忙时执行；内核 heap source 因而只能 O(1) 消费启动期预留且已清零的连续 chunk，不在 heap 锁内进入用户 FramePool、做区间查找或清零 1MiB。用户 FramePool 继续服务页表、匿名 backing 与 Tunnel 的 transitional raw 路径，但 API 显式标成 user inventory；它们在 funded broker 与所属后续切片落地时逐一取得 charge。进程自己的 malloc allocator 位于用户态 funded mapping 之上，与内核 heap 和物理库存分别优化。

### 批次 1B：系统储备物理隔离

1. 审计现有内核 heap 分配面和 Commit 后完成/恢复路径。区分物理 heap 容量与对象 metadata admission：前者由编译期 chunk limit 封顶并在耗尽时明确 OOM，后者按“过渡期 metadata admission 门”在每个后续切片新增可放大对象前完成；现有零分配完成路径导出 recovery ticket 数为零。
2. 把 `frame.rs` 的区间分类/放置抽成 host 可测的纯逻辑 `memory_supply` planner：固定容量输入输出、确定性放置、整体失败原子，输出 permanent、boot-held、system 子账户与 user-free 的互斥区间。metadata 只要求自身连续；heap chunk 逐块满足 1MiB 连续与对齐，不强迫全部 system reserve 物理连续。
3. 让单一 FramePool 只发布 user-free 与后续 user boot-held 回投；其 metadata 物理页由 `SystemSupply` 中的 `FramePoolMetadata` owner 持有，库存只能把这些地址看作永久 unavailable。启动日志同时证明 `managed = permanent + boot-held + system + user-free` 与 system 子账户求和。
4. 建立不可复制的 `HeapChunkTicket`/`RecoveryTicket`，启动期锁外清零 heap chunks；Talc `SystemSource` 改为单向 O(1) ticket pop。普通 heap 耗尽不得解锁 recovery，失败保持明确。
5. 把现有 raw API 收窄为 transitional `alloc_user_order`/`alloc_user_largest`，登记唯一调用点：进程页表、匿名 backing、Tunnel backing 和 user-inventory selftest；heap 不再出现在该表。
6. host debug/release 覆盖碎片布局、对齐、子预算不足、容量耗尽、用途不可互换、planner 失败零发布和 heap ticket 单调消费；`just check`、virt debug/release 与 sifive_u 复核分类闭包、heap 扩展及完整 acceptance。

批次 1B 已由提交 `0a944c7` 收口，未来复审对象固定在 [`todo-2026-09-system-supply-reserve-review.md`](todo-2026-09-system-supply-reserve-review.md)。planner workspace 的 1169 段容量由 16 memory、34 permanent、3 boot-held 与 16 heap chunks 的最坏切割推导；FramePool 的 2048 arenas 由 16 memory × 2 × `usize::BITS` 推导。planner/ticket 与 FramePool host debug/release、`just check`、virt debug/release、`sifive_u` 均通过；三条平台线都完成 16/16 acceptance，且启动日志证明全局分类与 system 子账户分别闭合。下一自然序进入切片 2。

批次 1A 验证基线：区间纯逻辑 host debug/release 覆盖多 memory nodes、多 tuple、空洞、相邻/重叠 reservation、溢出、页边界、DTB 自保护、BootPackage 实际范围、reserved-memory static/dynamic/no-map/reusable；页表 host 测试覆盖 1GiB/2MiB/4KiB 最大叶选择、洞、冲突预检与静态预算耗尽；平台启动日志给出 admitted range/静态表数量和各物理分类总量且总和闭合。

## 切片 2：MemoryPool 状态机与 capability 对象

先冻结运行期供给与 metadata 前置。平台账本发布时生成只含 `user-free + boot-held` 页数的一次性 `RootPoolSeed`；后续动态 `free_frames()` 不得充当 root 总额度真值。启动链在 heap 可用后消费 seed 铸造唯一 root core，内核 anchor 持有至本次启动结束；本片不提前把 root Handle 交给 init，正式 bootstrap 交付仍属于切片 5。建立 host 可测的 `metadata_admission` 计数/permit 基座，并把只含 `MetadataSponsor` 的最小 `ProcessResources` 前移；Pool core 同时受全局与创建进程 sponsor 的类型化硬上限约束，permit 随 core 和 sponsor 强引用到真实析构，root 使用 boot-lifetime primordial permit。

新增 host 可测的 `memory_pool` 纯逻辑 crate，唯一拥有四项计数、深度与线性 token 状态转换。`ChargeReservation` 与 `DelegationReservation` 分型；reserve/commit/rollback/return、charge split/merge 和 delegated credit 全部 checked。token 以 crate 内不可伪造 owner key 绑定状态实例，外部 Pool identity 只作稳定对象身份；child state 与 parent credit 不可拆分，只有消费 fully-available child 才能取回额度。纯逻辑层不分配帧、不访问 HandleTable，也不持 `Arc` 或内核锁；内核适配层以 parent 强引用和 RAII owner 把 reservation、charge 与退款连接到具体 core。所有额度非零，root total、identity、parent identity 与 depth 在构造边界冻结；仅深度和 ABI 表示域构成结构上限，不为 O(1) Derive 另设无依据的单次页数上限。parent 不登记 child，父子退款逐锁完成，不嵌套 Pool 锁。

增加 MemoryPool kernel object、metadata permit、Handle kind/role、rights、固定宽 Query ABI 与 rinlib typed owner wrapper：`Query` 要求 READ，`Derive` 要求 CREATE，Building binding 要求 GRANT；child 初始 rights 必须是来源 Handle rights 与目标最大 rights 的子集。duplicate/TRANSIT/GRANT 只传播同一 core authority。Query 返回 identity、parent identity、depth、total/available/reserved/allocated/delegated，不暴露对象列表；root 的 parent identity 为 0，全部计数使用 `u64` 页数。增加独立 `QuotaExceeded` 错误；未知 rights 位与非零 reserved 拒绝，不为当前无语义的 flags 制造 ABI 字段。HandleTable 同 generation 的 pinned 槽返回 `ObjectBusy`，generation 不匹配才返回 `StaleHandle`。

Derive 遵循单一发布事务：先解析并强持 parent，预留 sponsor/global permit、parent delegated reservation、child core、输出 Handle 槽与用户结果承诺；parent `reserved → delegated` 是不可逆线性化点，随后把内嵌 parent credit 的 child state 与 parent 强引用共同激活并公开 Handle，Commit 后不分配、不执行可恢复失败。单 Handle 结果发布骨架从 Job 私有实现上收为 Handle 层共用机制。

验证：host debug/release 覆盖零/溢出、父子守恒、并发 reserve、失败 rollback、charge split/merge、child 多引用、深度上限、自然归父、wrong-owner token、forget 只泄漏不扩容；metadata admission 覆盖全局/每 sponsor 耗尽、失败退款和独立对象延长 sponsor 寿命；shared ABI 覆盖布局、reserved、错误码与 rights；kernel/HandleTable 覆盖错误 kind/role/rights、输出 reservation 与 metadata exhaustion-before-Commit、同代 pin 冲突及 child 发布前后的计数快照。切片 2 可落 dormant syscall 与对象实现，但在切片 5 正式交付 root Handle 前不宣称用户态资源闭包已经开放。

切片 2 已实现收口：root seed 启动账本闭合、Pool 四项状态机、双层 metadata sponsor、capability/ABI/rinlib affine owner 与 dormant syscall 均已落地。host debug/release、shared ABI、完整 `just acceptance` 均通过；启动自测实际穿过 kind/role/rights、sponsor exhaustion、Handle 输出预留失败、Prepared rollback、Commit 与最后引用退款，并证明失败前后 root 快照不变。下一自然序进入切片 3 funded frame broker。

## 切片 3：funded frame broker

在 frame inventory 之上建立唯一资金化取得事务：先从 Pool 预留页额度，释放 Pool lock；再从 user FramePool claim 有界 extents，释放 inventory lock；锁外清零；最后把 reservation 不可失败地提交为与 extents 同寿命的 `MemoryCharge`。任一提交前失败按反向 affine owner 自动回滚，不嵌套 Pool、FramePool、AddressSpace 或 heap 锁。

broker 分开提供普通 user-funded claim、从 boot-held 收编为 primordial funded backing、固定 system ticket 三类输入，输出类型不可互换。本片建立新 API 并禁止新增 raw user path；现有调用点先逐一登记迁移归属，在其所属后续切片完成前保留最窄 transitional adapter。selftest、静态内核页表和启动过渡若继续走系统路径，必须由物理分类而非注释宣称其归属；raw runtime API 的最终删除属于切片 10。

验证：host 测试覆盖 quota 失败、物理失败、extent 上限、清零前放弃、commit、split/merge、retire、boot-held adopt 与 system/user 不可混用；故障注入后 Pool 方程与物理供给方程同时恢复。

## 切片 4：空壳进程与 Building 截止

把 ProcessCreate 改为 page-resource-light 的 Building shell，不再创建 root page table，也不消费 Pool Handle；它从可信全局 admission 原子取得固定内部 MetadataSponsor，任何 sponsor/壳/Job/输出 reservation 失败都保持零创建。Process 持稳定 `AddressSpaceState::{Unbound, Bound { ... }}`；Unbound query/kill/abandon/drain 合法，Map/Write/Attach/Start 返回阶段或 readiness 错误。ProcessResources 为 page pool 与未来显式 KernelMemoryBudget 等正交 binding 保留结构位置，不引入万能资源包。

增加 shared/kernel/rinlib/libprocess 的 `ProcessBindMemory(builder, pool)`：在精确 Building 状态取得 operation lease，pin Builder 与 Pool entry，预留 charge/root frame/metadata，锁外构造完整 BoundAddressSpace；发布时只复检 lease 仍有效且 AddressSpace 仍为 Unbound，不因 lease 登记后的 Terminating 截止撤销资格。Bound 发布与 pinned Pool entry 标记为逻辑已消费是同一提交点；后续摘槽、释放 pin、恢复 Builder 或移交 termination 接管者均为不可失败尾段。所有提交前路径保持 HandleTable、Pool、AddressSpace 与 Job 零变化；提交后调用线程消散不能恢复 entry 或放弃尾段。

修正统一 Building admission：`enter_building_op()` 只接受精确 Building，Bind/Map/Write/Grant/Attach 全部登记且登记冻结提交资格；Start 自身登记后只在 `building_ops == 1` 时提交。终止关闭后续登记并等待已有 operation leases，接管它们成功交付的资源；Start 不得越过已有 lease。检查 Handle pin 顺序和同 generation 冲突，避免先消费后才发现 Pool/Builder alias。

验证：ProcessCreate 无页分配；Bind 成功、重复 bind、错误 kind/rights、Builder==Pool slot alias、pin 冲突、Pool/metadata/frame 故障、调用线程消散以及 Bind 与 Start/Kill/Builder close 竞态；对 Map/Write/Grant/Attach 同样验证“登记获胜、截止后拒绝、终止等待”，Start 不越过任一已登记 Building operation。

## 切片 5：root pool 与 bootstrap 同构

启动账本闭合后，以 `root.total == user_supply` 铸造唯一 root core。创建 root Job 和 init 空壳，调用与 syscall 共用的 bind helper 安装 init PoolBinding；再装载 ELF、栈与 StartupBlock。BootPackage boot-held token 按最终用途切分：payload 对应页直接收编为 root-funded immutable backing，不经“先放回库存再重取”的竞态窗口；envelope、initial ELF 源页和不再使用的 padding 在复制/验证终结后回投 free inventory。

向 init StartupBlock 安装指向同一 root core 的管理 Handle以及 root Job/平台 primordial capabilities；Handle 与内部 binding 共享 authority，不复制额度。bootstrap Map/Write/Grant/Attach/Start 走普通 Building readiness 和 AddressSpace seam，只允许 boot-held adopt 作为特殊 backing 来源，不暴露物理地址 syscall。

验证：init 首次映射前 Pool/frame 总量闭合；payload adopt 前后页只出现一次；init 发布后 boot-held 仅含仍有明确 owner 的区间，其余 BootPackage 页均已回投；任一 bootstrap 故障点无双重库存或遗失页；init 可从 root 派生 child 并创建、绑定、启动第一个服务。

## 切片 6：页表与匿名 backing 全面资金化

让 TranslationTree 的 root、中间表、mega split 与未来 replacement 全部持有来源 Pool 的单页 funded frame；页表 Reserve 在 Commit 前准备完整资源，Publish 不分配。进程终止和 Unmap 通过可恢复游标分批 retire；`TableTree::Drop` 只接受已经 drained 的树并释放根。

匿名 `OwnedBacking` 先预留整笔 charge，再由 broker 取得不超过 extent 上限的零态 backing。部分 Unmap 在页边界守恒切分 extents 与 charge；摘出的 slice 随 epoch retire，最后确认后同时退款。Running Map 与 Building ProcessMap 共用 planner、result promise、PTE reservation 与 shootdown。

验证：root/table/backing 页数守恒、页表分配故障零发布、mega split、匿名多 extent、部分 Unmap/Protect、跨 hart stale translation、AddressSpace drain 中断接管、Drop 不递归兜底；重复 map/unmap 后 root Pool 与 FramePool 回到基线。

## 切片 7：公共 MemoryObject 与统一 ObjectView

增加 MemoryObject kind/role、固定宽 Create/Query/Seal ABI 和 rinlib affine wrapper。创建先从当前进程 MetadataSponsor 预留对象壳/backing/view 所需的固定 permits，再从进程绑定池取得固定长度、多 extent、零态 ObjectBacking；对象保存 permits 与 sponsor 强引用，跨进程运输或 creator Dead 不改 sponsor，直到对象真实析构才退款。Query 报告规范化长度与 Mutable/Sealing/Executable。普通 Handle 按 rights duplicate、TRANSIT、GRANT；view 以强引用独立保活对象。

扩展 Running `MemoryMap` 与 Building `ProcessMap` 的来源意图，使 Anonymous 与 MemoryObject 只在 authority/backing reserve 上不同，共用 placement、guard、结果承诺、PTE 和 shootdown。对象 offset/length 必须页对齐且在范围内；R/RW/RX 由 MAP/READ/WRITE/EXECUTE 与对象状态共同限制。公开 mapping 归 Process owner，可精确 Unmap/Protect；Tunnel lease owner 仍不可被普通调用解除。

把单 PA、单 translation 的 adapter 改为按 ObjectBacking extent 投影生成有界 translation 集；ledger 只保存强 `ObjectView { object, offset, length, permit }`，不增加第二张对象映射表。Seal 在对象锁内与 WritePermit 预留线性化，retiring writable view 收到全部远端确认后才减计数；MANAGE authority 不蕴含 MAP/WRITE。

验证：多 extent object 在不同 VA/权限映入多个进程、Handle 先关仍可访问、部分 object Unmap offset 保持且 backing 不切分、rights 拒绝、seal 与 writable permit/remote retire 竞态、对象最终 frame/charge 守恒。

## 切片 8：多页 Tunnel

Connection 改持与 MemoryObject 共用的 ObjectBacking core 和创建池 charge，删除单 `pa/frame` 字段。TunnelCreate 接受长度并返回 Endpoint、Invitation 与规范化映射几何；Attach 从 Connection 读取长度，完整预留多页 ObjectView，成功才消费 Invitation。两端页表成本各由所在进程绑定池支付。

create/attach/close 的 prepared transaction 对全部 translations、write permits、handle reservations、WaitContext 与 Remote Call slots 先 Reserve；Commit 后无普通失败。close retire 验证完整 lease range 和连续对象 offset，不再假设单 fragment/单 permit。Endpoint 不可 TRANSIT/GRANT，Invitation 保持 affine consume-on-success。

验证：最小一页、跨多个物理 extent、容量/extent 上限、VA 冲突、Attach 失败不消费、双端跨 hart close、进程 drain 接管、peer 状态和 Pool/frame/PTE/Handle/permit 总量守恒。

## 切片 9：Runnel v2 与真实消费者

librunnel 改为从 Tunnel 映射几何构造动态 slice；按 `notes/ideas/runnel.md` 实现 128 B RNL2 header、`u64` 游标、动态 capacity、几何 shadow 与既有 Acquire/Release/EOF/Broken/门铃闭环。RNL1 直接删除，不保留双版本分支。

FAL acceptance 至少用一条大于单页的数据流验证跨进程 Open 基础；现有 carryover IPC 压力线扩展为不同页数、物理多 extent、游标回绕模拟、创建/Attach/close/kill 竞态。该负载证明实现，不作为多页设计的合法性门槛。

## 切片 10：收尾

同步 `notes/impls/{mm,ipc,call,startup,task}.md` 与 FAL 实现现状；任何未落地的 KernelMemoryBudget、BufferQueue、pager、文件缓存、MemoryLease 和设备能力只留在 ideas/后续计划，不写成 impl 事实。审核代码中所有 frame 取得、Pool charge、ObjectView、metadata permit 与 Building operation 入口，删除 transitional raw runtime frame API，确保没有旁路。

每片先跑对应 host debug/release 与 `just check`；涉及启动后跑 `just virt` core 快线，涉及寄存器/调用边界后补 `just virt-release`。阶段收尾统一跑 `just acceptance`（debug stress + release core + sifive_u core）；涉及调度域契约再补 `just virt-hetero`/`just virt-nofd`。每条 QEMU 由路线独立硬上限和 workload/业务/reset 锚点判断。最后更新 COMPASS，并在提交完成后生成带实际提交哈希的未来 review 计划。

## 完成标准

- 启动日志与 host 模型能在静止点证明 platform supply、system reserve、root Pool 和 FramePool 总量闭合；
- Job 代码与方向均不含资源配额第二真值，ProcessCreate 不再隐式绑定内存；
- 所有用户可放大的帧分配都能定位到唯一 Pool charge，所有其它帧都能定位到物理隔离的 system/platform 分类；
- `ProcessBindMemory`、Building cutoff、bootstrap 同构在失败和竞态下保持 Handle/Pool/AddressSpace/Job 原子性；
- 过渡 metadata admission 表覆盖全部新增对象，permit 按 core/Builder/Control 等真实寿命分别退款，metadata exhaustion 在任一 Commit 前零副作用失败；
- 公共 MemoryObject、多页 Tunnel 与 Runnel v2 两侧 ABI、实现、文档一致；
- 普通 mapping、lease mapping、seal、close、drain 与跨 hart retire 在失败和竞态下 frame/charge/PTE/Handle/permit 守恒；
- 全验证矩阵 debug/release 通过，COMPASS 下一自然序转入 FAL 剩余面。
