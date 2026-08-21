# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## qemu sifive_u 用户态随机停滞

四服务负载在 `sifive_u` 5 核调试构建下仍可能无法于运行阶段 3 秒内全部完成，停止位置随机；同期 `virt` 4 核完整通过。故障样本缺少部分服务的退出回收以及最终的 `[Sched] 系统静默`；这与系统已经静默、仅因平台没有 shutdown device 而不退出 QEMU 是两种情况，后者不算故障。

QEMU monitor 样本显示未完成进程仍在用户态分配器/原子路径执行，PC 会变化，未见内核 trap，不能归类为固定服务故障或平台关机限制。后续应对 trap/上下文完整性、调度单一归属、用户地址空间与 TLB、用户态锁及跨 hart 迁移做系统 review；在取得直接证据前不得添加平台专用补丁。复现与超时规则见 AGENTS.md。

## 平台切换可能复用错误的内核链接产物

`Justfile` 把平台 `memory.x` 复制为固定的 `artifacts/memory.x`，链接脚本再通过 `INCLUDE` 读取；Cargo 不追踪这个工作区外部生成输入。连续切换 `virt` 与 `sifive_u` 时，源码和 `RUSTFLAGS` 未变便可能跳过重链，沿用另一平台的 `STACK_SIZE` 等链接常量，使对照结果失真。

修复构建依赖前，跨平台验证须先清理 `erhino_kernel` 的 Cargo 产物并重新构建；系统 review 应同时收口平台构建隔离与链接输入追踪。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
