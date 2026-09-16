# 用户态运输、RPC 与服务执行前置

> 状态：消息运输（`3060dd8`）和流运输/Runnel（`a2aabed`）已提交。通用执行与准入由 HighHolly 接手重构，实现、确定性回归、完整 acceptance 与集中复核均已完成，已提交为 `a3891b0`；固定提交复核登记于 [未来 Review](todo-2026-09-15-runtime-admission-review.md)。唯一修复/复核真值见 [Runtime 闭包报告](archived/review-2026-09-15-runtime-closure.md)，实现见 `notes/impls/runtime.md`。RPC/Outbox 已进入第一阶段施工，FAL 业务未进入施工。公共对象和时间已交付；[公共操作所有权](todo-2026-09-14-public-operation-ownership.md) 仍待实施，[共享包整理](archived/todo-2026-09-13-workspace-package-ownership.md) 已归档。当前只为用户态预付退休槽给 shared/timer_queue 补载荷绑定接口，没有修改内核或 shared ABI。

提交后的结构审视见[整体固定提交 Review](todo-2026-09-15-runtime-admission-review.md)：PM 实际停止的旧声明已更正，并记录失败交付、分页、核心状态、Runtime Wake、RPC/Outbox 和真实消费者边界的收敛建议；Runtime 清理、Job 单页收束、停止补证及 RPC/Outbox 已完成，固定提交序列统一由该 Review 承载。服务架构化不在范围内。

## RPC/Outbox 当前接力状态

当前接力已完成任务规模审计与设计收口，并完成第一阶段“出站状态与 Runtime 接缝”：`e0b5c45` 已将 Dispatcher 的协议状态接到 Runtime 任务推进，删除自持 WaitSet、来源登记、直接重臂/期限推进和 `mem::forget` 放弃循环；纯逻辑 `OutboundStage` 与有界 `Sweep` 已补 host testcase；用户态 RISC-V `librpc` check、`just check`、七面 `just clippy` 与 `librpc` host testcase 通过。

2026-09-16 的接续重审确认：这仍是同一个 RPC/Outbox 闭包，`e0b5c45` 是可复用的铺路基线，不需要另立 todo 或推翻出站 owner/阶段设计；但“出站阶段已完成、只剩入站”这一表述过强。当前尚未闭合 Dispatcher 与服务任务族的最终组合、任务间完成唤醒、业务 Commit 前的任务/来源/回复准入，以及回复故障后的完整退休。后续会话从同一计划继续，先完成下节列出的局部修正与设计收口，再实施入站 `RequestContext`/`PreparedResponse`/Outbox、真实消费者迁移、旧阻塞泵删除和最终组合验证。

## RPC/Outbox 接续重审与实施裁决（2026-09-16，基线 `e0b5c45`）

### 范围结论

本次是同一闭包的中途接力，不拆出新的“出站”或“Outbox”任务。已提交的 `Request`/Packet 消费式 owner、`PendingCall`/txid 路由、绝对 Deadline、共享 ReplyPort、来源注销等待、`OutboundStage` 与有界 `Sweep` 均保留；重审只修正会阻断最终组合的边界，不回滚已验证的运输和 Runtime 基础。

### 继续施工前的必要修正

1. 所有故障停止入口统一进入幂等的停止/注销/退休流程。`receive_replies` 不能先单独写 `sealed` 后再返回，否则后续 `begin_stop` 不会启动清理，挂起调用可能永久留在 `pending`。
2. 删除或私有化 `Dispatcher::reply_sender`。共享回复邮箱只能通过每个请求携带的 send-once 授权接收回复，不能暴露可伪造回复或填满邮箱的普通发送 owner。
3. 明确 Runtime 投递的 timeout 输入不能被提前消费后丢失；`Dispatcher` 必须在任意推进阶段保留并处理已交付的期限命中，`max_work=1` 不能使调用无限停驻。
4. 来源错误保留实际 `SystemCallError`，只在 RPC 对外错误边界做分类；不能把所有普通来源错误压成 `InternalError`，也不能把所有回复来源错误压成 `ObjectClosed`。
5. 清理错误字段必须有真实写入和观察责任；若清理由 Runtime 负责重试，则删除当前没有生产者的 `cleanup_error`，不要保留伪状态。

这些是接续前的局部修正，不构成新的闭包，也不要求重新实现消息/流运输。

### 最终任务类型与唤醒接缝

`Dispatcher` 不再把 `Task<()>` 作为最终服务组合接口。当前 `Task::Family = Self` 使它只能独占 `Runtime<Dispatcher>`，无法和入站任务、业务任务及 Outbox 共享同一个服务 Runtime。保留 Dispatcher 作为协议状态拥有者和可嵌入推进器，由服务侧的 `ServiceTask`/任务族统一实现 `Task<ServiceWorld>`，并转发 Dispatcher 的 `advance`、`registered`、`unregistered`、`refused`、`stop` 和 `deadline`。

服务 Runtime 的最终形状为：

```text
Runtime<ServiceTask, WaitSet, ServiceWorld>
  └── ServiceTask 任务族
      ├── RPC 出站协议任务（持有 Dispatcher 状态或其驱动）
      ├── 入站/业务请求任务（持有 RequestContext）
      └── 回复阶段（持有 PreparedResponse/Outbox）
```

任务之间不直接取得 Runtime 借用。需要提交下游调用或交付完成结果时，经 Runtime 应用的有界请求声明任务唤醒；顺序固定为先保存请求/结果 owner，再提交 wake 请求。txid、等待者关联和迟到回复仍由 librpc 持有，Runtime 只负责任务调度和唤醒，不保存协议状态。

### Commit 屏障与 Outbox 责任

入站请求的最终顺序冻结为：

```text
Receive/Delivery
  → RequestContext 解码与能力校验
  → PreparedResponse/Outbox、任务、来源和额度预留
  → 实际任务准入及来源登记
  → 业务副作用/Commit
  → 回复发送或明确放弃
  → 来源注销、Delivery/回复授权退休、精确退款
```

业务副作用前必须完成回复存储、发送额度、任务槽及所需来源的准入。`srv_fs` 当前 `serve_one` 先修改 MemFs、后创建回复 Packet，迁移时必须反转为先完成有界回复准备，再调用业务 Commit；不能以回复失败后补偿 MemFs 替代准入屏障。Outbox 持有 `RequestContext`、reply-once、Delivery、PreparedResponse 和发送阶段，回复失败不伪造已提交业务回滚。

通用 RPC 前缀不凭空推断服务端 Deadline。Outbox 接收调用方或协议 Header 已解析的绝对 Deadline；FAL Header 的期限由 FAL 解析层提供，librpc 只负责统一发送背压、接收和最终接受检查的阶段语义。

### 接续顺序与完成门

1. 完成上述五项局部修正，并补 Dispatcher×Runtime 的 `max_work=1`、故障停止、期限和来源错误测试。
2. 将 Dispatcher 改为可嵌入服务任务族的协议驱动，补有界任务提交/完成唤醒接缝，不恢复第二个事件循环。
3. 实现入站 `RequestContext` → `PreparedResponse` → Outbox 状态机，覆盖满箱、关闭、期限、取消、服务退出、调用者退出和退款；回复阶段沿用同一请求任务槽，不在 Commit 后申请不可保证的任务。
4. 整体迁移 `srv_fs` 请求往返和 `srv_init` 真实 RPC 验收；迁移期间删除旧回复授权缺少 `WAIT`、手工 receive/serve/send 阻塞泵及重复 close 路径。
5. 最后统一执行 host/目标检查、七面 clippy、core/release/platform 与跨机制失败/取消/退款组合验证，再做结构收口 Review。

在第 3 步前，不把 `PreparedResponse` 的现有构造函数或 `Dispatcher` 的独立 `Task<()>` 形状视为最终 API；在第 4 步前，不声称 RPC/Outbox 闭包完成。

## RPC/Outbox 闭包收口记录（当前工作树，未提交）

后续施工已沿同一闭包完成以下责任链：

- `Dispatcher` 不再实现固定 `Task<()>`；协议推进、来源回调和停止/期限接口可由服务任务族嵌入。Runtime 新增有界 `Wake` 请求，来源声明/移除/重臂及完成唤醒在请求缓冲暂满时保留重试责任。回复来源错误保留实际 `SystemCallError`，回复故障统一进入可退休停止路径；公开普通 `reply_sender` 已删除。
- `Outbox` 已成为正式入站回复 owner：持有 `RequestContext`、Delivery、send-once、PreparedResponse 和来源注册；先完成来源准入，再允许业务 Commit；支持可写背压、重臂、期限、关闭/错误、停止、来源注销和精确任务/输入额度退款。回复失败产生 `Abandoned`，不伪造业务回滚。
- `srv_init` 合法第二次 RPC 回复已迁移到真实 Outbox Runtime；第一次协议拒绝仍保留为刻意 raw/拒绝路径验收。
- `srv_fs` 已删除每请求 Runtime、`wait_many → serve_one` 重入泵、手写 RPC framing/回复校验、手工 `validate_request` 和重复 close。客户端使用 `Caller`；provider 运行于长期单一 Runtime：Ingress 先接收并预备 Outbox，RequestTask 在来源实际登记后才执行 MemFs 业务，再沿同一任务发送并退休回复。服务停止通过控制消息和 JoinHandle 显式收束。
- `srv_fs` 的业务 Commit 已置于 Outbox 来源准入之后；调用者退出/回复关闭的 `Abandoned` 是服务可继续运行的终态，不再升级为服务 panic。

验证证据：

- shared workspace host 测试全部通过；
- `libsrv`、`libprocess`、`librpc` host 测试全部通过（共 43 项相关测试）；
- `librpc`、`libsrv`、`libprocess`、`srv_fs`、`srv_init`、`srv_pm` 目标 clippy `-D warnings` 通过；
- 用户态 RISC-V 目标检查通过；
- `just virt` 通过：FAL fs 验收、服务监督、Outbox 真实回复、资源退款和显式 reset 均通过。

当前不把没有真实多路复用消费者的异步 `Dispatcher::begin_for` 另造测试服务；它已作为可嵌入协议驱动保留，后续首个真实多 in-flight 服务消费者出现时直接接入同一 `ServiceTask` 族。RPC/Outbox 当前闭包的真实消费者和旧路径删除门已关闭；提交前仍需按项目流程做结构 Review，提交后登记固定提交 Review。

## 开工流程与本任务审计门

本任务遵循 `AGENTS.md`「标准施工流程」，按下述机制顺序推进。消息、流运输的历史审计与交付记录保留；当前通用执行的责任链、修复证据和完成状态由唯一 Runtime 闭包报告维护。过程阶段不另立独立交付或验收语义；只有完整机制闭合后才进入该机制的组合验证。

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

#### 通用执行与准入设计裁决（2026-09-14，基线 `f4a4d57`）

**审计结论**：本闭包无内核/shared 改动——WaitSet 登记/Rearm、`EndpointEvents::register`、REAPABLE/CLOSED 观察与信号面全部已有；挂起关闭的停驻来源已核实为内核内部退休确认（tunnel close 等待内存事务跨 hart 收束，不等待 peer 进程），界域由 hart 前进决定。铺路现状：budget/wake 有 libfal 消费；work_queue/runtime/dispatcher 无消费者但按「铺路面与消费者」准则（AGENTS.md）续用不重建，替换仅限严格更优处（cookie 真值路由→token 表、Resource 领域大枚举→泛型分类、collect_process 独立循环→单一状态机+同步门面）。ThreadSpawn/JoinHandle 已完备，现存服务全为单线程测试形态；内核无配额机制，只有资源支付不变量与用户态准入库（metadata_admission+budget）。

**裁决与终态**（详尽论证见 `notes/ideas/framework.md`、`notes/ideas/runnel.md`）：

1. 单一闭包、单次实施、一次提交：执行核心单独拆出无消费者，数据面与监督两族消费者同为 pm/init，登记寿命/generation/重 arm 的 owner 形态必须一次定稿；实施序内 pm/init 直接写终态，无混合形态。实施序：budget 泛型化+删除 libsrv→librpc 无用依赖 → Runtime（独占 WaitSet、token 来源表+arm_generation 过滤、每来源预付槽「未消费不重 arm」、调度报告×生命周期两维、Gate 运行中派生、spawn→run→close 公开面、非空放弃逐槽有界析构替代 forget） → librunnel arm/poll（条件选信号集、typed 三条件、PeerAttached 满足后不复登记） → libprocess 监督状态机+同步门面（CLOSED≠Drain Complete；ObjectBusy 绝对期限重试；More 回队尾） → pm 全量任务化 → init 数据面/监督段+race 迁移 → 旧路径删除。
2. 消费者：pm 全量任务化为主证明负载（多任务公平/静默唤醒/期限/停止/退款，max_work=1 重放）；init 只迁生产结构段（数据面+常驻监督），验收脚手架保持顺序形态，验收拓扑与锚点业务语义不变；长存活纪律由 init 的持久 RootSupervisor 证明。
3. 后置接缝（按「铺路面与消费者」登记触发条件，非禁建）：专职退休执行线程——多线程服务中循环不应为退休停驻时；跨线程提交队列+门铃——首个多线程服务定形；额度等待登记——首个真实等待者定形公平/预留语义。RPC 闭包所需机制（多来源、期限、Gate、完成输入）均由本闭包真实责任消费，不预建 txid/Outbox 面。
4. 实施中验证点：核实 tunnel close 停驻在单 hart 节流验收下的实际时长不构成公平性障碍；若证据推翻（如发现跨进程依赖停驻），退休执行线程提前入闭包。

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

RPC 与 Outbox 是一个合并的机制闭包，不拆成两个各自验收的任务。内部按以下顺序施工：

1. 先收束 `Request`、`ReplyPort`、`PendingCall`、txid 和异步出站驱动，把现有 Dispatcher 改为消费 Runtime 的状态/观察能力；同步 Caller 作为同一出站状态机的阻塞门面。
2. 再接通 `RequestContext`、`PreparedResponse`、Outbox、reply-once、Delivery 和回复准入；这些对象共同承担入站请求直到回复终结的责任，不能按文件另拆。
3. 最后迁移真实消费者、删除旧阻塞泵和重复 close 路径，并在同一闭包内完成责任链收口。

上述阶段只表达实施顺序，不构成中间完成门。过程中只为稳定的 framing、所有权转换和状态推进函数保留必要 testcase；不为未闭合的 Dispatcher、Outbox 或临时消费者运行独立 QEMU/组合验收，也不把局部可用性记为机制交付。

- Request/RequestContext/PreparedResponse、txid、ReplyPort 与 PendingCall 共同表达 Unsent/Sent/完成。Unsent 失败返还请求；Sent 超时、关闭或取消本地等待报告结果未知，不自动重试副作用。
- 同一 Deadline 覆盖发送背压、接收和最终回复接受；注册与期限唤醒交执行核心，协议自己的 Deadline 和 txid 仍由 RPC 持有，不能因去重丢掉独立语义。
- 在业务 Commit 前预付回复存储和发送额度，Outbox 随同一请求/回复 owner 持有 reply-once、Delivery 与准入。Outbox 的协议责任属于 RPC，排队、期限和唤醒消费执行核心；libsrv 不另建一份拥有相同回复的记录。
- 收束当前 Dispatcher::on_ready/expire/pop_completed/shutdown_step 与 Runtime::turn/wait 的组合；服务不再逐个驱动多个内部控制循环。RPC 完成 FIFO 仍可作为内部结果队列保留。
- 协议拒绝、迟到回复、附带能力、取消和退出统一走 owner 收束；不能在长期存活服务中依赖 mem::forget 等待整个进程退出完成正常清理。
- 同步迁移 init 的真实 RPC 验收、srv_fs 既有请求往返以及全部现有 RPC 调用点，删除旧阻塞泵、相对期限重试和重复 close。

完成门：请求到回复或放弃的每条责任链接通；所有真实消费者与清理路径使用最终机制；旧阻塞泵、相对期限重试和重复 close 路径删除。只有到此闭包完整收口后，才统一执行满箱、Sent 后超时、迟到/畸形回复、服务退出、调用者退出、Outbox 退款和跨机制组合验证；局部阶段不单独宣称通过。

## 组合完成门与提交

组合收口验证本前置计划已完成的机制与本次 RPC/Outbox 闭包；不在局部阶段补主要功能或制造临时验收消费者。执行适当 host/目标检查、`just clippy`、core/release/platform 及跨机制失败/取消/退款组合，证据能定位。稳定纯函数和状态推进的 testcase 可在施工中随实现补齐，但不替代闭包完成后的整体验证。历史 stress 概率误判和 Tunnel 墙钟敏感截断见 [只读归档](archived/ref-2026-09-acceptance-timing-flake.md)；新现场命中其触发条件时重新立案，不以重跑直到绿替代证据。

提交不按文件机械拆分 ABI/owner/观察/退休迁移；RPC/Outbox 按上述单一闭包组织提交，内部阶段不各自登记为独立交付。闭包收口时登记真实调用者、删除的旧路径和整体验证证据。不能引入没有删除条件的 adapter 或测试专用运行体。已完成提交之后登记对应固定 hash 的未来 Review；提交、合并和 push 仍分别遵守授权边界。

## FAL 连接与唯一归属

本计划只迁移已有 FAL 调用的基础 API 使用；正式授权域、NodeStore/MemoryBackend/StoredValue、值与 wire、provider 装配属于 FAL 总计划。执行前置不以这些未接通草稿作为通用能力已成立的证明。

本计划前置完成后，按总计划完成内核执行结构整理（共享包契约与归属已归档），再进入 FAL 后端的准备/取消/替换值退休与稳定身份闭包，随后完成 grant/授权准入/provider/client 的共同迁移，最后扩展业务操作。后端产生的正常清理责任由后端持有并接入公共执行，不交给 handler 手工逐项 close；业务明确转移的能力才作为结果交付。具体责任与顺序唯一记录在总计划。

## 首次接管裁决记录（2026-09-15，历史）

以下为首次接管时的裁决，最终实施者及状态以文首和下一节为准。当时由 SilverSeal 接手实施；SharpGale/其他协作者停止对同一工作树编辑，不沿用当前实现中已证实错误的登记、输入和退休路径。以下问题按机制重构，不以局部补丁或兼容双轨掩盖：

1. **Runtime 登记事务**：WaitSet `register` 后不做首次 `rearm`，初始代次直接收编；Add/Arm 共用任务存活、来源数量、额度和节点预留准入。登记后的任何失败均保留撤销责任，禁止孤儿 token。
2. **Runtime 输入账本**：来源事件、期限和拒绝均进入每任务有界 inbox；advance 只消费实际读取部分，剩余输入和错误路径完整回存，不以 `has_pending` 布尔值替代事实。拒绝按请求顺序交付，不能覆盖或静默丢失。
3. **Runtime Gate 与生命周期**：请求应用有界、可观察、带 owner；`Complete` 不再允许没有交付闭包的异步请求。任务、来源退休和 WaitSet 收束共同决定 `run` 的完成，空队列不得在无限期限上误等待。来源清理使用有界退休队列和重试预算。
4. **Runnel 观察接缝**：Consumer 首次满足 `PEER_ATTACHED` 后撤销旧登记并按新条件重新 arm；持久电平不能靠 generation 过滤解决。EOF 已发布但尚未全部消费时不报告 `Writable`。poll 的错误、acknowledge 和重查结果由真实任务消费。
5. **监督状态机**：修复 `SuperviseTask` 首次登记不可达、Collector 取出后的错误恢复、期限消费/清除和同步门面的 Busy 节流；`Collector::step_close` 保持“成功关闭后兑现 control、失败保留 authority”的单一语义。
6. **真实消费者迁移**：pm/init 的 arm refusal、来源错误、PeerAttached 重规划、FlowTask 有界填箱和 capability owner 转移一并迁移；不保留只为通过测试的 adapter。`Task::Family` 继续保证队列族同构；通用监督任务若无真实驱动点则不作为完成证据。
7. **验证门**：先补 FakeSet 的 consumed/queued/持久电平/部分输入/撤销 Busy/Complete 请求模型，再运行受影响 host 测试、`just check`、`just clippy` 与 `just virt`/必要 release 路线。任何一个不变量未有回归证据，计划保持进行中。

## 接手后的最终实现（2026-09-15）

HighHolly 已接替 SilverSeal 实施，工作树保持单一写入者。原初轮实现和两轮复核暴露的问题保留在唯一 [Review 报告](archived/review-2026-09-15-runtime-closure.md)，不得据历史 passing 路线宣称当前责任链完成。

- Runtime：事件接收、来源退休和任务退休独立轮转；每任务输入 FIFO 只处理本轮预算的真实记录；来源退休与任务执行失败退避都有预付期限槽；最近期限不全表扫描。登记/注销回执、Gate 拒绝、Complete 后返还责任、停止时继续观察与最终退款共同接通。
- 公共接缝：SourcePlan 为不透明值描述，删除 Runnel 等待计划的动态 Box；input_budget 按实际类型与容量推导账户额度。shared/timer_queue 的 value_mut 仅用于预付后绑定 token，不修改调度算法或 ABI。
- Collector：Process/Job 共用带唯一身份和绝对期限的观察契约，区分 Ready/Timeout/SourceError。Process Close 前保留快照，错误按值返还原机器。Job 单栈推进、派生前 fallible 预留，删除错误包装 Box 和内部阻塞 wait；同步和 Runtime 驱动共用状态机。
- init：RootSupervisor 在服务启动前拥有长期槽和 services 待命机器；Job/Batch/Read 的输入、运行体、世界、结果和关闭责任均留在根。部分准入失败不停止已入队任务，多个运行体组合等待，永久失败不退出管理根或丢弃原机器。流建立失败也有预备清理槽。辅助 Job、启动 mailbox、委托副本与流控通知的能力槽均先于获取预备；pm sender 保持 typed owner，内核消费回执才触发移交，正常/失败关闭均由根账本承担。
- pm：域管理只使用 JobDriver，删除重复成员编排；实际以 max_work=1 驱动。本批让测试邮箱保持 Active 到 stop，检查实际登记、注销回执和最终账户退款；新增必检锚点已通过完整 acceptance。测试阶段关系保持，未建设正式服务架构。非零异常退出由 init 管理能力接管。
- 证据：正式 Runtime host 验证持续来源/Gate/退休/停止/退款、部分输入、独立来源重试、失败任务隔离；ProcessOperations 与真实 Runtime 集成验证注销 Busy、Close 恢复、首次观察超时/错误和连续 Drain Busy；init 的真实批次/独立运行体故障隔离锚点进入 QEMU 必检项。

验证与复核已完成：最终 `artifacts/acceptance-takeover-20260915-121051.log` exit 0，31 个改动源码哈希一致；host、just check、七面 lint、stress 16/16、release/sifive_u/nofd 与启动失败三线通过。当时 R1–R16 和 C1–C7 的关闭记录已归档。后续结构审视纠正了其中 R15 对 PM 实际停止的证明，本批已通过 Active 登记→stop→注销→退款补证关闭；Runtime 预付清理与 Job 单页收束的最终证据见固定提交 Review 的已授权批次。通用执行与准入初次交付为 `a3891b0`；本计划仍拥有尚未实施的 RPC/Outbox，不能把四闭包整体标为完成。
