# 批次 C-1：线程生命周期、持久 init/pm 监督与调度域 Review

> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](todo-2026-09-review-program.md) 为准；正文建议保留首审语境，不作为现行实施顺序。

## 审查范围、基线与方法

目标提交：`d741880`、`bdc83ef`、`004cae5`、`fcbd5b6`、`b161163`、`1d7dc92`。工作树在审查时为 `f9b3bda` 之后的文档整理状态；代码结论均来自 `git show <commit>:<path>` 或该提交隔离快照，不将当前工作树后续内容替代历史提交证据。全程只读，无代码修改、无提交。

审查覆盖线程/进程成员表、teardown barrier、ThreadDeparture、ThreadControl、ThreadSpawn/join、末线程终局、持久 init/pm 监督和 JobControl authority、sched_domain eligibility/D64、域内 idle/IPI，以及锁序、固定容量、失败原子性、用户 fault 不 panic 和 debug/release 差异。

## 执行命令与验证

```text
git status --short
git show --no-patch --format='%H %s' d741880 bdc83ef 004cae5 b161163 1d7dc92
git show <commit>:<file> | nl -ba
git diff <commit>^ <commit> -- <paths>
git grep -n -E '...' <commit> -- <paths>
git archive 1d7dc92 | tar -x -C /tmp/halcyon-c1-*
cd /tmp/halcyon-c1-*/os && cargo test -p sched_domain --target aarch64-apple-darwin

git archive 004cae5 | tar -x -C /tmp/halcyon-c1-*
cd /tmp/halcyon-c1-*/os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p stack_layout --target aarch64-apple-darwin
```

`sched_domain` 7 项 host 测试通过；`004cae5` 隔离快照相关 host 测试通过。未运行目标提交的 `just check`、`just virt*`、`just acceptance`、`sifive_u`、release/hetero/nofd QEMU；host 测试不能覆盖多 hart active/epoch/IPI/satp 联合时序、用户态 supervisor 失败剧本或稀疏 raw hartid。

## 逐提交结论

### `d741880`

成员表从单一 ThreadRecord 扩展为按 tid 有序成员表，加入 Staging/Ready/Running/Waiting/Exiting、ThreadDeparture、TerminationTodo、非 Resume 汇编出口的 `KERNEL_SATP`/本地同步、deliver_output 复检和 TunnelAttach close 重排。终止首达原因冻结，Waiting 取消、Staging 摘取、IPI 与 REAPABLE 发布在锁外推进；stale Waiting 由 pick gate/reap 吸收。正式 ThreadControl/result 语义由后续 `bdc83ef` 补全。

### `bdc83ef`

Running ThreadSpawn 使用 `Spawning → Ready`，失败走 rollback_spawn；ThreadDeparture 的 result obligation 延迟 ThreadControl DONE，末线程触发进程终止。Join/Drop 复用 ThreadControl + WaitMany + 用户态 packet，方向上形成单一语义入口。

### `004cae5`

补齐 spawn/kill、末线程 exit/kill、join/Drop、1024 成员容量和多平台 16/16 压力；execution sequence/epoch 复检和 Complete → result obligation → DONE 顺序成立。未发现本范围新增可直接证明的 P0/P1 线程竞态，但目标快照未由本次独立 QEMU 复放证明。

### `fcbd5b6`、`b161163`

`fcbd5b6` 建立 `root → services → pm_domain/acceptance`，pm 获得不含 CREATE 的 MANAGE|READ|WAIT 委托 JobControl，init 保留直接收束副本。pm 采用枚举→派生→kill→REAPABLE→drain→seal。`b161163` 主要调整调试打印和运行窗口，不改变监督机制。监督失败路径存在 F1–F3、F8。

### `1d7dc92`

`sched_domain` 以需求签名等价类划分域，D64 采用 FLEN 恰 64，Q→128；ProcessStart/bootstrap 在 runnable 提交前绑定域，enqueue/pick/idle/SSIP 按域路由。raw hartid 通过 slot 与 SBI 边界转换存在 F5；q-only 病态 capability 输入存在 F6。

## Findings

### P1-C1-01：监督 Drain/Query 失败后丢弃 control，服务可能永久未收束

位置：`fcbd5b6:user/systems/init/src/main.rs:580-599`（目标快照；当前主线同一逻辑位于 `user/services/srv_init/src/main.rs:958-975`）。

可达前提：服务达到 REAPABLE/CLOSED 后，`drain_to_completion(control)` 或 `query(control)` 返回错误，例如 ObjectBusy、输出路径 fault、阶段性错误或管理线程异常。

直接证据：错误分支只记录 `supervision degraded`，随后无条件 close(control) 并从 `supervised` 移除。关闭 ProcessControl 只消散 authority，不终止目标；Job 成员仍可能持有目标，AddressSpace/HandleTable 也可能未 drain 完成。

违反契约：`notes/ideas/task.md` 的 REAPABLE→有界 Drain→Complete/Dead 闭包；`notes/impls/startup.md` 的 init 监督要求。错误分支不能以日志替代资源屏障。

后果：init 丢失最后 control，服务可能残留在 services Job；后续只能依赖枚举/派生，若该路径不可达则形成无监督残留。

建议：错误时保留 supervision entry/control，区分 Busy、暂时失败和终态不一致并退避重试；超过预算进入明确 failure policy，由仍存活 manager 接管，确认 Drain Complete 和稳定 Query 后才 close/remove。

### P1-C1-02：必选服务启动失败被逐项吞掉，部分拓扑继续运行

位置：`fcbd5b6:user/systems/init/src/main.rs:219-263`；当前主线相应逻辑 `user/services/srv_init/src/main.rs:298-308`、`:250-253`。

可达前提：非 pm 服务的 spawn/map/write/start/eligibility 失败，或 pm_domain 第二靶进程启动失败；失败可由 OOM、不兼容执行需求、权限/容量错误触发。

直接证据：tar walker 对条目 spawn 失败只 debug 后继续；只有 pm 未启动才返回 stage error。pm 成功而 fs/driver/target 缺失时，`run()` 继续 IPC/监督剧本，预期服务集合没有缺失登记。

违反契约：bootstrap/init 必须按政策处理完整授权拓扑；实现文档要求全部声明服务进入 services 域并被监督；部分失败不得伪装成正常拓扑。

后果：验收可能在缺少服务条件下继续，pm 可能等待永远不会发生的协议事件，init 监督只覆盖成功子集，形成错误成功或无限等待。

建议：定义必选服务集合；任何必选失败立即进入统一 stage failure，关闭临时 authority、收束 services Job 并进入显式失败终局。非必选项也必须有显式 degraded policy，不以日志静默吞掉。

### P1-C1-03：此 finding 已由后续 `98d2449` 修复，不作为当前工作树债务

历史目标位置：`fcbd5b6:user/systems/init/src/main.rs:303-315`。目标提交的 `steady_state()` 只等待一次，成功或错误后返回。

违反契约：持久 root supervisor 不应因一次消息或 endpoint 错误自然返回。

后续证据：`98d2449` 已将当前主线 `steady_state()` 改为永久 loop（`user/services/srv_init/src/main.rs:437-455`），并由显式 SystemReset 失败路径进入该稳态。因此保留该条作为历史 Review finding 和修复证据，不要求当前代码另行行动。

### P1-C1-04：此 finding 已由后续 `98d2449` 修复，不作为当前工作树债务

历史目标位置：`1d7dc92:os/kernel/src/sched.rs:548-568` 及 `fcbd5b6` 稳态路径。目标提交仍由 `is_quiescent()`/`sbi::shutdown()` 隐式表达整机终局。

后续证据：`98d2449` 删除 `sbi::shutdown`、`is_quiescent` 和 idle 内停机分支，加入 capability 授权 `SystemReset`，init 在终态事实成立后显式提交 reset，平台拒绝后进入永久 supervisor。该历史缺口已闭合，不重复列为当前债务。

### P1-C1-05（历史旧位置已修复，当前 admission 仍需 E-1 复核）：稀疏 raw hartid 的 IPI 编码边界不闭合

位置：`1d7dc92:os/kernel/src/registry.rs:212-220`；`os/kernel/src/sbi.rs:205-208`。

可达前提：平台 DT 提供 raw hartid 稀疏且存在不适合 base=0 单 bit 表达的 ID，终止屏障、域唤醒或 Remote Call 需要触达该 hart。代码只限制 admitted slot 数量，没有对 raw ID 的 SBI mask 可表达范围建模。

直接证据（目标提交）：目标代码将 `1u64 << raw` 作为 mask 并以 base=0 发送；内部 slot 与 raw hartid 明确分离。当前 HEAD 的 `registry.rs:241` 已改为 `send_ipi(1, raw)`，因此旧 shift 位置已修复；当前重复 raw admission、HSM failure gate 和 slot order 的剩余问题由 E-1 报告 M3-2/M3-3/M3-4 承接，不在本条重复。

违反契约：`notes/impls/execution-context.md` 的 raw hartid 可稀疏、slot 仅内部身份；SBI mask/base 契约要求显式表达 raw hart 范围；IPI 错误应可诊断处理。

后果：远端 Running hart 无法被 teardown barrier/唤醒触达，可能永久不达 REAPABLE；debug 可能直接 panic。

建议：按连续 raw hart 段将 slot 集合分组，发送 `(mask, hart_mask_base)`；或 admission 阶段拒绝无法表达的 raw ID，建立明确可证明上界。SBI 非成功返回不能用 `require` 升级为内核 panic。

### P2-C1-06：q-only DT capability 未按 `q ⇒ d ⇒ f` fail closed

位置：`1d7dc92:os/kernel/src/board.rs:146-155`、`os/sched_domain/src/lib.rs:30-43`。

可达前提：病态 DT 声明 `q=true,d=false`。

直接证据：board parser 只断言 `d && !f`，没有断言 `q && !d`；`flen()` 对 q 返回 128，当前 D64 仍保守排除，但不一致的 CPU capability 被纳入 Base domain。

后果：掩盖平台契约错误，未来 capability 集合扩展后可能错误 eligibility。

建议：board admission 显式拒绝 `q && !d`、`d && !f`，补 q-only/d-only/malformed host tests。属于 P2 平台输入硬化。

### P2-C1-07（当前仍存在）：ThreadControl allowed_signals 暴露 CLOSED，但目标代码只发布 DONE

位置：目标 `bdc83ef:os/kernel/src/task/thread.rs:203-229`，`publish_done` 在 `:54-65`；当前主线仍为 `os/kernel/src/task/thread.rs:208`。

可达前提：合法 ThreadControl 持 WAIT rights 的调用者等待 `ObjectSignals::CLOSED`。

直接证据：`allowed_signals` 返回 `DONE | CLOSED`，但唯一终态发布只置 DONE；ThreadControl 没有 CLOSED 发布路径。Handle close 也不改变对象状态。

违反契约：允许等待的终态信号必须可达；Thread join 合同应使用 DONE，不应暴露永不发生的 CLOSED。

后果：合法等待可永久阻塞，signal ABI 的 allowed/observed 集合不闭合。

建议：若唯一终态是 DONE，移除 ThreadControl 的 CLOSED allowed signal 并补 ABI/host test；若需要 CLOSED，定义独立关闭状态及 DONE/CLOSED 顺序。

### P2-C1-08（当前仍存在）：pm/init 关键监督等待无期限，无失败升级政策

位置：`fcbd5b6:user/systems/pm/src/main.rs:188-197`、`user/frameworks/libprocess/src/lib.rs:133-140,158-161`、init `:562-600`；当前主线 `user/services/srv_init/src/main.rs:951` 使用 `WAIT_TIMEOUT_INFINITE`。

可达前提：受管成员停止产生终止进展、IPI/Drain 失效、协议失联或服务永不退出。

直接证据：pm kill 后无限等待 REAPABLE/CLOSED；公共 job_kill 等待 Job CLOSED 也无期限；init WaitMany 对服务 control 使用无限期限。没有 deadline、重试预算、超时升级或 manager 接管分支。

违反契约：失败/拒绝/超时必须可收束；用户态无限 Wait 不能替代内核 teardown 完成证明。

后果：单个坏服务可永久卡住 pm/init，系统无可诊断的失败节点。

建议：每个监督阶段使用 policy deadline/重试预算；超时后重新枚举/派生，区分 Busy 与移除；超过预算转显式 failed/unmanaged 状态，必要时整树 kill 或由 init 提交 reset。

## 已证实不变量

1. lifecycle 成员表是线程容器唯一真值，Staging/Spawning/Ready/Running/Waiting/Exiting/Gone 入口有明确 owner 交接。
2. 首达终止原因冻结；末线程正常离场与已有 Terminating 状态不互相覆盖。
3. active barrier 在非 Resume 出口完成 KERNEL_SATP/本地同步后清除，冻结 active 快照只减不增。
4. Waiting outcome 由单一仲裁，stale Waiting 最终在 enqueue/pick/reap 吸收。
5. Thread result obligation、ThreadControl DONE、JoinHandle acquire fence 的顺序成立。
6. Process/AddressSpace/HandleTable drain 有 gate、游标和预算，owner 主要在 AddressSpace 锁外析构。
7. pm 委托域不含 CREATE，init 保留直接收束副本；Job/Process 权限来自 capability。
8. 当前需求集下 sched_domain 的等价类、Q→FLEN128、D64 恰 64、最弱兼容域 tie-break 和域绑定前置成立。

## 与 C-2 的去重

- C-2 未报告持久 init/pm 或隐式 shutdown 的同根新 finding；其 F1 RemoteCalls token、F2 epoch arithmetic、F3 UserStack cleanup 与本报告独立。
- `004cae5` 的 ThreadDeparture/result obligation、JoinHandle、AddressSpace 交错仅在本报告和 C-2 各自负责范围内取证，不复制同一 finding；本报告只保留生命周期/调度结论。
- B-2 的 bootstrap `table.commit` 后可失败 Attach/Job/staged reserve 是更早的启动提交事务缺陷，与本报告的服务监督错误处理不同，分别保留。

## 后续行动与复核条件

以下为首审建议与复核条件；当前有效条目分别由 supervision、admission 与 capability 计划实施，历史已修条目只做回归核验：

1. 处理当前仍有效的 P1-C1-01、P1-C1-02、P1-C1-05；
2. 处理 P2-C1-06、P2-C1-07、P2-C1-08，并与 B-1/B-2/C-2 的 arithmetic、Drop 和平台边界 finding 统一修复策略；
3. P1-C1-03、P1-C1-04 仅保留历史目标提交缺口及 `98d2449` 修复证据，不作为当前代码债务；
4. 补多 hart/release/heterogeneous QEMU 和 supervisor 失败剧本；
5. 所有当前 findings 修复并复核后，再将本报告移入 `plans/archived/`。

## 最终判定

**不通过（当前仍有 P1）。** 线程生命周期主体、teardown barrier、ThreadDeparture/result obligation、末线程终局和调度域绑定结构总体成立；但服务监督在错误时丢失 authority、必选服务启动失败静默降级、稀疏 raw hartid 的 SBI IPI 表达未闭合，另有 signal、病态 capability 和无限等待等 P2 缺口。隐式 quiescent shutdown 和一次性 steady_state 属于目标历史缺口，已由后续 `98d2449` 修复，不应重复要求当前代码修复。
