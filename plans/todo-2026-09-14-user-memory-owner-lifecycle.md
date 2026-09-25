# 用户态映射 owner 与运行期清理

> 状态：线程退休承接已被 [用户态分层设计](todo-2026-09-runtime-kernel-library-separation.md) 触发，进入联合设计；其余映射/堆策略继续按真实需求推进。当前分支未改 MappedRegion/Join。本记录拥有用户态运行期退休、清理失败和可挂起析构的承接设计，不表示内核账本损坏。已完成的运输/执行路径由 [执行前置档案](archived/todo-2026-09-13-service-runtime-prerequisites.md) 拥有，不重复设计。

## 现状与目标

`user/rinlib/src/mm.rs` 的 MappedRegion 需要显式 unmap，失败返还 owner；遗忘 owner 不移除内核 AddressSpace 的映射。`user/rinlib/src/thread.rs` 的 UserStack/JoinHandle 持有仍被执行的栈和结果记录，DONE 后才可清理；堆 arena 也有独立使用寿命。简单补一个 Drop 无法表达可失败、可挂起的 unmap，也不能提前解除活动栈或仍承载对象的 arena。

目标是让长期运行程序通过完整 owner 表达映射、子范围和最后访问责任；正常释放或任务取消时，失败资源有可继续处理的拥有者。应区分运行期显式退休和整个进程最终回收，不能把等待杀进程作为正常清理。

## 源码复核：已有基础与实际差距

本轮按用户要求同时复核当前源码、真实消费者和 impls。结论：**不是缺少执行/退休基座，而是尚未把本进程的线程栈和环境 cleanup 接入已有基座**。前一轮的宽泛“退休承接待设计”不能解释为下表机制都不存在；部分事实已经由 runtime/fal 实现篇记录，不能归因于文档遗漏。

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

1. **同进程跨线程交付**：ThreadCleanup 的预付槽、发布内存序和同时 Drop 的并发规则；避免调用方直接借用/锁住整个 Runtime。现有 NodeStore 单线程结构不足以证明此点。
2. **工厂与 executor 的装配寿命**：worker 可提交时，接收 task 与 Notification 必须先就绪；executor 退出不得留下仍可提交的新责任。factory 的销毁不取消已准入槽。
3. **无拒绝承接与普通准入的差别**：Runtime::spawn 是可失败准入，不能在 Drop 代替先前取得的接收承诺。应优先让一个已准入 task 持有或观察明确的责任域，发布只变更预备状态；具体槽/观察数量和预算在这里确定。
4. **线程特有清理**：DONE、结果已初始化、环境退出、Map 结果义务和 stack Unmap 的一次性状态，继续归 libthread。既有 executor 的 Task Complete/来源退休不能替代这些业务事实。
5. **失效与进程终局**：Runtime、WorkQueue 或 NodeStore 非空 Drop 都不是续作保证；当前分别保留缺陷计数/进程边界兜底，不能据此宣称执行者自身丢失后仍自动工作。正式配置必须明确继续运行时的接管和整个进程退出的区别。

复用方向据此固定为：libthread 保留纯清理机器；上层组合通过既有 Task/Requests/WaitSet/期限推进它；新设计聚焦预备责任域及其跨线程交付、寿命与关闭。不得先新增第二套执行器、generic task 生命周期、独立 wait hub 或复刻 FAL NodeStore。只有取证证明某个现成机制不能满足具体契约时才设计必要扩展，并登记唯一 owner。

## 本轮代码对照裁决（2026-09）

源码与现有消费者核对后，当前事实分为三类：

| 类别 | 已有事实 | 裁决 |
|---|---|---|
| 可直接复用 | `MappedRegion::unmap(self)` 失败返还 affine token；`libexecution::Runtime` 已有 Task/Source 预付、失败保留、退避、注销确认和 `seal → stop → drain → close`；`libprocess` Collector 也保留失败机器 | 不重建通用退休执行器、Wait hub 或第二套重试协议；线程 cleanup 以独立状态机接入既有 Runtime |
| 文档已基本准确 | `notes/ideas/task.md` 已规定 DONE、结果发布、环境退出和栈释放的先后；`notes/impls/runtime.md` 已说明 Runtime 不是跨线程接收端；分层计划已将线程退休列为前置 | 保留这些方向，补充本节的具体接收域、关闭顺序和故障责任；不把现有接口草案写成已实现 API |
| 当前实现缺口 | `rinlib::thread::JoinHandle::Drop` 同步等待 DONE，`UserStack::release` 在非 Busy 失败时只记 abandoned；`wait_and_release` 对 wait/Control close 使用 `expect`；结果 packet、环境状态和栈没有可交给外部推进者的统一 owner | 必须在 libthread 施工时改为可转交的 `ThreadCleanup`，把可恢复错误变成 owner/状态返回；不能以计数、panic、进程最终 Drain 或调用者永不 Drop 作为运行期收束 |
| 领域不匹配 | `libexecution::Runtime`、FAL NodeStore、RootSupervisor 都有单一 owner 的退休实现，但 Runtime 没有跨 OS 线程公开接收端，FAL 使用单线程 `Rc/RefCell`，RootSupervisor 只覆盖 init 自有槽位 | 只复用它们的准入、逐步推进、失败保留和关闭顺序；线程接收域由 libthread 拥有，组合层适配 Runtime |
| 消费者边界 | `srv_init` 的容量校准 worker 使用 `join`；`test_hammer` 验证发布与 Drop；`test_target` 故意把 JoinHandle 留到进程终局，作为 kill 竞速 fixture | 这些是迁移和验收证据，不是长期线程服务或安全 Drop 的现成实现；新增正式能力前不扩大服务架构 |

现有 API 没有当前线程身份查询；现有消费者也没有跨线程传递 `JoinHandle` 的需求。因此首个实现不新增 current Tid ABI，而让 `JoinHandle` 保持线程局部（移除现有 `unsafe impl Send`）；可跨线程转交的只有已经稳定、类型擦除且满足 SAFETY 的 `ThreadCleanup` record。若未来需要跨线程 Join，再单独引入 current Tid 与自 join 契约。

## 接收域设计裁决

线程 cleanup 的唯一领域 owner 是 **libthread**。它只拥有线程结果、环境状态、ThreadControl、UserStack 和 cleanup 状态机，不依赖 `libexecution`、`libprocess` 或具体 runtime。执行器组合属于上层：通常由 libruntime 或服务监督者把 libthread 的 cleanup receiver 包装成一个已有 `libexecution::Task`，因此依赖保持单向：

```text
libkernel  ←  libthread  ←  libruntime / service supervisor  →  libexecution
                                     └─ ThreadCleanupTask 连接两者
```

`RetirementDomain` 是 libthread 的无业务固定容量接收域；`ThreadFactory` 只有在接收 task、观察 source 和通知发布端全部准备完成后才对外开放。它不创建后台线程，也不隐式启动消息泵。

### 责任槽与并发不变量

- 每个 `PreparedThread` 在分配 packet、环境和栈之前取得一个不可拒绝的 `RetirementPermit`；容量和费用由显式 domain 配置承担，失败路径不再申请新槽。
- 槽状态为 `Reserved → Ready → Claimed → Empty`。生产者只拥有自己的 Reserved 槽；发布前写入稳定的 type-erased cleanup record，随后以 release 发布 Ready。接收者以 acquire claim，完成后清除 record 并释放槽。
- cleanup record 在 prepare 阶段取得稳定存储；Drop、取消和 syscall 失败路径只移动既有指针/状态，不分配、不调用普通可失败 `Runtime::spawn`。
- domain 维护 outstanding 计数，包含 Reserved、Ready 和 Claimed；只有 cleanup 完成、结果/环境/Control/映射责任均已结算后才递减。通知只是提示，接收者始终以槽状态为准，可合并、重复或迟到。
- 多生产者不共享 Runtime 的 `&mut`，只写各自槽位并发布一次；接收者按固定游标有界扫描 ready 槽。若后续实测证明扫描成本不可接受，再在同一 domain 内增加预付 ready 链，不另造队列。
- `RetirementPermit`、发布端和 domain 的关闭标记必须满足 Send/Sync 的实际 SAFETY 论证；环境和结果本身仍只由 DONE 前子线程、DONE 后唯一 cleanup owner 访问。

### 生命周期与关闭顺序

```text
准备 domain 槽/通知
  → 接收 task 已准入并注册 source
  → 打开 ThreadFactory
  → 每次 prepare 先取得 permit，再准备 packet/环境/栈
  → start 发布 ThreadControl；Join 或 Drop 负责 DONE/cleanup
  → 同步完成则释放 permit；失败或异步路径发布 Ready
  → receiver task 逐步 claim/重试，完成后释放槽并退款
  → seal factory，禁止新 permit；仍接受已有 permit 的 publish
  → 等 outstanding = 0，再移除 source、关闭 receiver、Runtime close
```

Runtime 的全局 `seal` 可以先于生产者全部 Drop，但不能让 cleanup task 因 stop 意图提前 Complete：cleanup task 的 stop 只关闭新 permit，继续消费已有槽，直到 domain 报告 `sealed && outstanding == 0`。因此“Runtime 已 seal 但合法生产者仍退出”由 domain 状态承接，不依赖调用者再次 spawn。

factory、receiver task 和外层监督者的职责分别是：

| owner | 持有事实 | 失效时的责任 |
|---|---|---|
| ThreadFactory | 新 permit 的准入权和环境模板；不持有已发布 cleanup | Drop 只 seal/释放模板，不能取消已有 permit |
| PreparedThread/JoinHandle | 未发布任务、ThreadControl、packet、栈和一个 permit | start 失败、cancel、join、Drop 必须消费或发布 permit 一次 |
| RetirementDomain | 固定槽、发布状态、outstanding 和 receiver-lost 标记 | 保留已发布 record；不因 factory 消失而销毁 |
| ThreadCleanupTask | receiver source、当前 cleanup machine 和 cleanup 步进预算 | stop 后继续 drain；不可恢复错误交给外层 supervisor，不能丢槽 |
| 外层 supervisor | task、Runtime、domain 的最终接管 | receiver 失效时恢复推进或按明确进程故障政策收束；不能把 abandoned 计数当成功 |

通知端使用 libkernel 的 typed NotificationSignaler 或等价无堆安全包装。signal 失败不能丢弃 Ready 槽：`ObjectClosed` 只把 domain 标为 receiver-lost，record 仍由 domain 保留。首个实现把 receiver-lost 视为该执行域的进程完整性故障，由预先存在的 supervisor 负责终止或接管；只有另有预付的独立接收 task 时才允许重启。其它违反已建立 receiver lease 的错误同样不能降级为 abandoned 计数。

### ThreadCleanup 的提交与失败边界

`ThreadCleanup` 仍由 libthread 定义，不把执行器的 Task/Source 状态混入线程状态机。它至少分开以下事实：

1. ThreadControl 是否已观察 DONE；
2. closure/result 是否已发布且只可领取一次；
3. 环境 enter/leave 是否完成，是否有残余环境状态；
4. Control close 是否完成；
5. UserStack/MappedRegion unmap 是否完成；
6. cleanup record 和 permit 是否已归还。

每一步只做一个有界动作，Busy/等待/永久错误都保留完整 machine。`JoinReport` 在 outcome 已确定后仍可携带 Pending cleanup；Drop 不能返回该报告，因此只允许在 prepare 时已经取得 permit 的情况下把 machine 发布给 domain。同步 join、receiver task 和最终 supervisor 必须驱动同一状态机，不能各自重写 Busy 重试。

PreparedThread 的取消只处理未发布资源；ThreadSpawn 未提交失败返还 PreparedThread 或其完整 cleanup owner；内核已经提交但固定输出不完整时进入进程完整性故障，不能伪装成未启动。raw ThreadExit 仍是 unsafe 逃生口，DONE 但没有 ResultPublished 时只能生成 Incomplete，不得读取或 Drop 未初始化 T。
现有 `wait_and_release` 的 `expect` 只能作为施工前的故障证据，不能沿入 libthread：wait、Control close、固定输出不一致和 unmap 永久错误必须分别落入 Join/cleanup/supervisor 的明确 owner；仅保留结构性不可达断言，并为每个 unsafe 读取继续保留 DONE 与初始化状态证明。

### 接口轮廓与失败表（施工前草案）

以下名称表达 owner 和状态边界，字段布局与错误枚举仍待代码施工时冻结：

```rust
pub struct RetirementDomain { /* fixed slots, notification, counters */ }
pub struct RetirementPermit { /* one reserved slot; Send, not Clone */ }
pub struct ThreadCleanup { /* type-erased stable record, single-step machine */ }
pub struct ThreadCleanupTask { /* adapter owned by the Runtime task */ }

impl RetirementDomain {
    pub fn prepare(/* capacity, budget, notifier */)
        -> Result<(Self, RetirementReceiver), DomainPrepareFailure>;
    pub fn open(self, receiver_ready: ReceiverReady)
        -> Result<(Self, RetirementPublisher), DomainOpenFailure>;
    pub fn seal(&self);
    pub fn state(&self) -> DomainState;
}

impl RetirementPublisher {
    pub fn reserve(&self) -> Result<RetirementPermit, PermitFailure>;
}

impl RetirementPermit {
    pub fn publish(self, cleanup: ThreadCleanup) -> PublishStatus;
    pub fn release(self) -> Result<(), PermitReleaseFailure>;
}
```

`RetirementDomain` 本身不暴露 slot 地址、关闭权或可变 Runtime；`RetirementReceiver` 只能由已准入的 ThreadCleanupTask 消费，`RetirementPublisher` 只能 reserve/publish。`open` 的成功条件是 receiver task 已有稳定 source 和通知接收 owner；没有 open 就不能构造 ThreadFactory。`publish` 的返回值只报告 receiver-lost 等执行域状态，绝不表示 cleanup 已被丢弃；Ready record 始终留在 domain。

| 操作 | 前置 | 正常结果 | 失败/收束 owner |
|---|---|---|---|
| domain prepare | 固定槽、通知和账本可预付 | 未开放 domain + receiver owner | 原始配置和未消费资源返还；不产生可用 factory |
| receiver open | task/source/通知均已接通 | 发布端可 reserve | 保留 receiver/domain，关闭已注册 source；不能开放 factory |
| prepare thread | permit 先成功 | PreparedThread | 原 closure、环境和已取得资源由错误携回；已产生的 map 走同一 permit |
| start | packet/环境/栈稳定 | JoinHandle（线程局部） | 未提交返还 PreparedThread；内核已提交但固定输出失配进入进程完整性故障 |
| join | 借用/消费 JoinHandle，等待可用 | outcome + Complete/Pending cleanup | timeout/观察错返还完整 JoinHandle；cleanup 错误不重复领取 outcome |
| Drop | JoinHandle 线程局部，permit 已预付 | 同步完成或 Ready 发布 | publish 失败只标记 receiver-lost，domain 保留 record；supervisor 负责故障策略 |
| receiver claim | Ready 槽 | 独占 ThreadCleanup machine | 进度不足保留 Claimed；Busy/永久错误携完整 machine 重试或上交 supervisor |
| domain close | sealed 且 outstanding 为零 | source/通知/槽全部关闭 | 任一 owner 未清空则返还 domain，不伪造 Drained |

`JoinHandle` 的线程局部限制是首个实现的自 join 边界；`ThreadCleanup` record 才是跨线程 owner。同步 `join`、Drop 发布和 receiver task 都必须调用同一 single-step cleanup 函数，避免结果读取、环境析构、Control close 或 unmap 在不同路径重复执行。

### 三个施工前窄门的冻结结果

#### 1. 稳定 record 的类型擦除与安全边界

`ThreadCleanup` 不是把 `F/T/E` 转成共享可变对象，而是 prepare 阶段一次性分配的 `Box<CleanupRecord<F, T, E>>` 的唯一 owner。Ready 槽只保存非空指针和静态 vtable；record 的实际布局、closure、结果、环境、ThreadControl、栈 owner 和付款 Charge 都留在原始稳定存储中。这样 Drop 发布只转交既有指针，不在故障路径新分配。

vtable 只允许以下操作：

- 在唯一 receiver claim 后执行一个有界 `step`，返回 `Continue/Wait/Complete/Failure`，不暴露 record 内部引用；
- 在完成或明确终局时销毁 record，并由同一个 vtable 路径 drop 已初始化或明确未初始化的 `T`；
- 释放对应的 `CleanupBytes` Charge 和 permit。

`ThreadCleanup` 对外只实现移动，不实现 `Clone` 或 `Sync`；跨线程 `Send` 的 SAFETY 前提是 `F/T/E`、`MappedRegion`、typed Handle 和 Charge 均可由稳定 owner 移交，且发布后生产者不再访问 record。domain 的 atomics 只同步槽状态，不能让 record 的字段本身并发可见。receiver 以 acquire claim 后独占 record，永久错误也必须携带完整 record，不得调用裸 drop。record 的 Drop 不能隐式放弃未完成 cleanup；未完成的 task/runtime 由外层 owner 保留。

首版不把 `ThreadCleanup` 变成 `dyn Trait` 或要求 `T/E` 放入全局类型注册表。type-erased 只发生在固定 `unsafe fn` vtable 边界，所有函数指针绑定到同一 `CleanupRecord<F, T, E>`，每个 unsafe 入口写明：slot 已独占、DONE/初始化状态已满足、调用后 record 仍由返回值或 vtable owner 持有。

#### 2. 槽容量与付款预算

`RetirementDomain` 的容量不是经验常量，而是装配者提交的并发 cleanup 上限；factory 在打开前必须为全部可能存活的 permit 预付 `RetirementSlot`。libthread 通过 `ThreadResource` 提供自己的预算分类：

```text
ThreadResource::RetirementSlot  = 固定槽/permit 数量
ThreadResource::CleanupBytes     = 稳定 cleanup record 字节数
```

`RetirementDomain::input_budget(capacity)` 只计算固定槽、状态字和 ready 扫描存储；每个具体 `PreparedThread<F, T, E>` 在分配 record 前另取得 `size_of::<CleanupRecord<F, T, E>>()` 的 `CleanupBytes` Charge。Charge 跟随 record 一起转交，只有 cleanup 完成或准备阶段完整回滚才退款。slot 与 bytes 使用不同计量单位；不能用一个“线程数”同时代替两者。

预算顺序固定为：

1. 建立 `AccountView<ThreadResource>` 和 domain 固定槽；
2. 为 cleanup receiver task/source 预付既有 `libexecution` 的 Task/InputBytes；
3. receiver source 获得 registered 回执后开放 publisher；
4. 每个 prepare 先取得 RetirementSlot，再取得 record bytes、packet/环境/栈所需的各自 owner；
5. 任一后续失败按反向顺序返还尚未提交的 Charge，已提交 cleanup 继续持有原 Charge 直到 record 完成。

普通 allocator 的 OOM 不能成为 Drop 阶段的补偿路径。record、slot 和通知连接均在 prepare/open 阶段取得；Drop/发布只移动已存在的 owner。容量不足在 `reserve` 或 prepare 时显式返回，不能通过临时扩大 domain、`Runtime::spawn` 或全局 fallback 逃避预算。

#### 3. receiver supervisor 与同步 join

首版 receiver 失效策略是“预先存在的 supervisor 负责判断并收束”，不承诺自动重启：

- `ThreadCleanupTask::advance` 只报告 cleanup 的正常进度、可重试错误或不可恢复 `TaskFailure`；它不终止进程，也不丢弃 domain。
- domain 把通知 ObjectClosed、receiver lease 失效和无法继续 claim 记录为稳定 `receiver_lost` 状态；Ready/Claimed record 仍保留。
- 外层 supervisor 持有 Runtime、ThreadCleanupTask 和 domain 的最终 owner。可恢复故障必须保留完整 task/runtime 等待明确接管；首版无预付 replacement task 时，按进程完整性故障退出。
- 自动重启只有在未来另有独立 task slot、source slot、预算和新 receiver 的预付接管协议时才可加入；不能在故障处理路径临时 spawn。

同步 `JoinHandle` 首版保持线程局部，`ThreadCleanupTask` 不持有它，也不在 `advance` 中调用 `join`。Join 只等待目标 ThreadControl DONE，再直接驱动同一 `ThreadCleanup` step；若等待超时/观察失败，完整 Join owner 返回调用者。调用者不得在唯一负责推进目标 closure 或 cleanup source 的 executor turn 内执行阻塞 Join；这属于组合层装配约束，不能靠 libthread 隐式启动第二个事件循环解决。Drop 与 join 的自 join 风险由 JoinHandle !Send 排除，executor 依赖环由 owner 图和装配检查排除。

### 设计完成门

本节已完成接收域和三个施工前窄门的设计冻结：record 类型擦除与 SAFETY 边界、ThreadResource 预算、receiver supervisor 首版故障策略、JoinHandle 线程局部规则均已写明。代码施工仍需把这些约束落实到具体类型、unsafe 论证和真实消费者，并在施工后验证。

## 前置、触发与顺序

原延期建立在公共操作回收与执行/RPC/FAL 基座尚未接通的阶段。现在该基座已有实现，libthread 分层直接触发失败承接需求；本轮设计已完成，下一阶段转入代码施工前的范围确认、类型落地和消费者迁移。其它映射与堆扩展仍不自动扩大范围。

当真实长期服务需要动态管理大量普通 mapping、正式 detach/reaper，或当前调用者的正常释放无法由现有 owner 闭合时触发。触发提前发生则先重排 COMPASS；直接影响当前操作正确性的清理不得以本计划延期。

### 已触发的线程闭包

现有 `UserStack::unmap` 遇 Busy 循环睡眠，其它失败只累计 abandoned count 并保留映射到 ProcessDrain。新的 libthread 要同时保证结果初始化状态、环境 owner 和清理失败可恢复，不能沿用仅记计数的责任丢失。线程相关子集是分层迁移的必要前置；不因此重做全部 allocator 或普通映射。

Rust 不能通过 `must_use` 或仅要求显式 join 禁止调用者 Drop；删除 Drop 也不能产生正确收束。目标工厂在准备资源之前取得一个可无分配、不可拒绝接收剩余 owner 的退休准入。承接必须持有完整 cleanup、付款和观察/重试状态，存活与推进不依赖原调用者或已销毁的 factory。仅接口有接收函数不构成活性证明。

接收域与装配寿命设计已完成，三个施工前窄门也已冻结。下一阶段是把稳定 record、ThreadResource、receiver task 和线程局部 JoinHandle 落到 libthread/libkernel/libruntime 的具体结构，再迁移真实消费者；不另造第二套事件循环。

线程工厂的 safe Drop/隐式取消在代码接通前仍属于**施工阻塞**；当前设计已经给出不可拒绝的预付承接，不允许以调用者永不 Drop 或进程终局作为替代。生命周期契约由 [task](../notes/ideas/task.md#用户态线程资源收束) 拥有，具体承接设计由本记录拥有，分层计划只引用前置。

### 自然实施与验证顺序

1. 盘点 MappedRegion、堆 arena、UserStack/Join、对象 view 的访问寿命与真实调用者，区分自动释放、显式退休与仍被执行的资源。
2. 设计共同的拥有、切分、取消和失败返还契约；决定用户态 reaper/退休服务是否有真实需要及由谁提供，不预设新增内核接口。
3. 每类资源带全部调用者共同迁移，保留 DONE/结果义务、映射同步及 backing 的安全屏障；不得以自动 Drop 提前撤销仍在使用的内存。
4. 验证进程持续运行时的成功释放、并发 Busy、取消、清理失败后继续处理，以及最终进程回收；删除旧 owner/裸清理路径。

## 完成与删除门

正常资源释放不依赖业务逐段维护内部清理状态；所有失败仍有可识别 owner，活动访问结束前不复用地址或 backing。host/目标检查与实际堆、线程、映射组合有证据。最终方向入 notes，实际 API 入 impls，完成后归档本计划。

## 跨会话接力断点

本任务仍只有文档授权，接收域设计与施工前窄门已完成，代码施工尚未开始。分支与未提交成果见 [分层主线接力](todo-2026-09-runtime-kernel-library-separation.md#跨会话接力断点)；下一阶段需要先确认代码施工范围，再从本记录的接口草案开始实施。

### 先读哪里

1. 本篇「源码复核」与 [runtime 实现](../notes/impls/runtime.md#资源退休消费者与复用边界)：已实现基座、专用接线和真实差距。
2. [task 线程契约](../notes/ideas/task.md#用户态线程资源收束) 及分层计划的 libthread 接口稿：结果/cleanup 分离、DONE 屏障、环境状态和失败 owner。
3. 按已冻结施工门回到 `libexecution/runtime.rs`、`work_queue.rs`、`wake.rs`；FAL `store.rs`/`provider.rs` 与 init `supervisor.rs` 只作已落地机制对照，不把它们的领域状态抽成万能接口。

### 本轮设计产物与剩余门

本轮已经形成以下设计产物：

- owner/寿命图的文字版：factory、PreparedThread/JoinHandle、RetirementDomain、ThreadCleanupTask、Runtime 和外层 supervisor 的持有事实与失效责任见上节。
- 准入与关闭时序：接收 task/source 就绪后才开放 factory；seal 只禁止新 permit；`outstanding == 0` 后才允许注销 source 和关闭 Runtime。
- 公开接口边界：permit 预付、固定槽发布、Ready/Claimed 领取、cleanup machine 逐步推进、receiver-lost 保留 record；普通可失败 spawn 不承担 Drop 的必成接收。
- 最小验证边界：跨线程发布、通知合并/丢失、双重移交、Runtime seal 竞态、Busy/永久错误保留和 receiver 失效接管。

设计方向和施工前窄门已冻结。剩余工作属于代码实施与验证：将 vtable/record 的 unsafe 证明、预算扣账、receiver task 接线、JoinHandle 线程局部约束和现有消费者迁移落实到源码；任何新 ABI、额外服务或超出线程退休闭包的映射/堆扩展仍需另行记录。

## 本轮复核与验证

源码与消费者复核完成，上述现状表区分已实现基座、领域专用接线及线程未接线。原始责任链走查包括准入失败返还、Task stop 后继续清理、来源注销回执、失败 task 续作、非空 Runtime 关闭返还、FAL 最后引用发布和 root 失败保留；未重新执行已有 host 用例或 QEMU。跨线程接收设计已完成，代码施工仍未开始。

文档检查：`git diff --check` 通过；本轮只修改设计/契约文档，未渲染 Mermaid、未改 Rust/Cargo/ABI/构建配置，没有提交。
