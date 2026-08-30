# 线程资源模型与用户态多线程（ThreadSpawn）

> 状态：**三批均已完成并收口**。批二 ThreadSpawn/Exit/Yield、ThreadControl、
> guarded stack/join 与 user-memory 8B 由 `bdc83ef` 提交；批三竞态扩面、
> carryover IPC 压力线、FramePool 部分 extent 归还修复与文档同步由 `004cae5`
> 提交。host 148/148、common debug/release、virt-hetero、virt-nofd 的 16/16
> 矩阵及 `sifive_u` 连续十轮均通过。联合内存实施档案见
> [`todo-2026-09-user-memory-mapping.md`](todo-2026-09-user-memory-mapping.md)，
> 后续统一审查见 [`../todo-2026-08-30-user-memory-mapping-review.md`](../todo-2026-08-30-user-memory-mapping-review.md)。

## 决策记录

| # | 决策 | 结论 |
|---|---|---|
| D1 | 次线程栈归属 | 用户供栈。内核 spawn 不分配或接管用户地址空间 backing；rinlib `UserStack` 以普通 `MappedRegion` 建立双 guard 与可配置 usable 栈，join 后消费完整 reservation。首线程 Building 固定栈不复用为 Running 栈池 |
| D2 | join 形态 | 内核铸造 waitable ThreadControl 壳（第四对 core/shell）。离场是内核自有事实（成员表摘除），DONE 在摘除后锁外发布；壳 close 只消散观察权，不杀线程 |
| D3 | ThreadKill | 首版不实现，也不预留调用号。成员表 tid 寻址、join 壳与 pick/trap gate 保持未来增量入口；若出现线程局部终止需求，再以独立设计确定 authority、终因、结果与 ABI |
| D4 | 线程数上界 | shared 常量 `PROCESS_MAX_THREADS = 1024`，约束**并发成员数**（表长），超限 `ReachLimit`；tid 单调不复用（生灭循环不耗尽）。附带红利：终止取消循环（线程数 × WAIT_MANY_MAX）获得结构上界 |
| D5 | 首线程观察壳 | 不发。等首线程死无消费者（等全灭有 CLOSED）；「首线程先走、进程仍活」无现实场景；将来加是纯增量（Start 描述符 reserved 扩展），不锁面 |
| D6 | 线程=资源组装模型 | 采纳：ProcessAttach（外部附线程）+ ProcessGrant（外部装句柄）+ ProcessStart 纯化（入册）。无线程的进程停留在 Building；外部附线程无观察壳，Start 是唯一 Building→Running 发布点 |
| D7 | JoinHandle 收束 | 采用结构化收束：`JoinHandle::join` 等 DONE、取得用户结果并解除完整 guarded stack；未显式 join 时 Drop 走同一等待与解除路径并丢弃结果。首版不提供 detach，不建立常驻 reaper，也不允许静默保留到进程退出 |
| D8 | 线程结果义务 | 可能 park 的 MemoryMap 在调用线程登记 affine Map-result obligation；线程执行容器可先消散，但成员摘除、DONE 与栈接管必须晚于 obligation 完成。进程级 mandatory_ops 继续独立保护 AddressSpace Drain |
| D9 | Spawn ABI 与提交 | `ThreadSpawn(context_ptr, result_ptr)` 同步输出固定宽 `ThreadSpawnResult { tid, reserved, control }`。全部可失败资源先预留；lifecycle 锁内分配 tid 并插入不可被终止游标摘取的 Spawning 成员，固定宽输出成功后依次提交 handle、Ready 成员和调度占位，提交尾段不可失败 |
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
| 内存 | ProcessMap | MemoryMap/MemoryUnmap/MemoryProtect |
| 代码/数据 | ProcessWrite | ——（无） |
| 句柄 | ProcessGrant（新） | Transit/Grant |
| 线程 | ProcessAttach（新） | ThreadSpawn |

Building/Running 的本质即外部写通道的开/关。组装者（init/pm 经 libprocess，内核组装 init 为 bootstrap 特例）按序填资源，Start 是「资源齐备、入册调度」的纯状态转换。

### 生杀闭环

**进程的生 = Start 入册（首线程已在表），进程的死 = 末线程离场（被动终局边）**——线程 presence 从两端定义进程生命周期：线程数从 0（Building）到 ≥1（Start 门）到 0（终局）的完整闭环。

## ABI 面

| op | 通道 | 语义 |
|---|---|---|
| `ThreadSpawn(context_ptr, result_ptr) → ()` | 内部，Running 专属 | context 沿用 `ThreadStartContext`；result 是 16 字节固定宽 `ThreadSpawnResult`，成功交付 tid 与 waitable ThreadControl handle。内核不分配用户 backing；entry/sp 做当前 AddressSpace translate 前置校验 |
| `ThreadExit(code)` | 内部 | 本线程离场；末线程冻结 `(Exited, code)`（code 仅在此刻成为进程终态字段——lifecycle 的进程终因，不是线程遗产）。非末线程 code 丢弃，结果值走用户通道（spawn 时用户分配 result 槽，wrapper 先写后 exit） |
| `ThreadYield` | 内部 | 自入队尾，复用公平 FIFO |
| `ProcessAttach(builder, entry, sp, arg1, arg2) → tid` | 外部，Building 专属 | 组装者附线程；无观察壳；arg1/arg2 与内部 Spawn 同形（首线程传出生块地址与长度） |
| `ProcessGrant(builder, grants_ptr, count, out_values) → ()` | 外部，Building 专属 | 从组装者表摘 grants 装入目标表，句柄值写回 out_values（pin/commit 机制自原 Start 事务原样搬运）；组装者将值写入出生块后经 ProcessWrite 落盘。out_values 为第四参（写回目标），签名与实现一致 |
| `ProcessStart(builder, profile)` | 外部 | 活体检查门（已附线程 ≥1，唯一前置检查）→ Building→Running（含预育原子提取）→ 一次冻结 execution binding（profile 为判定输入，进程级属性）→ 预育线程整体入册。参数为平铺 (builder, profile)，无描述符 |

- **join**：`WaitMany(ThreadControl, DONE)`，不设 ThreadJoin syscall，也不预留调用号；结果值留在 rinlib 用户记录。离场线程在 ThreadExit 前的全部用户态存储对 joiner 可见，release/acquire 由 lifecycle 离场锁与 ThreadControl 电平发布链传递。rinlib 的 JoinHandle 结构化拥有记录、control 与 UserStack，join 或 Drop 都只在 DONE 后解除。
- **出生块**：纯用户约定数据（Header + 句柄值 + payload），内核机制（Handle 数值预留连续绑定、map_startup_block、map_bootstrap_block 的前缀协议）删除；init bootstrap 改走内核内嵌的同构 op 序列（payload 收编 owned backing 的特例保留在内嵌 Write 里）。
- **调用号处置**：ThreadExit 0x20 / ThreadYield 0x21 / ThreadSpawn 0x22 启用；join 是用户态组合，不进入 syscall ABI；首版 ThreadKill 未设计、未实现、未占号。

## 内核机制

### 预育容器（nursery）

稳定所有权容器清单不变（类队列 | hart current | WaitContext）；预育表只是成员表的 Building 期形态：`Staging { thread: Arc<Thread> }` 条目携带线程强引用，ProcessAttach 锁内插入，Start 同临界区整体转 Ready 并交出引用，Building abandonment 由终止游标逐条摘除。Running ThreadSpawn 不复用可被 Building 终止游标摘取的 Staging；它使用独立 Spawning 成员覆盖固定宽输出与不可失败提交之间的短暂所有权。

### ThreadSpawn 事务（reserve/commit/rollback 四要素）

可失败段：构造 ThreadControl/提交缓冲，预留 Handle 槽与目标域 Ready 槽，校验 entry/sp 与固定宽输出区间。lifecycle 线性化段检查 Running/线程上界，预留成员容量、分配 tid、构造 Thread 并插入 Spawning；从此成员阻止 REAPABLE，终止游标不摘取它。输出失败只会冻结调用进程并回滚 Spawning/Handle/Ready 预留；输出成功后依次提交 ThreadControl entry、Spawning→Ready 与调度占位，全部不可失败。Terminating 先到则在线性化段前拒绝；Spawning 先到则终止等待提交尾段自然收束。

### join 观察壳（第四对 core/shell）

ThreadControl 与执行 Thread 解耦：壳由 HandleTable 条目强持，内部 departure state 只持壳 weak；close 只消散观察权不杀线程，关闭后线程照常离场。DONE 电平发布于成员摘除及全部线程级 Map-result obligation 完成之后，并在释放 lifecycle/departure 锁后以 `take_completer → finish_offered` 逐个交付，不新增反向锁边。

### 末线程被动终局边

`thread_departed` 摘除后「表空 && 未终止 → 冻结 `(Exited, code)`」——与既有终止冻结「首达者胜」在同一线性化点合流：末线程 ThreadExit 与异 hart ProcessKill 竞争时，先完成转换者冻结终因（Exited vs Killed）。

### Start 拆解

start_staged 事务改造：线程出生移入 ProcessAttach（预育条目随成员表持有）；grants 移入 ProcessGrant；Start 收缩为“链锁内上行检查 seal + 活体门 + Building→Running（含预育原子提取）+ execution binding + Ready 完整批次发布”。原子的价值不损失：Start 仍是唯一 Building→Running 线性化点，回滚面反而变小（各 op 独立回滚）。

### 8A 后批二契约复审（2026-08-30）

复审确认首线程固定 Building 栈、出生块与 Running 次线程资源不可混用；可复用面只有 `ThreadStartContext`、UserContext 构造、进程 execution domain、调度 Ready reservation、成员/active barrier 与 ObjectWaitState。rinlib 安全面采用 `Builder → UserStack + 用户结果记录 → ThreadSpawn → JoinHandle`，默认栈为普通匿名 RW mapping、前后各一页 guard、sp 取 usable 顶端 16 字节对齐。

复审同时修正原计划两处不闭合：Running Staging 会被 termination 的 Building 游标摘走，改为独立 Spawning→Ready 不可失败提交；仅有进程 mandatory_ops 会允许被 kill 线程先发布 DONE，改由 Map-result obligation 把成员摘除与 join 完成延后到 result lease/transaction 完成之后。

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

**原记“sifive_u 确定性挂死”这一具体判断已证伪**：旧报告调查的稳定失败分别来自 U-mode `wfi` fault 与验收脚本固定超时，均已修复。后续捕获的负载存活时提前 quiescent 现场无法判定引入批次；现已由显式系统复位从结构上删除错误终局，调查归档于 [`todo-2026-08-29-early-quiescent-shutdown.md`](todo-2026-08-29-early-quiescent-shutdown.md)，不作为批一事务复审的隐藏结论。

### 实施批次总览

1. **批一：Start 拆解**——ProcessAttach/ProcessGrant/Start 纯化、ABI 两侧（shared + rinlib/libprocess + 全负载组装路径）、init bootstrap 内嵌同构序列、startup 机制删除、出生块转用户约定。验证：全负载等价回归（virt/virt-release/hetero/nofd/sifive_u/host）。
2. **批二：ThreadSpawn/Exit/Yield + join 壳（已完成，`bdc83ef`）**——shared 固定宽 ABI、Running `Spawning→Ready` 事务、ThreadControl DONE、线程级 Map-result obligation 与 rinlib guarded `UserStack`/结构化 `JoinHandle` 已贯通；user-memory 8B 真实多线程 gate 已通过。
3. **批三：竞态矩阵扩展 + carryover IPC 压力线 + 文档同步（已完成，`004cae5`）**——ThreadSpawn/末线程终局/join/容量竞态与 IPC 生命周期压力已进入 16/16 矩阵；非原语线程调用号删除，实现事实同步至 notes。

三个实现批次均独立提交、独立验证；批三以 common debug/release、hetero、nofd、十轮 `sifive_u` 与 host 全矩阵完成阶段收尾。

### 批二与 8B 验证状态（2026-08-30）

- common debug 与 release 均通过 13/13 场景；同 AddressSpace 旧 VA 精确复用观测到 2 个 active hart，8 轮普通 Unmap/Tunnel lease close 并发及 join/stack 回收完成；
- `virt-hetero`、`virt-nofd` 与 `sifive_u` 通过同一 13/13 矩阵，sifive_u 按既定 reset 后端失败终态收割；host 逻辑 crate 与 shared ABI 全绿；
- kill 与并发 ThreadSpawn 的 draining 失败路径命中并修正 `ADDRESS_SPACE → LEAF` ready rollback 反序；压力线程对 planner 的正式 `ObjectBusy` 背压只在完整失败事务边界退避，affine region token 不丢失；
- 批二验证时，ProcessKill 的进程级 `mandatory_ops` 常先挡住 committed Map，线程级 result obligation 未独立成为最终阻塞者；批三扩大真实 ThreadSpawn/Map/kill 时序后，nofd 负载已观测到 committed Map 直接延迟 ThreadDeparture。该证据来自公开 ABI，未增加 ThreadKill 或测试专用内核入口。

### 批三最终验证状态（2026-08-30）

- common debug 与 release、`virt-hetero`、`virt-nofd` 均通过 16/16；覆盖 spawn-vs-kill、末线程 exit-vs-kill、join 发布与 Drop、1024 并发成员上界和容量恢复、同 AddressSpace stale translation、Endpoint close/Unmap 竞争及 guarded stack 回收；
- carryover IPC 线通过 Mailbox 128 轮回滚压力、Tunnel 64 轮生命周期与 16 轮携存活 Endpoint 退出、跨进程共享页 Acquire/Release、ProcessDrain lease/backing 接管及固定 VA 复用；HandleClose 与内存事务竞争的 `ObjectBusy` 只在完整失败事务后退避；
- `sifive_u` 连续十轮均命中 16/16、`system reset failed: NotSupported` 与 acceptance watcher harvest，未触发 60 秒挂死兜底；
- os 纯逻辑 138 项与 shared ABI 10 项共 148/148；FramePool debug/release 各 16/16。release 暴露的部分 extent 归还缺陷已由沿目标路径惰性物化 occupied descendants 修复，归还复杂度保持 O(tree depth)；
- `just check`、`just build_user`、shared check、用户/内核 ELF audit 与 scoped rustfmt 全绿；正常 thread departure 不再写每线程统计，Process 强引用仍覆盖 `drop(Thread) → departure.request`。

## 竞态矩阵新增场景

1. spawn vs kill 线性化（Terminating 冻结后 spawn 拒绝；冻结前并发插入的 Staging 线程被收束吸收）；
2. 末线程 ThreadExit vs 异 hart ProcessKill（双冻结竞争，首达者胜，双侧终因均有胜出记录）；
3. join 唤醒 vs 离场时序（DONE 发布恰在成员摘除后；joiner 结果槽可见性——内存序轴断言）；
4. 并发 spawn/exit 风暴（tid 单调、表长守恒、cap 触发 ReachLimit 后恢复）；
5. 双 hart 同进程并行（同 satp、AddressSpace 事务串行化、epoch/fence.i 代次正确）；
6. Building 交错：attach/grant vs seal/kill（上行检查、abandonment 收编 nursery 与已装句柄）；
7. Start 活体门：零线程拒绝、1 与 N 线程入册各一例；
8. ProcessGrant 回滚（目标表满/失败路径句柄守恒）;
9. join 壳 close vs 线程离场（壳消散后离场照常，无泄漏无双重处置）。
10. 一 hart 缓存旧 mapping，另一 hart Unmap 完成后以不同 backing 重映射同 VA，前者不得读到旧页；
11. Unmap vs ThreadExit/ProcessKill 与 shootdown ack vs termination；
12. Endpoint HandleClose/Tunnel lease close vs 同进程普通 Unmap；
13. 次线程 guard stack、wrapper/result 槽与 join handle 的建立、离场确认、解除和复用；Map committed cookie 的发起线程消散由 join 接管。

## carryover IPC 压力线接入（触发条件 = 本里程碑落地）

- [x] 消息风暴：MailboxFull/ObjectBusy/回滚与资源守恒；
- [x] Tunnel create/attach/write/close 与进程退出风暴验证帧守恒；
- [x] host 双线程与 RISC-V 双 hart Acquire/Release 压测；
- [x] `sifive_u` 既有集成负载连续十轮。

## 文档同步清单

批一至批三的方向与实现文档均已同步：普通 StartupBlock 属用户态约定，ProcessGrant/Attach 是 Building 期独立组装动作，ProcessStart 只负责首次发布；ThreadSpawn/Exit/Yield、ThreadControl DONE、用户态 join/guarded stack 与双层结果义务分别由对应 ideas/impls 拥有篇记录。`COMPASS.md` 已转向下一自然序。

## 完成标准

- [x] 全负载等价回归、16/16 竞态矩阵与 carryover 四项全绿（debug 与 release），其中同 AddressSpace 多 hart、stale translation、Tunnel/Unmap 与 guarded stack 同时闭合 user-memory 8B；
- [x] lifecycle 无主线程特例、无单线程假设；
- [x] join 契约（DONE 电平 + 内存序）有显式 join、Drop 与结果可见性负载背书；
- [x] 出生块为用户约定，startup 内核机制删除，方向/实现文档同步完成；
- [x] 决策入档、代码由 `004cae5` 提交、COMPASS 自然序更新。
