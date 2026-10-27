# 进程间通信

IPC 建立在[对象、capability 与 Handle](object.md)模型上。对象类型、实际 rights 与当前状态共同决定合法操作；PID 不充当通信地址或授权。观察使用 [WaitOne 与持久 WaitSet](wait.md)。

## 控制、事件与数据

- [消息](message.md)通过 Mailbox 运输小 payload、capability、发送授权身份和独立 Delivery。
- [对象状态与 Notification](signal.md)提供就绪、终态与可合并通知；观察状态和消费业务是不同动作。
- [Tunnel](tunnel.md)提供两端授权、事件及共享 backing。Endpoint capability 与普通地址空间 View 分别表达端点和映射寿命。

[Runnel](runnel.md)在 Tunnel 上表达单工字节流。[BufferQueue](buffer-queue.md)表达记录及预注册缓冲的协议交接。控制请求、流数据、缓冲交接和状态提示可以组合，各自保留自己的权限、寿命与流控关系。

## 共同边界

内核负责对象授权、消息运输、映射与等待机制，不解释业务 kind、路径或共享页协议。TRANSIT 与 Building 期 GRANT 是不同的交付方式，具体角色是否可交付由其类型与实际权限决定。

发送授权与 badge 关联业务上下文，sender PID 仅为来源信息。LifetimeObserver 观察发送授权/use 的终结，ReplyObserver 观察一次回复权的结果；它们不反向成为业务控制者。请求处理责任与回复发送权分别持有。

启动根图由 launcher 在目标可运行前通过显式授权和用户态启动上下文建立；Mailbox 是可选择的启动资源，不占固定寄存器或固定 Handle 编号。
