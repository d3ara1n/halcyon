# 罗盘

本篇只登记任务入口及接手顺序。下列顺序是当前排期，不代表未注明的技术依赖；任务的状态、范围、已完成工作、阻塞、注意事项、验证证据与下一步由所链接的唯一工作记录拥有。完成记录与长期机制说明分别从计划自身及 [notes 索引](../notes/README.md) 查阅；施工和审查规则见 [AGENTS](../AGENTS.md)、[REVIEW](REVIEW.md)。

## 当前设计

> 部分区域处于矫正态，现有写法不作参考，见 [KNOWN_ISSUES 矫正区](../KNOWN_ISSUES.md#矫正区)。

1. [全项目重构总纲：剩余切面与接续顺序](todo-2026-09-29-system-redesign.md#剩余切面与接续顺序)。轻量任务及本轮文档范围见[收口记录](todo-2026-09-29-system-redesign.md#近期轻量收口与本轮整理)，后续代码/Review按各自记录接手；主线设计入口如下。
2. [OAL、fs 与服务目录改造](todo-2026-10-07-fal-service-redesign.md#当前接力)：本轮实施已提交；[固定交付独立 Review](archived/todo-2026-10-11-oal-fs-service-review.md)已完成，报告记录的证据缺口继续由主计划和既有 owner 承接；通用等待与付款前置分别由 [Notification 观察角色](todo-2026-10-10-notification-observer-roles.md#当前接力与范围)和[执行付款](archived/todo-2026-10-10-execution-funding.md#当前接力与范围)拥有。
3. [设备体系首批实施入口](todo-2026-09-device-system-design.md#当前进度与接力)：首批施工设计已闭合；按该案依赖、装配/内存/故障终局合同和验收门接续，代码施工另行授权。
4. [正式服务装配与监督](todo-2026-10-07-service-assembly.md)：通用装配关系可先设计，设备特有接线按设备前置。
5. [系统关机编排与设备故障终局](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)：设备管理故障接点随设备设计推进；正式长期服务的整体收束由本案闭合。

对象链已交付范围和[内存/hart兼容完成边界](todo-2026-10-01-memory-redesign.md#完成边界与-hart-兼容性复核)从各记录接手，不恢复旧内核拼装；运行时分层/线程生命周期见[复核档案](archived/review-2026-09-28-runtime-layering.md)。五批 Review 的当前有效性与后续责任统一从[总报告](review-2026-10-12-consolidated.md)接手；测量触发的[锁评估](todo-2026-10-01-lock-performance.md)按自身条件推进。

旧 midflight Copy 已按[机制替换证据](archived/todo-2026-09-29-fal-midflight-copy-flake.md#2026-10-11-历史证据复核与最终处置)关闭；Watch 历史未知错误继续由 [W05-04](todo-2026-10-07-fal-service-redesign.md#历史问题重新核定2026-10-11)承接，不恢复旧 Copy 施工。

## Review 收口

五批九份报告已统一核定，原报告与[分批执行记录](archived/todo-2026-10-12-review-convergence.md)保留历史；当前只使用[统一报告](review-2026-10-12-consolidated.md)的有效责任、触发条件与原编号映射。

| 类型 | 入口 |
|---|---|
| 五批 Review 当前责任与处置 | [统一报告](review-2026-10-12-consolidated.md) |
| 审计剩余发现 | [架构审计发现承接](todo-2026-09-14-architecture-audit-findings.md) |
| 设备体系设计基线审查档案 | [原发现与修订复核](archived/review-2026-10-23-device-system-design.md)；后续增量与施工入口见设备主计划 |

## 条件触发的计划

下列项目不占近期串行位置。触发或前置关系由各计划拥有，条件提前出现时重排本篇。

| 任务 | 排期条件 |
|---|---|
| [不可信域metadata硬隔离](todo-2026-09-14-kernel-memory-budget.md) | 开放不可信创建域或承诺独立硬容量隔离前；已有双维Pool付款不重做 |
| [FAL 扩展操作](todo-2026-09-fal-extended-operations.md) | 范围外操作出现真实消费者 |
| [平台保留内存生命周期](todo-2026-09-platform-reserved-memory-lifecycle.md) | 平台/设备消费动态保留区、memory-region 或 reusable 前 |
| [设备与中断审查承接](todo-2026-08-26-review-carryover.md) | IRQ 实施时核对设计重审约束及运行验证门 |
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
| ld-erhino 动态链接 | 明确动态链接需求 | [bootstrap](../notes/ideas/bootstrap.md) |
| F-only/Q/V/TSO 档位 | 真实需求 | [execution-context 实现](../notes/impls/execution-context.md) |
| TLS ABI | TLS 需求 | [task 实现](../notes/impls/task.md) |
| 用户态多用户 / ACL 政策（非刚需） | 明确的多用户产品需求；不扩张内核身份模型 | [object](../notes/ideas/object.md) |
| ASID 与定向 shootdown | 地址空间切换开销实测 | [task 实现](../notes/impls/task.md) |
| 更细粒度 heap 计量或硬容量保证 | 真实 OOM/退休故障、指定工作集的容量承诺或观测到的性能瓶颈 | [架构审计 A2/M3/M5](todo-2026-09-14-architecture-audit-findings.md)、[mm](../notes/ideas/mm.md) |
| Unmap 唤醒点前移 | 内存操作延迟瓶颈有测量证据 | [mm](../notes/ideas/mm.md) |
| FAL 正式故障注入补证 | 稳定正式注入入口出现，或生产故障暴露清理问题 | [FAL 交付档案](archived/todo-2026-09-fal-service-capabilities.md#跨会话接力断点) |
