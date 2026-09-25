# 用户态映射 owner 与运行期清理

> 状态：线程 cleanup 承接已被 [用户态分层设计](todo-2026-09-runtime-kernel-library-separation.md) 触发，进入联合设计；其余映射/堆策略继续按真实需求推进。当前分支未改 MappedRegion/Join。本记录拥有用户态运行期 cleanup、清理失败和可挂起析构的承接设计，不表示内核账本损坏。已完成的运输/执行路径由 [执行前置档案](archived/todo-2026-09-13-service-runtime-prerequisites.md) 拥有，不重复设计。

## 现状与目标

`user/rinlib/src/mm.rs` 的 MappedRegion 需要显式 unmap，失败返还 owner；遗忘 owner 不移除内核 AddressSpace 的映射。`user/rinlib/src/thread.rs` 的 UserStack/JoinHandle 是当前实现的线程资源 owner，DONE 后才可清理；目标实现把该责任移入 PTR 的 ThreadRecord。堆 arena 也有独立使用寿命。简单补一个 Drop 无法表达可失败、可挂起的 unmap，也不能提前解除活动栈或仍承载对象的 arena。

目标是让长期运行程序通过 PTR ThreadRecord 表达映射、子范围和最后访问责任；正常释放或任务取消时，失败资源仍留在 PTR 状态机中。应区分运行期 PTR cleanup 和整个进程最终回收，不能把 ProcessDrain 冒充正常清理。

## 源码复核：已有基础与实际差距

本轮按用户要求同时复核当前源码、真实消费者和 impls。结论：**不是缺少通用执行基座，而是尚未把本进程的 ThreadRecord、PTR 和 ThreadDriver 接入线程栈/环境 cleanup**。前一轮的宽泛“退休承接待设计”不能解释为下表机制都不存在；部分事实已经由 runtime/fal 实现篇记录，不能归因于文档遗漏。

| 已有机制与代码位置 | 实际责任 | 对本任务的复用边界 |
|---|---|---|
| `libexecution/work_queue.rs:90-178` insert | Task 配额、业务/执行重试 timer、输入空间与稳定记录先准备后发布；失败返还完整 task | 复用准入与稳定任务；不是随时可调用的跨线程提交队列，失败 Drop 时不能直接 spawn 再假定必成 |
| `libexecution/runtime.rs:1279-1377` prepare/adopt source | 内核登记前预付 source 记录、InputBytes 和注销重试 timer；失败退回资源 | 复用观察准备/退休；一个 task 准入成功不等于所有未来 source 都已登记，消费者要处理回执和拒绝 |
| `runtime.rs:513-535`、`work_queue.rs:394-490` | 失败保留 task/input/source/额度；业务 stop 后继续 advance；Complete 待来源注销才退款 | 清理任务不需要另一套调度、退避或注销机器；stop 的业务语义仍由该 task 定义 |
| `runtime.rs:569-612` shutdown_turn/close | seal 阻止新 task，已有任务继续清理；非空或集合关闭失败返还整个 Runtime | 复用分阶段关闭；必须在 seal 前建立承接 task，不能先停 executor 再从 Drop 提交新任务 |
| `libfal/store.rs:48-108,362-421` NodeStore | 节点由记录持有；NodeRef Drop 在最后 pin/link 消失时利用既有 Lease 入链并调用 Wake，无需分配；预算退休失败把 payload 放回原记录 | 有真实无分配退休实例，但 Rc/Cell/RefCell 是单线程、节点 pin/link 专用，不能直接当作跨线程栈队列或让 libthread 依赖 FAL |
| `libfal/provider.rs:112-217` Retirement | Notification source、take/rearm、backend 预算推进；正常退出等 backend 空且 source 实际移除 | 可复用 actor 接线方式；provider 的 grant/node 封闭条件不是线程的 DONE/Map 条件 |
| `libexecution/wake.rs:3-54` Wake/NotificationWake | 显式发布唤醒；目标 capability 准入时检查，publish 中不分配 | 保留事件发布契约；CLOSED 被接受不代表债务仍有消费者，也未提供跨线程 debt owner |
| `srv_init/supervisor.rs:137-162,294-329,1240-1308` | 预留账本后创建资源；Cleanup<T> 失败放回原槽，Disposition 退避；多 duty 公平推进，关闭失败保留 Runtime | 是正式监督模式；固定三类 Cleanup 单槽、init-local 装配不是通用线程服务，不能照搬容量或把另一进程栈交给它 unmap |
| `libprocess/supervise.rs:350-508`、`observation.rs:186-241` | 单步 Collector，停止/错误仍携完整 machine；注销完成后才交结果，同步和 Runtime 共用请求契约 | ThreadCleanup 可复用接口形状与既有执行桥接；Process REAPABLE/Drain 不等于 Thread DONE/栈收束，不强行合并领域机器 |
| `librpc/dispatcher.rs`、`caller.rs`、`exchange.rs` | CallOwner/ReplyCleanup 在取消、关闭失败后保留；已发请求不因本地取消撤回 | 证明失败责任返还已有真实消费者；不借用 RPC 类型包装线程资源 |

`notes/impls/runtime.md` 已描述 Task/Source 退休、失败保留、关闭返还和 root duty；`notes/impls/fal.md` 已描述 backend 的预登记 Notification 和显式 Wake。本轮补充它们与未来线程承接的能力边界，不宣称这些机制是新设计或已接入线程。

### 收窄后的未闭合责任

1. **PTR 并发状态**：ThreadRecord 的 atomic 状态、唯一 cleanup claim、结果发布与 ThreadControl::DONE 的内存序；避免调用方直接借用/锁住整个 Runtime。
2. **Attach/Spawn 登记时机**：子线程可能早于 syscall 返回离场，Attach/Spawn 都必须在执行前取得稳定 record、容量和 control/栈 owner；失败不能留下悬空登记。
3. **ThreadObservation 失效**：Thread 可 Clone/Drop，但不拥有 ThreadRecord；record move/Retired/driver 失效时，观察只能返回 stale/closed，不能形成悬空 Rust 引用。
4. **线程特有清理**：DONE、结果已初始化、环境退出、Map 结果义务和 stack Unmap 的一次性状态继续归 libthread；PTR driver 执行 step，不能用 ProcessDrain 替代运行期事实。
5. **失效与进程终局**：ThreadDriver、PTR 非空关闭和最后执行者边界尚未冻结；必须区分继续运行时的 PTR cleanup 和整个进程退出时的 ProcessDrain。

复用方向据此固定为：libthread 保留 ThreadRecord 与纯 cleanup step；libruntime 通过既有 Task/Requests/WaitSet/期限能力驱动 PTR。新设计聚焦 record 登记、观察失效、Attach/Spawn 交接、driver 寿命与关闭。不得先新增第二套执行器、generic task 生命周期、独立 wait hub 或复刻 FAL NodeStore。只有取证证明某个现成机制不能满足具体契约时才设计必要扩展，并登记唯一 owner。

## 本轮代码对照裁决（2026-09）

源码与现有消费者核对后，当前事实分为三类：

| 类别 | 已有事实 | 裁决 |
|---|---|---|
| 可直接复用 | `MappedRegion::unmap(self)` 失败返还 affine token；`libexecution::Runtime` 已有 Task/Source 预付、失败保留、退避、注销确认和 `seal → stop → drain → close`；`libprocess` Collector 也保留失败机器 | 不重建通用执行器、Wait hub 或第二套重试协议；PTR cleanup 以独立状态机接入 libruntime driver |
| 文档已基本准确 | `notes/ideas/task.md` 已规定 DONE、结果发布、环境退出和栈释放的先后；`notes/impls/runtime.md` 已说明 Runtime 不是线程 owner；分层计划已将 PTR/driver 列为前置 | 保留这些方向，补充本节的具体 PTR、Attach 交接、关闭顺序和故障责任；不把现有接口草案写成已实现 API |
| 当前实现缺口 | `rinlib::thread::JoinHandle::Drop` 同步等待 DONE，`UserStack::release` 在非 Busy 失败时只记 abandoned；`wait_and_release` 对 wait/Control close 使用 `expect`；当前没有 PTR/ThreadObservation/ThreadDriver | 必须在 libthread/libruntime 施工时建立 PTR ThreadRecord 和单步 driver，把可恢复错误留在 record 状态；不能以计数、panic 或 ProcessDrain 冒充运行期收束 |
| 领域不匹配 | `libexecution::Runtime`、FAL NodeStore、RootSupervisor 都有单一 owner 的逐步清理实现，但 Runtime 没有 ThreadRecord/PTR，FAL 使用单线程 `Rc/RefCell`，RootSupervisor 只覆盖 init 自有槽位 | 只复用它们的准入、逐步推进、失败保留和关闭顺序；PTR 由 libruntime 持有，libthread 提供记录和 step |
| 消费者边界 | `srv_init` 的容量校准 worker 使用 `join`；`test_hammer` 验证发布与 Drop；`test_target` 故意把 JoinHandle 留到进程终局，作为 kill 竞速 fixture | 这些是迁移和验收证据，不是长期线程服务或安全 Drop 的现成实现；新增正式能力前不扩大服务架构 |

现有 API 没有当前线程身份查询；目标 Thread 不需要读取或保存线程身份。ThreadObservation 只通过 capability、slot/generation 和原子状态观察 PTR；slot/generation 只用于过期拒绝，不用于身份政策。Attach/Spawn 的记录登记和跨线程观察规则仍需按同一 record 契约施工。

## ProcessThreadRegistry 设计裁决

线程生命周期的用户态唯一 owner 是进程级 **ProcessThreadRegistry（PTR）**。libthread 定义 ThreadRecord、ThreadObservation、结果发布和 cleanup 状态机；libruntime 建立 PTR 并拥有 ThreadDriver。libthread 不依赖 `libexecution`、`libprocess` 或具体 runtime，执行器组合属于 libruntime：

```text
libkernel  ←  libthread  ←  libruntime  →  libexecution
                    └─ ThreadRecord/PTR      └─ ThreadDriver 适配
```

PTR 是进程级稳定记录和容量 owner；Attached/Spawned 在进入执行前登记同一 ThreadRecord。公开 Thread 只持有可 Clone、可失效的观察/控制对象，不借用或拥有 ThreadRecord。ThreadFactory 只有在 PTR 记录容量、结果/栈 owner 和必要观察 capability 已准备后才对外开放；ThreadDriver 的执行形态和唤醒来源仍由本记录 TODO 持有。

### 责任槽与并发不变量

- 每个 Attached/Spawned record 在接管栈、packet、环境和 Control 前取得 PTR 记录/容量；失败路径不能临时申请第二份 owner。
- record 状态至少区分 `Registered → Starting → Running → ResultPublished/Unpublished → KernelDone → Retiring → Retired`；Thread Drop 只影响观察/控制 capability，不改变 record 生命周期。
- ThreadRecord 在稳定存储中登记；ThreadExit、取消和 syscall 失败路径只移动既有指针/状态，不分配、不在故障路径临时启动新的 driver。
- PTR 维护活跃、Ready、Retiring 和 Retired 前的责任计数；只有 cleanup 完成、结果/环境/Control/映射责任均已结算后才移除 record。通知只是提示，driver 始终以原子状态和 ThreadControl 事实为准，可合并、重复或迟到。
- 多生产者不共享 Runtime 的 `&mut`，只写各自槽位并发布一次；接收者按固定游标有界扫描 ready 槽。若后续实测证明扫描成本不可接受，再在同一 domain 内增加预付 ready 链，不另造队列。
- `RegistrySlot`、发布端和 domain 的关闭标记必须满足 Send/Sync 的实际 SAFETY 论证；环境和结果本身仍只由 DONE 前子线程、DONE 后唯一 cleanup owner 访问。

### 生命周期与关闭顺序

```text
准备 PTR 记录容量、预算和 driver 交接
  → PTR/driver 初始化，开放 ThreadFactory
  → Attached/Spawned 在进入执行前登记 ThreadRecord
  → public Thread 只取得观察/控制 capability
  → ThreadExit 后等待 ThreadControl::DONE
  → ThreadDriver 逐步 claim/reap ThreadRecord
  → seal PTR，禁止新记录；仍处理已有 record
  → 无可执行责任后关闭 driver/PTR
```

PTR 的 `seal` 可以先于生产者全部 Drop，但不能让 driver 因 stop 意图提前 Complete：seal 只关闭新 record 登记，继续消费已有 record，直到 PTR 报告没有可执行责任。具体 seal/close 顺序仍是本记录 TODO，不依赖调用者再次 spawn。

factory、PTR 和 ThreadDriver 的职责分别是：

| owner | 持有事实 | 失效时的责任 |
|---|---|---|
| ThreadFactory | 新 record 的准入权和环境模板；不持有已登记 record | Drop 只关闭新准入，不能取消已有 record |
| public Thread | 可 Clone/Drop 的观察与控制 capability | Drop 只失去本份观察权，不触碰 ThreadRecord |
| PTR | ThreadRecord、状态、栈、结果、环境、Control 和 cleanup owner | 不因 factory、Thread 或 driver 暂时失效而销毁 record |
| libruntime ThreadDriver | PTR 的单步 claim、等待来源和推进预算 | stop 后继续处理已有 record；不可恢复错误保留 PTR 状态，具体故障政策待 TODO |
| ProcessDrain | 进程终局的地址空间和内核资源回收 | 最后执行者无法继续用户态 cleanup 时兜底，不执行 Rust 析构 |

通知端使用 libkernel 的 typed NotificationSignaler 或等价无堆安全包装。signal 失败不能丢弃 PTR record：`ObjectClosed` 只把 driver 标为失效候选，record 仍由 PTR 保留。driver 失效后的接管、进程完整性故障和 ProcessDrain 边界尚未冻结，不能降级为 abandoned 计数，也不能在故障路径临时 spawn。

### ThreadCleanup 的提交与失败边界

`ThreadRecord`/cleanup step 仍由 libthread 定义，不把执行器的 Task/Source 状态混入线程状态机。它至少分开以下事实：

1. ThreadControl 是否已观察 DONE；
2. closure/result 是否已发布且只可领取一次；
3. 环境 enter/leave 是否完成，是否有残余环境状态；
4. Control close 是否完成；
5. UserStack/MappedRegion unmap 是否完成；
6. cleanup record 和 permit 是否已归还。

每一步只做一个有界动作，Busy/等待/永久错误都保留完整 record。结果可以先于 cleanup 完成而交付，但 ThreadRecord 必须继续留在 PTR；ThreadDriver、同步等待和关闭路径必须驱动同一状态机，不能各自重写 Busy 重试。

PreparedThread 的取消只处理未发布资源；ThreadSpawn 未提交失败返还 PreparedThread 或其完整 cleanup owner；内核已经提交但固定输出不完整时进入进程完整性故障，不能伪装成未启动。raw `ThreadExit(code)` 只在 libkernel 作为 unsafe 逃生口，libthread 不提供可绕过 wrapper 的安全 exit；DONE 但没有 ResultPublished 时内部保留 Unpublished 事实，不得读取或 Drop 未初始化 T。
现有 `wait_and_release` 的 `expect` 只能作为施工前的故障证据，不能沿入 libthread：wait、Control close、固定输出不一致和 unmap 永久错误必须分别落入 PTR/ThreadDriver 的明确状态；仅保留结构性不可达断言，并为每个 unsafe 读取继续保留 DONE 与初始化状态证明。

### 接口轮廓与失败表（施工前草案）

以下名称表达 owner 和状态边界，字段布局与错误枚举仍待代码施工时冻结：

```rust
pub struct ThreadRecord { /* private stack/result/environment/control */ }
pub struct ThreadObservation { /* cloneable, invalidatable observation */ }
pub struct ThreadDriver { /* libruntime-owned PTR step scheduler */ }
pub struct ThreadCleanup { /* single-step state inside ThreadRecord */ }

impl ThreadDriver {
    pub fn poll(&mut self, budget: WorkBudget) -> DriverProgress;
    pub fn close(self) -> Result<(), CloseFailure<Self>>;
}
```

PTR 不暴露 slot 地址、ThreadRecord 的可变引用或内部 cleanup owner；公开 Thread 只持有 ThreadObservation。Attached/Spawned 在 register 后共用 record 状态机。`reap_step` 的 claim 必须唯一，失败保留完整 record 和进度；driver 的具体等待来源、唤醒机制和关闭时机仍是 TODO。

| 操作 | 前置 | 正常结果 | 失败/收束 owner |
|---|---|---|---|
| PTR prepare | 记录、容量、预算和启动交接可预付 | 未开放 PTR + owner | 原始配置和未消费资源返还；不产生可用 factory |
| register Attached/Spawned | control/栈/packet/环境稳定 | PTR 持有 ThreadRecord，返回 Thread/结果领取权 | 未提交返还准备资源；内核已提交但固定输出失配进入进程完整性故障 |
| Thread::wait | 借用观察/控制 handle | Ready/TimedOut/观察错误，不移动 PTR owner | 调用者观察；PTR 继续持有 record |
| ResultReceiver::collect | 一次性结果领取权 | 已发布 T；不触碰 cleanup record | 未 DONE/未发布返还 receiver；PTR 继续推进 |
| PTR reap_step | KernelDone 且 cleanup claim 可用 | Continue/Wait/Complete/Failure | 失败保留 record/进度；不得重复领取结果或关闭 Control |
| Thread::Drop | public observation | 丢弃该 handle，不触碰 ThreadRecord | PTR 继续持有生命周期 owner |
| PTR close | seal 且所有可执行责任完成 | driver/source/record 关闭 | 非空或 driver 失效边界仍由 TODO 冻结 |

PTR 是唯一的用户态线程生命周期 owner；Thread 只拥有观察/控制 capability，ResultReceiver 只拥有一次结果领取权。同步观察、ThreadDriver 和关闭路径都调用同一 single-step cleanup 函数，避免结果读取、环境析构、Control close 或 unmap 在不同路径重复执行。

### 三个施工前窄门的冻结结果

#### 1. 稳定 record 的类型擦除与安全边界

`ThreadCleanup` 不是把 `F/T/E` 转成共享可变对象，而是 prepare 阶段一次性分配的 `Box<CleanupRecord<F, T, E>>` 的唯一 owner。Ready 槽只保存非空指针和静态 vtable；record 的实际布局、closure、结果、环境、ThreadControl、栈 owner 和付款 Charge 都留在原始稳定存储中。这样 Drop 发布只转交既有指针，不在故障路径新分配。

vtable 只允许以下操作：

- 在唯一 receiver claim 后执行一个有界 `step`，返回 `Continue/Wait/Complete/Failure`，不暴露 record 内部引用；
- 在完成或明确终局时销毁 record，并由同一个 vtable 路径 drop 已初始化或明确未初始化的 `T`；
- 释放对应的 `CleanupBytes` Charge 和 permit。

`ThreadRecord` 对外不暴露可变引用；跨线程 `Send` 的 SAFETY 前提是 record 的生产者发布后不再访问其字段，ThreadObservation 只通过原子状态和内核 capability 观察。PTR 的 atomics 只同步状态/claim，不能让 record 的字段本身并发可见。ThreadDriver 以 acquire claim 后独占一个 cleanup step，永久错误也必须保留完整 record，不得调用裸 drop。record 的 Drop 不能隐式放弃未完成 cleanup；未完成责任由 PTR 保留。

首版不把 `ThreadCleanup` 变成 `dyn Trait` 或要求 `T/E` 放入全局类型注册表。type-erased 只发生在固定 `unsafe fn` vtable 边界，所有函数指针绑定到同一 `CleanupRecord<F, T, E>`，每个 unsafe 入口写明：slot 已独占、DONE/初始化状态已满足、调用后 record 仍由返回值或 vtable owner 持有。

#### 2. 槽容量与付款预算

PTR 的容量不是经验常量，而是装配者提交的并发 ThreadRecord 上限；runtime 在开放 factory 前必须为全部可能存活的 record 预付 `RegistrySlot`。libthread 通过 `ThreadResource` 提供自己的预算分类：

```text
ThreadResource::RegistrySlot     = PTR 活跃 record 数量
ThreadResource::CleanupBytes     = 稳定 cleanup record 字节数
```

PTR 的容量预算只计算 record、状态字和 ready/观察存储；每个具体 Attached/Spawned record 在分配稳定存储前另取得 `size_of::<ThreadRecord<...>>()` 的 `CleanupBytes` Charge。Charge 跟随 record 一起保留，只有 cleanup 完成或准备阶段完整回滚才退款。slot 与 bytes 使用不同计量单位；不能用一个“线程数”同时代替两者。

预算顺序固定为：

1. 建立 `AccountView<ThreadResource>` 和 PTR 记录容量；
2. 为 libruntime ThreadDriver 的执行上下文/source 预付既有 `libexecution` 的 Task/InputBytes（若采用该装配）；
3. Attach/Spawn 交接获得 control/栈/packet/环境 owner 后登记 PTR；
4. 每个 record 先取得 RegistrySlot，再取得 record bytes 和其它资源 owner；
5. 任一后续失败按反向顺序返还尚未提交的 Charge，已登记 record 继续持有原 Charge 直到 Retired。

普通 allocator 的 OOM 不能成为 cleanup 阶段的补偿路径。record、容量和 driver 连接均在 runtime 初始化/Attach/Spawn 准入阶段取得；Thread/发布只移动已存在的 capability。容量不足在 register 或 prepare 时显式返回，不能通过临时扩大 PTR、临时 spawn 或全局 fallback 逃避预算。

#### 3. ThreadDriver 与关闭

ThreadDriver 的 owner 是 libruntime，不是 libthread 或任意创建者线程。它只负责调用 PTR 的单步推进，不取得 ThreadRecord 的长期所有权：

- driver 只报告 cleanup 的正常进度、可重试错误或不可恢复 failure；它不丢弃 PTR record。
- Notification/WaitSet 失效、driver 停止和 PTR 非空关闭必须保留完整 record 与进度。
- ProcessDrain 是最后执行者无法继续用户态 cleanup 时的终局，不执行 Rust 析构。
- driver 是否复用 `libexecution::Runtime`、是否需要独立执行上下文、何时 poll/reap、如何在 runtime 入口返回和 shutdown 前推进，均为本计划 TODO。

### 设计完成门

本节已完成 PTR owner、record 类型擦除与 SAFETY 边界、ThreadResource 预算的方向冻结。ThreadDriver 的执行形态/调度时机、ProcessAttach 的 control/栈 owner 交接、PTR 非空关闭、driver 失效和最后执行者收束仍是代码施工前的设计门；其余约束需落实到具体类型、unsafe 论证和真实消费者，并在施工后验证。

## 前置、触发与顺序

原延期建立在公共操作回收与执行/RPC/FAL 基座尚未接通的阶段。现在该基座已有实现，libthread 分层直接触发失败承接需求；本轮设计已完成，下一阶段转入代码施工前的范围确认、类型落地和消费者迁移。其它映射与堆扩展仍不自动扩大范围。

当真实长期服务需要动态管理大量普通 mapping、正式 detach/reaper，或当前调用者的正常释放无法由现有 owner 闭合时触发。触发提前发生则先重排 COMPASS；直接影响当前操作正确性的清理不得以本计划延期。

### 已触发的线程闭包

现有 `UserStack::unmap` 遇 Busy 循环睡眠，其它失败只累计 abandoned count 并保留映射到 ProcessDrain。新的 libthread/PTR 要同时保证结果初始化状态、环境 owner 和清理失败可恢复，不能沿用仅记计数的责任丢失。线程相关子集是分层迁移的必要前置；不因此重做全部 allocator 或普通映射。

Rust 不能通过 `must_use` 或仅要求显式 wait 禁止调用者丢弃 Thread；Thread Drop 只放弃观察权，PTR 继续持有 lifecycle owner。目标工厂在准备资源之前取得一个可无分配、不可拒绝登记 ThreadRecord 的 PTR 准入。PTR 必须持有完整 cleanup、付款和观察/重试状态，存活与推进不依赖原调用者或已销毁的 factory。仅接口有 step 函数不构成活性证明。

PTR owner 与 record 状态设计已完成。下一阶段是把稳定 record、ThreadResource、ThreadDriver 和 Attached/Spawned 启动交接落到 libthread/libkernel/libruntime 的具体结构，补齐 PTR 非空关闭与最后执行者收束，再迁移真实消费者；不在 libthread 另造 executor。

PTR 登记、ThreadObservation 失效和 ThreadDriver 的 safe 关闭在代码接通前仍属于**施工阻塞**；当前设计已给出唯一 owner 和单步 claim，不允许以调用者永不 Drop 或进程终局冒充正常回收。生命周期契约由 [task](../notes/ideas/task.md#用户态线程资源收束) 拥有，具体 PTR/driver 设计由本记录拥有，分层计划只引用前置。

### 自然实施与验证顺序

1. 盘点 MappedRegion、堆 arena、UserStack/ThreadObservation、对象 view 的访问寿命与真实调用者，区分 public handle 丢弃、PTR cleanup 和仍被执行的资源。
2. 设计共同的登记、状态步进、取消和失败返还契约；冻结 libruntime ThreadDriver 的执行形态及其 Attach/Spawn/关闭交接，不预设身份分支。
3. 每类资源带全部调用者共同迁移，保留 DONE/结果义务、映射同步及 backing 的安全屏障；不得以自动 Drop 提前撤销仍在使用的内存。
4. 验证进程持续运行时的成功释放、并发 Busy、取消、清理失败后继续处理，以及最终进程回收；删除旧 owner/裸清理路径。

## 完成与删除门

正常资源释放不依赖业务逐段维护内部清理状态；所有失败仍有可识别 owner，活动访问结束前不复用地址或 backing。host/目标检查与实际堆、线程、映射组合有证据。最终方向入 notes，实际 API 入 impls，完成后归档本计划。

## 跨会话接力断点

本任务仍只有文档授权，PTR owner/record 方向已完成，ThreadDriver 调度、Attach control/栈 owner 交接、关闭顺序和失败政策仍在 TODO，代码施工尚未开始。分支与未提交成果见 [分层主线接力](todo-2026-09-runtime-kernel-library-separation.md#跨会话接力断点)；下一阶段需要先冻结这些 TODO，再从本记录的接口草案开始实施。

### 先读哪里

1. 本篇「源码复核」与 [runtime 实现](../notes/impls/runtime.md#资源退休消费者与复用边界)：已实现基座、专用接线和真实差距。
2. [task 线程契约](../notes/ideas/task.md#用户态线程资源收束) 及分层计划的 libthread 接口稿：结果/cleanup 分离、DONE 屏障、环境状态和失败 owner。
3. 按已冻结施工门回到 `libexecution/runtime.rs`、`work_queue.rs`、`wake.rs`；FAL `store.rs`/`provider.rs` 与 init `supervisor.rs` 只作已落地机制对照，不把它们的领域状态抽成万能接口。

### 本轮设计产物与剩余门

本轮已经形成以下设计产物：

- owner/寿命图的文字版：factory、ThreadRecord、PTR、ThreadDriver、Runtime 和 ProcessDrain 的持有事实与失效责任见上节。
- 登记与关闭时序：Attach/Spawn 在执行前登记 record；seal 只禁止新 record；PTR 无可执行责任后才允许关闭 driver。
- 公开接口边界：Thread 观察/控制 capability、ResultReceiver 一次领取、PTR 唯一 cleanup claim、单步推进和失败保留；Thread Drop 不承担 lifecycle cleanup。
- 最小验证边界：跨线程观察失效、结果发布与 DONE 顺序、双重 claim、PTR seal 竞态、Busy/永久错误保留、Attach/Spawn 记录一致性和 driver 失效接管。

设计方向和 PTR/record owner 门已冻结。剩余工作属于代码实施与验证：将 vtable/record 的 unsafe 证明、预算扣账、ThreadDriver 接线、Attach control/栈 owner 交接、PTR 关闭和现有消费者迁移落实到源码；任何新 ABI、额外服务或超出线程 cleanup 闭包的映射/堆扩展仍需另行记录。

## 本轮复核与验证

源码与消费者复核完成，上述现状表区分已实现基座、领域专用接线及线程未接线。原始责任链走查包括准入失败返还、PTR record 保留、ThreadDriver 单步失败、非空 PTR 关闭、FAL 最后引用发布和 ProcessDrain 终局；未重新执行已有 host 用例或 QEMU。Attached/Spawned 统一 record 设计已完成，代码施工仍未开始。

文档检查：`git diff --check` 通过；本轮只修改设计/契约文档，未渲染 Mermaid、未改 Rust/Cargo/ABI/构建配置，没有提交。
