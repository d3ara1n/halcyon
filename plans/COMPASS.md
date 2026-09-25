# 罗盘

本篇只登记任务入口及接手顺序。下列顺序是当前排期，不代表未注明的技术依赖；任务的状态、范围、已完成工作、阻塞、注意事项、验证证据与下一步由所链接的唯一工作记录拥有。完成记录与长期机制说明分别从计划自身及 [notes 索引](../notes/README.md) 查阅；施工和审查规则见 [AGENTS](../AGENTS.md)、[REVIEW](REVIEW.md)。

## 近期顺序

1. [验收 fixture 与自检清理](todo-2026-09-23-acceptance-fixture-cleanup.md)：继续逐项审查未替代的自检窗口。

## 可择机任务与审查

以下不表示自然顺序，也不阻塞新的系统能力。架构审计剩余发现可随时择机复核；容量计量暂缓，待真实故障、消费者或明确的硬容量承诺再重开。Review 材料待 reviewer 汇总现有档案与 findings，形成明确范围、优先级和唯一入口的总体 Review 任务；FAL 固定提交审查计划也由该任务纳入。

| 类型 | 入口 |
|---|---|
| 审计剩余发现 | [架构审计发现承接](todo-2026-09-14-architecture-audit-findings.md) |
| Review | [FAL 集成基线](todo-2026-09-13-fal-integration-baseline-review.md) |
| Review | [公共时间与期限](todo-2026-09-13-monotonic-time-rpc-deadline-review.md) |
| Review | [库存来源与自检](todo-2026-09-frame-source-selftest-review.md) |
| Review | [设计审查后续内存机制](todo-2026-09-design-audit-followup-review.md) |
| Review | [消息运输](todo-2026-09-14-message-transport-review.md) |
| Review | [流运输与 Runnel](todo-2026-09-14-stream-transport-review.md) |
| Review | [公共操作所有权](todo-2026-09-18-public-operation-ownership-review.md) |
| Review | [通用执行、RPC 与 Outbox](todo-2026-09-15-runtime-admission-review.md) |
| Review | [用户态库知识归属与命名](todo-2026-09-21-library-knowledge-ownership-review.md) |
| Review | [FAL 库基线](todo-2026-09-21-fal-library-baseline-review.md) |
| Review | [FAL 注册发现设计](todo-2026-09-21-fal-service-design-review.md) |

## 条件触发的计划

下列项目不占近期串行位置。触发或前置关系由各计划拥有，条件提前出现时重排本篇。

| 任务 | 排期条件 |
|---|---|
| [用户态映射 owner](todo-2026-09-14-user-memory-owner-lifecycle.md) | 长期动态 mapping 或正式退休需求 |
| [内核 metadata 预算](todo-2026-09-14-kernel-memory-budget.md) | 不可信创建域或独立预算隔离需求；M2 核实结论可提前调整 |
| [FAL 扩展操作](todo-2026-09-fal-extended-operations.md) | 范围外操作出现真实消费者 |
| [平台保留内存生命周期](todo-2026-09-platform-reserved-memory-lifecycle.md) | 正式设备/DMA 接入前 |
| [设备与中断审查承接](todo-2026-08-26-review-carryover.md) | 设备/中断设计开始 |
| [系统关机编排](todo-2026-09-system-shutdown-orchestration.md) | 正式长期服务需关机收束，或对外承诺优雅关机前 |
| [电源管理服务](todo-2026-09-power-management-service.md) | 独立电源管理需求明确后 |
| [内核最终架构审查](todo-2026-09-kernel-final-architecture-review.md) | 主要用户态消费者及计划所列主线前置完成后 |

## 尚未立案的候选

以下仅保留任务登记与事实入口。触发后并入所属计划或建立唯一工作记录，不能把本表作为施工说明。

| 候选 | 触发 | 归属入口 |
|---|---|---|
| CPU 预约对象 | 不可信执行域接入 | [task](../notes/ideas/task.md) |
| fence.i 代码代次优化 | 计时手段可用且开销实测可见 | [task 实现](../notes/impls/task.md) |
| affinity / 跨域迁移 ABI | 真实多域硬件或放置需求 | [execution-context](../notes/ideas/execution-context.md) |
| initfs manifest / 服务编排 | 正式服务编排需求 | [bootstrap](../notes/ideas/bootstrap.md) |
| ld-erhino 动态链接 | 明确动态链接需求 | [bootstrap](../notes/ideas/bootstrap.md) |
| F-only/Q/V/TSO 档位 | 真实需求 | [execution-context 实现](../notes/impls/execution-context.md) |
| TLS ABI | TLS 需求 | [task 实现](../notes/impls/task.md) |
| 多用户 / ACL | 多用户需求 | [object](../notes/ideas/object.md) |
| ASID 与定向 shootdown | 地址空间切换开销实测 | [task 实现](../notes/impls/task.md) |
| 更细粒度 heap 计量或硬容量保证 | 真实 OOM/退休故障、指定工作集的容量承诺或观测到的性能瓶颈 | [架构审计 A2/M3/M5](todo-2026-09-14-architecture-audit-findings.md)、[mm](../notes/ideas/mm.md) |
| Unmap 唤醒点前移 | 内存操作延迟瓶颈有测量证据 | [mm](../notes/ideas/mm.md) |
| FAL 正式故障注入补证 | 稳定正式注入入口出现，或生产故障暴露清理问题 | [FAL 交付档案](archived/todo-2026-09-fal-service-capabilities.md#跨会话接力断点) |
