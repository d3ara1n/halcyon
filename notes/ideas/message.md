# 消息

消息是 IPC 的控制面：有界、单向、内核缓冲的 Mailbox 记录，承载小批量 payload、内核生成的来源 envelope，以及原子 TRANSIT 的 Handle。大块数据走 [Tunnel](tunnel.md)，状态观察走 [wait](wait.md)。

## 队列与发送授权

Mailbox 是具有唯一 receiver-owner 的接收队列。创建队列只交付 owner；由 owner 显式铸造发送授权，不隐含一个可调用的默认根入口。owner 不可复制，只能经直接 GRANT 安装，不能进入消息。

每次铸造产生一个独立的发送授权对象，绑定目标 Mailbox 和不可变 badge。它的 sender 与 send-once 是同一对象的不同 role；duplicate、移动、rights 裁剪和 send-once 派生保持发送授权身份与 badge。重新铸造即产生新身份，即使目标队列和 badge 相同也不合并寿命。

铸造同时交付该发送授权的 [Lifetime](object.md) 观察权。能力副本、运输中引用和调用交付责任独立于原持有进程存活；全部相关引用消散后，Lifetime 才进入 CLOSED。观察者不保活发送授权。接收服务不需要保留永不关闭的 sender 母本，也不按 PID 猜测最后一个客户端是否已退出。

内核入箱时填写接收 envelope：

- 执行 Send 的进程 provenance；
- 被调用的发送授权身份及其 badge；
- 协议 kind、payload 长度与附带 Handle 数。

身份和 badge 都不是 bearer token。普通 Send 只接受具 WRITE 的真实 sender，不接受 PID、badge 或对象身份数值作为目标。badge 是服务自定义标签；精确的授权实例由内核产生的发送授权身份区分。

owner 关闭使队列 CLOSED，清除排队消息和未接收的 entry。残留 sender 对该队列的调用失败。队列可调用性、发送授权的最终寿命、服务端政策撤销分别是不同事实。

## Delivery：接收后的交付责任

每条成功投递的消息都有一个内核产生的 Delivery。它保活被调用的发送授权，从排队、接收预留一直延续到接收方明确释放处理责任。

Receive 原子交付消息内容、业务 Handle 和 affine Delivery owner。Delivery 不占业务 Handle 槽，不是 reply capability，不携带 Send 权，也不绑定 RPC 的 txid。它可以显式移动给承担后续处理的一方，不能复制；关闭是叶子收束。服务把它保留在请求上下文中，直到处理、回复或拒绝责任完成。

所有 sender 已关闭，但仍有排队或处理中的消息时，Lifetime 不得提前终止。这样服务可以先按发送授权身份取得自己的状态引用，再在处理完成后释放 Delivery，不依赖接收线程和寿命观察线程之间的时序约定。

Peek 不取得 Delivery，Discard 和队列关闭释放尚未接收的 Delivery。Receive 回滚将原 Delivery 连同消息放回队头，不制造新的交付身份。接收方退出时，其 Delivery 按普通 Handle 账本收束。

## Header 与容量

发送方只能提交协议 kind、长度、Handle 数及 [Deadline](time.md)，不能填写 provenance、发送授权身份或 badge。接收结果由内核构造。长度与 Handle 数交叉校验，reserved 必须为零。

每条消息 payload 最多 4096 字节、业务 TRANSIT Handle 最多 8 个；每个 Mailbox 最多 16 条。Delivery 是额外的内核交付项，不压缩这 8 个业务槽。接收方必须为完整业务 Handle 集合及 Delivery 预留表空间。

内核不解释 kind、payload、badge 或附带 Handle 的上层含义。

## 非阻塞操作与事务边界

| 操作 | 语义 |
|---|---|
| MailboxCreate | 创建唯一 receiver-owner。 |
| MailboxMintSender | owner 原子取得一个独立 badged sender 及其 Lifetime 观察权。 |
| MailboxMakeSendOnce | 从具 DUPLICATE 的 sender 派生同一发送授权的一次性投递权。 |
| Send | 验证完整输入、预留消息及 Delivery、检查期限，然后原子入箱及搬移；永不阻塞。 |
| Peek | 观察队头 envelope 与容量，不改变队列。 |
| Receive | 原子安装业务 entry 和 Delivery，写出完整结果后移除队头；永不阻塞。 |
| Discard | 丢弃队头，关闭其业务 transit 与 Delivery。 |

Send 要么发布整条消息并摘除全部源 entry，要么两者均未发生。失败、满箱和投递前到期都保留源能力。Receive 要么完整安装并移除队头，要么不安装且队头仍在；调用期间输出区由调用者独占，失败输出不可解释。

send-once 的目标解析、入箱和消费在同一事务中完成。目标同时出现在 move 列表时，在任何摘除或入队前拒绝；成功投递才消费该 entry。内部临时引用的释放不代替业务 role 的关闭动作。

接收预留中的队头对并发 Receive/Discard 返回 busy。满箱返回 MailboxFull，空箱返回 ObjectNotAvailable，输出或表容量不足时不出队。

## 流控与等待

队列 READABLE 表示至少有一条可接收消息，WRITABLE 由队列和接收占位共同决定。满箱后观察 WRITABLE/CLOSED 再重试；接收方观察 READABLE/CLOSED。发送授权的等待绑定到实际队列电平源，并保留本次已验证的使用引用。

严格 FIFO 不提供选择性接收。队头装不进缓冲时，调用者只能规划容量、腾出空间或 Discard。需要不同授权域的入站隔离时，服务显式建立不同 Mailbox；同一个共享 FIFO 不承诺按 sender 公平。

准入、配额、授权状态、政策撤销、取消和重试属于服务协议。消息不承诺跨 Mailbox 的全局顺序，不做大 payload 分片，也不从 Delivery 推导业务完成或回复成功。
