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

`e0b5c45` 的 `dispatcher.rs` 已将 `Dispatcher` 改为
`libsrv::runtime::Task<()>`：它只拥有
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

## 接续重审

该提交是 RPC/Outbox 闭包的前半段铺路，不是最终服务组合 API。`Task<()>` 与
`Family = Self` 使 Dispatcher 只能独占一个 Runtime，不能和入站请求、业务
任务及 Outbox 组成同一服务任务族；后续应保留协议状态和推进逻辑，改为由服务
任务族嵌入并转发 Runtime 回调。任务间提交/完成需要经 Runtime 应用的有界
wake 请求，不能让业务轮询 Dispatcher 完成队列。

接续前必须修复：回复邮箱故障后统一启动停止收束、删除公开普通 `reply_sender`、
保留任意阶段交付的 timeout 输入、原样保留来源 `SystemCallError`，并删除或补齐
当前没有写入点的 `cleanup_error`。这些修正不推翻已提交的 Packet owner、
PendingCall、绝对期限、来源注销和 `OutboundStage`。

入站最终顺序是先准入 `PreparedResponse`/Outbox、任务和来源，再执行业务副作用；
回复失败只报告交付结果，不伪造业务回滚。当前 `srv_fs` 仍先执行 MemFs 再准备
回复，因此不能作为 Outbox 已接通的证据。

## 当前边界

`RequestContext`、`PreparedResponse` 与 Outbox 已闭合：Outbox 持有请求
Delivery、reply-once、预付回复存储和 Runtime 来源，先完成来源准入再允许业务
Commit，发送失败进入 `Abandoned` 并保持已提交业务事实，来源注销后任务才退休。
`srv_init` 合法回复和 `srv_fs` provider 已迁移真实 Outbox；`srv_fs` 的旧阻塞泵、
手写 framing/回复路径和每请求 Runtime 已删除。当前尚未实现服务端幂等键或去重，
它们仍是协议扩展，不属于基本 Outbox 完成门。

Dispatcher 已改成可嵌入协议驱动，不再固定实现 `Task<()>`；Runtime 提供有界
Wake 请求并保留 Gate 暂满时的重试责任。当前没有真实多 in-flight 服务消费者，
因此 `begin_for` 保留为后续真实消费者的接缝，不另造测试服务证明它。

同步 Caller 与异步出站任务共用 Request 阶段、绝对 Deadline 和 owner 语义；
同步 Caller 仍是阻塞门面，不另建协议状态机。公共单调时钟与绝对期限由
[`单调时间与 RPC 全调用期限`](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md)
接通。

host 覆盖全部 framing 分类；`srv_init` 真实双调用让第一条 protocol mismatch response 携带 capability，确认拒绝后 Handle 已 stale，再由同一 Caller 经新 ReplyPort 接收第二条合法 response。

FAL 对 RpcPrefix 的使用和当前 provider 见 [`fal.md`](fal.md)。
