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

Header 保存 magic、version、块长、pid、parent_pid、Handle 数、descriptor offset/length、payload offset/length 与 reserved。几何要求 Handle 数组、descriptor 与 payload 依序落在块内，空隙全零；descriptor 的 control_index 必须小于 handle_count，reserved 为零，栈区间无溢出且 usable 落在 reservation 内。Handle 数组保存目标进程 HandleTable 的真实句柄值（由 ProcessGrant 返回；Building ThreadCreate 的 Control 先在调用者表出生，再显式 Grant 到 child 表），不从 index 推导 slot/generation。普通 launcher 只写一次包含 Control 的完整块。

**`parent_pid` 语义**：描述目标进程的创建关系，即目标 launcher 的当前 `CurrentProcessInfo::pid()`（创建者 PID），与 ProcessQuery 快照中的 parent_pid 同一真值；孙进程的 parent 因而是直接父进程而非祖父。`SpawnRequest` 显式携带 launcher 从受控 `StartupContext` 读取的完整身份视图；不通过 runtime 全局 getter 隐式推导。

接收进程以出生参数 arg1/arg2（块基址与字节长度）在首线程入口读取（libruntime `startup::init` 沿用 a0/a1 契约）。init 的出生块 Handle 顺序固定为：Handle[0] root JobControl、Handle[1] primordial SystemReset、Handle[2] init ProcessControl、Handle[3] root MemoryPool、Handle[4] 入口 ThreadControl；索引由 `shared::startup::initial` 定义，descriptor 显式引用第 4 项。Body 付款 Pool 与 Handle[3] 指向同一个 root core，不复制额度。普通进程先排列显式 grants，Building ThreadCreate 的 Control 经 Grant 得到 child-local 句柄后追加到同一数组并由 descriptor 引用；其余 slot 含义由具体启动协议定义，不属于通用线格式。
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

Bootstrap私有Arc与SystemReset Arc都在root Pool中按实际live-layout先准入；初始Cap Vec按固定HANDLE_COUNT的实际Layout预付，commit消费分配后、发布Work前返费。Startup prefix编码器显式接收PAGE为payload_off/Vec容量，构造前按同一Layout付款，Loader持有block与其MetadataCharge，未发布失败一起收束。Bootstrap自己的header费随执行宿主最后loan释放，SystemReset公开对象费随其最后存活owner释放；这些出生费用不改StartupBlock格式、role或授权。

1. 通过普通 construct 创建 pid 1 的 Shell/Body，绑定执行需求，创建 Builder 与入口 Control；预约五个 primordial cap 槽，用真实句柄构造出生块并提交。Job 与 cap 表提交分段，表锁内没有 Job/对象业务提交。
2. 每步映射一个ELF run分片或栈分片，最多取VM的STEP_UNITS页；回填文件/出生块字节受同一分片字节数及uaccess上界共同约束。所有Map复用Operation/ChangeWork。记录持私有完成来源，在自身稳定WaitNode上arm/attach/复检，提示只唤醒同一维护记录；完成后唯一detach，不同步自旋推进整个映像。
3. payload的BootHeldExtent每次同样切出至多STEP_UNITS页，未移交尾段继续由Loader持有；片段与root charge合成BootFundedExtent，作为统一backing的物理owner进入普通只读View；普通页与boot页保持各自库存归还方式，View/整空间退休使用同一段游标。没有隐式lease区域。
4. 共用成员预约、Thread/Control/Departure 与 ReadyBatch 构造入口 Created，出生参数为块址/实际字节数、sp=USER_TOP。
5. 所有ELF字节拷贝完成后清除原始字节借用并归还package prefix；Loader从Option中取出唯一Builder，调用start后在私有锁外释放该强根。已提交启动由Builder队列根承担，Bootstrap仅持独立完成来源，等待VM静止、分批发布、Builder退费和WorkReceipt归还后的DONE。完成后detach并释放Loader/Bootstrap；保留Builder强根会阻止其last-live完成，因此等待期间没有该强边。启动错误冻结Shell并释放Loader；唯一initial无法构造属于致命错误：同步准入失败仍走Preparing→Failed，Ready后装载失败走既有runtime_stop全hart停驻。

VM/backing退款由[`mm.md`](mm.md)拥有，生命周期与启动门由[`task.md`](task.md)拥有。启动记录的分片准备也可能分配metadata，STEP_UNITS页数边界不代表完整原子时间界；源码检查、平台组合和未覆盖窗口由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)承载。

## 用户态公共 loader

`shared/elf` 是 bootstrap、用户态 launcher 与 host audit 共用的唯一静态 ELF admission。`validate` 一次检查 program-header 分类、segment 顺序/几何、文件边界、页级权限并集与 W^X、entry 的 executable file-byte 来源和 ISA requirement，并返回私有构造的 segments/runs/image_end；调用者不再重读原始 headers。`tools/audit-user-elf.py` 只启动同 crate 的 host binary。`user/libraries/libprocess` 直接按 runs 驱动映射；SpawnRequest 显式携带来源 MemoryPool，loader 以 Job/Pool CREATE authority 依次驱动 Create → 分块 ProcessMap/ProcessWrite → Grant → 映射按最终长度计算的出生区 → Building ThreadCreate → Grant Control 取得 child-local 句柄 → 追加 grants → 单次 Write 完整出生块 → Start。SpawnRequest 的 control rights 必须含 MANAGE，使任一步失败都能统一调用 `libprocess` 的观察/关闭收束链执行 builder close → 等 DONE → 核验 Dead → control close；Grant 已提交时，`SpawnFailure.grants` 返回 Consumed，否则返回 Retained，清理链自身的异常由 `cleanup_error` 单独保留。loader 不产生资源或创建 authority，调用者必须显式持 JobControl 与 MemoryPool。

`libprocess::map_birth_block`（`user/libraries/libprocess/src/management.rs`）将出生块放在validated ELF映像顶之上的页对齐位置，以READ权限分块映射；ThreadCreate后，launcher通过Building Write一次填充包含入口Control的完整块。内核`vm::operation::initial_write`在Building门保护下初始化这些页，不给接收进程增加WRITE映射权限。

平台入口以一次性StartupInput建立affine `StartupSeed`；`libprocess::context::StartupContext::adopt(seed)`先验证格式、索引和句柄去重，再Query全表核实可采用role，最后提交Capability owners。失败返回原Seed，不析构原始grant。入口descriptor的Control被转给`libthread::ThreadRegistry`，普通grant由runtime startup API显式转交。出生映像随目标地址空间持有，回收由[mm](mm.md)的View/地址空间退休路径承担。

## init/pm 当前政策

`user/services/srv_init` 把 opaque payload 当作私有 ustar，建立：

```text
root
├─ init
└─ services
   ├─ pm_domain
   └─ acceptance
```

init 以 `libprocess::RequiredLaunchSet` 声明式 manifest 要求 `srv_fs`、`srv_pm`、`drv_spi_sifive`、acceptance target 和 pm-domain target 全部存在并成功启动；`test_fp` 按 execution domain 可选，stress 档另要求 hammer 映像。必选缺失/失败使整个 stage 失败并由 services JobControl 收束，不继续发布正常拓扑。

所有常规服务是 services 的直接成员。init 的 RootSupervisor 在子服务启动前持有长期责任槽并预备 services 的待命 Job 机器；每个 ProcessControl 在启动成功后立即进入预留账本。辅助 Job、启动 mailbox、委托副本和流控通知均先预备根能力槽再创建；脚本借用 handle，pm sender 保持 typed owner，只有内核 Grant/Send 确认消费才从账本移交。关闭失败保留原 owner，活动 Job 借用的 root 不提前关闭。Process、Job、读取与待关闭 Runtime 的失败保留原机器、世界和结果，由同一个根组合等待与推进，不能只把裸 target 放回集合重新构造。只有 DONE 已观察和 Dead 快照成立后才兑现 control。pm 经出生块 grants 获得 Handle[0] mailbox owner 和 Handle[1] pm_domain JobControl；后者 rights 为 `MANAGE | READ | WAIT`，不含 CREATE。pm 使用正式 JobDriver 收束委托域，运行体以每轮一项推进；测试邮箱持续停驻到域完成后的 shutdown，回调确认 Active 登记实际消费 stop，最终检查注销与额度退款；不可恢复失败调用 ProcessAbort，由 init 的独立管理能力接管。具体结构与失败政策见 [runtime.md](runtime.md)。

`srv_fs` 的启动接缝前三项分别为 init 预建的 bootstrap Mailbox sender、release Notification owner、与普通 FAL grant 分离的 route-management Mailbox owner；A 的第四项是注册 Mailbox owner，B 的第四项是从 A 授权 root 派生的 exact-name 注册 sender。init 以同一映像启动两个独立 provider，为每个实例分别创建接缝、监督槽与 ProcessControl；provider 主线程直接运行正式 Runtime。A 回复 FAL/服务目录/授权 root 后，init 先订阅目录并派生 B 名称授权；B 主动 Register、PublishReady，init 通过 A 目录的 ServiceRecord 派生 B grant，再经 A 的 route endpoint 交付 B 母 grant 与权限 ceiling。普通 Namespace 只挂 A，`libfal::client`/`libfs::client` 通过 A 返回的 Delegate 访问 B，随后逐个 signal release。release 由 provider Runtime 自己观察；全部任务、来源和账户收束后，provider 经 bootstrap 返回固定宽 `ProviderReport`。缺少 grant 或 role 错误时 fail-closed。两组创建、ProcessGrant 消费、绑定、接收、release、报告和关闭均进入 RootSupervisor 能力账本；固定业务剧本属于 init 的验收政策，provider 内不再有 self-client 或兼容泵。

acceptance Job 收容一次性 IPC、Job 与可选竞态负载，结束后整域 job_kill。`srv_init` 默认编译为 core workload，只运行确定性内存、IPC、Tunnel、Job 与监督契约；`acceptance-stress` feature 在同一用户态编排器中追加 control/Tunnel 重复压力、非空集合自动退休与只读终态观察 和完整 16/16 竞态矩阵。profile 只改变 initfs 是否携带 `test_hammer` 及 init 的剧本分支，内核、StartupBlock 与 syscall ABI 均不感知。initfs 镜像打包时只剥离副本的调试段，构建目录保留原 ELF；新增独立 `test_fal` 后该收缩使 128 MiB sifive_u 上 A/B provider 与测试进程可同时装载，不减少用户程序的装载段或取消其他验收。init 在全部服务监督与资源收束锚点成立后，先以错误对象和裁剪掉 `MANAGE` 的 SystemReset 副本验证 capability 负路径，再直接提交 `Shutdown + Requested`。平台拒绝时记录明确错误并保留 RootSupervisor 的责任与单个 Assembly 接收 owner；失败路径不重建一套临时监督器。当前不经独立电源服务转发。

## 验证

- shared host：BootPackage/出生块 canonical geometry、零 padding 与空 payload；
- capability host：claim/restore、Close/shut 撤返还、rights 收窄、安装预约、真实持有数与 zero policy；
- libprocess host：ELF 前置、必选映像缺失/启动失败与非法 supervision policy 在 syscall 前拒绝并返还 authority；
- 工程静态门：`just clippy` 以 `-D warnings` 分别覆盖 shared/os/user host、kernel/user RISC-V、stress feature 与独立 gc target，完整日志写入 `artifacts/lint/`；`just acceptance` 在启动 QEMU 前先执行该门；
- QEMU acceptance：`virt` 是 debug core 快线，`virt-stress` 承担重复压力、非空 WaitSet 的进程自动退休与 16/16 竞态矩阵，`virt-release` 以 core 覆盖优化代码生成和 trap 寄存器保持，hetero/nofd 与 `sifive_u` 只叠加各自平台/调度域差异；`acceptance` 聚合阶段收尾所需的 stress、release、sifive_u、nofd 与外部 GDB 启动失败注入。每种 wrapper 都校验 `acceptance workload: core|stress`、必选拓扑、RPC reject cleanup、监督预算耗尽后 authority 续接、Tunnel 失败 owner/独立 View 退款和 committed IPC caller kill 等公共锚点。virt 必须由 QEMU 正常退出证明 shutdown 后端成功；`sifive_u` 在内核明确返回 reset 失败后主动收割。锚点随当前消费者维护，内部契约的未覆盖部分由所属计划记录。
