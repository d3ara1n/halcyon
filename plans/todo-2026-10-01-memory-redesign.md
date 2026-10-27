# 内存管理与地址空间重构

状态：**对象链所需backing/view/地址空间整体退休、已提交修改独立推进与真实消费者已交付，后续独立审查修复及用户fault接入已完成**。首次实施见[对象链档案](archived/todo-2026-10-01-object-chain-kernel-integration.md)，后续处置与证据见[审查修复档案](archived/review-2026-10-07-object-chain.md#最终复核与交付)。本案属于[全项目重构](todo-2026-09-29-system-redesign.md)，拥有backing/view/地址空间结构、事务与退休、分页模式扩展及模块边界；资金与预算交界见[预算分案](todo-2026-09-14-kernel-memory-budget.md)。本篇保留当前范围、已确认目标与条件后续；旧基线、未接入草图和编译断点见[历史快照](archived/ref-2026-10-27-memory-plan-history.md)，不恢复旧施工。

## 当前范围与后续

当前生产路线为Sv39，VM已独立接入，`adopt/reclaim`、PublishCursor、backing/View、退款及用户fault均有实际消费者；原编译中断、旧模块删除和ABI迁移已关闭。OrdMap只持一个arena，运行期页表不再保留preflight/PreparedTranslation旁路。当前代码面貌由[mm实现](../notes/impls/mm.md)拥有，下面目标分层不代替实现说明。

本篇保留的独立后续是实际平台/用户布局需要Sv48/Sv57时的接通，完成需分页模式策略、satp与地址布局、共享内核映射、fault/退休及跨hart组合共同验证。当前实现固定Sv39，原按空间/域选模式的已确认目标仍由本计划拥有；impls对当前单模式的描述不改变目标，也不构成方向冲突。不能凭纯层支持或DTB声明把多模式记为已实现。gap索引、表帧精确预付和退休fence精简均是有测量/证明前置的优化候选，分别见[性能专题](todo-2026-10-01-lock-performance.md)与[本轮未采纳项](archived/review-2026-10-07-object-chain.md#收敛结果与保留边界)，不阻塞当前Sv39合同。当前不再安排“重写内核恢复编译”。

## 完成边界与 hart 兼容性复核

2026-10-07按`refactor/system-redesign` / `f174d8e`核对代码及既有交付证据；本节记录当前完成边界和未决后续，不新增实现验收结论。

**当前Sv39责任链已经交付。** 物理库存/来源付款、backing/View、Map/Unmap/Protect准备与不可逆提交、分步PTE发布、跨hart确认、表页/物理页退休退款、进程整体地址空间回收及用户R/W/X fault等待/修复均有正式消费者。完成屏障晚于相应实际退款，提交后的推进不依赖调用者继续运行。实现唯一视图见[mm](../notes/impls/mm.md)，后续修复及验证边界由[审查档案](archived/review-2026-10-07-object-chain.md#最终复核与交付)拥有；其中记录memory_space/page_table host用例、正式静态门、完整acceptance和hetero通过，真实发布fault与模拟修复诊断分别提供证据。`ac9d7d9` 的[独立原始Review](archived/review-2026-10-12-substrate-object-chain.md)已由[统一报告](review-2026-10-12-consolidated.md#已退出当前修复队列的判断)核定原两项P1的更正依据，当前设计门与验证边界按统一报告和所属专题接续；不把退出错误修复队列宣称为全项目无缺陷。

**执行兼容性已有结构性接通，当前只承诺Base64/D64。** `shared/dtb/src/cpu.rs::parse`冻结每hart的status、RV64基线、F/D/Q/V及mmu-type；`os/kernel/src/main.rs`排除Bare hart并发布能力；`os/sched_domain`按执行需求兼容集合分域。`task/process.rs::bind_initial_execution`为进程一次选择兼容域，无兼容域返回NotSupported；线程出生继承冻结domain，ReadyBatch核对域身份，enqueue按线程域入队，各hart的run只从本域pick。`libprocess`从ELF需求选择profile；Q/V/F-only/TSO等未支持用户需求显式拒绝，不静默降级。这条保证依赖可信平台声明及程序正确声明实际执行需求，不包含探测任意虚假固件/程序声明。

**MMU兼容当前依靠共同Sv39模式。** DTB解析接受Bare/Sv39/Sv48/Sv57；Bare不进入Application hart集合，其余准入hart执行同一Sv39内核和用户空间。固定ISA规定支持Sv48必须支持Sv39、支持Sv57必须支持Sv48，见[supervisor规范](../references/normative/riscv-isa-v20250508/src/supervisor.adoc)的Sv48/Sv57章节，因此不同mmu-type上限当前不导致Sv39进程落到不支持Sv39的准入hart。`sched_domain::HartCapabilities`只持ISA状态能力，没有MMU模式字段；`mm.rs`的satp和`vm/space.rs::LEVELS=3`仍固定Sv39。按hart上限为各进程选择Sv48/Sv57并约束调度的能力尚未接通，不能把ISA分域等同于该能力。

尚未覆盖的承诺与下一步：

| 后续 | 当前事实、触发与完成标准 | owner |
|---|---|---|
| 更宽分页模式及模式策略 | 原“结构前提/轮次1”的按空间/域选择与多份内核树属于已确认目标，当前实现仅Sv39，差距是尚未接通。实际需要使用Sv48/Sv57前，按该目标补足详细接口与模式/布局/调度/同步及平台验证；若新证据要求改变目标，再记录设计选择。不能从impls的现状描述推导目标被撤销或必须重新裁决 | 本案；跨hart执行接点见[execution-context实现](../notes/impls/execution-context.md) |
| 不可信域硬容量隔离 | 双维Pool实际付款已交付；live metadata槽不是堆字节硬隔离或分配成功保证，Weak尾部和allocator碎片仍可能占用堆。开放不可信创建域或承诺域间硬容量前闭合 | [预算分案](todo-2026-09-14-kernel-memory-budget.md) |
| 设备/DMA与动态保留区 | 首批设备/DMA 接点已接通，DMA 静止仍不能由 CPU VM 完成推导；动态/reusable 保留区 fail closed，实际消费者需要前由保留区专题闭合 | [设备分案](todo-2026-09-device-system-design.md)、[保留区生命周期](todo-2026-09-platform-reserved-memory-lifecycle.md) |
| 更强性能与证明 | ASID/定向shootdown、减少预付表帧、精确物理CPU计量与WCET没有当前完整承诺；真实芯片微架构及全分配点失败注入未覆盖 | [性能专题](todo-2026-10-01-lock-performance.md)及相应机制计划 |

本次不重新启动内存总重构。当前Sv39可继续承载已约定系统工作；扩展能力必须按上表补足前提，不能用“当前完备”掩盖未来保证，也不能用未来未建设否定当前责任链已收口。

## 设备 DMA 接入合同

2026-10-23 为[设备首批方案](todo-2026-09-device-system-design.md#dma)确定内存接点；当前内核已接分步 BackingAcquisition、独立 DMA BackingUse/写许可与设备退休 owner。代码事实、运行证据和仍未覆盖的硬件门由设备主计划拥有，不扩大已交付 CPU VM 的完成边界。

- DMA mapping 持独立 `BackingUse(backing,[lo,hi))`，不只持 `Arc<Backing>` 或驱动 VM。现有 [MemoryObjectCore::acquire_use](../os/kernel/src/task/memory_object.rs)、[BackingUse/BackingRelease](../os/kernel/src/vm/backing.rs)提供范围保活/释放基础；页与原 charge 在最后实际 use 退出前不得退款。
- 设备可写的 Pin 必须在 MemoryObject 原状态机中取得写许可，Sealing/Executable 拒绝新写 Pin；既有设备写许可尚存时 Seal 可以请求，但不能公布 Executable。设备只读 Pin 只需相应读取授权与 backing use。补充的是 DMA 调用接点及同步后的许可归还，不在设备层复制 W^X 状态。
- Pin 在实际源授权仍有效时取得 use/许可，准入失败由原准备 owner 取消未提交许可并完成 use 回滚；映射提交后才交出设备地址。正常退休在可信硬件完成后归还写许可、释放 BackingUse，按原有预算推进物理归还和退款，最后发布 mapping 完成。
- VM Unmap/Protect/进程退出只释放其 CPU view/use/写许可，不能代退 DMA 的那一份；CPU 映射降权不必等待 DMA，整个 MemoryObject 进入 Executable 必须等待所有写者。首批 MemoryObject 固定长度，不新增 shrink/decommit；未来引入时必须守住实际 use 的区间保活，不将当前没有该接口写成设备侧独立保护机制。
- DMA control 全部消失时，地址可能已交付的 mapping 静态保留原 use、写许可和费用，不创建永远 runnable 的回收工作；尚未暴露地址的准备允许安全取消。明确的受信 Retire 才触发暴露后实际退休；首批孤立映射等系统外部复位。由设备资源 owner 持这项责任，Process DONE 和 CPU shootdown 不成为 DMA 完成证明。

实际接线：MemoryObjectCore::begin_pin 在有效源授权下取得分步 BackingAcquisition，原 root use 在该准入责任退出前不被最终 Work 释放；每次只增加一个实际段引用。reserve_dma_write 在原状态锁下调用 `MemoryObjectState::reserve_writes(1)`，持有原 core 与 affine WritePermit；不经 Views 的 CPU 来源计数，也不伪造 `PermitRequirement`。共享快照字段已统一为 write_permits，包含 CPU/DMA 写者。中途失败交原 Pin Work 回滚已持前缀，不能 Drop 尚有效的 BackingUse。归还写许可必须晚于可信硬件完成/必要平台同步；随后逐步释放 use，完成谓词只要求这一份责任退出，不要求其他合法 use 一并释放。

Pin 结果与物理地址分开：Pin 只交付 mapping 能力，第一次 ReadSegments 在可能写出任何地址之前标记 MayBeExposed。此前安装失败/取消/域封口可回滚；此后输出 fault、最后 Close 或进程死亡均不能推断硬件没有取得地址。暴露边界、静态保留和 Retire 由[设备 Pin 合同](todo-2026-09-device-system-design.md#dma-pin-的提交与结果寿命)拥有。范围 use 的物理地址枚举复用 BackingUse::physical_span，每次有界，不生成另一套物理库存表。

### MMIO 映射接入

纯层原 BackingView/MapBacking 已演进为 MappingOrigin/MappingSource，包含 MMIO 身份/偏移、基础页选择及强制完整片段退休的预约原语。实物 View 已改为 `MappingUse::Ram(BackingUse)` 或 `Mmio(MmioViewUse)`，稳定 Map 记录与每空间预付 Work/FIFO 已接入 VM；MMIO 不伪装 Funded/Boot RAM，不取得 MemoryObject 写许可。普通 VM 几何、发布、fault、同步和退休共用 ChangeContext，详见[设备 VM 接线](todo-2026-09-device-system-design.md#vm-来源发布与强制撤销)。MMIO control/use、层级树的分步准入/撤销、shared/libkernel 与公共 Map 分支已接入；原线程请求持准备并在目标页表下有界推进。virt 实际平台根及 ECAM 消费者已接通，非空退休、最后 Close 和退出组合的证据及精确交错/费用故障缺口由[设备续接记录](archived/ref-2026-10-27-device-delivery-history.md#平台-mmio-根与真实消费者续接)拥有；不把普通 RAM 回归或调度竞争当成全部跨 hart 交错证明。

普通 uaccess/IPC/结果写回及初始栈/上下文的检查已约束实物 View 来源为 RAM，保持 translate 前置校验和 SUM 直访；该来源索引在 PTE 发布前登记、同步后退休，覆盖几何账本分步替换时的空窗。MMIO 的访问由驱动专用 volatile 接口承担，不新增内核软件页表拷贝旁路，也不让 IO 页成为 DMA Pin 来源。新增映射来源须覆盖 View 切分、Protect、Unmap、失败回滚、空间退出及旧 VA 空洞再利用，不只覆盖 Map 成功路径。

实施与验收并入设备计划：现有 init/test_target 已接正常 Pin/Seal/Retire、未暴露取消、VM 退出和地址输出 fault 的保留；根终态观察与外部 GDB 路线核 reset 失败/初始进程 DONE 后的真实保留。首批 Pin/Seal、reader/Retire、付款/OOM 与设备停止的已取得证据和限制按设备计划当前收口表复用；不从这份早期接点说明恢复已关闭验证门，CPU 映射或物理地址读取本身仍不是 DMA 停止证明。

## 结构前提（用户确认）

分页模式按地址空间选择，约束为所属调度域内每个 hart 均支持该模式；需要更宽模式时，DTB 的 mmu-type 进入能力/放置策略，内核高半区为所需模式准备相应映射。当前仍只接通 Sv39，Sv48/Sv57 是上方条件后续，不宣称已经实现。

Process 只持地址空间引用，不认识其内部。各模式的分发位于内核 AnySpace 接点，具体层次由下面三层设计拥有；早期另建 VM 核心 crate 的候选不保留。

## 目标设计（设计轮次 1，已定稿）

经 advisor 审查一次、与 DeepAsh 一轮收敛确定。标“用户确认”为用户明确决定，其余为收敛结论。取代前期候选结构与待收敛点。

### 分层（用户确认，修正结构前提的五层为三层）

| Module | 拥有 |
|---|---|
| `page_table` | `PagingMode` 类型族（Sv39/48/57：层数、VA 位宽、satp 编码）、PTE 编码、表结构与有界修改/拆除原语 |
| `memory_space` | region/View 几何、修改协议、区间预约与租约、同步代次与确认条件、退休进度；只产出领域明确的纯值动作，不接回调、无通用 effect 框架 |
| 内核 `vm`（从 `proc.rs` 拆出） | 地址空间与 backing 实际 owner、锁、真实页表与帧、硬件执行、依赖登记/唤醒、内嵌 Work；syscall 授权后进入 |

不新建 VM 核心 crate，避免 ledger/冲突/退休分属两层形成双真值。用户区保留布局是 VM 政策，以布局值传入，不塞进 trait。`AnySpace` 在内核入口分发，Process 不匹配模式。

### 事务：四阶段与三个所有权分界

- 对外模型：准备 → 提交 → 同步 → 退休。类型只保留：可取消的准备 owner（资源、结果租约、发布承载、同步运输、回滚承载齐备）、不可撤销的已提交责任、同步完成证明 `RetireGrant`（不可复制，写明可释放的具体资源）。Validated 降为准入内部计划值，跨锁保留须带区间代次或真实预约。Committed/Published 合并；Retiring/Retired 并为同一记录游标，最后一次真实退款完成才 Done，已交出未回执的动作计入未完成。
- 内核不保留平行阶段机：同步运输属于所属事务，退休游标由事务 owner 持有。
- 提交协议条件：A 最终复检可失败但在不可逆发布前，Commit 后无 OOM/Busy；B PTE/ledger 变更、epoch 与 FenceRequest 同一受保护协议绑定，动作由稳定 owner 持“待执行”状态，PTE 未写不得接受 ack；C 确认携带空间身份 + 事务/epoch 身份，拒绝重复、迟到、错代；D 区分责任提交点与修改完成点，提交后的 PTE 发布为有界子步骤。
- Building 并入同一状态机：所有修改取得空间操作准入凭证并产出 `SyncPlan`；Building 准入证明确认集合为空，不由调用者传 bool、不凭 `active=0`。Start 与 Builder 在同一准入门仲裁：Builder 先准入则 Start 等其 lease 结束；Start 先关闭准入则后来者被拒，准备 owner 回滚，不降格为 Running 修改。首次进入空间仍按 epoch 做本地 SFENCE.VMA/FENCE.I。空退休批立即收口保留为快路径，不推广为 Building 同步完成全部工作。
- 准备 owner 在接纳首份资源前存在，lease 全程覆盖至回滚结束；绝不等待 Process/Space Done。
- 整体退休不是普通事务：独立终结状态机，关闭 mutation gate、等待既有事务与租约，复用同步与释放原语。

### 并发修改安全（用户确认：不停进程）

内核保证资源安全（不访问已释放/复用页、seal 后不可写、W^X）；用户可见一致性只承诺每页非旧即新，与常见 munmap/mprotect 语义一致。

1. PTE 为对齐 64 位单次 store。
2. 权限缩小（Unmap、RW→RO）：改 PTE → shootdown → 全部 ack 后才归还写许可与 use。
3. 权限扩大（Map、RO→RW）：改 PTE 不 shootdown；残留旧翻译引起的伪 fault 在 ledger 复检后本地 SFENCE.VMA 重试。
4. 不同时加 W 与 X：RW→RX 先撤 W 并确认，再加 X。
5. 大页拆分先建后换：完整构造等价下级表，单次替换父 PTE，再 shootdown。
6. 重叠修改由区间预约互斥；不重叠修改可并行。
7. fault 短暂进入空间锁读 ledger：命中撤销中区间按已撤销投递，命中建立中区间按 3 重试。

空间锁只护 ledger/PTE 短临界区，批间放锁。

### 大页不跨 View（用户确认）

叶大小只在 Map 时选：对齐 VA 块完全落在该 View `[start, end)` 内且 backing 物理连续同对齐才用大页，preflight 以 View 边界为输入。无事后提升；将来引入须守此不变量。因此完整释放一个 View 只清自身叶并回收变空中间表，只释放不分配，必成；空表判定由每表有效项计数提供。部分 Unmap/Protect 切入大页仍需拆分，可失败，拆分表页在准备阶段预取。

### 写许可、Backing 与 Tunnel

- 责任链：源授权 → 取 use/写许可 → 准备 → 提交 → 撤销或降权 → 翻译确认 → 归还写许可 → 释放 use → backing 独立回收退款。View 持 backing use、权限上限和写许可归还目标，不持 capability。拆分 RW view 时存续片段继承写责任，计数不短暂归零；Sealing 不禁止减少映射的合法拆分。归还责任在空间锁内摘出、锁外操作 backing 状态。
- Backing 统一但保留两种持有粒度：对象持整域 use `[0,N)`，匿名 view 只持对应区间；见下节“Backing 区间持有与退款”。
- Tunnel 退出 VM 生命周期：删除 `RegionOwner::Lease`、`MemoryRetireSink`、`LeaseRetire` 回接，由通用 View 承接；Endpoint Close 不撤销 View。
- Pool 的页与 metadata 采用独立守恒账本，实际容量费用及退款凭证由预算合同拥有；VM 准备成功即已取得完成所需资源。`Funded` 先还物理再退额度保留为不变量，分步回收与退款完成事实显式表达。
- 部分 Unmap/Protect 可在准备阶段因 metadata、表页或权限失败且不改旧映射；完整释放复用已有节点与游标，不再申请。

### Backing 区间持有与退款（设计轮次 2 收敛结论）

- **真值**：Backing 有稳定身份与原资金来源，内部按逻辑页 offset 分为有序段；段不跨物理连续范围与资金凭证边界，记录 `[start,end)`、物理起点、真实 Funded owner 与覆盖 use 数。extent 只是物理表示与批处理单位。
- **use**：View 持不可复制 `RangeUse(backing,[lo,hi))`；`Arc<Backing>` 只保内存安全，不是业务 use。重叠覆盖按逻辑区间计数，受 backing 短锁保护；匿名不重叠时退化为 affine owner 分割。
- **索引**：按 offset 的侵入式平衡树，节点出生时取得 metadata；每 step 定位并处理一个段，游标续跑。删除 Vec position/remove/insert。
- **释放不分配**：拆段节点、存续 View、页表拆分表页在准备阶段预取，不足即在改旧映射前失败；prepare 后并发变化以 backing 范围预约/版本复检排除。use 取得时两端切界已物化，后台合并不得吞掉 live use 依赖的边界。零 use 段锁内摘出、锁外按预算归还，归还中不得被新 acquire 复活。
- **退款粒度**：契约为基础页级——同步完成后无 use/pin 的页可归还，实际合并为物理连续、同来源的子段；物理 owner 与 charge 一起切分（`FundedExtent::split_at` 已支持），先还库存再退份额。反例：1GiB 匿名只剩 4KiB view，按 extent 计数则近 1GiB 不退；只退额度不放物理则重复授信。
- **承载**：部分 Unmap 产生的零 use 段由产生它的事务记录持 `RetireGrant` 推进退款；backing 内嵌记录只负责最终残余与本体退休。不为每页/extent 新建 Work，事务 Done 即确知所交资源已退款。

### 整空间退出屏障（设计轮次 2 收敛结论）

1. 在 dispatch 同一 gate 下永久关闭执行入口并固定退出代次；VM 另按自身 lease 等既有 Builder/事务/内核访问退出，既有事务不反等 Space.Done。
2. 收齐执行租约离场确认：hart 先切至内核 root、完成本地同步与显式访问发布，再 release 确认；回收方 acquire。线程标记 Killed 不算离场。
3. 映射冻结后，向全部已接纳 hart（上限 8）发整空间全量 fence，确认按 空间身份 + 退休代次 + hart 区分；最后 ack 前保活页表与 backing。离线 hart 不算自动 ack，须有停机清理或上线前强制清空协议。
4. 取得整体 `RetireGrant` 后在独占退休结构上直接拆 View/use/表页并退款，不逐 View shootdown。设备/DMA use 为独立屏障，CPU fence 不替其背书。

- 运行期进入协议：进入者锁外同步所见 epoch，gate 内复检入口仍开且 epoch 未变才发布执行租约；active 登记先于装 satp。Resume 快路径不得绕过 epoch/终止检查。
- ASID 只是硬件标签：凭证绑定 空间身份 + ASID 分配代次；用户映射不设 G 位；ASID 复用前须完成旧代次清除。SFENCE.VMA 不排序显式访问，离场 release-acquire 承担该顺序。
- 依据：固定规范 supervisor.adoc 写明切 satp 不终止已启动的翻译、不隐含失效，故只看 active 快照不足。
- **登记的后续优化**：现有离场在切 satp 后已做全量 sfence（`assembly.asm:345-346`），若离场确认纳入该证据，第 3 步广播可证冗余；触发条件为测得退休延迟成为瓶颈，不写入目标契约。

## 设计轮次 3 收敛结论：本链 VM 接缝（2026-10，advisor 一次 + LoneCrane 对抗）

以下为已接入的 VM 接缝合同，实际批次容量和成本由代码及既有交付证据核实，不再保留早期施工门。

### 表帧 charge

- 同一空间表页来自固定空间账户，同源校验在不可失败 `adopt` 之前。准备页为自带 charge 的 `FundedTableFrame`；`adopt` 将 charge 并入空间 `TableCharge: Option<AllocatedCredit>`，owner 只剩帧号。
- `reclaim` 是所有权移交：产出（物理 claim + 恰好一页 credit）的 affine owner，TLB 安全且锁外放帧后才退款；最后一页 take，其余 split(1)；与 `branches` 守恒断言。在途已摘出 owner 计入 Space.Done。root 由空间独立持完整帧，内核共享子树不进 TableCharge。

### 结果租约

准备阶段取得可回滚的空间访问 + 范围保护（复用 `ensure_no_transaction_conflict` 双向冲突），commit 原样移交、无空窗（删除 commit 清 lease）；最后一次写回后释放，不等 Space.Done。可取消 Wait 不跨等待持租约，获胜后写结果须短暂取保护并复检；退出/取消胜则不写。

### 分步修改（统一协议）

- 事务区间记录 `{txn, 阶段, 新旧计划}` 是过渡期真值，fault/查询先查它；ledger 与 PTE 均按段分步，每步成对推进一个可观察切片，OrdMap remove/insert、节点归还、移出 permit 计入单步成本。
- 写者互斥：prepare 在空间锁内用预取节点把 Region 在请求端点切开（同权限切分语义中性），footprint = 实际替换节点集合；预约覆盖节点禁止后台合并/重切；预约、冲突复检与端点物化同一保护协议。切分节点 metadata/写 use 在可见时移交空间，rollback 不合并、不退占用中的 permit。提交后不再 Stale。
- 游标只持 VA 与事务身份，每步从根重走；删除全树 generation 校验。
- 请求上限只用于可失败、需分配的部分修改（依据准备期预取资源）；完整释放与整空间退休沿原 owner 游标逐段撤账，不受上限，只释放不分配。

### 空表回收：最后离开者接管

- 覆盖范围完全在本事务预约内的空表，本事务在取 epoch 前摘除。
- 预约边界表（每连续片段每层至多 2 张）在事务结束发布或回滚时于空间锁内“检查其他预约 → 摘父 PTE → 移除自身预约”一次完成：空且无其他预约相交才摘；自身不计阻塞者；持续被占用时由下一个离开者或整空间退休兜底。
- 摘表后取新 epoch、fence 完成才放帧，不复用旧 epoch。废弃 `preflight_unmap_range` 的精确容量假设。
- 承载时点：可失败部分修改在发布预约前取得 ChangeWork、边界候选容量与内嵌同步运输；rollback 由同一记录继承并走同一路径。完整释放所需 Work、K 槽退休批次与同步承载随原资源 owner 出生/增长准入，释放只激活；整空间退休用出生内嵌记录。
- 固定容量批次：每批摘表 → 新 epoch → 覆盖本批 fence → 收 ack → 放帧退款 → 复用槽，批间不做可失败准入；可失败部分修改所需的额外承载在准备期取得，不保留另一套 PreparedTranslation 旁路。

### 同步 epoch

- epoch 在该事务（或本段）最后一次 PTE store 后于空间 gate 下取得，last-store → epoch 发布 → FenceRequest 可消费有真实可见顺序；责任提交 ID 与同步 epoch 分离。
- 覆盖对象在同一进入 gate 下确定：此前进入者由本批 fence 或离场清除证明覆盖，之后进入者按新 epoch 同步，不用早于发布的 active 快照。
- 零目标与 remote 同一谓词“发布已 armed ∧ required mask 完成”；Building 空集合来自准入证明。RW→RX 两段各取 epoch，第一段确认到齐才加 X。

### fault

先按R/W/X对照Committed整个窗口的事务终态目标权限：不允许即正常投递，guard/Unmap目标为None。允许但PTE未发布则捕获已有Completion来源，释放VM锁与Body短loan后停驻；已发布则SFENCE，X另FENCE.I，保持原pc/GPR重试，绝不锁内循环。Thread签名包含PTE、epoch、事务身份和已尝试hart位图，同组事实每hart只修复一次，无进展则正常终止；A/D由页表发布预置。

依赖索引由max_transactions预付，登记/摘除与纯层预约的取得/释放同VM锁仲裁，索引不持被等待的Change/Retire Work。摘索引前ledger/PTE已是终态，之前捕获的来源继续到真实退款/access归还后DONE；Frozen等待者先detach离场，内核S态fault不进入DONE等待。SUM读/写/cookie另在同VM锁内translate后SFENCE再直访，不依赖尚未前进的epoch。完整寿命、验证边界见[审查修复档案](archived/review-2026-10-07-object-chain.md#最终复核与交付)。

### remote 运输复用

每目标最终 ack = fence 完成 ∧ 该目标槽已归还到本 Work 内嵌保留态。顺序：持稳定 completion token → fence → 归还槽 → 最后访问旧批次后 release ack → 最后者锁外 wake。批次身份含 epoch，最后 ack 后不再读写旧槽；所有目标结束前 notified 等唤醒不触发复用。运输随所属 Work 出生并复用，旧全局槽及 generation 封顶路径已删除。

### Tunnel 退出 VM

通用 Map 直接接受 Endpoint capability 作 backing 授权，上限为 Endpoint/Invitation 已确认的 backing authority；View 只持 backing use，不强持 Endpoint。

## Unmap 完成点优化条件

当前完成屏障包含相应实际释放与退款。只有测量表明内存操作延迟构成瓶颈时，才由本案评估把调用者完成点提前到同步完成、退休继续后台推进；评估须重新闭合结果交付、线程 join、退出接管与退款观察的边界，并验证未完成退休仍由稳定 owner 推进。该候选不改变当前返回及退款保证，也不作为本轮待实施项。
