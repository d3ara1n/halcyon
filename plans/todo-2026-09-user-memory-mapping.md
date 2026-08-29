# 用户内存映射机制完整化

- 状态：实施中；帧库存树与 affine 帧发布已完成，待实施切片 3
- 方向真值：[`notes/ideas/mm.md`](../notes/ideas/mm.md)
- 自然序：本计划收口后重审 ThreadSpawn 用户态资源契约，再实施线程批二/批三

## 驱动问题

当前 Running 进程只有字节粒度 sbrk 语义的 `Extend`；`ProcessMap` 只服务 Building 组装。`AddressSpace.frames` 平坦持有全部 owned backing，`external_mappings` 只登记 Tunnel 借入 VA，页表、backing、区域和 lease 没有统一的可切割所有权真值。

结构性有界帧库存和 affine 发布 seam 已完成：库存不再接触帧内容，内核在 POOL 锁外清零后才发布不可复制、可消费式切分的 `FrameTracker`。当前下一缺口是把这些 owned extents、区域几何、结果 lease、WritePermit 与事务状态收进可 host 验证的统一规划模块。

现有 `page_table::map` 在落 PTE 期间临时申请中间表帧，无法保证 AddressSpace publish 段不再失败。Tunnel Endpoint close 又会撤销 object-owned mapping；同一地址空间多 hart 运行后，本地 `sfence.vma` 不能构成关闭完成边界。区域账本、页表资源预留、Handle close、Remote Call 与 ProcessDrain 必须作为同一所有权重构闭合，不能各自长出回滚规则。

## 已确认决策

| # | 决策 | 结论 |
|---|---|---|
| D1 | backing 模型 | 混合模型：匿名 mapping 直接拥有私有 backing；共享或独立生命周期使用显式 MemoryObject。不是每次匿名分配都产生 Handle |
| D2 | 映射真值 | 地址有序区域账本是唯一稳态真值，PTE 是硬件投影；空闲洞不入账，guard reservation 与 mapping 入账；AllocationKey 只标记一次分配来源，RegionKey 标记当前 fragment |
| D3 | 映射所有权 | 普通区域归地址空间；Tunnel 等对象区域归内部 lease。普通 Unmap 不能替换、切割或解除 lease-owned 区域 |
| D4 | placement | `Anywhere` 由内核选完整空洞；`FixedEmpty` 仅空闲时成功；不提供隐式覆盖旧映射的 fixed 模式 |
| D5 | Unmap | 严格全覆盖、失败原子、精确作用于请求区间；支持部分解除并切分左右区域，不因 AllocationKey 相同而隐式清理区间外 guard |
| D6 | 权限 | 只允许 R、RW、RX；无 write-only/WX。Protect 不超过创建时冻结的上限，不能 RW→RX |
| D7 | 可执行发布 | Building 受控回填后随 Start 发布；运行期代码走 `Mutable → Sealing → Executable`，WritePermit 覆盖 reserved/published/retiring，最后一个 permit 经 shootdown retire 后才完成 seal |
| D8 | MemoryObject 收束 | 可由普通 close 析构的 MemoryObject 必须固定长度且有硬容量上限；解除上限前必须设计显式 drain |
| D9 | shootdown | 完整 PTE store + request release + IPI 前 `FENCE RW,RW`；目标 acquire 后 `SFENCE.VMA`/可选 `FENCE.I` + ack release；完成与 Retire 逐项 ack acquire |
| D10 | 用户接口 | 内核 ABI 以地址区间为真值；rinlib runtime 内部以 affine region 管理 allocator arena 与线程栈；连续堆顶和线程专用栈 syscall 退役 |
| D11 | 模块形态 | AddressSpace 是统一深模块；Building、Running、bootstrap、Tunnel 只提交意图与 authority，不操作账本、extent、PTE、epoch 或请求槽 |
| D12 | 事务语义 | `Validate → Reserve → Commit → Publish → Synchronize → Retire → Complete`；Commit 前失败零业务副作用，Commit 后必成，调用完成时全局同步完成 |
| D13 | 并发可见性 | 不承诺任意多页在所有 hart 上瞬时切换；未同步并发访问在调用进行期间可观察旧状态或逐步发布状态，应用负责区间使用同步 |
| D14 | 性能纪律 | 库存锁只 claim/return 元数据；extent 清零在锁外、Commit 前完成。地址空间 spinlock 不跨清零、IPI 或 park |
| D15 | close 完成 | 撤销 object-owned mapping 的显式 HandleClose 在消费 entry 前预留事务，提交后必要时异步等待；ProcessDrain 保存可恢复 pending close |
| D16 | Map 结果交付 | Reserve 确定范围并验证 committed 初值为零的稳定输出槽；先写四个范围字段，最后在 execution gate 内以 release 写入 request cookie 作为 Commit；Commit 后不再 uaccess |
| D17 | active/终止闭包 | execution gate 下取 active/成员序列/准入快照，锁外预留目标槽，Commit 前在同一 gate 复检；enter/leave 与 Running→Terminating 截止共享该线性化 |
| D18 | 线程消散 | Commit 后线程只消散回复权；departed/join 不越过挂接于线程结果记录的 transaction，接管者在 join 后可恢复 Map 结果 |

## 目标模块

最终内核所有权关系：

```text
AddressSpace
├── RegionLedger       range / guard / protection / owner / AllocationKey / RegionKey
├── Backing            OwnedExtents / ObjectView
├── TranslationTree    PTE 与页表子树所有权
├── MemoryChange       validate / reserve / commit / publish / synchronize / retire
├── EpochState         stable identity / epochs / pending acknowledgements
└── DrainState         pending close / transaction / region / table progress
```

active-hart 成员关系仍由 Process 生命周期唯一拥有；AddressSpace 只通过内部 execution seam 取得快照、复检成员序列与事务准入、发布 epoch 义务并接收 enter/leave 确认，不复制成员位图。该 seam 同时线性化 Running→Terminating 截止。

外部调用者只使用少量按意图划分的 AddressSpace interface。纯逻辑几何、权限、owner 与事务规划放入 host 可测模块；实际 extent、页表帧、Remote Call 槽和 WaitContext 由内核 adapter 持有。内部 seam 可以为测试和适配分层，但不能泄漏为 Building、Tunnel 或 syscall 需要共同理解的协议。

Remote Call 仍是独立 hart 间短动作传输模块，不并入 AddressSpace；AddressSpace 是其真实消费者和业务事务所有者。物理 frame inventory 同样保持独立，只向内核帧所有权 adapter 提供 claim/return。

## 本计划范围

本计划交付：

- 帧库存锁外清零和不可伪造、可切分的 affine extent 所有权；
- 统一区域账本、guard、匿名 backing、ObjectView、AllocationKey/RegionKey 与部分切割；
- reservation-aware 页表资源准备和显式用户/内核子树所有权；
- 固定容量 MemoryChange、Remote Call 与 translation/instruction epoch；
- Building、bootstrap、Tunnel 与 Running 调用者迁入同一 AddressSpace seam；
- shared/rinlib 的 `MemoryMap`、`MemoryUnmap`、`MemoryProtect`；
- rinlib allocator arena、线程栈 guard 与 affine `MappedRegion`；
- `AddressSpace.frames`、`external_mappings`、brk/Extend 和旧本地 shootdown 路径删除；
- host 纯逻辑测试、锁持有验证与 RISC-V 双 hart 竞态验证；
- 每个实现切片落地后才同步对应 `notes/impls/`。

公共 MemoryObject 的 ObjectKind、HandleRole、创建/seal ABI、pager、COW、文件缓存与帧 capability 不在本计划公开；这些对象面由 [`todo-2026-09-ipc-data-plane-design.md`](todo-2026-09-ipc-data-plane-design.md) 承接。本计划仍实现并验证 AddressSpace 所依赖的内部 ObjectView、WritePermit 与 `Mutable → Sealing → Executable` 状态 seam；Tunnel 的既有固定 backing 作为真实 ObjectView/lease 消费者，不能以“公共 ABI 延后”为由跳过 backing 级 W^X 所有权。

## 迁移纪律

- 不把现有实现当兼容约束；旧字段、旧调用号和旁路机制在最后一个消费者迁移的同一切片删除。
- 不在旧 AddressSpace 外包一层新 ledger adapter；账本、backing、PTE 与事务所有权最终共同进入一个模块。
- 实施可以按调用者分批，但新旧路径不能同时拥有同一类区域；每一批都用数量守恒和失败原子测试证明唯一所有者。
- 纯逻辑模块只输出意图规划和状态转换，不持内核对象或执行硬件副作用；内核 adapter 不重新解释几何与权限。
- 单次请求页数、节点、PTE 步数、事务槽和 Remote Call 槽都有共享硬上限；性能优化在这些结构内进行，不以无界队列、忙等或 stop-the-world 换取简化。
- Commit 嵌套遵守既有 Lock Ladder：`ADDRESS_SPACE → LIFECYCLE/execution gate`；终止在 gate 内只发布截止，解锁后才接管 AddressSpace。MemoryObject state lock 不与 AddressSpace lock 嵌套，以 affine permit token 锁外移交。

## 实施切片

### 1. 结构性有界帧库存

- 状态：已完成（2026-09）

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| free inventory 与 claimed extent 互斥且总量守恒；任一步数不随碎片数增长 | DT memory arenas、启动 reservation 集合 | canonical free tree 或唯一 claimed physical extent |

`os/frame_pool` 已建立外置元数据的 canonical arena/order 树，分配、split、coalesce、`alloc_at`、reservation 发布和归还不再随全局碎片数扫描；普通 backing 已能由多个 power-of-two extent 组成。

验证：15 项 host 测试覆盖多 DT region、各 order、指定区间、碎片、reservation、失败原子、重复归还、数量守恒和 arena 数上界；`just check`、virt debug/release/hetero/nofd 与 sifive_u 已通过。

### 2. 帧发布与 affine extent

- 状态：已完成（2026-09）

目标是完成 frame inventory 与 backing 所有权之间的最终 seam，不在后续 AddressSpace 内补安全包装。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| 未清零 extent 不得发布；token 不可复制、伪造或产生重叠 split | inventory 返回的 claimed physical extent | 已初始化 affine FrameTracker，或失败时完整归还 inventory |

- 从 `os/frame_pool` 删除帧内容访问职责；claim/return 只修改树元数据并返回物理 extent；
- 内核 adapter 在 POOL 锁外清零已独占 extent，清零完成后才构造可发布 tracker；页表、堆、Tunnel 和用户 backing 不取得未初始化 token；
- `FrameTracker` 字段私有化，提供只读几何和消费式 split；禁止任意构造、复制、重叠 fragment 与隐式 forget/rebuild；
- 为页表所有权移交、BootPackage payload 收编和 Drain 建立显式 adopt/transfer seam，不再靠公开字段伪造 tracker；
- 保留 order 树、启动元数据 reservation 与既有库存测试，不重写已正确的树机制。

验证：纯库存 API 已删除帧内容后端；15 项 host debug/release 测试覆盖 extent 几何切割、库存失败原子、重复归还与数量守恒。内核启动自检覆盖锁外清零、消费式 split、分片归还计数与再次清零；页表 transfer/adopt、BootPackage reservation adopt、Drain table adopt 和堆 permanent transfer 均由真实负载覆盖。`just check`、virt debug/release/hetero/nofd 与 sifive_u 全部通过。

### 3. MemorySpace 纯逻辑规划

建立 host 可测的内部规划模块；它是 AddressSpace 的内部 seam，不是第二套公开地址空间。

**Entry gate**：切片 2 的 FrameTracker seam 与数量守恒测试已完成；`notes/ideas/mm.md` 中 AllocationKey/RegionKey、精确 guard 切割、Commit/UserWriteLease 边界和 `Mutable → Sealing → Executable`/WritePermit 状态机保持无待决语义。未满足时不得为 ledger key、事务阶段、结果 pin 或 ObjectView permit 编码。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| ledger fragment 是唯一范围真值；ReservationGroup 不复制区间；planner token、UserWriteLease 与 WritePermit affine | 映射意图、预留的节点/事务容量、OwnedExtents 或 ObjectView/WritePermit | PreparedChange 独占 publish 资源；Commit 后由 ledger、retiring fragment 与 transaction 分别唯一持有 |

- 有序 RegionLedger 记录 reservation/mapping、匿名或对象 view、对象偏移、当前/最大权限、Process/Lease owner、AllocationKey 与当前 RegionKey；
- 一次 Map 建立不含第二份范围真值的 ReservationGroup；精确 Unmap 覆盖 full reservation、usable-only 后 guard-only fragments、guard-only 与 mapping 中段，切割消费旧 RegionKey 并铸造左右/retiring key；只允许同 owner、种类、AllocationKey、backing 连续性与权限的 fragment 合并；
- 固定区域与事务容量，所有增加节点的操作先 reserve；实现 Anywhere、FixedEmpty、双 guard、兼容合并、严格 Unmap、部分切割和 Protect；
- MemoryChange 状态机明确 Validate/Reserve/Commit/Publish/Synchronize/Retire/Complete，以及 Commit 前 rollback、Commit 后接管和完成边界；
- planner 表达 WritePermit 的 reserve/publish/retire，及 Mutable/Sealing/Executable 与新写许可的线性化，不通过扫描 PTE 判断 seal；
- 结果槽以固定宽 UserWriteLease pin 到 Commit/rollback，并保存不重入 uaccess/AddressSpace lock 的有界写入投影；Unmap/Protect 冲突返回 Busy，不依赖“单事务”简化保证写回稳定；
- 规划结果描述 PTE install/remove/protect、OwnedExtents split/retire、ObjectView 偏移和所需资源上界，不执行硬件动作；
- fault lookup 只区分 free hole、guard、eager mapping，不建立未实现 pager 状态。

验证：表驱动、状态机和模型测试覆盖几何溢出、冲突、容量、四类 guard/reservation 精确切割、AllocationKey 保持与 RegionKey 消费、同组合法合并与跨组拒绝、UserWriteLease vs Unmap/Protect、object offset、lease 拒绝、权限上限、事务 Busy、Commit 前发起者消散和每个失败点零业务副作用；另覆盖 seal vs 新 writable Map/Protect、最后一个 permit 在 ack 前不得退出计数、seal 发起者消散及 Executable 不可回退。

### 4. 页表资源预留与所有权

使 TranslationTree 可以被 MemoryChange 安全发布，不在 PTE 修改中途临时申请资源。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| Publish 不分配、不失败；叶 backing 永不归页表所有；共享内核子树不被用户树 drain | 已清零 table frames 与 planner 的 PTE intent | TranslationReservation 未用部分归还，已用部分成为 TranslationTree 的唯一子树所有权 |

- 为 Map、mega split 和 Protect/Unmap 所需细化建立 preflight，精确或保守计算固定上限的中间表帧需求；
- reserve 阶段取得并清零表帧，publish 只链接已准备子树与写 PTE，不再返回 FrameExhausted；未消费 reservation 自动归还；
- 明确用户子树、共享内核高半区和栈窗口槽的所有权，地址空间 drain 不再靠 `clear_slots`/`leak_root` 手工知识配对；
- 保持页表 crate 不拥有叶数据 backing；PTE plan 与 RegionLedger plan 在 AddressSpace 内组合，不互相成为第二真值。

验证：host 覆盖最坏表帧数、mega split、资源不足零修改、publish 不失败、共享子树不回收、未消费 reservation 归还与整树 drain 数量守恒。

### 5. Remote Call 与完成闭包

由地址空间 epoch 这一真实消费者建立独立 Remote Call 模块。

**Entry gate**：切片 4 已提供不失败的 PTE Publish；RISC-V 规范依据、逐边 release/acquire 表、IPI 前 data fence、instruction epoch、execution gate 的 snapshot/revalidate/enter/leave 线性化，以及 `ADDRESS_SPACE → LIFECYCLE/execution gate` 锁序已在 `notes/ideas/mm.md` 固定。任何一条若只能靠“通常会按序执行”或反向取锁解释，禁止进入实现。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| request publication happens-before 目标 fence；每个快照成员恰有确认义务；全部 acquire ack 前旧资源不可 Retire | MemoryChange 独占的目标快照、未来 epoch 与每 hart reserved slot | ack 后 slot 归 Remote Call inventory；epoch 义务归零后 retiring backing/permit 才交给 Retire |

- 每 hart 固定容量请求槽，IPI 只作门铃；请求携带 tag、地址空间稳定 identity、translation/instruction epoch、失效范围和完成引用；
- Reserve 在 execution gate 下记录 active 集合、成员序列与事务准入状态，锁外预留全部目标槽；Commit 前重进 gate 复检，变化则零业务副作用回滚或重试；
- Commit 按 `ADDRESS_SPACE → LIFECYCLE/execution gate` 取锁，在 gate 内以完整 PTE store 发布页表与 epoch，并以 release 发布请求；释放两锁后执行 `FENCE RW,RW` 再发 IPI；
- SSIP 与其它返回用户态出口以 acquire 取得请求，验证 identity/epoch，执行本地 `SFENCE.VMA`/可选 `FENCE.I` 后 release 确认；
- 发起 hart 若在 active 快照内也走同一 acquire/fence/release-ack 路径，不以“本地页表写入者”身份跳过确认；
- 完成者逐项 acquire ack 后才 Retire，并以 release 清空槽；请求发布后不可取消，发起线程终止只放弃回复；
- Process 生命周期继续唯一拥有 active-hart 集合；快照后的 enter 在返回用户态前同步新 epoch，快照内 leave 在清 active 前完成确认。

验证：host adapter 确定性覆盖容量不足、成员序列/生命周期准入复检失败、Commit vs Running→Terminating、Lock Ladder 正序与反向拒绝、本地发起 hart 确认、门铃合并/丢失后由 pending level 补消费、乱序确认、重复门铃、slot ABA/identity 误配、快照前后 enter/leave、发起者终止和最后确认接管；RISC-V 双 hart litmus 证明 cookie/PTE/request 发布顺序、Unmap 后 backing 复用屏障及 instruction epoch。目标 hart 每项工作保持固定上界。

### 6. AddressSpace 替换与现有调用者迁移

组装最终 AddressSpace 深模块，并按调用者逐批切换；每批迁移同批删除对应旧路径。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| 同一 VA 只由新 AddressSpace 或旧路径之一拥有；迁移批次不产生 adapter 双真值；close/drain 复用同一 retire | 旧 AddressSpace 字段、Building backing、Tunnel lease 与页表所有权 | 统一 ledger/backing/TranslationTree/MemoryChange/DrainState；对应旧字段同批删除 |

1. **Building/bootstrap**：ProcessMap/Write、initial ELF、StartupBlock 与 BootPackage payload 使用 RegionLedger + OwnedExtents + reserved TranslationTree；删除平坦 backing 的对应路径。
2. **Tunnel/Handle close**：Connection backing 建立 ObjectView，Endpoint 持不可伪造 lease RegionKey；删除 `external_mappings` 与按 VA 搜索。HandleClose 以 pin → prepare → consume → commit 提交，必要时 Waiting；Drain 的 pending close 可在事务 Busy 时恢复推进。
3. **稳定身份与 drain**：satp/root/epoch 身份从可变账本锁分离；AddressSpace drain 顺序收束 pending close、已发布事务、区域 backing 和拥有的页表资源，不认识调用来源。

显式 close 返回前完成相关远端确认；进程 Terminating 在 active 集合归零后仍通过同一 retire 机制完成，不借“目标不再运行”绕过账本和 backing 所有权。迁移完成时 `AddressSpace.frames`、`external_mappings`、MappingLease 的 VA 所有权和本地-only unmap 路径全部删除。

验证：Building failure、bootstrap payload 收编、Tunnel create/attach/close、HandleClose vs close、Drain 接管和帧计数全覆盖；现有 virt/release/hetero/nofd/sifive_u 行为等价。本切片尚不公开 Running 内存 ABI。

### 7. Running ABI、rinlib 与 Extend 退役

在 `shared/`、内核和 rinlib 同步加入固定宽、保留字段清零的 `MemoryMapRequest`、`MemoryMapResult`、`MemoryProtection` 与 `MemoryPlacement`：

**Entry gate**：切片 3 的精确 region/group 与 seal/permit 模型、切片 5 的 shootdown happens-before、切片 6 的统一 AddressSpace seam 均已实现并通过各自验证；`MemoryMapRequest/Result` 的非零 cookie、末字段 release commit、稳定结果记录与 thread departed/join 挂接规则已经冻结。未满足时不得分配调用号或编写 rinlib wrapper。

| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| committed cookie 为零则无新 mapping；cookie 匹配则事务必成；rinlib region 只能消费一次；join 后结果与 mapping 均已完成 | 用户 request、UserWriteLease pin 的稳定 result record、AddressSpace PreparedChange | Commit 前错误归还全部资源；Commit 后 ledger 持 mapping，rinlib affine region 持解除责任 |

- `MemoryMapRequest` 携带非零 result cookie；`MemoryMapResult` 固定包含 usable base/bytes、reservation base/bytes 和自然对齐的末字段 committed cookie，调用前 committed 与全部保留字段必须为零；
- `MemoryMap`：当前进程隐式为目标；本批只开放 Anonymous，placement 为 Anywhere/FixedEmpty，声明 bytes、guard 与 R/RW 权限，匿名 RX 拒绝；
- Reserve 以 UserWriteLease 稳定 result record、生成有界写入投影并完成全部资源准备，先写四个 payload 字段；Commit 按既定锁序复检 active 与事务准入后，通过 lease 以 release 写 cookie。cookie 为零时 payload 无语义；Commit 后释放 lease，publish 后需要远端确认则通过 WaitContext park，完成阶段只写保存的 syscall 状态，不再 uaccess；
- `MemoryUnmap`：严格全覆盖、精确区间解除普通 mapping/reservation，不隐式清理同组区间外 guard；
- `MemoryProtect`：只在最大权限内变化，不提供 RW→RX；
- Commit 前失败返回明确区分 IllegalArgument、AddressConflict、NotMapped、RightsDenied、ReachLimit、OutOfMemory、ObjectBusy 与 ObjectClosed；Commit 后不再返回业务错误；
- rinlib runtime 内部建立 affine compound `MappedRegion`，完整 unmap 消费自身，部分 unmap 返回请求洞左右至多两个 mapped 或 reservation-only fragments；
- 可能 park 的 Map 使用随线程保留到 join 的结果记录；线程消散后由 join 接管者以 acquire 读取 committed cookie，且 departed 不早于该 transaction 完成；
- allocator 改持多个匿名 arena，线程模块用通用 mapping 建立带 guard 的 UserStack；应用不接触堆 mapping；
- 最后一个消费者迁移后，同批删除 shared `Extend`、rinlib wrapper、内核 brk/extend 和固定预映射栈心智模型。

### 8. 验证与文档收口


| 本切片不变量 | 进入所有权 | 退出所有权 |
|---|---|---|
| 每项完成判据都有 host 模型或 RISC-V 负载；文档事实不领先代码；失败后库存、ledger、PTE、permit、Handle 与 slot 总量守恒 | 全部切片产物与平台矩阵 | 可归档计划、已同步 impls 与解除阻塞的 ThreadSpawn 前置 |
host 必须覆盖：

- 库存锁外清零、affine extent 与数量守恒；
- 区域/事务 planner 的正常、冲突、容量、四类 guard 精确切割、AllocationKey/RegionKey、同组合并、UserWriteLease、Protect、lease 与 Commit 前失败原子；
- MemoryObject seal vs writable permit 的 reserve/publish/retire 竞态，确认前计数不下降，Executable 不回退；
- 页表资源预留、Publish 不失败、共享子树与 drain；
- Remote Call 门铃、epoch、逐边 release/acquire、成员序列/生命周期准入复检、Commit vs Terminating、Lock Ladder、乱序确认、终止接管、slot ABA 与事务 Busy；
- Map 结果 payload/cookie 次序、Commit 前消散、Commit 后消散、departed/join 接管；
- Map/Unmap/Protect/HandleClose 所有失败点的账本、PTE、backing、permit、Handle 与请求槽守恒。

RISC-V 负载必须覆盖：

- 双 hart 同地址空间的完成边界；调用进行期间不对未同步访问作瞬时可见断言；
- 一 hart 缓存旧映射，另一 hart Unmap 完成后重映射不同 backing 不得读到旧页；
- 权限收窄、instruction epoch 与 guard fault；
- Unmap vs ThreadExit/ProcessKill、shootdown ack vs termination；
- Endpoint HandleClose、Tunnel lease close 与普通 Unmap 竞争；
- allocator 多 arena 与次线程栈建立/释放。

阶段收尾执行全部 host 测试、`just check`、`just virt`、`just virt-release`、`just virt-hetero`、`just virt-nofd`、`just sifive_u`。实现事实只在相应切片落地后写入 `notes/impls/{mm,call,internals,task,ipc}.md`；COMPASS 在全链收口时把主线移到 ThreadSpawn 契约重审。

## 对 ThreadSpawn 的解除条件

以下条件全部成立后，`todo-2026-09-thread-model.md` 批二才能重开：

- 用户态可建立带未映射 guard 的次线程栈；
- join 后可在内核确认线程离场之后安全解除并复用完整 reservation；
- 同地址空间多 hart 下解除映射不会留下 stale translation；
- 栈、wrapper 上下文、结果槽、MappedRegion 与 join handle 的所有权由安全 rinlib interface 完整表达；
- Extend 已退役，ThreadSpawn 不依赖临时堆块或固定预映射栈池。

## 完成标准

- AddressSpace 对所有调用者呈现同一个深 seam，删除旧字段、旧 brk 和旁路 PTE 操作；
- frame inventory 锁不覆盖 extent 清零，地址空间锁不覆盖远端等待；
- extent、RegionKey、OwnedExtents/ObjectView、UserWriteLease、WritePermit、PTE、Handle close 与事务所有权均 affine 且可从结构推导；
- ReservationGroup 不复制区间真值；full、usable-only、guard-only 与中段 Unmap 都只改变请求区间，跨空洞/owner 失败且不留部分状态；
- Commit 前失败零业务副作用；Map committed cookie 为零时 payload 无语义且无新 mapping，匹配时后续必成且不再 uaccess；成功返回或 join 接管时全局同步完成；
- shootdown 的 PTE/request/ack/retire 每条 happens-before 边都有明确 release/acquire 或架构 fence，enter/leave 与快照不存在漏 hart 窗口；
- Mutable/Sealing/Executable 不能回退；任一 writable permit 在远端确认前仍计数，Executable 下不存在写入口或 writable stale translation；
- 帧库存、单次请求、事务、Remote Call、seal 完成槽和最终 drain 均有可证明硬上界；
- 正常、冲突、OOM、部分解除、并发和终止路径保持区域/PTE/backing/permit/Handle/slot 守恒；
- guard fault 和所有用户参数错误只产生规定错误或用户 fault，不 panic 内核；
- debug/release、virt/hetero/nofd/sifive_u 与 host 验证全绿；
- impls 只记录实际结构，方向与计划不冒充实现；
- ThreadSpawn 阻塞解除并重新审定用户态资源契约。
