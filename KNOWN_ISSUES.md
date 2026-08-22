# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## qemu sifive_u 静默判定不收敛

四服务负载在 `sifive_u` 5 核下能全部装载、运行并回收（4 hart Online、四个
pid 全部 reap），但 `[Sched] 系统静默` 判定不触发，QEMU 停不下来（该平台本
就无 shutdown 设备）。历史上更严重的「服务未完成即随机停滞」样本，其启动期
成因（过渡页表并发清零竞态）已于 2026-08 修复，见
`plans/2026-08-execution-context-stall.md`；剩余问题定位在 idle/静默谓词与
5-hart 收敛路径，尚未归因。

QEMU monitor 样本曾显示未完成进程仍在用户态分配器/原子路径执行，PC 会变化，
未见内核 trap。trap/上下文、hart 身份、能力调度与启动发布的已知契约缺口记录
在 `plans/reviews/system-audit/01-sbi.md`、`02-trap-context.md`，统一设计见
`notes/execution-context.md`。在取得直接证据前不得添加平台专用补丁。
`just sifive_u` 已内置运行阶段超时收束，通过与否看日志关键行。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
