# MemoryObject 实现

本文是公共 `MemoryObject` 对象适配层的实现归属点。backing、ObjectView、WritePermit、地址空间事务与可执行发布状态机的内存不变量由 [`mm.md`](mm.md) 唯一拥有；Handle、运输和 WaitMany 的通用机制由 [`ipc.md`](ipc.md) 拥有。

## 当前实现边界

公共 MemoryObject 尚未接入独立 Handle、系统调用 ABI 或 rinlib owner。当前已实现的是 `os/memory_space/src/object.rs` 中的纯逻辑对象状态与授权基元，以及 Tunnel 对这些基元的内部复用；这不构成公共 MemoryObject 接口。

`memory_space` crate 提供 `ObjectId`、`ObjectViewAuthorization`、`WritePermit`、`MemoryObjectState`、`ExecutableState` 与 `SealOutcome`。它不访问页表、物理帧、HandleTable、hart 或用户指针。

内核侧的对象 core 是 `os/kernel/src/task/memory_object.rs` 的 `MemoryObjectCore`，持 `ObjectBacking`、`MemoryObjectState` 与 metadata owner（sponsor 强引用 + `ObjectBackingPermit`）。对象身份（全局单调铸造的 `ObjectId`）与固定长度由 `MemoryObjectState` 保管——身份、长度与可执行状态同属对象的逻辑状态，单一真值点避免失步。等待面不属于 core：各使用方自己拥有 `ObjectWaitState`（Tunnel 在 `Endpoint` 上）。

`ObjectBacking`（`os/kernel/src/frame.rs`）是固定长度、不可分解的对象数据 backing，堆化持有多 extent 的单 extent owner 列表（`Vec<FundedExtent>`），不内联定长 funding 容器——定长容器只适合一次性事务结果，嵌进常驻对象会使每个对象无论实际 extent 数都占满整份槽位且构造路径逐层复制本体。它不暴露 split/merge：对象 backing 由创建者绑定池一次付清，view 的切割、降权与解除不切数据 backing。`ObjectBacking::project(offset, length, &mut spans)` 把逻辑页区间投影为有界物理 span 序列，追加写入调用方缓冲，单页退化为长度为一；这是对象 view 建立 translation 的唯一几何展开入口。

当前 Tunnel 的 `Connection` 持 `Arc<MemoryObjectCore>`，经其 `MemoryObjectState` 管理两侧 RW view 的写许可；对象 backing 由创建进程绑定的 MemoryPool 支付。公共 MemoryObject 将复用同一 core，因此对象身份与状态机不存在双来源。

## 对象状态与授权基元

`MemoryObjectState` 以固定 `ObjectId` 标识对象，并保存可执行发布状态、在途可写 view 数量和可选 seal waiter。状态单向经过 `Mutable → Sealing → Executable`：Mutable 可以授权符合最大权限的 view 并取得 `WritePermit`；Sealing 拒绝新的写许可；最后一个 permit 取消或退役后进入 Executable，并交出唯一 waiter token。

`ObjectViewAuthorization` 是在对象状态锁内取得的只读授权快照，不持物理 backing；它同时携带对象的固定长度，view 越界因此以对象自身几何为准，调用方不另传一份可能与对象不符的长度。实际含写权限的 view 另持不可复制 `WritePermit`。取消尚未提交的 permit 与同步后的 retire 使用不同入口，均验证 permit 属于同一 `ObjectId`；对象锁只保护状态与计数，permit 在进入 AddressSpace 事务前移出对象锁，退役也在地址空间锁外完成。

地址空间规划器以 `ObjectId + offset` 记录 object-backed region，并让 permit 从 Reserve 穿过 Publish、Synchronize 到 Retire。映射切割、stale translation 确认和逐批退役见 [`mm.md`](mm.md)「用户地址空间纯逻辑规划器」与「用户地址空间」。Tunnel 如何组合 Connection、Endpoint、Invitation 和内部对象状态见 [`tunnel.md`](tunnel.md)。

## 公共接口状态

当前 `shared/` 中没有 MemoryObject kind/role、Create/Query/Seal ABI 或 `EXECUTABLE` ObjectSignals 接口，`user/rinlib` 也没有对应 affine wrapper。公共对象能力尚未进入实现状态，本文不把 Tunnel 的内部对象状态描述为公共接口。

## 验证入口

`os/memory_space/tests/planner.rs` 覆盖对象授权、WritePermit、seal 状态、对象 offset、permit mismatch 和逐项 retire。Tunnel 对内部对象状态的组合验证见 [`tunnel.md`](tunnel.md)。公共 Handle、ABI、对象创建和跨进程 view 尚无实现，因此也不存在可宣称通过的公共接口验收。
