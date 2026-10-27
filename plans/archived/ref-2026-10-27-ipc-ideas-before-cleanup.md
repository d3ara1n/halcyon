# IPC 与数据面线稿整理前快照

> 2026-10-27 清理续接前的线稿。包含已被替换的 WaitMany/WaitContext、旧 Runnel 布局以及尚未重新审定的 BufferQueue 字段候选，不作为当前合同或施工入口。当前模块关系从 [notes 索引](../../notes/README.md) 进入。


---

## call.md

# 内核调用

内核调用分为 System Call（用户线程进入内核）与 Remote Call（一个 hart 请求另一 hart 处理内核短动作）。两者都必须保持内核路径有界，不产生可睡眠的内核执行流。

## System Call

同步调用在一次 trap 内完成；普通操作把结果返回调用线程，终局操作成功后可以不再返回该系统实例。异步调用在入口验证参数和 authority、预留业务事务后，把调用线程转入 Waiting；内核立即调度其他线程，不在调用栈上等待事件。调用名称不决定完成形态：HandleClose 若撤销 object-owned mapping，也必须在远端地址翻译确认完成后才向仍存活的调用者返回。

异步完成由 WaitContext 表达：对象订阅、可选 Timeout 与完成动作竞争唯一 outcome。普通异步结果由完成者写入保存的用户现场并重新入队，线程恢复时从原 ecall 之后继续；终止取消则直接消散线程，不返回用户态。已经越过业务 Commit 的操作不能随 WaitContext 取消：线程只放弃最终回复，业务对象继续推进，且线程 departed/join 必须等待其结果记录解除挂接。等待和 Timeout 的公开语义见 [`wait.md`](../../notes/ideas/wait.md)。

同步输出与异步等待结果具有不同交付边界：会返回的同步调用成功意味着结果已经写回；普通异步等待在 park 后不得依赖可能失效的用户指针。会创建进程资源且可能 park 的 `MemoryMap` 使用内存模型定义的提交前结果承诺：全部可失败工作完成后，以固定宽 UserWriteLease 稳定结果槽，先写 payload、最后 release 发布 cookie 并 Commit 业务；完成阶段只修改保存的返回状态。终局同步调用必须定义成功不返回及所有异常返回的语义。三者都不得因用户地址或参数错误 panic 内核。

系统调用表只命名内核独有且已经成立的原语，不为用户态策略、便利组合或未设计能力预占入口。线程 join 由可等待的 ThreadControl、DONE 电平、WaitMany 与 HandleClose 组成；结果记录、栈解除和 Drop 策略属于用户态库，因此不另设 ThreadJoin。ThreadKill 若未来出现，必须先确定终止 authority、终因和资源接管，再决定 ABI，不能只因调用号相邻而提前占位。

## 时间与对象观察

公共 MonotonicNow 读取系统单调时间域；有限 Send 和等待直接接受绝对 Deadline，完整契约由 [time](../../notes/ideas/time.md) 拥有。用户库的相对时长入口只做一次转换，不构成第二套内核等待机制。

对象查询描述实际持有的授权，不能按裸身份打开对象。普通 Close 放弃本地持有并由内核接管必要退休；集合、消息和进程回收均不要求调用者继续推进内部维护。释放与需要等待的完成操作分别表达，关系由 [object](../../notes/ideas/object.md) 和 [wait](../../notes/ideas/wait.md) 拥有。

## Remote Call

Remote Call 是 hart 间内核短动作的传输层，由 IPI 门铃、固定容量请求槽和可选完成通知组成。请求槽的 Pending 电平是工作真值，IPI 只提示目标 hart 检查槽，不携带业务载荷；门铃可以合并或重复，目标在每个 trap 安全出口都检查本 hart 固定槽。普通调度唤醒只需要门铃，不伪造 Remote Call 请求。

发起方在发布业务状态前预留全部目标槽；容量不足在业务 Commit 前返回忙，不建立无界请求队列。发布后，请求即由其业务所有者负责完成，发起线程终止只消散返回权，不能撤回远端已经可能观察的动作。平台 admission 预先保证 admitted hart 的 IPI 与周期性 trap 路径；Commit 后门铃异常只保留 Pending 等后续安全点补消费，不能改写为业务失败。请求合并只能保持每个原始请求的完成条件。

TLB shootdown 是首个消费者。页表写入方按[内存模型](../../notes/ideas/mm.md)发布 PTE、epoch 与请求并在 IPI 前执行 data fence；目标 hart acquire 请求，执行本地 `SFENCE.VMA` 和可选 `FENCE.I`，再 release 确认。需要等待确认的 System Call 以 WaitContext park 发起线程，最后一个 acquire 确认者推进业务事务并完成等待；内核不在调用栈上等待远端 hart。Remote Call 不拥有地址空间事务、Handle close 或 backing；哪些 hart 必须参与、何时允许复用 backing 由内存模型拥有。

落地形态见 [`../impls/call.md`](../../notes/impls/call.md)。


---

## rpc.md

# 通用 RPC

Mailbox 是 RPC 传输原语，内核不感知请求与应答。分层为：

1. Mailbox 运输消息、业务 capability、内核产生的发送授权身份与 provenance，以及独立 Delivery；
2. RpcPrefix 描述固定宽版本、request/response/oneway 与非零 txid；期待回复的 request 在业务 Handle slot 0 放置 send-once；
3. FAL、服务与设备协议定义自己的版本、kind、错误和 payload。

send-once 证明一次回复的投递权，txid 负责回复关联，发送授权身份与 badge 指向服务端上下文，sender PID 只是 provenance。Delivery 保留这条消息的处理责任，不证明业务完成，也不代替回复权。

## 所有权与投递阶段

调用的运输状态明确区分 Unsent、Sent 和 Completed。Unsent 拥有完整请求及尚未搬出的 capability；成功 Send 后进入 Sent，原能力不再属于调用者。投递前失败或到期返还未转移的 owner；投递后超时、服务退出或丢失回复只能报告业务结果未知。

收到的请求和回复先由运输 owner 持有，协议提取后才形成业务 owner。畸形、迟到、重复或不匹配的回复统一关闭未提取能力及 Delivery。请求上下文保留 Delivery，直到处理、回复或拒绝责任结束；异步处理时把整个上下文交给任务。

ReplyPort sender 派生的 send-once 是新的一次性发送对象，配对的 ReplyObservation 由调用者持有；以 WRITE、WAIT、TRANSIT 进入请求。WAIT 允许服务在满箱时等待可写或关闭，但服务 outbox 必须有资源上限和期限，不能阻塞整个请求循环。

## 并发与回复路由

同步请求可以拥有独立回复接收关系；失败、超时或调用者取消时释放本地持有，由内核接管相应资源退休。未投递的请求仍归调用者，已投递但未确认的业务结果仍为未知。回复资源的本地关闭不产生可失败清理 owner 或重试协议；请求取消与服务端已经接受的业务责任分别处理。

单调用的分步接口遵守同一规则：放弃尚未完成的操作后，旧回复不能进入后续调用；接受回复后形成终态，不能再次解释为未决调用。错误的未发送/已发送分类来自实际投递阶段，本地编码失败不会产生远端未知副作用。同步驱动和外部驱动共享这份语义；公共操作模型可面向长期同步/异步组合提前设计，避免各业务分别维护运输状态与清理规则。具体业务语义仍由相应协议拥有。

异步多 in-flight 使用一个 dispatcher 按不复用的 txid 路由。单个请求超时只退休该 pending 项，不能关闭其他请求共享的回复端口。不存在 pending waiter 的回复连同能力一起丢弃，不保留无限 tombstone。

同步与异步调用共享 framing、投递状态、Deadline 和 capability 所有权模型，端口的失效范围由各自路由结构决定。未完成的请求、待发送回复及其观察注册都必须经过有界准入。

## Deadline、取消与重试

调用入口把相对时长一次转换为 [绝对 Deadline](../../notes/ideas/time.md)，同一值覆盖投递背压、回复等待、Receive 与最终 framing 接受。内核 Send 在提交点核验投递期限，内核等待直接使用绝对时点，不通过反复相对等待延长预算。

一条回复只有在关联和 framing 合法、且最终接受检查仍未过期时才完成调用。就绪观察本身不保证仍在期限内。已经接受的回复不因随后服务关闭而改成未执行；尚未确认的副作用不能因超时推断未执行。

Timeout 只停止本地等待。服务取消、幂等键、去重和重试属于协议扩展，不下沉为内核强取消。库不得自动重试已投递的有副作用调用。有限 Deadline 的可表达性、精度和溢出由时间契约统一定义。

## 服务执行

服务可以同步处理短操作，或把 RequestContext 移交给有界任务。发送回复失败不自动回滚已提交的业务修改；只有协议显式定义的发布事务，例如一次性属性 Take，才以回复成功入箱作为业务提交点。

控制循环通过 [WaitSet](../../notes/ideas/wait.md) 推进 Mailbox、下游 PendingCall 和 outbox。来源关闭、观察失败、期限到达与正常完成都进入同一任务收束路径。

通用 framing 与调用生命周期属于 librpc；libkernel 提供 Mailbox、Handle、WaitMany、时间和等待基础契约，不解释 RPC。具体 Rust runtime 由可执行程序选择，librpc 不依赖 libruntime。

## Runtime 接缝

RPC 拥有请求/回复、关联、投递状态和业务结果的解释；执行层拥有就绪与期限的执行机会。协议状态与等待来源的拥有关系必须清楚，不能在两层各自复制一份相互校准的真值。

同步和异步组合使用一致的投递与结果语义。选择执行框架不改变能力运输、超时后的未知结果和取消责任，也不要求普通调用先运行某个特定服务循环。

任务取消、回复放弃与资源关闭分别表达。内核订阅和资源退休由内核接管，RPC 不承担注销重试或保持内核清理活性的循环。具体任务类型、回执协议与端口布局由独立设计确定，不作为领域线稿中的固定结构。


---

## ipc.md

# 进程间通信

IPC 建立在[对象、Capability 与 Handle](../../notes/ideas/object.md)模型上。每个通信对象只经进程本地 Handle 操作，对象类型、rights 与对象状态共同决定合法行为；不同角色是不同对象类型；PID 是 provenance，不是通信地址或 authority。一次等待与持久观察由 [WaitMany / WaitSet](../../notes/ideas/wait.md) 统一完成。

## 三个面

- **消息**（[message](../../notes/ideas/message.md)）是控制面：有界、非阻塞的 Mailbox 投递，承载小 payload、内核生成的来源与发送授权 envelope、原子 TRANSIT Handle 和独立 Delivery；
- **对象状态与 Notification**（[signal](../../notes/ideas/signal.md)）是事件面：对象以非消费式电平表达可读、关闭等条件，Notification 提供显式消费的 OR 位集合；
- **Tunnel**（[tunnel](../../notes/ideas/tunnel.md)）是数据面：两个与本地地址空间 lease 绑定的 Endpoint 映射同一有界多页共享区间，页内协议遵守[共享内存公共契约](../../notes/ideas/shared-memory.md)。

[Runnel](../../notes/ideas/runnel.md) 是 Tunnel 上的官方单工 FIFO 字节流协议；[BufferQueue](../../notes/ideas/buffer-queue.md) 是与其并列、引用预注册 MemoryObject 的记录与缓冲交接协议。控制请求、流数据、缓冲交接和状态提示可以组合，但不得让一个面伪造另一个面的 authority、生命周期或流控。

## 共同边界

内核负责对象引用、rights、badge 保存、Handle 运输、消息事务、映射生命周期、状态发布与等待完成；它不解释 kind、badge 业务含义、RPC、路径或共享页。

`TRANSIT` 允许 Handle 暂存于消息，`GRANT` 允许 Building 期的直接跨表安装。unique owner 只走直接 GRANT；sender、signaler 与 invitation 可按授权经消息委托；Endpoint 与 VM 绑定而不移动。

协议使用独立的 badged sender 表达不可伪造的授权上下文，发送授权身份区分实例，sender_pid 只作 provenance。Lifetime 不保活被观察授权，Delivery 保活处理中的调用；回复仍需要显式 send-once 或 sender，不能由 Delivery、PID 或身份数值推导。

启动根图由 launcher 在目标 runnable 前通过 ProcessGrant 与用户态 StartupBlock 建立，ProcessStart 只发布已经组装完成的线程。Mailbox owner 只是可选启动资源，不占固定寄存器或固定 Handle 数值。


---

## signal.md

# Notification

Notification 是按位 OR 合并、由消费者显式取走的独立对象。它适合无负载、允许合并的通知；需要身份、次数、顺序或数据时使用消息，需要高频共享状态时使用 Tunnel 协议。

## 角色与待决位

创建者获得唯一 owner Handle。owner 不可 duplicate 或 TRANSIT，可在目标进程 Building 期直接 GRANT；owner 关闭使对象进入 CLOSED。获授权的 signaler 可按 rights duplicate、TRANSIT 或 GRANT，并提交位掩码。

待决位按 OR 累积，重复提交同一位不会计数。存在任意待决位时对象的 READABLE 电平为真。

## 消费

消费者先用 WaitMany 观察 READABLE，再调用 NotificationTake 原子取得并清除所选位；未选择的位继续保留。WaitMany 从不改变待决位，因此多个等待者可以同时被唤醒，最终由 Take 协调消费竞争。

Notification 不是广播队列：一个消费者取走某位后，其他消费者不再看到该位。需要每订阅者可靠通知时，provider 必须为每个订阅关系持有独立 Notification signaler，或使用可重放日志。

ObjectSignals、WaitMany、WaitSet 与取消的通用契约见 [wait](../../notes/ideas/wait.md)，绝对期限见 [time](../../notes/ideas/time.md)。持久注册也只观察 READABLE/CLOSED，NotificationTake 仍是唯一的位消费入口。


---

## shared-memory.md

# 共享内存协议公共契约

共享内存只提供同一组字节在多个进程中的映射，不提供可信性、消息边界、所有权或流控。所有运行在共享区间上的协议都必须在自身格式之外遵守本公共契约；[Runnel](../../notes/ideas/runnel.md) 另行定义单工字节流字段与 EOF，[BufferQueue](../../notes/ideas/buffer-queue.md) 另行定义 region 注册、descriptor 与缓冲交接。

## 可互操作布局

协议布局只使用固定宽度整数，明确字段偏移、大小、对齐、保留区和初始值。多字节字段一律 little-endian。写者必须把保留位和保留字节置零；读取已支持版本时忽略保留内容。版本锚点必须在任何可解释的可变字段之前发布，接收方在建立角色前验证锚点、版本、长度和全部必要不变量。

并发控制字段必须天然对齐，并以协议指定宽度的原子访问完成；不得以普通读写、`volatile` 或编译器偶然行为代替原子。共享数据由集中定义的平台字节访问原语读写，可见性仍由控制字段的发布次序保护；原语必须同时满足映射存活、范围、编译器及硬件并发契约，不能只证明游标边界。

不可信共享区不能作为普通 Rust 共享/可变 slice 或含受限位型的结构暴露给调用者。访问方先将字段采样为固定宽整数，再验证并解释；数据只在共享区与本地合法缓冲之间复制。语言模型内的原子存储可用于合规双方的算法证明，但任意外部进程可破坏角色纪律，跨进程安全必须明确所依赖的平台访问契约，不能声称普通拷贝、volatile 或单方使用原子已经自动消除该边界。

## 发布与取得

共享页运行于 RISC-V 弱内存序环境。写者先写数据或普通字段，再以 **release** 发布控制字段；读者先以 **acquire** 取得控制字段、确认条件后才读取受保护数据。反向回收同理：读者完成读取或腾出空间后 release 发布进度，写者 acquire 取得进度后才覆写。

每个可变字段必须有唯一写者或明确的原子读改写规则。协议不允许两个角色以普通写竞争同一字段；跨端计数、游标和状态转换必须给出回绕、溢出与单调性规则。每方应保存足以验证对端进度的本地 shadow 状态；不可信字段在用于寻址、分配、循环或资源决策前必须验证边界。

## 角色与不信任边界

双方先由上层约定角色，再按角色获得读写视图。角色决定初始化者、字段唯一写者与合法状态转换；内核不解析页内布局或替协议裁决角色。

对端持映射即可写任意字节。合规双方可获得完整数据与发布/回收顺序；非合作对端可能伪造看似合法的内容、破坏完整性或拒绝服务，本端仍必须维持自己的映射、访问范围和语言安全，不承诺识别全部作弊。

发现格式错误、违反单调性、越界、未知必需版本或不可能状态时，本端进入不可逆 **Broken**：停止信任页内容和正常传输，向上层报告协议失败并释放本地持有。关闭不要求失败重试，实际映射和 backing 由内核安全退休；协议停止、访问撤销完成和物理回收分别表达。观察到对端关闭也使本端 Broken；旧页不得恢复为正常通道。

## 门铃与等待闭环

门铃只是 **hint**：提示重新检查控制块，不携带计数、不证明数据存在、不替代 acquire/release。多次门铃合并、早到或晚到都不改变控制块真值。

阻塞方必须完整闭环：检查可进展条件；无进展时显式确认本端门铃；再次检查；最后以 [WaitMany](../../notes/ideas/wait.md) 等待门铃或关闭；醒来后从头检查。清门铃只在协议已达到该角色无进展条件后合法，例如消费者读至空后才确认数据门铃。任何正进展在等待或返回前都必须按协议通知对端，至少关键状态转换与终态发布不能遗漏。

## 生命周期

共享页映射、端点关闭和帧回收由承载对象定义；协议不得仅凭页内标志释放或重用帧。对象终态通知后，本端停止访问不再受保证的内容，并依对象规则撤销映射或释放 Handle。共享内存不是持久化格式，除非上层另行定义保存、恢复和版本迁移契约。


---

## runnel.md

# Runnel

Runnel 是 Halcyon 的官方流式数据交换协议：运行在[隧道](../../notes/ideas/tunnel.md)共享区间上的**单工 SPSC FIFO 字节流**。本规范与[共享内存协议公共契约](../../notes/ideas/shared-memory.md)共同构成互操作要求；固定宽度、little-endian、对齐、Acquire/Release、角色视图、不信任对端和 Broken 规则均为规范义务。

Runnel 只回答字节流，不携带记录、Handle、MemoryObject 注册或 buffer ownership。需要记录边界、scatter/gather 或零拷贝缓冲交接时使用并列的 [BufferQueue](../../notes/ideas/buffer-queue.md)，不能把 descriptor ring 解释成 Runnel 的内部实现。

## RNL2 布局

共享区至少一页，固定使用前 128 B 控制块，剩余全部为环形数据区。Tunnel 返回的规范化映射长度为 `total_bytes`，`data_offset = 128`，`capacity = total_bytes - data_offset`。控制块和数据区同权时不拆成独立控制页。

| 偏移 | 类型 | 字段 | 唯一写者 |
|---|---|---|---|
| `0x00` | 对齐原子 `u32` | `MAGIC = 0x324C4E52`（小端字节为 `RNL2`） | 初始化者，最后发布 |
| `0x04` | 对齐原子 `u32` | 低 16 位 `VERSION = 2`，高 16 位 `header_bytes = 128` | 初始化者 |
| `0x08` | 对齐原子 `u64` | `total_bytes` | 初始化者 |
| `0x10` | 对齐原子 `u64` | `capacity` | 初始化者 |
| `0x18` | 对齐原子 `u64` | `head`：累计写入字节数 | 生产者 |
| `0x20` | 对齐原子 `u64` | `tail`：累计读取字节数 | 消费者 |
| `0x28` | 对齐原子 `u32` | `eof`：流结束标记（0/1） | 生产者 |
| `0x2C` | 对齐原子 `u32` | `flags = 0` | 初始化者 |
| `0x30–0x7F` | — | 保留，必须为零 | — |

初始化者通过共享访问原语初始化完整控制块，以指定宽度原子 relaxed store 写入版本、几何、游标、EOF 与 flags，保留字节置零，最后以 release store 发布 MAGIC；完成后才向上层交出 Invitation。attach 方先以 acquire 读取 MAGIC，再用原子 load 各取得一次版本和几何，验证版本/header 长度、flags、`total_bytes` 与本地 Tunnel 映射完全相等、`capacity = total_bytes - 128` 且容量位于协议硬上限内，随后把几何冻结为本地 shadow。读方忽略保留字节，未知 flags 拒绝；正常传输不再读取共享几何字段。对端以后篡改这些初始化字段不会改变本地寻址，若经诊断复检发现则 Broken。RNL1 不构成兼容面，升级时两侧同步替换。

## 游标、读写与 EOF

游标是 `u64` 自由计数而非数组下标，差值按模 2^64 回绕：`used = head -% tail`，且必须在 `0..=capacity`。capacity 必须小于 2^63，使局部 shadow 所验证的前进距离无歧义。空当且仅当 `head == tail`，满当且仅当 `used == capacity`。

累计进度不能直接对 capacity 取余用于寻址：一般容量不整除 2^64，整数回绕会改变余数。每个角色独立持有从零开始的本地物理 cursor，仅按该角色实际完成的字节数对 capacity 取余推进；累计进度只用于可读量/可写量和对端前进量检查。复制从物理 cursor 起至多分两段，数据流跨整数回绕仍保持连续。

一条连接上的每个角色只建立一次，不支持从任意旧进度重建角色。attach 的生产者必须验证本方尚未写入或发布 EOF；attach 的消费者必须验证本方尚未读取，允许创建方生产者已发布不超过容量的首批数据/EOF。角色从零产生这一前提使两端物理位置可归纳一致，无需再在共享区复制一套位置真值。

生产者独占 `head` 和 `eof`，消费者独占 `tail`；双方不得写对方字段。生产者先写数据，再 release 发布 head；消费者 acquire 读取 head 后读数据，再 release 发布 tail；生产者 acquire 读取 tail 后才覆写。双方保存最近一次已接受的对端游标：生产者只接受 tail 前进不超过此前 outstanding，消费者只接受 head 前进不超过此前 free；更新 shadow 后仍须满足 `used <= capacity`，每次复制长度不得超过本地验证容量。违反任一条件即 Broken。

生产者在最后一次 head 发布后以 release 置 `eof = 1`，此后不再写入。消费者先 acquire 读取 EOF，再 acquire 读取并冻结最终 head；只有已观察 EOF 且 tail 追上该最终 head 才是正常 EOF。已经观察到 EOF 后，其回落或最终 head 再变化均为 Broken。`PEER_CLOSED` 或页内错误不是 EOF。

## 门铃与阻塞循环

会阻塞的封装在每次写入、腾出空间或发布 EOF 产生正进展后，都必须在等待或返回前通知对端；所有可与阻塞封装混用的公开读写入口遵守同一规则。只有先建立带明确排序证明的等待意图或事件代次握手，才能按空到非空、满到非满等边沿省略通知；观察环状态发生边沿本身不构成无丢唤醒证明。双方明确约定永不阻塞的纯轮询模式可以省略门铃，但不能与阻塞模式无约定混用。

对端仍为 Invited 时没有现存接收者需要唤醒，Attach 的首次检查必须发现此前发布的数据/EOF；这与对端已关闭是不同状态。门铃只提示状态改变，真实可读量和可写量始终来自控制块。

数据进度一经发布不能因后续通知失败而回滚。公开操作必须同时报告错误和已经完成的字节数，批量操作累计整次调用的进度，避免调用者重试整段造成重复数据。EOF 已发布后的通知失败同样不撤销 EOF。

所有驱动方式共用非阻塞推进与等待准备：检查控制块；无进展则 acknowledge DATA；重新检查；仍无进展才等待 DATA、PEER_CLOSED 或 CLOSED。阻塞门面使用 WaitMany，服务事件循环使用 WaitSet；醒来都从真实控制块重查。消费者只在读至空后确认 DATA，生产者只在无空间时确认并等待腾空提示。客户端跨多批等待须传同一绝对 Deadline；等待超时报告已完成字节数，不把流误判为 Broken，也不自动关闭资源。

角色独占 Endpoint，向执行框架只交付安全的观察注册能力，不导出原始 Handle 或映射访问权。建立阶段创建端两种角色都可观察内核的 PEER_ATTACHED，不能用环可读/可写量替代 Attach 事实；实际协议验证仍由 attach 方完成。Attach 错误必须明确区分 Invitation 尚未消费与已消费后协议失败，清理 owner 随错误保留。Create 在内核已发布但用户态输出校验失败时，同样须返还 Endpoint/Invitation 或异常几何清理 owner，不只返回错误码。

等待条件是三个互不相同的观察事实——可写空间出现、已发布 EOF 后对端消费至最终 head、对端映射建立——不能合并为一个布尔结果；每次唤醒后从控制块重查并返回带领域载荷的类型化结果。登记按当前等待条件选择信号：对端建立是持久电平，满足后不再重复登记；数据条件随 acknowledge→重查→再等待循环逐轮重新登记。登记寿命由执行核心拥有，角色只声明当前等待什么；业务不手工编排 acknowledge、重 arm 或 token。

首次协议或等待错误形成不可逆终态，由业务 owner 结束任务与观察关系并释放本地资源；内核资源退休不要求执行框架反复注销或重试 Close。生产者的“已发布 EOF 且对端已消费全部字节”可以作为上层完成的一个前置事实；后端写入、持久性、最终状态和取消仍由上层控制协议决定。Runnel 不把传输 EOF 或 Endpoint 关闭转换成文件操作成功。

## 分工

| 关注点 | 归属 |
|---|---|
| 多页 backing、映射、端点、邀请、Attach 状态与关闭 | Tunnel |
| DATA、WaitMany、WaitSet 与门铃调用 | 对象状态和等待机制 |
| 布局、角色、游标、内存序、EOF 与 Broken | Runnel |
| 记录、region 注册、descriptor 与 buffer 交接 | BufferQueue |
| 连接身份鉴权与请求语义 | 上层服务协议 |


---

## buffer-queue.md

# BufferQueue

BufferQueue 是运行在 [Tunnel](../../notes/ideas/tunnel.md) 上、与 [Runnel](../../notes/ideas/runnel.md) 并列的单向记录与缓冲交接协议。它面向块 I/O、网络包和其它已经具有记录边界的数据，不替代字节流。控制面通过 Mailbox 运输 MemoryObject capability；共享区只保存稳定的 region 引用和固定宽 descriptor，绝不保存进程本地 Handle、虚拟地址或裸指针。

## Region 注册

每端维护只存在于本进程的 region table。注册由控制面完成：一方通过 Mailbox TRANSIT 一个具适当 MAP/READ/WRITE rights 的 MemoryObject Handle，并携带候选 region id、generation、对象长度和协议权限；接收方验证、映射并显式应答后，该 `(region, generation)` 才能出现在 descriptor 中。Handle 数值和本地映射地址不进入共享区。

region id 允许复用，但 generation 必须变化。注销先停止新提交，等待所有引用该 generation 的 descriptor 完成或被协议终止，再由双方确认后解除映射和关闭 Handle；任何陈旧 generation、越界 offset/length 或未完成注册都使队列 Broken。关闭 Tunnel 终止整条 queue，不把页内注销标志当作 backing 回收 authority。

## Submit 与 completion

一条 BufferQueue 包含两个有界 SPSC 定长元素环：submit ring 由提交方写、处理方读；completion ring 由处理方写、提交方读。共享 header 固定声明版本、总长度、两个环的 offset/element size/capacity 与 feature 位；每个游标有唯一写者，发布、取得、shadow 校验、门铃与 Broken 遵守共享内存公共契约。

首版 descriptor 至少表达：

```text
region:u32 + generation:u32 + offset:u64 + length:u64 + cookie:u64 + flags:u32 + reserved
```

`offset + length` 必须无溢出且位于本地已注册对象范围；`cookie` 只由上层关联请求，不构成 authority。一个 descriptor 引用单个连续对象子范围。scatter/gather 由有硬段数上限的一组 descriptor 表达，组的开始、结束与失败原子性由具体协议版本明确；首版实现可以只开放单段而不改变 region seam。

submit 成功发布前，buffer 属提交方；处理方 acquire 接受合法 descriptor 后取得协议所有权，直到对应 completion release 发布；提交方 acquire completion 后才重新使用或注销该范围。completion 必须回显足以拒绝陈旧或伪造完成的 region generation、cookie 与结果长度。submit ring 满和 completion ring 满都是普通背压，不允许覆盖未消费项。

处理方只有在已经为该 submit 保留一个 completion 槽后，才算 acquire 接受 descriptor 并取得 buffer 的协议所有权；若没有 completion 容量，必须停止消费 submit 并施加背压。completion reservation 与已接受 descriptor 一一对应，不能因处理失败、取消或 peer 行为而丢弃。初始协议令 completion capacity 不小于最大在途 submit 数，避免以隐藏队列补偿环容量。

## 所有权边界

BufferQueue 的 ownership 是双方协议承诺，不是内核撤销：只要两端仍持 RW mapping，恶意或损坏进程仍能越权访问 buffer。安全封装以 affine 用户态类型阻止本端正常代码在在途期间访问，并把对端违约转为 Broken，但不能宣称获得硬件强制独占。

需要 CPU mapping 与设备 DMA 之间的强制排他、IOMMU 更新、cache maintenance、pin、撤销或驱动崩溃回收时，必须使用设备资源设计提供的 DMA 对象和 lease。普通 MemoryObject、Tunnel 或 Rust move 不能替代这些硬件契约，也不为“帧移交”增加第二套内核 syscall。

## 门铃与失败

双方只在从无进展变为可能进展的关键转换上通知：submit 从空变非空、completion 从空变非空，以及对端可能因腾出环槽继续。门铃可合并且不携带 descriptor 数量；等待者确认 DATA 后重查两个环，再进入 WaitMany。未知必需 feature、共享几何改变、游标非法前进、descriptor 越界、completion 不匹配或 peer close 都使本端不可逆 Broken。

## 边界

内核不创建 BufferQueue 对象，不解析 descriptor，不登记 region table，也不保证协议 ownership。Tunnel 只提供共享控制区、门铃与生命周期，MemoryObject 只提供 backing identity、映射和 seal，Mailbox 只运输注册 capability。三者组合后，BufferQueue 在用户态隐藏环布局、注册状态、背压和交接规则；删除该模块会让这些复杂度散回每个驱动和服务调用者，因此它是独立协议模块，不是 Runnel 的配置项。


---

## framework.md

# 用户态库与服务框架

用户态按知识领域组合能力。库提供机制与契约，capability 提供实际授权，可执行程序选择语言环境和业务角色。通用知识不依赖某个服务或具体 runtime，领域政策不反向进入基础库。

## 内核契约、领域库与可替换运行时

### 知识 owner 与组合关系

| 领域 | 拥有的知识 |
|---|---|
| shared ABI | 内核与用户态共用的固定宽契约 |
| libkernel | 内核调用、安全资源持有、释放与完成观察 |
| 进程库 | 启动上下文、装载、组装和监督政策组合 |
| 线程库 | 用户栈、线程语言环境与结果交付 |
| Rust runtime | 程序入口、allocator、语言错误与终止政策 |
| 通用执行与 RPC | 业务任务、等待组合、请求与回复责任 |
| 各领域库 | 文件、服务、设备及其业务语义 |

### 依赖图与 Cargo 边界

依赖从知识使用者指向提供者。runtime 可以组合进程、线程和内核接口；公共领域库不通过 runtime 取得这些知识。crate 边界根据共同变化的契约确定，不为转发类型增加门面层。

### 启动上下文与入口方案

启动能力以一次性交接进入程序环境；身份与 payload 的描述不产生额外 authority。入口采用、装载与初始资源关系由 [bootstrap](../../notes/ideas/bootstrap.md) 拥有。

### 线程环境与 ThreadRegistry 方案

线程执行、用户栈、语言环境和结果各有对应寿命。运行时可以组合线程库的资源管理，但不复制其账本。用户线程资源关系由 [task](../../notes/ideas/task.md#用户态线程资源收束) 拥有，具体登记容器并非跨层契约。

### 消息、Notification 与运行时事件

普通程序可以直接使用 IPC 和等待原语；RPC 增加请求/回复关系，事件循环组合业务推进。可用 API 不意味着每个程序自动创建接收器、服务连接或后台线程。

### allocator、终止与平台边界

领域库使用语言分配接口，所选 runtime 负责 allocator 组合。启动和内存扩充所需的底层操作不依赖尚未完成初始化的语言环境。

程序环境解释入口返回、panic 和 OOM。内核负责已决定终止的进程资源收束；语言层不执行 ProcessDrain，也不负责内核退休事务的持续推进。

### 库知识与运行角色

链接进程、驱动或服务库不授予对应权限，也不自动启动管理活动。声明、发现、授权和实际激活是不同关系，由显式装配组合。

## 通用执行与服务组合

执行机制拥有业务任务、任务就绪与等待来源的组合，不理解文件节点、注册记录或设备种类。领域状态拥有者推进业务；长期服务通过公平执行共享线程或组合多个执行上下文。

资源 Close 是必成且不隐式等待的本地释放，执行循环不为析构任意 owner 另建退休线程、定时重试或内核维护阶段。任务停止、协议取消和业务完成仍由各领域的操作 owner 承担。

等待登记有明确寿命和稳定身份，迟到结果不能触及复用的任务。用户态取消消费关系与内核订阅的实际退休分别表达；服务无需反复调用才能使已经放弃的内核登记被清理。

付款账户、操作授权和执行者分别拥有事实。资源退款跟随真实资源结束，不能因任务从就绪表中删除就提前退还。通用记账不枚举某个业务领域的资源类型。

监督者决定服务终止、重启和故障升级，观察内核的明确终态。已终止进程的 Handle 和页表回收不是监督任务的业务工作。

## FAL 与文件客户端

FAL 表达目录授权、对象访问与业务结果；文件客户端组合进程 namespace 和跨 provider 访问。服务发现使用这些基础能力，但其注册状态与政策归服务领域。

协议、后端与宿主业务分别拥有自己的变化。公共 provider 不依赖具体注册服务，服务代码不复制通用运输和资源释放。目标关系由 [oal](../../notes/ideas/oal.md)、[fs](../../notes/ideas/fs.md) 和 [service](../../notes/ideas/service.md) 拥有。

## Runnel 与驱动框架

共享数据运输与业务完成分别表达。流关闭不冒充文件写入成功，数据面状态也不代替业务最终结果。Runnel 与上层流协议按各自契约组合。

设备、驱动、匹配与接管有各自 authority 和资源来源。驱动框架组合内核资源与业务执行，不向通用执行或内核对象层注入设备政策。模块关系由 [device](../../notes/ideas/device.md) 拥有。


---

## wait.md

# 等待与持久观察

对象公开可观察状态，等待把这些状态与线程或业务执行联系起来。一次等待和持久观察集合复用同一来源事实；它们不授予被观察对象的业务操作权。

## ObjectSignals

可等待条件表达当前资源或操作的事实，例如消息可读、通知存在、对端状态和任务完成。观察不消费业务资源，醒来后由对应操作重新核验可用性。

已经获得的就绪事实与来源后续状态变化分别表达，迟到交付不凭最新状态伪造过去的快照。具体条件与完成边界由对象领域拥有。

## WaitMany

一次等待组合有限的对象条件与绝对期限，产生一次结果。等待持有必要的来源关系，结果可以由就绪、关闭或期限决定；用户线程终止时由内核收束已经接受的登记。

解析后的观察不依赖原 Handle 槽继续存在。观察关系是否延长某种业务使用，由该使用契约明确规定，不由临时存储引用偶然决定。

## WaitSet

持久观察集合汇集长期来源，向服务提供有界批次的就绪记录。登记身份与业务任务身份分开，旧记录不能误命中新登记或已经复用的任务。

集合拥有订阅与结果承载，来源仍拥有自己的状态。一次性等待与持久登记不各自复制来源状态机。

## WaitSet 的收束

放弃登记或关闭集合后，调用者不再拥有相应消费关系；内核接管订阅和在途交付的收束。普通 Close 使用 [object](../../notes/ideas/object.md) 的必成本地释放语义，不等待整个集合退休，也不让用户维护注销重试。

物理退休不等待观察者运行或取走通知，避免相互观察成为清理依赖环。确有独立用途的完成屏障单独表达，不附着于每次析构。

## 安装、完成与取消

接受观察前备齐所需承载；登记、就绪和取消具有唯一仲裁关系。结果交付与内部来源退休由各自 owner 承担，原调用者消失不会遗失其中任何一项。

宽度增长的通知和退休通过 [kernel](../../notes/ideas/kernel.md) 的有界维护推进。用户侧执行框架只组合任务与结果，不驱动内核订阅的内部清理阶段。

## mm.md 清理前线稿

# 内存模型

内存管理同时维护物理资源所有权、地址空间隔离和映射生命周期。地址空间区域账本是映射关系的唯一真值，页表只是它的硬件投影；物理库存、backing、虚拟区域、对象 view 与借出 lease 必须各有唯一所有者。所有调用者通过同一个地址空间深模块提交意图，不各自组合帧、PTE、回滚和地址翻译同步。

## 物理内存

物理帧是库存资源，以 affine extent 所有权表达。平台交付的内存范围不是天然可用集合：固件与设备保留区、内核永久占用和仍被启动环境使用的区间必须先统一规范化并扣除，重叠、越界或来源矛盾应使启动失败。其余供给在任何普通分配发生前划为互不借用的系统储备与用户供给；系统储备用于内核堆、完成路径和恢复所需的固定资源，不能在压力下退化为用户池的隐式透支。

系统储备不是与用户库存共享同一分配器后附加的额度标签。用户 FramePool 只发布 user supply；系统页在发布前由 `SystemSupply` 以用途类型接管，FramePool metadata、内核 heap chunk 与 recovery ticket 不能互相伪造或转成用户 extent。库存 metadata 按实际 RAM 几何精确计算；heap ticket 数是编译期物理容量政策，耗尽时普通 metadata admission 失败，不能借 user supply，也不冒充按对象精确计费。recovery ticket 数必须由 Commit 后路径的静态并发上界导出；没有这类消费者时可以为零，新增消费者必须先扩充预算。平台供给无法满足完整子预算时 fail closed，不按内存比例缩减正确性承诺。

内核堆、用户物理库存和进程用户堆是三个独立层次。内核堆只承载 metadata 与对象壳，由预清零的固定连续 heap tickets 单向扩展，可以针对小对象、固定物理容量与失败可预测性优化；对象级存量与多租户隔离由正交的 metadata admission 负责，不能从 heap 容量反推。用户 FramePool 只做 MemoryPool-backed extent 的物理取得，可以针对 order、碎片、锁外清零与 funding 事务优化；进程在取得 funded anonymous mapping 后由用户态 allocator 自行细分虚拟区间，可以按服务 workload 选择 arena、slab 或其它策略。三者不共享配额、回退路径或 allocator 政策，只在物理页唯一所有权上闭合。

平台保留内存不是同一种生命周期的区间集合。静态专用区永久排除普通供给；`no-map` 同时从内核标准直映射的 admitted ranges 中扣除，禁止无 owner 控制的虚拟映射与推测访问；动态保留请求必须在普通供给发布前按大小、对齐和允许范围整体放置；`reusable` 区始终属于平台 region owner，只能以可撤回 loan 暂借给可丢弃、可重建或可迁移的 backing。不能闭合动态放置或 reclaim 协议的平台描述必须明确拒绝，不能把未知语义降级为普通永久 reservation。

内核直映射由规范化 admitted ranges 定义，不由固定大页清单定义。页表构造器对每段连续范围 eager 建表并自动选择硬件允许的最大叶，洞与边界才下沉到更小粒度；启动所需中间表来自与用户供给隔离的固定系统预算，预算不足是平台 admission 失败。平台名称、模拟器行为或自备 DTS 只提供输入事实，不改变该契约。

用户供给中的空闲库存不能依赖由自身供血的通用堆。启动阶段尚未回投但未来属于用户供给的区间以 boot-held token 保留；运行期 claim、funded extent 与 returning extent 同样各有唯一 affine 所有者。任一时刻的物理状态集合互斥，静止点满足：

```text
user_supply = free_inventory + boot_held + funded_extents
```

事务窗口可以暂时出现 claimed-but-unfunded 与 returning 状态，但二者必须由同一 funding 事务拥有，不能同时出现在其它桶中；只有冻结新事务并排空已提交 retire 后才能作全局供给断言。root MemoryPool 的固定总额度恰等于 user supply，因而页数上不超配；系统储备、永久页和设备专用内存均不进入该额度。

库存只决定哪些帧已经从空闲集合原子取走，不在库存锁内完成与页数线性的内容初始化。claim 成功后，调用者独占 extent，先在库存锁外清零，再把“已初始化 backing”发布给页表或对象；初始化失败或事务放弃仍由 affine funding 自动归还。单次可清零页数、extent 数、PTE 步数和其它不可中断工作各有独立硬上限，平台测量只用于选择常量，不替代静态工作边界。

内核 heap source 不在 allocator 忙碌期间进入用户 FramePool 或执行大块清零。启动期 planner 预留并清零固定粒度 heap chunks；运行期扩堆只从专用 ticket 序列做 O(1) 单向消费。recovery tickets 独立保留，不能因普通 heap 压力自动解锁；具体完成路径只有在取得对应 typed permit 后才能消费。

精确长度不要求物理连续。普通用户 backing 由若干 extent 组成；需要连续物理区间的页表、DMA 或其他消费者必须显式请求受 order 上限约束的连续块。extent token 不允许复制或任意构造；切割消费原 token 并返回互不重叠的剩余 token，合并只接受物理相邻且所有权语义相同的 token。失败回滚、延迟 retire 与正常归还遵守同一数量守恒。

资源持有的关闭与 backing 的实际退休分开。容器和大对象的释放由内核拥有稳定进度，不把任意宽度遍历放进 Close 或最终引用析构；容量由资源模型约束，不以维持同步析构为理由强迫上层组合许多小对象。

## 单一地址翻译模式

统一内核运行环境只选择一个所有 admitted hart 都支持的地址翻译模式。共享物理内存配多套模式会复制内核映射、割裂线程迁移和回收契约，却不增加隔离能力；若不存在共同支持模式，启动应明确失败。

内核栈容量与保护洞跨度属于部署容量选择，不是内核范式或平台身份的固定约束。容量应覆盖合理调用链并以明确的物理/虚拟资源成本论证；栈帧审计从实际保护布局推导，不能另设无独立依据的小阈值。需求触及旧限额时先重校容量与布局，不为验收平台的任意小栈扭曲通用机制；保护洞仍须防止大帧跨越到其它映射，不能用提高审计阈值绕过这一正确性义务。

内核映射在所有用户地址空间中保持一致，使用户 trap 可以直接进入共同入口。用户半区属于进程，内核半区属于系统；共享内核页表子树与地址空间拥有的用户子树必须由显式所有权记录区分，建立、借用和收束经过同一地址空间模块，不能依赖创建与 Drop 调用点的手工配对知识。

## AddressSpace 深模块

每个进程从创建起拥有一个稳定 AddressSpace 身份，但 Building 空壳的地址空间最初可以不持有页额度、区域账本或页表。AddressSpace 以一次性状态转换表达资源附入：

```text
AddressSpace
├── Unbound
└── Bound
    ├── PoolBinding
    ├── RegionLedger       虚拟区域、权限、owner、AllocationKey 与 RegionKey
    ├── Backing            OwnedExtents 或 ObjectView
    ├── TranslationTree    PTE 与页表子树所有权
    ├── MemoryChange       validate → reserve → commit → publish → synchronize → retire
    ├── EpochState         translation/instruction epoch 与待确认义务
    └── DrainState         区域、事务与页表的可恢复收束进度
```

`ProcessBindMemory` 是 Building authority 下唯一的 Unbound → Bound 操作：它消费具绑定 authority 的 MemoryPool Handle，先以 Building operation lease 冻结提交资格并 pin 源 entry，再在锁外准备不可转移的 PoolBinding 与根页表。发布时复检 lease 仍有效且 AddressSpace 仍为 Unbound；期间发生的终止截止不能撤销已经登记的资格，若目标已进入 Terminating，成功发布的 Bound 状态直接成为收束方接管的资源。Bound 发布同时把 pinned entry 标记为逻辑已消费，是唯一提交点；随后摘槽和释放 pin 只是不可失败尾段，不能向调用者恢复 Handle。提交前失败保持 Pool Handle 和空壳不变；提交后绑定不可替换、转移或重新计费。未绑定时 Map、Write、线程附入和 Start 均不成立，Handle grant 等不依赖地址空间的组装动作仍可独立完成。bootstrap 对 initial process 使用同一内部绑定语义，不形成第二种地址空间创建机制。

AddressSpace 的外部 interface 只接受绑定、映射变更意图、调用 authority 与有界 drain，不向 Building 组装、Running syscall、bootstrap 或对象 lease 暴露区域容器、extent 排列、页表步骤、epoch 或 Remote Call 请求。Unbound drain 是空操作；Bound 地址空间的全部普通释放必须经可恢复 drain，最后析构只能验证树已清空并做常数工作，不能递归替代收束。

Building 与 Running 只在 authority、允许的来源和生命周期阶段上不同，共用 Bound 地址空间中的同一变更机制。Tunnel 等对象以内部 lease authority 建立和撤销 view。新的映射来源或调用通道必须进入这个 interface，不能旁路建立 PTE 或另建地址占用表。

地址空间身份独立于可变账本锁；Bound 后拥有稳定 translation epoch。active-hart 成员关系的唯一真值仍属于进程执行模型；AddressSpace 通过固定内部 interface 取得发布时快照，并只拥有由该快照产生的 epoch 义务与完成确认，不维护第二份成员集合。Process 的短 execution gate 同时线性化 Building 组装准入、Start、enter/leave、Running 事务 Commit 与终止截止。Building operation 只可在精确 Building 状态登记，登记即冻结该操作的提交资格；Start 只能在除自身外没有组装操作在途时发布 Running，终止可以先发布截止但必须接管并等待已经登记的操作完成。Running 地址空间事务则以 Commit 为胜负点：终止先线性化时未 Commit 事务回滚，Commit 先线性化时终止路径接管该必成事务。

Commit 与终止竞争共享的 execution gate，其嵌套顺序（AddressSpace commit lock 在外、gate 在内，终止不反向嵌套）由[任务模型](../../notes/ideas/task.md)唯一拥有。MemoryPool、MemoryObject 与 AddressSpace 的状态锁之间不跨调用长期持有：额度、WritePermit 和其它 affine token 在各自 owner 锁内预留后转入 PreparedChange，rollback/retire 先从 AddressSpace 摘出 token，解锁后再回到来源对象推进计数。短锁只保护账本、状态和有硬上限的发布；帧清零、用户态工作和远端确认等待均不得持锁。

## 区域账本

区域按虚拟地址排序。空闲洞不入账；guard 等 reservation 入账但不安装 PTE；mapping 同时记录虚拟区间、backing view、对象内偏移、当前权限、权限上限和解除所有者。任一稳态有效用户 PTE 必须恰由一段 mapping 覆盖，reservation 范围内不得存在有效 PTE。

地址空间拥有普通 mapping 与 reservation。对象可以借出由内部 lease 拥有的 mapping；这类区域仍进入同一账本参与冲突检测，但普通内存操作不能替换、切割或解除，只有对应 lease 可以撤销。地址空间最终收束前，全部 lease 必须先关闭或被进程收束接管。

部分解除在请求边界切分区域。匿名 mapping 的 OwnedExtents 随区域切分；解除的中段转入事务 retire 所有权，只有相关 hart 确认旧翻译失效后才归还库存。对象 mapping 只切分 ObjectView 并调整对象内偏移，不改变对象对 backing 的所有权。账本决定以上关系，PTE 的存在或解除本身不决定帧归谁。

一次成功 Map 建立一个内部 `ReservationGroup`，以不可复用的 `AllocationKey` 标记共同来源；完整 reservation 的 guard 与 mapping 都引用该组。每个当前 ledger fragment 另有唯一 `RegionKey`。切割消费旧 fragment 及其 key，为存活左右片段和 retiring 中段分别铸造新 key；`AllocationKey` 保持不变，直到最后一个 ledger 或 retire fragment 消散。ReservationGroup 不保存第二份占用区间，也不授予按组操作的隐式 authority；区域账本中的当前 fragment 集合始终是范围真值。

普通 `MemoryUnmap` 只作用于请求区间，不因命中某个 AllocationKey 而解除区间外同组 fragment。解除完整 reservation 会同时移除 guard 与 mapping；只解除 usable mapping 会留下两侧 reservation-only fragment；只解除 guard 或 mapping 中段也只切割对应范围。请求跨空洞、越界或不同解除 owner 时整笔失败。对象 lease-owned fragment 仍只能由 lease authority 撤销。

ledger 只在 owner、区域种类、AllocationKey、backing identity/连续 offset、当前与最大权限全部相同且几何相邻时兼容合并；合并同样消费旧 RegionKey 并铸造新 key。不同 AllocationKey 的相邻区域保持独立，不能为了节省节点丢失分组来源。

## Backing

匿名 mapping 是最常见路径，其私有 OwnedExtents 由 mapping 直接拥有，不为每次分配产生用户可见 Handle。区域切割消费原 affine backing，存活片段与 retiring 中段各自取得互不重叠的 extent 和同源 charge；retiring slice 在地址翻译确认前不得归还。需要共享或独立于某个地址空间存活的字节才使用 MemoryObject。

MemoryObject 是固定长度的共享 backing identity，不是地址空间、虚拟区域或用户态分配器。Handle 的 `MAP` right 授权建立新 view，`READ`、`WRITE` 与 `EXECUTE` 分别约束 view 可请求的数据和执行权限；mapping 建立后以 ObjectView 强引用独立保活对象。对象 backing 始终唯一拥有完整 extents 与 charge，任一 view 的切割、降权或解除只变换 ObjectView 和 WritePermit，不切割或重复持有数据 backing。同一对象可以按不同地址和更窄权限映入多个进程。

MemoryObject 的 backing 由真实使用关系保活；本地引用关闭后，内核在安全条件达成时完成退休。对象容量与映射规模由准入和实际成本决定。resize、COW、文件缓存或 pager 具有各自的内容与缺页契约，不因本地 Close 与退休分离而自动引入。

## MemoryPool 与 backing charge

MemoryPool 是 page-backed storage 的 capability 账户，不是物理分区，也不代表全部内核内存。root pool 的固定总额度由可信 user supply 铸造；子池只转移额度，所有池继续从同一用户帧库存取得物理 extent。Pool Query 观察的是资源 authority 与当前占账，不承诺物理连续性、特定位置或某次请求能满足 extent 上限。连续 DMA、设备内存和未来 overcommit/pager 由各自契约拥有。

每个 Pool core 维护固定容量的四项守恒：

```text
total = available + reserved + allocated + delegated
```

`ChargeReservation` 从 available 预留额度，放弃时原额回滚；物理 backing 全部取得后，reservation 不可失败地提交为与 extent 同寿命的 `MemoryCharge`，计入 allocated。`MemoryCharge` 不可复制，只能在同一 pool 内随 backing 做守恒 split/merge，最终归还 allocated。派生子池把 reservation 提交为与 child core 不可分离的 `ParentCredit` 并计入 delegated；只有销毁全部额度已回到 available 的 child，才能兑回这笔 credit，任何仍可继续使用的 child 都不能与退款同时存在。普通 Handle duplicate、TRANSIT 或 GRANT 只改变同一 core 的 authority 可达性，不改变四项计数。额度不足与实际库存不足是不同失败：前者表示 quota 不足，后者表示物理或元数据资源暂不可满足。

Pool 形成单向强引用图：Handle、进程绑定与 MemoryCharge 各自保活来源 core；child 以不可分离的 ParentCredit 义务强持 parent，parent 不登记 child、backing、进程或地址空间。最后一个 child 引用消散时，只有其全部额度已回到 available 且 child 身份同步终结，才沿有界深度父链自然归还；任何计数不一致或“退款后仍可操作 child”的状态都属于内核不变量失败，不能通过错误退款扩大父池。Pool 不提供 child 枚举、关闭状态、等待电平、reparent 或通用 revoke，因此普通 close 不扫描对象图。

Derive 是不可撤销的资源 grant，不是可召回租借。父级在所有自然引用和 charge 消散前不能强制取回额度；MemoryObject 或 Pool capability 跨进程、跨 Job 转移也不重记来源。需要强制收回的未来场景必须从一开始使用具有显式成员、撤销准入和有界 drain 的 MemoryLease/资源域，不能改变普通 Pool 和既有 view 的单调授权语义。

运行期 frame-backed 资源按唯一来源支付：AddressSpace 根与中间页表由目标进程固定绑定池支付；匿名 backing 由所在进程绑定池支付；Tunnel 与 MemoryObject 的数据 backing 由创建者绑定池支付，而每端映射所需页表仍由各自进程绑定池支付。bootstrap payload 在收编为 init backing 时取得 primordial charge，释放时首次进入用户库存并归还额度；已收编 payload 属于 funded extent，不再属于 boot-held。stale translation 确认前，frame 与 charge 都不得恢复可用。

进程的 PoolBinding 只授予内部 backing 分配，不自动产生可查询、派生或运输的用户 Handle。资源管理者若需隔离预算，先派生子池，再通过 Building-only `ProcessBindMemory` 原子消费具 `GRANT` authority 的 Pool Handle；失败不消费，成功后绑定不可转移。Job 不提供默认池或第二份配额。

物理页与内核 metadata 是不同的资源维度，Pool 表达各自的付款与额度，对象、存储和已接受操作持有相应费用。用户态资源管理政策将资金与 Job、执行和设备能力组合交付。费用记账与不可信域之间的硬容量隔离是不同能力；后者的独立资源边界由[预算计划](../todo-2026-09-14-kernel-memory-budget.md)拥有。

## Map、Unmap 与 Protect

Running 进程对自己的地址空间具有固有管理权；组装者只凭 ProcessBuilder 在 Building 期操作目标地址空间。两条 authority 通道经过校验后消费同一 MemoryChange，不各自维护布局、backing 或回滚规则。

Map 以字节长度请求匿名或 MemoryObject backing，并同时声明权限、placement 与两端 guard。匿名来源由目标进程绑定池取得 backing；对象来源验证 Handle、对象内页对齐 offset、范围和 rights，不重新分配数据页。`Anywhere` 由账本选择完整空洞；`FixedEmpty` 只在指定范围完全空闲时成功。不存在隐式覆盖旧区域的 fixed 模式，替换必须显式 Unmap 后再 Map。成功结果包含可访问区间及含 guard 的完整 reservation 区间；AllocationKey 与 RegionKey 都是内核账本身份，不进入以地址区间为真值的用户 ABI。

单次请求的页数、guard 数量、区域切分数、PTE 步数和元数据增长都有共享硬上限。超出上限由用户侧在安全 interface 后分段组合；libkernel 以 affine region 表达通用映射所有权，runtime 用它承载 allocator arena，libthread 用它承载本进程用户线程栈。完整解除消费原 region；部分解除消费原 region，并按请求洞的左右两侧返回至多两个 compound fragment，每个 fragment 可以同时描述 usable mapping 与相邻 reservation-only guard。该所有权类型不是应用扩堆 interface；内核 ABI 仍以地址区间为真值。

Unmap 采用严格全覆盖语义：目标范围必须被调用者可解除的 mapping 或 reservation 完整覆盖；空洞、越界或 object-owned lease 使整笔请求在 Commit 前失败。调用者负责先停止其它线程对该区间的访问；内核保证成功返回后不存在 stale translation，不能替应用修复并发 use-after-unmap。

Protect 只能在 mapping 创建时冻结的权限上限内改变当前权限。权限只允许只读、读写与读执行，不提供 write-only 或 W+X；增加权限、收窄权限和解除映射都走同一 MemoryChange 与地址翻译同步纪律。

连续堆顶不是地址空间真值。所选 runtime 的 allocator（如 libruntime）通过 libkernel 取得并持有若干匿名 arena，向应用提供 Rust 分配器语义；公共库使用 core/alloc，不依赖该 allocator 的实现。libthread 同样消费通用 Map/Unmap 建立本进程内带 guard 的线程栈；新进程首栈的外部组装由 libprocess 经 Building 通道完成。不存在内核线程栈分配调用或固定预映射栈池。

## Guard 与故障

Guard 是占据虚拟区间但不持有 backing、也不安装 PTE 的 reservation。它阻止自动选址或 fixed mapping 占用该洞；普通 Unmap 对 guard 与 mapping 都遵守同一精确请求区间，不执行组级隐式清理。Guard fault 与普通未映射访问都属于用户程序违约并终止该进程，不升级为内核失败。

不存在按需分页时，成功 mapping 的全部页在返回前已有 backing 和有效 PTE。未来 pager 必须通过显式允许 fault 的新语义接入：账本区分可解析的缺页 mapping 与永远不可解析的 guard/空洞，既有 eager mapping 不因 pager 出现而改变。

用户内存访问经统一 uaccess seam 验证；内核不把用户指针当普通引用长期保存。一般异步调用不得在 publish 后依赖可能失败的用户写回；Map 使用下述提交前结果承诺，把固定宽写回放在不可逆边界。其它调用若确需异步写回，必须各自定义地址空间并发变化时的故障契约，不允许以内核 panic 代替。

## MemoryChange 事务

Map、Unmap、Protect 以及 object-owned lease 的建立与撤销都由 AddressSpace 内部事务完成：

```text
Validated → Reserved → Prepared → Committed → Published
          → Synchronized → Retiring → Completed
```

每个阶段都是不可互换的仿射 owner，转换消费旧状态并产出新状态；只有 Validate、Reserve、Prepare 与 Commit 前最终复检可以返回业务错误。Commit 后接口只能推进固定进度或进入下一个状态，不得分配普通 metadata、返回可恢复错误或让调用点自行猜测尚未归还的 owner。

阶段表达服务于所有权与提交资格，不要求所有层复制相同的状态机。区域规划只拥有逻辑关系，地址空间事务聚合物理投影、资源来源和完成义务；调用者提交意图与经过验证的 authority，不能分别驱动账本、页表和退款。只允许完整事务 owner 跨越阶段边界，不能把错误返回与尚未处理的资源责任拆开。

资源来源的保活必须覆盖从受保护观察到最终归还的整个窗口。计划形成时即持有其涉及对象的稳定来源引用；随后锁外取得许可、重入复检，失败仍能直接归还来源，不依赖原来的 live view 仍在账本中。多个事务可同时涉及同一对象，每笔独立持有完成责任；唯一 owner 指 affine 资源恰有一个处置者，不要求共享对象只能有一个强引用。

Validate 检查完整范围、authority、生命周期、权限与固定宽输出槽，并在 AddressSpace 锁内形成精确计划和已有来源的保活责任，不预留新增 backing 或写许可。Reserve 不是一段跨锁调用：先离开 AddressSpace，从绑定 Pool、物理库存、metadata admission 与 Remote Call 容量分别取得全部 affine reservation，再重入 AddressSpace 复检计划所依赖的代次、区域与 execution snapshot，最终组装为 Prepared change。任何锁秩低于 AddressSpace 的资源源都不得在 AddressSpace 锁内进入；复检失败必须由专用 rollback owner 收集页表 funding、backing、WritePermit、view owner 与结果义务，并在锁外统一归还，普通 Drop 不得隐含“视为成功”。与页数线性的 backing 清零在独占 extent 后、Commit 前完成。
页表模块必须把结构 preflight、资源供给与 owner 发布分离：preflight 只计算精确需求，调用方锁外提供已经资金化的表页，Publish 只把 owner 与 PTE 同时纳入 TranslationTree，不分配、不进入 Pool，也不发生普通可恢复失败。并行准备后未消费的表页以及从树中摘除的表页必须以 affine owner 显式返回，由 AddressSpace 锁外的 rollback 或 retire 路径收束；不能提交为裸帧号后再靠物理地址重建资源所有权。

Commit 是不可逆线性化点。此前任何失败都保持账本、PTE、backing 和请求槽零副作用；一旦 Commit 成功，事务归 AddressSpace 且必然完成。区域与 PTE 可以在有硬上限的 Publish 段逐项更新，未同步的其它线程在调用进行期间可能观察到旧状态或正在发布的新状态；内核不承诺任意多页在所有 hart 上瞬时切换。Synchronize 完成并向仍存活的调用者返回后，所有目标 hart 必须已经达到新 epoch，这才是对外完成边界。

Map 的 Reserve 已经唯一确定 usable 与 reservation 范围。`MemoryMapResult` 使用调用者提供的稳定结果槽，包含四个固定宽范围字段和位于末尾、自然对齐的 committed cookie；request 携带非零 cookie，调用者把 committed 初始化为零，Validate 拒绝非零初值或不满足原子对齐的槽。Reserve 从 ledger 取得覆盖该固定宽可写范围的 `UserWriteLease`，阻止其它事务在 Commit 或 rollback 前撤销、降权或替换结果 backing；lease 保存有界、已验证的写入投影，使 Commit 不必重新进入 uaccess 或 AddressSpace lock。全部可失败检查与资源预留完成后，内核先写 payload，再在 execution gate 内通过 lease 以 release store 把同一 cookie 写入 committed 字段；该 store 就是 Map 的 Commit。用户侧内核封装只有以 acquire 观察到 cookie 匹配后才可相信 payload 完整，cookie 为零时 payload 内容无语义；在 System Call 成功返回前仍不得访问新 mapping。Commit 后释放 UserWriteLease，不再执行可能失败的 uaccess。

结果槽必须位于调用前已经存在且可写的 eager mapping；UserWriteLease 只 pin 固定宽 ledger/backing 与写权限，不把用户指针变成可长期解引用的内核引用。与 lease 冲突的 Unmap/Protect 在各自 Commit 前返回 Busy。用户侧内核封装与线程 owner 必须共同保证可能 park 的 Map 结果记录保留到 join 的完成边界，不能依赖某个具体 runtime 的私有缓冲。若调用线程在 Commit 前消散，事务释放 lease 并回滚；若在 Commit 后消散，结果承诺已经属于进程，事务继续完成。线程的 departed/join 完成边界不得越过仍挂接于该线程结果记录的 committed transaction，因而接管者在 join 后观察 cookie 时也已经越过 mapping 完成边界。

Unmap/Protect 的旧 backing、旧权限或 writable-view permit 在 Synchronize 期间由事务隔离持有，不能归还库存、映射到其它地址、退出 seal 计数或被另一对象重新解释。Retire 只在全部确认后释放旧 OwnedExtents、旧 ObjectView、写 permit 与过期元数据。Retire 批次的对象来源集与容器容量必须在 Commit 前从完整计划推导并去重；同一地址空间对同一对象的 view owner 在一笔批次中至多交出一次，每笔仍持足以完成其全部许可归还的来源引用。Commit 后可以依据账本确定 live view 是否已经消散，但不能再从 live view 查询补找退款来源；其它批次先完成、最后 Handle 消散或对象全部区域移除，都不使当前义务失去 owner。一般事务状态属于地址空间而非发起线程；等待远端确认的 System Call 通过 WaitContext park，发起线程终止只消散最终返回权。进程终止冻结后拒绝新事务并接管已 Commit 的事务；只有线程、active hart、事务与对象 lease 全部完成或进入可恢复收束后，地址空间才可进入最终 drain。
容量证明覆盖资源的整个生命周期，而不只是一次创建。常驻 backing 经多次切割后可以积累碎片；每笔变更的发布资格必须同时覆盖当前存量、其它在途事务尚未兑现的增长与本次最大瞬时增长，不能用初始 funding 的 extent 上限代替。配额许可与实际承载这些 owner 的存储容量必须同时预付；内部存储方式可以替换，但提交后增长不得重新触发可失败分配。

全局对象准入上限不自动承诺等量的 heap 字节或任意未来工作集容量。当前 `Bound AddressSpace` 数量政策只表达 metadata ledger 的条件性准入；新建和普通变更在 Commit 前按具体计划预留所有可失败资源，预留失败是普通可恢复错误。Commit 后的事务、退出和 drain 必须仅消费已持有的 owner、reservation、work debt 与稳定游标并最终完成。若未来要对指定最大工作集作物理容量硬保证，必须另行定义布局上界、完成余量和连续空间/专用 arena 预留，不得从对象数量常量或 aggregate heap 观测推导。

最后一个地址翻译确认只把事务推进到 Retiring，不授予在 Remote Call 完成回调中执行任意宽度收束的许可。若 backing slice、表页或元数据的退役超过单次固定预算，事务成为显式的内核 work debt：由安全出口按游标逐批推进，未完成时由明确的 hart 唤醒所有者再次敲门，全部 owner 已在容器锁外释放后才进入 Complete、兑销 mandatory operation 并唤醒调用者。预算必须覆盖完整调用链，包括查找、容器移动、最后引用析构和通知传播；不能把一次许可归还可能引出的任意宽度通知隐藏为常数工作。具有结构硬上界的收束可作为一个固定粒度，其余工作必须显式保留进度。该机制没有后台内核线程，不绑定原发起线程，也不允许终止路径另建同步扫描旁路；进程终止只接管同一事务与同一游标。唤醒点取 Complete 而非 Synchronize 是刻意取舍：单管线单完成点让结果义务、mandatory operation 与 join 边界共用一个定义，代价是调用者返回延迟包含全部 retire 批次、随 extent 数线性增长。若实测成为瓶颈，合法的范式内优化是把调用者返回提前到 Synchronize（对外完成边界不变），retire 纯后台推进；两者都不改变 Commit 与 Synchronize 的既有语义。

## 跨 Hart 地址翻译同步

本协议依据 [RISC-V Privileged Architecture「Supervisor Memory-Management Fence Instruction」](../../references/normative/riscv-isa-v20250508/src/supervisor.adoc)、[RVWMO Memory Consistency Model 的适用范围说明](../../references/normative/riscv-isa-v20250508/src/rvwmo.adoc)、[RVWMO explanatory material「Fences」与「Explicit Synchronization」](../../references/normative/riscv-isa-v20250508/src/mm-eplan.adoc) 以及 [Zifencei「FENCE.I Instruction」](../../references/normative/riscv-isa-v20250508/src/zifencei.adoc)。RVWMO 尚未形式化页表遍历、`SFENCE.VMA` 与 `FENCE.I` 的组合，因此不能仅凭语言原子序推导完成条件；协议同时遵守 privileged architecture 给出的 data fence → IPI → remote `SFENCE.VMA` → ack 顺序。

`SFENCE.VMA` 只同步执行它的 hart。Reserve 在 Process execution gate 下取得 active 集合、成员序列与事务准入状态快照，锁外预留每个目标的 Remote Call 槽；Commit 前重新取得 gate，若成员序列或生命周期准入已变化则在零业务副作用状态下回滚或重试。复检必须同时确认将要发布的 identity/epoch 未耗尽；耗尽在任何不可逆修改前拒绝，不能先环回再定义错误。复检成功后，AddressSpace 在 gate 内完成 Map 结果 cookie、完整 PTE 宽度写入、新 translation epoch 和请求状态 release 发布，再释放 gate。随后执行保守的本地 `FENCE RW,RW` 并触发 IPI，保证 PTE、epoch 与请求槽先全局可见；IPI 只是门铃。目标 hart 以 acquire 取得请求，验证稳定 AddressSpace identity 与 epoch，执行覆盖请求的本地 `SFENCE.VMA`，若请求携带 instruction epoch 再执行 `FENCE.I`，最后以 release 确认。完成者必须以 acquire 观察每一项确认后才能 Retire 旧 backing、权限或 view permit，并在释放槽前以 release 发布空闲；后续槽预留以 acquire/CAS 取得。初始实现可以合法地全局 over-fence，范围与 ASID 优化不得削弱完成条件。

请求槽的 Pending 状态而非 IPI 边沿是真值；门铃允许合并和重复，每个 trap 安全出口都检查本 hart 的固定槽，因此已经可见的请求不会因单次门铃边沿合并而丢失。平台 admission 必须先保证 admitted hart 的 IPI 与周期性 trap 路径可用；Commit 后的门铃异常不能转成 System Call 业务错误，Remote Call 只能保留 Pending 并由后续安全点补消费。平台永久违反 admission 契约属于系统级失败，不能假装事务已完成。

发起 hart 若属于 active 快照，也必须作为普通目标执行同一套 acquire → local fence → release ack，不能因正在修改页表而隐式视为已确认；不在快照中的发起 hart 只负责发布，不产生虚构的本地义务。

| happens-before 边 | 发布侧 | 观察侧 | 所保证事实 |
|---|---|---|---|
| PTE/epoch → 请求 | 完整宽度 PTE stores；请求状态 release；IPI 前 data fence | 目标请求状态 acquire | 目标执行失效前已能观察对应 PTE 与 epoch，门铃不会越过业务发布 |
| 既有用户写入 → 失效确认 | 目标 hart 在 trap 安全点处理请求，`SFENCE.VMA`，必要时 `FENCE.I`，随后 ack release | 事务完成者读取 ack acquire | 旧 writable view 的显式写入及本地翻译失效先于 permit/backing retire |
| 全部确认 → backing 复用 | 完成者对每项确认作 acquire，再推进 Retire | 后续库存/object owner 经事务发布取得资源 | 任何可能持旧翻译的目标 hart 均已失效，旧帧才可被重新解释 |
| 请求完成 → 槽复用 | 完成者清理 payload 后以 release 发布 Empty | 新预留者以 acquire/CAS 取得槽 | 新请求不能观察旧 payload、identity 或完成引用 |
| active 快照 → enter/leave | Commit 在 execution gate 内复检成员序列并发布 epoch | enter/leave 在同一 gate 内线性化并 acquire epoch | 快照前 active hart 必须确认；快照后 enter 必须先本地同步；leave 不能逃逸既有义务 |

active-hart 集合的唯一真值仍属于 Process 执行模型，enter、leave 与 AddressSpace 快照共享上述短 execution gate。enter 在登记 active 前以 acquire 读取 epoch并完成所需本地 fence，随后在 gate 内复检 epoch 未变化才登记并获准返回用户态；若变化则退出 gate、同步并重试。Commit 复检成功后在 gate 内发布新 epoch，因此 gate 释放后的 enter 必然观察它。leave 在 gate 内只有确认自己承担的 epoch 后才清除 active。由此，在线性化上早于 Commit 快照的 enter 必在目标集合中；晚于快照的 enter 必在返回用户态前自行达到新 epoch；早于快照的 leave 已不再执行该地址空间，晚于快照的 leave 必先履行确认。

最后一个 acquire 确认推进事务，原发起线程是否仍存在不影响完成。Process 终止、hart 离场和普通 SSIP 都只能协助消费同一请求，不能以“目标将不再运行”伪造确认。地址空间 root、ASID 或稳定 identity 在全部请求与 active 关系收束前不可复用；未来若复用 ASID，必须遵守规范「ASID Usage」要求另行完成覆盖该 ASID 的 fence。

## 可执行发布

可执行内存遵守 backing 级 W^X，而不只检查单个 PTE。Running 匿名 mapping 只产生数据权限；Building 构造方可以在目标尚不可运行时经受控写入口填充最终只读或可执行 backing，ProcessStart 在首次执行前统一发布代码代次。

运行期动态代码使用 MemoryObject 的单向状态机：

```text
Mutable --SealExecutable--> Sealing --last WritePermit retired--> Executable
```

| 状态 | 允许的新操作 | 拒绝的操作 | 前进条件 |
|---|---|---|---|
| Mutable | 只读 view；取得 WritePermit 后的 writable view 或受控直接写 | executable view | `SealExecutable` 在线性化点改为 Sealing；若 permit 为零可同点进入 Executable |
| Sealing | 只读 view；既有 writable view 的撤销/降权 | 新 WritePermit、直接写、executable view、回到 Mutable | 最后一个 WritePermit 完成 retire |
| Executable | 只读或读执行 view | 任意写入口、WritePermit、回到 Mutable/Sealing | 终态 |

Mutable 允许只读 view，并允许在对象锁内取得 `WritePermit` 后建立或重新启用 writable view；不允许 executable view。`SealExecutable` 要求对象定义的管理 authority，并与 WritePermit 预留在同一对象锁上线性化：先取得 permit 的变更计入 seal 等待，先进入 Sealing 的对象拒绝新 writable view、重新加写权限和直接写入口。Sealing 允许既有 writable view 继续存在直至其 owner 显式撤销，也允许通过 Unmap/Protect 把既有写授权切分或收窄；这种后继 permit 只延续原区域内的写范围，不是新的写授权。Sealing 仍拒绝新 writable view、把原只读范围重新加写或扩大写范围，状态不可回退；Executable 永久拒绝全部写入口与 writable permit，只允许只读或读执行 view。普通 Protect 不能把读写 mapping 转成读执行，也不能重新打开已发布对象。

WritePermit 的计数覆盖 reserved、published 和 retiring 三个阶段。Map/Protect 在 Commit 前放弃 permit 可以直接回滚；Commit 后移除 W 权限或 Unmap 时，permit 随旧 view 进入 retire，只有 active-hart 快照全部完成 `SFENCE.VMA` 并以 acquire 收齐确认后才退出计数。最后一个 permit 的 retire 负责把 Sealing 单向推进到 Executable 并发布对象的 `EXECUTABLE` 电平（见下）；seal 发起线程消散不撤销已发布状态转换。RX view 只能在观察 Executable 后建立，并为可能执行该代码代次的 hart 推进 instruction epoch 与 `FENCE.I`。

状态机不遍历 view。对象只维护带溢出检查的 permit 计数；seal 完成不设专用等待槽，而是以 ObjectSignals 的 `EXECUTABLE` 电平位表达：进入 Executable 后持续为真，持 WAIT 的 Handle 经 WaitMany 观察该位，任意数量等待者复用通用等待面，无容量上限、无排队政策，重复 SealExecutable 在 Executable 上幂等成功。该形态以等待面的结构性容量取代按对象记账的完成槽，不改变「seal 状态属于对象、发起者消散后继续、最后一个 permit retire 自动完成」的契约。受控直接写同样取得临时 WritePermit，数据写入结束并完成必要的 release 后才归还。

只读 view 不阻塞 seal，已有 writable 最大权限但当前不含 W 的 view也不持 permit；它日后尝试重新加 W 时必须重新取得 permit，因此在 Sealing/Executable 下失败。新代码版本通过新对象、新代次和用户态引用切换表达，不原地修改已发布代码。Handle 消散不撤销 view 或 seal；view 与 pending seal 保留真实使用责任，backing 通过内核退休关系安全释放。

具体帧库存、页表、内核高半区、用户布局和当前调用面见 [`../impls/mm.md`](../../notes/impls/mm.md)。
