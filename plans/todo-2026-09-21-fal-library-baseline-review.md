# FAL F1–F3c 与库重排前基线固定提交 Review

> 【未来审查计划】固定审查提交 `dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`（`chore: 保存 FAL F1–F3c 与库重排前基线`），父提交为 `84eeed6`。本计划在基线提交后登记，只审查该提交包含的实现与声明，不将施工快照视为整体 FAL 完成交付，不阻塞当前库重排 L0。

## 改动概要

该提交保存此前连续施工的 F1–F3c、实际消费者与文档，共 47 个文件，增加 8349 行、删除 3508 行：

- FAL2 wire/client、授权快照、GrantTable、MemoryBackend 与预算/退休责任，独立双 provider、Namespace/DirectoryGrant、跨 provider Delegate 和 route-management endpoint。
- 同域 Move、Record/Handle 编码与出口、affine Take 的回复提交/未投递恢复、普通属性 Copy。
- provider-local Watch、generation、Query/Unsubscribe、Notification owner 静默退出、节点删除和 provider 停止。
- RPC PreparedResponse/Outbox 的未投递能力恢复接口、Capability 接口、记账 shrink/refund，以及 init/fs 的真实装配、在途退出和监督收束。
- 删除 FAL1、MemFs、slot-1 anchor、自客户端及旧运输泵，调整两条平台路线超时并同步实现文档。
- 登记按领域组织库、独立公共记账/执行、服务知识不向下层渗透的原则；新建库重排计划，F3d 等待该前置。

## 审查范围与重点

1. wire、值编码、业务 capability 槽和实际 owner 数量是否一致；错误输入、权限不足、重复字段/槽位和分配失败是否保留正确清理责任。
2. DirectoryGrant、AccessSnapshot、sender identity、badge、Lifetime 与账户来源是否保持授权边界；Derive、Delegate 和 route 装配有无权限放大或借用转关闭权。
3. Move 的准备/最终复查/提交是否处理并发状态变化，尤其 Take 预留与目录关系；跨 provider 错误是否保持公开契约。
4. Record/Handle 的 repeatable 导出与 affine Take 是否分别保持正确语义；Outbox 投递、未投递恢复、回复放弃、节点版本和 Charge 收缩是否共享一致提交点。
5. Watch 的准备—安装窗口、稳定节点与授权复查、事件代次、取消确认、abandoned 回复、静默退出及 provider 终态是否有唯一 owner，最终观察注销与退款是否完整。
6. 双 provider 的真实跨进程路径、下游调用与 provider 退出、Runtime 停止及后端退休是否闭合；源码里的固定验收政策是否与正式机制明确分开。
7. 旧路径是否真正删除；host/target 与 QEMU 证据是否覆盖对应实现，文档是否准确标示铺路、已接通能力及尚未执行的整体门。
8. 通用记账/执行仍位于 libsrv、FAL ServiceRecord 预算槽、shared service 空壳等已登记错位是否与库重排计划的现状一致。新原则描述目标，不能将该固定提交误判为已经完成库重排；新增未登记问题仍须给出具体证据。

## 已有证据与限制

证据入口为该固定提交中的 [FAL 总计划](todo-2026-09-fal-service-capabilities.md) 和 `notes/impls/{fal,rpc,runtime,startup}.md`：

- 计划记录 F3c 最近已通过 `just check`、全部用户程序 RISC-V ELF 构建、libfal host 27 项、libfs host 17 项、七面 `just clippy`、`THROTTLE=100 just virt` 与 `THROTTLE=100 just virt-release`。
- 提交前本会话重新执行 `git diff --check`、暂存区检查与文档检查；没有重跑上述代码测试和 QEMU。既有日志仅在本机 artifacts，审查时应按计划核对，不能把文字记录替代所需复现。
- F3/F4 收尾的完整 stress、sifive_u、virt-nofd、boot-failure 与 `just acceptance` 尚未对本次业务闭包统一执行；公共前置的历史完整验收不替代这些证据。
- 服务注册/发现、Open、流 Copy、独立 test_fal 和库/目录重排不属于已完成能力。它们分别由 FAL 总计划与库重排计划拥有。

## 完成门与问题归属

未来 reviewer 使用新上下文只读核对固定提交与其父提交，遵循 [Review 纪律](REVIEW.md)，不以当前分支后续迁移代码替代该快照。finding 给出位置、可达性、影响与违反的契约；若需要补证，明确区分缺少证据和已证实缺陷。

库归属及依赖问题继续由[库重排计划](todo-2026-09-21-library-knowledge-ownership.md)唯一承接，FAL 尚未交付的业务由总计划承接；不为同一问题另起平行任务。其他实际 finding 的证据与修复/复核入口集中登记，不直接修改代码。审查与复核完成后归档本计划；本文件的存在不表示已经执行 Review。
