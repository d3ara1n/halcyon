# 公共操作所有权与内核执行结构收束

> 状态：P0–P6 已全部完成并归档。专题最终结构保持独立领域执行器和债务容量，只统一账本机械协议、请求代次仲裁、Process 级退休交棒与两组安全点公平预算；公开 ProcessDrain、REAPABLE/CLOSED 和管理者职责未改变。最终 host、七面 clippy 与完整 acceptance 证据见文末 P6 收口记录。
>
> 本文是公共操作所有权专题的只读实施档案。共享算法/目录已由 [包归属档案](todo-2026-09-13-workspace-package-ownership.md) 完成，FAL 领域与业务由 [总计划](../todo-2026-09-fal-service-capabilities.md) 拥有；自然顺序已经进入 FAL 的重新接手与规模审计。

## 已明确的能力与职责

- ProcessDrain 是当前唯一公开的内核资源回收步进 syscall，要求目标 ProcessControl 的 MANAGE；由受信任管理者负责，普通应用不用在运行循环中替内核轮询清理。是否只有 pm/init 具有管理权取决于实际 capability 交付，内核不按服务名或 PID 授权。
- REAPABLE 在目标线程、active hart、Building 操作与 mandatory 操作通过屏障后持续成立；管理者据此开始回收，More 表示仍有工作。`libprocess::collect_process` 已实现等待 → Drain → Query → Close；当前 init/pm 在集成剧本中调用，常驻事件驱动监督不视为已交付。
- Job 完成要求 sealed 且成员/child 表为空，大量进程资源已由各自 ProcessDrain 收束；自身 CLOSED 和祖先摘除复用有界 CompletionCursor，不新增 JobDrain。
- WaitSet 普通 Close、内存/Tunnel 提交后的同步与退休、通知完成及未发布回滚由内核闭合；内部 drain_current 不是用户 ABI。管理者的域级回收与内核必成尾段可以一致分层，共用算法，不要求所有层都由同一主体驱动。
- 回收是管理服务职责，不需要先建立预算激励。页/metadata 预算仍用于分配和隔离，独立触发，不成为本项的动机前置。
- 用户可以跳过 rinlib、乱序、遗漏后续调用或退出；内核必须保持合法状态、稳定 owner 和资源守恒。持有合法资源、业务不进展、可接管回收和责任丢失是不同结论，不能混称内核状态损坏。

## 规模审计与拆分裁决（2026-09-16）

本次盘点以当前真实调用者为准，不按文件数量拆任务。公共操作的责任链已经覆盖：

```text
来源观察/线程返回
  → WaitContext/ObserverSink 完成仲裁
  → WorkDebt 预付、park/wake、step、finish
  → 对象通知与句柄退休
  → ProcessDrain / 内存 completion / unpublished rollback / Job finalization
  → mandatory、REAPABLE、CLOSED、结果回传与退款
```

### 真实消费者与边界

- **等待入口**：`WaitMany`、`HandleClose`、`ProcessDrain` 都经 `WaitPlan → sched::park_request_wait → wait::install`；`WaitContext::finish_step` 当前直接执行 `DrainRequest`，使等待层知道具体请求的批次、`ObjectBusy` 和 `ReachLimit` 语义。
- **WorkDebt 推进**：`deferred_work` 维护 MemoryChange、Unpublished、Termination、Finalization 四类债务；`notify_work` 维护对象通知与 Finish 债务；`retirement` 另有对象退休债务。三处分别重复组织 `arm_wake → dependency → park → wake → finish`，但仍必须保留各表的预算隔离和 FIFO/owner 语义。
- **对象契约**：`KernelObject` 同时承载身份、观察订阅、通知排水、句柄关闭和退休能力；`ObjectWaitState` 同时保存电平真值与通知调度游标。WaitSet 是当前唯一 `RetirementTarget` 实现，但其退休执行还负责 work slot 归还和回复组织。
- **退休与 ProcessDrain**：普通 `HandleClose`、进程退出的 `retire_entry`、WaitSet 注册失败清理共同进入 `RetirementTarget`；ProcessDrain、Unpublished debt、Finalization debt 共同消费 `Process::drain_batch`，但目前由 `drain_gate`、`drain_active` 和多个 `Dependency` 分散仲裁。
- **其他真实消费者**：`MemoryChangeCompletion::advance_retire` 负责内存事务完成后的 mandatory/REAPABLE/等待者收束；`UnpublishedBound::rollback` 负责启动失败回滚；`Job::CompletionCursor` 负责 Job 成员/子 Job 完成传播；线程离场、Notification、Mailbox、MemoryObject、Lifetime、Tunnel 等对象均消费通知排水和完成责任。
- **驱动与退款**：`sched`/`trap` 安全点驱动 deferred/notify 排水；每张 WorkDebt 表有独立预付槽和 Reservation Drop 退款；Pending 电平与 IPI 门铃是唤醒真值，门铃失败不能替代 pending。

### 结构审计结果与当前状态

| 项目 | 当前结果 | 后续归属 |
|---|---|---|
| 多套 `arm_wake/register/park/finish` 编排与 Reservation 退款机械代码 | P1 已统一账本机械状态；领域 `step`、来源复检和完成回调保持独立 | 维持现状，除非 P5/P6 发现相同语义重复 |
| `WaitContext` 直接持有 `DrainRequest` 并执行 ProcessDrain | P2 已关闭；请求执行已移入 `DrainExecutor`，等待层仅保留完成/取消接缝 | 已关闭 |
| `ObserverSink`/`FinishClass` 混合线程、请求和持久观察完成政策 | P3+P4 已收窄为共享 offer/finish 交接；请求首用/重启政策由 `DrainExecutor` 持有 | 已关闭 |
| `RetirementTarget` 同时组织退休状态、执行容量和等待回复 | P3+P4 已拆出 `RetirementTicket` 与 `RetirementCompletion`，driver 在锁外交付 | 已关闭 |
| `KernelObject` 与 `ObjectWaitState` 混合身份、电平观察、通知排水、句柄关闭 | 身份/权限基础契约保留；通知执行改为 `advance_waiter` 单步接缝，来源锁仍持电平真值 | 已关闭，P5 不重开 |
| ProcessDrain、Unpublished、Finalization 共享 `drain_batch` 但维护多份驱动真值 | P5 已收敛为类型化 Process 级结果、阻塞和批次许可接缝；各债务表仍保留独立容量与 owner | 已关闭；P6 只做组合验证 |
| `MemoryChangeCompletion` 的完成/退休交接 | 已通过 `RetirementCompletion` 接入锁外交付；继续保留独立 Remote ack 和内存退休状态机 | 已关闭，P5 只处理其 Process 级驱动交接 |

### 拆分决定

本专题保持为**一个机制闭包**，不拆成可独立完成的“等待任务”“请求任务”“退休任务”。三者共享 owner 转移、预付容量、唤醒、取消、完成与退款；任意一项单独交付都会留下双轨或未归属责任。

施工阶段按以下依赖组织，阶段之间不登记独立验收，也不把阶段提交描述为专题完成：

| 阶段 | 施工焦点 | 必须保持的边界 | 依赖 |
|---|---|---|---|
| P0 | 完成类型、owner、锁序、唤醒和退款图；冻结未决语义 | 不改公开 ABI、ProcessDrain、REAPABLE/CLOSED 或用户监督分工 | 无 |
| P1 | 收敛 WorkDebt 的通用推进协议：预付、发布、step、arm/wake、park、finish、取消和退款 | 保留各债务表的容量、owner、FIFO 和安全点预算；不合并不同工作类别 | P0 |
| P2 | 分离请求执行与等待仲裁；`WaitContext` 不再持有具体 `DrainRequest`/`FinishDependency` | `WaitMany`、`HandleClose`、`ProcessDrain` 仍共享合法的 WaitPlan/park 语义 | P1 |
| P3+P4 | 合并对象契约与退休执行：拆分身份/观察/通知/句柄能力，分离退休状态与执行器的容量、调度、回复责任，并迁移 WaitSet、内存完成与普通 Close 的真实消费者 | 保留统一电平真值、来源锁序、退休 ticket、完成回执和失败 owner；不恢复用户 Seal/Drain ABI | P1、P2 |
| P5 | 收敛 ProcessDrain、Unpublished、Finalization 三条驱动路径的仲裁真值和 driver 接缝 | 保留 ProcessDrain 管理者职责、有界批次、REAPABLE 屏障和 Job CompletionCursor；不重新设计对象退休后端 | P2、P3+P4 |
| P6 | 全部真实消费者迁移、删除重复编排/失效分支，并执行专题级组合验证 | 覆盖正常、失败、取消、退出、接管、跨 hart、旧 epoch/handle 和完整退款 | P1–P5 |

依赖图为：

```text
P0 → P1 → P2 → P3+P4 → P5 → P6
```

P3 与 P4 原先看似可以分开，但两者共同拥有 `KernelObject::retirement()`、`RetirementTarget`、WaitSet close、普通 HandleClose 和内存完成后的退休交接；拆开会把退休 ticket、执行槽和回复责任留在两个阶段之间。因此合并为一个闭包。P5 只在该闭包稳定后处理 ProcessDrain、Unpublished 和 Finalization 的多驱动仲裁。P6 才是本专题唯一的整体完成与验收阶段。

### P2 设计闭包

P2 采用“独立请求执行者 + 独立等待完成”的方案，不把 `DrainRequest` 换成 trait 后继续让 `WaitContext` 执行业务。历史实现中 `WaitContext::finish_step` 曾同时推进请求、退休依赖和线程交付；P2 已将请求执行移入 `DrainExecutor`，当前 `finish_step` 只注销订阅并完成线程交付。P3+P4 继续收窄剩余的可复用完成存储和取消接缝，不重新把领域执行状态塞回等待层。

目标类型与所有权：

```text
调用者 Thread
  └─ RequestWork / DrainBatch
       ├─ 目标 Process + 已验证 ProcessControl
       ├─ 调用者 Process + output/budget/work_done
       ├─ ThreadResultObligation
       ├─ drain_active 批次许可
       ├─ 捕获的 WaitIdentity/epoch
       └─ 当前 RetirementDependency 与登记 key

WaitContext
  ├─ WaitCore/WaitEpoch
  ├─ admitted thread、订阅和 timeout
  ├─ finish slot 与最终交付状态
  └─ 取消请求的窄接缝
```

`WaitContext` 删除具体 `DrainRequest` 和 `FinishDependency` 字段及其 `finish_step` 分支；它只负责完成仲裁、订阅注销、结果交付和线程返回。请求执行者独立推进 `drain_batch`，在业务结果写回完成后才向等待上下文 offer `KernelComplete`。请求不会读写用户线程寄存器；寄存器/`sepc` 处理仍由等待层完成。

请求状态机为：

```text
Idle → Prepared → Runnable/Taken
  → Runnable
  → Blocked → register dependency → Parked → Runnable
  → Finalizing → Idle
```

等待状态机保持：

```text
Installing → Armed → Finishing → Done
```

两者不共享终态真值。正常完成顺序为“请求写回结果 → 释放请求责任 → offer KernelComplete”；取消顺序为“WaitCore 标记 Abandoned → 请求 owner 取消当前依赖并停止追加工作 → 释放批次许可/结果义务 → 等待层完成线程交付”。目标 Process 已启动的退休责任继续由目标自身和后续管理者收束，取消不回滚已提交的目标资源工作。

线性化与生命周期约束：

- 批次准入继续在 `drain_gate` 下取得 `drain_active`，输入、预算、调用者身份和输出地址随后冻结。
- 等待完成只能表示请求结果已成立；不能在 `bind_request` 阶段预先 offer `KernelComplete`。
- 请求捕获 `ThreadResultObligation`，保证结果访问期间调用线程不会越过 DONE/REAPABLE；`Arc<Process>` 只保证进程结构存活，不能替代结果义务。
- WaitPlan 安装前丢弃必须消费启动责任并完成取消收尾，不遗留 `drain_active`、依赖登记或结果义务。
- 旧 epoch 的取消只能命中同一轮依赖；下一批次须同时满足旧请求已退休、依赖已脱离、结果义务已归还、等待已 Done、请求槽已回交且 epoch 可递增。
- 依赖登记继续复用 P1 票据：`arm_wake → 来源锁内 register+复检 → park`；早到 Wake、park 前 Wake、取消前后竞争均由同一 WakeAction 闭合，不新增 raw 取消通道。
- 请求安全点工作与公开 `ProcessDrainResult.work_done` 分账；零业务进度的依赖登记/取消仍消耗执行预算，但不得虚增公开 Drain 工作量。

P2 暂不改变 `KernelObject`/`ObjectWaitState`、`RetirementTarget` 或 ProcessDrain 三驱动仲裁；这些分别由 P3+P4、P5 负责。P2 的真实消费者是 `ProcessDrain` 与现有 continuation/selftest 路径，必须删除 `WaitContext::finish_step` 的请求执行分支，不保留兼容双轨。

### P2 设计债务

| 设计债务 | 当前保留 | 删除条件 |
|---|---|---|
| 请求使用可复用完成上下文 | `DrainExecutor` 通过 `WaitContext` 的窄接口复用 epoch、finish slot 和线程交付；首用/重启政策已由 `DrainExecutor.used` 拥有，等待层只校验静止与执行 restart | 保留为当前最终接缝；不得泛化到普通 WaitMany/Sleep，后续只在出现第二个可复用请求消费者时再抽象包装 |
| 结果义务与请求工作槽容量 | 请求槽按 `PROCESS_GLOBAL_LIMIT` 出生预付，结果义务来自调用线程；两者不借用其他债务表 | 当前依据已冻结；容量调整只能随进程全局 admission 或真实成本重校 |
| 对象退休依赖包装 | 原单变体 `FinishDependency` 已删除，`RetirementTicket` 持有已提交对象根并封装登记、取消和完成查询 | 已关闭；P5 直接消费 ticket，不恢复请求层对象查询 |

### 阶段纪律

- 每个阶段可以有局部 host/type/test 检查，用于发现实现错误；这些检查不构成专题验收。
- 阶段期间不得为了通过局部测试保留没有删除条件的 adapter、重复 owner 或测试专用运行体。
- 发现公开语义、ABI、锁序或 owner 边界被当前实现推翻时，回到 P0/P1 修订任务图，不继续在后续阶段堆补偿代码。
- 只有 P6 完成全部真实消费者迁移、失败/退出/退款路径、旧路径删除和整体验证后，才更新本计划为完成并归档。

### P1 设计闭包

P1 采用静态泛型 `DebtLedger<T, SLOTS>`，每张内核债务表保留独立静态账本和 `TableId`。账本统一拥有：

- `Spinlock<WorkDebts<T, HARTS, SLOTS>>` 与 owner FIFO；
- Pending 电平的增减及 idle 可见性；
- 与来源账本绑定的 Reservation、Taken ticket 和 Wake ticket；
- Reserved → Pending、Taken → Requeue/Park/Finish/Rearm、Parked → Pending 的状态转换；
- Reservation Drop 退款、Wake 后原 owner 门铃和门铃失败后的 Pending 保留。

账本不拥有业务 `step`、来源条件复检、完成回调、回复交付或预算分配。四类 deferred debt、对象通知、Finish debt 和对象退休仍保留各自 payload、容量分区、推进顺序和安全点门铃政策；内存 completion、通知发布等已在当前安全点内可继续推进的路径显式使用静默发布入口。Taken 票据不能跨账本交还，避免相同 payload 类型的不同债务表混用。

必须保持的转换不变量：

| 转换 | Pending 变化 |
|---|---:|
| Reserved → Pending | +1 |
| Pending → Taken | 0 |
| Taken → Requeue | 0 |
| Taken → Parked | -1 |
| Taken + 早到 Wake → Pending | 0 |
| Parked → Pending | +1 |
| Taken → Finish | -1 |
| Taken → Rearm/Reserved | -1 |
| Reserved → Empty（退款） | 0 |

账本锁内只做固定槽、链和计数操作；业务推进、来源登记/复检、对象回调、payload 析构和门铃均在锁外。P1 不改变 WorkDebt 纯逻辑 crate、公开 ABI、ProcessDrain、REAPABLE/CLOSED 或用户监督分工，也不把不同债务表合并成一个队列。

### 当前设计债务与保留项

这些不是本阶段的完成缺陷，而是已登记、带归属和删除条件的后续责任：

| 设计债务 | 当前保留 | 归属与删除条件 |
|---|---|---|
| 各债务表仍各自编写 `step`/预算循环和来源复检 | P1–P5 保留领域执行器，只共享账本机械协议和 Process 级阻塞/完成接缝 | P6 只审计是否存在实际重复语义；没有第二个真实适配器时不再抽象 |
| `WakeAction` 仍以静态回调连接不同账本 | 保留窄的 raw wake ABI，避免为消除回调函数引入 unsafe 类型擦除 | P1 完成后若真实消费者需要统一来源协议，再在 P2/P4 设计；不得凭代码重复直接引入动态分配 |
| Reservation 的领域别名仍存在 | 领域类型继续表达不同容量、payload 和发布政策，机械 Drop/计数由 `DebtLedger` 统一 | P6 前保留；当调用点全部使用账本票据且别名不再承载语义时才删除薄别名 |
| 门铃日志从债务类别改为统一 Work-debt 文案 | 门铃失败的行为不变：Pending 保留、idle 仍可见 | 若诊断需要按类别定位，新增结构化来源标签，不恢复各表独立门铃实现 |
| `KernelObject`、`RetirementTarget` 和 ProcessDrain 的责任混合 | P2–P5 已分别收窄等待、对象退休和 Process driver 接缝；各层保留自己的状态机 | P6 只检查真实消费者、旧路径和重复 owner，不把账本 API 扩展成更高层执行器 |

P2 施工中识别的两项设计债务已经关闭：请求债务已并入通知/Finish/退休的共享 16 步预算；取消接缝已收束为 `WaitOperation` 的 start/cancel 协议，WaitPlan 独占启动责任，WaitContext 持弱取消引用。取消不执行请求 step，也不新增独立 cancelled 真值。

### 当前施工状态

P1 已完成施工：内核新增静态 `DebtLedger<T, SLOTS>`，并已迁移四类 deferred debt、对象通知 debt、Finish debt 和对象退休 debt 的账本、Pending、Reservation、Taken、park/wake、finish/rearm 机械路径。`os/work_debt` 纯逻辑 crate 的算法未改；本轮仅因新增内核请求账本占用第 8 个静态 `TableId`，把动态构造的 ID 起点从 8 调整为 9。现有业务 step、来源依赖注册、预算保留、完成回调和对象责任仍由原模块持有。WaitSet 交错自检仍直接使用底层 `WorkDebts`，这是刻意保留的低层票据窗口测试，不是生产执行路径；删除条件是 P3+P4 退休接口稳定后将该自检迁移到正式账本接缝。

本轮开发检查：`just check`、七面 `just clippy`、`cd os && cargo test -p work_debt --target aarch64-apple-darwin`（18 项）和 `git diff --check` 通过。账本状态转换、真实生产调用点迁移和锁外 payload 析构边界已核对；P1 到此收口。上述结果不构成后续阶段或本专题整体验收。

P2 已完成施工：请求执行责任已从 `WaitContext::finish_step` 移出，新增独立的 `DrainExecutor`/请求债务表；`WaitContext` 仅保留完成仲裁、订阅注销、线程交付及请求取消的窄接缝。`ProcessDrain` 捕获调用者 `Process` 与 `ThreadResultObligation`，请求债务在共享安全点推进，目标 `Process` 的批次许可由请求 owner 释放。取消与请求依赖交错、WaitPlan 安装前丢弃、continuation 请求 owner 断言，以及结果义务/请求槽退款均已接通。当前 `just check`、七面 `just clippy`、os workspace host 回归和 `just virt` core 路线通过；这些是 P2 的开发/机制验证，不是本专题整体验收。上述 P2 记录保留其阶段边界；当前实现状态以本计划后文的 P3+P4 收口记录和 P5 接手交接为准。

P3+P4 已完成实现与阶段验证：`KernelObject` 只提供单步 `advance_waiter`，通知账本统一拥有预算循环；对象来源锁内只推进一次，完成交接和通知槽归还仍在锁外。新增 `RetirementTicket` 持有已提交对象强引用，`DrainExecutor` 与 `PendingClose` 直接使用票据登记/取消/查询退休完成，删除请求层的 `FinishDependency` 与反复 `ObjectRef::retirement()` 查询。`RetirementTarget::finish` 现在返回显式 `RetirementCompletion`，通用 driver 在对象锁外交付回复和 mandatory 生命周期，不再由全局函数混合组织。`MemoryChangeCompletion` 保留自己的 Remote ack、资源退休和结果义务阶段，但最终等待回复也通过同一 `RetirementCompletion` 由 deferred-work driver 锁外交付；它没有被强行改造成 `RetirementTarget`。请求首用/重启政策由 `DrainExecutor` 拥有，WaitContext 只保留窄 restart 校验。WaitSet actor 的静止/最终退休窗口由现有交错自检覆盖，未发现需要继续拆分状态字段的证据。开发与组合验证：`just check`、七面 `just clippy`、os workspace host 回归、完整 `just acceptance` 均通过；后者包含 stress 16/16、release、sifive_u、nofd 和 panic/alloc/fatal boot-failure。release 构建仅保留既有 `runtime_stop::parked_mask` dead-code 警告及链接器 RWX 提示。四类工作同时 pending 的统一公平性仍作为 P6 专题组合门，不提前制造测试服务。

### 结构审计修订与已收口边界（2026-09-16）

P3 与 P4 合并不是为了减少文件数，而是因为它们共享同一责任链：`KernelObject::retirement()` 是对象契约通向 `RetirementTarget` 的唯一接缝，WaitSet 普通 Close、进程退出的 `retire_entry`、内存事务 `MemoryChangeCompletion::advance_retire` 和通知完成都必须共同决定退休 ticket、执行槽、完成回复与最后 owner 的交接。单独先做对象拆分会留下没有 owner 的退休执行责任，单独先做退休执行又会继续依赖当前混合的对象接口。

合并后的 P3+P4 已覆盖以下真实消费者，而不是只整理 `object.rs`：

- `KernelObject`、`ObjectWaitState`、`ObserverSink`/`FinishClass` 的职责边界与 waiter drain 接缝；
- `RetirementTarget`、`WaitSet`、普通 `HandleClose` 和进程退出的对象退休路径；
- `MemoryChangeCompletion::advance_retire`、`UnpublishedBound::rollback` 及其结果义务/完成通知交接；
- 通知、Finish、退休三类账本在共享安全点中的公平预算与锁外 payload 交付。

P2 留下的可复用完成上下文已经收窄：`DrainExecutor` 拥有首用/重启政策和 `WaitOperation` 启动/取消责任；`WaitContext::prepare_reusable_wait` 只验证旧轮 Done、finish slot 已归还和 epoch 可递增，再执行窄 restart。普通 WaitMany/Sleep 仍为单次上下文。`MemoryChangeCompletion` 使用一次性 `prepare_kernel`，只共享最终 `RetirementCompletion` 交付，不因表面相似而取得可复用请求接口。

`MemoryChangeCompletion` 是 P3+P4 的第二个真实请求/完成来源。它不能继续作为 P6 的“其他消费者”旁观者，也不能在没有设计闭包的情况下机械泛化成新的通用执行器。该闭包必须在设计阶段明确它与 `DrainExecutor` 的共同部分、不同的进度/结果 owner、取消边界和预算记账；若共同部分不足以形成稳定协议，则保留两个领域执行器，但删除重复的对象退休/完成交接代码。

P5 已完成三条 Process 级驱动路径的仲裁：`ProcessDrain` 请求、Unpublished rollback 和 Finalization debt；Termination debt 作为终止链前段共同纳入。它没有重新打开 `RetirementTarget` 的对象契约，也没有引入自动 reaper、用户 Seal/Drain ABI 或持续轮询。P6 的组合验证仍须明确覆盖四类债务同时就绪时的安全点公平性，确认后进入的请求/通知/退休类别不会因静态预算预留而饿死。

### P6 接手交接

P5 已收口；P6 从当前未提交工作树接手。代码入口仍为 `os/kernel/src/deferred_work.rs`、`os/kernel/src/task/request.rs`、`os/kernel/src/task/proc.rs`、`os/kernel/src/task/process.rs`、`os/kernel/src/task/job.rs` 与 `os/kernel/src/task/handle.rs`。P6 的接手审计发现 Waiting 已发布而请求尚未 start 的真实跨 hart 窗口，以及迟到旧 epoch 取消回调可能触及复用 activation 的问题，因此撤销“只补验证、不再修改结构”的绝对限制；不重开公开 ProcessDrain 语义，也不把领域执行器机械合并。

P6 必须复核并保持：ProcessDrain 的管理者 capability 与公开结果不变；REAPABLE 是开始管理回收的屏障而非自动 reaper 触发；`More` 必须有真实公开工作进度；调用者取消只放弃回复，不撤销已提交的对象/内存退休；PublishDead 前的 Finalization 根不能因 caller/control/Job 成员引用消散而丢失；Unpublished rollback 不能留下 Process 成员、Pool/metadata owner 或未发布强根。

P5 的开发验证和完整 acceptance 已完成；P6 不新增测试服务，统一验证四类债务同时 pending 的公平性、ProcessDrain/Unpublished/Finalization 与对象退休交错、跨 hart/旧 epoch/调用者退出/完整退款，并执行专题最终 `just acceptance`。门铃失败契约保持为 Commit 后 Pending 责任不丢、后续安全点与 idle 双检可见；永久 IPI 失效仍是平台 admission 失败，本专题不新增自动恢复协议。

### P5 规模审计与施工裁决（2026-09-16，已完成）

源码责任链确认 P5 不是三条可独立交付的并列路径。ProcessDrain、Unpublished rollback 和 Finalization 共同写入唯一 `Process::drain_batch`/`DrainState`；Finalization 由该状态机在 AddressSpace 完成后发布，再回到同一状态机推进；Job `CompletionCursor` 也挂在该进程的 finalization 状态中。Termination debt 虽不进入 `drain_batch`，但它逐稳定成员槽推进并发布 REAPABLE，是 Unpublished 与管理者 Drain 开始前的同一终止链前段，必须纳入 P5 的真实消费者与验证面。

因此 P5 保持一个机制闭包，不按债务表拆成独立完成阶段，也不与 P6 合并。P6 保留为旧路径删除、四类债务公平性与跨 hart/退出/退款的专题组合门。P5 内部已按以下顺序完成，步骤本身不作为独立交付：

1. 收拢安全点与单债务 turn 的预算真值，修正 Unpublished 账本容量归属，明确 keyed/unkeyed 依赖接口，并把 Termination debt 登记为本阶段真实路径。
2. 将 REAPABLE 合取收敛为 lifecycle 内部唯一谓词；全部完成路径只返回是否应发布，锁外统一调用 `Process::publish_reapable`。
3. 将 `drain_gate`、批次许可、`DrainState` 零进度原因和三类 driver 的交接收敛为类型化 Process 级接口；保留各债务表独立容量、FIFO 和 owner，不机械合并队列。
4. 补齐 Termination 三态槽、Finalization 与活动管理批次交错、PublishDead 强根交棒、Unpublished rollback 退款及同一 Process 路径互斥的机制验证；P6 再执行跨类别组合门。

当前闭包内代码债务及删除门：

| 债务 | 本阶段处理 |
|---|---|
| REAPABLE 合取散落于 lifecycle 多个写路径 | 改为 lifecycle 内部唯一谓词，所有写路径复用；`Process::publish_reapable` 保持唯一锁外发布入口 |
| Unpublished 账本借用 `MEMORY_CHANGE_GLOBAL_LIMIT` | 当前唯一生产者是 boot `spawn_from_elf`，改为有独立依据的 boot-only 单槽容量；未来通用 launcher 若进入内核须重新按真实并发来源立案，不预付 4096 个静态槽 |
| 安全点 16 步、单债务 4 步在 deferred/notify/request/retirement 重复 | 移入 `work_ledger` 作为单一机械预算真值，各领域只保留类别预留顺序 |
| `Dependency::cancel` 对 unkeyed waiter 静默无效 | 接口显式命名为 keyed 取消；unkeyed 依赖只由生产者 `notify`，不得假装可按 epoch 取消 |
| `drain_dependency().expect(...)` 用动态不变量表达唯一零进度原因 | `drain_batch` 返回类型化 Blocked/More/Complete，driver 不再回查 `PendingClose` 猜测阻塞来源 |
| `drain_active` 与 `drain_gate` 的访问分散于 request/finalization | 收入 Process 级批次许可和推进接口，driver 不直接读写仲裁字段 |
| Termination debt 未列入 P5 三路径描述 | 纳入同一终止责任链和本阶段机制测试，不另建 JobDrain 或自动 reaper |
| Unpublished rollback 入口不得已进入 Finalization；PublishDead 后允许两张账本短暂双根 | rollback 入口保留结构断言；P6 以同一 `drain_gate` 的串行交棒、强根释放和退款自检覆盖合法重叠 |

### P5 收口记录（当前工作树）

本轮已完成以下 P5 内部迁移，未改变 shared/kernel 公开 ABI：

- `work_ledger` 统一安全点总预算与单债务 turn；deferred、notify、request、retirement 不再各自保存同一数值。
- Unpublished 账本改为独立 boot-only 单槽；当前唯一生产入口是 `spawn_from_elf`，未来正式 launcher 并发接入前须重新按真实来源建立 admission。
- `LifecycleInner::is_reapable` 成为所有完成路径共用的合取谓词；`Process::publish_reapable` 继续是唯一锁外发布入口。
- `DrainBatchOutcome::{More, Blocked, Complete}` 成为 `Process::drain_batch` 的类型化结果；ProcessDrain、Unpublished 和 Finalization 不再通过 `PendingClose` 动态回查猜测零进度原因。
- 批次许可的获取、释放和活动快照集中到 `Process` 方法；底层原子仍保留为锁外依赖谓词，避免在 Dependency 锁内回取 `drain_state` 造成锁序反转。
- keyed/unkeyed 唤醒接口已分名；Process 级不同来源的 blocked 交接复用同一个私有 `park_process_debt` 接缝。
- Unpublished rollback 入口增加未进入 Finalization 的结构断言；推进到 PublishDead 后允许与 Finalization 强根短暂重叠，Termination debt 继续作为同一进程终止责任链前段纳入 P5。

当前验证已通过：`just check`、七面 `just clippy`、os workspace host 回归、`just virt` core、`just virt-release` 和完整 `just acceptance`。完整 acceptance 覆盖 stress 16/16、release、sifive_u、virt-nofd 以及 panic/alloc/fatal boot-failure；启动自检覆盖 Unpublished 类型化 `Blocked(RetirementTicket)`、boot-only 单槽容量、ProcessDrain 取消/重启/退款和真实监督收束。P5 已收口；P6 仍负责四类债务同时 pending 的公平性、跨 hart/旧 epoch/调用者退出、旧路径残留审计和专题组合完成门。

### P6 规模审计与施工裁决（2026-09-18）

P6 保持一个专题完成门，内部按依赖分为四个施工阶段：请求启动/取消/代次静止 → Process/对象/内存退休 owner 交接 → 两组四类债务公平性 → 真实运行期组合与最终验收。前一阶段是后一阶段成立的正确性前提，不能拆成独立交付。

本轮结构裁决：

- Waiting Done 与 executor 静止是两层状态；允许等待先完成，但复用必须等请求状态、依赖和预付槽全部回到 idle。`start/cancel` 以捕获 `WaitKey` 仲裁，取消先赢后的迟到 start 合法，旧轮延迟回调不能触及新轮。
- Unpublished 到 PublishDead 时允许与 Finalization 短暂双根；`drain_gate` 只在 Process 内部按 managed/unpublished/finalization 类型化接口串行游标，活动管理批次令 Finalization park，许可释放负责唤醒。
- 公平性分为 deferred 与 control 两组，每组四类；承诺单位是入口 runnable 类别至少取得一次执行机会。阻塞登记计执行成本，不增加公开 Drain `work_done`。
- 门铃失败只承诺 Pending 不丢。测试以无门铃的 quiet publication、Pending 电平、park/wake 和 idle 双检证明责任保存，不伪造永久平台故障后的自动进展。

已到删除条件的债务同步清理：WaitSet 票据夹具迁入正式 `DebtLedger`；删除 `PendingClose::dependency` 动态查询、Process 重复 `drain_waiter` 根、MemoryChange 空 `FinishWaiter` 阶段和 notify 预算别名；`drain_gate`/`drain_active` 不再由生产 driver 直接访问。静态表 1–9 后，运行时表 ID 起点相应移到 10。

### P6 完成清单

- [x] 四类 Process 级债务（MemoryChange、Unpublished、Termination、Finalization）同时 pending 时，每类在单安全点预算内均能取得进展；静态预留不造成饥饿。
- [x] `ProcessDrain` 与 Unpublished/Finalization、对象退休、通知/Finish debt 的交错覆盖正常、阻塞、唤醒、取消和完整退款。
- [x] 跨 hart、旧 epoch/handle、调用者退出、管理者接管和门铃失败后的 Pending 保留均有可定位证据。
- [x] 扫描旧 `FinishDependency`、动态 `drain_dependency`、重复预算真值、直接访问 `drain_active`、无删除条件的 adapter/测试专用运行体；到期残留已删除。
- [x] 运行专题最终验证（受影响 host、`just check`、`just clippy`、必要的 core/release/platform/boot-failure），记录日志与退出原因。
- [x] 完成结构 Review，确认 owner、锁序、失败/退出/退款和文档一致后，归档本计划并解除 FAL 后端前置。

### P6 收口记录（2026-09-18）

请求启动/取消竞态已按捕获代次闭合：Waiting 发布后取消可以先赢，迟到 start 不再 panic；启动先赢的 debt 继续观察取消，旧轮已进入的延迟回调也不能取走新轮 activation。确定性夹具覆盖 cancel-before-start、旧回调跨 restart、parked kill、未安装丢弃、坏输出和预付槽退款。

Process 退休交棒已收口：`drain_gate` 只由 Process 内部接口持有；Finalization 在活动管理批次期间 park，释放许可后醒来；Unpublished 走真实 `UnpublishedBound::rollback`，到 PublishDead 后允许与 Finalization 短暂双根并最终释放最后一个 Process 根。WaitSet 票据自检已迁入正式账本，旧动态 dependency、重复 waiter 根和空完成阶段已删除。

`work_debt::FairBudget` 承担两组四类的共同预算机械协议，16/4 数值仍只在内核 `work_ledger` 定义。host 测试证明四类同时入口 pending 的最坏分配 `[13,1,1,1]`、缺席类别不占预留和持续 backlog；启动自检以真实 MemoryChange/Unpublished/Termination/Finalization 与 Notification/Finish/Request/Retirement payload 同时排队，分别观察内存预算余量、rollback 游标、termination Finish 发布、Finalization park，以及 Request 结果和 Retirement 取消进度。quiet publication、Pending 电平与 idle 双检覆盖门铃缺失时责任不丢。

用户态组合不再把线程已创建当成请求已提交：同 authority 探针观察 `ObjectBusy`，并以独立完成通知确认目标 Drain syscall 尚未返回后终止 caller；精确 active/parked 取消由内核确定性夹具负责，真实多 hart workload 负责证明 caller 退出、已提交退休继续、原管理者接管与 Pool 全退款。旧 handle、旧 epoch、StoreAccess、`max_work=1`、16/16 竞态矩阵和最终监督保持通过。

最终结构 Review 未发现实现级阻塞问题；首次指出的两个证据缺口已关闭：用户态日志改称并验证“在途 Drain 周期”而不把 `ObjectBusy` 单独等同于 active permit；deferred 四类为 MemoryChange 与 Termination 增加独立可定位的进展断言。

最终验证：

- `cd os && cargo test --workspace --exclude erhino_kernel --target aarch64-apple-darwin`：通过，日志 `artifacts/check/public-operation-p6-os-host.log`。
- `cd shared && cargo test --workspace --target aarch64-apple-darwin`：通过，日志 `artifacts/check/public-operation-p6-shared-host.log`。
- `just check`：通过。
- `just clippy`：shared-host、os-host、kernel-target、user-host、user-target、user-stress、user-fp 七面通过，日志位于 `artifacts/lint/`。
- `just acceptance`：最终源码快照通过 debug stress（含 16/16 竞态和 `max_work=1`）、release core、`sifive_u` core、`virt-nofd` core，以及 panic/alloc/fatal 三种 Ready 前 boot-failure；各路线均以预期 reset/收割终态退出。

施工中两次预期内失败均已归因并修复：`artifacts/failed-acceptance-20260918-111949-70484.log` 暴露 finalization 夹具扫描上限不足；`artifacts/failed-acceptance-20260918-113130-75136.log` 暴露 ownership 探针抢先取得批次。`artifacts/failed-acceptance-20260918-114420-87143.log` 暴露空 HandleTable 无游标进度观测，加入真实 owner entry 后关闭。它们不是最终验收证据。

### 不纳入本专题

- 公开等待/过程 ABI、ProcessDrain 预算语义、REAPABLE/CLOSED 条件和用户态监督职责。
- `os/work_debt` 纯逻辑 crate 的包归属与算法另由共享包计划拥有；本专题只调整内核侧使用与责任边界。
- Runnel 观察登记/取消、Delivery/Peek、RPC/Outbox、Runtime 任务协议及服务架构。
- 用户态 mapping/堆/线程栈 owner、KernelMemoryBudget、CPU 预约、设备/DMA、系统关机编排和根监督者故障恢复。
- Job 的公开面和 `CompletionCursor` 算法本身；只处理其与 Process/退休执行的连接点。

## 收口后的结构边界

| 当前实现 | 保留边界 |
|---|---|
| `task/wait.rs` 的 `WaitContext` 负责完成仲裁、订阅注销、线程交付和窄取消/restart 接缝；`DrainExecutor` 持有请求状态 | 等待层不执行具体 `DrainRequest`；请求预算、目标、结果义务和批次许可仍由请求执行者拥有 |
| `task/object.rs` 的 `KernelObject` 保留身份、类型、权限和对象定义的观察/关闭能力；通知执行通过 `advance_waiter` 单步接缝 | 来源锁仍持统一电平/历史真值；通知账本负责预算，不向对象锁内注入跨对象工作 |
| `task/retirement.rs` 的 `RetirementTarget` 与 `RetirementTicket`/`RetirementCompletion` 分层 | 退休后端拥有业务状态和推进算法；driver 拥有容量/调度，完成回复和 mandatory 生命周期在锁外交付 |
| `deferred_work.rs`、`task/notify_work.rs`、`task/retirement.rs`、`task/request.rs` 各保留独立 WorkDebt 表 | 只共享账本机械协议；payload、容量分区、来源依赖、预算顺序和领域执行器不机械合并 |
| `task/proc.rs` 的 MemoryChange、Unpublished、Finalization 与 ProcessDrain 仍共享部分 `drain_batch` 资源路径 | P5 已收口 Process 级 driver 仲裁；P6 只验证交错、公平性和残留路径，不恢复用户 Seal/Drain 或自动 reaper |

当前目标资源的 PendingClose::Entry 可以等下一批管理者推进；已启动对象退休 ticket 则由内核继续。PublishDead、祖先传播与批次 Done 也可处于不同预算段。重构必须保持这些边界清楚，不把它们视为需要恢复用户 Seal/Drain 的理由，也不擅自改变 CLOSED 或 ProcessDrain 完成语义。

## 自然顺序与提前触发

推荐串行位置：

```text
运输 owner/Runnel 拆为消息运输 → 流运输/Runnel → 通用执行/准入 → 完整 RPC/Outbox（执行前置）
  → 共用包契约与归属（已有计划）
  → 内核等待/请求/退休结构收束（本计划）
  → FAL 后端/授权与业务（总计划）
```

当前公共内核已存在完整的操作/回收路径；结构耦合不自动等于运输不能开工。让实际用户态消费者先完成，可以校验真实使用边界，避免为尚未接通的框架重写内核。

提前触发仅限具体证据：执行前置实际需要新请求/退休类型，或无法在现有 Close 完成与执行契约下形成正确的操作闭包。发现时明确缺少的能力、owner、取消与验证，提升对应完整机制为真实前置；不把整个内核重构、根监督者重建设计或 KernelMemoryBudget 一并提升。

共享包搬迁与本项没有回收算法依赖，默认先完成包迁移再改内部结构以分离验证范围；若证据要求重排，同步 COMPASS 和唯一计划。

## 实施顺序与完成门

本计划执行前遵循 `AGENTS.md`「标准施工流程」；以下只记录本专题的审计对象、设计重点和完成门，不重复定义通用流程。

本节的实施步骤对应上方 P0–P6。P0–P5 是注意力集中的内聚施工阶段，不是独立交付；阶段内允许编译、类型检查和局部 host 回归，禁止据此更新专题完成状态或执行整体验收。只有 P6 同时完成全部真实消费者迁移、失败/退出/退款路径、旧机制删除及组合验证，才形成唯一完成门。

1. 从实际调用者建立来源观察、线程返回、有界执行三者的类型/所有权/锁序图；区分单批 DrainRequest 和完整 Process 回收状态。现有经过验证的批次契约、REAPABLE 与用户监督分工作为设计输入，不因本项重开自动回收选择。
2. 明确公共执行协议与各类预付容量、停驻/唤醒、取消回复和必成责任；涉及硬件/ABI 时先从 `references/CONTRACTS.md` 取证，引用外部实现时从系统索引选择官方证据。
3. 内核真实消费者共同迁移；正常 Close、ProcessDrain、内存 completion、unpublished 与 finalization 保持同一算法与正确的驱动层次。公开语义若确需改变，先论证并确认具体方案，再共同迁移 shared/rinlib/监督者。
4. 删除具体请求侵入等待层、重复执行编排和不再成立的状态/trait 分支；不保留没有删除条件的 adapter。
5. 验证错误顺序、旧 handle/epoch、调用线程取消、管理者接管、pending 对象退休与完整退款；完成身份不跨越 Thread DONE、结果记录和地址翻译同步屏障。执行相应 host、just check、just clippy、core/release/platform 与退出组合；历史 stress 墙钟敏感现场见 `plans/archived/ref-2026-09-acceptance-timing-flake.md`，新失败按归档触发条件重新立案。

完成需要全部真实消费者、失败/退出、旧机制删除和可定位验证；本次设计审视不是安全性证明。最终契约进 ideas，实现事实进 impls，完成后归档本计划；提交与合并另需明确授权。

## 其他事项的唯一归属

- Runnel 终态访问、完整观察/取消 owner、RPC/Runtime 组合、领域配额分类、可挂起 Close 的实际执行上下文、持续监督消费 REAPABLE：执行前置，不等待本项整体完成。
- Delivery 独立身份与 Peek：运输闭包先审视，保留交付责任，不预设删除 ABI。
- TimerQueue token 编码、OrderedTable 预付契约和包位置：已有包归属计划。
- 普通 mapping/堆/线程栈 owning 接口：[独立延期计划](../todo-2026-09-14-user-memory-owner-lifecycle.md)，不延期当前运输清理。
- 不可信分配域 metadata 隔离：[KernelMemoryBudget](../todo-2026-09-14-kernel-memory-budget.md)，不用来激励 pm Drain。
- 根监督者意外退出后没有专用自动 Drain 的事实仍保留。只有目标需要根故障恢复时才需设计相应接管/重建能力；正常 reset 可由仍存活的 init 提交。根终局与通用关机边界由 [系统关机编排计划](../todo-2026-09-system-shutdown-orchestration.md) 在承诺正式服务与终局政策前界定，不作为当前运输开工的假定缺口。
