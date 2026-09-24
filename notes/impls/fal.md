# FAL 实现现状

方向见 [`../ideas/fal.md`](../ideas/fal.md) 与 [`../ideas/fs.md`](../ideas/fs.md)。通用 RPC/Runtime/Runnel 的实现分别由 [`rpc.md`](rpc.md)、[`runtime.md`](runtime.md) 与 [`runnel.md`](runnel.md) 拥有；本篇只记录 FAL 当前代码事实。唯一施工导航是 [`FAL 整体计划`](../../plans/todo-2026-09-fal-service-capabilities.md)。

## 正式生产链

F1、F2 与 F3a–F3d 已接通先前的组合门；F3e 的 Open/流正在同一未提交工作树接入。init 为 A 提供四项启动 grant（bootstrap、release、route、注册 Mailbox owner）；A 在注册 root 的 Lifetime 观察准入后发行 root sender。init 接收 A 的 FAL 根、服务目录根与授权 root，先订阅目录根，再以 DelegateName 从 A 取得 exact-name `fs.secondary` sender，作为 B 的第四项启动 grant。每个 `srv_fs` 进程主线程仍只运行一个 Runtime，A 在同一运行体内承载内存域和 Registry：

```text
srv_init / libfs::resolve
  → libfal::client / librpc::Caller / Mailbox / Delivery
  → srv_fs::Ingress + RequestTask + Outbox
  → GrantTable::snapshot / AccessSnapshot
  → ServiceBackend / MemoryBackend 或 libservice::Registry / NodeStore
```

FAL1 protocol id、FalHeader、slot-1 anchor、`MemFs`、旧 provider dispatch 和同进程泵已随最后一个消费者迁移整体删除。`DirectoryGrant<G>` 持正式 endpoint owner，provider 只按内核 sender identity 取得授权快照。

### FAL2 积木与接线状态

`store.rs`、`backend.rs`、`data.rs`、`value.rs`、`grant.rs`、`authority.rs`、`protocol.rs` 和 `resource.rs` 现已由 `srv_fs` 的正式 provider/client 共同消费：

- `NodeStore` 用 `NodeRef` 的 pin 与目录 link 分账，PreparedNode/PreparedMutation/PreparedWrite/StoredValue 预付容量与账户额度；冲突原样返还事务，旧属性、旧块和摘链节点进入有界退休。
- `MemoryBackend` 具备 Create/Delete/Write/Property/Move 的 prepare、位置/结构代次校验和无分配 commit；Move/Delete 同时推进目标节点版本，为节点 Watch 提供单一代次真值；所有后端访问要求不可外部构造的 `AccessSnapshot`。
- `GrantTable::snapshot` 是 `AccessSnapshot` 的唯一构造点；GrantTable 绑定真实 provider Mailbox identity，以 sender context 查表，持 Lifetime CLOSED 观察和账户收费。`prepare_derive` 从活动父 grant 继承账户与运输 ceiling、拒绝权限放大，并为稳定目录 root 创建独立 sender/Lifetime。Runtime task 先登记 Lifetime source 再安装和回复发布；回复放弃会关闭 sender 并走同一退休退款路径。
- FAL2 `protocol.rs` 提供 Lookup/Create/Read/Write/ReadAt/WriteAt/Delete/Enumerate/Link/Derive/Move/Take/Subscribe/QuerySubscription/Unsubscribe，以及 Open/Start/QueryStream/FinishStream/CancelStream；Lookup 成功体为严格 Found/Delegate/LinkBoundary 三态，Delegate 与 Derive 成功各要求回复 capability slot 0，Move 的请求 slot 1 携带目标 DirectoryGrant，Subscribe 的请求 slot 1 携带 Notification signaler，Open 的成功回复槽 0/1 分别为 minted StreamControl sender 和单次 Invitation；属性值按自身 Handle 字段声明业务槽，枚举项流按声明 count 完整消费且拒绝残尾。
- `srv_fs::server::run` 通过泛型 `run_with_backend` 统一拥有 Runtime/WaitSet、GrantTable、`ServiceBackend`（内存域与可选 Registry）、退休 Notification/`NotificationWake`、Outbox 和 provider-local 根 grant。Ingress 从内核填入的 `sender_context_id` 获取 `AccessSnapshot`，payload 中的数字不能伪造 FAL 授权。通用 State/Retirement/Grant/Watch 在 `libfal::provider`，Read/Delegate/Request 与 Dispatcher 留在 `srv_fs` 编排。
- A 当前仅有一套 FAL Mailbox/GrantTable/Watch，内存根和发现根各获独立 sender-context 授权及 NodeRef；`ServiceBackend` 按节点存储身份选择后端，跨域 Move 仍返回 CrossDevice。注册控制另有 Mailbox/AuthorityTable。A 的两枚 bootstrap FAL root 保留 GRANT 供 init 在 Building 阶段直授，普通派生 grant 不继承该运输权；共享入口不提供两域独立接收、封口或公平额度。
- provider 由同一 `libbudget::Account` 建立 ExecutionResource、FalResource 和 ServiceResource 三种不可变视图：分别支付 Runtime Task/InputBytes、FAL Node/Bytes/Grant/Watch/WaitSource，以及 Registry Authority/Registration/Bytes/WaitSource。Task/Source 上限按 G=16、A=16、R=16、W=24、请求并发预留 Q=16、下游 D=8、流表 S=15 与固定任务/来源公式推导；每流预留流任务与一个控制回复任务、最多三个流来源和一个控制回复来源，FAL WaitSource 额度另外增加 2S。Node 额度同时覆盖内存根及 Registry 根/记录，服务来源付款覆盖 A+2R。Q 是共享任务池内的并发预留，不是独立隔离配额；Bytes 尚需结合极端 Record/节点负载验证。

- F2 的既有 Delegate 请求链仍运行：A 的 route endpoint 保存从服务发现所得 B 母 grant 与 FAL ceiling；普通 Lookup 命中绑定后，A 的 Dispatcher 非阻塞调用 B 的 Derive，`DelegateTask` 回复独立子 grant、consumed 与 remaining。此前 init 从两个 bootstrap 直接取得业务 root；F3d 工作树已删 B 的直交旁路，改由 A 目录 Record 出口取得 B 母 grant，再装配 `/second` route。`libfs::client::Transport` 继续跨越 `/second` 与 `/second/f2-dir/leaf` 验证授权衰减，启动 grants 缺失或 role 错误时 fail-closed。
### Owner 与收束边界

未提交准备阶段必须携带全部 owner：节点 pin、目录/块 Permit、Account Charge、能力 owner、回复和 Delivery。准备失败或 commit 冲突原样返还这些责任。成功提交后，旧值和最后引用由后端结果与退休队列承接；回复失败只能形成 `OutboxResult::Abandoned`，不能伪造业务回滚。服务退出需要停止准入、取消/完成请求、收束 GrantTable、backend retire queue、Delivery、reply-once、WaitSet，并确认账户退款。

后端成功替换/摘链后的旧值由显式 retire task 通过 Notification 电平唤醒并按预算推进；release Notification 是同一 Runtime 的正式 source。服务退出顺序为停止准入、Dispatcher 取消/完成下游调用、撤销全部 grant Lifetime source、收束 backend retire、移除 route/release/retire source、关闭 Runtime。固定宽 `ProviderReport` 在全部账户归零后报告 committed、回复 abandoned 和已投递下游调用 abandoned。

## F3a 同域 Move 接线

F3a 在 FAL2 基础操作之上增加 `Op::Move` 与 `Status::CrossDevice`。请求正文携带源稳定父目录相对路径、源名字、目标最终名字和可选的 NodeId/version 预期位置；业务 slot 1 携带目标 DirectoryGrant，slot 0 仍由 RPC 保留为 reply-once。`libfal::client::Client::move_entry` 复制目标 sender 并以 typed capability 发送，失败时由 RPC packet 返还；`libfs::client::Transport::move_entry` 将该操作暴露给稳定 DirectoryGrant/Namespace 走路层。

`srv_fs::Ingress` 按 opcode 精确准入业务 handle 数，使用 `GrantTable::validate_received` 校验目标 sender 的真实 object identity、Mailbox 归属、role 和活动 grant。不同 provider 的目标 sender 返回 `CrossDevice`，不会进入后端事务。相同 provider 的请求创建 `MoveOperation`，先完成源/目标权限和稳定位置准备，再由 `RequestTask` 按 Runtime budget 分步调用 `validate_move_step`，循环检查结束后无分配 `commit`；准备失败、循环冲突、版本冲突和回复放弃均保留后端 owner。

当前跨 provider 的 Move `CrossDevice`、A/B 根枚举、同 provider Move 的源/目标收束和 Watch CREATE/MODIFY/DELETE 业务观察已迁至独立 `test_fal`；每个 provider 的同域目录 Move、旧路径消失与新路径出现仍由 `srv_init` 的 FAL2 监督剧本验证，F4 已完成消费者迁移与组合收口。

## F3b Record、Handle、Take 与属性 Copy

属性值使用 `value.rs` 的统一非递归编码：标量、同构 Array、异构具名 Record 与 Handle 共享总 payload、元素和 capability 槽预算；重复字段、未引用槽、重复槽、错误 role/rights、非法 ExportPolicy 均在存储前拒绝。Create/Write 从 `RequestContext::HandleSet` 一次取走全部业务 owner，准备失败仍由请求上下文或 `StoreFailure` 完整持有；提交替换后的旧值沿既有退休责任关闭。

repeatable Handle 属性保存完整母本和 ExportPolicy。Read 同时要求 `READ_PROPERTY` 与 `ACQUIRE_CAPABILITY`，按声明的运输 rights duplicate，重写为从回复 slot 0 开始的 canonical 槽位，并由客户端再次执行值/槽完整验证。DirectoryGrant 不走该路径，因为裁剪内核 rights 不能收窄 FAL ceiling；当前返回 `Unsupported`，正式目录能力出口仍由 Derive 负责。

affine 属性只能通过 `Take`。`PreparedTake` 先预付空 Blob、取出原值并设置节点独占 Busy 门；该门阻止 Read/Write/Move/枚举和元数据观察越过预留。`TakeOperation` 在取出 owner 前预留政策与恢复容器，再将原始 owner 放入预备回复；Outbox 成功入箱后 `commit_take` 无失败地发布空值与新版本并把 Bytes charge 收缩到实际空值，任何未投递终态则从 Packet 逆序取回 owner 并 `rollback_take` 恢复原值。DirectoryGrant 的 affine Take 同样返回 `Unsupported`，不能绕过 Derive 的 FAL ceiling。回复放弃的 Take 不计为业务 committed。服务 Runtime 与 Outbox 仍拥有期限、关闭、来源注销和 Delivery 收束。

基本属性 Copy 由 `libfal::client::copy_property` 和 `libfs::client::Transport::copy_property` 编排为源 Read 后目标 Create。它只接受无 capability 的完整属性快照，目标已存在返回 Exists，不覆盖、不执行 copy+delete，也不承诺跨 provider 原子性。`srv_init` 对每个 provider 验证本地 Copy，并在两个独立 provider 间验证跨域 Copy；同一剧本还验证 repeatable Handle 两次读取、affine Take、关闭回复 Mailbox owner 后恢复再取以及 Take 后空值。

## F3c Watch

F3c 发布 `Subscribe`、`QuerySubscription` 与 `Unsubscribe`。客户端 `Subscription` 独占 Notification owner，并复制一份 grant 使用引用；provider 消费具备 `SIGNAL | WAIT | TRANSIT` 的 signaler。每项订阅由独立 Runtime task 拥有 signaler、Watch/WaitSource charge、被观察 `NodeRef`、回复 Outbox 和来源注销责任。任务先登记 signaler 的 `CLOSED` 来源，登记回调再按原 AccessSnapshot 重新解析路径、确认稳定 NodeId 与当前 WATCH 权限，并在同一推进点安装记录和取得节点版本；安装后的事件可早于 Subscribe 回复，安装前变化则由回复代次显式体现，不形成丢失窗口。

provider-local Watch 表配置上限为 24，`libfal::watch::WakeBatch` 在 Runtime 单步请求容量不足时保留未发出的唤醒游标；表以单调 subscription id 寻址，同时保存内核 sender context，Query/Unsubscribe 必须由原 grant context 发起。目录直接成员 Create/Delete/Move 及 Registry Ready/Drain 产生可见提交效果；属性和成功 Take 产生 MODIFY，被观察节点 Delete 产生 `DELETE | TERMINATED`。每项订阅按 OR 合并位，一次提交最多唤醒每个 Watch task 一次；signal syscall 只由该任务执行。

Unsubscribe 先从发布表摘除并清空尚未 signal 的 pending 位，再唤醒任务撤销 source，回复确认后不再产生新事件；Notification owner 静默关闭走同一退休路径。Subscribe 回复 abandoned 会由服务端主动摘表、撤源和退款，不依赖客户端最终关闭；provider 停止时先合并并 signal `TERMINATED`，随后撤源关闭 signaler。`srv_init` 在两个独立 provider 上验证目录 CREATE、节点 MODIFY、代次推进、外来 grant context 拒绝、显式取消后静默、owner 静默消散、节点删除终态、provider 停止终态与最终 Watch/WaitSource 退款。

## F3d 当前施工事实

`libservice::{protocol,record,authority,registry,resource,client}` 已有严格注册 codec、固定 ServiceRecord schema、sender-context authority、Registry 的 Starting/Ready/Draining/Terminal 与投影。Registry 只实现只读 `Backend`；A 的 `ServiceBackend` 在同一 Runtime 内装配 MemoryBackend 的写面和 Registry 的只读根，不再要求 Registry 实现假 `MutationBackend`。A 的同一 Runtime/Dispatcher 服务两个 FAL 根和 RegistrationControl Mailbox；A mint 的 root sender 在 Lifetime source 准入后出版，init 用 root 调用 DelegateName，取得同样先有 Lifetime/Outbox 来源再交付的 exact-name `fs.secondary` sender；B 持该 sender 异步 Register→PublishReady。init 在 B 启动前已订阅 A 目录根；收到 CREATE 后读取 `fs.secondary` Record，经 Directory Derive 取得 B 子 grant 并真实调用，B root 不直授 init。B 退出后 init 检查旧记录 TERMINATED、目录根 DELETE 和再读 NotFound。

`RegistrationReplyTask` 以 control sender 的内核 `object_id` 同时作为实例和请求 context，长期持有建立 Outbox、control Lifetime、endpoint CLOSED 与建立期限。两个观察和 Outbox 回复来源都收到实际登记回调后，才安装 Starting 并交付 sender；endpoint 关闭或控制消散会摘名并发 Watch effects。Registry 在 endpoint 来源解除后才允许关闭 endpoint 母本；已观察 authority 在 seal 后仍由发行 task 移除，不能被批量退休提前擦除。Terminal 壳保留到 control Lifetime 消散且 endpoint 已退休。Withdraw/BeginDrain 的根 DELETE 与旧记录 DELETE|TERMINATED 效果已接上；Mailbox policy 使用直接 snapshot，Directory policy 使用异步派生。

通用 provider 已完成 State/退休/Grant/Watch 的部分提取：`ServiceBackend` 按 NodeRef 身份分派内存/Registry 两个域，`serve_v2`/Task 的泛型契约已接通。`World<B>` 只持一份 `libfal::provider::State<B>`，其中拥有 FAL mailbox、backend、GrantTable、Watch、退休 owner 与统计；注册任务借用同一 backend/Watch。State 在库中结算回复计数并负责后端 seal、分步退休及关闭失败时的 owner 返还；`provider::Retirement` 驱动通知来源的登记/注销和预算推进，`provider::Grant` 拥有 prepared grant、Lifetime/Reply source、GrantTable 安装撤销、初始 sender 出口与派生回复，`provider::Watch` 拥有 WatchOwner、backend 安装复查、pending effect 通知、Subscribe 回复和 owner 退休。`srv_fs` 只保留这些类型的 Task<World> 薄适配及请求阶段 owner 构造。Dispatcher、注册控制、route 管理、root grant 出版和进程停止政策留在宿主。外层透传 Ingress/RegistrationIngress 拒绝回复和 Dispatcher 已登记事务的实际期限。Create/Link 用 `CreateInput`，MemoryBackend 内构私有 `Body`；`MutationBackend` 的 Position/Mutation/TakeReservation 是关联 owner，通用任务不持 Memory 具体 prepared 类型，Registry 无写 trait。`AccessSnapshot::output_transport` 已用于普通 Read、Directory Read 与 affine Take 的出口上限；Directory 只支持 repeatable Record，affine Take 返回 Unsupported。`libfal::watch::WakeBatch` 保留跨 Runtime 单步的唤醒债务。

MemoryBackend 的 Property 替换在提交后先用固定两步退休旧值（当前 Property 至多一个 handle，加字节清理）；正常关闭立即完成，失败时由后端保留唯一 `retiring_property` owner，借 NodeStore 的既有退休唤醒让 RetireTask 在运行期按预算重试。旧 owner 未释放前后续 Property 准备/提交返回 Busy，防止清理债务无界累积；`has_retire_work`、`is_empty` 和 close 均计入该 owner。已提交的新值与 Watch 效果不因旧值关闭失败回滚，回复只表示提交结果。Registry 构造先完成 AuthorityTable、Registration Counter 等可失败分配，最后才提交根；A 在 Registry 构造失败时 seal/retire 尚未发布、无外部能力的 Memory 根，避免构造期丢失账户责任。

`ServiceBackend` 持唯一 `route::Binding`，附着于 MemoryBackend 根的 NodeId。`Backend::lookup` 仅从内存根出发、完整组件命中且授权含 TRAVERSE|ACQUIRE_CAPABILITY、输出含 TRANSIT 时复制目标 owner 并返回 DelegationBoundary；Registry 根和内存子根不能匹配。route mailbox、RouteIngress 和 RouteReplyTask 都在 `srv_fs`：入口只交接未消费 RequestContext，回复任务等 Runtime Gate/Outbox 来源准入后才取目标 sender 并提交 `bind_route`，绑定不会因未准入留下无 owner 副作用。初始 GrantTask 只安装授权、观察 Lifetime 并携 task id 交付 sender，宿主按启动时的 root/Registry task id 分类。route 不伪造 FAL 节点，Seal 丢弃后端绑定；MemoryBackend 的 Property commit 复核 Take reservation，冲突时返还 prepared owner。Read/Delegate/Request 留在宿主编排；Read 的 Directory export 持有 Dispatcher `dispatch_intent`、下游 submit/cancel 状态和 completion 回调。这是当前实际组合边界，没有必须继续提取的宿主 hook；libfal 不反向依赖 srv_fs，也不复制 Dispatcher 状态机。

`Ingress` 与注册控制的 `RegistrationIngress` 各自把收到的 packet 保留到 Runtime Task Gate 正式准入；准入后由同一请求任务准备业务与执行提交。`REQUEST_HEADROOM` 只参与总 Task/Source 容量算术，实际准入由 Runtime Gate 判定；无法立刻准入时保留已有请求 owner，经 Outbox 返回 Quota，拒绝回复无法取得任务槽时暂停该入口继续接收并等待推进，不将正常满载升级为 provider fatal。Read/Delegate 的下游提交与取消用同一 `dispatch_intent` 表示：上游 Outbox abandoned 或停机使未提交 owner 本地回收，Pending 经 Dispatcher.cancel 撤除并等完成回调退休；已投递的远端业务副作用不能因此推定回滚。注册 Ready、Drain、Withdraw 先取得正式回复任务准入，再由 Registry 提交唯一状态和可见投影；提交返回固定容量 `Transition { info, effects }`，任务只发布提交返回的 Watch effects，重复请求没有新事件。注册回复任务合并 Outbox 的 Runnable 结果，来源/可写重 arm 请求未完成时保持可推进。Starting 期限仅作 Runtime 唤醒提示，`Registry::expire_if_starting` 在 Registry 内按当前状态条件撤出，Ready 提交后迟到 timer 不能撤销新状态。

`srv_init` 除 A→B→init 的正常发现/失效外，还使用 root 注册授权为自有 Mailbox endpoint 派生 exact-name authority，经 Register→PublishReady 后单独关闭 endpoint owner；在 control sender 保活时，它观察目录 DELETE、经 Query 确认 EndpointClosed 并验证旧名 Read NotFound，最后释放 control。另一注册路径先阻塞建立回复，经 QueryName 确认 Starting 后关闭回复 owner 并观察摘名；随后重新注册同名实例，用满回复箱阻塞 Ready/Withdraw，以根 CREATE/DELETE 确认提交，并在回复箱仍满时查询 control 状态，确认副作用不依赖回复送达，也不因之后放弃回复而回滚。stress 同时向普通 FAL/注册入口送入超过当前容量的请求，检验 Quota 响应与恢复；在 A 仍运行时关闭多个上游 reply owner，确认静默下游取消后请求通路恢复。一个被取消的下游随后投递带 minted sender 的合法 Derive 成功回复，init 等待该 sender 的内核 Lifetime CLOSED，确认 Dispatcher 清理随附能力而非污染新请求。provider 正常停止在 Runtime、GrantTable、Watch、双后端退役后分别检查 FAL、Execution 和 ServiceResource 账户全部归零，不依赖进程消亡回收额度。Runtime FakeSet 覆盖来源上限拒绝的回调与任务/输入额度归还；正式服务的来源登记拒绝无法通过当前外部接口稳定制造。注册入口与 route 的名称复制、FAL Move/Take 请求字符串复制使用可失败预留并沿各自 Resource 错误出口收束；注册回复任务仅对可能发布效果的操作预留唤醒 batch。Registry 单 Record Read 的 endpoint 快照容器使用可失败预留；到期只按实例状态与 `establish_deadline` 条件推进，没有第二套 Starting 有序索引。名称校验由服务协议统一拥有，注册 control 索引只有实例 id。内存后端 Lookup/Link 使用切片迭代，只有跨请求保存的 consumed/target/remaining 做可失败复制；route 边界同样显式预留后沿 Resource 错误出口返回。

## F3e Open 与流的当前接线

`srv_fs::Ingress` 将 `Open` 按原 GrantTable 身份取得 `AccessSnapshot`，在 Runtime Gate 准入后复制相对路径、校验 Stream 节点权限与 checked 范围，并 pin `NodeRef`。流表位于 `World::streams`，`OrderedTable::PreparedEntry` 在创建任何能力前预付节点、表项及两个来源的业务账户容量；大表项持稳定节点、授权快照、单侧 Runnel 角色、1024 字节待提交缓冲、进度、结果、Finish waiter 和 affine owner。Open task 只持预备表项或已安装的内核 minted sender identity、Outbox 和 Lifetime/Tunnel 的 Runtime source id。typed Create 的失败 owner 入表，不在映射或来源仍存活时析构。初始 Open 回复期限上界为 5 秒，offer 上界为该期限加 2 秒并受调用者有限会话期限约束；会话期限不因进度重置。

创建端是 Read 的 Producer、Write 的 Consumer；交付槽 0 是仅具 `WRITE | WAIT | TRANSIT` 的 StreamControl sender，槽 1 是 `MAP | TRANSIT` 的 Invitation。两者仍走原 FAL 主 Mailbox：后续 Start/Query/Finish/Cancel 从不可伪造的 `sender_context_id` 找流表，不经 GrantTable，也不创建第二套控制 Mailbox。Start 仅在 Open 已投递、Runnel 实际观察 PEER_ATTACHED、offer 未到期时改为 Active；控制请求各持独立 Outbox，Finish 将一个等待 task id 放在表项，不占住接收入口，Cancel/Query 可以继续准入。终态结果在 control Lifetime CLOSED、会话期限或 provider stop 中最早条件前保留；数据来源先注销、再关闭角色，结果壳仍可 Query/Finish。Outbox abandoned 或建立中途失败先撤来源，再收能力与表项；关闭失败保留 owner 并按时钟停驻重试。

数据推进一次最多处理 1024 字节。Read 的终点在 Open 时冻结为当时长度和请求范围的较小值，后续覆盖可能改变尚未读到的数据，增长不延长终点；`transported` 计发布字节，`accepted` 来自已验证 Runnel 消费尾，只有 EOF 被消费才成功。Write 从环取出后先保留在表项缓冲，`MemoryBackend::prepare_write`/`commit` 成功才增加 `accepted`，超过声明范围的首个额外字节只计 `transported` 并形成部分失败；每次成功提交后取后端节点版本并通过既有 Watch/WakeBatch 发布 MODIFY。业务失败、EOF 与取消只冻结流表结果，不能把 Runnel 环推进或 transport EOF 当最终成功。`libfs::client::Transport::open_stream` 组合 namespace 位置、Open、typed Attach 和 Start；`read_stream_until` 在返回正常 EOF 前先完成 Finish 并核对业务成功，写方须显式 Finish，错误类型保留已领取的控制/Invitation/Attach owner。

当前 Open 消费者由 `test_fal` 与 init 的控制剧本共同拥有：`test_fal` 负责普通流写入/回读、Watch、冻结读、Offer 生命周期、受限 Open、条件身份和 pin；init 保留 Cancel/Query/Finish 控制、部分结果及 provider 监督所需的操作。终态唤醒先于来源注销和角色 close，因此 close 失败重试仍保留 owner/charge，不阻塞已冻结的 Finish 结果。Finish 的 Outbox 持有请求 Delivery，sender 的最后一个外部副本关闭不会提前终止仍在处理的发送授权寿命。provider 正常退出检查 StreamTable 空表及 FAL/Execution/ServiceResource 账户归零；两侧阶段和退出由同一 A/B 进程组合验证。

## F3f 条件 Open 与流 Copy 的当前接线

Open 固定体现在可携 `expected_identity`（零为无条件，非零为本次预期 NodeId）。`srv_fs::StreamTask::prepare` 在同一次 `FalBackend::resolve` 取得的 NodeRef 上核验，再分配表位、Tunnel 或回复；`libfs::Transport::open_stream` 从 Position 的身份填写该条件。`test_fal` 的用例先 Resolve，再删除同名重建，验证旧位置 Open 返回 Conflict 且替代者未写入；另一流已 Open 后 unlink，仍通过 pin 完成 Write/Finish。raw Open 继续允许零条件；这里的 NodeId 仅在同一个 provider/父 grant 下使用，不作为跨 provider 全局身份。

`libfs::client::copy` 持目标父 DirectoryGrant、单组件名字、provider object id 与 Create `NodeInfo` 构成 CreatedTarget；Create 是独占的，`libfal::Client::call_classified` 将本地编码/准备失败、未投递、已投递未知和服务明确拒绝分开，Unknown 保留目标 locator，不自动重试。CreatedTarget 写 Open 以 NodeInfo.identity 作条件；显式 `delete_if_current` 先 Lookup 当前身份与版本，再条件 Delete，替代者或并发版本变化均不删除，普通失败不自动删部分目标。

Copy 在源和目标先后 Open 后，以 `libexecution::Runtime` 一条任务推进两个 RNL2 角色，一轮至多 1024 字节；每端一个数据来源与只看 CLOSED/PEER_CLOSED 的终态来源，可选取消来源，总上限五个。数据电平只在需要读/写且未武装时 rearm，终态来源在对端背压期间也保持可观察；来源均注销后才交还 Stream owner。Runtime 的本地 Budget/ExecutionResource 预付 Task 与按五来源推导的 InputBytes，正常关闭检查双额度归零；关闭失败保留 WaitSet、Task 与缓冲快照，由 CopyFailure 在仍持双端流时续作有界退休。数据 EOF 仅结束搬运，随后源 Finish 确认，目标才发 EOF 并等待目标 Finish；字节搬运、两端 accepted/transported 完全一致且两端 Completed 才交付 CopySuccess。失败先尝试分别关闭两端数据角色，再为源/目标各自建立 control Cancel；清理期限为有限上界且不晚于传入业务期限。任一调用失败时 CopyFailure 仍携源/目标 Stream、CreatedTarget 或已送出 Create 的 Unknown locator，不将 Drop 当退款证明。

`test_fal` 使用正式 A/B 执行跨环、同 provider、空源、创建冲突、预取消、进度后取消、条件清理及不误删替代者等 Copy 场景。`CopyFailure` 不自动 Delete；清理先分别尝试解除两端数据角色，再为源/目标各自建立 control Cancel，两个取消请求拥有独立 owner，避免一端等待阻止另一端停止。自动失败清理使用有限期限，不延长原业务成功期限。取消 RPC 错误按源/目标保留 typed CallOwner，关闭失败返回原 owner 供显式重试，不能因另一端关闭成功报告全部清理成功。消费者另验证已投递 Create 的回复隔离、完成操作终态和 provider CLOSED 后的新请求未投递；init 保留监督/注册控制剧本，普通写入/回读/Watch 由 `test_fal` 负责。
新增资源验证先占满正式 StreamTable/Watch 配额，核对 `Quota` 拒绝、owner 释放与重新准入；在途 Copy 在目标已接受至少 4 KiB 后向 init 报告 armed，由 init 释放 B provider，消费者核对部分进度、失败边界和目标 owner 可取回。

独立 `test_fal` 进程已从 init 获得裁剪 `GRANT` 的 A 根/服务发现目录、命令 Notification owner 和报告 signaler；它先订阅目录并发 `DiscoveryArmed`，init 方才启动 B。进程从正式 ServiceRecord 中取得 B grant 并调用 B，不经 bootstrap 直授；init 完成 A/B fixture 后以正式 route Bind 将 `/second` 接到 B，再发送 `Continue`，消费者才解析 `/second`、`/second/f2-dir/leaf` 并检查 Delegate ceiling，同时验证发现根看不到内存 route、受限 Open 返回 Permission。消费者另建 A root Watch，以派生的不同授权上下文 Query 同一 id 必须返回 Permission，再取消订阅。对 init 在 A/B 创建的 repeatable Handle 属性各连续 Read 两次，取得的 MailboxSender 显式 Close；对两端 affine 属性则以 typed Mailbox、minted sender 和 send-once 回复授权发一次 Take，关闭 Mailbox owner 后在同一绝对期限内重取并核对空值，再关闭 sender/lifetime。A/B 的根枚举与同 provider Move 使用 init 已创建的 fixture，核对 f2-dir/f2-link、源消失和目标出现；CREATE/MODIFY/DELETE Watch 使用消费者自建属性：各自在 Create/Write 前订阅，核对 Events 与 generation/Active/NodeDeleted，取消后条件 Delete 验证旧订阅不再观察到新事件。进度后取消 Copy 允许 pump 内取消或目标已传满但 Finish 前取消，两种结果都须保留至少 4 KiB 进度、无本地 owner 泄漏并可条件清理部分目标；轮询不提供严格中途屏障。已消费的发现 Watch、业务局部 owner 与 Client 都在 `Complete` 前退出作用域。`SecondaryDiscovered`、`Continue`、`Complete` 与 `ProviderClosed` 由 init 的有限等待和 ProcessControl 核验串联。A/B 已有 Property Copy/ReadAt、bounded Read、普通流内容/冻结终点、Offer 丢弃、未 Attach 到期、pre-Start EOF、已投递 Create 回复隔离、完成操作终态及 provider CLOSED 后未投递请求也由消费者执行；init 的第二次 Record 读取、Cancel/Query/Finish 和 provider 退出报告是监督装配所需，与测试进程的独立发现并非同一责任。Open reply abandonment 使用满箱 typed reply Mailbox 和独立 Lifetime 验证；provider 中途退出、终态失败和双端退款由 A/B 退出组合及完整矩阵覆盖。
资源配额场景在先前 Watch/Stream 退休可能异步的情况下以正式 `Quota` 作为饱和边界，并等待释放后的重新准入，不把固定槽位序号当作同步事实。


## F1 接手边界

F1 不再拆成“先后端、后授权”的文件阶段，而是一个单 provider 纵向闭包：

```text
服务状态拥有者
  ├── Runtime / WaitSet / 任务与期限
  ├── GrantTable / AccessSnapshot / sender identity
  ├── FAL2 request/response/provider/client 接缝
  ├── MemoryBackend / NodeStore / Data / StoredValue
  └── retire queue / explicit Wake / Account refund
```

F1 的唯一真实消费者是正式 provider 运行体及其最小 client。不得以 v1 `MemFs`、同进程 self-pump、slot-1 anchor 或 host mock 充当 F1 消费者。完成门必须覆盖授权不可伪造、五类 mutation 的正常/准备失败/取消/冲突/提交/旧值退休、服务公平推进、期限、调用者退出、服务退出、Delivery/Outbox 和 metadata/account 退款。

F1 的目标类型图已进一步冻结：`libexecution::Runtime` 是 WaitSet 的唯一登记/接收/关闭 owner；grant Lifetime 由正式 Runtime task 观察，GrantTable 不再直接借用 WaitSet；backend retire 由预先登记的 Notification source 显式唤醒。provider-local 根 DirectoryGrant 与基础操作最小 client 已建立。执行与 FAL 领域分别使用 `ExecutionResource`、`FalResource`，并通过同一 `libbudget::Account` 的两个 view 共享付款来源而不混合分类。

F2 已接通独立 provider/client、namespace/Delegate、真实启动能力图，并删除 v1 临时 anchor、无鉴权 MemFs 与同进程旧泵；F3a–F3c 已接通公开 Move、Record/Handle/Take、属性 Copy 与 Watch，F3e/F3f 已接通 Open/流与双端 Copy，F4 已完成独立 `test_fal` 业务消费者、RPC→FAL→Create 生命周期、退出观察和整体验收。后续实现以各机制的正式消费者和唯一工作记录为准；发现生产 owner 或失败路径缺陷时回其机制拥有处修复。
