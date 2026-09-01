# 内存模型

内存管理同时维护物理资源所有权、地址空间隔离和映射生命周期。地址空间区域账本是映射关系的唯一真值，页表只是它的硬件投影；物理库存、backing、虚拟区域、对象 view 与借出 lease 必须各有唯一所有者。所有调用者通过同一个地址空间深模块提交意图，不各自组合帧、PTE、回滚和地址翻译同步。

## 物理内存

物理帧是库存资源，以 affine extent 所有权表达。平台交付的内存范围不是天然可用集合：固件与设备保留区、内核永久占用和仍被启动环境使用的区间必须先统一规范化并扣除，重叠、越界或来源矛盾应使启动失败。其余供给在任何普通分配发生前划为互不借用的系统储备与用户供给；系统储备用于内核堆、完成路径和恢复所需的固定资源，不能在压力下退化为用户池的隐式透支。

系统储备不是与用户库存共享同一分配器后附加的额度标签。用户 FramePool 只发布 user supply；系统页在发布前由 `SystemSupply` 以用途类型接管，FramePool metadata、内核 heap chunk 与 recovery ticket 不能互相伪造或转成用户 extent。库存 metadata 按实际 RAM 几何精确计算；heap ticket 数是编译期物理容量政策，耗尽时普通 metadata admission 失败，不能借 user supply，也不冒充按对象精确计费。recovery ticket 数必须由 Commit 后路径的静态并发上界导出；没有这类消费者时可以为零，新增消费者必须先扩充预算。平台供给无法满足完整子预算时 fail closed，不按内存比例缩减正确性承诺。

内核堆、用户物理库存和进程用户堆是三个独立层次。内核堆只承载 metadata 与对象壳，由预清零的固定连续 heap tickets 单向扩展，可以针对小对象、固定物理容量与失败可预测性优化；对象级存量与多租户隔离由正交的 metadata admission 负责，不能从 heap 容量反推。用户 FramePool 只做 MemoryPool-backed extent 的物理取得，可以针对 order、碎片、锁外清零与 funding 事务优化；进程在取得 funded anonymous mapping 后由用户态 allocator 自行细分虚拟区间，可以按服务 workload 选择 arena、slab 或其它策略。三者不共享配额、回退路径或 allocator 政策，只在物理页唯一所有权上闭合。

平台保留内存不是同一种生命周期的区间集合。静态专用区永久排除普通供给；`no-map` 同时从内核标准直映射的 admitted ranges 中扣除，禁止无 owner 控制的虚拟映射与推测访问；动态保留请求必须在普通供给发布前按大小、对齐和允许范围整体放置；`reusable` 区始终属于平台 region owner，只能以可撤回 loan 暂借给可丢弃、可重建或可迁移的 backing。不能闭合动态放置或 reclaim 协议的平台描述必须明确拒绝，不能把未知语义降级为普通永久 reservation。

内核直映射由规范化 admitted ranges 定义，不由固定大页清单定义。页表构造器对每段连续范围 eager 建表并自动选择硬件允许的最大叶，洞与边界才下沉到更小粒度；启动所需中间表来自与用户供给隔离的固定系统预算，预算不足是平台 admission 失败。平台名称、模拟器行为或自备 DTS 只提供输入事实，不改变该契约。

用户供给中的空闲库存不能依赖由自身供血的通用堆。启动阶段尚未回投但未来属于用户供给的区间以 boot-held token 保留；运行期 claim、funded extent 与 returning extent 同样各有唯一 affine 所有者。任一时刻的物理状态集合互斥，静止点满足：

```text
user_supply = free_inventory + boot_held + funded_extents
```

事务窗口可以暂时出现 claimed-but-unfunded 与 returning 状态，但二者必须由同一 funding 事务拥有，不能同时出现在其它桶中；只有冻结新事务并排空已提交 retire 后才能作全局供给断言。root MemoryPool 的固定总额度恰等于 user supply，因而页数上不超配；系统储备、永久页和设备专用内存均不进入该额度。

库存只决定哪些帧已经从空闲集合原子取走，不在库存锁内完成与页数线性的内容初始化。claim 成功后，调用者独占 extent，先在库存锁外清零，再把“已初始化 backing”发布给页表或对象；初始化失败或事务放弃仍由 affine funding 自动归还。单次可清零页数、extent 数、PTE 步数和其它不可中断工作各有独立硬上限，平台测量只用于选择常量，不替代静态工作边界。

内核 heap source 不在 allocator 忙碌期间进入用户 FramePool 或执行大块清零。启动期 planner 预留并清零固定粒度 heap chunks；运行期扩堆只从专用 ticket 序列做 O(1) 单向消费。recovery tickets 独立保留，不能因普通 heap 压力自动解锁；具体完成路径只有在取得对应 typed permit 后才能消费。

精确长度不要求物理连续。普通用户 backing 由若干 extent 组成；需要连续物理区间的页表、DMA 或其他消费者必须显式请求受 order 上限约束的连续块。extent token 不允许复制或任意构造；切割消费原 token 并返回互不重叠的剩余 token，合并只接受物理相邻且所有权语义相同的 token。失败回滚、延迟 retire 与正常归还遵守同一数量守恒。

普通对象只在持有固定容量 backing 时适用有界 close；进程地址空间等宽度可增长的容器以有界 drain 释放。任何解除容量上限的 backing 对象都必须先取得可恢复的 drain 生命周期，不能把无界页遍历放进最后一个引用的析构。

## 单一地址翻译模式

统一内核运行环境只选择一个所有 admitted hart 都支持的地址翻译模式。共享物理内存配多套模式会复制内核映射、割裂线程迁移和回收契约，却不增加隔离能力；若不存在共同支持模式，启动应明确失败。

内核映射在所有用户地址空间中保持一致，使用户 trap 可以直接进入共同入口。用户半区属于进程，内核半区属于系统；共享内核页表子树与地址空间拥有的用户子树必须由显式所有权记录区分，建立、借用和收束经过同一地址空间模块，不能依赖创建与 Drop 调用点的手工配对知识。

## AddressSpace 深模块

每个进程从创建起拥有一个稳定 AddressSpace 身份，但 Building 空壳的地址空间最初可以不持有页额度、区域账本或页表。AddressSpace 以一次性状态转换表达资源附入：

```text
AddressSpace
├── Unbound
└── Bound
    ├── PoolBinding
    ├── RegionLedger       虚拟区域、权限、owner、AllocationKey 与 RegionKey
    ├── Backing            OwnedExtents 或 ObjectView
    ├── TranslationTree    PTE 与页表子树所有权
    ├── MemoryChange       validate → reserve → commit → publish → synchronize → retire
    ├── EpochState         translation/instruction epoch 与待确认义务
    └── DrainState         区域、事务与页表的可恢复收束进度
```

`ProcessBindMemory` 是 Building authority 下唯一的 Unbound → Bound 操作：它消费具绑定 authority 的 MemoryPool Handle，先以 Building operation lease 冻结提交资格并 pin 源 entry，再在锁外准备不可转移的 PoolBinding 与根页表。发布时复检 lease 仍有效且 AddressSpace 仍为 Unbound；期间发生的终止截止不能撤销已经登记的资格，若目标已进入 Terminating，成功发布的 Bound 状态直接成为收束方接管的资源。Bound 发布同时把 pinned entry 标记为逻辑已消费，是唯一提交点；随后摘槽和释放 pin 只是不可失败尾段，不能向调用者恢复 Handle。提交前失败保持 Pool Handle 和空壳不变；提交后绑定不可替换、转移或重新计费。未绑定时 Map、Write、线程附入和 Start 均不成立，Handle grant 等不依赖地址空间的组装动作仍可独立完成。bootstrap 对 initial process 使用同一内部绑定语义，不形成第二种地址空间创建机制。

AddressSpace 的外部 interface 只接受绑定、映射变更意图、调用 authority 与有界 drain，不向 Building 组装、Running syscall、bootstrap 或对象 lease 暴露区域容器、extent 排列、页表步骤、epoch 或 Remote Call 请求。Unbound drain 是空操作；Bound 地址空间的全部普通释放必须经可恢复 drain，最后析构只能验证树已清空并做常数工作，不能递归替代收束。

Building 与 Running 只在 authority、允许的来源和生命周期阶段上不同，共用 Bound 地址空间中的同一变更机制。Tunnel 等对象以内部 lease authority 建立和撤销 view。新的映射来源或调用通道必须进入这个 interface，不能旁路建立 PTE 或另建地址占用表。

地址空间身份独立于可变账本锁；Bound 后拥有稳定 translation epoch。active-hart 成员关系的唯一真值仍属于进程执行模型；AddressSpace 通过固定内部 interface 取得发布时快照，并只拥有由该快照产生的 epoch 义务与完成确认，不维护第二份成员集合。Process 的短 execution gate 同时线性化 Building 组装准入、Start、enter/leave、Running 事务 Commit 与终止截止。Building operation 只可在精确 Building 状态登记，登记即冻结该操作的提交资格；Start 只能在除自身外没有组装操作在途时发布 Running，终止可以先发布截止但必须接管并等待已经登记的操作完成。Running 地址空间事务则以 Commit 为胜负点：终止先线性化时未 Commit 事务回滚，Commit 先线性化时终止路径接管该必成事务。

Commit 与终止竞争共享的 execution gate，其嵌套顺序（AddressSpace commit lock 在外、gate 在内，终止不反向嵌套）由[任务模型](task.md)唯一拥有。MemoryPool、MemoryObject 与 AddressSpace 的状态锁之间不跨调用长期持有：额度、WritePermit 和其它 affine token 在各自 owner 锁内预留后转入 PreparedChange，rollback/retire 先从 AddressSpace 摘出 token，解锁后再回到来源对象推进计数。短锁只保护账本、状态和有硬上限的发布；帧清零、用户态工作和远端确认等待均不得持锁。

## 区域账本

区域按虚拟地址排序。空闲洞不入账；guard 等 reservation 入账但不安装 PTE；mapping 同时记录虚拟区间、backing view、对象内偏移、当前权限、权限上限和解除所有者。任一稳态有效用户 PTE 必须恰由一段 mapping 覆盖，reservation 范围内不得存在有效 PTE。

地址空间拥有普通 mapping 与 reservation。对象可以借出由内部 lease 拥有的 mapping；这类区域仍进入同一账本参与冲突检测，但普通内存操作不能替换、切割或解除，只有对应 lease 可以撤销。地址空间最终收束前，全部 lease 必须先关闭或被进程收束接管。

部分解除在请求边界切分区域。匿名 mapping 的 OwnedExtents 随区域切分；解除的中段转入事务 retire 所有权，只有相关 hart 确认旧翻译失效后才归还库存。对象 mapping 只切分 ObjectView 并调整对象内偏移，不改变对象对 backing 的所有权。账本决定以上关系，PTE 的存在或解除本身不决定帧归谁。

一次成功 Map 建立一个内部 `ReservationGroup`，以不可复用的 `AllocationKey` 标记共同来源；完整 reservation 的 guard 与 mapping 都引用该组。每个当前 ledger fragment 另有唯一 `RegionKey`。切割消费旧 fragment 及其 key，为存活左右片段和 retiring 中段分别铸造新 key；`AllocationKey` 保持不变，直到最后一个 ledger 或 retire fragment 消散。ReservationGroup 不保存第二份占用区间，也不授予按组操作的隐式 authority；区域账本中的当前 fragment 集合始终是范围真值。

普通 `MemoryUnmap` 只作用于请求区间，不因命中某个 AllocationKey 而解除区间外同组 fragment。解除完整 reservation 会同时移除 guard 与 mapping；只解除 usable mapping 会留下两侧 reservation-only fragment；只解除 guard 或 mapping 中段也只切割对应范围。请求跨空洞、越界或不同解除 owner 时整笔失败。对象 lease-owned fragment 仍只能由 lease authority 撤销。

ledger 只在 owner、区域种类、AllocationKey、backing identity/连续 offset、当前与最大权限全部相同且几何相邻时兼容合并；合并同样消费旧 RegionKey 并铸造新 key。不同 AllocationKey 的相邻区域保持独立，不能为了节省节点丢失分组来源。

## Backing

匿名 mapping 是最常见路径，其私有 OwnedExtents 由 mapping 直接拥有，不为每次分配产生用户可见 Handle。区域切割消费原 affine backing，存活片段与 retiring 中段各自取得互不重叠的 extent 和同源 charge；retiring slice 在地址翻译确认前不得归还。需要共享或独立于某个地址空间存活的字节才使用 MemoryObject。

MemoryObject 是固定长度的共享 backing identity，不是地址空间、虚拟区域或用户态分配器。Handle 的 `MAP` right 授权建立新 view，`READ`、`WRITE` 与 `EXECUTE` 分别约束 view 可请求的数据和执行权限；mapping 建立后以 ObjectView 强引用独立保活对象。对象 backing 始终唯一拥有完整 extents 与 charge，任一 view 的切割、降权或解除只变换 ObjectView 和 WritePermit，不切割或重复持有数据 backing。同一对象可以按不同地址和更窄权限映入多个进程。

可由普通 Handle close 触发最终析构的 MemoryObject 必须受硬容量上限约束，长度创建后不可改变。更大的逻辑对象由用户态协议组合多个 MemoryObject；未来若需要无界对象、resize、COW、文件缓存或 pager，必须先为对象建立有界 drain 与明确的缺页及 funding 协议，不能改变既有 eager mapping 的成功语义。

## MemoryPool 与 backing charge

MemoryPool 是 page-backed storage 的 capability 账户，不是物理分区，也不代表全部内核内存。root pool 的固定总额度由可信 user supply 铸造；子池只转移额度，所有池继续从同一用户帧库存取得物理 extent。Pool Query 观察的是资源 authority 与当前占账，不承诺物理连续性、特定位置或某次请求能满足 extent 上限。连续 DMA、设备内存和未来 overcommit/pager 由各自契约拥有。

每个 Pool core 维护固定容量的四项守恒：

```text
total = available + reserved + allocated + delegated
```

`ChargeReservation` 从 available 预留额度，放弃时原额回滚；物理 backing 全部取得后，reservation 不可失败地提交为与 extent 同寿命的 `MemoryCharge`，计入 allocated。`MemoryCharge` 不可复制，只能在同一 pool 内随 backing 做守恒 split/merge，最终归还 allocated。派生子池把 reservation 提交为与 child core 不可分离的 `ParentCredit` 并计入 delegated；只有销毁全部额度已回到 available 的 child，才能兑回这笔 credit，任何仍可继续使用的 child 都不能与退款同时存在。普通 Handle duplicate、TRANSIT 或 GRANT 只改变同一 core 的 authority 可达性，不改变四项计数。额度不足与实际库存不足是不同失败：前者表示 quota 不足，后者表示物理或元数据资源暂不可满足。

Pool 形成单向强引用图：Handle、进程绑定与 MemoryCharge 各自保活来源 core；child 以不可分离的 ParentCredit 义务强持 parent，parent 不登记 child、backing、进程或地址空间。最后一个 child 引用消散时，只有其全部额度已回到 available 且 child 身份同步终结，才沿有界深度父链自然归还；任何计数不一致或“退款后仍可操作 child”的状态都属于内核不变量失败，不能通过错误退款扩大父池。Pool 不提供 child 枚举、关闭状态、等待电平、reparent 或通用 revoke，因此普通 close 不扫描对象图。

Derive 是不可撤销的资源 grant，不是可召回租借。父级在所有自然引用和 charge 消散前不能强制取回额度；MemoryObject 或 Pool capability 跨进程、跨 Job 转移也不重记来源。需要强制收回的未来场景必须从一开始使用具有显式成员、撤销准入和有界 drain 的 MemoryLease/资源域，不能改变普通 Pool 和既有 view 的单调授权语义。

运行期 frame-backed 资源按唯一来源支付：AddressSpace 根与中间页表由目标进程固定绑定池支付；匿名 backing 由所在进程绑定池支付；Tunnel 与 MemoryObject 的数据 backing 由创建者绑定池支付，而每端映射所需页表仍由各自进程绑定池支付。bootstrap payload 在收编为 init backing 时取得 primordial charge，释放时首次进入用户库存并归还额度；已收编 payload 属于 funded extent，不再属于 boot-held。stale translation 确认前，frame 与 charge 都不得恢复可用。

进程的 PoolBinding 只授予内部 backing 分配，不自动产生可查询、派生或运输的用户 Handle。资源管理者若需隔离预算，先派生子池，再通过 Building-only `ProcessBindMemory` 原子消费具 `GRANT` authority 的 Pool Handle；失败不消费，成功后绑定不可转移。Job 不提供默认池或第二份配额。

内核堆 metadata、Handle 槽和对象壳不由页额度伪装计费。它们属于正交的 KernelMemoryBudget：长期系统中，ProcessResources 同时持页池绑定与内核内存预算绑定，用户态资源管理器按政策与 Job、CPU 预约和设备能力组合交付。该预算落地前，可信 ProcessCreate/bootstrap 从物理隔离系统储备支撑的全局 admission 中为进程附入固定内部 MetadataSponsor；Running 操作只能消费该 sponsor 的有界 permits，独立于进程存活的对象随 permit 保活 sponsor 到自身真实析构。该过渡 sponsor 不进入用户 ABI，也不能转授或扩容；它只与每容器硬上限和 Commit 后零分配共同保证失败安全，不代表 MemoryPool 提供完整 DoS 隔离。开放不可信创建域前必须以显式 KernelMemoryBudget binding 替换这一全局政策。

## Map、Unmap 与 Protect

Running 进程对自己的地址空间具有固有管理权；组装者只凭 ProcessBuilder 在 Building 期操作目标地址空间。两条 authority 通道经过校验后消费同一 MemoryChange，不各自维护布局、backing 或回滚规则。

Map 以字节长度请求匿名或 MemoryObject backing，并同时声明权限、placement 与两端 guard。匿名来源由目标进程绑定池取得 backing；对象来源验证 Handle、对象内页对齐 offset、范围和 rights，不重新分配数据页。`Anywhere` 由账本选择完整空洞；`FixedEmpty` 只在指定范围完全空闲时成功。不存在隐式覆盖旧区域的 fixed 模式，替换必须显式 Unmap 后再 Map。成功结果包含可访问区间及含 guard 的完整 reservation 区间；AllocationKey 与 RegionKey 都是内核账本身份，不进入以地址区间为真值的用户 ABI。

单次请求的页数、guard 数量、区域切分数、PTE 步数和元数据增长都有共享硬上限。超出上限由用户态运行时在安全 interface 后分段组合；rinlib 内部以 affine region 表达 allocator arena、线程栈等映射所有权。完整解除消费原 region；部分解除消费原 region，并按请求洞的左右两侧返回至多两个 compound fragment，每个 fragment 可以同时描述 usable mapping 与相邻 reservation-only guard。该所有权类型不是应用扩堆 interface；内核 ABI 仍以地址区间为真值。

Unmap 采用严格全覆盖语义：目标范围必须被调用者可解除的 mapping 或 reservation 完整覆盖；空洞、越界或 object-owned lease 使整笔请求在 Commit 前失败。调用者负责先停止其它线程对该区间的访问；内核保证成功返回后不存在 stale translation，不能替应用修复并发 use-after-unmap。

Protect 只能在 mapping 创建时冻结的权限上限内改变当前权限。权限只允许只读、读写与读执行，不提供 write-only 或 W+X；增加权限、收窄权限和解除映射都走同一 MemoryChange 与地址翻译同步纪律。

连续堆顶不是地址空间真值。rinlib allocator 在内部取得并持有若干匿名 arena，向应用只暴露分配器语义，不暴露 Extend、brk 或堆映射操作；线程库同样消费通用 Map/Unmap 建立带 guard 的栈。不存在内核线程栈分配调用或固定预映射栈池。

## Guard 与故障

Guard 是占据虚拟区间但不持有 backing、也不安装 PTE 的 reservation。它阻止自动选址或 fixed mapping 占用该洞；普通 Unmap 对 guard 与 mapping 都遵守同一精确请求区间，不执行组级隐式清理。Guard fault 与普通未映射访问都属于用户程序违约并终止该进程，不升级为内核失败。

不存在按需分页时，成功 mapping 的全部页在返回前已有 backing 和有效 PTE。未来 pager 必须通过显式允许 fault 的新语义接入：账本区分可解析的缺页 mapping 与永远不可解析的 guard/空洞，既有 eager mapping 不因 pager 出现而改变。

用户内存访问经统一 uaccess seam 验证；内核不把用户指针当普通引用长期保存。一般异步调用不得在 publish 后依赖可能失败的用户写回；Map 使用下述提交前结果承诺，把固定宽写回放在不可逆边界。其它调用若确需异步写回，必须各自定义地址空间并发变化时的故障契约，不允许以内核 panic 代替。

## MemoryChange 事务

Map、Unmap、Protect 以及 object-owned lease 的建立与撤销都由 AddressSpace 内部事务完成：

```text
Validate → Reserve → Commit → Publish → Synchronize → Retire → Complete
```

Validate 检查完整范围、authority、生命周期、权限与固定宽输出槽，并在 AddressSpace 锁内形成不占有外部资源的精确计划。Reserve 不是一段跨锁调用：先离开 AddressSpace，从绑定 Pool、物理库存、metadata admission 与 Remote Call 容量分别取得全部 affine reservation，再重入 AddressSpace 复检计划所依赖的代次、区域与 execution snapshot，最终组装为 Prepared change。任何锁秩低于 AddressSpace 的资源源都不得在 AddressSpace 锁内进入；复检失败只析构尚未发布的 owner，保持账本与 PTE 零变化。与页数线性的 backing 清零在独占 extent 后、Commit 前完成。
页表模块必须把结构 preflight、资源供给与 owner 发布分离：preflight 只计算精确需求，调用方锁外提供已经资金化的表页，Publish 只把 owner 与 PTE 同时纳入 TranslationTree，不分配、不进入 Pool，也不发生普通可恢复失败。并行准备后未消费的表页以及从树中摘除的表页必须以 affine owner 显式返回，由 AddressSpace 锁外的 rollback 或 retire 路径收束；不能提交为裸帧号后再靠物理地址重建资源所有权。

Commit 是不可逆线性化点。此前任何失败都保持账本、PTE、backing 和请求槽零副作用；一旦 Commit 成功，事务归 AddressSpace 且必然完成。区域与 PTE 可以在有硬上限的 Publish 段逐项更新，未同步的其它线程在调用进行期间可能观察到旧状态或正在发布的新状态；内核不承诺任意多页在所有 hart 上瞬时切换。Synchronize 完成并向仍存活的调用者返回后，所有目标 hart 必须已经达到新 epoch，这才是对外完成边界。

Map 的 Reserve 已经唯一确定 usable 与 reservation 范围。`MemoryMapResult` 使用调用者提供的稳定结果槽，包含四个固定宽范围字段和位于末尾、自然对齐的 committed cookie；request 携带非零 cookie，调用者把 committed 初始化为零，Validate 拒绝非零初值或不满足原子对齐的槽。Reserve 从 ledger 取得覆盖该固定宽可写范围的 `UserWriteLease`，阻止其它事务在 Commit 或 rollback 前撤销、降权或替换结果 backing；lease 保存有界、已验证的写入投影，使 Commit 不必重新进入 uaccess 或 AddressSpace lock。全部可失败检查与资源预留完成后，内核先写 payload，再在 execution gate 内通过 lease 以 release store 把同一 cookie 写入 committed 字段；该 store 就是 Map 的 Commit。用户或 rinlib 只有以 acquire 观察到 cookie 匹配后才可相信 payload 完整，cookie 为零时 payload 内容无语义；在 System Call 成功返回前仍不得访问新 mapping。Commit 后释放 UserWriteLease，不再执行可能失败的 uaccess。

结果槽必须位于调用前已经存在且可写的 eager mapping；UserWriteLease 只 pin 固定宽 ledger/backing 与写权限，不把用户指针变成可长期解引用的内核引用。与 lease 冲突的 Unmap/Protect 在各自 Commit 前返回 Busy。rinlib 为可能 park 的 Map 把结果槽放进随线程保留到 join 的运行时记录，而不是可提前回收的临时缓冲。若调用线程在 Commit 前消散，事务释放 lease 并回滚；若在 Commit 后消散，结果承诺已经属于进程，事务继续完成。线程的 departed/join 完成边界不得越过仍挂接于该线程结果记录的 committed transaction，因而接管者在 join 后观察 cookie 时也已经越过 mapping 完成边界。

Unmap/Protect 的旧 backing、旧权限或 writable-view permit 在 Synchronize 期间由事务隔离持有，不能归还库存、映射到其它地址、退出 seal 计数或被另一对象重新解释。Retire 只在全部确认后释放旧 OwnedExtents、旧 ObjectView、写 permit 与过期元数据。一般事务状态属于地址空间而非发起线程；等待远端确认的 System Call 通过 WaitContext park，发起线程终止只消散最终返回权。进程终止冻结后拒绝新事务并接管已 Commit 的事务；只有线程、active hart、事务与对象 lease 全部完成或进入可恢复收束后，地址空间才可进入最终 drain。
最后一个地址翻译确认只把事务推进到 Retiring，不授予在 Remote Call 完成回调中执行任意宽度收束的许可。若 backing slice、表页或元数据的退役超过单次固定预算，事务成为显式的内核 work debt：由安全出口按游标逐批推进，未完成时由明确的 hart 唤醒所有者再次敲门，全部 owner 已在容器锁外释放后才进入 Complete、兑销 mandatory operation 并唤醒调用者。该机制没有后台内核线程，不绑定原发起线程，也不允许终止路径另建同步扫描旁路；进程终止只接管同一事务与同一游标。唤醒点取 Complete 而非 Synchronize 是刻意取舍：单管线单完成点让结果义务、mandatory operation 与 join 边界共用一个定义，代价是调用者返回延迟包含全部 retire 批次、随 extent 数线性增长。若实测成为瓶颈，合法的范式内优化是把调用者返回提前到 Synchronize（对外完成边界不变），retire 纯后台推进；两者都不改变 Commit 与 Synchronize 的既有语义。

## 跨 Hart 地址翻译同步

本协议依据 [RISC-V Privileged Architecture「Supervisor Memory-Management Fence Instruction」](../../references/normative/riscv-isa-v20250508/src/supervisor.adoc)、[RVWMO Memory Consistency Model 的适用范围说明](../../references/normative/riscv-isa-v20250508/src/rvwmo.adoc)、[RVWMO explanatory material「Fences」与「Explicit Synchronization」](../../references/normative/riscv-isa-v20250508/src/mm-eplan.adoc) 以及 [Zifencei「FENCE.I Instruction」](../../references/normative/riscv-isa-v20250508/src/zifencei.adoc)。RVWMO 尚未形式化页表遍历、`SFENCE.VMA` 与 `FENCE.I` 的组合，因此不能仅凭语言原子序推导完成条件；协议同时遵守 privileged architecture 给出的 data fence → IPI → remote `SFENCE.VMA` → ack 顺序。

`SFENCE.VMA` 只同步执行它的 hart。Reserve 在 Process execution gate 下取得 active 集合、成员序列与事务准入状态快照，锁外预留每个目标的 Remote Call 槽；Commit 前重新取得 gate，若成员序列或生命周期准入已变化则在零业务副作用状态下回滚或重试。复检成功后，AddressSpace 在 gate 内完成 Map 结果 cookie、完整 PTE 宽度写入、新 translation epoch 和请求状态 release 发布，再释放 gate。随后执行保守的本地 `FENCE RW,RW` 并触发 IPI，保证 PTE、epoch 与请求槽先全局可见；IPI 只是门铃。目标 hart 以 acquire 取得请求，验证稳定 AddressSpace identity 与 epoch，执行覆盖请求的本地 `SFENCE.VMA`，若请求携带 instruction epoch 再执行 `FENCE.I`，最后以 release 确认。完成者必须以 acquire 观察每一项确认后才能 Retire 旧 backing、权限或 view permit，并在释放槽前以 release 发布空闲；后续槽预留以 acquire/CAS 取得。初始实现可以合法地全局 over-fence，范围与 ASID 优化不得削弱完成条件。

请求槽的 Pending 状态而非 IPI 边沿是真值；门铃允许合并和重复，每个 trap 安全出口都检查本 hart 的固定槽，因此已经可见的请求不会因单次门铃边沿合并而丢失。平台 admission 必须先保证 admitted hart 的 IPI 与周期性 trap 路径可用；Commit 后的门铃异常不能转成 System Call 业务错误，Remote Call 只能保留 Pending 并由后续安全点补消费。平台永久违反 admission 契约属于系统级失败，不能假装事务已完成。

发起 hart 若属于 active 快照，也必须作为普通目标执行同一套 acquire → local fence → release ack，不能因正在修改页表而隐式视为已确认；不在快照中的发起 hart 只负责发布，不产生虚构的本地义务。

| happens-before 边 | 发布侧 | 观察侧 | 所保证事实 |
|---|---|---|---|
| PTE/epoch → 请求 | 完整宽度 PTE stores；请求状态 release；IPI 前 data fence | 目标请求状态 acquire | 目标执行失效前已能观察对应 PTE 与 epoch，门铃不会越过业务发布 |
| 既有用户写入 → 失效确认 | 目标 hart 在 trap 安全点处理请求，`SFENCE.VMA`，必要时 `FENCE.I`，随后 ack release | 事务完成者读取 ack acquire | 旧 writable view 的显式写入及本地翻译失效先于 permit/backing retire |
| 全部确认 → backing 复用 | 完成者对每项确认作 acquire，再推进 Retire | 后续库存/object owner 经事务发布取得资源 | 任何可能持旧翻译的目标 hart 均已失效，旧帧才可被重新解释 |
| 请求完成 → 槽复用 | 完成者清理 payload 后以 release 发布 Empty | 新预留者以 acquire/CAS 取得槽 | 新请求不能观察旧 payload、identity 或完成引用 |
| active 快照 → enter/leave | Commit 在 execution gate 内复检成员序列并发布 epoch | enter/leave 在同一 gate 内线性化并 acquire epoch | 快照前 active hart 必须确认；快照后 enter 必须先本地同步；leave 不能逃逸既有义务 |

active-hart 集合的唯一真值仍属于 Process 执行模型，enter、leave 与 AddressSpace 快照共享上述短 execution gate。enter 在登记 active 前以 acquire 读取 epoch并完成所需本地 fence，随后在 gate 内复检 epoch 未变化才登记并获准返回用户态；若变化则退出 gate、同步并重试。Commit 复检成功后在 gate 内发布新 epoch，因此 gate 释放后的 enter 必然观察它。leave 在 gate 内只有确认自己承担的 epoch 后才清除 active。由此，在线性化上早于 Commit 快照的 enter 必在目标集合中；晚于快照的 enter 必在返回用户态前自行达到新 epoch；早于快照的 leave 已不再执行该地址空间，晚于快照的 leave 必先履行确认。

最后一个 acquire 确认推进事务，原发起线程是否仍存在不影响完成。Process 终止、hart 离场和普通 SSIP 都只能协助消费同一请求，不能以“目标将不再运行”伪造确认。地址空间 root、ASID 或稳定 identity 在全部请求与 active 关系收束前不可复用；未来若复用 ASID，必须遵守规范「ASID Usage」要求另行完成覆盖该 ASID 的 fence。

## 可执行发布

可执行内存遵守 backing 级 W^X，而不只检查单个 PTE。Running 匿名 mapping 只产生数据权限；Building 构造方可以在目标尚不可运行时经受控写入口填充最终只读或可执行 backing，ProcessStart 在首次执行前统一发布代码代次。

运行期动态代码使用 MemoryObject 的单向状态机：

```text
Mutable --SealExecutable--> Sealing --last WritePermit retired--> Executable
```

| 状态 | 允许的新操作 | 拒绝的操作 | 前进条件 |
|---|---|---|---|
| Mutable | 只读 view；取得 WritePermit 后的 writable view 或受控直接写 | executable view | `SealExecutable` 在线性化点改为 Sealing；若 permit 为零可同点进入 Executable |
| Sealing | 只读 view；既有 writable view 的撤销/降权 | 新 WritePermit、直接写、executable view、回到 Mutable | 最后一个 WritePermit 完成 retire |
| Executable | 只读或读执行 view | 任意写入口、WritePermit、回到 Mutable/Sealing | 终态 |

Mutable 允许只读 view，并允许在对象锁内取得 `WritePermit` 后建立或重新启用 writable view；不允许 executable view。`SealExecutable` 要求对象定义的管理 authority，并与 WritePermit 预留在同一对象锁上线性化：先取得 permit 的变更计入 seal 等待，先进入 Sealing 的对象拒绝新 writable view、重新加写权限和直接写入口。Sealing 允许既有 writable view 继续存在直至其 owner 显式撤销，但状态不可回退；Executable 永久拒绝全部写入口与 writable permit，只允许只读或读执行 view。普通 Protect 不能把读写 mapping 转成读执行，也不能重新打开已发布对象。

WritePermit 的计数覆盖 reserved、published 和 retiring 三个阶段。Map/Protect 在 Commit 前放弃 permit 可以直接回滚；Commit 后移除 W 权限或 Unmap 时，permit 随旧 view 进入 retire，只有 active-hart 快照全部完成 `SFENCE.VMA` 并以 acquire 收齐确认后才退出计数。最后一个 permit 的 retire 负责把 Sealing 单向推进到 Executable 并发布对象的 `EXECUTABLE` 电平（见下）；seal 发起线程消散不撤销已发布状态转换。RX view 只能在观察 Executable 后建立，并为可能执行该代码代次的 hart 推进 instruction epoch 与 `FENCE.I`。

状态机不遍历 view。对象只维护带溢出检查的 permit 计数；seal 完成不设专用等待槽，而是以 ObjectSignals 的 `EXECUTABLE` 电平位表达：进入 Executable 后持续为真，持 WAIT 的 Handle 经 WaitMany 观察该位，任意数量等待者复用通用等待面，无容量上限、无排队政策，重复 SealExecutable 在 Executable 上幂等成功。该形态以等待面的结构性容量取代按对象记账的完成槽，不改变「seal 状态属于对象、发起者消散后继续、最后一个 permit retire 自动完成」的契约。受控直接写同样取得临时 WritePermit，数据写入结束并完成必要的 release 后才归还。

只读 view 不阻塞 seal，已有 writable 最大权限但当前不含 W 的 view也不持 permit；它日后尝试重新加 W 时必须重新取得 permit，因此在 Sealing/Executable 下失败。新代码版本通过新对象、新代次和用户态引用切换表达，不原地修改已发布代码。Handle 消散不撤销 view 或 seal；view 与 pending seal 都强持对象，最终 close 仍遵守固定 backing 容量上限。

具体帧库存、页表、内核高半区、用户布局和当前调用面见 [`../impls/mm.md`](../impls/mm.md)。
