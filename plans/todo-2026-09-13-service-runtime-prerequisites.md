# 用户态运输、RPC 与服务执行前置

> 状态：当前下一任务。消息运输闭包（`3060dd8`）与流运输/Runnel 闭包（`a2aabed`，未来复核见 [流运输 Review](todo-2026-09-14-stream-transport-review.md)）均已实施并提交；下一实施为通用执行与准入闭包，其设计闭包未完成前不得编码。公共对象/观察/退休与公共时间已完成原交付，现有 rinlib/Runnel/RPC/libsrv 中仍有未接通草稿。ProcessDrain 的管理者职责和 REAPABLE 触发已澄清，不重做回收契约、不增加预算激励前置。[内核执行结构收束](todo-2026-09-14-public-operation-ownership.md) 与 [共享包整理](archived/todo-2026-09-13-workspace-package-ownership.md) 已完成并归档；实际发现阻断正确性的缺口时才按完整机制调整依赖。总体顺序见 [FAL 总计划](todo-2026-09-fal-service-capabilities.md)。

## 开工流程与本任务审计门

本任务先遵循 `AGENTS.md`「标准施工流程」，再进入下面三个机制闭包。当前阶段只完成接手与基线复核；运输闭包实施前必须形成可追溯的任务规模审计、拆分/合并决策和设计记录。

### 规模审计

审计必须覆盖：`Capability/Sender/SendOnce`、`Packet/ReceiveBuffer/MessageStorage/Delivery`、Invitation/Endpoint、Runnel Producer/Consumer、WaitSet 观察、RPC/Outbox/Runtime 的真实依赖；内核/shared/rinlib 两侧；每个 owner 与 authority 的转移；正常、满箱、未 Attach、初始化失败、部分传输、EOF/Broken、对端退出、取消、期限、调用者退出、Close/Drain、退休和退款；锁序、跨 hart、停驻/唤醒；现有 raw/阻塞/重复 close 路径及其删除条件。审计必须区分已实现代码、未接通草稿和目标设计，不以源码存在或局部测试通过替代责任证据。

### 拆分/合并决策

按可独立证明的语义闭包调整任务：强耦合的 ABI、运输 owner、观察协议、失败/取消/退休和真实消费者必须共同迁移；只有能独立定义完成语义、失败边界、删除条件和验证门的部分才拆分。Delivery/Peek 的公开语义、Runnel 角色与其真实调用者不得先拆成孤立类型任务；Runtime/ProcessDrain/Close 的关系也需按停驻和接管责任判断，不预设全面内核重构为前置。审计结论进入本计划或其唯一子计划，发现新前置即同步 `COMPASS.md`。

首轮拆分已裁决（2026-09-14）：原运输闭包按机制拆为「消息运输 owner」与「流运输与 Runnel 角色」两个闭包，理由与共同约束：

- 两者内核对象面（Mailbox/Delivery 与 Tunnel/Endpoint）、真实消费者分布与失败矩阵独立，可分别定义完成门并分别删除旧路径。pm/init 同时使用两机制，消费者迁移按机制切片，每个闭包各自完成该机制的调用点迁移与旧路径删除，中间态不留双轨。
- 顺序偏好消息侧先行：Runnel attach 消费消息侧收束的 typed Invitation owner，接缝不引入 adapter。已登记的 Runnel 终态访问修复独立于该顺序，可先行。
- 两闭包共用同一 typed owner 纪律（成功投递消费 owner、失败返还完整 owner、raw ABI 只保留有真实用途的 unsafe 边界）；流闭包延续消息闭包定稿的 owner 形态，不得另立风格。设计门分开过：Delivery 身份与 Peek 是消息侧裁决；流侧无悬置内核 ABI（PEER_ATTACHED 已落地）。

这不是孤立类型任务拆分：每个闭包各自携带真实消费者迁移、失败路径与旧路径删除。

二轮拆分已裁决（2026-09-14）：观察/登记/取消的消费语义从流运输闭包移入通用执行闭包。基线 `d22b9d7` 随混合快照入库的 `Producer/Consumer::{register, peer_attached, prepare_wait, all_consumed}` 没有任何真实消费者，register/peer_attached 还在运行期 fail 后绕过终态检查访问已关闭 Endpoint（已立案缺陷）；它们是为 Runtime 事件驱动接管预留的草稿面，不是完成交付。流运输闭包按「残留即见即清」删除这四个方法——缺陷随之消失，阻塞路径内部协议保持私有；事件驱动的登记接入、三条件观察结果与取消语义由通用执行闭包与 Runtime 首个真实消费者共同设计落地，不预设形状。rinlib `EndpointEvents` 的事件登记是通用 ABI 设施而非 Runnel 专面，保留。

### 设计完成门

消息运输闭包编码前必须确定 Delivery 身份与 Peek 语义，完成 Capability/Packet/Receive/Invitation 的类型图、所有权图、状态机、线性化点、锁阶和失败/取消/退出/退款路径，并确定真实消费者迁移与旧路径删除顺序。流运输闭包编码前必须完成 Runnel 角色、typed Invitation 消费与阻塞路径失败边界的同类设计，并延续消息闭包定稿的 owner 形态；不设计任何观察/登记/取消 API（二轮拆分裁决移入通用执行）。通用执行闭包编码前必须确定 Runtime 任务、WaitSet 注册、期限、Park/Wake、Close/Drain 停驻和监督接管边界，并确定 Runnel 登记接入面与三条件观察结果的形态。RPC 闭包编码前必须确定 Request/Reply/Outbox 阶段、Deadline 覆盖范围、迟到回复和调用者退出语义。设计结论进入 `notes/ideas/`；实现事实进入 `notes/impls/`。

只有上述审计、拆分/合并和设计门完成后，才可将对应闭包标记为实施中；实施中发现证据推翻前提时，停止编码并回到审计/设计步骤。

## 闭合目标与边界

公共内核保证状态合法性、持久观察和已提交对象维护责任；用户态执行基座保证任务驱动、背压、取消、下游调用及正常清理。用户直接调用 ABI、遗漏步骤或退出，不能使内核不变量依赖用户补调维护接口。用户库自己的完整操作 owner 应接管机械连接，业务只表达意图、处理结果和领域状态。

本任务接通 Packet/Delivery → Request/Task/Outbox → terminal → retire → refund。运输由 rinlib/Runnel 拥有；任务调度、观察注册寿命、任务唤醒与期限唤醒由执行核心拥有；RPC 路由及请求/回复责任由 librpc 拥有。执行核心不认识 FAL 节点、grant、Watch 或服务记录，也不依赖 RPC 的请求状态。

库依赖图必须单向：异步 RPC 消费执行能力，不能与 libsrv 的执行核心互相依赖。服务 schema 或控制协议若需要 RPC，与纯执行能力明确分层；包的最终拆分在类型/依赖图审视后决定，不用相互回调或临时 adapter 掩盖循环。

## 可以先行的局部收口

Runnel 新增的 `Producer/Consumer::register` 与 `peer_attached` 直接访问 Guest.endpoint，而运行期 fail 可先成功关闭 Endpoint；后续查询便绕过 Channel 的终态检查，进入 `closed channel accessed its mapping` 的 expect。该缺陷位于 `librunnel/src/lib.rs` 的 Channel::fail、Guest::endpoint/close 与新增观察方法。

二轮拆分裁决（见「拆分/合并决策」）改修复为删除：这批无消费者草稿面（`register`/`peer_attached`/`prepare_wait`/`all_consumed`，随基线 `d22b9d7` 入库）在流运输闭包开工时或其之前直接删除，缺陷随之消失，不新增独立 todo。删除时复核无其它终态后 endpoint 访问路径（`wait_peer_closed` 有 check 前置，不受影响）；阻塞推进的 acknowledge→重查→wait 保持私有协议，已有 host 覆盖。

同时明确“可写空间”“EOF 后全部消费”“对端已建立”是不同等待条件；不能用布尔结果代替全部操作。完整观察/登记/取消 API 的设计与实现移入通用执行闭包，由 Runtime 作为首个真实消费者共同定形；本计划不保留任何无消费者观察面。

## 自然顺序：四个实施闭包与组合完成门

四个闭包依次实施，每项包含真实消费者迁移、失败/取消/退休和旧路径删除；不把“消费者迁移”列成靠后的独立施工阶段。每项可以有若干提交，但不能以未被真实责任消费的类型草稿标记完成。原「运输 owner 与 Runnel 角色」闭包已按机制拆分为消息运输与流运输两段，边界与共同约束见「拆分/合并决策」。

### 消息运输 owner

目标是可直接使用的完整消息运输操作，不要求调用方维护已消费 owner 或共享协议的唤醒顺序。

1. 开工先确定内核消息运输契约：Delivery 保活是否需要独立对象身份；Peek 是否有独立无消费观察消费者，或由 Receive 的明确容量不足结果承接需求。两者是设计选择，不是预定删除项；必须保留交付责任、失败原子性、资源记账和资源不足时 Discard 的前进能力，不以少一个调用号作为理由。
2. 收束 Capability/Sender/SendOnce、Packet、ReceiveBuffer/MessageStorage 与 Delivery。未知能力在 typed 转换边界验证；正式构造已知的 role 不在每次重试重复 Query。成功投递消费 owner，失败返还完整 owner；消除用 delivered tombstone 表达仍可操作 Packet 的必要性。预付接收存储共用交接路径。
3. 消息中的 transit 集合（含 Invitation）统一由 typed 叶 owner 收束；同步迁移 pm/init 与全部现有消息收发调用者，删除重复 close 与失败后丢失承载的分支。

完成门：正常、满箱、投递失败、接收写回失败、transit 回滚、对端退出与调用者退出均有真实 owner；现有消息路径使用最终 owner，旧路径删除；host/目标检查及相关 QEMU 组合通过。若选择改变 Delivery/Peek 的公开契约，必须先确认具体语义并同次迁移内核/shared/rinlib/所有消费者。

#### 规模审计结论与设计裁决（2026-09-14，基线 `e583d5e`）

内核/shared 侧已闭合，本闭包不触碰内核与 shared：Peek(0x42) 已实装且 `receive()` 两段式分配依赖它；Delivery 已是独立 KernelObject（kind=14，rights 仅 TRANSIT|GRANT，ReceiveResult.delivery 字段与 HandleQuery related_id 已落）；Send 满箱/期限/关闭失败零消费、moves/once 原子（selftest 断言准入不泄漏）；Receive 写回失败 rollback 恢复队头或锁外关闭 transit；Discard 丢队头并关闭全部 transit 与 Delivery；锁序 HANDLE_TABLE(100)→MAILBOX(210) 有秩栈断言。

两项裁决：

1. Delivery 独立身份保持现状——已是独立对象，rinlib 侧只欠持有/出口收束（Reply::accept 后无显式出口、detach_reply drop send-once owner）。
2. Peek 保留并正式化为公开 API——删除后退化为按 PAYLOAD_MAX 上限分配或 BufferTooSmall 盲试（错误码不携带所需尺寸），均劣于现状；无副作用队头观察对非阻塞推进有真实价值。

缺口全部在 rinlib 类型层与消费者：`Sender`/`SendOnce` 无类型身份，send-once 靠 raw Handle + 每次运行时 Query；`Packet` 用 `delivered: bool` tombstone 与 `Capability.handle=None` 双重表达消费；`publish`/`try_send`/`try_reply` 失败仅返回错误码，承载去向靠约定；`MintedSender`/`make_send_once` 工厂返回裸 Handle/泛化 Capability，未利用「内核构造已保证 role」这一事实；消息侧类型零 host 单测。消费者分布：librpc 全 typed（受 Packet 语义拖累）；pm 接收侧 typed、发侧 raw；init 创建侧全 raw（Invitation 以裸 HandleMove 发送，失败后承载无 owner 管理，`srv_init/main.rs:1007-1022`）；srv_fs v1 泵全 raw 且有失败后 close 丢弃 reply 的模式。验收/竞态代码（time_checks/race/public_ipc/test_hammer）的 raw `send_raw_until` 等是对 unsafe 边界与内核契约的刻意验证，保留为 raw，不列为迁移对象。

实施要点（自底向上）：

1. `MintedSender`/`make_send_once` 工厂直接产出 typed owner（内核构造已保证 role，不再 Query）；未知能力在唯一转换边界 Query 一次后进入 typed owner，重试路径不再重复 Query。
2. `Packet` 改消费式投递：`try_send(self)` 成功返回回执并消费 Packet，失败返还完整 Packet；删除 delivered tombstone；push 时 Query 一次并缓存 role/rights，同 owner 重试不再 Query。
3. 收束 Delivery 出口：`Reply`/`RequestContext` 提供显式 delivery 访问/移交；`detach_reply` 返还 send-once owner 而非 drop。
4. 共同迁移：librpc exchange/caller/dispatcher（Packet 消费式 + typed 目标）、pm 发侧、init 创建侧消息面（Packet+typed Invitation 发送；Runnel 工厂内部仍 raw，属流闭包）、srv_fs v1 泵消息面（保留泵结构，只换 owner；泵整体删除在 FAL 段）。
5. 补消息侧 host 单测：push/pop 失败返还、try_send 失败返还、HandleSet take 边界、tombstone 消除后的类型状态断言。

#### 实施记录（2026-09-14）

已按上述要点完成：`MailboxSender`/`SendOnce` typed owner（铸造零 Query、转换边界单次 Query、`close`/`into_capability`/`into_raw` 完整出口）；`Packet` 消费式 `try_send(self)`/`try_reply(self, once)` 与 `SendFailure`/`ReplyFailure` 完整返还，delivered tombstone 删除；`MintedSender.sender`、`send_once` 返回 typed；`RequestContext.reply` 改 typed SendOnce（decode 校验后摘取），`detach_reply` 返还 owner，`PreparedResponse::try_send` take/restore。真实调用者迁移：librpc exchange/caller/dispatcher、pm 流控发侧、init 生产侧 Invitation 转移（失败路径不再丢裸 HandleMove 承载）、srv_fs v1 泵全消息面（含失败分支显式关 owner）。删除的旧路径：Packet tombstone、try_send/try_reply 每次重试 Query destination、srv_fs 失败后 close 丢弃 reply、init 失败后 HandleMove 丢失。验收/竞态夹具（time_checks/race/public_ipc/test_hammer）保留 unsafe raw 直验内核契约，属计划内 raw 边界用途。验证：rinlib host 7 项（新增 Packet/HandleSet 4 项）、用户态四框架 host 68 项、七面 clippy、virt core 与 virt-release 全绿；内核与 shared 未改动。

### 流运输与 Runnel 角色

前置：消息运输闭包完成——Runnel attach 消费消息侧收束的 typed Invitation owner，接缝不引入 adapter。[无消费者观察面删除](#可以先行的局部收口) 是唯一例外，独立于闭包顺序可先行。

1. 完成 Runnel Producer/Consumer 的构造、Attach、部分进度、EOF/Broken 与 Endpoint cleanup。未消费 Invitation、已消费但角色未建立的 transport、运行期 terminal 各有完整失败 owner。raw ABI 只保留有真实用途的 unsafe 边界。
2. 阻塞推进路径（acknowledge→重查→wait）保持为角色内部协议并受 host 测试覆盖；业务不操作原始共享 cursor 或自行拼等待顺序。删除无消费者观察草稿面（register/peer_attached/prepare_wait/all_consumed）；事件驱动的登记/观察/取消接入面属通用执行闭包，本闭包不实现无真实消费者的 API。
3. 同步迁移 pm/init 与全部现有运输工厂和流路径调用者，删除 raw 消费捷径和失败后丢失承载的分支。

完成门：未 Attach、初始化失败、部分传输、EOF/Broken、对端退出与清理失败均有真实 owner；现有数据路径使用最终角色，旧路径与无消费者观察面删除；host/目标检查及相关 QEMU 运输组合通过。

#### 实施记录（2026-09-14）

已按上述范围完成：删除随基线 `d22b9d7` 入库的无消费者观察草稿面（`Producer/Consumer::{register, peer_attached, prepare_wait, all_consumed}`），register/peer_attached 在运行期 fail 后绕过终态检查访问已关闭 Endpoint 的缺陷随之消失（复核删除后无其它终态后 endpoint 访问路径，`wait_peer_closed` 有 check 前置）；删除原始 ABI 工厂（`create/attach_producer/consumer`），typed `Producer/Consumer::create/attach` 成为唯一构造入口；srv_init 数据面创建侧迁移至 `Consumer::create`，Invitation 以 typed owner 直接 `into_capability()` 进 Packet，消除 `Capability::from_raw` 接缝，`CreateFailure::Protocol` 双 owner（本地映射与未发布邀请）显式关闭并记录。pm 接收侧已 typed，不变；srv_init 自检/race 与 test_hammer 的 rinlib 原始 tunnel 调用属刻意内核契约验证，保留为 raw。阻塞推进的 acknowledge→重查→wait 保持私有协议，host 覆盖不变。验证：用户态框架 host 95 项（librunnel 15）、七面 clippy、virt core 与 virt-release 全绿；内核与 shared 未改动。观察/登记/取消接入面按二轮拆分裁决移入通用执行闭包。

### 通用执行与准入

前置：消息与流运输闭包完成，实施者已核对现有 Close 的挂起/失败边界与操作 owner 的执行契约。现有 ProcessDrain 分工不作为缺失前置；若执行模型确实需要现有机制不具备的能力，再按证据提升完整专题。目标是在没有 FAL 的情况下，运行体也能独立承接现有数据处理及清理责任。

- Budget/Account/Charge 提供账户、额度、预留与退款机制；FAL 的 Node/Grant/Watch/ServiceRecord 等资源分类移回领域，不能不断扩充执行核心的枚举。
- 常驻监督是实际执行消费者：管理者纳管后观察 ProcessControl 的 REAPABLE/CLOSED；就绪后进入公平回收队列，More 继续安排下一批，失败保留 authority 并重试/升级。不让普通应用轮询 Drain，不先等待依赖 Drain 才发布的对端关闭；保持当前管理拓扑，不为本阶段改成唯一 pm 创建服务。
- Runtime 拥有稳定任务、来源与任务的绑定、注册寿命、arm generation/迟到事件过滤、期限唤醒和任务 Wake。WorkQueue 可以是内部算法，不要求调用者同时操作 WorkQueue 和 Runtime 才维持一致性。
- 提供绑定实际任务的唤醒能力；来源先发布工作再唤醒。业务无需手工建立 Notification、WaitSet token 和任务 ID 的隐含连接。退款只有令等待额度的任务可继续时才需唤醒；节点最后 pin 消散导致新的退休工作时必须唤醒。
- Runnable/Parked/Complete 由任务报告，运行体统一公平推进、停止准入和退休。保留不同领域的工作队列与语义身份，不合并 txid、NodeId 和 task ID，不要求领域另建事件循环。
- 完成 Close 可挂起时的执行上下文选择。每 step 只关闭一个 owner 不证明事件循环有界；不得在任意 Drop 中让唯一控制线程无限等待，也不得让用户分步维护内核对象作为补偿。Runnel 数据推进的失败路径也会经 Channel::fail 调用 Endpoint::close，不能仅凭 read/write 正常路径不等待就宣称整个操作不会停驻。
- 以现有 pm/init 的 Runnel 数据处理等实际责任接入，证明多任务公平、静默唤醒、期限、停止与最后退款；不创建只用于宣称前置通过的临时服务。当前非空 WorkQueue::drop 遗忘任务表，只增加诊断计数；运行体被放弃时必须明确由谁接管用户态任务/charge/注册，不能在长期存活服务中把异常遗忘当作正常取消。

完成门：运行体实际拥有机械注册/唤醒/退休连接；max_work=1 与多来源持续就绪仍有公平推进；停止和资源不足不会令任务失去 owner；真实消费者与清理路径使用最终机制，FAL 不作为唯一完成证据。

### 完整 RPC 与回复交付

前置：消息/流运输与通用执行闭包完成。同步 Caller 可以独立阻塞使用；异步 Dispatcher 单向接入公共执行核心，二者共享 framing、投递状态和期限语义，不强求相同的端口失效范围。

- Request/RequestContext/PreparedResponse、txid、ReplyPort 与 PendingCall 共同表达 Unsent/Sent/完成。Unsent 失败返还请求；Sent 超时、关闭或取消本地等待报告结果未知，不自动重试副作用。
- 同一 Deadline 覆盖发送背压、接收和最终回复接受；注册与期限唤醒交执行核心，协议自己的 Deadline 和 txid 仍由 RPC 持有，不能因去重丢掉独立语义。
- 在业务 Commit 前预付回复存储和发送额度，Outbox 随同一请求/回复 owner 持有 reply-once、Delivery 与准入。Outbox 的协议责任属于 RPC，排队、期限和唤醒消费执行核心；libsrv 不另建一份拥有相同回复的记录。
- 收束当前 Dispatcher::on_ready/expire/pop_completed/shutdown_step 与 Runtime::turn/wait 的组合；服务不再逐个驱动多个内部控制循环。RPC 完成 FIFO 仍可作为内部结果队列保留。
- 协议拒绝、迟到回复、附带能力、取消和退出统一走 owner 收束；不能在长期存活服务中依赖 mem::forget 等待整个进程退出完成正常清理。
- 同步迁移 init 的真实 RPC 验收、srv_fs 既有请求往返以及全部现有 RPC 调用点，删除旧阻塞泵、相对期限重试和重复 close。

完成门：请求到回复或放弃的每条责任链接通；满箱、Sent 后超时、迟到/畸形回复、服务退出与调用者退出均有真实验证；所有旧路径删除后再标 RPC 闭包完成。

## 组合完成门与提交

组合收口验证已经完成的四条闭包，不在最后阶段补主要功能或首次迁移消费者。执行适当 host/目标检查、just clippy、core/release/platform 及跨机制失败/取消/退款组合，证据能定位。历史 stress 概率误判和 Tunnel 墙钟敏感截断见 [只读归档](archived/ref-2026-09-acceptance-timing-flake.md)；新现场命中其触发条件时重新立案，不以重跑直到绿替代证据。

提交不按文件机械拆分 ABI/owner/观察/退休迁移；每个闭包登记真实调用者、删除的旧路径和验证证据。不能引入没有删除条件的 adapter 或测试专用运行体。已完成提交之后登记对应固定 hash 的未来 Review；提交、合并和 push 仍分别遵守授权边界。

## FAL 连接与唯一归属

本计划只迁移已有 FAL 调用的基础 API 使用；正式授权域、NodeStore/MemoryBackend/StoredValue、值与 wire、provider 装配属于 FAL 总计划。执行前置不以这些未接通草稿作为通用能力已成立的证明。

本计划的四项闭包完成后，按总计划完成内核执行结构整理（共享包契约与归属已归档），再进入 FAL 后端的准备/取消/替换值退休与稳定身份闭包，随后完成 grant/授权准入/provider/client 的共同迁移，最后扩展业务操作。后端产生的正常清理责任由后端持有并接入公共执行，不交给 handler 手工逐项 close；业务明确转移的能力才作为结果交付。具体责任与顺序唯一记录在总计划。
