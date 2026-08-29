# 线程资源模型批一 Review 计划

> 状态：**已执行并收口**。对象覆盖批一提交 `794a4c0`、文档提交 `275e4f1` 与首轮修复 `51f4184`；首轮报告见 [`review-2026-08-29-thread-model-batch1.md`](review-2026-08-29-thread-model-batch1.md)，后续事务复审见 [`review-2026-08-29-thread-model-transaction-convergence.md`](review-2026-08-29-thread-model-transaction-convergence.md)。Review 纪律见 [`REVIEW.md`](../REVIEW.md)，设计决策见 [`todo-2026-09-thread-model.md`](../todo-2026-09-thread-model.md)。

## 提交对照

| 提交 | 内容 |
|---|---|
| `794a4c0` / `275e4f1` / `51f4184` | Start 拆解为 Attach/Grant/Start 三 op、方向与实现入档，以及首轮组装语义和验收基础设施修复；后续事务复审补齐原子批量 Ready、BuildingLease、capability consume/transfer、execution binding、公共线程出生路径与用户态失败所有权。 |

## Review 轴

### 事务正确性（对照计划「内核机制」节）

- **attach**：`BuildingLease` 统一准入登记；`Process::attach_thread` 集中现场校验、tid 分配与 Thread 构造，`attach_member` 在 lifecycle 临界区原子插入，失败零副作用。
- **grant**：受保护 builder + transfer grants 的单一 pin 事务；交付前置序（reserve → 输出写回校验 → commit）；失败路径 unpin + rollback 全还原，成功只消费 grants 并恢复 builder。检查 shared `PROCESS_MAX_GRANTS`、调用者侧 `TooManyGrants` 与 `SpawnFailure::grants`（Retained/Consumed）的所有权一致性。
- **start**：活体门、并发 Attach 导致的 expected 失配与 ObjectBusy 语义；`pin_consume` 独占 builder；Ready 完整批次在一次队列锁内 reserve，提交区只剩不可失败的 execution binding、builder consume 与 batch commit。
- **begin_running(expected, out)**：活体门 + 计数一致性 + Staging→Running 状态转换 + 强引用交出在同 lifecycle 临界区原子完成；并发 attach 在 gate 前插队 → StaleCount；kill 游标在 gate 后触达 → 表内已无 Staging（计划「实施决策」2 的竞态修复）。

### Staging 强引用环

- 环的存在域论证：Staging Arc ↔ Thread.process 仅 Building 期；Terminating 后 take_first_staging 游标必然打破（REAPABLE 合取含表空）。
- 游标驱动方：run_termination_todo 的调用点覆盖（kill/abandon/fault/exit 全路径）——验证无路径在终止后遗留 Staging。
- building_ops 与 Staging 的交互：enter_building_op 拒绝后 attach 返回 ObjectClosed，无半插入。

### ABI 与两侧同步

- shared 调用号 0x1d/0x1e 与文档注释；ProcessStartDescriptor/ProcessAttachDescriptor 无残留引用；`ThreadStartContext` 由 Attach/Spawn 共用并保持 32 B 布局；出生块构造归用户态。
- rinlib/libprocess/init/hammer 四处调用的语义等价（grants 空 → grant 调用跳过；出生块放置 image_top 页对齐约定）。

### bootstrap 内嵌序列

- launch_bootstrap 与普通组装共用 `Process::attach_thread` 和 execution binding 编码；boot 路径保留无并发、失败即 fatal 的内嵌提交序列。
- map_bootstrap_block 的 payload 收编特例保留论证（计划 D6c：出生块用户化但 bootstrap payload 收编保留在内核内嵌 Write）。

### 独立未决问题

首轮报告中“sifive_u 确定性卡死”的判断已证伪；后续发现的低频提前 quiescent 有独立现场，但不能据现有证据判定与旧现场同源或由批一引入。调查、装备与完成标准统一由 [`todo-2026-08-29-early-quiescent-shutdown.md`](../todo-2026-08-29-early-quiescent-shutdown.md) 跟踪，不留在已完成 review 计划中。

### 既有回归面

- kill-vs-start 场景新语义：attach 由 init 组装侧完成，锤只拉 Start——竞速点仍是 Building→Running 线性化（virt 已过，语义等价论证）。靶入口指令已由 wfi 改为自旋（`j .`），避开 U-mode 特权指令 fault 污染终因；处理记录见批一报告 A1。
- seal gate (start) 场景：seal 后 Start 拒绝路径的 leave_building_op 配平；建 Building 已与 race.rs 共用 `build_spin_building`（失败自清理）。
- 资源守恒：所有 acceptance 域均完成 Drain 并命中终态锚点；精确空闲帧数会随 init 常驻映像变化，不以跨构建的固定绝对值替代完整服务收束证据。
