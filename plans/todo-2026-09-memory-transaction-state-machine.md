# 地址空间事务与进程启动发布的纵向重构

> 当前主线先闭合完整完成链的直接前置，再冻结地址空间事务与启动提交；调度全寿命准入已完成代码迁移及集成验证，其余核心迁移尚未开始。多页 Tunnel / Runnel 切片 8/9 不推进。本计划拥有事务失败闭包与启动发布的实施责任；历史证据、其它 findings 归属及复核入口见 [`todo-2026-09-review-program.md`](todo-2026-09-review-program.md)。

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

本节记录尚待闭合的设计输入，不代替原 Review 的目标提交证据。已完成前置的实现与测试记录见下节；不得将前置验证当成地址空间事务整体完成。

- ledger 已有消费式阶段类型；内核仍以 `MemoryChangePlan`、`MemoryChangeReservation`、`PreparedMemoryChange`、`PublishedSpaceChange`、`RetiringSpaceChange` 跨调用点编排。问题不是名称层数，而是调用者仍能拆开发布和归还责任。
- `ReclaimedTableFrames` / `ObjectMapFailure` 携带裸 permits，Running 与 Tunnel 仍分别执行提取、cancel 与 rollback。Tunnel complete 失败的第二次 `take_permits` 当前取得空集合，不能据两次调用就判为实际双 cancel。
- `acquire_view_permits` 在 Validate 解锁后重新查 `view_core`；Retire/rollback 也有来源查询。计划与最终归还之间的来源保活不能靠 live ledger 恰好仍有记录。
- retiring object 容量已有去重预留，但 batch-local owner 仍在 Retire 时构造；重复对象、并行批次与最后 live view 消散须一起证明。
- `OwnedBacking::release_one` 会 remove/insert extents。只在 backing 创建时预留“初始 extent 上限 + 单次 split 上限”，不能覆盖多次部分 Unmap 的累计碎片，也不能表达在途事务已占的增长责任。
- `commit_shootdown` 接受任意发布闭包；epoch 检查发生在原子增量之后。预算、身份门禁和发布动作尚未成为完整提交包。
- `TableTree::prepare` 每次仅为单个 translation 的需求预留 `owners` 容量，而 `publish_batch` 顺序发布多个已准备项；前置逐项 reserve 不能证明批次累计增长。该函数还断言批次非空且多项只能是 Map，与 ledger 可产生的 guard-only Unmap（零项）和多 region Unmap/Protect（多项）不匹配。内核 `commit_inner` 在 ledger Commit 后直接调用它；必须整体冻结账本与页表的批次契约，不能按用户场景追加绕过分支。
- `UnpublishedBound` 显式 rollback 与 Drop 重复终止/摘 Staging/drain 循环；每次 `drain_batch(16)` 有界不意味着外层循环或 Drop 有界。
- Bootstrap 已在提交前取得统一调度准入，并向 boot 交付带 credit 的线程；Handle/Job/lifecycle 仍分段推进，提交与首次 Ready 之间仍有 syscall 自检。完整 Start 协议的审查边界继续包含 `boot.rs` 与 `sched.rs`。

### 基线保留与重构约束

按完整纵向单元替换，不独立叠加局部容量或析构修补：

- `RetiredSpaceResource::View` 携带完整 view owner 的方向保留，纳入统一锁外释放出口。原表达式提取出的 `core` 仍保活对象，因此尚不能把原位置认定为已证实的 Pool/FramePool 锁内退款。
- `assemble_prepared_backing` 的一次性固定大容量预留不作为最终方案；由每次事务的存量/在途增长证明替换。
- 已提交的资金化、同步、去重和失败处理机制不整体回退；迁移时直接吸收正确行为并删除被替代的编排。

## 完成交付的直接前置

### 已完成：线程全寿命调度准入

已提交 `d453368`：`ready_queue`、Start/Bootstrap/ThreadSpawn、Ready/Running/Waiting 与终止交付按同一不可复制的执行 owner 迁移。固定 per-hart WaitPlan 槽覆盖全部 Switch 出口；不会因 trap 尾段把 Park 吸收为 Killed 而留下意图。契约由 [`ideas/task.md`](../notes/ideas/task.md) 拥有，实现、容量证明和 API 由 [`impls/task.md`](../notes/impls/task.md) 拥有，完整 Bootstrap gate 仍属单元二。

验证：8 项 host 测试 debug/release、`just check`、`just acceptance`（stress 16/16、release core、sifive_u core）、`virt-hetero`、`virt-nofd` 通过。提交、唤醒、轮转和退款由 allocator 计数探针验证不分配。最终集成日志 `.git/validation/acceptance-ready-final.log`；首次已知 15/16 flake 与意图接管修复前的失败日志均保留，不混作最终通过结果。

### 已完成：等待来源保活、命中快照与有界通知排水

当前工作树已实现并验证等待闭包的三项前置：`WaitContext::Registration` 强持对象来源；`ObjectWaitState::update` 在对象锁内冻结 `WaitOutcome` 候选，后续清位不抹除命中；`take_completer` 不重读 live signals，并保留同一 WaitMany 的最小 `item_index`。每个 RegisteredSubscription 预付一个固定通知槽；对象发布交出 one-shot `notify_work::WorkDebts` 任务，安全点按 16/4 预算推进，剩余债务重排。所有 waitable object 已接入同一 drain trait，ProcessBuilder 的不可达 WAIT/CLOSED 面已删除。

验证：`just check`、`ready_queue`/`wait_context` host tests、`virt` 与 `virt-stress` 通过（16/16）。本切片的预算覆盖 offer、候选移交和注册槽；尚未覆盖 ThreadDeparture/ProcessDrain 成员摘除、对象 backing 析构及 detached Tunnel close 的完整后置责任。

### 尚待冻结：所有完成责任的诚实预算与不可失败来源

以下仍在本计划的同一闭包内，不能只修 MemoryChange 的局部尾段；自然序是通用通知/离场交付责任 → MemoryChange 与 Tunnel 退役 → ProcessDrain 完整终段。不是先改纯逻辑 crate 再接 adapter，各前置须与全部实际发布者和消费者一起交付。

| 现状与位置 | 最终责任与收口门 | 验证 |
|---|---|---|
| `RetiringSpaceChange::advance` 在退役中填充来源表，permit 仍可能回查 `view_core`；`release_view_region` 查询 live regions | Prepare 为全部 retiring fragment/permit 捕获稳定来源与索引；最后 view owner 的交接独立于后续 live ledger；删除退役时补找来源 | Protect→Unmap、两个 Release 批次反序退役；不出现 view 消散后的查询/析构错误 |
| 同一 retire unit 含来源扫描、最多 4096 region 扫描及 `retiring_views.clear()` | 每个扫描、owner 摘除/析构都有计费单位或可续进游标；若接受固定结构上限，必须给出完整单步 WCET，不只引用 batch budget | 边界容量、反序批次、最重析构时实际工作与声明预算一致 |
| `Notification::signal`、`Endpoint::set_signals` 在 update 后解锁，finish_waiters 重取锁；消费者可在间隙清位 | 对象发布与候选快照捕获在同一临界段，不把“同一把锁”误当“同一临界段”；后续清位不抹掉候选，也不合并不同更新的 observed | 所有可清除电平的 set→clear；尤其 Notification signal/take 与 Tunnel notify/ack |
| `WaitContext::Registration` 弱持观察对象，Waiting 成员也仅弱持 Context；对象队列是部分等待的唯一强根 | Waiting 执行责任由调用方生命周期持有，注册强持观察授权，对象订阅弱指 Context；完成/取消显式切断各边，不能由最后 Handle 消散暗中丢掉线程 | 无限等待时关闭最后一个目标 control Handle，随后目标完成及调用方 kill，均无孤儿 Waiting/Exiting |
| `ObjectWaitState::take_completer` 原先由每个对象同步循环；现已接入 `notify_work::WorkDebts`，每订阅预付槽并按 16/4 推进 | 所有 waitable object 共用候选快照、固定槽与 drain trait；剩余 offer/注销责任可重排，不为内存对象另造通知尾段 | 混合兴趣、Installing/Deferred、取消竞态、CLOSED 全兴趣匹配；槽耗尽 fail closed；最多 1024 waiter 和每等待最多 64 订阅的完整成本 |
| `ThreadResultObligation::Drop` 可直接摘成员并触发 REAPABLE/DONE fanout | 以稳定成员凭据和固定成本通知消除隐式动态工作；仍需异步的离场责任由出生时预付 owner 接管。最终选择须服从完整 primitive 上界，不机械增加状态机 | kill 早于事务完成、末线程离场、结果晚到；DONE 不早于结果义务解除 |
| `Tunnel::close_detached` 在 REAPABLE 后才分配 sink、规划页表并资金化 | Endpoint/lease 的准入必须支付未来不可避免的退役，或消费已有可复用退役工作区；显式 close 与 detached close 共用最终事务，不以 OOM 重试掩盖收束缺口 | 准入后持续 allocator failure 仍可完成关闭与 CLOSED，无新增 sink 或 funding 来源 |
| `ProcessDrain` 在返回资源 complete 后于预算外 publish_dead、Job unlink、最多 32 层祖先完成 | Process/Job 收尾进入持久终段，稳定成员凭据与通用通知责任保证进度；Complete 只在责任已完成或不可丢失地交出后返回 | 最小预算、深 Job 链、大量 waiter、管理者交替推进与终态观察 |
| `sched::run` 在 epoch execution gate 之外仍有每次 dispatch 的无条件 `fence.i` | 冻结 Building 写、首次进入、跨 hart dispatch 和 Running 变更的 instruction epoch 证明；由统一同步协议承担责任后删除附加路径 | 多 hart RX/首次执行/迁移及 release 验证，不以重复 fence 掩盖 epoch 缺口 |
| `write_drain_result` 的最终 `check_range` 可在业务副作用后绕过 `deliver_output` 的失败政策 | 最终交付统一经过受保护输出边界；初始无副作用验证和提交后交付不能混成同一拒绝出口 | 最终 Drain 批次与输出页 Unmap 竞态，保持统一故障终因与已提交目标状态 |

本表是上述缺口的唯一实施真值；完成一项即删除对应待办并转入长期 notes，不口头延期。

### 通用通知的设计候选：命中批次与稳定等待根

**状态：第一版固定槽与 primitive 预算已实现；仍需把对象析构、ThreadDeparture、ProcessDrain 与 detached Tunnel 的后置责任接入同一预算表。** 固定版本外部取证见 [`等待通知参照`](ref-2026-09-wait-notification-research.md)；不能从 Zircon 的同步 observer、seL4 单目标 signal 或 managarm WorkQueue 推出本系统已满足预算。

#### 语义与所有权

保留 WaitCore 的单 outcome 仲裁。命中候选保留、仲裁获胜、注册清理与执行交付分别承担责任：候选不能被后续清位抹去，但也不等同于已经取得完成权。初始检查与同一对象更新的最小 item_index 契约不变；不新增跨对象真实事件时间排序。

```text
Job 的活体根 → Process 生命周期 Waiting 成员 → Arc<WaitContext>
WaitContext → AdmittedThread → Process
WaitContext → 注册凭据 → 强持观察对象 / 命中批次
观察对象 → 开放命中批次 → 弱 WaitContext + 同对象输入项组
已冻结批次 → 预付 work debt → 有界 offer / 清理
WaitContext 完成 owner → 解除注册与 Waiting 挂接 → Ready 或 departure
```

执行责任以 Process 的活体根为依托；订阅不是线程的唯一保活根。Waiting→完成/终止必须显式提取或解除生命周期中的 Context 强引用，不能只是把 Weak 改成 Arc 后保留隐式循环。注册持有观察来源直至注销，保留已验证授权的寿命；对象侧弱引用只用于投递，不决定线程是否存活。

`ProcessControl` 是关键反例：其 core 回指和 core 中的 control 回指均 weak。无超时的 WaitMany 安装完成后，最后一个 control Handle 关闭可以消散唯一对象队列强根；现有 `take_first_waiting` 又会把失效 weak Context 对应成员转 Exiting。该静态引用图已取证，需补真实验收，不能把它误判为 Ready 容量回归。

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

#### 编码前仍须完成的冻结门

1. 给出 `SignalSchema`、`SignalEpoch`、注册凭据和完成队列的实际字段/方法、存量与在途容量公式、metadata sponsor 归属及耗尽出口；不使用任意大数组代替证明。
2. 对候选冻结/清位、重复输入最小索引、Installing/Deferred、timeout/kill 与后台投递的交错建立 host 模型；同时证明原 Handle 关闭时的稳定根、取消时断环与每项注册精确归还。
3. 用真实 primitive 计费：发布 B 组、逐注册 offer/cancel、timer 注销、成员摘除、来源与存储析构分别列出上界。然后确定是否需要独立 departure debt，并保证最小 work turn 能推进最重合法 primitive。
4. 一次迁移所有真实 waitable object、WaitContext、Lifecycle、ThreadDeparture、定时来源和既有 MemoryChange work 接线。ProcessBuilder 的 allowed_rights 不含 WAIT，其 CLOSED 等待实现属于不可达残留，应删除，而不是增加一个虚构的用户等待用例。

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

### 设计冻结（当前）

先完成以下交付，不改核心代码：

1. 本计划与关联专题的 owner/依赖矩阵，修订 notes 方向和实现现状口径。
2. 对应实际模块的最终类型/方法图、资源字段转移表、跨锁失败路径及 Commit 动作表。
3. 每类容器的存量/在途/瞬时增长公式，退役与通知的实际 work unit 上界。
4. Bootstrap/普通 Start 的共用提交协议，私有失败与已发布 ProcessDrain 的分界。
5. 失败注入位置、守恒计数、并行事件序列与删除清单。

以上全部确认后再编码；无法证明的前置先交回其契约 owner，不能用临时类型填空。

### 纵向单元一：地址空间事务

一次迁移 `memory_space` 的必要接口、内核 AddressSpace、funding/abort/retire、匿名/object/Building/Tunnel 的全部相关调用点及测试。设计冻结须先处理上节 Ready 容量前置，证明完成链实际达到不可失败的调用者交付，而不只停在 ledger Complete。使用最终接口直接迁移，不先保留旧入口完成独立 crate 里程碑。

完成门：来源保活、存储预算、epoch/同步、abort 和 Complete 闭合；原 `ReclaimedTableFrames::take_permits`、平行 abandon/complete helpers、任意发布闭包及被替代阶段入口同单元删除。私有 Bound 整体构造与 Start 尚未完成不能被写成这个单元的成果。

### 纵向单元二：构造失败与启动发布

在单元一和统一 ELF admission 接口成立后，一次迁移 `proc.rs`、`process.rs`、`job.rs`、`lifecycle.rs`、`boot.rs`、`sched.rs` 的构造/start/Ready 接线与失败测试。

完成门：普通 Start 与 Bootstrap 共用提交协议；未发布失败有显式 drain 驱动；无重复 rollback/Drop 脚本、Bootstrap 专用后置 enqueue、未登记的 payload owner 安装窗口。成功的普通 Building 组装语义不变。

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

当前只完成统筹方向与计划重排；精确类型/方法签名、容量证明和组合故障模型仍待设计冻结审阅。未执行本轮代码迁移或新增验收，不允许以既有 `just check` / host 成绩宣称本计划完成。代码实施、提交与最终对外反馈各按用户确认的边界进行。
