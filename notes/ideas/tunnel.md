# 隧道

隧道是 IPC 的**数据面机制**：内核创建一段固定长度、有硬容量上限的多页共享 backing 及其两个本地端点。内核不解释共享区格式；页内协议必须遵守[共享内存协议公共契约](shared-memory.md)。[Runnel](runnel.md) 是官方单工 FIFO 字节流，[BufferQueue](buffer-queue.md) 是并列的记录与缓冲交接协议。

## Backing 与容量

`TunnelCreate` 由创建进程绑定的 [MemoryPool](mm.md) 支付零态 backing。长度非零、按页取整，并同时受最大页数与最大 extent 数硬上限约束；逻辑区间连续不要求物理连续。Connection 使用与公共 MemoryObject 相同的 backing core、extent 投影和 WritePermit 机制，但不向任一端返回可独立映射或转授的 MemoryObject Handle。删除 Tunnel 模块后，参与方关系、门铃和 object-owned lease 复杂度会散回调用者；删除共享 backing core 则会在 Tunnel 与公共 MemoryObject 间产生两套帧所有权，因此两者只共享内部实现，不合并外部 interface。

Tunnel 不预留固定“控制页”。页内协议自行声明 header 与数据区几何；若控制区和数据区具有相同 RW 权限，按整页分区不能增加隔离，只会浪费容量。未来需要不同映射权限或独立生命周期时，应使用多个 MemoryObject 或新的多区域协议，不在 Tunnel 中暗藏区域表。

## 建立与邀请

`TunnelCreate(bytes, placement)` 建立持有 backing 的 Connection，并向创建者交付本地完整区间的 Endpoint、一次性 peer invitation 与规范化后的映射几何。Endpoint 与本进程的 object-owned 地址空间 lease 绑定，不可 duplicate、TRANSIT 或 GRANT；invitation 不可 duplicate，可按授权 TRANSIT 或直接 GRANT 给预期对端。它不是随机字符串、全局登记 key 或可猜测 bearer id。

持 invitation 的一方调用 `TunnelAttach`，在本地选择合法映射位置；内核从 Connection 读取实际长度，预留完整多页 ObjectView，成功时原子消费 invitation 并返回 Endpoint 与映射几何。映射、权限、输出空间、页表 charge 或其它预留失败均不消费 invitation。A 端先关闭使 invitation 终态且 attach 失败；invitation 被丢弃通知 A 对端放弃；attach 先完成则双方进入存活关系。

## 映射、Connection 与关闭

Connection 独占共享 backing 与两端参与方关系。每个端点按[内存模型](mm.md)把覆盖完整 backing 的 object-backed view 绑定到所在进程的内部 lease；区域与普通 mapping 进入同一冲突账本，但普通地址空间操作不能替换、切割或解除。端点 close 在消费 Handle 前预留完整 lease 撤销事务，提交后由 AddressSpace 持有 retire 状态和 Connection 强引用；显式调用者在相关 hart 确认失效后完成，进程 drain 则保留可恢复进度。

一端关闭不突然拆除幸存端映射，幸存端以 `PEER_CLOSED` 得知页内对端数据已不可信并按协议进入 Broken。两端参与方都关闭只结束逻辑关系；全部 retiring view 完成后，最后一个 Connection 引用才释放 extents 并把 charge 归还创建者的 MemoryPool。关闭不可复活。attach 在 Connection 的单一线性化点竞争 invitation 与关闭：提交前任何失败完全回滚，提交后不再有可失败步骤。

## 门铃与对象状态

每个 Endpoint 是可等待对象。对端调用 `TunnelNotify` 时，内核置本端 `DATA`；它只是提示重新检查控制块，不计数、不证明数据存在。端点拥有者只在页内协议达到无进展条件后调用 `TunnelAcknowledgeData` 确认本端 `DATA`，再重查控制块并以 WaitMany 等待 `DATA`、`PEER_CLOSED` 或 `CLOSED`。不存在通用信号清除，终态位不能确认。

隧道机制方向中立：共享 backing 与两个端点不区分读写。单工、双工、记录边界、门铃时机与缓冲所有权由页内协议定义；双工字节流仍使用两条反向单工隧道。

## 边界

创建、attach、通知、等待和关闭都要求对应 role 与 rights。Endpoint 是可等待对象；Invitation 只承担一次性 MAP/TRANSIT/GRANT authority，不公开 WAIT 或 ObjectSignals。内核只验证对象关系、映射生命周期和状态发布；不验证 attach 者业务身份，也不解析共享区。隧道没有全局 id、按 PID 查找的端点、永久 registry、descriptor discipline 或内核强制的 buffer ownership。

Invitation 是纯能力：持有即授权，内核不区分误投递与有意委托。撤销粒度是整条隧道；不存在撤回邀请但保留同一 A 端连接的操作。单次消费加 Handle generation 退休使旧 invitation 不可重放。
