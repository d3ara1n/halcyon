# 用户态库知识归属、依赖、目录与命名固定提交 Review

> 【未来审查计划】固定审查提交 `96ee03b0d1641c86ed8ab05951bad6954ea84db4`（`refactor(user): 重排用户态库归属与命名`），父提交为 `2124413710bdb8a3bc8a0fa5e73d06f2839af176`。本计划在实现提交后登记，只读审查该提交形成的最终结构，不阻塞 FAL F3d。

## 改动概要

该提交按 Git rename 识别统计共改动 78 个文件，增加 1053 行、删除 877 行：

- 新建 `libbudget`，以非领域 `Account`、带布局身份的 `BudgetSlot`、不可变 `AccountView<K>` 与非泛型 `Charge` 组合 `metadata_admission`；零单位 Charge 同样保活付款账户与结构名额。
- 新建 `libexecution`，迁入 Runtime、WorkQueue、Wake、任务/来源/期限/停止/退休协议及 `ExecutionResource`；RPC、进程、Runnel、FAL 和服务运行体全部迁移。
- `libfal` 删除 Task、InputBytes 与 ServiceRecord 分类；`srv_fs` 以同一 Account 的执行/FAL 两个 view 组合付款来源，grant 派生继续继承原账户。
- 删除旧 `libsrv` 公共机制包与 shared 空服务 ABI；服务领域保留给 F3d 的真实 `libservice` 消费者，不建立空 facade。
- 全部用户态公共库从 `user/frameworks/` 迁至 `user/libraries/`，同步 Cargo path、workspace、Justfile、服务/驱动/测试依赖和有效文档入口。
- 新增 `user/README.md` 用户态组件命名规则：单词领域写全，多词使用公认缩写，`srv_` / `drv_` / `test_` 只作二进制角色前缀；删除无实现、无符号消费者的空 `libdrv`，未来正式名称为 `libdriver`，服务领域正式名称为 `libservice`。
- 同步方向、实现、COMPASS、FAL 与架构审计计划，并将 debug core 默认墙钟上限从 30 秒重校为 45 秒。

## 审查范围与重点

1. `libbudget` 的全局/账户双层额度、失败回滚、结构账户名额、跨布局拒绝、view 克隆不分配/不扩额、零与非零 Charge 保活及 `shrink_to` 退款是否一致。
2. `libexecution` 相对原 Runtime/WorkQueue/Wake 的迁移是否保持任务、来源、Gate、输入、期限、停止、退休和失败 owner；领域库是否不再通过服务包取得公共执行。
3. `srv_fs` 的执行与 FAL 视图是否来自同一个 Account 且单位绑定正确；GrantTable、AccessSnapshot、子 grant 与收到的授权是否始终继承可信付款来源。
4. Cargo DAG、公开类型与资源枚举中是否仍有反向服务依赖、旧 `libsrv` 运行路径、执行槽混入 FAL、ServiceRecord 下沉或重复记账实现。
5. `user/frameworks/` → `user/libraries/` 是否完整；目录叶名、package、默认 target、crate identifier、path 依赖、构建脚本与文档入口是否一致且无兼容目录。
6. 删除 shared `service::Endpoint` 与空 `libdrv` 是否确无运行消费者；未来 `libservice` / `libdriver` 名称是否只表达方向而未制造空 crate 或伪消费者。
7. `user/README.md`、`user/libraries/README.md`、AGENTS、ideas/impls、COMPASS、FAL 总计划和架构审计是否分别准确描述命名规则、目标结构、实现事实与历史基线。
8. Review 最终结构中的 bug、性能回退、分配面、重复 owner、旧路径、重复真值与未登记设计债务；不得只以编译通过或 rename 相似度判断正确。

## 已有证据与限制

实现计划及最终证据见[归档计划](archived/todo-2026-09-21-library-knowledge-ownership.md)：

- `git diff --check`、`just check`、七面 `just clippy` 通过；Cargo metadata 无 `libsrv` / `libdrv` package 与 `user/frameworks/` 构建路径。
- host 测试：`libbudget` 6、`libexecution` 22、`libfal` 27、`libfs` 17、`libprocess` 16，shared workspace 通过。
- 默认 50% `just virt` 与 `just virt-release` 通过；debug core 原 30 秒上限无 panic 超时后重校为 45 秒。
- `THROTTLE=100 just acceptance` 通过 stress 16/16、release、`sifive_u`、`virt-nofd` 和 panic/alloc/fatal boot-failure，无残留 QEMU。
- 命名 L5 只删除空 crate/空依赖并修改文档与构建枚举；其后重新通过 `just check`、七面 `just clippy`、默认 50% `just virt`、Cargo metadata 与 Markdown 本地链接检查，未重复完整平台聚合。
- 日志与 artifacts 只存在于实施机器；未来 reviewer 需要复现时应按归档计划的命令重跑，不能把文字记录当作独立运行证据。

## 完成门与问题归属

未来 reviewer 使用新上下文只读核对固定提交及其父提交，遵循 [Review 纪律](REVIEW.md)。finding 必须给出具体位置、可达性、影响和违反的契约；缺少运行证据与已证实实现缺陷应分开陈述。

F3d 服务注册/发现、`libservice` 的真实 schema/状态机、Open、流 Copy 与独立 `test_fal` 不属于该提交的已交付能力，由 [FAL 总计划](todo-2026-09-fal-service-capabilities.md) 继续拥有。审查完成后归档本计划；本文件的存在不表示 Review 已执行。
