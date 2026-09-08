# Review findings 统筹导航

> A–E 首审已结束，不重复发起首审。地址空间、等待、调度、生命周期终段、启动发布、Admission、Capability/owner、Supervision 与工程 lint 的实现均已完成，专题实施计划已经归档；当前等待形成固定修复提交后再恢复整批 Review。事务证据入口为 [`memory-transaction-state-machine` 档案](archived/todo-2026-09-memory-transaction-state-machine.md)。

## 文档职责

| 文档 | 唯一职责 |
|---|---|
| 本文 | findings → 契约 owner / 实施计划的导航与跨专题依赖，不重复定义方案 |
| `review-*.md` | 固定目标提交的历史证据、发现与复核条件；正文中的建议不是当前实施顺序 |
| 专题 `todo-*.md` | 当前目标、方案、实施单元、未决前置与完成门；同一缺口只在所属专题安排实施 |
| `notes/ideas/` | 可脱离当前代码成立的系统方向与契约 |
| `notes/impls/` | 可由当前代码取证的实现现状，不把计划目标写成已实现 |
| `COMPASS.md` | 当前方向、位置与各入口，不另排一套任务 |

报告与专题计划可以并存，但不得同时声称拥有该 finding 的当前行动真值。报告保留历史结论，修复后追加复核证据；不改写目标提交曾经存在的缺陷。所有有效条目完成复核后才归档该报告，不能因为其中一个机制子集完成就整篇归档。

## 系统机制与交付边界

### 地址空间事务与进程启动

[`memory-transaction-state-machine` 实施档案](archived/todo-2026-09-memory-transaction-state-machine.md)记录两个已完成的纵向单元：

1. 地址空间：ledger / funding / 来源保活 / abort / PTE publish / Remote / retire，连同匿名、MemoryObject、Building、Tunnel 调用者一次迁移。
2. 进程：私有 Bound 构造失败 / 普通 Start / Bootstrap / Job-lifecycle gate / 首次 Ready 发布一次迁移。

二者共享失败与提交原则，不合并成万能事务框架。基础机制继续保留：ledger 真值、funded owner、PoolBinding、MemoryObject permit、execution gate、Remote 确认链、有界 work debt 与 ProcessDrain。

完成闭包包含真实调用者交付，不止于 ledger Complete。线程全寿命调度准入、全部等待意图出口、通用通知、ThreadDeparture、Tunnel 不可失败 detached close、ProcessDrain/Job 终段、地址空间来源与容量、Bootstrap/普通创建发布均已接成同一完成责任链并通过 core/stress。validated ELF 已作为独立 admission 子单元接入 Bootstrap/libprocess/audit；公共 EXECUTE authority 也已接入 shared/kernel/rinlib 与真实 RX capability 矩阵；事务联合代码门已闭合，现状由 `notes/impls/` 记录，实施证据保留在各专题档案，不新增平行修复计划。

### 其它专题

- [`admission-fail-closed` 档案](archived/todo-2026-09-admission-fail-closed.md)：平台/ELF 输入的规范化、checked validation 与 immutable admission。
- [`identity-generation-boundaries` 档案](archived/todo-2026-09-identity-generation-boundaries.md)：实例身份、代次、不可回绕与耗尽策略。
- [`capability-owner-error-boundary` 档案](archived/todo-2026-09-capability-owner-error-boundary.md)：rights/signal、用户态 affine owner、已接收 capability 与 ReplyPort 的消费边界。
- [`supervision-authority-policy` 档案](archived/todo-2026-09-supervision-authority-policy.md)：用户态 authority 保留、有限等待与失败升级。
- 工程 lint 门已由 E-2 的 E2-7-01 闭合：`just clippy` 覆盖七个编译面，并作为 `just acceptance` 的前置门；报告保留首审与复核证据。

以上是职责域，不是要求“一个专题全做完才开始下一个”的队列，也不是一套共用状态机。关联子单元按前置关系衔接；独立问题不强塞进当前内存重构。

## Findings 实施归属

| 报告 / 条目 | 契约与实施 owner | 当前处置边界 |
|---|---|---|
| A：WritePermit rollback、同对象 owner、post-Commit 容量 | 内存事务计划，纵向单元一 | 已以事务来源、AVL view owner、region delta、backing growth 与预付 completion 闭合；终段复核发现并修复 Existing owner 竞态，未留高严重度 finding |
| A：EXECUTE capability、RX rights | Capability 计划，rights 纵向子单元 | 已增加独立 EXECUTE 位并接入 RX required rights、派生裁剪与 Seal/Map/Unmap 矩阵；MemoryChange 未重造权限规则 |
| B-1 F-1/F-2：DT status、FramePool arithmetic | Admission 计划，平台输入子单元 | 已按 DTSpec 固定 status 接受集；CPU/memory canonical admission、FramePool checked preflight 与失败原子性测试已闭合 |
| B-1 F-3/F-4：MemoryPool Drop、SystemSupply query | Capability/owner 计划，相应消费边界子单元 | typed leaf owner 已统一不可失败 close；SystemSupply 几何快照与 ticket owner 分离，消费后查询/计数测试通过 |
| B-2 F-1：Bootstrap post-commit | 内存事务计划，纵向单元二 | typed Handle commit、Job/lifecycle/execution 同锁区发布与首次 Ready 预付已闭合；终段复核未见 Commit 后可恢复失败 |
| C-1 P1-01/P1-02/P2-08：监督 authority、服务缺失、无限等待 | Supervision 计划 | RequiredLaunchSet、有限 collect/job_kill policy、失败 authority/progress 返还与 pm→init/root escalation 已闭合；一 work-unit 耗尽续接及 acceptance 通过 |
| C-1 P2-06：q-only CPU capability | Admission 计划 | canonical CPU admission 已强制 `q => d => f`，host 负向与 hetero/nofd QEMU 路线通过 |
| C-1 P2-07：ThreadControl CLOSED | Capability 计划，signal 子单元 | 已收窄为仅允许真实持续电平 DONE，并加入 stress 负向断言；待报告复核 |
| C-2 F1/F2：Remote token identity、AddressSpace epoch | Identity 计划 | Remote/work `TableId`、epoch CAS、全局单调 ID/token 与用户态 cookie/txid 已统一为不可回绕耗尽；跨表 owner 返还和最大代次退休测试通过 |
| C-2 F3：UserStack cleanup | Capability/owner 计划 | Map/Unmap 对 ObjectBusy 按 tick 重试；终端 cleanup 错误记录并留给 AddressSpace drain，不再 panic/无限等待；core 验证零 abandoned |
| D-1 P2-D1-03：MappingLease 失败/析构验证 | 内存事务计划，纵向单元一 | 显式 close 走完整 MemoryChange；REAPABLE detached close 并入 ProcessDrain，无后置 funding |
| D-2 F-02：启动 reservation 区间 | Admission 计划 | 已由 canonical memory/reservation admission 的 checked 对齐、排序、合并与重叠拒绝覆盖；不保留独立修复路径 |
| D-2 F-05/F-06：ELF entry、PT_INTERP/flags | Admission 计划，ELF 纵向子单元 | 已由 `os/elf::validate` 统一 runtime、audit、launcher 与 Bootstrap；entry file-byte、header/flags 和页权限负向测试通过 |
| D-2 F-07：reservation token | Identity 计划 | Handle/Job token 已使用永久耗尽的独立 domain；work-debt token 携 TableId；Ready 继续由保活 core 校验 |
| E-1 M3-1：Bound 镜像失败 | 内存事务计划，纵向单元二 | 预付 `UnpublishedReservation` 与显式 rollback 已闭合；Drop 只作未消费 token 断言 |
| E-1 M3-2/M3-3/M3-4：hart identity/Gate/order | Admission 计划 | raw hart 排序去重、strict registry、RuntimeGate 单向终态和 HSM/CSR/超时失败广播已闭合；普通 IPI 保留业务真值并返回失败 mask |
| E-2 E2-5-01：RPC reject capability/port | Capability/owner 计划，RPC 接收子单元 | 所有 reject 关闭已收 Handle 并 discard ReplyPort；QEMU 双调用验证 Handle stale 与新端口成功 |
| E-2 E2-7-01：clippy / lint 门 | E-2 报告 | 已以七面 `-D warnings` recipe 和 acceptance 前置门闭合；完整日志按面保存在 `artifacts/lint/` |

### 本轮终段复核

最终只读高严重度审查发现一项可达回归：object Map 在 Complete 阶段冻结 `Existing` owner，而并发最后 region retire 可在 Commit 前摘除它。修复后每次 object Map 都预付一个可插入 AVL 候选节点，Commit 按当时表状态复用或安装；满表 existing→remove→insert 由 `ordered_table` 回归测试覆盖。审查同时复核 Commit 后分配/错误、Handle/Job/lifecycle/Ready 原子发布、backing/permit 退役、三队列 fairness 与 Tunnel detached close，未发现其它高严重度 finding。

### 已有后续修复证据的历史条目

以下不是当前重复实施队列；改到对应 seam 时只做回归核验：

- C-1 P1-03/P1-04：`98d2449` 已以持久 supervisor 与显式 SystemReset 收口；旧 raw-hart shift（P1-05）由 `send_ipi(1, raw)` 替换，当前 hart admission 剩余由 E-1 承接。
- D-1 P1-D1-01/P1-D1-02：已有 `TimeoutRegistration`、`finish_installing` 与 TimerQueue cancel。
- D-2 F-01/F-03/F-04：已有 funded payload owner、按模型选择 QEMU memory、ProcessCreate 稳定 Control 与 Job member。

历史报告的最终判定按目标提交保留；上述状态不意味着本轮已重新运行其完整验证。

## 前置与自然顺序

```text
整体设计：owner / 提交资格 / 失败边界 / 预算 / 验证模型
    ├─ Ready 全寿命容量（已完成）→ 通用通知/离场交付 → 地址空间与 Drain 完成闭包
    ├─ Identity 策略及所需凭据 ──→ 地址空间纵向单元一
    ├─ EXECUTE/authority ───────→ 对象 RX 联合验收
    ├─ ELF validated image ────→ 构造与启动纵向单元二
    └─ 平台 admission ─────────→ 完整启动失败/SMP 联合验收

单元一 → 单元二 → 跨机制验收与原报告复核 → 多页 Tunnel / Runnel

用户态消费边界 ↔ 监督错误接管：独立设计交接，同步其公共契约
```

- Commit 后不可失败的承诺依赖 epoch/Ready/Remote 等凭据在修改前处理耗尽；不能先宣称事务闭合，再把这些门禁留给以后补。
- 任务分片只服务整体完成门：总体设计自顶向下约束 owner、容量、失败与发布边界；实施按依赖自下向上推进。分片代码可以暂未接通后续阶段，但必须直接采用最终接口，并在总计划登记连接点和剩余责任。
- 不把“先改纯逻辑 crate → adapter 接回旧内核 → 最后清理调用者”当作阶段顺序。一个纵向单元跨所有必要模块；中间施工可先编辑底层、用编译器定位未迁移调用点，但不为短暂可编译引入兼容层或平行真值。
- 设计确认、中间施工提交、完整纵向单元验收、提交后的 Review 是不同边界。中间提交只提供可回溯基线；局部编译或测试通过不构成专题完成。
- 多页 Tunnel/RNL2 的直接实现前置已经闭合，但仍等待本次固定提交后的 A–E 复核；最终架构 Review 继续要求数据面既定触发条件满足。

## 首审报告索引

每份报告保留其目标提交、执行命令和验证限制。此前施工提交与当前待提交工作树共同完成 A–E 所有已登记实现项，并通过 host、七面 lint 与完整 acceptance；五份专题实施计划因此归档。Review program 仍不能归档：当前工作树必须先形成固定修复提交，再以该提交为对象按九份原报告逐项复核，并把剩余验证缺口或新 finding 写回唯一真值点。

| 批次 | 报告 | 历史目标范围 |
|---|---|---|
| A | [`memory-transaction-unification`](review-2026-09-memory-transaction-unification.md) | `d2ff81e`、`5c0bbb0`、`d6a162c`，相关对象投影前置按 seam 引用 |
| B-1 | [`memory-supply-and-pool`](review-2026-09-memory-supply-and-pool.md) | `198e665`、`0a944c7`、`4715f3a`、`48227c8` |
| B-2 | [`process-bind-page-table-retire`](review-2026-09-process-bind-page-table-retire.md) | `7c76097`、`c522e50`、`7225673`、`cfad6cf`、`addb4a5`、`b4bfb20` |
| C-1 | [`lifecycle-and-scheduling`](review-2026-09-lifecycle-and-scheduling.md) | `d741880`、`bdc83ef`、`004cae5`、`fcbd5b6`、`b161163`、`1d7dc92` |
| C-2 | [`remote-call-user-memory`](review-2026-09-remote-call-user-memory.md) | 用户内存 `1cd6ab2` 至 `9358963`、`6199985`、`bdc83ef`、`004cae5` |
| D-1 | [`mechanism-generalization`](review-2026-09-mechanism-generalization.md) | `15c7811`、`9c03251`、`95deea6` |
| D-2 | [`bootstrap-launcher`](review-2026-09-bootstrap-launcher.md) | `29c6519..1bc83ac` |
| E-1 | [`system-audit-03-04`](review-2026-09-system-audit-03-04.md) | `e5db4f3`，审计分片 3–4 |
| E-2 | [`system-audit-05-07`](review-2026-09-system-audit-05-07.md) | `e5db4f3`，审计分片 5–7 |

历史首审清单位于 `archived/todo-*-review.md`；归档这些清单不表示 findings 已解决。系统审计规范仍见 [`todo-2026-08-system-audit.md`](todo-2026-08-system-audit.md)。

## 执行与复核纪律

1. 先核对当前代码与目标契约，区分有效缺陷、历史已修、验证缺口和待证假设；不能把历史建议机械转成实现。
2. 在所属专题冻结方案与依赖，再按完整纵向单元实施。发现前提冲突暂停该单元，修订计划后继续。
3. 每单元覆盖正常/失败/并发路径、真实锁阶、最后 owner 析构、容量及 work 计费，并同步删除旧入口和文档残留。
4. 使用仓库 Just/host 验证规则；完整日志保留，已知 flake 按 KNOWN_ISSUES 判读。只读分析不能冒充新增测试结果。
5. 修复提交之后按原报告逐项复核，记录修复提交、执行证据和剩余条目；报告全部闭合后归档。
6. 设备/中断 carryover 和最终架构 Review 保持既定触发条件，不提前伪造完成。

首审过程中曾有 provider 错误与 reviewer/explorer 配额失败，没有完成的委派不构成证据。今后是否委派以当前工具状态为准，不把历史账户限制作为永久项目约束。
