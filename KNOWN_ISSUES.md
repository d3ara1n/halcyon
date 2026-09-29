# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## 矫正区

以下区域处于待矫正状态：其现有写法、结构与模式不作为新工作的参考。进入或引用这些区域前先读对应条目，按条目指定的依据工作；条目随承接任务完成而删除。

| 范围 | 问题 | 改用的依据 | 承接 |
|---|---|---|---|
| `notes/ideas/` 全体 | 大量设计决策、状态机、owner 与失败路径写入 ideas，超出线稿粒度 | [notes/README](notes/README.md) 的 ideas 定义与粒度判据，不模仿现有篇幅写法 | 后续触及相应篇的设计任务顺手清理 |
| FAL 全栈：`user/libraries/{libfal,libfs}`、`user/services/srv_fs`、`user/tests/test_fal`、`notes/{ideas,impls}/{fal,fs}.md` | 实现质量低、重复模式与状态膨胀，待整体重新设计与重构 | 代码仅作现状事实，不作为新代码结构或写法范本 | FAL 重新设计计划（待立项） |
