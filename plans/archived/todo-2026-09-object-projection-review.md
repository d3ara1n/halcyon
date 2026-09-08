# 历史审查清单：对象映射多段投影

> 本文件已由 `plans/archived/todo-2026-09-review-program.md` 批次 A 合并承接并归档；归档不表示审查完成。正式结论并入 `plans/review-2026-09-memory-transaction-unification.md`。

审查对象是提交，不是当前工作树。本篇记录本次任务对应的提交与改动概要，供日后 Review 对照。

## 提交范围

| 提交 | 性质 | 概要 |
|------|------|------|
| `51b3742` | 实现 | 对象映射脱离单页单 PA，走有界多段投影 |
| `0fad27f` | 实现 | 栈窗口回落到堆化 backing 的实测帧 |
| `310d089` | 收尾 | 清理死代码（extent_count、table_budget、core.identity）并同步文档 |
| `0a4eacb` | 文档 | 推翻匿名 backing 重构，主线收回对象侧真前置 |
| `81b5b3e` | 文档 | 曾冻结的（已推翻）最终形态 —— 保留供审查推导链 |

前置基座见 `6e18b8f`（admission 五类 + ObjectBacking + MemoryObjectCore）与 `16dd3b4`（Tunnel 迁移）。

## 改动概要

**对象映射多段化**（`os/kernel/src/task/proc.rs`）
- `prepare_object_mapping(va, object_offset, spans, authorization, permits)`：逐段 `preflight_map`。
- `ObjectMappingPlan` 持 `Vec<TranslationPreflight>`；`complete_object_mapping` 收 `Vec<Vec<TableFrameToken>>`、循环 `prepare`、经 `publish_batch` 发布，与匿名侧 `complete_owned_mapping` 同形。
- `ObjectMappingLease` 增 `object_offset`；`prepare_object_unmap` 复核该偏移而非假定 0。

**长度真值上收**（`os/memory_space/src/{object,space}.rs`）
- `MemoryObjectState` 持 `object_bytes`；`ObjectViewAuthorization` 携带它；`MapBacking::Object` 删除 `object_bytes` 参数。
- `MemoryObjectState::new` 签名增一参（对象长度）。

**backing 形态**（`os/kernel/src/frame.rs`）
- `ObjectBacking` 堆化持 `Vec<FundedExtent>`；`project(offset, length, &mut spans)` 写入调用方缓冲。
- 删除 `FundedBackingStorage`（含 `split_off`/`merge_from`/`projection_capacity`）。

**栈窗口**（`os/platforms/linker.ld`、`os/tools/audit_elf.py`、`os/platforms/qemu/sifive_u/memory.x`、`os/kernel/src/mm.rs`）
- STACK_GUARD 0x4000→0x3000、审计上限 0x3800→0x2800、虚拟跨度注释 2.25→2.19MiB。
- sifive_u `STACK_SIZE` 保持 0x10000（约束调用链总和，非单帧）。

## 审查关注点

1. **多段 preflight 的失败原子性**：`complete_object_mapping` 中途某段 `prepare` 失败时，已成功的 translations 随 `ReclaimedTableFrames` 回收、ledger 已 rollback、`failed_owners` 单独回传——核对与匿名侧 `complete_owned_mapping` 的回收路径是否真正等价，尤其 `funded[index]` 被 `mem::take` 后剩余段的 owner 归属。
2. **`table_outcomes` 容量预留时机**：在 `prepare` 循环之前预留，Commit 阶段 `publish_batch` 断言容量足够。确认预留失败路径未泄漏已 prepare 的 translations。
3. **`object_offset` 复核的完备性**：`prepare_object_unmap` 现比对 `region_offset == lease.object_offset`。多页 view 被部分 unmap 后 lease 是否仍成立、`ObjectMappingLease` 是否需要随之切分——当前 Tunnel 单页不触发，公共多页对象（步骤 8）必须复查。
4. **对象长度与 backing 几何的一致性**：`MemoryObjectCore::new_tunnel_connection` 用 `backing.pages() * PAGE_SIZE` 作 `object_bytes`。公共 Create 路径需保证二者同源，不得出现状态机长度与实际 backing 不符。
5. **堆化 backing 的分配失败面**：`fund_object_backing` 中 `into_extents` 失败被映射为 `FundError::Physical(OutOfMemory)`。确认此时 funding 事务的物理与 charge 已由 RAII 完整回滚。
6. **栈容量的两个独立约束**：单帧上限（audit_elf.py，卡 guard 洞跨度）与调用链总和（STACK_SIZE）是不同约束。sifive_u 保持 0xF000 formal 的理由已写入注释；审查时确认后续改动没有再把两者混为一谈。

## 已知非回归项

`virt-stress` 竞态矩阵的 `memory-vs-kill` 与 `last-thread-exit-vs-kill` 偶发失败已登记 KNOWN_ISSUES，改动前后各连续三轮对比确认与本次改动无关。审查时若遇单轮失败，先复跑再判定。
