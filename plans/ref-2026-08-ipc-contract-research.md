# IPC 契约外部调研（seL4 / Zircon / Mach / QNX / L4 与信号投递模型）

> 用途：为 eRhino 的 IPC 契约设计（message 邮箱 vs 会合、signal 排队 vs 掩码、tunnel 共享帧所有权三个决策点）提供外部取证。属 `ref-` 对照资料，只记录成熟系统的实际契约与立场，**不代行本项目的设计决策**——映射要点给出各决策点的事实与选项，结论待用户拍板。
> 证据纪律：优先官方文档/源码/论文，逐条注明来源；二手博客仅作交叉印证，不做主证据。seL4 以官方 Manual 与 tutorial 为准，Zircon 以 fuchsia.dev 官方文档为准，QNX 以 qnx.com 官方开发者文档为准，Mach 以 XNU 源码与 GNU Mach 参考手册为准。
> 一句话总览：**成熟系统的分裂点不在"同步还是异步"，而在「控制面（小消息）与数据面（大块数据）分轨」**——seL4/L4/QNX 以同步会合承载小消息 + 共享内存承载数据；Zircon 以内核缓冲邮箱承载小消息 + VMO 承载数据；Mach 试图用单一 port 邮箱扛下一切而付出复杂与性能代价；信号/通知在微内核生态中已收敛为「粘滞位掩码 + 唤醒」，投递在同步检查点而非抢占注入。

---

## 0. 三个决策点与各系统立场速览

| 决策点 | Mach | Zircon | seL4 / L4 | QNX |
|---|---|---|---|---|
| 消息模型 | 端口邮箱（内核缓冲队列，满时发送阻塞） | 邮箱（内核缓冲，异步双向，上限 64KiB/64 handles，满走异常） | 同步会合（无缓冲、双方就绪才传递） | 同步 send-receive-reply 为主 + pulse 异步补充 |
| 通知/信号 | 通知走消息（port dead notification 等） | 信号 = 对象状态掩码（level），非排队 | notification = 粘滞位掩码（机器字宽的二进制信号量数组）+ 唤醒 | 异步事件走 pulse/事件投递；POSIX 信号经 pulse 实现 |
| 大数据/共享 | OOL 内存描述符（拷贝或 COW） | VMO + channel 传 handle 转移所有权 | 帧能力随消息转移（cap transfer）+ 共享映射 | 共享内存 + 消息传递地址 |

各节细述如下。

---

## 1. seL4

### 1.1 同步 IPC：会合（rendezvous）模型

**会合语义**：seL4 的 IPC 是双方必须同时就绪的同步会合。endpoint 是内核对象，维护两个等待队列——等待发送的线程与等待接收的线程。发送方执行 `seL4_Send` 会阻塞直到消息被另一线程接收；接收方执行 `seL4_Recv` 会阻塞直到有发送方到达。若 n 个线程阻塞在某 endpoint 上等待接收，n 个发送方各发一条，则全部被唤醒并收到消息；第 n+1 个发送方则进入等待发送队列。非阻塞变体 `seL4_NBSend`/`seL4_NBRecv` 做轮询式发送/接收：仅在对方已就绪时成功，否则立即返回失败（且 `NBSend` 不返回是否送达的结果，以避免形成反通道）。

[来源: seL4 IPC Tutorial — docs.sel4.systems/Tutorials/ipc](https://docs.sel4.systems/Tutorials/ipc)；[seL4 Reference Manual §4 IPC — github.com/seL4/seL4/blob/master/manual/parts/ipc.tex](https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex)

**call = send+recv 原子组合，但阻塞在 reply capability 上**：`seL4_Call` 实质是 `Send` + `Recv` 的组合，唯一重大区别是：其接收阶段不是阻塞在 endpoint 上，而是阻塞在一个内核生成的一次性 **reply capability** 上。reply capability 存放在接收方（服务端）线程的 TCB 中；服务端用 `seL4_Reply` 调用该能力把回复发给客户端并唤醒它。`seL4_ReplyRecv` 合并两件事：先回复当前请求者、再阻塞在指定 endpoint 上接收下一条——这是服务端循环的惯用形态（`info = seL4_ReplyRecv(endpoint, info, &sender)`）。

reply capability 单槽限制了服务端同时挂起的请求数：若服务端需要在硬件操作完成后才回复（异步服务场景），可用 `seL4_CNode_SaveCaller` 把 reply capability 保存到 CNode 空槽中，从而同时持有多个未决请求。这是 seL4 在裸原语层为「客户端请求 → 服务端稍后回复」提供的唯一结构支持。

[来源: seL4 IPC Tutorial（reply capability / SaveCaller 段）](https://docs.sel4.systems/Tutorials/ipc)

**消息传递：IPC buffer + 消息寄存器（MR）**：每线程有一个 IPC buffer，包含消息负载（数据 + 能力）。消息寄存器（MR）是 IPC buffer 中一块有界的字数组，每寄存器一机器字；消息最大长度由 `seL4_MsgMaxLength` 给出。小消息直接走 CPU 寄存器（`seL4_FastMessageRegisters` 个字），**无需拷贝**；超过寄存器容量的部分才由内核在收发双方 IPC buffer 间拷贝（有界）。消息描述由单字 `seL4_MessageInfo_t` 编码：`length`（使用的寄存器数）、`extraCaps`（消息携带的能力数）、`capsUnwrapped`（被解包的能力位图）、`label`（内核原样传递的标签字段）。

**Cap transfer（能力随消息转移）**：消息可携带能力（发送方在 IPC buffer 的 caps 区填入能力并置 `extraCaps`；接收方需先指定接收路径 `receiveCNode`/`receiveIndex`/`receiveDepth`）。接收方一次只能接收一个能力（多个能力需逐个约定协议）。接收到的能力的访问权限继承接收方对该 endpoint 的权利——能力转移不是无条件放权。

**Badge（徽标）**：endpoint 能力可用 `seL4_CNode_Mint` 打徽标，多客户端共享一个 endpoint 时靠徽标区分发送者。发送方用带徽标的能力发消息，内核把徽标注入接收方的 `badge` 字段。另有「能力解包」机制：若消息中第 n 个能力恰指发送所用的 endpoint 本身，内核不传能力而把其徽标放入接收方 IPC buffer 第 n 槽并置 `capsUnwrapped` 对应位——服务端借此廉价识别「哪个客户端」。

**fastpath**：内核为 IPC 保留高度优化的快速路径，条件是：使用 `seL4_Call` 或 `seL4_ReplyRecv`、消息装入快速寄存器、双方地址空间有效、无能力转移、且被唤醒线程之上无更高优先级可运行线程。注意 fastpath 要求「调用-回复」模式而非裸 send/recv——seL4 官方明言 IPC 是微内核系统服务与客户端间沟通的核心机制，因此必须有快路径。

[来源: seL4 IPC Tutorial（IPC Buffer / Cap transfer / Message Info / Badges / Fastpath 各段）](https://docs.sel4.systems/Tutorials/ipc)

### 1.2 异步：notification（通知对象）

notification 是 seL4 的异步机制，是一个**数据字 + 等待 TCB 队列**的内核对象。数据字本质是「一组二进制信号量数组」：各发送方用不同位，接收方观察哪些位被置位。notification 有三态：`Idle`（无等待者且无已信号数据）、`Active`（已被信号、数据字非零）、`Waiting`（有 TCB 排队等待）。

- `seL4_Signal`：Idle 态 → 数据字设为所用能力的徽标、转 Active；Active 态 → 徽标按位或入数据字；Waiting 态 → 唤醒队首 TCB 并把徽标交给它，队列空则转 Idle。可见**位是粘滞的**（多次信号 OR 合并），等待者消费时清空。
- `seL4_Wait`：Idle 态 → TCB 入队转 Waiting；Active 态 → 取走数据字、字清零、转 Idle；Waiting 态 → 追加队尾。
- `seL4_Poll`：非阻塞版 Wait，无论状态立即返回。

notification 与 TCB 可绑定（bound notification），使线程可同时等待信号与 IPC；中断投递也走 notification——设备中断把信号注入 notification，用户态处理程序 wait 于其上。徽标宽度 = 机器字（32 位架构 32 位、64 位架构 64 位）。

[来源: seL4 Notifications Tutorial — docs.sel4.systems/Tutorials/notifications](https://docs.sel4.systems/Tutorials/notifications)；[seL4 API Reference](https://docs.sel4.systems/projects/sel4/api-doc.html)

要点：seL4 的「同步 IPC + 异步 notification」是**双轨**——数据靠会合传递（有界、无缓冲、确定），事件/中断靠粘滞位掩码 + 唤醒。数据面与通知面分离，正是本项目 notes 里 message（控制面）与 signal/通知（事件面）分设的同类结构。

### 1.3 为何在 IPC 之上又建 RPC 框架（microkit / CAmkES）

seL4 裸原语是「主动发送方 + 被动接收方」的会合：客户端（主动方）发起，服务端（被动方）阻塞在 recv 上等请求。这个形态对请求-响应足够，但存在多个现实缺口，官方以框架补之：

- **CAmkES**：在架构描述语言（ADL）中声明组件接口，编译期生成胶水代码，把「跨地址空间调用」包装成标准 C 函数调用语义，屏蔽手写能力管理与编组。官方动机是易用性 + **生成的胶水代码可形式化验证**——组件平台的自动生成部分带形式化正确性保证。它解决的不是性能，而是「手写裸 IPC 的正确性与维护性」。
- **Microkit**（原 seL4 Microkit，MCS 配置）：引入 **passive PD（被动保护域）** 概念——被动 PD **不拥有自己的调度上下文**，仅在收到 protected procedure call（`microkit_ppcall`）或处理子 PD 故障时，**借用调用方的调度上下文执行**；收到 notification 时按其 notification 绑定的调度上下文运行。reply capability 与调度对象跟踪这条「借用链」，过程结束把调度上下文归还调用方。
- **主动 vs 被动**：Liedtke《On micro-kernel construction》确立「地址空间 + 线程 + IPC」最小抽象集，后续 L4 生态发展出主动对象（服务端自持线程）与被动对象（无自有线程、客户端线程迁移进来执行）之分——被动服务模型因省线程、性能好而成为 L4 生态服务器标准。seL4/Microkit 的 passive PD 即此脉络的延续：**服务不消费 CPU，除非被调用**。

[来源: Microkit User Manual — docs.sel4.systems/projects/microkit/manual/latest/](https://docs.sel4.systems/projects/microkit/manual/latest/)；[CAmkES Manual — docs.sel4.systems/projects/camkes/manual.html](https://docs.sel4.systems/projects/camkes/manual.html)；[Liedtke, "On micro-kernel construction", SOSP'95](https://doi.org/10.1145/224056.224075)

对本项目的启示：seL4 的教训是**裸会合原语 + 请求-响应协议需要 reply 能力与库层/框架层封装**才能好用；被动服务（无自有执行体、被调用才运行）是微内核服务的主流形态——对应本项目「用户态服务承担长工作、内核只做短路径转发」的方向，服务进程并不天然拥有独立调度权重，而是被请求驱动。

---

## 2. Fuchsia Zircon

### 2.1 channel 消息模型：内核缓冲的异步邮箱

Zircon 官方文档对模型有明确陈述：**「Zircon API 在发送与接收两侧都是异步的，要求内核为发送方缓冲数据，直到接收方排空」**。channel 是双向的、按方向排队的消息传输：字节数据与 handle 一起随消息移动。这不是会合——发送不等待接收方就绪，直接入内核缓冲队列。

**上限**：`ZX_CHANNEL_MAX_MSG_BYTES = 65536`（64KiB）、`ZX_CHANNEL_MAX_MSG_HANDLES = 64`；iovec 变体总字节同样不得超过 64KiB、iovec 元素数不得超过 8192，超限返回 `ZX_ERR_OUT_OF_RANGE`。大于 64KiB 的负载不走 channel——改用 socket 字节流、或共享内存 VMO + channel 传 handle 的组合（见 2.3）。

**流控哲学（关键）**：Zircon 明确反对「满则返回错误码」的异步流控。官方文档指出：异步系统若在极罕见场合返回 "try again" 类错误，应用处理代码极少正确，且「循环重试」会把异步服务退化为同步服务、引入活锁与死锁。因此 Zircon 的设计主张是：**健康系统中以合理速率发送 IPC 应假设永远成功**；内核为每个对象实例实现缓冲上限，当超限时在调用线程**抛策略异常（policy exception）**而非返回错误，应用通常不处理该异常、任其传播到崩溃分析服务。上层防超限靠应用级策略：流控、请求-响应、请求过期、sidecar VMO 等。

[来源: zx_channel_write — fuchsia.dev/reference/syscalls/channel_write](https://fuchsia.dev/reference/syscalls/channel_write)；[Zircon Kernel IPC Limits — fuchsia.dev/fuchsia-src/concepts/kernel/ipc_limits](https://fuchsia.dev/fuchsia-src/concepts/kernel/ipc_limits)

**handle 随消息传递**：`zx_channel_write_etc` 对每个 handle 指定 disposition：`ZX_HANDLE_OP_MOVE`（消费源 handle）或 `ZX_HANDLE_OP_DUPLICATE`（保留源 handle），并可对转移的 handle **裁剪权利**——例如去掉 `ZX_RIGHT_TRANSFER` 可阻止接收方再次转授。两种操作都要求源 handle 持有 `ZX_RIGHT_TRANSFER`，DUPLICATE 还要求 `ZX_RIGHT_DUPLICATE`。channel 是 Zircon 中传递 handle（即传递能力）的唯一路径。

**同步 RPC**：`zx_channel_call` 在用户态层面组合 write + read（带 txid 匹配），发送后阻塞等待对端回复——内核本身不提供「会合」特殊机制，同步只是用户态协议（FIDL 的 request-response 语义）。

[来源: zx_channel_write_etc — fuchsia.dev/reference/syscalls/channel_write_etc](https://fuchsia.dev/reference/syscalls/channel_write_etc)；[zx_channel_call — fuchsia.dev/reference/syscalls/channel_call](https://fuchsia.dev/reference/syscalls/channel_call)；[Life of a handle in FIDL — fuchsia.dev/fuchsia-src/concepts/fidl/life-of-a-handle](https://fuchsia.dev/fuchsia-src/concepts/fidl/life-of-a-handle)

**channel / socket / FIFO 分工**：Zircon 有三个带 peer 的内核对象，各司其职——channel = 消息 + handle 转移（能力通道）；socket = 字节流（不能传 handle）；FIFO = 共享内存的控制面优化（固定元素与缓冲上限，小载荷高效）。共享内存本身不在这些对象里，而是 VMO。

[来源: Zircon fundamentals — fuchsia.dev/fuchsia-src/get-started/learn/intro/zircon](https://fuchsia.dev/fuchsia-src/get-started/learn/intro/zircon)；[Channel / Socket / FIFO 内核对象参考 — fuchsia.dev/fuchsia-src/reference/kernel_objects/](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/)

### 2.2 signals 与 channel 为何分开：信号是状态掩码，不是排队事件

Zircon 的 signals 是所有内核对象通用的**状态位掩码**（`zx_signals_t`），表达「对象的某个状态条件为真/假」——如 `ZX_CHANNEL_READABLE`（channel 有可读消息）、`ZX_CHANNEL_PEER_CLOSED`、VMO 的 `ZX_VMO_ZERO_CHILDREN` 等。它是**电平式（level）状态，不是排队事件**：一个信号要么被置位要么未置位；等待者因信号置位而唤醒后，信号可能已被消费/清除，因此**必须重新检查实际状态**，不存在「错过的事件计数」。`zx_object_wait_async` 的 `ZX_WAIT_ASYNC_EDGE` 选项只在信号从 inactive 跳变到 active 时投递一个数据包，且若调用时信号已 active 则不投递——边沿语义是显式选项，默认仍是电平语义。

**等待机制的汇聚**：`zx_object_wait_one`（阻塞单个对象）、`zx_object_wait_async`（注册异步等待到 `zx_port`）、`zx_port_wait`（从 port 取排队的数据包 `zx_port_packet_t`，类型 `ZX_PKT_TYPE_SIGNAL_ONE`，携带 trigger/observed 信号位）。port 是事件汇聚点：一个线程可在 port 上注册对多个对象的异步等待，一次 wait 取回多个对象的状态变化包。**注意层次**：对象本身的状态是电平的；port 排队的是「状态变化」的数据包——事件汇聚层有队列，状态层没有。

**为何分开**：channel 负责数据（消息 + handle），signals 是通用对象状态通知——几乎所有对象（channel、VMO、event、timer、interrupt）都有信号，但只有 channel 类对象能携带数据。纯信号无数据对象由 `zx_event`/`zx_eventpair` 提供（eventpair 两端可互置 `ZX_USER_SIGNAL_n` 位与 `ZX_EVENTPAIR_SIGNALED`，一端关闭时对端收到 `ZX_EVENTPAIR_PEER_CLOSED`）——这实际上就是「粘滞位掩码 + 唤醒」的 Zircon 版信号量，与 seL4 notification 同构。

[来源: Zircon Signals — fuchsia.dev/fuchsia-src/concepts/kernel/signals](https://fuchsia.dev/fuchsia-src/concepts/kernel/signals)；[zx_object_wait_async — fuchsia.dev/reference/syscalls/object_wait_async](https://fuchsia.dev/reference/syscalls/object_wait_async)；[zx_eventpair_create — fuchsia.dev/reference/syscalls/eventpair_create](https://fuchsia.dev/reference/syscalls/eventpair_create)

### 2.3 VMO 共享内存 + channel 传 handle：所有权随消息转移

VMO（Virtual Memory Object）是 Zircon 的共享内存原语：`zx_vmar_map` 把 VMO 映射进地址空间，`zx_vmo_duplicate` 生成减权副本，`zx_vmo_set_size` 调整大小。**共享 = 同一个 VMO 对象被多进程映射**；把 VMO handle 经 channel 传给另一方 = 转移所有权/权利（MOVE 或 DUPLICATE + 权利裁剪）。

**fuchsia.io 的组合范式**：文件协议是「channel 承载 FIDL 消息 + VMO 承载可映射数据」的组合——目录/文件句柄本身是一个 channel 上跑的 `fuchsia.io` 协议（Open、Read、Write、Seek、GetAttr 都是 channel 上的 FIDL 请求-响应）；对于可 mmap 的文件，客户端经 `File.GetBackingMemory` 拿到一个 **VMO handle**（带 VmoFlags：READ/WRITE/EXECUTE、PRIVATE_CLONE=COW / SHARED_BUFFER=直访；不指定则实现自选语义），之后大块数据走 VMO 映射，控制与元数据走 channel 消息。`zxio` 库把这两条路封装成统一 I/O 接口，读小数据走 channel 消息、映射走 VMO。注意：GetBackingMemory 是可选能力（实现可拒绝），客户端必须处理失败。

[来源: fuchsia.io — fuchsia.dev/reference/fidl/fuchsia.io](https://fuchsia.dev/reference/fidl/fuchsia.io)；[Rights — fuchsia.dev/fuchsia-src/concepts/kernel/rights](https://fuchsia.dev/fuchsia-src/concepts/kernel/rights)

对本项目的启示：Zircon 呈现的是「**控制面（channel 小消息，内核缓冲、异步）+ 数据面（VMO 共享映射）+ 事件面（signals/eventpair 状态掩码）**」三轨组合，并刻意回避「满则错误码」的流控陷阱。这与本项目 notes 的「消息（控制）+ tunnel（数据面共享帧）+ 信号（事件面）」的三件套结构高度同构，区别只在 Zircon 的 channel 走内核缓冲、tunnel 走共享内存直访。

---

## 3. Mach（历史教训向）

### 3.1 message/port 模型

Mach 的 IPC 以 port 为核心：**port 是内核管理的消息队列**，同时是对象句柄、通信端点与能力载体——一个 port 有多重身份。任务通过与 port 的权利（right）交互：**receive right**（唯一，某一时刻仅一个任务持有，可从消息转移）、**send right**（可多个任务持有）、**send-once right**（只能发一条消息，用完即毁）。每个任务维护一张整数 port name 表（名字空间），名字类同 fd 但只在本任务内有意义——同一整数在不同任务指不同 port。这是「port right = 能力」的所有权模型：持有 right 即可对该 port 对应的对象行使全部操作。

消息经 `mach_msg` 系统调用收发（`mach_msg_trap`），MIG（Mach Interface Generator）根据接口定义生成编组/解组的 RPC 桩。**消息排队语义**：发送到队列满的 port 时默认无限阻塞；可用 `MACH_SEND_TIMEOUT` 指定超时（到期返回 `MACH_SEND_TIMED_OUT`）或 `MACH_SEND_INTERRUPT` 允许被软中断打断；发送到 send-once right 不受队列上限约束。大块数据用 **out-of-line（OOL）内存描述符**随消息传递：内核经 `vm_map_copyin` 处理——小区域物理拷贝、大区域 copy-on-write 虚拟映射，接收方获得内存而发送方引用计数相应调整。端口事件（如 port 销毁）以 **dead name notification** 异步通知：`mach_port_request_notification` 指定被监视 port 与接收通知的 port（`MACH_NOTIFY_DEAD_NAME`），内核随后向通知 port 发送一条消息。

[来源: Apple "Mach Overview" — developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html)；[XNU mach_port_insert_right(3) — web.mit.edu/darwin/src/modules/xnu/osfmk/man/](https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/mach_port_insert_right.html)；[GNU Mach Reference Manual "Message Send" — gnu.org/software/hurd/gnumach-doc/](https://www.gnu.org/software/hurd/gnumach-doc/)

### 3.2 被批评的问题

**性能**：Liedtke 在《Improving IPC by Kernel Design》中实测第一代微内核（Mach 为代表）短消息 IPC 普遍需 **50–500 微秒**，并论证这不是微内核架构的固有代价而是设计与实现问题；经「最小化 + 重构 IPC 路径（架构、算法、编码三层优化）」的 L3/L4 取得 **10–20 倍**提升，证明 IPC 可以是高效的基础机制。L4 侧对 Mach 的批评集中在：异步缓冲邮箱需要内核缓冲管理、消息拷贝、能力/rights 追踪与 cache 足迹膨胀，而同步会合免去这些。

**复杂性**：Mach 的 port 多重身份（任务句柄/对象句柄/队列/能力）与 rights 管理是公认复杂来源。安全研究指出 **rights 即全权（all-or-nothing）**：持有 port right 即可访问对象全部服务，无细粒度权限控制，系统只能自建过滤/间接层；生命周期与类型的手工追踪（含引用计数操纵）是权限提升漏洞的温床；每任务名字空间带来查找/解析成本与混淆风险。MIG 桩与消息格式进一步堆高心智负担。

**现代收敛**：Apple 自身也弃用裸 mach port 做通用 IPC——开发者文档层面直言 Mach API 难用、需手工管理 rights 与并发陷阱，XPC 包在其上提供连接生命周期、launchd 端点解析、消息序列化、entitlement/QoS 校验，mach port 在实际系统里主要退化为底层传输与 send right 载体。

[来源: Liedtke, "Improving IPC by Kernel Design" — os.itec.kit.edu/downloads/improving-ipc.pdf](https://os.itec.kit.edu/downloads/improving-ipc.pdf)；[Minear, "Providing Policy Control Over Object Operations in a Mach Based System", USENIX Security'95](https://www.usenix.org/legacy/publications/library/proceedings/security95/full_papers/minear.pdf)；[OWASP MASTG-KNOW-0104: Low-Level System IPC Mechanisms](https://mas.owasp.org/MASTG/knowledge/ios/MASVS-PLATFORM/MASTG-KNOW-0104/)

### 3.3 历史教训

Mach 是「**单一异步缓冲邮箱扛一切**」路线的代表，其失败不在邮箱思想本身，而在：① 缓冲/拷贝/rights 追踪的系统性开销（性能）；② 把能力、对象、队列、身份全塞进一个 port 概念（复杂性与安全）；③ 缺少对「控制小消息 + 数据大块」的显式分轨（OOL 是事后补丁，仍需 vm 层参与）。L4 以「同步会合（无缓冲、无拷贝、确定）+ 最小抽象」直接回应，并在 notify/共享内存上补齐事件与数据面。**Mach 的历史价值是反面教材：邮箱路线必须显式处理缓冲上限、满时行为、能力/权利语义与身份混淆，否则复杂度与性能双输。**

---

## 4. QNX / L4 家族

### 4.1 QNX：同步消息传递为根本，异步事件为补充

QNX 官方架构文档将同步消息传递列为 IPC 基础：`MsgSend`（客户端阻塞直至服务端 `MsgReceive` 收到并 `MsgReply` 回复；`MsgReply` 本身不阻塞）、`MsgReceive`（服务端阻塞等待，收到后客户端转 REPLY-blocked，回复后客户端从 `MsgSend` 返回）。通道（channel，服务端创建）与连接（connection，客户端建立）构成寻址。官方给出的同步立场：**同步模型天然确定、内核直接完成线程间移交控制权、无数据排队/丢失、流控内建**——阻塞本身即同步与调度。

异步面是二级补充：**pulse** 是无阻塞的小消息（16 字节，携带优先级，`MsgSendPulse`），用于事件/通知/中断反馈，不要求服务端先 receive——本质是「小载荷、可丢弃语义、非阻塞」的异步通道；`MsgDeliverEvent`/`MsgNotify` 提供事件投递与注册。官方主张用同步 send-receive-reply 构建鲁棒服务，异步事件用于避免死锁与不可预测执行模式。**POSIX 信号在 QNX 上经消息通道实现**：信号经 pulse 机制投递到目标线程，并保留 57–64 号系统保留信号（常阻塞 + 排队，专供 `sigwaitinfo` 同步等待模式，无丢失无打断）。

[来源: Synchronous message passing — qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Sync_messaging.html](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Sync_messaging.html)；[MsgSend — qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/m/msgsend.html](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/m/msgsend.html)；[Pulses](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.getting_started/topic/s1_msg_Pulses.html)；[MsgSendPulse](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/m/msgsendpulse.html)；[Special signals](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Special_signals.html)

### 4.2 L4 家族：同步 fast path + notify 位掩码

L4 的 IPC 以同步会合为核心，寄存器 fast path：短消息直接放寄存器、双方会合、内核直接切换控制权；长消息经缓冲与 timeout 参数。**notify（通知）是 L4 的异步机制：单字 bitmask，无 payload，非阻塞——置位 + 唤醒等待者**。seL4 沿此把模型拆成两轨：同步 IPC（会合，数据面）+ notification（异步粘滞位掩码，事件面，见 §1.2）。L4Re 的共享内存库（l4shmc）把信号与共享缓冲再缝合：缓冲区的信号（l4shmc 的 wait/try 接口）+ 初始化状态位掩码，是「共享内存 + 通知位」的组合实践。

L4 选同步会合的理由（Liedtke 一脉）：无中间缓冲（免分配、免双拷贝、免内核内存被 DoS）、确定性强、支持直接移交控制权与懒惰调度优化；异步缓冲则要付缓冲管理与拷贝的持续代价。而 Mach 的异步路线因 rights 管理复杂与缓存足迹大而昂贵。

[来源: L4Re IPC concepts — l4re.org/doc/l4re_concepts_ipc.html](https://www.l4re.org/doc/l4re_concepts_ipc.html)；[L4 IPC overview — os.inf.tu-dresden.de/L4/l4libman/l4_ipc.html](https://os.inf.tu-dresden.de/L4/l4libman/l4_ipc.html)；[L4Re shared memory signals — l4re.org/doc/group__api__l4shmc__signal__cons.html](https://l4re.org/doc/group__api__l4shmc__signal__cons.html)；[Liedtke, "Improving IPC by Kernel Design"](https://os.itec.kit.edu/downloads/improving-ipc.pdf)

---

## 5. 信号投递模型对比

### 5.1 四种模型的本质

**POSIX 信号**：软件中断，可**抢占**目标线程在任意执行点插入 handler（含中断系统调用、`sigreturn` 返回）；标准信号**合并**（同信号多次置位只投递一次、不排队），实时信号（RT）才排队；有 signal mask 与阻塞集。它是「异步抢占注入 + 可选排队」的模型，语义复杂（可重入、不确定的投递点、与系统调用的交互）。

**Zircon signals**：对象状态位掩码，**电平**语义（置位/未置位），不是事件队列；等待者被唤醒后必须重查状态（见 §2.2）；无 handler 注入——消费方式是显式 wait 系统调用在同步检查点返回。event/eventpair 是纯信号对象。

**seL4 notification**：粘滞位掩码（机器字宽的二进制信号量数组），信号置位并唤醒等待线程；位在 `Wait` 消费时清空（Active→Idle）；无 handler 注入，消费在 `Wait`/`Poll` 返回点。`Signal` 不关心接收方此刻是否在等——位先粘滞、后来者取走（Idle→Active→...→消费）。

**L4Re notify / l4shmc**：同样位掩码 + wait/try 同步检查点消费。

### 5.2 对比维度

| 维度 | POSIX 信号 | Zircon signals | seL4 notification |
|---|---|---|---|
| 排队 vs 状态 | 标准合并、RT 排队 | 状态掩码（level） | 状态掩码（粘滞位） |
| 投递时机 | 抢占注入（任意点 handler） | 同步检查点（wait 返回） | 同步检查点（wait/poll 返回） |
| 载荷 | 可带 siginfo | 无（纯状态） | 字宽位图（各发送方一位） |
| handler 机制 | 有（sigaction 注入） | 无 | 无 |
| 消费语义 | 自动/手动阻塞控制 | 重查状态 | 取字并清零 |
| 去重 | 标准信号合并 | 电平天然合并 | 位 OR 天然合并 |

### 5.3 微内核生态的收敛结论

微内核生态（Zircon、seL4、L4、QNX）的事实标准是：**通知 = 一组粘滞位掩码 + 唤醒，消费在同步检查点（wait/recv 返回、返回用户态边界），不做抢占式 handler 注入**。理由可以归纳为：

1. **与内核「短路径、不等待」的定位一致**：投递即置位 + 唤醒（等价于本项目「内核请求完成 → wake」），没有长路径上的 handler 状态机。
2. **位 OR 合并天然去重**：多次事件置同一位置，等待者一次消费，无 POSIX 式合并/排队复杂度。
3. **无抢占注入的可重入问题**：handler 在任意点插入要求用户态栈/寄存器保存与重入保护，与简单同步检查点相比是系统性复杂度。
4. **事件与数据的分离**：通知只表达「发生了某类事件」，具体数据由接收方经共享内存/消息另行取用——事件面与数据面解耦。

对照：POSIX 式抢占注入在微内核语境普遍被视为历史包袱（Starnix 在 Fuchsia 上模拟 POSIX 信号需在 Zircon 之上重造一整套投递/翻译层即为反证）。

[来源: Zircon Signals — fuchsia.dev/fuchsia-src/concepts/kernel/signals](https://fuchsia.dev/fuchsia-src/concepts/kernel/signals)；[Signal translation in Starnix — fuchsia.dev/fuchsia-src/concepts/starnix/signals](https://fuchsia.dev/fuchsia-src/concepts/starnix/signals)；[POSIX <signal.h> — pubs.opengroup.org](https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/signal.h.html)；[seL4 Notifications Tutorial](https://docs.sel4.systems/Tutorials/notifications)

---

## 6. 对本项目的映射要点

### 6.1 决策点一：message —— 邮箱（mailbox）vs 会合（rendezvous）

**各系统立场与理由**：

- **Mach（邮箱）**：内核缓冲队列；失败于缓冲/拷贝/rights 开销与单一 port 的多重身份（§3.3）。反面样本。
- **Zircon（邮箱）**：明确「发送与接收均异步、内核为发送方缓冲」；但配套 64KiB/64 handles 硬上限、**拒绝「满则错误码」**、超限走策略异常，并以应用级流控/请求-响应兜底（§2.1）。现代邮箱实践。
- **seL4/L4（会合）**：无缓冲、无拷贝、确定、fast path；代价是请求-响应需要 reply capability + 框架封装（CAmkES/Microkit），服务端呈被动模型（§1.1、§1.3）。
- **QNX（同步为主）**：同步 send-receive-reply 为根本（流控/确定性内建），pulse 做异步补充（§4.1）。

**本项目映射**：

- 现行 notes/message.md 是**邮箱模型**（Send/Peek/Discard/Receive，负载小、目标 + 类型 + 负载，Pid=0 指内核）；FAL 的大块流数据已规划走 tunnel 共享帧（notes/tunnel.md）。这与 Zircon「小消息 channel + 大块 VMO 共享」、QNX「同步小消息 + 共享内存」的组合范式同构——**控制面邮箱 + 数据面共享帧是成熟系统的一致选择，方向无需推翻**。
- 本项目内核「永不等待、异步 syscall = 内核请求 + wake」（notes/call.md）与邮箱模型天然契合：Send 即内核登记投递、不阻塞；与 seL4/QNX 的会合（内核要等对端就绪）相比，邮箱不需要内核侧排队等待对端，短路径更彻底。**会合的「无缓冲/确定性」优势在本项目由数据面隧道（共享帧直访）获得，不必在控制面付出会合的内核等待代价。**
- 必须借 Zircon 补的功课：**邮箱需要显式缓冲上限与满时行为**。三个候选语义——(a) 满则返回错误码（Zircon 明言会养出错误的重试代码）；(b) 满则阻塞（回归会合语义，违背内核不等待）；(c) 满则上限外由协议承担（配合流控）。Zircon 主张「合理速率下永远成功 + 超限异常」，本项目没有异常机制（用户态 fault 一律杀进程），需另选——如上限 + 错误码 + 上层流控，或仿 QNX 以请求-响应配对天然限流。这是 message 契约必须拍板的点。

### 6.2 决策点二：signal —— 排队 vs 状态掩码

**各系统立场与理由**：POSIX 排队/合并（复杂）；Zircon/seL4/L4 一致采用**状态掩码（粘滞位 + 唤醒，同步检查点消费）**（§5.3）；Mach 把通知做成消息（port dead notification），是「排队」路线，代价是又一条消息队列要管理。

**本项目映射**：

- 现行 notes/signal.md 写「信号是单向、**抢占**、无负载、匿名」，其中「抢占」与协作式内核矛盾——本项目内核态不可打断、无抢占注入点；投递若做成 POSIX 式 handler 注入，需要一整套用户态重入/保存机制，与微内核生态收敛结论（§5.3）相悖。**建议改向：通知 = 状态掩码（粘滞位）+ 唤醒，消费在同步检查点（wait/recv 返回、sret 边界）。**这与现有「异步 syscall 完成 → wake 回 Ready」模型完全同构——wake 即投递。
- 内核 → 进程的通知：置位 + 唤醒（对应 seL4 bound notification、Zircon eventpair）；进程 → 进程的通知：经内核转发置位 + 唤醒（对应 seL4 notification 经能力、Zircon eventpair 经 handle）。「无负载、匿名」维持——事件面不带数据，数据走消息或隧道。
- 排队需求（如同一事件连续发生 3 次）在掩码模型下退化为「计数语义」，微内核生态的处理是让数据面承载计数（消息排队或共享内存计数器），通知只表达「有」。若确有强排队需求，参考 QNX 的保留信号（常阻塞 + 排队 + sigwaitinfo）或 Zircon 的 port 包队列（事件汇聚层排队、状态层电平）——分层而非混同。

### 6.3 决策点三：tunnel 共享帧 —— 所有权模型

**各系统立场与理由**：

- **seL4**：能力（帧 cap）随 IPC 转移（cap transfer），权利继承发送方对 endpoint 的权利；共享映射靠 cap 复制 + 显式 map（§1.1、notifications 教程中的共享内存练习）。
- **Zircon**：VMO handle 随 channel 消息转移，`MOVE`/`DUPLICATE` + **权利裁剪**（去掉 TRANSFER 阻止再转授）；共享 = 同 VMO 多进程映射（§2.3）。所有权模型最清晰。
- **Mach**：OOL 内存描述符，内核 vm_map_copyin（小拷贝/大 COW），rights 随消息——受 vm 层牵制、语义重。
- **QNX/L4Re**：共享内存区域 + 消息传递地址/句柄；l4shmc 用信号位 + 位掩码协调（§4.2）。

**本项目映射**：

- 现行 notes/tunnel.md：Open 创建通道、双方映射同一页帧、单工/双工由协议定、Runnel FIFO 协议（1 页 = 3×1KiB 缓冲 + 1KiB 控制块）、「安全 = 不安全，仅内存拷贝不用系统调用」。自述缺陷：**接收端无法判断数据到来（盲等）、发送端遇接收端睡死则死锁**。
- 关键事实：隧道页的控制块由双方直接读写（无内核参与），所有权与协调全靠协议约定。**但「创建/关闭/唤醒」必须经内核**：谁的请求创建（对应 Zircon 的 VMO 由谁创建后传 handle、seL4 的帧 cap 由谁持后转移）、隧道句柄是否可再转授（对应 Zircon 的 TRANSFER 权利）、关闭时帧归还与双端引用（对应 seL4 的能力引用计数、Zircon 的 PEER_CLOSED 信号）。notes/tunnel.md 自述的两个缺陷的修法正是「内核参与通知」：接收端盲等 → 隧道配一个事件位（内核置位 + wake，对应 Zircon eventpair、seL4 notification）；发送端死锁 → 对端关闭时给事件位（对应 ZX_CHANNEL_PEER_CLOSED / seL4 cap revoke 的异步通知）。
- 所有权模型的候选：(a) 全局隧道 id + 内核登记（简单，但匿名 Open 无法确认服务方——notes 已自述此缺陷）；(b) 每进程隧道句柄 + 可随消息转移（类 Zircon handle/seL4 cap，需消息系统支持句柄随消息，引入能力面）；(c) 纯共享页 + 内核只做创建/关闭登记。**本项目无能力系统（capability），引入随消息转移的句柄是否值得，是 tunnel 契约拍板点。** 无论选哪个，都要明确：帧由谁创建、谁有权利转移、生命周期（双方关闭协议 + 内核回收）三件事——这决定「干净被杀」可达性（进程退出时其持隧道必须可回收，否则重蹈旧内核帧泄漏）。

### 6.4 汇总：三条线与本项目已有模型的关系

| 面 | 成熟系统 | eRhino 现状（notes） | 待拍板 |
|---|---|---|---|
| 控制面 | Zircon 邮箱 / seL4-QNX 会合 | message.md 邮箱（Send/Peek/Discard/Receive） | 缓冲上限与满时语义 |
| 数据面 | VMO / 共享帧 / OOL | tunnel.md 共享帧 + Runnel FIFO | 帧所有权、句柄转移、内核参与的创建/关闭/唤醒 |
| 事件面 | 粘滞位掩码 + 唤醒 | signal.md（现写「抢占」，与协作式矛盾） | 改向状态掩码 + 同步检查点投递 |

三面分离是成熟系统的一致结构，eRhino 已有雏形且方向一致；本次调研的增量证据集中在「邮箱满时行为」「信号投递模型」「共享帧所有权」三个具体决策，均需用户拍板后回写 notes/message.md、signal.md、tunnel.md 并同步 shared/ ABI。

---

## 参考来源清单

- seL4 Reference Manual（IPC/Notifications 章）：https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex ；https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 IPC Tutorial：https://docs.sel4.systems/Tutorials/ipc ；Notifications Tutorial：https://docs.sel4.systems/Tutorials/notifications
- seL4 API Reference：https://docs.sel4.systems/projects/sel4/api-doc.html
- Microkit User Manual：https://docs.sel4.systems/projects/microkit/manual/latest/ ；CAmkES Manual：https://docs.sel4.systems/projects/camkes/manual.html
- Liedtke, "Improving IPC by Kernel Design"：https://os.itec.kit.edu/downloads/improving-ipc.pdf ；"On micro-kernel construction"：https://doi.org/10.1145/224056.224075 ；"From L3 to seL4"：https://doi.org/10.1145/2517349.2522720
- Zircon IPC Limits：https://fuchsia.dev/fuchsia-src/concepts/kernel/ipc_limits ；zx_channel_write：https://fuchsia.dev/reference/syscalls/channel_write ；zx_channel_write_etc：https://fuchsia.dev/reference/syscalls/channel_write_etc
- Zircon Signals：https://fuchsia.dev/fuchsia-src/concepts/kernel/signals ；zx_object_wait_async：https://fuchsia.dev/reference/syscalls/object_wait_async ；zx_eventpair_create：https://fuchsia.dev/reference/syscalls/eventpair_create
- Kernel objects（channel/socket/FIFO）：https://fuchsia.dev/fuchsia-src/reference/kernel_objects/ ；fuchsia.io：https://fuchsia.dev/reference/fidl/fuchsia.io
- Apple Mach Overview：https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html
- GNU Mach Reference Manual：https://www.gnu.org/software/hurd/gnumach-doc/
- Minear, "Providing Policy Control Over Object Operations in a Mach Based System"：https://www.usenix.org/legacy/publications/library/proceedings/security95/full_papers/minear.pdf
- OWASP MASTG-KNOW-0104（XPC 收敛）：https://mas.owasp.org/MASTG/knowledge/ios/MASVS-PLATFORM/MASTG-KNOW-0104/
- QNX Neutrino 8.0 官方文档（Synchronous message passing / MsgSend / Pulses / Special signals / sigwaitinfo / Events）：https://www.qnx.com/developers/docs/8.0/
- L4Re IPC concepts：https://www.l4re.org/doc/l4re_concepts_ipc.html ；L4 IPC overview：https://os.inf.tu-dresden.de/L4/l4libman/l4_ipc.html
- POSIX <signal.h>：https://pubs.opengroup.org/onlinepubs/9799919799/basedefs/signal.h.html ；Starnix signal translation：https://fuchsia.dev/fuchsia-src/concepts/starnix/signals

## Gaps（未取证/存疑点）

- Mach 的 port 队列上限具体数值与 send-once 不受限的精确实现细节未深入（不影响结论）。
- seL4 MCS 的 scheduling context 借用链的完整规则（passive PD 在 notification 调度上下文上运行的边界情况）只取官方手册概述，未读实现源码。
- Zircon 缓冲上限的具体数值未公开（官方明言不公开常量），报告只取「上限存在 + 超限走策略异常」这一官方事实。
- QNX 的「零拷贝」声称（内核权限校验后直接拷贝）未逐条核对文档原文，报告中仅表述为「移交控制权/无排队」，未断言零拷贝。
