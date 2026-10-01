# 内存管理与地址空间重构

状态：**设计轮次 1 已定稿（见“目标设计”），剩余设计门待闭合；未实施**。本案属于[全项目重构](todo-2026-09-29-system-redesign.md)，拥有 backing/view/地址空间的结构、事务与退休、分页模式扩展及其模块边界；资金形式与预算交界见[预算分案](todo-2026-09-14-kernel-memory-budget.md)。首个范围为对象持有与释放链所需部分：地址空间整体退休、view/backing 归还、已提交修改的独立推进（见[进程回收定稿](todo-2026-09-29-process-automatic-reclamation.md#回收结构目标设计设计轮次-1已定稿)剩余门）。

## 现状事实（当前分支只读盘点）

- 地址空间、事务 owner、view owner、shootdown、drain 阶段与 Map syscall 入口均在 `os/kernel/src/task/proc.rs`（5475 行），与 Process 混合；无独立 VM 模块。
- `os/page_table::TableTree<M, LEVELS>` 对 Sv39/48/57 与表帧来源泛型、host 可测；上层 `proc.rs:46` 固定 `LEVELS = 3`，`mm.rs:9` 固定 Sv39。
- `os/memory_space` 事务为七个类型状态（Validated→…→Retired），内核再包 `MemoryChangeCompletion`、`RetiringSpaceChange`。
- 多路径：Builder::map 同步 Commit；Running Map 走 shootdown + 异步退休；进程退出走 `drain_inner` 四段。view 释放分 `release_view_region` 与 `drain_view_region`。
- Tunnel 特例：`Connection` 强持 `MemoryObjectCore`，`RegionOwner::Lease`，`LeaseRetire` 以 Weak + 临时强持破环。
- 付款：页/表页/对象 backing 由目标进程 Pool 付款，metadata 另走全局 `MetadataSponsor` 双层计数；退款依赖 `Funded` 字段析构序。
- 非有界原语：整段清零 `frame.rs:846`；退休遍历本批 views `proc.rs:1341`；`proc.rs:4180` 注释与代码不符。
- ABI 残留：`Rights::MAP`、Tunnel `0x60–0x63`、ProcessDrain（均已由其他分案定为删除）。

## 结构前提（用户确认）

- **分页模式按地址空间选，不按 hart**：地址空间跨 hart 使用并跨 hart shootdown；约束为所属调度域内每个 hart 支持该模式。DTB `mmu-type` 作为 hart 能力进入 `sched_domain`，进程创建时按域确定模式；内核高半区按每种模式各备一份。
- **瘦身与模块化**：从 `proc.rs` 拆出地址空间；Process 只持地址空间引用，不认识其内部。

| 层 | 内容 | host 单测 |
|---|---|---|
| `os/page_table`（已有） | 按模式泛型的表结构、编码、有界 drain | 是 |
| 分页模式 trait（新增） | `Sv39/Sv48/Sv57` 类型：层数、VA 位宽、用户区布局、satp 编码 | 是 |
| `os/memory_space`（瘦身） | region 几何、ledger、view/backing 关系 | 是 |
| VM 核心 crate（新增） | `AddressSpace<M: PagingMode>` 纯状态机：提交/同步/退休有界 step、所需 TLB 确认集合，产出纯值动作 | 是（模拟表帧来源） |
| 内核 `vm` 模块（自 `proc.rs` 拆出） | 锁、真实帧、satp、remote fence、维护 `Work`、syscall 入口 | QEMU |

内核以 `enum AnySpace { Sv39(..), Sv48(..), Sv57(..) }` 一次分发，其下单态化，不用 `dyn`。

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
- 内核不保留平行阶段机：`MemoryChangeCompletion` 缩为运输接缝，`RetiringSpaceChange` 游标并入事务 owner。
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
- MetadataSponsor 不并入 Pool 余额：统一账户归属与退款凭证，物理页与 metadata 存储分开计量；VM 只要求准备成功即已取得完成所需资源。`Funded` 先还物理再退额度保留为不变量，分步回收与退款完成事实显式表达。
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

### 需改写的非有界原语

`page_table::take_owner` 线性扫描表页 owner（drain_step 依赖）；ledger Commit 全表排序与 Vec 搜索（`space.rs:1408-1421`）；逐 delta 发布（`proc.rs:3197-3226`）；extent 释放的查找与 Vec 搬移；整段清零（`frame.rs:846`）；退休遍历本批 views（`proc.rs:1341`）。均须改为结构有界原语，不能只切外层循环。

### 剩余设计门与 owner

1. **本案**：上述原语改写方案；随后确定 Rust 类型、`memory_space` 实施与内核拼装草图。backing 区间持有/退款与整空间退出屏障已闭合（设计轮次 2）。
2. **预算分案**：账户凭证的计量形式与物理页/metadata 是否可兑换。
3. **维护执行**：VM Work 的 step 计费接入，无新增机制。

## 接力

设计轮次 2 已闭合 backing 区间持有/退款与整空间退出屏障。

施工进度（未提交，未接入内核）：
- `os/memory_space/src/backing.rs` 已实施 `SegmentMap<O: SegmentOwner>`：按逻辑页 offset 分段、`NodeReserve` 预取拆段节点（准备阶段不足即失败）、不可复制 `RangeUse`、`acquire` 在端点切界、`begin_release`/`release_step` 游标式有界释放（每步至多处理一段，零 use 段摘出交调用者退款）、`begin_retire` 后拒绝新 acquire。5 项 host 单测；原 planner 20 项仍通过；clippy 干净。
- `os/memory_space/src/ordmap.rs` 新增 `OrdMap<K,V>`：arena AVL，节点经 `reserve` 预取，insert/remove 不分配，单次操作受树高 O(log n) 约束；4 项 host 单测（含与 BTreeMap 对照、顺序插入树高界）。`SegmentMap` 索引已换为 `OrdMap`（原 Vec 链表过渡项关闭）。region ledger 后续复用同一结构。

- `page_table` 表帧 owner 改为帧承载（用户确认方案 C）：`TableFrameMemory` 新增 `adopt(owner)`/`reclaim(frame)`，树不再存 owner Vec，`take_owner` 由线性扫描变 O(1)；只保留 `branches` 计数供收束断言。host 30 项通过。否决：A 每表旁置 512 项 owner 数组（嵌入式内存成本翻倍）；B 帧池每帧元数据（为单一需求新建全局子系统、复制资金真值）。
- **内核编译中断（用户确认 C1）**：内核 `TableMem` 未实现 `adopt/reclaim`，`just check` 失败（E0046）。不加临时 adapter；目标形状为表帧 owner = 帧号 + 所属空间账户，退款经空间账户完成，于内核 `vm` 重写时一并落地。恢复条件：对象持有与释放链 + `vm` 整体重写完成。期间验证仅 host 单测，后续各面重构以此形状为基准。

### 组件状态总表（换会话先读）

| 组件 | 状态 | 说明 |
|---|---|---|
| `os/page_table` | 目标形状，可用 | 表帧 owner 帧承载；host 30 项通过。待加：每表有效项计数（空表判定，随 vm 一并） |
| `os/memory_space/ordmap.rs` | 目标形状，可用 | 共用有界有序索引 |
| `os/memory_space/backing.rs` | 目标形状，可用 | `SegmentMap`/`RangeUse`；内核侧 `SegmentOwner` 实现待 vm |
| `os/memory_space/space.rs` | 目标形状（crate 侧），可用 | 事务三分界：`PreparedChange` →`commit`→ `CommittedChange` →`synchronize`（产出 `RetireBatch` 即 RetireGrant）→ `RetiringChange` →`complete`。ledger `regions` 已换 `OrdMap<usize, Region>`（键为起始页地址）：查找/覆盖/邻居/fault 分类按 floor/iter_from，commit 按键 remove/insert，预取槽在 reserve 取得、rollback 撤销，commit 不分配。剩余 sort/position 只作用于单次请求规模（replacements、deltas、permits）或 `max_transactions` 有界的事务表，不随全表增长。`find_hole` 仍为全表扫描（O(regions)），登记为后续：需 gap 索引时再改。host 20 项通过 |
| `os/memory_space/object.rs` | 暂留 | 写许可/seal 状态机语义符合目标，接入 Backing 时复核 |
| `os/maintenance`、`os/capability` | 目标形状，可用 | 未接入内核 |
| 内核 `task/proc.rs` 地址空间部分、`frame.rs` 的 `FundedTableFrame`/`ObjectBacking`、`task/memory_object.rs`、`task/tunnel.rs` lease 路径 | **编译中断，待连带重写** | 随 vm 拆分整体替换，不逐个修补；`TableMem` 缺 `adopt/reclaim` 是当前唯一编译错误源 |
| 内核 `deferred_work`/`work_ledger`/`notify_work`/`request`/`retirement`、`os/work_debt` | 待删除 | 随对象持有与释放链接入删除 |
| 用户态 `libkernel` mm/tunnel、`libprocess` Collector、ABI（`Rights::MAP`、Tunnel 0x60–0x63、ProcessDrain） | 待连带修改 | 随 ABI 同步面一次修改 |

下一步：本阶段 crate 侧已收尾，内核 `vm` 拼装草图见下节。接续顺序：进入性能专题（锁优化/无锁化评估，用户要求在本阶段后讨论）；随后按责任链施工方式与对象持有与释放链一起整体重写内核，恢复编译。

### 内核 vm 拼装草图（设计产物，未写代码）

- **模块**：`os/kernel/src/vm/` 从 `task/proc.rs` 拆出：`space.rs`（`AddressSpace` 实际 owner）、`change.rs`（事务执行与 Work）、`retire.rs`（整空间退休 Work）、`fault.rs`、`syscall.rs`（Map/Unmap/Protect 授权后进入）。Process 只持 `Arc<AddressSpace>`，不认识内部。
- **分页模式分发**：`enum AnySpace { Sv39(Space<Sv39>), Sv48(Space<Sv48>), Sv57(Space<Sv57>) }`，在 syscall/fault/退休入口一次 match，往下单态化。模式在进程创建时按所属调度域 hart 能力（DTB `mmu-type`）选定；内核高半区按模式各备一份共享子树，不随用户根退休。
- **状态与锁**：`AddressSpace { identity, account, gate: TicketSpinlock<SpaceState>, retire: WorkRecord }`；`SpaceState` 含 `MemorySpace`（ledger）、`TableTree<TableMem<M>, LEVELS>`、epoch、执行入口/准入状态、在途事务与访问租约计数。锁序：Process 业务锁 → `ranks::ADDRESS_SPACE` → backing（`SegmentMap`）短锁 → 帧池/HEAP；`MAINTENANCE` 队列锁为叶锁，不在空间锁内取。IPI 一律锁外发。
- **表帧 owner**：`TableMem<M>` 的 `FrameOwner` = 帧号 + 空间账户引用（不复制 charge）；`adopt` 只是身份转交，`reclaim` 凭帧号重建，退款经空间账户结算。SAFETY 论证：一个帧号在树中至多对应一个 branch PTE（由 page_table 不变量保证）。每表有效项计数随本拼装加入 page_table，供完整释放 View 时有界判定空表。
- **事务执行（Work）**：每个已受理修改一条 `ChangeWork`，准入时预付记录、表页、`OrdMap`/`SegmentMap` 节点。step：commit（PTE+ledger+epoch 同一受保护协议，分批发布）→ 产出 `SyncPlan`，Running 时登记确认目标（空间身份+事务代次+hart），Building 为空集合 → 收齐 ack 后 `synchronize` 得 RetireBatch → 按预算归还写许可、`RangeUse`、零 use 段并退款 → `complete`。从不等待 Process/Space Done。
- **整空间退休（Work）**：进程回收 RetireSpace 阶段 publish `space.retire`。step：关闭执行入口与修改准入 → 等在途事务/访问租约归零（Blocked）→ 收齐离场确认 → 全部已接纳 hart 全量 fence 并收 ack → 拿整体 RetireGrant 后按游标拆 View/use/表页并退款 → Done 唤醒进程记录。
- **fault 路径**：短暂进入空间锁读 ledger（`fault_class`）；命中扩权中区间本地 SFENCE.VMA 重试，命中撤销中区间按已撤销投递 fault。
- **入口握手**：dispatch/Resume 锁外同步所见 epoch，gate 内复检入口仍开、epoch 未变才登记执行租约；先登记后装 satp。

## 设计轮次 3 收敛结论：本链 VM 接缝（2026-10，advisor 一次 + LoneCrane 对抗）

闭合本案剩余门 1 与进程分案门 2。仅设计结论；K 取值与每步成本证明在施工门完成。

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
- 固定容量批次：每批摘表 → 新 epoch → 覆盖本批 fence → 收 ack → 放帧退款 → 复用槽，批间不做可失败准入；`PreparedTranslation.retired` 的 Vec 只留给可失败部分修改。

### 同步 epoch

- epoch 在该事务（或本段）最后一次 PTE store 后于空间 gate 下取得，last-store → epoch 发布 → FenceRequest 可消费有真实可见顺序；责任提交 ID 与同步 epoch 分离。
- 覆盖对象在同一进入 gate 下确定：此前进入者由本批 fence 或离场清除证明覆盖，之后进入者按新 epoch 同步，不用早于发布的 active 快照。
- 零目标与 remote 同一谓词“发布已 armed ∧ required mask 完成”；Building 空集合来自准入证明。RW→RX 两段各取 epoch，第一段确认到齐才加 X。

### fault

先按 R/W/X 对照事务记录中该阶段承诺权限：不允许即正常投递；允许但 PTE 未发布则锁外让出/停驻等发布依赖；已发布但本地翻译陈旧则 sfence 重试；绝不锁内循环。

### remote 运输复用

每目标最终 ack = fence 完成 ∧ 该目标槽已归还到本 Work 内嵌保留态。顺序：持稳定 completion token → fence → 归还槽 → 最后访问旧批次后 release ack → 最后者锁外 wake。批次身份含 epoch，最后 ack 后不再读写旧槽；所有目标结束前 notified 等唤醒不触发复用。`os/remote_call` 的 finish→Empty 与 generation 封顶 Retired 随本链改写为内嵌可复用生命周期。

### Tunnel 退出 VM

通用 Map 直接接受 Endpoint capability 作 backing 授权，上限为 Endpoint/Invitation 已确认的 backing authority；View 只持 backing use，不强持 Endpoint。
