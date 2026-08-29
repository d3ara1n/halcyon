# 隧道

隧道是 IPC 的**数据面**：内核创建一段有硬容量上限的共享 backing 及其两个本地端点。内核不解释页内格式；页内协议必须遵守[共享内存协议公共契约](shared-memory.md)。[Runnel](runnel.md) 是官方单工 FIFO 协议。

## 建立与邀请

`TunnelCreate` 分配零态共享 backing，建立持有它的 Connection，并向创建者交付一个映射在本地的 Endpoint Handle 与一次性 peer invitation Handle。Endpoint 与本进程的 object-owned 地址空间 lease 绑定，不可 duplicate、TRANSIT 或 GRANT；invitation 不可 duplicate，可按授权 TRANSIT 或直接 GRANT 给预期对端。它不是随机字符串、全局登记 key 或可猜测 bearer id。

持 invitation 的一方调用 `TunnelAttach`，在本地选择合法映射位置后原子消费 invitation 并取得第二个 Endpoint。Connection 创建时 A 端存活、B 端为 invitation；invitation 被丢弃即通知 A 对端放弃；A 先关闭使 invitation 终态且 attach 失败；attach 先完成则 B 存活，此后 A 关闭通知 B。映射、权限、输出空间或预留失败均不消费 invitation。

## 映射、Connection 与关闭

Connection 独占共享 backing 与两端参与方关系；backing 的固定长度不改变对象关系与关闭语义，页内协议以自身版本字段演进。每个端点按[内存模型](mm.md)把一个 object-backed view 绑定到所在进程的内部 lease；该区域与普通 mapping 进入同一冲突账本，但普通地址空间操作不能替换、切割或解除。端点 close 在消费 Handle 前预留 lease 撤销事务，提交后由 AddressSpace 持有 retire 状态和 Connection 强引用；显式调用者在相关 hart 确认失效后完成，进程 drain 则保留可恢复进度。一端关闭不突然拆除幸存端映射，幸存端以 `PEER_CLOSED` 得知页内对端数据已不可信并按协议进入 Broken。两端参与方都关闭只结束逻辑关系；全部 retiring view 完成后，最后一个 Connection 引用才释放 backing。

关闭不可复活。attach 在 Connection 的单一线性化点竞争 invitation 与关闭：提交前的任何失败完全回滚，提交后不再有可失败步骤。进程退出与显式关闭消费同一 lease 撤销机制，只在调用者等待方式和 active-hart 集合上不同。

## 门铃与对象状态

每个 Endpoint 是可等待对象。对端调用 `TunnelNotify` 时，内核置本端 `DATA`；它只是提示重新检查控制块，不计数、不证明数据存在。端点拥有者只在页内协议达到无进展条件后，调用专用 `TunnelAcknowledgeData` 确认本端 `DATA`，再重查控制块并以 WaitMany 等待 `DATA`、`PEER_CLOSED` 或 `CLOSED`。不存在通用信号清除，终态位不能确认。

隧道机制方向中立：共享 backing 与两个端点不区分读写。单工、双工、门铃语义及确认时机由页内协议定义；双工通信使用两条反向单工隧道。

## 边界

创建、attach、通知、等待和关闭都要求对应 role 与 rights。Endpoint 是可等待对象；Invitation 只承担一次性 MAP/TRANSIT/GRANT authority，不公开 WAIT 或 ObjectSignals。内核只验证对象关系、映射生命周期和状态发布；不验证 attach 者业务身份，也不解析共享页。隧道没有全局 id、按 PID 查找的端点或永久 registry。

Invitation 是纯能力：持有即授权，内核不区分误投递与有意委托，投递责任在转移方。撤销粒度是整条隧道——创建者关闭自己端点使 invitation 终态、attach 失败，已 attach 的对端收 `PEER_CLOSED`；不存在撤回邀请但保留隧道的操作。由于每个 side 只有一个槽位，这个粒度与「换对端必须旧端退场」的参与方关系自洽。单次消费加 generation 退休使 invitation 不可重放：观察过旧值的人无法复用。
