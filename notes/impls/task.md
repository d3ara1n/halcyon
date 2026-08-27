# 任务模型实现

> 任务是什么、进程与线程的分工等概念层见 [`../ideas/task.md`](../ideas/task.md)；本篇记录其在内核中的落地。线程持有 UserContext（用户现场，每线程一份）与调度状态；进程持有资源和从 ELF 得出的执行能力需求。完整上下文契约见 [`execution-context.md`](execution-context.md)。

## 用户地址空间布局

低半区 `[0, 2^38)`，无 trampoline 与隧道区——共享内核映射后用户半区完整归用户：

```
[0, brk')             ELF 段（text/rodata/data/bss，LOAD 段原样映射）
[brk', block_end)     StartupBlock（只读，launch 映射；见 startup.md）
[block_end, 栈区底)   堆，Extend 向上扩展，逐页映射
[栈区底, 2^38)        栈区：主线程栈 8MiB 钉在半区顶，未来线程栈向下生长
```

- 堆扩展（Extend）从当前 brk 起逐页映射，返回新 brk；brk 基点在 launch 时越过启动块，块与堆结构性互不重叠。物理连续性不做要求，虚拟连续性由「从 brk 起步映射」结构性保证。
- 主线程初始 sp = `2^38`（栈顶，16 字节对齐）；a0 = StartupBlock 基址、a1 = 块字节数是入口参数（rinlib 启动契约，见 [startup.md](startup.md)）。
- 用户 tp 置 0（rinlib 未用 TLS；引入 TLS 时再定义 ABI）。
- ASID 恒 0，地址空间切换时 `sfence.vma` 全量冲刷；ASID 分配（sv39 仅 9 位，需复用策略）作为优化留待演进——启用时同步引入 remote call 的 TLB shootdown 消费者。

## 调度：域—类—执行点三层组合

```
执行点（每 hart 一份，HartLocal）        调度域（共享）                    调度类（策略容器）
┌─────────────────────────┐        ┌──────────────────────┐      ┌────────────────────┐
│ current: Option<Thread> │        │ SchedDomain           │◀─────│ trait SchedClass    │
│ domain: &SchedDomain    │───────▶│  classes: 优先级序数组 │      │ enqueue / pick /    │
│ （调度循环 + idle 循环）  │        │ （M3: [Fair] 单类）     │      │ has_ready           │
└─────────────────────────┘        └──────────────────────┘      └────────────────────┘
```

- **执行点**：hart 的运行现场（当前线程、所属域、trap 锚），见 `internals.md`「tp 寄存器」。调度循环与 idle 循环是执行点的行为。
- **调度域**：一组能力兼容且策略相同的 hart 共享的调度类层次，hart 经 HartLocal 指向所属域。硬件 capability 是准入事实，domain 是 capability 与调度策略的派生对象；能力需求不是调度 class。线程只归属一个 compatible domain，跨域迁移显式转移队列所有权。域内类按优先级序查询，先到先得。
- **调度类**：一类线程的就绪容器 + 选择策略。实现整体可替换（轮转队列 / 无锁队列 / 窃取），加优先级类 = 向域的类数组插项——扩展是横向加项，不改结构。

时间片为固定量子，tickless：调度循环每次新 dispatch 前调用 `arm_quantum`，Resume 热路径不重置量子；同时取全局期限表最早项与量子截止的较近者设置本 hart timer。公平性由 FIFO 队列的结构性质保证，不依赖额外记账字段。

ProcessWrite 可由其他 hart 通过物理直映射填充新可执行帧。当前没有代码代次与 active-hart 集合，因此调度循环在每次新 dispatch 前执行本 hart `fence.i`；首次执行和迁移都不会观察帧复用前的旧指令缓存。Running process 没有 Building 写入口，Resume 热路径无需重复同步。

### 单一归属不变量

任意线程任意时刻恰处于一个归属：

```
某类队列（Ready） ｜ 某 hart 的 current（Running） ｜ 无容器（Waiting/Dead）
```

容器成员资格是状态真值，`Thread.state` 只是镜像；全部转换经调度器入口（`enqueue` / `pick` / `wake`）在锁内完成，锁序单向：期限表锁 → 类锁。

## 线程状态

`Ready / Running / Waiting / Dead`。Waiting：线程不在任何容器，等待其登记的内核请求完成；请求完成时 `wake()` 直接回 Ready——结果已写入 TrapFrame，无中间态。

### 等待的所有权与仲裁

- **强引用随容器走**：线程的 Arc 恰由其所在容器持有——就绪队列、执行点调度循环、或等待条目。等待条目强持有等待中的线程；不存在从容器反向到线程的长期指针（进程不回指线程），退出回收的 Drop 链因此能真正释放帧。lifecycle 的 Waiting 记录只持 weak WaitContext（触达取消用），在 park 发布时于 lifecycle 锁内线性化。
- **发布时序**：「可被唤醒」严格晚于「离开一切 hart 引用」——dispatcher 只把等待意图写入 HartLocal 私有槽，调度循环在 `clear_context` 之后的 Park 分支才向全局等待结构发布。完成方永远见不到仍在本 hart 执行的线程，双容器竞态在结构上不可能。
- **代数仲裁**：每次阻塞创建独立 `WaitContext/WaitCore`，状态为 `Installing → Armed → Finishing → Done`。对象命中、deadline 与取消候选通过同一个 outcome 竞争；唯一赢家取得线程所有权并负责跨对象清理。终止取消（kill/abandonment）同样经 offer(Abandoned) 竞争；胜者负责线程消散与离场确认。期限表强持同一 WaitContext，过期扫描只 offer Deadline，不存在每线程 `wait_gen` 平行机制。

## Job、Building process 与发布

root Job 由内核 static anchor 强持（所有权图：anchor ─strong→ root Job，
parent ─strong→ children，child ─weak→ parent；Job 直接成员表 ─strong→
未 Dead 的 Process cores，Process ─weak→ Job）。JobCreate 派生层级；
ProcessCreate 必须持 CREATE，生成空 AddressSpace、HandleTable、affine
ProcessBuilder 与从 Building 起即存在的 ProcessControl，并在事务提交点
把 Building process 插入 Job 直接成员表（对 Seal/枚举可见，输出失败/
回滚不遗留成员）。全局进程表已退役：单调 PID 分配器保留，未 Dead
core 的生命周期根是 Job 成员表；Start 的事务 marker 同样落在成员表。

ProcessStart 在提交前预构造主线程并在公平类队列放入 reservation marker；
marker 不参与查找、pick 或 `has_ready`。GRANT/StartupBlock/输出全部
准备成功后，提交区先做 lifecycle 线性化（Building→Running，kill/
abandonment 先行则完整回滚返回 ObjectClosed），再替换预留项。PID 单调
不复用，`parent_pid` 只供诊断，授权仅来自 Job/Process capabilities。

ProcessControl 贯穿 Building/Running/Terminating/Dead 保持同一对象身份
（HandleTable 条目强持 shell，shell ─weak→ core）；关闭 control 只消散
authority。固定宽 ProcessQuery、异步幂等 ProcessKill、REAPABLE 电平
已接入；Dead 后 shell 冻结终态快照持续可查。D64 profile 在
capability-derived 调度域接线前明确拒绝。

> 演进点：marker 预留（reserve/commit/rollback）当前由公平类的自由函数
> 承载，不在 `SchedClass` trait 契约内。接入 D64 eligibility 时，
> ProcessStart 的发布必须按线程执行需求路由进兼容域——届时需把预留
> 语义上收为所有类实现的接口，或由域层提供统一的预留通道；不得把现有
> 自由函数当稳定接口直接跨域复用。

## 生命周期
- **创建**：唯一 init 由内核从 BootPackage initial ELF 构造（同样获得
  Building 起存在的 ProcessControl，无结构特例）；后续进程由用户态
  `libprocess` 驱动 ProcessBuilder 映射、回填并发布。内核不解析 initfs
  或服务拓扑。
- **状态机**：`Building → Running → Terminating → Dead`，真值在
  Process 内嵌 lifecycle（原子 state 快读 + 顶级锁保护终因/成员记录/
  active 位图）。Exit、fault、ProcessKill 与 Building abandonment 在
  各自适用状态竞争首次终止线性化点冻结终因（reason + i64 code），
  后续事件幂等不覆盖；fault 经稳定 ProcessFaultCode 编码，不固化裸
  scause。成员记录（Gone/Ready/Running/Waiting/Exiting）是线程容器
  唯一真值：pick 后 trap 入口统一检查 Terminating（惰性撤销），enqueue
  无条件入队不反向触碰 lifecycle 锁；Waiting 经 weak WaitContext 取消
  （Abandoned 不回用户态，线程随上下文消散）；Running 由 kill 锁外发
  IPI，目标在任意 trap 入口吸收为 Killed。
- **退出收束**（有界分批，管理者驱动）：调度循环非-Resume 出口先归一
  内核 satp（含全量 SFENCE.VMA）并清 active 位再处置线程；reap 先
  drop 线程强引用再做离场确认（thread_departed → REAPABLE 持续电平）。
  任何容器路径都只到达 REAPABLE；Dead 仅由 ProcessDrain 的 Complete
  分支发布：HandleTable 先逐槽扫描摘项（take_next_bounded 硬预算），
  对象 close 回调锁外执行（仍可用地址空间解除外部映射），随后
  AddressSpace 分阶段：数据帧 tracker 逐个经帧池有界归还（FreeScan
  游标可恢复、O(1) 校验重启），页表 L0/L1 表帧逐槽登记归还，root 帧
  经 leak_root 交出后单独走 RootFree 阶段有界归还（绕过 TableTree
  Drop 的递归扫描）。work unit 是真实执行步数（链扫描每步、槽位检查、
  完成插入），预算是硬执行上界。完成时发布序固定：shell 先冻结终态
  快照并置 CLOSED（原子清 REAPABLE，外部无 Dead+REAPABLE 混合视图）
  → core 内部置 Dead → Job 成员表摘除（core 仅剩空壳）。并发批次以
  drain_gate（try_lock → ObjectBusy）仲裁；Drain 进度存目标进程
  （handle 游标 + 地址空间阶段游标 + 在途归还游标），同 authority 可
  接管。init 持久保留全部服务 control：WaitMany(REAPABLE|CLOSED) →
  Drain 至 Complete → 终态快照；对象 close 回调（如隧道 PEER_CLOSED）
  发生在 Drain 期间，用户态等待序必须先监督后观察终态位。
- **创建/启动事务**：ProcessCreate 先锁定 Job 成员 marker，capability
  可见前完成不可失败的成员提交；JobCreate 同构（child marker →
  输出预留/写入（槽仍 Reserved，槽号对外不可用）→ 层级提交回调 →
  table.commit，失败回滚 marker）。ProcessStart 提交前全部可失败
  步骤完成（含 HandleTable pin：builder+grants 翻转 Pinned，多线程
  调用方下其他线程不可见不可关），lifecycle 线性化失败则无损 unpin
  后整体回滚；线性化后只余容量已预留的不可失败消费。
- **close callback fanout 固定上界（约束）**：当前 role 集合下，唯一
  级联关闭者是 MailboxOwner（close_owner 清空队列 ≤ MAILBOX_CAPACITY(16)
  × MESSAGE_HANDLE_MAX(8) = 128 条 transit 条目），而全部可 transit
  角色（MailboxSender/Once、NotificationSignaler、TunnelInvitation、
  ProcessControl、JobControl、ProcessBuilder）的 close_transit 均为
  叶子（no-op / O(1) abandon / O(1) 解除外部映射 + 对端信号），无同步
  递归 drain 另一容器的路径——单次 close 的成本是 ≤ ~130 步的固定
  常数，不随进程状态增长。**新增可 transit 的容器 role 时必须重审
  此证明**（任何在 close_transit 中同步排空另一对象容器的角色都会
  破坏该固定上界，需改为有界 continuation）。
- **用户态页故障一律杀进程**：本内核无按需分配，所有区域创建时显式
  映射，fault 即程序缺陷。打印诊断行（pid / sepc / 故障地址 / 操作）
  后走终止路径，绝不 panic 内核。

### 锁序契约（lifecycle 顶级锁）

Process lifecycle 锁是跨子系统转换的顶级状态锁，方向约束是
**lifecycle 锁内不出游**：

- 不在 lifecycle 锁内调用 subscribe/unsubscribe、offer/finish、enqueue、
  IPI、对象 close callback、uaccess 或页表操作——这些动作经
  TerminationTodo 在解锁后执行；
- 不在 lifecycle 锁内获取任何其他锁（对象锁、WaitContext/期限表锁、
  调度类锁、地址空间/HandleTable 锁）；
- 反向的单向嵌套——在其他锁内进入 lifecycle（如 ProcessControl 快照
  在 shell state 锁内调 lifecycle 快照）——因 lifecycle 不出游而安全，
  不构成环，属合法调用序。

JobState 锁与之同纪律：成员摘除/层级变更在锁外驱动 lifecycle 动作，
对象锁（JobControl wait）不与 JobState 锁嵌套。

## sleep

第一个异步系统调用（模型见 [`call.md`](call.md)），用于验证整条异步通路：ms > 0 时登记期限后线程转 Waiting；期限到达由 timer 唤醒 `wake()` 回 Ready，sret 后 a0 = NoError。期限表全局共享，登记时由发起 hart 立即 arm 自己的 timer（唤醒所有权，见 `internals.md`）。
