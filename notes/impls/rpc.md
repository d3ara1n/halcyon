# 通用 RPC 实现

方向见 [`../ideas/rpc.md`](../ideas/rpc.md)。通用 framing 与同步调用器位于 `user/frameworks/librpc`；rinlib 只提供 Mailbox、Handle 与 WaitMany，不解释 RPC。

## Framing

`RpcPrefix` 是 16 字节 little-endian 头：版本、message kind（Request/Response/Oneway）、reserved 与非零 txid。解码拒绝未知版本/kind、非零 reserved 和零 txid。

期待回复的 Request 在 Handle slot 0 放置裁剪为 `WRITE | TRANSIT` 的 send-once；内核只运输该 capability，不理解 txid 或 request/response。`sender_pid` 只提供来源信息，immutable badge 承载服务端铸造的 session/grant 上下文。

## 同步 Caller

`Caller` 懒创建并复用线程私有 ReplyPort；同一实例只允许一个 outstanding call。发送步骤是：

1. 分配单调非零 txid 并编码 RpcPrefix；
2. 从 ReplyPort sender 派生 send-once，作为 slot 0 与业务 Handles 一起投递；
3. WaitMany 同时观察 ReplyPort READABLE/CLOSED 与服务 endpoint CLOSED；
4. 接收后验证 protocol id、Response kind 与 txid。

公开参数 `timeout_ms` 是相对毫秒超时，零表示无限。超时只停止本地等待：Caller 关闭并废弃整个 ReplyPort，下次调用懒重建；迟到回复因 owner 已关闭而投递失败。返回 `CallError::Timeout`，不自动重试可能有副作用的请求。

## 当前边界

异步多 in-flight dispatcher、协作式 Cancel、idempotency key 与服务端去重尚未实现。它们属于 RPC 层扩展，不改变 Mailbox 与 WaitMany ABI。

FAL 对 RpcPrefix 的使用和当前 provider 见 [`fal.md`](fal.md)。
