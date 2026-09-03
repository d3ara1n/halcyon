# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## virt-stress 竞态矩阵偶发 flake

`just virt-stress` 的 16 项竞态矩阵偶发 15/16 或 14/16 后 panic，命中场景为 `memory-vs-kill` 与 `last-thread-exit-vs-kill`。2026-09 实测：改动前后各连续三轮，两侧都出现过单轮失败、也都出现过 16/16，**与具体改动无关**。

成因是判定方式而非内核缺陷：这两个场景要求在有限轮次内两种终因（正常退出 / 被 kill）各胜出至少一次，胜负由真实时序决定，轮数少时可能连续偏向同一侧。失败时 srv_init 直接 panic，QEMU 因而被超时收割，退出码是 124 而不是断言失败——容易被误读为挂死。

复现与判读：单轮失败先复跑，连续多轮同一场景失败才视为回归。根治方向是让场景自身在轮次内强制两种终因都发生（而非依赖概率），属 srv_init 验收编排改进，不改内核。
