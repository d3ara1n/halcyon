# 用户态执行与监督

当前运行配置是验收配置，但实现成熟度按责任层而不是 binary 整体判定。`user/libraries/libexecution` 的 actor 执行机制、`libprocess` 的 Process/Job 收束状态机和 `librpc` 的调用/Outbox owner 已是正式机制；`srv_init::supervisor` 的根监督、`srv_pm` 的 JobDriver 与 `srv_fs` 的双 provider Runtime 是正式装配接缝。固定拓扑、阶段剧本、消息数量、日志和容量仍是验收政策，不构成未来正式运行配置。FAL1 自客户端和兼容泵已经删除；后续服务若新增过渡路径，仍须在对应专题计划登记删除条件。内核对象和 ABI 使用扁平 WaitSet、Process DONE 和 Job CLOSED；ProcessDrain 已删除。

成熟度采用四类记录：正式机制（可脱离当前剧本成立的 owner/状态机契约）、正式装配接缝（启动 grant、endpoint 与监督 authority 的目标结构）、验收政策（可替换的场景编排）和过渡路径（有唯一计划、删除条件与验证门）。同一服务可同时包含四类；实现文档描述当前归属，方向文档不记录阶段标签。

## Runtime 的调度、输入与退休

Runtime/WorkQueue/Assembly 的来源、Gate、公平性与退休机制由 [framework](framework.md#libexecution-的执行与扁平装配) 唯一描述；本篇拥有实际运行体装配、付款视图和业务收束。运行体只借用 Registration，集合接收、停驻和关闭由 Assembly 持有。

`libbudget` 在 metadata_admission::Permit/SponsoredPermit 上建立 Budget/Account、带身份 BudgetSlot、AccountView<K> 与 Charge。Account 是付款身份，视图只绑定分类；跨 Budget 的槽被拒绝，克隆不重置额度。零单位 Charge 仍持账户，shrink_to 在实际占用缩小时立即返还本地与 sponsor/global 两层额度。
用户树显式选择libbudget::AllocatorBacked（C=()）：本层不新增逐分配软额度，使用可执行程序选择的allocator，既有领域Charge仍在业务记录中，混合布局计算用当前节点allocation_bytes。它不新开户、不替用户选择运行时、不证明Kernel metadata已付款；Task、Source InputBytes、FAL/Service费用与退款owner保持各自业务合同，逐heap字节保留/Weak尾部不由此声称覆盖。Runtime Source InputBytes包含实际AVL节点布局与独立timer值布局。

Runtime 消费 AccountView<ExecutionResource>，不借 FAL 分类或裸槽位。srv_fs 的 FAL 与执行视图共享同一个 Account；grant 派生保留同一付款身份。FAL Take 用 shrink_to 把已移交属性的 Bytes charge 缩到空值实际占用。
登记和实际注销分别经 `Task::registered`、`Task::unregistered` 交付回执。任务不必等第一条来源事件才能取得 SourceId，因而首次等待超时也能撤销观察；机器在注销回执后才兑现被观察 owner 的关闭。

## 组合等待与失败隔离

`DriveState` 区分 Runnable、Waiting(deadline)、Drained。单运行体由 Assembly.run 驱动；多运行体的外层每轮只接收一个有界候选批，再经 feed 分流，取健康运行体与退避的最早绝对期限统一停驻。RootSupervisor 与 fs 使用同一扁平装配，不注册集合成员或保留 idle 集合。

任务失败保留任务、输入、来源、Gate 和额度。`defer_failed_task` 使用独立于业务期限的预付重试槽，也支持暂停直到管理者显式恢复；来源事件可以保留，但不越过执行退避反复调度。执行重试不伪造业务 timeout。基础设施故障由外层保留整个 Runtime。

Runtime.close 在任务、来源或 Gate 未清空时返还原运行体；Assembly 无 Registration 借用后才关闭实际集合，失败返还接收 owner。非空 Runtime 或接收 owner 异常 Drop 使整装配失效，正常失败仍由原运行体保留责任并推进退款。

## 资源退休消费者与复用边界

执行器不提供任意资源 Drop 的必成提交入口；Runtime.spawn 失败返还任务。Pump 动态准备 Assembly 与 Runtime，失败 carrier 持同一个真实集合的 Raw/Ready 所有权枚举、任务与缓冲快照；先退休任务和来源，再关接收 owner。源/目标各 DATA 与终态来源，加可选取消，共五项，由实际类型大小推导 InputBytes，不另建接收缓冲。

FAL 是正式退休消费者：`libfal/store.rs` 的 NodeRef Drop 在最后 pin/link 消失时，把已有 Lease 链入 RetireQueue 并调用 `libexecution::wake::Wake`，发布不新分配；payload 一直由 NodeStore 记录持有。`retire_step` 取出 payload 推进一步，无论返回成功或错误先放回记录，完成后才删除并释放计费。`libfal/provider.rs::Retirement` 已接上持久 Notification source、take、backend 预算清理、停止后的空表检查与注销回执。完整节点语义归 [FAL 实现](fal.md)。

这条路径使用 Rc/Cell/RefCell，属于 provider 单线程节点/链接生命周期；不是跨 OS 线程的任意 cleanup 队列。非空 NodeStore Drop 记录 abandoned 并保留表，WorkQueue 非空 Drop 记录 abandoned 后销毁任务；这些不是运行体被丢弃后仍有推进者的证明。NotificationWake 把 ObjectClosed 当可结束发布处理，也不能单独证明该责任另有接收者。

RootSupervisor 的 Cleanup<T> 在 [supervisor.rs](../../user/services/srv_init/src/supervisor.rs) 以原槽承接 Endpoint/EndpointCleanup/Capability 的关闭失败，由根轮转推进；资源工厂先预留 capability 账本再创建对象。它是当前根装配的明确 owner，不是公共任意资源服务；线程栈/环境的实际 owner 见 [framework](framework.md)。

## Process 与 Job 收束

`libprocess/supervise.rs` 的 Collector 等待 DONE 后进入 Verify、Close，不主动驱动内核回收。`ProcessOperations` 使真实系统调用和故障替换测试消费同一台机器。Close 成功前保留 snapshot；所有失败与停止结果都持有完整 Collector。管理者给原机器续作额度时不重新触发回收、不回退游标、不丢 Closing 快照。

`observation.rs` 定义带唯一身份、control、signals 和绝对 deadline 的 Observation。错误身份或提前 timeout 不改变正在等待的请求；有效 Ready、实际超时和来源错误有独立结果。发出观察不消耗失败次数，实际超时只计一次。恢复接口只接受结果并改变状态，下一步必须再次调用 step，避免驱动器忽略恢复时隐含返回的新事件。已选中的 Ready 不因恢复或注销较晚被改判为超时；对于 Process/Job 的持久终态信号，ObservationSlot 在期限到达且尚无输入时只做一次 At(0) 非阻塞裁决，避免 WaitSet 记录尚未取出便误判超时。这不是对停驻任务的周期轮询。错误阶段由原机器生成，等待尝试和未完成成员的工作量计入真实进度。

`ObservationSlot` 连接 Runtime：处理登记回执、来源事件、期限、来源错误及注销回执，再将匹配结果交给原机器。`SuperviseTask` 与 `job_driver.rs` 共用此连接。同步门面使用同一个 Observation 的绝对等待，RetryAt 使用绝对 sleep；状态机内部没有阻塞 wait。

`JobCollector` 使用单一 frame stack，每步处理一个枚举批、成员或子 Job 阶段。每个 frame 的 JobPage 只持一份 JOB_ENUMERATE_MAX 容量的预付页，成员与 children 按阶段复用；页内条目完成后才推进 position，页耗尽且 more 才按 next_cursor 取下一页，不累积全量列表。每批验证实际条数、严格递增 ID 和游标，不越过未决占位；零进展预算可由 replenish 续接。栈位和子页在派生 authority 前 fallible 预留，child 完成后才回退并关闭其 control，关闭失败仍保留父页和位置。后续页失败保留前批已提交的进度，枚举输入游标不变。成员直接使用同一个 Process Collector。`JobKillFailure` 按值返还原 JobCollector，不在入口或错误包装中依赖不可失败 Box 分配。

## init 与 pm 的真实装配

RootSupervisor 在 main 中建立，实际持一个 Assembly；Job、Batch 各两槽与 Read 一槽的 Runtime 全部从它取得 Registration。services 根建立即准备兜底 frame、运行体与任务槽，子服务启动后再激活。没有原六项 WaitItem/空闲集合或每 duty 的集合接收 owner。

Job duty 持有原机器、待准入任务、Runtime、世界与失败处置；Batch 同时持有已准入任务、未准入输入、失败/成功结果；Read 同时持有运行体、流 owner、缓冲与进度。setup、partial-spawn、run、close 失败不销毁这些槽。批次准入失败不阻止已入队成员继续工作；结果逐项结算，错误摘要与完整失败 owner 分离。

根按持续游标推进可工作运行体，接收一次候选批并给各运行体 feed，再用共同集合与最早期限停驻。可恢复错误退避，永久错误仍以原类型持有责任，其他运行体继续。失败脚本结束后 services 机器接管管理范围，旧 control 仍由原 duty 兑现。流建立失败的 Endpoint/Invitation 有预备清理槽；辅助 controls、mailbox/委托副本与通知由能力账本持有，内核确认 Grant/Send 消费后才兑现移交。正常关闭与失败 reset 均覆盖该账本，活动 Job 的 root 不提前关闭。

pm 的 DomainTask 直接使用 JobDriver，MailboxTask/Flow/域收束共享单个 Assembly 与 Runtime，推进量子仍为每轮一项。域结束后 seal/shutdown_turn，注销来源与退休任务后检查 Task/InputBytes 归零，先关闭 Runtime 再关闭实际集合。不可恢复错误调用 ProcessAbort，由 init 的独立管理能力接管。fs 两个 provider Runtime 共用一个 Assembly，外层接收并分别 feed，两侧关闭后才能释放集合。

init 的 done→腾位→TAIL 使用同一绝对期限。main/raw 中的故障隔离、正常退出、能力基线、单/多来源等待与 Tunnel 夹具使用新接口；持久源的陈旧提示和嵌套拒绝由原集合夹具承接，非空集合退出改观察自动退休。Sender 与流映射各自持实际 owner。QEMU 组合进度与仍未覆盖的运行行为由对象链工作记录拥有，不以源码接通替代完整验收。
