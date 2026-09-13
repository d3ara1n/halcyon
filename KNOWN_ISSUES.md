# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## virt-stress 竞态矩阵偶发 flake

`just virt-stress` 的 16 项竞态矩阵偶发 15/16 或 14/16 后 panic，命中场景为 `memory-vs-kill` 与 `last-thread-exit-vs-kill`。2026-09 实测：改动前后各连续三轮，两侧都出现过单轮失败、也都出现过 16/16，**与具体改动无关**。

已定位的判定缺口：有限轮次要求两种合法终因各胜出至少一次，胜负取决于真实时序，合法单侧偏胜也会失败。这不是已发现的非法内核结果；不能将这一判断扩展为所有竞态均无问题。失败可能由 panic/业务失败锚点主动收割，也可能硬 timeout，不能仅凭退出码 124 判为挂死。

后续完善统一见 [验收可靠性计划](plans/todo-2026-09-13-acceptance-reliability.md)，保留真实竞速并确定性覆盖两种终因，不靠重跑直到绿。

## virt-stress Tunnel 静默窗口截断

2026-09-13 的一轮 THROTTLE=100、300s stress 在 concurrent Tunnel close round7 后截断。该处之后是无逐轮日志的 24 项 Close/Attach 矩阵；后续 GDB 诊断及普通复跑都完成该矩阵，但不证明原截断原因。仍未归因，不归入上述概率失败。

运行配置、阶段观测、超时/业务失败分类与调查完成门统一见 [验收可靠性计划](plans/todo-2026-09-13-acceptance-reliability.md)。暂缓调查不阻塞当前前置施工；不得声称已修复或把未绿 stress 记为通过。
