# Bootstrap 与用户态 launcher

Bootstrap 只负责建立第一个用户态 authority root；服务发现、配置、文件系统、设备模型和后续程序装载都从该 root 在用户态展开。内核只启动一个 initial process，约定名称为 init；init 进入用户态后不因 PID 或名称获得任何隐藏权限。

## 两层格式

启动产物分为内核可见的 BootPackage 与只对 init 有意义的 initfs：

```text
BootPackage
├── fixed envelope
├── initial ELF
└── page-aligned opaque payload（initfs）
```

BootPackage envelope 是 eRhino boot ABI，只描述自身总长、initial ELF 与 opaque payload 的边界。字段固定宽、little-endian、版本化，reserved 必须为零；所有 offset/length 必须无溢出、位于外部加载器声明的物理区间内且互不重叠。payload 起点页对齐，尾部页填充为零。

内核不把 payload 当文件系统，不解释路径、配置、其他 ELF 或归档格式。payload 初期可以是 ustar，未来可以换为索引归档、压缩镜像或其他私有协议，而不改变 BootPackage 或内核。

## 唯一 initial process

内核启动路径只执行：

1. 规范化平台物理供给，扣除永久保留与系统储备，并把 BootPackage 等仍在使用但将归入用户供给的页登记为 boot-held；
2. 验证 BootPackage envelope，建立唯一 root MemoryPool 账户和初始 metadata admission；
3. 创建 root Job 与 init 的 Building 空壳；
4. 以普通 `ProcessBindMemory` 的内部同构语义把 init 绑定到 root pool；
5. 解析并装载唯一的 initial ELF，构造 StartupBlock，收编 payload backing，并把不再使用的 BootPackage 页回投用户库存；
6. 安装 root pool、root Job 与平台 primordial capabilities，附入首线程；
7. 通过普通 readiness 检查首次发布 init runnable。

BootPackage 缺失、损坏或 initial ELF 不可执行属于启动失败，不能退化为“没有服务也继续运行”。任何步骤失败都必须保持系统储备与用户供给不重叠，且不能留下已计入 root pool 又可从库存取得的同一物理页；在 initial process 发布前无法恢复的失败是明确的 boot failure。内核不遍历 `bin/`，不识别 pm/fs/driver，不组装服务间 mailbox，也不决定启动顺序。

initial process 通常自然取得 PID 1，但 PID 1 只作 provenance。其 authority 完全来自 StartupBlock 中显式安装的 root MemoryPool、root Job、设备资源等 capabilities。init 的不可转移 PoolBinding 与可派生、可授予的 root pool Handle 指向同一账户，前者支付 init 的内部 page-backed storage，后者授权用户态资源管理；共享 authority 不复制额度。内核为 init 执行的特殊动作仅存在于 Building 阶段的 bootstrap launcher，并复用普通绑定、映射和启动契约，不形成可由普通进程调用的物理映射或特权 syscall。

## initfs 作为 StartupBlock payload

initfs 直接成为 init StartupBlock 的 opaque payload，不另建 Archive、MemoryObject 或 Handle：

```text
init 虚拟地址空间中的 StartupBlock
[Header][actual Handles][zero padding][read-only mapped initfs]
```

StartupBlock outer 允许 Handle 数组结束与 payload 起点之间存在零填充。普通进程的小 payload 可以紧凑复制；init 的大 payload 从 BootPackage 页直接映射到同一连续虚拟块，`startup_payload()` 对二者提供相同切片语义。

映射必须为用户可读、不可写、不可执行。payload 页不进入普通进程可转授对象图：关闭 Handle 不能影响它，也没有任何运行时入口能重新映射或转授这些页。映入 init 前，这些页从 boot-held 原子转换为由 init root pool 支付的 funded backing；页与 primordial charge 随地址空间销毁一起经正常 retire 返回库存和 pool，不允许只迁移物理所有权而遗漏计费。BootPackage 物理占用按 envelope 的实际总长保留，而不是按 Devicetree 中的最大装载窗口保留。

BootPackage 的 boot-held token 在 envelope 验证后按最终用途切分：完整 payload 页直接收编，仍承载被映射尾页的部分页随 backing 保留；只用于 envelope、initial ELF 源数据或对齐填充且复制完成后不再被访问的页回投 free inventory。init 发布前必须完成该切分，boot-held 只允许保留仍有明确启动环境 owner 的区间；不能让 root pool 的可用额度长期对应既非 funded 也不可 claim 的遗失页。

这种映射不是进程特权口子：没有运行时入口能选择物理地址，也没有其他进程可请求同类映射。它与 initial ELF、主栈和 StartupBlock prefix 一样，是 bootstrap 作为 Building 组装者经同一 AddressSpace interface 构造首地址空间的固定输入；不同之处只在 backing 来自已验证的 boot-held 页而非空闲库存。

## init 的职责与终止

init 是系统配置选定的持久 root supervisor。它初始取得完整 root JobControl、系统复位 authority 与其他平台 primordial capabilities，在用户态维护系统进程拓扑、恢复政策和长期 authority，因此可以 spawn、kill、派生、授权、收束服务并显式提交系统终局。该职责来自初始 capability 图与配置，不由内核按 PID 或名称授予隐藏权限。

init 独占解释 initfs，并按配置：

- 建立初始文件系统路由与设备模型；
- 通过公共 loader 能力创建服务进程；
- 创建 endpoint、Job 和管理 capability；
- 将 ProcessControl、JobControl、设备资源与 namespace grants 交给相应服务；
- 完成授权图发布后进入持续监督循环。

init 位于其受管 services Job 之外，直接持有该 JobControl 及系统服务的 ProcessControls，负责服务的创建、退出处理、重启和递归收束。系统服务与其受托管理子域同属 services 域，整树可一次封口收束；委托只转移子域的域内管理 authority，init 对受托域保留直接收束权。pm 是 services Job 内可被 init 监督的系统服务，可以管理 init 显式委托的子域或向其他进程提供进程管理协议，但不承担根监督链角色。委托的语义在 capability 转移本身：启动时在 Building 阶段经直接 grant 交付与运行时经管理协议授予是同一转移的两个时机，不构成第二种委托机制。重启是监督政策的维度：受管服务收束后是否重新创建由配置决定，重启域（Open Job）为此保持可用；不重启政策下收束完成即终态记录。

init 完成授权图发布后进入持续监督循环；全部受管服务收束完毕后仍由用户态政策决定继续监督、关机或重启。无就绪线程、无期限或全部 hart 进入 WFI 只描述调度状态，不构成隐式停机；系统终局必须经 [`system-reset.md`](system-reset.md) 定义的 authority 显式提交。

init 的退出、panic、fault 或显式 kill 在内核中仍按普通进程处理：已经交付的 capabilities 与服务可以继续运行，内核不级联终止服务、不自动重启 init，也不重新铸造丢失 authority。但系统会失去唯一的拓扑维护、恢复、资源收束和系统复位保证，进入配置定义的 unmanaged/failed 状态。这里没有 init 专用内核回收或自动停机路径，只有用户态政策上的管理根失效。

归档内路径只属于 initfs 协议。内核不提供按路径 spawn，也不因二进制名称授予 authority。

## 用户态 ELF 装载

内核只为 bootstrap initial process 解析 ELF。其余 ELF 由 launcher 在用户态解析，内核提供有界的 Building-process 构造机制：

```text
Job capability + MemoryPool capability
  → ProcessCreate
  → Building process + affine ProcessBuilder + ProcessControl
  → ProcessBindMemory
  → ProcessMap / ProcessWrite 组装映像与启动信息
  → ProcessGrant 安装初始 capabilities
  → ProcessAttach 附入一个或多个线程现场
  → ProcessStart(execution profile)
  → consume ProcessBuilder and publish Running process
```

ProcessCreate 只建立空壳，MemoryPool authority 在 Bind 前仍由 launcher 持有；Bind 成功后目标取得不可转移内部 binding，不自动取得 Pool Handle。launcher 若要让目标继续 Query、Derive 或转授预算，必须另经 ProcessGrant 安装显式 Pool capability。launcher 负责 ELF program-header、段重叠、BSS、最终页权限、栈布局和执行需求；内核只验证地址范围、页权限、W^X、Building 状态、内存已绑定、入口可执行、栈可写与 ABI 对齐，不读取文件名或 ELF 结构。

这套逻辑不是由每个有 spawn 权的服务各写一遍，而是分层为公共用户态能力：

- **libelf**：解析 ELF、校验 program headers 与执行需求；kernel bootstrap 与用户态 launcher 共用同一纯逻辑实现；
- **libprocess**：规划页、驱动 ProcessBuilder，组装参数、namespace、grants 与初始线程现场，并以 ProcessStart 完成高层 spawn；
- **ld-erhino**：未来处理 `PT_INTERP`、重定位和共享库解析，运行在新进程内，通过显式 loader service/DirectoryGrant 取得库；init/pm 不亲自链接每个动态库。

只有持 Job/Process 构造 capability 的进程能使用这些机制；链接库提供实现复用，不产生 authority。多数服务不持 spawn 权，需要新进程时调用 pm 协议。

匿名映射与 Building-only 写入是地址空间构造原语，不是 `SpawnElf`。显式 MemoryObject backing 是另一种映射来源，不改变 Process 的生命周期和提交边界；具体映射语义由[内存模型](mm.md)拥有。

## Job 与 Process capability

root Job 与 root MemoryPool 由内核各自铸造并作为 init 的首批启动 Handles 交付。init 的 PoolBinding 已在 Handle 安装前指向同一 root pool core；两条引用共享账户而不复制供给。init 按配置从 root pool 派生 child pool、从 root Job 创建 child Job，再把两者与 CPU、设备等 capability 组合交给服务；关闭最后一个 JobControl 不隐式杀死成员，关闭最后一个 Pool Handle 也不撤销已绑定或已分配内存。ProcessCreate 必须持有 Job 创建权；创建关系记录为 `parent_pid`，但不产生管理权。

Process 生命周期：

```text
Building → Running → Terminating → Dead
```

- **ProcessBuilder**：Building 阶段唯一、affine 的构造权；关闭即放弃构造并使目标进入终止收束；
- **ProcessControl**：ProcessCreate 即产生的稳定管理/观察 capability，贯穿 Building、Running、Terminating 与 Dead；管理、等待、复制和运输由 rights 收窄；
- **JobControl**：创建、封口、分页枚举与按 ID 派生直接成员、故障收束的 authority，不是进程权限等级；
- **MemoryPool**：page-backed storage authority；可以共享、移动和派生 child，但成功附入进程的 PoolBinding 不再是 Handle，也不可转移。

Building 阶段的 Bind、Map/Write、Grant 与 Attach 各自是边界明确的原子组装动作；ProcessStart 必须检查 AddressSpace 已 Bound、至少一条线程已附入、执行资源和 Job 门均满足，并在没有其它 Building 操作在途时冻结执行绑定、消费 builder、一次发布全部预育线程。ProcessControl 身份不因启动而更换。launcher 可以按配置保留、转交或立即关闭 control；关闭 control 不终止进程。Start 前失败保持目标不可运行，并由组装者决定重试或放弃后完整收束；已成功 Bind 或 Grant 的资源已归目标所有，不因后续 Attach/Start 失败自动退回。

内核只提供 JobSeal、直接成员的有界分页枚举、按 ID 派生 capability 和单进程控制原语；递归 JobKill 由 pm 在用户态组合。Open Job 变空后仍可用于服务重启，Sealed Job 才在全部成员和 child Jobs 收束后进入 Dead。多数进程不持 JobControl，只通过 pm 协议请求创建或管理。

Dead 表示进程的地址空间和 HandleTable 已经完成释放；exit status 与终态信号由仍存活的 ProcessControl shell 观察，观察 capability 不让已死亡进程继续占用运行资源。大规模 Handle 与页表收束由持管理 authority 的服务以有界内核原语分批驱动，Terminating 在最后一批完成前持续成立。

## 内核短路径

进程构造和收束 syscall 都必须有明确上界：单次 pool derive、Bind 所需页表准备、映射页数、写入字节、grant 数、线程附入数，以及单次关闭的 Handle 和回收的页表节点都有限；launcher 与 pm 以用户态循环完成大映像和大资源环境。内核不在一个 syscall 中遍历 archive、Job 子树、Pool 后代、完整 HandleTable、完整地址空间、对象图或不受限路径策略。

## 外部参照的边界

Zircon 把 bootdata 作为显式 VMO 交给普通 userboot，由 userboot 解析 BOOTFS 和后续 ELF；seL4 把 BootInfo 与 root capabilities 交给 root task，由用户态构造后续 VSpace/TCB/CNode。Halcyon 采用相同的责任边界，但不因此照搬 VMO、untyped 或某个具体启动协议；本系统的独立理由是让 eRhino 只保留不可替代的 bootstrap 机制，并让后续装载遵守同一 capability 模型。
