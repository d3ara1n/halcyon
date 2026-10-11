# 验收运行整理交付独立 Review

状态：**已登记，待执行。** 本篇固定已提交交付的审查范围，不表示独立审查通过，也不重新启动清理施工。

## 固定目标与范围

- 目标提交：`a06e455d064429d2e6e9984b03474a62104fb9e2`（`perf(acceptance): 聚焦缺页验收并优化平台运行组合`）。
- 父提交：`985fb179a7455d0b251c306007f92d4e8d64b5ce`；审查两者间的完整交付及实际依赖。
- 范围：Justfile 的 workload、构建档位和聚合分组；srv_init 的 memory 分支、既有线程/内存消费者及收束；QEMU/GDB 验收判定；FS1 输出和重复标记清理；相关手册、impls 与归档记录。
- 非目标：重设计内核内存、线程或监督机制，删除仍有独立覆盖的 fixture/自检，补齐历史精确故障注入。若发现本次改动引入跨边界问题，追到真实责任 owner。

## 审查重点

1. memory 是否复用同一正式目标并观察真实 RetryFault；GDB detach 后是否要求目标 Dead/Completed、Job/辅助能力收束和显式 reset，是否存在提前收割或假通过。
2. 新分支的映像借用、Pool、Job 与 RootSupervisor 责任；正常、失败、关闭失败和目标消散时是否保留原监督与资源 owner。
3. core/stress/hetero/nofd 的独立 required 是否保留；memory 的专属锚点是否匹配实际消费者；失败文本、退出码、终态收割和完整日志策略是否一致。
4. 平台默认 release 与显式 debug、八面 clippy、聚合三组的覆盖边界；无调试器 stress 是否独立保留，配置和打包是否可能使用错误档位或缺少 hammer 映像。
5. Copy 汇总删除与 FAL2→FS1 改名是否仅消除重复表示，源码输出和脚本同步；文档是否准确区分完整业务、平台观察、受限诊断与历史证据。

## 现有证据与边界

[实施档案](archived/todo-2026-09-23-acceptance-fixture-cleanup.md#实施验证与交付检查)拥有设计、保留理由和交付前检查。本机证据在 `artifacts/acceptance-cleanup/summary.json`、`results.json`、`policy.json`、`wrapper-checks.json` 与各完整日志：

- `just check`、八面 clippy、一次完整 `just acceptance`（392.79秒含构建）、额外 release hetero（34.84秒含构建）通过。
- 无调试器 debug stress 保留16/16矩阵；memory 实际观察发布窗口，完成目标、Job与辅助能力收束和 reset。
- core/stress/hetero/nofd 的 required 集合仅同义改名并删除一条重复 Copy 汇总；rejected/终态/日志策略逐字不变。真实 memory 日志回放证明完整输入通过、缺收束标记/错误profile/追加panic被拒绝，回放不是新QEMU运行。
- 平台实际验证为 release；显式 debug 仅核对命令展开，不算新运行。host 逻辑库未改，没有重复跑全套 host 单测；clippy 覆盖host测试目标。
- 392.79与原留存784.55秒的比较包含构建，不是严格性能基准；GDB停顿也不构成时序或公平性证据。

交付前只读结构检查不替代本篇独立 Review。既有 Watch 未知错误和精确来源/映射/迟到Bind窗口仍由[原所属计划](todo-2026-10-07-fal-service-redesign.md)拥有，不因验收优化或普通运行通过关闭。

## 执行与报告

按 [REVIEW](REVIEW.md) 使用新上下文只读审查。先核源码、覆盖映射和现有证据，仅为新发现的具体风险追加必要验证；不自动重跑所有路线。

形成 `review-<日期>-acceptance-cleanup.md` 后，由报告唯一承接 findings、修复和复核，本篇登记报告入口并归档。登记本身不启动审查或新施工。
