# FAL 实现现状

方向见 [`../ideas/fal.md`](../ideas/fal.md) 与 [`../ideas/fs.md`](../ideas/fs.md)。通用 RpcPrefix/Caller、Runtime 与 Runnel 的实现分别由 [`rpc.md`](rpc.md)、[`runtime.md`](runtime.md) 与 [`runnel.md`](runnel.md) 拥有；本篇只记录 FAL wire、后端积木、授权草稿和 `srv_fs` 当前消费边界。唯一施工导航是 [`FAL 整体计划`](../../plans/todo-2026-09-fal-service-capabilities.md)。

## 当前分层

当前仓库同时存在两层，不能混称已经交付的 FAL：

1. **v1 验收路径**是现有真实消费者。`libfal::{header,provider,memfs,...}` 使用 `PROTOCOL_ID = "FAL1"`，slot 0 是 send-once 回复，slot 1 是临时对象 anchor。`srv_fs` 在同一进程中创建 Mailbox，另起用户线程运行 `libsrv::Runtime` 驱动的 `server::run`；客户端与 provider 的每次调用真实经过内核 Mailbox/RPC，但共享同一进程和 `MemFs` 实例。
2. **v2 后端与授权积木**已经进入源码，但尚无正式 provider/client 纵向消费者。`protocol.rs` 定义 `FAL2`、32 字节 Header、稳定位置和 Lookup/Derive/Enumerate/Create/Link/Read/Write/Take/Open/ReadAt/WriteAt/Move/Delete/Watch/Unwatch；存在源码和 host 测试不等于协议已经发布。

## v2 已有积木

`user/frameworks/libfal` 当前包含：

- `store.rs`：`NodeId`/`NodeRef`、链接与 pin 分账、`PreparedNode`、有界 retire queue，以及通过 `libsrv::wake::Wake` 显式唤醒空闲退休执行者；
- `backend.rs`：基于 `NodeStore` 的 `MemoryBackend`、稳定 `Position`、`PreparedMutation`，以及目录、属性、流、链接的准备/校验/提交路径；
- `data.rs`：稀疏分块数据、预付写入和逐块退休；
- `value.rs`：Integer/Decimal/String/Blob/Handle/Array/Record 的长度化编码、能力槽策略、FAL ceiling 与 affine/repeatable 出口描述；
- `authority.rs`：FAL 领域权限与不可外部伪造的访问快照；
- RISC-V 目标下的 `grant.rs`：绑定真实 Mailbox identity 的 `GrantTable`，持 NodeRef、权限上限、Lifetime 观察和账户，不持可延长 authority 的 sender 母本；
- `resource.rs` 与 `libsrv::budget` 连接 FAL 元数据、节点和存储 charge。

这些模块已经表达目标类型图的一部分，但尚未由正式 provider 统一拥有。`PreparedMutation` 的所有准备失败、取消、冲突、旧值退休和 wake 路径还没有通过同一个生产消费者形成闭包；Grant、namespace、v2 codec 和后端也未共同迁移。

## 当前 `srv_fs` 边界

`user/services/srv_fs/src/server.rs` 已不再是手写同步 pump：它使用 `WaitSet`、`libsrv::Runtime`、`RequestContext` 与 `Outbox` 驱动 Mailbox ingress 和回复。但业务仍调用 v1 `provider`/`MemFs`，且客户端与 provider 位于同一进程。

因此当前 QEMU 的 `fs acceptance passed` 只证明：v1 framing、Mailbox/Delivery/send-once、Runtime 调度、Lookup/Enumerate/Create/Link/属性与定位读写能够组合；它不证明 DirectoryGrant、跨 provider Delegate、FAL2、Open 流状态机、Watch、注册/发现或独立 provider/client 已交付。

仍待删除的旧路径包括 v1 临时 anchor、无鉴权 `MemFs`、同进程装配以及 unsupported/占位业务分支。删除必须随正式 v2 provider/client 的真实迁移一起发生，不能先删消费者或保留双轨 adapter。

## 当前验证真值

公共操作 P6 最终快照已运行完整 os/shared host 回归、七面 `just clippy` 和 `just acceptance`；其中会编译并运行现有 libfal/libfs 测试及 `srv_fs` v1 组合。该结果是当前草稿代码的回归基线，不是 v2 FAL 的交付证据。

下一步不是继续补单个 FAL 文件，而是执行总计划中的 **F0 重新基线审计**。审计完成后，首个实现闭包预期为“后端准备/取消/退休 owner”：让 NodeStore、PreparedMutation、payload retirement、显式 wake 与账户退款由一个真实 provider 纵向消费，再进入授权域和 v2 provider/client 迁移。
