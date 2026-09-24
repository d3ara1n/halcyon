# 验收 fixture 与启动期自检逐步清理

> 状态：进行中；启动包映像已零复制借用，首批清理和验收判定收敛已验证。消费者矩阵已核对；`test_target`/`test_hammer` 的独立覆盖尚无等价替代，暂不删除。

## 目标

逐步审查并清理 `test_target`、`test_hammer`、内核启动期自检和服务内验收残留，减少启动期资源消耗、启动包体积、验收拓扑和长期维护面。删除须核对原覆盖契约；已有正式消费者或更小的既有验收路径能够观察目标行为时可直接清理，独立失败窗口尚无替代观察点时暂留。不能用删除 anchor 冒充验证完成，不把测试编入内核或变成正式成员作为长期目标。

## 当前事实

- `user/tests/test_target` 当前被 `srv_init` 启动并保留映像；core acceptance 使用它做 Job 枚举、派生 MANAGE control、kill/supervision、IPC committed-kill 和 Job 管理验证；stress race matrix 也复用其映像。
- `user/tests/test_hammer` 是 stress-only 竞态矩阵的并发创建/收束靶子；init 在 stress 配置下借用并检查其映像存在。
- `srv_init` 原先将两份 ELF 复制进 init 堆，以便在启动包遍历结束后重复 spawn；现改为借用启动包只读切片，sifive_u 原 `Heap allocation error, layout = 5001400` 已消除，完整 acceptance 通过。
- `Justfile`、`user/Cargo.toml`、BootPackage 装配、RequiredLaunchSet、acceptance anchor 和 plans/notes 均可能持有这些 fixture 的重复真值，清理时必须统一收口。
- 内核堆自检在 `frame::init` 后主动分配 8192 个 `u32`，迫使全局堆领取不可退回的 system ticket；后续正式启动本身使用该堆，QEMU 验收不依赖自检锚点。
- `srv_pm` 启动前的两次 `sys_sleep` 共延迟 40 ms，只验证 Sleep 唤醒；init 的公共时间验收已通过同一底层调用检查绝对期限及唤醒。
- 竞态测试指令中的 `entry`/`sp` 一直为零，hammer 侧 START 固定使用 Base64 profile；两个字段不提供任何竞态观察。

## 非目标

- 不在本记录中改动 FAL provider、Registry、Dispatcher 或进程生命周期机制。
- 不为了让某个平台 acceptance 通过而删除其测试覆盖；先建立替代覆盖和独立证据。
- 不直接清理 `artifacts/` 历史日志；仅在源码与计划不再引用旧路径后按构建规则处理生成物。

## 施工顺序

1. **盘点消费者与覆盖契约**：列出每个 fixture 的正常、失败、取消、跨域、监督和资源收束覆盖，标明唯一 owner、启动条件、stress/core/platform 分面和可替代的正式消费者。
2. **评估替代路径**：优先复用现有真实服务或更小的已有验收进程；若无法保持独立观察点，保留 fixture 并压缩其映像保留方式，记录理由，不引入测试专用新服务。
3. **删除或缩减单一 fixture**：同步移除 Cargo/Justfile/BootPackage/RequiredLaunchSet/init retention/acceptance anchor/文档引用；删除后不得留下第二套启动清单或隐式路径。
4. **分层验证**：先包级构建与 host 检查，再 core、stress、sifive_u、virt-nofd、boot-failure 等受影响路线；逐项核对原覆盖契约和最终账户归零。
5. **结构收口**：更新 COMPASS 与本记录状态，关闭已删除 fixture 条目；若替代路径不足，记录为保留项而不是强行删除。

## 已实施

- `shared/tar::walk` 显式保留输入 slice 生命周期；`srv_init` 的 `target_image`/`hammer_image` 直接借用启动包切片，不再复制 ELF 到 init 堆。
- 当前不删除两个 binary：`test_target` 仍支撑 Job/IPC/监督验收，`test_hammer` 仍支撑 stress race matrix；删除前必须完成消费者矩阵和替代覆盖审计。
- 首批清理：移除内核堆启动自检及入口、仅供自检使用的 `frame::remaining_heap_chunks` 包装、`srv_pm` 的两次启动 Sleep；竞态指令由 5 个 word 缩为 3 个，删除两侧无效字段与冗余 START helper。

## 消费者与覆盖审计

| 映像 / 装配 owner | 现有独立观察点 | 删除或替代条件 |
|---|---|---|
| `test_target` / `srv_init::launch_test_services` | core 首实例进入 acceptance Job，验证枚举、派生 control、kill 与预算耗尽后的监督续接；同一映像在 pm_domain 中以无保留 control 的成员验证 pm 派生接管。`public_ipc::committed_kill` 用其 `ipc-kill` 模式制造跨线程 Close/Drain 与外部 Kill；Job 组合使用普通模式和 `retirement` 模式的 256 项 WaitSet。stress 矩阵中的双 Drain 与最后 control 消散另需独立进程。 | 只有替代进程保留相同的 Job 归属、启动 payload、独立控制权消散、IPC 并发与退休窗口，且 core/stress 锚点仍能分别判定时才删；复用服务进程的正常退出不等价。 |
| `test_hammer` / `srv_init::race_matrix` | stress-only 双锤通过 Mailbox/Notification 同刻执行 syscall；同一映像的 TARGET 模式提供自灭、fault、park、线程与映射 churn、Tunnel 退出等独立地址空间。16 个场景均使用双锤或该映像的 TARGET 模式；双 Drain/最后 control 还额外使用 `test_target`。 | 必须维持双执行点和独立地址空间的竞争/故障窗口，以及 16/16 逐场景断言和收束；改为 init 内串行调用或常驻服务不等价。 |

启动包只按 workload 装入映像：`test_target` 始终由 `Justfile::make_initfs` 打包，并在 `RequiredLaunchSet` 中要求存在且成功启动首实例及 pm_domain 实例；`test_hammer` 仅在 stress 打包，在 init 中只保留映像、不作为常驻服务 spawn，stress 配置单独要求它存在。两个 `target_image`/`hammer_image` 切片借用 init 的只读 StartupBlock payload；该 backing 由 init 地址空间的 root PoolBinding 支付，随地址空间退休归还，不是可提前回收的 init 堆副本。`tools/qemu-acceptance.sh` 检查 committed-kill、监督预算续接等 core 锚点，以及 stress 16/16、256 项 WaitSet 与 Tunnel close/Attach 锚点；单个 Job 子项目前多为仅记日志，不能把聚合通过误述为每个子项都被脚本独立断言。

验收判定收敛：`test_derive_kill` 的枚举/派生失败仍用保留 control 收束靶进程，但最后返回错误，不再将降级当作通过；`tools/qemu-acceptance.sh` 把 Job 子项既有的 ` FAILED` 日志判为失败。两个 fixture 的独立进程与并发覆盖保持不变。

## 首批验证

- `just check`、`just clippy` 七个分面和 `git diff --check` 通过；删除自检后暴露的死代码包装已清除，再次检查通过。
- `just virt`、`just virt-stress`、`just virt-release`、`just sifive_u`、`just virt-nofd` 均完成既有验收，stress 竞态矩阵为 16/16；sifive_u 以明确 reset 失败后的预期收割结束。
- `just virt-boot-failure` 的 panic、alloc、fatal 三种注入均确认四 hart 在 Failed 后停驻。各 QEMU/GDB 进程已退出；本批未运行与改动无关的 host 纯逻辑测试，亦未重复执行已通过的聚合路线。
- 判定收敛后 `bash -n tools/qemu-acceptance.sh`、七面 `just clippy` 与完整 `just acceptance` 通过，debug stress 16/16、release core、sifive_u、virt-nofd 和 boot-failure 原有锚点保持；没有单独注入 Job 子项失败日志的自动化用例。

## 完成门

- 每个被删除或保留的 fixture 都有唯一理由、owner、替代覆盖或保留条件。
- 删除后无旧 binary、旧启动清单、旧 anchor、旧文档真值残留；启动包和 init 堆占用可核对。
- core/stress 及所有受影响平台路线通过，验收日志不再依赖已删除 fixture 的 anchor。
- 生成物和临时进程按项目规则清理；工作树中的清理范围可独立审查。

## 下一步

保留两个 fixture 直到上述独立窗口有等价消费者；下一批逐项审查内核其余自检中时钟回退、WaitSet 交错及 Tunnel 堆耗尽等窗口，不整批删除。零复制借用已经解除当前平台内存阻塞，但不替代 fixture 生命周期清理。
