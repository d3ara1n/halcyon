# 进程间通信

IPC 由共同的[对象与 Handle](object.md)模型组成。每个通信对象只经进程本地 Handle 引用，lifecycle role 与 rights 共同决定可执行操作；PID 用于身份和管理，不是通信地址。对象状态由[等待](wait.md)统一观察，任何阻塞只经 `WaitMany`。

## 三个面

- **消息**（[message](message.md)）是控制面：有界、非阻塞的邮箱投递，承载小批量数据、内核 sender envelope 和受 rights 约束的 Handle move；
- **对象状态与 Notification**（[signal](signal.md)）是事件面：对象以非消费式电平表达可读、关闭等条件，Notification 提供显式消费的 OR 位集合；
- **隧道**（[tunnel](tunnel.md)）是数据面：两个不可转移的本地端点映射同一段连续页区间，供协议直接交换批量数据。所有页内协议还须遵守[共享内存协议公共契约](shared-memory.md)。

[Runnel](runnel.md) 是隧道页上的官方单工 FIFO 字节流协议。控制请求、流式数据和状态提示各归其面；协议可以组合三者，但不得让一个面伪造另一个面的所有权或流控。

## 共同边界

内核负责对象引用、rights 校验、消息事务、映射生命周期、状态发布与等待完成；它不解释消息 kind、不解析共享页、不代替服务鉴权。服务发现返回授权后的邮箱 sender Handle；协议以不可伪造的 sender 身份和显式转入的回复 Handle 建立会话。

IPC 操作立即完成或返回错误；等待资源变化时，调用者先检查对象状态，再调用 WaitMany。启动时唯一的根图由 `ProcessCreate`/`ProcessStart` 在进程可运行前以通用 startup resources 建立；Mailbox receiver 只是可选 grant，取得后可承载版本化 `STARTUP` 消息。该根图建立过程不是 PID 消息寻址的例外。
