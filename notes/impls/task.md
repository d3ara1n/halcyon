# 任务模型实现

> 任务是什么、进程与线程的分工等概念层见 [`../ideas/task.md`](../ideas/task.md)；本篇记录其在内核中的落地。线程持有 UserContext（用户现场，每线程一份）与调度状态；进程持有资源和从 ELF 得出的执行能力需求。完整上下文契约见 [`execution-context.md`](execution-context.md)。

## 进程执行环境

进程持有 AddressSpace 与 HandleTable；用户半区映像、启动布局终点、首线程栈和 ASID 由 [`mm.md`](mm.md) 唯一记录。Start 提交点把 ELF 判定的 `IsaRequirement` 与兼容 `SchedDomain` 合成一次性的非零 execution binding ID；需求与域从同一个 `AtomicUsize` 解码，未绑定、部分冻结与 Base64/零哨兵重合均不可表示。首线程出生现场：sp 为组装者供给的栈顶（16 字节对齐），a0/a1 为出生块基址与长度（组装者经 ProcessAttach 的 arg1/arg2 传入）；用户 tp 当前置零。Running 期堆由 rinlib 的匿名 `MappedRegion` arena 组成，任务层不持连续堆顶。

## 调度：域—类—执行点三层组合

```
执行点（每 hart 一份，HartLocal）  调度域（共享，boot 冻结）      调度类（策略容器）
┌─────────────────────────┐  ┌──────────────────────┐  ┌─────────────────────────┐
│ owner: AdmittedThread   │  │ SchedDomain           │◀─│ trait SchedClass         │
│ （调度循环 + idle 循环）  │  │  classes: 优先级序数组 │  │ enqueue / pick /         │
│ 域归属经 per-slot 域表   │─▶│  idle_mask: 域空闲位图 │  │ has_ready / reserve /    │
└─────────────────────────┘  └──────────────────────┘  │ publish_batch            │
                                                       └─────────────────────────┘
```

- **执行点**：hart 的运行现场（当前线程、trap 锚），见 `internals.md`「tp 寄存器」。调度循环与 idle 循环是执行点的行为。
- **调度域**：`SchedDomain` 持有优先级序的调度类数组与域内 `idle_mask`。线程经 `process.domain()` 只进入已绑定域的类队列，wake 只向该域 idle hart 发门铃，静默谓词遍历全部域。硬件能力、域划分、D64 eligibility 与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一记录。
- **调度类**：公平类以 LEAF 锁保护 `os/ready_queue::ReadyQueue<Arc<Thread>>`，通过 `SchedClass` 实现 enqueue/pick/has_ready 与 reserve_batch/publish_batch。`SchedDomain::reserve_ready` 返回绑定实际类存储的 `ReadyBatch`；Start、Bootstrap、ThreadSpawn 都在不可逆点前取得整批准入，失败不产生前缀发布。

`ready_queue::Admission` 支付整个可调度寿命的存储，未交付 credit 随 Drop 原子退还。`AdmittedThread = Admitted<Arc<Thread>>` 不可 Clone，由值与一个同源 credit 组成；队列只接受该 owner。Running/Waiting 保留 credit，进入 Ready 不重新分配；取消或最终离场才归还。纯逻辑队列以保活的容量 core 校验来源，不使用整数 token 或 marker，取消不反取队列锁，pick 不扫描出生预留。

容量不变量为 `storage.capacity ≥ outstanding ≥ ready.len`，outstanding 含所有已准入线程与尚未交付的出生 credit。独占 reserve 先按全部 outstanding 与本次数量检查溢出并预留实际 VecDeque，成功才增加计数；并发方只能原子退款，快照至多保守多留。队列存储由调度类长期持有并复用高水位，credit 只表示可调度位置责任，不是 CPU 配额或独立 metadata 预算。整批 publish 的工作量随本次线程数变化，不扫描其它 Ready 存量。

`os/ready_queue/tests/admission.rs` 覆盖 OOM、部分取消、错误队列、等待者与新出生并存、FIFO 模型及持 Ready 锁时的远端退款；allocator 计数探针要求发布、轮转、唤醒和退款不发生分配。

时间片为固定量子，tickless：调度循环每次新 dispatch 前调用 `arm_quantum`，Resume 热路径不重置量子；同时取本 hart TimerQueue 堆顶与量子截止的较近者设置 timer。公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。

ProcessWrite 可由其它 hart 通过物理直映射填充可执行帧。调度循环先经 `synchronize_local` 和 lifecycle execution gate 复检 AddressSpace 的 translation/instruction epoch，再进入用户态；指令流同步由 AddressSpace 事务从真实 Install/Protect 意图派生，并由远端请求或 `_ret_to_user` 的统一地址空间切换出口执行，调度器不再重复发出 `fence.i`。epoch 与 active 确认协议见 [`mm.md`](mm.md)。

### 单一归属不变量

任意线程任意时刻恰处于一个稳定容器：

```
调度类队列（Ready） ｜ hart current（Running） ｜ WaitContext（Waiting）
```

lifecycle 成员表记录 `Staging / Spawning / Ready / Running / Waiting / Exiting`。Staging 是 Building 期预育形态：条目携带线程强引用，内嵌 bootstrap Attach 在无并发条件下由 `attach_member` 锁内分配 tid、构造并插入；syscall Attach 则凭已登记 lease 进入 `attach_registered_member`，若终止已在登记后截止，提交仍成功但新线程不再入容器，而是作为终止接管资源在 lifecycle 锁外直接析构。Start 由 `begin_running` 同一临界区整体把现存 Staging 转 Ready 并提取全部强引用。Spawning 是 Running 期 ThreadSpawn 的提交中间态：调用先预留 ThreadControl Handle 与目标域全寿命调度准入，再在 lifecycle 锁内校验 Running、分配 tid 并插入；输出成功后不可失败地提交 Handle、Spawning→Ready 与调度 owner，输出失败则完整回滚。终止路径不摘尚未完成提交的 Spawning，待提交尾段完成后按普通成员收束。Exiting 表示终止路径已取得离场所有权。线程最终离场即从成员表摘除，不保留 Dead 记录。tid 从 1 起单调不复用，0 是非身份值；稳定 `MemberKey { slot, generation, tid }` 是内核唯一线程身份，`Thread` 不重复保存 tid。slot 可以复用，generation 防止旧 departure 或等待凭据错指新成员，tid 负责 ABI 输出与诊断。并发成员数硬界为 1024。容器成员资格是真值；Waiting 完成后先经 `sched::enqueue` 发布 Ready，lifecycle 记录由下一次 `enter_running` 收编。timer queue 与类队列均为 Lock Ladder LEAF 锁。

### 等待的所有权与仲裁

- **执行 owner 随容器走**：`AdmittedThread` 由就绪队列、执行点调度循环或等待条目唯一拥有；临时底层 Arc 借用/保活不复制调度准入。等待安装整体移交 owner，不克隆可运行责任。lifecycle 的 Waiting 记录只持 weak WaitContext（触达取消用），在 park 发布时于 lifecycle 锁内线性化。Commit 前预构造的内核事务 Context 不含 Thread，线程离开执行点后才移入。
- **发布时序**：「可被唤醒」严格晚于「离开 hart 执行点」——dispatcher 把 WaitPlan 移入 `sched::HART_WAIT_PLANS` 的固定内联槽，无装箱；槽按 hart slot 寻址，以 LEAF 锁保护，不进入 HartLocal 的 trap ABI。调度循环在 `clear_context` 后的每个 Switch 出口统一取走意图：Park 在 active 确认后交给 WaitContext；Killed 在 active 确认后放弃未安装计划，再推进 departure；Requeue 要求无意图。这样终止在 trap 尾段吸收 Park 时也不会遗留责任。预构造 Context 在安装前可以接受 Deferred outcome，但不能取得完成权或触达线程。
- **完成仲裁**：对象命中、Timeout、错误与终止取消竞争唯一 outcome；任务层只依赖“赢家取得线程所有权并负责离场”这一结果。WaitCore、timer token、rejected-park 竞态与订阅清理由 [`ipc.md`](ipc.md) 唯一记录。

## Job、Building process 与发布

root Job 由内核 static anchor 强持（所有权图：anchor ─strong→ root Job，
parent ─strong→ children，child ─weak→ parent；Job 直接成员表 ─strong→
未 Dead 的 Process cores，Process ─weak→ Job）。JobCreate 派生层级；
ProcessCreate 必须持 CREATE，只生成稳定身份、空 HandleTable、`AddressSpaceState::Unbound`、affine ProcessBuilder 与从 Building 起即存在的 ProcessControl；它不创建 ledger/root page table，也不取得 MemoryPool charge。Process core 的 sponsor slot 与 `ProcessResources` 同寿命，Builder/Control 各自持有 sponsor+global 类型化 permit：Builder 随最后 authority 消散退款，Control permit 随最后终态观察壳消散，core Dead 不会替仍存壳提前退款。创建事务在提交点把 Building shell 插入 Job 直接成员表（对 Seal/枚举可见，输出失败/回滚不遗留成员）。`ProcessBindMemory(builder, pool)` 以 Builder MANAGE 与 Pool GRANT 为 authority，在 Building operation lease 下 pin 两项、串行化同一 shell 的竞争 Bind，并在锁外准备 AddressSpace metadata permit、单页 funded root 与完整页表；`Unbound → Bound` 发布和 Pool entry 逻辑消费在同一 HandleTable→AddressSpace 提交段完成，失败保留 Pool Handle 并自然回滚全部 owner。内核没有全局进程表：单调 PID 分配器只分配身份，未 Dead core 的生命周期根是 Job 成员表。用户态组装序列为 ProcessBindMemory → ProcessMap/Write → ProcessGrant → ProcessAttach → ProcessStart（线程是组装资源，完整事务见 [`startup.md`](startup.md)），Start 按预育成员数在目标调度类原子预留完整 Ready 批次。
每个 Job 在创建时冻结 jid/parent_jid 不可变字段（Dead 后父对象可先
释放，快照仍可应答）。

ProcessStart 负责 `Building → Running` 首次发布（readiness 要求 AddressSpace 已 Bound，活体门要求预育表非空；含预育原子提取），并与 Job seal/termination 在 lifecycle 提交点竞争；成功提交一次冻结 execution binding，完整 capability/Ready 顺序与回滚事务由 [`startup.md`](startup.md) 唯一记录。所有 Building 操作只在精确 Building 登记 lease；登记即冻结其提交资格，终止等待已有 lease，Attach 在截止后把已登记提交直接交给终止接管，Start 自身只有在 `building_ops == 1` 时才能越过截止，不能跨过并发 Bind/Map/Write/Grant/Attach。PID 单调不复用，`parent_pid` 只供诊断，授权仅来自 Job/Process capabilities。

ProcessControl 贯穿 Building/Running/Terminating/Dead 保持同一对象身份
（HandleTable 条目强持 shell，shell ─weak→ core）；关闭 control 只消散
authority。固定宽 ProcessQuery、异步幂等 ProcessKill、REAPABLE 电平
已接入；Dead 后 shell 冻结终态快照持续可查。执行需求与域绑定见
[`execution-context.md`](execution-context.md)。

## Job 管理面（`task/job.rs`）

Job 的创建域/管理域机制面（ABI 见 `shared/src/proc.rs`）：

- **成员/子表**：`os/ordered_table` 提供有容量上限的 fallible AVL（键为 Pid/JobId，条目为事务占位或强持对象）。创建期以 `PreparedEntry` 在锁外预分配节点，提交/删除不分配；查找、插入、摘除为 O(log n)，不在终止或完成路径做宽度 memmove。枚举按事务屏障分页扫描，单批至多 `JOB_ENUMERATE_MAX` 项。
- **JobId**：全局单调不复用分配器（root 恒 1，与 Pid 分立空间）；
  Pid/JobId 分配都在 owner Job 锁内与占位插入同临界区，表内 ID 序 =
  分配序（消除多核乱序分配窗口下的枚举漏项）。
- **创建/启动闸门**：JobCreate/ProcessCreate/ProcessStart（Attach/Grant 不上行检查，仅 Building 态准入）的「上行检查
  祖先 seal + 提交」在先父后子链锁（≤JOB_DEPTH_MAX(32) 把，短临界区）
  内线性化，与 JobSeal（持单锁）在 owner 锁上互斥，先到者定胜负；
  任一祖先 sealed → ObjectClosed；JobCreate 超深度 32 → IllegalArgument。
  祖先 weak 升级失败即「祖先已完成释放 ⟹ 曾 sealed」，同样 ObjectClosed。
- **JobSeal**：O(1) 置位幂等，不扫表；Job 无收束工作，完成 = 自身
  sealed ∧ 两表空，完成即置 dead 并发布 JobControl 的 CLOSED（等待
  CLOSED 即「直接成员全部完成」屏障）。触发点三处（seal 时已空/成员
  摘除后空/child 完成后空），事件驱动自底向上传播：逐级「放子锁、
  取父锁」从父表移除并再判定，单步有界；root 完成发 CLOSED 但不从
  任何表移除不释放。Dead 后两表必空（完成不变量即冻结），JobQuery
  计数自然为零，快照的 jid/parent_jid 来自不可变字段。
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
  shell 身份，电平不分叉）；shell 已消散时从 core 铸造新 shell 并在
  铸造点重放 REAPABLE 或 CLOSED——control 消散的进程由此接回管理
  入口（派生兑底）。递归 JobKill 是用户态政策，
  公共实现 `libprocess::job_kill`（逐层 seal → 有限 stall 枚举 → 派生 kill →
  有限 wait/drain/query → 等 CLOSED）。默认 policy 固定单次 wait timeout、
  wait/drain/query 次数、单次 drain work 与 enumerate stall 上限；失败返回
  Job/Process authority、阶段与进度，不默认 close。`collect_process` 的
  `SupervisionTarget` 只在 Drain Complete 且 Query 核验 Dead 后关闭 control。

## 生命周期
- **创建**：唯一 init 由内核从 BootPackage initial ELF 构造（内嵌与
  用户态同构的组装序列：Map/Write → 出生块 → Attach → Start，仅 payload
  收编为 bootstrap 特例，无结构特例）；后续进程由用户态 `libprocess`
  驱动 ProcessBuilder 组装。内核不解析 initfs 或服务拓扑。
- **状态机**：`Building → Running → Terminating → Dead`，真值在
  Process 内嵌 lifecycle（原子 state 快读 + 顶级锁保护终因/线程成员
  表/active 位图）。Exit、fault、ProcessKill 与 Building abandonment 在
  各自适用状态竞争首次终止线性化点冻结终因（reason + i64 code），
  后续事件幂等不覆盖；fault 经稳定 ProcessFaultCode 编码，不固化裸
  scause。线程成员表使用稳定 slot/generation 身份（离场即摘除、表空即无线程），
  是线程容器唯一真值：pick 后 trap 入口统一检查 Terminating（惰性撤销），
  enqueue 无条件入队不反向触碰 lifecycle 锁。每个 Process 出生时预付一枚
  termination debt；首次终止固定成本发布 IPI 与 continuation，后者每个 work
  unit 只检查一个稳定成员槽，Waiting 逐项 offer(Abandoned)、Staging 逐项锁外
  释放，空槽扫描同样计费。单 outcome 仲裁与自然完成方无双重处置；对
  唤醒后未再调度的 stale Waiting 记录 offer 必然落败，由 pick gate
  吸收后 reap 摘除。Running 由终止待办向冻结时刻的 active 位图
  快照发 IPI（冻结后 enter_running 拒绝，位只减不增），目标在任意
  trap 入口吸收为 Killed。active 与 Running/Terminating 准入每次变化都推进
  execution sequence，地址空间事务 Reserve 快照 `(sequence, active)`，Commit 在
  `ADDRESS_SPACE → LIFECYCLE` 锁序下拒绝同值 ABA；已经 Commit、终止不可撤销的事务
  同时增加 `mandatory_ops`，只有业务 Complete 后才递减。dispatch/leave 以本 hart
  已确认的 AddressSpace epoch 作为登记/清除 active 的硬 gate。自杀路径排除本 hart。
  ThreadExit 只结束当前线程；末线程以首次终止线性化点冻结进程 Exited 终因。ThreadYield 以 Requeue outcome 在完整 syscall 边界重新排队。ThreadSpawn 仅接受当前 Running 线程 authority 与固定宽 `ThreadStartContext`/`ThreadSpawnResult`，内核铸造 waitable ThreadControl。每次离场由 `ThreadDeparture` 在执行容器释放后摘除成员；若发起线程仍挂有 committed Map 结果义务，则延迟摘除和 DONE 发布，义务归零后再继续。ThreadControl close 只消散观察壳，不影响线程 core；等待面只允许真实可达的 DONE，不暴露从不发布的 CLOSED。join 不是 syscall：rinlib 以 WaitMany(DONE) + HandleClose 组合，并在 Acquire 后接管结果与用户栈。未实现的 ThreadKill 不在 syscall 枚举中占号。
- **退出收束**（有界分批，管理者驱动）：trap 汇编非-Resume 出口统一
  先切内核 satp（含全量 SFENCE.VMA）再交回 Rust——出口边界一处承担，
  终止来源无需各自记得归一（见 [execution-context.md](execution-context.md)
  「地址空间归属纪律」）；reap 先 drop 线程强引用再做离场确认。REAPABLE 是
  `members 为空 && active == 0 && building_ops == 0 && mandatory_ops == 0` 的持续电平：
  线程全部离场但 Remote completion 尚未收束时不会提前发布。
  线程终止清理只推进到 REAPABLE；完整资源回收仍由管理者提交 ProcessDrain 批次。
  `DrainRequest` 捕获目标、输出和剩余预算，经可复用 WaitContext 在依赖未完成时挂起；
  关闭回复或调用者退出不撤销已经启动的对象退休，但不会自动提交剩余 Process 全程回收。
  Dead 在 drain 的 PublishDead 阶段发布，不等于该批已返回 Complete。HandleTable 先逐槽扫描摘项
  （take_next_bounded 硬预算），扫描与 close 各计一个 work unit；预算恰在
  摘项后耗尽时 entry 存入 Process `pending_close`，下一批优先在表锁外消费。
  REAPABLE 后 Tunnel detached close 只提交无失败逻辑关闭，不再创建 MemoryChange
  或取得 funding，因此没有 callback retry 分支。任意非零预算返回 More 时都有
  正进展。Handle 完成后 AddressSpace 先逐 fragment
  丢弃不可达 ledger，再逐 extent 从 `OwnedBacking` 摘下并经 `pending_free` 归还
  order 树；随后按 owned/shared 槽真值收束 L0/L1 与 root 表帧。Pool-backed
  bootstrap extent 与最终 PoolBinding 只在 AddressSpace 锁内摘除并计费，实际
  physical/charge owner 析构由调用层在锁外完成，遵守 MEMORY_POOL → ADDRESS_SPACE
  的 Lock Ladder。预算分别计费 close 尝试、ledger fragment、extent 摘取/归还与页表槽检查/摘除；单次 order 树操作另有只依赖地址位宽与 DT memory region 上限的结构常数界，批次执行量受 budget 线性约束。
  完成时发布序固定：shell 先冻结终态快照并置 CLOSED（原子清 REAPABLE，外部无
  Dead+REAPABLE 混合视图）→ core 内部置 Dead → Job 成员表摘除（core 仅剩
  空壳）。PublishDead 前先发布出生预付的 Finalization 独立强根，后续祖先传播和 Done
  可以跨预算推进；该终段不会因 caller/control 或 Job 成员根消散而丢失。批次以
  drain_active 取得全寿命许可，drain_gate 串行每次实际推进；并发批次返回 ObjectBusy。
  Drain 进度存目标进程（handle 游标/pending close + 地址空间阶段游标 + 待归还 extent），
  持有可恢复监督 authority 的管理者可以接管。这是可恢复的管理者驱动，不是终止后的自动回收。init 持久保留服务 control，并按负载阶段监督：高峰竞态矩阵前先查询并收束已进入 Terminating/Dead 的短寿命服务，释放其 AddressSpace；仍处于 Building/Running 的成员留在集合，末尾再统一 WaitMany(REAPABLE|CLOSED) → Drain 至 Complete → 终态快照。对象 close 回调（如隧道 PEER_CLOSED）发生在 Drain 期间，用户态等待序必须先监督后观察终态位。
- **当前监督与资助**：`srv_init::launch_test_services` 通过 `SpawnRequest.memory_pool = root_memory_pool()` 为服务及委托域靶提供同一来源；`libprocess::spawn` 复制 GRANT-only Pool authority 后交 ProcessBindMemory，没有自动派生每个子进程的固定额度。pm 获得不含 CREATE 的委托 JobControl，init 保留独立域 control 兜底；该授权没有绑定独立页池。`ProcessResources::try_new` 为新进程从全局 admission 建立 MetadataSponsor，未消费父进程的可委派 metadata 预算。页 charge 退回其来源 Pool，metadata permit 退回原 sponsor/global counter，均不因执行 Drain 的进程而改记。当前依靠可信 init/pm 的显式监督政策，不能声称已建立每个管理域的独立资助与回收激励；`max_work` 只是批次工作界限，deferred work 也没有按资助者归账的 CPU 预约计费。
- **创建/启动事务**：ProcessCreate 先锁定 Job 成员 marker并预留 caller Handle 槽；输出写入后先形成 `HandleTable::PreparedCommit`，再在 `HANDLE_TABLE → JOB_INNER` 临界区把 capability 与成员同时发布。Bootstrap 采用同一 typed commit，并在锁区内继续提交 lifecycle Running 与 execution binding；所有可恢复失败都在此之前。JobCreate 同构保留 child marker 与预留槽协议。ProcessStart 事务见 [`startup.md`](startup.md)。
- **对象关闭**：叶 role 在 Handle 摘出后、表锁外执行有界 callback；容器通过退休后端提交独立执行者，ProcessDrain 保存 PendingClose::Retirement 完成 ticket，不执行对象内部扫描。尚未启动关闭的摘出项保存为 PendingClose::Entry，仍等下一批推进。具体关闭与准入契约见 [`ipc.md`](ipc.md)，不能把所有关闭都当成同步 callback。
- **用户态页故障一律杀进程**：本内核无按需分配，所有区域创建时显式
  映射，fault 即程序缺陷。打印诊断行（pid / sepc / 故障地址 / 操作）
  后走终止路径，绝不 panic 内核。

### 锁序契约（Lock Ladder）

锁序由 Lock Ladder 运行时断言强制（`os/kernel/src/sync.rs` 的 `ranks`
表，debug 构建）：每把锁在构造点声明 rank，获取时断言 per-hart 秩栈
单调——新秩须大于栈顶，或同秩且链段 key 严格递增；违规 panic 同时报告
请求锁的源码位置（经 RawWriter，不依赖堆与锁）。release 构建零开销。
bootstrap 期（tp 未建立，单核）使用专用帧，formal entry 汇合点切换至
per-hart 帧。

秩分配（数字唯一真值在 `sync::ranks`，此处列序即序）：

| rank | 锁 | 链段 key |
|---|---|---|
| DRAIN_GATE | 收束批次仲裁，一次性覆盖最广，恒最先 | — |
| DRAIN_CURSOR | HandleTable 收束游标与 pending close | — |
| HANDLE_TABLE | caller→child 嵌套 | pid 递增 |
| LEAF | CONSOLE、REGISTRY、ROOT anchor、各域就绪队列、per-hart TimerQueue、WaitContext 两锁；Remote Reserve 所需 admitted mask 已是安装后不可变的原子快照，不进入该锁 | — |
| JOB_INNER | Job 链锁（≤32 把同持） | jid 递增 |
| MAILBOX / CONNECTION | IPC 对象状态锁 | — |
| MEMORY_OBJECT | MemoryObject 可执行状态与 affine WritePermit 账目；permit 在进入 AddressSpace 前移出此锁，Retire 也在 AddressSpace 锁外归还 | — |
| ADDRESS_SPACE | 用户地址空间 | — |
| NOTIFICATION | 唯一以 space 为外层的对象锁边 | — |
| OBJECT_WAIT | Job.wait、ProcessControl、Endpoint、ProcessBuilder、Process.control 回指槽 | — |
| LIFECYCLE | 生命周期顶级锁（从不出游；被链锁/对象壳在锁内进入） | — |
| MEMORY_COMPLETION | Commit gate 内填充一次的 PublishedChange 槽、Thread 输出失败终止待办槽 | — |
| REMOTE_CALL | 固定 hart 请求槽；只在 AddressSpace/Lifecycle Commit 内短发布 | — |
| HEAP | talc（RankedRawSpinlock 类型级注入；几乎被全部容器锁内获取，故置顶） | — |
| POOL | 物理帧池（HEAP 与空间锁的内层） | — |

三类锁的共同纪律「锁内不出游」不变：lifecycle 的出游动作经 TerminationTodo
解锁后执行；对象电平发布经 `take_completer` 在锁外 `finish_offered`；close 回调在表锁释放
后执行；drain 完成分支显式先释放 drain_gate 再让 process 强引用
消亡（close 回调链不进 gate 持有区）。新增锁在构造点声明 rank 即受
断言保护；需要同秩多持的锁用 `Spinlock::chained` 声明单调 key
（Job 链锁 = jid、HandleTable 嵌套 = pid）。

### reserve/commit/rollback 协议

Job 成员表/子表与 HandleTable 槽位的 marker 事务遵循同一协议四要素：①占位条目对查找/枚举不可见；②各 identity domain 的非零单调 token
凭据防错认，最大值发行后永久 Exhausted，不回绕；③commit/rollback 按 token 定位，结构性不可消失；HandleTable 跨 owner 发布先经
`prepare_commit` 形成私有字段的 affine token，最终 `commit_prepared` 不返回可恢复错误；
④marker 的提交/回滚全部在容器锁内完成，无分配失败路径。`attach_member` 的插入是另一类锁内
try_reserve 原子操作，失败无副作用，以“失败时条目不可见”闭合。KOID、PID/JID、AddressSpace 与事务 token 共用 `os/monotonic_id` 的耗尽机制，但各自持独立 allocator，不合并身份域；用户可达构造在发布前返回 ReachLimit。出生块由组装者经 Write 交付，无内核回滚面。

## sleep

Sleep 复用 WaitContext：ms > 0 时换算为单调 `expires_at`，线程转 Waiting；per-hart `TimerQueue` 到期弹出稳定 token并竞争 Timeout outcome，Sleep action 写回成功后 enqueue。发起 hart 是 timeout owner；对象/终止提前完成会按 token 从 owner queue 注销。跨 hart 注销只删除队列项，不远程重编程 timer。
