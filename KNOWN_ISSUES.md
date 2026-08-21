# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## formal-entry 竞态（临时缓解在位）

`hart_formal_entry` 入口处的一次单块 DBCN 写入（rt.rs 中 `println!("DBG: formal enter")`）
是承重的：移除它，virt 4 核 3/3 复现 formal-entry 后无输出悬挂（无 fatal、无超时报告，
boot hart 未到达 publish_online）；改为 log_topic 的多块写入序列同样悬挂；逐字保留则稳定通过。

已知事实：

- 悬挂点在 `formal_entry_baseline` 返回前后、`publish_online` 之前；不经过任何 trap 路径
  （`_bootstrap_fatal` 与 `_trap_entry` 均未触发，legacy putchar 直写标记可证）。
- sifive_u 上同一位置悬挂，且探测恢复路径已确认执行到尾部（'R' 标记），
  内联汇编 clobber 补全后能走到基线末尾。
- 探测本身非必要条件：跳过 senvcfg 探测块仍悬挂。
- gdb/monitor 在 sifive_u 上只能看到 harts[0]，U54 hart 不可观测；
  `-d exec` 日志中内核地址的 TB 从未出现（与实际执行矛盾，原因未知）。

调查方向：formal entry 的 CSR 编程序列与 OpenSBI（M-mode）之间的同步缺口；
优先怀疑 set_timer ecall / UXL 的 csrw sstatus 序列在无延迟情况下与固件的交互。
修复前不得移除该打印或改写其分块形态。

## qemu sifive_u 用户态随机停滞

四服务负载在 `sifive_u` 5 核调试构建下仍可能无法于运行阶段 3 秒内全部完成，停止位置随机；同期 `virt` 4 核完整通过。故障样本缺少部分服务的退出回收以及最终的 `[Sched] 系统静默`；这与系统已经静默、仅因平台没有 shutdown device 而不退出 QEMU 是两种情况，后者不算故障。

QEMU monitor 样本显示未完成进程仍在用户态分配器/原子路径执行，PC 会变化，未见内核 trap，不能归类为固定服务故障或平台关机限制。trap/上下文、hart 身份、能力调度与启动发布的已知契约缺口记录在 `plans/reviews/system-audit/01-sbi.md`、`02-trap-context.md`，统一设计见 `notes/execution-context.md`；它们是系统级重构输入，不构成该随机停滞的直接归因。在取得直接证据前不得添加平台专用补丁。复现与超时规则见 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
