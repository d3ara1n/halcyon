# 任务模型

任务是对“受管理作业”的统称，内核中由 Job、进程和线程三种不同职责承载，不以一个万能 Task 结构混合资源、执行与政策。

## Job：创建与收束域

Job 是进程创建、成员管理与故障收束的层级容器。root Job 由内核在初始 launch 中交给 init；持有相应 capability 的服务可以创建子 Job 并在该域内创建进程。

Job 表达故障收束和管理域，不是用户身份或进程权限等级。设备、目录和服务访问仍由各自 capability 授权。

资源预算不属于 Job。page-backed storage 由显式 MemoryPool capability 持有并派生，内核 metadata 由正交的 KernelMemoryBudget 支付，流量资源（CPU）由 budget/period 的预约对象表达，MMIO、IRQ 与 DMA 各由自身资源 capability 授权。Job 不绑定默认资源包、不汇总成员持有物，也不在 capability 跨 Job 转移时改记费用归属；用户态资源管理服务可以按政策把 Job、MemoryPool、KernelMemoryBudget、预约与设备资源组合交付，但该组合不形成第二份内核预算真值。线程洪水仍由结构消化：spawn 不增加 CPU 配额，成员表与每进程线程上限只界定内核工作量。

Job 的生命周期是 Open、Sealed、Dead。Open Job 即使没有成员也保持可用，允许管理者按政策重新创建服务；JobSeal 是幂等的封口操作，使本 Job 及后代不再接受新 Job、Building process 或 ProcessStart。Job 层级深度最多为 32，创建和启动沿有界祖先链检查 effective seal，因而根封口无需递归遍历即可立即覆盖后代。封口只封创建、不向下传播终止；完成只看本 Job 自身的 sealed 与成员收束，递归封口与递归终止都是用户态政策（逐层 JobSeal 组合）。Sealed Job 在直接进程全部 Dead、child Jobs 全部 Dead 后进入 Dead，并以 JobControl 的 CLOSED 电平发布完成。关闭 JobControl 只消散 authority，不封口或终止成员。

内核维护 Job 层级、直接成员关系、封口状态和完成计数，并向持 MANAGE authority 的管理者提供有界分页枚举与派生 child JobControl、ProcessControl 的机制；枚举与派生以单调不复用的 Pid/JobId 寻址——ID 不构成全局操作入口，唯一操作角色是在已持 JobControl 的直接成员域内作派生选择子，authority 完全来自 capability；内核不递归遍历无界子树执行 JobKill。JobKill 是 init、pm 等用户态管理者组合 JobSeal、分页枚举、ProcessKill、资源收束和 CLOSED 等待形成的政策操作。树遍历、失败恢复、批量顺序和重启决策归用户态，单个状态转换和 capability 派生仍由内核强制。

## 进程：可组装的资源环境

进程由稳定管理壳与可附入资源共同构成。ProcessCreate 只在 Job 中创建不可运行的 Building 空壳，并同时产生 affine ProcessBuilder 与稳定 ProcessControl；空壳持有 PID、创建关系、生命周期、HandleTable、线程成员槽和 Unbound AddressSpace 身份，但不隐式取得页额度、页表根、用户 mapping、线程、CPU 预约或设备资源。创建壳本身必须取得覆盖 core、Builder、Control 与固定运行期 metadata 上限的 sponsor；长期由 KernelMemoryBudget 支付，在其公开前由可信创建路径附入不可转授、不可扩容的内部 MetadataSponsor。

进程的资源环境可以包含：

- 经 Building authority 一次性附入的页池与内核 metadata 预算绑定；
- 从 Unbound 转为 Bound 后的用户地址空间与内存布局；
- HandleTable 与显式授予的 capabilities；
- 线程成员关系与进程级执行需求；
- Job 归属、CPU 预约及由其它 capability 取得的资源；
- PID、`parent_pid` 与退出信息：以诊断为主；PID 另在 JobControl 枚举域内充当派生选择子，不构成全局操作入口。

进程不以“驱动级”“服务级”等 ambient 权限授权 syscall。ProcessBuilder 只授权 Building 组装，关闭即放弃构造并请求目标终止；ProcessControl 从空壳创建起授权查询、终止、等待和受保护资源收束，观察者可持去除 MANAGE 的 READ/WAIT control。创建关系本身不产生管理权。

进程生命周期统一为：

```text
Building -> Running -> Terminating -> Dead
```

Building 不是一笔必须整体回滚的大事务，而是若干各自原子的资源附入：内存绑定、Map/Write、capability Grant、线程 Attach 以及未来其它资源绑定分别完成，成功项立即属于目标；后续失败不把已经交付的 capability 或资源倒流回组装者。`ProcessBindMemory` 是唯一页资源绑定操作，消费 Pool Handle 并把 AddressSpace 从 Unbound 转为 Bound；绑定成功后不可替换、运输或按映射重新选择费用来源。未绑定进程仍是合法 Building 壳，可以接收不依赖地址空间的 grants，也可以被放弃或终止。

ProcessStart 是唯一首次发布 runnable 的提交点，也是 Building 组装截止和执行需求对兼容调度域绑定的冻结点。Start 必须同时确认内存已绑定、至少一条线程已附入、所需执行资源已满足、Job 启动门开放，并且除 Start 自身外没有 Building 操作在途。Building 操作只能在精确 Building 状态登记，登记即冻结该操作的提交资格：登记先于终止截止时，操作获准完成且终止路径等待并接管其成功结果；Start 因其它操作仍在途而不能抢先发布。Start 或终止先线性化时，后续组装登记拒绝，因此不会出现 Running 后才提交的 Map、Grant、Attach 或 Bind。

Terminating 阶段禁止新线程和新等待，撤销 Ready、取消 Waiting，并等待各 hart 上的 Running 线程离场。随后管理者以有界操作先收束 HandleTable，再收束地址空间；Unbound 地址空间直接完成该阶段，Bound 地址空间只有在事务、lease、backing 与页表全部经有界 drain 释放后才完成。全部资源收束后进程进入 Dead，ProcessControl 的 CLOSED 才发布，因此等待 CLOSED 是资源收束完成屏障，不只是终止请求已受理。

Exit、fault、ProcessKill 与 Building abandonment 在各自适用的 Building 或 Running 状态竞争进入 Terminating 的唯一线性化点；用户态 JobKill 最终也只对成员执行同一个 ProcessKill 原语。首个完成转换者冻结终因与退出码，后续事件只能协助收束，不得覆盖已经可观察的终态事实。ProcessKill 成功表示请求已被接受或目标此前已经越过不可逆终止边界，不表示 teardown 已完成；自杀式调用不返回用户态。

Running→Terminating 的准入截止与 AddressSpace Commit 共享 Process execution gate。Commit 按 AddressSpace lock 在外、execution gate 在内的顺序取得两者；终止只在 gate 内发布截止，释放后才接管 AddressSpace，不反向取锁。终止先线性化时，尚未 Commit 的 Running 内存事务必须回滚且结果 cookie 保持零；事务 Commit 先线性化时，终止路径只能接管并等待其 synchronize/retire，不能把已承诺结果重新解释为失败。Building Bind/Map/Write/Grant/Attach 使用登记获胜的 operation lease，Start 与终止负责关闭新登记并等待已有 lease；两类事务共享 gate，但不能混用 Commit 与登记两个胜负点。

运行资源与终态观察壳分离。所属 Job 的直接成员表强持尚未 Dead 的 Process core，Process 只以非拥有关系回指 Job；ProcessControl shell 同样只以非拥有关系定位 core。线程全部离场后 Process 仍由 Job 保活到 Drain 完成，Dead 发布时才从 Job 成员表移除。此后 AddressSpace 和 HandleTable 不受观察者持有的 control 影响，终态 shell 由最后一个 control Handle 保活并持续提供 PID、状态与终因快照。metadata admission 或 charge 必须覆盖 core、Builder 与 Control 各自的实际寿命，不能因 Dead 提前退款仍由观察壳占用的资源。

线程、active hart 与已提交的地址空间修改全部离开执行或完成不可逆撤销后，ProcessControl 发布持续可见的 REAPABLE 电平。持 MANAGE authority 的管理服务以调用者给定、内核封顶的预算反复执行受保护收束；每批只处理有界数量的 Handle 和页表资源，进度保存在目标进程而非某个调用者中，同 authority 可在管理者重启后接管。最后一批完成时清除 REAPABLE 并发布 Dead/CLOSED，不把无界析构塞入单次内核路径。

## 线程：执行单元

线程属于且仅属于一个进程，持有独立 UserContext 与 FP 状态；栈是进程资源，由组装者或进程自己供给。只有线程参与调度；进程提供资源环境与执行需求，不是调度队列成员。

内核无「主线程」角色，只有「首线程」——组装时附入的第一条线程（tid 1），携带出生信息只因信息必须交给第一个执行者；是否初始化、是否 spawn 工作线程、初始化完是否退出，全是用户政策。线程出生不携带资源、死亡不留遗产、生存只耗 CPU——执行需求是进程级属性，栈与句柄是进程资源。

**进程的生 = Start 入册（首线程已在表），进程的死 = 末线程离场**——线程 presence 从两端定义进程生命周期。无线程的进程停留在 Building，从未活过。

线程任意时刻恰处于一个稳定所有权容器：

```text
某调度类 Ready 队列 | 某 hart current | 某 WaitContext
```

启动发布与终止离场可以有短暂过渡阶段，但不能同时属于两个稳定容器。线程离场后从成员关系中移除，不保留“Dead thread”容器。

线程只进入其执行需求已经绑定的兼容调度域，调度类在域内表达选择策略；硬件能力、域划分与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一拥有。线程对 CPU 的消耗从预约对象扣减：预约决定配额（能否跑），调度类决定次序（先后），两者作用于不同谓词，配额过滤在 pick 边界进行；预约对象、Job 归属与调度域 eligibility 是正交面，预约对象桥接 capability 世界与调度器世界。

## 组装双通道

线程、内存、句柄与其它资源各有外部组装通道和内部运行通道。外部通道由唯一 ProcessBuilder 在 Building 期执行一次性 Bind、Map/Write、Grant 与 Attach；内部通道由 Running 进程自己的 syscall 执行 Map/Unmap、Spawn 与 Transit。两条内存通道只区分 authority 与可用阶段，共用[内存模型](mm.md)定义的 Bound AddressSpace、backing 和事务；连续堆顶不构成第三套内核映射机制。

自然组装顺序是：

```text
ProcessCreate
  -> ProcessBindMemory
  -> ProcessMap / ProcessWrite
  -> ProcessGrant
  -> ProcessAttach
  -> ProcessStart
```

ProcessGrant 可以在内存绑定前独立完成，因为 HandleTable 属于空壳；但 Map、Write、线程 Attach 与 Start 都要求 Bound AddressSpace。未来 KernelMemoryBudget、CPU 预约或设备资源各自使用独立绑定/Grant，不引入万能资源包或重新扩大 ProcessCreate 事务。Start 只发布已经满足 readiness 的资源环境；组装者在 Start 之后不再持 Builder authority。

创建子进程不隐式继承父进程权限。launcher 在 Building 期通过 ProcessGrant 明确选择 capability 与目标 rights，每项目标 rights 只能缩小；Grant 一旦成功就是独立完成的所有权转移，不因后续 Bind、Attach 或 Start 失败而回到组装者。opaque payload 同样由组装者在 Start 前写入目标地址空间，Start 只发布已组装完成的资源环境。`parent_pid` 仅记录创建关系，与 capability 派生无关。

## 多线程终止边界

多线程进程必须维护成员关系与 active-hart 集合。进程回收要先阻止任何线程再次返回用户态，再取消 Ready/Waiting，并请求各 hart 上的 Running 线程在安全边界自行离场。active 身份既服务终止屏障，也为地址空间 translation epoch 提供执行点快照：hart 只有在完成已承诺的地址翻译同步并切回 kernel satp 后才能清除身份，快照后进入者必须在返回用户态前观察最新 epoch。最后一个终止确认到达且在途地址空间事务完成后，管理者才能分批回收页表和数据帧。ThreadSpawn 必须复用同一成员关系、active-hart barrier 与 WaitContext cancellation；线程消散可以放弃回复，但其 departed/join 完成不能越过仍挂接结果记录的 committed System Call。join 确认线程离场且该挂接已解除后，用户态才可接管结果并按[内存模型](mm.md)解除或复用其栈。

## 用户态线程资源收束

内核只创建执行现场与可等待的 ThreadControl，不分配或接管用户栈。rinlib 以普通匿名 mapping 组成双 guard 的 UserStack，并把 stack mapping、用户结果记录与 ThreadControl 组合为 affine JoinHandle。join 是 ThreadControl DONE、统一等待与壳关闭之上的用户态资源策略，不另设系统调用。首版采用结构化收束：显式 join 返回结果；未显式 join 时 handle 析构也等待内核 DONE 后解除完整 stack reservation 并丢弃结果，不以静默泄漏或内核代拆映射冒充 detach。未来若需要 detach，应由正式用户态 reaper 接管这组资源，而不是削弱 join 完成边界。

ThreadControl 的 DONE 只表示成员关系已经摘除，且所有仍引用该线程结果记录的 committed System Call 已完成。可能 park 的 Map 必须登记线程级结果义务；执行容器可以在终止时放弃回复并消散，但该义务归零前不能摘除成员、发布 DONE 或允许 joiner 解除栈。进程级 mandatory operation 仍独立保护 AddressSpace 的最终 Drain，两层计数分别回答“线程结果记录可否接管”和“进程资源可否收束”，不得合并。
