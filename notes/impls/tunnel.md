# Tunnel 实现

Tunnel 是内核提供的共享内存连接对象：`Connection` 持有共享 backing 与两端关系，每个 `Endpoint` 持有本地地址空间中的 lease，`Invitation` 是一次性对端接入 capability。内核不解析页内协议；协议实现见 [`runnel.md`](runnel.md) 及对应的用户态库。

## Connection、Endpoint 与 Invitation

当前实现位于 `os/kernel/src/task/tunnel.rs`。`ConnectionState` 保存两侧 lease 与 `Alive`、`Invited`、`Closed` 状态；`Connection` 持 `Arc<MemoryObjectCore>`（对象身份、单页 `ObjectBacking`、可执行发布状态机与 backing metadata owner 的统一 core，见 [`memory-object.md`](memory-object.md)），复用 `memory_space` 的对象授权和 `WritePermit` 基元，但不向用户公开独立 MemoryObject Handle。

当前 Tunnel 对外仍是单页，但内核侧已走多段对象投影路径——单页只是长度为一的退化情形，多页几何只需改变投影区间。backing 由 `MemoryObjectCore` 持有——创建进程绑定的 MemoryPool 经 funded broker 支付，随对象持物理 extent 与 Pool charge。`Endpoint` 与 `Invitation` 各持 `EndpointPermit` / `InvitationPermit`，attach 端由附着进程支付，与创建端分账；backing 持 `ObjectBackingPermit`，view 所有权（对象强引用 + `ObjectViewPermit`）与 `WritePermit` 的生命周期归 AddressSpace 统一管理（见 [`mm.md`](mm.md)）。Tunnel view 是 object-owned lease：`ObjectMappingLease` 记录位置、对象内偏移与权限，撤销与退役 fragment 复核都以它为凭据；单页容量和释放事实由本篇记录。显式 close 通过内存事务在 Commit 前预留 bounded work debt；REAPABLE 后的 detached close 只提交逻辑关闭，映射资源由 ProcessDrain 收束。

`Endpoint` 是可等待对象，允许 `WAIT | SIGNAL | MANAGE`，可观察 `DATA | PEER_CLOSED | CLOSED`，不可进入 TRANSIT/GRANT。`Invitation` 允许 `MAP | TRANSIT | GRANT`，不可等待；它不可复制，成功 attach 后消费，失败不消费。Endpoint 与本进程地址空间 lease 绑定，不能通过 Handle 运输。

## Create 与 Attach

`TunnelCreate` 先为 Connection（`ConnectionPermit`）、Endpoint、Invitation、共享 backing 与两侧 Handle 预留 metadata，再在地址空间中建立创建端映射。映射、页表、输出槽、Handle 或 metadata 任一提交前步骤失败，事务回滚且不发布对象或消费资源。

`TunnelAttach` 从 Invitation 取得 Connection 的实际映射几何，在接入进程预留完整 object-backed view 和页表资源；只有映射准备、Handle 输出和 AddressSpace Commit 全部成功后，才在线性化点消费 Invitation、安装对端 Endpoint 并发布同步请求。Connection 已关闭或 Invitation 已放弃时，Attach 返回终态错误。

创建与接入的对象授权、WritePermit、MemoryChange、Remote 确认和资源退款由 [`mm.md`](mm.md) 的统一事务核拥有；本篇只记录 Tunnel 如何把这些机制组合成两端连接。Create/Attach 的映射准备收敛为 `plan_side_mapping`：锁外取得投影与 view 所有权，重入 AddressSpace 组装事务，失败路径统一走 `abandon_mapping`。

## 关闭与终止接管

显式 Endpoint close 在摘除 Handle 前预留完整 lease 撤销事务。本端 side state 提交为 Closed，幸存端收到 `PEER_CLOSED`；本端和等待者收到 `CLOSED`，已经发布的映射不会在 stale translation 确认前提前拆除。

进程 REAPABLE 后，`close_detached` 在 Connection 锁内取走 lease 并提交 side Closed，锁外发布关闭通知；不创建 `RetiringSpaceChange`、lease retire sink 或另一笔 Unmap。Handle 阶段之后由 ProcessDrain 逐区域归还 view/WritePermit，再收束 backing/PTE。`pending_close` 只承接 Handle 摘除与 close 之间的预算切分。具体资源游标由 [`mm.md`](mm.md) 唯一拥有。

Invitation 在未 attach 前被关闭或进入 transit 清理时，连接一侧转为 Closed，并向创建端发布 `PEER_CLOSED`。Attach 与 Invitation 放弃在同一 Connection 状态锁下竞争，旧 generation 不能重放。

## 门铃与等待

`TunnelNotify` 要求 Endpoint 的 SIGNAL right，向对端置 `DATA` 电平；它只提示重新检查共享区，不携带数据计数。`TunnelAcknowledgeData` 要求 MANAGE right，仅清除本端 DATA 电平，不确认终态。调用者以 WaitMany 等待 `DATA | PEER_CLOSED | CLOSED`，醒来后重新检查页内协议。

Endpoint 的等待订阅复用通用 ObjectWaitState/WaitContext；WaitContext、TimerQueue 和 WaitMany 实现见 [`ipc.md`](ipc.md)。Tunnel 不拥有页内数据格式、游标、记录边界或 buffer ownership。

## 验证入口

`test_hammer::concurrent_tunnel_close` 覆盖 Endpoint close 与同地址空间普通 Unmap 的 8 轮并发；`tunnel_exit_target` 留存 Endpoint，由 stress 的 16 轮进程退出验证 ProcessDrain 接管。core 检查正常 Create/Attach/close、peer 状态及 Pool charge 退款。

`os/kernel/src/task/tunnel/selftest.rs` 在调度开始前建立隔离 Building fixture，调用真实 Create/Attach 验证 VA Conflict、无效输出和完整 Prepare 后无 Running 提交资格的回滚。创建端持真实已提交 view，Attach 失败后核对 Invitation 保留、无目标 PTE 和 write_views 不变。表页 funding 以暂存全部可用额度触发 QuotaExceeded；测试 owner 真实占满堆，投影准备返回 NoFrame，Attach 返回 OutOfMemory，结束后释放全部压力分配，无生产故障开关。

输出竞态用确定性顺序固定：输出初检成功→真实 MemoryUnmap 撤销输出页→Tunnel Prepare→正式 deliver_output 复检失败→abandon_mapping。fixture 经 lifecycle 的 Running/active 与离场接口模拟调用者，检查 Fault 终因、Invitation 未消费、permit/PTE 回滚；最终以每批 `max_work=1` 的重复 ProcessDrain 归还全部 owner，比较 Pool/frame 和 16 类 metadata admission 库存。该用例验证真实内核事务与写回失败 seam，不依赖概率窗口。

`test_hammer::tunnel_close_attach` 在用户态执行三组各 8 轮：Attach 先完成、creator close 先完成、并发竞争。成功必须消费 Invitation；失败必须保留可关闭的 Invitation；两端关闭后重复 close 必须拒绝，每轮以普通映射重用两端 VA 验证 lease/PTE 已撤销。所有新用例由 acceptance 强制锚点检查，原 [`D-1 P2-D1-03`](../../plans/archived/review-2026-09-mechanism-generalization.md) 保留提交后复核记录。用户态页内协议见 [`runnel.md`](runnel.md)。
