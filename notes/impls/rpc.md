# 通用 RPC 实现

方向见 [`../ideas/rpc.md`](../ideas/rpc.md)。通用 framing 与同步调用器位于 `user/frameworks/librpc`；rinlib 只提供 Mailbox、Handle 与 WaitMany，不解释 RPC。

## Framing

`RpcPrefix` 是 16 字节 little-endian 头：版本、message kind（Request/Response/Oneway）、reserved 与非零 txid。解码拒绝未知版本/kind、非零 reserved 和零 txid。

期待回复的 Request 在 Handle slot 0 放置裁剪为 `WRITE | TRANSIT` 的 send-once；内核只运输该 capability，不理解 txid 或 request/response。`sender_pid` 只提供来源信息，immutable badge 承载服务端铸造的 session/grant 上下文。

## 同步 Caller

`Caller` 懒创建并复用线程私有 ReplyPort；同一实例只允许一个 outstanding call。发送步骤是：

1. 从独立 `monotonic_id` domain 分配单调非零 txid 并编码 RpcPrefix；最大值后永久返回 ReachLimit，不回绕路由旧回复；
2. 从 ReplyPort sender 派生 send-once，作为 slot 0 与业务 Handles 一起投递；
3. WaitMany 同时观察 ReplyPort READABLE/CLOSED 与服务 endpoint CLOSED；
4. 接收后由纯逻辑 `validate_response` 验证 protocol id、Response kind 与 txid；任一拒绝都逐项关闭已安装 Handle、废弃当前 ReplyPort，再返回 typed rejection。

ServiceClosed、Wait/Receive 错误和 timeout 同样废弃端口，`Caller` 自身析构也关闭仍存 ReplyPort；因而失败、提前放弃或并发迟到回复都不能污染下一次调用。可进入 transit 的 role 都是固定上界叶 close（Tunnel Endpoint 不具 TRANSIT），reject cleanup 不建立新的异步 owner。公开参数 `timeout_ms` 是相对毫秒超时，零表示无限。超时只停止本地等待：Caller 关闭并废弃整个 ReplyPort，下次调用懒重建；迟到回复因 owner 已关闭而投递失败。返回 `CallError::Timeout`，不自动重试可能有副作用的请求。

## 当前边界

异步多 in-flight dispatcher、协作式 Cancel、idempotency key 与服务端去重尚未实现。当前实现没有 typed Delivery 或完整投递阶段 owner；公共 IPC 与 dispatcher 的实施由 [`FAL 整体计划`](../../plans/todo-2026-09-fal-service-capabilities.md) 承接，未来 ABI 以该计划和期限计划为准。

同步 Caller 的有限 `timeout_ms` 当前只用于请求投递成功后的 ReplyPort 等待；之前的 `send_blocking` 在 MailboxFull 时无限等待，因此它还不是完整调用 deadline。公共单调时钟与绝对期限现已由 [`单调时间与 RPC 全调用期限`](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md) 接通；RPC 的完整阶段策略、Unsent/Sent 和迟到回复仍由执行前置继续承接。

host 覆盖全部 framing 分类；`srv_init` 真实双调用让第一条 protocol mismatch response 携带 capability，确认拒绝后 Handle 已 stale，再由同一 Caller 经新 ReplyPort 接收第二条合法 response。

FAL 对 RpcPrefix 的使用和当前 provider 见 [`fal.md`](fal.md)。
