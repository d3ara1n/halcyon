# Metadata 容量观测提交后独立审查

> 状态：待独立 Review。本记录只安排固定提交的事后审查，不替代架构审计 A2/M3/M5 的未完成容量校准，也不表示已验证 32 个 Bound AddressSpace 可同时驻留。

## 固定对象

- 提交：`bfe38ff244c3fa283240444ab26c0030214ccbd9`，父提交 `be6dae30e6018d93890e71f95bc0c2c00e0585cf`。
- 范围：Talc `counters`、Ready/reset 堆快照、heap ticket 消费数、RV64 ledger 条目布局输出，以及对应 mm/internals、审计工作记录和 COMPASS 更新。
- 未包含：分配准入、共享 ABI、独立 KernelMemoryBudget、运行中高水位采样或 32 张账本饱和负载。未闭合容量责任仍由[架构审计工作记录](todo-2026-09-14-architecture-audit-findings.md)拥有。

## 审查重点

1. 观测是否在 HEAP 锁外访问 SYSTEM_SUPPLY 和 console；Ready、授权 reset、失败 reset 与其他 hart 并发时是否引入锁序反转、重入分配或不可接受的延迟。
2. Talc `counters` 的 claimed/allocated/available/fragment 与 ticket 消费量的语义是否在日志和 impls 中准确；两点快照是否被误称为真实高水位或最大连续可分配块。
3. `MemorySpace::entry_sizes()` 是否只暴露私有条目的目标布局；393472/12591104 字节的下界是否正确，是否误含 Vec/allocator、AddressSpace 壳或其它 metadata。
4. 可选计数功能的常数级运行成本、对象大小和日志行为是否适合正式内核；文档、原工作记录与本次提交的验证边界是否一致。

## 已有证据与完成门

提交前 `just check`、`just clippy` 七面、virt/core 和 virt/stress 均通过；本机摘要在 `artifacts/heap-core-run.log`、`artifacts/heap-stress-run.log`，成功路线的 QEMU 原始临时日志不在 Git 中。core Ready/Reset 与 stress Reset 的采样值及缺失的 stress Ready 行由[架构审计工作记录](todo-2026-09-14-architecture-audit-findings.md#本轮核验与接力)保留。这些不是独立 Review 结论，审查应固定在上述提交，不能用后续 HEAD 替代。

独立 reviewer 完成只读审查，形成无 finding 的结论，或将发现与修复责任转入唯一 `review-*.md` 报告并在此登记入口后，本计划归档。Review 不授权新的实施、提交或推送。
