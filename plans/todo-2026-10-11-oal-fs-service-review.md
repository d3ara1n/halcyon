# OAL、文件与服务目录交付独立 Review

状态：**已登记，待执行。** 本篇登记代码交付后的独立审查，不表示审查已经通过，也不重新启动已结束的实施。

## 固定目标与范围

- 目标提交：`a1ca124fdb70c1f20034f3e7d24d587f4323c98e`（`refactor(fs): 完成 OAL 文件与服务目录重构及机制收敛`）。
- 父提交：`4523bbfe65287f6b84ab73d9f034dae1a526d38d`；审查上述两点间的完整交付及其实际依赖，不能只看最后一轮收敛。
- 范围：P1—P4 的 liboal/libfs/libservice、Notification 观察角色与共享 ABI、执行付款、RPC 支撑、provider/init/test_fs 消费者、C01—C11 收敛，以及相关工具、验收和文档。
- 非目标：磁盘持久化、高层跨域 Move、正式服务监督、设备体系，以及未因本次交付变化的其他系统重构。实际缺陷跨越边界时追到责任 owner，不以文件清单截断问题。

## 审查重点

1. liboal、libfs、libservice 的知识边界，瞬态 ObjectInfo、开放 trait、Namespace/walk 和 Boundary；可信 sender context、授权衰减、当前目录范围及完整出口 rights。
2. Notification Owner/Signaler/Observer 的派生、signals、付款、业务关闭与物理保活；Wait/WaitSet、LifetimeObserver/ReplyObserver 与所有 ABI 消费者一致，旧名称无兼容残留。
3. TaskSubmission、Task/Bound 来源付款与 Requests 作用域；失败完整返还、来源实际注销后退款、共享 Dispatcher 与私有 PendingCall 的费用归属，执行存储与领域 payload 不重复收费。
4. 准备阶段的独占 permit、提交阶段不分配、mutation 的当前权限和冲突条件。重点检查 C11 同目录不同名字并发创建、父目录移出授权范围及版本耗尽，不将其他事务的必要冲突检查一并消除。
5. Watch 的 sequence/pending/Ack、终态、只读就绪电平及来源暂退再登记；Bind 的真实 backing、pin、未知提交结果、迟到回复与能力退休。
6. 流 Finish 的 Delivery 释放、终态冻结与映射退休；Copy 的进度、创建 Unknown、取消、独立业务成功、活 waiter 领取及失败后原任务持有清理责任。
7. C01—C11 删除的重复状态与过渡分支是否确实可由当前 owner 推导；普通叶能力 Drop 与实际映射可失败释放的边界；旧 libfal、DirectoryGrant、两阶段发布和按路径回删是否彻底移除。
8. srv_fs 装配、正式客户端与验收控制器的职责；验收标记是否对应真实行为，工具期限与结果判定是否保持唯一真值；impls 是否准确描述当前代码。

## 证据与未覆盖边界

[主计划当前接力](todo-2026-10-07-fal-service-redesign.md#当前接力)拥有交付前结构检查、三轮收敛和最终验证。最终源码指纹为 `ca8ba7025a3d44a832672b4a4310b4da222969fb060430a9474f0bef3143066a`（363 项输入）；本机证据 `artifacts/convergence3/acceptance-summary.json` 记录 libfs43/librpc16 共59项 host、目标 lint、just check、七面 clippy、原失败 nofd 路线与完整 just acceptance（784.55秒）通过。源码及工具与固定提交一致；文档收口不改变上述输入。

C10 首轮 nofd 的真实 Conflict、修前确定失败的 host 用例与 C11 修复后证据均保留。C11 证明的是不同名字创建被整体版本误拒绝，不能据此宣称旧 midflight 或 W05-04 的历史根因已查明。早期 P4、40次连续门与前两轮收敛属于各自基线。

剩余精确注入和历史未知由[主计划](todo-2026-10-07-fal-service-redesign.md)及 [Notification N6](todo-2026-10-10-notification-observer-roles.md#n6-注入前置再核2026-10-11)继续拥有，不在本篇复制债务清单。执行付款合同和完成证据见[归档前置](archived/todo-2026-10-10-execution-funding.md)。实现审计入口为 [fs](../notes/impls/fs.md)、[service](../notes/impls/service.md)、[RPC](../notes/impls/rpc.md)、[框架](../notes/impls/framework.md)与 [IPC](../notes/impls/ipc.md)，以固定提交代码为准。

## 执行与报告

按 [REVIEW](REVIEW.md) 使用新上下文只读审查。现有验证可直接引用，仅为新发现的具体风险追加适当检查；不得把未覆盖的精确故障窗口改记为普通运行通过。此前交付前收敛和结构检查不替代本次独立 Review。

形成 `review-<日期>-oal-fs-service.md` 后，由报告唯一承接 findings、修复和复核；本篇登记报告入口并归档。不在报告产生前预填通过结论，也不在本轮提交后自动启动新的施工。
