# IPC 对象实现

公共对象、消息、观察与内核退休前置已完成。ABI 位于 `shared/erhino_shared/src/{object,message,wait,wait_set,call}.rs`，内核位于 `os/kernel/src/task/`，用户封装位于 `user/rinlib/src/ipc/`。施工证据见 [公共前置档案](../../plans/archived/todo-2026-09-13-public-ipc-wait-prerequisites.md)。公共时间、运输/RPC/服务执行和 FAL 业务仍未全部完成，不能把本前置完成视为总体交付。

## Handle 与对象身份

`os/handle_table` 保存 generation slot、object、role、rights。目标槽先 reserve，失败 rollback 推进 generation；准备后的 commit 不分配。消息移动要求 TRANSIT，Building direct grant 要求 GRANT；请求权限必须属于源 entry 与目标 role 的权限交集。所有最后引用及 close callback 在表锁外交接。

`ObjectHeader` 使用不回绕 KOID。HandleQuery 只描述已有对象的 kind、role、rights、badge 与关联身份，不按身份打开对象或授予权限。Mailbox/Notification/WaitSet owner 不可 DUPLICATE/TRANSIT，但可按明确 GRANT 权限直接移交；Tunnel Endpoint 依赖本地 VM lease，不可作为非映射 Capability 运输。

rinlib `Capability` 拥有合法非映射 entry，适用于叶对象和 affine owner；包装不自动授予 TRANSIT/GRANT。`into_raw` 移交唯一责任，失败 close 返还 owner；Drop 使用 `close_object_owner`，允许等待预付的内核退休，返回错误报告不变量破坏，不静默丢弃责任。WaitSet 的 `create_with_rights` 支持显式 GRANT，默认 `create` 权限仍为 READ/WAIT/MANAGE；`into_capability` 消耗原 owner，不复制关闭责任。

## Mailbox 与交付

MailboxCreate 只交付 owner。MintSender 要求 owner MANAGE，原子创建独立 MailboxSender 和对应 LifetimeObserver。sender 的 KOID/badge 不可变，并强持目标队列；duplicate、once 派生和 move 保持该上下文。MessageHeader 的 PID 是来源信息，badge/context ID 是目标发送授权；内核生成，不由 payload 声明。

Lifetime 只记录被观察的 sender KOID，不反向保活授权。最后 sender 引用释放时发布 CLOSED。每条 Message 的 affine Delivery 强持发送上下文；排队、接收预留、已安装接收能力均保留这项责任。Delivery 的 role 仅允许 TRANSIT/GRANT，不可 DUPLICATE。

队列上限为 16，每条最多 8 项业务 transit 加 1 项 Delivery。`MailboxState::publish` 从完整状态推导信号：READABLE 当且仅当队列非空且无 receiving，WRITABLE 的占用包含 receiving 占位；关闭冻结 CLOSED 并清除可读/可写。预留期间 Peek/Discard/另一 Receive 返回 Busy。

Send 按 HandleTable → Mailbox 锁阶准备 moves、Delivery、消息及容量，在同一提交内检查原 Deadline；失败保留 moves/once，成功才消费。Receive 初检输出后预留队头与目标槽，`finish_receive` 复制成功后原子交付；失败先退款表槽与队头，锁外通知。owner 已关闭时拒绝回插并锁外关闭 transit，成功复制则交付已经独立预留的消息。Discard/owner close 的队列析构在源锁外，fanout 有固定容量界限。

rinlib `wait_message_until` 在 Busy/NotAvailable 后独立检查原 Deadline，并用同一期限等待 READABLE/CLOSED；满箱重试不重新计算期限。时钟换算和全部绝对期限边界的验证由 [时间任务](../../plans/archived/todo-2026-09-monotonic-time-rpc-deadline.md) 拥有。

rinlib typed 运输层：`MailboxSender`/`SendOnce` 是投递目标的类型身份——铸造（`Mailbox::mint`/`send_once`）由内核保证 role 不 Query，未知能力只在 `from_capability` 唯一转换边界 Query 一次（返回描述供一次性 rights 检查）。`Packet` 为消费式出站 owner：`try_send(self)`/`try_reply(self, once)` 成功即消费全部 transit owner 与回复授权，失败以 `SendFailure`/`ReplyFailure` 完整返还 Packet（与 send-once），没有 delivered tombstone；`Request`/`PreparedResponse` 以 take/restore 搬运完整 Packet 维持重试。`RequestContext::decode` 校验通过后才摘取槽位并构造 typed 回复授权；处理终结前 `delivery` 显式暴露。验收/竞态夹具仍以 unsafe `send_raw*` 直验内核契约，属保留的 raw 边界用途。

## 通知与等待

Notification 保存 OR pending bits；READABLE 表示非零，Take 消费指定位。各对象的 `ObjectWaitState` 在源锁内记录 inactive→active 完整快照与 serial。通知与终态退休共用 `select_snapshot`，按未见且相关的最小 serial 选择，不按 signal 位序；无候选时当前 CLOSED 是终态 fallback。

立即命中/拒绝安装、取消、扫描和 unsubscribe 都交出完整 retired Subscription。`WaitAdvance::finish` 在来源锁外释放引用及执行跨对象工作。CLOSED 下 Complete/Deferred/Lost 均摘来源槽，不依赖观察者消费事件来断开来源自环。

`os/wait_context` 仲裁 Installing/Armed/Finishing/Done 与 outcome。WaitIdentity/WeakWaitIdentity 捕获不可回绕 epoch，lifecycle、source、timer、内存完成和请求取消都使用捕获身份。持久完成在 Done 前冻结 `(epoch,outcome)`，目标缓存按代次发布，旧回调不读取重置后的隐式当前 outcome。

每 hart `TimerQueue` 使用 arena、索引最小堆和包含 owner/slot/generation 的稳定 token。单字 TimeoutRegistration 在 Unregistered/Token/Closed 仲裁退休责任；对象完成/错误/终止/到期只有赢家注销 token。跨 hart 删除在 owner queue 锁下进行，锁外释放 Context，owner 在下次装填点更新 timer。

## WaitSet 与内核退休

WaitSet ABI 只有 Create/Register/Rearm/Receive/Remove 和 READABLE/CLOSED。one-shot 消费后 Rearm，在源锁内重查电平；正常 rearm 不重新分配完成存储。Remove 立即使 token/未消费 ready 失效，物理退休由内核继续。

Create 预付唯一 retirement actor、普通 Close 回复与资源；Register 预付来源订阅、ready 和可复用 finish。Register/Rearm 登记 operations，跨来源工作锁外返回；actor 仅在 operations=0、cycle Done、source_id=0 时删除 registration。

普通非空 Close 按 HandleTable → WaitSet → Lifecycle 提交 CLOSED/mandatory，摘 owner 后锁外发布 actor。成功回复表示自身退休完成；调用者终止或未安装 plan 只取消回复，不撤销 actor。CLOSED 的观察与私有 completion 分离，自观察/交叉观察不形成清理等待环。

`task/retirement.rs` 的预付 WorkDebt 强根独占对象推进。progress 和 completion 是独立 dependency：actor 监听前者，ProcessDrain/unpublished 监听后者。Blocked 不轮询、不计 runnable，早到 WakeToken 锁存；槽/Pending 更新在同一队列锁中，backend/source/最后引用交接在锁外。

## ProcessDrain 继续

Process 出生预付 reusable drain_waiter。`request.rs::DrainRequest` 捕获 Process/Control、输出、一次截断预算、累计 work 和 affine drain_active，暂停不重新解析 Handle、不重置预算。More 必须有正工作；内部零工作 Blocked 停驻同一请求。

finish 首轮/恢复共用 StepResult。依赖按 context+epoch 登记/取消；新轮已停驻时旧取消仍拒绝。队列归还容量/Pending、请求/依赖退役后才 Done，锁外交付 admitted thread 或 Departure。间接结果写回失败冻结 caller Fault/StoreAccess；普通 WaitMany 仅观察，输出失败返回 MemoryNotAccessible。

Job 摘除前转交预付 Finalization 强根。后台与 Native 共用 drain_gate/游标；Native active 时后台停驻，DrainRequest Drop 先清 active 再通知。Caller/Control 消散不丢终段传播，完成后才释放根。

## 预算与准入

notification/finish/retirement 共用控制安全点 16 步，对进入时已有 runnable 的后类保留最低进度。运行中才出现的类别不保证同轮完成；下一安全点按实际 pending 参与预算。deferred memory/unpublished/termination/finalization 另共用独立 16 步，不合称全局单一 16。

20 类 admission 分开对象、消息交付、注册与等待责任；真实 heap 分配仍 fallible。finish 物理槽分区 Thread 8192/Kernel 8320/Persistent 8192，注册不能借用清理预留。固定槽与计数分别验证，不把计数等同驻留字节或跨表并发库存。

## 验证证据

- `wait_set/selftest.rs`：旧回调、Closed seen/迟到安装、通知/退休先行历史、反位序多 interest、终态 Deferred、operations 门、来源 reset × Remove/Close、两种三类压力和实际固定槽退款。
- `retirement/selftest.rs`：Taken/业务完成/槽交回/新责任的排列，未安装 Close 取消、ticket 早晚 wake、actor 最后根、actor/Kernel finish/Object 构造耗尽退款。
- `task/selftest.rs` 与 `wait_set/selftest/continuation.rs`：真实空间/域准入/等待安装/终止债务；Native parked 2/8 后恢复恰剩余 6；兄弟撤输出页冻结 StoreAccess；复用新轮也 parked，旧 epoch 不取消新依赖；所有 Pool/metadata/control/deferred/actor 退款。Ready 前夹具不执行用户指令或证明远端 shootdown。
- `mailbox/selftest.rs`：预留 Busy/精确电平、真实撤页后 partial header 回滚、旧编号跨新槽仍 Stale、完整 payload/业务及 Delivery KOID 保持、Lifetime 实装通知、full/closed-owner 失败与退款。
- `srv_init/public_ipc.rs`：真实双用户接收线程，确定性 FIFO 交接及 forced Full 后自由竞争，64 条 4096-byte payload/独立授权，不丢不重、单接收者顺序、once/Delivery/Lifetime 收束；非空转换 Drop；两跨进程 CLOSED 提交后 kill，两个目标精确终因和 Pool charge 退款。
- 外部只读 GDB `artifacts/check/public-ipc-exit-gdb.log`：生产 Kill(0x131) 前 active 非零、三成员、mandatory=1；随后进入已安装 Waiting 取消，epoch=1、reusable=false/KernelResult0（Close 回复）。另 hart 用户 PC 与 actor 同时存在；只证明本次窗口，不把所有用户运行都标为 exact-window。ELF/SHA256 保留于同目录。
- 最终日志 `artifacts/check/public-ipc-final-{core,sifive,release,nofd,boot-failure,clippy,host,shared-host}.log`：正常 core/平台/release/nofd、三种 Failed 全 hart 停驻、七面 lint、140+23 host 测试通过。普通路线 required anchors 包含全部新增公共自检与用户组合。

完整 stress 的先前 300s Tunnel 墙钟敏感截断与概率 15/16 判定缺口已完成首轮验收收口：确定性终因覆盖、运行身份和阶段观测已接入；历史现场与未来重开条件见 [`验收时间敏感归档`](../../plans/archived/ref-2026-09-acceptance-timing-flake.md)。本前置不把历史偶发现象当作当前 correctness 缺陷；若未来复现归档触发条件，必须重新立案并保留完整身份与进度证据。本前置纳入 `task/fal-service-capabilities` 的混合集成基线，提交定位见 FAL 总计划交接节；不完成时间/执行/FAL。

Tunnel/Endpoint/backing 的机制见 [tunnel.md](tunnel.md) 与 [mm.md](mm.md)，Runnel 数据布局见 [runnel.md](runnel.md)，ProcessDrain 业务游标与 Job 生命周期见 [task.md](task.md)。
