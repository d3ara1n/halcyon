# FAL 实现现状

方向见 [`../ideas/fal.md`](../ideas/fal.md) 与 [`../ideas/fs.md`](../ideas/fs.md)。通用 RPC/Runtime/Runnel 的实现分别由 [`rpc.md`](rpc.md)、[`runtime.md`](runtime.md) 与 [`runnel.md`](runnel.md) 拥有；本篇只记录 FAL 当前代码事实。唯一施工导航是 [`FAL 整体计划`](../../plans/todo-2026-09-fal-service-capabilities.md)。

## 正式生产链

F1、F2 与 F3a–F3c 已闭合到当前 core/release 开发门。`srv_init` 以三项启动 grant 分别启动两个独立 `srv_fs`；每个 provider 的主线程直接运行唯一 Runtime，没有进程内 self-client、worker 泵或第二套 WaitSet。正式调用链是：

```text
srv_init / libfs::resolve
  → libfal::client / librpc::Caller / Mailbox / Delivery
  → srv_fs::Ingress + RequestTask + Outbox
  → GrantTable::snapshot / AccessSnapshot
  → MemoryBackend / NodeStore / Data / StoredValue
```

FAL1 protocol id、FalHeader、slot-1 anchor、`MemFs`、旧 provider dispatch 和同进程泵已随最后一个消费者迁移整体删除。`DirectoryGrant<G>` 持正式 endpoint owner，provider 只按内核 sender identity 取得授权快照。

### FAL2 积木与接线状态

`store.rs`、`backend.rs`、`data.rs`、`value.rs`、`grant.rs`、`authority.rs`、`protocol.rs` 和 `resource.rs` 现已由 `srv_fs` 的正式 provider/client 共同消费：

- `NodeStore` 用 `NodeRef` 的 pin 与目录 link 分账，PreparedNode/PreparedMutation/PreparedWrite/StoredValue 预付容量与账户额度；冲突原样返还事务，旧属性、旧块和摘链节点进入有界退休。
- `MemoryBackend` 具备 Create/Delete/Write/Property/Move 的 prepare、位置/结构代次校验和无分配 commit；Move/Delete 同时推进目标节点版本，为节点 Watch 提供单一代次真值；所有后端访问要求不可外部构造的 `AccessSnapshot`。
- `GrantTable::snapshot` 是 `AccessSnapshot` 的唯一构造点；GrantTable 绑定真实 provider Mailbox identity，以 sender context 查表，持 Lifetime CLOSED 观察和账户收费。`prepare_derive` 从活动父 grant 继承账户与运输 ceiling、拒绝权限放大，并为稳定目录 root 创建独立 sender/Lifetime。Runtime task 先登记 Lifetime source 再安装和回复发布；回复放弃会关闭 sender 并走同一退休退款路径。
- FAL2 `protocol.rs` 提供 Lookup/Create/Read/Write/ReadAt/WriteAt/Delete/Enumerate/Link/Derive/Move/Take/Subscribe/QuerySubscription/Unsubscribe；Lookup 成功体为严格 Found/Delegate/LinkBoundary 三态，Delegate 与 Derive 成功各要求回复 capability slot 0，Move 的请求 slot 1 携带目标 DirectoryGrant，Subscribe 的请求 slot 1 携带 Notification signaler，属性值按自身 Handle 字段声明业务槽，枚举项流按声明 count 完整消费且拒绝残尾。
- `srv_fs::server::run` 统一拥有 Runtime/WaitSet、GrantTable、MemoryBackend、退休 Notification/`NotificationWake`、Outbox 和 provider-local 根 grant。Ingress 从内核填入的 `sender_context_id` 获取 `AccessSnapshot`，payload 中的数字不能伪造授权。
- provider 由同一 `libbudget::Account` 建立两个不可变视图：`AccountView<ExecutionResource>` 支付 Runtime 的 Task/InputBytes，`AccountView<FalResource>` 支付 Node/Bytes/Grant/Watch/WaitSource。两个视图共享付款身份与总账额度，Request/Outbox 不重复计入机械执行槽。

- F2 已接通两个独立 provider、独立管理 endpoint 与真实 Delegate：init 为两个 `srv_fs` 分别提供 bootstrap/release/route mailbox，取得 object identity 不同的 root grant，再经 provider A 的 route endpoint 交付 provider B 的母 grant和 FAL rights ceiling。A 的普通 Lookup 命中绑定后不直接转交母本，而由同一 Runtime 中的 `librpc::Dispatcher` 非阻塞调用 B 的 Derive；完成后 `DelegateTask` 将独立子 grant、consumed 与 remaining 随严格 capability slot 0 回复。init 的 Namespace 只挂 A 的 `/`，正式 `libfs::client::Transport` 已通过 `/second` 与 `/second/f2-dir/leaf` 跨越 Delegate 访问 B，并验证授权衰减。启动 grants 缺失或 role 错误时 fail-closed。
### Owner 与收束边界

未提交准备阶段必须携带全部 owner：节点 pin、目录/块 Permit、Account Charge、能力 owner、回复和 Delivery。准备失败或 commit 冲突原样返还这些责任。成功提交后，旧值和最后引用由后端结果与退休队列承接；回复失败只能形成 `OutboxResult::Abandoned`，不能伪造业务回滚。服务退出需要停止准入、取消/完成请求、收束 GrantTable、backend retire queue、Delivery、reply-once、WaitSet，并确认账户退款。

后端成功替换/摘链后的旧值由显式 retire task 通过 Notification 电平唤醒并按预算推进；release Notification 是同一 Runtime 的正式 source。服务退出顺序为停止准入、Dispatcher 取消/完成下游调用、撤销全部 grant Lifetime source、收束 backend retire、移除 route/release/retire source、关闭 Runtime。固定宽 `ProviderReport` 在全部账户归零后报告 committed、回复 abandoned 和已投递下游调用 abandoned。

当前 host 验证为 `libbudget` 6 项、`libexecution` 22 项、`libfal` 27 项、`libprocess` 16 项；`libfs` 既有 17 项保持通过基线。库迁移后的 `just check`、七面 `just clippy`、`git diff --check` 与 `THROTTLE=100 just acceptance` 已通过，完整覆盖 stress 16/16、release、`sifive_u`、`virt-nofd` 与 panic/alloc/fatal 三类 boot-failure。双 provider/退出组合纳入后，`VIRT_NOFD_TIMEOUT=45s`、`SIFIVE_U_TIMEOUT=60s` 均已按实际负载重校。QEMU 除双 provider/Delegate 正常链外还覆盖两条确定性退出链：B 在业务提交后因满回复箱停驻，release 产生 `abandoned=1`；A 的下游 Derive 已进入 init 持有的静默 Mailbox，release 产生 `downstream_abandoned=1`，随后 Cancelled 客户端回复也以 `abandoned=1` 收束。两端最终均完成 Runtime、GrantTable、Watch、route owner、Delivery/reply-once、账户退款与监督回收。

## F3a 同域 Move 接线

F3a 在 FAL2 基础操作之上增加 `Op::Move` 与 `Status::CrossDevice`。请求正文携带源稳定父目录相对路径、源名字、目标最终名字和可选的 NodeId/version 预期位置；业务 slot 1 携带目标 DirectoryGrant，slot 0 仍由 RPC 保留为 reply-once。`libfal::client::Client::move_entry` 复制目标 sender 并以 typed capability 发送，失败时由 RPC packet 返还；`libfs::client::Transport::move_entry` 将该操作暴露给稳定 DirectoryGrant/Namespace 走路层。

`srv_fs::Ingress` 按 opcode 精确准入业务 handle 数，使用 `GrantTable::validate_received` 校验目标 sender 的真实 object identity、Mailbox 归属、role 和活动 grant。不同 provider 的目标 sender 返回 `CrossDevice`，不会进入后端事务。相同 provider 的请求创建 `MoveOperation`，先完成源/目标权限和稳定位置准备，再由 `RequestTask` 按 Runtime budget 分步调用 `validate_move_step`，循环检查结束后无分配 `commit`；准备失败、循环冲突、版本冲突和回复放弃均保留后端 owner。

当前计划内消费者为 `srv_init` 的 FAL2 provider 剧本：每个 provider 执行一次同域目录 Move 并检查旧路径消失、新路径出现；双 provider 剧本额外把 A 的源目录发送给 B，要求返回 `CrossDevice` 且源保持可见。此处消费者属于 F3a 的计划内装配，不提前建立独立 `test_fal`。

## F3b Record、Handle、Take 与属性 Copy

属性值使用 `value.rs` 的统一非递归编码：标量、同构 Array、异构具名 Record 与 Handle 共享总 payload、元素和 capability 槽预算；重复字段、未引用槽、重复槽、错误 role/rights、非法 ExportPolicy 均在存储前拒绝。Create/Write 从 `RequestContext::HandleSet` 一次取走全部业务 owner，准备失败仍由请求上下文或 `StoreFailure` 完整持有；提交替换后的旧值沿既有退休责任关闭。

repeatable Handle 属性保存完整母本和 ExportPolicy。Read 同时要求 `READ_PROPERTY` 与 `ACQUIRE_CAPABILITY`，按声明的运输 rights duplicate，重写为从回复 slot 0 开始的 canonical 槽位，并由客户端再次执行值/槽完整验证。DirectoryGrant 不走该路径，因为裁剪内核 rights 不能收窄 FAL ceiling；当前返回 `Unsupported`，正式目录能力出口仍由 Derive 负责。

affine 属性只能通过 `Take`。`PreparedTake` 先预付空 Blob、取出原值并设置节点独占 Busy 门；该门阻止 Read/Write/Move/枚举和元数据观察越过预留。`TakeOperation` 在取出 owner 前预留政策与恢复容器，再将原始 owner 放入预备回复；Outbox 成功入箱后 `commit_take` 无失败地发布空值与新版本并把 Bytes charge 收缩到实际空值，任何未投递终态则从 Packet 逆序取回 owner 并 `rollback_take` 恢复原值。DirectoryGrant 的 affine Take 同样返回 `Unsupported`，不能绕过 Derive 的 FAL ceiling。回复放弃的 Take 不计为业务 committed。服务 Runtime 与 Outbox 仍拥有期限、关闭、来源注销和 Delivery 收束。

基本属性 Copy 由 `libfal::client::copy_property` 和 `libfs::client::Transport::copy_property` 编排为源 Read 后目标 Create。它只接受无 capability 的完整属性快照，目标已存在返回 Exists，不覆盖、不执行 copy+delete，也不承诺跨 provider 原子性。`srv_init` 对每个 provider 验证本地 Copy，并在两个独立 provider 间验证跨域 Copy；同一剧本还验证 repeatable Handle 两次读取、affine Take、关闭回复 Mailbox owner 后恢复再取以及 Take 后空值。

## F3c Watch

F3c 发布 `Subscribe`、`QuerySubscription` 与 `Unsubscribe`。客户端 `Subscription` 独占 Notification owner，并复制一份 grant 使用引用；provider 消费具备 `SIGNAL | WAIT | TRANSIT` 的 signaler。每项订阅由独立 Runtime task 拥有 signaler、Watch/WaitSource charge、被观察 `NodeRef`、回复 Outbox 和来源注销责任。任务先登记 signaler 的 `CLOSED` 来源，登记回调再按原 AccessSnapshot 重新解析路径、确认稳定 NodeId 与当前 WATCH 权限，并在同一推进点安装记录和取得节点版本；安装后的事件可早于 Subscribe 回复，安装前变化则由回复代次显式体现，不形成丢失窗口。

provider-local Watch 表固定上限 8；表以单调 subscription id 寻址，同时保存内核 sender context，Query/Unsubscribe 必须由原 grant context 发起。发布记录只在后端 commit 成功后更新：目录直接成员 Create/Delete/Move 分别产生 CREATE/DELETE/RENAME，属性、流和成功 Take 产生 MODIFY，被观察节点 Delete 产生 `DELETE | TERMINATED` 并保留 `NodeDeleted` 终因。每项订阅按 OR 合并位，一次提交最多唤醒每个 Watch task 一次；signal syscall 只由该任务执行。

Unsubscribe 先从发布表摘除并清空尚未 signal 的 pending 位，再唤醒任务撤销 source，回复确认后不再产生新事件；Notification owner 静默关闭走同一退休路径。Subscribe 回复 abandoned 会由服务端主动摘表、撤源和退款，不依赖客户端最终关闭；provider 停止时先合并并 signal `TERMINATED`，随后撤源关闭 signaler。`srv_init` 在两个独立 provider 上验证目录 CREATE、节点 MODIFY、代次推进、外来 grant context 拒绝、显式取消后静默、owner 静默消散、节点删除终态、provider 停止终态与最终 Watch/WaitSource 退款。

## 剩余能力的代码基线

FAL 总计划第 4/8 节已完成跨阶段边界及 F3d 详细设计，下一步为实施；本轮没有新增运行代码。注册表、`ServiceRecord`、`RegistrationControl`、`libservice` 及通用 provider/Registry 后端均尚不存在，不能把设计完成记成服务发现已交付。F3e/F3f 仍需各自的局部详细设计。

当前源码对 F3d 的约束：

- `srv_fs::World`、`lookup_v2`、`node_info_v2`、`serve_v2` 和 Watch 安装复查直接消费 `MemoryBackend/Body`；`progress_dispatch` 的结果路由面向 Delegate。当前每个进程只有一个 FAL provider；设计中的同 Runtime 双 provider 尚未接入。
- `NodeRef` 只保持稳定身份及 pin，`NodeStore::get_mut` 仍能改变 payload，不提供不可变属性快照。现有同步 Read 不能直接延长为跨下游 RPC 的借用。
- `StoredValue::prepare` 可以接收 Directory 标签及政策，但 `duplicate_for_reply` 与 affine Take 的 Directory 出口返回 Unsupported；目标 provider 的 `GrantTable::prepare_derive` 与正式 Delegate 已存在。标签并不验证目标 FAL 协议；设计所需通用异步 Record 派生尚未接通。
- `GrantTable::prepare_derive` 继承父账户/运输政策；当前 route Delegate 的权限计算为路径 grant 与绑定 ceiling 交集。这是路径语义，尚无服务 Record 的独立出口权限计算。
- `Issuance::output_transport` 已存入 GrantState 并随 Derive 继承，但未进入 AccessSnapshot；`output_rights` 无实际调用者，Record Read/Take 未执行该运输上限。F3d 总计划单列其接通责任；现状不宣称影子政策已经生效。
- `watch::Table`/WakeSet 上限为 8；effects 最多 3 个节点，publish 同步写 pending/generation 后返回需唤醒 task。`publish_watch_events` 逐个调用 `Requests::wake`；Runtime 单次 Requests 容量为 16，没有一条批量唤醒原语。可配置准入和持有剩余责任的 WakeBatch 尚未实现。
- `server::run` 当前配置 Task=32、Source=64、Node=64、Bytes=2 MiB、Grant=16、Watch=8、FAL WaitSource=48；InputBytes 从 Runtime 推导。唯一 Account 绑定执行/FAL 两个 view，服务 view 尚不存在。FAL Request/Outbox/Offer/Stream 的零槽当前不计费，不能把它们说成已预付独立额度。
- 停止时 seal GrantTable/Runtime，`shutdown_turn` 逐任务交付 stop；任务驱动 Outbox、撤源及退休，Drained 后关闭 Runtime、授权表与 route owner，并断言账户退款。新增 Registry/出口责任尚不在这条实际收束链中。

当前 `srv_init` 仍从两个 srv_fs 的 bootstrap 分别接收 root，route-management 仅拥有跨 provider 路由绑定。设计中的 A 目录承载、B StartupBlock 名称授权/主动发布、init 先订阅后发现及真实调用尚未迁移；旧启动链的删除条件与顺序由唯一总计划拥有。公共记账/执行和库重排基线仍为 `96ee03b`，不因本次设计重开。


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

F2 接通独立 provider/client、namespace/Delegate、真实启动能力图，并在最后一个旧消费者迁移时删除 v1 临时 anchor、无鉴权 MemFs 与同进程旧泵；F3a–F3c 已依次接通公开 Move、Record/Handle/Take、属性 Copy 与 Watch。剩余设计与任务顺序由 FAL 总计划唯一拥有；F4 只建立独立 `test_fal`、完成组合验收与归档，不承接尚未完成的生产 owner 或失败路径。
