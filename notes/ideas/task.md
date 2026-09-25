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

Building 不是一笔必须整体回滚的大事务，而是若干各自原子的资源附入：内存绑定、Map/Write、capability Grant、线程 Attach 以及未来其它资源绑定分别完成，成功项立即属于目标；后续失败不把已经交付的 capability 或资源倒流回组装者。`ProcessBindMemory` 是唯一页资源绑定操作，消费 Pool Handle 并把 AddressSpace 从 Unbound 转为 Bound；绑定事务语义由[内存模型](mm.md)唯一拥有。未绑定进程仍是合法 Building 壳，可以接收不依赖地址空间的 grants，也可以被放弃或终止。

ProcessStart 是唯一首次发布 runnable 的提交点，也是 Building 组装截止和执行需求对兼容调度域绑定的冻结点。Start 必须同时确认内存已绑定、至少一条线程已附入、所需执行资源已满足、Job 启动门开放，并且除 Start 自身外没有 Building 操作在途。Building 操作只能在精确 Building 状态登记，登记即冻结该操作的提交资格：登记先于终止截止时，操作获准完成且终止路径等待并接管其成功结果；Start 因其它操作仍在途而不能抢先发布。Start 或终止先线性化时，后续组装登记拒绝，因此不会出现 Running 后才提交的 Map、Grant、Attach 或 Bind。

Terminating 阶段禁止新线程和新等待，撤销 Ready、取消 Waiting，并等待各 hart 上的 Running 线程离场。随后管理者以有界操作先收束 HandleTable，再收束地址空间；Unbound 地址空间直接完成该阶段，Bound 地址空间只有在事务、lease、backing 与页表全部经有界 drain 释放后才完成。全部资源收束后进程进入 Dead，ProcessControl 的 CLOSED 才发布，因此等待 CLOSED 是资源收束完成屏障，不只是终止请求已受理。

Exit、fault、ProcessKill 与 Building abandonment 在各自适用的 Building 或 Running 状态竞争进入 Terminating 的唯一线性化点；用户态 JobKill 最终也只对成员执行同一个 ProcessKill 原语。首个完成转换者冻结终因与退出码，后续事件只能协助收束，不得覆盖已经可观察的终态事实。ProcessKill 成功表示请求已被接受或目标此前已经越过不可逆终止边界，不表示 teardown 已完成；自杀式调用不返回用户态。

Running→Terminating 的准入截止与 AddressSpace Commit 共享 Process execution gate。Commit 按 AddressSpace lock 在外、execution gate 在内的顺序取得两者；终止只在 gate 内发布截止，释放后才接管 AddressSpace，不反向取锁。终止先线性化时，尚未 Commit 的 Running 内存事务必须回滚且结果 cookie 保持零；事务 Commit 先线性化时，终止路径只能接管并等待其 synchronize/retire，不能把已承诺结果重新解释为失败。Building Bind/Map/Write/Grant/Attach 使用登记获胜的 operation lease，Start 与终止负责关闭新登记并等待已有 lease；两类事务共享 gate，但不能混用 Commit 与登记两个胜负点。

运行资源与终态观察壳分离。所属 Job 的直接成员表强持尚未 Dead 的 Process core，Process 只以非拥有关系回指 Job；ProcessControl shell 同样只以非拥有关系定位 core。线程全部离场后 Process 仍由 Job 保活到 Drain 完成，Dead 发布时才从 Job 成员表移除。此后 AddressSpace 和 HandleTable 不受观察者持有的 control 影响，终态 shell 由最后一个 control Handle 保活并持续提供 PID、状态与终因快照。metadata admission 或 charge 必须覆盖 core、Builder 与 Control 各自的实际寿命，不能因 Dead 提前退款仍由观察壳占用的资源。

线程、active hart 与已提交的地址空间修改全部离开执行或完成不可逆撤销后，ProcessControl 发布持续可见的 REAPABLE 电平。持 MANAGE authority 的管理服务以调用者给定、内核封顶的预算反复执行受保护收束；每批只处理有界数量的 Handle 和页表资源，进度保存在目标进程而非某个调用者中，同 authority 可在管理者重启后接管。进程自身资源收束完成后，在 PublishDead 阶段清除 REAPABLE 并发布 Dead/CLOSED；Job 成员摘除、祖先传播等 Drain 终段责任可以跨预算继续，直到 ProcessDrain 返回 Complete，不把无界析构塞入单次内核路径。

## 回收责任、资源资助与执行

回收驱动、资源额度和监督 authority 是三个独立维度。Job 表达谁可以管理成员；MemoryPool 与 KernelMemoryBudget 表达资源从哪里支付、释放后退到哪里；执行上下文承担实际推进工作。某个进程执行了回收，不因此取得被释放资源，也不改变资源的原始资助来源。单次推进的工作上限只限制一轮内核执行量，不等于页额度、元数据额度或 CPU 预约。

ProcessDrain 可以由受信任的进程管理服务专职承担，由根监督者负责其故障接管；普通应用无需承担整个进程退出后的回收驱动。职责范围由 ProcessControl 的 MANAGE、Job 的管理/创建 authority 及其实际交付决定，内核不识别名为 pm 的特权进程。只读终态观察不授予回收权；若把创建或管理 authority 委派给其他主体，必须同时定义其监督责任，不能仍声称只有固定服务能够 Drain。

在受信任管理者模型中，回收是服务职责，不需要以独立资源预算作为执行职责的激励前提。预算仍负责限制分配和隔离资源压力，两者不互相替代。根监督者由内核启动不等于已经存在根进程自动回收机制；根故障、恢复和最终停止需要明确契约。

用户态管理策略也可以把一个监督域与一组资源 capability 共同委派，令资助者具备收回自身额度所需的管理权。预算对象不必成为 Job 的字段，也不按创建者 PID 推导退款。管理者可以复用清理后仍存活的子池；只有子池本身及其全部资助关系消散后，完整委派额度才归还父池。转授到其他进程仍然存活的对象继续占用原来源，结束一个进程不等于收回它曾经资助的一切资源。

资助和接管必须超越某一管理线程的寿命：管理者退出后，上级应能恢复监督 authority 并接管持久回收状态；单有预算对象保活并不产生新的执行者。可信根监督者是这条链的运行前提，其故障由系统恢复或停止政策处理。资源压力可以促使资助者回收，但不能作为安全证明；放弃自己的有限额度不得损坏内核状态或侵占其他域已经保证的容量。

有界回收需要可暂停的工作状态，不必然要求用户逐批 syscall，也不必然要求全部自动推进。若公开 Drain，应限制为明确资源域的回收驱动：内核拥有游标、每批合法性、依赖和退款，用户决定何时投入后续工作；已经接受的操作尾段与无法交付给用户的回滚由内核闭合。两种驱动可以共用同一机制，但不能把某个对象的内部阶段任意公开成新的维护协议。

可暂停请求的等待身份与业务执行代次必须共同捕获。线程一旦公开为 Waiting，终止方就可能立即取得取消权，因此请求的启动与取消不能依赖“安装者随后一定先执行”的时序；二者须按同一代次仲裁唯一消费权，迟到的旧轮回调不得触及已经复用的执行器。等待结果已经 Done、业务执行器已经静止、预付槽已经归还是不同事实；上下文复用必须合取后两者，不能仅凭用户回复完成推断内部责任已经退休。

安全点公平性以同一执行组内“入口时 runnable 的类别至少获得一次执行机会”为单位，而不是宣称所有内核工作共享一个全局额度。阻塞登记本身可以消耗执行预算，但不得伪造公开业务进度；持续 backlog 由后续安全点继续服务。不同 payload、容量来源、取消边界和 owner 的债务保留独立账本，只共享可证明的预算机械协议。

## 线程：执行单元

线程属于且仅属于一个进程，持有独立 UserContext 与 FP 状态；栈是进程资源，由组装者或进程自己供给。只有线程参与调度；进程提供资源环境与执行需求，不是调度队列成员。

内核无「主线程」角色，只有「首线程」——组装时附入的第一条线程（tid 1），携带出生信息只因信息必须交给第一个执行者；是否初始化、是否 spawn 工作线程、初始化完是否退出，全是用户政策。线程出生不携带资源、死亡不留遗产、生存只耗 CPU——执行需求是进程级属性，栈与句柄是进程资源。

**进程的生 = Start 入册（首线程已在表），进程的死 = 末线程离场**——线程 presence 从两端定义进程生命周期。无线程的进程停留在 Building，从未活过。

线程任意时刻恰处于一个稳定所有权容器：

```text
某调度类 Ready 队列 | 某 hart current | 某 WaitContext
```

启动发布与终止离场可以有短暂过渡阶段，但不能同时属于两个稳定容器。线程离场后从成员关系中移除，不保留“Dead thread”容器。

线程首次可运行之前，必须取得覆盖整个可调度寿命的目标类存储准入。该责任与执行容器一起在 Ready、Running、Waiting 间移动；线程暂时不在 Ready 队列不释放未来再入队的容量。新的线程出生必须同时计入已经准入和仍在准备的全部线程，不能借用等待者或执行者未来要回来的位置。只有执行容器最终离场、线程不可能再入队时才归还准入。准入不是 CPU 配额，也不重复记录线程状态；它只保证一旦承诺线程可运行，后续唤醒和轮转不再因队列存储分配失败。

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

内核只创建执行现场与可等待的 ThreadControl，不分配或接管用户栈。libthread 以普通匿名 mapping 组成双 guard 的 UserStack，并把栈、入口状态、结果记录与 ThreadControl 组合为 affine JoinHandle。join 是 DONE、统一等待与内核资源关闭之上的用户态协议，不另设 syscall；基础映射和等待由 libkernel 提供。成功 join 同时意味着执行已经结束和包装器拥有的资源已收束，任务返回值与清理失败分别报告。默认 Drop 保留等待线程结束的结构化语义，不能将尚在运行的线程静默 detach；若收束失败则必须移交预先准入的退休 owner，不把诊断计数当资源承接。此承接机制属于 [用户内存 owner 计划](../../plans/todo-2026-09-14-user-memory-owner-lifecycle.md) 的必要前置，当前实现尚未具备。

ThreadControl 的 DONE 只表示成员关系已经摘除，且所有仍引用该线程结果记录的 committed System Call 已完成。可能 park 的 Map 必须登记线程级结果义务；执行容器可以在终止时放弃回复并消散，但该义务归零前不能摘除成员、发布 DONE 或允许 joiner 解除栈。进程级 mandatory operation 仍独立保护 AddressSpace 的最终 Drain，两层计数分别回答“线程结果记录可否接管”和“进程资源可否收束”，不得合并。

### 线程环境与结果交接

libthread 拥有本进程线程的准备、发布与回收契约。`ThreadEnvironment`、`ThreadFactory`、`PreparedThread` 是拟定的接口角色：调用者构造工厂时显式提供运行环境，工厂为每次启动准备独立的环境 owner。所选语言 runtime 可以实现该环境；需要线程的通用库只消费 libthread 的契约，不导入具体 runtime。外部组装新进程首线程仍属于 libprocess 的 Building 通道，不由此接口代替。

环境是显式的生命周期接口：父侧准备可发送的每线程状态，子侧进入/离开，DONE 后可在收束者侧退休。接口只理解这些阶段，不含具体 runtime 类型、默认全局 hook 或服务政策。语言环境实现必须在自己的状态中记录部分成功，不允许线程库猜测哪些外部资源已建立。当前只承诺 abort 型 Rust 环境，不因接口预留接点建设 TLS、unwind 或 detach。

### 准备与发布

工厂使用调用方选择的栈策略和已配置的收束域；不存在脱离程序环境的隐式默认工厂。准备之前先准入一个完整责任槽，供正常、失败、Drop 和取消路径共同使用。槽的容量和计费由显式资源配置决定，不在失败路径临时分配，也不以任意新常数限制线程总量。

准备顺序是：退役责任准入 → 入口/结果 packet 与环境的可失败准备 → 带 guard 的栈 → PreparedThread。每一步失败完整返还尚未执行的任务、环境状态与已取得资源；调用者可以取消或处理清理，线程尚未进入 Ready。便利 spawn 可以将 prepare/start 组合，但不能吞掉清理失败 owner。

PreparedThread 的 start 只提交一次 ThreadSpawn，失败返回 PreparedThread 而不是销毁任务，成功产生 JoinHandle。子线程可能在父侧收到 syscall 返回前运行，所以所有 packet 地址、环境、结果槽与回收责任必须在提交前稳定；不能在 syscall 成功之后才分配 Join 所需记录。不得在进入 syscall 时持有子线程退出必须取得的用户锁。

内核发布成功但输出不符合固定结果契约属于完整性失败，不能当作未启动并释放栈。正常错误遵守未发布返还；完整性失败只能进入进程故障策略，不能伪造可重试 PreparedThread。

### 子侧环境与结果发布

```text
Prepared → Published → Entering → Running → Leaving → ResultPublished
                                                      ↓
                                内核 ThreadExit → DONE → Collecting → Collected
```

父侧只操作它自己的 Join/回收控制，不与子侧同时取得 packet 的 `&mut`。DONE 前，子侧是其任务与环境状态的唯一执行者；DONE 后，执行访问才转交给收束者。共享发布字使用原子操作，任务/结果存储的已初始化状态独立维护，不能用一枚含义混杂的 done 位描述全部事实。

子侧进入失败时不运行任务，保留未执行 closure 和部分环境状态，发布 EnterFailed。正常任务返回先将 T 放入明确已初始化的存储，再完成环境 leave；leave 失败也必须保留这个 T，并报告单独的环境结束错误。最终 publication 用 release 发布完整 outcome，之后只执行不再访问业务数据的 ThreadExit。收束者在 DONE 后 acquire 观察发布状态；结果发布早于 DONE 不允许提前回收栈。

线程未经过终结包装器而结束时，DONE 不证明任务返回或环境已经正常离开。未发布结果归为 Incomplete，不读取未初始化的 T。closure 是否已经移入调用栈、结果是否实际已写入、环境的哪些动作已完成分别依据状态恢复；不能对已移走 closure 执行第二次 Drop，也不能从抛弃的任意调用栈恢复 Rust 局部变量。安全环境契约不得承诺任意指令位置终止后的通用析构。底层直接 ThreadExit 是需要调用者自行承担这些义务的 raw/unsafe 逃生口，不作为普通 Rust closure 的安全提前返回方式。进程终止仍是进程级最终资源边界。

闭包及返回值需要 Send 与静态拥有的捕获，不提供借用调用者栈的 scoped thread。任务、结果和环境状态只能被移出/丢弃一次；具体环境若违反其 unsafe 初始化/终结契约，按进程完整性失败处理，不继续交付可能已损坏的语言对象。

### Join、期限和清理

JoinHandle 的等待以 ThreadControl DONE 为事实。期限只限制本次等待/重试政策，不杀子线程、不撤销已提交 MemoryChange，也不把未完成包装成取消成功。等待期限到达或观察错误时，完整 Join owner 返还调用者，可再次 join 或移交明确的监督者。

DONE 后才解码并移出可交付的 ThreadOutcome，然后收束环境、packet、栈映射和 Control。至少保留足够的离场证明到栈已不再被执行；环境需要最后依赖的资源必须比它晚释放。已取得的返回值不因 Unmap 失败被丢弃：返回结果与 ThreadCleanup 分别移交，失败后重新推进不得重复环境析构、Control close 或 outcome 领取。wait、Control close、固定输出不一致和 Unmap 永久错误必须进入明确的 Join/cleanup/supervisor owner，不能由 Drop 的 panic 或 abandoned 计数吞掉。

同步 Join 与外部执行器使用同一收束状态机。一次推进执行一个有界动作或返回观察/重试请求，不在 libthread 内启动任务循环；libexecution 可以在使用侧持有该状态机并登记观察。同步驱动使用相同结果，不另写第二套 Busy 重试规则。状态机不依赖 libexecution。

尚未开始的 PreparedThread 取消只回收未发布资源；Join 的取消只放弃当前等待并返还 owner；Drop 默认仍等待 DONE。调用方无法在 Drop 接收错误，因此 factory 必须事先取得资源退休的稳定承接：接收剩余 ThreadCleanup 是无分配、不可拒绝的责任转交，之后由既有用户态执行/监督配置按预算推进。移交前不能丢失观察、环境状态、映射或计费。

承接配置必须复用已有通用执行、等待、准入和停止契约。线程 cleanup 的跨线程交付由 libthread 拥有固定容量 `RetirementDomain`：每个 PreparedThread 先取得不可拒绝的 permit，cleanup record 以 release/acquire 发布到稳定槽位，既有用户态执行器以任务形式逐步推进，直到结果、环境、Control、映射和付款责任全部结算。domain seal 只禁止新 permit，已有 permit 仍可发布；receiver task 只有在 `sealed && outstanding == 0` 后才能注销来源并关闭 Runtime。libthread 不依赖具体 executor、FAL 或 runtime，也不把通知当作债务 owner。首个实现的 JoinHandle 保持线程局部，跨线程只转交已经稳定且满足 SAFETY 的 ThreadCleanup record；不为自 join 新增 current Tid ABI。没有这些前提就不能开放宣称可安全 Drop 的线程工厂；最终进程回收或缺陷计数不能代替运行期精确收束。当前实现与复用边界见 [运行体实现](../impls/runtime.md#资源退休消费者与复用边界)，具体施工前置与代码差距由[用户内存 owner 计划](../../plans/todo-2026-09-14-user-memory-owner-lifecycle.md)记录。
