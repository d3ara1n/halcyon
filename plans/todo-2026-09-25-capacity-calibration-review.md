# 条件性容量校准提交后独立审查

> 状态：待独立 Review；本记录只安排固定提交的事后检查，不重开容量计量，也不替代交付前结构检查。

## 固定对象

- 提交：`3dd7c4f1c8ee535c22b5bb2f0dafb98402a81098`，父提交 `40d0b34`。
- 范围：metadata Counter 峰值和 Ready 重置、heap 边界采样与 source 失败布局、Bind/Reset 日志、`srv_init` capacity feature 和 `just virt-capacity`、验收日志保留开关，以及条件性准入契约与计划导航。
- 不包含：改变 `32` 的政策数值、独立 `KernelMemoryBudget`、allocator 内瞬时峰值或最大连续块实现。此前 Talc 两点快照的独立审查仍由 [metadata 容量观测 Review](todo-2026-09-24-metadata-capacity-observation-review.md) 固定原提交承载；可一并阅读，但不要把审查目标合并为浮动 HEAD。

## 审查重点

1. Counter 成功 acquire、部分失败退款与 Ready 重置的并发语义；20 类 usage/peak/reset 是否使用同一顺序。
2. HEAP 与 admission 锁序、trap/Bind/Reset 采样的开销和可重入性；source failure 布局与采样极值是否被误解释为 allocator 全部失败或硬容量保证。
3. 正式 Create/Bind 的顺序与四线程并发上限、第 33 张失败回滚，以及 Map/Unmap/backing/drain 和测试 fixture 异常路径的 owner、退款与逐项收束。
4. ideas/impls/工作记录是否一致地表达提交前 fallible、提交后必成的条件性准入；COMPASS 的可择机排期是否仍保留需要先完成的独立预算触发条件。

## 已有证据与完成门

提交前 `just check`、capacity feature 的 `just clippy`、`metadata_admission` host 8 项测试、`just virt`、`just virt-stress` 与重复 `just virt-capacity` 通过；负载数值和证据边界见[架构审计记录](todo-2026-09-14-architecture-audit-findings.md)。QEMU 完整日志保存在本机忽略版本控制的 `artifacts/`，不属于固定提交的一部分。上述只是交付证据，不是独立 Review 结论。

独立 reviewer 以固定提交只读审查。无 finding 时记录结论并归档；有 finding 时生成唯一 `review-*.md` 报告，在此登记入口后归档本计划，修复与复核改由报告承载。Review 不授权新的实施、提交或推送。
