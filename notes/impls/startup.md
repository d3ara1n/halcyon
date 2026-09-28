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

Header 保存 magic、version、块长、pid、parent_pid、Handle 数、descriptor offset/length、payload offset/length 与 reserved。几何要求 Handle 数组、descriptor 与 payload 依序落在块内，空隙全零；descriptor 的 control_index 必须小于 handle_count，reserved 为零，栈区间无溢出且 usable 落在 reservation 内。Handle 数组保存目标进程 HandleTable 的真实句柄值（由 ProcessGrant 或 Building ThreadCreate 输出），不从 index 推导 slot/generation。普通 launcher 只写一次包含 Control 的完整块。

**`parent_pid` 语义**：描述**目标进程**的创建关系，因此是**组装者自身的 pid**（= 目标的创建者，与内核 ProcessQuery 快照的 parent_pid 同一真值），不是组装者的父。组装者在构造出生块时必须传 `libruntime::startup::pid()`——传 `env::parent_pid()` 会错位一代（init parent 0 spawn 服务时出生块里会是 0 而非 1）。当前无消费方（潜伏字段），但不能靠「没人读」放任语义错误。

接收进程以出生参数 arg1/arg2（块基址与字节长度）在首线程入口读取（libruntime `startup::init` 沿用 a0/a1 契约）。init 的出生块 Handle 顺序固定为：Handle[0] root JobControl、Handle[1] primordial SystemReset、Handle[2] init ProcessControl、Handle[3] root MemoryPool、Handle[4] 入口 ThreadControl；索引由 `shared::startup::initial` 定义，descriptor 显式引用第 4 项。内部 PoolBinding 与 Handle[3] 指向同一个 root core，不复制额度。普通进程先排列显式 grants，Building ThreadCreate 的 child-local Control 追加到同一数组并由 descriptor 引用；其余 slot 含义由具体启动协议定义，不属于通用线格式。

## 组装 ABI（Building 期外部通道）

`shared/erhino_shared/src/proc.rs` 与 `shared/erhino_shared/src/call.rs` 定义 fixed-width ABI，`libkernel` 封装位于 `user/libraries/libkernel/src/process.rs`：

- JobControl `CREATE`：JobCreate、ProcessCreate；
- JobControl `MANAGE`：JobSeal、JobDerive；`READ`：JobQuery、JobEnumerate；
- ProcessCreate：一次事务交付 page-resource-light 的 Unbound shell、affine ProcessBuilder 与稳定 ProcessControl；
- **ProcessBindMemory**：以 Pool GRANT authority 一次性把 shell 绑定为完整 AddressSpace；成功消费传入 Pool Handle，失败保留；
- ProcessMap/Write：只接受精确 Building 且已 Bound 的 process，完成匿名零页映射与 backing 回填；
- **ProcessGrant**：把 grants 从组装者表移入目标表，输出目标侧句柄值数组（组装者写入出生块）；
- **ProcessBindExecution**：与所有 Building 操作共用 lease，在首次 ThreadCreate 前冻结 ISA requirement 与兼容调度域；重复绑定返回 ObjectNotAvailable。
- **ThreadCreate**：拷入 `ThreadStartContext`，target 为 ProcessBuilder 或 self 哨兵；校验 Bound/执行绑定，构造完整 Created 线程、ThreadControl 与 Running 期全寿命 Ready 准入，输出 `ThreadCreateResult{tid, control}`。Building control 原子安装目标表，Running control 安装调用者表；输出写失败冻结调用者 `Faulted(UserAccess)`。
- **ThreadStart**：只接受调用进程自己的带 MANAGE right 的 ThreadControl；Running Created 在线程 lifecycle 锁内转 Ready 并消费准入发布，Building 线程拒绝，不分配。
- **ProcessStart**：只检查 Bound、执行绑定、已创建线程、Job 门与无并发 Building 操作，随后消费 builder 并批量发布 Building Created 线程；发布尾段已改为 ThreadCreate 时预留准入与 Thread 内嵌发布链，Start 锁内摘取、锁外逐项交给调度器。
- **Building abandonment**：ProcessBuilder 最后 authority 消散进入同一进程终止入口；Process 与 Running 未启动线程的 Created 项均经 ThreadDeparture 发布 DONE，ThreadControl 的最后 capability 引用消散也在成员锁内领取 Created 的离场权。
- **bootstrap**：在无并发启动路径复用 ThreadCreate 的 Control、Created 成员、准入及 ThreadDeparture，使用相同 descriptor/Handle 索引规则；其初始资源准备仍由内核承担。

## 唯一 init bootstrap

`os/kernel/src/boot.rs` 复用 Building 的绑定与映射基元构造 init，并与普通发布路径共享同一 Commit 前准备/最终不可失败发布纪律：

1. 验证 BootPackage 并以不可伪造的 `BootHeldExtent` 按 payload_off 切分物理 owner，消费 supply seed 铸造唯一 root Pool；
2. 创建 pid 1 的 Unbound shell，调用与 syscall 共用的 Bind helper 安装 root-funded PoolBinding；`UnpublishedBound` 持出生时预付的 deferred slot，构造失败由显式 rollback 发布有界 ProcessDrain continuation，Drop 只断言 owner 已移交；
3. 创建 root JobControl、primordial SystemReset、init ProcessControl、指向同一 root core 的 MemoryPool 管理 Handle 与入口 ThreadControl，预留五个 Handle 槽；前四项索引固定，第五项由 descriptor 引用，实际安装在后面的提交段；
4. 以真实句柄值构造出生块 prefix；payload owner 先与 root charge 合成 `BootFundedExtent`，prefix 在发布前直接回填，随后以 owner 借用投影完成可失败映射并无分配地安装本体，形成不可公开 Unmap 的只读 lease backing，不经历回库存再取得或无 owner 映射窗口；
5. 在最终发布前准备 execution domain、`ReadyBatch` 全寿命调度准入、单线程 staged 缓冲与首线程 Attach；初始 entries 由 `PendingEntries` affine owner 保持，Handle reservation 先验证为 `PreparedCommit`；
6. 最终临界区按 `HANDLE_TABLE → JOB_INNER → LIFECYCLE` 同时发布 capability、Job membership、Running 状态和 execution binding，不存在仍返回 `Result` 的不可逆动作；随后发布 `UnpublishedBound` 并交出携带同源 credit 的 `AdmittedThread`；
7. `boot.rs` 执行内存池 syscall 自检、回投 package prefix，最后消费该 owner，经无分配 Ready enqueue 首次发布线程。成功后 Kill 由 lifecycle/pick gate 接管，不能遗失尚未入队的 owner。

结构与组合验证由 [`地址空间事务与启动纵向档案`](../../plans/archived/todo-2026-09-memory-transaction-state-machine.md) 记录。validated ELF admission 已由 `elf::validate` 接入此发布链；它在任何 Process/页表 owner 构造前冻结映像事实，不改变这里的构造 owner 与提交协议。

initial ELF 与 prefix 完成后，package 前缀 owner 首次回投帧池；payload backing 与 root PoolBinding 在 init AddressSpace 有界收束时于锁外同步归还物理 extent 与 charge。内核没有 pid 特判的保留洞。

## 用户态公共 loader

`shared/elf` 是 bootstrap、用户态 launcher 与 host audit 共用的唯一静态 ELF admission。`validate` 一次检查 program-header 分类、segment 顺序/几何、文件边界、页级权限并集与 W^X、entry 的 executable file-byte 来源和 ISA requirement，并返回私有构造的 segments/runs/image_end；调用者不再重读原始 headers。`tools/audit-user-elf.py` 只启动同 crate 的 host binary。`user/libraries/libprocess` 直接按 runs 驱动映射；SpawnRequest 显式携带来源 MemoryPool，loader 复制 GRANT-only authority并依次驱动 Create → BindMemory → 分块 ProcessMap/ProcessWrite → Grant → 映射按最终长度计算的出生区 → Building ThreadCreate → 将 child-local Control 追加 grants → 单次 Write 完整出生块 → Start。SpawnRequest 的 control rights 必须含 MANAGE，使任一步失败都能统一调用 `libprocess` 的 drain/close 收束链执行 builder close → ProcessDrain → control close；Grant 已提交时，`SpawnFailure.grants` 返回 Consumed，否则返回 Retained，清理链自身的异常由 `cleanup_error` 单独保留。loader 不产生资源或创建 authority，调用者必须显式持 JobControl 与 MemoryPool。

当前 `libprocess::map_birth_block`（`user/libraries/libprocess/src/lib.rs`）使用 READ|WRITE 映射出生块，ThreadCreate 后一次填充；`libprocess::context::StartupContext` 在 `libruntime` 入口完成 StartupBlock 校验后的 grants 一次采用，并按索引保有 Capability owner。入口 descriptor 的 Control 被转给 `libthread::ThreadRegistry`，普通 grant 由 runtime startup API 显式转交。内核 `AddressSpace::write_building`（`os/kernel/src/task/proc.rs`）不要求目标最终 PTE 可写，因此保持现有 Building 写入契约。统一只读/进程寿命映像及 owner 责任见 [bootstrap 接收契约](../ideas/bootstrap.md#启动映像的寿命)，本轮迁移已通过完整 acceptance。

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

所有常规服务是 services 的直接成员。init 的 RootSupervisor 在子服务启动前持有长期责任槽并预备 services 的待命 Job 机器；每个 ProcessControl 在启动成功后立即进入预留账本。辅助 Job、启动 mailbox、委托副本和流控通知均先预备根能力槽再创建；脚本借用 handle，pm sender 保持 typed owner，只有内核 Grant/Send 确认消费才从账本移交。关闭失败保留原 owner，活动 Job 借用的 root 不提前关闭。Process、Job、读取与待关闭 Runtime 的失败保留原机器、世界和结果，由同一个根组合等待与推进，不能只把裸 target 放回集合重新构造。只有 Drain Complete 和 Dead 快照成立后才兑现 control。pm 经出生块 grants 获得 Handle[0] mailbox owner 和 Handle[1] pm_domain JobControl；后者 rights 为 `MANAGE | READ | WAIT`，不含 CREATE。pm 使用正式 JobDriver 收束委托域，运行体以 max_work=1 推进；测试邮箱持续停驻到域完成后的 shutdown，回调确认 Active 登记实际消费 stop，最终检查注销与额度退款；不可恢复失败调用 ProcessAbort，由 init 的独立管理能力接管。具体结构与失败政策见 [runtime.md](runtime.md)。

`srv_fs` 的启动接缝前三项分别为 init 预建的 bootstrap Mailbox sender、release Notification owner、与普通 FAL grant 分离的 route-management Mailbox owner；A 的第四项是注册 Mailbox owner，B 的第四项是从 A 授权 root 派生的 exact-name 注册 sender。init 以同一映像启动两个独立 provider，为每个实例分别创建接缝、监督槽与 ProcessControl；provider 主线程直接运行正式 Runtime。A 回复 FAL/服务目录/授权 root 后，init 先订阅目录并派生 B 名称授权；B 主动 Register、PublishReady，init 通过 A 目录的 ServiceRecord 派生 B grant，再经 A 的 route endpoint 交付 B 母 grant 与权限 ceiling。普通 Namespace 只挂 A，`libfal::client`/`libfs::client` 通过 A 返回的 Delegate 访问 B，随后逐个 signal release。release 由 provider Runtime 自己观察；全部任务、来源和账户收束后，provider 经 bootstrap 返回固定宽 `ProviderReport`。缺少 grant 或 role 错误时 fail-closed。两组创建、ProcessGrant 消费、绑定、接收、release、报告和关闭均进入 RootSupervisor 能力账本；固定业务剧本属于 init 的验收政策，provider 内不再有 self-client 或兼容泵。

acceptance Job 收容一次性 IPC、Job 与可选竞态负载，结束后整域 job_kill。`srv_init` 默认编译为 core workload，只运行确定性内存、IPC、Tunnel、Job 与监督契约；`acceptance-stress` feature 在同一用户态编排器中追加 control/Tunnel 重复压力、`max_work=1` Drain 和完整 16/16 竞态矩阵。profile 只改变 initfs 是否携带 `test_hammer` 及 init 的剧本分支，内核、StartupBlock 与 syscall ABI 均不感知。initfs 镜像打包时只剥离副本的调试段，构建目录保留原 ELF；新增独立 `test_fal` 后该收缩使 128 MiB sifive_u 上 A/B provider 与测试进程可同时装载，不减少用户程序的装载段或取消其他验收。init 在全部服务监督与资源收束锚点成立后，先以错误对象和裁剪掉 `MANAGE` 的 SystemReset 副本验证 capability 负路径，再直接提交 `Shutdown + Requested`。平台拒绝时记录明确错误并保留 RootSupervisor 的责任与空闲等待集合；失败路径不重建一套临时监督器。当前不经独立电源服务转发。

## 验证

- shared host：BootPackage/出生块 canonical geometry、零 padding 与空 payload；
- handle_table host：consume/transfer 两类 pin、builder 保护、自授予/重复拒绝、rights 回滚、reservation 与 TRANSIT/GRANT；
- libprocess host：ELF 前置、必选映像缺失/启动失败与非法 supervision policy 在 syscall 前拒绝并返还 authority；
- 工程静态门：`just clippy` 以 `-D warnings` 分别覆盖 shared/os/user host、kernel/user RISC-V、stress feature 与独立 gc target，完整日志写入 `artifacts/lint/`；`just acceptance` 在启动 QEMU 前先执行该门；
- QEMU acceptance：`virt` 是 debug core 快线，`virt-stress` 承担最小预算 Drain、重复压力与 16/16 竞态矩阵，`virt-release` 以 core 覆盖优化代码生成和 trap 寄存器保持，hetero/nofd 与 `sifive_u` 只叠加各自平台/调度域差异；`acceptance` 聚合阶段收尾所需的 stress、release、sifive_u、nofd 与外部 GDB 启动失败注入。每种 wrapper 都校验 `acceptance workload: core|stress`、必选拓扑、RPC reject cleanup、监督预算耗尽后 authority 续接、Tunnel 失败库存守恒和零 abandoned stack 等公共锚点，避免构建 feature 与所需语义错配。virt 必须由 QEMU 正常退出证明 shutdown 后端成功；`sifive_u` 在内核明确返回 reset 失败后主动收割。
