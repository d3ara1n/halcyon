# 执行任务与来源的付款归属

状态：**已完成并归档。** 通用显式付款、全部直接消费者及 E1—E7 验证已收口；不同账户的实际业务装配与 E8 由主计划 W04-02 拥有并已完成，后续 Bind、流、Copy 与三轮收敛已接通并通过聚合。2026-10-11 用户确认本轮结束并授权提交。所属 [OAL/fs/service 主计划](../todo-2026-10-07-fal-service-redesign.md) 要求执行费用与可信入口绑定的付款身份一致；本篇保留通用前置合同与当时证据，后续新需求不在本篇续写。

## 当前接力与范围

- 原设计/实施分工：GrandOwl 设计、MellowPond 协调、RubyRaven 实施；该轮已结束。当前维护与提交前工作以主计划接力为准，不重新排批。
- 盘点基线：`refactor/system-redesign` / `4523bbfe65287f6b84ab73d9f034dae1a526d38d`，加当前未提交迁移。Notification 前置已在工作树实施，其他会话改动保留。本篇只记录与本案有关的事实，不能将矫正区现状当作目标依据。
- MellowPond 已审定：Task 提交时显式冻结付款；来源默认继承 Task，独立操作 owner 可创建不可变的显式来源付款绑定；Runtime 构造账户只支付基础设施；SourceEntry 只保存实际 Charge；失败返还完整提交责任；不虚构 AccountClosed，不新建全局账户表，不改内核 ABI。
- 本案拥有：TaskSubmission、Runtime/WorkQueue/Gate 的准入及失败合同，来源付款绑定，费用估算口径，通用消费者 API 迁移，取消/重试/退款和通用验证。
- 主计划拥有：fs/service 可信入口及控制的资金绑定、领域费用、boot 多账户政策、W04-01/W04-02 的真实业务与资源闭环。本案规定接缝，具体入口签发和业务协议在主计划定义，不复制其 findings。
- 非目标：账户封口/撤销 API、跨进程付款能力、按账户拆 Runtime/Task、租户 CPU 公平、内核 Pool 改绑、硬容量隔离、完整逐字节 allocator 记账、新 RPC 或 syscall 线格式。
- 交付结论：通用付款前置、E1—E7 分级验证与全部消费者迁移已完成；W04-02/E8 真实多账户及后续组合由主计划记录并已收口。本篇归档保留合同与证据，后续新的调用者、契约变更或实际失败由新任务或主计划承接，不重启已完成施工。

## 设计基线与费用盘点

### 发生点

| 当前位置 | 实际收费/占用 | 当前 owner 与问题 |
|---|---|---|
| `Runtime::new`，runtime.rs 311—345 | `InputBytes = fixed_input_bytes(source_limit)`；包含 SourceEvent scratch 与固定 Requests 操作区 | 存储随 Runtime 保留；同时保存 `self.account`，被错误地复用为所有任务和来源的默认付款方 |
| `Runtime::spawn` 353—360 | 调 `queue.insert(..., &self.account)` | 调用者不能提交任务自己的 execution 视图 |
| `Requests::spawn` 154—160、Gate 1099—1102 | 操作只携带 T 和 max_sources；应用时取 Runtime 的账户 | 无法冻结子任务付款；失败回执也只返 T |
| `WorkQueue::insert`，work_queue.rs 116—192 | **仅扣 `ExecutionResource::Task = 1`**，存于 Slot._task_charge | 已能接受 AccountView，但 Slot 未保留后续来源准入可用的视图；Slot/TaskTimers/source-token Vec 未另扣 InputBytes |
| `Runtime::prepare_source` 1188—1233 | 每条来源扣 `InputBytes = SOURCE_INPUT_BYTES`，含 PreparedEntry<SourceEntry> 与一个退休 TimerNode 的布局费用口径 | 一律取 Runtime.account；SourceEntry._charge 已能保留真实扣账至退休 |
| `Runtime::input_budget` 300—309 | 固定输入口径 + source_limit × 每来源口径 | 是单账户装配时的合计估算，不是全额在 Runtime 构造时扣账；不能把它全部给 H，再以为 A/B 私有来源也已付款 |
| `Assembly::new`，assembly.rs 29—64 | InputBytes，按 `input_budget()` 的 ReadyRecord 批缓存及 SetState 大小口径 | 共享设施费用；Registration/bind 只克隆 Rc，不再收费 |
| `SourceRegistration` | 当前只保存登记相位，不持 handle/Charge | 可承担稳定付款绑定，但实际扣账仍应留在 Runtime 的来源记录中 |
| `libbudget::AccountView/Charge` | view 为同一账户解释资源槽；Charge 保存 account/slot/units 及 sponsor | 已支持一个 Budget 多 Account、全局与本地双层准入、按原 Charge 退款；无需新账户机制 |

源码：[runtime](../../user/libraries/libexecution/src/runtime.rs)、[work_queue](../../user/libraries/libexecution/src/work_queue.rs)、[assembly](../../user/libraries/libexecution/src/assembly.rs)、[source_registration](../../user/libraries/libexecution/src/source_registration.rs)、[libbudget](../../user/libraries/libbudget/src/lib.rs)。行号是盘点位置，施工以符号为准。

`Account` 没有 close/seal/revoke API；AccountView、Charge 保活 Account，Account 又保活 Budget。普通 clone 不建立账户、不重置额度。`Budget::new(..., 1)` 的第二参数限制账户数，并非 Budget 本身只能有一个账户。

### 实际消费者与付款断点

- srv_fs boot 当前创建一个 Budget、一个 Account，切出 execution/fs/service 三个视图。它们此时身份相同，所以普通成功路径不能发现 execution 始终记 host 的问题。
- libservice `Root::prepare` 用 `state.account` 支付自身 Bytes，也把同一个账户传入访问/发布角色。RoleTable 已能冻结传入账户，缺的是可信发行与 execution 视图接线。
- libservice Request::accept 已从真实 sender context 解析 Access/Authority/Registration/Watch 控制的领域账户；libfs Request::accept 从 grant 得到领域账户。它们的 Runtime Task/来源仍未按该身份准入。工作树中 W04-01 的代码接线不替代 W04-02 证据。
- Dispatcher 是一个实际的多操作任务：同一 PendingCall 表中，每个调用各有 CLOSED、WRITABLE/ReplyObserver 来源，另有一个共享 reply 来源。不能把“来源属于同一个 Task”推导成“来源必须由同一账户付款”。
- srv_init 的 supervisor helper 当前为每个 Runtime 创建单账户；srv_pm 当前也用单账户。它们可保持原政策，但必须显式提交任务付款视图。

## 目标模型与计费范围

### 三个独立维度

1. **执行 owner**：Task 是调度、事件路由、回执与最终清理主体；SourceId.task 仍授权对应任务注销来源。
2. **收费资源**：Task 名额、固定输入容量、来源记录是不同实际占用，各有唯一 Charge owner。
3. **付款绑定**：可信装配或操作 owner 提供 AccountView；准入后 Charge 冻结实际原账。一个 Task 可推进多个付款方的操作，一个账户也可支付多个 Task。

账户不决定调度拆分，不为每个账户复制 Runtime/Assembly；同一物理共享来源只有一个明确 sponsor，不能按每次就绪事件轮换 payer。业务订阅等另有独立记录时，按各记录的真实 owner 计费，不对一份共享存储重复收 Bytes。

| 资源 | 目标付款绑定 | 费用归还 |
|---|---|---|
| Runtime 固定 scratch/Requests 区 | Runtime::new 的 infrastructure sponsor H | Runtime 释放该存储 |
| Assembly 接收批缓存/共享状态 | Assembly 的 sponsor，通常 H | 原 Assembly/SetState 存储释放 |
| Task 名额 | TaskSubmission 显式视图；业务任务可为 A/B，共享 actor 可为 H | Slot 及其最终清理责任结束 |
| Task 默认来源输入 | Slot 冻结的 Task 视图 | 对应 SourceEntry 真正退休 |
| 独立操作的私有来源输入 | 操作/观察 owner 的 Bound 视图 A/B | 各 SourceEntry 真正退休，不随任务位置改账 |
| Dispatcher 共享 reply 来源 | 构造时指定的共享 sponsor，通常 H | 共享来源实际注销 |
| 领域请求、Outbox payload、节点/控制等 | 主计划定义的原领域付款绑定 | 原领域 owner 与真实存储释放 |

### 本案不扩大记账承诺

本案保留 `ExecutionResource::{Task, InputBytes}` 的资源分类，修复其付款身份与生命周期，不把 `InputBytes` 改称“所有执行内存字节”。Task=1 是任务名额准入，不是对 Slot 的 sizeof 收费。

当前 WorkQueue 的节点、TaskTimers、source-token Vec 经 AllocatorBacked/程序 allocator 供给，没有独立 InputBytes Charge。它们仍受实际分配失败、任务/来源结构容量及进程内存供给约束；**本案不据 Task 名额宣称已经逐字节归账到租户，也不宣称已经预留硬内存容量。** 新 Slot 中的付款视图和 SourceRegistration 的绑定也沿相应存储 owner 持有，不新增一份虚假的“绑定费用”。

Runtime 的固定 Requests 区随 TaskSubmission/来源命令布局扩大，其 InputBytes 公式必须同步；SourceEntry 的计费口径若实际结构改变也须同步。源码计算保留唯一真值，不能在 boot/测试复制手写 sizeof 常数。

领域已收取的 `_task_bytes`、payload/缓冲费用不因执行付款迁移重复收取。不得笼统给整个 T 再加执行 Bytes：T 可能包含已按领域计费的 inline 数据，也可能只是指向另付费 Box 的指针。完整执行存储逐字节记账不在当前承诺；以后若承诺该能力，须从实际分配 owner 重划范围并统一删除重复账，由本案的“后续触发”记录承接，不能只增加一个 Bytes 枚举就宣告完成。

原内核 WaitSet、Mailbox、Notification 等仍由其真实 kernel Pool 付款；用户态 AccountView 的变化不修改这个 payer。当前修复提供用户态额度归属，不提供内核 Pool 隔离或 CPU 配额保证。

## Task 提交与 Gate 合同

### 公开 API 形状

使用一个拥有完整提交责任的 `TaskSubmission<T>`，包含私有字段 `task`、`max_sources`、`AccountView<ExecutionResource>`。构造消费传入的 T 和视图；提供借用付款视图以及消费式 `into_parts`，没有入队后的改账接口。

```rust
TaskSubmission::new(task, max_sources, execution_view)
Runtime::spawn(submission) -> Result<u64, InsertFailure<T>>
Requests::spawn(submission) -> Result<(), TaskSubmission<T>>
```

`Requests::task_funding() -> &AccountView<ExecutionResource>` 只读暴露本次 advance 所属 Slot 的固定视图，供需要沿父任务付款的代码显式构造子任务提交物。公开 `Requests<'a, T>` 是仅在 advance 调用期间存在的借用视图，持有非可选的 `&'a AccountView<ExecutionResource>` 与内部命令缓冲区的独占借用；Runtime 单独持有该缓冲区。Runtime 在 advance 前构造此视图，返回后结束借用，再由 Gate 消费缓冲区。暂存的命令各自拥有付款快照，不借用此视图。不得实现成可脱离作用域的公开空 Requests，再用 Option::expect/panic 检查当前付款是否存在。

`Task::advance` 接收 `&mut Requests<'_, Self::Family>`；`refused/registered/unregistered/stop` 均不接收 Requests，不能调用这个作用域读取接口，也不能在这些回调内提交命令。Requests 的构造及字段保持私有，不提供 Clone/Default 或取出内部缓冲区的接口；借用生命周期禁止任务把本次 Requests 或其付款引用保存到自身/World 后再在回调中使用。回调只接回 owner 或记录意图，后续 advance 再提交；refused 仍可读取完整返回的 TaskSubmission 自带的视图。业务显式 clone 出的 AccountView 是独立持有的既有付款绑定，不是越过作用域访问 Requests。

此接口不按任意 task id 查账户，也没有修改接口。子任务仍须显式构造 TaskSubmission；读取父任务视图不恢复省略付款的 spawn。

`InsertFailure<T>` 和 `RequestFailure::Spawn` 均携带完整 `submission` 与错误，不再只返 `task`。WorkQueue 的内部 insert 接受该提交物，拆开准备时仍能完整重组失败结果。

`Runtime::new(..., infrastructure: &AccountView<ExecutionResource>)` 只取得固定输入 Charge。删除 Runtime 中供普通 spawn/source 使用的 `self.account` 默认账；无需为了默认付款长期保留基础设施 view，原固定 Charge 已保活并标识 H。

现有单账户消费者也统一迁到显式 TaskSubmission，重复使用同一 view 是合法政策。**不保留 `spawn(task, max_sources)` 的旧接口，不提供省略付款时回落 Runtime 总账户的兼容路径。** Runtime 本身不根据 task 类型、sender、PID 或正文 account id 推断付款。

### 准入与发布

1. 直接 spawn 或 Requests 暂存前，提交物已拥有付款视图；克隆 view 不新建账户、不扣 Task 费用。
2. Requests 操作区满则完整返还提交物，未发生新的 execution acquire。暂存成功后 Gate 独占提交物，不到应用时重新读取 World 中的某个“当前账户”。
3. Gate/直接准入检查 Runtime/队列状态与结构限额，以提交物视图取得 Task=1；准备 timer、source-token 容量和表节点。新增失败沿准备 owner 释放，业务 T 本来持有的资源仍随返回提交物保留。
4. 表发布后 Slot 持不可变的 execution view 与 `_task_charge`。发布后的 ready/期限/stop/finalizing 不换付款方。子任务可晚于父任务退出，自己的 view/Charge 保持付款来源。
5. Gate 因父任务错误拒绝剩余 spawn、Runtime sealed、来源装配失效等，也走完整提交物返回；不能只改正常 insert 错误而遗漏 reject_operation。

Task 生命周期继续为 Active→Finalizing→释放；Complete 不是立即归还 Task Charge。只有活动及 retiring 来源收束、timer 注销、Slot 实际释放后才退 Task 名额。既有回收 owner 与公平队列保持，不另造付费清理任务。

## 来源绑定与登记状态

### 稳定绑定

引入只表达付款选择的 `SourceFunding` 值：`Task`（默认）或 `Bound(AccountView<ExecutionResource>)`。它不是新账户、配额、付款 capability 或 Runtime 全局对象。它可克隆为命令快照，不能改写已经形成的 Charge。

- `SourceRegistration::new()` 使用 Task 模式。
- `SourceRegistration::funded(view)` 在创建逻辑观察角色时固定 Bound 模式。
- helper 内的 funding 字段私有，没有 `set_payer`，`install(plan, kind)` 不接受账户参数。上层如需统一构造接口，可传 SourceFunding 创建 helper，但只能在其出生时选择。
- SourcePlan 仍只描述 handle/signals，不持 handle 关闭权或付款 owner。普通 Requests::register/add_source 在暂存时克隆当前 task_funding；SourceRegistration::funded 通过 crate 私有登记入口克隆自身 Bound 视图。不公开可每次换账的 register_funded 便捷口。
- 内部 Register 命令始终携带已解析的 AccountView 快照；Task 模式在暂存时解析为当前 Slot 的固定视图，Bound 模式取 helper 的固定视图。Gate 只用命令中的视图准入，不再读取 World 或其他可变默认账户，也不解释领域身份。

默认模式覆盖从属于同一 Task 的逻辑观察。需要跨 Task 移交、或置于复用 actor 内的独立业务观察，必须由其操作 owner 携带显式 Bound 绑定。不能把 Task 模式跨任务复用解释为自动保留旧付款；现有 SourceId 的任务归属与注销权限仍不变。

### 状态与费用

| helper 相位/动作 | 付款绑定 | Runtime 费用及责任 |
|---|---|---|
| Empty | 保留出生模式/Bound view | 无该次登记 Charge |
| install 命令未入队 | 原相位/绑定不变 | 无新 Charge，借用 handle 仍归业务 owner |
| Installing / CancellingInstall | 命令快照与 helper 固定绑定 | Gate 可准备来源；取消不提前丢安装回执责任 |
| Attached | 不可修改 | SourceEntry 持原账户 acquire 取得的 Charge |
| RemovePending / Removing | 原绑定不变 | 原 Charge、entry、退休 timer 保留到实际注销 |
| unregistered / 安装拒绝→Empty | **只清相位，不清绑定** | 已释放的本次记录费用归还原账；重装仍沿同一逻辑绑定 |

来源准备先检查 Task/来源表准入，以所选视图 acquire InputBytes，再准备 SourceEntry/退休 timer/表节点，最后调用 SourceOps::register_item。成功后只以既有 prepared entry 发布 token/任务关联，不新增可失败分配。

SourceEntry 保存实际 Charge 已足够，不另存一个可变 payer id，不从当前 Task 的账户推算退款。SourceEntry.task 继续控制 feed、回执和注销授权；付同一个账户不授予删除另一个 Task 来源的权限。

取消、关闭操作 owner 或结束 Task 都不能让已登记来源失去清理 owner。helper 不在 Drop 隐式注销；实际 token、Charge 和退休承载一直由 Runtime 持有，原 Task 按回执规则保留被观察资源至可释放边界。

## 失败、重试与账户寿命

| 事件 | 返还或保留的 owner | 费用处理与后续 |
|---|---|---|
| Requests spawn 区满 | 完整 TaskSubmission | 不新增执行扣账；父 owner 决定拒绝或稍后提交 |
| Task quota/准备分配/表容量失败 | 完整 TaskSubmission 与错误 | 回滚本次 Task Charge/准备存储；不销毁原 T 中已持有的领域责任 |
| Gate 拒绝剩余 spawn | 原命令中的完整提交物 | 不读取新默认账户、不丢 max_sources/funding |
| 来源 quota/准备失败 | helper 固定绑定及原业务 handle；kind/error 回执 | 未发布记录，刚取得的原账 Charge 随准备 owner 回滚 |
| SourceOps 登记失败 | 同上；准备节点由本次调用收束 | 不留下 token/半记录；实际费用原路回滚。若扩展命令独占预留，失败须返还或明确释放该预留 |
| 安装中取消后仍获 token | 原 Task/helper 接回并进入 RemovePending | 真实 token 必须注销，不能按“已取消”提前退款 |
| Remove 失败/退避 | 原 SourceEntry、Charge、退休 timer、Task | 不重新 acquire；只改原预付 timer 的期限，保留原付款至实际清理 |
| 内核已 Remove，但回执前时钟等准备失败 | 原用户态来源记录仍存活 | 按现有协议保留记录/Charge并重试；确认已不存在也须完成用户态清理 |
| Task Complete/Cancel/Seal | 原 Slot 与所有来源退休责任 | 来源费用按各记录释放；Task Charge 等 finalizing 结束，不借完成标记提前退款 |
| 账户额度耗尽 | 已准入 owner 全部保留 | 仅新 acquire 返回 QuotaExceeded，不自动终态、不换账、不阻断已付费清理 |
| 外部发行者丢弃 Account/view | 操作 view、Gate 快照及 Charge 仍保活原 Account/Budget | 不等于账户关闭；账户名额到最后真实持有结束才归还 |
| 来源已释放后回执后的 Task deadline 错误 | Task 仍在原 Slot 中 | 已实际释放的来源可退款，Task 错误由原 owner 处理；不为保持错误而重建来源账 |

登记准入失败的 Quota/OOM 交还业务层处理，不在 Runtime 自动无限重试。业务可拒绝本次请求，或在契约允许时保留 owner、沿原绑定使用既有绝对期限/退避机制重试；额度配置永远不足不能导致忙循环。

来源注销重试与新登记重试不同：注销是已提交责任，使用原 entry 中的退休 timer/记录，不需要再申请费用。入口 revoke/服务 stop 仍由领域控制业务准入与终结；钱耗尽不是业务撤销，业务撤销也不改变退款账户。

本案不新增 AccountClosed。若未来需要账户 seal，须另定义停止新扣账与存量责任的关系；不能把 Arc 析构或额度零冒充关闭协议。

## RPC 与可信装配接缝

### Dispatcher、PendingCall 与 Outbox

- Dispatcher 构造显式接受共享来源 sponsor 的 execution view（借用参数、成功时克隆绑定）。共享 reply_registration 采用该 Bound 视图，通常为 H；TaskSubmission 单独支付 Dispatcher 的 Task 名额。
- `begin/begin_for` 显式接受本次调用的 execution view（借用参数）；成功建立 PendingCall 时冻结，用于该调用 CLOSED 与 progress 来源。参数借用者在 StartFailure 后仍持原绑定，原 service/Request owner 按原错误合同返还。
- progress 从 WRITABLE 观察切换到 ReplyObserver 观察时，沿同一 SourceRegistration 绑定重新安装，不能改回 Dispatcher 的 H。共享 reply 来源不因接收到 A/B 的回复轮流改账。
- Outbox 的 `prepare/from_prepared` 显式接受 SourceFunding 选择并克隆为自己的稳定登记绑定；普通一任务一请求路径可显式选 Task，复用 owner 内的独立回复选择 Bound。CLOSED→WRITABLE 切换、取消、超时和重试保留绑定。失败仍完整返还 RequestContext/回复责任；借用的付款选择不被工厂吞掉。
- 现有同步 Caller::ReplyCleanup 仅持 Mailbox/Sender 并 retry_close，没有 Runtime 来源或 execution Charge；不因名字含 Cleanup 强加资金字段。若上层用任务推进它，TaskSubmission 必须来自该责任原有的可信上下文；以后若真正新增来源再按上述规则绑定。

现有 Outbox/Dispatcher 的消息提交与未知结果语义不变，本案不重做 Caller/Dispatcher 协议或增加新的回复通道。

### 入口与拒绝路径

可信发行者为一个 Account 建立 domain_view 与 execution_view，在组成入口付款上下文时核对 account_id 相同。fs/service 的具体组合类型由主计划拥有；libbudget 不依赖 ExecutionResource，libexecution 不依赖 FsResource/ServiceResource，也不提供按裸 id 找账户的表。

Root::prepare 接收可信发行的付款上下文，Root/角色记录和其 TaskSubmission 使用约定视图；不再把 state.account 当作所有入口的隐式账户。访问/发布角色、派生 grant、registration/watch 控制、请求及下游调用沿原可信绑定继承。付款证明来自持有的上下文，不能接受正文 account id 自报付款。

H 支付共享接收、未识别请求处理和有界准入拒绝。A 的业务任务准入失败后，不能退回 H 继续正常业务。拒绝回复须由已存在且有界的基础责任承接：若尚未登记的原 Outbox 使用 Task 模式，作为纯拒绝责任在 Ingress 内推进时继承 H；若它已显式 Bound A，则不能悄悄换成 H，须在无已登记/已公开来源的边界消费原准备责任，转为独立、预付的拒绝回复责任。已经分配的原 payload/Charge 继续由原 owner 保留到实际释放，不因改成拒绝正文而提前退 A。

主计划须据实际 Ingress 选择并写死上述路径，不引入任意 `set_payer` 或让 host 兜底执行失败的普通请求。来源已登记的操作，必须先按原绑定注销，不能借“转拒绝”绕过退休。

## 预算估算与单账户迁移

将当前混合 `Runtime::input_budget` 拆为语义明确的两个查询：

- `Runtime::<T,S>::fixed_input_budget(source_limit)`：现有 SourceEvent scratch 与固定 RequestOperation<T> 暂存区口径，归基础设施 sponsor。
- `Runtime::<T,S>::source_input_budget(count)`：count × SOURCE_INPUT_BYTES，count 按同时保留的来源记录计算，包含 active 与仍在退休的记录；不等同于一次已声明的 active max_sources。

`Assembly::input_budget()` 保持其共享设施口径。删除 Runtime 的旧混合 helper，不留单账户总账便利口；需要总限时由装配者显式求和。所有乘加检查溢出，实际类型布局变化必须反映到函数结果。

同一 Budget 的 InputBytes 全局限额应覆盖已计划的固定设施与各来源占用。H 的局部限额包括固定设施及自身共享来源；A/B 的局部限额只覆盖其付款责任。同一 view 被多个组件共享时，其使用量本来就累加，不能给每个组件独立假定“全部额度归我”。

Task 数、max_sources、Runtime 全局表容量仍是结构限制，账户配额不能绕过它们。预扣额度不等于物理 reserve 成功；上述计算不把可失败的 allocator/kernel 登记变成必成。所有业务提交前所需的来源、回复和清理准备仍按主计划完成。

## 实际迁移清单

本表来自本轮 rg 和符号核对；同期主计划可能新增消费者，实施时再枚举，不据历史行号遗漏新调用。

| 位置 | 迁移内容 |
|---|---|
| `libexecution/src/runtime.rs` | 删除全体任务/来源默认账；TaskSubmission、两条 spawn 与 Gate reject/失败回执；advance 作用域 task_funding 与登记命令的固定快照；费用估算；SourceEntry/退休保持原 Charge |
| `libexecution/src/work_queue.rs` | insert 接完整提交物；Slot 持固定 execution view；失败重组原提交；保留 Task=1 与原 finalizing 退款点 |
| `libexecution/src/source_registration.rs`、`lib.rs` | 稳定 SourceFunding 与公共类型；phase 复位不清绑定；注释区分“绑定 view”与“实际 Charge owner” |
| `libexecution/src/assembly.rs`、`source.rs` | 基础设施 sponsor/Registration 借用边界；不为 bind 重复扣账、不让 SourceOps 解释领域 payer |
| `librpc/src/dispatcher.rs` | new 共享 sponsor；begin/begin_for 的每调用资金；PendingCall 两种私有来源及切换；StartFailure 的借用/所有权合同 |
| `librpc/src/outbox.rs`、相关构造调用点 | prepare/from_prepared 固定 SourceFunding；CLOSED/WRITABLE 重装及取消保留原账；不更改消息提交语义 |
| `librpc/src/caller.rs`、`exchange.rs` | 核对 ReplyCleanup/失败 owner；同步 Caller 不因本案被迫引入 Runtime 付款，不机械增加无用途字段 |
| `libfs/src/{authority,grant,provider,watch}.rs` 的真实付款记录 | 与主计划接入 fs/execution 同身份上下文、控制/请求/来源持有；保留原领域 Charge |
| `libservice/src/{authority,root,provider,registry,watch}.rs` | Root 可信发行资金、各入口/控制的绑定接续、Request 与 Outbox 接入；主计划定义领域装配政策 |
| `srv_fs/src/server/{boot,family,services,lifecycle}.rs` 与 `server.rs` | 统一 TaskSubmission；Ingress 派生和失败返还；公共任务/根/控制 sponsor；Dispatcher 构造/下游调用；boot 同 Budget 多 Account |
| `srv_init/src/{supervisor,main}.rs` | runtime helper 不再吞掉后续提交所需 view；由装配 duty 保存显式视图并提交各任务；Assembly/Dispatcher/Outbox 资金；保留既有单账户政策 |
| `srv_pm/src/main.rs` | Runtime 构造、MailboxTask、StreamTask/FlowTask/DomainTask 的显式提交；不留默认付款旧 API |
| `libprocess/tests/{observation_runtime,execution_recovery}.rs`、libexecution 现有 host 用例 | 显式视图/提交物、FakeSet 失败与退款断言；原职责恢复测试不得被付款迁移绕过 |
| `user/tests/test_fal` 与主计划正式装配验收 | W04-02 原付款及双账户真实运行；P4 改名 test_fs 后按新名接续，不建平行测试服务 |
| `notes/impls/{framework,runtime,fs,service}.md` 与实际机制引用 | 实施后回填真实费用范围/owner；不要提前把设计写成实现或把额度收费写成完整物理记账 |

实际 grep 命中还包括 libexecution 自身测试与 Assembly::run 等通用驱动；它们须全部编译迁移。libprocess 的领域 Observation 类型不因本案改名；libbudget Account/Charge 机制没有缺失的多账户功能，不为透传视图修改其身份模型。

## 验证门

按[构建与验证手册](../BUILD-AND-TEST.md)执行；优先扩展现有 FakeSet、WorkQueue/SourceRegistration、libprocess 恢复测试及正式进程。断言实际账户 usage/global usage 与 owner 收束，不用只读取 payer 字段的测试冒充付款正确。

| 门 | 要捕获的错误与证据 |
|---|---|
| E1 显式任务资金 | 同一 Runtime 的 A/B Task 分别扣本账户；H 固定基线不替代业务付费；单账户显式提交仍正常；不同 task 的 advance 作用域不串账，已暂存提交不因父任务结束丢失绑定 |
| E2 默认/复用来源 | A Task 的默认来源扣 A；同一 H Task 可同时持 Bound A/B 来源；共享来源 H 不因事件来自不同调用换账 |
| E3 Gate 失败 owner | Requests 满、队列表满、Quota、准备分配失败及 reject_operation 都完整返提交物；新费用回滚，原领域资源/资金仍可由返回 owner 重试或释放 |
| E4 来源失败/重装 | SourceOps 拒绝、安装取消、CLOSED→WRITABLE/ReplyObserver 切换、注销后重装不改变 Bound 账户；phase 清空不清资金 |
| E5 退休与存量活性 | Remove busy/时钟失败时 Charge 不提前退；Task Complete 等退休；额度耗尽/发行者 Arc 丢弃后仍能消费事件、重试清理和退原账，不新增 acquire |
| E6 权限与隔离 | 相同 payer 不允许跨 Task 注销；A 局部满而全局有余时 B 可用；全局满则 B 局部有余也拒绝；释放真实 owner 后恢复 |
| E7 范围/生命周期 | H 固定存储保留到真正释放；view clone 不开新账户；源/InputBytes 与 Task 名额各按原 owner 释放；不以 Slot 结束提前退款仍存活的别处领域 payload |
| E8 真实多账户业务（主计划 W04-02） | 同一 Budget 的独立 A/B 入口、请求/Watch 控制、Query/Ack/Cancel、终态壳、回复未领取及注销延迟，核原付款及最终 baseline；同一 Dispatcher 含 A/B 下游调用 |

E8 的场景、锚点、日志及完成状态唯一记录在主计划。若需同时区分 host 与两个租户，可在同一 Budget 建 H/A/B 三账户；“双账户验证”的关键是不同付款身份与共同全局限额，不要求物理上恰好两个 Account。测局部隔离时确保全局还有余量；测全局总限时单独构造条件。

执行顺序：受影响 host 用例（显式 host target）及开发编译→完整调用链迁移后 `just check`/`just clippy`→必要正式 QEMU 路线。跨机制组合收口按手册执行 `just acceptance`，可与主计划共用同基线证据；通过的检查不因文档整理重复运行。代码实施前本篇不提供任何执行通过结论。

## 分工、删除门与完成条件

1. MellowPond 已完成全文复核；主计划链接本案并闭合入口装配/拒绝路径，排期确定后由 RubyRaven 接续。同一责任链内按依赖编辑，不为中间编译状态建 adapter。
2. 通用前置实施 TaskSubmission、source funding、实际扣账和失败/退休，迁所有直接消费者；主计划完成可信角色与控制上下文、boot 多账户和正式业务。
3. E1—E7 及[交付前结构检查](../REVIEW.md)在本案记录；E8/W04-02 在主计划记录并由本案链接。通用前置完成不等于 W04-02 或整个 Watch 完成。
4. impls 随代码回填，两个计划各自记录事实、未覆盖窗口和下一步；没有实现证据时不归档、不标系统能力交付。

删除门：Runtime 全体任务/来源的 `self.account` 默认账；省略 Task 付款的旧 spawn 接口；只返 T 的 spawn 失败结果；混合口径的旧 Runtime::input_budget；重装时从当前 World 取新 payer 的路径。保留有独立合同的 Task 默认来源，不把它误删为兼容接口；它继承的是显式 Task 绑定，不是 Runtime 总账。

来源只存原 Charge，退款不查当前账户。没有为不同 payer 新建运行体、全局账户查找表、新的内核对象或跨域回调。费用尚未覆盖的物理存储范围必须继续明确，不以此次迁移洗掉边界。

## 后续触发与当前证据

完整逐字节执行存储记账不属于本次能力：现状是 Task 名额、声明范围内的 InputBytes 与领域费用并存；实现者不得增加重复账。触发条件是明确承诺每项执行存储按原账户计字节、或以测量证据要求更强容量隔离。届时由 libexecution 机制 owner 与领域存储 owner 在独立任务中逐项确定实际分配、持有/释放位置及计量，替换触发即删除旧的重叠估算；验证须覆盖 inline/Box payload、容器容量和失败回滚，不能仅比对 sizeof 常数。更强的跨域 metadata 硬容量继续由[预算分案](../todo-2026-09-14-kernel-memory-budget.md)拥有。

通用源码、全部直接消费者和相应impls已完成，旧默认账/旧spawn/混合预算删除门已核对；E1—E7各类证据见下节，不把host多账户等同于QEMU多账户。主计划W04-02仍待boot同Budget多账户装配及E8实际业务/退款；下一步由RubyRaven按主计划988接通，MellowPond协调。该门完成后再进Bind/Unbind→流→Copy。本案无commit/push，未登记固定提交Review；原设计轮只编辑本篇的历史事实不作为当前施工状态。

## 施工记录（2026-10-10，RubyRaven）

core路线复验事实由主计划W05-06拥有：唯一Justfile预算翻倍，完整场景到reset后仍缺旧G05-20锚点；不把该运行记作本案验证。已全文阅读本案并按符号重扫实际消费者，辅助explorer sub-13只盘点，主实施者亲读runtime/work_queue/source_registration及RPC持有/回执链。没有修改Kernel ABI、libbudget账户机制或引入新运行体。

**核心初稿。** funding.rs拥有TaskSubmission与出生SourceFunding；Slot持不可变execution view与实际Task Charge，InsertFailure/RequestFailure::Spawn返完整提交物。Runtime单独持CommandBuffer，Requests<'a,T>在advance由当前Slot view和缓冲独占借用构造，无Clone/Default/Option付款字段；暂存Register命令已经解析并拥有view，Gate只用它。固定scratch/命令布局预算与来源预算分开checked计算，旧Runtime默认账户已删。SourceRegistration只复位phase，出生Task/Bound绑定保留，SourceEntry仍只有原Charge；退休/失败代码不新增扣账。

**开发基线记录。** core-dev1.log只证明初稿的libexecution非test开发check；该断点已由后续完整迁移取代，不是当前下一步。

**迁移与删除。** srv_fs/pm/init所有Runtime及Requests提交均显式TaskSubmission；Ingress从Request固定execution view提交，失败回到既有未准入拒绝owner；init duty保留出生view，失败恢复task后以同一view/上界重试，不让runtime helper吞掉payer。Dispatcher.shared reply出生Bound(H)，PendingCall在begin/for冻结私有view，Outbox明确SourceFunding。fs/service各自Funding私有构造，仅真实发行边界借用两view校验account_id；原角色、派生、注册/Watch控制、Request及终态壳保留配对，Root存储费由显式入口域账户支付。显式revoked fs grant仍有付款索引。同步Caller/ReplyCleanup不持执行来源，不补无用资金字段。没有新Account/Kernel表、ABI/正文付款id、adapter、第二运行体或set_payer。

删除证明 artifacts/execution-funding/deletion-proof.json 中四项rg均exit1零匹配：libexecution self.account默认账、旧spawn签名/Spawn task字段、Runtime::input_budget、bare Requests<T>。全体实际spawn参数另列，含TaskSubmission ctor或完整返回submission。原Task默认来源属于显式继承，保留其独立合同。

**验证与证据边界。** RV64 workspace bins开发check consumers-target-dev3.log=0；此前dev1为误用不存在的stress feature，dev2发现漏传withdraw视图及slot的Option处理，日志完整保留，修后不建过渡接口。host-all-final1.log=0：libexecution38、libfs30、libprocess4、libservice12，另2 compile_fail Requests作用域doctest；其后只新增E5精确窗口及E3准备分配失败的完整funding/max_sources断言，host-final-window.log=0：libexecution39、libprocess4、scope doctest2。host-lint1/window.log=0；just check check1.log=0、just clippy clippy1.log=0（七个检查面）、git diff --check=0。源码与测试分离后，最后变化仅host夹具/用例和说明，不重复正式代码检查。

| 门 | host/源码证据 | 正式单账户运行与剩余范围 |
|---|---|---|
| E1 | 同Runtime实际A/B Task debit，H固定baseline不动；局部Quota/globalQuota拒绝完整提交并恢复。现有Spawner/Gate验证父结束后的拥有命令。scope view借用及命令owning snapshot源码检查 | core普通显式提交/父派生实际执行；实际不同payer任务仍E8 |
| E2 | 默认Task来源实际扣A；H任务的Bound A/B及共享H同时实扣，mask重装仍各回原账 | core Dispatcher/Consumer/Outbox实际跑；同Dispatcher实际A/B调用仍E8 |
| E3 | Requests容量、队列表、Quota、libprocess注入allocator拒绝及失败Gate reject完整返task/max_sources/view，新收费rollback；prepared payload责任源码审阅 | core既有Quota拒绝/失败隔离接缝实际执行；不同账户领域payload窗口仍E8 |
| E4 | SourceOps拒绝实测refund，Bound helper拒绝复位与mask重装保持原账；现有取消安装/旧回执用例保留 | core实际Outbox/Dispatcher单账户生命周期，不宣称跨账户真实ReplyObserver切换已验 |
| E5 | busy保持原Charge；精确新增Active来源实际Remove成功→now_ns失败→回执前仍收费，额度已耗尽时以ObjectNotFound重试清理、只退原账；Finalizing先前时钟失败用例保留 | 真实Kernel Remove延迟/取时故障注入不由host替代，E8相关窗口主计划承接 |
| E6 | 同payer跨Task注销在普通/Complete/失败Gate被拒；A局部满B仍可用、global满B拒绝、真实owner释放恢复 | core角色拒绝/退出收束正常；独立账户真实隔离仍E8 |
| E7 | Runtime固定H baseline到实际Runtime.close，Assembly实际close后归零；view克隆和issuer owner释放不新账户；Task/来源分别退，外部领域Charge不由Slot退款。两域错配account_id返回错误 | core所有既有资源归零/监督/reset锚点到达；完整逐分配字节账不在承诺内 |

**正式复验。** just virt 的 virt1.log/virt1.exit=1；guest artifacts/failed-acceptance-20261010-174321-76550.log 到正常Shutdown reset，无panic。共同required+core共50锚点核对，非FAL缺失0，首失败仍旧G05-20 Open；新两provider独立Watch、DeriveDirectory/Rename、registry/退出消费者存在，事实见virt1-summary.json。guest日志创建到末写跨度48.521398s，包含少量wrapper/flush、不含构建，不是精确guest时钟；不把相比前轮的时长变化归因为本案，也不称严格路线通过。脚本、场景及其他route默认未改。

**结构检查。** 实施者依REVIEW核对Slot唯一不可变资金、CommandBuffer独占借用/Gate拥有快照、Charge实际退款、来源权限owner、控制终态保留、Ctor私有及错误返还、消费者完整迁移/删除、说明归属；没有新unsafe/锁或跨hart共享，现有单actor与Arc预算机制不变。sub-14 reviewer亲读新合同与核心并抽查消费者，未发现可达新增代码finding。其指出的E5旧用例只覆盖Finalizing不是Active成功Remove后时钟失败，已新增精确host窗口补测；指出service/付款计划旧状态已修。RPC实际跨账户切换与E8仍明确未覆盖，没有用source/host洗成系统交付。更少缠绕方案就是当前单缓冲+借用view，不保存长期Requests/可选scope、不双持payer id；Root/Funding属于实际域发行知识，不新增反向依赖或公共拼接构造。
