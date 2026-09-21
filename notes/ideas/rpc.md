# 通用 RPC

Mailbox 是 RPC 传输原语，内核不感知请求与应答。分层为：

1. Mailbox 运输消息、业务 capability、内核产生的发送授权身份与 provenance，以及独立 Delivery；
2. RpcPrefix 描述固定宽版本、request/response/oneway 与非零 txid；期待回复的 request 在业务 Handle slot 0 放置 send-once；
3. FAL、服务与设备协议定义自己的版本、kind、错误和 payload。

send-once 证明一次回复的投递权，txid 负责回复关联，发送授权身份与 badge 指向服务端上下文，sender PID 只是 provenance。Delivery 保留这条消息的处理责任，不证明业务完成，也不代替回复权。

## 所有权与投递阶段

调用的运输状态明确区分 Unsent、Sent 和 Completed。Unsent 拥有完整请求及尚未搬出的 capability；成功 Send 后进入 Sent，原能力不再属于调用者。投递前失败或到期返还未转移的 owner；投递后超时、服务退出或丢失回复只能报告业务结果未知。

收到的请求和回复先由运输 owner 持有，协议提取后才形成业务 owner。畸形、迟到、重复或不匹配的回复统一关闭未提取能力及 Delivery。请求上下文保留 Delivery，直到处理、回复或拒绝责任结束；异步处理时把整个上下文交给任务。

ReplyPort sender 派生的 send-once 保持同一发送授权，以 WRITE、WAIT、TRANSIT 进入请求。WAIT 允许服务在满箱时等待可写或关闭，但服务 outbox 必须有资源上限和期限，不能阻塞整个请求循环。

## 并发与回复路由

Mailbox 严格 FIFO，无选择性 Receive。同步 Caller 私有 ReplyPort，同一 port 同时最多一个 outstanding call；失败或超时可以关闭并废弃整个 port。

异步多 in-flight 使用一个 dispatcher 按不复用的 txid 路由。单个请求超时只退休该 pending 项，不能关闭其他请求共享的回复端口。不存在 pending waiter 的回复连同能力一起丢弃，不保留无限 tombstone。

同步与异步调用共享 framing、投递状态、Deadline 和 capability 所有权模型，端口的失效范围由各自路由结构决定。未完成的请求、待发送回复及其观察注册都必须经过有界准入。

## Deadline、取消与重试

调用入口把相对时长一次转换为 [绝对 Deadline](time.md)，同一值覆盖投递背压、回复等待、Receive 与最终 framing 接受。内核 Send 在提交点核验投递期限，内核等待直接使用绝对时点，不通过反复相对等待延长预算。

一条回复只有在关联和 framing 合法、且最终接受检查仍未过期时才完成调用。就绪观察本身不保证仍在期限内。已经接受的回复不因随后服务关闭而改成未执行；尚未确认的副作用不能因超时推断未执行。

Timeout 只停止本地等待。服务取消、幂等键、去重和重试属于协议扩展，不下沉为内核强取消。库不得自动重试已投递的有副作用调用。有限 Deadline 的可表达性、精度和溢出由时间契约统一定义。

## 服务执行

服务可以同步处理短操作，或把 RequestContext 移交给有界任务。发送回复失败不自动回滚已提交的业务修改；只有协议显式定义的发布事务，例如一次性属性 Take，才以回复成功入箱作为业务提交点。

控制循环通过 [WaitSet](wait.md) 推进 Mailbox、下游 PendingCall 和 outbox。来源关闭、观察失败、期限到达与正常完成都进入同一任务收束路径。

通用 framing 与调用状态属于 librpc；任务推进、观察与执行准入属于 libexecution；服务组合政策属于 libservice；rinlib 只封装内核对象、运输、时间和等待。

## Runtime 接缝

RPC/Outbox 是一个机制闭包，出站与入站只按依赖顺序施工，不各自形成验收任务。Runtime 是唯一的用户态执行与观察拥有者：它负责 WaitSet 来源登记、arm generation、事件输入、绝对期限唤醒、注销重试、任务公平推进、停止和最终退休。RPC 不再持有自己的 WaitSet、来源表、重臂循环、期限驱动或进程级放弃循环。

RPC 任务只拥有协议状态：PendingCall、txid 路由、Request/Reply 阶段、ReplyPort、RequestContext、Outbox、Delivery 和业务结果。Dispatcher 是可嵌入的协议推进器，不以固定 `Task<()>` 作为服务组合边界；最终由服务自己的任务族统一实现 `Task<ServiceWorld>`，转发 Dispatcher 与入站/业务任务的 Runtime 回调。RPC 通过 Runtime 的任务请求声明来源登记、重臂与移除，并在任务输入中消费来源事件；任务间提交和完成通过 Runtime 应用的有界唤醒请求连接，Runtime 不保存 txid 或业务状态。Runtime 回执来源身份和注销结果，RPC 保留对应状态直到回执完成。入站 Outbox 必须在业务 Commit 前完成回复存储、任务和来源准入；回复失败只终结交付责任，不伪造业务回滚。协议期限仍由 RPC 保存并决定业务超时，Runtime 只根据任务报告的最近绝对期限负责唤醒。

同步 Caller 是同一出站状态机的阻塞门面，不维护第二套投递或错误阶段语义。`Dispatcher` 的最终形态是 RPC 状态/路由任务，不是独立事件循环；`RequestContext`、`PreparedResponse` 与 Outbox 共同持有入站请求至回复终结的责任。业务 Commit 前必须完成回复存储、发送额度、任务和来源准入，Commit 后回复失败只报告结果，不伪造业务回滚。

施工阶段只为稳定的 framing、所有权转换和状态推进函数保留 testcase。真实消费者迁移、旧阻塞泵删除、失败/取消/退出/退款闭合和跨机制组合验证在整个 RPC/Outbox 闭包收口时统一完成。
