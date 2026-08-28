# 线程资源模型批一 Review 计划

> 【未来审查计划】对象是批一提交 `794a4c0`；Review 纪律见 [`REVIEW.md`](REVIEW.md)。设计决策（D1–D6 + tid + 否决方案五条）与完整推导见 [`todo-2026-09-thread-model.md`](todo-2026-09-thread-model.md)（本篇是其 review 轴，不重复计划内容）。

## 提交对照

| 提交 | 内容 |
|---|---|
| `794a4c0` | feat(task): 线程资源模型批一——Start 拆解为 Attach/Grant/Start 三 op。shared ABI（调用号 0x1d/0x1e、AttachDescriptor、cap 常量）；lifecycle（Staging 携带 Arc、attach_member 闭包事务、begin_running 原子提取、终止游标 take_first_staging）；内核三 op（attach/grant/start 纯化、出生块内核机制删除、requirement 上移 Process、bootstrap 内嵌同构序列）；handle_table pin 泛化（builder/grant 皆可选）；用户态（rinlib 封装、libprocess 新组装序列与出生块自构造、init/hammer 负载适配）；AGENTS「重构即收益」原则；notes 四篇同步 |

## Review 轴

### 事务正确性（对照计划「内核机制」节）

- **attach**：闭包内 `Arc::try_new` 取 HEAP 锁（LIFECYCLE→HEAP 合法秩）——验证失败路径零副作用（tid 未消费、表未插、强引用未构造）；validate_initial_context 在 enter_building_op 之后，leave 配平无遗漏。
- **grant**：交付前置序（reserve → 输出写回校验 → commit）；失败路径 unpin + rollback 全还原，调用者句柄可重试；成功即目标表可见。MAX_START_GRANTS 上界与调用者侧 grants[..len] 截断的一致性。
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

### 未决问题（review 会话继续调查）

**sifive_u 竞态矩阵 kill-vs-exit 确定性卡死**（virt/virt-release/hetero/nofd 全过）：

- 症状：round 0 spawn target 成功（探针至 started）、`fire_race` 后 `h.report(0)` 永不到达；QEMU CPU 采样 142%（50% 节流下）≈ 2.5 核自旋——内核自旋死锁特征，非等待丢失。
- 已排除：spawn 全步骤（mapping/writing/stack/grant/birth block/attach/startarted 全打印）、锤 kill 路径（kill-vs-kill 4 轮全过）、锤 report 路径（探针 report ready status 0 反复出现）。
- 疑点优先级：① sifive_u 5 核（vs virt 4 核）下 IPI 位图/门铃路径——`ipi_slots` 组装（u64 位图 slot ≤ 63 假设在 5 核下的行为）、`registry::ipi_slots` 展开、wake_one 的 idle_mask 载入序；② kill-vs-exit 双侧冻结竞争（target Exit syscall 与异 hart Kill 的 request_termination 竞争）在某时序下自旋；③ 142% 是否节流伪影（throttle 脚本 50% 下单核满转应显 ~50%，2.5 核说明真自旋）。
- 装备：`just sifive_u-gdb`（GDB 宏 dump_harts 定位各 hart PC）；`tools/qemu-acceptance.sh --allow-timeout`；init race.rs 探针已移除（调查时重加）。
- 注意：探针实验显示卡点会随后移（先卡 spawn 中段、后卡 report）——不是固定 PC 的死循环，更像**条件自旋**（锁自旋/IPI 等待循环），怀疑静态 PC 采样需多采样点或用 GDB break 在 sched.rs 调度循环。

### 既有回归面

- kill-vs-start 场景新语义：attach 由 init 组装侧完成，锤只拉 Start——竞速点仍是 Building→Running 线性化（virt 已过，语义等价论证）。
- seal gate (start) 场景：seal 后 Start 拒绝路径的 leave_building_op 配平。
- 帧守恒：静默停机帧数对比（virt 248842 free / 释放前后差值应与改动前一致量级）。
