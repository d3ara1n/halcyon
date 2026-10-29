# Notification 观察角色与统一等待契约

状态：三角色、共享 ABI、typed owner 与实际消费者已经接通并交付。本篇保留目标合同与 N6 精确失败/费用窗口的唯一承接；原设计盘点、迁移清单与逐轮验证见[历史快照](archived/ref-2026-10-27-notification-plan-history.md)。

## 当前接力与范围

本案拥有 Notification 三角色、观察角色命名、派生授权、共享 ABI、libkernel 类型、Wait/WaitSet 接入与付款/保活。fs/service Watch 的业务协议、sequence/pending、Query/Ack/Cancel、控制 Lifetime 和业务退款由[OAL/fs 计划](todo-2026-10-07-fal-service-redesign.md)拥有。

Owner 消费/关闭、Signaler 发送并可观察关闭、Observer 只读观察 READABLE/CLOSED 的分工已经确定。通用接线与 N7 跨平台/release 证据已收口，fs/service 的同步与异步 Watch、流及 Copy 已接通；当前唯一后续是 N6，不继续旧迁移。后续由承担本计划的实施者维护证据。

非目标是共享通知接收端协议、badged signaler、逐事件队列、泛型公共 Observer、任意对象统一派生接口、多对象一次性等待/嵌套 WaitSet，以及不可信域 metadata 硬隔离。

下表缩写：R=READ、Q=WAIT、S=SIGNAL、M=MANAGE、D=DUPLICATE、T=TRANSIT、G=GRANT。来源的 CLOSED 是不可逆关闭；领域 PEER_CLOSED、DONE 与 EXECUTABLE 按各自业务事实解释，不因名字相近合并。

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

沿用[等待源分案的轮次5](archived/todo-2026-10-02-wait-source.md#设计轮次-5轮次-4-定向修正用户裁决禁止嵌套-waitset)：来源只提示，节点 owner 自摘，扁平集合，当前电平复查，无 Rearm/事件结果队列。本案替代该计划末节关于“Signaler 不可等待、Observe 仅 CLOSED”的角色约定，不改其通用等待机制。

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

## 消费者与验证门

验证按[构建手册](BUILD-AND-TEST.md)执行；下表拥有验证合同，实际实施证据由末节工作记录拥有。优先扩展现有 srv_init/public_ipc、time_checks、race 和正式 srv_fs/test_fs 路线，不新增专用测试服务、临时消费者或模拟内核。

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

按[构建与验证手册](BUILD-AND-TEST.md)选择受影响 shared/kernel/user package 的静态检查和 host 测试；运行期按上述风险选择 ipc/process/transport 等领域及必要压力场景，不把旧聚合路线作为固定门。编译暂时失败不能成为搭兼容 adapter 的理由。同一基线通过的检查不重复；与主计划共用组合运行时明确哪些日志覆盖本案。

持续就绪活性、并发安装和所有失败窗口不能仅由一次 QEMU 通过推定。无法通过现有设施运行的内部窗口记录精确 owner、审计证据和未覆盖部分；不得将“设计上应当成立”列为执行通过。

## 证据与当前限制

N1—N5 的真实角色、就绪、运输、Startup 和 Owner 退出已运行；持续就绪与注册交错另有 host/源码依据，不外推所有调度。N7 的公共 IPC/时间/等待/进程/内存及 stress 矩阵随下游完整组合取得证据，N8 的 fs/service 同步与异步 Watch 由主计划拥有。原开发轮 core/stress 的退出 1 保留为旧锚点失败，后续 P4 组合成功不覆盖改写原结果。

完整命令、版本和各门限制见[逐门证据](archived/ref-2026-10-27-notification-plan-history.md#实施工作记录角色迁移与开发证据2026-10-10rubyraven)与[后续聚合](archived/ref-2026-10-27-notification-plan-history.md#p4-下游聚合证据回填2026-10-11)。本次文档整理没有补跑这些路线。

### N6 注入前置再核（2026-10-11）

N6 尚未覆盖 mint-specific QuotaExceeded、OutOfMemory、KOID 耗尽、输出 fault/安装 Frozen、接受后 Close 及观察壳/source 费用逐项退款。负向权限检查后 pending/Owner 正常，或其他 Pool/IPC 的退出退款，均不能替代这些时点。

Mint 的观察壳费用来自调用进程 funding Pool；普通可指定 Pool 的 Invitation 负向测试不证明它的付款行为。2026-10-11 的设施核对没有得到隔离的观察派生失败/逐项费用接点，原调查仅作为当时依据，不据旧工具清单否定未来设施。

触发条件为已有隔离目标/正式设施能控制 Mint 失败并观察实际费用，或出现相关真实故障。触发后区分 observer 壳、core、handle metadata、WaitSet source 与维护引用，分别观察失败后原 Owner/pending 及真实释放后的退款；若进程因真实 fault 终止，按退出合同观察收束。当前不在 init 主进程制造输出 fault、不耗尽全局 KOID，也不为补证另建测试服务或内核测试 ABI。

合法普通 owner 的 Handle Close 不分配且 Claimed 也可关闭，不能用 stale handle 错误代替并发接受/源关闭窗口。本篇在 N6 闭合前保持活跃，不把普通组合成功写作精确注入完成。
