# IPC 对象实现

ABI 位于 `shared/erhino_shared/src/{object,message,wait,wait_set,call}.rs`，内核位于 `os/kernel/src/task/`，用户封装位于 `user/libraries/libkernel/src/ipc/`。CapTable、Mailbox、Process、Tunnel、单来源续体、扁平 WaitSet 与实际 runtime/监督已接同一契约。接续与验证边界由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。

## Handle 与对象身份

[`os/capability`](../../os/capability/src/table.rs) 保存 generation slot、authority 与 rights，kind 从具体对象查询，没有独立 role。安装在表内 reserve 后放锁写回/必成提交出生，再回表 commit；shut 撤销返还资格时返回最后持有事件，锁外交付正常放弃。事件批次容量在 reserve 准备，commit 不分配。消息移动要求 TRANSIT，直接 grant 要求 GRANT；所有业务释放动作在表锁外交接。

ObjectHeader 使用不回绕 KOID。HandleQuery 返回已有对象的 kind、rights、object_id、related_object_id、badge 与 reserved，不包含 signals 或 allowed_signals，也不按身份打开对象或授予权限。NotificationCore 持唯一 Owner/source header，Owner 自关联，Signaler 与 NotificationObserver 各持独立 header、关联同一 core KOID。Owner 上限 READ/WAIT/MANAGE/GRANT，Signaler 上限 SIGNAL/WAIT/TRANSIT/GRANT/DUPLICATE，Observer 上限 WAIT/DUPLICATE/TRANSIT/GRANT；Owner 与 Observer 观察 READABLE/CLOSED，Signaler 只观察 CLOSED。NotificationMintObserver(0x3d) 只接受 Owner[MANAGE]，输出必须含 WAIT 且不超 Observer 上限，不消费输入 Owner。三角色共享 core、WaitQueue 和 Work；Signaler、Observer 与等待 loan 都不增加 Owner 业务 holders，最后 Owner 关闭、清位，其他引用只保活观察存储。Observer 实际 Arc 壳由派生调用者 Pool 付款，沿共同 handle install 预约/锁外写回/commit；失败释放未交付 owner 和费用，已经接受后允许交付 CLOSED 观察角色。

libkernel `Capability` 拥有合法非映射 entry，适用于叶对象和 affine owner；包装不自动授予 TRANSIT/GRANT。`into_raw` 移交唯一责任，失败 close 返还 owner；Drop 使用 `close_object_owner`，允许等待预付的内核退休，返回错误报告不变量破坏，不静默丢弃责任。WaitSet 的 `create_with_rights` 支持显式 GRANT，默认 `create` 权限仍为 READ/WAIT/MANAGE；`into_capability` 消耗原 owner，不复制关闭责任。

`os/capability::CapTable<A,C>` 的槽数组、安装句柄和释放事件数组均持固定布局存储及 charge；金融政策由调用方传入，内核选择该表所属进程的默认 Pool。增长期间新旧块同时收费，搬移保留所有槽编号与代次，旧空块完整释放；逻辑 remove/rollback 不缩容量。metadata 失败映射 QuotaExceeded，分配失败为 OutOfMemory，提交只使用已准备容量。最后事件锁外投递；句柄输出 buffer 可继续存活并持付款到最后使用结束。Handle 的槽宽度来自 ABI u32，实际可达容量另受 metadata 供给约束；无单独旧 64k 限额。源码及金融 owner 见 [`table.rs`](../../os/capability/src/table.rs)、[`handle.rs`](../../os/kernel/src/task/handle.rs)；共用存储与退款顺序由 [`mm.md`](mm.md#已付款保留存储) 拥有。

## Mailbox 与交付

MailboxCreate 只交付 owner。MintSender 要求 owner MANAGE，原子创建独立 MailboxSender 和对应 LifetimeObserver。sender 的 KOID/badge 不可变，并强持目标队列；duplicate、once 派生和 move 保持该上下文。MessageHeader 的 PID 是来源信息，badge/context ID 是目标发送授权；内核生成，不由 payload 声明。

唯一 MailboxReceiver 的 ObjectHeader 是 core header，不另铸造接收角色 KOID；Sender 的 related_id 因而对应 Receiver 的真实对象身份。Receiver/各发送对象的 Holders 与计数仍独立，core 不作为第二个 capability 对象公开。

[`Lifetime`](../../os/kernel/src/task/lifetime.rs) 用同一 OBJECT_WAIT 域保存 authority/use 计数、电平与侵入式节点；计数双零永久 CLOSED。一个 Sender 持一份 LifetimeOwner，派生 OnceSender 独立取得父域 authority；最后 Sender capability、Once 的 Sent/Abandoned 各归还其 authority。每次准备 Delivery 取得一份仿射 LifetimeUse，失败退回，成功随排队/接收预留/接收 capability 转移。临时 Arc/观察 loan 不计入业务计数，Lifetime 只记录 sender KOID，不反向持发送对象。

[`Delivery`](../../os/kernel/src/task/delivery.rs) 保存 use lease、数值 sender_context_id 与付款 Pool；最后 capability 在自身 LEAF 锁下取出 lease，锁外归还。业务 use 不依赖 Delivery 存储最终 Drop；未发布出生的 Drop 也退同一 lease。用户态 Delivery::finish 消费式关闭其唯一表项，只有成功后清空 owner；已移交领域责任的操作可以据此结束入口 use 并独立保管回复权，关闭失败不丢原 Delivery。只允许 TRANSIT/GRANT，不可 DUPLICATE。Lifetime 及 ReplyObserver 的观察来源只提示等待 owner，不代摘节点；ReplyObserver 的 DONE/PEER_CLOSED 分别对应 Sent/Abandoned，Once 在 MAILBOX 锁内选定终态，持有操作 authority 的执行者在锁外发布。

队列上限为16，每条最多8项业务transit加1项Delivery。MailboxCore出生时付款并初始化固定`Buffer<Option<Message>>`环；head/len与receiving占位拥有容量真值，enqueue/rollback/pop不分配或扩容。消息本体内嵌槽，payload/handles独立付款；handle数组在发送时预留Delivery槽，接收只push，不另分配。Send先做无分配长度检查，再接受普通Sender use或取得Once独占Invocation并校验Pending，从真实对象决定本次payer。普通Sender使用实际发送进程Pool；Once从发行时metadata charge取得make_send_once调用者Pool，输入正文/raw moves、验证/Invocation/rights scratch、运输payload/handles和Delivery均使用该账户。付款不改变uaccess的当前进程或发送PID/context。成功运输payload/handles保留原payer至consume/discard/shut，Delivery可继续持原费用到最后capability/存储释放；发行者死亡或Pool句柄关闭不提前退款。临时scratch随本次owner释放。MAILBOX同步域拥有缓冲和WaitQueue：READABLE当且仅当队列非空且无receiving，WRITABLE占用包含receiving，PEER_CLOSED表示可投递Sender/Pending OnceSender为零；关闭冻结CLOSED。出生SenderCount负责计数与失败回滚，Once在准备提交时入计数、Sent/Abandoned时退出；预留期间Peek/Discard/另一Receive返回Busy。

[`handle.rs::Authorization`](../../os/kernel/src/task/handle.rs) 校验 Occupied 槽并持调用者 BodyLease、对象存储与 rights 快照，不抽取 Cap，也不新增 capability 计数。普通 Send 在表锁释放前取得 LifetimeUse，MakeSendOnce 在表内取得独立父域 authority；锁边为 HANDLE_TABLE → OBJECT_WAIT，凭证取得不发布、不回到表锁。Close 先到则拒绝接受，接受先到则 Close 只终结新调用资格，已取得 use/派生 owner 独立收束。普通 Query/WAIT 可与 Send 使用同一表项，不要求 DUPLICATE。

`Invocation` 只用于 Once 的消费和消息移交：持接管 Cap 与返还 ticket。Send 依当前 ABI 按 header/payload/moves/count/length 复制输入，拒绝重复 move 及 sender 自移交；每项 transit 先保存完整 authority，在 core 准入/预留队列/原 Deadline 检查都成功后才原位裁剪 rights 并入队。失败返原槽/原权限；Close/shut 撤预约则锁外放弃，不能复活已关闭槽。成功接管的 Once 先发布 Sent 再消费原 authority。入队、用户写回、安装和最后持有事件都在表锁外。

Receive 接受非消费 READ 并持 BodyLease，core 的 receiving token 仲裁唯一取队头事务。事务独立持有已摘出的 Message/Delivery；最后 Receiver Close 可以关闭 core，但不取走事务 owner。预留目标表槽后，在 VM 锁内通过受保护 SUM 复制输出。写回失败按原 tid/失败 VA 冻结 UserAccess，撤表预约；core 未关闭时回插队头，已关闭时锁外释放消息。成功复制后表 commit，随后完成出队。进程表 shut 撤销新槽公开时锁外释放返回事件，不能回滚已复制消息。Close 只关准入并激活出生 Work，Work 交替扫描一个来源候选和释放一条仍在队列的消息（至多九份 authority），引用/门铃均锁外执行；事务中消息的收束归唯一 Receive owner。

libkernel `wait_message_until` 在 Busy/NotAvailable 后独立检查原 Deadline，并用同一期限等待 READABLE/CLOSED；满箱重试不重新计算期限。时钟换算和全部绝对期限边界的验证由 [时间任务](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md) 拥有。

libkernel typed 运输层：`MailboxSender`/`SendOnce` 是投递目标的类型身份——铸造（`Mailbox::mint`/`send_once`）由内核保证 kind 不 Query，未知能力只在 `from_capability` 唯一转换边界 Query 一次（返回描述供一次性 rights 检查）。MakeSendOnce 输出完整 HandlePair；`send_once` 返回 ReplyPermit，分别持 typed SendOnce 与 ReplyObserver；`MintedSender.lifetime` 持 LifetimeObserver。三个公开 Observer 的数字 kind 保持 13/18/20，内部 Lifetime/LifetimeOwner/LifetimeUse 不改名。Notification 工厂返回 typed Owner/Signaler，Owner 提供 take/mint_observer、Signaler 提供 signal；未知能力唯一转换边界 Query 一次，observer 必须 WAIT，返回描述供领域额外授权检查。所有角色独占 Capability，无隐式 Clone，duplicate 经真实裁剪调用，close 失败完整返还 owner；显式 into_capability/into_raw 移交责任。安全层没有裸 Handle 的 Notification signal/take/observe 函数，正式原始夹具使用带 SAFETY 的 sys_notification_*。Request 持观察权，Packet 运输回复权；发送后观察权移交 Sent owner。raw 夹具显式关闭不用的观察表项。`Packet` 为消费式出站 owner：`try_send(self)`/`try_reply(self, once)` 成功即消费全部 transit owner 与回复授权，失败以 `SendFailure`/`ReplyFailure` 完整返还 Packet（与 send-once），没有 delivered tombstone；`Request`/`PreparedResponse` 以 take/restore 搬运完整 Packet 维持重试。`RequestContext::decode` 校验通过后才摘取槽位并构造 typed 回复授权；处理终结前 `delivery` 显式暴露。验收/竞态夹具仍以 unsafe `send_raw*` 直验内核契约，属保留的 raw 边界用途。

## 通知与等待

[`capability::signal`](../../os/capability/src/signal.rs) 拥有纯电平侵入式链。稳定节点由等待 owner 提供，挂接/摘除与电平、扫描游标受来源同一同步域保护；删除当前节点会前移游标，轮次边界隔开活动中新增节点，更新只请求后续轮次而不重置游标。每步检查一个节点或移动一次边界，短暂电平不锁存为完成；来源不代摘等待节点。

[`wait_source.rs`](../../os/kernel/src/task/wait_source.rs) 的 WaitQueue 随来源 core 内嵌 WorkRecord；chain 的 UnsafeCell 由 core 业务锁保护，core Arc 与 pin 节点保持地址稳定。节点只存目标 Weak 与等待 epoch/成员身份，owner 通过 ObservationSource 强持观察存储到 detach 返回。来源锁内只 upgrade 并复制 Delivery（强目标/身份，无节点借用）；解锁后 hint，不写结果或清理节点。维护重复活动与强根结算见 [`internals.md`](internals.md)。

[`NotificationCore`](../../os/kernel/src/task/notification.rs) 在 NOTIFICATION 锁下保存 OR pending bits 与 closed，并更新同域电平；READABLE 表示非零，Take 消费指定位。core 强持付款 Pool，记录关联其 Account；最后 Owner authority 关闭准入/清位并激活来源 Work。Work 每步处理一个候选，临时强目标带出锁后才提示，无工作回 Idle。旧 ObjectWaitState/serial/ObserverSink 不参与 Notification 的新来源。

SignalSource是无消息缓冲的已付款终态观察宿主，OBJECT_WAIT唯一拥有公开电平/节点链和独立内部完成状态，内嵌WorkRecord。Shell、ThreadControl、Job、ReplyObserver使用它；业务终态发布不反取业务锁。单一内部维护消费者先arm再在同域登记/复检WakeToken；complete_internal只提交内部退款事实并锁外唤醒，不更新公开电平。Process退休借Shell既有来源存储，内部退休结束与公开Process DONE是两个事实，避免额外私有Source晚于Body屏障退款。ThreadDeparture最后退款后持DONE来源直接发布，不因Control消散跳过。Tunnel在Connection同步域使用同一WaitQueue；来源及观察引用不强持待退款的privateWork。

[`wait.rs`](../../os/kernel/src/task/wait.rs) 的 WaitOne 复制固定输入，在表锁内解析 WAIT 存储引用、锁外取得来源 loan 与快照；输入兴趣必须非空且属于对象允许 mask。a1 返回当前匹配电平，关闭总附 CLOSED，NONE 表示到期；无异步用户输出指针。libkernel 从返回电平构造 WaitResult，Mailbox/Endpoint/WaitSet 的单对象便利等待使用该入口。线程续体/唯一 detach 由 [`task.md`](task.md) 拥有；timer 节点由 [`time.md`](time.md) 拥有。

## WaitSet 与内核退休

[`wait_set.rs`](../../os/kernel/src/task/wait_set.rs) 只接受真实来源，按 ObjectKind 拒绝 WaitSet 成员。ABI 为 Create/Register/Receive/Remove，没有 Rearm。MemberRequest 拥有 handle/signals/cookie，ReadyRecord 是 token/cookie/observed/reason，无 arm_generation/历史错误结果。

Member位于`Pin<OwnedBox<Member,MetadataCharge>>`，唯一拥有稳定WaitNode与来源loan；Pinned Member和Registration树节点由集合Pool分别按布局付款，挂链前在所有集合锁外准备完整owner。Add锁外attach、回锁复检close/容量后发布；失败带完整候选出锁，Member先detach、再释放分配/退两份费用。新增成员进候选队列覆盖初始电平/安装窗口；token单调不复用。写回失败走Remove，Remove/Close获同表项者独占detach。

候选是表项内双向token链，hint同集合锁挂队尾。Receive以receiving串行化，先在锁外用接收者Pool付款候选/ReadyRecord两个scratch数组；锁内取有界一批，锁外查来源，回锁验证token仍在。满足者回队尾并返回，查询中的新提示不会被不满足结果清掉。批次先取完再重入队，永久就绪成员本批至多一次；输出失败不消费电平，满足项继续在队尾。候选/输出数组实际capacity费用随各自owner释放。

最后capability锁外close，集合锁内关准入/发布CLOSED，出生Work强持core并交替自摘一成员/扫描一观察候选。摘出的完整节点和Pinned Member均到集合锁外detach、释放、退款，不依赖调用者续调。观察loan仍可查询CLOSED/自摘，不代摘线程节点。WaitSet Arc宿主、内嵌Work/节点、独立Member/索引均按实际布局付款；core及观察loan的存储费用随last-live结束，逻辑Close不提前退款。

libexecution 的持久来源/Assembly 已接当前 ABI，源码与 owner 由 [framework](framework.md#libexecution-的执行与扁平装配) 拥有。正式消费者与验收编排边界见 [runtime](runtime.md)；init/test 使用持久成员、Process DONE 与独立 Endpoint/View，不保留旧集合/Drain/Tunnel 接口。

## 退出与预算

ProcessDrain syscall/request 不存在；进程 Frozen 后回收记录自主完成实际 Body 退款，再发布 Shell DONE。显式等待使用 WaitOne 或持久 WaitSet；capability Close 不隐式等待。退出执行 owner 唯一 detach 来源、清停时节点，再发布 ThreadDeparture，不保留异步用户输出义务。

Mailbox/Lifetime/Delivery、Notification/观察壳、WaitSet和SignalSource均出生时先在原付款Pool取得实际Arc布局费用，再分配；末字段强持payer并在全部先行成员销毁后归还费用。消息固定环与独立payload/handles/运输scratch各自付款，运输不改变原payer。只为保活的_pool字段已由charge取代，业务身份、authority/use和financial存活分别计。QuotaExceeded与OutOfMemory分开，拒绝/写回失败沿原owner退款。Pool Account承接维护执行，来源候选、VM片段、消息清理每step有结构上界；Account最终析构计量及运行注入覆盖的限制由工作记录拥有。

## 验证设施

[`srv_init/public_ipc.rs`](../../user/services/srv_init/src/public_ipc.rs) 使用真实用户线程验证同槽 Sender 的 Send/Query/WAIT、接收竞争与背压、transit/Once/Delivery/Lifetime 收束，以及已提交等待的调用者退出。[`srv_init/time_checks.rs`](../../user/services/srv_init/src/time_checks.rs) 使用实际 syscall 验证绝对期限、过期优先级、取消观察和已提交投递的保留。

capability 的 signal host 用例验证插入边界、删除扫描游标、瞬态电平与关闭提示；table host 用例验证消费 claim/restore、Close/shut、rights 和持有数。内部安装、迟到提示、写回撤销及实际 metadata 付款的证据边界由[对象链工作记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。历史自检和运行结果不作为当前实现说明，当前源码不保留旧 actor/Drain 自检。

Tunnel/Endpoint/backing 的机制见 [tunnel.md](tunnel.md) 与 [mm.md](mm.md)，Runnel 数据布局见 [runnel.md](runnel.md)，Process 自动回收与 Job 生命周期见 [task.md](task.md)。
