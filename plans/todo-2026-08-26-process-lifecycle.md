# 进程生命周期与终止屏障计划

## 目标

在现有 Job、ProcessBuilder、ProcessControl 基座上完成显式 ProcessKill、状态查询、多线程安全的资源收束，以及供 pm 组合 JobKill 的 JobSeal/分页枚举机制。终止必须统一覆盖 Building、Ready、Running、Waiting 和多 hart 执行，不把关闭 capability 等同于政策动作，不以一进程一线程的偶然条件设计一次性接口；无界 Job 遍历和资源回收循环不得落入单次内核路径。

## 当前基座

- ProcessBuilder 是 affine Building authority；ProcessStart 成功后永久消费。当前 ProcessControl 仍到 ProcessStart 才产生，需前移到 ProcessCreate，使 Building 可管理。
- ProcessControl 可 duplicate/TRANSIT/GRANT，进程退出后发布 CLOSED；关闭 control 不终止目标。
- exit code 已进入轻量 control shell，但尚无固定宽查询 ABI。
- 当前一进程一线程，进程不持线程成员表；Waiting 所有权位于 WaitContext，Running 所有权位于 HartLocal，Ready 所有权位于调度类。
- D64 调度域 eligibility 尚未接线，ProcessStart 明确拒绝 D64。

## 设计前必须确认

### Process 状态与观察壳

定义 Building、Running、Terminating、Dead 的唯一真值与线性化点；明确 ProcessControl 查询的 fixed-width snapshot、exit reason/code、PID 与状态。Running 资源在何时与 Dead shell 解耦必须有单一所有权路径。

### Kill 仲裁

ProcessKill 是显式 MANAGE 操作；需要定义重复 kill、Exit/fault/kill 同时到达、目标已 Dead、调用者杀自身时的结果。Kill 只请求终止，真正 teardown 必须由不再运行目标地址空间的收束点完成。

### 各容器撤销

- Ready：从调度类取得线程所有权，或以统一终止标志在 pick 边界收束；
- Running：记录 active hart，IPI/remote call 请求离开用户态，并等待确认；
- Waiting：通过 WaitContext 的正式 Cancelled/Abandoned outcome 竞争并清理对象订阅与 deadline；
- 多线程：成员表与 active-hart bitmap 必须先于 ThreadSpawn 建立，最后线程离开后才能 drain Handles、清 PTE 和归还帧。

不得为单线程首版分别写三条不相容的 kill 快路径。

### Job 管理域

Job 保持内核对象，但递归 JobKill 是 init/pm 等管理者的用户态政策操作。内核提供幂等 JobSeal、直接 child Job/process 的有界分页枚举、成员完成计数和 capability 派生；Job 深度硬上限为 32，ancestor seal 与并发 JobCreate/ProcessCreate/ProcessStart 通过有界祖先检查形成统一封口边界。Open Job 为空仍存活，Sealed Job 仅在直接进程与 child Jobs 全部 Dead 后发布 CLOSED。JobControl close 继续只关闭 authority。

### 有界资源收束

线程全部离场后，HandleTable callback 与页表/帧回收仍可能随资源规模无界。管理者通过 ProcessControl 驱动固定预算的受保护收束操作；内核每次只处理有明确上界的 entry/页表节点，最后一批完成后才发布 Dead/CLOSED。需定义可恢复游标、持 MANAGE authority 的接管语义，以及 pm 获知待收束进程的持续电平。

### 已确认的 Process ABI

```text
ProcessCreate(job, control_rights)
    -> ProcessCreateResult { builder, control, pid, reserved }
ProcessStart(builder, descriptor) -> ()
ProcessQuery(control) -> ProcessSnapshot
ProcessKill(control, code) -> ()
ProcessDrain(control, max_work) -> ProcessDrainResult
```

- `ProcessCreateResult` 为 32 字节、8 字节对齐；Builder 与 Control 原子安装。`control_rights` 由调用者显式请求并校验为 ProcessControl 最大 rights 的子集。
- ProcessControl 从 Building 起保持同一对象身份。`ProcessStartDescriptor` 删除 `control_rights` 后为 48 字节，Start 只消费 Builder，不再产生 Control。
- `ProcessSnapshot` 为 40 字节：`pid:u64`、`parent_pid:u64`、`state:u32`、`reason:u32`、`code:i64`、`reserved:u64`。`ProcessState` 判别值固定为 Building=0、Running=1、Terminating=2、Dead=3；`ProcessExitReason` 固定为 None=0、Exited=1、Fault=2、Killed=3、Abandoned=4。Building/Running 要求 reason=None、code=0；Abandoned 的 code 固定为 0；Terminating/Dead 的 reason/code 在首次终止线性化点冻结。未知判别值必须由用户态拒绝，不降级解释。
- Fault 的 code 使用 `repr(i64)` 的稳定 `ProcessFaultCode`：Unknown=0、InstructionAccess=1、IllegalInstruction=2、Breakpoint=3、LoadAccess=4、StoreAccess=5、InstructionMisaligned=6、LoadMisaligned=7、StoreMisaligned=8；不把裸 `scause` 固化为生命周期 ABI。
- `ProcessKill` 要求 MANAGE，调用者提供完整 i64 code，reason 固定为 Killed；后到的幂等 Kill 返回成功但不覆盖已有终因。Query 要求 READ，等待 REAPABLE/CLOSED 要求 WAIT。
- `ObjectSignals::REAPABLE = 1 << 3`，只对 ProcessControl 等明确允许该位的 lifecycle role 生效。`ProcessDrain` 仅在 REAPABLE 或 Dead 上适用并要求 MANAGE。调用者给出非零 `max_work`，内核以 `PROCESS_DRAIN_MAX` 限制单次工作；work unit 是内核定义、单元成本有上界的收束记录，不承诺等同某类资源数量。
- `ProcessDrainResult` 为 16 字节、8 字节对齐：`work_done:u32`、`status:u32`、`reserved:u64`；`ProcessDrainStatus` 固定为 More=0、Complete=1。reserved 必须为零，不计算精确 remaining。Dead 上调用幂等返回 `{0, Complete}`；并发批次以 ObjectBusy 仲裁。
- Handle 先于地址空间收束，使对象 close callback 仍可撤销 external mapping；最后一批地址空间资源完成后，ProcessDrain 原子清 REAPABLE、置 Dead/CLOSED。

### 已确认的 Job ABI

```text
JobSeal(job_control) -> ()
JobQuery(job_control, out: *JobSnapshot) -> ()
JobEnumerate(job_control, kind, cursor, buf: *u64, buf_len) -> JobEnumerateResult
JobDerive(job_control, kind, id, rights, out: *Handle) -> ()
```

- 调用号接 Process 控制段：`JobSeal = 0x19`、`JobQuery = 0x1a`、`JobEnumerate = 0x1b`、`JobDerive = 0x1c`。
- `JobSeal` 要求 MANAGE，幂等：重复 seal 成功且不改变既有状态；sealed 后该 Job 及全部后代的创建/启动口经上行检查永久关闭（ObjectClosed）。
- `JobQuery` 要求 READ。`JobSnapshot` 为 40 字节、8 字节对齐：`jid:u64`、`parent_jid:u64`（root 为 0）、`state:u32`、`live_processes:u32`、`live_children:u32`、`reserved:u32`、`reserved2:u64`。`JobState` 判别值固定 Open=0、Sealed=1、Dead=2；未知判别值由用户态拒绝。计数是非精确近似值（不构成协议依据）；Dead 后返回冻结快照。
- `JobEnumerate` 要求 READ；`kind` 固定 0 = child Jobs（JobId 序）、1 = member processes（Pid 序）。按 ID 升序返回 ≤ min(buf_len, JOB_ENUMERATE_MAX) 个 8 字节 ID；`cursor` 为上批 `next_cursor`（首批传 0）；遇未决事务占位即终止本批，`next_cursor` 严格小于该占位 ID，占位不输出。
- `JobEnumerateResult` 为 16 字节、8 字节对齐：`next_cursor:u64`（本批最后返回条目的 ID，无返回时等于入参 cursor）、`actual:u32`、`more:u32`（0/1）。契约：`more=1 ⇒ actual ≥ 1 ∨ next_cursor == 入参 cursor`（后者是占位屏障的零进展情形，调用方以原 cursor 重试；占位窗口是创建方单个 syscall 内的临界区，协作式内核下有界完成，重试不活锁）；`more=0` 表示表内无任何 ID > next_cursor 的（可见或占位）条目。违反即内核违约，用户态拒绝——rinlib 校验拒绝 `more=1 ∧ actual=0 ∧ next_cursor ≠ 入参 cursor`。`buf_len` 必须非零（0 为 IllegalArgument，对齐 ProcessDrain max_work=0 先例）；`buf_len` 超过 MAX 按 MAX 截断，不以 BufferTooSmall 表达。
- `JobDerive` 要求 MANAGE；kind 同上；`rights` 必须为源 JobControl handle rights ∩ 目标角色 allowed_rights 的子集，超集 RightsDenied；目标不在直接成员表（含已完成移表）ObjectNotFound。ID（Pid/JobId）不构成全局操作入口——唯一操作角色是 JobControl 直接成员域内的派生选择子。
- JobCreate 超出层级深度上限 32 返回 IllegalArgument（父链深度是 parent handle 的属性，参数非法类）。
- Job 的 ObjectSignals 维持仅 CLOSED：Job 无收束工作，完成即 CLOSED；等待 CLOSED 即「直接成员全部完成」屏障。
- `JOB_ENUMERATE_MAX` 为 shared 编译期常量，定值 128。

### 调度域 eligibility

把 ELF execution profile 转换为 compatible domain，明确无兼容 hart、运行中 capability 变化及迁移语义；完成后再接受 D64。

## 已拍板的结构决策（2026-08-27，设计方 GoldHolly）

1. **Building abandonment**：builder authority 是 affine 单一权；最后一个 builder authority 经 close_handle/close_transit 消散时尝试 Building→Terminating(Abandoned, 0)，先到终因不覆盖。Building process 在 ProcessCreate 事务提交点加入 Job 直接成员表（对 Seal/Enumerate 可见）；输出失败/回滚不得遗留成员。
2. **无人收束的进程**：内核绝不因 control 未传出而同步无界回收。step 4 同批给持久 init 加最小正式监督闭环：保留已启动服务的 controls、WaitMany(REAPABLE|CLOSED)、Drain 至 Complete；现有启动路径中关闭 control 的也要改为 init 保留。Job 派生兜底在 step 5 补。
3. **强持归属**：全局 ProcessTable 强持退役；Job 直接成员表是未 Dead Process core 的唯一生命周期根；Process→Job、ProcessControl→Process 均 weak，只保留单调 PID 分配器。Start 的事务 marker 改落 Job 成员表。
4. **Waiting 触达**：不给 Thread 加独立回指槽；lifecycle 锁内的成员记录是唯一容器真值（Ready | Running(hart slot) | Waiting(Weak<WaitContext>) | Exiting）。park 发布把 Waiting(context) 与可取消性在 lifecycle 协议中线性化；Kill 锁内取 weak context/标记、锁外 offer(Abandoned)。
5. **Ready 撤销**：pick 边界惰性撤销——pick 取得后、加载 user satp 前检查 lifecycle，Terminating 直接收束；enqueue/requeue/park 发布同样过 lifecycle gate；Kill 唤醒 idle hart 保证 ready entry 被推进。
6. **Start vs Kill**：Kill 先行后 Start 返回 `ObjectClosed`（不新增 InvalidState）；Job effective seal 导致永久不可提交同样 ObjectClosed，保持不可逆关闭语义。
7. **Dead shell**：control 冻结 snapshot、只 weak 指 core；Dead 发布时 core 完成收束、从 Job 成员表移除后直接释放。control Handle 只保活 shell。
8. **跨 hart kill 最小正确版随 step 2/3 落地**：lifecycle/active bitmap 在进入 user satp 前登记；Kill 向 active mask 发 IPI；SSIP 独立检查 terminating；目标统一非-Resume 出口先切 kernel satp + 本地全量 SFENCE.VMA，再清 active bit/ack。step 7 是 ThreadSpawn 前的泛化与压力验证，不是补基本正确性。

## 已拍板的结构决策（2026-08-27 第二批：Job 管理面，step 5 前置）

取证与完整推导（含被否选项）见 [archived/todo-2026-08-27-job-management-design.md](archived/todo-2026-08-27-job-management-design.md) 与 [ref-2026-08-27-job-enumerate-derive-research.md](ref-2026-08-27-job-enumerate-derive-research.md)。

9. **枚举 = 单调 ID 序游标分页**：`JobEnumerate` 以「上批最后返回条目的 ID」为游标——ID 单调不复用 ⇒ 免内核枚举状态、断点续扫、竞态良定义（`id > cursor` 的存活成员必然在后续批出现，ID ≤ cursor 未返回者必然已移除）；单批条目数内核常量封顶；遇未决事务占位即终止本批、游标停在其前（屏障闭合「跳过占位又越过它」的漏项窗口）。条目仅 8 字节 ID，状态经派生后 Query——观察不消费 authority、不占 HandleTable 槽。
10. **派生 = JobDerive 按 ID 单目标**：kind 区分 child Job→JobControl 与 member process→ProcessControl；请求 rights ⊆ 源 handle rights ∩ 目标角色 allowed_rights，超集 RightsDenied（显式拒绝，不学 seL4 静默降级）；目标不在直接成员表 ObjectNotFound——ID 不复用保证 NotFound 只意味着「已完成」，永不错指。ID（Pid/JobId）不构成全局操作入口，唯一操作角色是 JobControl 直接成员域内的派生选择子。派生复用存活的 ProcessControl shell（单一 shell 身份，REAPABLE/CLOSED 电平不分叉）；shell 已消散时从 core 铸造新 shell，并在铸造点重放已达成的电平（如 REAPABLE），派生兜底由此接上 drain 入口。
11. **seal 只封创建，完成 = 自身 sealed && empty**：JobSeal 是 O(1) 置位（不扫表——宽度无界）；创建/启动提交点沿父链上行 ≤32 检查任一祖先 sealed → ObjectClosed（决策 6 语义）。完成条件是自身 sealed && members 空 && children 空，三触发点（seal 时已空／成员移除后空／子完成后空）事件驱动、自底向上传播、单步有界。递归封口与递归 JobKill 是用户态政策（逐层枚举+seal+kill+drain）；「只 seal 根不 seal 子」会被未 seal 的 child 卡住 CLOSED，属调用者政策不完整，可由派生兜底救回。否决「effective_sealed 完成条件」（祖先 seal 向下游历无界、协作式内核无后台触达）与「内核递归」（Zircon 宏内核惯性，无界路径）。root Job 由 static anchor 永持，完成发 CLOSED 但不移除不释放。
12. **JobId 引入与派生兜底**：`JobId(u64)` 全局单调不复用，root 恒 1，与 Pid 分立空间。决策 2 的「Job 派生兜底」由 JobDerive 直接覆盖，无需专门机制：进程 Building 提交点入表、Dead 发布点移表，REAPABLE 而 control 全消散者必然仍在表内——枚举可见、从 core 铸造 MANAGE control、drain 至 Complete；authority 消散的空壳 child Job 同构可救（派生 JobControl + 显式 seal → 完成）。
13. **rights 复用**：JobSeal/JobDerive 要求 MANAGE，JobQuery/JobEnumerate 要求 READ；不新增 ENUMERATE 位（枚举=观察、派生=铸权，现有位语义自然，避免「能查状态不能查成员」的人为割裂）。
14. **JobQuery 固定宽快照**：`JobSnapshot` 40 字节——jid、parent_jid（root 为 0）、state（Open=0/Sealed=1/Dead=2）、live_processes/live_children（非精确近似值，不构成协议依据，正确性走 CLOSED 等待与枚举收敛）、reserved 必须为零；Dead 后冻结快照，观察壳由存活 JobControl 保活；未知判别值用户态拒绝。
15. **实施约束（结构）**：成员/子表改按 ID 有序的 fallible 结构（首版有序 Vec + try_reserve + 二分定位：单批枚举自 partition_point 连续取 O(log n + N) 达成固定上界，插入/删除 O(width) memmove 仅在创建路径、不在完成标准的固定上界清单，以「宽度使 memmove 可观测」为换 fallible 有序树的触发条件；alloc BTreeMap 无失败路径、帧耗尽即 alloc_error_handler 内核 fatal，违反 OOM 戒律故不用；Vec+swap_remove 退役——序被破坏且无法做占位屏障）；Pid 分配挪入 owner Job 锁内与占位同临界区（消除多核「先分配后入表」乱序漏项）；「上行检查+提交」在先父后子链锁（≤32 把）内线性化，锁序规范 **Job 链锁 → lifecycle 锁 → 对象锁**（seal 持单锁与提交在 owner 锁互斥，等价 Ziron「AddChild 与 Kill 同锁」）；完成传播放子锁后取父锁（延迟触发安全：sealed ⇒ 无新成员，判定幂等）。

## 实施顺序

1. ~~调研成熟系统的 process/job kill、wait、dead-object shell 与 SMP 线程组退出路径，确认状态机和责任边界；~~ 已完成，证据见 `ref-2026-08-task-termination-research.md`，方向已进入 `notes/ideas/{task,bootstrap}.md`；
2. ~~将 ProcessControl 前移到 ProcessCreate，建立 Process lifecycle 锁、无强引用环的线程成员表（含跨 hart IPI 最小正确版与 WaitContext cancellation 契约，见上「已拍板的结构决策」）；~~ 已完成（含 C1-C5/H1-H3/M1-M2 集中修复：Building 操作准入、Gone 时点、Start pin 事务、root 帧有界释放、硬预算计费、零分配发布、快照一致性、创建事务序）；
3. ~~实现 fixed-width ProcessQuery、Building/Running ProcessKill、ProcessControl rights 与 CLOSED 等待；~~ 已完成；
4. ~~实现固定预算 Process 收束游标和管理电平，同批给持久 init 加最小监督闭环（保留 controls、WaitMany(REAPABLE|CLOSED)、Drain 至 Complete），证明最后线程、active-hart ack、Handle/page-table drain 与 Dead 发布的顺序；~~ 已完成（live kill 正路径：srv_target Waiting 取消 + Building kill + 自终止）；
5. ~~实现 Job 直接成员记账、ancestor seal、JobSeal 和有界分页枚举，再由 `libprocess`/pm 组合递归 JobKill~~ 已完成（2026-08-27：JobSeal/Query/Enumerate/Derive 四 syscall、有序 fallible 成员表、链锁封口闸门、完成传播与 libprocess 递归 job_kill；验收线 1–4 全过，virt ×6 / sifive_u / host 全绿，实施档案见 [archived/todo-2026-08-27-job-management-impl.md](archived/todo-2026-08-27-job-management-impl.md)）；
6. ~~以当前 ustar 私有政策由持久 init 硬编码建立 services Job、启动并监督其中的 `srv_pm` 等服务；pm 只管理显式委托的子域，不提前设计 manifest；~~ 已完成（2026-08-28：root → services → pm_domain/acceptance 拓扑；pm 经 StartupBlock grants 持 MANAGE|READ|WAIT 委托域 JobControl，对域内 Running 靶走 枚举→派生（铸造）→kill→drain→seal 全链；init 保留复制件兜底、失败整树 job_kill(services)、全部收束后进稳态不自终止，终态交 quiescent 静默停机；设计公理入档 ideas/bootstrap.md，拓扑快照两处打印；验收 virt ×7 / sifive_u（5s 窗口）/ host 全绿）；
7. ~~接入 ThreadSpawn 前完成多线程 teardown barrier；active hart 必须切回 kernel satp、执行本地全量 SFENCE.VMA 后才确认离场，不以 SBI 请求已发送代替完成；~~ 已完成（2026-08-28：线程成员表（tid 寻址有序 fallible Vec、离场即摘）取代单值 ThreadRecord，Gone 态删除；等待取消改锁外游标零分配；IPI 目标 = 冻结时刻 active 位图快照；归一收敛到 trap 汇编非 Resume 出口（execution-context.md 已知简化消解）；同批消解 KNOWN_ISSUES 写回 panic 面（deliver_output 复检即杀 + 分发出口终止检查）；实施计划见 [todo-2026-08-28-thread-teardown-barrier.md](todo-2026-08-28-thread-teardown-barrier.md)，验收 virt ×4 / sifive_u / host 全绿）；
8. 接入 capability-derived 调度域 eligibility，再开放 D64；
9. 对 Building/Ready/Running/Waiting、自杀、重复 kill、并发 Exit/fault、pm 接管、Job 枚举/派生/seal/完成传播竞态（含多核 ID 乱序分配窗口）和最后 control 关闭做 host/virt 多核验证；
10. 同步 `notes/impls/{task,execution-context,ipc,startup}.md`。

## 完成标准

- 用户可通过显式 capability 终止进程，并由 pm 以 JobSeal/分页枚举组合递归 JobKill；关闭 control 永不隐式终止；
- 任意状态的目标最终只 teardown 一次，等待订阅、Handle、地址空间和线程容器无泄漏；
- teardown 前所有 active hart 已在本地切回 kernel satp、完成全量 SFENCE.VMA 并确认离开目标地址空间；
- 任一内核调用的 Job 祖先检查/枚举、Handle drain 与页表回收工作量均有固定上界，普通管理者中断后可由持久 init 或其他同 authority 接管；init 自身失效是配置定义的系统级管理根失败，不引入内核特例；
- Dead shell 可稳定查询终态，资源已释放且不保留 Process/Thread 环；
- 用户可触发的所有竞态只返回确定状态或完成终止，不 panic 内核。
