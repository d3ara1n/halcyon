# 内存管理

方向见 [`../ideas/mm.md`](../ideas/mm.md)。当前实现分八层：启动物理供给规划、帧库存、MemoryPool 额度与准入、资金化帧取得 broker、用户地址空间纯逻辑规划器、可 host 测试的页表、内核地址空间与启动协议、现有用户地址空间接入；本篇是内存实现事实的唯一拥有者。

## 启动物理供给与系统储备

`os/memory_supply` 是 `no_std`、禁止 unsafe、零堆分配的纯逻辑规划器；`os/kernel/src/frame.rs` 在 BSS 中提供固定 workspace。输入只接受板级层已经接纳并页对齐的 managed、permanent 与 boot-held 区间，输出是借用的 `InventoryPlan` 与拥有 typed tickets 的 `SystemSupply`。workspace 在失败后可以被下一次规划覆盖；只有完整 `Ok(Plan)` 才能拆出可发布对象，因此失败不会把部分分类写入全局 FramePool 或 system ticket store。

分类顺序固定为：permanent 先裁进 managed 并归一化；boot-held 同样裁剪后再扣除 permanent；metadata、heap chunks、recovery tickets 依次从剩余 gap 低地址确定性放置；最后的补集才是 user-free。metadata 只要求页对齐且自身连续；每个 heap chunk 独立要求 1 MiB 连续与同尺寸对齐，不要求整个 system reserve 连续。任一输入非法、算术溢出、workspace 容量不足或任一子预算无法完整放置都使启动 fail closed。

固定 range 容量不是平台实测值。令 `M=MAX_MEMORY_REGIONS=16`、`P=MAX_PERMANENT_RESERVATIONS=34`、`B=MAX_BOOT_HOLDS=3`、system ranges 为一段 metadata 加 `H=16` 段 heap 与 `R=0` 段 recovery：permanent 裁剪后最多 `M×P` 段，boot-held 扣除 permanent 后最多 `M×P+M×B` 段，归一化前 unavailable 最多 `2MP+MB+1+H+R` 段，最终 user-free 补集再增加至多 `M` 段。因此 `MAX_CLASSIFIED_RANGES = M+2MP+MB+1+H+R = 1169`；全部数组常驻 BSS，不进入冷启动栈。

当前 system 子预算分别为：FramePool 树 metadata 每个 managed frame 精确 2 字节，再加固定 2048 项 `ArenaMetadata` 后整页上取整；内核 heap 为 16 个各 1 MiB 的 tickets；现有 Commit 后 completion、rollback、drain 与 remote-call 路径不分配物理页，故 recovery 为 0。后者不是永久假设：任何新增 recovery 页消费者必须先给出并发上界、提高独立 ticket 数并补 exhaustion 测试，不能从普通 heap 或 user inventory 借页。

`FramePoolMetadata` 的区间清零后直接成为 FramePool 两个外置 metadata 切片，并随库存永久保留。全部 `HeapChunkTicket` 在全局 allocator 首次使用前由启动线程预清零；Talc `SystemSource::acquire` 在 HEAP 锁内只从 `SYSTEM_SUPPLY` O(1) pop 一个 ticket 并 claim，不进入 FramePool、不扫描区间、不执行 memset。heap ticket 一经消费便永久归堆，耗尽只导致普通 metadata OOM；固定物理容量不替代后续 MetadataSponsor/KernelMemoryBudget 的对象级 admission。`RecoveryTicket` 与 heap 没有公共消费类型，当前数组容量为零。

FramePool 先把完整 managed RAM 注册为 unavailable，只发布 planner 给出的 user-free。DTB、cold bootstrap 与 BootPackage 等 boot-held 页在 owner 生命周期结束且 transition 映射退休后经 `release_range` 回投同一 user inventory；permanent 与 system 页从不经过该入口。启动日志分别断言：

```text
managed = permanent + boot_held + system + user_free
system  = metadata + heap + recovery
```

## 用户帧库存

物理帧的用户库存由 `os/frame_pool` 的**外置元数据分级 order 树**管理。`frame_pool` 不使用堆且不含 unsafe；内核 adapter 负责真实帧清零、全局 POOL 锁与 `FrameTracker` RAII 所有权。

### 结构

每个 DT memory region 先按全局帧号作 canonical power-of-two 分解，形成物理对齐 arena。单个任意区间至多产生两倍地址位宽个 arena；板级最多 16 个 memory region，`MAX_ARENAS = 2048` 由 `16 × 2 × usize::BITS` 推导。arena 不跨 DT 物理缺口，因此任何 order 块天然物理连续并按自身大小对齐。

每个 arena 使用完全二叉树，节点一个 `u8`：`0..` 表示该子树当前可提供的最大 order，`u8::MAX` 表示没有空闲块。整块空闲由父节点直接代表；向下分配时才物化两个子节点，归还时沿祖先即时合并。分配和归还不维护运行期碎片链，也不读取被管理帧内容。

树节点每个托管帧精确占 2 字节；固定 2048 项 `ArenaMetadata` 与树节点同处 planner 交付的 metadata 区间。`FramePool` 自身只持两个外置切片和计数器，不把大数组压入内核栈，也不从自己管理的 user-free 页反向供血。

```rust
pub struct FramePool<'a> {
    /* tree metadata slice, arena metadata slice, counters */
}

pub struct FrameTracker {
    geometry: Option<ExtentGeometry>,
}
```

`FramePool` 不再参数化帧内容后端，claim/return 路径只能读写外置树元数据。内核 adapter 取得 claimed geometry 后先释放 POOL 锁，再经 `phys_to_virt` 清零完整 extent；清零结束才构造 `FrameTracker`。`ExtentGeometry` 可复制但只表达几何，私有字段的 `FrameTracker` 才表达 affine 所有权。

### 操作与契约

- `add_managed_region(start, end)`：注册完整 DT memory，初态全部 unavailable；重叠、arena 超限或元数据不足在修改前失败。
- `release_range(start, end)`：只把 planner 的 user-free 或生命周期结束的 user boot-held 区间发布为空闲；bootstrap、DTB 与 BootPackage prefix 回投走此入口。
- `alloc_order(order)` / `alloc_largest(max_count)`：纯库存 crate 的 order 与最大 extent 原语；内核仅以 transitional `alloc_user_order` / `alloc_user_largest` 暴露给页表、匿名 backing、Tunnel backing 和库存自检，锁外清零后才发布 tracker。
- `alloc_at(base, count)`：预验证完整指定区间空闲后，按 canonical blocks 精确取走；失败不改变库存。
- `dealloc(base, count)`：任意区间 canonical 分解后沿祖先合并；重复归还或与现有空闲库存重叠立即触发断言。普通 `FrameTracker` 持 power-of-two extent，BootPackage payload 可持任意长度保留区间。

库存步骤上界只取决于 `MAX_ARENAS` 与地址位宽；单 extent 归还只沿一棵树上行，任意区间的 canonical block 数同样由地址位宽和 DT region 数限制。清零与返回帧数线性，但在 POOL 锁外执行，也不存在随全局碎片数增长的扫描。

`FrameTracker` 不可复制或由安全代码任意构造；只暴露只读几何，`split_at` 消费原 tracker 并产生两个精确相邻 tracker。过渡期用户页表以 `TableFrameToken::Owned(FrameTracker)` 直接保存 affine owner，不再把 tracker 拆成裸帧号后通过 unsafe 重建。`FrameTracker::Drop` 直接走结构性有界归还。ProcessDrain 不再保存帧池扫描游标：owner 从拥有结构摘下与下一 work unit 的实际归还分开计费，页表 owner 通过通用 drain cursor 进入同一路径。

## 物理供给验证

16 项 FramePool host 用例覆盖库存结构、失败原子性与守恒；7 项 memory_supply debug/release 用例覆盖 permanent/boot 优先级、碎片化独立 chunk 放置、子预算不足、固定容量耗尽、失败 workspace 重规划、用途分型与 ticket 单调消费。内核启动自检分别覆盖 user inventory 的 claim/split/dealloc/re-zero、system heap ticket 首次消费，以及 funded broker 的真实 Pool/FramePool commit、extent-limit rollback 与双账本自然退款。系统储备批次收口时（验收拆档前的 full workload）virt debug/release 与 `sifive_u` 均完成 16/16 acceptance；实测闭包为 virt debug `262144 = 1495 + 9515 + 4236 + 246898`、virt release `262144 = 1308 + 254 + 4236 + 256346`、sifive_u debug `32768 = 1063 + 9515 + 4124 + 18066`，对应 system 子账户分别为 virt `4236 = 140 + 4096 + 0` 与 sifive_u `4124 = 28 + 4096 + 0`。当前阶段收尾由 `just acceptance` 分别执行 debug stress、release core 与 sifive_u core。

## MemoryPool 额度与 metadata 准入

`os/kernel/src/frame.rs` 在平台账本首次发布完成时冻结一次性 `RootPoolSeed`，其页数严格等于当时 `user-free + boot-held`；后续 boot-held 回投只改变物理库存，不改变 root 总额度。heap 可用后，`task/memory_pool.rs` 消费 seed 铸造唯一 root core，并由内核全局 anchor 持有到本次启动结束。启动日志同时打印帧分类和 root 页数，可直接核对这一闭包。

`os/memory_pool` 是 `no_std`、禁止 unsafe 的四项额度状态机，恒保持：

```text
total = available + reserved + allocated + delegated
```

charge 与 delegation 使用不同的线性 reservation/credit 类型，所有转换均作 checked arithmetic。每个状态实例另有 crate 内原子铸造、外部不可伪造的 `OwnerKey`；ABI Pool identity 只作稳定诊断身份，即使调用者重复提供相同 identity，token 也不能跨实例提交。child 在 Prepared 阶段消费唯一 delegation reservation；Commit 将 parent `reserved → delegated`，并把 delegated credit 内嵌进 child state。只有消费 fully-available child 才能取出 parent credit，因此不存在 child 仍可继续派生而额度已提前归父的安全 API。深度上限唯一来自 shared ABI 常量，当前为 32。

`os/metadata_admission` 提供固定容量 `Counter/Permit/SponsoredPermit`，单个 permit 可以原子预付多个 slots，失败和析构精确退款。`task/resources.rs` 在 `ProcessResources` 中持不可转授的 `MetadataSponsor`，其全局 slot 同时是 Process core permit，随 core 真实析构退款；ProcessBuilder、ProcessControl、Pool core 与 Bound AddressSpace 各自持按真实对象寿命退款的 sponsor+global permit，终态 Control 壳或已转移 Pool core 不因 creator Dead 提前退款。root Pool core 使用只占全局 slot 的 primordial permit。当前全局 sponsor、Builder、Control、Pool core 与 AddressSpace 壳上限均为 4096；每 sponsor 最多一个 Builder、一个 Control、1024 个 Pool core 与一个 Bound AddressSpace。Bound AddressSpace 另以 `AddressSpacePermit` 一次预付 4096 个 Region slots 和 4 个 planner transaction slots；全局分别为 131072 与 128，故过渡政策最多同时接纳 32 个预留完整 planner 容量的 Bound AddressSpace。ProcessResources 另以原子 reservation 串行化同一 shell 的竞争 Bind，不形成资源第二真值。
切片 6A 建立的三类操作准入已接入公开内存事务：backing slice 为全局 32768 / 每 sponsor 4096，MemoryChange、内存 WaitContext 与 Remote completion 各为全局 128 / 每 sponsor 4。`MemoryOperationPermits` 在提交前按 change → wait → remote 顺序取得三类 owner，任一中途失败由 RAII 回滚；拆分后 `MemoryChangePermit` 与 `RemoteCompletionPermit` 随 `MemoryChangeCompletion` 保活，`MemoryWaitPermit` 随 `WaitContext` 保活，均只在最终 Complete 后由真实对象寿命退款。公开 MemoryMap/Unmap/Protect 与 Tunnel Create/Attach/Close 还会在 Commit 前预留固定 work-debt 槽；任一准入、WaitContext、Remote 槽或 completion 分配失败都发生在 Commit 前。普通 anonymous backing slice 的真实 owner 接线仍属于切片 6E。

内核 `MemoryPool` 对象以 `Prepared → Committing → Active` 表达 Handle 发布事务。Prepared owner 在任何发布前失败时析构并回滚 parent reservation；Commit 后 child state 内嵌 credit、同时强持 parent，最后一个 core 引用消散时消费 child state、逐把锁把额度归父。`MemoryPoolQuery` 要求 READ，`MemoryPoolDerive` 要求 CREATE；child rights 必须是来源 rights 与 Pool 最大 rights 的子集，GRANT 同时用于 ProcessBindMemory 的 consume-on-success authority。init 的出生块固定安装同一 root core 的完整管理 Handle；内部 PoolBinding 与用户 Handle 共享 core，不复制额度。rinlib `MemoryPool` 是带 Drop close 的 affine owner，显式 `into_handle` 才把 authority 移回通用传输面。

14 项 Pool host debug/release 用例覆盖守恒、并发、rollback、split/merge、wrong-owner、重复 identity、深度与 parent credit 消费；8 项 admission 用例覆盖单槽/批量全局与本地耗尽、部分失败退款、精确批量退款和 sponsor 强保活。启动自检另穿过 ProcessBuilder/ProcessControl、MemoryChange/Wait/Remote 组合准入与 backing slice 的本地耗尽或退款，以及 Pool 的错误 kind/role/rights、真实 sponsor exhaustion、Handle 输出预留失败、Prepared rollback、Commit、多引用与最后引用退款；debug `virt` core 已通过，既有阶段基线的 stress/release/sifive_u 完整 acceptance 结论不因本批改写。

## 资金化帧取得 broker

`os/funded_frame` 是 `no_std`、禁止 unsafe、零堆分配的事务编排 crate。它不认识 capability、锁或物理地址，只以 `QuotaSource/QuotaReservation` 与 `PhysicalSource/PhysicalClaim` 两组仿射端口执行同一条 `reserve quota → bounded claim → clear → commit` 路径。页数与 extent 数是调用方分别给出的工作边界；当前内核实例以栈上固定数组容纳最多 64 extents，其 debug 帧成本由栈 guard 与 ELF audit 共同约束。全部 extent 取得前不清零，因而库存中途失败、extent 超限或非法 claim 会先析构已取物理 owner，再析构 reservation 回滚额度；全部清零后 quota commit 由类型变为不可失败。切片 6A 增加 `QuotaCredit` 与 `PhysicalClaim::split_at`：`Funded::split_off(&mut self, boundary)` 只在栈上建立后缀 storage，同步切出跨 extent 的物理 owner 与同源 credit，失败保持原 owner；`merge_from(&mut self, &mut donor)` 先验证固定 extent 容量，再合并 credit 并转移几何，成功后 donor 成为空 owner，wrong-owner 或容量失败保持双方不变。两种接口都避免按值回传原 owner，不公开两侧拆包，也不为错误恢复额外内联一份最大 extent 数组。

内核 `task/memory_pool.rs` 的 `PreparedMemoryCharge` 与 `MemoryCharge` 把纯逻辑 `ChargeReservation/AllocatedCredit` 接回来源 Pool core：前者析构回滚 reserved，后者强持 Pool 并在最后析构时退 allocated。`frame.rs` 的 `ClaimedUserExtent` 只在 POOL 锁内摘取 geometry，锁外清零；通用 `FundedFrames` 内部不可见的 `funded_frame::Funded` 按 extents 在前、charge 在后的字段顺序析构，且不提供可提前拆散两侧 owner 的公共入口。AddressSpace root 使用单页/单 extent 的专用 `FundedRootFrame`，避免把 64-extents 通用存储内联进空壳与 Bind 调用栈。BootPackage 则由不可伪造的 `BootHeldExtent` 表达未入库存的启动 owner，payload 经 root charge 转成可同步 split 的 `BootFundedExtent`；普通 broker 仍无绕过清零的 generic adopt。system tickets 继续由 `SystemSupply` 的独立类型拥有。

AddressSpace root 与 bootstrap payload 已接入 Pool charge；中间页表、普通 anonymous backing、Tunnel 与库存自检仍使用登记在 `frame.rs` 的 transitional raw adapter，分别在后续所属切片迁移。bootstrap 先把 `BootHeldExtent` 与 root charge 合成同时强持两侧的 `BootFundedExtent`，再由地址空间于任何 ledger/PTE 发布前回填 prefix、以 `BootBorrowed` 只读投影完成可失败映射，最后无分配地把 owner 本体装入 backing；因此 quota 或映射失败时外层 owner 始终覆盖全部借用期。payload backing split 同步切割 boot-held 物理 owner 与 charge；ProcessDrain 在 AddressSpace 锁内只摘 `BootFundedExtent`/`PoolBinding`，调用层锁外先归物理后退额度。通用 backing 的全面资金化与同一锁外 retire seam 属切片 6，不向 generic broker 增加任意拆包。11 项 broker host debug/release 用例覆盖 quota/库存失败、非法 claim、extent 上限、清零前放弃、提交、跨 extent split/merge、wrong-owner、失败 owner 保全、析构顺序与双方程恢复；内核启动自检继续验证真实锁与 RAII 接线。

## 页表模式选择

单模式（全系统同一 satp 模式）是共享内核映射的结构性上限而非妥协：内存物理上同一份，异模式 hart 各持平行映射树，反而割裂调度域（进程不可跨模式迁移）——多模式无收益。模式是启动期从硬件自动识别，不依赖手工配置：dtb 各 cpu 节点的 `mmu-type` 给出每个 hart 的支持上限，取全体 Application hart 的**最小上限**（硬件允许集），与内核支持集取交集选最高模式；不支持交集（如仅 sv32）则拒绝启动。运行时选出的模式作为常量贯穿后续初始化（satp 组装、地址宽度断言）。

内核支持集由编译期决定，当前只有 Sv39。

## 页表纯逻辑（os/page_table crate）

`os/page_table` 是 `no_std + alloc`、禁止 unsafe 的独立 crate，host 与内核 target 复用同一份代码。页表树 const 泛型于 `LEVELS`（3=Sv39、4=Sv48、5=Sv57），PTE 编码、VA 宽度与各层覆盖页数都由级数推导。页表逻辑不直接解引用物理地址；启动期未发布树与运行期 owner-aware 树使用两条明确 seam：`EagerFrameMemory` 可在构造中即时取得裸表号，`TableFrameMemory` 只投影调用方已取得的 `TableFrameOwner` 并访问表内容，不认识 Pool 或分配器。

运行期 `TableTree` 同时保存 root owner、每张 owned 分支表的 affine owner ledger 及 shared/owned root 位图；硬件 PTE 只是几何投影。内核当前以 `BorrowedRoot(FrameNumber)` 和 `Owned(FrameTracker)` 组成最窄过渡 token：root 在切片 6D 前仍由 `PoolBinding` 保活，中间表 owner 已不再拆成裸帧号，也不存在 unsafe adopt 回收路径。所有新表页在成为 owner token 前已清零。

### 类型与事务边界

- `FrameNumber`、`Vpn`、`Ppn` 是页号 newtype；`Pte` 集中编码 V/R/W/X/U/G/A/D 与 leaf/branch 判别。Map/Protect 在 preflight 拒绝超出 PTE 编码、V=0、无 RWX 以及 W 且非 R 的非法叶标志。
- `TableTree<M, LEVELS>` 拥有 root token 和全部 owned 中间表 token，不拥有叶数据 backing。root 的固定宽 `owned_root`/`shared_root` 位图与 owner ledger 同步；`attach_shared_root` 只登记外部子树，不把外部 owner 纳入本树。
- `TranslationPreflight` 是锁内只读结构快照，记录树代次、发布计划与精确表页需求；调用方在树外供给 owner 后，`prepare` 重检代次与结构，资源不足保持零 PTE 修改并把全部 owner 交还给失败值。`PreparedTranslation` 是 Commit 前 affine token；单项 Publish 要求仍是当前代次，同一事务的多项 Publish 只接受同代次 Map 批次。两者均不分配、不进入帧来源，并显式返回未消费 owner 与 Unmap 摘除的 owner。

### Preflight 与发布

Map 仍把连续 VPN/PPN 区间切成最大可行 mega 段。preflight 只读遍历现树：缺失路径以“表层级 + 覆盖区编号”去重；兼容 mega 的细化同样登记将要 split 的路径；已有异 PPN、异 flags、更细子树或 shared root 槽冲突在 owner 供给前返回。Publish 按同一切段重放，只能写叶、链接已供给表页或展开已验证的兼容 mega；同代次 Map 批次中较早项建立共享路径时，多余 owner 由结果显式退给调用方。

Unmap 与 Protect 递归携带当前表的真实覆盖基址。preflight 只为目标区间部分覆盖的现有 mega 计数：完整覆盖直接改叶，部分覆盖逐级精确预留；普通 4 KiB Unmap 因而预留零帧。Unmap 对未映射洞保持宽松，Protect 则要求区间完整映射且当前 flags 全部匹配。split 后 512 个子项保持原物理连续性和 flags；Unmap Publish 自底向上剪除新空的 owned 分支并返回 owner，内核把它们随 `PublishedChange` 保活到 Remote ack 后，确认前不归还。

页表 drain 使用 `DrainCursor<LEVELS>` 保存层级无关的固定宽遍历状态；每个 `step_drain` 至多摘一个 owned 分支 owner，调用方锁外析构后再推进。owned 槽与分支 ledger 清空后，`finish_drain` 才交出 root owner。`TableTree::Drop` 不遍历 PTE，只接受已 drain 的常数终态；未 drain 树析构立即拒绝。当前 AddressSpace 的 Building/bootstrap、Running 公开映射与 Tunnel ObjectView 均已接入 owner-aware API，但仍通过集中 raw adapter 在 AddressSpace 锁内供给过渡 token；Running/Tunnel 以 `table_transaction_active` 独占 Prepared 到 Commit/rollback 的树代次，避免两个非重叠 ledger 事务因共享页表路径而形成 stale Prepared。切片 6D 将依此 seam 把锁外 funded 供给接入全部 Map/Unmap/Protect 路径。

### 测试集（host）

30 项 tree 用例与独立 drain 用例覆盖：未对齐 8192 页跨表映射的精确 18 帧需求、mega 选择与细化、跨子表 Unmap、Protect、幂等与冲突、非法叶 flags、资源不足零修改、preflight 重检、同代次 Map 批次的多余 owner 显式归还、Map-vs-pruning-Unmap 与双 Unmap stale 检出、shared root Map/Protect 拒绝、完整 Unmap 剪枝与确认前保活、owned/shared root 槽转换、共享子树不回收、`max_work=1` drain、未 drain Drop 拒绝及 drained Drop 常数终态。debug/release host 测试与 clippy `-D warnings`、`just check`、virt core 均通过。

## 用户地址空间纯逻辑规划器（os/memory_space crate）

`os/memory_space` 是 `no_std + alloc`、禁止 unsafe 且不依赖其它 crate 的内部规划模块。它只拥有页对齐半开区间、区域账本、backing view、权限、owner、事务阶段和 MemoryObject 写许可状态，不访问页表、物理帧、用户指针、hart 或内核对象。内核 `AddressSpaceState` 已把该 planner 与匿名 `OwnedBacking`、MemoryObject view、reservation-aware `TableTree` 组合为同一事务；Building/bootstrap、Running 公开映射与 Tunnel lease 共用该 seam。

`MemorySpace` 在构造时一次性预留区域与在途事务的硬容量。有序 ledger 中每个 fragment 持唯一 `RegionKey`，同一次 Map 的 guard 与 mapping 共享 `AllocationKey`；fragment 另持 `AddressSpace`/lease owner、匿名 backing identity 或 `ObjectId + offset` view，以及当前/最大权限。`Anywhere` 在 ledger 与在途事务之间选 first-fit 完整空洞；`FixedEmpty` 不覆盖旧区域。Unmap 严格要求请求区间连续覆盖且 owner 一致，完整 reservation、usable-only、guard-only 与 mapping 中段都按精确交集切割；Protect 同样消费旧 key，只有 owner、种类、AllocationKey、连续 backing 与权限全部兼容的相邻 fragment 才合并。fault lookup 只返回 free、guard 或 eager mapping。

变更由不可复制的类型状态表达：`ValidatedChange → PreparedChange → CommittedChange → PublishedChange → SynchronizedChange → RetiringChange → RetiredChange`。Validate 可以分配规划元数据但不改 ledger；Reserve 复检 region snapshot、真实范围冲突、UserWriteLease pin 与 WritePermit multiset，并预留 Commit 所需的 fragment、retire permit 和事务容量。rollback 只存在于 Commit 前并归还全部 permit。Commit 是不可失败的 ledger 线性化点，不再分配；其后的 Publish、Synchronize、BeginRetire、FinishRetire 与 Complete 也不返回可恢复错误，错配 token 视为内核所有权不变量破坏。`begin_retire` 只把 retire owner 移入显式 `RetireBatch` 并进入 Retiring；`finish_retire` 拒绝仍含 fragment 或 permit 的 batch，故 ledger 只有在全部真实 owner 已逐项退休后才能进入 Retired。内核 adapter 按 translation intent 在 Commit 前准备真实表帧和 leaf 投影，Commit 后发布 PTE 与 ledger。

`UserWriteLease` 把非页对齐结果区间投影为固定上限的 writable backing segments，并以 RegionKey pin 到 Commit 或 rollback；与结果范围或变更 footprint 相交的其它在途事务返回 Busy。MemoryObject 状态独立实现 `Mutable → Sealing → Executable`：writable replacement 必须携带不可复制 `WritePermit`，permit 从 Reserve 覆盖到 retiring fragment 完成 Synchronize 后交给 Retire；最后一个 permit 退出计数时完成 seal，无人在等待也不回退状态。公共对象的 `EXECUTABLE` 电平与 WaitMany 等待面属于切片 7 的对象层，planner 只保证状态单向推进。

`os/work_debt` 是 `no_std`、禁止 unsafe 的固定容量纯逻辑队列。全局槽可在尚未知 owner hart 时 Reserve；最终 Remote ack 所在 hart 在 Publish 时成为 owner，槽进入该 hart 的 FIFO。Remote completion 正运行于同一次 `drain_current` 安全点，Remote drain 返回后会立即观察新 Pending，初次发布无需冗余 self-IPI；每个安全点最多推进固定步数，预算耗尽的未完成项回到同一 FIFO 尾部并重新敲 owner，Finish 后代次递增再复用。Pending 电平而非 IPI 边沿是真值：敲门失败只记录告警，scheduler idle 的双重检查禁止仍有 Pending 的 hart 入睡。Remote completion 本身只把 `PublishedSpaceChange` 转成持有 ledger token、table cursor、backing cursor 与 `RetireBatch` 的 `RetiringSpaceChange` 并发布 debt；safe point 每步最多摘一个 table owner/outcome、一个 backing extent、一个 object fragment 或一个 WritePermit，owner 都在 AddressSpace 锁外析构。最后一步依次 FinishRetire/Complete ledger、完成 Tunnel sink、兑销 mandatory operation、释放线程结果义务并唤醒 WaitContext；completion 与三类 metadata permit 随 work-debt 最后强引用一同消散。

host debug/release 共 19 项 planner 测试，覆盖区间溢出与对齐、容量和 backing 边界、Anywhere/FixedEmpty、双 guard、四类精确 Unmap、AllocationKey 保持与 RegionKey 消费、同 allocation 合并与跨 allocation 拒绝、owner/权限上限、object offset、UserWriteLease projection/Busy/rollback、非重叠并行事务、stale 与 permit mismatch 失败原子、显式 Retiring 阶段、live retire owner 禁止 Complete、seal/permit 逐项 retire、逐区域 drain，以及 2000 步确定性 shadow model 的逐页覆盖与 fragment 不重叠。`os/work_debt` 的 5 项 host 测试另覆盖全局容量回滚、owner FIFO 隔离、最小预算下多批重排与公平性、重复/缺失门铃下 Pending 电平，以及代次复用。两 crate 的 clippy `-D warnings`、`just check`、默认 core 与 stress QEMU 验收均通过；stress 覆盖最小预算 Drain、Tunnel exit 与完整竞态矩阵。

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
- **布局**：每槽 `[槽底 guard | formal (stack_size − emergency) | emergency guard | emergency]`，步长 `stack_size + 2×guard`。formal sp 从 emergency guard 洞下方起；emergency 占槽顶、fatal 路径专用，独立 guard 使其溢出不再踩入 formal。物理侧按槽连续打包 `stack_size` 字节（formal+emergency 相邻），guard 纯虚拟不占帧——这是 Linux `CONFIG_VMAP_STACK` 同构：物理页同时存在于直映射别名中，但内核只经 sp/窗口 VA 引用栈，**禁止经 phys_to_virt 触碰栈内存**；该禁律由 debug 断言兜底（`phys_to_virt` 拒绝栈物理打包区，release 构建无检查）。qemu/virt 每槽物理量为 `0x40000`；sifive_u 为 `0xA000`（`0x9000` formal + `0x1000` emergency）。固定容量 funded-frame broker 使当前 debug 最大单帧为 `0x2390`，因此 guard 洞为 `0x3000`，ELF audit 上限为保留 `0x800` 余量的 `0x2800`；两平台运行栈仍由 guard fault 与 acceptance 共同验证。
- **建表**：mm init 内、satp 发布前，静态子表（1 中间 + 若干叶表，不入帧池）按 `layout.mappings` 逐页映射（RW、不可执行——`flags::KERNEL_STACK` 无 X）、guard 洞置 invalid；所有 hart 与全部用户表共享同一子树。
- **地址转换**：`virt_to_phys` 是全函数（直映射线性算术 + `layout.translate` 互逆）；`phys_to_virt` 只对 admitted direct-map range 成立，并在 debug 构建拒绝 `no-map`、范围外地址和栈物理打包区。同一栈物理页同时有直映射别名与窗口 VA，PA→VA 无唯一逆；SBI ecall 传 PA 前仍须经 `virt_to_phys`（console 缓冲在栈上即依赖此）。
- **用户表拷贝**：栈窗口槽随直映射槽一起拷入用户 root（trap 在用户 satp 下即取调度栈指针）；正常 ProcessDrain 在有界 Root 阶段逐槽剥离共享顶层项后才归还 root，`AddressSpace::drop` 只作未完成构造/回滚的防御兜底。
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

任意用户页表共享内核高半区与栈窗口，用户 trap 不切 satp；`stvec` 恒指共同内核入口。用户上下文存于内核对象，`sscratch` 存本 hart 陷阱锚。内核稳态 SUM=0，只有 user-copy guard 可以临时直访用户 VA。

## 用户地址空间

`Process.space` 是稳定 `AddressSpace` 外壳：单调不复用的 identity、translation/instruction epoch 和内部 `AddressSpaceState::{Unbound, Bound}` 锁分离。Unbound 不持 ledger、页表或页额度；一次性 ProcessBindMemory 发布的 Bound 状态持完整 `PoolBinding`、`MemorySpace` ledger 与 `TableTree`，root tree 只借用 binding 中唯一 funded root 的物理帧。Bound 内的 ledger 是全部用户区域的 VA 真值；anonymous 区域由 `OwnedBacking(BackingId + logical page offset + affine extent owner)` 持有，Tunnel 区域由 `ObjectView(ObjectId + LeaseKey + RegionKey + PageRange)` 引用 Connection 的固定 backing，PTE 只是投影。Remote Call 只引用外壳身份与 epoch。Running 变更在 Commit 前快照 lifecycle execution gate，准备 planner/backing 或 WritePermit/PTE/WaitContext、三类 metadata permit、deferred-work 槽和全部 Remote 目标槽；Commit 在 `ADDRESS_SPACE → LIFECYCLE` 下复检 active sequence、发布 ledger/PTE/epoch 并登记 mandatory operation，锁外敲门铃。最后 ack 只进入 Retiring 并发布 work debt；owner hart 在后续 trap/scheduler 安全点逐批退休资源，最终 Complete 才解除外部义务。

低半区 `[0, 2^38)` 完全归用户；Bind 构造 root 时通过 `attach_shared_root` 挂入内核高半区顶层项（含栈窗口槽），PTE 安装与 shared 位登记同一调用完成。ELF、StartupBlock 和首线程栈只在 Bound 后由 Building 组装事务建立；`image_end` 只记录 Building 期映像/出生块放置终点，不是 Running 堆顶。首线程栈仍是 launcher 在 `2^38 - 8MiB` 建立的启动资源；内核只为 primordial init 执行同构 bootstrap 映射。ASID 恒 0，地址空间切换与 Remote Call 第一版均执行保守全量 `sfence.vma`。

- `MemoryMap = 0x50` 接受 80 字节固定宽 request，只开放 anonymous Anywhere/FixedEmpty 和 R/RW；64 字节 result 含 usable/reservation 几何、三项零保留字段及偏移 56 的末字段 committed cookie。Reserve 以 `UserWriteLease` pin 已存在的 RW 结果槽并保存物理投影；所有资源准备完成后先写 payload，execution gate 内以 release store 发布调用者的非零 cookie，再不可失败提交 ledger/PTE。成功返回前 Remote ack 已完成，Commit 后不再 uaccess；cookie 为零时 payload 无语义。
- `MemoryUnmap = 0x51` 对普通 AddressSpace owner 执行严格全覆盖与精确区间切割；空洞、guard/mapping 种类违约或 lease owner 拒绝整笔请求。匿名 `OwnedBacking` 在 Publish 后仍持旧 extent，Remote ack 后的 Retiring cursor 每步在 AddressSpace 锁内只切下至多一个 owner，随后在锁外归还；Protect 的旧视图仍被新 ledger 完整覆盖时以 `BackingRetire::Retain` 跳过 backing 释放。
- `MemoryProtect = 0x52` 只在创建时最大权限内改变 mapping；公开匿名 Map 不接受 RX，RW 区不能转 RX。涉及执行权限的已有映像变更推进 instruction epoch 并触发 `FENCE.I`。
- rinlib `MappedRegion` 不可复制，记录 reservation 与可选 usable 子区间；完整 Unmap 消费 token，部分 Unmap 返回左右至多两个 mapping/reservation-only fragment。全局 talc allocator 持固定 64 槽 arena inventory，以 64KiB 起步并几何增长到 16MiB，不在 acquire 路径为 arena 元数据递归分配。`Extend`、sbrk wrapper 与 Running heap transaction 已删除。
- rinlib `UserStack` 用一次 anonymous `MappedRegion` 建立 `[低 guard | usable | 高 guard]`，usable 大小按页上取整，sp 取 usable.end。`Builder::spawn` 在提交前建立栈和 heap packet，失败仍由调用者完整解除；成功后 `JoinHandle` affine 持有 ThreadControl、完整 reservation 与 packet。显式 join 和 Drop 都先 WaitMany(DONE)、执行 Acquire、关闭壳，再以同一个 `MappedRegion` token 解除包含双 guard 的 reservation；前者取走结果，后者析构结果。`thread::spawn_raw` 只包装 ThreadSpawn 原语，不接管这些资源，调用者必须自行建立唯一收束路径。
- owned anonymous、ELF、bootstrap stack、StartupBlock 与公开 Map 页均由 `OwnedBacking.extents` 持有；旧 `AddressSpace.frames`、`alloc_map` 与 `DrainStage::Frames` 已删除；PTE 调用先 prepare 精确表帧 reservation，再不可失败 publish；
- bootstrap StartupBlock prefix 是 owned backing；opaque payload 页由 boot-held token 直接转为 root-funded immutable lease backing，不经历“先回库存再取出”的窗口，地址空间销毁时在锁外同时归还物理 extent 与 charge；initial ELF 复制完成后 package prefix 页对齐前缀回投帧池；
- ProcessMap/Write 只服务精确 Building 且已 Bound 的 process；Map 创建 anonymous zero pages并使用最终权限，拒绝 write-only/W+X，Write 经已发布 PTE 的物理直映射回填 backing；Unbound 返回 ObjectNotAvailable，Running 发布后不再存在该写入口；
- ProcessDrain 对 Unbound shell 直接完成；Bound 先逐区域清空 ledger，再逐 extent 归还 backings，最后收束页表与 PoolBinding。lifecycle 的 Building/mandatory operation 屏障分别保证截止前组装提交资格与 REAPABLE 前无公开在途 `PublishedChange/RetiringChange`；已进入终止的 committed 事务仍由原 work debt 完成，发起线程消散不改变所有权。
- TunnelCreate/Attach 使用内部 MemoryObject view 建立 lease-owned RW mapping；每个 reserved/published/retiring writable view 持一个 affine WritePermit。MemoryObject state 与 Connection side state 分锁，permit 在进入 AddressSpace 前移出对象锁，Retire 也在 AddressSpace 锁外逐项归还。Create/Attach 与显式 Endpoint HandleClose 都使用预构造 WaitContext、metadata/work-debt 准入和 mandatory Remote completion；Close 先提交 ledger/PTE Unmap，远端确认后只发布 Retiring，最终批才发布 CLOSED/PEER_CLOSED 并完成 syscall。Terminating 进程 active 已归零，ProcessDrain 的 detached close 把 `RetiringSpaceChange + LeaseRetire` 固定保存在 Endpoint 中，每个 drain close work unit 推进同一状态机一步；若与在途 transaction 冲突或尚未完成，entry 原样留在 `pending_close` 供下一批重试，不存在同步 retire 旁路。旧 `external_mappings`、按 VA 搜索、本地 `sfence.vma` 与 Drop 隐式解除已删除。

## 架构边界

admitted hart、无 MMU hart 与 AMP 边界见 [`execution-context.md`](execution-context.md)。当前内核与用户目标均为 RV64；地址运算使用 usize，外部线协议才使用固定宽编码。
