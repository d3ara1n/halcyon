# 线程资源模型与用户态多线程（ThreadSpawn）

> 批一 Start 拆解及事务复审已收口；批二 ThreadSpawn 暂缓。多线程 teardown、
> 域 eligibility 与竞态矩阵等内核前置已经就绪，但 Running 进程尚无完整
> Map/Unmap、guard 与 remote TLB shootdown，无法兑现用户供栈的完整政策面。
> 当前由 [`todo-2026-09-user-memory-mapping.md`](todo-2026-09-user-memory-mapping.md)
> 阻塞；该计划收口后先重审本篇用户态资源契约，再实施批二。

## 决策记录

| # | 决策 | 结论 |
|---|---|---|
| D1 | 次线程栈归属 | 用户供栈。内核 spawn 不分配用户地址空间 backing；仍分配有固定结构上界的 Thread、观察壳与容器容量。栈大小/guard/放置/回收是应用政策。该方向不等于允许以 Extend 堆块作为永久降级实现：批二须等待完整 Map/Unmap 前置收口后再定用户态接口 |
| D2 | join 形态 | 内核铸造 waitable 观察壳（第四对 core/shell）。离场是内核自有事实（成员表摘除），内核事实经内核对象观察，与 REAPABLE/CLOSED 同纪律；亦为未来 ThreadKill 预留（壳不依赖 wrapper 跑完） |
| D3 | ThreadKill | 首版不实现，保留号维持不可用。延后不锁面：成员表 tid 寻址、泛化终止机器、join 壳三项前置均在本次落地；将来需加终止标志位 + pick gate/trap 入口检查扩展，是加检查不是改结构。壳携带离场方式字（v1 仅 Normal），未来 Killed 增判别值不改 ABI |
| D4 | 线程数上界 | shared 常量 `PROCESS_MAX_THREADS = 1024`，约束**并发成员数**（表长），超限 `ReachLimit`；tid 单调不复用（生灭循环不耗尽）。附带红利：终止取消循环（线程数 × WAIT_MANY_MAX）获得结构上界 |
| D5 | 首线程观察壳 | 不发。等首线程死无消费者（等全灭有 CLOSED）；「首线程先走、进程仍活」无现实场景；将来加是纯增量（Start 描述符 reserved 扩展），不锁面 |
| D6 | 线程=资源组装模型 | 采纳：ProcessAttach（外部附线程）+ ProcessGrant（外部装句柄）+ ProcessStart 纯化（入册）。无线程的进程停留在 Building——「没活过」；活体检查门（表非空）是 Start 唯一前置检查。已附线程数 N 不限（N 与门性质正交）；出生块为纯用户约定数据；外部附线程无观察壳（组装不是协作，组装者在 Start 之后不再观察内部状态）；Start 名字保留（开始调度/生命周期/Running） |
| tid | 编号起点 | tid 从 1 起，0 保留为非身份值——与 pid/JobId 的哨兵纪律（`parent=0`）对齐；pid 1 = init、jid 1 = root、tid 1 = 首线程，各 ID 空间的 1 号均为奠基者 |

### 已否决替代方案（防同题重议）

- **无线程 Running 稳态**（「惰性金库」）：只能写入（Handle 只进不出——授出 Handle 需要线程发起 syscall）、只能毁灭（kill/Drain）的沉默容器，无消费者；且需为被动终局边加 `has_ever_run` 疣标志豁免。零线程 Start 若无门，被动终局边无触发点（从未有 departure 事件），金库态从后门回归——门是承重墙。
- **「附加首线程」独立 op（Start 原子性保留版）**：附加 op 需要携带全部 Start 出生信息，与 Start 逐字段相同——是改名不是拆分。真增量（线程=资源、组装双通道）由 D6 采纳版完整获得。
- **Running 期外部插线程**：外部写通道在 Running 全部关闭（自治宪法：地址空间只受自己的线程支配），插入者无处准备现场（栈/入口上下文），硬插即投毒。对称性由「每类资源都有内外双通道」表达，不由「每个通道永久有效」表达。
- **纯用户态 join**（wrapper signal Notification）：在无 ThreadKill 世界 sound，但裸 syscall 用户无 join 契约，且 signal 先于真正离场——joiner 见「完成」时线程可能仍在 ThreadExit 中。被 D2 否决。
- **Enroll/Retire 改名**：Start 语义纯化后名字仍然精确（开始调度）；Retire 遮盖 {Fault, Killed, Abandoned} 暴力路径且 Kill 契约已冻结，不改。

## 模型公理

### 归属判据

一个机制放线程还是进程，三问判定：**出生时内核分配什么**（执行现场→线程；资源存量→进程）、**死亡时留下什么**（有遗产→资源侧；只剩可观察事件→执行侧）、**生存期消耗什么**（CPU→执行；内存/句柄→资源）。

一句话：**线程出生不携带资源、死亡不留遗产、生存只耗 CPU；进程持有全部资源，被所有线程共享。**

### 无角色平等

内核无「主线程」概念，只有「首线程」= 创建序事实：Start 通道出生的第一条线程（tid 1），携带 StartupBlock（a0/a1）只因启动信息必须交给第一个跑起来的人。是否初始化全局状态、是否 spawn 工作线程、初始化完是否退出（触发被动终局）——全部是用户政策。调度容器规则、终止屏障、取消仲裁、join 电平对所有线程同构。

进程级 Exit(code) 与线程级 ThreadExit(code) 分层：前者杀全部线程（POSIX `exit()` 层），后者本线程离场、末位者被动铸造终局。

### 线程是进程资源：组装双通道

| 资源 | 外部通道（Building 期，组装者持 builder） | 内部通道（Running 期，进程自己的 syscall） |
|---|---|---|
| 内存 | ProcessMap | Extend |
| 代码/数据 | ProcessWrite | ——（无） |
| 句柄 | ProcessGrant（新） | Transit/Grant |
| 线程 | ProcessAttach（新） | ThreadSpawn |

Building/Running 的本质即外部写通道的开/关。组装者（init/pm 经 libprocess，内核组装 init 为 bootstrap 特例）按序填资源，Start 是「资源齐备、入册调度」的纯状态转换。

### 生杀闭环

**进程的生 = Start 入册（首线程已在表），进程的死 = 末线程离场（被动终局边）**——线程 presence 从两端定义进程生命周期：线程数从 0（Building）到 ≥1（Start 门）到 0（终局）的完整闭环。

## ABI 面

| op | 通道 | 语义 |
|---|---|---|
| `ThreadSpawn(entry, sp, arg1, arg2) → (tid, handle)` | 内部，Running 专属 | 不分配用户 backing：创建内核执行基底（UserContext + 调度链 + join 壳）；内核元数据在可失败段完成有界预留。sp/entry 做 translate 前置校验，坏 sp 的 fault 走用户 fault 杀进程。arg1/arg2 为出生参数；安全用户态封装待 Map/Unmap 前置完成后重新设计 |
| `ThreadExit(code)` | 内部 | 本线程离场；末线程冻结 `(Exited, code)`（code 仅在此刻成为进程终态字段——lifecycle 的进程终因，不是线程遗产）。非末线程 code 丢弃，结果值走用户通道（spawn 时用户分配 result 槽，wrapper 先写后 exit） |
| `ThreadYield` | 内部 | 自入队尾，复用公平 FIFO |
| `ProcessAttach(builder, entry, sp, arg1, arg2) → tid` | 外部，Building 专属 | 组装者附线程；无观察壳；arg1/arg2 与内部 Spawn 同形（首线程传出生块地址与长度） |
| `ProcessGrant(builder, grants_ptr, count, out_values) → ()` | 外部，Building 专属 | 从组装者表摘 grants 装入目标表，句柄值写回 out_values（pin/commit 机制自原 Start 事务原样搬运）；组装者将值写入出生块后经 ProcessWrite 落盘。out_values 为第四参（写回目标），签名与实现一致 |
| `ProcessStart(builder, profile)` | 外部 | 活体检查门（已附线程 ≥1，唯一前置检查）→ Building→Running（含预育原子提取）→ 一次冻结 execution binding（profile 为判定输入，进程级属性）→ 预育线程整体入册。参数为平铺 (builder, profile)，无描述符 |

- **join**：`WaitMany(壳 Handle, DONE)`，不设 ThreadJoin syscall（0x23 保留号留空）；结果值读用户槽。**内存序契约**：离场线程在 ThreadExit 前的全部用户态存储对 joiner 可见——release/acquire 经 lifecycle 锁（RV AMO aq/rl）与 finish_offered 完成链传递。
- **出生块**：纯用户约定数据（Header + 句柄值 + payload），内核机制（Handle 数值预留连续绑定、map_startup_block、map_bootstrap_block 的前缀协议）删除；init bootstrap 改走内核内嵌的同构 op 序列（payload 收编 owned backing 的特例保留在内嵌 Write 里）。
- **保留号处置**：ThreadExit 0x20 / ThreadYield 0x21 / ThreadSpawn 0x22 启用；ThreadJoin 0x23 不设；ThreadKill 0x24 维持不可用。

## 内核机制

### 预育容器（nursery）

稳定所有权容器清单不变（类队列 | hart current | WaitContext）；预育表不是独立容器，而是**成员表的 Building 期形态**：`Staging { thread: Arc<Thread> }` 条目携带线程强引用，ProcessAttach 在 lifecycle 锁内原子插入（闭包式：tid 分配 + 线程构造 + 表插入同临界区，失败零副作用），Start 提交点在 begin_running 同一临界区内整体转 Ready 并交出强引用，Building abandonment 由终止游标 take_first_staging 逐条摘除释放（打破 Staging↔Thread.process 引用环）。内部 ThreadSpawn 的 Staging 瞬态同样由成员表携带（syscall 上下文构造，线性化点插入）。

### ThreadSpawn 事务（reserve/commit/rollback 四要素）

可失败段：cap 检查（表长 < 1024）→ try_reserve（成员表容量 / Thread Arc / 壳槽位 / 目标类 Ready 位）→ sp/entry translate 前置校验。线性化段（lifecycle 锁内，不可失败）：分配 tid、插入 `(tid, Staging)`。提交段：壳铸造 → commit_ready → staging_ready。Terminating 冻结后拒绝（lifecycle 锁内检查）。

### join 观察壳（第四对 core/shell）

Thread↔壳，与 Process↔ProcessControl、Job↔JobControl 同构：壳由 HandleTable 条目强持，weak 回指线程；close 只消散观察权不杀线程；壳 close 后线程照常离场（weak 升级失败 = 观察者已走）。DONE 电平发布于 thread_departed 摘除后**锁外**（take_completer/finish_offered 模式，锁序无新边）。离场方式字 v1 仅 Normal。

### 末线程被动终局边

`thread_departed` 摘除后「表空 && 未终止 → 冻结 `(Exited, code)`」——与既有终止冻结「首达者胜」在同一线性化点合流：末线程 ThreadExit 与异 hart ProcessKill 竞争时，先完成转换者冻结终因（Exited vs Killed）。

### Start 拆解

start_staged 事务改造：线程出生移入 ProcessAttach（预育条目随成员表持有）；grants 移入 ProcessGrant；Start 收缩为“链锁内上行检查 seal + 活体门 + Building→Running（含预育原子提取）+ execution binding + Ready 完整批次发布”。原子的价值不损失：Start 仍是唯一 Building→Running 线性化点，回滚面反而变小（各 op 独立回滚）。

## 实施批次

### 批一及事务复审状态（2026-08-29）

**已落地（内核与用户态双侧，全链路编译绿）**：

- shared ABI：`ProcessAttach = 0x1d`、`ProcessGrant = 0x1e` 调用号；Start 收缩为平铺参数 `(builder, profile)`；`ThreadStartContext`（entry/sp/arg1/arg2，Attach/Spawn 共用）；`PROCESS_MAX_THREADS = 1024`、`PROCESS_MAX_GRANTS = 64`；startup.rs 注释改为 BirthBlock 语义（出生块 = 用户约定数据）。
- lifecycle.rs：`Staging { thread: Arc<Thread> }` 携带强引用；`attach_member(闭包)` 原子插入（Closed/Limit/Oom 零副作用，tid 从 1 起）；`begin_running(expected, out)` 合并预育提取；`take_first_staging` 终止游标；`building()` 不再预留容量。
- proc.rs：requirement/domain 合成一次冻结的 execution binding；`Process::attach_thread` 统一 ProcessAttach/bootstrap 线程出生；`map_startup_block`/`rollback_startup_block` 已删；`launch_bootstrap` 内嵌同构序列。
- process.rs：`attach()`/`grant()`/`start()` 三 op 落地；BuildingLease 统一操作登记，grant 采用受保护 builder + grants transfer，Start 采用 Ready 原子批量预留，失败路径无损还原；`run_termination_todo` 增加 Staging 游标与 REAPABLE 复判。
- job.rs：`start_commit_gate(process, expected, out)` 携带提取缓冲。
- handle_table：pin 事务拆为 consume/transfer 两类，明确 builder 保护与 grants 所有权；新增测试。
- syscall.rs：Attach/Grant/Start 分发接线。
- rinlib：`sys_process_attach/grant`、`start(builder, profile)` 新封装。
- libprocess `spawn()` 组装序列：Map/Write → Grant（空批跳过）→ 组装者构造并写入出生块 → Attach → Start；失败由 `SpawnFailure` 明示 grants 的 Retained/Consumed 所有权与 cleanup_error，并统一 abandon/drain/close。
- 负载适配：init seal_before_start、race.rs build_spin_building（组装者 attach，锤只拉 Start）、hammer start_target 新签名。rinlib 启动契约（env/rt 的 a0/a1 解析）零改动。

**验证状态**：

| 线 | 结果 |
|---|---|
| `just check`（内核） | ✅ |
| host 单测（shared 7、os 纯逻辑 105、rinlib 1、libprocess 5） | ✅ |
| `just virt` / `just virt-release` | ✅ 锚点全命中，矩阵 10/10；现行回归由显式 reset 收束 |
| `just virt-hetero` / `just virt-nofd` | ✅ 两域绑定与 D64 无兼容域清理通过 |
| `just sifive_u` | ✅ 验收面全命中，由终态锚点收割 |

**原记“sifive_u 确定性挂死”这一具体判断已证伪**：旧报告调查的稳定失败分别来自 U-mode `wfi` fault 与验收脚本固定超时，均已修复。后续捕获的负载存活时提前 quiescent 现场无法判定引入批次；现已由显式系统复位从结构上删除错误终局，调查归档于 [`archived/todo-2026-08-29-early-quiescent-shutdown.md`](archived/todo-2026-08-29-early-quiescent-shutdown.md)，不作为批一事务复审的隐藏结论。

### 实施批次总览

1. **批一：Start 拆解**——ProcessAttach/ProcessGrant/Start 纯化、ABI 两侧（shared + rinlib/libprocess + 全负载组装路径）、init bootstrap 内嵌同构序列、startup 机制删除、出生块转用户约定。验证：全负载等价回归（virt/virt-release/hetero/nofd/sifive_u/host）。
2. **批二：ThreadSpawn/Exit/Yield + join 壳（受内存前置阻塞）**——spawn 事务（预育结构已在批一落地）、末线程冻结边、tid 从 1、cap；在完整 Map/Unmap、guard 与 TLB shootdown 收口后，先重审栈/wrapper/结果槽/join handle 的用户态所有权，再实现 rinlib。验证：等价回归 + 多线程功能负载。
3. **批三：竞态矩阵扩展 + carryover IPC 压力线 + 文档同步**——见下两节。

每个实现或修复批次独立提交、独立验证；批三是阶段收尾（`just virt-release` 必跑）。

## 竞态矩阵新增场景

1. spawn vs kill 线性化（Terminating 冻结后 spawn 拒绝；冻结前并发插入的 Staging 线程被收束吸收）；
2. 末线程 ThreadExit vs 异 hart ProcessKill（双冻结竞争，首达者胜，双侧终因均有胜出记录）；
3. join 唤醒 vs 离场时序（DONE 发布恰在成员摘除后；joiner 结果槽可见性——内存序轴断言）；
4. 并发 spawn/exit 风暴（tid 单调、表长守恒、cap 触发 ReachLimit 后恢复）；
5. 双 hart 同进程并行（同 satp、space 锁 Extend 串行化、fence.i 代次正确）；
6. Building 交错：attach/grant vs seal/kill（上行检查、abandonment 收编 nursery 与已装句柄）；
7. Start 活体门：零线程拒绝、1 与 N 线程入册各一例；
8. ProcessGrant 回滚（目标表满/失败路径句柄守恒）;
9. join 壳 close vs 线程离场（壳消散后离场照常，无泄漏无双重处置）。

## carryover IPC 压力线接入（触发条件 = 本里程碑落地）

- 消息风暴：MailboxFull/ObjectBusy/回滚与资源守恒；
- Tunnel create/attach/write/close 与进程退出风暴验证帧守恒；
- host 双线程与 RISC-V 双 hart Acquire/Release 压测；
- sifive_u 既有集成负载连续至少十轮。

## 文档同步清单

批一的方向与实现文档已随事务复审同步：普通 StartupBlock 属用户态约定，ProcessGrant/Attach 是 Building 期独立组装动作，ProcessStart 只负责首次发布；init bootstrap 仅保留内核内嵌同构序列。批二/批三仍须同步：

- `notes/ideas/task.md`：ThreadSpawn、ThreadExit、join 壳完成后的稳定方向契约；
- `notes/impls/{task,execution-context}.md`：spawn 事务、末线程终局边、join 壳与实际上下文入口；
- `notes/impls/mm.md`：用户态线程栈与结果槽的最终布局约定；
- `notes/ideas/call.md`：ThreadExit/Yield/Spawn 调用语义与保留号处置；
- `COMPASS.md`：阶段收口后的自然序。

## 完成标准

- 全负载等价回归 + 新增 9 场景 + carryover 四项全绿（debug 与 release）；
- lifecycle 无主线程特例、无单线程假设（既有）；
- join 契约（DONE 电平 + 内存序）有验证负载背书；
- 出生块为用户约定，startup 内核机制删除，文档同步完成；
- 决策入档（本篇 + notes）、代码全绿已提交、COMPASS 更新。
