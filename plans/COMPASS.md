# 罗盘

本篇只登记任务入口及接手顺序。下列顺序是当前排期，不代表未注明的技术依赖；任务的状态、范围、已完成工作、阻塞、注意事项、验证证据与下一步由所链接的唯一工作记录拥有。完成记录与长期机制说明分别从计划自身及 [notes 索引](../notes/README.md) 查阅；施工和审查规则见 [AGENTS](../AGENTS.md)、[REVIEW](REVIEW.md)。

## 当前设计

> 部分区域处于矫正态，现有写法不作参考，见 [KNOWN_ISSUES 矫正区](../KNOWN_ISSUES.md#矫正区)。

1. [全项目重构设计总纲](todo-2026-09-29-system-redesign.md)：无兼容约束重审；按独立契约形成 ideas 与成型实施分案。
2. [Capability 与资源释放](todo-2026-09-28-capability-last-reference.md)、[内核维护执行](todo-2026-09-29-kernel-maintenance-execution.md)、[进程自动回收](todo-2026-09-29-process-automatic-reclamation.md)：共同闭合本地释放与自动收束主线。
3. [内存管理与地址空间重构](todo-2026-10-01-memory-redesign.md)：地址空间整体退休与 view/backing 归还是上一条主线的前置。
   - **下阶段入口**：[对象持有与释放链内核整体接入](todo-2026-10-01-object-chain-kernel-integration.md)；其后 [锁优化与无锁化评估](todo-2026-10-01-lock-performance.md)。
4. [设备体系整体设计](todo-2026-09-device-system-design.md#当前进度与接力)：结合全项目目标确定基础对象关系后继续。

运行时分层与线程生命周期已完成：[运行时分层 review](archived/review-2026-09-28-runtime-layering.md)。

## 旧基线待办

以下保留旧基线问题入口；新重构是否需要先处理，由目标消费者与验证需求决定，不自动作为全项目重构前置。

1. [FAL midflight Copy 间歇失败](todo-2026-09-29-fal-midflight-copy-flake.md)：旧 FAL 验收基线问题，后续按相关消费者保留或替换情况承接。
2. [验收 fixture 与自检清理](todo-2026-09-23-acceptance-fixture-cleanup.md)：继续逐项审查未替代的自检窗口。

## 可择机任务与审查

以下不表示自然顺序，也不阻塞新的系统能力。架构审计剩余发现可随时择机复核；容量计量暂缓，待真实故障、消费者或明确的硬容量承诺再重开。Review 材料待 reviewer 汇总现有档案与 findings，形成明确范围、优先级和唯一入口的总体 Review 任务；FAL 固定提交审查计划也由该任务纳入。

| 类型 | 入口 |
|---|---|
| 审计剩余发现 | [架构审计发现承接](todo-2026-09-14-architecture-audit-findings.md) |
| Review | [条件性容量校准提交](todo-2026-09-25-capacity-calibration-review.md) |
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
| [内核 metadata 预算](todo-2026-09-14-kernel-memory-budget.md) | 不可信创建域或独立预算隔离需求；M2 核实结论可提前调整 |
| [FAL 扩展操作](todo-2026-09-fal-extended-operations.md) | 范围外操作出现真实消费者 |
| [平台保留内存生命周期](todo-2026-09-platform-reserved-memory-lifecycle.md) | 正式设备/DMA 接入前 |
| [设备与中断审查承接](todo-2026-08-26-review-carryover.md) | 设备/中断设计开始 |
| [系统关机编排](todo-2026-09-system-shutdown-orchestration.md) | 正式长期服务需关机收束，或对外承诺优雅关机前 |
| [电源管理服务](todo-2026-09-power-management-service.md) | 独立电源管理需求明确后 |
| [内核最终架构审查](todo-2026-09-kernel-final-architecture-review.md) | 主要用户态消费者及计划所列主线前置完成后 |
| [Halcyon std 平台层](todo-2026-09-28-halcyon-std.md) | 出现必须依赖 std 的真实第三方需求 |

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
| 用户态多用户 / ACL 政策（非刚需） | 明确的多用户产品需求；不扩张内核身份模型 | [object](../notes/ideas/object.md) |
| ASID 与定向 shootdown | 地址空间切换开销实测 | [task 实现](../notes/impls/task.md) |
| 更细粒度 heap 计量或硬容量保证 | 真实 OOM/退休故障、指定工作集的容量承诺或观测到的性能瓶颈 | [架构审计 A2/M3/M5](todo-2026-09-14-architecture-audit-findings.md)、[mm](../notes/ideas/mm.md) |
| Unmap 唤醒点前移 | 内存操作延迟瓶颈有测量证据 | [mm](../notes/ideas/mm.md) |
| FAL 正式故障注入补证 | 稳定正式注入入口出现，或生产故障暴露清理问题 | [FAL 交付档案](archived/todo-2026-09-fal-service-capabilities.md#跨会话接力断点) |
