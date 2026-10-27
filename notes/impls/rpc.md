# 通用 RPC 实现

通用 framing、调用器及订阅会话位于 `user/libraries/librpc`；`libkernel` 提供 Mailbox、Handle、Wait/WaitSet、时间等底层封装，不解释 RPC。领域协议依赖 librpc 的运输与会话机制，提供自己的编解码和业务行为。

## Framing

`RpcPrefix` 是 16 字节 little-endian 头：版本、message kind（Request/Response/Oneway）、reserved 与非零 txid。解码拒绝未知版本/kind、非零 reserved 和零 txid。

期待回复的 Request 在 Handle slot 0 放置裁剪为 `WRITE | TRANSIT` 的 send-once；内核只运输该 capability，不理解 txid 或 request/response。`sender_pid` 只提供来源信息，immutable badge 承载服务端铸造的 session/grant 上下文。

## 同步 Caller

`Caller` 懒创建并复用线程私有 ReplyPort；同一实例只允许一个 outstanding call。`CallOperation` 借用 Caller 及其 ReplyPort，保存发送前 Request、Deadline 与可选取消能力；同步 `call_cancelable` 驱动它。操作状态严格为 `Unsent(Request)`、持 ReplyObserver 的 `Sent` 或 `Completed`：

1. 从独立 `monotonic_id` domain 分配单调非零 txid 并编码 RpcPrefix；最大值后永久返回 ReachLimit，不回绕路由旧回复；
2. 从 ReplyPort sender 派生 send-once，作为 slot 0 与业务 Handles 一起投递；
3. CallWait 拥有持久扁平 WaitSet，观察 ReplyPort READABLE/CLOSED、服务 WRITABLE/CLOSED、取消 READABLE/CLOSED 与 ReplyObserver ABANDONED（PEER_CLOSED）；发送后注销旧服务来源并只留 CLOSED，仍使用同一绝对 Deadline。Unsent 在再次投递前复核取消；Sent 合法回复先于取消；到期直接查询真实来源，不把空候选视为未就绪；
4. 接收后由纯逻辑 `validate_response` 验证 protocol id、Response kind 与 txid；任一拒绝都逐项关闭已安装 Handle、废弃当前 ReplyPort，再返回 typed rejection。

同步驱动在 ServiceClosed、Wait/Receive 错误、timeout 和外部取消时废弃当前 ReplyPort；可进入 transit 的 role 都是固定上界叶 close（Tunnel Endpoint 不具 TRANSIT），reject cleanup 不建立新的异步 owner。`CallOperation::advance` 只做一次非阻塞发送或回复尝试；`wait_ready` 按未发送/已发送分别观察 service writable 或 ReplyPort，并观察 service close/cancel。成功接收回复进入 `Completed`，完成操作不能再次推进；`abort` 按阶段返还原 Request 或 ReplyCleanup。已发送操作被显式 abort 或直接 Drop 时都会隔离旧 ReplyPort，迟到回复不能进入下一调用。`CallError` 的互斥 owner 是 `Unsent(Request)`、`Reply(ReplyCleanup)` 或 None：发送前拒绝保留原请求；发送后未确认按 Sent 返回未知，关闭失败保留 typed owner 供 `retry_cleanup`。RPC 不自动重试可能有副作用的已投递请求。

分步接口由 `librpc::CallOperation` 直接提供；init 的 cancellable RPC 消费可观察阶段、非阻塞推进、等待、abort 和完成回复。文件客户端直接组合 Caller 与领域编码，Copy 和 Watch 的异步操作组合 Dispatcher；不存在旧 `libfal::ClientOperation` 包装层。`caller.rs` 仅在 RISC-V 目标编译，正式消费者覆盖未投递/已投递取消、迟到回复隔离与结果领取；host framing/OutboundStage 测试不替代真实运输行为。

## 异步出站任务

[`dispatcher.rs`](../../user/libraries/librpc/src/dispatcher.rs) 的 `Dispatcher` 是
`libexecution::runtime::Task<()>`：它只拥有
PendingCall、txid 路由、Request/Reply 阶段、ReplyPort、MessageStorage 和
完成 FIFO。来源登记、事件输入、期限唤醒、来源注销重试、
任务停止和最终退休全部由 Runtime 拥有。Dispatcher 通过 SourceRegistration 向 Requests 声明持久观察和移除，在 `Input` 中消费 `SourceEvent`；不再持有独立
WaitSet、来源表、直接期限推进或 `mem::forget` 放弃循环。

出站阶段由纯逻辑 `OutboundStage` 表达：`Ready → WaitingWritable → Ready`
可因满箱重试，成功投递进入 `Sent`，超时、关闭、拒绝或合法回复进入
`Terminal`。该状态机有 host testcase；开发验证与未覆盖范围由相应工作记录承载。实际嵌入消费者为 srv_fs 的 Bind/Boundary 下游与发布/撤出，以及 init 的文件/服务 Watch、Copy 和独立付款调用；它们共享各自宿主运行体。

## 当前嵌入与边界

`Dispatcher` 由宿主 World 持有，运输任务负责 advance、期限及来源回调，业务任务通过 World 借用提交调用、接收完成；它不拥有第二套 WaitSet，也不在 handler 中同步等待。完成进入有界 FIFO，由原任务取得并继续推进，停止时先取消或收束下游调用，再注销来源、关闭回复端点并退休 Dispatcher owner。srv_fs 的正常发布和撤出通过同一 Dispatcher 推进，生命周期和角色交付由[服务装配](service.md#进程装配与行为限制)拥有。init 在根就绪后复用原 ReadDuty 并发调用 A/B 服务目录，验证各私有来源实际付款/退款及共享 H 归属；provider 不为这组验收调用预留槽或延迟 Ready。

Dispatcher 的调用期限索引为侵入式 TimerQueue<txid>，每个 PendingCall 在提交前 fallible 分配 pin Box 节点；终态取消节点后才摘路由/释放调用，非空 Dispatcher Drop 先排空索引。最早期限继续向 Runtime 的任务期限入口声明，没有独立硬件时钟或执行循环；底层节点/队列契约见[时间实现](time.md)。每调用保留服务 CLOSED 与一个 progress 来源槽：Writable 实际注销回执后，binding role 转 Abandoned，同一槽登记 ReplyObserver。事件核 SourceId，终态保留观察权到来源收束，不增加第三个每调用槽。

Dispatcher 的 new 明确借用共享 reply 的 sponsor H 并以 SourceRegistration::funded 出生绑定；begin/begin_for 借用每次调用的 execution view，PendingCall 的 CLOSED、WRITABLE 与 ReplyObserver 来源以 Bound 模式克隆保留原 view。共用 Dispatcher 的不同调用因此可以独立计费，event、phase 清空或来源重装不按当前推进任务重新决定付款。SourceFunding 的命令解析与实际 Charge 退休由 [执行框架](framework.md#libexecution-的执行与扁平装配)拥有。

`begin/begin_for` 接受 `ServiceOwner`：`Owned(MailboxSender)` 保管独占 typed 角色，`Shared(Arc<MailboxSender>)` 保活同一表项；共享不派生 handle，也不要求 DUPLICATE。准入仍核 WRITE|WAIT；准入失败和完成原样返还输入 owner 变体，实际来源退出前不能领取完成。Watch 直接恢复独占 sender，派生调用的临时独占 sender 随完成值在解码后释放；均不为普通叶 Close 另建关闭重试槽。CopyTask 的路径调用持 Shared，实际 Namespace 入口无 DUPLICATE；控制会话仍独占提交。call_phase 只观察活调用的投递阶段，ignored_reply_count 记录实际未对应活调用的回复，不参与路由或业务判断。

Dispatcher 停止时保留仍有 waiter 的完成结果，原任务通过同一 txid 领取；失败 wake 回执撤销 waiter 后，由停止退休扫描收束。外层正常停机先让业务任务清理至 Complete，再停止 Dispatcher，避免把活任务所持领取责任交给完成 FIFO 的丢弃路径。

Dispatcher 保留协议状态、PendingCall、回复存储、期限和运输 owner；Runtime 仍拥有
WaitSet、来源登记、期限唤醒和最终退休。后续增加消费者时复用这一嵌入边界，不恢复
独立 WaitSet、阻塞泵、无限重试或业务轮询完成队列。

入站最终顺序是先准入 `PreparedResponse`/Outbox、任务和来源，再执行业务副作用；
回复失败只报告交付结果，不伪造业务回滚。`srv_fs`在Commit前预备Outbox及CLOSED观察；正文就绪后才尝试发送。满箱使阶段转WaitingWritable，每次发送尝试后立即协调所需来源，声明撤销旧CLOSED-only登记；在实际注销回执后登记WRITABLE|CLOSED，未完意图/拒绝重试保持Runnable。即使推进预算1也不在缺少WRITABLE登记时Parked，reply-once和原绝对期限由同一Outbox持有；Delivery通常随请求保管，领域把操作交给独立长期owner后可显式finish_delivery结束入口use，失败保留原Delivery并重试，不释放回复权。成功或失败终态注销来源后退休，SourceRequest不重复发布。具体组合验证与未覆盖窗口见对象链工作记录。

Outbox prepare/from_prepared 明确取得并克隆出生 SourceFunding，CLOSED 到 WRITABLE、取消及实际注销不会更换绑定，没有 set_payer。领域 Request 以 Task 模式把来源费用交给已冻结的任务 view；未登记且尚未任务准入的纯拒绝可以由原 Ingress 的 H 作用域推进，其领域 payload owner/Charge仍由原领域值保留。同步 Caller/ReplyCleanup 不持执行来源，不增加付款字段。

内核消息付款与上述执行来源费用分开：回复使用的 Once 由 make_send_once 发行进程 Pool 支付本次输入 scratch、正文、能力数组和 Delivery 存储，排队未读及接收后尾部按各自寿命保留原 payer。响应者的用户态回复缓冲、Request 与来源仍由服务自身预留付款。发行者额度不足可使 Outbox Abandoned；不能据回复 Unsent 推断业务未提交，也不承诺所有回复必成。

## 带序号确认的订阅客户端

[subscription::ConsumerTask](../../user/libraries/librpc/src/subscription.rs) 拥有控制 sender、NotificationObserver、单个 SourceRegistration、捕获的领域快照及在途 Query/Ack。具体领域的 Watch 实现静态 Protocol：移交两个真实角色及初始快照、按领域协议编码/校验调用，并把快照投影为 sequence/pending/terminal；通用会话不解释事件位、路径、文件种类或服务名字，也不拥有 provider 状态。fs/service 保留独立 WatchInfo 与 Status。Query/Ack 无业务输入能力，正常回复无能力。

会话依次经过 Listening→Removing→Query/InFlight→Reading→Ack/InFlight。READABLE 锁存后先实际注销通知来源，再发 Query；Reading 保管原序号并停驻，应用完成真实重查后 acknowledge_read 才发 Ack。Ack 完成后按返回 pending 重新监听，来源登记重新检查当前电平，终态且无待确认事件转 Retiring。在途时不重复登记持续就绪的来源；来源采用 Task 付款，PendingCall 从当前任务视图冻结 Bound 付款。

取消、期限或 CLOSED 只冻结停止意图：已有 txid 继续由 Dispatcher 交付完成，恢复独占控制后注销通知来源，再释放两个角色。Dispatcher 返还 CallError 前已收束调用来源；未发送 Request 的 Packet 与两个非映射角色均沿 typed Drop 必成关闭，客户任务不另存 CallError 或本地 Close 重试状态。真实来源未收到 unregistered 前仍持有观察角色。准入前 abandon_unadmitted 不发 RPC、不建来源。该类型不创建 Runtime/Assembly；宿主将客户和唯一 Dispatcher 放在同一装配内，并保留客户任务至共享运输来源真正退出。

## 当前边界

`RequestContext`、`PreparedResponse` 与 Outbox 已闭合：Outbox 持有请求
Delivery、reply-once、预付回复存储和 Runtime 来源，先完成来源准入再允许业务
Commit，发送失败进入 `Abandoned` 并保持已提交业务事实，来源注销后任务才退休。
尚未成功投递时，`PreparedResponse::drain_capabilities` 与 Outbox 同名出口可逆序逐项
取回 Packet 中的业务 capability；该出口不复制 owner，也不访问已被成功发送消费的
Packet。它只提供运输 owner 的显式收束点，业务是否已经提交、能否恢复仍由上层协议
状态机决定；文件操作的提交与未知结果边界见[文件实现](fs.md)。
`srv_init` 合法回复和 `srv_fs` provider 已迁移真实 Outbox；`srv_fs` 的旧阻塞泵、
手写 framing/回复路径和每请求 Runtime 已删除。当前尚未实现服务端幂等键或去重，
它们仍是协议扩展，不属于基本 Outbox 完成门。

`Dispatcher` 可嵌入协议驱动；Runtime 提供有界 Wake 请求并保留 Gate 暂满时的重试责任。`begin_for` 由文件/服务 Watch ConsumerTask 与 CopyTask 使用，来源事件与结果唤醒回到各自 waiter/task。init 原客户段可准入多个独立 Copy 与一个共享 Dispatcher，先让所有客户物理清理完成，再封运输，后者退出后唤醒原客户完成任务退休。

同步 Caller 与异步出站任务共用 Request 阶段、绝对 Deadline 和 owner 语义；
同步 Caller 仍是阻塞门面，不另建协议状态机。公共单调时钟与绝对期限由
[`单调时间与 RPC 全调用期限`](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md)
接通。

host 覆盖全部 framing 分类；`srv_init` 真实双调用让第一条 protocol mismatch response 携带 capability，确认拒绝后 Handle 已 stale，再由同一 Caller 经新 ReplyPort 接收第二条合法 response。

文件与服务目录对 RpcPrefix 的使用分别见[文件实现](fs.md)和[服务目录实现](service.md)。
