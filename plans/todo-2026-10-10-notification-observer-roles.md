# Notification 观察角色与统一等待契约

状态：**三角色、共享 ABI、typed owner 与实际消费者迁移已落地；check/clippy/host 通过，core/stress 已取得实际运行证据，N6 注入窗口与专项结构检查收口见末节。** 用户已选择 provider 持有消费责任、向客户端交付只读就绪观察能力的 A 方向，并授权内核重构。本案属于 [OAL/fs/service Watch](todo-2026-10-07-fal-service-redesign.md) 的交付闭包，独立拥有通用内核前置；不以当前代码/开发证据宣告主计划交付。

## 当前接力与范围

- 设计 owner：GrandOwl；协调及主计划 owner：MellowPond；实施 owner：RubyRaven。
- 盘点基线：`refactor/system-redesign` / `4523bbfe65287f6b84ab73d9f034dae1a526d38d`，加当前未提交的 OAL/fs/service 迁移。其他会话的工作树改动保留。本篇现状表来自源码，不能把整项目矫正区的现状当作目标正确性的依据。
- 已确认方向：Owner 消费/关闭，Signaler 发送并直接观察接收端关闭，唯一 NotificationObserver 只读观察 READABLE/CLOSED。用户确认的是 A 的责任分配；以下派生 API、命名与类型收敛是据此形成、已由 MellowPond 全文复核通过的详细设计。
- 本案拥有：Notification 三角色、相关公开观察角色命名、派生授权、共享 ABI、libkernel 类型与真实调用点、Wait/WaitSet 接入、付款和存储保活、通用验证。
- 主计划拥有：fs/service Watch 请求/回复槽位及运输权限、sequence/pending、Query/Ack/Cancel、控制 Lifetime、终态与业务退款、正式 Watch 消费者及组合验收。本案只声明接入条件，不复制这些协议。
- 非目标：共享通知接收端协议、badged signaler、逐事件队列、泛型公共 Observer、任意对象的统一观察派生 syscall、WaitMany/嵌套 WaitSet、全对象业务寿命重写、按观察者转移原对象付款、不可信域 metadata 硬隔离。
- 当前接力：通用三角色、ABI/typed与IPC/Startup/退出消费者及专项结构检查已收口；fs/service同步与异步Watch、流及Copy已由主计划接通，P4完整聚合已通过，N7跨平台/release证据见末节。N6精确失败/费用注入仍按唯一条目保留owner、触发与完成门，不把普通运行升格为N6注入通过；本篇不另排已完成施工，本轮实施结束，代码提交由主计划固定；本篇只继续承接N6，未闭合前不归档。

## 设计盘点基线

本表中的 rights 是对象上限，实际 capability 还须持相应权限。缩写：R=READ，W=WRITE，Q=WAIT，S=SIGNAL，M=MANAGE，D=DUPLICATE，T=TRANSIT，G=GRANT，C=CREATE，X=EXECUTE。`CLOSED` 强制终止等待与业务信号的区别见后文。

### 可等待对象及 Notification 相邻角色

| 当前公开 kind / 角色 | rights 上限；合法 signals | 创建或派生授权 | 来源、业务终态与最后能力 | 付款及观察保活 |
|---|---|---|---|---|
| MailboxReceiver | R/Q/M/G；READABLE、PEER_CLOSED、CLOSED | MailboxCreate，当前进程创建 | MailboxCore 的队列/WaitQueue；最后 Receiver 关闭队列。READABLE 表示可接收队头；PEER_CLOSED 表示当前可投递 sender 数为零，可因新 MintSender 消失 | core、固定环、Receiver 由创建进程 Pool 付款；等待 loan 保活 core，不计 Receiver authority |
| MailboxSender | W/Q/D/T/G；WRITABLE、CLOSED | Receiver[M] 的 MintSender，独立 sender 身份/badge，附带 Lifetime | 与 Receiver 共用 MailboxCore 来源；CLOSED 是接收队列关闭，不是该 sender 的 Lifetime 终结。最后能力归还 sender count 和 Lifetime authority | sender/Lifetime 由派生进程 Pool 付款；队列保留原 payer；等待 core 不新增发送 authority/use |
| MailboxOnceSender | W/Q/T/G；WRITABLE、CLOSED | Sender[D]，输出 rights 不超过源；附带 ReplyObservation | 队列来源；发送成功消费授权；放弃发布 Abandoned；结果由独立 ReplyObservation 承载 | Once、结果观察壳及来源由派生进程 Pool 付款；队列 loan 不保活 Pending 业务责任 |
| ReplyObservation | R/Q/D/T/G；DONE、PEER_CLOSED | MakeSendOnce 原子配对产生 | 独立 SignalSource；Sent→DONE，Abandoned→PEER_CLOSED，互斥且不变。关闭观察不取消 Once、不改变结果；来源通用 closed 不置位 | 壳和 SignalSource 由配对创建进程 Pool 付款；仅数值关联 Once，不反向强持发送业务 |
| Lifetime | Q/D/T/G；CLOSED | MintSender 原子配对产生；内部 Once 派生 authority，已接受 Delivery 持 use | Lifetime 的 OBJECT_WAIT 域；authority/use 双零才永久 CLOSED。观察 capability 的持有数不计入这两个计数 | 派生进程 Pool；观察只保活域的存储，不保活 sender、消息或业务 use |
| NotificationOwner | R/Q/M/G；READABLE、CLOSED | NotificationCreate 原子产生 Owner/Signaler | NotificationCore 的 OR pending/WaitQueue；Take 消费位；最后 Owner 关闭 core、清位并发布 CLOSED | core/两角色由创建进程 Pool 付款；core loan 不计 Owner authority |
| NotificationSignaler（当前不可等待） | S/D/T/G；无 | 与 Owner 配对创建 | OR 置位；最后 Signaler 无关闭 core 的 hook | 角色及 core 强引用只保活存储，不延长 Owner 业务寿命 |
| NotificationObservation | Q/D/T/G；仅 CLOSED | Owner[R] 或 Signaler[S] 的 NotificationObserve | 同一 NotificationCore；纯观察，不 Take、不关闭 core | 观察壳由派生进程 Pool 付款；core 保留创建 payer，观察强持 core |
| ProcessControl / ProcessShell | R/Q/M/D/T/G；DONE | ProcessCreate 的 Job[C]+Pool[C]；JobDerive 按领域权限派生 | Shell 的 SignalSource；Body/VM 退休屏障后 DONE，非来源 closed；关闭 Control 不杀进程 | 所选进程 Pool 支付 Shell/来源等；等待只保活观察来源，不能保活 Body 私有资源 |
| ThreadControl | Q/D/T/G；DONE | ThreadCreate 随线程产生，与 affine ThreadStartTicket 分开 | SignalSource；执行离场及相关访问/结果责任收束后 DONE；关闭观察壳不承担启动或停止线程 | 线程所属进程 Pool；Control 不强持执行 Thread，来源保留真实费用 |
| Job | C/M/R/Q/D/T/G；CLOSED | JobCreate 的 parent Job[C]+Pool[C]；JobDerive | SignalSource；封口后的域收束发布 CLOSED；丢弃控制能力不等于 Seal/关闭整个域 | 创建时指定 Pool；观察来源与 Job 业务所有权分开，源存储付费到释放 |
| MemoryObject | R/W/Q/M/D/T/G/X；EXECUTABLE | MemoryObjectCreate 使用调用进程 funding Pool，无输入 Pool capability；Seal 需 M | MemoryObjectCore/WaitQueue；完成封存后 EXECUTABLE。最后 capability 归还 backing owner，观察引用不代替 backing 使用授权 | 调用进程 Pool 支付 core、backing 及对象壳；source loan 保活 core 的观察存储，实际 backing 退休按内存契约 |
| TunnelEndpoint | R/W/Q/S/M；DATA、PEER_ATTACHED、PEER_CLOSED、CLOSED | TunnelCreate 要 Pool[C]；另一侧 Attach 消费 Invitation[M]，输出权限取邀请与 Endpoint 上限交集 | Endpoint 的来源受 Connection 同步域保护；最后 Endpoint 关闭本侧、通知对侧；两侧都关才交还 backing owner | Create 指定 Pool 支付 backing/Connection/首侧；Attach 调用进程 Pool 支付新 Endpoint/WaitQueue；来源强引用不等于 Endpoint authority，不反向保留 private retirement Work |
| WaitSet | R/Q/M/G；READABLE、CLOSED | WaitSetCreate；Register/Remove 要 M，Receive 要 R | 集合来源；候选队列决定 READABLE，Receive 逐源复查；最后 capability 关闭准入并自主分批拆成员 | 集合 Pool 支付 core/成员/索引，Receive 调用者 Pool 支付 scratch；成员 loan 保活来源至 detach；WaitSet 不得作另一集合的成员 |

源码入口：[mailbox](../os/kernel/src/task/mailbox.rs)、[lifetime](../os/kernel/src/task/lifetime.rs)、[notification](../os/kernel/src/task/notification.rs)、[shell](../os/kernel/src/task/process/shell.rs)、[thread](../os/kernel/src/task/thread.rs)、[job](../os/kernel/src/task/job.rs)、[memory_object](../os/kernel/src/task/memory_object.rs)、[tunnel](../os/kernel/src/task/tunnel.rs)、[wait_set](../os/kernel/src/task/wait_set.rs)。关键定位：Mailbox roles 474/538/604/698，Lifetime 191，Notification 202/221/233，ProcessShell 903，ThreadControl 61，Job 691，MemoryObject 268，Endpoint 202，WaitSet 424。行号是本次盘点位置，施工以符号为准。

全仓 `impl KernelObject` 盘点还覆盖 MemoryPool、ProcessBuilder、ThreadStartTicket、TunnelInvitation、Delivery、SystemReset：它们当前 `allowed_signals` 为 None，不纳入等待角色改造。Delivery 虽不可等待，却持 LifetimeUse；不能把不可等待误写为没有业务寿命责任。

`CLOSED` 表示来源不可逆关闭，WaitOne/WaitSet 在 `Snapshot.closed` 时强制附加该位，不要求 interest 含 CLOSED。`PEER_CLOSED` 仍是领域电平：Mailbox 的无 sender 可消失；ReplyObserver 的 Abandoned 不可变；Endpoint 对端关闭不可逆且可能与本端 DATA/CLOSED 共存。DONE/EXECUTABLE 也是各自业务屏障，不据名字改写成来源关闭。ProcessBuilder 等的内核内部完成来源不因此成为用户可等待对象。

### 已确认的实现缺口与资料偏差

1. 当前 Notification Owner 无 DUP/T，Signaler 无 Q，Observation 只能看 CLOSED，确实无法交付 Watch 要求的可运输 READABLE 观察角色。service provider 目前给 Owner 请求 DUP/T，真实建立失败。
2. `notification::observe` 没有现行业务调用者；现存调用链是 shared 调用号→syscall dispatch→内核实现及 libkernel raw/safe 封装。没有独立的 Owner→CLOSED-only 授权消费者需要保留。
3. 当前 HandleDescription 不含 signals 或 allowed_signals；`notes/impls/ipc.md` 对该字段的描述有偏差。本文不据文档假设存在权限查询能力。本案采用静态角色集合，不增加该字段；实施时修正实际说明。
4. `libprocess::context::adoptable_kind` 未包含当前 kind 20。Observer 保留 GRANT 能力就必须接通启动交付的收编，不能只验 IPC。
5. 主计划已承接 Watch Ack 的空 Take 幂等问题、永久 Signal 错误重试及最终维护唤醒；本案不另建一份修复清单或以通用角色完成关闭这些条目。

## 统一角色模型与收敛选择

### 状态、能力与等待登记分层

1. **状态/来源 owner** 持有业务事实，并在自己的同步域更新电平。NotificationCore、MailboxCore、Lifetime、Endpoint/Connection、SignalSource 是不同事实的宿主；不因统一等待而复制状态。
2. **consumer/producer/observer 是能力职责**，由公开 ObjectKind 加 rights 表达，不给 capability 表另加 role 字段。具有操作权的角色也可观察相关就绪或关闭，如 MailboxSender 的 WRITABLE/CLOSED。不要求每类对象人为产生三种壳。
3. **纯 observer** 没有推进/消费/关闭被观察业务的权限，也不取得业务 authority/use。它可拥有真实 kernel capability，持有计数只约束观察壳本身；观察能力的 Drop 与被观察业务的结束是两件事。
4. **Wait/WaitSet 登记** 是经过授权后持有的 source loan 和节点，不是新的业务 authority。登记时确定合法 interest；后续 Close 原 handle 不撤销已有登记，注销归等待 owner。
5. **业务完成与来源关闭** 不统一为“最后 Arc 消失”。Process/Thread DONE、Reply 的 Sent/Abandoned、Lifetime 的双零、Mailbox 当前无 sender、Notification Owner 关闭各有明确事实。通用等待只解释当前电平和 source 的 closed 标记。

本案涉及的 IPC 纯观察能力统一采用 `Observer` 后缀：公开 kind 13 `LifetimeObserver`、18 `ReplyObserver`、20 `NotificationObserver`。内部 Lifetime 仍表示 authority/use 计数域，LifetimeOwner/LifetimeUse 仍是内部业务凭证；它们不改叫 Observer。内部 `ObservationSource` 表示电平存储的接口，`WaitNode` 表示登记节点，二者也不是公共能力角色。本案不按后缀重命名其他领域的全部对象（例如线程领域的 ThreadControl）；角色职责的一致性不要求所有对象采用同一个命名模板。

不引入公共通用 Observer wrapper 或 `Observe(any_handle, mask)`：三种 observer 的派生授权、所观察事实、结果与金融保活关系不同；共同等待已经由 KernelObject/ObservationSource/WaitQueue 完成。新增一层擦除领域知识的壳会把授权映射移到公共层，不能提供额外正确性。

### Notification 目标三角色

| 目标角色 | rights 上限 | 合法 signals | 操作及最后能力语义 |
|---|---|---|---|
| NotificationOwner | R/Q/M/G | READABLE、CLOSED | R 执行 Take；Q 等待；M 铸造 Observer；最后业务 capability 关闭 core；仍不可 DUP/T |
| NotificationSignaler | S/Q/D/T/G | 仅 CLOSED | S 执行 OR Signal；Q 观察接收端关闭。持有、复制、等待或关闭 Signaler 都不维持/结束 Owner 的业务寿命 |
| NotificationObserver | Q/D/T/G | READABLE、CLOSED | 只能等待/复制/运输；不能 Signal、Take、铸造新角色或关闭 core；只强持观察存储 |

这是 A 的静态只读角色方案，不采用候选 C 的每实例可观察 mask。新角色复用原 kind 20；删除旧 CLOSED-only NotificationObservation，不与 NotificationWaiter 并列。Signaler 的 Q 是可选授予权限，不给已有 SIGNAL-only capability 暗加 WAIT。

Signaler 直接观察 CLOSED，与 MailboxSender 一一对应：发送角色强持目标 core；等待权限独立于发送权限；目标接收 owner 关闭后，两者观察到同一来源 CLOSED；等待引用不保活接收 owner。区别来自机制本身：Mailbox 有容量，故发送角色还有 WRITABLE；Notification OR 置位无队列容量等待，不虚构 WRITABLE。Mailbox sender 的自身 Lifetime 是另一条授权/use 责任链，不能挪用来代表 Notification 接收端关闭。

旧 Owner→CLOSED-only 派生明确删除。需要仅等待关闭时，在 Observer 上请求 CLOSED；这仍授予观察 READABLE 的能力，不冒充 CLOSED-only 授权。只持 Signaler 的人可用获授的 WAIT 观察 CLOSED，不能派生 Observer。不存在未登记的兼容出口；将来若出现“只交付关闭事实、不得泄露就绪”的独立需求，须重新论证授权接口，不能悄悄扩充这个 Observer。

### 身份与物理布局

NotificationCore 持唯一 owner/source ObjectHeader；Owner 的 `header()` 投影这个 header，与 MailboxReceiver/core 同构。Signaler、Observer 各持自身 ObjectHeader。三者 `related_object_id` 均指 core/Owner 的真实 KOID，Owner 自关联；core 不另成为第二个可授予对象。

Observer 只持 `Arc<NotificationCore>`，不持 `Arc<NotificationOwner>` 或任何 Owner capability。Signaler 同样只持 core。把现有 Owner header 移入 core，不新增影子 ID、反向 owner 引用或第二个 WaitQueue。related ID 仅用于可信描述和诊断，不能按 ID 打开对象或授予权限。

## ABI、派生与失败合同

### 共享与 syscall

| 项目 | 目标 |
|---|---|
| ObjectKind 13 | `Lifetime` 原位改名 `LifetimeObserver`；计数、rights、signals 不变 |
| ObjectKind 18 | `ReplyObservation` 原位改名 `ReplyObserver`；结果、rights、signals 不变 |
| ObjectKind 20 | `NotificationObservation` 原位改名 `NotificationObserver`；采用上述静态 READABLE/CLOSED 契约 |
| syscall 0x3d | `NotificationObserve` 改为 `NotificationMintObserver`；a0=Owner handle，a1=输出 rights，a2=输出 Handle 地址 |
| NotificationCreate/Signal/Take | 号及参数布局不变；Create 允许 Signaler 的新 WAIT 上限；Take 的空结果仍为 ObjectNotAvailable |
| WaitOne、WaitSet、HandleDescription | 数据布局不变；不增加信号 mask 查询字段、不增加观察专用等待调用 |

不分配新 kind/调用号再保留旧口，不留旧名字 alias。ABI 不冻结，shared、kernel、libkernel、启动收编、实际协议消费者必须同一迁移闭包更新。kind 20 的语义变化是显式协议升级，不能运行新旧 ABI 混合映像。

`NotificationMintObserver` 的输入必须是 Owner[M]。M 授权签发独立能力；Q 仅授权观察已有角色，R 仅授权消费 pending。单凭 READ/WAIT 不得铸造新 Observer，与 MailboxReceiver[M] 的 MintSender 授权分工一致。

输出 rights 必须含 WAIT，且是 WAIT|DUPLICATE|TRANSIT|GRANT 的子集；未知位、缺 WAIT、请求 READ/SIGNAL/MANAGE 等均返回 RightsDenied。有 MANAGE 但 kind 不符返回 WrongObjectType；缺 MANAGE 按共同表解析返回 RightsDenied。无效/过期 handle 使用共同解析错误；不为本接口发明同义错误。

### 接受、线性化与回滚

1. 在 handle 表内接受 Owner[M]，取得 Authorization 的 BodyLease、对象存储及 rights 快照；检查 kind 和输出 rights。在表锁外 clone core，不新增 Owner holders 或业务 use。
2. 派生进程 Pool 预付 Observer 的实际 Arc 布局；取得新 KOID、构造壳、mint 请求 rights、预留本进程 handle 表项。观察创建不改变 pending/closed。
3. 沿现有 `handle::install` 的“预约→锁外 uaccess 写回→必成 commit”发布。输出失败完整释放未公开 Observer、预约与费用；不关闭 Owner，不清 pending。进程已 Frozen 时沿安装/表关闭共同路径收束。
4. 与 Owner Close 的竞争：Close 先于接受则解析失败；接受之后 Owner 可以关闭。已接受的观察创建允许交付一个已 CLOSED 的 Observer；它不会重新打开业务。普通 Arc/BodyLease 不能被当作延期 Owner Close 的授权。
5. 缺额度返回 QuotaExceeded，分配失败返回 OutOfMemory，KOID 耗尽返回 ReachLimit。失败不消费输入 Owner；不以忽略错误或保留半安装壳完成调用。

一般 Duplicate 仍只能裁剪原 rights；去掉 WAIT 后的观察引用不能等待。强类型收编须拒绝缺 WAIT 的 Observer。mint 初始必须 WAIT，不意味着通用裁剪必须保留 WAIT。

Signal 和 Take 继续在同一个 NOTIFICATION 同步域线性化；Owner Close 在该域不可逆设置 closed、清 pending、提交 CLOSED。Take 先改位再向调用者复制输出的现有 ABI 语义不在本案改成事务或业务 Ack。

| 输入/事件 | 来源状态转换 | 责任 |
|---|---|---|
| Create 成功 | pending=0、closed=false | 两个角色共同安装；未发布失败沿原 owner 回滚 |
| Signal(nonzero) | open 时 pending OR bits；closed 时 ObjectClosed | Signaler[S]，不分配通知项；一次 update 提交后续扫描 |
| Take(nonzero mask) | open 时原子提取并清命中位；无命中 ObjectNotAvailable | Owner[R]；READABLE 随剩余位重新计算 |
| 最后 Owner capability 放弃 | open→closed，pending=0；不可逆 | Owner hook 提交来源关闭，已付款 core Work 接管扫描 |
| Observer/Signaler/登记释放 | 不改变 pending/closed | 各自只交还引用/登记及实际费用，不能触发 Owner hook |

## 与 Wait/WaitSet 的共同合同

`KernelObject::allowed_signals` 继续由角色给出静态集合，`wait_source` 返回同一 core 的 ObservationSource。WaitOne 和 WaitSetRegister 都先验证 handle 的 WAIT、非空合法 interest 及其属于 allowed_signals，再取得 source loan。Signaler 的 READABLE 请求在此被拒；Observer 的 READABLE/CLOSED 走普通路径，不能在 WaitSet 加 Notification kind 特判。

沿用[等待源分案的轮次5](todo-2026-10-02-wait-source.md#设计轮次-5轮次-4-定向修正用户裁决禁止嵌套-waitset)：来源只提示，节点 owner 自摘，扁平集合，当前电平复查，无 Rearm/事件结果队列。本案替代该计划末节关于“Signaler 不可等待、Observe 仅 CLOSED”的角色约定，不改其通用等待机制。

### Take、复查与不丢持续就绪

- pending 非零时 Owner/Observer 的 READABLE 成立；Take 只清 mask 覆盖的位，仍有其他位则 READABLE 保持，全部清空才消失。Observer 等待或 WaitSetReceive 不消费任何位。
- Signal 与 Take/Close 同 core 锁：Signal 先发生且随后被 Take 消费的短暂电平允许不被 Observer 捕获，这是既有纯电平契约；Take 之后的新 Signal 重新置位并由持久来源维护覆盖等待者。
- Register 的锁外 attach、集合发布复检及初始候选覆盖“先置位后登记/登记中置位”；Receive 查询期间到达的新 hint 不因一次不满足快照被清掉。仍满足的成员排队轮转，不满足的旧候选被移出。由此不会把持续 READABLE 遗失为永久停驻。
- READABLE 持续时反复 Receive 返回它是正确行为，不能承诺“无论业务怎么写都不自旋”。Watch 客户端须处理并 Ack；Ack 成功清空后旧候选重新复查并撤出。若同一订阅已有异步处理在途，客户端按既有 SourceRegistration 的 remove/注销回执路径暂时移除该来源，并由当前任务持有待处理责任；完成后重新登记，初始候选再次复查。不得丢掉待处理责任再等新边沿；不得用零 interest（非法）、消费 Observer 或另造 edge latch 避免忙循环。该调度策略由客户端组合层承担，libexecution 不认识 Watch。
- CLOSED 为持续终止事实，客户端结束后应移除成员并完成来源注销。一直保留 CLOSED 成员、无处理地重复 Receive，同样属于调用方循环错误。

来源更新在 NOTIFICATION(400) 锁下同步 pending/closed 与 WaitQueue；每次只提交维护责任。来源 Work 每步检查一个候选，锁内复制临时强目标，放锁后发送 hint；目标/来源状态锁不嵌套。WaitSet(550)/线程 members 锁只取身份与引用，放锁后查来源，再回目标锁提交。新角色不增加锁、跨 hart 特判或单次遍历全部 observers 的关闭路径。

## 寿命、付款及交付边界

| 资源或责任 | 付款/执行 owner | 释放或完成条件 |
|---|---|---|
| NotificationCore、Owner、Signaler | NotificationCreate 调用进程 Pool；core Work 用该 Pool 的维护账户 | 最后 Owner capability 先关闭业务；实际存储各自到最后真实引用消失才退款 |
| Observer 壳与新 handle 表项 | MintObserver 调用进程 Pool；消息接收表项由接收进程自身表的付款规则承担 | 壳自身最后引用释放；运输不转移 core/壳原 payer |
| WaitOne 节点 | 线程已有稳定节点及出生准入 | 正常/超时/Frozen/退出由等待 owner 唯一 detach，然后释放 source loan |
| WaitSet 成员、索引、source loan | 集合 Pool；Receive scratch 为本次调用者 Pool | Remove/集合 Close 的唯一 owner 完成 detach，实际分配释放后退款 |
| 最后 Owner 关闭后的通知维护 | core 出生的 WaitQueue/WorkRecord、原维护账户 | 已提交扫描由内核自主推进；无用户续调、无临时内核线程 |
| Watch 记录及终态通知 | 主计划的 provider 与可信入口账户 | 按主计划的控制寿命、Ack、终态及来源清理合同收束；不以 Observer handle 关闭代替 |

Observer/Signaler 的 capability 持有数不进入 Owner holders；它们的 WAIT source loan 也不进入任何业务 authority/use。即使所有观察壳都已关掉，已登记成员仍可持 core；即使 core 已 CLOSED，这些引用仍可保留观察存储及费用。不能宣称关闭通知或释放 Watch 记录就已退还所有内核 metadata。原 payer 的实际费用到最后引用/已提交维护释放才归还，跨域硬容量保证仍由[预算分案](todo-2026-09-14-kernel-memory-budget.md)拥有。

每次 mint 新增一个按实际布局计费的 Observer Arc 及既有 handle 表项/预约成本；不按当前 observer 数量分配旁置数组。core 只持一份 WaitQueue/WorkRecord，登记节点归等待方预付。新增角色不引入随观察人数增长的同步发布/关闭循环；既有表增长、分配和安装成本仍遵守其所属合同，不能将整次 syscall 未经测量地宣称为固定时延。

Owner 唯一消费角色不等于只有一个执行线程可以调用它。provider 若多线程操作同一 Owner，必须在自己的状态 owner 中串行化 sequence/Ack/Take；内核不知道 Watch 的 sequence，不能替业务拼接确认事务。

终态提示不等于逐事件送达：Signal 后立即关闭 Owner 会清 pending，观察者可能只看到 CLOSED。需要客户端读到业务终态的 provider 必须按主计划保留终态控制记录和通知责任至确认/明确关闭；进程故障只保证终止可观察，不保证最后业务原因仍可查询。Observer 最后副本消失不取消 Watch，控制 Lifetime 仍是独立责任。

## libkernel 类型与消费者迁移

### 公开安全层

`libkernel::ipc::notification` 拥有 `NotificationOwner`、`NotificationSignaler`、`NotificationObserver`，各独占一个 Capability；`create(owner_rights, signaler_rights)` 返回拥有两者的 NotificationPair。Owner 提供 `take(mask)`、`mint_observer(rights)`；Signaler 提供 `signal(bits)`；Observer 没有消费/设置方法。共同等待继续接受借用的 handle，不另造等待状态机。

`libkernel::ipc::message` 拥有 `LifetimeObserver`、`ReplyObserver`：`MintedSender.lifetime`、`ReplyPermit.observation` 改用相应类型。它们只有观察、显式能力转换及合法复制/关闭操作，不获得关闭业务或设置结果的方法。既有 READ 权上限是否有操作由原 Reply 契约决定，本案不借改名删除权限或改变结果。

所有这些类型沿同一个转换合同：

- 由成功内核工厂返回时直接构造，不再 Query；未知 IPC/启动能力在 `from_capability` 唯一边界 Query 一次，核 kind 和必需 rights，失败完整返回原 Capability 与错误。
- Observer/LifetimeObserver/ReplyObserver 的可用观察类型必须有 WAIT；NotificationOwner/Signaler 的具体操作仍按实际 rights 由内核校验，不能因类型正确就补权。返回 HandleDescription 供领域一次性校验额外的运输上限，不重复 Query。
- `into_capability` 消费 typed owner，供 Packet/启动授权等异质能力容器；`as_handle` 仅借用，不复制关闭责任。显式 `duplicate(rights)` 经真实 Duplicate 调用裁剪，返回可用 typed Observer 时仍须含 WAIT；若有意去掉 WAIT，转入通用 Capability 层表达不可等待引用。Observer 复制不能变 Owner，不实现会隐式复制 handle 的 Rust Clone。Drop/失败沿 Capability 的唯一关闭责任收束。
- 不提供公开安全的“已验证”构造器绕过 kind 检查。工厂内部构造限制在 crate；确有原始边界时使用已有 unsafe raw 调用并写明 ownership。
- `call::sys_notification_*` 保留 ABI raw 层，用于正式底层夹具及明确原始边界。安全 `ipc::notification` 只保留 typed 工厂/角色操作，删除旧裸 HandlePair 工厂形状及接受裸 Handle 的 signal/take/observe 函数；普通生产消费者整体迁移到 typed API，低层验收显式使用 raw 层。

### 实际迁移清单

下表按本轮 rg/源码结果登记；实施前再次枚举确认同期主计划新增消费者，不能只改直接出现旧 enum 名的文件。

| 位置 | 迁移责任 |
|---|---|
| `shared/erhino_shared/src/{object,call}.rs` | 三个公开 kind 改名，0x3d 改名/参数合同，合法 rights/signals 注释；编号与结构布局断言同步 |
| `os/kernel/src/{syscall.rs,task/notification.rs}` | MintObserver、Owner MANAGE、输出 WAIT/子集校验、三角色静态集合、共同来源与身份、实际付款/回滚 |
| `os/kernel/src/task/{lifetime,mailbox}.rs` | Lifetime 的公开 kind 改名；Reply 壳及 kind 改名，保留内部 Lifetime 与结果/计数语义 |
| `os/kernel/src/task/{object,wait,wait_set}.rs` | 核对统一授权/来源入口；仅在确有重复校验缺陷时共用修复，不加观察角色分流；保留扁平限制 |
| `user/libraries/libkernel/src/{call.rs,ipc/notification.rs,ipc/message.rs,ipc.rs}` | raw ABI、typed Notification/两个配对 Observer、公共出口；完整失败 owner |
| `user/libraries/libprocess/src/context.rs` | `adoptable_kind` 的 kind13/18 改名和 kind20 接入；不把本案扩大为所有未使用 kind 的收编重构 |
| `user/libraries/librpc/src/{exchange,caller,dispatcher}.rs` | Request/OperationState/Sent 对 ReplyObserver 的所有权接力；wait 引用不改变结果/取消语义 |
| `user/libraries/libexecution/src/wake.rs` | NotificationWake 持 typed Signaler；接纳未知 Capability 时只通过 typed 转换 Query 一次，并使用其返回描述验证 SIGNAL，不再重复 kind 查询。保留 bit 非零、关闭处理等执行领域政策，不把“正确 kind”误当作已获 SIGNAL |
| `user/libraries/libfs/src/grant.rs` | Prepared/已安装 grant 的 LifetimeObserver 类型及交接，清理回执先于关闭/退款 |
| `user/libraries/libservice/src/{root,provider}.rs` | LifetimeObserver 持有与来源；Watch 的 Notification typed 接入按主计划分工 |
| `user/services/srv_fs/src/server/{boot,lifecycle,family,services}.rs`、`server.rs` | 出生/退休通知 typed 持有、能力容器边界及实际 Lifetime 来源 |
| `user/services/srv_init/src/{main,supervisor,filesystem,public_ipc,time_checks,race}.rs` | 普通通知创建/消费与监督交付迁移；保留原始 ABI 夹具用途，改用明确 raw 层；补 NotificationObserver 的启动交付 |
| `user/services/srv_pm/src/main.rs`、`user/tests/{test_fal,test_target,test_hammer}/src/main.rs` | 正式通知消费者/验收目标迁移；测试端使用实际新角色，不伪造能力。test_fal 随主计划 P4 改名 test_fs，届时按新路径与新组件名接续，不保留旧名兼容消费者 |
| `user/libraries/{libfs,libservice}/src/{client,provider,watch}.rs` 的实际存在文件及主计划新建 Watch 文件 | provider 使用 Owner[M] 铸造真实 Observer；客户端唯一转换边界；不在本案重定义领域消息结构 |
| `notes/impls/{ipc,task,framework,runtime,fs,service}.md` 及实际引用篇 | 实施后按机制拥有篇更新当前面貌、命名和付款；修正 HandleDescription 字段误述；不提前回填目标行为 |
| 活跃等待源/对象链计划中的当前入口 | 链接本案接管的角色契约；保留历史轮次/归档当时名称，不批量重写历史证据 |

`libprocess::management/supervise/observation` 的 Observation 表示用户态等待请求/结果，不是 ReplyObservation capability，不做同词批量改名。`LifetimeOwner/LifetimeUse`、内部 `Lifetime` 同样保留。libexecution 的通用 SourceRegistration 不需认识 fs/service、Observer 子类或业务 Ack。

## 消费者与验证门

验证按[构建手册](BUILD-AND-TEST.md)执行；下表拥有验证合同，实际实施证据由末节工作记录拥有。优先扩展现有 srv_init/public_ipc、time_checks、race 和正式 srv_fs/test_fal 路线（test_fal 随主计划 P4 改名 test_fs 后按新名接续），不新增专用测试服务、临时消费者或模拟内核。

| 门 | 独立契约与要捕获的错误 | 验证边界 |
|---|---|---|
| N1 授权/静态身份 | Owner 无 M 不能签发；输出缺 Q/越权失败；非 Owner 不能签发；Observer 不能 Take/Signal/Mint；Signaler 可 Q(CLOSED)、不可 Q(READABLE)；Owner 仍不可 DUP/T | 真实 syscall 与 HandleQuery；负向验证后原 pending/Owner 可继续使用 |
| N2 电平/多观察者 | 多个 Observer 登记不消费；Owner Take 部分位保持 READABLE、全清后消失；再次 Signal 可再次观察；预先置位再注册仍可见；不把短暂电平当日志 | 现有 WaitOne/WaitSet 真实夹具，跨用户线程交错；复用纯 signal 主机测试提供底层证据 |
| N3 注册/公平/清理 | 查询时新 hint 不丢；持续就绪成员轮转；清空后旧候选消退；关闭 handle 不取消登记；Remove/集合 Close 唯一 detach | 现有持久 WaitSet 压力及进程退出路线；检查没有新增内核原地循环 |
| N4 业务与存储寿命 | Observer/Signaler/已登记 loan 存活仍允许最后 Owner 关闭；Observer 与 Signaler 都见 CLOSED；关闭所有 Observer 不关闭 Owner；Signal/Take 在 core 关闭后失败 | 正式 IPC 交付及 Owner 所在进程退出；不依赖创建进程继续 syscall 推进 |
| N5 运输/收编 | Duplicate 子集、IPC move、带 GRANT 的启动直接交付保持 kind/related ID/静态语义；恶意错误 kind 收编返还 owner；缺 WAIT 不形成可用 typed observer | Packet/ProcessGrant/StartupContext 的既有正式设施，特别覆盖 kind20 |
| N6 失败/费用 | observer mint 的 quota/分配/安装/写回失败不清 pending、不丢 Owner；源已关闭仍可完成已接受的观察派生；壳/source/登记实际释放前不提前退款 | 现有 Pool 额度/失败设施及源码责任链审计；不能在 init 自身制造输出 fault，需沿既有隔离子进程场景；无现成注入路径的窗口明确记录证据缺口 |
| N7 邻接语义 | Lifetime authority/use 双零、Reply Sent/Abandoned、Mailbox sender peer/queue 状态、Process/Thread DONE、Endpoint 与 MemoryObject 等待不因名称/typed 迁移改变 | 现有公共 IPC、监督、Tunnel/内存路线与类型检查；不为每个旧函数添加镜像单测 |
| N8 正式 Watch 接入 | fs/service 真正交付 Observer，Query/重读/Ack 后清就绪，旧 Ack 保留后续事件，终态/退出/控制寿命闭合 | 主计划拥有场景与日志；本案引用证据，不重复认领领域验收 |

开发阶段先编译受影响 shared/kernel/user 与既有 host 测试（host 显式 `aarch64-apple-darwin`）；编译暂时失败不能成为搭兼容 adapter 的理由。实现闭合后执行 `just check`、`just clippy`；按上述风险复用 `just virt`、`just virt-stress` 及必要路线，跨机制组合收口按构建手册执行 `just acceptance`。同一基线通过的检查不重复；与主计划共用组合运行时明确哪些日志覆盖本案。

持续就绪活性、并发安装和所有失败窗口不能仅由一次 QEMU 通过推定。无法通过现有设施运行的内部窗口记录精确 owner、审计证据和未覆盖部分；不得将“设计上应当成立”列为执行通过。

## 施工顺序、删除门与完成条件

1. **已完成设计复核与依赖对齐**：MellowPond 全文复核通过，主计划已链接本案并将 Watch 槽 1 定为 NotificationObserver；两篇各持唯一契约。
2. 同一责任链迁移 shared 名称/ABI、kernel 三角色/派生/身份、libkernel owned 类型及全部实际消费者。编辑可按依赖顺序，不能把中间编译通过当作阶段交付；不留旧 Observe、旧 Observer kind alias 或假 Waiter。
3. 将 NotificationObserver 接入已有 Wait/WaitSet、IPC、启动交付和 Watch；保留 Mailbox/Lifetime/Reply 的原业务语义，完成 owned 类型传递及失败清理。
4. 按 N1—N8 验证并执行[交付前结构检查](REVIEW.md)，更新当前 impls、本篇与主计划各自证据；主计划独有的 Ack/终态/维护问题在该计划关闭。
5. 核对资源/权限/正常失败/取消退出、旧路径删除、真实消费者及文档。通用前置可按自身 N1—N7 独立说明完成，但主计划仍以 N8 及完整 Watch 合同为门；不得把“内核有新角色”报告为系统 Watch 已完成。

删除门：代码中不再有旧 `NotificationObserve`、公开 `NotificationObservation`/`ReplyObservation`/`ObjectKind::Lifetime`、从 Signaler 派生关闭壳、Owner DUP/T 的 Watch 伪路径；内部 Lifetime 及历史档案不在删除范围。不存在用动态 signals mask、控制 sender 可写、定时轮询或回调消息来冒充只读就绪角色的替代出口。

## 设计移交时的证据与未完成事项

已完成：源码盘点、现有等待/付款合同核对、三角色及公共命名设计、ABI/typed 迁移和验证门起草；MellowPond 已全文复核通过并完成主计划链接与 Watch 槽位对齐。设计交 RubyRaven 实施。

文档检查：本篇 15 个本地链接及标题锚点核对通过，围栏与行尾空白检查通过；现状表已与独立只读盘点交叉核对，并亲读修正 MemoryObject/Tunnel 的付款入口。没有运行编译、host 测试或 QEMU。

设计移交时未完成：代码迁移、实现结构检查、所有 N1—N8 执行证据、impls 实现回填；当前状态从后续实施记录接手。现有 OAL/fs/service 未提交改动及 Watch 接线进度继续由主计划承载；原通知方案待裁决阻塞已解除，按本前置依赖恢复施工。没有本案新增的临时实现或未登记兼容结构。

本案不改变硬件/寄存器调用约定，使用现有 shared syscall 编码、uaccess 和等待基础合同。已阅读[外部契约索引](../references/CONTRACTS.md)；此前 QNX/seL4/Zircon 对照仅是设计参考，不充当本案已完成的固定版本外部契约取证，也不据此新增硬件假设。

### 实施工作记录：角色迁移与开发证据（2026-10-10，RubyRaven）

**基线与范围。** HEAD/分支仍为 4523bbf / refactor/system-redesign，前三批及第四批的 staged git mv/dirty 改动完整保留。本轮先完成主计划 R04 六项及 OOM 清理记录，再实施本案；没有 commit/stash/reset/push，严格 QEMU 脚本未改。实施前重新枚举的 inventory 为 artifacts/service-p3-batch4/notification-consumers-baseline.txt 与 observer-owner-fields-baseline.txt；raw/旧名/删除侧复核存 artifacts/notification-observer/。实际工具链 rustc 1.100.0-nightly c54751567b19c4ceb08b0412d83529c2568cba8b / host aarch64-apple-darwin，LLVM 23.1.0（toolchain.txt）。

**成组迁移。**

| 切面 | 当前源码与责任 |
|---|---|
| shared/kernel | 三个公开 kind 在 13/18/20 原位改名；0x3d 改 MintObserver，参数为 Owner/M、完整 u64 rights、输出指针。Owner header 移入 core，三角色 related_id 指同一 core 身份；Signaler 可获 WAIT，只看 CLOSED，Observer 看 READABLE/CLOSED。来源仍只有原 WaitQueue/Work/NOTIFICATION 锁，WaitOne/WaitSet 无新特判 |
| mint/失败 | Authorization 接受 MANAGE，核 Owner kind 及输出必需 WAIT/静态子集；表锁外 clone core，不加 Owner holders/use。调用进程 Pool 支付实际 Observer Arc，KOID/分配/额度失败沿局部 owner 退回；handle::install 预约、锁外输出、必成 commit，写回失败 rollback/release_all，不清原 pending、不复活已 CLOSED core |
| libkernel | 五个独立 named typed owner：NotificationOwner/Signaler/Observer、LifetimeObserver、ReplyObserver。private capability_role 宏生成静态 role owner 的相同采用/转换/close 责任，无公共泛型 Observer 类型；unknown Capability 的 from_capability Query 一次，失败 RoleAdoptFailure 返原 owner，Observer 必需 WAIT；from_minted 限 crate，工厂不重复 Query，无 Clone |
| ABI 夹具出口 | call 模块保持私有，按 libkernel 既有 root 导出惯例只公开四个 sys_notification_*，全部 unsafe fn，均写 # Safety 的借用、有效输出及成功后唯一关闭责任；安全 notification 模块只提供 typed 工厂/角色方法，没有裸 Handle signal/take/observe |
| 真实消费者 | RPC Request/OperationState::Sent/Dispatcher 持 ReplyObserver；fs Grant/服务 Root/Request 持 LifetimeObserver；NotificationWake 用 typed Signaler，采用描述同次检查 SIGNAL 与非零 bit；srv_fs Release/Retire 持 typed Owner；init RootSupervisor ledger 保存实际两个角色，生产驱动只借 ledger 中 typed owner；pm、独立消费者、target/hammer 采用真实通知角色 |
| IPC/启动/退出 | libprocess::context 接纳 kind20；现有 public_ipc 正式线程/Packet 用例覆盖 Observer 运输，现有 test_target 与自动退休用例覆盖带 GRANT 的 kind20 Startup 收编。退休源 Owner 直接授予子进程，父方只留 Observer/Signaler；真实 Kill/DONE 后两者看 CLOSED，Signaler Signal=ObjectClosed，不依赖子进程续调 |
| 文档/业务接缝 | ipc/framework/runtime 实现篇回填身份、静态集合、typed/付款及真实退休 owner；活跃 wait-source 当前入口链接本案，不批改历史轮次。service Watch provider 持 Owner[MANAGE] 铸造真实 Observer，客户端 typed 采用，领域控制运输与 N8 证据由主计划记录 |

**删除清单。** 本案没有删除整个新文件或引入兼容 adapter。删除旧 Observe 的 Owner[READ]/Signaler[SIGNAL] 两出口、旧 CLOSED-only 壳、旧自由函数 signal/take/observe 与裸 HandlePair 的安全工厂结果，全部实际消费者同步迁移。Lifetime/Reply 旧公开名删除，内部 Lifetime/LifetimeOwner/LifetimeUse 与 libprocess Observation 保持。删除侧精确账见 migration-numstat-final.txt（共享 ABI、kernel、libkernel 及此前未修改的 typed owner 消费文件）；主计划已 dirty 的 main/fs/service 及旧 FAL 删除统计不冒充本案新增删除量。

```sh
rg -n 'NotificationObserve\b|NotificationObservation\b|ReplyObservation\b|ObjectKind::Lifetime\b' shared os user
```

退出 1，deleted-names.txt 为 0 行；使用词边界避免将新 NotificationObserver 误匹配为旧调用名。raw-call-sites.txt 逐调用记录仅 libkernel 工厂/声明/导出及明确的 main/public_ipc/race ABI 夹具。对 libraries 排除 libkernel，再加 srv_fs/srv_pm/drivers/test_fal/test_target/test_hammer 与 init 的 supervisor/filesystem/time_checks 检索 sys_notification_* 为零；生产通知消费者没有 raw 调用。main 的裸别名/StaleHandle、transit、Owner 负向、MailboxFull 和持久 WaitSet，race 的发令枪属既有明确原始夹具；每处都有 SAFETY，不加安全裸 handle facade。

**验证命令与退出码。** 下列日志前缀统一 artifacts/notification-observer/；host 与自定义 RV64 分开。

```sh
just check
just clippy
just --command bash -c 'cd user && cargo check --workspace --bins --exclude test_fp --features srv_init/acceptance-stress --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem'
just --command bash -c 'cd user && cargo test -p libkernel -p librpc -p libprocess -p libexecution -p libfs -p libservice --target aarch64-apple-darwin'
just --command bash -c 'cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin'
just --command bash -c 'cd shared && cargo test --workspace --target aarch64-apple-darwin'
just --command bash -c 'cd user && cargo clippy --workspace --bins --exclude test_fp --features srv_init/acceptance-stress --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings'
just virt
just virt-stress
```

| 证据 | 日志 / 退出码 / 边界 |
|---|---|
| kernel check | kernel-check.log，0；user Rust type migration 不改变此后 kernel 代码 |
| 七分面 lint | clippy2.log 与 artifacts/lint/，0，shared/os/user host、kernel/user target、stress、独立gc；初次误公开全 call 导致无关 raw API 缺 Safety 的 clippy1 失败已修为只导出四个明确接口，不加 lint allow，失败保留 |
| RV64 workspace stress check | user-check6.log，0；编译断点中的 types/import/fixture 语法失败日志保留，不用 adapter |
| 受影响 host | user-host-tests.log，0：execution33/fs23/kernel6/process4/rpc16/service11，另既有 process 文档用例2；os-host-tests.log/shared-host-tests.log 均0。没有 fake Kernel role 或新测试服务 |
| 最后夹具补证 | N4 增加“全部 Observer 已关闭仍保留原 pending/Owner”真实断言后，user-target-final-fixture.log 0；只补受影响 target lint 和 virt2，不重跑未受影响的通过项 |
| core | virt1.log / virt2.log 均1；guest分别 artifacts/failed-acceptance-20261010-092956-28815.log / failed-acceptance-20261010-093610-33881.log。guest实际新角色/业务和reset完成；严格验证器缺旧FAL2 Open control and terminal cases passed after 16384 bytes，属主计划 G05-20 的 init exercise_open_streams/StreamControl，不能把命令1写成通过 |
| stress | virt-stress1.log，1；guest artifacts/failed-acceptance-20261010-093106-30013.log 已完成新角色、Startup/Owner退出、256登记自动退休、16/16竞态矩阵和reset；同一 G05-20 锚点使严格路线失败，脚本未改、不放宽、不重建旧协议 |

**N1—N7 按门的证据边界。** 下表中的实际锚点来自上述完整 guest，core-anchors.txt/stress-anchors.txt 为检索摘要；单次 schedule 不冒充所有内部交错。

| 门 | 实际运行证据 | 仅源码/编译/host | 缺口 |
|---|---|---|---|
| N1 | core/stress 的 Notification observer roles passed：缺M、输出缺WAIT/越权/未知位、Observer无Take/Signal/Mint、Signaler非法READABLE的WaitOne/WaitSet、Mailbox[M]错型、identity及负向后pending存活；既有 capability badge/affine owner用例核Owner运输拒绝 | 静态三集合与通用 WAIT 校验，源码无WaitSet角色特判，root四raw接口unsafe | 未单独注入Close先于接受的每个失败时点；属于共同解析和N6窗口 |
| N2 | 同一锚点：两个真实用户线程与两个Observer、预先置位后登记、部分Take仍READABLE、全清后Receive空、新Signal再次可见、不消费观察 | capability signal host既有轮次/插入边界/瞬态电平测试全部通过 | 任意线程排序不能由一次实际运行穷尽；没有承诺逐事件日志 |
| N3 | 同一锚点两个持续成员Receive(1)轮转、清空后撤候选、cap关闭后来源loan仍看CLOSED、Remove/集合Close；WaitSet persistent levels...锚点及stress256登记/16矩阵 | 共同候选复查/hint/唯一detach无代码特判；existing signal/table/Runtime host | 查询中hint精确内部窗口由既有host+源码支撑，未用GDB单次暂停作公平或计时证明 |
| N4 | core新夹具核全部Observer关闭不清pending/不终止Owner；Observer/Signaler/loan存活时最后OwnerClose仍CLOSED。stress Notification observer process exit passed，子进程Owner经真正Kill/DONE自主关闭，父方Observer/Signaler仍活，Signal=ObjectClosed | Owner Terminal holders与存储Arc/loan分离；Take关闭竞争返回ObjectClosed分支核源码 | Take已接受后与最后Close插入的精确窗口未单独强迫；普通已关闭表项只返回共同解析错误 |
| N5 | core Packet运输及Duplicate裁剪、错型采用返owner/缺WAIT拒绝；stress pid21 Notification observer startup grant passed，kind20经真实ProcessGrant/StartupContext交付并与Owner源身份配对 | Adopt失败完整Cap返还；from_minted crate-only，无公开已验证构造/Clone；context kind20实际迁移 | 公共IPC kill夹具的post-ready日志可被正常Kill竞争省略，不据此声称缺执行；stress用例先完成采用/身份检查再发布ready，已有确定锚点 |
| N6 | 当前运行的负向签发后pending/Owner正常，以及邻接IPC退出/退款锚点；不能据此推定全部mint费用失败门通过 | 亲读Authorization/MetadataCharge/Arc.try_new/KOID/handle.install：源接受不增holder，观察壳由调用者实际付款；reserve失败release_all，写回失败rollback/release_all，Frozen commit沿最后事件释放，core最后引用/来源Work前不提前退款 | 未注入mint-specific QuotaExceeded/OutOfMemory/KOID耗尽、输出fault/安装Frozen、接受后Close窗口，也未单独测观察壳/source费用逐项退款。现有公共Pool/IPC设施没有可直接复用的这些观察mint时点；不在init自己制造output fault，不新建故障服务。owner RubyRaven/协调MellowPond；稳定隔离故障或正式Pool饱和设施出现、或相关故障暴露时，用真实子进程/原设施补证，失败保原业务、全部实际owner释放后退款为标准 |
| N7 | 原core/stress shared Sender、Once/Delivery/Lifetime、threaded Receive、public time、committed IPC kill、WaitSet、监督/Job、Tunnel/内存及16/16矩阵均继续运行至reset；P4完整acceptance3另已通过debug stress/release/sifive_u/nofd及Boot/IPI/VM故障路线，证据见末节 | typed只改独立观察owner/公开命名，内部Lifetime计数/Reply结果及相邻等待实现不变；check/clippy/host均通过 | 跨平台/release/完整聚合缺口已补齐；N6精确mint费用/失败时点仍按本篇唯一条目保留，不由这些路线代替 |

**三类覆盖与交付边界。** 已实际跑到：Notification typed/真实IPC/Startup与Owner退出、service WatchName Query→重读/GetEntry/Ack/旧Ack/Replace/Withdraw与两个provider退休；已编译但没有对应新行为运行：fs Watch客户端 typed采用（provider仍Unsupported）、其他已编码但未接通领域请求。流/Copy/Bind与G05能力继续由主计划唯一登记，本案不核销。现有正式Watch消费者同步调用：等待WaitOne已返回后才发Query等RPC，没有持续Observer登记跨在途RPC，也没有新增异步Watch runtime；未来/第五批实际异步客户端须按主计划保留在途责任并暂移来源，不用纯电平重复Receive冒充业务工作。

没有本案新建容器：core原WaitQueue/Work单份；每次Mint只新增真实Observer Arc及共同handle预约。用户typed壳内嵌Capability、不分配；消息/WaitSet已有独立prepare容量/资金owner不变。本轮格式化触及的boot/startup/tar无关差异已逐hunk撤回，不reset其他任务。本篇专项辅助review与主实施者REVIEW核对完成后追加结果；N6执行缺口不冒称已测，不归档，不登记整个系统Watch完成。

**交付前结构检查。** 主实施者按 REVIEW 核对 ABI 两侧、静态角色与权限/输出完整mask、唯一core header与related_id、Authorization/BodyLease不增业务holders、源码锁序/每step一候选、实际付款与所有安装失败owner、五named typed采用/复制/close及RPC/启动交接、旧出口零匹配和当前impls。未发现本案需要代码修复的可达缺陷；没有新增通用wrapper/动态mask/WaitSet特判/原地Observer遍历/Kernel线程或双ABI。专项只读辅助review（sub-7，134s）独立核对Notification、五角色边界、付款/install回滚、RPC/Lifetime持有、Wake和kind20采用，结论无有证据支持的可达缺陷；它未重跑测试，未把stress纳入其报告，实际stress证据由主实施者上述记录承担。该辅助审查不是固定提交的独立Review，不替代本次主实施者检查或N6补证。

业务检查发现的WatchControl原账户付款遗漏 W04-01只登记在主计划，属于下一批Watch剩余；不扩进通用Notification前置，也不将service开发锚点写成N8全合同交付。N6未注入时点已列owner/触发/验证门，本案保持活跃、不归档。工具进程检查确认本轮QEMU/throttle/acceptance全部结束，无保留常驻进程；git diff --check通过，本轮新增文档链接/锚点与表格另核对。

### P4 下游聚合证据回填（2026-10-11）

主计划已接通fs/service同步与异步Watch、流及Copy、可信domain/execution付款、A/B控制及终态壳；原W04-01遗漏已沿主计划通用付款与W04-02真实多账户闭合。test_fal已改为test_fs，目录/package/target/crate/启动/initfs同步，无别名；以上原批次记录保留当时证据，不继续作为当前施工入口。

`just acceptance`第三轮完整exit0，784.97s含构建与全部路线；主证据 artifacts/p4/acceptance3.log/.exit/.elapsed、acceptance3-summary.json。debug stress/release/nofd和VM fault完整stress正常reset，sifive_u完整core后reset accepted→明确reset failed、按原allow-timeout规则收割，required/rejected均0。Notification真实角色、相邻IPC/等待/监督/退款锚点随正式负载回归；stress16/16及256登记保留。Boot panic/alloc/fatal的4hart Gate Failed、真实SBI IPI失败后的稳定停驻、VM发布真实RetryFault和外部模拟repair R/W/X/repeat/uaccess均通过；路径与边界由[主计划P4聚合记录](todo-2026-10-07-fal-service-redesign.md#p4-锚点裁决实施与聚合验收2026-10-11)及结构化证据拥有，不复制注入设计。

N7原“跨平台/release/完整acceptance未执行”缺口现已补齐。N6 mint-specific Quota/allocator/KOID/输出fault/Frozen/接受后Close及观察壳逐项费用没有新增精确注入，仍保留原唯一owner、触发与完成门；公共VM/Boot故障不能替代该窗口。主计划已完成P4交付前结构检查、旧路径删除及impls回填，专项无新增可达finding；本篇N6未闭合前不归档。所有本次工具进程已退出，无常驻QEMU/GDB，未commit/push。

### N6 注入前置再核（2026-10-11）

GrandOwl 在主计划提交前收敛中亲读 notification::mint_observer、handle::install/close、既有 init public_ipc/Pool 场景、test_target 与 tools/check-*。Mint 的 metadata/observer 壳付款固定为 thread.process.funding_pool；现有 test_pool_budget 的可指定 Pool 负向是 Invitation 创建，不能把它当成 Notification mint 的费用证明。test_target 现有入口用于 IPC kill 与退休，不含隔离的 mint 失败/费用观测场景；现有 GDB 工具分别固定 Boot/IPI/VM 故障点，没有 observer MetadataCharge、allocator、KOID、handle reserve/install 或 uaccess 写回点。

N6 精确窗口仍保留，不靠在 init 主进程耗尽资源或制造输出 fault 来补证。具体前置是：在现有隔离目标/正式注入设施上准备可控的 Mint 失败点、付款域及释放后的费用观察，能区分 observer 壳、core、handle 元数据、WaitSet source 与待维护引用；各接受/失败点恢复后继续观察原 Owner/pending（进程已被真实 fault 终止则按进程退出契约核 owner 收束），最终按实际物理退出核退款。该设施出现或相关真实故障暴露时补证；本案不新增测试专用服务、内核测试 ABI 或全局 KOID 耗尽。合法普通 owner 的 Handle Close 当前不分配、Claimed 也必成，其资源性失败假设不适用；不能用伪造 stale handle 的错误替代 mint 并发接受/源关闭窗口。当前复核由 GrandOwl 记录，后续 N6 仍由本案承接，不向信息协作代理派发编辑。
