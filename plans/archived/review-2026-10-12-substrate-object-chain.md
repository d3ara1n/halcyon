# 基础内核/运行时责任链：固定提交 Review

状态：已完成，只读审查。未修改生产代码、导航计划或其他 Review 计划。

主固定目标：`ac9d7d9364770a69c8135e918d512069980c06bb`
父提交：`2aee0286dae3c3a8118f8d24f96e79a35f337c92`
主计划：`plans/archived/todo-2026-10-07-object-chain-convergence-review.md`
次级固定目标：`606b59db22f071943a4bfc2c174633454e449401`
次级计划：`plans/archived/todo-2026-09-frame-source-selftest-review.md`

本报告的源码位置和结论均以固定提交为准，当前 HEAD 不替代固定基线。旧的公共操作所有权、Runtime/RPC/Outbox 审查仅用于确认替代机制；已删除的 DebtLedger、ProcessDrain 和旧来源相位不在本次重审范围。

## 结论摘要

| 编号 | 严重度 | 类型 | 结论 | 建议归属 |
|---|---|---|---|---|
| F1 | P1 | 实际反例 | 取消中的来源在迟到注册回执后只进入 `RemovePending`，没有产生 Remove 操作或重新调度，任务和来源可永久停驻，阻塞 Runtime 关闭 | `user/libraries/libexecution` 的 `SourceRegistration` 与 Runtime/Gate |
| F2 | P1 | 实际反例 | IPI 失败时，已经进入无定时器 `wfi` 的 hart 没有结构性唤醒或重试保证，可能永远不消费 `runtime_stop` 请求 | `os/kernel` 的 `runtime_stop`、调度 idle 路径和平台门铃策略 |

除 F1、F2 外，本次固定范围内没有确认新的基础 owner、退款、对象关闭或 VM 映射反例。仍有动态验证缺口，见“证据边界与残余风险”。

## Findings

### F1：迟到注册回执未触发注销，来源和任务可永久停驻

**类型：实际反例。**

**固定位置**

- `user/libraries/libexecution/src/source_registration.rs:75-121`
- `user/libraries/libexecution/src/runtime.rs:1135-1183`
- `user/libraries/libexecution/src/work_queue.rs:584-617`
- `user/libraries/libprocess/src/observation.rs:145-231`
- `user/libraries/libprocess/src/supervise.rs:367-406`

**可达路径**

1. `ObservationSlot::begin` 入队 Register 请求，`SourceRegistration` 进入 `Installing`。
2. 任务停止时，`SuperviseTask::advance` 调用 `ObservationSlot::cancel`；注册状态进入 `CancellingInstall`，结果记录为 `ObjectClosed`。
3. 注册尚未回执时，`ObservationSlot::poll` 因 `registration.is_installing()` 不执行 `remove`，随后 `SuperviseTask::advance` 返回 `Step::Parked`。
4. `WorkQueue::finish_advance` 只有在 `Step::Parked` 且存在 pending input 时才重新调度；此时请求已被 Gate 消费，没有 pending input，任务不再入队。
5. 之后 Gate 仍可在 `allow_sources` 为真时完成原 Register。`Runtime::apply_operation` 调用 `SourceRegistration::registered`，而 `CancellingInstall` 只转换为 `RemovePending`；该回调返回后没有入队 Remove，也没有重新调度任务。
6. 因此真实来源已经进入 Runtime 的来源表和等待集合，但持有注销责任的任务永久停驻。后续 `Runtime::close` 仍观察到来源或任务存活并返回 `ObjectBusy`；没有路径交付 `unregistered` 或释放来源。

**违反的契约**

SourceRegistration 的注释和状态机要求“安装中取消保留回执责任”，并要求 Runtime 在实际 Remove 成功后交付 `unregistered`。迟到回执不能只改变本地相位而丢失提交后的 Remove 责任。任务进入 Parked 后，Runtime 仍必须保证已提交来源的清理能够由责任持有者继续推进，不得依赖已经停止且未再调度的调用者。

**影响**

这是跨取消、迟到回执、最后引用和 Runtime 关闭的生命周期泄漏。来源实际已分配但没有退款/退休，任务也不能完成停止；在监督任务或 Runtime 关闭路径上可表现为永久 `ObjectBusy`。该问题位于基础 `libexecution` 责任链，不是 FAL、fs 或 RPC 业务语义。

**建议归属与方向**

由 `libexecution::SourceRegistration` 与 Runtime/Gate 共同修复。迟到 `registered` 必须产生可执行的后续动作，例如返回“立即注销”结果，由 Runtime 在同一 Gate 处理中入队 Remove 并保证任务或独立 Runtime owner 被调度；或者建立等价的 Runtime 侧收束队列。修复必须保留 token 身份校验，并继续由实际 Remove 成功路径唯一交付 `unregistered`。不能仅把 `CancellingInstall` 改名为终态或在 Drop 中隐式注销。

### F2：IPI 失败后无定时器 idle hart 没有停止请求的结构性消费保证

**类型：实际反例；属于旧 R4-B1 责任链的未闭合路径，不是对已删除机制的重复审查。**

**固定位置**

- `os/kernel/src/runtime_stop.rs:10-42`
- `os/kernel/src/registry.rs:251-268`
- `os/kernel/src/sched.rs:653-690`

**可达路径**

1. Hart B 已无 ready work、无维护工作、无远程 VM 工作，也没有未来 timer，进入 `sched::idle`，卸载 timer 后执行 `wfi`。
2. Hart A 发布运行停止请求。`runtime_stop::request` 设置全局 `REQUESTED`，对 admitted harts 只调用一次 `try_ipi_slots`。
3. 假设向 B 的 SBI IPI 返回失败。`try_ipi_slots` 只返回失败位图，没有持久的待重试门铃；`request` 也不建立独立唤醒源。
4. A 随后在安全点将 `STOPPED` 置位并停驻。B 没有 timer，也没有成功 IPI 或其他未来中断时，仍可停留在 `wfi`，无法执行 `runtime_stop::check`，因此不会设置 `PARKED`，也不会进入终态。

**违反的契约**

`runtime_stop` 的已发布请求是必须由所有仍拥有运行责任的 hart 消费的终态事实。当前实现注释称平台失败由后续锁外安全点吸收，但无定时器 `wfi` 本身允许无限等待；一次失败的 IPI 不能证明存在后续安全点。终态不能依赖调用者继续推进或偶然出现的未来业务事件。

**影响**

运行停止可能只停驻部分 hart。未停驻 hart 仍可能保有调度、对象维护或用户上下文责任，使 `PARKED` 证据不完整，并使停止、重启或故障收束依赖未承诺的外部唤醒。该问题是内核运行时活性和终态 owner 问题，不涉及设备/DMA 或上层业务协议。

**建议归属与方向**

由 `os/kernel` 的 `runtime_stop`、调度 idle 路径和平台 doorbell 策略共同闭合。需要提供可证明的重试或独立唤醒机制，或者把平台失败提升为有明确证据的 fail-stop 终态，并让所有调用方据此停止等待完整 `PARKED`。仅在发送方再次调用 `request` 或等待下一次业务 IPI，不能满足责任链。

## 覆盖范围

### 主对象链

已按固定目标检查以下责任链及其直接消费者：

- VM fault、uaccess、地址空间映射和本地刷新路径；
- funding reservation、funded extent、frame owner、split/partial failure 和 Drop 退款；
- boot-held extent、boot funding、frame/source 初始交接；
- `MemoryObject`、`Backing`、`View`、SegmentMap 使用计数、最后 capability 引用和根 Work；
- `Work`/`Job`、child sealing、closing children、Process/Job retirement 和父子 owner 顺序；
- `SourceRegistration`、Runtime Gate、来源添加/删除、Busy、超时、取消、迟到回执和 Runtime close；
- maintenance Work、remote transport、跨 hart 责任转移、IPI 门铃及 `runtime_stop`。

上述范围中，F1 和 F2 是确认的反例。MemoryObject/Backing/View 的最后引用与物理段 owner 分离、Job 的 child sealing、funded owner 在失败时退款，以及维护 Work 的提交后持有关系，未发现可由固定提交直接证明的重复 owner 或丢失 owner。

### frame/source/selftest 次级范围

对固定目标 `606b59db22f071943a4bfc2c174633454e449401` 及其主对象链接入，检查了：

- `BootHeldExtent` 的受保护 adopt、prefix/payload split 和 boot payload 映射；
- `BootFundedExtent` 的物理范围与 funding 记账顺序；
- `fund`/reservation/commit 失败路径和 owner Drop 退款；
- `MemoryPool` selftest、父 pool refund 以及 child source pool 的保活；
- source split、部分重叠拒绝和失败后的 owner 保留。

未确认该次固定范围新增的 frame/source/selftest 反例。child source 的存活由 funded pool owner 持有，source registration 的迟到回执问题则由 F1 统一覆盖，不另列重复 finding。

## 排除项与跨边界记录

明确排除：

- FAL、fs、libservice 的业务语义；
- Notification、Watch、Bind、Copy 的具体协议；
- 验收脚本；
- 设备和 DMA。

`librpc`、FAL/fs 交界只核对基础 owner 的交接位置。它们通过 `libexecution::SourceRegistration` 消费来源寿命；F1 若被触发，上层消费者会继承基础清理失败，但本报告不评价其业务结果，也不复制业务 Review 的 finding。

## 证据边界与残余风险

- 本报告只读取父提交与固定目标的差异及固定目标源码；当前 HEAD 更晚，不能作为目标已修复或未修复的证据。
- 未修改代码，也未运行新的构建、测试或 QEMU 验收。F1 是由固定源码状态机和队列收尾路径构成的可达反例，不以缺少运行测试降级为“仅证据缺口”。
- 既有 QEMU IPI 失败场景曾验证后续真实广播可以唤醒并恢复部分路径；这不能覆盖 F2 所需的“无未来 timer、无后续成功门铃”场景。
- 尚未有完整平台矩阵证明 SBI IPI 失败后的重试、hart 停驻、TLB 实际缓存行为和 maintenance WCET/栈边界。它们是残余验证风险，不在没有固定提交级反例时扩展为新的 finding。
- 报告没有重新审查已删除的 DebtLedger、ProcessDrain 或旧来源相位，也没有把基础 owner 问题改写为上层协议问题。
