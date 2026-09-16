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

## 异步出站任务

`dispatcher.rs` 的 `Dispatcher` 已改为 `libsrv::runtime::Task<()>`：它只拥有
PendingCall、txid 路由、Request/Reply 阶段、ReplyPort、MessageStorage 和
完成 FIFO。WaitSet 来源、arm generation、事件输入、期限唤醒、来源注销重试、
任务停止和最终退休全部由 Runtime 拥有。Dispatcher 通过 `Requests::arm_source`、
`rearm`、`remove` 声明观察操作，在 `Input` 中消费 `SourceEvent`；不再持有独立
WaitSet、来源表、直接期限推进或 `mem::forget` 放弃循环。

出站阶段由纯逻辑 `OutboundStage` 表达：`Ready → WaitingWritable → Ready`
可因满箱重试，成功投递进入 `Sent`，超时、关闭、拒绝或合法回复进入
`Terminal`。该状态机有 host testcase；目标代码的 Runtime 接缝通过用户态 RISC-V
`cargo check -p librpc -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem`、
`just check` 与七面 `just clippy`。当前尚无真实异步服务消费者或完整组合验收。

## 当前边界

`RequestContext`、`PreparedResponse`、Outbox、协作式 Cancel、idempotency key
与服务端去重仍未闭合。当前 Dispatcher 只是 RPC/Runtime 闭包的第一阶段实现，
尚未迁移 `srv_init`/`srv_fs` 真实路径，也未删除 srv_fs 的旧阻塞泵。完整入站
回复责任、业务 Commit 前准入、失败/退出/退款和跨机制验收由
[`RPC/Outbox 前置计划`](../../plans/todo-2026-09-13-service-runtime-prerequisites.md)
继续承接。

同步 Caller 与异步出站任务共用 Request 阶段、绝对 Deadline 和 owner 语义；
同步 Caller 仍是阻塞门面，不另建协议状态机。公共单调时钟与绝对期限由
[`单调时间与 RPC 全调用期限`](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md)
接通。

host 覆盖全部 framing 分类；`srv_init` 真实双调用让第一条 protocol mismatch response 携带 capability，确认拒绝后 Handle 已 stale，再由同一 Caller 经新 ReplyPort 接收第二条合法 response。

FAL 对 RpcPrefix 的使用和当前 provider 见 [`fal.md`](fal.md)。
