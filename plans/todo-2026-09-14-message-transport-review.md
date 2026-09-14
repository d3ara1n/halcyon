# 消息运输闭包未来代码 Review

> 【未来审查计划】固定对象为 `3060dd8`（`feat(transport): 消息运输闭包——typed owner 收束与消费者迁移`），父提交 `e583d5e`。提交后生成，只安排已完成代码的独立只读复核，不审查方案，不阻塞流运输闭包或后续执行前置。

## 提交范围

- rinlib 新增 `MailboxSender`/`SendOnce` typed owner：铸造路径（`Mailbox::mint`/`send_once`）零 Query，未知能力在 `from_capability` 唯一转换边界 Query 一次并返回描述供一次性 rights 检查；`close`/`into_capability`/`into_raw` 完整出口。
- `Packet` 改消费式投递：`try_send(self)`/`try_reply(self, once)` 成功即消费全部 transit owner 与 send-once 授权，失败以 `SendFailure`/`ReplyFailure` 完整返还；删除 `delivered` tombstone 与每次重试的 destination Query。
- librpc：`Request.packet` 改 Option take/restore；`RequestContext.reply` 改 typed SendOnce（decode 校验通过后才摘取槽位）；`detach_reply` 返还 owner；`PreparedResponse::try_send` 失败双返还；`Dispatcher::begin` 在转换边界失败时完整返还 service owner。
- 消费者迁移：pm 流控发侧 typed；init 生产侧 Invitation 经 Packet 转移（修复失败后裸 `HandleMove` 承载丢失）；srv_fs v1 泵全消息面 typed（修复失败后 `close` 丢弃 reply 模式）；libfal/grant 适配 `MintedSender.sender` 类型。
- 内核与 shared 零改动；验收/竞态夹具保留 unsafe `send_raw*` 直验内核契约。

## 复核清单

1. 消费式投递的 take/restore 无泄漏窗口：`Request::try_send`/`PreparedResponse::try_send` 失败路径完整恢复 Packet 与 reply，成功路径不留可再操作的 owner；`Default` 占位 Packet 不产生分配或可误投递状态。
2. typed 转换边界 Query 恰一次：`from_capability` 失败完整返还 owner；`from_validated` 仅在 decode 校验事务内使用，错误标记经内核投递以 WrongObjectType 拒绝，无 UB 面。
3. `MailboxSender::send/send_until` 无 moves 语义与原 raw 空发送等价；pm 流控满箱探测与 WRITABLE 唤醒行为不变。
4. init Invitation 转移失败路径：push 拒绝与 try_send 失败均显式关闭返还的承载，无泄漏、无双重关闭；成功路径 `pm_sender` 回转原始形态供后续夹具复用的边界清楚。
5. srv_fs 泵：`validate_request` 校验失败时 Handle/Delivery 收束完整；`try_reply` 失败显式关闭 Packet 与回复授权；同进程泵语义与原实现等价（core 验收 fs 段通过）。
6. `detach_reply`/`CallError::unsent` 关闭策略确定：撤回的 send-once 显式关闭，不留悬挂授权；`Dispatcher` Drop 的 `mem::forget` 属 RPC 闭包既有边界，不因本次扩散。
7. `MintedSender.lifetime` 在 srv_fs 泵中随作用域关闭，不改变泵可见行为；libfal `issue` 交付的仍是泛化 Capability，无类型口径漂移。
8. 无残留 delivered tombstone、重试 Query destination、失败丢承载的旧模式；grep 验证验收夹具外的 `send_raw` 均在计划登记的 raw 边界用途内。

## 已有验证证据

- rinlib host 7 项（新增 Packet 构造/裁剪/占位与 HandleSet take 边界 4 项）、librpc 6 项、librunnel 15 项、libfal 42 项、libsrv 5 项通过。
- 七面 `just clippy`（shared-host/os-host/kernel-target/user-host/user-target/user-stress/user-fp）全部通过。
- `just virt` core 与 `just virt-release` 全绿：typed pm 流控（满箱探测/WRITABLE 唤醒）、typed Invitation 转移、RNL2 流、typed fs 泵、RPC、监督与显式 reset 锚点完整。
- sifive_u/virt-nofd/stress 属四闭包组合完成门，未在本提交运行。

## 边界与完成门

本计划仅拥有 `3060dd8` 的固定提交复核；流运输/Runnel 闭包、通用执行与 RPC 闭包由[执行前置计划](todo-2026-09-13-service-runtime-prerequisites.md)继续拥有，不在本 Review 内重审。未来 reviewer 使用新上下文只读审查目标提交，核对清单各点，记录有证据的 finding 或确认无 finding；无 finding 后归档本计划，有 finding 由一份 review 报告承载修复与复核。
