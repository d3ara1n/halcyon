# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## fs 集成负载在 FAL 桩上 unwrap panic

`ea24d68` 将用户态桩接口语义恢复为返回错误后，`systems/fs` 的
`create_directory(...).unwrap()`（main.rs:19）即 panic 退出回收——
错误码经 FileSystemError 映射为 `Unknown`，可读性也差。行为本身
符合「FAL 未实现 → 服务被杀」的预期，但集成负载失效：fs 的 main
后续段落（目录枚举/属性读写）不再被执行。接入 FAL 时以真实实现
消解；若在那之前需要跑 fs 后续段落，先给 fs main 补优雅的错误处理。

## page_table unmap_range 跨表批量解除算错子表基址

`os/page_table/src/lib.rs` `unmap_range` 对每个递归子表都用初始
`vpn_start` 推导 `table_base`，未传入该表实际覆盖的 VA 基址——跨
512 页边界的批量解除会解除错误的 PTE 区间并遗留残留映射，随后归还
数据帧即形成 UAF。当前唯一调用方 `extend_heap` 回滚逐页调用单页
unmap，不触发该路径；属潜伏缺陷，接入批量 unmap 前必须修复并补
跨表测试。

trap/上下文、hart 身份、能力调度与启动发布的已知契约缺口记录在
`plans/reviews/system-audit/01-sbi.md`、`02-trap-context.md`，统一设计见
`notes/impls/execution-context.md`。在取得直接证据前不得添加平台专用补丁。
`just sifive_u` 已内置运行阶段超时收束，通过与否看日志关键行。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
