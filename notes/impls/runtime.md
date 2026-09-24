# 用户态执行与监督

当前运行配置是验收配置，但实现成熟度按责任层而不是 binary 整体判定。`user/libraries/libexecution` 的 actor 执行机制、`libprocess` 的 Process/Job 收束状态机和 `librpc` 的调用/Outbox owner 已是正式机制；`srv_init::supervisor` 的根监督、`srv_pm` 的 JobDriver 与 `srv_fs` 的双 provider Runtime 是正式装配接缝。固定拓扑、阶段剧本、消息数量、日志和容量仍是验收政策，不构成未来正式运行配置。FAL1 自客户端和兼容泵已经删除；后续服务若新增过渡路径，仍须在对应专题计划登记删除条件。内核对象和 ABI 沿用现有 WaitSet、ProcessDrain 与 Job 管理面。

成熟度采用四类记录：正式机制（可脱离当前剧本成立的 owner/状态机契约）、正式装配接缝（启动 grant、endpoint 与监督 authority 的目标结构）、验收政策（可替换的场景编排）和过渡路径（有唯一计划、删除条件与验证门）。同一服务可同时包含四类；实现文档描述当前归属，方向文档不记录阶段标签。

## Runtime 的调度、输入与退休

`runtime.rs` 独占来源集合、登记表及有序 Gate，`work_queue.rs` 拥有稳定任务、ready FIFO、任务期限及执行失败退避槽。任务采用 `Active → Finalizing → Retiring` 生命周期；业务 `Complete` 不立即摘除任务，只有所有来源实际注销后才退款和删除。停止冻结新任务准入，已有任务仍可登记清理所需的观察。Finalizing 不再接受 stop/wake 重排，摘除前解除 ready 归属。

执行面轮转期限、维护、任务；维护面继续轮转事件接收、来源退休、任务退休。每次扫描和丢弃旧记录也计工作量，某个工作面没有剩余预算不会阻断另一工作面。来源退休各有出生时预付的 TimerQueue 槽，成功注销只取消自身槽；最近重试期限从堆顶取得，不扫描全来源表。`shared/timer_queue::value_mut` 允许在预付后绑定外部登记返回的 token，不改变期限、堆序或 generation。

每个任务的输入 FIFO 链接位于正式来源记录中，以稳定 token 寻址。来源 ready 进入预付 pending 槽，仅入队一次；advance 最多取本次预算允许的记录，未消费记录按原序还回队首。无需遍历该任务的全部潜在来源，也没有注销留下的无界 tombstone。Rearm、Remove 与迟到 generation 共用同一账本。

`Requests` 的有限容量是每次任务推进的输出政策，不是服务连接数限制。其缓冲和额度在 Runtime 创建时预备，推进间复用；PendingGate 只持结算状态，存在 Gate 时不推进新的任务，每步从同一缓冲执行或拒绝一个操作；失败 advance 的剩余请求也保留到后续 Gate 结算。拒绝即使发生在业务提出 Complete 后，也让原任务再次取得推进机会，处理返还的责任；来源超过该任务的上限时 Gate 通过 `RequestFailure::Source` 返还错误，不登记来源，任务与 InputBytes 在真正退休后退款。失败推进的业务期限与 ready/Hold 状态独立保留，Gate 收尾重新读取任务状态而不清空业务 timer；同一已投递期限不会被重复武装。任务执行和 Gate 回调前按该任务的通知义务确认到期，不依赖有预算的期限堆已弹出其 timer；较早的执行 Retry 不能使任务在未取得已到期业务输入时改写期限。Rearm/Remove 在正常、Complete 和失败 Gate 中都校验任务归属，维护面使用独立内部入口。请求、接收与 scratch 存储、来源记录都有准入；`Runtime::input_budget` 按实际类型大小和来源数量推导装配账户所需的输入额度。

创建期间 `Runtime::try_new` 在容量/分配拒绝时按值返还原 SourceSet 和错误，输入 Charge 随未完成装配退款；已有 `new` 保持原错误签名，FAL Copy 这种在用户调用中动态创建 WaitSet 的消费者使用 `try_new`，明确关闭失败仍保留 set。Copy 的一条任务最多登记源/目标各 DATA 与仅终态来源、可选取消来源共五项；本地 Budget/Account 按 `Runtime::input_budget` 预付，所有来源注销回执和任务退休后检查 Task/InputBytes 归零，Set 关闭失败由 CopyFailure 带缓冲快照续作。

`libbudget` 在 `metadata_admission::Permit` 与 `SponsoredPermit` 上建立非领域化 Budget/Account、带预算身份的 `BudgetSlot`、不可变 `AccountView<K>` 与非泛型 Charge。Account 是唯一付款身份；领域视图只把分类绑定到同一实际布局，跨 Budget 的槽会被拒绝，视图克隆只克隆 Arc、不重新分配绑定或创建账户。零单位 Charge 也持有账户，保证结构性账户名额直到真实 owner 释放；计量 Charge 支持只减不增的 `shrink_to`，资源实际占用缩小时立即返还差额，并同步更新本地账户与 sponsor/global 两层计数。FAL 成功 Take 用这一出口把原属性 Bytes charge 收缩到空值实际长度，避免把已移交值的历史容量保留到节点退休。

Runtime 固定消费 `AccountView<ExecutionResource>`；任务与输入字节不再借用 FAL 或服务分类，也不由装配者传入裸槽位。`srv_fs` 的 FAL 与执行视图共享一个 Account：两个视图保留同一付款身份，各自绑定 FAL 和执行实际额度；grant 派生继续克隆 FAL 视图，不能重置本地限额。

`SourcePlan` 是不透明的值类型，携带普通 WaitSet 登记意图，不拥有关闭权或共享数据访问权。Runnel 等领域生成计划，运行体注入任务 cookie 并登记。计划本身不分配，也不通过任意注册回调引入另一套执行路径。Add/Arm 入口统一为 Register 值计划，直接经 SourceOps 登记。在调用内核前准备额度、来源节点和退休期限槽；失败退款，初始登记直接使用第一代次。

登记和实际注销分别经 `Task::registered`、`Task::unregistered` 交付回执。任务不必等第一条来源事件才能取得 SourceId，因而首次等待超时也能撤销观察；机器在注销回执后才兑现被观察 owner 的关闭。

## 组合等待与失败隔离

`DriveState` 区分 Runnable、Waiting(deadline)、Drained，`Runtime::run` 和外层组合驱动消费同一状态。外层只能借用运行体集合的观察目标；集合 ready 后调用 `notified`，原 Runtime 仍独占实际 ready 记录的接收和路由。

任务失败保留任务、输入、来源、Gate 和额度。`defer_failed_task` 使用独立于业务期限的预付重试槽，也支持暂停直到管理者显式恢复；来源事件可以保留，但不越过执行退避反复调度。执行重试不伪造业务 timeout。基础设施故障由外层保留整个 Runtime。

`close` 在任务、来源或 Gate 未清空时返还完整 Runtime；WaitSet 关闭失败同样返还原运行体。异常 Drop 只是进程放弃的兜底，不是正常清理或精确退款的证明。

## Process 与 Job 收束

`libprocess/supervise.rs` 的 Collector 从等待 REAPABLE/CLOSED 开始，依次进入 Drain、Verify、Close。`ProcessOperations` 使真实系统调用和故障替换测试消费同一台机器。Close 成功前保留 snapshot；所有失败与停止结果都持有完整 Collector。管理者给原机器续作额度时不重建 Drain、不回退游标、不丢 Closing 快照。

`observation.rs` 定义带唯一身份、control、signals 和绝对 deadline 的 Observation。错误身份或提前 timeout 不改变正在等待的请求；有效 Ready、实际超时和来源错误有独立结果。发出观察不消耗失败次数，实际超时只计一次。恢复接口只接受结果并改变状态，下一步必须再次调用 step，避免驱动器忽略恢复时隐含返回的新事件。已选中的 Ready 不因恢复或注销较晚被改判为超时；对于 Process/Job 的持久终态信号，ObservationSlot 在期限到达且尚无输入时只做一次 At(0) 非阻塞裁决，避免 WaitSet 记录尚未取出便误判超时。这不是对停驻任务的周期轮询。错误阶段由原机器生成，等待尝试和未完成成员的工作量计入真实进度。

`ObservationSlot` 连接 Runtime：处理登记回执、来源事件、期限、来源错误及注销回执，再将匹配结果交给原机器。`SuperviseTask` 与 `job_driver.rs` 共用此连接。同步门面使用同一个 Observation 的绝对等待，RetryAt 使用绝对 sleep；状态机内部没有阻塞 wait。

`JobCollector` 使用单一 frame stack，每步处理一个枚举批、成员或子 Job 阶段。每个 frame 的 JobPage 只持一份 JOB_ENUMERATE_MAX 容量的预付页，成员与 children 按阶段复用；页内条目完成后才推进 position，页耗尽且 more 才按 next_cursor 取下一页，不累积全量列表。每批验证实际条数、严格递增 ID 和游标，不越过未决占位；零进展预算可由 replenish 续接。栈位和子页在派生 authority 前 fallible 预留，child 完成后才回退并关闭其 control，关闭失败仍保留父页和位置。后续页失败保留前批已提交的进度，枚举输入游标不变。成员直接使用同一个 Process Collector。`JobKillFailure` 按值返还原 JobCollector，不在入口或错误包装中依赖不可失败 Box 分配。

## init 与 pm 的真实装配

`RootSupervisor` 在 main 中建立，由脚本借用。services 根一建立，就预备其兜底 frame、Runtime 和任务槽，再启动子服务；正常阶段该任务处于未激活的待命状态。Job、Batch 各两槽分别覆盖原操作和 services 兜底，Read 一槽覆盖当前流阶段；加上空闲集合，组合等待最多六项，满足现有 WaitMany ABI。

Job duty 持有原机器、待准入任务、Runtime、世界与失败处置；Batch 同时持有已准入任务、未准入输入、失败/成功结果；Read 同时持有运行体、流 owner、缓冲与进度。setup、partial-spawn、run、close 失败不销毁这些槽。批次准入失败不阻止已入队成员继续工作；结果逐项结算，错误摘要与完整失败 owner 分离。

根按持续游标推进可工作运行体，统一等待所有健康集合和最早期限，不在某个 duty 内调用无限 run。可恢复错误退避，永久错误仍以原类型持有责任，其他运行体继续。失败脚本结束后，services 机器接管管理范围；已摘除对象的旧 control 仍由原 duty 兑现，不能靠重新枚举替代。所有责任确已结束后才尝试失败 reset；平台拒绝时保留根。流建立失败的 Endpoint/Invitation 也有预备清理槽。辅助 Job controls、启动 mailbox/委托副本及流控通知通过预备能力账本的工厂创建，脚本只借用 handle；内核确认 Grant/Send 消费后才兑现本地 owner。pm sender 始终保持 typed owner，不转回无人负责的 raw handle。正常关闭和失败 reset 均覆盖该账本，活动 Job 借用的 root 不会提前关闭。

pm 的 DomainTask 直接使用 JobDriver，删除另一套成员枚举/收束编排；运行体每轮 `max_work=1`。MailboxTask 在分发测试消息后继续停驻，Flow 在 TAIL 后派生 Domain，保留测试阶段顺序。域收束后 main 调用 seal/shutdown_turn；stop 回调检查邮箱已有实际登记，最终检查注销回执已完成、Task/InputBytes 额度归零，并输出必检锚点 `pm: active mailbox stop and refund passed`。不可恢复错误在 owner 仍存活时以非零状态退出，由 init 的独立管理能力接管；该政策不用于 init。

init 的 done→腾位→TAIL 使用同一绝对期限。root 故障隔离验收通过正式运行体构造部分准入失败、一个失败成员与健康成员，再给原机器续作额度；另让一台 Job Runtime 暂停时完成另一台 Batch Runtime。两条成功锚点均纳入 QEMU 验收脚本。另验证创建 mailbox 后 duplicate 拒绝仍保留两端 owner、关闭接收端后 Send 拒绝仍保留待移交 JobControl，以及正常结束时辅助能力回到仅保留 services 根的账本基线。seal/create 竞态夹具的临时 JobControl 副本也走同一账本，仅在发送成功后兑现移交。

源码与验证索引见 `plans/archived/review-2026-09-15-runtime-closure.md`。同步便利门面和刻意 raw 的顺序验收脚手架继续保留；RPC/Outbox、FAL 业务与内核公共操作重构不属于本闭包。

当前清理批次的 host 证据见 `artifacts/cleanup-paging-host.log`：分配失败时预付 Gate 仍返还失败派生任务并退休；Job 多页成员/children、嵌套、条目消失、零进展、后页错误与关闭失败均从原状态恢复，栈和子页分配失败先于派生能力。core 及完整 acceptance 已验证 Active 邮箱停止及退款；最终 `artifacts/acceptance-cleanup-paging-20260915-131912.log` exit 0，源码哈希一致，两份集中复核均无 finding。完整边界见固定提交 Review 的已授权批次记录。
