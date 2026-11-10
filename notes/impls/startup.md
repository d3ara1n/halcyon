# 启动资源与用户态 launcher 实现

方向见 [`../ideas/bootstrap.md`](../ideas/bootstrap.md)、[`../ideas/object.md`](../ideas/object.md) 与 [`../ideas/task.md`](../ideas/task.md)。当前启动链是 BootPackage → 唯一 init → 用户态 launcher；内核不遍历归档、不识别服务名或服务拓扑。组装模型（线程是组装资源、双通道、生杀闭环）见 [`../ideas/task.md`](../ideas/task.md)「线程」；本篇记录机制落地。

## BootPackage v1

`shared/erhino_shared/src/boot.rs` 定义 64 字节 little-endian envelope：magic、version、header_len、flags、total_len、initial ELF offset/length、payload offset/length和 reserved。validator 使用 checked arithmetic，要求 canonical offset、页对齐 payload、零 padding和窗口内完整几何；payload 可为空。

`tools/make-boot-package.py` 原子生成 `artifacts/boot-package.bin`。Just 构建把 `srv_init` 作为唯一 initial ELF，其余验证程序暂以确定序 ustar 组成 opaque payload。DTS `/chosen/boot-package` 只声明物理装载窗口；`board.rs` 验证窗口完整落在 DT memory 内。

`boot.rs` 在帧池注册前验证 envelope，并以实际 `total_len` 收窄保留区。内核只解析 initial ELF，不解释 payload。

## 出生块（Birth Block）线格式

出生块是**组装者与接收进程的启动约定数据**。普通进程由用户态 launcher 构造和映射，内核不解析其内容；唯一 init 的出生块则由内核 bootstrap 构造并映射。两者共用 `shared/erhino_shared/src/startup.rs` 的线格式及构造/校验函数，布局：

```text
[StartupBlockHeader (56 B, version 3)]
[Handle × handle_count]
[EntryThreadDescriptor (48 B, section version 2)]
[zero padding]
[opaque payload]
```

Header 保存 magic、version、块长、pid、parent_pid、Handle 数、descriptor offset/length、payload offset/length 与 reserved。几何要求 Handle 数组、descriptor 与 payload 依序落在块内，空隙全零；descriptor 的 control_index 必须小于 handle_count，reserved 为零，栈区间无溢出且 usable 落在 reservation 内。Handle 数组保存目标进程 HandleTable 的真实句柄值（由 ProcessGrant 返回；Building ThreadCreate 的 Control 先在调用者表出生，再显式 Grant 到 child 表），不从 index 推导 slot/generation。普通 launcher 在取得全部 child-local 句柄后构造包含入口 Control 的前缀，再分步写前缀和 payload；完整块写入后才交出 PreparedProcess。

**`parent_pid` 语义**：描述目标进程的创建关系，即目标 launcher 的当前 `CurrentProcessInfo::pid()`（创建者 PID），与 ProcessQuery 快照中的 parent_pid 同一真值；孙进程的 parent 因而是直接父进程而非祖父。`SpawnRequest` 显式携带 launcher 从受控 `StartupContext` 读取的完整身份视图；不通过 runtime 全局 getter 隐式推导。

接收进程以出生参数 arg1/arg2（块基址与字节长度）在首线程入口读取（libruntime `startup::init` 沿用 a0/a1 契约）。init 的出生块基础 Handle 顺序为：Handle[0] root JobControl、Handle[1] primordial SystemReset、Handle[2] init ProcessControl、Handle[3] root MemoryPool、Handle[4] 入口 ThreadControl、Handle[5] 只读平台描述 MemoryObject；索引由 `shared::startup::initial` 定义，descriptor 显式引用第 4 项。总数按基础项加实际平台资源记录数计算；受支持 virt 为三个 MMIO 根、一个 PCI INTx 根和一个 DMA domain 根，其余当前平台为零根。Body 付款 Pool 与 Handle[3] 指向同一个 root core，不复制额度。普通进程先排列显式 grants，Building ThreadCreate 的 Control 经 Grant 得到 child-local 句柄后追加到同一数组并由 descriptor 引用；其余 slot 含义由具体启动协议定义，不属于通用线格式。
`StartupContext::adopt(StartupSeed)` 先完成 StartupBlock/role/Query 全表校验，失败返还 Seed 且不建立任何 owner；成功一次返回 `StartupAdoption { entry, entry_control, context }`，入口 Control 已从应用 grants 中移出。只有持 unsafe 构造的 Seed 者（libruntime 平台入口）能到达该值，libruntime 把 descriptor/control 交给入口 `ThreadRegistry`，应用只经 `startup::take()` 取得 context；任一采用失败记录诊断并 `ProcessAbort`，不调用 main。应用取得 Context 后必须消费 `into_parts()`，`InitialGrants` 通过 `take(index)` 或 `try_adopt(role)` 单次领取，未领取 grant 由 Drop 正常 close。

## 组装 ABI（Building 期外部通道）

`shared/erhino_shared/src/proc.rs` 与 `shared/erhino_shared/src/call.rs` 定义 fixed-width ABI，`libkernel` 封装位于 `user/libraries/libkernel/src/process.rs`：

- JobControl `CREATE`：JobCreate、ProcessCreate；
- JobControl `MANAGE`：JobSeal、JobDerive；`READ`：JobQuery、JobEnumerate；
- ProcessCreate：Job[CREATE] 与 Pool[CREATE] 创建已付款的 Building Shell/Body，返回 affine Builder 和稳定 Control，来源 authority 保留；没有 BindMemory。
- ProcessMap/Write：只允许 Building。Map 使用统一异步 VM 操作；Write 用短 Building lease 初始化最终 RO/RX/RW 页。
- ProcessGrant：完整 claim/验证、目标表预约、锁外输出后必成转授；失败保留来源权限。
- ProcessBindExecution：在首次 ThreadCreate 前冻结需求与域，重复返回 ObjectNotAvailable。
- ThreadCreate：验证 entry/sp、预付 ReadyBatch 并构造 Created/Control/Departure；Building Control 安装调用者表；launcher 以独立 Capability 接管，Grant 失败关闭，成功才采用 child-local 输出，启动责任属于 Builder；Running 返回 affine ThreadStartTicket。
- ThreadStart：消费 Pending 票据并发布预付 Ready owner，失败保留票据；Control 只观察 DONE。
- ProcessStart：提交 Running 并关闭 Building 修改门，消费 Builder。内嵌 Work 等 VM 既有工作完成后开放 entry，再按冻结的槽/tid 批次逐个发布；调用线程通过普通完成续体等待。
- 放弃：Pending Builder 的最后 authority 冻结 Shell；已提交启动、VM 工作、Created 离场及 Body 回收各由其出生记录继续，不依赖调用者。

## 唯一 init bootstrap

[`boot.rs`](../../os/kernel/src/boot.rs) 验证 envelope、ELF 和物理 owner，建立 root Pool，再发布 [`boot/bootstrap.rs`](../../os/kernel/src/boot/bootstrap.rs) 的 Bootstrap 维护记录。记录保存 validated ELF、package prefix owner、payload owner、出生块与装载游标，不重新解释 program headers。

Bootstrap私有Arc、SystemReset Arc及平台描述MemoryObject都在root Pool中按实际live-layout先准入；初始Cap Vec按基础项加实际资源记录数的Layout预付，commit消费分配后、发布Work前返费。Startup prefix编码器显式接收PAGE为payload_off/Vec容量，构造前按同一Layout付款，Loader持有block与其MetadataCharge，未发布失败一起收束。Bootstrap自己的header费随执行宿主最后loan释放，SystemReset公开对象费随其最后存活owner释放。

1. 通过普通 construct 创建 pid 1 的 Shell/Body，绑定执行需求，创建 Builder 与入口 Control；按实际初始图预约 cap 槽，用真实句柄构造出生块并提交。Job 与 cap 表提交分段，表锁内没有 Job/对象业务提交。
2. 先每步复制至多一个目标页内的 DTB 字节至平台描述对象，完成后归还原 DTB reservation；随后每步映射一个ELF run分片或栈分片，最多取VM的STEP_UNITS页。回填文件/出生块字节受同一分片字节数及uaccess上界共同约束。所有Map复用Operation/ChangeWork。记录持私有完成来源，在自身稳定WaitNode上arm/attach/复检，提示只唤醒同一维护记录；完成后唯一detach，不同步自旋推进整个映像。
3. payload的BootHeldExtent每次同样切出至多STEP_UNITS页，未移交尾段继续由Loader持有；片段与root charge合成BootFundedExtent，作为统一backing的物理owner进入普通只读View；普通页与boot页保持各自库存归还方式，View/整空间退休使用同一段游标。没有隐式lease区域。
4. 共用成员预约、Thread/Control/Departure 与 ReadyBatch 构造入口 Created，出生参数为块址/实际字节数、sp=USER_TOP。
5. 所有ELF字节拷贝完成后清除原始字节借用并归还package prefix；Loader从Option中取出唯一Builder，调用start后在私有锁外释放该强根。已提交启动由Builder队列根承担，Bootstrap仅持独立完成来源，等待VM静止、分批发布、Builder退费和WorkReceipt归还后的DONE。完成后detach并释放Loader/Bootstrap；保留Builder强根会阻止其last-live完成，因此等待期间没有该强边。启动错误冻结Shell并释放Loader；唯一initial无法构造属于致命错误：同步准入失败仍走Preparing→Failed，Ready后装载失败走既有runtime_stop全hart停驻。

[`boot/watch.rs`](../../os/kernel/src/boot/watch.rs) 的 InitialWatch 在初始图准备期预付 Work/节点/费用，只持 Shell 的独立观察来源，不持 live capability、Body 或退出租约。Bootstrap 在 ProcessStart 前挂入它的 DONE 节点并发布；观察 Work 先 arm 再复检，正常长期运行时停驻，不轮询。装载记录完成后释放临时输入，该独立 boot owner 继续观察。实际 init DONE 后，Work 锁外 detach 并提交 runtime_stop；hint 只唤醒，不在回调内停机。DMA 稳定保留不依赖观察速度，终局不会因 init cap 表关闭而 unpin。

VM/backing退款由[`mm.md`](mm.md)拥有，生命周期与启动门由[`task.md`](task.md)拥有。启动记录的分片准备也可能分配metadata，STEP_UNITS页数边界不代表完整原子时间界；源码检查、平台组合和未覆盖窗口由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)承载。

### 只读平台描述

[`frame::DeviceTreeReservation`](../../os/kernel/src/frame.rs) 持原始 DTB 的有效字节范围及可归还片段；片段来自 DTB 页覆盖减永久 reservation，最多为永久排除段数加一。`main` 在单核启动期撤去 transition 别名后把该 owner 交给 `boot`，正式直映射继续可读。`Bootstrap::Loader` 持 owner 到复制完成；失败析构同样只归还它实际拥有的部分，不释放永久平台保留区。

[`shared::platform`](../../shared/erhino_shared/src/platform.rs) 定义 48 字节小端头、16 字节资源关联项和紧随其后的原 DTB。头含版本、有效总长、DTB 偏移/长度及记录数，记录关联资源种类、相对 blob 的节点 token 偏移与初始 Handle 索引。解析检查 canonical 几何、溢出、重复句柄槽和逐项槽位关系；同一 DT 节点可关联多个同类资源。board 的已准入 IoRoots 与 DTB owner 一起移交 boot，Bootstrap 按同一顺序写关联记录、铸造真实 MMIO/IRQ/DMA control 并安装 capability；PLIC 控制器在 registry 建立后初始化，IRQ 根只委托已核 PCI source 和 admitted target，DMA 根固定为 TrustedPhysical/Coherent。PIO 不产生记录。

根的对象/节点/Work 由 root Pool 付款；全部描述、cap 槽和启动输入准备成功后才允许 Start。铸根或安装前失败先释放已铸 capabilities，再沿原 Shell/Builder 失败路径收束。记录只关联节点和真实 handle 槽，不复制物理几何；几何由 control 查询和只读 DT 对应。

平台对象使用正常 root Pool funding 和 MemoryObject owner，分配页清零。初始化仅在初始进程 Building、无可运行消费者时经 `initialize_boot_page` 写入，单次范围不得跨目标页；这不是公开对象的写权限旁路。对象只交付 READ/WAIT/DUPLICATE/TRANSIT/GRANT，不交 WRITE/EXECUTE/MANAGE。用户看不到原 DTB 首尾页的其他内容；对象尾部 padding 为零，原 reservation 可在初始进程 Start 前归还。

[`srv_init::platform`](../../user/services/srv_init/src/platform.rs) 用 Description::parse 严格核完整初始数组长度与逐项槽关系；[`libpci::platform`](../../user/libraries/libpci/src/platform.rs) 共同拥有只读 View、描述格式、DTB totalsize、节点 token、尾部零填充及实际 kind/rights/几何核验，供 init 和设备服务复用。后继只解码只读事实，不把初始 handle_index 当成自己的授权。共享解析器 [`shared/dtb`](../../shared/dtb/src/lib.rs) 的 `node_at_offset` 从结构块按文法推进，拒绝属性值中伪装的 BEGIN_NODE 字样。每条 MMIO 记录必须有真实 MmioControl、完整启动权限、未关闭/无成员状态，并与节点 ECAM 或 memory window 的 CPU 几何一致；拒绝 PIO、重复物理根和不支持的资源类型。IRQ 记录必须为唯一 IrqControl，初始 flags/generation 为零，source 范围与 DT 的 PCI interrupt-map 相符且 target 非空。init 将只读对象/映射与 typed 控制根保存到进程退出。失败不发布输入，已有 root owner 正常关闭，Unmap 失败时实际 View 归 VM 自动退出。DMA 记录必须为唯一 DmaDomainControl，快照的显式模式与一致性匹配支持 profile，flags/members/reserved 为零，DT host 声明 coherent。通用 `libprocess::context` 的可采用类型表包含 MMIO/IRQ control/use 和 DMA domain/use/mapping，可正常 Grant 给后继进程；它不解释平台数据。

## 用户态公共 loader

### ELF 验证与装配

[`shared/elf::Validation`](../../shared/elf/src/lib.rs) 是 bootstrap、用户态 launcher 与 host audit 共用的静态 ELF admission。每步处理一个 program header、一个页权限区间或一个 ISA 解析单元；有限 LOAD/边界数组之外的属性字符串逐字节前进。它检查 segment 顺序/几何、文件边界、页级权限并集与 W^X、entry 的 executable file-byte 来源和 ISA requirement，错误冻结首个拒绝结果，不能重试后跳过检查。`validate` 只是循环驱动同一验证器；输出仍为私有构造的 segments/runs/image_end，调用者不重读原始 headers。属性解析按 vendor 和 Tag_File 各自长度限定范围，只有 riscv 命名空间的架构声明可用于选择执行需求。

Validation 持不可变输入 owner，内部 ELF/ISA/架构字符串状态只保存原映像范围和游标，没有指向自身输入的跨步借用。输入可以是借用切片或拥有型只读映射；构造失败返还原输入，跨步移动不改变解析状态。

[`libprocess::Preparation`](../../user/libraries/libprocess/src/preparation.rs) 的借用入口持固定映像、payload 和 grants；拥有型入口持 ImageView、payload Vec 和 OwnedGrant。两者共用验证游标、未发布进程、阶段机和 GrantOutcome。拥有型构造立即接管输入，初始校验错误由第一次 advance 返回并冻结；宿主仍须显式取消和关闭输入。完成前不会交出 Builder 或启动目标。

| 阶段 | 持有与单步工作 |
|---|---|
| 验证 | 推进 Validation；核出生块与栈的边界，尚未创建进程 |
| Create、映像与栈 | PreparedProcess 持 Building 的 Builder/Control；按 validated runs 映射、按 segments 写文件字节，再映射主栈 |
| 授权与入口 | 消费显式 grants、映射出生区、绑定执行需求、创建入口线程并转授其 Control；成功 Grant 才记录消费事实 |
| 出生块与完成 | 构造只含 ABI 有界句柄和 descriptor 的前缀，分别写前缀与 payload；完成后移出唯一 PreparedProcess |

每步至多一页写入或一个 ABI 有界出生操作。Map 分片上界来自 `PROCESS_MAP_MAX_BYTES`，每笔映射对应自己的 backing/region；Write 按目标页边界切分。借用输入贯穿装配，拥有型输入直接接管调用者交付的存储，不复制完整映像。OwnedGrant 在 ProcessGrant 成功的同一步以 into_raw 解除本地关闭责任，之后失败不得再次关闭已消费 handle；借用分支仍由调用者按 GrantOutcome 兑现移交。

[`prepare`](../../user/libraries/libprocess/src/management.rs) 同步驱动同一 Preparation；完成后返回 affine PreparedProcess，唯一持未启动 builder/control。`try_start` 失败保留原 owner，`cancel_to_collector` 显式移交原取消责任；同步 start/cancel 及 Drop 保留完整失败收束。SpawnRequest 的 control rights 在 Create 前核 MANAGE/WAIT/READ，覆盖取消、DONE观察与终态查询。普通 spawn 组合 prepare/start；root 的装配入口每轮只推进一步，并继续服务同一 Assembly 的管理观察，失败取消也交原收集账本。

Preparation 首次失败后冻结状态，重复 advance 返回同一错误。显式 cancel 关闭尚未转授的入口 Control，再关闭 Builder，把实际 DONE/Verify/Close 交给 Collector；Builder 关闭失败仍保留原 Building owner。完成或取消后保留 GrantOutcome，它只描述调用者输入句柄是否已消费，不能证明启动、退款或进程终态。直接 Drop 内部 PreparedProcess 会同步取消；root 使用显式交接以继续推进其它职责。Collection 的状态机由[运行与监督](runtime.md#process-与-job-收束)拥有。

拥有型 Preparation 的 close_inputs 只在完成移交或成功取消后执行，Unmap 失败仍保留原映像 owner。Collector 负责目标进程，输入映射是装配者自己的责任，两者分别确认。srv_driver 的原 ProcessTask 显式推进同一 Preparation、在 Start 前附着 DONE，取消 Collector 保留到原来源退休和关联 Run 脱离；root 继续使用同一状态机的借用入口。

目标映像/栈及目标句柄表由新进程 Pool 付款；调用者自己的 handle 安装、ProcessWrite 暂存和 Grant 输入缓冲仍付调用者 Pool。`LOADER_WRITE_BUFFER_BYTES` 来自 `PROCESS_PAGE_SIZE`，装配在目标预付之外按同一值计一次串行写入的临时 metadata 峰值。出生块位于 validated ELF 映像顶的页对齐地址，最终映射为 READ；内核 Building Write 负责初始化，不给接收进程增加 WRITE 权限。

### 程序映像的构造与读取

[`libprocess::image`](../../user/libraries/libprocess/src/image.rs) 分别表达构造、交付和读取三种寿命：

| 类型 | 持有资源与完成条件 |
|---|---|
| ImageCopy | 唯一持未发布 MemoryObject，逐页映入、复制、解除，再 Seal；`waiting()` 提供实际 EXECUTABLE 观察对象，宿主决定如何等待 |
| Image | 对象实际 Executable 后缩权交付，只持对象及有效字节长度，构造和能力转交方不保留完整映射 |
| ImageView | 独立持只读 MappedRegion 和有效长度，view 强引用保活 backing；bytes 借用绑定自身，关闭映射前有效 |

同步 Image::copy 循环驱动同一 ImageCopy。显式 cancel/close 在 Unmap 失败时保留映射 owner，允许重试；ImageCopy 不冻结首错；Unmap 失败后仍持原页，重试解除成功才前移游标。无宿主接管的 Drop 清理失败使当前进程失败，由 VM 退出收束。root 在拷贝失败后显式取消，取消仍失败则持原 owner 进入故障终局。

平台入口以一次性StartupInput建立affine `StartupSeed`；`libprocess::context::StartupContext::adopt(seed)`先验证格式、索引和句柄去重，再Query全表核实可采用role，最后提交Capability owners。失败返回原Seed，不析构原始grant。入口descriptor的Control被转给`libthread::ThreadRegistry`，普通grant由runtime startup API显式转交。出生映像随目标地址空间持有，回收由[mm](mm.md)的View/地址空间退休路径承担。

## init 的组态装配与根监督

[`shared/formation`](../../shared/formation/src/lib.rs) 拥有 v2 组态模型、二进制编码、资源/付款/引用校验；其 host 编译器按序深合并源 TOML，`--list` 与实际打包使用同一解析器。组态描述持续服务，没有 finish、task、动态 Export、业务启动依赖或就绪/正常停止报告。内核只交付 BootPackage 和初始 authority，不解释服务名单。

[`srv_init::executor::load`](../../user/services/srv_init/src/executor.rs) 在创建服务 Job 之前解码、校验所有引用及实际平台根展开数，为每实例编码真实出生表、settings 和 Namespace 描述，并验证 ELF。实例 Pool 包括映像 load runs、主栈、实际 StartupBlock 页数及工作集；metadata 独立换算成槽，合计与根 Pool 可用额度比较。出生几何由 [`compact_block_len`](../../shared/erhino_shared/src/startup.rs) 唯一推导，公共 Preparation 同样使用，不把长设定或 Namespace 描述误算成一页。

Executor 先派生全部具名/实例 Pool，建立 Job，再按声明的 payer 预建所有 Mailbox，最后铸造各 Sender/Lifetime。Mailbox receiver 只持一个施工 owner，sender 主输出可按同一真实身份复制，Lifetime 是独立输出。各工厂的选定 Pool 直接支付对象及持久派生存储；能力转授不迁费。init 自有共享 Image 缓存、输入拷贝及装载暂存继续由自身付款。付款工厂和运输寿命见 [IPC 实现](ipc.md)。

实例按确定序逐个经原 Preparation 构造和 ProcessStart，不等待其他服务 Ready；彼此引用 sender 不产生启动排序。receiver/Lifetime 输入 move，普通 sender 输入和 Namespace binding 使用缩权别名；只有施工源保留所需 GRANT，目标按声明接收。借用 grants 的所有退出边，包括中途观察到另一服务 DONE，均按 Preparation 的 GrantOutcome 兑现消费事实，不能在消费后再次关闭源 handle。所有进程 Start 前均在同一 WaitSet 登记 DONE；装载/拷贝的有界步之间同时消费已发布进程的观察。发布完成后丢弃普通施工 sender 和未交付的 Lifetime，不保留多余 receiver；仅明确声明终局角色的 sender 持续由 init 持有。

### 出生与初始 Namespace

[`libprocess::birth`](../../user/libraries/libprocess/src/birth.rs) 的 v2 帧包含独立的命名 capability 表、settings 及 named data sections。输入和 section 各按名字规范排序；Birth 按真实 kind/rights 采用能力，data section 一次领取，finish 拒绝任何未采用项。它不解释 OAL 或领域政策。

[`shared/oal`](../../shared/oal/src/lib.rs) 拥有规范路径规则与 Namespace birth 描述。描述只关联绝对 prefix 和命名输入，不保存 PID、裸 handle 或组态 Source。init 将 `Instance.namespace` 解析为 `oal.entry.N` 输入和 `oal.namespace` section；[`liboal::Namespace::adopt_birth`](../../user/libraries/liboal/src/namespace.rs) 验证并采用真实 sender，建立调用方本地完整绑定。没有可变占位 backing，也不要求客户发现后再拼接初始系统视图。

[`system.toml`](../../user/formations/system.toml) 为 device、driver、fs 分别交付 `/`、`/services`、`/devices`、`/drivers` 四项初始前缀；设备观察来源与 driver 的 claim 来源独立，控制端、发布根和来源 admin 不混入普通视图。前缀绑定不合成文件根的目录项，所以 `/` provider 的枚举不自动列出其他绑定点。各宿主在本地权限/Funding 与 Lifetime 观察实际安装后才开始读取早到请求；init 不观察该本地采用门，不因此推导业务可用。

### 根监督与故障终局

Executor 的持续监督没有正常完成结果；任一基础实例出现 Dead 都进入系统失败，不将 Completed 解释成部署完成。没有 init 的正常关机/重启操作或反启动序停止链。已有设备 Binding/硬件静止与运行期服务注册/撤出仍由其领域契约负责。

[`terminal`](../../user/services/srv_init/src/terminal.rs) 持 services、平台根与 reset 的借用及静态撤销入口。只有资源显式声明 `terminal_revoke` 才委托该协议给故障终局；资源引用名或后缀不产生协议身份/权限。终局封 services、在同一有界期限内 best-effort 请求撤销、封平台根、有界收集后提交 SystemFailure 冷重启。不等待撤销 Closed，不将 DONE 当作硬件静止；内核仍独立保留活 DMA。复位拒绝后终止 init，由既有 initial DONE 保护承担停驻。不包含恢复、重试、救援或正式验收进程编排。

## 验证入口与边界

编译、ELF 审计与 host 测试由[构建与验证手册](../../plans/BUILD-AND-TEST.md)拥有。正式 System 通过同一组态编译与映像打包链启动；运行证据来自实际服务、应用和设备使用。host 测试不覆盖真实进程出生、调度、停止与硬件访问。
