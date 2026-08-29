# 任务模型

任务是对“受管理作业”的统称，内核中由 Job、进程和线程三种不同职责承载，不以一个万能 Task 结构混合资源、执行与政策。

## Job：创建域与资源预算

Job 是进程创建与资源预算的层级容器。root Job 由内核在初始 launch 中交给 init；持有相应 capability 的服务可以创建子 Job 并在该域内创建进程。

Job 表达故障收束和管理域，不是用户身份或进程权限等级。设备、目录和服务访问仍由各自 capability 授权。

资源预算以对象界定而非记账。库存资源（内存）以持有表达：对象拥有帧、映射借出视图、分配即派生切片。流量资源（CPU）以预约表达：budget/period 的预约对象是 capability，无预约或预算耗尽的线程不被调度，refill 状态即调度器真值。域的上限由域内可达的持有物与预约决定，内核不维护动态计数；上限的分配与超额政策归用户态资源服务。线程洪水由结构消化：spawn 不增 CPU 配额（共享预约），且消耗域 backing。配额过滤的边界在调度循环的 pick，eligibility 的判定在入队侧域路由，两者是线程的不同正交面（见「线程」节），接入预约不触碰域路由结构。

Job 的生命周期是 Open、Sealed、Dead。Open Job 即使没有成员也保持可用，允许管理者按政策重新创建服务；JobSeal 是幂等的封口操作，使本 Job 及后代不再接受新 Job、Building process 或 ProcessStart。Job 层级深度最多为 32，创建和启动沿有界祖先链检查 effective seal，因而根封口无需递归遍历即可立即覆盖后代。封口只封创建、不向下传播终止；完成只看本 Job 自身的 sealed 与成员收束，递归封口与递归终止都是用户态政策（逐层 JobSeal 组合）。Sealed Job 在直接进程全部 Dead、child Jobs 全部 Dead 后进入 Dead，并以 JobControl 的 CLOSED 电平发布完成。关闭 JobControl 只消散 authority，不封口或终止成员。

内核维护 Job 层级、直接成员关系、封口状态和完成计数，并向持 MANAGE authority 的管理者提供有界分页枚举与派生 child JobControl、ProcessControl 的机制；枚举与派生以单调不复用的 Pid/JobId 寻址——ID 不构成全局操作入口，唯一操作角色是在已持 JobControl 的直接成员域内作派生选择子，authority 完全来自 capability；内核不递归遍历无界子树执行 JobKill。JobKill 是 init、pm 等用户态管理者组合 JobSeal、分页枚举、ProcessKill、资源收束和 CLOSED 等待形成的政策操作。树遍历、失败恢复、批量顺序和重启决策归用户态，单个状态转换和 capability 派生仍由内核强制。

## 进程：独立资源环境

进程持有：

- 用户地址空间与内存布局；
- HandleTable；
- 线程成员关系；
- Job 归属与资源归属（预算以持有与预约表达，不记账）；
- PID、`parent_pid` 与退出信息：以诊断为主；PID 另在 JobControl 枚举域内充当派生选择子（见「Job」节），不构成全局操作入口。

进程不以“驱动级”“服务级”等 ambient 权限授权 syscall。ProcessCreate 同时产生 affine ProcessBuilder 与稳定的 ProcessControl：前者只授权构造，后者从 Building 起授权查询、终止、等待和受保护资源收束；观察者可持去除 MANAGE 的 READ/WAIT control。创建关系本身不产生管理权。

进程生命周期应统一为：

```text
Building -> Running -> Terminating -> Dead
```

Building 阶段经独立操作完成 ELF/地址空间、用户态启动信息、capability 安装与线程附入；ProcessStart 是唯一首次发布 runnable 的提交点，也是执行需求对兼容调度域绑定的冻结点。Terminating 阶段禁止新线程和新等待，撤销 Ready、取消 Waiting，并等待各 hart 上的 Running 线程离场。随后管理者以有界操作先收束 HandleTable，再收束地址空间；只有全部资源完成收束，进程才进入 Dead。ProcessControl 的 CLOSED 只在该完成点发布并对外表示 Dead，因而等待 CLOSED 是资源收束完成屏障，而不只是终止请求已受理。

Exit、fault、ProcessKill 与 Building abandonment 在各自适用的 Building 或 Running 状态竞争进入 Terminating 的唯一线性化点；用户态 JobKill 最终也只对成员执行同一个 ProcessKill 原语。首个完成转换者冻结终因与退出码，后续事件只能协助收束，不得覆盖已经可观察的终态事实。Terminating 已可通过固定宽快照观察稳定的 state、reason 与完整 i64 code，但这不表示资源已经回收；非终止状态以显式 reason 表示 code 无语义，不借用退出码哨兵或保留编码。

ProcessKill 是持 MANAGE authority 对 Building 或 Running process 发出的异步、幂等终止请求。成功表示请求已被接受，或目标此前已经越过不可逆终止边界；不表示本次调用首次触发转换，也不表示 teardown 已完成。自杀式调用不返回用户态。调用者若需分辨目标阶段，应使用 ProcessQuery；等待 Dead 以具 WAIT authority 的 ProcessControl 观察 CLOSED，随后查询稳定终态。关闭 ProcessControl 仍只消散 authority，不隐式终止进程。

运行资源与终态观察壳分离。所属 Job 的直接成员表强持尚未 Dead 的 Process core，Process 只以非拥有关系回指 Job；ProcessControl shell 同样只以非拥有关系定位 core。线程全部离场后 Process 仍由 Job 保活到 Drain 完成，Dead 发布时才从 Job 成员表移除。此后地址空间和 HandleTable 不受观察者持有的 control 影响，终态 shell 由最后一个 control Handle 保活并持续提供 PID、状态与终因快照。

线程与 active hart 全部离场后，ProcessControl 发布持续可见的 REAPABLE 电平。持 MANAGE authority 的管理服务以调用者给定、内核封顶的预算反复执行受保护收束；每批只处理有界数量的 Handle 和页表资源，进度保存在目标进程而非某个调用者中，同 authority 可在管理者重启后接管。最后一批完成时清除 REAPABLE 并发布 Dead/CLOSED，不把无界析构塞入单次内核路径。

## 线程：执行单元

线程属于且仅属于一个进程，持有独立 UserContext 与 FP 状态；栈是进程资源，由组装者或进程自己供给。只有线程参与调度；进程提供资源环境与执行需求，不是调度队列成员。

内核无「主线程」角色，只有「首线程」——组装时附入的第一条线程（tid 1），携带出生信息只因信息必须交给第一个执行者；是否初始化、是否 spawn 工作线程、初始化完是否退出，全是用户政策。线程出生不携带资源、死亡不留遗产、生存只耗 CPU——执行需求是进程级属性，栈与句柄是进程资源。

**进程的生 = Start 入册（首线程已在表），进程的死 = 末线程离场**——线程 presence 从两端定义进程生命周期。无线程的进程停留在 Building，从未活过。

线程任意时刻恰处于一个稳定所有权容器：

```text
某调度类 Ready 队列 | 某 hart current | 某 WaitContext
```

启动发布与终止离场可以有短暂过渡阶段，但不能同时属于两个稳定容器。线程离场后从成员关系中移除，不保留“Dead thread”容器。

线程只进入其执行需求已经绑定的兼容调度域，调度类在域内表达选择策略；硬件能力、域划分与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一拥有。线程对 CPU 的消耗从预约对象扣减：预约决定配额（能否跑），调度类决定次序（先后），两者作用于不同谓词，配额过滤在 pick 边界进行；预约、Job 归属与调度域 eligibility 是线程的正交面，预约对象桥接 capability 世界与调度器世界，不重复记账。

## 组装双通道

线程与内存、句柄同为进程资源，各有内外两个操作通道：外部通道（Building 期，组装者持 builder：Map/Write/Grant/Attach）与内部通道（Running 期，进程自己的 syscall：Extend/Spawn/Transit）。Building/Running 的本质即外部写通道的开与关；Start 是「资源齐备、入册调度」的纯状态转换，其唯一前置是活体门——至少已附入一条线程。组装不是协作：组装者在 Start 之后不再观察进程内部状态。

## 权利派生

创建子进程不隐式继承父进程权限。launcher 在 Building 期通过 ProcessGrant 明确选择 capability 与目标 rights，每项目标 rights 只能缩小；Grant 一旦成功就是独立完成的所有权转移，不因后续 Attach 或 Start 失败而回到组装者。opaque payload 同样由组装者在 Start 前写入目标地址空间，Start 只发布已组装完成的资源环境。`parent_pid` 仅记录创建关系，与 capability 派生无关。

## 多线程终止边界

多线程进程必须维护成员关系与 active-hart 集合。进程回收要先阻止任何线程再次返回用户态，再取消 Ready/Waiting，并请求各 hart 上的 Running 线程在安全边界自行离场。active-hart 确认必须晚于目标 hart 切回 kernel satp 和本地全量 SFENCE.VMA；最后一个确认到达后，管理者才能开始分批回收页表和数据帧。任何 ThreadSpawn 接入都必须复用同一成员关系、active-hart barrier 与 WaitContext cancellation，不得另造单线程专用 kill/reap 路径。
