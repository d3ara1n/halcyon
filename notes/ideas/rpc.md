# 通用 RPC

Mailbox 是唯一 RPC 传输原语；内核不感知请求与应答。分层如下：

1. **Mailbox transport**：消息、TRANSIT Handle，以及内核生成的 `sender_pid/sender_badge`；
2. **通用 RPC framing**：固定宽 rpc 版本、flags（request/response/oneway）与 txid；期待回复的 request 必须把 send-once 放在 Handle slot 0；
3. **业务协议**：FAL、pm、driver 等各自定义版本、kind、错误与 payload。

send-once 证明谁有权向哪个 ReplyPort 完成一次投递；txid 只负责回复关联；sender_badge 表示服务端铸造的 grant/session 上下文；sender_pid 只表示 provenance。四者不能互相替代。

## 并发与回复路由

Mailbox 严格 FIFO、无选择性 receive。同步 Caller 为每线程懒创建并复用 ReplyPort，同一 port 同时只允许一个 outstanding call；超时关闭整个 port 并废弃，迟到回复随 owner 终态消亡。

异步多 in-flight 由单 dispatcher 按 per-process 单调非零 txid 分发。无 pending waiter 的迟到、重复或错误 txid 回复静默丢弃；txid 不重用，不依赖 tombstone。

ReplyPort sender 派生的 send-once 保持原 badge，并以 `WRITE | TRANSIT` 进入请求。服务成功回复后 capability 消费；target 与自身 TRANSIT alias 由内核拒绝。

## 超时、取消与重试

等待 deadline 只表示调用方停止等待，不撤销已入服务队列或正在执行的请求。超时后业务结果是“是否执行未知”；通用库不得自动重试有副作用调用。

幂等、idempotency key、服务端去重和协作式 Cancel 由业务协议或 librpc 扩展定义，不下沉为内核强取消。服务 endpoint CLOSED 与 deadline 都必须纳入同步和异步调用的完成路径。

## 落点

通用 framing 与 Caller/dispatcher 属独立 `librpc`；libfal、libfs、libsrv 和设备协议依赖它。rinlib 只提供对象、消息与等待基础封装，不解释 RPC。
