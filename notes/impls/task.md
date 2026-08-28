# 任务模型实现

> 任务是什么、进程与线程的分工等概念层见 [`../ideas/task.md`](../ideas/task.md)；本篇记录其在内核中的落地。线程持有 UserContext（用户现场，每线程一份）与调度状态；进程持有资源和从 ELF 得出的执行能力需求。完整上下文契约见 [`execution-context.md`](execution-context.md)。

## 进程执行环境

进程持有 AddressSpace 与 HandleTable；用户半区布局、brk、StartupBlock、主栈和 ASID 由 [`mm.md`](mm.md) 唯一记录。任务侧在创建主线程时设置 sp 为用户半区顶（16 字节对齐），a0/a1 为 StartupBlock 基址与长度；用户 tp 当前置零。

## 调度：域—类—执行点三层组合

```
执行点（每 hart 一份，HartLocal）  调度域（共享，boot 冻结）      调度类（策略容器）
┌─────────────────────────┐  ┌──────────────────────┐  ┌─────────────────────────┐
│ current: Option<Thread> │  │ SchedDomain           │◀─│ trait SchedClass         │
│ （调度循环 + idle 循环）  │  │  classes: 优先级序数组 │  │ enqueue / pick /         │
│ 域归属经 per-slot 域表   │─▶│  idle_mask: 域空闲位图 │  │ has_ready / reserve /    │
└─────────────────────────┘  └──────────────────────┘  │ commit / rollback        │
                                                       └─────────────────────────┘
```

- **执行点**：hart 的运行现场（当前线程、trap 锚），见 `internals.md`「tp 寄存器」。调度循环与 idle 循环是执行点的行为。
- **调度域**：`SchedDomain` 持有优先级序的调度类数组与域内 `idle_mask`。线程经 `process.domain()` 只进入已绑定域的类队列，wake 只向该域 idle hart 发门铃，静默谓词遍历全部域。硬件能力、域划分、D64 eligibility 与绑定冻结由 [`execution-context.md`](execution-context.md) 唯一记录。
- **调度类**：当前公平类以单锁 FIFO 保存 Ready 线程，并通过 `SchedClass` trait 实现 enqueue/pick/has_ready/reserve/commit/rollback；Ready 容量在目标容器提交前预留。

时间片为固定量子，tickless：调度循环每次新 dispatch 前调用 `arm_quantum`，Resume 热路径不重置量子；同时取本 hart TimerQueue 堆顶与量子截止的较近者设置 timer。公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。

ProcessWrite 可由其他 hart 通过物理直映射填充新可执行帧。lifecycle 已维护 active-hart bitmap，当前用于终止屏障；尚未建立代码代次。调度循环在每次新 dispatch 前执行本 hart `fence.i`，首次执行和迁移都不会观察帧复用前的旧指令缓存。Running process 没有 Building 写入口，Resume 热路径无需重复同步。

### 单一归属不变量

任意线程任意时刻恰处于一个稳定容器：

```
调度类队列（Ready） ｜ hart current（Running） ｜ WaitContext（Waiting）
```

lifecycle 成员表记录 `Ready / Staging / Running / Waiting / Exiting`：Staging 表示 Start 已线性化但尚未提交 Ready，Exiting 表示终止路径已取得离场所有权。线程最终离场即从成员表摘除，不保留 Dead 记录。容器成员资格是真值；Waiting 完成后先经 `sched::enqueue` 发布 Ready，lifecycle 记录由下一次 `enter_running` 收编。timer queue 与类队列均为 Lock Ladder LEAF 锁。

### 等待的所有权与仲裁

- **强引用随容器走**：线程的 Arc 恰由其所在容器持有——就绪队列、执行点调度循环、或等待条目。等待条目强持有等待中的线程；不存在从容器反向到线程的长期指针（进程不回指线程），退出回收的 Drop 链因此能真正释放帧。lifecycle 的 Waiting 记录只持 weak WaitContext（触达取消用），在 park 发布时于 lifecycle 锁内线性化。
- **发布时序**：「可被唤醒」严格晚于「离开一切 hart 引用」——dispatcher 只把等待意图写入 HartLocal 私有槽，调度循环在 `clear_context` 之后的 Park 分支才向全局等待结构发布。完成方永远见不到仍在本 hart 执行的线程，双容器竞态在结构上不可能。
- **完成仲裁**：对象命中、Timeout、错误与终止取消竞争唯一 outcome；任务层只依赖“赢家取得线程所有权并负责离场”这一结果。WaitCore、timer token、rejected-park 竞态与订阅清理由 [`ipc.md`](ipc.md) 唯一记录。

## Job、Building process 与发布

root Job 由内核 static anchor 强持（所有权图：anchor ─strong→ root Job，
parent ─strong→ children，child ─weak→ parent；Job 直接成员表 ─strong→
未 Dead 的 Process cores，Process ─weak→ Job）。JobCreate 派生层级；
ProcessCreate 必须持 CREATE，生成空 AddressSpace、HandleTable、affine
ProcessBuilder 与从 Building 起即存在的 ProcessControl，并在事务提交点
把 Building process 插入 Job 直接成员表（对 Seal/枚举可见，输出失败/
回滚不遗留成员）。内核没有全局进程表：单调 PID 分配器只分配身份，
未 Dead core 的生命周期根是 Job 成员表。ProcessCreate 使用成员占位，ProcessStart 另在目标调度类使用 Ready reservation。
每个 Job 在创建时冻结 jid/parent_jid 不可变字段（Dead 后父对象可先
释放，快照仍可应答）。

ProcessStart 负责 `Building → Running` 首次发布，并与 Job seal/termination 在 lifecycle 提交点竞争；完整 pin、grant 顺序与回滚事务由 [`startup.md`](startup.md) 唯一记录。PID 单调不复用，`parent_pid` 只供诊断，授权仅来自 Job/Process capabilities。

ProcessControl 贯穿 Building/Running/Terminating/Dead 保持同一对象身份
（HandleTable 条目强持 shell，shell ─weak→ core）；关闭 control 只消散
authority。固定宽 ProcessQuery、异步幂等 ProcessKill、REAPABLE 电平
已接入；Dead 后 shell 冻结终态快照持续可查。执行需求与域绑定见
[`execution-context.md`](execution-context.md)。

## Job 管理面（`task/job.rs`）

Job 的创建域/管理域机制面（ABI 见 `shared/src/proc.rs`）：

- **成员/子表**：按 ID 有序的 fallible 结构（首版有序 Vec + try_reserve +
  二分定位，键为 Pid/JobId，条目为事务占位或强持对象）。枚举自
  partition_point 连续取，单批 O(log n + N) 固定上界；插入/删除的
  O(width) memmove 只在创建路径，不在终止/完成短路径。
- **JobId**：全局单调不复用分配器（root 恒 1，与 Pid 分立空间）；
  Pid/JobId 分配都在 owner Job 锁内与占位插入同临界区，表内 ID 序 =
  分配序（消除多核乱序分配窗口下的枚举漏项）。
- **创建/启动闸门**：JobCreate/ProcessCreate/ProcessStart 的「上行检查
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
  公共实现 `libprocess::job_kill`（逐层 seal → 枚举 → 派生 kill →
  drain → 等 CLOSED）。

## 生命周期
- **创建**：唯一 init 由内核从 BootPackage initial ELF 构造（同样获得
  Building 起存在的 ProcessControl，无结构特例）；后续进程由用户态
  `libprocess` 驱动 ProcessBuilder 映射、回填并发布。内核不解析 initfs
  或服务拓扑。
- **状态机**：`Building → Running → Terminating → Dead`，真值在
  Process 内嵌 lifecycle（原子 state 快读 + 顶级锁保护终因/线程成员
  表/active 位图）。Exit、fault、ProcessKill 与 Building abandonment 在
  各自适用状态竞争首次终止线性化点冻结终因（reason + i64 code），
  后续事件幂等不覆盖；fault 经稳定 ProcessFaultCode 编码，不固化裸
  scause。线程成员表（按 tid 升序的有序 fallible Vec，离场即摘除、
  表空即无线程）是线程容器唯一真值：pick 后 trap 入口统一检查
  Terminating（惰性撤销），enqueue 无条件入队不反向触碰 lifecycle
  锁；Waiting 由终止路径经锁外游标逐条 offer(Abandoned)（每次只持
  一个 weak、零分配，单 outcome 仲裁与自然完成方无双重处置；对
  唤醒后未再调度的 stale Waiting 记录 offer 必然落败，由 pick gate
  吸收后 reap 摘除）；Running 由终止待办向冻结时刻的 active 位图
  快照发 IPI（冻结后 enter_running 拒绝，位只减不增），目标在任意
  trap 入口吸收为 Killed；自杀路径排除本 hart。ThreadSpawn/ThreadExit
  调用号当前返回 FunctionNotAvailable，现有 Exit 是进程级终止。
- **退出收束**（有界分批，管理者驱动）：trap 汇编非-Resume 出口统一
  先切内核 satp（含全量 SFENCE.VMA）再交回 Rust——出口边界一处承担，
  终止来源无需各自记得归一（见 [execution-context.md](execution-context.md)
  「地址空间归属纪律」）；reap 先 drop 线程强引用再做离场确认
  （thread_departed 摘成员 → REAPABLE 持续电平）。
  任何容器路径都只到达 REAPABLE；Dead 仅由 ProcessDrain 的 Complete
  分支发布：HandleTable 先逐槽扫描摘项（take_next_bounded 硬预算），
  扫描与 close callback 各计一个 work unit；预算恰在摘项后耗尽时，
  entry 存入 Process `pending_close`，下一批优先在表锁外关闭。因此任意
  非零预算返回 More 时都有正进展。Handle 完成后 AddressSpace 分阶段：
  数据帧 tracker 逐个经帧池有界归还（FreeScan
  游标可恢复、O(1) 校验重启），页表 L0/L1 表帧逐槽登记归还，root 帧
  经 leak_root 交出后单独走 RootFree 阶段有界归还（绕过 TableTree
  Drop 的递归扫描）。work unit 是真实执行步数（链扫描每步、槽位检查、
  完成插入），预算是硬执行上界。完成时发布序固定：shell 先冻结终态
  快照并置 CLOSED（原子清 REAPABLE，外部无 Dead+REAPABLE 混合视图）
  → core 内部置 Dead → Job 成员表摘除（core 仅剩空壳）。并发批次以
  drain_gate（try_lock → ObjectBusy）仲裁；Drain 进度存目标进程
  （handle 游标/pending close + 地址空间阶段游标 + 在途归还游标），同
  authority 可接管。init 持久保留全部服务 control：WaitMany(REAPABLE|CLOSED) →
  Drain 至 Complete → 终态快照；对象 close 回调（如隧道 PEER_CLOSED）
  发生在 Drain 期间，用户态等待序必须先监督后观察终态位。
- **创建/启动事务**：ProcessCreate 先锁定 Job 成员 marker，capability
  可见前完成不可失败的成员提交；JobCreate 同构（child marker →
  输出预留/写入（槽仍 Reserved，槽号对外不可用）→ 层级提交回调 →
  table.commit，失败回滚 marker）。ProcessStart 事务见 [`startup.md`](startup.md)；任务层只拥有其 lifecycle 提交点与状态转换。
- **对象 close callback**：Handle 摘出后才在表锁外执行；各 role 的
  callback 与固定 fanout 上界由 [`ipc.md`](ipc.md)「Handle close
  callbacks」唯一记录。任务层只依赖“单次 callback 有固定上界”这一契约。
- **用户态页故障一律杀进程**：本内核无按需分配，所有区域创建时显式
  映射，fault 即程序缺陷。打印诊断行（pid / sepc / 故障地址 / 操作）
  后走终止路径，绝不 panic 内核。

### 锁序契约（Lock Ladder）

锁序由 Lock Ladder 运行时断言强制（`os/kernel/src/sync.rs` 的 `ranks`
表，debug 构建）：每把锁在构造点声明 rank，获取时断言 per-hart 秩栈
单调——新秩须大于栈顶，或同秩且链段 key 严格递增；违规即 panic
（经 RawWriter，不依赖堆与锁）。release 构建零开销。bootstrap 期
（tp 未建立，单核）使用专用帧，formal entry 汇合点切换至 per-hart 帧。

秩分配（数字唯一真值在 `sync::ranks`，此处列序即序）：

| rank | 锁 | 链段 key |
|---|---|---|
| DRAIN_GATE | 收束批次仲裁，一次性覆盖最广，恒最先 | — |
| DRAIN_CURSOR | HandleTable 收束游标与 pending close | — |
| HANDLE_TABLE | caller→child 嵌套 | pid 递增 |
| LEAF | CONSOLE、REGISTRY、ROOT anchor、各域就绪队列、per-hart TimerQueue、WaitContext 两锁 | — |
| JOB_INNER | Job 链锁（≤32 把同持） | jid 递增 |
| MAILBOX / CONNECTION | 对象状态锁 | — |
| ADDRESS_SPACE | 用户地址空间 | — |
| NOTIFICATION | 唯一以 space 为外层的对象锁边 | — |
| OBJECT_WAIT | Job.wait、ProcessControl、Endpoint、ProcessBuilder、Process.control 回指槽 | — |
| LIFECYCLE | 生命周期顶级锁（从不出游；被链锁/对象壳在锁内进入） | — |
| HEAP | talc（RankedRawSpinlock 类型级注入；几乎被全部容器锁内获取，故置顶） | — |
| POOL | 物理帧池（HEAP 与空间锁的内层） | — |

三类锁的共同纪律「锁内不出游」不变：lifecycle 的出游动作经 TerminationTodo
解锁后执行；对象电平发布经 `take_completer` 在锁外 `finish_offered`；close 回调在表锁释放
后执行；drain 完成分支显式先释放 drain_gate 再让 process 强引用
消亡（close 回调链不进 gate 持有区）。新增锁在构造点声明 rank 即受
断言保护；需要同秩多持的锁用 `Spinlock::chained` 声明单调 key
（Job 链锁 = jid、HandleTable 嵌套 = pid）。

### reserve/commit/rollback 协议

Job 成员表/子表、HandleTable 槽位与调度类就绪队列（`SchedClass` trait 的 reserve/commit/rollback 契约，域路由按 eligibility 选定目标类）三处的 marker 事务
遵循同一协议四要素：①占位条目对查找/枚举/pick 不可见；②单调 token
凭据防错认（token 零值非法）；③commit/rollback 按 token 定位，结构性
不可消失（`expect` 论证：在途 syscall 的预留只能由本事务消费）；
④全部在容器锁内完成，无分配失败路径。StartupBlock 的逆序 unmap 属
线性撤销，不使用 marker。

## sleep

Sleep 复用 WaitContext：ms > 0 时换算为单调 `expires_at`，线程转 Waiting；per-hart `TimerQueue` 到期弹出稳定 token并竞争 Timeout outcome，Sleep action 写回成功后 enqueue。发起 hart 是 timeout owner；对象/终止提前完成会按 token 从 owner queue 注销。跨 hart 注销只删除队列项，不远程重编程 timer。
