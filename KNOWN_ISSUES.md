# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## sifive_u 非确定性内核野跳转

四服务负载在 sifive_u 5 核下能全部装载、运行并回收，静默判定与 SRST
停机在多数轮次收敛（2026-09 地基工程后；此前的两大根因——IDLE_MASK
slot/raw 混用、Extend 单位错配引发的 OOM 自锁——已修复，见
`plans/2026-09-pre-ipc-groundwork.md`）。

剩余问题：约 1/3 轮次出现 S 态致命 trap——**cause=0xc（取指页故障）、
stval=0x0、sepc=0x0** 的内核野跳转，所在 hart 停驻进而阻塞静默收敛。
完整 GPR 转储可由 fatal 路径取得，直接证据已在手，待专项归因。历史
启动期成因（过渡页表并发清零竞态）已于 2026-08 修复，见
`plans/2026-08-execution-context-stall.md`。

trap/上下文、hart 身份、能力调度与启动发布的已知契约缺口记录在
`plans/reviews/system-audit/01-sbi.md`、`02-trap-context.md`，统一设计见
`notes/execution-context.md`。在取得直接证据前不得添加平台专用补丁。
`just sifive_u` 已内置运行阶段超时收束，通过与否看日志关键行。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
