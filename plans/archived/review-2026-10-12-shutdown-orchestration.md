# 系统关机编排设计 Review

## 审查状态与范围

本报告是第五批唯一独立 Review 的只读产物。审查对象为
`plans/todo-2026-09-system-shutdown-orchestration.md` 当前系统关机编排设计，
并核对其直接依赖的服务装配、设备体系、内核维护执行、进程自动回收计划，以及
`notes/impls/{startup,task,framework,service}.md` 和当前 `srv_init` 的关机路径。

未修改生产代码、关机计划、`plans/COMPASS.md`、Review 收口计划或其他报告；本文件是
本批关机编排 Review 的唯一新增文件。

明确排除：电源管理服务自身的未来设计、设备内部 MMIO/IRQ/DMA 语义、FAL 注册业务
协议、对象链/执行/RPC/运输固定提交的重新 Review。以下只检查这些领域与系统关机
编排的责任交界，不复制相邻报告已经记录的领域内部 findings。

## 审查结论

**暂不能把系统关机编排作为可实施的通用协议，也不能以当前 init 验收剧本宣称
系统级优雅关机已闭合。**

计划准确指出 `SystemReset` 不负责枚举服务、通知业务或等待资源，也正确把服务、
设备、自动回收和电源管理分到不同专题。然而正文仍停留在待设计问题清单：没有定义
用户态关机意图如何形成全局准入封闭和范围快照，没有定义跨服务/文件/设备/供应/进程/
维护责任的收束顺序和唯一聚合 owner，也没有统一的拒绝、失败、无响应、期限、升级及
强制终止状态机。`SystemReset` capability 的当前来源和同步调用可以证明授权负路径，
不能证明最终提交前置、提交失败终局或根监督者死亡后的责任链。

当前 init 确实有一条固定验收顺序：服务和测试 workload 完成后，向文件 provider
发停止信号，等待 provider/消费者报告，观察注册撤出，再提交 `Shutdown +
Requested`；失败路径由 `RootSupervisor` 继续清理后提交 `Shutdown +
SystemFailure`。这是一条测试剧本，不是对任意正式服务、动态委托域、外部工作、设备
硬件安全或系统范围快照的通用证明。

## Findings 摘要

| 编号 | 严重度 | 性质 | 结论 | 建议归属 |
|---|---|---|---|---|
| SO-01 | P1 | 设计缺口，当前实现边界 | 关机意图没有转成全局准入封闭、迟到拒绝和范围快照 | 关机编排；接服务装配 |
| SO-02 | P1 | 设计缺口 | 必须收束的对象范围、跨域依赖顺序和唯一聚合 owner 未闭合 | 关机编排；接设备/维护/自动回收 |
| SO-03 | P1 | 设计缺口，当前实现仅为局部事实 | 正常退出、拒绝、失败、无响应、期限、升级和强制终止没有统一有界状态机 | 关机编排；服务停止入口由服务装配提供 |
| SO-04 | P1 | 设计缺口，当前实现事实 | shutdown、reboot、system failure 的政策差异和绕过后的安全证明未定义 | 关机编排；接设备/维护 |
| SO-05 | P1 | 设计缺口，当前实现事实 | `SystemReset` 最终许可、提交失败及根监督者存活/意外退出的两类终局未闭合 | 关机编排；接启动/内核终局 |
| SO-06 | P1 | 设计与证据缺口 | 跨专题前置没有唯一 owner、触发条件、完成事实和延期标准 | 关机编排；各前置计划 |

## Findings

### SO-01：关机意图没有形成全局准入封闭和范围快照

**性质：设计缺口，当前实现只能证明固定验收边界。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`「必须闭合的语义」第 15-16
  项、「设计纪律」和「完成时」。
- `notes/impls/startup.md`「init/pm 当前政策」。
- `notes/impls/task.md`「Job 管理面」和「生命周期」。
- `notes/impls/framework.md`「进程装配与行为限制」。
- 当前验收入口 `user/services/srv_init/src/main.rs:1311-1318,
  1959-1995`。

**可达或可推导路径**

1. 用户态某个编排者决定进入 shutdown、reboot 或 system failure；计划没有定义该
   意图的唯一 owner、授权输入、线性化点或广播/交接事实。
2. 当前 init 只在固定 workload 完成后依次通知已知的 `srv_fs` consumer/provider，
   再收集已知进程和服务；`RequiredLaunchSet` 只描述启动包中的固定验收拓扑。
3. 在关机意图产生、服务枚举和各项停止之间，其他已获得创建/启动/发布能力的服务
   仍可能创建子 Job、启动进程、发布服务、接收新请求或提交新的外部工作。现有
   `ProcessShell::Frozen` 只关闭单个进程的准入，不是系统范围的 admission barrier。
4. 计划没有定义全局 generation/epoch、已登记根集合、动态成员的冻结规则，也没有
   规定封闭后迟到的创建、委托、发布、请求和外部工作应返回何种事实，以及已经进入
   提交窗口的工作如何纳入本次范围。

**违反的设计契约**

- 计划明确要求关机开始后封闭新服务、新委托域和新外部工作的准入，并依据范围
  快照判定对象已经收净；但没有给出实际的封闭事实、快照内容或迟到规则。
- 任务模型的单进程 Frozen、Job Seal 和 Process/Job Done 各自只覆盖对应对象；
  不能把局部封口拼接成系统级封闭。
- 机制、授权与政策必须分离；已经提交的责任不能因原调用者或监督者消散而丢失。

**影响**

没有全局封闭点，任何“枚举当前服务/Job 后等待它们退出”的实现都存在逃逸窗口：
新服务或新委托域可能在快照之后出现，或者旧服务在快照之后提交外部工作。若实现
直接按枚举结果关闭 owner，会误收未纳入范围的工作；若继续允许新准入，则最终
`SystemReset` 条件永远没有稳定的集合。反过来，若只依赖 init 固定名单，则未在
名单中的正式服务不受系统关机控制。

**建议归属**

由关机编排计划定义一个唯一的意图接收/编排 owner，并与服务装配的启动授权、服务
发布和委托域入口形成单向接点。至少应冻结：

- 意图的授权来源、首个不可逆提交点和全局 admission generation；
- 服务、Job、Process、发布注册、文件/设备会话及外部工作各自的范围快照身份；
- 封闭后新准入的统一拒绝事实，以及已获提交资格但尚未完成的责任转交；
- 快照之后发现的合法新责任如何证明“不属于本次范围”或纳入同一收束 owner。

这只定义系统边界和责任转换，不复制服务目录的 Register 语义或设备内部停止协议。

### SO-02：收束对象范围、跨域顺序和唯一聚合 owner 未闭合

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`「必须闭合的语义」第 16-19
  项。
- `plans/todo-2026-09-device-system-design.md`「关键未决项与唯一承接」中的
  「整体停止和上级失效」及「全局场景矩阵」。
- `plans/todo-2026-09-29-kernel-maintenance-execution.md`「PM 与设备管理的责任
  边界」和「维护与设备侧」。
- `plans/todo-2026-09-29-process-automatic-reclamation.md`「终态屏障、发布顺序与
  观察」。
- `notes/impls/startup.md`「init/pm 当前政策」和 `user/services/srv_init/src/main.rs:
  1960-1995`。

**可达或可推导路径**

1. 关机需要同时处理服务业务、文件请求/流/订阅、已发布入口、设备绑定和供应、
   驱动/管理域进程、Job/Process 终止、对象 capability 释放、VM/地址空间退休以及
   已提交的内核维护工作。
2. 当前进程自动回收只证明单进程的 Frozen、Space.Done、私有退款和 Job 成员摘除；
   它明确不证明设备/DMA 退休、共享 backing、供应释放或服务业务完成。
3. 当前维护执行明确把驱动退出、CPU 映射撤销、硬件 DMA 已停止区分为不同事实，并
   要求设备 owner 在没有安全证据时保持隔离；它没有被关机计划聚合成全局完成谓词。
4. 当前 init 的文件收束顺序是一次具体剧本：停止/观察 consumer、停止 provider、
   观察注册撤出、关闭 provider 控制并收集进程。计划没有说明该顺序对多个正式服务、
   有状态文件系统、共享供应、设备绑定或并行停止是否成立，也没有定义谁在跨域等待、
   谁拥有最终收束责任。

**违反的设计契约**

- 不能以 Process DONE、Job Dead、单次 workload 完成、provider stopped、维护批次
  完成或注册 Withdrawn 单独代替业务收束、设备静默或资源安全释放。
- 每个事实必须有唯一 owner；系统级聚合不能让调用者、根监督者或最后一个观察者
  临时承担其他领域的清理责任。
- 设备计划和维护计划已经明确“局部完成不能冒充系统已收束”；关机计划必须消费
  这些明确事实，而不是重新定义或省略它们。

**影响**

如果实现按进程/Job 终态推进下一阶段，可能在服务仍有已提交 I/O、设备仍可 DMA、
供应仍被共享租约持有或维护 Work 仍未完成时释放资源或提交 reset。若实现反过来
等待所有 capability/观察壳消失，又会把合法共享对象和不阻塞系统终局的 observer
错误纳入范围，造成无界等待。没有唯一聚合 owner 时，跨阶段失败也无法判断应由
服务、设备管理者、维护记录还是关机编排者继续推进。

**建议归属**

由关机计划建立“范围事实—完成谓词—owner—前置”的依赖图，而不是只写线性通知
顺序。至少分别列出：

1. 新业务/委托/外部工作已封闭；
2. 服务业务 drain、发布入口撤出和已提交请求的领域终态；
3. 文件系统会话、来源和供应资源的释放事实；
4. 设备绑定/供应停止所需的硬件安全证明（由设备计划提供）；
5. Process/Job 私有终态、对象/VM/维护 Work 的已提交责任；
6. 仍合法存活的共享对象、观察壳和隔离资源为何不阻塞最终提交。

每个节点应有唯一推进 owner、可并行关系、依赖唤醒、失败出口和最终范围快照
引用。设备内部 MMIO/IRQ/DMA 语义仍归设备计划，本 finding 只要求关机拥有其
消费条件。

### SO-03：正常退出到强制终止没有统一的有界状态机

**性质：设计缺口，当前实现只有局部 deadline/retry 事实。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`「必须闭合的语义」第 17-18
  项及「设计纪律」。
- `plans/todo-2026-10-07-service-assembly.md`「设计门」第 3、4 项（服务停止接点
  尚未形成系统协议）。
- `notes/impls/framework.md`「进程装配与行为限制」。
- `user/services/srv_init/src/main.rs:818-855, 1960-1995`。
- `user/services/srv_init/src/supervisor.rs:1886-1905`。

**可达或可推导路径**

1. 一个正式服务可能正常完成、明确拒绝停止、报告不可恢复失败、无响应、在期限
   内部分完成，或在停止请求之后继续产生迟到结果。
2. 当前文件 consumer 使用固定 phase deadline，超时返回错误；`RootSupervisor`
   对若干 owner 保留 retry disposition，并在失败清理集合为空后尝试
   `SystemReset(SystemFailure)`。
3. 这些局部路径没有与服务业务 drain、注册撤出、设备停止、供应释放、进程 Kill
   和强制终止后的回收闭包共享统一状态/结果分类。计划也没有规定“拒绝”是系统
   failure、可降级、可重试还是保留隔离后继续。
4. `ProcessControl.Done` 和 `Job` 完成可观察，但没有定义服务级“业务已收束”与
   “强制终止后所有已提交责任仍由谁推进”的系统观察事实。

**违反的设计契约**

- 正常、失败、取消、退出、超时和强制终止必须沿同一稳定责任链收束，不能依赖
  调用者继续发送下一条通知。
- 期限是政策边界，不是硬件完成证明；超时后必须明确升级动作和保守资源状态。
- 已提交责任不因服务进程、调用者或根监督者消散而丢失。

**影响**

施工者会在每个服务或每个 provider 自行定义超时和重试，造成不同的“拒绝/失败/
无响应”含义。最危险的结果是超时后直接关闭 endpoint、释放 backing 或把进程
DONE 视为硬件/业务安全；另一种结果是不断 retry 同一观察而无升级上界，关机永远
无法进入最终阶段。服务已提交的 I/O、注册退休、设备维护和进程回收还可能被拆成
互相不知晓的清理 actor。

**建议归属**

由关机计划定义系统级有界状态机，服务装配只提供每项服务的停止入口和业务完成
证明。每个步骤应明确：

- `Accepted/Draining/Completed/Refused/Failed/TimedOut/Unknown/Forced` 等结果的
  语义和观察来源；
- 单调 deadline、有限升级次数和每次升级的 owner；
- 正常退出、拒绝、失败、无响应与强制终止之后分别保留哪些 endpoint、租约、
  backing、隔离资源和维护记录；
- 进程终止只关闭用户执行，不替代领域 drain；被强制终止的已提交工作如何转交
  稳定 owner，并以何事实允许下一阶段。

不要在本案重新定义服务目录 Active/Withdrawn 或 `SourceRegistration` 的内部状态；
系统状态机只消费这些领域已经确认的终态/未知结果合同。

### SO-04：shutdown、reboot、system failure 的政策差异和绕过证明未定义

**性质：设计缺口，当前实现只区分 reset reason，不构成政策。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`「必须闭合的语义」第 20-22
  项。
- `notes/impls/startup.md`「init/pm 当前政策」。
- `notes/impls/internals.md`「idle 与系统复位」。
- `user/services/srv_init/src/main.rs:1321-1343`、
  `user/services/srv_init/src/supervisor.rs:1897-1904`。
- `user/libraries/libkernel/src/system.rs:11-20`。

**可达或可推导路径**

1. 当前正常验收最终调用 `SystemReset(Shutdown, Requested)`；失败收束路径也调用
   `SystemReset(Shutdown, SystemFailure)`。这证明了现有剧本的两个原因编码，不证明
   三类系统政策。
2. `SystemReset` 是无等待叶子对象，调用成功意味着系统终止；它本身不执行业务
   drain，也不提供“跳过哪些阶段仍然安全”的证据。
3. 计划没有定义 reboot 是否必须完成与 shutdown 相同的业务/设备收束，system
   failure 是否允许跳过业务 drain、哪些供应/设备动作绝不能跳过，以及绕过后如何
   保留隔离、维护和退款责任。
4. 也没有定义不同政策下共享对象、持久文件、未确认外部写入、设备供应和观察壳的
   终态差异，或 reset 失败后是否继续原政策、升级为 fail-stop 或保守停驻。

**违反的设计契约**

- 紧急故障可以缩短正常步骤，但不能用未定义的“紧急”标签抹去资源/安全边界。
- 设备和维护契约要求无安全证据时保持隔离，不能以超时或 reset 请求本身证明已
  停止。
- 关机、重启和系统故障是不同用户政策，必须显式说明其前置、可绕过步骤和终局。

**影响**

同一个 `ResetAction`/`ResetReason` 组合可能在不同调用点被解释成不同的清理程度；
实现者可能为了让 system failure 尽快复位而释放仍可被设备访问的 backing，或者让
reboot 保留本应丢弃的业务状态。测试只能证明 QEMU 复位发生，不能证明绕过路径仍
满足 capability、隔离和已提交责任的边界。

**建议归属**

由关机计划补一张政策矩阵，至少逐项说明 shutdown、reboot、system failure：

- 意图授权和准入封闭是否相同；
- 服务业务 drain、文件同步/撤出、设备停止、供应释放、进程终止和维护收束哪些
  必须执行，哪些允许跳过；
- 每个允许跳过的步骤留下什么隔离/保活责任，谁支付并继续推进；
- 哪个事实允许进入 `SystemReset`，以及 reset 拒绝、后端不支持和异常返回的政策
  结果。

设备内部停止证明由设备计划拥有；本案只要求把“可绕过”与“必须消费的证明”写成
系统终局条件。

### SO-05：`SystemReset` 最终许可、提交失败及根监督者终局未闭合

**性质：设计缺口，当前实现事实暴露了边界。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`「问题边界」和「必须闭合的
  语义」第 21-22 项。
- `notes/impls/startup.md`「唯一 init bootstrap」和「init/pm 当前政策」。
- `notes/impls/internals.md`「idle 与系统复位」。
- `user/services/srv_init/src/main.rs:1265-1286, 1317-1343`。
- `user/services/srv_init/src/supervisor.rs:1886-1905`。
- `user/libraries/libkernel/src/system.rs:11-20`。

**可达或可推导路径**

1. bootstrap 把 primordial `SystemReset` capability 放入 init 出生块；init 采用后把
   handle 交给 `RootSupervisor`，并在验收结束时取回。
2. 当前 `submit_shutdown` 只验证错误对象和 rights attenuation，然后调用一次
   `system::reset`。调用返回错误时，根监督者进入 `idle_forever`；成功路径不返回。
3. 失败清理路径在 owner 集合为空后取出 reset capability，调用
   `SystemReset(Shutdown, SystemFailure)`；调用失败只记录错误，没有进一步的系统
   终局契约。
4. 计划没有定义 reset capability 是否始终由关机编排 owner 保留、哪些收束事实是
   最终提交的不可省略前置、提交竞争期间新的准入如何排除，也没有分别定义根监督
   者在“仍存活并提交 reset”和“意外退出而未提交/提交未知”时的终局。

**违反的设计契约**

- `SystemReset` 只有 capability 授权的最终整机动作，不负责判断系统是否适合复位；
   因此最终许可条件必须由用户态形成完整、可观察的收束事实。
- 已提交责任不因根监督者退出而消散；当前没有专用 init 全程自动 Drain 路径，
   不能把“由内核启动”或一次 reset 调用代替故障契约。
- capability 的取得、转交、最终提交和提交失败必须各有 owner，不能只在启动时
   证明一次 rights。

**影响**

当前实现能证明 init 拥有 primordial authority，也能证明错误对象/缩权副本被拒绝，
但不能证明“此时提交 reset 已经安全”。当 reset 被平台拒绝、不可支持、竞争繁忙
或调用进程在提交前意外退出时，系统没有定义谁继续监督已封闭的范围、是否允许有界
重试、何时进入保守 fail-stop，以及资源/设备隔离责任由谁保活。若 root supervisor
意外退出后没有独立责任根，已提交的服务/设备/维护工作可能只剩观察壳而没有终局
推进者；若错误地补一个临时内核 reaper，又会越过本计划排除的执行边界。

**建议归属**

由关机计划闭合 reset 提交流程：

- 明确唯一 reset capability owner、允许的派生/转交边界和最终提交锁存点；
- 把范围封闭、所有必需领域终态、设备安全证明、维护责任和可保留隔离资源列为
  最终许可条件，不能用单一 `Process DONE` 或 Job/Runtime 空集合替代；
- 定义提交成功、明确拒绝、后端不支持、竞争繁忙、异常返回和结果未知的有界状态；
- 分别写明根监督者存活时的提交终局，以及其意外退出时谁持有/接管已提交责任、
  是否只允许保守停驻、何时可判定系统失败。

`SystemReset` 对象自身的 capability 校验和 SBI 映射不在本 finding 重新 Review；
这里要求的是系统政策和最终责任接入。

### SO-06：跨专题前置没有唯一 owner、触发条件和完成标准

**性质：设计与证据缺口。**

**位置 / 章节**

- `plans/todo-2026-09-system-shutdown-orchestration.md`第 3、11、26、30-32 行。
- `plans/todo-2026-10-07-service-assembly.md`第 17-20、27-33、41-43 行。
- `plans/todo-2026-09-device-system-design.md`第 66-84、115-123、125-148 行。
- `plans/todo-2026-09-29-kernel-maintenance-execution.md`第 205-213、242-257 行。
- `plans/todo-2026-09-29-process-automatic-reclamation.md`第 59-68、109-128 行。
- `plans/todo-2026-09-power-management-service.md`全文。

**可达或可推导路径**

1. 关机计划把设备停止/供应释放交给设备体系，把服务停止接点交给服务装配，把
   电源管理服务列为未来独立设计，并依赖维护执行与自动回收提供局部完成事实。
2. 服务装配当前仍未闭合正式服务、就绪、停止和恢复；设备体系仍处 D1/D2-D6
   设计阶段；维护执行说明已经定义了机制边界，但明确不拥有设备业务状态或全局
   关机政策。
3. 这些计划没有共同的一张“关机前置 DAG”：谁先闭合、什么事件触发关机计划
   开始消费、什么事实才算前置完成、未完成时是阻塞/拒绝/降级/延期，以及未来
   电源管理服务何时成为必要接点均未登记。
4. 当前 startup/init 的验收顺序把已知 provider、consumer、pm-domain 和测试
   workload 组合起来，但没有把它们提升为正式系统前置，也没有给出未接通能力的
   唯一延期记录和删除/接通标准。

**违反的设计契约**

- 跨组件契约必须有唯一拥有计划，前置必须有 owner、顺序和完成门；不能通过互相
  引用把未决选择留给实施者。
- 基础机制完成、局部实现存在或一次验收通过，都不能替代系统能力的真实消费者和
  收束链。
- 未来电源管理服务不得被本案预设，但其未来接入触发条件和与关机编排的边界必须
  可定位，不能成为隐含的“以后再说”阻塞。

**影响**

在没有唯一前置表的情况下，关机施工者可能先按当前 init 剧本实现，再临时决定
服务停止入口、设备静默证明、维护 Work 观察或 reset 失败政策。这样会产生第二份
服务/设备停止真值，或者把尚未支持的设备能力伪装成已闭合。Review 也无法区分
“设计未决定”“当前平台不支持”“实现已有但证据不足”和“延期但不阻塞本范围”。

**建议归属**

由关机计划维护一份最小跨专题前置表，逐项列出：

- 前置问题及唯一 owner；
- 关机编排开始消费它的触发条件；
- 正常、拒绝、失败、超时、未知和隔离终态的完成事实；
- 对本次 shutdown/reboot/system failure 的阻塞或可绕过关系；
- 若延期，唯一承接记录、接通条件、删除条件和验证标准。

服务装配、设备、维护、自动回收各自只提供自己的领域事实；电源管理服务仍由其
未来计划拥有，不在这里提前设计服务拓扑或 capability 分配。

## 当前事实与证据边界

- `notes/impls/startup.md` 记录了当前 BootPackage、唯一 init、`RequiredLaunchSet`、
  `RootSupervisor`、fs 双 provider、pm-domain 和固定验收收束。它证明的是当前
  启动/验收装配和局部监督事实，不是通用关机协议。
- `notes/impls/task.md` 记录了 Process/Job/Thread、Frozen、Space retirement、
  Job 完成传播和维护 Work 的当前实现边界；本报告没有重新 Review 对象链、执行、
  Process/Job 或 VM 的内部正确性。
- `notes/impls/framework.md` 和 `notes/impls/service.md` 说明 Runtime/Assembly、
  service Register/Withdraw、来源退休和 provider 行为。这里仅检查它们如何被系统
  关机聚合，不复制服务目录协议或执行库 finding。
- 当前源码确有固定文件 consumer/provider shutdown、注册撤出观察和
  `SystemReset` rights 负路径；`RootSupervisor` 在失败清理集合为空时尝试
  `SystemFailure` reset，reset 失败后继续保持 supervisor/idle。没有源码或计划事实
  证明这些局部路径覆盖动态正式服务、设备安全停止、全局范围快照或根监督者意外
  退出。
- 本次未运行构建、host 测试、QEMU 或平台复位验收；这是设计 Review。上述 findings
  主要由计划缺失的契约和当前实现与计划边界的可达路径构成，不因没有新增运行测试
  而降级为纯测试缺口。

## 交界与不重复审查

- 服务装配报告已经审查正式服务的启动依赖、服务健康/恢复、Register 未知结果和
  服务级停止接点；本报告只检查这些事实如何进入系统范围封闭、跨域聚合和最终
  reset，不重复服务目录或 provider 内部 finding。
- 设备体系报告已经审查设备身份、authority、跨域事务、IRQ/DMA/供应和硬件安全
 证据；本报告只要求关机拥有设备完成证明的消费条件，不重新定义 MMIO/IRQ/DMA
  协议。
- 内核维护执行和进程自动回收计划已经拥有 Work/Process/Job 的执行与局部终态；
  本报告只检查它们作为系统关机前置时的交接、范围和聚合，不重新评估其固定提交
  或内部状态机。
- 电源管理服务仍是未来设计占位。本报告不预设其职责、拓扑、协议或 capability
  分配，只要求关机计划登记未来接入的唯一触发和边界。
