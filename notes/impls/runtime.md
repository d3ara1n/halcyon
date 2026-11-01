# 用户态执行与监督

正式 System 由 init 的组态执行器装配和监督，设备、驱动管理者与文件宿主各自组合运行机制。`libexecution` 拥有 actor 执行与来源退休，`libprocess` 拥有 Process/Job 收束，`librpc` 拥有调用与 Outbox；这些库不要求专用测试进程参与推进。

## Runtime 的调度、输入与退休

Runtime/WorkQueue/Assembly 的来源、Gate、公平性与退休机制由 [framework](framework.md#libexecution-的执行与扁平装配) 唯一描述；本篇拥有实际运行体装配、付款视图和业务收束。运行体只借用 Registration，集合接收、停驻和关闭由 Assembly 持有。

RPC 的固定/每调用来源付款和 Outbox 绑定由[通用 RPC 实现](rpc.md#当前嵌入与边界)拥有。实际消费者在提交任务前准备 execution view；各任务保留自己的输入、来源与资金，客户端在 Dispatcher 构造前冻结共享资金。

`libbudget` 在 metadata_admission::Permit/SponsoredPermit 上建立 Budget/Account、带身份 BudgetSlot、AccountView<K> 与 Charge。Account 是付款身份，视图只绑定分类；跨 Budget 的槽被拒绝，克隆不重置额度。零单位 Charge 仍持账户，shrink_to 在实际占用缩小时立即返还本地与 sponsor/global 两层额度。
用户树显式选择libbudget::AllocatorBacked（C=()）：本层不新增逐分配软额度，使用可执行程序选择的allocator，既有领域Charge仍在业务记录中，混合布局计算用当前节点allocation_bytes。它不新开户、不替用户选择运行时、不证明Kernel metadata已付款；Task、Source InputBytes、文件/服务费用与退款owner保持各自业务合同，逐heap字节保留/Weak尾部不由此声称覆盖。Runtime Source InputBytes包含实际AVL节点布局与独立timer值布局。

Runtime 仅从显式 TaskSubmission 取得任务付款，构造 view 只支付固定基础设施，不借领域分类或裸槽位。srv_fs 的可信发行接口同时借用领域与 execution 两份 view，校验 account_id 相同并在角色、控制及 Request 中冻结；[boot 装配](../../user/services/srv_fs/src/server/boot.rs) 在同一 Budget 下为宿主和各业务入口创建独立 Account，入口配额由 deployment 提供。grant 派生与控制终态壳保留原领域/执行付款配对。
登记和实际注销分别经 `Task::registered`、`Task::unregistered` 交付回执。任务不必等第一条来源事件才能取得 SourceId，因而首次等待超时也能撤销观察；机器在注销回执后才兑现被观察 owner 的关闭。

## 组合等待与失败隔离

`DriveState` 区分 Runnable、Waiting(deadline)、Drained。单运行体由 Assembly.run 驱动；多运行体的外层每轮只接收一个有界候选批，再经 feed 分流，取健康运行体与退避的最早绝对期限统一停驻。fs 使用扁平 Assembly；init 的组态执行器直接管理自己的 WaitSet 和实例观察。

任务失败保留任务、输入、来源、Gate 和额度。`defer_failed_task` 使用独立于业务期限的预付重试槽，也支持暂停直到管理者显式恢复；来源事件可以保留，但不越过执行退避反复调度。执行重试不伪造业务 timeout。基础设施故障由外层保留整个 Runtime。

Runtime.close 在任务、来源或 Gate 未清空时返还原运行体；Assembly 无 Registration 借用后才关闭实际集合，失败返还接收 owner。非空 Runtime 或接收 owner 异常 Drop 使整装配失效，正常失败仍由原运行体保留责任并推进退款。

## 资源退休消费者与复用边界

执行器不提供任意资源 Drop 的必成提交入口；Runtime.spawn 和 Requests.spawn 失败返还完整 task/max_sources/funding 提交物。原任务之外仍存活的领域 payload 继续由其领域 Charge owner 持有，不随 Slot 退休提前退款。

文件 backing 是正式退休消费者：`libfs/store.rs` 的 NodeRef Drop 在最后 pin/link 消失时，把已有 Lease 链入 RetireQueue 并调用 `libexecution::wake::Wake`，发布不新分配；payload 一直由 NodeStore 记录持有。`retire_step` 取出 payload 推进一步，无论返回成功或错误先放回记录，完成后才删除并释放计费。`srv_fs/server/lifecycle.rs::Retire` 持 typed NotificationOwner、持久来源登记与实际注销回执，以 backing 真实待退休项判断立即可推进，不以存活根尚未清空表示 runnable；停止后 Empty 保持推进至来源注销。NotificationWake 持 typed Signaler，未知 Capability 经一次角色转换及返回描述的 SIGNAL 校验采用。完整节点与请求责任归 [文件实现](fs.md)。

这条路径使用 Rc/Cell/RefCell，属于 provider 单线程节点/链接生命周期；不是跨 OS 线程的任意 cleanup 队列。非空 NodeStore Drop 记录 abandoned 并保留表，WorkQueue 非空 Drop 记录 abandoned 后销毁任务；这些不是运行体被丢弃后仍有推进者的证明。NotificationWake 把 ObjectClosed 当可结束发布处理，也不能单独证明该责任另有接收者。

init 的能力与实例 owner 由[组态执行器](../../user/services/srv_init/src/executor.rs)持有；失败交统一 terminal 进行有界收束。它不提供公共任意资源退休服务。线程栈与环境的 owner 见 [framework](framework.md)。

## Process 与 Job 收束

`libprocess/supervise.rs` 的 Collector 等待 DONE 后进入 Verify、Close，不主动驱动内核回收。`ProcessOperations` 使真实系统调用和故障替换测试消费同一台机器。Close 成功前保留 snapshot；所有失败与停止结果都持有完整 Collector。管理者给原机器续作额度时不重新触发回收、不回退游标、不丢 Closing 快照。

`observation.rs` 定义带唯一身份、control、signals 和绝对 deadline 的 Observation。错误身份或提前 timeout 不改变正在等待的请求；有效 Ready、实际超时和来源错误有独立结果。发出观察不消耗失败次数，实际超时只计一次。恢复接口只接受结果并改变状态，下一步必须再次调用 step，避免驱动器忽略恢复时隐含返回的新事件。已选中的 Ready 不因恢复或注销较晚被改判为超时；对于 Process/Job 的持久终态信号，ObservationSlot 在期限到达且尚无输入时只做一次 At(0) 非阻塞裁决，避免 WaitSet 记录尚未取出便误判超时。这不是对停驻任务的周期轮询。错误阶段由原机器生成，等待尝试和未完成成员的工作量计入真实进度。

`ObservationSlot` 连接 Runtime：处理登记回执、来源事件、期限、来源错误及注销回执，再将匹配结果交给原机器。`SuperviseTask` 与 `job_driver.rs` 共用此连接。同步门面使用同一个 Observation 的绝对等待，RetryAt 使用绝对 sleep；状态机内部没有阻塞 wait。

`JobCollector` 使用单一 frame stack，每步处理一个枚举批、成员或子 Job 阶段。每个 frame 的 JobPage 只持一份 JOB_ENUMERATE_MAX 容量的预付页，成员与 children 按阶段复用；页内条目完成后才推进 position，页耗尽且 more 才按 next_cursor 取下一页，不累积全量列表。每批验证实际条数、严格递增 ID 和游标，不越过未决占位；零进展预算可由 replenish 续接。栈位和子页在派生 authority 前 fallible 预留，child 完成后才回退并关闭其 control，关闭失败仍保留父页和位置。后续页失败保留前批已提交的进度，枚举输入游标不变。成员直接使用同一个 Process Collector。`JobKillFailure` 按值返还原 JobCollector，不在入口或错误包装中依赖不可失败 Box 分配。

## init 的真实装配

[`Executor`](../../user/services/srv_init/src/executor.rs) 持一个 WaitSet、各实例的 Record、具名 Pool/Job、封存映像和路由。Start 前登记 Control DONE 与报告来源；实例正常退出后按实际 ProcessSnapshot 判断终因，resident 只有在预期停止且正常完成时才接受退出。异常交系统故障终局。结构与协议由[启动实现](startup.md#init-的组态装配与根监督)拥有。

### 顺序装配的等待与推进

ImageCopy 与 Preparation 每次 advance 前以 At(0) 消费已登记观察；映像等待 EXECUTABLE 时使用直接 WaitOne。就绪、task 依赖与常驻监督使用同一 pump，等待并接收有限批次，按 cookie 分派 DONE 和报告。撤销路由使用有限期限的同步 Caller，Query 未 Closed 时按固定间隔重查。当前执行器没有另一套 Job/Batch/Read duty 或业务验收 Runtime。

srv_fs 每个 provider 进程各建自己的 Assembly 与 Runtime，接收、feed、任务退休及集合关闭在该进程内收束；不同 provider 不共享 Assembly。
