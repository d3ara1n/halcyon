# 任务模型实现

> 任务是什么、进程与线程的分工等概念层见 [`../ideas/task.md`](../ideas/task.md)；本篇记录其在内核中的落地。线程持有 UserContext（用户现场，每线程一份）与调度状态；进程持有资源和从 ELF 得出的执行能力需求。完整上下文契约见 [`execution-context.md`](execution-context.md)。

公共 [`task::Thread`](../../os/kernel/src/task/mod.rs) 指向 [`process/thread.rs`](../../os/kernel/src/task/process/thread.rs)，稳定持 Shell、成员凭据、内嵌现场以及 satp/域/FP 档位。Shell 以已付款 `OwnedBox<ProcessBody,MetadataCharge>` 唯一拥有 Body；成员槽、短访问与离场回执分别管理。Body 的 AnySpace、Views 与待激活 RetireWork 由真实 ProcessCreate 和 bootstrap 构造。独立 Shell/Builder/Thread/Departure/Control/Ticket 的 Arc 按真实布局出生付款，内嵌 Work/Frame/节点不另计；共用口径见 [mm](mm.md#已付款保留存储)。公开组装/控制入口见 [`process.rs`](../../os/kernel/src/task/process.rs)，接续与验证边界由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。

## 进程执行环境

Body 持有 AnySpace、CapTable 和 Members；两张私有槽数组均以 [`funded_storage`](../../shared/funded_storage/src/lib.rs) 持有固定容量及付款凭证，由所属 Shell 的 Pool 支付，DropBody 在全部 Shell/VM 锁外释放后才发布 DONE。用户地址几何由 [`mm.md`](mm.md) 拥有。ProcessBindExecution 在 Building 短租约内一次写入兼容 SchedDomain 和 IsaRequirement，重复返回 ObjectNotAvailable；Thread 出生时冻结自己的域、satp 与 FP 档位。首线程现场：sp 为组装者给出的 16 字节对齐栈顶，a0/a1 为出生块基址/长度，tp 置零。Running 堆由 libruntime 的匿名映射组成，任务层不持连续堆顶。

## 调度：域—类—执行点三层组合

```
执行点（每 hart 一份，HartLocal）  调度域（共享，boot 冻结）      调度类（策略容器）
┌─────────────────────────┐  ┌──────────────────────┐  ┌─────────────────────────┐
│ owner: AdmittedThread   │  │ SchedDomain           │◀─│ trait SchedClass         │
│ （调度循环 + idle 循环）  │  │  classes: 优先级序数组 │  │ enqueue / pick /         │
│ 域归属经 per-slot 域表   │─▶│  idle_mask: 域空闲位图 │  │ has_ready / reserve /    │
└─────────────────────────┘  └──────────────────────┘  │ enqueue / publish_one            │
                                                       └─────────────────────────┘
```

- **执行点**：hart 的运行现场（当前线程、trap 锚），见 `internals.md`「tp 寄存器」。调度循环与 idle 循环是执行点的行为。
- **调度域**：`SchedDomain` 持有优先级序的调度类数组与域内 `idle_mask`。新 Thread 出生固定 `domain`，ReadyBatch 对该域复核后才发布；wake 只向该域 idle hart 发门铃，静默谓词遍历全部域。硬件能力、域划分、D64 eligibility 与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一记录。
- **调度类**：公平类以 LEAF 锁保护 `os/ready_queue::ReadyQueue<Arc<Thread>,MetadataCharge>`，通过 `SchedClass` 实现 enqueue/pick/has_ready 与 reserve_batch。`SchedDomain::reserve_ready` 返回绑定实际类存储的 `ReadyBatch`，每线程以 `publish_one` 无分配交付；Start、Bootstrap、ThreadCreate 都在不可逆点前取得每线程准入，Start/Bootstrap 通过预留发布链交付，失败不产生前缀发布。

`ready_queue::Admission` 拥有整个可调度寿命的容量位置，未交付 credit 随 Drop 原子退还。`AdmittedThread = Admitted<Arc<Thread>>` 不可 Clone，由值与一个同源 credit 组成；队列只接受该 owner。Running/Waiting 保留 credit，进入 Ready 不重新分配；取消或最终离场才归还。Capacity Arc 是唯一身份/原子计数，内核在 root seed 前的 boot 固定占用中供给；不在 Admission 内持存储 charge，远端退 credit 不反取 Pool/队列锁。pick 不扫描出生预留。

容量不变量为 `storage.capacity ≥ outstanding ≥ ready.len`，outstanding 含所有已准入线程与尚未交付的出生 credit。`ReadyStorage` 使用固定已付 `Buffer<Option<Admitted<T>>>` 环，logical head/len 与全部初始化槽独立；尾索引、pick、scan单项轮转不分配。类锁内 reserve 复核全部 outstanding，候选不足不接受；锁外从系统 root Pool 预付并初始化候选，再回锁搬移 FIFO、不复制 owner，旧空环带出锁外释放。调度域冻结且长期持有高水位容量，账单属于系统域，不能把客户端 Pool 钉为全局缓存来源。每次候选为 `2*capacity+count`，退款只降低 outstanding；再次落败证明实际容量已超过所采样容量两倍，重试次数受自然字宽限制。初始化 O(capacity)、搬移 O(ready.len)，不可把次数界限冒充常量时长。metadata拒绝/OOM/溢出在提交前返回；Wake/Requeue沿用已付位置，credit归还不缩容量。整批 publish 只随本次线程数变化。

`os/ready_queue/tests/admission.rs` 覆盖 OOM、部分取消、错误队列、等待者与新出生并存、FIFO 模型及持 Ready 锁时的远端退款；allocator 计数探针要求发布、轮转、唤醒和退款不发生分配。

时间片为固定量子，tickless：调度循环取得U类准入后才新dispatch，选线程失败的尝试也结费并回仲裁。`arm_quantum`建立本次线程绝对期限，取该期限、本hart TimerQueue堆顶与U类剩余ticks截止的最早者设置timer；Switch清本hart绝对期限。Resume只有在线程期限未到且原类别仍有费用/机会时才能领取继续机会，`rearm_quantum`复用同一线程绝对期限，不发新时间片或类别额度；否则Requeue。域内线程轮转由FIFO保证，U/M有限机会、费用条件与实际计时由[`internals.md`](internals.md)「Commit 后债务账本」拥有。

内核IRQ关闭时不能依赖未来timer先触发trap。调度循环每安全点复检本hart堆顶期限电平，已到期置软件pending并调用既有有界service_expired；连续M/无U/尚未idle也能把到期等待转为Ready。复检只读取一个堆顶，后续hint在Timer锁外；不在同一安全点排空全部期限。

调度器先在 Shell gate 取得 Running 准入，再在 Body 成员锁登记运行容器，最后在 VM gate 检查入口、同步 translation/instruction epoch 并登记 active；各锁分段释放。若 VM 封口先赢，归还 Running 准入并请求离场。trap 非 Resume 出口切回内核 root 并全量 SFENCE 后，调度器先注销 VM active、再归还 Shell Running 计数。epoch、SUM 与空间访问协议见 [`mm.md`](mm.md)。

### 单一归属不变量

任意线程任意时刻恰处于一个稳定容器：

```
调度类队列（Ready） ｜ hart current（Running） ｜ Body 成员槽（Waiting）
```

[`process/members.rs`](../../os/kernel/src/task/process/members.rs) 记录 Reserved/Created/Ready/Running/Waiting/Exiting，Shell gate 记录在途成员预留和未结离场回执。仿射 MemberReservation 在 Job→Shell→Members 锁序下 reserve/commit/rollback；出生失败或 Frozen 竞争输者自动撤销预留。槽数组先尝试复用；确需增长则在全部祖先门外预付和分配，再持门搬移并保持 slot/generation。候选落败时保持原 owner 到锁外释放，每次重试候选严格大于已采样容量，容量只增不减，竞争次数由自然字宽限制。成员槽可以复用，`MemberKey { slot, generation, tid }` 防止旧回执错指新线程，tid 单调不复用；活成员上界来自 `PROCESS_MAX_THREADS`，退役代次槽继续占实际已付容量。

[`task/thread.rs`](../../os/kernel/src/task/thread.rs) 的 ThreadCreate 在准备期经 Job→Shell 门受理成员预约，验证入口/栈并预付 ReadyBatch，准备 Control 与未 arm 票据。cap 表 reserve 后放锁写回，成功后在全部表锁外兑现成员预约并 arm，再回表公开；Frozen/Seal 不取消先受理的预约，shut 则令 cap commit 返回释放事件，在表锁外走正常放弃。Running 进程返回 Pending 票据，Building 的启动责任留给 Builder。Created 成员拥有真实 Thread；ThreadStartTicket 的 Prepared/Pending 仅持 Weak，外域票据不保活待回收的私有现场。ThreadStart 在 caller 表内 claim 票据，锁外 upgrade/start，成功消费 authority，失败恢复或锁外释放；失去执行容器返回 ObjectClosed。观察 ThreadControl 只发布 DONE。Pending 最后持有与 Start/终止扫描竞争 Created；已摘除槽的放弃幂等返回，获胜者在 Thread 最后释放后由 ThreadDeparture 结算。

Frozen 与外部访问/Ready 发布共用 Shell 原子 ingress 字；Start 的短访问、Waiting→Ready 与 Running→Ready 的发布凭证贯穿实际入队。回收 Work 先等 ingress 清零，再扫描成员和 Ready。成员锁内发布登记若看见 Frozen，则将线程交回执，不入队；若登记先赢，则入队后 Release 递减，末发布者在锁外唤醒回收。每次扫描只旋转/摘一个队首，命中 owner 在队列锁外 reap。全量扫描长度及回收阶段见 [`process/reclaim.rs`](../../os/kernel/src/task/process/reclaim.rs)，证据边界由所属计划拥有。

### 等待的所有权与仲裁

- **执行 owner 随容器走**：AdmittedThread 不可复制，在 Ready 队列、调度栈与成员 Waiting 之间移交；Thread 内嵌来源节点、timer 与 UnsafeCell<Option<Entry>>，只有当前执行 owner 访问续体。成员槽独占非回绕 epoch/active_wait/pending，与容器状态在同锁下仲裁。旧 epoch 或已离场身份的提示幂等落败。
- **发布时序**：dispatcher 将只含来源/兴趣/绝对期限的 WaitPlan 放入 HART_WAIT_PLANS 内联槽；完全退出 VM 与 hart 现场后才 begin_wait/挂链/登记 timer。安装后的首次重读覆盖已就绪和登记窗口。提示方只在成员锁内置 pending；Waiting 时取出 owner，经 Shell Ready ingress 交付队列并发送目标域门铃，Running/Ready 时只留下提示。
- **续体与唯一清理**：调度拾取成员后，在进入用户 VM 前先处理续体；复检前清 pending，一次读取电平/关闭/期限。满足则 detach 并写保存现场，不满足则同成员锁判断晚到 pending，移交 Waiting 或 Requeue；不原地复检循环。正常返回与 Frozen reap 同用 wait::detach：注销 timer、自摘来源节点、释放 loan、关闭成员 epoch。退出顺序为 detach→释放执行 owner→Departure.request，Frozen 扫描只取得 Waiting owner，不代写结果。来源机制见 [`ipc.md`](ipc.md)。

## Job、Building process 与发布

root Job 由内核 static anchor 强持；parent 强持 children，child 弱持 parent；Job 成员表强持未 Dead 的 Shell，Shell 弱持 Job。

ProcessCreate同时要求Job[CREATE]、Pool[CREATE]，不消费来源authority。Job链门预留pid/成员，Shell与构造lease出生后发布骨架；随后fallible构造ledger、funded root、VM、Views、成员/cap表并安装唯一Body。失败冻结Shell，由既有回收记录收束；未装Body空空间可局部退款。封口后的最后预约回滚交回依赖并唤醒已发布的Job Work。

Builder 与 Control 在调用者表预约后，先锁外写 ProcessCreateResult，再 arm Builder，最后表 commit；输出失败冻结新 Shell。进程从出生即付款固定，无 Unbound/BindMemory。组装顺序为 Create → Map/Write/Grant/BindExecution → ThreadCreate → Start；所有目标访问持短 Building BodyLease，不跨异步等待。

来源 Job/Pool、非消费 Builder 组装、Control Query/Kill 使用 Authorization，保持源表项可供合法并发使用；Building/冻结/Start 的领域闸门继续仲裁业务准入。Builder Start 与启动票据真正消费授权，使用独占 claim；Grant 的来源移交也使用 claim，不把目标 Builder 的非消费授权与所移交的 capability 混合。

ProcessMap 复用 `vm::operation::initial_map`，由 Operation/ChangeWork 完成并以私有 DONE 回复。ProcessWrite 先从调用者拷入有界缓冲，再持 Building VM lease 通过译址校验填入目标页；允许填充最终为 RO/RX 的初始映像，入口仍封闭。ProcessGrant 先 claim 全部来源并验证权限，输出目标句柄成功后才收窄并移交 cap；任何之前的失败恢复原权限，目标表锁内不调用业务对象。

ProcessControl就是Shell本体，关闭仅退authority；Query在Dead后仍读同一终态。Control类型上限含TRANSIT，Create/JobDerive仍按调用者及源权限明确取子集，普通监督配置不默认给运输权。Kill 在首次 Frozen 时固定原因，其后幂等；私有 Body 何时归还由回收记录决定。组装与 bootstrap 的消费者见 [`startup.md`](startup.md)。

## Job 管理面（`task/job.rs`）

Job 的创建域/管理域机制面（ABI 见 `shared/erhino_shared/src/proc.rs`）：

- **成员/子表**：有容量上限的OrderedTable以Pid/JobId索引事务占位或强对象。完整节点/付款owner由表所属Job Pool支付，在祖先门外先备无身份占位，门内复核seal/容量、取ID/token、改候选并插入。删除返完整owner，所有Job门外退款后才发布完成责任；两表最后占位回滚均重新检查封口完成。查找/插入/摘除O(log n)，枚举每批至多JOB_ENUMERATE_MAX。共用存储契约由[mm](mm.md#已付款保留存储)拥有。
- **JobId**：全局单调不复用分配器（root 恒 1，与 Pid 分立空间）；
  Pid/JobId 分配都在 owner Job 锁内与占位插入同临界区，表内 ID 序 =
  分配序（消除多核乱序分配窗口下的枚举漏项）。
- **创建/启动闸门**：JobCreate/ProcessCreate/ProcessStart（Attach/Grant 不上行检查，仅 Building 态准入）的「上行检查
  祖先 seal + 提交」在先父后子链锁（≤JOB_DEPTH_MAX(32) 把，短临界区）
  内线性化，与 JobSeal（持单锁）在 owner 锁上互斥，先到者定胜负；
  任一祖先 sealed → ObjectClosed；JobCreate 超深度 32 → IllegalArgument。
  祖先 weak 升级失败即「祖先已完成释放 ⟹ 曾 sealed」，同样 ObjectClosed。
  祖先Arc链以JOB_DEPTH_MAX固定Buffer准备、守卫数组按实际链长Buffer准备，两者由当前Job Pool按布局付款，不在门内分配。守卫析构先解全部链锁，再退数组charge，随后释放祖先链；Start/Create失败不留下临时付款。
- **JobSeal**：JobInner以Open/Sealing{cursor}/Waiting{token}/Dead拥有封口状态；首次Seal同锁置Sealing并发布出生内嵌Work，锁外kick。每步至多扫描一个直接child项，锁外对子调用seal/publish；不递归step。已受理ChildReservation提交时在父锁内读封口、门外让新child出生即seal，游标越过占位不丢责任。扫描结束但成员/child未空则同锁arm并保存WakeToken，最后依赖释放在门外wake。
  child完成先从父表摘完整节点并增closing_children，门外退款并发布child CLOSED，再回父锁减计数；父不得在该在途窗口发布Dead/CLOSED。完成Job无等待凭证，两表与在途计数均空；根无父表仍发布自身CLOSED。CLOSED保证父枚举已移除本Job。Job强持明确Pool/Account，节点、链临时存储与公开/私有宿主各按实际布局付款、按最后owner退款。root/child统一Remintable，Close只退authority，父表/root anchor与显式封口决定生命周期。
- **JobEnumerate**：游标分页（cursor = 上批最后返回条目 ID）；遇未决
  事务占位即终止本批（屏障：next_cursor 严格小于占位 ID，占位不计
  actual 但计入 more）；契约 `more=1 ⇒ actual ≥ 1 ∨ next_cursor ==
  入参 cursor`（零进展屏障，调用方以原 cursor 重试——占位窗口在
  创建方单个 syscall 内，协作式内核下有界完成，重试不活锁）。单批
  上界 JOB_ENUMERATE_MAX(128)，条目 8 字节 ID。
- **JobDerive**：按 ID 单目标派生（kind 0 = child JobControl，1 =
  member ProcessControl）；请求 rights ⊆ 源 Handle rights ∩ 目标角色
  allowed_rights，超集 RightsDenied；目标不在直接成员表（含已完成
  移表）ObjectNotFound。派生 ProcessControl 复用存活 shell（单一
  shell身份，电平不分叉）；内核直接复用表内ProcessShell。关闭最后child capability不会撤销父MANAGE的重新派生资格。内核负责后代封口传播与空子树自动收束；杀成员、预算与失败政策由用户态JobCollector拥有，仍用单一
  显式 frame stack 逐层 seal、分页枚举、派生 kill、Process 收束与 CLOSED
  观察；`job_kill` 是消费同一机器的同步门面。默认 policy 给出观察期限、
  重试次数与工作量上限；失败按值返还原 Job/Process 机器，保持阶段、
  快照和 authority，不默认 close。只有 DONE 已观察且 Query 核验 Dead
  后才关闭 ProcessControl；详细运行期连接见 [`runtime.md`](runtime.md)。

## 生命周期

[`ProcessShell`](../../os/kernel/src/task/process/shell.rs) 拥有 Building/Running/Terminating/Dead 与首次终因。首次冻结在 Shell gate 关闭外部准入，再锁外关闭 VM 入口并移交 active 快照、敲运行 hart 门铃，最后发布出生时的回收 WorkRecord；重复终止不覆盖终因。ProcessControl 是该 Shell 的 capability 身份，没有弱回指、重铸窗口或第二份终态快照。

构造 lease 覆盖 Job 出生提交至 Body 安装/回滚；Frozen 拒绝安装，构造者先锁外归还未发布 Box，再解除 lease。外部 Body 访问与 Ready 发布共享 ingress 计数；已登记 ThreadDeparture 每步取得短 receipt access，不跨等待。Shell 的 member_count 是未结回执真值，成员表的槽占用用于容量和身份，二者不互相代替。

自然最后成员回执结算（或最后预留撤销）在同一 Shell gate 检查 Running 且两计数均零，冻结 Completed。它与 Kill/fault 争同一首因；随后复用锁外 VM 封口与回收发布。空间退休仍等待独立的 VM lease、实际运行与结果回执屏障。

[`ProcessBuilder`](../../os/kernel/src/task/process/builder.rs) 的阶段为 Prepared/Pending/Starting/Publishing/Started/Aborted。Builder固定MANAGE|GRANT|TRANSIT，无DUPLICATE；Create铸造和类型上限使用所属module唯一常量。缓冲运输成功移动同一authority、失败返原owner，接收端可仅保留MANAGE。Pending最后authority消散以Aborted冻结Shell；已提交Start由Work继续持责任。Start 在 Job 链门确认未 seal、执行需求已绑定、有 Created 且无外部 Body 借用/成员预约，提交 Running 并关闭 Building VM 准入；成功后消费 Builder authority，独立内嵌 Work 接管。Work 通过 VM 的 arm/复检等待既有访问 lease（包括失败准备的回滚）清零，才开放 entry。Frozen 会取走 startup waiter 并唤醒该记录。

首个 Ready 发布前冻结成员槽终点与 tid 上界，每步只取一槽的旧 Created，经短 ingress 发布预付 ReadyBatch。已运行线程新建的 Pending 或复用槽的新 tid 不会被此批启动。Start 提交时把门内已接受的 BodyLease 转为不携带 Body 指针的 WorkReceipt，只保留已提交责任的 ingress；实际数据每步另借短 BodyLease。所有槽交接完或 Frozen 后，step 清 Shell 强根并结束执行；最后 Builder 引用消散，末字段先退宿主费、再归还 WorkReceipt、最后发布私有启动 DONE。因此 Frozen 不能在启动宿主实际退款前越过 WaitIngress。Start调用期间唯一authority由Invocation持有；未提交失败回Pending，由Invocation恢复/释放决定后续放弃，已提交启动由记录完成。

[`ThreadDeparture`](../../os/kernel/src/task/process/departure.rs) 是出生付款的独立 Work。reap/Created 放弃先标记离场责任，Thread 的末字段在现场、等待节点、Ready 资格销毁及宿主退款后才请求该记录。step 短借 Body 摘成员并归还访问，不提前解除 Shell 回执计数。Departure 最后引用消散时，末字段先退宿主费、发布 ThreadControl DONE，再 member_departed；父回收因此不能领先 Thread/Departure 私有费用。完成来源由回执直接强持，不依赖 Control 升级；未提交出生销毁只退款，不发布成员离场。VM/Start 不保留异步用户输出责任，访问租约和退款 owner 分别证明空间退休安全。

[`reclaim.rs`](../../os/kernel/src/task/process/reclaim.rs) 的阶段为 WaitIngress → SweepMembers/Ready → WaitShell → BeginRetire → ReleaseHandles → WaitVm → RetireSpace/WaitRetire → DropBody → Settle。成员扫描逐槽交出 Created/Waiting，Waiting 放弃赢得完成时调用 finish_offered；队列扫描每步只检查一个队首并锁外 reap。外部准入、已提交完成回执和运行计数均归零后，retiring 排除其他 Body 访问，回收 Work 才排空 cap 表并等 VM 访问租约。RetireSpace 从 Body 移出 RetireWork 执行根；父回收借 Shell 既有 SignalSource 的内部完成槽，不保活被等待宿主，也不提前公开业务 DONE。退休宿主最后退款后才 take/drop Body；BodyStorage 的字段/Box/费用完整释放后才发布 Dead/DONE、摘 Job 成员。Shell 与公开观察存储仍按真实存活期付费，来源不置通用 closed。VM 退休机制由 [`mm.md`](mm.md) 拥有。

`libthread::ResultCell<T>` 以独立状态字区分 Empty/Published/ReceiverGone；ResultProducer 是唯一写者，trampoline 在闭包完成后发布结果。PreparedThread 独占 Reserved 槽、packet、栈、Pending 票据与观察 Control；Start 成功消费票据并登记 Registry Live，失败返还 self。Drop 析构 packet、关闭票据，再把仍持 Control 的 record 交 Registry 等异步 DONE；Close 返回不是可回收栈的证明。reap 的收束顺序为 DONE → retire 环境残余 → 释放 Control → Unmap → Free；ObjectBusy 保留 record，ResultReceiver::Drop 只放弃结果，不等待。

用户态监督使用 Process DONE → Query 核验 → Close；Shell/Body/VM、等待源与真实退出消费者使用同一生命周期。独立宿主与节点已按 layout/实际容量付款；内部安装/取消/写回失败、资金拒绝/allocator OOM、迟到 token 及跨 hart 退款窗口的运行覆盖仍有边界。Account 最终析构计量与完整原子上界由对象链工作记录承接；代码存在或正常路线通过不替代完整资金验收。

### 锁序契约（Lock Ladder）

锁序由 Lock Ladder 运行时断言强制（`os/kernel/src/sync.rs` 的 `ranks`
表，debug 构建）：每把锁在构造点声明 rank，获取时断言 per-hart 秩栈
单调——新秩须大于栈顶，或同秩且链段 key 严格递增；违规 panic 同时报告
请求锁的源码位置（经 RawWriter，不依赖堆与锁）。release 构建零开销。
bootstrap 期（tp 未建立，单核）使用专用帧，formal entry 汇合点切换至
per-hart 帧。

秩与数值唯一真值在[sync::ranks](../../os/kernel/src/sync.rs)，下表按获取顺序列实际owner：

| rank | 锁 | 同秩key/边界 |
|---|---|---|
| DRAIN_GATE | 启动SupplyInputs仲裁 | 启动期输入 |
| DRAIN_CURSOR | Shell回收阶段 | 移出阶段后锁外推进 |
| HANDLE_TABLE | capability表 | caller→child按pid递增 |
| LEAF | console、registry、root、Ready、TimerQueue、Thread等待节点 | 不嵌套来源/目标回调 |
| JOB_INNER | Job链 | 父→子、jid递增，深度≤JOB_DEPTH_MAX |
| RESOURCE_TREE | 设备资源准入、封口和成员账本 | 单独短锁；VM、Pool、等待来源和 Work 发布均在门外 |
| PLIC | 控制器寄存器、IRQ 消费状态与内嵌观察链 | 不嵌套资源树、VM 或 WaitSet；所有 Hint/维护发布与退款在门外 |
| MAILBOX / CONNECTION | IPC状态 | 电平提示门外执行 |
| MEMORY_POOL | 页/metadata额度 | 父级退款逐锁、不嵌套Pool |
| MEMORY_OBJECT | 可执行状态/WritePermit | VM/Views门外取/还 |
| ADDRESS_SPACE | VmState、ledger/PTE与依赖索引 | — |
| VM_VIEWS | 实物View/来源 | VM→Views→Backing |
| BACKING | 物理段索引、分步取得与 root 退休 | root Work 发布与终态同门，允许 BACKING→MAINTENANCE；门铃与退款在全部外层门外 |
| NOTIFICATION | 通知电平 | space可为外层 |
| OBJECT_WAIT | 对象Source、Builder等状态 | 回调门外 |
| WAIT_SET | 持久集合 | 不嵌套来源锁 |
| LIFECYCLE | Shell准入/回执 | 不嵌套VM |
| PROCESS_MEMBERS | Body成员稳定槽 | Shell→Members |
| WORK_DEBT | Thread预付ReadyBatch槽 | 取出后入队 |
| REMOTE_CALL | 固定运输槽 | 发布可在VM门内，执行/回调门外 |
| MAINTENANCE | 维护队列/Record状态 | 零分配，不取其他锁；IPI在全部门外 |
| HEAP | talc | — |
| SYSTEM_SUPPLY | 预清零system ticket | HEAP→O(1)票据消费 |
| POOL | FramePool库存 | claim后门外清零 |

锁内只改本域状态/取待办，Frozen的VM封口与发布在Shell门外；对象提示/finish_offered、cap最后释放和业务close回调均在表/来源锁外。内部完成Source可在来源门内调用maintenance::wake提交队列状态：QUEUE门保护活跃执行根，归还必非最后引用；Idle/Done的潜在退休根由返回值带出全部领域门后销毁，kick亦在门外。VM/Backing/Members旧容量与完整PreparedEntry带出所有外层门后退款，不把Drop中的付款锁当隐含合法反向边。每把新锁在构造点声明rank；Job链和HandleTable同秩多持用chained的单调key。LockGuard显式!Send/!Sync，只能在原hart恢复IRQ与ladder；Lock<T>自身的共享能力仍按T:Send提供。

诊断输出也遵守该边界：持有 `CONNECTION`、`ADDRESS_SPACE` 或更高阶业务锁时不得调用
console/log；需要报告的 shootdown、attach/close 等诊断先保存不可变结果，释放业务锁后再执行
best-effort 输出。这样保留 Lock Ladder 的严格降序，不以放松 console rank 或吞掉日志错误解决违规。

### reserve/commit/rollback 协议

Job成员/子表与HandleTable槽位以非零单调token保护占位，枚举不越未决占位；每个身份域最大值只发行一次，不回绕。跨owner发布按reserve→锁外写结果→锁外必成资金/成员提交→表commit执行；Claimed authority由Invocation独占，失败restore或在表外release，业务accept/close不在表锁内调用。表被shut后，已经受理的必成责任继续交付，cap发布则返释放事件，由外层owner收束。Job预约回滚按token定位同锁摘完整节点，节点/charge在门外释放，封口Work依赖在退款后交回。KOID、PID/JID、空间与事务各自拥有单调allocator，不合并身份域。

## sleep

Sleep复用Thread等待续体与绝对Deadline；per-hart TimerQueue到期先摘节点、清owner，再锁外提示原Thread/epoch。执行owner在安全点probe并提交Timeout，来源/终止提前完成走同一detach注销timer。跨hart取消只删除匹配epoch，不远程编程timer；没有第二份WaitContext、token消费表或无主周期唤醒。
