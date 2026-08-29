# 线程资源模型批一 Review 计划

> 【未来审查计划】对象是批一提交 `794a4c0`；Review 纪律见 [`REVIEW.md`](REVIEW.md)。设计决策（D1–D6 + tid + 否决方案五条）与完整推导见 [`todo-2026-09-thread-model.md`](todo-2026-09-thread-model.md)（本篇是其 review 轴，不重复计划内容）。

## 提交对照

| 提交 | 内容 |
|---|---|
| `794a4c0` | feat(task): 线程资源模型批一——Start 拆解为 Attach/Grant/Start 三 op。shared ABI（调用号 0x1d/0x1e、AttachDescriptor、cap 常量）；lifecycle（Staging 携带 Arc、attach_member 闭包事务、begin_running 原子提取、终止游标 take_first_staging）；内核三 op（attach/grant/start 纯化、出生块内核机制删除、requirement 上移 Process、bootstrap 内嵌同构序列）；handle_table pin 泛化（builder/grant 皆可选）；用户态（rinlib 封装、libprocess 新组装序列与出生块自构造、init/hammer 负载适配）；AGENTS「重构即收益」原则；notes 四篇同步 |

## Review 轴

### 事务正确性（对照计划「内核机制」节）

- **attach**：闭包内 `Arc::try_new` 取 HEAP 锁（LIFECYCLE→HEAP 合法秩）——验证失败路径零副作用（tid 未消费、表未插、强引用未构造）；validate_initial_context 在 enter_building_op 之后，leave 配平无遗漏。
- **grant**：交付前置序（reserve → 输出写回校验 → commit）；失败路径 unpin + rollback 全还原，调用者句柄可重试；成功即目标表可见。MAX_START_GRANTS 上界与调用者侧超限报错（`SpawnError::TooManyGrants`，不截断）的一致性。
- **start**：活体门（count==0 → ObjectNotAvailable）在 reserve 之后、gate 之前——空表与并发 attach 插队的 ObjectBusy/StaleCount 语义；pin builder（Some）/pin grants（None）两个泛化用法；提交区不可失败论证（reservations/staged 容量全部前置预留）。
- **begin_running(expected, out)**：活体门 + 计数一致性 + Staging→Running 状态转换 + 强引用交出在同 lifecycle 临界区原子完成；并发 attach 在 gate 前插队 → StaleCount；kill 游标在 gate 后触达 → 表内已无 Staging（计划「实施决策」2 的竞态修复）。

### Staging 强引用环

- 环的存在域论证：Staging Arc ↔ Thread.process 仅 Building 期；Terminating 后 take_first_staging 游标必然打破（REAPABLE 合取含表空）。
- 游标驱动方：run_termination_todo 的调用点覆盖（kill/abandon/fault/exit 全路径）——验证无路径在终止后遗留 Staging。
- building_ops 与 Staging 的交互：enter_building_op 拒绝后 attach 返回 ObjectClosed，无半插入。

### ABI 与两侧同步

- shared 调用号 0x1d/0x1e 与文档注释；ProcessStartDescriptor 删除后无残留引用（全树 grep）；AttachDescriptor 布局断言（32B）；出生块构造函数归属用户态的注释改述。
- rinlib/libprocess/init/hammer 四处调用的语义等价（grants 空 → grant 调用跳过；出生块放置 image_top 页对齐约定）。

### bootstrap 内嵌序列

- launch_bootstrap 与 start_staged 的结构同构性（同 reserve/commit 序列、同 begin_running 原子语义）；boot fatal 断言的适用性（无并发、无回滚）。
- map_bootstrap_block 的 payload 收编特例保留论证（计划 D6c：出生块用户化但 bootstrap payload 收编保留在内核内嵌 Write）。

### 未决问题（已按批一报告 §B4 挂起）

**sifive_u「确定性卡死」已证伪（batch1 review 报告 §B）**：55+ 轮（HEAD/节流档/gdbstub/多配置）零真挂死；唯一确凿现场属于未提交中间态代码。全部可观测失败由负载缺陷（wfi → U-mode IllegalInstruction，kill-vs-start 靶首条指令即 fault）与验收基础设施超时误杀（`run_qemu_acceptance_timed` 5s 硬超时）解释。挂死根因未定性（无现场），复现装备留档：
  - 判据：hang-hunt 模式（日志停增 + 无终态 + QEMU 存活）+ `qemu -s` gdbstub + `riscv64-elf-gdb thread apply all bt`；
  - gdbstub 会改变时序，挂死复现概率可能下降（记录在案）；
  - 若再次复现：优先 GDB 多采样（条件自旋的静态 PC 需多样本），次选在 `send_ipi` 与 `idle()` wfi 醒来处加计数探针（探针已在批一提交移除，重加见 git log 794a4c0 前版本）。

### 既有回归面

- kill-vs-start 场景新语义：attach 由 init 组装侧完成，锤只拉 Start——竞速点仍是 Building→Running 线性化（virt 已过，语义等价论证）。靶入口指令已由 wfi 改为自旋（`j .`），避开 U-mode 特权指令 fault 污染终因；处理记录见批一报告 A1。
- seal gate (start) 场景：seal 后 Start 拒绝路径的 leave_building_op 配平；建 Building 已与 race.rs 共用 `build_spin_building`（失败自清理）。
- 帧守恒：静默停机帧数对比（virt 基线 248842；探针移除后为 248839，差 3 帧已定因）。
