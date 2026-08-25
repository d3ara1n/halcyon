# 消息

消息是 IPC 的**控制面**：有界、单向、内核缓冲的邮箱记录，承载小批量负载、不可伪造的发送者身份以及原子转入的 Handle。大块流式数据走[隧道](tunnel.md)，状态等待走[对象信号与 Notification](signal.md)。

## 邮箱与寻址

邮箱是显式创建的可等待对象。`MailboxCreate` 交付唯一且不可复制、不可转移的 receiver-owner，以及可按 rights 复制或转移的 sender。receiver-owner 只能在所属进程内由线程共同使用；其关闭或进程退出立即使邮箱 `CLOSED`，清除队列和其中未接收的 Handle。普通 `Send` 只接受具发送权的邮箱 Handle，绝不接受 PID 或全局 id。

`READABLE` 当且仅当队列非空；取走或丢弃最后一条消息时清除。服务通过发现协议取得服务 sender Handle。sender 是内核记录的进程身份，只用于鉴别和审计，不自动构成回复地址；请求方需要回复时，显式转入一个裁剪后的回复邮箱 sender Handle。

## 两种 envelope

发送方提交的 `SendHeader` 仅含协议 `kind`、payload 长度和转入 Handle 数，不能填写 sender。内核入箱时生成接收方可见的 `MessageHeader`，在同样的协议字段之外填写 sender 身份。长度和 Handle 数在 header、用户缓冲和 `HandleMove[]` 间必须交叉校验；不匹配即拒绝。

每条消息的 payload 最多 4096 字节、转入 Handle 最多 8 个；每个邮箱最多容纳 16 条消息。内核不解释 kind、payload 或 Handle 的上层含义。负载入箱后已与发送方用户内存独立；转入 Handle 由消息持有直至接收或丢弃。

## 非阻塞操作与事务边界

| 操作 | 语义 |
|------|------|
| `Send` | 复制完整输入、校验目标和所有 move，并原子移动全部 Handle 入箱；永不阻塞。 |
| `Peek` | 观察队头接收 header、payload 长度和 Handle 数；不改队列或所有权。 |
| `Receive` | 完整写出接收 header、payload 和全部转入 Handle，才移除队头；永不阻塞。 |
| `Discard` | 丢弃队头及未接收的转入 Handle；永不阻塞。 |

原子性以系统调用返回时的内核状态为界：Send 要么发布整条消息并移除全部源项，要么两者都未发生；Receive 要么安装全部项、写出完整消息并移除队头，要么队头仍在。Receive 正在处理的队头对并发 Receive 与 Discard 返回 `ObjectBusy`；发送满箱返回 `MailboxFull`，空箱操作返回 `ObjectNotAvailable`。输出空间或 Handle 槽不足返回 `BufferTooSmall` 且不出队。

调用者必须在调用期间独占输出区。内核复制用户输入和写回输出时，用户地址空间保护覆盖校验和复制，避免校验后的内容或映射发生变化；失败输出不可解释。进程仅在不处于这类事务时进入回收，所有失败路径都恢复预留状态。

## 流控与等待

消息层不为发送者排队等待接收方。请求—应答配对、重试退避、准入和并发额度均属上层协议；超过容量是可观测失败而非静默丢弃。消费者检查后以 `WaitMany` 观察 `READABLE` 或 `CLOSED`，醒来后重试 Receive；Receive 本身没有阻塞形态。

## 边界

消息可转移 Handle，但不使 Handle 数值成为全局凭据；把数值写入 payload 不赋予接收方引用。消息不承诺跨邮箱或跨发送者的全局排序，也不做大负载分片重组。
