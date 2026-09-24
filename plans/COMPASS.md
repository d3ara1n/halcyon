# 罗盘

当前任务与按需入口。施工规则由 [AGENTS](../AGENTS.md) 拥有，进度、阻塞和验证证据由各任务唯一工作记录拥有；本篇只保留接手摘要与链接，不累积历史接力指令。

## 当前接手

**当前接手：FAL 服务能力与内核身份统一提交后的独立 Review**：[审查计划](todo-2026-09-24-fal-kernel-closure-review.md) 固定提交 `dda8a5b6f4fd7700378f9e85804c6b1d30826601`，核对 FAL 责任链、`kernel` 身份迁移、构建入口、notes/plans 归档与提交范围。FAL 基本能力、内核身份施工和 Agent 规范审查均已完成；当前不执行新的实现改动。

- 当前唯一施工入口已从 FAL 切换到固定提交审查；FAL 实现计划与内核身份计划均已完成，保留其验证证据和历史审查导航。
- FAL 的 D0–D5、F4 结构复核/组合验收以及内核身份改名的构建、host、QEMU、boot-failure 证据分别写入所属计划；剩余仅为各固定提交的独立 Review。
- 当前状态：准备接手 `60e3253` 的只读规范/导航审查；不修改实现代码，不把当前未提交工作树混入目标提交结论。

## 按主题阅读

| 主题 | 方向或规则 | 当前实现 |
|---|---|---|
| 内核边界、有界推进与提交后责任 | [kernel](../notes/ideas/kernel.md)、[object](../notes/ideas/object.md) | [internals](../notes/impls/internals.md)、[call](../notes/impls/call.md) |
| 任务、调度域与生命周期 | [task](../notes/ideas/task.md)、[execution-context](../notes/ideas/execution-context.md) | [task](../notes/impls/task.md)、[execution-context](../notes/impls/execution-context.md) |
| 内存、资源来源与事务 | [mm](../notes/ideas/mm.md) | [mm](../notes/impls/mm.md)、[memory-object](../notes/impls/memory-object.md) |
| 用户态领域与执行 | [库分层](../user/libraries/README.md)、[framework](../notes/ideas/framework.md) | [runtime](../notes/impls/runtime.md)、[startup](../notes/impls/startup.md) |
| FAL、服务与 RPC | [fal](../notes/ideas/fal.md)、[service](../notes/ideas/service.md)、[rpc](../notes/ideas/rpc.md) | [fal](../notes/impls/fal.md)、[rpc](../notes/impls/rpc.md) |
| 其他设计主题 | [notes 索引](../notes/README.md) | 同索引中的 impl 拥有篇 |
| 构建、QEMU 与平台 | [BUILD-AND-TEST](BUILD-AND-TEST.md) | 参数与命令以 Justfile 为准 |
| 故障调查与工具 | [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md)、[TOOLING-PITFALLS](TOOLING-PITFALLS.md) | 按故障选择，不作为每次接手必读材料 |
| 交付前检查与独立审查 | [REVIEW](REVIEW.md) | 完成条件由 AGENTS 唯一定义 |

验收配置不等于全部实现都是临时机制。服务进程中的正式机制、装配接缝、验收政策与过渡路径分别在 impls 和唯一工作记录中说明；当前 FAL 实现入口为上表对应篇。

## 活跃工作记录

专题实施与独立 Review 分别拥有自己的工作记录；同一问题不在两处安排。已完成但尚保留审查导航的入口明确标注，不作为新的施工顺序。

| 文件 | 状态与范围 |
|---|---|
| [FAL 服务能力](todo-2026-09-fal-service-capabilities.md) | F3d/F3e/F3f 与 F4-1/F4-2/F4-3 已完成；完整验收通过，保留无稳定正式注入入口的验证限制 |
| [验收 fixture 清理](todo-2026-09-23-acceptance-fixture-cleanup.md) | 消费者矩阵、映像 owner 与失败判定已核对；两个 binary 暂留，余下逐项审查其他自检窗口 |
| [内核身份统一](todo-2026-09-22-kernel-identity.md) | 已完成；package、默认 target、crate identifier、产物与有效入口统一为 `kernel`，构建/host/QEMU/boot-failure 验证通过 |
| [Agent 规范与导航审查](archived/todo-2026-09-22-agent-guidance-review.md) | 已归档并关闭；报告见 [archived/review-2026-09-24-agent-guidance.md](archived/review-2026-09-24-agent-guidance.md) |
| [FAL/内核闭包提交审查](todo-2026-09-24-fal-kernel-closure-review.md) | 当前接手；固定提交 `dda8a5b6`，FAL 责任链、`kernel` 身份迁移与文档边界 |
| [FAL 集成基线审查](todo-2026-09-13-fal-integration-baseline-review.md) | 待审固定 `d22b9d7`；公共对象前置与混合集成边界 |
| [公共时间审查](todo-2026-09-13-monotonic-time-rpc-deadline-review.md) | 待审固定 `c6e0a84`；时钟、绝对期限与消费者 |
| [库存来源审查](todo-2026-09-frame-source-selftest-review.md) | 待审固定 `606b59d`；库存、boot-held owner、清零与退款 |
| [设计审查后续复核](todo-2026-09-design-audit-followup-review.md) | 待审 `4b27ce6`、`8aa7bc2`；RX 同步、来源保活与 Sealing 收缩 |
| [库归属重排审查](todo-2026-09-21-library-knowledge-ownership-review.md) | 待审固定 `96ee03b`；记账、执行、消费者与命名迁移 |
| [FAL 设计审查](todo-2026-09-21-fal-service-design-review.md) | 固定文档提交 `1000270`；待对应实现闭合后审查，非实现验收证据 |
| [FAL 库基线审查](todo-2026-09-21-fal-library-baseline-review.md) | 待审固定 `dfcf7a3`；F1–F3c 与库重排前基线 |
| [消息运输审查](todo-2026-09-14-message-transport-review.md) | 待审固定 `3060dd8`；typed 运输、消费式 Packet 与失败路径 |
| [流运输审查](todo-2026-09-14-stream-transport-review.md) | 待审固定 `a2aabed`；typed 工厂、观察草稿删除与终态边界 |
| [运行期准入审查](todo-2026-09-15-runtime-admission-review.md) | 待审 `a3891b0` → `5de2780` → `e0b5c45` → `4e18e5e`；Runtime、RPC、Outbox 与消费者 |
| [公共操作所有权审查](todo-2026-09-18-public-operation-ownership-review.md) | 待审固定 `8e0467a`；工作债务、请求代次与退出交棒 |
| [执行前置交付记录](todo-2026-09-13-service-runtime-prerequisites.md) | 前置已完成；保留交付与固定提交审查导航，不拥有后续执行施工 |
| [用户内存 owner](todo-2026-09-14-user-memory-owner-lifecycle.md) | 执行基座和 FAL 基础之后，由长期动态 mapping / 正式 reaper 需求触发 |
| [内核 metadata 预算](todo-2026-09-14-kernel-memory-budget.md) | 不可信分配/创建域隔离触发，默认排在 FAL 基础与映射 owner 后 |
| [FAL 扩展操作](todo-2026-09-fal-extended-operations.md) | 真实消费者触发；不隐含在基本 FAL 交付中 |
| [平台保留内存生命周期](todo-2026-09-platform-reserved-memory-lifecycle.md) | 正式设备/DMA 接入前完成相关规范支持 |
| [电源管理服务](todo-2026-09-power-management-service.md) | 未来设计；职责、拓扑、协议与能力分配单独裁决 |
| [系统关机编排](todo-2026-09-system-shutdown-orchestration.md) | 未来设计；不预设电源管理服务的执行主体与职责 |
| [设备审查承接](todo-2026-08-26-review-carryover.md) | 设备、中断、DMA 接入时触发 |
| [内核最终架构审查](todo-2026-09-kernel-final-architecture-review.md) | 等主要用户态消费者及该计划前置完成 |
| [架构审计发现](todo-2026-09-14-architecture-audit-findings.md) | 逐项并入所属专题或独立立案，不占活跃串行位 |

## 待触发事项

仅索引尚无独立计划的事项，详情由对应文档拥有；触发后并入所属工作记录或立案。本表不重复活跃计划或 KNOWN_ISSUES。

| 事项 | 触发 | 详情 |
|---|---|---|
| CPU 预约对象 | 不可信执行域接入 | [task](../notes/ideas/task.md)「线程」 |
| fence.i 代码代次优化 | 具备计时手段且开销实测可见 | [task 实现](../notes/impls/task.md)「调度」 |
| 显式 affinity / 跨域迁移 ABI | 真实多域硬件或放置需求 | [execution-context](../notes/ideas/execution-context.md)「调度域」、[域准入档案](archived/todo-2026-08-28-domain-eligibility.md) |
| initfs manifest / 服务编排 | 正式服务编排需求 | [bootstrap](../notes/ideas/bootstrap.md) |
| ld-erhino 动态链接 | 构想态，尚无明确触发 | [bootstrap](../notes/ideas/bootstrap.md) |
| F-only/Q/V/TSO 档位 | 真实需求 | [execution-context 实现](../notes/impls/execution-context.md) |
| TLS ABI | TLS 需求 | [task 实现](../notes/impls/task.md) |
| 多用户 / ACL | 多用户需求 | [object](../notes/ideas/object.md) |
| ASID 与定向 shootdown | 地址空间切换开销实测 | [task 实现](../notes/impls/task.md) |
| admission / Remote 槽高水位 | KernelMemoryBudget 立案或容量重校 | [mm](../notes/ideas/mm.md)「MemoryPool 与 backing charge」 |
| Unmap 唤醒点前移 | 有测量证据的内存操作延迟瓶颈 | [mm](../notes/ideas/mm.md)「MemoryChange 事务」 |

## 完成证据与历史入口

当前实现从 notes 读取；以下记录仅作追溯，不继承其中的历史“下一步”。

| 主题 | 记录 |
|---|---|
| 本次导航整理前的完整记录 | [COMPASS 快照](archived/ref-2026-09-22-compass-snapshot.md)，保留原工作树中的历史过程与旧接力摘要 |
| Agent 规范与导航整理 | [完成记录](archived/todo-2026-09-22-agent-guidance.md)；内核命名整改仍由活跃计划承接 |
| 公共对象前置 | [交付档案](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md) |
| 公共时间 `c6e0a84` | [交付档案](archived/todo-2026-09-monotonic-time-rpc-deadline.md) |
| 公共操作 `8e0467a` | [交付档案](archived/todo-2026-09-14-public-operation-ownership.md) |
| 用户态库归属 `96ee03b` | [迁移档案](archived/todo-2026-09-21-library-knowledge-ownership.md) |
| A–E 审查收口 `228b6a5` | [Review program](archived/todo-2026-09-review-program.md) |
| 内存事务与数据面 | [MemoryObject 统一](archived/todo-2026-09-memory-object-unification.md)、[数据面档案](archived/todo-2026-09-memory-object-data-plane.md) |
| 验收时间敏感现象 | [调查与重开条件](archived/ref-2026-09-acceptance-timing-flake.md) |
