# 线程资源模型事务收束复审报告

> 对象：`c1b2ac2..51f4184`，重点复审 `794a4c0` 的线程资源模型改造、`275e4f1` 的设计入档与 `51f4184` 的首轮修复。状态：**已收口**。本报告补充并取代首轮报告“内核侧未发现正确性缺陷”的最终结论；顶层模型保持不变，事务与文档缺口已在本报告同批改动中修复并验证。

## 结论

`ProcessCreate` 创建无线程的 Building process；组装者以 ProcessMap/Write、ProcessGrant 和 ProcessAttach 填入进程资源；ProcessStart 只执行活体检查、冻结 execution binding、消费 builder，并首次发布全部预育线程。Running 期后续线程由 ThreadSpawn 承接。该模型成立，不需要把 Attach 或 Grant 重新并入 Start，也不需要把 Start 改名为 Publish。

复审发现的问题不在顶层模型，而在独立 syscall 之间的事务边界：Ready 预留可能留下前缀、Building 操作登记依赖手工配平、builder 与 grants 共用含混 pin 协议、Grant 成功后的所有权和失败清理未显式表达，以及 requirement/domain 可观察为部分绑定。上述问题均已通过机制收束，而非调用点补丁修复。

## Findings 与修复

### 1. Ready 批次并非原子预留

Start 逐条 reserve marker；中途失败时只能回滚已返回 token，存在部分预留与错误恢复面的复杂性。

调度类改为 `reserve_ready_batch / commit_ready_batch / rollback_ready_batch`。完整批次在一次目标队列锁内预留，失败不产生任何 marker；Start 提交区只消费已完整预留的批次。

### 2. Building 操作登记依赖分支手工配平

Map/Write/Attach/Grant/Start 分别调用 enter/leave，新增失败分支容易遗漏，进而阻塞终止或错误发布 Running。

引入 `BuildingLease`：构造时登记，Drop 自动配平；Start 成功由 `commit_running` 显式消费登记。所有 Building 外部通道统一使用同一 RAII 准入机制。

### 3. capability pin 混合了消费与转移

旧泛化接口用可选 builder/grants 表达两种事务，允许 builder alias、恢复规则和返回值依赖调用者约定，不能从类型与调用序列证明成功时究竟消费哪些 entry。

HandleTable 拆为 `pin_consume / commit_pinned_consume` 与 `pin_transfer / commit_pinned_transfer`。ProcessStart 只消费 builder；ProcessGrant 把 builder 作为受保护 authority、grants 作为转移集合，在 pin 前拒绝自授予、重复 handle 与 rights 放大，成功后恢复 builder、只摘除 grants。

### 4. Grant 输出与跨调用所有权未闭合

安全封装没有结构性要求 grants/output 等长；Grant 成功后再发生 Attach/Start 失败时，高层错误又无法说明 grants 已归目标，容易被调用方重复关闭或误以为自动退回。

shared 以 `PROCESS_MAX_GRANTS` 提供唯一上界；rinlib 安全 API 强制输入输出等长。`libprocess::SpawnFailure` 以 `GrantOutcome::{Retained, Consumed}` 报告最终所有权，并将清理链错误独立保存为 `cleanup_error`。成功 Grant 是独立完成的所有权转移，不与后续 syscall 建立隐式 escrow。

### 5. execution requirement 与 domain 不是单一真值

requirement 和 domain 分开冻结时可形成部分绑定；Base64 判别值为零，又与“尚未冻结”哨兵重合，使双冻结断言失去结构保证。

Process 现在用单个 `AtomicUsize` 保存 execution binding。非零编码同时包含 requirement 与稳定 domain index，零值只表示未绑定；`SchedDomain` 在 boot 冻结表中取得稳定 index，解码统一经 `domain_by_index`。Start 与 bootstrap 均一次冻结完整绑定。

### 6. 线程出生存在两套实现

ProcessAttach 与 bootstrap 分别校验现场、分配 tid 和构造 Thread，后续 ThreadSpawn 接入会形成第三套路径。

shared 用 `ThreadStartContext` 表达 Attach/Spawn 共用现场；`Process::attach_thread` 集中现场校验、tid 分配、Thread 构造和 lifecycle 插入。ProcessAttach 与 bootstrap 只保留授权和输入 adapter，ThreadSpawn 批二复用同一出生入口。

### 7. 用户态失败清理不完整

竞态负载 helper 在 Building 组装失败后只关闭 builder/control，没有驱动 ProcessDrain 到完成；`SpawnRequest` 也未保证调用者持有完成清理所需的 MANAGE authority。

rinlib 新增 `abandon_to_completion`，统一执行 builder close → Drain 完成 → control close。libprocess 要求 control rights 含 MANAGE；所有失败路径保留原始错误，并单独报告清理错误。负载 helper 改用同一清理机制。

### 8. 文档同时表达新旧两套启动模型

多篇 ideas/impls 仍称 ProcessStart 直接 GRANT、构造普通 StartupBlock 和设置首线程现场；旧 review 又把无法定性的现场写成批一之外的问题。

方向文档统一为“Building 期独立组装，Start 唯一首次发布”；普通 StartupBlock 明确为用户态约定，init bootstrap 是唯一内核内嵌同构特例。提前 quiescent 的首份现场已包含线程模型改造，不能据现有证据判定引入批次，继续由独立调查计划跟踪。

## 验证

- `just check` 与完整用户态构建、ELF audit 通过；
- shared host tests 7 项、os 纯逻辑 host tests 105 项、rinlib 1 项、libprocess 5 项通过；
- `THROTTLE=100 just virt`、`virt-release`、`virt-hetero`、`virt-nofd` 通过；
- `THROTTLE=100 ACCEPTANCE_TIMEOUT=30 just sifive_u` 由终态锚点正常收割；
- 全部竞态矩阵 10/10，所有服务与 acceptance 域完成 Drain；
- `git diff --check` 通过，无遗留 QEMU、acceptance 或 throttle 进程。

## 保留风险

ThreadSpawn/Exit/Yield 与 join 壳属于批二，未在本报告范围实现。Ready reserve OOM、Grant 后续组装失败等路径已有结构化回滚和 host 覆盖，但尚无内核级故障注入矩阵。提前 quiescent 与 FramePool 普通 dealloc 无界扫描是独立已知问题，不因本轮事务收束而宣告解决。

## 文档归属

方向真值见 `notes/ideas/{task,bootstrap,object}.md`；实现真值见 `notes/impls/{task,startup,execution-context,ipc}.md`；后续批二/批三由 [`../todo-2026-09-thread-model.md`](../todo-2026-09-thread-model.md) 跟踪；提前 quiescent 调查现已由显式复位收口并归档于 [`todo-2026-08-29-early-quiescent-shutdown.md`](todo-2026-08-29-early-quiescent-shutdown.md)。
