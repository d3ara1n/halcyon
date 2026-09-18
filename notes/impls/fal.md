# FAL 实现现状

方向见 [`../ideas/fal.md`](../ideas/fal.md) 与 [`../ideas/fs.md`](../ideas/fs.md)。通用 RPC/Runtime/Runnel 的实现分别由 [`rpc.md`](rpc.md)、[`runtime.md`](runtime.md) 与 [`runnel.md`](runnel.md) 拥有；本篇只记录 FAL 当前代码事实。唯一施工导航是 [`FAL 整体计划`](../../plans/todo-2026-09-fal-service-capabilities.md)。

## F0 审计结论（2026-09）

F0 已完成。当前源码证明：FAL 仍是“v1 验收路径 + 未接线 v2 积木”两层并存，整体业务没有交付。

### 唯一真实生产链

`srv_init` 以空 grants 启动 `srv_fs`。`srv_fs` 的 `Fs` 同时持有 provider `MailboxSender`、同步 `librpc::Caller` 和 worker；worker 内运行 `Runtime + WaitSet + Outbox`，业务状态是 `MemFs`。主线程把 provider 自己的 sender 句柄挂入私有 `PrefixTable` 作为根 anchor。每次调用经过：

```text
libfs::resolve
  → srv_fs::Fs::call
  → librpc::Caller / Mailbox / Delivery
  → srv_fs::Ingress + RequestTask + Outbox
  → libfal::provider::serve
  → libfal::memfs::MemFs
```

这条链是真实内核运输和 RPC 验收，但不是正式 FAL：slot 1 anchor 只被传输，provider 不解释身份或权限；`MemFs` 从自身 root 行走，属性位承担全部访问判断，`property_write` 忽略授权上下文；`Move/Copy/Open` 仍为 Unsupported。`libfs::PrefixTable` 持裸 Handle，`Delegate` 只有 host mock 形态，启动链没有交付 FAL endpoint。

### v2 积木与接线状态

`store.rs`、`backend.rs`、`data.rs`、`value.rs`、`grant.rs`、`authority.rs`、`protocol.rs` 和 `resource.rs` 已表达部分目标结构，但全仓没有正式生产调用者：

- `NodeStore` 用 `NodeRef` 的 pin 与目录 link 分账，PreparedNode/PreparedMutation/PreparedWrite/StoredValue 预付容量与账户额度；冲突原样返还事务，旧属性、旧块和摘链节点进入有界退休。
- `MemoryBackend` 具备 Create/Delete/Write/Property/Move 的 prepare、位置/结构代次校验和无分配 commit；所有后端访问要求不可外部构造的 `AccessSnapshot`。
- `GrantTable::snapshot` 是 `AccessSnapshot` 的唯一构造点；GrantTable 绑定真实 provider Mailbox identity，以 sender koid 查表，持 Lifetime CLOSED 观察和账户收费。它借用 WaitSet，而正式 Runtime 独占 WaitSet，因此首个 provider 必须由同一服务状态拥有者共同协调 Runtime、GrantTable、backend 和 retire wake。
- FAL2 `protocol.rs` 只有 request/header 解码和字段校验，没有 response 编码、client 构造、provider trait 或生产引用。
- `FalResource` 中 Request/Outbox/Watch/Offer/Stream/ServiceRecord 等槽位尚无实际消费者；`NotificationWake` 只有定义，没有正式构造点。

### Owner 与收束边界

未提交准备阶段必须携带全部 owner：节点 pin、目录/块 Permit、Account Charge、能力 owner、回复和 Delivery。准备失败或 commit 冲突原样返还这些责任。成功提交后，旧值和最后引用由后端结果与退休队列承接；回复失败只能形成 `OutboxResult::Abandoned`，不能伪造业务回滚。服务退出需要停止准入、取消/完成请求、收束 GrantTable、backend retire queue、Delivery、reply-once、WaitSet，并确认账户退款。

当前 v2 非空 `Drop` 只记录 `abandoned_nodes`/`abandoned_blocks`/`abandoned_grants`，而不是完成业务退休；v2 backend/data/value/grant/protocol 没有模块测试，只有 `store` 的三项 host 测试。这些是 F1 的真实验证与接线责任，不是 FAL 已交付证据。

## F1 接手边界

F1 不再拆成“先后端、后授权”的文件阶段，而是一个单 provider 纵向闭包：

```text
服务状态拥有者
  ├── Runtime / WaitSet / 任务与期限
  ├── GrantTable / AccessSnapshot / sender identity
  ├── FAL2 request/response/provider/client 接缝
  ├── MemoryBackend / NodeStore / Data / StoredValue
  └── retire queue / explicit Wake / Account refund
```

F1 的唯一真实消费者是正式 provider 运行体及其最小 client。不得以 v1 `MemFs`、同进程 self-pump、slot-1 anchor 或 host mock 充当 F1 消费者。完成门必须覆盖授权不可伪造、五类 mutation 的正常/准备失败/取消/冲突/提交/旧值退休、服务公平推进、期限、调用者退出、服务退出、Delivery/Outbox 和 metadata/account 退款。

F2 才接独立 provider/client、DirectoryGrant、namespace/Delegate、真实启动能力图和跨 provider 路由；F3 才接 Open/Watch/注册发现/Move/Copy 等长生命周期业务；F4 删除 v1 临时 anchor、无鉴权 MemFs、同进程泵并完成独立装配与组合验收。
