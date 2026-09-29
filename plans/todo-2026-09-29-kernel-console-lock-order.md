# Kernel console 锁序 panic

## 状态

独立待处理任务。由运行时分层第二轮复核发现，尚未进入本轮 runtime layering 施工范围。

## 现象与证据

`just acceptance`、`just virt` 和 `just virt-stress` 在本轮 runtime 改动后均能完成内核自检、初始进程启动、服务拓扑建立以及 FAL2 中段检查，随后触发 debug kernel 锁序断言：

- 断言位置：`os/kernel/src/sync.rs:172`
- 请求位置：`os/kernel/src/console.rs:37`
- 现象：在持有 `Rank(220)` 的上下文中获取 `Rank(150)` 的 console 锁
- 验收结果：脚本按 failure-without-progress 以退出码 124 收束
- 复现日志：
  - `artifacts/dev/runtime-layering-round2-just-acceptance.log`
  - `artifacts/dev/runtime-layering-round2-virt-core.log`
  - `artifacts/dev/runtime-layering-round2-virt-stress.log`

本轮 runtime 改动未修改 kernel 锁实现；日志证明用户态启动采用、服务拓扑和 FAL2 已越过原先的启动停滞点，但不能据此把 kernel panic 归因于 runtime layering 已闭合。

## 范围

### 目标

1. 固定触发该锁序违规的第一调用链、持锁集合和 rank key。
2. 判断违规来自正常日志路径、panic/fatal 路径、跨 hart 输出交错，还是验收负载的调用顺序。
3. 在不绕过 Lock Ladder 断言、不把日志失败转成静默的前提下，修正 console 输出责任链或调用方锁边界。
4. 以 core、stress 和必要的 release/platform 路线重新验证，并更新 kernel 相关实现说明。

### 非目标

- 不修改 runtime layering 的 StartupSeed、ThreadRegistry、ResultCell 或 Unmap owner 责任链。
- 不把 panic 日志改成无条件忽略锁序错误。
- 不以增大验收超时替代定位或修复。
- 不在本计划中重做全局 Lock Ladder；发现跨专题锁序缺口时另立任务。

## 前置与未知项

- 需要固定与复现该现象相同的 kernel/debug ELF 和 QEMU 路线，避免跨构建比较地址。
- 需要先读取 `plans/DEBUG-PLAYBOOK.md` 与 `plans/TOOLING-PITFALLS.md`，按第一坏事件而不是最终超时定位。
- 当前只知道 console lock 请求点和 rank 违规，不知道外层 `Rank(220)` 锁的真实 owner、是否为正常日志调用或异常路径，也不知道违规是否在基线 `66f4e9c` 可复现。
- 需要用基线和本轮构建各至少一次复现对照；若基线零样本，应重新审查归因。

## 调查顺序

1. 固定 workload、QEMU 参数、kernel ELF 和完整串口日志，清理残留 QEMU。
2. 对 `console_write` 的所有调用点建立持锁上下文表，标出 `Rank(220)` 的具体锁与调用链。
3. 用低成本日志/调用探针或 panic 前后事件窗口确定第一笔违规获取；必要时使用 DEBUG-PLAYBOOK 的 A/B 或执行环窗方法。
4. 依据第一调用链决定修复归属：
   - 调整调用方锁外输出；
   - 调整 console/panic 输出的锁路径；
   - 或修正文档/测试对合法日志路径的契约。
5. 更新受影响的 `notes/impls` 唯一拥有篇；若锁序长期契约改变，同步对应 `notes/ideas`。
6. 执行静态检查和最小复现，再执行构建手册要求的完整验收组合。

## 完成门

- 第一违规获取点、外层锁 owner、rank key 和可达调用链有源码/日志证据。
- 修复不绕过锁序断言，不遗失 console/panic 输出责任，也不引入新的反向锁序。
- 基线对照和修复后复现结果均记录，至少 core 与 stress 路线不再触发该 panic。
- `just check`、`just clippy` 及适用的 acceptance/platform 路线通过；完整日志放在 `artifacts/dev/`。
- 受影响的 `notes/impls`/`notes/ideas` 与 `COMPASS` 状态准确，计划关闭前不宣称 runtime layering acceptance 已通过。

## 关联入口

- [运行时分层第二轮复核](review-2026-09-28-runtime-layering.md)
- [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md)
- [构建与验证手册](BUILD-AND-TEST.md)
