# Tunnel 实现

[`task/tunnel.rs`](../../os/kernel/src/task/tunnel.rs) 提供事件与接入授权。Connection 拥有共享 backing 的完整 root use；Endpoint 和 Invitation 没有地址空间字段。用户映射使用普通 MemoryMap/View，由地址空间独立管理，机制见 [mm](mm.md)。页内协议属于 [Runnel](runnel.md)。

## Connection、Endpoint 与 Invitation

Connection的CONNECTION锁拥有双侧Prepared/Invited/Attached/Closed和root owner；侧状态仅Weak持Endpoint/Invitation，防止Arc成为业务寿命。Endpoint/Invitation强持Connection，各自真实Arc布局出生付款；Connection也持自己的费用，末字段保持原payer至last-live。Create的Connection/初始Endpoint/Invitation由显式Pool付款，Attach出生Endpoint及其来源维护Account由接入调用者Pool付款。MemoryObjectCore提供固定长度/来源身份/授权，Connection独占BackingOwner。

Endpoint 允许 READ/WRITE/WAIT/SIGNAL/MANAGE，不允许复制、运输或 EXECUTE。Invitation 允许这些目标授权及 TRANSIT/GRANT，持有表项的范围决定 Attach 产出的 Endpoint 权限；它不允许等待和复制。接入权本身要求 MANAGE，映射随后要求 READ 或 READ|WRITE。权利收窄不能在 Attach 时恢复。

双方均 Closed 时，在 Connection 锁内取走唯一 root owner、锁外放弃。root 的内嵌维护记录退掉残余，但已发布 View 的 use 继续独立保留物理段；等待来源的普通 Arc loan 不保留 root。最后 View 退休及完成退款由 VM 记录推进。

## Create 与 Attach

Create 的固定宽请求在 [`shared/tunnel.rs`](../../shared/erhino_shared/src/tunnel.rs)：显式 Pool[CREATE]、非零字节数和结果地址，按页取整至最多 TUNNEL_MAX_PAGES。它出生 Connection、Endpoint、Invitation 和 root；没有 VA 选址或映射。调用者表预约两槽，锁外写 TunnelCreateResult，再锁外提交 Prepared→Attached/Invited，最后表 commit。输出失败时 Prepared 侧的 cap 归零不触碰真实侧，最后 Connection Drop 把未交付 root 移交原维护记录。

Attach 用 a0 接收 Invitation Handle，a1 返回单个 Endpoint Handle。Invocation 持表 claim 和短 BodyLease；在业务提交前取得 Endpoint/cap 与目标表预约。Connection 锁内复查本侧邀请身份与另一侧 Attached，将本侧改 Attached 并发布双方 PEER_ATTACHED。此后消费 Invitation、提交表项和最后持有事件均必成，表锁内不取 Connection 锁。准备失败保留邀请；对端关闭先提交则 Attach 失败，Attach 先提交则新 Endpoint 随后观察 PEER_CLOSED。接入成功不保证任何一方已经建立 View。

## 关闭与门铃

Endpoint 最后 authority 消散在 Connection 锁内提交本侧 Closed；本端来源置 CLOSED 并清 DATA/PEER_ATTACHED，对端置 PEER_CLOSED 并清 PEER_ATTACHED。Invitation 未接入时放弃同样关闭该侧；接入提交后其最后持有事件不再放弃已经移交的 Endpoint。关闭不触碰 ledger/PTE，不产生隐式 Unmap；普通 View 由显式 Unmap 或整空间回收结算。

EndpointNotify 与 EndpointAcknowledgeData 均要求 SIGNAL。Notify 只向已 Attached 的对端置 DATA；邀请未接入返回 ObjectNotAvailable，Closed 返回 ObjectClosed。Ack 只清本端 DATA。Notify/Ack/Attach/Close 的电平与侧状态在同一 Connection 锁内仲裁，关闭不会被迟到 DATA 覆盖。

Endpoint 内嵌 WaitQueue 和 WorkRecord。snapshot/attach/detach/单候选 step 都使用 Connection 同步域；更新电平只记录需要扫描，锁外激活来源 Work，锁外发送 Hint。等待为纯电平，提示只令执行 owner 重新检查；不持有消息或数据进展。PEER_ATTACHED 是接入授权事实，Runnel 使用自己的 Start 字段证明共享内存就绪。

## 用户态 owner

[`libkernel Endpoint`](../../user/libraries/libkernel/src/ipc/tunnel.rs) 只拥有 capability 与事件借用，[`Invitation`](../../user/libraries/libkernel/src/ipc/invitation.rs) 拥有接入授权。Create 返回 Endpoint、Invitation 和字节长度；Attach 成功消费 Invitation，失败返还 owner。输出校验异常保留已发布表项，不产生旧 EndpointCleanup/lease owner。

[`librunnel MappedEndpoint`](../../user/libraries/librunnel/src/mapping.rs) 组合 Endpoint 与独立 MappedRegion。它先普通 RW Map，再提供有界 SharedMemory 借用；safe 借用排除消费映射 owner。close 依次 Unmap 和 Close，失败保存剩余清理责任；Drop 各尝试一次，不同步自旋。View 与 root 来源权限不依赖 Endpoint 仍有 authority。

SharedMemory 的 RV64 控制字段使用自然对齐整数 load/store 与 acquire/release fence，数据使用字节访问，不建立不可信共享 slice。公共 unsafe 构造要求完整、稳定、对齐的 RW 普通 RAM 映射，并明确借用寿命；MappedEndpoint 为其提供实际证明。取证见 [共享访问](../../plans/ref-2026-09-shared-memory-access.md)。

## 当前限制

Connection/Endpoint/Invitation的Arc与内嵌来源记录均按实际布局付款，Backing/Views/节点/数组各持费用；birth失败、退出和last-live沿原owner收束，普通heap仍fallible。用户态使用独立事件端点、Invitation与普通View，关闭不改变映射owner或原payer。内部输出撤销、逐点金融拒绝/allocator OOM和跨hart尾根窗口的证据边界只由[对象链计划](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)记录。
