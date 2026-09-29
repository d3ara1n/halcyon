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
5. 以未发布 Bound owner 解析并装载唯一的 initial ELF，构造 StartupBlock，收编 payload backing，并把不再使用的 BootPackage 页回投用户库存；
6. 在提交前准备 root pool、root Job 与平台 primordial capabilities、执行需求绑定、挂起的入口线程（含 Job member 与 Ready 准入）；
7. 以与普通 ProcessStart 相同的提交协议一次发布全部状态并首次发布 init runnable。

BootPackage 缺失、损坏或 initial ELF 不可执行属于启动失败，不能退化为“没有服务也继续运行”。任何步骤失败都必须保持系统储备与用户供给不重叠，且不能留下已计入 root pool 又可从库存取得的同一物理页；Bind 成功但尚未发布的进程由专用未发布 Bound owner 持有，失败必须经有界 drain 收束，不能依赖页表或地址空间普通 Drop。Handle、Job member、线程、执行域和 Ready 容量的全部可失败准备均位于唯一启动提交点之前；提交后只允许固定、不可失败的发布序列，首次 runnable 交付也属于该责任，不能留给外层普通入队重新分配容量。在 initial process 发布前无法恢复的失败是明确的 boot failure。内核不遍历 `bin/`，不识别 pm/fs/driver，不组装服务间 mailbox，也不决定启动顺序。

未发布构造的失败责任由显式构造 owner 持有，成功时转交启动协议，失败时交给收束驱动。构造 owner 表示唯一处置权，不要求进程只有一个强引用；未运行线程对进程的引用必须经生命周期动作明确解除。资源摘取与正式进程 Drain 使用同源机制，启动驱动可以在启动环境显式逐批推进，但普通 Drop 不能隐藏遍历完整地址空间的循环；若构造入口用于运行期，则必须具备对应的有界完成驱动。

initial process 通常自然取得 PID 1，但 PID 1 只作 provenance。其 authority 完全来自 StartupBlock 中显式安装的 root MemoryPool、root Job、设备资源等 capabilities。init 的不可转移 PoolBinding 与可派生、可授予的 root pool Handle 指向同一账户，前者支付 init 的内部 page-backed storage，后者授权用户态资源管理；共享 authority 不复制额度。内核为 init 执行的特殊动作仅存在于 Building 阶段的 bootstrap launcher，并复用普通绑定、映射和启动契约，不形成可由普通进程调用的物理映射或特权 syscall。

## initfs 作为 StartupBlock payload

initfs 直接成为 init StartupBlock 的 opaque payload，不另建 Archive、MemoryObject 或 Handle：

```text
init 虚拟地址空间中的 StartupBlock
[Header][actual Handles][EntryThreadDescriptor][zero padding][read-only mapped initfs]
```

StartupBlock outer 在 Handle 数组后携带入口线程 descriptor，descriptor 与 payload 之间允许零填充。普通进程的小 payload 可以紧凑复制；init 的大 payload 从 BootPackage 页直接映射到同一连续虚拟块，`startup_payload()` 对二者提供相同切片语义。

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
- 按配置声明必选/可选服务集合，只有必选映像、构造和初始授权全部成功才发布完整拓扑；可选失败形成显式 Degraded；
- 完成授权图发布后进入带 deadline、工作预算与 authority handoff 的持续监督循环。

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
  → ProcessBindExecution 固定执行需求与初始域
  → ProcessMap / ProcessWrite 组装映像、栈与启动信息
  → ProcessGrant 安装初始 capabilities
  → ThreadCreate(Building) 创建一个或多个挂起线程，Control 安装到目标表
  → ProcessStart
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

Building 阶段的 Bind、BindExecution、Map/Write、Grant 与 ThreadCreate 各自是边界明确的原子组装动作；每个挂起线程在创建时已取得完整调度准入与离场资源。ProcessStart 只检查 AddressSpace 已 Bound、至少一条挂起线程、Job 门满足且没有其它 Building 操作在途，然后消费 builder、摘取预留发布链并发布全部挂起线程。地址空间变更与启动发布是两个职责明确的协议，不把整个 ELF 装载包装为一笔巨型内存事务。

Bootstrap 与普通 Start 共用同一发布协议：资源准备全部发生在各自的创建动作中，Start 只复检 Job、生命周期和线程集合，提交后由明确 owner 完成不可失败的 Ready 交接。初始成员和 capabilities 尚私有、普通进程已经处于可观察的 Building，是输入所有权的差别，不产生第二套提交尾段。终止早于提交时拒绝启动，挂起线程经离场协议发布 DONE；晚于提交时由已提交启动责任与正常终止机制接管全部线程，不因尚未实际入队而丢失它们。ProcessControl 身份不因启动而更换。launcher 可以按配置保留、转交或立即关闭 control；关闭 control 不终止进程。Start 前失败保持目标不可运行，并由组装者决定重试或放弃后完整收束；已成功的 Bind、Grant 或 ThreadCreate 资源已归目标所有，不因后续失败自动退回。

内核只提供 JobSeal、直接成员的有界分页枚举、按 ID 派生 capability 和单进程控制原语；递归 JobKill 由 pm 在用户态组合。Open Job 变空后仍可用于服务重启，Sealed Job 才在全部成员和 child Jobs 收束后进入 Dead。多数进程不持 JobControl，只通过 pm 协议请求创建或管理。

Dead 表示进程的地址空间和 HandleTable 已经完成释放；结构化终局原因与终态信号由仍存活的 ProcessControl shell 观察，观察 capability 不让已死亡进程继续占用运行资源。大规模 Handle 与页表收束由持管理 authority 的服务以有界内核原语分批驱动，Terminating 在最后一批完成前持续成立。

## 内核短路径

进程构造和收束 syscall 都必须有明确上界：单次 pool derive、Bind 所需页表准备、映射页数、写入字节、grant 数、线程附入数，以及单次关闭的 Handle 和回收的页表节点都有限；launcher 与 pm 以用户态循环完成大映像和大资源环境。内核不在一个 syscall 中遍历 archive、Job 子树、Pool 后代、完整 HandleTable、完整地址空间、对象图或不受限路径策略。

## 外部参照的边界

Zircon 把 bootdata 作为显式 VMO 交给普通 userboot，由 userboot 解析 BOOTFS 和后续 ELF；seL4 把 BootInfo 与 root capabilities 交给 root task，由用户态构造后续 VSpace/TCB/CNode。Halcyon 采用相同的责任边界，但不因此照搬 VMO、untyped 或某个具体启动协议；本系统的独立理由是让 eRhino 只保留不可替代的 bootstrap 机制，并让后续装载遵守同一 capability 模型。

## 用户态接收与一次性交接

程序环境与 syscall 分层见 [framework](framework.md)、[用户侧内核接口](kernel-interface.md)。本节拥有启动映像、初始能力和应用上下文的接收契约；它不定义 payload 中的服务 schema，也不赋予启动身份额外权限。

### 启动映像的寿命

所有遵守该程序环境契约的 launcher 都必须交付连续、完整、用户只读且不可执行的 StartupBlock 映像，并在首次进入应用前保持内容稳定。普通 launcher 可以直接按最终只读权限 Map，再经 Building Write 填充；不以接收者自律代替映射权限，也不为此复制 init 的大 payload。内核 bootstrap 与用户态 launcher 交付相同的接收契约，具体 backing 来源仍不同。

接收方把该映像作为**进程寿命输入**：直到进程地址空间收束前保持映射，不向安全用户态 API 交付可解除或写入它的 region owner。来自平台入口的块基和字节数不包含完整 reservation/guard/backing 责任，不能据此伪造 MappedRegion。libkernel 的任意地址 Unmap/Protect 或原始写入是 unsafe 边界；正常安全 region 操作必须使用自身真实 owner，不能影响此输入。

这项保活由地址空间及合法 launcher/程序环境契约承担，不是将 Rust 切片随意强转为 static。公开的 StartupPayload 持有经验证的只读视图凭据，借出的字节切片绑定到该视图的借用；它可以显式 clone 并跨本进程线程共享，但不拥有原始 Handle 数组，也不拥有 backing 回收权。丢弃所有 payload 视图不提前解除映像；只有进程地址空间收束返还 backing 与计费。未来若需要启动区提前回收，必须增加真实的映射交接契约后再改变此寿命，不能让当前上下文 Drop 偷做。

StartupBlock、初始 grants 和 payload 视图是进程环境资源；入口线程的执行栈与 ThreadControl 是线程资源。构建者经 Building Map 建立入口栈，ThreadCreate 把 child-local Control 安装到目标表，并将该句柄加入 StartupBlock 的 grants/Handle 数组；版本化 `EntryThreadDescriptor` 只引用对应零基索引与栈 reservation、usable 范围。子进程 runtime 以原子 once 采用该描述符，用 reservation 重建普通 region owner 并把入口线程登记为普通 ThreadRecord；第二次进入 `_start` 属于进程完整性失败，这是 Rust profile 只有一个入口的强制点。未采用前进程终止时，这些都是进程资源，由 ProcessDrain 收回，不跨地址空间运输 Rust 对象。采用后入口线程与运行期线程使用同一结果、离场和 cleanup 状态机，不按 TID/PID 分支。

进入 Rust 安全接口的原始指针接管仍是 unsafe：调用者须保证完整映像可读、长度可表示、内容稳定及上述寿命。outer 格式校验只能验证已可访问字节的布局，不能证明一个任意指针可读。平台入口适配器是该不变量的唯一生产接点，非法启动输入按程序失败收束，不把用户地址错误变成内核 panic。

### 启动凭据与采用

入口适配器创建不可 Clone、不可安全伪造的 StartupSeed。它唯一持有本次启动的一次接收责任，包含稳定映像的来源证明及尚未被其它用户态 owner 接管的初始表项集合。相同入口或指针不能经安全接口再次生成凭据；新线程调用不走第二次进程启动采用。

启动采用按以下顺序完成：

1. **纯验证**：无堆检查 outer 格式、边界、reserved 与长度表示。Handle 数值和 payload 此时只是借用数据，不向应用公开可消费的 owner。
2. **准备**：语言环境建立 allocator 后，按实际 Handle 数预留全部槽位与验证工作区；拒绝 INVALID 和重复表项数值。两个不同 Handle 指向同一 object 并不构成重复。
3. **核实**：对每个表项查询其真实 kind/role/rights，验证已知、可安全采用的非本地映射 role。不得按 object_id 数值重新打开对象，也不能要求接收后的 rights 仍含来源方此前使用的 GRANT/TRANSIT。
4. **采用提交**：所有可失败工作结束后，一次消费 Seed，把每个初始 entry 的关闭责任安装到唯一 InitialGrants 槽，并产生 CurrentProcessInfo 和 payload 视图。该用户态提交不执行新的内核 Grant，不改变表项 identity；提交后不再分配或执行可失败 Query。
5. **交付**：runtime 私有地把入口线程描述与其 Control 交给入口 ThreadRegistry；应用只取得不含描述的 StartupContext，消费式拆出身份、payload 与 InitialGrants，runtime 不保留可再次领取的副本。

准备或核实失败返回错误和原 Seed，未建立部分 typed owner，也不关闭任意 raw 初始表项。分配失败可以在已有环境中重试；非法/重复/不支持的输入不能跳过继续调用应用，runtime 走进程失败出口，内核表项由进程收束负责。丢弃未经验证的 Seed 不能以 Drop 猜测各 raw Handle 的安全关闭方式。

StartupSeed 的来源保证与 HandleQuery 缺一不可：Query 核实对象属性，Seed 保证表项尚未被用户态重复接管。单纯解析一段字节、得到一串看起来合法的 Handle，不足以创建 StartupContext。纯格式解析结果也不能安全转换为 CurrentProcessInfo 的来源凭据。

### 应用可见的接口责任

- **CurrentProcessInfo**：可以复制的启动身份与创建关系视图，字段由启动路径形成，不通过普通构造器任意填写。它用于正确传递创建者 provenance；不证明内核认证，不等于 ProcessControl。进程管理 API 接收该上下文而不读取 runtime 全局，也不接受易混淆的裸 parent_pid 替代。
- **StartupPayload**：只读、可 clone/共享的 opaque payload 视图；没有文件系统、服务配置或设备语义，解析这些语义的库自行负责。
- **InitialGrants**：本进程初始非映射 Capability 的唯一槽位 owner，不可 Clone；独占 move 可以跨线程，共享并发领取需由调用方显式同步。`take(index)` 与 `try_adopt(role)` 都是消费式领取，成功后槽位不可再次取得，未领取项由 Drop 正常关闭；不存在重复读裸 Handle 的接口。
- **StartupContext**：应用侧身份、payload 与 InitialGrants 的一次性交接值；入口描述不属于它。不存在安全的 `startup_handles() -> &'static [Handle]` 接管捷径。

按索引领取是仅发生在用户态槽位上的原子语义：索引越界或已领取都不修改其它槽；成功取走完整 Capability 并留下 Taken 状态。随后将它转换为具体 role 的失败，返还该 Capability 给调用者，不自动放回原槽或关闭。需要预先检查 role 时使用槽描述，不将检查结果当成第二个 owner。

未领取能力由 InitialGrants 继续负责。显式关闭剩余项只关闭它实际拥有的非映射 entry，已领取者完全由新的 owner 处理；关闭不是杀进程、Drain Job 或设备停机。合法表项的 Drop 使用 [object](object.md) 已定义的关闭/退休契约，可能停驻但不启动用户态业务清理循环；异常终止仍以进程 HandleTable 收束为最终责任。不因一个未用的管理 Handle 被 Drop 就宣告它管理的资源已经结束。
