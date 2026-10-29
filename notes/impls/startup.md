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

## init 的静态部署与根监督

[`srv_init::deployment`](../../user/services/srv_init/src/deployment.rs) 拥有编译期部署。`system` 只接受一个文件宿主、设备与驱动管理者及三份按需驱动映像；完整输入先核缺失和重复，内核不解析服务名单。正式拓扑为：

```text
root
├─ init
└─ services
   ├─ device_management → srv_device
   ├─ driver_management → srv_driver
   ├─ drivers → 按实际设备启动的 PCI / xHCI / HID worker
   └─ srv_fs（文件与独立 registry）
```

init 在初始化后继续承担这一必需集合的根监督。没有应用不提交停止，空闲交原 Assembly 等真实来源；System 不运行 pm、secondary、test_fs、test_target 或压力负载。服务管理器、initfs 服务描述、恢复和持久化尚未接入；未来服务管理器管理的普通后代不能仅因进程树位置升级为根必需宿主故障。

RootSupervisor 在 bind_services 时准备系统 Terminal，持续保有 services 的待命 JobCollector、平台封口与 reset 权限。实例观察由 [`supervision`](../../user/services/srv_init/src/supervision.rs) 在同一 Assembly 的 Runtime 推进，容量来自部署实际同时存在的实例及其来源种类；System 三个，验收可另有 secondary。`Record` 独占 PreparedProcess 或 Running Control，并保存唯一 PID/DONE 快照。`start_instance` 完成启动后只返回 PID；需要当前进程状态的装配者通过 `instance_snapshot` 查询原 Record 的 Control，不接管或复制它。设备 SourceAdmin 和文件 Bootstrap 分别持各自领域结果，完成谓词分别读取设备 Closed、文件 Stopped。DONE 与 fs bootstrap 来源实际 registered 后才消费 Start；gate、停止意图与 fs 启动绝对期限在 Start 前复核。

root 每次接收先推进实例 DONE/已知故障，再推进调用、装载、普通批次与客户端，每个任务每轮有界。必需宿主未提交停止就 Completed、异常 DONE、启动或停止失败均封新工作，进入同一个 `system_failure`，不重启、不由用户 fault 触发 kernel panic。Terminal 有界封来源、平台与执行后 ColdReboot/SystemFailure；reset 拒绝则终止 init，boot 的原 DONE owner 接管停驻。观察运行体退休不销毁系统 Terminal。

### 文件宿主与授权

[`srv_fs::startup`](../../user/services/srv_fs/src/startup.rs) 定义私有版本 1、profile 两个 little-endian u32；仅接受 System/AcceptancePrimary/AcceptanceSecondary，拒绝旧空串和 `secondary` 字符串。五项应用 grant 为 bootstrap sender、release Notification owner、文件管理 receiver、registry receiver 或 secondary 发布 inbox、实际 Pool；采用前核 count、真实 kind 与准确 rights。profile 选择入口和容量，不授予权限。

System 与 Primary 同时运行文件与 registry，分别发行一组、A/B 两组文件/管理/目录/发布根；Secondary 只发行管理根，文件根沿原单步 Register 发布 `fs.secondary`。同一 [`boot`](../../user/services/srv_fs/src/server/boot.rs) 创建 backend、业务与控制 ingress、release、retire、运输及根任务；必要来源真实安装后才交根、注册和 Ready。停止先到时不发布，已领取未移交的 sender 立即释放。

[`filesystem::Host::launch`](../../user/services/srv_init/src/filesystem/launch.rs) 逐步用共用 ELF Validation 计算 load runs，独立 Pool 页数为实际 runs＋ABI 主栈＋出生页＋显式堆工作集；ProcessCreate 与交给宿主的 Pool grant 为同一付款身份。启动阶段取一个部署绝对期限，准备失败按 GrantOutcome 兑现未消费能力，Building 取消交原 Collector。root 在移交前查询私有 sender context，预期 PID 来自 PreparedProcess。

[`Bootstrap`](../../user/services/srv_init/src/filesystem/hosting.rs) 持预付 ReceiveBuffer/有限报文存储，以及私有 receiver、停止 signaler、Pool 的唯一生命周期关联；这些 capability 的关闭 owner 仍在 root 能力账本，装配和停止通过原实例记录借用。每轮只领取一条报文，核原 context/PID、空正文、准确能力数和 typed sender 权利；四根按 kind 收齐，重复或额外消息拒绝。认证 Ready、真实 Process 仍 Running 且 root gate 开放后才取出 Host。Host 只持业务根、入口身份和实例关联，不保存进程 Control/PID 或上述生命周期句柄。正式根由 RootSupervisor.system_host 保管；验收根交对应消费者，管理与发布母权留 init。

文件 Derive 按路径/FsRights 缩权；目录交付只能使用已有 Scope::Root 与 DESCRIBE/ENUMERATE/GET_ENTRY/WATCH 角色的别名，没有远程目录 Derive，不以去掉 GRANT 冒称目录业务缩权。需要发布的消费者通过 DelegateName 取得 exact-name 角色。注册发现、endpoint 可用、Ready、DONE 分别表达不同事实，不自动重绑已有 Namespace。

### 停止与收集

内部 `request_stop` 单调保存动作、StopPhase 与冻结总期限，Seal services 并封本地 Start/交付；重复同一动作返回原阶段，不能刷新期限或换动作。其责任集合来自原记录，包含尚未 Ready/Start 的实例，不由调用者传入可遗漏的 Host 清单。阶段为 Consumers→Files→Devices→Root→Completed。memory的部署声明为空，不执行Driver/Device停止；其它部署声明的管理者缺失仍失败。已经建立的实例观察运行体在领域停止之后统一由停止聚合退休，从未建立的运行体不调用退休接口。root 的客户端和普通受管进程先沿原 owner 收束，正式自持根随后释放；文件按 secondary 在 primary 前的顺序，之后交原设备安全链。总期限进入 root 的最早等待与到期升级，覆盖来源注销、取消、Collector 和 services Job 收集。

fs 由 `begin_filesystem_stop` 从原记录取得停止接点，先记 Stopping/绝对期限再 signal release。宿主封普通准入，原流、Copy、映射、注册、Watch 和来源继续退休，Runtime/Assembly 与领域账户收空后发 Stopped，再正常退出。root 保存认证 Stopped 和真实 Dead/Completed/NONE DONE 两项事实；顺序任意，缺项到期失败，异常 DONE 不被 Stopped 掩盖。DONE 先到仍保留原 receiver；到期逐条排空已有有限报文后才裁决缺报告。停止中迟到根核验后立即释放，迟到 Ready 不重新开放交付。

`detach_filesystem` 汇齐两项事实、实际注销两来源后返回唯一待收集进程。正式停止立即交给原 Collector，控制权移交之间没有可失败操作；`close_filesystem_report` 确认来源已注销后才关闭停止与报告接点，最后关闭 Pool 的本地 owner。正式停止报告实际余额，不等待外域仍持有的 backing；零账不是设备安全或正常停止谓词。

设备仍按实际 Source Closed、Binding/Provision 安全与管理进程正常 DONE 完成，父供应保留到后代清理结束。最后由原 JobCollector 收集 services、关闭辅助根并标记 Completed，init 才提交 Requested reset。运行证据及未覆盖交错由服务装配唯一工作记录拥有。

### 验收装配

默认 core、stress、memory、device 仍为编译期部署选择，与 system feature 互斥。init 先以 `AcceptanceImages` 一次核完验收归档的映像集合（缺失或重复即拒绝），再由 [`kernel_acceptance`](../../user/services/srv_init/src/kernel_acceptance.rs) 的 `KernelAcceptance::run` 用原 Preparation/spawn 与 root 账本启动 `test_kernel`，等其实际 Dead/Completed/NONE。`finish` 以它为输入执行根监督自检、核主 Pool 双维归零并关闭、核 held Pool 自报告后不变；此后才准入 srv_device。`test_fp` 由 init 启动，必须存在映像并正常 Completed；唯一降级为 spawn 明确 NotSupported、grants Retained 且取消无错误，表示无兼容 D64 域，此时以单进程 Collector 收集 test_kernel 并报告二成员失败隔离自检在该平台不适用。memory 不启动 FP 和基础宿主，声明实例集合为空，停止只推进声明集合中的提供者。

[`test_kernel`](../../user/tests/test_kernel/src/main.rs) 按领域分模块承载验收：`ipc_checks`、`ipc_thread_checks`、`memory_checks`、`job_checks`、`tunnel_checks`、`rpc_checks`、`time_checks`、`once_checks`、`race_checks` 及 MMIO/IRQ/DMA 资源检查；`main` 只负责启动子进程与编排顺序，`startup` 负责出生采用与保留模式子进程。它以自身 StartupContext 的 PID 核发送者，以自身 CurrentProcessInfo 启动 srv_pm/test_target/test_hammer，只持 services 下独立验收 Job 的子树授权，不持根 Job。pm 的 mailbox 与 `DERIVED_CONTROL_RIGHTS` 的 pm_domain JobControl 由 test_kernel 交付，委托域由同一正式 JobDriver 收束。测试本地 [`Harness`](../../user/tests/test_kernel/src/harness.rs) 保存能力和子进程 owner，复用 libprocess Collector 与 libexecution 的流读取运行体；测试失败使 test_kernel 失败，init 的原 services 终局负责整树收集。startup capture、idle ownership、Dispatcher funding 及 RootSupervisor 自身失败隔离仍由 init 验证；失败隔离使用正常完成的 test_kernel 与 test_fp，原部分准入、InvalidPolicy 和同一 Collector 恢复判据不变。

[`test_kernel::birth`](../../user/tests/test_kernel/src/lib.rs) 是当前验收私有出生格式：56 字节，依次为小端 u32 部署模式位、u8 进程角色、12 个输入槽号、保留零字节与四个小端 u64 映像有效长度。部署模式由 init 按 feature 设置；进程角色区分 init 启动的主验收进程与主进程为跨 reset 保留 fixture 自启动的两种子进程，二者不共用位空间。输入为主 Pool、验收 Job、衰减 reset、pm/target/hammer 映像、MMIO/IRQ/DMA 根、retention Pool、自身映像及私有报告 sender，缺省时槽号为 255。接收侧按输入核真实 kind/rights，并拒绝缺失、重复与多余能力；映像通过现有 sealed Image/ImageView 采用。`test_kernel::report` 拥有 held Pool 快照的 104 字节报告格式。

主付款域按同时存活场景组合既有 `libdriver::boot::WORKER_POOL`：core/device 及资源专项四份，stress 与 memory 五份。Once 的独立两份 metadata 域与主进程、pm 共存；stress 的主进程、pm、两 hammer 与并发目标取五份。memory 的主进程与一个 hammer 虽为其子集，同样执行1024个线程成员并存的raw storm，借用已验证的stress五份组合，普通单进程工作集不覆盖该线程数相关成本。它们是已有部署工作集的组合，不是从内核内部布局闭合的精确峰值。缺少公共工作集推导的限制由[组态计划债务](../../plans/todo-2026-10-28-init-formation.md#债务)承接。

DMA exposed/output-fault 故意跨 reset 保留的计费独立使用两份 WORKER_POOL 的 retention Pool。它是 RootSupervisor 通用 held Pool 集合的一员：init 从根 Pool 派生并持有至系统结束，不随任何实例收集而关闭。主测试通过自身 sealed 映像各启动一次保留模式子进程，其出生、scope/use/MemoryObject 创建、目标出生与原 Seal/退出核验全部在 retention Pool 内进行；unexposed 与 race 等可回收 fixture 仍使用主付款域。test_kernel 在最后一个保留子进程收集后核残留页数恰为每 fixture 保留页数之和，再经私有 sender 报告全快照；init 核实际 sender PID、kind、固定长度和零能力，主进程收集后与同一 held Pool 再次逐字段比对。init 只持通用判据（主 Pool 归零、held Pool 报告后不变），不认识 fixture 细节。metadata 残留只记录实际值和保留对象构成，不以任意上界代替判据；dma-install-failure 不执行两项保留模式，不建立 retention Pool。

core/stress 后半段通过同一 Host::launch 和设备装配运行真实消费者；secondary 保留非空 Copy/Watch 停止，primary 保持运行至全局 request_stop，随后接原 Driver/Device 停止链。[`filesystem::acceptance`](../../user/services/srv_init/src/filesystem/acceptance.rs) 的 `track_stopped_host` 先预留接管容量，取得已完成 Stopped/DONE 且来源实退的唯一 Control，立即纳入原批次后关闭报告接点；`collect_stopped_pool` 用原单进程 Collector 收集 secondary，再取原 Bootstrap Pool 核双维零并关闭。它不再与 pm 控制权耦合。

reset 的对象类型负测使用 test_kernel 自身验收 Job；权限负测使用只有 DUPLICATE 的 SystemReset 别名。真实 reset authority 不交测试进程，init 的 `submit_shutdown` 仍消费已完成停止意图与原 authority，提交唯一最终系统动作。根拓扑遍历与 init 自身不变量留在 init。System 不打包或运行 test_kernel；验收模式继续使用同一 BootPackage/启动 ABI，strip 只操作归档副本。

## 验证

- shared host：BootPackage/出生块 canonical geometry、零 padding 与空 payload；
- capability host：claim/restore、Close/shut 撤返还、rights 收窄、安装预约、真实持有数与 zero policy；
- libprocess host：ELF 前置、必选映像缺失/启动失败与非法 supervision policy 在 syscall 前拒绝并返还 authority；
- 工程静态门：`just clippy` 以 `-D warnings` 覆盖 shared/os/user host、kernel/user RISC-V、stress/memory/device/resource/System feature 与独立 gc target，共十一个分面，完整日志写入 `artifacts/lint/`；`acceptance-core` 与完整 `acceptance` 在启动 QEMU 前执行该门；
- QEMU 常规回归：`acceptance-core` 组合静态门、无调试器 debug stress 与 release core。`virt` 提供 debug core；`virt-stress` 保留重复压力、非空 WaitSet 自动退休与 16/16 竞态矩阵；`virt-release` 覆盖优化代码生成和 trap 寄存器保持。完整 core/stress wrapper 核验 workload、必选拓扑、RPC reject cleanup、监督 authority、Tunnel 和 IPC 等实际业务锚点。
- 平台与故障：`acceptance-platforms` 运行完整 core 的 sifive_u/nofd，默认 release，可在单条 recipe 显式选 debug；hetero 在调度域变更时额外运行。`acceptance-faults` 包含 Boot/IPI/VM fault/VM repair/DMA retention。VM fault 使用 memory workload，GDB 观察真实 RetryFault 后 detach，wrapper 核线程完成、Job/辅助能力收束和 reset；不再重复 FS/PM 与整套 stress。VM repair 是受限现场诊断，不声明完整业务收束。
- 完整 `acceptance` 组合上述三组，各路线只执行一次。正常 virt 必须由 QEMU 正常退出证明 shutdown 后端成功；`sifive_u` 在内核明确返回 reset 失败后主动收割，仍须满足完整 core 锚点。触发选择由[构建手册](../../plans/BUILD-AND-TEST.md)拥有；内部契约的未覆盖部分由所属计划记录。
