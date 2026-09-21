# FAL 注册发现设计固定提交 Review

> 【未来审查计划】固定文档提交 `1000270ef2b54c35bc56acda10943dd94c0e7e12`（`docs(fal): 闭合注册发现设计与实施接力`），父提交 `3607f22`。该提交只修改六份文档，不交付 F3d 运行能力。待 F3d 实现闭包完成后，结合最终实现执行只读设计 Review；本记录不阻塞施工，不另立实施计划。

## 改动概要

- 重整唯一 FAL 总计划，保留 F0–F3c 基线与证据，明确设计、F3d、F3e、F3f、F4 的依赖和完成门。
- 裁决 A 同 Runtime 承载内存及服务目录 provider，B 发布 FAL2 DirectoryGrant，init 先订阅后发现、真实调用并装配 route。
- 明确 Registry 单一真值、平面投影、控制身份、条件撤出再注册、完整快照与 Drain 排序，以及控制 Lifetime/endpoint CLOSED 的分工。
- 将 Backend/provider 接缝、通用异步 Directory Record 出口、output_transport 实际校验、预付分批 Watch 唤醒及失败退款归入同一 F3d 闭包。
- 同步 `notes/ideas/{service,fal,framework}.md`、`notes/impls/fal.md` 和 COMPASS；Open/Copy 保留局部详细设计门，未预建运行类型。

## 审查重点与边界

1. 对照固定提交核对 authority、名称/实例/endpoint 身份、状态与投影、快照取得及实际调用是否形成一致契约；普通文件修改不能绕过注册。
2. Directory 目标域权限、存储域 Read/AcquireCapability 和运输上限是否分离；派生身份、付款继承及失败 owner 是否有唯一解释。
3. 同名竞争、迟到 Ready/撤出、回复未知、两类 CLOSED、建立期限、停止、退休及退款是否由同一机制闭合；不以 Query 快照证明旧请求已停止。
4. Backend 与两个真实后端是否检验了正确的领域边界；Watch 剩余唤醒责任、容量成本与退出接管是否避免重复真值或隐式依赖。
5. 方向、代码基线和任务状态是否各自真实，外部固定版本证据是否支持所引用的事实；未实现的 F3d/Open/Copy 不得被记作已验证能力。

本提交没有运行代码，不能据此执行“测试全绿代码批次”的代码 Review。F3d 后续实现应另固定其最终提交，并核对这里的设计是否被实现证据修订；实现责任始终由 [FAL 总计划](todo-2026-09-fal-service-capabilities.md) 拥有。

## 证据与完成门

已有证据仅为六份 Markdown 的 80 个本地链接、3 个入向计划锚点、围栏及 `git diff --check`；没有新增编译、host 测试或 QEMU 证据。

未来 reviewer 使用新上下文，遵循 [Review 纪律](REVIEW.md)，每条 finding 给出固定位置、违反的契约与实际影响。审查结果及 findings 进入单一 Review 报告，本计划完成后归档；本文件存在不表示 Review 已执行。
