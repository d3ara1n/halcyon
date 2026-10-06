# 通用 RPC 实现

方向见 [`../ideas/rpc.md`](../ideas/rpc.md)。当前通用 framing 与同步调用器位于 `user/libraries/librpc`；`libkernel` 当前提供 Mailbox、Handle、WaitMany、时间等底层封装，不解释 RPC。目标结构是由 `libkernel` 提供这些内核契约，迁移见 [运行时与内核契约库分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md)；本篇以下实现事实仍按当前 libkernel 代码描述。

## Framing

`RpcPrefix` 是 16 字节 little-endian 头：版本、message kind（Request/Response/Oneway）、reserved 与非零 txid。解码拒绝未知版本/kind、非零 reserved 和零 txid。

期待回复的 Request 在 Handle slot 0 放置裁剪为 `WRITE | TRANSIT` 的 send-once；内核只运输该 capability，不理解 txid 或 request/response。`sender_pid` 只提供来源信息，immutable badge 承载服务端铸造的 session/grant 上下文。

## 同步 Caller

`Caller` 懒创建并复用线程私有 ReplyPort；同一实例只允许一个 outstanding call。`CallOperation` 借用 Caller 及其 ReplyPort，保存发送前 Request、Deadline 与可选取消能力；同步 `call_cancelable` 驱动它。操作状态严格为 `Unsent(Request)`、持 ReplyObservation 的 `Sent` 或 `Completed`：

1. 从独立 `monotonic_id` domain 分配单调非零 txid 并编码 RpcPrefix；最大值后永久返回 ReachLimit，不回绕路由旧回复；
2. 从 ReplyPort sender 派生 send-once，作为 slot 0 与业务 Handles 一起投递；
3. CallWait 拥有持久扁平 WaitSet，观察 ReplyPort READABLE/CLOSED、服务 WRITABLE/CLOSED、取消 READABLE/CLOSED 与 ReplyObservation ABANDONED（PEER_CLOSED）；发送后注销旧服务来源并只留 CLOSED，仍使用同一绝对 Deadline。Unsent 在再次投递前复核取消；Sent 合法回复先于取消；到期直接查询真实来源，不把空候选视为未就绪；
4. 接收后由纯逻辑 `validate_response` 验证 protocol id、Response kind 与 txid；任一拒绝都逐项关闭已安装 Handle、废弃当前 ReplyPort，再返回 typed rejection。

同步驱动在 ServiceClosed、Wait/Receive 错误、timeout 和外部取消时废弃当前 ReplyPort；可进入 transit 的 role 都是固定上界叶 close（Tunnel Endpoint 不具 TRANSIT），reject cleanup 不建立新的异步 owner。`CallOperation::advance` 只做一次非阻塞发送或回复尝试；`wait_ready` 按未发送/已发送分别观察 service writable 或 ReplyPort，并观察 service close/cancel。成功接收回复进入 `Completed`，完成操作不能再次推进；`abort` 按阶段返还原 Request 或 ReplyCleanup。已发送操作被显式 abort 或直接 Drop 时都会隔离旧 ReplyPort，迟到回复不能进入下一调用。`CallError` 的互斥 owner 是 `Unsent(Request)`、`Reply(ReplyCleanup)` 或 None：发送前拒绝保留原请求；发送后未确认按 Sent 返回未知，关闭失败保留 typed owner 供 `retry_cleanup`。RPC 不自动重试可能有副作用的已投递请求。

分步接口已由 `libfal::ClientOperation` 接通：调用者可观察阶段、非阻塞推进、等待、abort 和完成回复；`call_classified` 将编码失败、未投递、已投递未知与明确业务拒绝分开。`caller.rs` 仅在 RISC-V 目标编译，生命周期证据由正式 A/B `test_fal` 的迟到回复、完成后不可推进、provider CLOSED 后未投递请求及 Copy 消费提供；host framing/OutboundStage 测试不替代该行为证据。

## 异步出站任务

[`dispatcher.rs`](../../user/libraries/librpc/src/dispatcher.rs) 的 `Dispatcher` 是
`libexecution::runtime::Task<()>`：它只拥有
PendingCall、txid 路由、Request/Reply 阶段、ReplyPort、MessageStorage 和
完成 FIFO。来源登记、事件输入、期限唤醒、来源注销重试、
任务停止和最终退休全部由 Runtime 拥有。Dispatcher 通过 `Requests::add_source`、`remove` 声明持久观察，在 `Input` 中消费 `SourceEvent`；不再持有独立
WaitSet、来源表、直接期限推进或 `mem::forget` 放弃循环。

出站阶段由纯逻辑 `OutboundStage` 表达：`Ready → WaitingWritable → Ready`
可因满箱重试，成功投递进入 `Sent`，超时、关闭、拒绝或合法回复进入
`Terminal`。该状态机有 host testcase；当前目标接缝的开发验证与未覆盖范围由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)承载。`srv_fs` 的跨 provider Delegate 是真实异步消费者：Dispatcher 在同一 Runtime 内向下游 Derive，完成后唤醒原 DelegateTask。

## 当前嵌入与边界

`Dispatcher` 现由外层服务 Runtime 通过 `Runtime::get_task_mut` 提交调用、接收
完成并推进来源注销；它不拥有第二套 WaitSet，也不在 handler 中同步等待。完成结果
进入有界 FIFO，由原任务取得并继续推进，停止时先取消或收束下游调用，再注销来源、
关闭回复端点并退休 Dispatcher owner。`srv_fs` 的跨 provider Delegate 是当前真实
多 in-flight 消费者：A 的 Runtime 将 Derive 投递给 B，B 关闭或 A 停止时均沿
`CallPhase` 和 `OutboxResult` 的明确阶段收束。

Dispatcher 的调用期限索引为侵入式 TimerQueue<txid>，每个 PendingCall 在提交前 fallible 分配 pin Box 节点；终态取消节点后才摘路由/释放调用，非空 Dispatcher Drop 先排空索引。最早期限继续向 Runtime 的任务期限入口声明，没有独立硬件时钟或执行循环；底层节点/队列契约见[时间实现](time.md)。每调用保留服务 CLOSED 与一个 progress 来源槽：Writable 实际注销回执后，binding role 转 Abandoned，同一槽登记 ReplyObservation。事件核 SourceId，终态保留观察权到来源收束，不增加第三个每调用槽。

Dispatcher 保留协议状态、PendingCall、回复存储、期限和运输 owner；Runtime 仍拥有
WaitSet、来源登记、期限唤醒和最终退休。后续增加消费者时复用这一嵌入边界，不恢复
独立 WaitSet、阻塞泵、无限重试或业务轮询完成队列。

入站最终顺序是先准入 `PreparedResponse`/Outbox、任务和来源，再执行业务副作用；
回复失败只报告交付结果，不伪造业务回滚。`srv_fs`在Commit前预备Outbox及CLOSED观察；正文就绪后才尝试发送。满箱使阶段转WaitingWritable，每次发送尝试后立即协调所需来源，声明撤销旧CLOSED-only登记；在实际注销回执后登记WRITABLE|CLOSED，未完意图/拒绝重试保持Runnable。即使推进预算1也不在缺少WRITABLE登记时Parked，reply-once、Delivery和原绝对期限始终由同一Outbox持有。成功或失败终态注销来源后退休，SourceRequest不重复发布。具体组合验证与未覆盖窗口见对象链工作记录。

## 当前边界

`RequestContext`、`PreparedResponse` 与 Outbox 已闭合：Outbox 持有请求
Delivery、reply-once、预付回复存储和 Runtime 来源，先完成来源准入再允许业务
Commit，发送失败进入 `Abandoned` 并保持已提交业务事实，来源注销后任务才退休。
尚未成功投递时，`PreparedResponse::drain_capabilities` 与 Outbox 同名出口可逆序逐项
取回 Packet 中的业务 capability；该出口不复制 owner，也不访问已被成功发送消费的
Packet。它只提供运输 owner 的显式收束点，业务是否已经提交、能否恢复仍由上层协议
状态机决定；FAL affine Take 的提交/回滚边界见 [`fal.md`](fal.md)。
`srv_init` 合法回复和 `srv_fs` provider 已迁移真实 Outbox；`srv_fs` 的旧阻塞泵、
手写 framing/回复路径和每请求 Runtime 已删除。当前尚未实现服务端幂等键或去重，
它们仍是协议扩展，不属于基本 Outbox 完成门。

`Dispatcher` 已改成可嵌入协议驱动；Runtime 提供有界 Wake 请求并保留 Gate 暂满时
的重试责任。`begin_for` 已由跨 provider Delegate 真实消费，后续消费者应沿用其
waiter/task 归属和停止收束契约。

同步 Caller 与异步出站任务共用 Request 阶段、绝对 Deadline 和 owner 语义；
同步 Caller 仍是阻塞门面，不另建协议状态机。公共单调时钟与绝对期限由
[`单调时间与 RPC 全调用期限`](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md)
接通。

host 覆盖全部 framing 分类；`srv_init` 真实双调用让第一条 protocol mismatch response 携带 capability，确认拒绝后 Handle 已 stale，再由同一 Caller 经新 ReplyPort 接收第二条合法 response。

FAL 对 RpcPrefix 的使用和当前 provider 见 [`fal.md`](fal.md)。
