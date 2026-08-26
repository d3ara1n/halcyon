# 消息

消息是 IPC 的**控制面**：有界、单向、内核缓冲的 Mailbox 记录，承载小批量 payload、内核生成的来源 envelope，以及原子 TRANSIT 的 Handle。大块数据走 [Tunnel](tunnel.md)，状态等待走[对象信号与 Notification](signal.md)。

## Mailbox 与 sender capability

`MailboxCreate` 交付唯一 receiver-owner 与 badge 为零的初始 sender。owner 不可复制，只能经直接 GRANT 安装，不能进入消息；sender 可按 rights duplicate、TRANSIT 或 GRANT。

持 owner 的一方可铸造带不可变 `u64 badge` 的 sender。duplicate、移动和 send-once 派生保持 badge。以该 sender 投递时，接收 envelope 由内核填写：

- `sender_pid`：执行 Send 的进程 provenance，只用于审计或上层身份政策；
- `sender_badge`：目标 sender capability 的授权上下文；
- `kind`、payload 长度与 Handle 数。

badge 数值本身不是 bearer token；authority 来自不可伪造的 sender Handle。普通 Send 只接受具 `WRITE` 的 sender，不接受 PID、badge 数值或全局对象 id。

owner 关闭或其进程退出使 Mailbox `CLOSED`，清除队列和其中未接收的 entry。`READABLE` 当且仅当队列非空；`WRITABLE` 当且仅当占用（含在途接收占位）低于容量。

## 两种 header

发送方提交的 `SendHeader` 只含协议 kind、payload 长度和 Handle 数，不能填写 PID 或 badge。内核入箱时生成接收方 `MessageHeader`。长度和 Handle 数必须与用户缓冲交叉校验，reserved 必须为零。

每条消息 payload 最多 4096 字节、TRANSIT Handle 最多 8 个；每个 Mailbox 最多 16 条。内核不解释 kind、payload、badge 或 Handle 的上层含义。

## 非阻塞操作与事务边界

| 操作 | 语义 |
|---|---|
| `MailboxCreate` | 原子创建 owner 与 badge-0 sender。 |
| `MailboxMintSender` | owner 铸造同一 Mailbox 的 badged sender，rights 只能取允许集子集。 |
| `MailboxMakeSendOnce` | 从具 `DUPLICATE` 的 sender 派生相同 badge 的一次性投递权。 |
| `Send` | 复制完整输入、校验目标与全部 `TRANSIT` move，原子入箱；永不阻塞。 |
| `Peek` | 观察队头 header、payload 长度和 Handle 数，不改变队列。 |
| `Receive` | 完整写出 header/payload 并安装全部 entry 后才移除队头；永不阻塞。 |
| `Discard` | 丢弃队头并按 role 关闭其中 entry。 |

Send 要么发布整条消息并摘除全部源 entry，要么两者均未发生。Receive 要么完整安装、完整写出并移除队头，要么队头仍在。调用期间输出用户区由调用者独占，失败输出不可解释。

send-once 的目标解析、badge 取得、入箱与消费在同一 HandleTable 临界区完成。若同一 send-once 同时出现在目标与 move 列表中，Send 在任何摘除或入队前返回参数错误；失败不消费。否则首次成功投递后该 entry 必须消亡。

Receive 正在处理的队头对并发 Receive/Discard 返回 busy。满箱返回 MailboxFull，空箱操作返回 ObjectNotAvailable，输出空间或 Handle 槽不足返回 BufferTooSmall 且不出队。

## 流控与等待

消息层不为发送者排队。满箱后发送者观察 `WRITABLE | CLOSED`，醒来重试；接收方观察 `READABLE | CLOSED`。等待只提示状态，真实操作仍可能因并发再次失败。

严格 FIFO 的代价是队头消息若装不进接收方缓冲或 HandleTable，后续消息不可达。调用者只能规划资源、Peek 后腾出容量，或 Discard 队头；内核不提供选择性接收。

准入、公平、每客户端配额、请求取消与重试属于上层协议。公共 Mailbox 可服务无状态 RPC；需要持续授权上下文时使用 badged sender 或独立 session。

## 边界

消息不承诺跨 Mailbox 或跨发送者的全局顺序，也不做大 payload 分片。`sender_pid` 不能代替授权，`sender_badge` 不能脱离 sender capability 使用。Handle 原始数值写进 payload 不产生 authority。
