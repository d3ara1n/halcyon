# FAL 服务能力与公共 IPC 前置

> 状态：公共对象/观察/退休与公共时间已完成并归档；执行基座尚未完成，FAL 业务继续暂停，整体未交付。先完成执行前置再恢复业务；已有代码和先前“冻结”的记录不是免审视依据。用户允许为通用机制清理 ABI，内核与 rinlib 同步迁移。
>
> 方向参考：`notes/ideas/{object,message,wait,time,rpc,framework,fal,fs,service,tunnel,runnel}.md`。本文件拥有 FAL 业务与总体依赖/交付导航；公共对象/观察/退休由 [公共前置计划](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md) 拥有，时钟/绝对期限由 [期限计划](archived/todo-2026-09-monotonic-time-rpc-deadline.md) 拥有，运输/RPC/服务执行由 [执行前置计划](todo-2026-09-13-service-runtime-prerequisites.md) 拥有。计划审视由实施者负责，代码 reviewer 只审查代码；提交后登记未来代码 Review。

## 开发分支与交接

集成基线提交：`d22b9d71ef810145bf4d5bfb3673ffec8640f361`（`chore(fal): 保存公共对象收口后的集成开发基线`），共 131 个文件。它包含公共对象前置交付与其余草稿，不是最终 FAL 合并提交。固定该提交的未来代码复核见 [集成基线 Review](todo-2026-09-13-fal-integration-baseline-review.md)；继续施工从本分支最新 HEAD 接手，不退回基线覆盖后续改动。

- 开发分支：`task/fal-service-capabilities`，由本地 `master` 的 `5d406a4` 分出；该基点包含原有四个未推送提交。当前基线是公共对象交付与时间/执行/FAL 草稿的集成快照，不声称仅包含公共对象前置，也不声称 FAL 已交付。
- 交接入口：先读 `plans/COMPASS.md`、本节和对应专题计划；实现现状看 `notes/impls/`，目标契约看 `notes/ideas/`。会话内任务编号只作临时导航，不能写入项目语义，计划文件是跨会话真值。
- 开发方式：本次混合快照不按文件硬拆。后续按运输、RPC/Outbox、服务执行等机制闭包逐项提交；每项包含真实调用者、失败/取消/退休、旧路径删除和验证，不以 passing fragment 标完成。总体完成并经授权后合并回 `master`；提交不包含 push 或合并授权。

| 专题 | 交接状态 | 接手入口与剩余责任 |
|---|---|---|
| 公共对象、观察与退休前置 | 完成，已归档 | `notes/impls/ipc.md` 与公共前置档案；保持内核拥有退休、捕获 epoch/预算、来源锁外交接及准入退款，不恢复 Seal/Drain |
| 公共时间与绝对期限前置 | 已完成并归档 | `archived/todo-2026-09-monotonic-time-rpc-deadline.md` 与 `notes/impls/time.md`；完整期限与运行期协作停止已接通，跨硬件 epoch 连续时间按唯一延后项保留 |
| 运输/执行前置 | 公共对象与时间已完成，工作量大，草稿未交付 | `todo-2026-09-13-service-runtime-prerequisites.md`；按 typed transport、RPC context、Outbox/Runtime、真实消费者迁移、组合收口五个机制闭包推进，完成 Packet/Delivery→任务/RPC/Outbox→terminal→retire→refund 后才恢复 FAL |
| FAL 业务 | 暂停，整体未交付 | 本计划；store/backend/grant/protocol 等均须重审，`srv_fs` 仍有 v1 MemFs/同进程泵，不用该路径补偿尚未完成的执行基座 |
| 验收可靠性改进 | 用户独立延期 | `todo-2026-09-13-acceptance-reliability.md` 和 KNOWN_ISSUES；概率覆盖误失败与原 Tunnel 静默截断分别处理，未绿 stress 不记通过 |
| workspace 包归属 | 独立延期 | `todo-2026-09-13-workspace-package-ownership.md`；本主线不搬目录/包或修改 path 依赖以整理归属 |

代码定位：时间为 `shared/src/time.rs`、`os/kernel/src/clock.rs`、sched/wait/mailbox 与 `user/rinlib/src/time.rs`；执行为 rinlib ipc、`librunnel`、`librpc/{caller,dispatcher,exchange}.rs`、`libsrv/{budget,work_queue,runtime,wake}.rs`。执行前置先完成 typed transport，再接 RPC context、Outbox/Runtime 和真实消费者；每阶段保持失败/取消/退休/退款闭包，不按旧盘点直接续写。

当前验证基线：七面 `just clippy`、140+23 host、virt core/release、128MiB sifive_u、virt-nofd、panic/alloc/fatal 三类 boot-failure 通过；GDB 只读捕获已安装 Close/active 的一次真实取消窗口，不保证每轮 exact-window。完整 stress 的旧截断/概率失败仍未解决；本快照没有总体 `just acceptance` 通过声明。

完整日志与诊断 ELF/SHA256 位于本机 `artifacts/check/public-ipc-final-*`、`public-ipc-exit-*` 及 boot-failure/lint 目录，均被 Git 忽略，不随 clone 交付。异机接手先 `just check`、`just clippy`，显式 host target 的算法/shared 单测，再按变更风险运行 `just virt`、`just virt-release`、`just sifive_u`、`just virt-nofd`、`just virt-boot-failure`；若需复现诊断，用档案指明的生产源码断点重新取证，不依赖旧二进制地址。总体 stress/acceptance 在收尾如实判定，不能用重跑直到绿替代验收可靠性计划。

## 开工审视与任务依赖

本次任务划分失败的根因是把公共前置和正式服务能力混入一项过大的施工任务，并将文档确认误当成前置已成立。以后开工先反推正常、失败、取消、退出和退款路径，审视前置是否齐备、任务是否按机制闭合；自顶向下设计、自底向上完成完整前置。已有草稿不构成保留理由。

```text
公共时间/Deadline ──────────────┐
公共对象/通知/WaitSet/内核退休 ──┴─> 运输/Runnel/RPC/Outbox/libsrv
                                       └─> 授权域/稳定后端/v2客户端
                                             └─> Open/Watch/注册/Move/Copy
                                                   └─> 真实装配/旧路径删除/总体交付
```

时间与公共对象任务共同核对绝对 Send/Wait ABI，执行任务明确依赖二者；来源通知、完成、普通 Close 和 ProcessDrain 强耦合，共同放在公共对象任务内，不拆散。完整前置允许且必须适当验证；前置完成不是 FAL 完成，分片验证也不能代替任务闭包。workspace 包归属另见 [未来整理计划](todo-2026-09-13-workspace-package-ownership.md)，本轮不搬迁。

本文下面保留的基线与代码连接点是审视材料，不是已完成证据。公共前置章节的旧 Seal/Drain 等候选已经被普通 Close/内核退休替代，旧 ABI 和用户维护编排已删除，不继续照旧施工或恢复兼容。

## 1. 基线、交付范围与自然序

调查基线 `5d406a4`：多页 Tunnel/RNL2 已实现，`606b59d` 完成库存来源与正式自检。当前代码仍是：entry 自带 badge、Mailbox owner/sender 共用一个对象、Receive 后无内核交付 owner、WaitMany 相对毫秒且最多 64 项；`libsrv` 为空壳，`srv_fs` 用同进程客户端泵，无正式 grant、Delegate、Open、Watch、服务发现。

整体目标是以通用对象寿命、消息交付、持久观察和绝对期限支撑正式服务，再一次接通 FAL。交付包含：

- Mailbox 队列与发送授权分离、Lifetime、Delivery、HandleQuery；
- 持久 WaitSet 及有界收束，复用现有对象信号与通知债务；
- 期限计划拥有的 MonotonicNow、绝对 Send/Wait 与完整 RPC deadline；
- typed 运输 owner、异步 RPC、libsrv 执行/准入和 Runnel 安全观察；
- 稳定节点、DirectoryGrant、真实跨 provider 路由；
- Record/Handle 属性、注册/发现、正式 Open、Watch、同域 Move 与普通 Copy；
- 独立 provider/client 进程、全部失败路径、旧机制删除及整体组合验证。

不实现 CPU 预约、KernelMemoryBudget 公共 ABI、设备/中断/DMA、BufferQueue、系统关机政策或通用异步语言运行时。FAL 超出基本操作面的能力唯一承接见 [`扩展操作计划`](todo-2026-09-fal-extended-operations.md)。

自然序：公共时间与 IPC 前置 → 服务执行/授权对象 → FAL 正式消费者 → BufferQueue 与设备/中断/DMA → 异构。最终全局架构 Review 等本专题主要消费者完成。

## 当前施工位置

公共对象、观察与退休前置已完成：独立发送授权/Delivery/Lifetime、WaitSet 普通内核退休、捕获 Native 继续/轮次取消、最后拥有根和真实消费者共同验证，旧维护 ABI 删除；新增真实线程接收、跨进程提交后 kill 与 GDB Waiting/active 窗口。最终 core/sifive/release/nofd、启动失败、七面 lint 和 163 项 host 测试通过，纳入当前 FAL 分支混合集成基线。完整 stress 的截断/概率判定仍由验收可靠性计划独立延期，不宣称通过或已修复。自然序转 运输/RPC/服务执行前置 → FAL；具体证据见公共前置档案与 notes/impls/ipc.md。下列条目是旧施工盘点，不是当前实现或交付保证，执行/业务恢复时须按实际代码重审：

- `shared/src/time.rs` 已写入 Deadline、ClockSnapshot、ClockGeometry 和换算测试；`kernel/clock.rs` 接入单一时钟来源，调度量子与启动期限不再截断频率。
- shared/kernel/rinlib 的绝对 WaitMany/Sleep/Send 已开始纵向迁移；同步 Caller 与异步 Dispatcher 已写入同一 deadline、Unsent/Sent 错误阶段和未发送 Request/能力归还，尚未形成编译与组合证据。
- entry badge 已迁至独立 MailboxSender；Lifetime、Delivery、HandleQuery、ReceiveResult 和对应 syscall/封装已写入。Capability/HandleSet、ReceivedMessage、ReceiveBuffer/MessageStorage、Packet 和 RequestContext 已承担运输 owner；原始移动 Send 已改为显式 unsafe，既有验收消费端开始清除重复关闭，正式 srv_fs 旧泵仍待整体替换。
- ObserverSink 已将 WaitContext 与 WaitSet arm 接入同一来源/完成债务。WaitSet 的注册、ready、rearm、remove、seal/drain 和 ProcessDrain 接管代码已写入，组合竞态与预算证据尚未补齐。
- 通知入队不再同步访问 registry，用户 trap 尾部开始有界推进；剩余 pending 的完整安全点/门铃验证属于未完成责任。
- Tunnel PEER_ATTACHED 与 Runnel 的安全 WaitSet 注册/等待准备已开始接线；AttachFailure 的真实 typed owner、清理及两类角色的统一驱动尚未完成。
- libsrv 的 Budget/Account/Charge 共用 metadata_admission，任务与输入缓冲先准入；WorkQueue 持有稳定任务记录、公平 ready FIFO 和预付期限槽，Runtime 轮转期限/输入/任务并保留 max_work=1 的轮转位置。Seal 后分步停止任务、真实退休后清空；外部 WaitSet 最后收束。正式任务与 FAL 状态机尚未接入，不能将公共框架源码存在视为完成。

施工中识别的预付缺口已进入最终结构：WaitSet 注册持久保留来源订阅与 finish debt，消费后在来源锁内重置静止 arm，并用不回绕代次隔离旧记录；正常 Rearm 不分配。任务期限也不能在业务提交后重新申请完成槽，TimerQueue 已增加保留 token/generation 的 park/reschedule，WorkQueue 每任务预付一个期限槽，停用不占活动堆、恢复不分配；时间机制的具体实施仍由期限计划拥有。这些代码都待整体竞态、预算及组合复核，不使用轮询/额外重试 adapter 掩盖缺口。

- `libfal/store.rs` 已写入 NodeId/NodeRef、PreparedNode、NodeStore 和 RetireContext。目录链接与 pin 分账；准备期独占 store 容量与账户额度；未入表节点取消不排入退休链；最后链接/pin 消散只排队，具体 payload 逐步退休，失败保留真实记录。其正式目录、属性和流 payload 尚待接通。
- `libfal/authority.rs` 已写入十项 FalRights 与不可由外部构造的 AccessSnapshot；`grant.rs` 已写入授权域 GrantTable、发行政策和 Lifetime 观察。表绑定实际 Mailbox identity，只持 observer/root/account，不持 sender 母本；两个登记节点先预付、外部注册分配 token 后复用预付节点 key，再交付 sender。发送能力自身运输 rights 与属性出口 ceiling 分开。Provider 必须按实际 related Mailbox identity 选择授权域，并按同一 NodeStore 判断 Move 的事务域；不能把“不同入口域”直接当成 CrossDevice。正式 provider/domain 装配仍未实现。
- OrderedTable 已支持有序名字键、借用查询/游标/删除及准备后确定 key，整数键继续走同一个 AVL 实现。名字事务不得退回不能 fallible reserve 的 BTreeMap 路径。

- `backend.rs` 已写入目录/属性/流/链接的非递归 payload、准备/最终校验/无分配 Commit，以及 Create/Delete/属性替换/定位写/Move；目录循环检查按预算追踪祖先并冻结结构代次。Create/属性准备失败返还输入 owner，旧属性由退休任务承接；正式 provider 仍未调用这些接口。
- `value.rs` 已写入正式长度化属性编码、Array/Record、非递归总预算验证、唯一完整槽引用、实际 capability 描述与出口政策验证、canonical slot 重写和存储 owner。Directory 出口包含 FAL ceiling，必须由正式导出任务向目标 provider Derive，不能直接 duplicate 母本。旧 property/memfs/provider v1 路径仍待整体删除。
- `data.rs` 已写入稀疏分块数据、预付写块与无分配替换、逐块退休。正常定位写范围受 payload 上限约束，Open 数据任务以有界块推进。
- `protocol.rs` 已写入 v2 操作、稳定预期位置与完整 Header Deadline；本地根 Derive 与普通解析区分，避免跨 provider 路由再递归。v2 未接通真实生产者/消费者，不表示协议已交付。
- NodeStore 的退休发布已发现必须显式唤醒空闲 actor，`libsrv/wake.rs` 和 store 构造依赖已写入预先配置的 Notification 唤醒；正式域必须先注册该来源再公开 grant，最后才关闭 Notification owner，禁止等待下一次业务请求偶然退休。
- `rinlib/ipc/invitation.rs` 与 EndpointCleanup 已写入邀请未消费/已消费失败 owner；原始 Tunnel/Runnel Attach 已改为 unsafe，五处既有调用点已标明原始责任。Runnel Channel/ProducerCore/ConsumerCore 构造失败以 InitFailure 原样返还 Transport，安全 Producer/Consumer create/attach 接入 typed Invitation，协议初始化失败返还 Endpoint，不自动丢弃承载。pm 接收侧已迁入 typed Invitation/Producer::attach，init 创建侧与正式 Open 仍待迁入安全工厂；host 用例只已写入，未运行。
- 代码 reviewer 的静态追踪发现 OrderedTable 内部 scan cursor 仍为 u64，已统一为 K；未运行构建或测试。其他 IPC 并发观察不构成运行安全性证据。

下一连接点：先完成公共对象/通知/内核退休与时间前置，再完成运输/Runnel/RPC/Outbox/libsrv 执行前置；二者未成立前不继续 provider 或后端业务扩展。之后重新核对 FAL 草稿、接通正式业务与独立进程，最后执行总体组合门。公共前置不能再随业务施工反复补修。

实现细节收口：WaitSet 的 ready 双向链接存在正式 OrderedTable 注册记录中；入队/摘除使用固定数量的有界 AVL 查找，避免新增 unsafe intrusive 指针或无界 tombstone 扫描。收束政策上限统一从 shared `DRAIN_WORK_MAX` 取得，不能用用户传入的巨大预算把短内核路径放大成全表操作。

## 总体推进视图

这不是一次“给文件库加几个操作”的改动，而是一次正式用户态服务栈交付。工作量来自三条同时需要闭合的链：授权链（sender/Lifetime → grant → 稳定节点 → 出口衰减）、执行链（WaitSet → 公平任务 → 下游 RPC/背压 → 业务完成）、责任链（Packet/Delivery → 请求/outbox/流 → terminal → retire → 精确退款）。缺少任一条，演示路径可能工作，但正式服务契约不成立。

总体目标保留，设计与任务边界按证据持续修正。下面五段是导航；前两段由独立前置计划安排并包含其闭合完成门，后三段在前置完成后恢复。A–G 旧落点只作为代码盘点参考，不构成继续按文件分片的任务划分。

| 施工段 | 直接形成的最终结构 | 后续连接与段末检查对象 |
|---|---|---|
| 1 公共资源基座 | 内核身份/Delivery/WaitSet/时间，rinlib 运输与映射 owner，Runnel 构造失败 owner | 正式服务能无分配地恢复观察、按阶段归还资源；全部 raw 消费入口标明责任，不能让安全 API 消费另一 owner 的借用值。 |
| 2 服务任务运行体 | libsrv 任务、账户、显式债务唤醒、RPC dispatcher、Outbox 与有界 retire | 一个运行体实际驱动请求、下游调用和退役；所有完成/取消路径保留 Delivery，源码存在不等于已接通。 |
| 3 授权目录与值 | NodeStore/PreparedMutation、GrantTable、Namespace/走路、Record/Handle/Take、跨 provider Derive | 替换旧路径模型和无鉴权 anchor；稳定位置、真实权限衰减与运输槽布局同步迁移。 |
| 4 长生命周期业务 | Open offer/Attach/Start/EOF/Finish、Watch、注册/发现、同域 Move 与客户端 Copy | 全部 terminal/retire、部分进度、取消、静默退出与旧实例竞态闭合；不在 handler 中等待。 |
| 5 真实装配与整体收口 | 两个 provider、独立 test_fal、init 启动能力图、全消费者迁移与旧路径删除 | 回到全部契约复核，集中执行第 14 节 host/静态/QEMU 组合门，再同步 impls；满足总体完成门之后才交付。 |

当前业务暂停，公共对象/观察/退休任务重审后施工，时间前置同步核对。此前第 2–3 段源码是超前草稿；执行任务不得依赖未完成的内核基座，FAL 不得依赖未完成的执行基座。每轮记录实际责任链、前置证据、剩余连接和删除门；发现新缺失前置先修订任务图，不立即切回业务打补丁。

## 2. 公共对象类型与所有权参考

```text
HandleTable Entry = object + role + rights

MailboxOwner ──> Mailbox(queue, receive reservation, signals)
Sender / SenderOnce ──> MailboxSender(identity, badge, queue, LifetimeOwner)
                               |
                               └── strong queue reference
LifetimeObserver ──> LifetimeState(observed identity, CLOSED, subscriptions)
                         ^
                         └── LifetimeOwner（仅内核，observer 不反向持 Sender）

queued Message ──> Delivery ──> MailboxSender
Receive ──原子移交──> Delivery Handle ──> 用户 RequestContext

WaitSetOwner ──> WaitSet ──> Registrations ──> validated source references
                               └── 一个预付 ready slot / arm completion
```

### 2.1 Mailbox 与 Lifetime

- `Mailbox` 只拥有队列/接收/容量电平，receiver-owner 唯一且不可 TRANSIT。
- `MailboxSender` 是独立 KernelObject，拥有不可变 badge、自己的 koid、目标 Mailbox 强引用和一个 affine `LifetimeOwner`。SenderOnce 与普通 sender 引用同一对象，只变 role。
- `LifetimeState` 是可等待 KernelObject，只拥有观察状态、被观察对象 koid 和订阅，不持 Sender/业务对象强引用。公开 observer 可 WAIT、DUPLICATE、TRANSIT、GRANT，不提供提前终止权。
- `LifetimeOwner` 是通用内核内部基元，最终释放只发布一次 CLOSED。不暴露用户态“寿命 owner”，不引入外部续租或原进程保活依赖。
- 使用对象强引用本身保持 sender、运输、临时 syscall 使用与 Delivery 的连续所有权；最终析构发布 Lifetime。不读 `Arc::strong_count`，不在 HandleTable 外另算一份 sender 数。
- `Entry` 删除 badge 字段及 `entry_with_badge`；badge 的唯一真值迁到 MailboxSender。所有非 Mailbox 对象不增加无意义的授权计数或标签。
- MailboxCreate 只交付 owner；MailboxMintSender 原子交付 `{sender, lifetime_observer}`。通用便利工厂可以组合这两个调用，失败关闭尚未发布的 owner，不保留内核默认 badge-0 sender 路径。原先只请求 READ|WAIT 的 ReplyPort 创建者需显式取得 MANAGE 完成 Mint；只接收不铸造的服务可以由 launcher 预先 Mint，再通过 GRANT 收窄其 owner。
- 重新 Mint 是新对象，即使 badge 相同也有新 koid；Duplicate/MakeSendOnce 不新建寿命实例。服务登记以发送授权 koid 为键，badge 作为不可变标签。
- 队列 CLOSED 与发送授权 Lifetime CLOSED 不等同。sender 的可写/队列关闭等待绑定实际 Mailbox 电平源；本次等待另保留原 sender 使用引用。按真实电平源合并订阅，不复制 WRITABLE 状态。
- provider 政策撤销在用户态拒绝新的授权准入；已准入 RequestContext、独立派生 grant 和已建立流按各自契约收束。普通 close 不实现递归 revoke。

### 2.2 Delivery 与 Receive

- 每条成功 Send 有一个独立 `Delivery` KernelObject，强持被调用 MailboxSender。Delivery role affine，可 TRANSIT/GRANT，不可 DUPLICATE、不可 Send、不可 WAIT；关闭仅释放一个交付责任。
- Message 在排队和接收预留中拥有 Delivery；Receive 原子将其安装为接收方 Handle。业务 Handle 上限仍为 8，Delivery 额外占一项真实表槽，不占业务槽编号。
- 接收 ABI 使用结构化请求/结果：`ReceiveResult` 包含 MessageHeader 与本地 Delivery Handle；Peek 只返回 MessageHeader，不能伪造一个尚未安装的 Delivery Handle。业务 `handle_count` 不包含 Delivery。
- MessageHeader 增加 `sender_context_id`，等于被调用 MailboxSender 的 koid；保留内核填入的 sender_pid、sender_badge。发送方没有填写这三项的入口。
- Send 在发布前完成 Delivery/队列/运输存储预留；成功入队与 moves/send-once 消费原子化。投递期限的检查位置由期限计划定义。
- Receive 预留 `业务 handles + 1`，完整写回后一次提交；输出失败恢复同一个 Message/Delivery，owner 同时关闭则统一清理，不复制责任。
- `OwnedMessage`/`RequestContext` 保留 Delivery，直到处理和回复/拒绝责任终结。即使请求派生异步任务或 outbox，移动整个上下文，不留下裸 sender_context_id 与已释放的寿命。
- 消息中的 transit 集合统一由 `TransitEntries` 叶收束 owner 管理，覆盖 Discard、owner close、回滚与异常路径。不能只 drop `Vec<Entry>` 而漏掉 Invitation 等 close callback。
- 不把所有 Entry 的 Drop 泛化成 close：容器 owner、ProcessBuilder、映射 owner 仍走其既有显式协议。删除无生产调用且会静默丢 Pinned entry 的 `HandleTable::drain/into_entries` 辅助路径，测试改走正式事务/有界摘除。

### 2.3 HandleQuery

新增只读查询，输入必须是调用者真实持有的 Handle；不要求新管理权，不允许以对象 ID 打开对象。固定宽结果包含 `object_id, related_object_id, kind, role, rights, badge` 及零 reserved。不暴露内核地址，不在此绕过 WAIT 提供任意动态电平读取。

- MailboxSender 的 object_id 是发送授权身份，related_object_id 是目标 Mailbox；
- Lifetime 的 related_object_id 是被观察对象；
- Delivery 的 related_object_id 是被调用发送授权；
- 不适用的关联身份/标签置零。

复用 `ObjectHeader` 和 `monotonic_id` 的单一 koid 来源，不建立第二个全局对象注册表。公开 kind/role 使用固定 wire 判别，不直接拷贝 Rust enum 内存布局。查询持有期间防止 entry 被并发移走；输出失败无副作用。

## 3. WaitSet：公共持久观察

### 3.1 类型与接口

`WaitSet` 为唯一 owner 的可增长容器。公开接口：Create、Register、Rearm、Receive、Remove、Seal、Drain；最终通过 HandleClose 关闭空集合。owner rights 为 READ/WAIT/MANAGE/GRANT 的相应子集，不可 DUPLICATE/TRANSIT。

`Registration` 包含：不复用 token、cookie、interest、原已验证使用引用、真实信号源、源订阅 token、arm 代次、仲裁状态和一个预付 ready slot。状态为 Installing → Armed → Queued → Disarmed；Remove 进入 Removing → Dead；Rearm 产生新 arm 代次。

- 每轮最多一条 ready 记录；第一次获选的完整 observed 快照冻结，后续变化不拼成另一快照。
- Receive 事务交付固定上界批次，失败不消费；成功后该轮 Disarmed。
- 每个 registration 持久占有来源订阅和预付完成槽，Armed/Queued/Disarmed 不撤销再安装；正常 Rearm 不分配、不重新申请通知/完成债务。只在 Remove/Seal/退役时注销来源。
- Rearm 在源锁内重置已经结束的 arm、更新 source epoch 基线并观察当前电平，已为真时立即进入完成路径。ready 记录带 arm_generation，返回新代次用于过滤旧记录；溢出显式拒绝，不能回绕。
- Remove 使 token 不再进入可接收队列，清理已排队记录及在途完成责任；用户已经取得的旧记录须按 token 有效性过滤。
- ready 队列以 registration 内联链接实现唯一入队；摘除只做固定数量的有界注册表查找，不留大量 tombstone 让一次 Receive 无界扫描。
- 每个 Register 只处理一个来源；规模由 metadata/队列预算准入，不能把 WaitMany 的 64 项变成整个服务的连接上限。

### 3.2 与既有等待机制合并

把来源订阅的完成目标抽象为 `ObserverSink`：一次性 WaitContext 或 WaitSet registration 的一个 arm 周期。两者复用 `ObjectWaitState` 的电平、代次、发布快照、预付通知槽和 `notify_work` 排水。

源锁内只进行有界快照与原子 offer，不锁住另一个 WaitSet、不执行用户回调。完成引用在源锁外转交已预付的 finish 路径；周期完成后先归还该 registration 的 finish 槽，再发布 ready，使立即消费/rearm 的另一 hart 也能复用该责任。持久完成不注销来源订阅，来源锁内的 Lost/Complete 分支均保留持久订阅，普通线程等待仍按原语义移除。安装、提前命中、取消、Rearm、输出失败竞争同一轮状态，迟到完成不能命中新 arm。

通知发布统一拆成两个阶段：锁内/析构路径只把预付 debt 放入当前 hart 队列并设置 pending，不执行 IPI 或获取 registry；用户 trap 统一尾部、调度安全点及入 idle 前，在全部业务锁释放后有界推进通知，并为剩余 pending 发布门铃。现有 `notify_work::publish` 会经 `try_ipi_slots` 获取 rank 150 的 REGISTRY 锁，不能从高秩锁内的 Lifetime 析构直接调用。所有通知消费者迁到同一发布/安全点协议，不新增 Lifetime 专用队列，也不靠未来偶然 timer 唤醒。

WaitMany 保留单次 64 项的输入上限及最小 item_index 规则；它是有界的一次等待，与逐项增长的 WaitSet 共用来源机制。禁止新增一套独立电平实现、用户态轮流等待子集、每连接阻塞线程或计时轮询适配器。

### 3.3 收束与预算

WaitSet 为增长型容器，状态为 Active → Sealed → Draining → Done → Closed。Seal 停止 Register/Rearm 和新 ready 交付，发布 REAPABLE；全部退役完成发布 DONE；最终 owner close 才发布 CLOSED。

Drain 每个 work unit 推进一个真实注册/完成责任/存储块的收束，游标属于集合；并发 Drain 使用单一 gate 仲裁。未完成的源注销与 finish 责任仍在注册账本中。存储分块释放，最终 Drop 不再遍历全部空槽。

非空 HandleClose 返回 ObjectBusy 并保留 entry，不隐含先 seal 的部分副作用。rinlib 显式 close 使用 Seal/Drain/Close；预算耗尽或期限到达返还完整 owner 与阶段。异常 Drop 单次尝试并记录诊断，交 ProcessDrain 接管。

ProcessDrain 的 pending close 扩展为“原 entry 或该对象的 RetireState”，复用同一 WaitSet drain 内核；外层 max_work 约束真实内层工作，不能用一个外层 unit 隐藏全表注销。运行中和进程退出不建立两份清理算法，不增加内核线程。

## 4. 时间、期限与信号扩展连接点

时间具体契约、换算、回绕边界及施工由期限计划唯一拥有。这里冻结连接点：

- MonotonicNow 返回同一系统时间域的 u64 纳秒；Deadline 为显式 Infinite/At，不复用零表示无限；
- WaitMany 直接接受绝对期限；相对便利入口只在用户库转换一次；
- Send 在全部预留之后、入队线性化点核验同一绝对期限；过期不消费能力；
- RPC 的最终回复接受再次检查 Deadline；同步与异步共享投递阶段语义；
- libsrv 以用户态 deadline heap 和 WaitSet READABLE 的一次绝对等待组合定时，不增加服务专用内核 timer；
- Tunnel Endpoint 增加 PEER_ATTACHED：当且仅当对端状态为 Alive 时成立；Invited 不成立，peer close 清除并发布 PEER_CLOSED。这是 Connection 事实，不是 FAL Start 或 Runnel 验证结论。

信号位和 epoch 数量由一个规范列表派生，不能在 `SIGNAL_BITS`、KNOWN 和各数组长度中留下互不关联常数。

## 5. 用户态类型与执行框架

### 5.1 rinlib / librpc

- 叶能力、Invitation、Delivery、Lifetime observer 均有不可伪造 owner；消息结果先拥有全部资源，再通过 take 提取。构造 owner 只来自正式 syscall/Receive/StartupBlock 授权入口。
- 通用出站 packet 拥有 move 集合：成功投递消费，失败返还。不能在错误枚举中只返回错误码而让调用者猜 Handle 是否还在本地。
- AttachFailure 区分未消费 Invitation 与已消费后持有 Endpoint 的协议初始化失败；不得对可能已消费的 raw Handle 重复 close。
- Runnel Producer/Consumer 继续独占 Endpoint，提供非阻塞推进、等待准备、WaitSet registration 和 peer 状态查询；不导出 raw Handle、共享 slice 或可复制数据角色。
- Runnel 的阻塞门面与事件循环共用同一推进/acknowledge/重查逻辑。原错误后不可逆终态、已完成字节与清理 owner 规则保留。
- RPC PendingCall：Unsent(packet) → Sent(reply routing) → Completed；过期/失败按阶段返回 owner 或 OutcomeUnknown。send-once 请求 slot 0 使用 WRITE|WAIT|TRANSIT。
- 同步 Caller 私有端口，失败可整端废弃；多 in-flight dispatcher 共享端口，单个超时只移除对应 txid，迟到回复连资源一起丢弃。二者共享 framing、deadline 和运输状态，不共享错误的端口失效范围。
- 期待回复但 framing 违约时，只有在 txid 和 reply role 已可验证的情况下发送协议拒绝；否则释放输入，由调用期限结束对端等待。

### 5.2 libsrv

一个服务状态拥有者修改 GrantTable、NodeStore、RegistrationTable、SubscriptionTable 和 StreamTable。控制循环：收集有界 ready 批次 → 按稳定任务 token 分发 → 每任务有限工作 → 未完成任务排队尾 → 无立即工作时以最近绝对期限等待。

请求、下游 PendingCall、outbox、流和用户态 retire 都是正式任务状态，不在 handler 中同步等待。服务端不调用无限 send_blocking、write_all，也不持后端状态锁等待客户端/下游。未来设备 I/O 通过异步完成接回同一调度结构。

资源账户由启动/授权政策分配并随派生继承来源，不以 PID 生成 authority。预算覆盖节点及数据、grant、等待注册、请求、outbox、service record、Watch、Open offer 与活动流。达到预算在发布前拒绝；错误/退出精确退款。

第一版 Tunnel backing 由 provider 的 PoolBinding 支付，按授权账户预留连接配额；不假装已经支持逐连接使用客户端 MemoryPool。账户出资身份与当前持有进程/Job 正交。

独立授权域可使用独立 Mailbox，域内共享 badged sender。所有入口汇入 WaitSet 和公平任务循环；不能宣称一个共享 16 项 FIFO 已提供对恶意 sender 的强公平。

## 6. 稳定节点、授权和路径

### 6.1 数据与类型

- `NodeStore` 集中拥有稳定节点记录，目录项保存稳定 NodeId；`NodeRef` 保留被 grant/流引用的对象，不用字符串路径充当 root。
- GrantState = `{sender_context_id, root: NodeRef, rights, output_transport_rights, account, policy_state, lifetime_registration}`。存储 Lifetime observer，不存 sender 母本。
- `Namespace` 拥有 `prefix → DirectoryGrant`；替换/卸载返还 owner；解析持有自己的引用。
- `Position` 是稳定父 DirectoryGrant + 最终名字 + 可选 expected NodeId/version；Found 的元数据是快照，不是之后操作的授权证明。
- `RequestContext` 持 Delivery、reply_once、输入 owner、授权快照和操作状态；异步路径整体移动。
- 节点 pin、目录链接、打开流与账户 charge 分账。最后引用释放把节点加入用户态 retire 队列，有界释放，不在控制循环的 Drop 中递归遍历子树。
- 后端修改通过正式 `PreparedMutation` 拥有节点/目录项变更、capability 所有权差量、数据存储预留和输出预算。Validate/Reserve 可失败，Commit 不分配、不等待、不再失败；Create、属性替换、WriteAt、Move 共用这个事务边界，不各自维护回滚矩阵。
- 配额检查不能代替真实存储预留。采用 fallible allocation/预付槽和数据块；不能让用户触发的 Commit 调用没有 fallible reserve 的 BTreeMap 插入、无预算 Vec 扩容或递归复制，再以 allocator panic 处理正常资源不足。

### 6.2 权限与撤销

FAL rights 按 ideas/fal.md 的十项业务权利实现。检查入口、每个中间目录及最终对象，结果取 grant ceiling、节点能力和政策交集；不得沿用 memfs 的 X/R/W 属性作为唯一鉴权。

AcquireCapability 专门约束属性/记录中的 capability 出口，不重复取代 Traverse 所授权的受限目录派生、路由和 ReadStream/WriteStream 所授权的建流操作。FAL 操作可以明确铸造新对象，内核 DUPLICATE 仅约束同对象 entry 复制，不被描述成禁止一切用户态再委派。每个 GrantState 的输出运输 rights 由发行政策保存和收窄，不能根据客户端报送的数字放大。

DeriveGrant 创建独立 sender/Lifetime、稳定 root 与收窄 rights，先登记状态和观察再交付 sender；发布失败关闭 sender，Lifetime/Delivery 统一完成回收。父 grant 关闭不撤销 child。

政策撤销在线性化点停止该 context 的新准入；已准入上下文继续持授权快照。删除 registry 项后，旧 sender 的新请求返回 GrantRevoked，不需要保留无界 tombstone，因为 context koid 不复用。已打开流与订阅有自己的管理/取消入口，不把普通 grant close 偷换成强撤销。

### 6.3 解析与 Delegate

客户端顺序解释符号链接和 `..`，回退仅使用已持有的逻辑父帧；孤立 grant 不能越根。更具体的 namespace 路由按组件覆盖，路由前缀的逻辑父层只用于导航，不伪造远端父目录。绝对链接使用显式 namespace，未提供者返回明确边界错误。

每次 Delegate/Link 必须恰好覆盖请求且有合法推进；修复 `verify_cover` 的非等长接受和提前 normalize `link/..`。所有重试共享解析预算与连接 Deadline。

真实路由绑定持有目标 provider 的根 grant。A 对来访 grant 计算 `来访 ceiling ∩ 绑定政策 ∩ B 母本上限`，向 B 非阻塞 DeriveGrant 后才交给客户端。请求 B 的本地根派生不再次递归路由；反向/循环 Delegate 由客户端全调用预算拒绝。

创建/删除/移动使用稳定父目录和最终名字。Open 本地确认目标、鉴权、pin 和准入不可拆成信任客户端 Lookup 的两步。携带能力的写操作先解析到最终 provider，再运输能力；冲突返回明确未提交结果，普通超时不自动重试。

## 7. Wire、属性与其他操作

升级 FAL wire 版本，全部消费者同步迁移，不保留 v1 分支。内核 envelope 留 shared，FAL wire 留 libfal。具体 opcode 从共享的唯一枚举分配，不预留未实现业务。

请求业务 slot 0 是 send-once，额外输入从 1 起；回复从 slot 0 起独立编号。Delivery 不属于业务槽。所有 kind 明确准确 Handle 数、槽用途、允许的 kind/role/rights；全局验证未引用项、重复槽引用、错误 role、未知 flags、reserved、长度和嵌套预算。

| 能力 | 交付契约 |
|---|---|
| Record | 异构具名字段；一次读写覆盖全部元数据与 Handle；编码按总 payload/Handle 容量预算，不使用旧 VALUE_MAX 魔数兜底。 |
| Repeatable Handle 属性 | 存储完整 owner 和 ExportPolicy，每次读取 duplicate/衰减；要求 AcquireCapability。 |
| affine Take | PreparedTake 独占预留值，出站成功入箱后不可失败地置空；发送失败恢复值，预留期间并发写/取返回 Busy。 |
| Handle 属性 Write | 先校验与预留，再原子替换，最后释放旧值。未提交拒绝尽可能按明确响应槽返还新值；回复不可交付时由服务关闭，调用者收到 Sent/OutcomeUnknown。 |
| 同域 Move | 源 target grant 与收到的目标目录 sender 都通过真实 HandleQuery/context 登记验证；同存储事务域一次完成源 Remove、目标 Create、防目录循环、名称更新与事件代次。跨域 CrossDevice。 |
| Copy | 普通 Stream 经客户端 CreateExclusive/Open/传输/双方 Finish；不携带能力的数据属性整值复制。失败返回部分目标身份/进度，不 copy+delete，不默认覆盖已有目标。 |
| Delete | 删除最终目录项，不追踪链接 target；非空目录返回 NotEmpty。已经 pin 的节点不改身份。 |

Directory 类型 capability 的导出必须远端实际派生。其他协议的 endpoint 按发布者显式出口政策交付；没有通用业务衰减协议时不能根据 FAL rights 猜其 badge 权限。

协议错误至少区分 GrantRevoked、权限不足、CrossDevice、Conflict/StalePosition、CursorInvalid、资源/配额不足、Busy、Cancelled、Unsupported 与内部错误；运输 Timeout/ServiceClosed/OutcomeUnknown 留在调用错误层，不混成一个 FAL Internal。

## 8. 服务注册与发现

`libsrv` 拥有 ServiceRecord schema 和注册控制协议；`libfal` 只提供 Record 与可组合 provider interface。首个承载者可以是 srv_fs 的服务目录后端，不创建所有进程必须经过的全局注册权威。

状态：Absent → Starting → Ready → Draining → Absent。Register 创建独立 RegistrationControl sender/Lifetime，并捕获已预先限制权限的 endpoint；instance 使用该注册控制对象的不复用身份，记录/目录代次描述同一实例的状态变更。

- 注册上级 capability 限定名称/子树和资源账户；普通目录写权不能绕过状态机。
- Starting 不交付 endpoint，并有有限建立期限；PublishReady 一次发布完整 Record。
- Ready 的读取取得一个完整快照及其 capability；不逐字段读取。快照后 endpoint 可以关闭，发现不承诺即时可用。
- BeginDrain 撤出新发现；endpoint CLOSED 或 RegistrationControl Lifetime CLOSED 触发条件撤销。
- 所有撤销带 instance/generation，旧完成不能删除新实例；已授出的 sender 不因名称撤销失效。
- 记录关闭、导出中的副本、outbox 与观察都有明确 owner，挂起的导出任务持完整记录快照。
- 初始队列 owner、启动控制 sender/Lifetime 由 launcher 显式交付；普通启动控制不充当无鉴权目录 anchor。provider ready 后向 init 交付正式根 grant，再由 init 组装客户端 namespace。

## 9. Open 的完整状态机

### 9.1 接口与数据承诺

Open 请求声明方向 Read/Write、现有节点位置/预期身份、offset、范围约束、RNL2 版本、几何请求和连接 Deadline。回复包含实际几何、offer_deadline，业务槽 0 为 StreamControl sender、槽 1 为 affine Invitation。StreamControl 是独立 sender/Lifetime，不靠猜测 stream ID 授权。

客户端 connect：Open → Attach → 协议验证 → Start，全部消费原连接 Deadline。provider 验证 PEER_ATTACHED 且 offer 未到期才接受 Start；客户端自报附着不构成证据。Start 回复失败可能已进入 Active，按普通 Sent/OutcomeUnknown 和放弃路径收束。

Read 时 provider Producer；Write 时 provider Consumer。基本协议不含创建、append、truncate、双工、快照保证或持久化保证。范围上限 checked 验证，计数不回绕；实际复制每轮受资源/工作预算约束。

### 9.2 类型与阶段

`OpenStream` 持 NodeRef、账户 reservation、方向/范围、Runnel 单侧 owner、StreamControl Lifetime observer、等待注册、计数、终态及清理 owner。

| 状态 | owner 与推进 |
|---|---|
| Preparing | 已鉴权/pin，预留 stream/观察/outbox/内存；任何失败零发布并释放。 |
| Offered | Invitation 与控制 sender 待交付或已交付；保留有限 offer_deadline，禁止执行文件修改。 |
| Active | Start 已接受；以非阻塞任务推进，协商的空闲政策不因伪唤醒或无进展重置。 |
| Terminal | 固定状态与字节计数，服务可回答 Query/Finish；不能提前撤销对端仍需读取的映射关系。 |
| Retiring | 注销观察、关闭 Endpoint/未消费 Invitation、释放节点和额度；失败保留 owner，完整结束才删除账项。 |

Query 立即返回状态；Finish 允许一个有界的待回复请求等待稳定结果，后续查询不分配无限 waiter；Cancel 幂等地发起停止并返回已确定进度。相互竞争由单一服务状态拥有者线性化。

Read 在生产结束发布 EOF，保留端点；Finish 成功需已验证对端 tail 追上最终 head。客户端正常 EOF 在收到业务成功后才向上层返回。Write 的共享环发布进度不是后端进度；客户端发布 EOF 后等待 provider 消费、完成后端并返回 Finish。后端失败保留实际接受字节数，允许部分结果，不声称 rollback。

### 9.3 失败与退款

- 回复发送失败：关闭未交付的 sender/Invitation 和本地 Endpoint。
- 回复入箱但未接收：消息清理释放 Invitation/控制 sender，Lifetime 和 PEER_CLOSED 驱动同一退役。
- 长期未 Attach/Start：offer 到期；即使客户端仍活着也关闭创建端，拒绝迟到 Start。
- Attach 失败：typed failure 返还仍持资源，客户端释放控制权；已消费后的协议失败关闭本地 Endpoint。
- 数据端、控制权消散、取消、空闲政策到期：进入统一 terminal/retire，记录部分字节。
- provider 退出：客户端从 sender CLOSED 和 PEER_CLOSED 收束；最终结果未确认前不能报告成功。
- close 失败：保留真实 Endpoint/清理记录及配额，不通过删除 StreamTable 项伪造退款。重试/升级受服务政策约束，最终由监督者 Drain 进程兜底。

RNL2 EOF 不能承载后端错误；不修改共享 header 去塞文件状态，不把 PEER_CLOSED 当正常 EOF。Drop 是放弃，不是 Finish。

## 10. Watch

客户端持唯一 Notification owner，Subscribe 输入业务槽 1 为 SIGNAL|WAIT|TRANSIT signaler。服务在一次状态修改中鉴权、安装订阅并取 generation，回复 subscription_id/generation/effective_mask；订阅安装后的事件可早于回复并保持 pending。

SubscriptionState 持 NodeRef、授权账户、发起 context 身份及 signaler；Unsubscribe 验证所属 grant，不能靠猜 subscription_id 取消别人的订阅。客户端 Subscription 保留其 DirectoryGrant 使用引用。

create/delete/modify/rename 按位合并；另有 TERMINATED 位，原因通过查询。取消确认后不再 signal，但不伪造清空客户端已 pending 位。客户端同时等待服务 CLOSED；provider 通过 signaler CLOSED 发现 owner 消散，无需等下一次业务事件才清理。

范围只覆盖本 provider 的节点或目录直接成员。客户端采用先订阅后快照，枚举代次失配重读；不承诺可重放、递归或跨 provider Watch。

## 11. 锁阶、失败边界与清理规则

- 内核保持 HandleTable → Mailbox 的投递/接收事务顺序；用户输出校验/预留先于公开 entry，Receive 回滚保存完整 owner。
- MailboxSender 的 badge/目标/identity 不可变，不增加发送热路径对象锁。
- 新 Lifetime 状态锁位于既有 lifecycle 之后、work-debt 之前（命名秩 LIFETIME = 620）；它只发布自身终态，不取 Mailbox、HandleTable、WaitSet 或业务锁。
- WaitSet 状态锁使用命名秩 WAIT_SET = 550。来源锁与目标 WaitSet 锁绝不嵌套；Installing/offer/finish 协议跨越两段临界区。
- 所有 source offer 在来源锁内只做原子仲裁；ready 入队、源注销、关闭其他 owner 和析构在相应锁外推进。
- MEMORY_COMPLETION、WORK_DEBT、REMOTE_CALL、HEAP 等高于或等于 LIFETIME 的基础设施锁内不得析构可能最后释放 MailboxSender 的任务/引用；先取出任务，解锁再释放。不能靠升高 Lifetime 秩掩盖回调锁反转。
- Lifetime、Delivery、WaitSet registration 和 finish 责任都在公开前完成 metadata admission。发布信号及 Close/Drain 不再申请不可保证的清理存储。
- 用户态单一状态拥有者串行提交本地后端；存储/节点/grant 锁不跨 RPC、共享流等待或下游 I/O。后续并行后端必须维护这一边界。
- 失败分为未提交 owner 原样归还、已提交待完成、业务结果未知、终态待清理。不得把“回复失败”统一当业务回滚，也不得把“结果已完成”统一当全部资源已退休。

## 12. 施工图与连接点

任务按开工审视后的前置依赖推进，强耦合机制共同迁移并验证，不引入过渡 adapter 或双轨。完整前置具有自己的完成门，局部编译/测试或单个提交不构成 FAL 总体交付；下表是代码连接点盘点，不是独立验收分片。

| 分片 | 主要落点 | 必须接上的后续责任 |
|---|---|---|
| A 时间 | 期限计划；shared、sched/clock、rinlib | 绝对 Wait/Send、同步/异步 RPC、offer/服务政策，不能只做 Now 读数。 |
| B 身份/交付 | os/handle_table、kernel task/{object,handle,mailbox}，新增 lifetime/delivery；shared、rinlib | 全部 Mailbox 创建、duplicate、send-once、transit、ProcessGrant、Receive/Discard/Drain 迁移。 |
| C 持久观察 | kernel task/{wait,object,notify_work}，WaitSet；trap/sched 安全点、ProcessDrain、rinlib | WaitMany 共用来源机制、通用通知/完成发布、普通 Close 内核退休与 ProcessDrain/异常退出；旧 Seal/Drain ABI 删除。 |
| D 执行/协议基座 | librpc、libsrv、rinlib Tunnel、librunnel | 真正可组合等待、PEER_ATTACHED、outbox、账户、全部 owner/期限失败路径。 |
| E 授权/后端 | libfal、libfs | NodeStore、GrantState、namespace、Delegate、位置竞态与严格 wire。 |
| F 业务能力 | libsrv service backend、libfal/libfs | Record/Handle、注册/发现、Open、Watch、Move、Copy 及所有 terminal/retire。 |
| G 真实装配 | srv_init、srv_fs、test_fal、现有服务/驱动/验收消费者 | 启动能力图、独立进程、全 ABI 迁移、旧泵/anchor 删除、整体组合验证。 |

方向文档已经描述最终契约；实施中只把真实落地事实同步到 impls，不能把本表状态提前写成已实现。代码提交前按项目要求展示摘要并取得 commit 授权；本计划不包含 commit 或 push 授权。

## 13. 唯一残留/删除门

| 现状 | 目标与位置 | 删除触发与验证 |
|---|---|---|
| Entry.badge、entry_with_badge、默认 badge-0 sender | MailboxSender 单一身份；handle_table/kernel mailbox/rinlib | B 全调用者迁移，grep 无旧字段/默认根旁路，duplicate/transfer/rollback 保持 sender identity。 |
| Receive 后只剩整数 envelope | 独立 Delivery owner；kernel Message/shared/rinlib/librpc | B/D 请求、回复、拒绝、outbox、退出均持/移交 owner；无未归属 receipt。 |
| 手动 transit close、测试专用不安全 drain | TransitEntries 与正式有界表事务 | B 全失败路径退款，删除无生产调用的丢 Pinned 辅助接口。 |
| 相对内核等待、截断 ticks/ms、无限发送背压 | 期限计划唯一真值 | A/D/G 全部消费者迁移；旧内核路径删除，便利 wrapper 共用绝对核心。 |
| WaitMany-only 服务、无执行体 libsrv | WaitSet + 一个公平事件循环 | C/D/F 完整工作/退役，无固定 64 项服务上限或用户态轮询补偿。 |
| 裸 anchor/Position/同进程 fs 泵、无鉴权 memfs | 稳定 grant/节点及独立客户端 | E/G 删除 srv_fs 自泵与 slot-1 anchor，越权和竞态路径验证。 |
| Open/Move/Copy Unsupported、无 Record/Watch/发现 | 本计划声明的正式能力 | F/G 所有真实消费者和失败门完成；超出范围只由扩展操作计划承接。 |

若在这些路径发现新的前置缺口，先修订对应唯一计划和自然序，再继续；不得留下实现中的兼容字段或口头延期。上述旧路径是待删除的当前实现，不是授权引入新的过渡机制。

## 14. 整体验证与完成门

基线以上所有承诺接通后，集中完成验证。测试必须验证真实状态机和失败不变量，不以模型测试代替实际跨进程业务。

- host：身份/运输/回滚；来源发布快照；WaitMany/WaitSet 的安装、rearm、remove、关闭竞态与预算；完整期限；FAL codec、节点身份、权限、路由/链接、Record/Handle、Open/Watch 与部分结果。
- 确定性内核面：最后 sender/在途 Send/queued Delivery/已 Receive Delivery 的关闭顺序，接收写回失败、Discard、队列 owner 退出、跨表运输；Lifetime observer 本身不保活目标；额度恢复。
- WaitSet：超过 64 个真实来源，部分持续就绪不饿死其他任务，输出失败不消费、Remove 对迟到完成有效、非空 Close 保留 owner、max_work=1 Drain、源与集合两侧退出，最终库存退款。通知补证覆盖高秩锁内最后 sender 消散、纯 Resume syscall 返回、idle 前新发布和预算耗尽后的 pending 门铃，不依赖下一次偶然 timer。
- 时间：非整千 timebase、跨 hart 非倒退、checked overflow、raw 回绕边界、计算 deadline 后的抢占窗口、满箱直到到期、已入箱超时/迟到回复、下一调用成功。
- 业务拓扑：两个独立 srv_fs 实例（不同后端/路由域）及独立 test_fal；通过真实启动 grant 组装 namespace，不用客户端泵或共享同一 MemFs 实例。
- 授权：转交后原进程退出仍可用；最后引用与 Delivery 收束后退款；根逃逸、权限放大、伪造对象身份、错误 role、同 badge 不同 sender、Delegate 衰减、路径重命名竞态。
- 流：超过一页且多次跨环；Read/Write、零长度、背压、EOF/最终状态、部分写错误；不 Attach、不 Start、Start 前后关闭、投递失败、客户端/服务退出；一个阻塞流期间控制面与其他流继续推进。
- 注册/Watch：Ready 快照一致、旧实例清理不能删新实例、已授 sender 不因撤销名称失效；订阅先于快照、安装后回复前变化、独立消费者、取消和静默退出。
- 组合门：完整 host/静态检查、`just clippy`、`just acceptance`；涉及调度域契约时追加 virt-hetero。按项目脚本保存日志、保留退出码、收割 QEMU 残留；已知竞态矩阵 flake 按 KNOWN_ISSUES 判读。

只有公共前置、全部真实消费者、失败/退役路径、旧机制删除、组合验证和文档现状同时满足，才标记整体完成。提交后登记对应固定提交的未来代码 Review；方案本身不送 reviewer。
