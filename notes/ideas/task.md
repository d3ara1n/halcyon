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

进程与线程遵循同一出生与离场形状：Create 备齐资源并保持挂起，Start 只发布可运行责任，离场后持续提供各自的终局事实。进程的事实是首因及 Drain 后的 CLOSED，线程的事实是成员摘除与结果义务归零后的 DONE；Control 只持观察/管理 authority，不是核心资源或出生责任的 owner。两者的持久事实不依赖创建者继续调用。

进程生命周期为：

```text
Building -> Running -> Terminating -> Dead
```

Building 不是一笔必须整体回滚的大事务，而是若干各自原子的资源附入：内存绑定、Map/Write、capability Grant、线程 Attach 以及未来其它资源绑定分别完成，成功项立即属于目标；后续失败不把已经交付的 capability 或资源倒流回组装者。`ProcessBindMemory` 是唯一页资源绑定操作，消费 Pool Handle 并把 AddressSpace 从 Unbound 转为 Bound；绑定事务语义由[内存模型](mm.md)唯一拥有。未绑定进程仍是合法 Building 壳，可以接收不依赖地址空间的 grants，也可以被放弃或终止。

ProcessStart 是唯一首次发布 runnable 的提交点，也是 Building 组装截止点。执行需求在第一次线程创建前由独立 Building 绑定固定，线程创建据此取得全寿命调度准入；Start 不再接收执行描述、不选择调度域、不分配交接或调度结构，只确认内存已绑定、至少一条挂起线程已创建、Job 启动门开放、除 Start 自身外没有 Building 操作在途，然后关闭组装权并摘取预留发布链，发布全部已准备的挂起线程。Start 可以因并发终止、Job seal 或在途组装被拒绝，但不因资源不足失败。BindMemory、BindExecution、Map、Write、Grant 与 Building ThreadCreate 共用一次 Building operation lease 和同一计数；操作只能在精确 Building 状态登记，登记即冻结该操作的提交资格：登记先于终止截止时，操作获准完成且终止路径等待并接管其成功结果；Start 因其它操作仍在途而不能抢先发布。Start 或终止先线性化时，后续组装登记拒绝，因此不会出现 Running 后才提交的 Map、Grant、ThreadCreate 或 Bind。

Terminating 阶段禁止新线程和新等待，撤销 Ready、取消 Waiting，并等待各 hart 上的 Running 线程离场。随后管理者以有界操作先收束 HandleTable，再收束地址空间；Unbound 地址空间直接完成该阶段，Bound 地址空间只有在事务、lease、backing 与页表全部经有界 drain 释放后才完成。全部资源收束后进程进入 Dead，ProcessControl 的 CLOSED 才发布，因此等待 CLOSED 是资源收束完成屏障，不只是终止请求已受理。

Completed（末线程离场）、ProcessTerminate、ProcessAbort、fault、ProcessKill 与 Building abandonment 在各自适用的 Building 或 Running 状态竞争进入 Terminating 的唯一带原因线性化点；syscall 按各自 authority 仍分开，用户态 JobKill 最终只对成员执行 ProcessKill。首个完成转换者冻结结构化终局原因，后续事件只能协助收束，不得覆盖已经可观察的终态事实。ProcessKill 返回 Accepted 或 AlreadyTerminating，不表示 teardown 已完成；自杀式调用不返回用户态。未来 ThreadKill 使用同样的两值返回形状，不因此提前引入调用号。

### 终局原因

进程是资源体，不是交付一个整数结果的作业；线程是执行体，离场即完成。系统不设进程或线程退出码。进程终局只记录内核能证明的原因：

| 原因 | 触发 | 附带事实 |
|---|---|---|
| `Completed` | 所有线程离场，进程达成完成 | 无 |
| `Terminated` | 进程主动 `ProcessTerminate()`，未达成完成即结束 | 无 |
| `Aborted` | 所选 runtime 判定程序无法继续（panic、OOM、入口返回错误）并调用 `ProcessAbort()` | 无 |
| `Killed` | 经 ProcessControl authority 终止；JobKill 最终也使用此机制 | 实际内核入口类别，不记录调用者提供的数值 |
| `Faulted` | 内核检测到不可恢复用户 fault | tid、稳定 fault 类型、地址 |
| `Abandoned` | Building 期未 Start 即放弃 | 无 |

程序只能选择 Completed（让线程全部离场）、Terminated 或 Aborted；Killed/Faulted/Abandoned 是内核事实，用户不能伪造。失败细节、配置错误或业务状态走日志或与监督者的 IPC 协议，不塞入终局字段。监督者依据原因决定重启、隔离或上报。

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

内核无「主线程」角色。线程统一经 **ThreadCreate → 启动** 两段出生：ThreadCreate 创建完整但挂起的线程（UserContext、全寿命调度准入、成员登记和 ThreadControl），失败零副作用；挂起线程不运行、不入 Ready。Building 期由 ProcessControl authority 在目标进程创建，Control 原子安装到目标 HandleTable，并只能由 ProcessStart 批量发布；Running 期由进程对 self 创建，Control 交给调用者，由 ThreadStart 单独发布。两种 authority 与发布点不同，线程对象、准入和离场协议相同，出生信息只是启动交接需要，不赋予数字身份政策含义。

**进程的生 = Start 发布（至少一个挂起线程已创建），进程的完成 = 末线程离场**。末线程离场使进程以 Completed 进入 Terminating；Dead/CLOSED 仍以资源 Drain 完成为准。无线程的进程停留在 Building，从未活过；挂起未启动的线程被终止或放弃时同样经离场协议发布 DONE。

关闭最后一份 ThreadControl capability 引用（包括运输中、已提交未交付的引用）若线程仍为 Created，就与 ThreadStart/进程终止在成员锁内争取 Created→Exiting 的唯一离场权；关闭非末份只消散授权，不提前放弃。只剩 WAIT 权时也等待最后一份引用消散，观察者仍可等待 DONE。未启动线程与已启动线程共用 ThreadDeparture；迟到的离场请求观察已有提交，不重复发布。ThreadStart 要求所属进程自己的 MANAGE Control，WAIT 只观察。

线程任意时刻恰处于一个稳定所有权容器：

```text
某调度类 Ready 队列 | 某 hart current | 某 WaitContext
```

启动发布与终止离场可以有短暂过渡阶段，但不能同时属于两个稳定容器。线程离场后从成员关系中移除，不保留“Dead thread”容器。

线程首次可运行之前，必须取得覆盖整个可调度寿命的目标类存储准入。该责任与执行容器一起在 Ready、Running、Waiting 间移动；线程暂时不在 Ready 队列不释放未来再入队的容量。新的线程出生必须同时计入已经准入和仍在准备的全部线程，不能借用等待者或执行者未来要回来的位置。只有执行容器最终离场、线程不可能再入队时才归还准入。准入不是 CPU 配额，也不重复记录线程状态；它只保证一旦承诺线程可运行，后续唤醒和轮转不再因队列存储分配失败。

线程只进入其执行需求已经绑定的兼容调度域，调度类在域内表达选择策略；硬件能力、域划分与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一拥有。线程对 CPU 的消耗从预约对象扣减：预约决定配额（能否跑），调度类决定次序（先后），两者作用于不同谓词，配额过滤在 pick 边界进行；预约对象、Job 归属与调度域 eligibility 是正交面，预约对象桥接 capability 世界与调度器世界。

## 组装双通道

线程、内存、句柄与其它资源各有外部组装通道和内部运行通道。外部通道由唯一 ProcessBuilder 在 Building 期执行一次性 Bind、执行需求绑定、Map/Write、Grant 与 ThreadCreate；内部通道由 Running 进程自己的 syscall 执行 Map/Unmap、ThreadCreate/ThreadStart 与 Transit。两条内存通道只区分 authority 与可用阶段，共用[内存模型](mm.md)定义的 Bound AddressSpace、backing 和事务；连续堆顶不构成第三套内核映射机制。

自然组装顺序是：

```text
ProcessCreate
  -> ProcessBindMemory
  -> ProcessMap / ProcessWrite
  -> ProcessGrant
  -> ProcessBindExecution
  -> ThreadCreate(Building)
  -> ProcessStart
```

ProcessGrant 可以在内存绑定前独立完成，因为 HandleTable 属于空壳；但 Map、Write、ThreadCreate 与 Start 都要求 Bound AddressSpace，ThreadCreate 还要求执行需求已绑定。执行需求绑定一次且不可改变，不由首个线程隐式决定。未来 KernelMemoryBudget、CPU 预约或设备资源各自使用独立绑定/Grant，不引入万能资源包或重新扩大 ProcessCreate 事务。Start 只发布已经满足 readiness 的资源环境；组装者在 Start 之后不再持 Builder authority。

创建子进程不隐式继承父进程权限。launcher 在 Building 期通过 ProcessGrant 明确选择 capability 与目标 rights，每项目标 rights 只能缩小；Grant 一旦成功就是独立完成的所有权转移，不因后续 Bind、Attach 或 Start 失败而回到组装者。opaque payload 同样由组装者在 Start 前写入目标地址空间，Start 只发布已组装完成的资源环境。`parent_pid` 仅记录创建关系，与 capability 派生无关。

## 多线程终止边界

多线程进程必须维护成员关系与 active-hart 集合。进程回收要先阻止任何线程再次返回用户态，再取消 Ready/Waiting、离场挂起线程，并请求各 hart 上的 Running 线程在安全边界自行离场。active 身份既服务终止屏障，也为地址空间 translation epoch 提供执行点快照：hart 只有在完成已承诺的地址翻译同步并切回 kernel satp 后才能清除身份，快照后进入者必须在返回用户态前观察最新 epoch。最后一个终止确认到达且在途地址空间事务完成后，管理者才能分批回收页表和数据帧。运行期 ThreadCreate/ThreadStart 复用同一成员关系、active-hart barrier 与 WaitContext cancellation；线程消散可以放弃回复，但其 DONE 不能越过仍挂接结果记录的 committed System Call。DONE 且该挂接已解除后，用户态才可按[内存模型](mm.md)解除或复用其栈。

## 用户态线程资源收束

内核只创建执行现场与可等待的 ThreadControl，不分配或接管用户栈，也不记录线程结果。用户态按寿命把线程责任拆成三个 owner，不以一个 record 承载不同寿命：

| owner | 内容 | 寿命 | 提供者 |
|---|---|---|---|
| `ThreadRecord`（Registry 槽） | 栈 region、环境残余状态、Control 引用、cleanup 进度 | 登记至 DONE 后机械 cleanup 完成 | libthread `ThreadRegistry` |
| 共享 ThreadControl | 内核 entry 与持久 DONE | 最后一个引用释放时 Close | libkernel 类型，Arc 共享 |
| `ResultCell<T>` | 一次性结果与领取/放弃仲裁 | 结果被领取或析构，最后一个引用释放内存 | libthread |

`ThreadRegistry` 的机制（固定容量槽、唯一 cleanup claim、单步 `reap_step`、seal）全部属于 libthread，只对环境类型泛型，不含任务或结果类型。所选 runtime 建立并持有一个进程级实例、提供容量与环境实现；不存在跨库登记端口或第二份槽规则。公开 `Thread` 持有共享 ThreadControl，DONE 是持久事实，观察永不失效；结果领取权 `JoinHandle<T>` 持有 `ResultCell<T>`。record 退休不影响观察与结果，未领取结果也不占 Registry 槽。

ThreadControl 的 DONE 只表示成员关系已经摘除，且所有仍引用该线程结果记录的 committed System Call 已完成。可能 park 的 Map 必须登记线程级结果义务；执行容器可以在终止时放弃回复并消散，但该义务归零前不能摘除成员、发布 DONE 或允许 cleanup 解除栈。进程级 mandatory operation 仍独立保护 AddressSpace 的最终 Drain，两层计数分别回答“线程结果记录可否接管”和“进程资源可否收束”，不得合并。

### 线程环境

`ThreadEnvironment` 是显式生命周期接口：创建侧准备可发送的每线程状态，子侧进入/离开，DONE 后 cleanup 侧退休残余状态。retire 必须有界且不停驻，因为它在 cleanup step 中执行；enter/leave 在线程自身执行，可以执行任意语言初始化。实现必须在自身状态记录部分成功，线程库不猜测外部资源。当前只承诺 abort 型 Rust 环境，不建设 TLS、unwind 或 detach。

### 创建、登记与启动

```text
prepare: Registry 槽准入 → 环境准备 → 带 guard 的栈 → ResultCell/packet
  → ThreadCreate（挂起，取得 Control）→ 登记 ThreadRecord
  → ThreadStart → Thread + JoinHandle<T>
```

线程在 Create 与 Start 之间不运行，因此 record、packet 与 ResultCell 在 Start 之前已经稳定，不存在“子线程早于 syscall 返回运行”的登记竞争。Start 前的每一步失败按反向顺序返还；ThreadCreate 已成功而放弃启动时，关闭最后 Control 引用由内核离场并发布 DONE，record 随后按普通 cleanup 收束，closure 在放弃者线程上 Drop。ThreadStart 的拒绝返还 PreparedThread。已成功的内核操作若写回输出用户地址失败，统一以发起 tid 和失败 VA 冻结 `Faulted(UserAccess)`，不能报告可重试成功；提交前事务仍回滚其未公开资源。进入 syscall 时不得持有子线程退出所需的用户锁。

Building 期创建的线程由构建者供栈（Building Map）；其 child-local Control 是 StartupBlock grants/Handle 数组的一项。版本化 `EntryThreadDescriptor` 只记录该项的零基索引与栈 reservation、usable 范围，接收时由同一列表解析，不复制句柄值。子进程 runtime 入口用一次性原子采用该描述，在自身 Registry 中按普通 region owner 登记入口栈 record 后才进入应用 wrapper；运行期栈也由该 owner/cleanup 模型收束，不按来源分支。Rust profile 只有一个 `_start` 采用者，第二次进入 `_start` 是进程完整性失败；其它 Building 线程须使用别的入口契约。采用前进程终止时这些都是进程资源，由 ProcessDrain 收束，不跨地址空间运输 Rust 对象。

### 结果发布

`ResultCell` 以单个原子状态 `Empty → Published | ReceiverGone` 仲裁 T 的唯一移动或析构，且 T 总在用户线程上 Drop：

| 路径 | 动作 |
|---|---|
| 子线程正常返回 | 写入 T → 环境 leave → CAS Empty→Published（release）；失败说明领取方已放弃，子线程自行 Drop T |
| 子线程 enter 失败 | 不运行 F，在子线程 Drop F，发布 EnterFailed |
| JoinHandle 领取 | 观察 DONE → acquire 读取：Published 取出 T；Empty 返回 Incomplete，不读取未初始化内存 |
| JoinHandle Drop | CAS Empty→ReceiverGone；失败说明已发布，在本线程取出并 Drop T |

leave 失败保留 T 并单独报告环境错误。Incomplete 只出现于绕过 wrapper 的 raw ThreadExit 或完整性故障；libkernel 的 `ThreadExit()` 是 unsafe 原语，libthread 不提供可绕过 wrapper 的安全 exit。闭包及返回值需要 Send 与静态拥有的捕获，不提供 scoped thread。

### 离场与 cleanup

`ThreadExit()` 不带参数：线程离场即完成。DONE 后 `reap_step` 只执行机械动作：环境 retire、释放 Control 引用、Unmap 栈、归还槽；Busy 返回 Wait，永久错误保留 record 并进入进程完整性故障，不降级为计数或 panic。任意用户代码（F、T、enter/leave）从不在 cleanup 中执行。

系统没有专职 driver 线程，cleanup 为协作式推进，契约只承诺容量不被已死线程占用，不承诺及时回收：

- prepare 取槽前先 reap 已 DONE 的记录，槽耗尽只说明确有等量线程仍活着或 Busy；
- 线程 wrapper 在 ThreadExit 前对其它已 DONE 记录做有界次数 reap；
- join 观察到 DONE 时推进一次 reap；
- 已运行 libexecution 的程序可把 DONE 登记为等待来源加速回收，不创建隐藏线程。

积压以 Registry 容量为上界；进程终局时剩余 record、自身栈和最后离场线程的资源由 ProcessDrain 收束，这是正常终局而非泄漏。期限只限制本次观察，不杀子线程、不撤销已提交 MemoryChange。Thread 与 JoinHandle 的 Drop 不等待、不触碰 record。

### 入口线程与程序终局

runtime 入口线程与其它线程对等：`main` 返回 `Ok` 只结束该线程，进程在全部线程离场时 Completed；返回错误、panic 或 OOM 由 runtime 记日志后 `ProcessAbort()`。程序需要在未完成时结束整体，显式调用 `ProcessTerminate()`。施工计划见 [用户内存 owner 计划](../../plans/todo-2026-09-14-user-memory-owner-lifecycle.md)。
