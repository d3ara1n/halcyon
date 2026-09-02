# MemoryObject 实现

本文是公共 `MemoryObject` 对象适配层的实现归属点。backing、ObjectView、WritePermit、地址空间事务与可执行发布状态机的内存不变量由 [`mm.md`](mm.md) 唯一拥有；Handle、运输和 WaitMany 的通用机制由 [`ipc.md`](ipc.md) 拥有。

## 当前实现边界

公共 MemoryObject 尚未接入独立 Handle、系统调用 ABI 或 rinlib owner。当前已实现的是 `os/memory_space/src/object.rs` 中的纯逻辑对象状态与授权基元，以及 Tunnel 对这些基元的内部复用；这不构成公共 MemoryObject 接口。

`memory_space` crate 提供 `ObjectId`、`ObjectViewAuthorization`、`WritePermit`、`MemoryObjectState`、`ExecutableState` 与 `SealOutcome`。它不访问页表、物理帧、HandleTable、hart 或用户指针。当前 Tunnel 的 `Connection` 以内部 `MemoryObjectState` 管理两侧 RW view 的写许可，但 backing 仍是 Connection 直接持有的单页 `FrameTracker`，尚无公共 `ObjectBacking` 对象壳。

## 对象状态与授权基元

`MemoryObjectState` 以固定 `ObjectId` 标识对象，并保存可执行发布状态、在途可写 view 数量和可选 seal waiter。状态单向经过 `Mutable → Sealing → Executable`：Mutable 可以授权符合最大权限的 view 并取得 `WritePermit`；Sealing 拒绝新的写许可；最后一个 permit 取消或退役后进入 Executable，并交出唯一 waiter token。

`ObjectViewAuthorization` 是在对象状态锁内取得的只读授权快照，不持物理 backing。实际含写权限的 view 另持不可复制 `WritePermit`。取消尚未提交的 permit 与同步后的 retire 使用不同入口，均验证 permit 属于同一 `ObjectId`；对象锁只保护状态与计数，permit 在进入 AddressSpace 事务前移出对象锁，退役也在地址空间锁外完成。

地址空间规划器以 `ObjectId + offset` 记录 object-backed region，并让 permit 从 Reserve 穿过 Publish、Synchronize 到 Retire。映射切割、stale translation 确认和逐批退役见 [`mm.md`](mm.md)「用户地址空间纯逻辑规划器」与「用户地址空间」。Tunnel 如何组合 Connection、Endpoint、Invitation 和内部对象状态见 [`tunnel.md`](tunnel.md)。

## 公共接口状态

当前 `shared/` 中没有 MemoryObject kind/role、Create/Query/Seal ABI 或 `EXECUTABLE` ObjectSignals 接口，`user/rinlib` 也没有对应 affine wrapper。公共对象能力尚未进入实现状态，本文不把 Tunnel 的内部对象状态描述为公共接口。

## 验证入口

`os/memory_space/tests/planner.rs` 覆盖对象授权、WritePermit、seal 状态、对象 offset、permit mismatch 和逐项 retire。Tunnel 对内部对象状态的组合验证见 [`tunnel.md`](tunnel.md)。公共 Handle、ABI、对象创建和跨进程 view 尚无实现，因此也不存在可宣称通过的公共接口验收。
