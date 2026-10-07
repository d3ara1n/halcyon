# 内存管理

方向见 [`../ideas/mm.md`](../ideas/mm.md)。当前实现分八层：启动物理供给规划、帧库存、MemoryPool 额度与准入、资金化帧取得 broker、用户地址空间纯逻辑规划器、可 host 测试的页表、内核地址空间与启动协议、现有用户地址空间接入；本篇是内存实现事实的唯一拥有者。

[`vm/`](../../os/kernel/src/vm/mod.rs)由真实Process/Builder/bootstrap/Tunnel消费，用户runtime/监督已接当前契约。页/metadata双维Pool、独立对象、动态保留容量与计划scratch均接实际付款owner；私有容量先释放/退款才发布对应完成屏障。下文描述现有owner与推进路径，内部失败/交错、完整维护原子及最终交付门由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。

## 启动物理供给与系统储备

`os/dtb::memory` 先以 DTSpec status 规则接纳 memory 与 reserved-memory：缺省/`okay` 可用，合法不可用状态跳过，未知或非单一 NUL 字符串拒绝；RAM tuple 经 checked 页对齐、排序与重叠拒绝后才形成固定容量 `PlatformMemory`。动态 reservation 与 `reusable` 在 owner 机制存在前保持 fail closed。

`os/memory_supply` 是 `no_std`、禁止 unsafe、零堆分配的纯逻辑规划器；`os/kernel/src/frame.rs` 在 BSS 中提供固定 workspace。输入只接受板级层已经接纳并页对齐的 managed、permanent 与 boot-held 区间，输出是借用的 `InventoryPlan` 与拥有 typed tickets 的 `SystemSupply`。workspace 在失败后可以被下一次规划覆盖；只有完整 `Ok(Plan)` 才能拆出可发布对象，因此失败不会把部分分类写入全局 FramePool 或 system ticket store。

分类顺序固定为：permanent 先裁进 managed 并归一化；boot-held 同样裁剪后再扣除 permanent；metadata、heap chunks、recovery tickets 依次从剩余 gap 低地址确定性放置；最后的补集才是 user-free。metadata 只要求页对齐且自身连续；每个 heap chunk 独立要求 1 MiB 连续与同尺寸对齐，不要求整个 system reserve 连续。任一输入非法、算术溢出、workspace 容量不足或任一子预算无法完整放置都使启动 fail closed。

固定 range 容量不是平台实测值。令 `M=MAX_MEMORY_REGIONS=16`、`P=MAX_PERMANENT_RESERVATIONS=34`、`B=MAX_BOOT_HOLDS=3`、system ranges 为一段 metadata 加 `H=16` 段 heap 与 `R=0` 段 recovery：permanent 裁剪后最多 `M×P` 段，boot-held 扣除 permanent 后最多 `M×P+M×B` 段，归一化前 unavailable 最多 `2MP+MB+1+H+R` 段，最终 user-free 补集再增加至多 `M` 段。因此 `MAX_CLASSIFIED_RANGES = M+2MP+MB+1+H+R = 1169`；全部数组常驻 BSS，不进入冷启动栈。

当前 system 子预算分别为：FramePool 树 metadata 每个 managed frame 精确 2 字节，再加固定 2048 项 `ArenaMetadata` 后整页上取整；内核 heap 为 16 个各 1 MiB 的 tickets；现有 Commit 后 completion、rollback、drain 与 remote-call 路径不分配物理页，故 recovery 为 0。`SystemSupply` 分离不可变 Range 快照与 `Option<Ticket>` owner，消费后 `heap_ranges`/`recovery_ranges` 仍可查询原规划，`consumed_*` 与 `remaining_*` 独立报告状态，查询不能复活 ticket。后者不是永久假设：任何新增 recovery 页消费者必须先给出并发上界、提高独立 ticket 数并补 exhaustion 测试，不能从普通 heap 或 user inventory 借页。

`FramePoolMetadata` 的区间清零后直接成为 FramePool 两个外置 metadata 切片，并随库存永久保留。全部 `HeapChunkTicket` 在全局 allocator 首次使用前由启动线程预清零；Talc `SystemSource::acquire` 在 HEAP 锁内只从 `SYSTEM_SUPPLY` O(1) pop 一个 ticket，邻接最近arena时extend，否则独立claim。Source保存本实例最近返回的end与已转交ticket映射末端，不resize/truncate、不合并物理空洞；避免把相邻1MiB供给永久分割成无法容纳1MiB请求的独立arena。不进入 FramePool、不扫描、不执行 memset。ticket消费永久归堆，耗尽或真实碎片导致普通 metadata OOM；固定供给不替代对象级 admission。`RecoveryTicket` 与 heap 没有公共消费类型，当前数组容量为零。

FramePool 先把完整 managed RAM 注册为 unavailable，只发布 planner 给出的 user-free。DTB、cold bootstrap 与 BootPackage 等 boot-held 页在 owner 生命周期结束且 transition 映射退休后经 `release_range` 回投同一 user inventory；permanent 与 system 页从不经过该入口。启动日志分别断言：

```text
managed = permanent + boot_held + system + user_free
system  = metadata + heap + recovery
```

## 用户帧库存

物理帧的用户库存由 `os/frame_pool` 的**外置元数据分级 order 树**管理。`frame_pool` 不使用堆且不含 unsafe；内核 adapter 负责全局 POOL 锁、真实帧清零与来源分型的 affine 所有权：普通页由 `ClaimedUserExtent` 与 Pool charge 合成 funded owner，保留内容的启动页由 `BootHeldExtent` 直接持有。

### 结构

每个 DT memory region 先按全局帧号作 canonical power-of-two 分解，形成物理对齐 arena。单个任意区间至多产生两倍地址位宽个 arena；板级最多 16 个 memory region，`MAX_ARENAS = 2048` 由 `16 × 2 × usize::BITS` 推导。arena 不跨 DT 物理缺口，因此任何 order 块天然物理连续并按自身大小对齐。

每个 arena 使用完全二叉树，节点一个 `u8`：`0..` 表示该子树当前可提供的最大 order，`u8::MAX` 表示没有空闲块。整块空闲由父节点直接代表；向下分配时才物化两个子节点，归还时沿祖先即时合并。分配和归还不维护运行期碎片链，也不读取被管理帧内容。

树节点每个托管帧精确占 2 字节；固定 2048 项 `ArenaMetadata` 与树节点同处 planner 交付的 metadata 区间。`FramePool` 自身只持两个外置切片和计数器，不把大数组压入内核栈，也不从自己管理的 user-free 页反向供血。

```rust
pub struct FramePool<'a> {
    /* tree metadata slice, arena metadata slice, counters */
}

pub(crate) struct BootHeldExtent {
    geometry: Option<ExtentGeometry>,
}
```

`FramePool` 不再参数化帧内容后端，claim/return 路径只能读写外置树元数据。内核 adapter 取得 claimed geometry 后立即由私有 `ClaimedUserExtent` 持有，先释放 POOL 锁，再经 `phys_to_virt` 清零完整 extent；broker 在全部 extent 清零后提交 Pool charge 并返回 funded owner。`ExtentGeometry` 可复制但只表达几何，不能复制物理所有权。`BootHeldExtent` 不走普通清零路径，只能在已验证的启动 reservation 移交点以 unsafe adopt 接管保留内容。

### 操作与契约

- `add_managed_region(start, end)`：注册完整 DT memory，初态全部 unavailable；frame/arena/metadata 的目标计数与字节游标全部 checked 预计算，重叠、算术溢出、arena 超限或元数据不足在任何 metadata/arena 写入前失败。
- `release_range(start, end)`：只把 planner 的 user-free 或生命周期结束的 user boot-held 区间发布为空闲；bootstrap、DTB 与 BootPackage prefix 回投走此入口。
- `alloc_order(order)` / `alloc_largest(max_count)`：纯库存 crate 的 order 与最大 extent 原语；内核生产路径以来源 Pool 的 funded owner 取得匿名 backing 与 Tunnel backing，锁外清零后才发布；内核不提供脱离来源 Pool 的普通 raw 分配入口。
- `alloc_at(base, count)`：预验证完整指定区间空闲后，按 canonical blocks 精确取走；失败不改变库存。
- `dealloc(base, count)`：任意区间 canonical 分解后沿祖先合并；重复归还或与现有空闲库存重叠立即触发断言。普通 claim 初始为 power-of-two extent，资金化切分后的 owner 与 BootPackage payload 可持任意长度区间。

库存步骤上界只取决于 `MAX_ARENAS` 与地址位宽；单 extent 归还只沿一棵树上行，任意区间的 canonical block 数同样由地址位宽和 DT region 数限制。清零与返回帧数线性，但在 POOL 锁外执行，也不存在随全局碎片数增长的扫描。

`BootHeldExtent` 不可复制或由安全代码任意构造；只暴露只读几何，`split_at`消费原owner并产生精确相邻owner，Drop结构性有界回投库存。普通funded owner先归还物理页、再退来源Pool charge。用户页表root持完整FundedTableFrame；分支经adopt将唯一物理身份交branch PTE、费用并入空间表页聚合账，摘除时凭该帧号与一页charge重建完整owner。树结构与shared位图保证每个寄存帧只reclaim一次；重建后仍走同一物理→费用退款顺序。摘owner与下一work unit实际归还分开计费，整树退休用有界drain cursor。

## 物理供给验证

FramePool host 用例覆盖库存结构、极端几何、失败原子性、部分重叠归还的修改前拒绝与守恒；7 项 memory_supply debug/release 用例覆盖 permanent/boot 优先级、碎片化独立 chunk 放置、子预算不足、固定容量耗尽、失败 workspace 重规划、用途分型与 ticket 单调消费。`frame::selftest::run` 在 root Pool 建立后、Ready 发布前的启动静止点覆盖正式普通页与表页入口、真实 Pool/FramePool commit、extent-limit rollback、1+3 页切分的两种释放顺序和精确双账本退款。私有 `DirtyInventory` 委托正式 `UserInventory` 取得独占 claim，锁外写脏并读回完整范围，再交给 broker 的真实 clear；成功后逐字节检查所有 extent 为零，不依赖重取相同 PA。四页单 extent 是当前两平台启动供给的 fixture 前置，不是普通分配 ABI 承诺。MemoryPool 自检另验证 child 支付真实表页、funded owner 保活来源 core 与最后 owner 释放后的父级退款；事务先后与失败前不清零的事件证据由 broker host 模型拥有。切分后存活侧读写只验证访问范围，直映射不会随库存归还失效，因此不能单独证明独占所有权；该不变量还由 affine split/Drop 几何和库存测试支持。system heap 自检覆盖 ticket 首次消费。系统储备批次收口时（验收拆档前的 full workload）virt debug/release 与 `sifive_u` 均完成 16/16 acceptance；实测闭包为 virt debug `262144 = 1495 + 9515 + 4236 + 246898`、virt release `262144 = 1308 + 254 + 4236 + 256346`、sifive_u debug `32768 = 1063 + 9515 + 4124 + 18066`，对应 system 子账户分别为 virt `4236 = 140 + 4096 + 0` 与 sifive_u `4124 = 28 + 4096 + 0`。当前阶段收尾由 `just acceptance` 分别执行 debug stress、release core 与 sifive_u core。

## MemoryPool 额度与 metadata 准入

`os/kernel/src/frame.rs` 在平台账本首次发布完成时冻结一次性 `RootPoolSeed`，其页数严格等于当时 `user-free + boot-held`；后续 boot-held 回投只改变物理库存，不改变 root 总额度。heap 可用后，`task/memory_pool.rs` 消费 seed 铸造唯一 root core，并由内核全局 anchor 持有到本次启动结束。启动日志同时打印帧分类和 root 页数，可直接核对这一闭包。

`os/memory_pool::LedgerState` 是 `no_std`、禁止 unsafe 的单维额度状态机；内核的页与 metadata 各持一份，分别恒保持：

```text
total = available + reserved + allocated + delegated
```

charge 与 delegation 使用不同的线性 reservation/credit 类型，所有转换均作 checked arithmetic。每个状态实例另有 crate 内原子铸造、外部不可伪造的 `OwnerKey`；ABI Pool identity 只作稳定诊断身份，即使调用者重复提供相同 identity，token 也不能跨实例提交。child 在 Prepared 阶段消费唯一 delegation reservation；Commit 将 parent `reserved → delegated`，并把 delegated credit 内嵌进 child state。只有消费 fully-available child 才能取出 parent credit，因此不存在 child 仍可继续派生而额度已提前归父的安全 API。深度上限唯一来自 shared ABI 常量，当前为 32。

`task/resources.rs::MetadataCharge` 独占 metadata allocated credit并强持真实付款Pool；`payer()`用于同一对象后续存储政策，不复制财务真值。成本以RV64自然字宽槽计，Arc含控制头、值对齐/padding；内嵌Work/Frame/WaitQueue/节点只计宿主一次，独立Arc/Box/数组各持自己的费用。root初始metadata来自完整system heap几何，扣启动已分配空间与root Pool/Account固定成本；子Pool和独立Account分别由父付款，Account自持charge，不形成自身付款环。值销毁退live槽，外部Weak可能继续保留分配块；这不是堆字节硬隔离或分配成功保证，普通分配始终fallible。region/transaction数组、Job/Views/WaitSet树节点、Pinned Member、Job链临时数组、Operation/账本存储头及全部动态计划scratch已按容量付款；独立对象/Source/Work、Shell/Thread、VM/Views/Backing与真实消息数组也出生付款。固定大小cap发布批次使用栈数组；ProcessWrite/Grant、WaitSet接收和Message输入/运输scratch用MetadataBuffer，按执行者Pool取得费用。具体owner和终态边界由对象、任务与事务章节拥有。

内核 MemoryPool 以 Prepared/Committing/Active 表达表项发布。PoolDerive 请求显式给两维预算与 rights，一维可为零、整体零拒绝；两维在同一 Pool 锁内预约，第二维失败退第一维。Prepared 分配/输出失败退两份预约；锁外必成 commit 一起转 delegation，child 强持 parent。最后 Pool core 引用消散时逐锁归还两份 parent credit；维护 Account 可由执行器继续保活，最后 Account 析构才向父 Account 转剩余维护欠账。自身和 Account 的父付款分别随字段/独立值析构退款。父链深度由同一 ABI 常量限制，不遍历子列表。Query/Derive 使用 Authorization 接受非消费授权并持短 BodyLease；发布采用表预约→锁外输出→锁外必成资金提交→表 commit。Create/Map 等显式来源使用 CREATE，来源表项不因调用被消费。

### 已付款保留存储

[`shared/funded_storage`](../../shared/funded_storage/src/lib.rs) 拥有固定布局的 Buffer/OwnedBox 和唯一付款凭证。Funding 只针对 Layout 返回 charge，不拥有业务身份/授权；内核 MetadataFunding 在领域选定的 Pool 扣槽，无隐式全局源。Buffer固定 `Box<[MaybeUninit<T>]>` 只暴露初始化前缀/有界写入，不隐式增长/缩容；clear/pop只改逻辑内容，存储及 charge继续保留。IntoIter持剩余初始化区间、分配和凭证，部分消费不提前退费。OwnedBox失败返原值，解构先释放Box再退分配charge；into_pin利用固定Box地址稳定，安全Pin接口不能取回!Unpin目标owner，移动pin容器不移动目标，析构先运行值的detach再退款。零字节布局无需分配账。

Buffer.clear对无需Rust析构的类型直接清初始化长度，其他类型仍运行slice析构；显式forget_values按mem::forget语义结束初始化值，保留allocation与付款owner。CapTable在occupied=0（含无Claimed）、Members在count=0且entry=None时忘空前缀，随后bulkfree数组。OrdMap的K为Copy，remove已take所有V，len=0后同样忘空节点前缀，再释放唯一arena和费用。ChangeStorage的普通记录bulkfree不能替代affine许可在取消/退休入口的逐项收束。

增长备已付空块，持领域门复核后 replace_storage 搬移值、不析构/分配/退费；旧空块保持同一种owner，带出高阶锁释放。旧/新来源分别付款，同时保留的峰值必须可负担。CapTable允许HANDLE_TABLE→Pool短边，Members在Job/Shell/Members门外准备/析构，Ready/Backing/VM ledger同样持owner。`ordered_table::OrderedTable<V,C,K>`的AVL链持OwnedBox<Node,C>，prepare显式payer，remove/pop返完整PreparedEntry；旋转/插入/摘除不退款，into_value/into_parts在外层才释放节点。Kernel的Job/Views/WaitSet全为MetadataCharge，未用候选由原操作到锁外清理。Buffer的显式try_reserve/try_reserve_exact要求caller提供Funding，只在门外准备；失败保原块，truncate/clear不退保留容量。操作scratch持同类owner，阶段转换不分配或析构付款。

## 资金化帧取得 broker

`os/funded_frame` 是 `no_std`、禁止 unsafe、零堆分配的事务编排 crate。它不认识 capability、锁或物理地址，只以 `QuotaSource/QuotaReservation` 与 `PhysicalSource/PhysicalClaim` 两组仿射端口执行同一条 `reserve quota → bounded claim → clear → commit` 路径。页数与 extent 数是调用方分别给出的工作边界；当前内核实例以栈上固定数组容纳最多 64 extents，其 debug 帧成本由栈 guard 与 ELF audit 共同约束。全部 extent 取得前不清零，因而库存中途失败、extent 超限或非法 claim 会先析构已取物理 owner，再析构 reservation 回滚额度；全部清零后 quota commit 由类型变为不可失败。切片 6A 增加 `QuotaCredit` 与 `PhysicalClaim::split_at`：`Funded::split_off(&mut self, boundary)` 只在栈上建立后缀 storage，同步切出跨 extent 的物理 owner 与同源 credit，失败保持原 owner；`merge_from(&mut self, &mut donor)` 先验证固定 extent 容量，再合并 credit 并转移几何，成功后 donor 成为空 owner，wrong-owner 或容量失败保持双方不变。两种接口都避免按值回传原 owner，不公开两侧拆包，也不为错误恢复额外内联一份最大 extent 数组。

内核 `task/memory_pool.rs` 的 `PreparedMemoryCharge` 与 `MemoryCharge` 把纯逻辑 `ChargeReservation/AllocatedCredit` 接回来源 Pool core：前者析构回滚 reserved，后者强持 Pool 并在最后析构时退 allocated。`frame.rs` 的 `ClaimedUserExtent` 只在 POOL 锁内摘取 geometry，锁外清零；通用 `FundedFrames` 内部不可见的 `funded_frame::Funded` 按 extents 在前、charge 在后的字段顺序析构，且不提供可提前拆散两侧 owner 的公共入口。AddressSpace 页表帧（root 与中间表同形）使用单页/单 extent 的专用 `FundedTableFrame`，避免把 64-extents 通用存储内联进空壳与 Bind 调用栈。BootPackage 则由不可伪造的 `BootHeldExtent` 表达未入库存的启动 owner，payload 经 root charge 转成可同步 split 的 `BootFundedExtent`；普通 broker 仍无绕过清零的 generic adopt。system tickets 继续由 `SystemSupply` 的独立类型拥有。

AnySpace的root、中间页表、匿名与Tunnel backing使用同一broker来源Pool。root持完整FundedTableFrame，分支的adopt/reclaim由[vm/table.rs](../../os/kernel/src/vm/table.rs)的FrameMemory与空间聚合charge接回实际付款；TableTree持root、分支数量和shared/owned位图，不另存分支owner表。普通片段与BootFundedExtent都进入SegmentMap。Unmap/整空间退休在VM锁内摘取或重建owner，运输fence确认后在锁外按物理→额度退款。
## 分段 backing 与逐段退款

`PhysicalExtent` 区分普通 FundedExtent 与保留内容的 BootFundedExtent；两者都提供守恒切分、物理投影和同源退款，SegmentMap 与 View 生命周期不按来源分叉。

[memory_space::SegmentMap](../../os/memory_space/src/backing.rs) 按逻辑页起点索引物理/资金 owner，段保存 end 与覆盖 use 数。NodeReserve 在准备期预取节点；acquire 先验证连续覆盖/计数/端点额度，再切段增 use。RangeUse 不可复制，split_use 至多物化一个内部切界且不改变段 use 数；失败返原 token，append 失败返物理 owner，错误 map 的 begin_release 也返原 use。begin_retire 只关新 acquire/append，已有 use 继续拆分或释放。

每次release_step处理一个段，仅零use段移交真实owner。segment_at只在map同步域内借owner，跨锁须RangeUse。OrdMapStorage只有一份固定节点Buffer，空节点value=None，其闲置left保存下一空槽；free_head定位空链，空槽数由高水位减live len推导。reserved单独表达尚未兑现的预约。增长保持索引和空链，候选不足不接受新额度；换块后的旧空owner返外层退款。切段用reserve_recycled复用刚摘出的槽，insert/remove/切分提交均不分配。

[vm/backing.rs](../../os/kernel/src/vm/backing.rs)把funded frames分为FundedExtent，核对Pool与charge同源；SegmentOwner切分同步拆物理claim/credit，不退款、不取Pool锁。BACKING锁在ADDRESS_SPACE/VM_VIEWS之后。初始索引由创建Pool付款，增长由触发操作Pool付款；锁外备单arena、锁内复核/换块/预约，旧块锁外释放。最终段与预约均空时关闭准入、摘空存储，容量退款后才推进完成；观察Arc继续存活不推迟退款。物理解析只返回一个连续段。

完整域 BackingOwner 独占 root use，public MemoryObject/Connection 放弃它时，原位关 acquire 准入并把 root 游标交给 backing 出生内嵌的唯一 Work。每步摘一段，锁外放物理/退 credit；仍被 View 覆盖的段保留给 View 自己的责任。创建失败尚无外部 use 时，释放原 funding 批，其 extent 数已有结构上界。

Backing::fund 拥有匿名/MemoryObject/bootstrap 的 funding 准备；固定 64 extent 批次只存在于该调用链，不随大 VM 请求准备值传播。普通 BackingUse 只有 Arc<Backing>/RangeUse，不包含 Work、完成观察或等待锁。它由准备/事务/View owner 显式接管；retire 产生普通值型 BackingRelease，由已有事务/空间 Work 每次调用 step 处理一段，锁外退款后返回进展。完成后才允许销毁游标；直接丢弃尚有责任的 use/游标触发结构断言。BackingSplit 仅预取一个段节点，切分既有 use 不再分配记录。

## 实物 View 索引与退休组合

[vm/views.rs](../../os/kernel/src/vm/views.rs) 拥有每空间真实 use 和写许可来源；MemorySpace 拥有几何/权限/预约。两者不是可互相替换的副本，实物索引允许更细的等价切界。Body 出生 Views，ChangeWork/RetireWork 直接使用它，没有 RetireSink callback 或子退款执行器。

PreparedView在ledger预约保护下锁外预付View与必要ObjectSource节点；本操作已持写许可时，permits保证同来源稳定存在，prepare先在Views锁外取ObjectId，再锁内核实来源，省掉同来源候选。RO Map仍备可插入节点，不能用一次查表快照省略。commit只移出必要节点，落败候选与取消的use由Operation带到锁外清理。ViewBoundary在VM锁外备索引/物理切段，提交门复检两端；缺同源候选保持ledger原样。已成端点或实物消失不消费候选，等价切界失败后无需合并。hold_writes复用已有来源，缺失才锁外付节点再复检；最后来源节点摘出后锁外退款。

ViewRetirement 同时保存精确 next 和当前 BackingRelease：每步取 next 对应的一项或推进一个物理段，不用“找不到条目”猜完成。游标结束后才取下一项、归还相应 WritePermit。父 Work 在自己的预算中完成退款，不登记 backing 子任务、不因 backing 释放而 Blocked。

ObjectSource 按 ObjectId 强持 MemoryObjectCore，分别记录实物 view 和本空间 permit 份数；两者都为零才摘来源。只读 View 也保留原授权状态，使原 maximum=RW 的映射降权后可按相同对象状态重新申请写许可。切界增 view 份数，退休减 view 份数；permit 取消/退休在锁外调用来源。全对象计数仍只由 MemoryObjectState 拥有。对象适配见 [memory-object.md](memory-object.md)。

ChangeWork/RetireWork把阶段移到唯一执行owner后在父状态锁外推进，Advancing防止观察者误判完成；两个独立Arc出生付款，末字段[`completion.rs`](../../os/kernel/src/vm/completion.rs)承接最后退款回执。全部先行私有字段析构、宿主费归还后才交回访问lease并交付完成。整空间须确认Views/许可索引空、根表和charge释放；父回收不强持待完成的RetireWork，激活后Body只保留VM/Views。Views Drop禁止无界清空尚有资源的索引。普通事务与永久封口保留各自协议。

公开 MemoryMap/MemoryUnmap/MemoryProtect 由 [vm/operation.rs](../../os/kernel/src/vm/operation.rs) 接入：首份资源前出生 Operation/ChangeWork 和空间访问 lease，收集来源许可、ledger 预约、实物 use/切界节点、PTE 游标与表帧。未提交 Drop 把 Preparation 移交原记录 Rollback，逐项取消/退款；未挂入Views的许可用object+后缀起点标识，先直接取消后缀再经Views取消已挂入部分，所有几何/token/实际容量持续由同一失败根持有；部分取得的表帧直接存进操作，不在返回错误时集中析构。Map 在来源 capability 接管期间取得独立 use，按真实连续 PPN 分段构造 Install，不把整个 backing 当连续。

Map 的 guard 参数是字节数、向上按页取整；结果槽由 UserWriteLease 预约。当前调用线程在 VM 锁内先经 SUM 写 payload 前缀，再以对齐 u64 release store 发布 committed cookie；之后 Views 安装、ledger Commit 和记录发布必成，不再异步写用户缓冲。成功调用通过已有单来源线程续体等待私有 DONE 并返回单位值，Caller 冻结只撤回复权。尚未提交的错误由原记录继续回滚，不依赖调用者重试。

修改已有范围时，先在 VM 锁外准备两端实物候选与已付款 ledger 存储；回 VM 门核对真实容量、范围预约和两端候选，再先切实物、后切 ledger 并捕获快照。纯容量竞争退出门重采样，几何/来源变动则拒绝，ledger 原样。后续失败只能留下比几何更细的等价实物切界，不能让 ledger 比实物更细而破坏 drain 的精确游标。回滚或数据退休后还负责最后空表；Process/Builder/bootstrap/Tunnel 已消费同一空间/backing；完整预算由[对象链工作记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)承接。

## 空表的最后离开者

普通修改在叶 PTE fence 后释放旧 use/许可及未用表帧，但保留自身预约。TableCleanup 自底向上按预算扫描范围内部，忽略本事务而保护其他预约；固定批次满就发布新的 epoch/fence，确认并归还全部运输槽后，锁外释放该批表帧，再继续扫描。内部扫描不靠只检查两个端点代替。

最终边界复查和 ledger complete 在同一 VM 临界区完成，固定批次可容纳两端的所有层级。这样两个相邻事务不能先各自跳过对方、再分别退出而漏掉空表。最低层不存在时仍检查更高层，不把“不存在”与“非空”混为停止条件。被摘边界表同样保持到额外 fence 完成，操作 DONE 晚于真实表帧/credit 归还。

准备失败的 Rollback 也在撤预约的同一临界区复查边界；若摘到了表，原 ChangeWork 先 fence 再退表，不增设清理执行器。结果 lease 对应的页仍实际映射且在事务期间禁止失效，因此不会成为另一个无 PTE 的空表责任。保持预约期间，新大页映射不能抢占尚未处理的空分支；放行前该分支已摘或由相邻真实映射持有。

## 页表模式选择

新 AnySpace 出生会核对 funded root 属于其付款 Pool、ledger 不跨用户地址域，并通过 mm::install_kernel_top_level 安装内核 direct-map/stack-window 的共享根项。TableTree 将这些根标记为借用，用户页表整空间 drain 不回收内核下级表；独立根表本体与 charge 在完成屏障之后释放。

单模式（全系统同一 satp 模式）是共享内核映射的结构性上限而非妥协：内存物理上同一份，异模式 hart 各持平行映射树，反而割裂调度域（进程不可跨模式迁移）——多模式无收益。**当前实现固定 Sv39**（`os/kernel/src/mm.rs` 与上下文汇编按三级页表组装）；启动期按 DTB `mmu-type` 自动选式是后续工作——需要先完成模式常量贯穿（satp 组装、地址宽度断言）再打开选式，且选式应取全体 Application hart 的最小硬件上限与内核支持集的交集，不支持交集则拒绝启动。

内核支持集由编译期决定，当前只有 Sv39。

## 页表纯逻辑（os/page_table crate）

`os/page_table` 是 `no_std + alloc`、禁止 unsafe 的独立 crate，host 与内核 target 复用同一份代码。页表树 const 泛型于 `LEVELS`（3=Sv39、4=Sv48、5=Sv57），PTE 编码、VA 宽度与各层覆盖页数都由级数推导。页表逻辑不直接解引用物理地址；启动期未发布树与运行期 owner-aware 树使用两条明确 seam：`EagerFrameMemory` 可在构造中即时取得裸表号，`TableFrameMemory` 只投影调用方已取得的 `TableFrameOwner` 并访问表内容，不认识 Pool 或分配器。

运行期TableTree持完整root owner、分支表数量及shared/owned root位图。中间表通过TableFrameMemory::adopt把物理身份寄存在branch PTE；内核FrameMemory把费用并入空间聚合charge，reclaim按已摘帧号拆出一页charge并重建FundedTableFrame。所有新表页在取得owner前已清零；PTE的唯一父边与shared位图共同保证分支只取回一次。

### 类型与事务边界

- FrameNumber/Vpn/Ppn表达页号；Pte集中编码V/R/W/X/U/G/A/D及leaf/branch。PublishCursor构造先拒绝越界与非法叶flags。
- TableTree不拥有叶数据backing，attach_shared_root只标记借用子树。root与分支采用上述完整owner/寄存协议。
- PublishCursor是唯一运行期Map/Unmap/Protect游标，调用方持足额预付FrameOwner迭代器；publish_step只借当前树推进，不保存可被相邻事务失效的准备期树快照。

### 分步发布与空表退休

Map按最大兼容mega段推进，缺失路径从预付迭代器取表页；Unmap/Protect部分覆盖mega时逐级拆分，子项保持原物理连续性与flags。每次publish_step按budget限制槽工作，不分配、不进入帧来源。worst_case_frames只依赖请求几何与LEVELS：Map统计各级触及表，Unmap/Protect为两端各一条拆分链。Sv39后两者保守预取四页，普通4KiB Unmap同样适用，不把当前树快照的零页需求当跨事务保证。


Unmap发布只清叶，不同步摘空表。上层ChangeWork按最后离开者协议分批调用detach_empty_table，在本事务预约保护下协调相邻预约；已摘表owner保活到remote fence确认与运输槽归还后再锁外退款。该协议与普通叶发布共用同一TableTree，末端预约撤销与边界复查在同一VM临界区完成。

整空间退休用DrainCursor保存固定宽层级遍历状态；step_drain至多摘一个owned分支，锁外退款后再推进，shared子树不进入。分支数归零且owned位图空后finish_drain才交root；TableTree::Drop只接受已drain终态，不遍历PTE。Building/bootstrap、Running与Endpoint来源Map共用此运行期协议；EagerMapper只构造尚未发布的内核永久树。

### 测试集（host）

host用例直接驱动生产PublishCursor/publish_step，覆盖跨表与mega选择/细化、跨子表Unmap、Protect、非法flags/越界、shared root拒绝、相邻发布交错、空表摘除与fence前owner保活、shared/owned根转换及退款。独立drain用例覆盖max_work=1、shared子树不回收、未drain Drop拒绝与常数终态。测试不依赖已删除的代次/preflight/PreparedTranslation协议，也不把测试实际用帧数当生产最坏供帧界。运行证据由所属工作记录拥有。

## 用户地址空间纯逻辑规划器（os/memory_space crate）

`os/memory_space` 是 `no_std + alloc`、禁止 unsafe 的内部规划模块，使用 `funded_storage` 取得类型化容量 owner，不认识内核付款政策。它拥有页对齐半开区间、区域账本、backing view、权限、owner、事务阶段和 MemoryObject 写许可状态，不访问页表、用户指针、hart 或内核对象。内核 AnySpace 把该 planner 与 funded TableTree、实物 Views/Backing 组合；Building/bootstrap、Running 与对象 lease 共用该 seam。`MemorySpace<C>` 的泛型仅为存储 charge，对象 region 仍只存 ObjectId/offset，物理与资金 owner 由 SegmentMap/内核 Backing 持有。

`MemorySpace<C>`构造时按Limits支付region单节点arena与固定transaction Buffer；max_regions/max_transactions是逻辑准入真值。换块期间旧/新节点与预约同时存活，候选按实际需求锁外准备、锁内复核，不能以逻辑上限或clear代替存储成本。Reserve失败返原ValidatedChange，借出的WritePermit Buffer不变；Operation锁外补RegionStorage后重试。fragment持唯一RegionKey，同次Map的guard/mapping共享AllocationKey；owner、backing identity及当前/最大权限只由ledger拥有。Anywhere取完整first-fit空洞，FixedEmpty不覆盖旧区域；Unmap要求连续覆盖与同owner，Protect只合并allocation/backing/权限一致且不含writable对象的相邻片段。整空间drain清空值/事务后，AnySpace锁外退款私有容量再完成。

`ChangeRequest` 由调用者稳定持有；`validation_capacity` 只采样目标几何、guard、实际 lease 投影和可写对象模板数，不预约或 mint。调用者在领域门外给 Funding 准备 `ChangeStorage<C>`：一份付费 OwnedBox 存储头持全部 typed Buffer，空候选不分配。Validate 在同一账本借用下复检容量并填槽，成功把存储头移入 `ValidatedChange<C>`；失败不取走 owner。coverage 借不可变 OrdMap 的首/末 key 与数量迭代，没有 key Vec；模板原地排序/压缩，无归一化副本。普通 remove 兼作身份快照，no-op Protect 保留独立快照，结果 lease 直接按 segments 的 RegionKey/几何/RW 复核。

阶段 owner 为 `ValidatedChange<C> → PreparedChange<C> → CommittedChange<C> → RetiringChange<C>`，始终移动同一存储头。已验证 token 在门外按归并后的实际模板/退片段/旧许可数准备物化数组；Reserve 最终复核 snapshot、冲突、permit multiset、身份序列和 arena 余量，全部足额后仅填槽/mint。供给许可数组始终由 caller 持有，成功把许可逐份移入 Region，错误保持 caller 数组和原 validated owner。Commit 只不可失败地交付责任，ledger 由 commit_step 分批发布；synchronize 的单一 RetiringChange 同时持同步证明与旧片段/许可，逐单位移出退休。complete 要求两类退休 owner 均空，注销事务并返回存储头供门外释放；rollback 撤预约后亦返回存储头，caller 按 RollbackStep 一次移出一个 Region/可选许可取消，再释放实物容量。完成/回滚返回的存储在 affine 元素已收束后可复用，清旧普通退休描述，不重复退休。

Views/Backing 按真实 fragment 和 permit 责任处理实物；ledger 不另维护 ObjectRegionDelta 或第二个退休批次。实际 PTE 储备与游标在提交前准备并付费，发布只借调用者 FrameOwner Iterator，不要求特定容器或进入帧来源。存储头、所有数组与旧/新换块峰值各按实际 layout/capacity 付款，空但保留的块不提前退款。

`UserWriteLease<'a>` 是 PreparedChange 内结果投影的借用，非页对齐地址区间的 writable segments 和 RegionKey 由同一付费存储头持有；事务记录保留结果范围至 rollback 或 complete，冲突事务返回 Busy。MemoryObjectState 单向 Mutable → Sealing → Executable：新写授权只在 Mutable 接受，既有 writable 区间的切分/缩小允许 replacement；总许可数仍为唯一真值。对象方法借 caller 门外预付的输出 Buffer，计数/容量全部检查后才追加许可，不在对象门内分配临时数组。取消或 TLB 同步后的退休逐份归还许可，最后一份退出后完成 seal，无人等待也不回退。公共 EXECUTABLE 电平由 [memory-object.md](memory-object.md) 的对象适配拥有。

planner 用例覆盖地址/guard/权限、预约、切界、对象授权、逐片段退休以及只忽略自身的空表末项检查。页表用例覆盖 owner 投影、分步发布与 drain；运输/维护的跨 hart 组合不由纯层测试代替。旧 work_debt 表与对应消费者已删除。
## 内核地址空间与启动协议

### 链接与线性偏移

内核镜像 VMA = PA + `KERNEL_VA_BASE`（`0xFFFFFFC0_00000000`），LMA = PA（链接脚本 `AT()`）。单一偏移覆盖镜像与全物理直映射：

```rust
pub fn phys_to_virt(pa: usize) -> usize { pa + KERNEL_VA_BASE }
pub fn virt_to_phys(va: usize) -> usize { va - KERNEL_VA_BASE }
```

直映射的物理定义域由 `BoardInfo::direct_map_regions()` 给出：当前板级政策取 `[0, max(DRAM 末))`，再扣除 Devicetree 静态 `/reserved-memory/no-map` 区间，因此仍覆盖首段 MMIO，但允许任意页对齐洞。`page_table::EagerMapper` 对每个 admitted range 自动选取 1GiB、2MiB 或 4KiB 中最大的合法叶；中间表来自 `mm.rs` 的固定静态 arena（上限按平台 reservation 容量推导），不进入 FramePool、系统堆或用户供给。`phys_to_virt` 在 debug 构建同时检查地址属于已发布直映射定义域。当前内核虚拟空间仍只有直映射区与栈窗口两个分区。

### 栈窗口

正式内核栈的专用虚拟分区：高半区顶 vpn2 槽（链接脚本 `STACK_WINDOW_VA_BASE = 0xFFFFFFFFC0000000`，与直映射解耦——直映射槽数上限 255，满配也只到 510，与顶槽结构性互斥）。目的：栈向下溢出立即 store page fault，溢出即时可见（对照 `plans/DEBUG-PLAYBOOK.md` 的静默踩踏事故；构建期兑底见 os/tools/audit_elf.py）。

- **布局真值链**：`os/stack_layout` 纯逻辑 crate 是几何唯一真值（构造期整体校验，host 可测）；数字只写在链接脚本（`STACK_SIZE`/`STACK_GUARD`/`EMERGENCY_SIZE`/`HART_NUM_LIMIT`/窗口基址）→ 汇编 `_ENTRY_CONSTS` 物化 → 内核 `mm::stack_layout()` 构造消费；audit_elf.py 从 ELF 符号表读 `STACK_GUARD` 构建期强制「单函数最大帧 ≤ guard 洞跨度」——否则一次 sp 下调整体越过洞落入邻槽，即时可见失效。
- **布局**：每槽 `[槽底 guard | formal (stack_size − emergency) | emergency guard | emergency]`，步长 `stack_size + 2×guard`。formal sp 从 emergency guard 洞下方起；emergency 占槽顶、fatal 路径专用，独立 guard 使其溢出不再踩入 formal。物理侧按槽连续打包 `stack_size` 字节（formal+emergency 相邻），guard 纯虚拟不占帧——这是 Linux `CONFIG_VMAP_STACK` 同构：物理页同时存在于直映射别名中，但内核只经 sp/窗口 VA 引用栈，**禁止经 phys_to_virt 触碰栈内存**；该禁律由 debug 断言兜底（`phys_to_virt` 拒绝栈物理打包区，release 构建无检查）。qemu/virt 与 sifive_u 每槽均为 `0x40000`（256KiB，含 4KiB emergency）；8 槽占 2MiB，sifive_u 的 128MiB DRAM 成本为 1.56%。guard 洞为 `0x3000`，ELF audit 默认直接从 ELF 的 `STACK_GUARD` 派生 12KiB 上限，不保留独立的经验阈值或未经论证的扣减余量。显式 `--max-frame` 只能收紧、不能超过 guard。定长 `MAX_FUNDED_EXTENTS` 容器只作一次性 funding 事务结果、不进入常驻对象；调用链累计栈占用与单帧跳跃是不同约束，两平台运行栈仍由 guard fault 与 acceptance 共同验证。
- **建表**：mm init 内、satp 发布前，静态子表（1 中间 + 若干叶表，不入帧池）按 `layout.mappings` 逐页映射（RW、不可执行——`flags::KERNEL_STACK` 无 X）、guard 洞置 invalid；所有 hart 与全部用户表共享同一子树。
- **地址转换**：`virt_to_phys` 是全函数（直映射线性算术 + `layout.translate` 互逆）；`phys_to_virt` 只对 admitted direct-map range 成立，并在 debug 构建拒绝 `no-map`、范围外地址和栈物理打包区。同一栈物理页同时有直映射别名与窗口 VA，PA→VA 无唯一逆；SBI ecall 传 PA 前仍须经 `virt_to_phys`（console 缓冲在栈上即依赖此）。
- **用户表拷贝**：栈窗口槽随直映射槽一起拷入用户 root（trap 在用户 satp 下即取调度栈指针）；整空间自动退休在有界 Root 阶段逐槽剥离共享顶层项后才归还 root，`AddressSpace::drop` 只作未完成构造/回滚的防御兜底。
- **bootstrap 边界**：cold-bootstrap 使用链接脚本固定的 32KiB 临时栈；它没有 formal/emergency 双窗口与独立 guard，但过渡表只按 4KiB 映射启动实际需要的内核镜像与 DTB 页，不再用 1GiB 叶掩盖栈越界或 `no-map`。全员 Online 后先撤销 bootstrap 临时叶再整体回投该区，单函数帧仍受同一 ELF audit 上限约束。

### 启动：PA 执行 → 高半区

QEMU 以 raw binary 引导（`-kernel` 加载 ELF 会按 VMA 估算内核末端，高半区 VMA 直接溢出 DRAM；raw bin 由 `riscv64-elf-objcopy -O binary` 按 LMA 生成，ELF 仅供 gdb/符号），并通过 `-dtb` 交付仓库自备 DTS；`-m` 必须与该 DTS 的 `/memory` 容量一致，否则 firmware 会按另一物理上界重定位运行时 DTB，破坏 boot-held RAM 契约。`_start` 在 **bare satp、PC=PA** 下执行：

1. `_start` 及开 MMU 前的全部代码与位置无关纪律：`la` 取到的是 VMA，访问 PA 需减链接期常量 `_va_pa_delta`（镜像 VMA 基 - LMA 基）；
2. cold-bootstrap 清零永久 transition root/middle/leaf arena，以 4KiB 叶精确建立内核物理镜像和 DTB 实际页面的 identity/高半区别名；leaf 数量由链接期内核跨度断言和 2MiB DTB 上限共同约束；
3. 写入临时 satp、`sfence.vma` 后跳到高半区；
4. 高半区继续段把板级 admitted ranges 交给 eager mapper 构建正式内核页表，写正式 satp 并再次 `sfence.vma`，此后内核恒在高半区执行。

**地址纪律**：raw 引导无 ELF 加载器清 bss，各空间 bss 由对应入口汇编清零；SBI ecall 的地址参数（DBCN base_addr、HSM start_addr）一律传 PA，内核指针必须先 `virt_to_phys`；正式内核表无低半区 identity 映射，切表后一切受准入 PA 访问必须经 `phys_to_virt`，`no-map` 区只能由未来明确 owner 的专用映射机制访问。裸 PA 直访只存在于 bootstrap 与永久 secondary PA 前导。

### secondary hart

HSM 唤醒入口是永久无栈 PA 前导：从 record PA 取得同一张精确 transition 表，按“过渡 satp→`sfence.vma`→高半区→正式 satp→`sfence.vma`”进入 formal entry。DTB 消费后、bootstrap 全员 Online 后分别先撤销相应临时叶再回投物理页；此后 transition 表只覆盖永久 entry 设施与正式内核页，不留宽 DRAM 映射或已回投页别名。完整生命周期见 [`execution-context.md`](execution-context.md)。

### trap

任意用户页表共享内核高半区与栈窗口，用户trap不切satp；stvec恒指共同入口，sscratch存本hart陷阱锚，稳态SUM=0。U态page-fault（instruction/load/store 12/13/15）由[vm/fault.rs](../../os/kernel/src/vm/fault.rs)按R/W/X目标判定：Mapping用当前权限，Committed整个发布窗口用唯一ChangeKey与目标权限，Map guard不属于可访问承诺。未允许/未映射/其他异常正常终止进程。

目标允许且软件PTE已有U及对应访问位时，锁内SFENCE，取指另FENCE.I，重试原指令；A/D由内核pte_flags预置，fence不承担补位。Thread执行owner保存事实签名及已修复dense-hart位图，同组事实每hart只修复一次，迁移不能抹掉已尝试历史。目标允许但尚无相应PTE时，捕获既有事务Completion来源，锁外沿原Wait epoch协议停驻至DONE。RetryFault不写返回寄存器、不加sepc；来源失败保留原cause/address，Frozen复用detach/退出。

VmState出生预付的dependencies Buffer容量等于ledger.max_transactions；纯层reservation与ChangeKey→SignalSource登记同锁发生，rollback/complete释放预约时同锁摘索引，source在门外释放。索引只持独立完成来源，不持被等待的ChangeWork/RetireWork；SignalSource自身的通知Work与它们没有自持环。先前等待方仍持原source至Completion最后退款/access回执后DONE。access归零须索引空；Body最终销毁空buffer再发布完成。

### 用户内存访问

[`uaccess.rs`](../../os/kernel/src/uaccess.rs)的直接拷贝接收AnySpace，调用者持VM锁；统一prepare_direct_access先用check_range逐页translate核溢出、上界与U/R/W，再执行本地全量SFENCE.VMA，最后进入SumGuard直访用户VA。读、写与cookie发布共用该入口，不用尚未随每个PTE批次前进的epoch猜本地TLB已同步。空拷贝无访问，非空用RV64 LB/SB，不为合法VA 0构造Rust引用。cookie用对齐SD与FENCE RW,W作release发布。Building跨空间初始化先translate，再经物理直映射写入；单次长度界保持原契约。

安全定宽 `write_user` 仅开放显式 ABI 输出类型白名单，其完整对象表示必须无 padding；任意 `T` 的输入/输出 helper 为 unsafe，由调用者论证位型有效与对象表示可读。输出失败只在 [`Thread`](../../os/kernel/src/task/process/thread.rs) 保存首次 UserAccess fault；trap 的 syscall 业务锁全部退出后调用 `publish_output_fault`，再经 Shell 冻结/VM 封口，不在空间锁内重入终止路径。所有现存用户访问使用 AnySpace，初始映像的 put_initial 只在 Building VM lease 内允许填充最终 RO/RX 页。

## 用户地址空间

Body强持已付款VmShell、Views和出生待激活RetireWork；后者在激活时把唯一保留根交执行器，父回收只观察独立完成事实。VmShell锁拥有AnySpace、entry/access/Building门、active集合、epoch和访问租约。AnySpace ledger是VA真值，TableTree是物理PTE/root owner，Views是仿射backing use/对象许可索引。门控制的执行owner先取稳定satp/epoch，锁外本地同步，回门复检后才记active；切回supervisor satp后退出active。RunPermit先交回VM强根，再发布Shell运行计数归零；VmAccessLease在同VM门内先交回本强根再发布access计数归零。父Body/退休责任在非零计数时保活VM，同门复检使quiescent也证明贷款强根已交回，不能只靠先减计数、稍后drop字段。

低半区由 PROCESS_USER_TOP 定义；空空间出生就付 root，挂入内核 shared 高半区项，没有 Unbound/BindMemory。ELF、块与栈只在 Building 准入内建立；公开 stack/heap 均为普通 Map。ASID 恒 0，切换与修改保守使用全量 sfence.vma；取指变更另 fence.i。几何规划、页表发布和实物索引由同一 VM 锁协调，退款与对象状态锁在外。

- MemoryMap 请求的来源零值为匿名，否则是 MemoryObject/Endpoint authority；目标权限按 READ/WRITE/EXECUTE 校验，Endpoint 无 EXECUTE。Map 结果 cookie 最后 release 发布，之后无可失败用户写回。Guard 单位为字节。
- MemoryUnmap/Protect 预约精确范围，先准备物化实物边界，再细分 ledger；来源许可与退款节点在提交前取得。已提交内容由 ChangeWork 分批发布/fence、退片段和空表，不要求调用者活着。RO View 保留其对象授权来源，合法再升权仍经对象状态校验。
- Map 的公开入口、ProcessMap 与 bootstrap 共用 Operation。在首份资源前出生 ChangeWork；失败 Drop 把准备包移交该记录 Rollback，范围/表页/use/permit 都有归还 owner。成功直接由同一记录发布与退休，不为每 use 再出生子 Work。
- ProcessStart 关闭 Building 修改，启动记录等访问租约与 VM 操作清零后开放 entry；整空间退休等入口封闭、active、访问租约，再逐区域/实物 use/表帧释放。最终根表与实际资金结算后才可释放 Body。
- Tunnel Close/Attach 不改 VA/PTE。Connection root 与普通 Endpoint View 由 BackingUse 独立保活；映射使用 MappedRegion token 显式 Unmap，进程退出使用相同 RetireWork 收束。
- libkernel MappedRegion 保存完整 reservation 与可选 usable 区间，部分 Unmap 返回左右残片；libruntime allocator 与 libthread 栈是其实际消费者，退出等 ThreadControl DONE 后才允许释放栈。

### 事务完成责任

Operation的Prep以PreparationPlan持candidate/validated/reserved唯一owner和已付PTE游标/储备、boundary、caller许可数组、来源use。Prepared View失败返use，不另造补偿Work。ChangeWork完成数据退休后在VM门复查末项、释放范围预约；已摘空表仍经附加fence/退款。Outcome::Done只结束推进，最后执行/发布/wake强根消散后，末字段才退宿主费、归还访问资格并发布DONE。Operation.submit发布前取得返回的观察来源，避免完成并销毁先于取得来源。回滚同样在撤预约前复查边界，不遗漏上层空分支。RetireWork最后退款只提交Shell来源的内部完成事实；Body销毁后父回收再发布Process DONE。独立观察来源可由真实观察loan继续付费存活，不保活私有执行宿主。

vm::remote 使用每 hart 的侵入式 FIFO 收件链与内嵌 Batch，发布前由操作取得全体目标 slots。ack 和槽归还都完成后才允许相关帧退款；最后 ack 用预先 arm 的 WakeToken 唤醒原 Work。每 hart LocalEpoch 只由所属 hart 更新，属于运输与执行门的同步事实，不是 active 集合。来源 Hint 与 DONE 只促使重试，结果通过普通等待续体交付寄存器值；没有 ThreadResultObligation、WaitContext 或异步用户输出指针。

## 架构边界

admitted hart、无 MMU hart 与 AMP 边界见 [`execution-context.md`](execution-context.md)。当前内核与用户目标均为 RV64；地址运算使用 usize，外部线协议才使用固定宽编码。
