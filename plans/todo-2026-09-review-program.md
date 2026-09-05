# Review findings 统筹导航

> A–E 首审已结束，不重复发起首审。当前工作是按系统机制收口 findings，而不是逐篇报告顺序打补丁。事务与启动主线处于设计冻结前，入口为 [`todo-2026-09-memory-transaction-state-machine.md`](todo-2026-09-memory-transaction-state-machine.md)。

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

由 [`memory-transaction-state-machine`](todo-2026-09-memory-transaction-state-machine.md) 拥有两个纵向单元：

1. 地址空间：ledger / funding / 来源保活 / abort / PTE publish / Remote / retire，连同匿名、MemoryObject、Building、Tunnel 调用者一次迁移。
2. 进程：私有 Bound 构造失败 / 普通 Start / Bootstrap / Job-lifecycle gate / 首次 Ready 发布一次迁移。

二者共享失败与提交原则，不合并成万能事务框架。基础机制继续保留：ledger 真值、funded owner、PoolBinding、MemoryObject permit、execution gate、Remote 确认链、有界 work debt 与 ProcessDrain。

### 其它专题

- [`admission-fail-closed`](todo-2026-09-admission-fail-closed.md)：平台/ELF 输入的规范化、checked validation 与 immutable admission。
- [`identity-generation-boundaries`](todo-2026-09-identity-generation-boundaries.md)：实例身份、代次、不可回绕与耗尽策略。
- [`capability-owner-error-boundary`](todo-2026-09-capability-owner-error-boundary.md)：rights/signal、用户态 affine owner、已接收 capability 与 ReplyPort 的消费边界。
- [`supervision-authority-policy`](todo-2026-09-supervision-authority-policy.md)：用户态 authority 保留、有限等待与失败升级。
- 工程 lint 门由 E-2 的 E2-7-01 独立承接；尚无其它专题重复拥有它。

以上是职责域，不是要求“一个专题全做完才开始下一个”的队列，也不是一套共用状态机。关联子单元按前置关系衔接；独立问题不强塞进当前内存重构。

## Findings 实施归属

| 报告 / 条目 | 契约与实施 owner | 当前处置边界 |
|---|---|---|
| A：WritePermit rollback、同对象 owner、post-Commit 容量 | 内存事务计划，纵向单元一 | `4e62979` 有局部修复；统一来源 owner、跨批次与存储预算尚未闭合 |
| A：EXECUTE capability、RX rights | Capability 计划，rights 纵向子单元 | 完整 RX authority/验收的前置；不能由 MemoryChange 重造权限规则 |
| B-1 F-1/F-2：DT status、FramePool arithmetic | Admission 计划，平台输入子单元 | 编码前回到固定规范核对接受/拒绝集合；不凭历史建议猜标准 |
| B-1 F-3/F-4：MemoryPool Drop、SystemSupply query | Capability/owner 计划，相应消费边界子单元 | 用户态错误政策与纯 ticket 查询独立实现，不套内核事务类型 |
| B-2 F-1：Bootstrap post-commit | 内存事务计划，纵向单元二 | 审查必须延伸至 `boot.rs` / Ready 发布，不能只前移 Attach |
| C-1 P1-01/P1-02/P2-08：监督 authority、服务缺失、无限等待 | Supervision 计划 | 保留控制权直至可靠收束或明确接管；不由内核代做政策 |
| C-1 P2-06：q-only CPU capability | Admission 计划 | 与 canonical CPU admission 同单元 |
| C-1 P2-07：ThreadControl CLOSED | Capability 计划，signal 子单元 | allowed signal 与真实可达状态一致 |
| C-2 F1/F2：Remote token identity、AddressSpace epoch | Identity 计划 | 策略唯一；事务依赖的凭据与 epoch 门禁随单元一接入 |
| C-2 F3：UserStack cleanup | Capability/owner 计划 | 与 Join/reaper、监督政策确定交接，不在 Drop 无限重试 |
| D-1 P2-D1-03：MappingLease 失败/析构验证 | 内存事务计划，纵向单元一 | 对当前 Tunnel lease 结构复核，不复活历史 MappingLease 类型 |
| D-2 F-02：启动 reservation 区间 | Admission 计划 | 先核对当前规范化供给机制，已被覆盖的历史路径不重复修复 |
| D-2 F-05/F-06：ELF entry、PT_INTERP/flags | Admission 计划，ELF 纵向子单元 | runtime parser、audit、launcher 共用 validated image；构造单元二消费它 |
| D-2 F-07：reservation token | Identity 计划 | Handle/Job/Ready 各容器凭据绑定身份；相关 Start 接线随单元二完成 |
| E-1 M3-1：Bound 镜像失败 | 内存事务计划，纵向单元二 | 私有失败驱动与正式 Drain 同源，不隐藏在 Drop 中 |
| E-1 M3-2/M3-3/M3-4：hart identity/Gate/order | Admission 计划 | canonical admitted 集合与失败广播 |
| E-2 E2-5-01：RPC reject capability/port | Capability/owner 计划，RPC 接收子单元 | 消费拒绝消息及旧端口，不复用污染状态 |
| E-2 E2-7-01：clippy / lint 门 | E-2 报告 | 独立工程项，不阻塞内存结构设计；不能宣称全仓 lint-clean |

### 已有后续修复证据的历史条目

以下不是当前重复实施队列；改到对应 seam 时只做回归核验：

- C-1 P1-03/P1-04：`98d2449` 已以持久 supervisor 与显式 SystemReset 收口；旧 raw-hart shift（P1-05）由 `send_ipi(1, raw)` 替换，当前 hart admission 剩余由 E-1 承接。
- D-1 P1-D1-01/P1-D1-02：已有 `TimeoutRegistration`、`finish_installing` 与 TimerQueue cancel。
- D-2 F-01/F-03/F-04：已有 funded payload owner、按模型选择 QEMU memory、ProcessCreate 稳定 Control 与 Job member。

历史报告的最终判定按目标提交保留；上述状态不意味着本轮已重新运行其完整验证。

## 前置与自然顺序

```text
整体设计：owner / 提交资格 / 失败边界 / 预算 / 验证模型
    ├─ Identity 策略及所需凭据 ──→ 地址空间纵向单元一
    ├─ EXECUTE/authority ───────→ 对象 RX 联合验收
    ├─ ELF validated image ────→ 构造与启动纵向单元二
    └─ 平台 admission ─────────→ 完整启动失败/SMP 联合验收

单元一 → 单元二 → 跨机制验收与原报告复核 → 多页 Tunnel / Runnel

用户态消费边界 ↔ 监督错误接管：独立设计交接，同步其公共契约
```

- Commit 后不可失败的承诺依赖 epoch/Ready/Remote 等凭据在修改前处理耗尽；不能先宣称事务闭合，再把这些门禁留给以后补。
- 不把“先改纯逻辑 crate → adapter 接回旧内核 → 最后清理调用者”当作阶段顺序。一个纵向单元跨所有必要模块，采用最终接口并在同单元删除旧路径。
- 可以先编辑某个模块、用编译器定位未迁移调用点；中间工作树能否编译不是架构决策依据，不为它引入临时机制。
- 设计确认、完整纵向单元验收、提交后的 Review 是不同边界；不能把“分阶段确认”解释为每个 crate 独立交付。
- 多页 Tunnel/RNL2 只在直接依赖的事务/启动及 authority 闭合后恢复。其他独立 findings 继续按专题推进；最终架构 Review 仍要求 A–E 基础问题与既定数据面触发条件满足。

## 首审报告索引

每份报告保留其目标提交、执行命令和验证限制。当前代码准备基线为 `4e62979`，本批只交付文档；不能以当前实现替代历史快照，也不能把干净工作树视为 findings 已闭合。

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
