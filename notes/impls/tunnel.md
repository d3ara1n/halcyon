# Tunnel 实现

Tunnel 是内核提供的共享内存连接对象：`Connection` 持有共享 backing 与两端关系，每个 `Endpoint` 持有本地地址空间中的 lease，`Invitation` 是一次性对端接入 capability。内核不解析页内协议；协议实现见 [`runnel.md`](runnel.md) 及对应的用户态库。

## Connection、Endpoint 与 Invitation

当前实现位于 `os/kernel/src/task/tunnel.rs`。`ConnectionState` 保存共享帧、两侧 lease 与 `Alive`、`Invited`、`Closed` 状态；`Connection` 另持内部 `MemoryObjectState`，复用 `memory_space` 的对象授权和 `WritePermit` 基元，但不向用户公开独立 MemoryObject Handle。

当前 Tunnel backing 是单页 `FrameTracker`。该实现的容量、extent 和释放事实由本篇记录，底层帧与地址空间所有权见 [`mm.md`](mm.md)。

`Endpoint` 是可等待对象，允许 `WAIT | SIGNAL | MANAGE`，可观察 `DATA | PEER_CLOSED | CLOSED`，不可进入 TRANSIT/GRANT。`Invitation` 允许 `MAP | TRANSIT | GRANT`，不可等待；它不可复制，成功 attach 后消费，失败不消费。Endpoint 与本进程地址空间 lease 绑定，不能通过 Handle 运输。

## Create 与 Attach

`TunnelCreate` 先为 Connection、Endpoint、Invitation、共享 backing、对象 view 和两侧 Handle 预留资源，再在地址空间中建立创建端映射。映射、页表、输出槽、Handle 或 metadata 任一提交前步骤失败，事务回滚且不发布对象或消费资源。

`TunnelAttach` 从 Invitation 取得 Connection 的实际映射几何，在接入进程预留完整 object-backed view 和页表资源；只有映射准备、Handle 输出和 AddressSpace Commit 全部成功后，才在线性化点消费 Invitation、安装对端 Endpoint 并发布同步请求。Connection 已关闭或 Invitation 已放弃时，Attach 返回终态错误。

创建与接入的对象授权、ObjectView、WritePermit、MemoryChange、Remote 确认和资源退款由 [`mm.md`](mm.md) 的统一内存事务拥有；本篇只记录 Tunnel 如何把这些机制组合成两端连接。

## 关闭与终止接管

显式 Endpoint close 在摘除 Handle 前预留完整 lease 撤销事务。本端 side state 提交为 Closed，幸存端收到 `PEER_CLOSED`；本端和等待者收到 `CLOSED`，已经发布的映射不会在 stale translation 确认前提前拆除。

进程进入 drain 后，detached close 把已提交的 `RetiringSpaceChange` 与 lease retire sink 保存在 Endpoint 的固定状态中，由后续 drain 批次继续推进。未完成时 entry 保留在 `pending_close`，不建立同步扫描或第二套回收路径。该退役游标、work debt、Remote ack 和最终资源退款的具体步骤由 [`mm.md`](mm.md) 唯一拥有。

Invitation 在未 attach 前被关闭或进入 transit 清理时，连接一侧转为 Closed，并向创建端发布 `PEER_CLOSED`。Attach 与 Invitation 放弃在同一 Connection 状态锁下竞争，旧 generation 不能重放。

## 门铃与等待

`TunnelNotify` 要求 Endpoint 的 SIGNAL right，向对端置 `DATA` 电平；它只提示重新检查共享区，不携带数据计数。`TunnelAcknowledgeData` 要求 MANAGE right，仅清除本端 DATA 电平，不确认终态。调用者以 WaitMany 等待 `DATA | PEER_CLOSED | CLOSED`，醒来后重新检查页内协议。

Endpoint 的等待订阅复用通用 ObjectWaitState/WaitContext；WaitContext、TimerQueue 和 WaitMany 实现见 [`ipc.md`](ipc.md)。Tunnel 不拥有页内数据格式、游标、记录边界或 buffer ownership。

## 验证入口

内核 Tunnel host/QEMU 验证覆盖 Create/Attach 的失败原子性、Invitation consume-on-success、权限与状态、跨 hart close、Endpoint drain 接管及 Pool/frame/PTE/Handle/permit 守恒。用户态页内协议不在本篇重复验证，见 [`runnel.md`](runnel.md)。
