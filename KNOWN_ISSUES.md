# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust-analyzer 无法区分 os / user 的 target

`halycon.code-workspace` 用 `linkedProjects` 把 shared/os/user 链进同一个 rust-analyzer 实例，但 `cargo.target` 是实例级单值，全局设为 `riscv64gc-unknown-none-elf`——user/ 的自定义 target（`rinlib/riscv64-unknown-erhino-elf.json` + build-std）在分析时被忽略。user 侧补全/类型大体可用，但 check-on-save 与真实构建（`just`）结果不一致，cfg / target feature 相关的差异不可见。

上游未解决：linkedProjects 不支持 per-project target（rust-lang/rust-analyzer#8521）；子目录 `.cargo/config.toml` 的 `build.target` 不被尊重（#11064、#11900）；JSON 自定义 target 还需 `-Zjson-target-spec`（#21821）。

根治方向：VSCode multi-root（os / user / shared 各为 folder，各自 `.vscode/settings.json` 配 target，每 folder 独立 ra 实例），代价是根目录文件不属于任何 folder。未实施。

## static mut 全局（review #6 未完成）

`os/kernel` 有 `#![allow(static_mut_refs)]` 临时豁免。标量全局已改 Atomic（KERNEL_SATP 等），复合结构（PROC_TABLE / FRAME_ALLOCATOR / ROOT / MOUNTPOINTS / HARTS）仍是 `static mut`。动这些全局时按 `plans/2026-07-code-review.md` 的方向改，不要延续 `static mut` 写法。

## 调度器 SMP soundness（review #4 未拍板）

`task/sched/unfair.rs` 把全局 ProcessCell / ThreadCell 包成 `Arc<UpSafeCell<...>>`，多 hart 同时 `get_mut()` 是别名 UB。方案 A/B/C 见 `plans/2026-07-code-review.md`，尚未拍板。涉及 `task/sched/` 的改动先读该条目。
