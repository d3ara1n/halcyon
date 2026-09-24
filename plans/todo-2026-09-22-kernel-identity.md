# 内核 Rust 组件身份统一

## 状态与决定

已完成。用户于 2026-09-22 明确：全仓组件身份规则继续生效，`os/kernel/` 的 Rust package、默认 binary target 与 crate identifier 统一为 `kernel`；`erhino_kernel` 已清理，不是命名例外。系统品牌 Halcyon、微内核专名 eRhino 与用户态基础库 rinlib 不受此构建身份迁移影响。

本计划承接 FAL 完成后的内核构建身份整改；改名不改变内核 ABI、权限或运行语义。

## 当前事实与目标

- 登记基线：`task/fal-service-capabilities`，FAL 工作树仍含未提交改动；本闭包只同步内核身份相关路径。
- 目录叶名为 `os/kernel`，package、默认 binary target、Justfile 产物和有效调试/验收入口统一使用 `kernel`。
- 目标：源码构建身份与有效命令、产物路径统一使用 `kernel`，不保留 package alias、兼容二进制或旧路径副本。
- owner：本专题实施者；唯一待改清单由本计划承载，其他文档只链接。

## 依赖与迁移范围

前置是核清当前工作树、构建脚本和真实消费者；此项不改变内核 ABI、权限或运行语义。自然顺序：

1. 搜索非归档源码、Cargo manifest/lockfile、workspace 配置、Justfile、工具脚本和有效文档中的 package 名、默认 target、crate 引用、ELF/bin 路径、host 排除参数、GDB/审计入口。
2. 同一闭包同步 manifest、必要锁文件、构建/检查/验收脚本与调试产物路径，不留下新旧双轨。已有 `os/kernel/` 目录保持。
3. 更新当前 README、AGENTS、构建手册与相关 impls 中的有效用法；固定提交审查、历史基线与归档中的旧名保留为当时事实。
4. 验证 Cargo 身份、实际产物和 QEMU 消费链后收口本计划；保留全仓命名规则。

## 失败边界与清理条件

- 构建、审计、调试或 QEMU 仍要求旧名时不得宣称完成；不通过复制产物或 alias 临时兼容。
- 不删除其他任务产物或覆盖未提交改动；旧产物的清理限定于本次迁移确认的生成路径。
- 清理验证：非历史的执行路径没有残留 `erhino_kernel` 依赖，生成产物、调试符号入口和文档示例一致。

## 验证门

静态检索和 Cargo 元数据核对后，执行 `just check`、`just clippy`、显式 host target 的受影响测试和 `just build_kernel`，确认新产物由正式构建链产生；使用现有 core QEMU 验证装载链，并检查 boot-failure/GDB 符号入口。根据实际涉及的脚本补对应检查，不把纯改名扩展为无关机制重构。

已完成并验证：Cargo metadata 只产生 `kernel`；`just build_kernel`、`just check`、`just clippy`、`cd os && cargo test --workspace --exclude kernel --target aarch64-apple-darwin`、`just virt` 和 `just virt-boot-failure` 均通过。boot-failure 的 panic/alloc/fatal 三类符号注入均能使 4 hart 停驻 Gate Failed。有效非历史路径不再残留 `erhino_kernel`；固定审查记录和历史参考中的旧名保留为历史事实。
