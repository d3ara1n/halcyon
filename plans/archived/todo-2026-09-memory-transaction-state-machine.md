# 地址空间事务与进程启动发布的纵向重构

> 状态：两个纵向单元及其直接完成链已按最终结构接通并通过组合压力，validated ELF 与 EXECUTE authority 联合代码门也已闭合；本实施计划现已归档。多页 Tunnel / Runnel 切片 8/9 仍暂停，历史证据、其它 findings 归属及提交后复核入口见 [`Review program`](todo-2026-09-review-program.md)。

## 目标与边界

从系统所有权和提交语义出发，收口两个相互衔接但职责不同的协议：

1. **地址空间事务**：匿名与对象来源、Running / Building authority、Tunnel lease 共用完整 MemoryChange；Commit 前失败保持资源责任，Commit 后由内核承担固定预算下的必成义务。
2. **进程构造与启动发布**：Building 各组装操作独立提交；普通 ProcessStart 与 Bootstrap 共用启动闸门、执行绑定和首次 Ready 发布。Bootstrap 的失败必须有显式收束驱动，不把完整地址空间销毁藏进 Drop。

不创建覆盖 MemoryChange、Job、RPC 和 supervisor 的万能事务框架。它们共享失败与所有权原则，不共享无意义的阶段类型或锁协议。页表是 ledger 的硬件投影，Job 是进程生命周期根，MemoryPool 是资源来源，三者不能为统一编排而合并真值。

### 必须保留的契约

- 稳定 AddressSpace identity、Unbound → Bound 一次性附入、PoolBinding 与 funded frame/charge 守恒。
- RegionLedger 的范围、权限、AllocationKey / RegionKey、authority 与 UserWriteLease 真值。
- MemoryObject 固定 backing、单向 seal、WritePermit 覆盖 reserved/live/retiring 的计数。
- execution gate 与 active 集合单一归属；Remote Pending / acquire ack / epoch 确认链。
- Commit 后 mandatory operation、线程结果义务、work debt 与 Complete 的先后关系。
- Building Bind/Map/Grant/Attach 是独立原子动作：先前成功操作的资源属于目标；后续 Start 失败不把已消费资源恢复给组装者。
- 普通已发布进程只经终止屏障和 ProcessDrain 收束；不引入内核线程、内核抢占或同步全树销毁旁路。

## 跨报告与前置归属

| 需求 | 本计划责任 | 关联证据或契约 owner |
|---|---|---|
| permit rollback、批量 view owner、Commit 后容量 | 完整地址空间事务纵向迁移 | Review A 前三项 P1 |
| Tunnel 失败、close/attach/owner 消散 | 同一 MemoryChange 的调用者与退役消费者 | Review D-1 P2-D1-03 |
| Bound 镜像失败、Bootstrap 提交窗口 | 构造失败 owner 与完整 Start 发布 | Review B-2 F-1、E-1 M3-1 |
| epoch、在途 reservation 身份与耗尽 | 在所迁移 Commit 边界消费已验证凭据；耗尽只能在不可逆点前拒绝 | [`identity-generation-boundaries`](todo-2026-09-identity-generation-boundaries.md) 拥有身份策略；相关 seam 随本计划迁移，不延后补正确性 |
| ELF 合法性 | 构造端消费统一 validated image，不复制 parser 规则 | [`admission-fail-closed`](todo-2026-09-admission-fail-closed.md) 拥有 ELF 输入子单元；接入 Bootstrap 前完成 |
| EXECUTE / RX authority | 消费已验证的映射 authority，不另造权限真值 | [`capability-owner-error-boundary`](todo-2026-09-capability-owner-error-boundary.md) 拥有 ABI 子单元；完整 RX 验收以其完成为前置 |
| 用户态 Drop、RPC reject、supervision | 不在内核事务中实现用户态政策 | 对应专题计划独立纵向收口 |

上述前置按接口依赖推进，不要求先完成相关专题的所有无关事项，也不允许为跨计划交接增加临时 adapter。

## 当前实现取证边界

本节记录当前已接通的结构事实，不代替原 Review 的目标提交证据；ELF/EXECUTE 代码前置已完成，platform 前置仍由 admission 计划拥有。

- `MemoryChangePlan → MemoryChangeReservation → PreparedMemoryChange → PublishedSpaceChange → RetiringSpaceChange` 聚合 ledger、页表、backing、object view、Remote、work debt、mandatory/result/wait 完成责任；Commit 前失败显式 rollback，Commit 后只消费已准备 owner。
- AddressSpace 的 object view 使用有容量上限的 fallible AVL；owner 强持对象 core、`ObjectViewPermit` 与 O(1) `region_count`。事务在 Reserve 聚合 `ObjectRegionDelta`，Commit 线性更新，Retire 与 ProcessDrain 不回扫 live ledger。
- anonymous backing 以 `reserved_extent_growth` 计入其它在途事务尚未兑现的最坏增长并预留 Vec 容量；退役 split 使用预付 metadata permit，并把被替换 extent 的 permit 回收到 continuation，只有实际净增长才消耗额度。
- Unmap planner 把连续覆盖请求聚合为一个页表 Remove，guard 页自然为空；Map/Protect 同代次同类批次无分配发布。guard-only Unmap 保持零 translation。
- AddressSpace epoch 在修改前以 CAS 门禁耗尽；Remote token 带不可伪造 `TableId`；全部 Remote/work/Ready 槽在 Commit 前取得。
- WaitContext 创建时预付 Finish continuation；普通通知与 Finish、MemoryChange 与 Unpublished rollback 各自在安全点总预算内保留最低推进额度，turn 无法越过保留线。
- `UnpublishedBound` 只允许显式 `publish` 或 `rollback`；Drop 只检查 affine 协议。Bootstrap 的 `SpawnedProcess` 是 crate-private `must_use` launch token，唯一调用者必须交给 launch 或显式回滚。
- ProcessCreate 与 Bootstrap 先形成 `HandleTable::PreparedCommit`，再在 `HANDLE_TABLE → JOB_INNER → LIFECYCLE` 发布区内原子提交 capability、Job membership、Running 状态和 execution binding；最终提交不返回可恢复错误。
- Tunnel 在 REAPABLE 后的 detached close 只消费现有 lease 并发布逻辑关闭，不再新建 Unmap、页表 funding、sink 或事务工作区；地址空间资源由紧随其后的 ProcessDrain 统一退役。

### 基线保留与重构约束

按完整纵向单元替换，不独立叠加局部容量或析构修补：

- `RetiredSpaceResource::View` 携带完整 view owner 的方向保留，纳入统一锁外释放出口。原表达式提取出的 `core` 仍保活对象，因此尚不能把原位置认定为已证实的 Pool/FramePool 锁内退款。
- `assemble_prepared_backing` 的一次性固定大容量预留不作为最终方案；由每次事务的存量/在途增长证明替换。
- 已提交的资金化、同步、去重和失败处理机制不整体回退；迁移时直接吸收正确行为并删除被替代的编排。

## 完成交付的直接前置

### 已完成：线程全寿命调度准入

已提交 `d453368`：`ready_queue`、Start/Bootstrap/ThreadSpawn、Ready/Running/Waiting 与终止交付按同一不可复制的执行 owner 迁移。固定 per-hart WaitPlan 槽覆盖全部 Switch 出口；不会因 trap 尾段把 Park 吸收为 Killed 而留下意图。契约由 [`ideas/task.md`](../../notes/ideas/task.md) 拥有，实现、容量证明和 API 由 [`impls/task.md`](../../notes/impls/task.md) 拥有，完整 Bootstrap gate 仍属单元二。

验证：8 项 host 测试 debug/release、`just check`、`just acceptance`（stress 16/16、release core、sifive_u core）、`virt-hetero`、`virt-nofd` 通过。提交、唤醒、轮转和退款由 allocator 计数探针验证不分配。最终集成日志 `.git/validation/acceptance-ready-final.log`；首次已知 15/16 flake 与意图接管修复前的失败日志均保留，不混作最终通过结果。

### 已完成：等待、离场与终段责任链

`WaitContext::Registration` 强持观察对象；对象在状态锁内冻结候选，后续清位不抹除命中。每项订阅预付通知 debt，每个 Context 在创建时预付 Finish debt；offer、逐项注销、timer cancel、Waiting 交付和来源析构按 16/4 安全点预算推进。主通知与 Finish 队列各有最低进展额度，持续主队列压力不能饿死完成责任。

`ThreadDeparture` 使用稳定 `MemberKey { slot, generation, tid }`，结果义务最后释放后才摘除成员并发布 DONE；成员槽复用不能误认旧 departure。每个 Process 出生时另预付 termination debt，首次终止只发布 IPI/continuation，后者逐稳定槽计费清理 Waiting/Staging。MemoryChange 完成依次推进 mandatory、result obligation 与 WaitContext，不把扫描或 fanout 藏入最后一个 owner 的 Drop。

`Process::Drop` 只接受空 HandleTable 常数终态，不再作为无界 close 兜底。`ProcessDrain` 持久化 Handle、AddressSpace、`PublishDead`、`PropagateJob` 与 `Done` 阶段；Job child/member 使用有容量上限的 fallible AVL，摘除和祖先传播不做宽度 memmove。detached Tunnel close 在 REAPABLE 后无分配、无 funding、无可恢复错误，映射资源统一由 AddressSpace drain 收束。`write_drain_result` 的提交后输出统一经过 `deliver_output` 故障政策。

validated ELF 与公共 RX capability 已分别在 admission/capability 专题闭合并由本事务消费；每次 dispatch 的保守 `fence.i` 优化继续按 `COMPASS.md` 的测量触发条件保留。

### 通用通知与稳定等待根的最终连接（已完成）

**状态：候选冻结、注册来源保活、通知/Finish 双债务、离场交付与退役连接均已交付。** 固定版本外部取证见 [`等待通知参照`](../ref-2026-09-wait-notification-research.md)。实现采用对象内单一订阅表与冻结候选，不采用下文设计期比较过的 `SignalSchema`/兴趣分组容器；下文保留为方案推导记录，不再构成待办。

#### 语义与所有权

保留 WaitCore 的单 outcome 仲裁。命中候选保留、仲裁获胜、注册清理与执行交付分别承担责任：候选不能被后续清位抹去，但也不等同于已经取得完成权。初始检查与同一对象更新的最小 item_index 契约不变；不新增跨对象真实事件时间排序。

```text
Process 生命周期 Waiting 成员 → Weak<WaitContext>
观察对象订阅项 → Arc<WaitContext> → AdmittedThread → Process
WaitContext 注册凭据 → 强持观察对象与订阅身份
观察对象 → 预付通知槽 → 有界候选扫描 / offer
WaitContext → 预付 Finish 槽 → 有界注销 / Ready 或 departure
Process → 预付 termination 槽 → 有界 Waiting/Staging 清理
```

执行责任由对象订阅项中的 Context 强引用、Context 中的对象注册凭据与 Process 中的预付 termination 槽共同闭合。该有意形成的注册环不是析构副作用：自然命中由预付 Finish continuation 逐项注销，终止由 lifecycle weak 定位 Context 并发布同一 Finish 责任。注册在注销前强持观察来源，保留已验证授权的寿命；Process 成员表不以额外强引用制造第二个生命周期真值。

`ProcessControl` 是这条规则的验收反例：其 core 回指和 core 中的 control 回指均 weak；关闭最后一个 control Handle 后，无超时 WaitMany 仍由对象订阅项保活 Context，终止 continuation 的 weak 升级因此不能失效。stress 的 kill-vs-abandon 与线程等待退出路径覆盖该闭包；它与 Ready 容量互不替代。

#### 命中批次，不延后重读 live signals

推荐按对象自己的合法普通电平集合建立兴趣掩码分组。设普通电平数为 `b`，CLOSED 独立作为全部分组的终态命中，则分组数 `B = 2^b`；当前真实可等待对象至多两种普通电平，故至多四组。对外仍逐输入项验证 role、rights、signals，不把不同角色权限合并成额外授权。

同一 Context 对同一对象的多个输入项形成一个 `WaitGroup`，记录原始 item_index/cookie/mask 与按信号位预计算的输入项集合。对象更新以同一完整快照计算命中集合，组内用最低输入位选最小 item_index；不让兴趣桶的遍历顺序决定 ABI 结果。

每个开放分组持有已预付的 `SignalEpoch`：

```text
Open ──同锁冻结完整 signals 快照并摘出整个分组──→ Frozen → Draining → Done
Open ──最后注册取消──→ Retiring → Done
```

发布只捕获有限分组并交出已有 owner，不逐等待者调用回调。新注册不加入已冻结批次；旧批次不被下一次更新覆写。注册时若 live signals 已命中则仍立即形成候选，无需创建开放批次。

所有与成员数成正比的工作留给批次游标：逐个弱引用升级、offer、注销、空槽/容器 backing 收束。不能在最后一步 clear 整个成员表或让最后一个 Arc 隐式排空批次。开放批次的空槽复用与冻结后禁止新增的纪律，必须连同 affine 注册凭据证明不发生 ABA。

按单信号位建立多链也可降低分组数量，但会给同一 WaitGroup 引入多份成员、重复候选和更多取消责任。当前少量对象条件位下优先采用精确兴趣分组；`SignalSchema` 必须显式验证 B 的容量与发布成本，不能对未来新增位静默接受指数增长。分组索引可作实现替换，候选快照与 owner 契约不变。

#### 准入、预算与离场

- 等待 Context、注册通知槽及其完成队列位置，在接受对应责任前取得真实存储与固定槽；不能在 signal、最后 permit 退役或 outcome 获胜之后首次分配。当前 `notify_work` 以 8192 个注册槽作为全局硬界，耗尽在 subscribe 阶段 fail closed；metadata sponsor 的长期归属和跨进程预算仍需与 resources 纵向接线。
- 完成队列只调度各责任 owner，不合并它们的事务状态机，也不以任意闭包掩盖工作上界。每次取出 owner 后释放队列锁，再执行一个已量化的 primitive；所有 work 类型共享安全点预算。
- 容量证明必须覆盖开放、已发布、Taken、等待清理和已交付但尚有真实引用的 owner。不能只按当前 Waiting 线程数定额，因为已交付线程可以开始下一次等待，而旧 Context/批次仍可能在退役。
- 锁顺序须冻结为对象状态 → 批次注册状态 → completion/work slot；生命周期锁只接管 Context，不在持有时进入低秩对象或队列。游标状态锁取出工作后释放，再触碰业务锁；来源 Arc 和真实 owner 在业务锁外消散。
- `ThreadDeparture` 先改为持稳定成员凭据，消除完成时线性查找/移位；结果义务解除不直接 fanout。通用通知变成固定成本发布后，重新量化“成员摘除 + DONE/REAPABLE 发布”的总成本：确为固定短 primitive 时无需机械增设离场状态机；若仍有动态责任，必须由出生时预付的 owner/游标接管，不藏在 Drop。

#### 已兑现的连接冻结门

1. `ObjectWaitState` 在对象锁内冻结 signals 快照与最低 item index；注册凭据强持观察对象，取消显式断环并精确退款。
2. 通知槽与 Finish 槽各为 8192，分别覆盖开放/排队/Taken 状态；耗尽发生在 subscribe/Context 创建前，Commit 后不申请。
3. offer、注销、timer cancel、成员摘除与来源析构逐项计费；每安全点 16 步、每债务 turn 4 步，存在 Finish 时主通知最多使用 15 步。
4. 所有 waitable object、WaitContext、Lifecycle、ThreadDeparture、定时来源和 MemoryChange 已迁移；ProcessBuilder 的虚构 WAIT/CLOSED 面已删除。

## 目标结构：供设计冻结审阅

以下规定职责和必要凭据，不预设通过增加一套同名阶段壳解决问题。编码前必须把实际方法签名、字段归属、容量公式和提交锁区写完整并确认。

### 地址空间事务的唯一编排者

```text
调用者：意图 + validated authority + 输出/lease 发布需求
    ↓
AddressSpace 事务入口
    Validate → Reserve/Prepare → 最终复检 → Commit/Publish
                   │                         │
                   └─失败责任 → 锁外 abort   └─同步义务 → Retire 游标 → Complete
```

- `memory_space` 只拥有逻辑计划与 ledger 阶段，不持内核对象、Pool 或 Remote owner。
- 内核阶段 owner 聚合对应 ledger token、页表 reservation、资源与完成义务；外部调用者不能分别推进 ledger 与硬件阶段。两层组合是长期职责分层，不是为了兼容旧接口的平行状态机。
- 由事务拥有来源表：在 Validate 的同一受保护观察中取得所有涉及对象的强引用，锁外取得 permits，重入复检几何/代次。来源表覆盖新 permit、旧 retiring permit、只读 view 与 batch 保活；失败时不靠重新查询 live view 找退款对象。
- Commit 前失败统一返回/转移完整 abort 责任，包括来源表、permits、表页、backing、view admission、输出 lease、Remote/work reservations。AddressSpace 锁内只撤销登记和摘出资源；最外层操作驱动在明确释放业务锁后消费 abort，调用点不逐项 cancel。
- Commit 后的对象来源与资源归属已经确定；可以在锁内按 ledger 决定是否摘除该空间的最后 view owner，但不得再靠查询 live ledger 补找 permit 的来源强引用。
- 完成状态按阶段互斥存储，不以数个互相独立的 `Option` 组合表达任意非法状态。Complete 必须晚于真实资源释放、ledger 收口，随后才兑销 mandatory/result obligation 并完成 waiter。

### 容量与工作预算

预留的不仅是 metadata permit，也包括承载它的实际存储槽和未来增长：

```text
所需容器容量 ≥ 当前存量 + 其它在途事务尚未兑现的增长 + 本事务最大瞬时增长
```

- backing extent 存储必须在每次切分前完成增长预留，不能用单次 funding 的 `MAX_FUNDED_EXTENTS` 充当常驻 backing 的寿命上限。
- retirement batch 保存按 ObjectId 去重的来源槽，fragment/permit 以稳定索引关联；对象集合在 Prepare 完整形成，Retire 不临时追加未知 owner。
- ledger replacements、backing/view 安装槽、页表 outcomes、对象退役槽、split 存储和 permits、epoch、Remote/work slots、输出义务一起形成 Commit 资格。
- 有容量保证的 insert/append 可以是内部实现；禁止的是 Commit 后扩容或调用者绕过容量凭据，而不是机械禁止某个 `Vec` 方法名。Rust 没有通用 no-allocation effect，私有接口/有界容器和 allocator 故障测试共同提供证明。
- 每个退役步骤的预算包含查找、容器移动、最后引用析构和触发的通知工作。不得把可增长扫描或 waiter fanout 隐藏在“一枚 permit”之下；有结构硬上界的工作须列出上界，否则纳入已有完成游标。
- epoch/identity 耗尽在 cookie、PTE 或 lifecycle 不可逆修改前检查；Commit 后不能通过 panic 定义普通容量失败。

### 锁与发布边界

锁秩以 `os/kernel/src/sync.rs::ranks` 为真值，不为容纳错误析构而调整 rank。

- 资源取得按来源锁单独执行；MemoryPool/MemoryObject 低于 AddressSpace，不能从 AddressSpace 内回取。
- Running 提交的核心顺序为 AddressSpace → Lifecycle → completion/work/Remote 所需高秩锁；Tunnel 若同时安装 Handle 与 Connection 状态，从 HandleTable → Connection 外层进入。
- Start 使用 HandleTable（如需 pin/消费）→ Job 链 → Lifecycle；Ready 容量在提交前独立预留，提交后的队列交接在业务锁释放后进行。
- 失败和退役出口必须明确指出释放的是哪些锁；“AddressSpace 锁外”不自动等于已释放所有可能被 callback 重入的业务锁。
- 内部发布由有限、已准备的领域动作构成，不向调用者开放任意 `FnOnce(&mut AddressSpaceState)` 作为正式事务入口。Handle/Connection 的原子参与由专用已准备凭据表达，不引入通用回调式事务引擎。
- PTE/epoch 发布、data fence、Remote Pending、目标本地 fence 和 ack 保持现有硬件契约；见 `references/CONTRACTS.md` 中 Supervisor Memory-Management Fence Instruction、RVWMO 与 Zifencei 条目。

### 构造与启动的所有权边界

```text
私有构造 owner ──失败──→ 显式构造收束驱动
      │
      └─完整 PreparedStart ──提交闸门──→ 已提交启动责任 ──Ready 发布──→ 完成

已发布 Building + builder authority ──PreparedStart──→ 同一提交协议
      └─准备失败：保留 Building；放弃构造时走普通终止/ProcessDrain
```

- 构造 owner 表示唯一处置责任，不宣称 `Arc<Process>` 必然只有一个强引用；Staging → Thread → Process 的环由明确生命周期动作解除。
- Bootstrap 的私有 Job member/初始 Handle 准备，与普通 Start 已经存在的 Job member/待消费 builder，是输入事实的差别，不允许变成两套提交尾段。
- PreparedStart 持有经过预留的完整线程承接存储、Ready batch、domain/requirement、Building operation、启动 authority 和必要成员/Handle 发布凭据。
- 同一闸门复检祖先 seal、Building 状态、组装操作数和线程集合，成功后只消费既有凭据。失败原样保留重试/abort 责任；成功后的 kill 由已提交启动责任与现有 lifecycle/pick gate 接管，不能丢弃尚未入队线程。
- Bootstrap 的首次 Ready 发布属于本协议，不在返回后调用可分配的普通 enqueue。启动自检不得作为不可逆提交与责任交接之间的任意可失败步骤。
- 私有 Bound 失败使用与正式 Drain 同源的资源游标，明确 active/building/mandatory/staging/handle 的前置。启动驱动可显式循环，但 Drop 本身不循环清空整树；若驱动接入运行期，则必须使用现有有界完成机制。
- boot-held payload 以唯一 funded owner 贯穿准备和交付；若需要借用几何，借用不得逃出拥有它的构造状态，不能依赖“稍后记得 install”的普通可调用旁路。

## 实施单元与完成门

### 设计冻结（已完成）

owner 图、锁序、Commit 资格、容器容量和完成步骤已经按本计划的目标结构落入类型与调用链；施工没有引入 adapter、兼容状态或第二真值。

### 纵向单元一：地址空间事务（已完成）

`memory_space` planner、AddressSpace、页表、funding、匿名/object view、Running/Building/Tunnel 调用者已纵向迁移。来源保活、O(1) view 账目、跨在途 backing 增长、epoch/Remote 身份、失败 rollback、同步与有界 Complete 已闭合；旧的裸 permit/来源回查/detached retire 旁路已删除。

### 纵向单元二：构造失败与启动发布（已完成机制接线）

`proc.rs`、`process.rs`、`job.rs`、`lifecycle.rs`、`boot.rs` 与 `sched.rs` 已接通：普通 Start、ThreadSpawn 与 Bootstrap 都在不可逆点前预留全寿命 Ready；ProcessCreate/Bootstrap 以 typed Handle commit 与 Job member 同锁区发布；未发布 Bound 由预付 continuation 显式 rollback。构造端消费私有 validated ELF，公共对象 RX 只接受 `MAP|READ|EXECUTE`，两条独立 authority 已在本机制之外完成并接入。

### 验证与报告复核

每个纵向单元自带测试、残留审计与 notes 更新，不能把清理集中在最后。两单元和直接前置闭合后执行组合压力与全部报告条目复核；其它独立 findings 未完成的报告继续保留，不能按“已处理事务部分”整篇归档。

**修改顺序不等于交付阶段。** 可以先编辑某个 crate，但工作树中途编译失败不是引入 adapter 的理由；编译器用于发现所有尚未迁移的调用点。可交付/可提交的检查点必须是采用最终接口、测试通过且旧路径已删除的完整纵向单元。不得以保持每个中间编辑状态可编译为架构目标。

## 必须新增的验证

| 类别 | 故障/交错 | 必须观察 |
|---|---|---|
| 准备失败 | backing/表页/metadata/容器/permit/Wait/Remote/work/Ready 任一预留失败 | 未发布 cookie、ledger/PTE 不变；Pool/frame/permit/Handle/Job/reservation 责任全部恢复 |
| stale | Validate 后移除来源 view、execution snapshot 改变、Start 并发 Attach/kill/seal | 明确 pre-Commit 错误，不 panic；来源强引用保持到 abort 完成 |
| 对象退役 | 同对象多 RO/RW/混合 fragment、多对象；不同批次交错；最后 Handle/view 消散 | 每笔 permit 恰一次归还、owner 不重复摘、Seal 不提前也不永久卡住 |
| 碎片存储 | 同一 backing 连续多次打洞至容量边界，穿过初始预留数量；多个在途变更 | Commit 后 allocator 禁用仍完成；增长额度不重复使用 |
| 页表批次 | 多项 Map 需要互不相同的新增表页；guard-only 零项；跨不同 region 的 Unmap/Protect | 批次总容量在 Prepare 兑现；所有合法 ledger 计划都有对应可发布形态，Commit 后不因批次形状断言失败 |
| 构造失败 | 多段 ELF 后段失败、stack/payload/table/Attach/Job/Ready 失败 | staging 环断开；bound tree 正常 drain；无孤立成员/Handle/boot-held charge |
| 已提交接管 | cookie/Start gate 后杀调用者或目标、Remote ack 延迟/重排、重复门铃 | 责任不丢；ack 前不复用；Complete 前不越过 mandatory/join 屏障 |
| 有界进展 | drain/retire `budget=1`、最终对象析构、Seal/close 通知 fanout | 每步 work 诚实计费，无隐藏无界循环或普通分配 |
| 身份边界 | 错误实例/代次/阶段、epoch 最大值 | 修改前拒绝，绝不先环回再报错 |

- host debug/release 覆盖 ledger、page_table、funded_frame、memory_pool、metadata_admission、work_debt、remote_call、handle_table、调度相关模型；shared 纵向 ABI 变更另跑 shared。
- `just check`、`just virt`、`just virt-release`、`just virt-stress`、`just acceptance`；涉及 Start/domain 另跑 `virt-hetero` / `virt-nofd`。`acceptance` 已包含 sifive_u，专项负向启动另留独立日志。
- QEMU/内核注入补真实锁阶、最后 Arc 析构、跨 hart 与启动失败；纯逻辑测试不能替代这些证据。已知有限轮次竞态 flake 按 KNOWN_ISSUES 判读，不据单次 124 宣称内核挂死。
- 残留搜索按语义逐项检查：旧阶段入口、裸 permit 返回、重新查找退款来源、页表/extent 来源旁路、任意发布闭包、后置分配、隐藏 drain Drop、重复 lifecycle/owner 真值、文档声称已完成的未实现目标。

## 当前交付状态

地址空间与生命周期纵向机制已完成实现、残留审计和组合压力；host planner/page-table/handle/work-debt/ordered-table/Ready/Remote/WaitContext 与 shared 测试全部通过。`just check`、virt core/stress、release core、sifive_u、hetero 与 nofd 均通过；stress 覆盖 260 页 fragmented backing、同地址空间多 hart、1024 线程、Tunnel 16 轮和 16/16 竞态矩阵。

validated ELF admission 与公共 MemoryObject EXECUTE capability 均已完成并接入，地址空间事务的联合代码门闭合。本计划已归档；提交后 Review 按 Review program 另行执行，platform、RPC 与监督 findings 的实现记录分别归入同批其它专题档案。
