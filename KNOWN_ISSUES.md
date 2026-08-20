# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## 调度挂起（待重写解决）

mm bug 修复后 4 核不再 panic，但单核 `-smp cores=1` 仍在 fs 的 32MB extend 后挂起：gdb 见 fs 阻塞在用户态等待循环，pm/init 从未被调度；4 核下 fs main 也未在观测窗口内完成。调度/唤醒路径问题，重写 M3 消灭，背景见 `plans/2026-08-mm-map-bug.md`「附带发现」。

## rust-analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## static mut 全局（review #6 未完成）

`os/kernel` 有 `#![allow(static_mut_refs)]` 临时豁免。标量全局已改 Atomic（KERNEL_SATP 等），复合结构（PROC_TABLE / FRAME_ALLOCATOR / ROOT / MOUNTPOINTS / HARTS）仍是 `static mut`。动这些全局时按 `plans/2026-07-code-review.md` 的方向改，不要延续 `static mut` 写法。

## 调度器 SMP soundness（review #4 未拍板）

`task/sched/unfair.rs` 把全局 ProcessCell / ThreadCell 包成 `Arc<UpSafeCell<...>>`，多 hart 同时 `get_mut()` 是别名 UB。方案 A/B/C 见 `plans/2026-07-code-review.md`，尚未拍板。涉及 `task/sched/` 的改动先读该条目。
