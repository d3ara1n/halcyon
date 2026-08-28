# Job 管理面实施入口（step 5）

- 状态：**已实施收口（2026-08-27）**——四条验收线全过（枚举派生 kill
  通路、封口与完成传播、派生兑底、seal 闸门与枚举收敛），virt ×6 /
  sifive_u / host 单测全绿；实现现状见 notes/impls/task.md「Job 管理面」。
- 角色分工：设计与决策权在设计方（用户 + 设计会话）；实施侧遇到设计缺口、文档冲突或不可实施点，**停下向用户回报，不自行拍板、不降级实现**。

## 权威文档（按优先序，冲突时以先者为准）

1. [`todo-2026-08-26-process-lifecycle.md`](todo-2026-08-26-process-lifecycle.md)：「已拍板的结构决策（第二批：Job 管理面）」决策 9–15 +「已确认的 Job ABI」——**设计的最终形态**；
2. [`archived/todo-2026-08-27-job-management-design.md`](todo-2026-08-27-job-management-design.md)：完整推导、被否选项、实施约束（数据结构/锁序/游标闭合）、竞态闭合清单；
3. [`ref-2026-08-27-job-enumerate-derive-research.md`](../ref-2026-08-27-job-enumerate-derive-research.md)：Zircon/Windows/seL4/Linux 契约取证（引用外部先例时用）；
4. `notes/ideas/task.md`「Job」节：概念层（含预算原则——**注意预算是远期方向，本次不实施**）。

## 实施边界

**做**：

- `shared/`：调用号 0x19–0x1c、`JobSnapshot`(40B)/`JobEnumerateResult`(16B)、`JOB_ENUMERATE_MAX` 常量、编译期尺寸断言（对齐 Process ABI 风格）；
- `os/kernel/`：JobState 重构（members/children 改按 ID 有序映射，`Vec + swap_remove` 退役）；JobId 分配器（单调不复用，root 恒 1，锁内分配）；Pid 分配挪入 owner Job 锁内与占位同临界区；JobSeal/JobQuery/JobEnumerate/JobDerive 四个 syscall；ancestor seal 上行检查（JobCreate/ProcessCreate/ProcessStart 提交点，先父后子链锁线性化）；完成传播（三触发点、放子锁后取父锁的延迟触发）；Sealed+empty 的 CLOSED 发布；root Job static anchor 特例；
- `user/rinlib/`：四个 syscall 的 wrapper 与用户态校验（判别值范围、reserved 为零、`more=1 ∧ actual=0 ∧ next_cursor ≠ 入参 cursor` 违约拒绝——占位屏障零进展例外见 Job ABI 契约）；
- `user/frameworks/libprocess/`：JobKill 组合的公共实现（逐层 JobSeal → 枚举 → 派生 kill → drain → 等 CLOSED）；
- `user/systems/init/`：验收钩子（见验收线）；
- 文档同步：`notes/impls/task.md` 补 Job 管理面与锁序规范「Job 链锁（先父后子）→ lifecycle 锁 → 对象锁」（step 10 的此部分提前）。

**不做**（明确划出）：pm 服务与 services Job 编排（step 6）；ThreadSpawn 与多线程屏障（step 7）；调度域 eligibility 与 D64（step 8）；MemoryObject / CPU 预约 / 预算机制（远期方向，仅记录于 ideas，不写代码）。

## 验收线（建议场景，实施中可提调整但需回报）

1. **枚举→派生→kill 通路**：virt 上 init 经 `JobEnumerate`+`JobDerive` 取得 srv_target 的 ProcessControl 并 kill（替代或并行于现有保留 control 路径），验收 Dead/Killed；
2. **封口与完成传播**：scratch child Job 在成员收束后 `JobSeal` → CLOSED 电平可等待；
3. **派生兜底**：control 消散的 REAPABLE 进程经枚举+派生接管，drain 至 Complete；
4. **竞态可行子集**：枚举 vs 并发 Create/Dead 移除、seal vs Start 提交（多核 virt 下可制造的子集）；
5. 既有回归面不破：host 75 单测、virt ×6、sifive_u。

## 验证纪律

按 AGENTS.md：`just check`、`cd shared && cargo check`、host 单测显式 host target；`just virt` 先构建后运行、运行超时 2s（节流 4s）；判定看启动日志关键行，QEMU 退出即通过。

## 关键代码入口（省探索成本）

- 内核：`os/kernel/src/task/job.rs`（JobState/事务 marker/root anchor/PID 分配器现状）、`task/process.rs`（drain/publish 序）、`task/lifecycle.rs`（锁序契约头注释）、`syscall.rs`（分派）；
- ABI：`shared/src/call.rs`（调用号）、`shared/src/object.rs`（Rights/ObjectSignals）、`shared/src/proc.rs`（Pid/结构体风格）；
- 用户态：`user/rinlib/src/process.rs`（Process ABI 校验风格）、`user/frameworks/libprocess/src/lib.rs`（spawn/drain helper 现状）、`user/systems/init/src/main.rs`（step 4 监督闭环——验收钩子挂点）。

## 收口动作

实施完成、全部验收过线后：本文件归档；主线计划 step 5 划线收口；COMPASS 同步。若实施中产生新的设计结论（而非既有设计的展开），先回设计方拍板再入档。
