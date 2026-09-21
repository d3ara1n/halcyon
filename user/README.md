# 用户态

用户态总体布局与运行配置尚未定稿；本篇当前只记录已经成立、对全部用户态 Rust 组件生效的共同规则。库的知识归属与依赖纪律见 [`libraries/README.md`](libraries/README.md)。

## Rust 组件命名

目录叶名、Cargo package 名、默认 binary/library target 名与 crate identifier 必须一致。名称描述稳定的领域或进程角色，不描述源码阶段、实现位置或临时装配。

### 领域名称

- 单个普通领域词使用完整英文单词：`libprocess`、`libservice`、`libdriver`、`libdevice`、`libbudget`、`libexecution`。
- 多词领域使用业界公认且在本项目中含义唯一的缩写：Remote Procedure Call → `librpc`，File System → `libfs`，File Abstraction Layer → `libfal`。
- 正式专名保持正式拼写：Runnel → `librunnel`；`rinlib` 是用户态基础运行库的既有正式名称。
- 单词过长时，只能使用已有、通行且不会与其他领域冲突的缩写；不得仅为省字符临时截短。缩写一旦成为公开组件名，应在本篇或相应方向文档中记录其展开与领域含义。
- acronym 在 Rust 标识符中使用小写；文档正文仍按正式写法书写，如 RPC、FAL、FS。

`lib` 是库身份前缀，不是缩写许可。单词 `service`、`driver`、`device` 不因前面已有 `lib` 而缩成 `srv`、`drv`、`dev`；因此未来服务与驱动公共库分别使用 `libservice`、`libdriver`，不使用 `libsrv`、`libdrv`。

### 二进制角色前缀

可执行 crate 使用固定角色前缀，前缀之后的领域名仍遵守上述规则：

| 角色 | 格式 | 示例 |
|---|---|---|
| 系统服务 | `srv_<domain>` | `srv_init`、`srv_fs`、`srv_pm`、未来的 `srv_process` |
| 用户态驱动 | `drv_<domain>` | `drv_spi_sifive` |
| 验收进程 | `test_<domain-or-scenario>` | `test_fp`、`test_hammer`、`test_target` |

`srv`、`drv`、`test` 在这里表示二进制运行角色，只能作为带下划线的前缀使用；它们不能替代库的领域名称。库是否包含管理接口、客户端接口或服务端接口，不改变其 `lib<domain>` 身份。

### 变更门

组件改名必须在同一闭包内同步目录、Cargo package、默认 target、crate import、workspace membership、path 依赖、构建脚本和有效文档入口，不保留只为兼容旧名字的 package alias、重复目录或重导出 facade。固定提交 Review、归档计划和历史基线中的旧名称保留为当时事实，不批量改写成当前名称。
