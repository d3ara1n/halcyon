# 公共操作所有权与内核执行结构收束

> 状态：独立待实施的结构收束，不是运输/服务执行前置的整体开工阻塞。执行本专题前先按 `AGENTS.md`「标准施工流程」完成接手、规模审计、拆分/合并和设计闭包。审视基线为 `master@5d406a4...task/fal-service-capabilities@bf48cab`。ProcessDrain 的职责与 REAPABLE 触发已澄清，保留管理者有界推进；不以全体进程持续轮询、缺乏预算激励或必须自动回收作为改造依据。
>
> 当前下一任务是 [运输/服务执行前置](todo-2026-09-13-service-runtime-prerequisites.md)。本文件只拥有内核等待、请求和退休执行的结构收束；共享算法/目录已由 [已归档包归属计划](archived/todo-2026-09-13-workspace-package-ownership.md) 完成，FAL 领域与业务由 [总计划](todo-2026-09-fal-service-capabilities.md) 拥有。不重复安排同一问题。

## 已明确的能力与职责

- ProcessDrain 是当前唯一公开的内核资源回收步进 syscall，要求目标 ProcessControl 的 MANAGE；由受信任管理者负责，普通应用不用在运行循环中替内核轮询清理。是否只有 pm/init 具有管理权取决于实际 capability 交付，内核不按服务名或 PID 授权。
- REAPABLE 在目标线程、active hart、Building 操作与 mandatory 操作通过屏障后持续成立；管理者据此开始回收，More 表示仍有工作。`libprocess::collect_process` 已实现等待 → Drain → Query → Close；当前 init/pm 在集成剧本中调用，常驻事件驱动监督不视为已交付。
- Job 完成要求 sealed 且成员/child 表为空，大量进程资源已由各自 ProcessDrain 收束；自身 CLOSED 和祖先摘除复用有界 CompletionCursor，不新增 JobDrain。
- WaitSet 普通 Close、内存/Tunnel 提交后的同步与退休、通知完成及未发布回滚由内核闭合；内部 drain_current 不是用户 ABI。管理者的域级回收与内核必成尾段可以一致分层，共用算法，不要求所有层都由同一主体驱动。
- 回收是管理服务职责，不需要先建立预算激励。页/metadata 预算仍用于分配和隔离，独立触发，不成为本项的动机前置。
- 用户可以跳过 rinlib、乱序、遗漏后续调用或退出；内核必须保持合法状态、稳定 owner 和资源守恒。持有合法资源、业务不进展、可接管回收和责任丢失是不同结论，不能混称内核状态损坏。

## 仍成立的结构收束项

| 现状与位置 | 目标与边界 |
|---|---|
| `task/wait.rs` 的 WaitContext 持有 DrainRequest，finish_step 执行具体进程请求；ObserverSink 还承担只适用于请求的依赖 | 等待层负责完成仲裁及线程返回；具体请求拥有输入与执行状态，通过明确的内部执行协议连接。保留合法批次语义，不以改名或另一层转发代替责任分离 |
| `task/object.rs` 的 KernelObject 同时包含身份、观察、重臂/取消、通知 drain 和退休能力 | 分清必需对象契约与可组合能力，减少每个对象必须了解的通知/执行知识；保留统一电平真值与来源锁序 |
| `task/retirement.rs` 的 RetirementTarget 参与 work slot 归还和回复组织 | 执行器持有可独立维护的调度/容量交接知识，对象保留自己的退休状态与终点 |
| `deferred_work.rs`、`task/notify_work.rs`、`task/retirement.rs` 共用 WorkDebts，仍各自组织 Pending、预算、park/wake 与收尾 | 共用有依据的执行协议，保留不同工作类别的准入隔离、唤醒来源与具体算法；不机械合并所有队列 |
| `task/{proc,process,handle,lifecycle,thread}.rs`、内存完成、未发布回滚与 Job finalization 都消费等待/执行机制 | 改造包含全部真实消费者、失败/退出及最终退款；不能只改 WaitSet 后宣称通用执行已收口 |

当前目标资源的 PendingClose::Entry 可以等下一批管理者推进；已启动对象退休 ticket 则由内核继续。PublishDead、祖先传播与批次 Done 也可处于不同预算段。重构必须保持这些边界清楚，不把它们视为需要恢复用户 Seal/Drain 的理由，也不擅自改变 CLOSED 或 ProcessDrain 完成语义。

## 自然顺序与提前触发

推荐串行位置：

```text
运输 owner/Runnel → 通用执行/准入 → 完整 RPC/Outbox（执行前置）
  → 共用包契约与归属（已有计划）
  → 内核等待/请求/退休结构收束（本计划）
  → FAL 后端/授权与业务（总计划）
```

当前公共内核已存在完整的操作/回收路径；结构耦合不自动等于运输不能开工。让实际用户态消费者先完成，可以校验真实使用边界，避免为尚未接通的框架重写内核。

提前触发仅限具体证据：执行前置实际需要新请求/退休类型，或无法在现有 Close 完成与执行契约下形成正确的操作闭包。发现时明确缺少的能力、owner、取消与验证，提升对应完整机制为真实前置；不把整个内核重构、根监督者重建设计或 KernelMemoryBudget 一并提升。

共享包搬迁与本项没有回收算法依赖，默认先完成包迁移再改内部结构以分离验证范围；若证据要求重排，同步 COMPASS 和唯一计划。

## 实施顺序与完成门

本计划执行前遵循 `AGENTS.md`「标准施工流程」；以下只记录本专题的审计对象、设计重点和完成门，不重复定义通用流程。

1. 从实际调用者建立来源观察、线程返回、有界执行三者的类型/所有权/锁序图；区分单批 DrainRequest 和完整 Process 回收状态。现有经过验证的批次契约、REAPABLE 与用户监督分工作为设计输入，不因本项重开自动回收选择。
2. 明确公共执行协议与各类预付容量、停驻/唤醒、取消回复和必成责任；涉及硬件/ABI 时先从 `references/CONTRACTS.md` 取证，引用外部实现时从系统索引选择官方证据。
3. 内核真实消费者共同迁移；正常 Close、ProcessDrain、内存 completion、unpublished 与 finalization 保持同一算法与正确的驱动层次。公开语义若确需改变，先论证并确认具体方案，再共同迁移 shared/rinlib/监督者。
4. 删除具体请求侵入等待层、重复执行编排和不再成立的状态/trait 分支；不保留没有删除条件的 adapter。
5. 验证错误顺序、旧 handle/epoch、调用线程取消、管理者接管、pending 对象退休与完整退款；完成身份不跨越 Thread DONE、结果记录和地址翻译同步屏障。执行相应 host、just check、just clippy、core/release/platform 与退出组合；stress 已知问题仍由验收可靠性计划拥有。

完成需要全部真实消费者、失败/退出、旧机制删除和可定位验证；本次设计审视不是安全性证明。最终契约进 ideas，实现事实进 impls，完成后归档本计划；提交与合并另需明确授权。

## 其他事项的唯一归属

- Runnel 终态访问、完整观察/取消 owner、RPC/Runtime 组合、领域配额分类、可挂起 Close 的实际执行上下文、持续监督消费 REAPABLE：执行前置，不等待本项整体完成。
- Delivery 独立身份与 Peek：运输闭包先审视，保留交付责任，不预设删除 ABI。
- TimerQueue token 编码、OrderedTable 预付契约和包位置：已有包归属计划。
- 普通 mapping/堆/线程栈 owning 接口：[独立延期计划](todo-2026-09-14-user-memory-owner-lifecycle.md)，不延期当前运输清理。
- 不可信分配域 metadata 隔离：[KernelMemoryBudget](todo-2026-09-14-kernel-memory-budget.md)，不用来激励 pm Drain。
- 根监督者意外退出后没有专用自动 Drain 的事实仍保留。只有目标需要根故障恢复时才需设计相应接管/重建能力；正常 reset 可由仍存活的 init 提交。根终局与通用关机边界由 [系统关机编排计划](todo-2026-09-system-shutdown-orchestration.md) 在承诺正式服务与终局政策前界定，不作为当前运输开工的假定缺口。
