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

1. 验证 BootPackage envelope；
2. 解析并装载唯一的 initial ELF；
3. 创建 root Job 与平台 primordial capabilities；
4. 构造 init 的 StartupBlock；
5. 首次发布 init runnable。

BootPackage 缺失、损坏或 initial ELF 不可执行属于启动失败，不能退化为“没有服务也继续运行”。内核不遍历 `bin/`，不识别 pm/fs/driver，不组装服务间 mailbox，也不决定启动顺序。

initial process 通常自然取得 PID 1，但 PID 1 只作 provenance。其 authority 完全来自 StartupBlock 中显式安装的 root Job、设备资源等 capabilities。内核为 init 执行的特殊动作仅存在于 Building 阶段的 bootstrap launcher，不形成可由普通进程调用的物理映射或特权 syscall。

## initfs 作为 StartupBlock payload

initfs 直接成为 init StartupBlock 的 opaque payload，不另建 Archive、MemoryObject 或 Handle：

```text
init 虚拟地址空间中的 StartupBlock
[Header][actual Handles][zero padding][read-only mapped initfs]
```

StartupBlock outer 允许 Handle 数组结束与 payload 起点之间存在零填充。普通进程的小 payload 可以紧凑复制；init 的大 payload 从 BootPackage 页直接映射到同一连续虚拟块，`startup_payload()` 对二者提供相同切片语义。

映射必须为用户可读、不可写、不可执行。payload 页不进入普通进程可转授对象图：关闭 Handle 不能影响它，也没有任何运行时入口能重新映射或转授这些页；页所有权在映入 init 时即移交为该地址空间的 backing，随地址空间销毁自然归还物理池。BootPackage 物理占用按 envelope 的实际总长保留，而不是按 Devicetree 中的最大装载窗口保留。

这种映射不是进程特权口子：没有运行时入口能选择物理地址，也没有其他进程可请求同类映射。它与 initial ELF、主栈和 StartupBlock prefix 一样，是内核构造首地址空间的固定部分。

## init 的职责与终止

init 是临时的授权根，不是永久管理根。它初始取得完整 root JobControl 与平台 primordial capabilities，因此在机制上可以 spawn、kill、派生和授权；“不负责运行期管理”是启动配置规定的职责，而不是内核人为削弱其能力。

init 独占解释 initfs，并按配置：

- 建立初始文件系统路由与设备模型；
- 通过公共 loader 能力创建服务进程；
- 创建 endpoint、Job 和管理 capability；
- 将 ProcessControl、JobControl、设备资源与 namespace grants 交给相应服务；
- 完成授权图发布后退出。

init 的正常退出、panic、fault 或显式 kill 都按普通进程处理。已经交付的 capabilities 与服务继续存在；仍只由 init 持有的 authority 随 HandleTable drain 消散。内核不级联终止服务、不自动重启 init，也不重新铸造丢失 authority。初始化配置必须保证所有需要长期存续的管理权在 init 退出前完成交付。

归档内路径只属于 initfs 协议。内核不提供按路径 spawn，也不因二进制名称授予 authority。

## 用户态 ELF 装载

内核只为 bootstrap initial process 解析 ELF。其余 ELF 由 launcher 在用户态解析，内核提供有界的 Building-process 构造机制：

```text
Job capability
  → ProcessCreate
  → Building process + affine ProcessBuilder
  → map anonymous target pages
  → copy bounded bytes into mapped target pages
  → ProcessStart(entry, stack, execution profile, grants, payload)
  → Running process + ProcessControl
```

launcher 负责 ELF program-header、段重叠、BSS、最终页权限、栈布局和执行需求。内核只验证地址范围、页权限、W^X、Building 状态、入口可执行、栈可写与 ABI 对齐，不读取文件名或 ELF 结构。

这套逻辑不是由每个有 spawn 权的服务各写一遍，而是分层为公共用户态能力：

- **libelf**：解析 ELF、校验 program headers 与执行需求；kernel bootstrap 与用户态 launcher 共用同一纯逻辑实现；
- **libprocess**：规划页、驱动 ProcessBuilder，并组装参数、namespace、grants 与 ProcessStart 的高层 spawn 接口；
- **ld-erhino**：未来处理 `PT_INTERP`、重定位和共享库解析，运行在新进程内，通过显式 loader service/DirectoryGrant 取得库；init/pm 不亲自链接每个动态库。

只有持 Job/Process 构造 capability 的进程能使用这些机制；链接库提供实现复用，不产生 authority。多数服务不持 spawn 权，需要新进程时调用 pm 协议。

匿名映射与 Building-only 写入是地址空间构造原语，不是 `SpawnElf`。将来增加显式 MemoryObject backing 时，它是另一种映射来源，不改变 Process 的生命周期和提交边界。

## Job 与 Process capability

root Job 由内核铸造并作为 init 的首批启动 Handle 交付。init 初始持完整 JobControl，按配置把长期管理权交给 pm 或其他服务；关闭最后一个 JobControl 不隐式杀死成员，终止必须是显式操作。ProcessCreate 必须持有 Job 创建权；创建关系记录为 `parent_pid`，但不产生管理权。

Process 生命周期：

```text
Building → Running → Terminating → Dead
```

- **ProcessBuilder**：Building 阶段唯一、affine 的构造权；关闭即放弃并回收未发布进程；
- **ProcessControl**：Running 后的显式管理/观察 capability；管理、等待、复制和运输由 rights 收窄；
- **JobControl**：创建子 Job/Process、预算和故障收束的 authority，不是进程权限等级。

ProcessStart 成功时消费 builder，原子安装 GRANT entries、映射通用 StartupBlock、设置首线程上下文、发布进程并返回 ProcessControl。launcher 可以按配置保留、转交或立即关闭该 control；关闭 control 不终止进程。失败保持 builder、调用方 Handles 与目标不可运行状态；同一 builder 不得同时作为 ProcessStart target 与 grant 项。

Dead 进程的地址空间和 HandleTable 应立即释放，exit status 与终态信号可由仍存活的 ProcessControl 观察；观察 capability 不应让已死亡进程继续占用运行资源。

## 内核短路径

进程构造 syscall 必须有明确上界：单次映射页数、单次写入字节、启动 Handle 数与普通 payload 长度都有限，launcher 以循环完成大映像。内核不在一个 syscall 中遍历 archive、解析 ELF、解析对象图或执行不受限路径策略。

## 外部参照的边界

Zircon 把 bootdata 作为显式 VMO 交给普通 userboot，由 userboot 解析 BOOTFS 和后续 ELF；seL4 把 BootInfo 与 root capabilities 交给 root task，由用户态构造后续 VSpace/TCB/CNode。eRhino 采用相同的责任边界，但不因此照搬 VMO、untyped 或某个具体启动协议；本系统的独立理由是让内核只保留不可替代的 bootstrap 机制，并让后续装载遵守同一 capability 模型。
