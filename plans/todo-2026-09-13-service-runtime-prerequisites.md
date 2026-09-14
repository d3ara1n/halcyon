# 用户态运输、RPC 与服务执行前置

> 状态：当前下一实施任务，从运输 owner 与 Runnel 闭包开始。公共对象/观察/退休与公共时间已完成原交付，现有 rinlib/Runnel/RPC/libsrv 中仍有未接通草稿。ProcessDrain 的管理者职责和 REAPABLE 触发已澄清，不重做回收契约、不增加预算激励前置。[内核执行结构收束](todo-2026-09-14-public-operation-ownership.md) 与 [共享包整理](todo-2026-09-13-workspace-package-ownership.md) 独立安排在本执行前置后；实际发现阻断正确性的缺口时才按完整机制调整依赖。总体顺序见 [FAL 总计划](todo-2026-09-fal-service-capabilities.md)。

## 闭合目标与边界

公共内核保证状态合法性、持久观察和已提交对象维护责任；用户态执行基座保证任务驱动、背压、取消、下游调用及正常清理。用户直接调用 ABI、遗漏步骤或退出，不能使内核不变量依赖用户补调维护接口。用户库自己的完整操作 owner 应接管机械连接，业务只表达意图、处理结果和领域状态。

本任务接通 Packet/Delivery → Request/Task/Outbox → terminal → retire → refund。运输由 rinlib/Runnel 拥有；任务调度、观察注册寿命、任务唤醒与期限唤醒由执行核心拥有；RPC 路由及请求/回复责任由 librpc 拥有。执行核心不认识 FAL 节点、grant、Watch 或服务记录，也不依赖 RPC 的请求状态。

库依赖图必须单向：异步 RPC 消费执行能力，不能与 libsrv 的执行核心互相依赖。服务 schema 或控制协议若需要 RPC，与纯执行能力明确分层；包的最终拆分在类型/依赖图审视后决定，不用相互回调或临时 adapter 掩盖循环。

## 可以先行的局部收口

Runnel 新增 `Producer/Consumer::register` 与 `peer_attached` 直接访问 Guest.endpoint，而运行期 fail 可先成功关闭 Endpoint；后续查询便绕过 Channel 的终态检查，进入 `closed channel accessed its mapping` 的 expect。该缺陷位于 `librunnel/src/lib.rs` 的 Channel::fail、Guest::endpoint/close 与新增观察方法，可以在运输闭包开工时首先通过统一的终态访问边界修复并验证，不等待完整 Runtime，也不新增独立 todo。本次仅定位和立案，未修代码。

同时明确“可写空间”“EOF 后全部消费”“对端已建立”是不同等待条件；不能用当前 prepare_wait 的布尔结果代替全部操作。完整观察/取消 API 的调整仍属于下面的运输闭包，局部终态修复不能当作异步接口已经完成。

## 自然顺序：三个实施闭包与组合完成门

三个闭包依次实施，每项包含真实消费者迁移、失败/取消/退休和旧路径删除；不把“消费者迁移”列成靠后的独立施工阶段。每项可以有若干提交，但不能以未被真实责任消费的类型草稿标记完成。

### 运输 owner 与 Runnel 角色

目标是可直接使用的完整运输操作，不要求调用方维护已消费 owner 或共享协议的唤醒顺序。

1. 开工先确定内核运输契约：Delivery 保活是否需要独立对象身份；Peek 是否有独立无消费观察消费者，或由 Receive 的明确容量不足结果承接需求。两者是设计选择，不是预定删除项；必须保留交付责任、失败原子性、资源记账和资源不足时 Discard 的前进能力，不以少一个调用号作为理由。
2. 收束 Capability/Sender/SendOnce、Packet、ReceiveBuffer/MessageStorage 与 Delivery。未知能力在 typed 转换边界验证；正式构造已知的 role 不在每次重试重复 Query。成功投递消费 owner，失败返还完整 owner；消除用 delivered tombstone 表达仍可操作 Packet 的必要性。预付接收存储共用交接路径。
3. 完成 Runnel Producer/Consumer 的构造、Attach、部分进度、EOF/Broken 与 Endpoint cleanup。未消费 Invitation、已消费但角色未建立的 transport、运行期 terminal 各有完整失败 owner。raw ABI 只保留有真实用途的 unsafe 边界。
4. 将 ack→重查→登记/等待的正确协议封装进角色的推进与观察准备；业务不操作原始共享 cursor 或自行拼等待顺序。取消或遗忘准备操作只能影响自身协议进展，不能破坏内核映射与通知资源的归属。
5. 同步迁移 pm/init 与全部现有运输工厂和运行调用者，删除重复 close、raw 消费捷径和失败后丢失承载的分支。

完成门：正常、满箱、未 Attach、初始化失败、部分传输、对端退出与清理失败均有真实 owner；现有数据路径使用最终角色，旧路径删除；host/目标检查及相关 QEMU 运输组合通过。若选择改变 Delivery/Peek 的公开契约，必须先确认具体语义并同次迁移内核/shared/rinlib/所有消费者。

### 通用执行与准入

前置：运输闭包完成，实施者已核对现有 Close 的挂起/失败边界与操作 owner 的执行契约。现有 ProcessDrain 分工不作为缺失前置；若执行模型确实需要现有机制不具备的能力，再按证据提升完整专题。目标是在没有 FAL 的情况下，运行体也能独立承接现有数据处理及清理责任。

- Budget/Account/Charge 提供账户、额度、预留与退款机制；FAL 的 Node/Grant/Watch/ServiceRecord 等资源分类移回领域，不能不断扩充执行核心的枚举。
- 常驻监督是实际执行消费者：管理者纳管后观察 ProcessControl 的 REAPABLE/CLOSED；就绪后进入公平回收队列，More 继续安排下一批，失败保留 authority 并重试/升级。不让普通应用轮询 Drain，不先等待依赖 Drain 才发布的对端关闭；保持当前管理拓扑，不为本阶段改成唯一 pm 创建服务。
- Runtime 拥有稳定任务、来源与任务的绑定、注册寿命、arm generation/迟到事件过滤、期限唤醒和任务 Wake。WorkQueue 可以是内部算法，不要求调用者同时操作 WorkQueue 和 Runtime 才维持一致性。
- 提供绑定实际任务的唤醒能力；来源先发布工作再唤醒。业务无需手工建立 Notification、WaitSet token 和任务 ID 的隐含连接。退款只有令等待额度的任务可继续时才需唤醒；节点最后 pin 消散导致新的退休工作时必须唤醒。
- Runnable/Parked/Complete 由任务报告，运行体统一公平推进、停止准入和退休。保留不同领域的工作队列与语义身份，不合并 txid、NodeId 和 task ID，不要求领域另建事件循环。
- 完成 Close 可挂起时的执行上下文选择。每 step 只关闭一个 owner 不证明事件循环有界；不得在任意 Drop 中让唯一控制线程无限等待，也不得让用户分步维护内核对象作为补偿。Runnel 数据推进的失败路径也会经 Channel::fail 调用 Endpoint::close，不能仅凭 read/write 正常路径不等待就宣称整个操作不会停驻。
- 以现有 pm/init 的 Runnel 数据处理等实际责任接入，证明多任务公平、静默唤醒、期限、停止与最后退款；不创建只用于宣称前置通过的临时服务。当前非空 WorkQueue::drop 遗忘任务表，只增加诊断计数；运行体被放弃时必须明确由谁接管用户态任务/charge/注册，不能在长期存活服务中把异常遗忘当作正常取消。

完成门：运行体实际拥有机械注册/唤醒/退休连接；max_work=1 与多来源持续就绪仍有公平推进；停止和资源不足不会令任务失去 owner；真实消费者与清理路径使用最终机制，FAL 不作为唯一完成证据。

### 完整 RPC 与回复交付

前置：运输与通用执行闭包完成。同步 Caller 可以独立阻塞使用；异步 Dispatcher 单向接入公共执行核心，二者共享 framing、投递状态和期限语义，不强求相同的端口失效范围。

- Request/RequestContext/PreparedResponse、txid、ReplyPort 与 PendingCall 共同表达 Unsent/Sent/完成。Unsent 失败返还请求；Sent 超时、关闭或取消本地等待报告结果未知，不自动重试副作用。
- 同一 Deadline 覆盖发送背压、接收和最终回复接受；注册与期限唤醒交执行核心，协议自己的 Deadline 和 txid 仍由 RPC 持有，不能因去重丢掉独立语义。
- 在业务 Commit 前预付回复存储和发送额度，Outbox 随同一请求/回复 owner 持有 reply-once、Delivery 与准入。Outbox 的协议责任属于 RPC，排队、期限和唤醒消费执行核心；libsrv 不另建一份拥有相同回复的记录。
- 收束当前 Dispatcher::on_ready/expire/pop_completed/shutdown_step 与 Runtime::turn/wait 的组合；服务不再逐个驱动多个内部控制循环。RPC 完成 FIFO 仍可作为内部结果队列保留。
- 协议拒绝、迟到回复、附带能力、取消和退出统一走 owner 收束；不能在长期存活服务中依赖 mem::forget 等待整个进程退出完成正常清理。
- 同步迁移 init 的真实 RPC 验收、srv_fs 既有请求往返以及全部现有 RPC 调用点，删除旧阻塞泵、相对期限重试和重复 close。

完成门：请求到回复或放弃的每条责任链接通；满箱、Sent 后超时、迟到/畸形回复、服务退出与调用者退出均有真实验证；所有旧路径删除后再标 RPC 闭包完成。

## 组合完成门与提交

组合收口验证已经完成的三条闭包，不在最后阶段补主要功能或首次迁移消费者。执行适当 host/目标检查、just clippy、core/release/platform 及跨机制失败/取消/退款组合，证据能定位。完整 stress 的概率误判和原 Tunnel 静默截断仍由 [验收可靠性计划](todo-2026-09-13-acceptance-reliability.md) 独立拥有，未通过不得记为通过。

提交不按文件机械拆分 ABI/owner/观察/退休迁移；每个闭包登记真实调用者、删除的旧路径和验证证据。不能引入没有删除条件的 adapter 或测试专用运行体。已完成提交之后登记对应固定 hash 的未来 Review；提交、合并和 push 仍分别遵守授权边界。

## FAL 连接与唯一归属

本计划只迁移已有 FAL 调用的基础 API 使用；正式授权域、NodeStore/MemoryBackend/StoredValue、值与 wire、provider 装配属于 FAL 总计划。执行前置不以这些未接通草稿作为通用能力已成立的证明。

本计划的三项完成后，按总计划完成独立的共享包及内核执行结构整理，再进入 FAL 后端的准备/取消/替换值退休与稳定身份闭包，随后完成 grant/授权准入/provider/client 的共同迁移，最后扩展业务操作。后端产生的正常清理责任由后端持有并接入公共执行，不交给 handler 手工逐项 close；业务明确转移的能力才作为结果交付。具体责任与顺序唯一记录在总计划。
