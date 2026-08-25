# 通用 RPC

邮箱是唯一的 RPC 传输原语：内核消息面不感知请求与应答，任何调用/回复语义都由用户态协议构造。分层如下：

1. **内核 mailbox transport**：消息、Handle move 与内核盖章的 sender 身份（见 [message](message.md)），对 RPC 零感知；
2. **通用 RPC framing**：固定宽前缀——rpc 版本、flags（request / response / oneway）与 txid。期待回复的 request 必须把裁剪后的 send-once 回复授权移入 Handle slot 0，这是跨协议公共约定；
3. **协议层**（FAL、pm、driver……）：各自持有协议版本、kind 与业务错误码；协议 header 以 RpcPrefix 起头（前缀嵌入，不做双层信封，避免重复的版本/类型/长度字段）。

send-once 与 txid 分工：send-once 证明**谁有权向哪里回复**（授权，一次性交付后消亡）；txid 证明**回复对应哪个请求**（关联）。两者不可互相替代。

## 并发与回复路由

mailbox 严格 FIFO、无选择性 receive：多线程共用一个 reply mailbox 会互相取走对方的回复。wire format 保留非零 txid，同步与异步两条路径：

- **同步调用**：每线程懒创建并复用 ReplyPort，同一 ReplyPort 同时只允许一个 outstanding call（同步线程本就阻塞，约束无代价）；超时即关闭整个 ReplyPort 废弃重建，迟到回复随 owner 关闭消亡，服务端对 send-once 的投递失败即干净丢弃，无需回收协议。
- **异步/多 in-flight**：单 dispatcher 按 txid 分发。迟到回复、重复投递与伪造 txid 统一落到「无 pending waiter 即静默丢弃」的同一条路径，不设 tombstone：txid 为 per-process 单调分配的 u64，不重用，回绕实践不可达，新 in-flight 撞上旧 txid 不可能发生。

同步调用超时由等待面的期限参数承载：WaitMany 接受可选 deadline（相对毫秒，0 为无限），到期以不指向任何观察项的 Deadline 结果交付。ReplyPort 的超时废弃重建以此为出口。

## 落点

通用 framing 由独立的 librpc 实现（与 librunnel 同层）；libfal、libfs 与 libsrv 依赖它。rinlib 保持纯运行时，不含 RPC。
