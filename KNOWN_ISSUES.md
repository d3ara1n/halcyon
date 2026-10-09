# 已知问题

记录会随时间消灭的问题，修复后删除条目；持久性约定在 AGENTS.md。

## rust_analyzer 环境前提

多 workspace 各自 target 无需编辑器配置：RA 按 workspace root 读取 `.cargo/config.toml` 的 `build.target`（2026-08 实测，含 user/ 自定义 JSON target）。

钉住的 nightly 需 `rustup component add rust-analyzer`（rust-toolchain 换 nightly 版本后要重装）。Zed 在 PATH 上找不到可用 RA 时会静默回退到自己下载的 stable RA，与 nightly cargo 可能不匹配。

## 矫正区

以下区域处于待矫正状态：其现有写法、结构与模式不作为新工作的参考。进入或引用这些区域前先读对应条目，按条目指定的依据工作；条目随承接任务完成而删除。

全项目重审范围表示目标结构需要重新确定，不等于其中每个实现已被证明错误。代码与 impls 继续如实描述现状，不能依据目标 ideas 把未实施行为回填成事实。

| 范围 | 问题 | 改用的依据 | 承接 |
|---|---|---|---|
| 全项目代码与当前实现说明：`os/`、`shared/`、`user/`、`notes/impls/` | 已授权无兼容约束整体重审；现有组件、接口和消费者不能作为必须保留某项机制的理由 | [系统重构总纲](plans/todo-2026-09-29-system-redesign.md) 的已确认原则及独立分案；只读盘点保留为现状事实 | 各分案设计闭合后精确登记实施范围；未完成前不作为新结构范本 |
| `notes/ideas/` 全体 | 施工和重构期间的未审定线稿；清理、精简或局部目标改写均不代表机制已重新设计 | [系统重构总纲的审定登记](plans/todo-2026-09-29-system-redesign.md#重构设计依据与-ideas-审定登记)及所属 todo 的详细设计；未登记的 ideas 范围不作目标依据 | 各分案在计划中完成详细设计后登记可参考范围；文档清理、设计审定与代码重构分别记录 |
| FAL 与服务目录：`user/libraries/{libfal,libfs,libservice}`、`srv_fs`、`srv_init` 中对应调用、`user/tests/test_fal`、`notes/{ideas,impls}/{fal,fs,service}.md` | 文件、发现与服务表达耦合，重复模式与状态膨胀，待整体拆分重构 | 代码仅作现状事实，不作为新结构范本；以[OAL/fs/服务目录改造计划](plans/todo-2026-10-07-fal-service-redesign.md)的 D0 详细设计与源码迁移表为依据 | D0 方案已成稿，下一步先做设计 Review；P1—P4 代码迁移及验收未开始；服务装配仍由其独立分案拥有 |
