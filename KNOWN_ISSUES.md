# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## 用户态多线程落地前的写回 panic 面

IPC 对象层 review（`plans/archived/review-2026-08-ipc-object.md`）确认：
`MailboxCreate`/`HandleDuplicate`/`MailboxMakeSendOnce`/`TunnelCreate`/
`TunnelAttach`/`Receive` 的用户写回以 `expect("validated ... must remain
writable")` 收尾，前提是「check_range 到写回之间同进程无映射变更」。当前
单线程进程下成立；`ThreadSpawn` 落地后，同进程异 hart 线程可在两次
space 锁之间 `HandleClose` tunnel endpoint（经 `unmap_external` 解除映射），
若被解除的页恰为某输出缓冲，写回校验失败即 panic 内核——违反「用户可
触发的 fault 杀进程绝不 panic」戒律。接入用户态多线程（服务化阶段）前
改为锁内复检 + 优雅错误或进程终止路径，并同步修正 `uaccess.rs` 头注释
「同进程无并发映射变更者」的前提表述。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。
