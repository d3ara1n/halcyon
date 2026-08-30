# 进程间通信

IPC 建立在[对象、Capability 与 Handle](object.md)模型上。每个通信对象只经进程本地 Handle 操作，role、rights 与对象状态共同决定合法行为；PID 是 provenance，不是通信地址或 authority。等待统一由 [WaitMany](wait.md) 完成。

## 三个面

- **消息**（[message](message.md)）是控制面：有界、非阻塞的 Mailbox 投递，承载小 payload、内核生成的 `sender_pid/sender_badge` envelope 与原子 TRANSIT Handle；
- **对象状态与 Notification**（[signal](signal.md)）是事件面：对象以非消费式电平表达可读、关闭等条件，Notification 提供显式消费的 OR 位集合；
- **Tunnel**（[tunnel](tunnel.md)）是数据面：两个与本地地址空间 lease 绑定的 Endpoint 映射同一有界多页共享区间，页内协议遵守[共享内存公共契约](shared-memory.md)。

[Runnel](runnel.md) 是 Tunnel 上的官方单工 FIFO 字节流协议；[BufferQueue](buffer-queue.md) 是与其并列、引用预注册 MemoryObject 的记录与缓冲交接协议。控制请求、流数据、缓冲交接和状态提示可以组合，但不得让一个面伪造另一个面的 authority、生命周期或流控。

## 共同边界

内核负责对象引用、rights、badge 保存、Handle 运输、消息事务、映射生命周期、状态发布与等待完成；它不解释 kind、badge 业务含义、RPC、路径或共享页。

`TRANSIT` 允许 Handle 暂存于消息，`GRANT` 允许 Building 期的直接跨表安装。unique owner 只走直接 GRANT；sender、signaler 与 invitation 可按授权经消息委托；Endpoint 与 VM 绑定而不移动。

协议使用 badged sender 表达不可伪造的服务端授权上下文，`sender_pid` 只作 provenance。回复必须取得显式 send-once 或 sender capability，不能由 PID 或 badge 数值推导。

启动根图由 launcher 在目标 runnable 前通过 ProcessGrant 与用户态 StartupBlock 建立，ProcessStart 只发布已经组装完成的线程。Mailbox owner 只是可选启动资源，不占固定寄存器或固定 Handle 数值。
