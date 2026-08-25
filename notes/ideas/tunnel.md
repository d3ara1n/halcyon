# 隧道

隧道是 IPC 的**数据面**：内核创建的一页共享内存及其两个本地端点。内核不解释页内格式；页内协议必须遵守[共享内存协议公共契约](shared-memory.md)。[Runnel](runnel.md) 是官方单工 FIFO 协议。

## 建立与邀请

`TunnelCreate` 分配零态页，建立持有该帧的 Connection，并向创建者交付一个映射在本地的 Endpoint Handle 与一次性 peer invitation Handle。Endpoint 不可 duplicate、不可 transfer；invitation 不可 duplicate，只能以 `TRANSFER` 经消息交给预期对端。它不是随机字符串、全局登记 key 或可猜测 bearer id。

持 invitation 的一方调用 `TunnelAttach`，在本地选择合法映射位置后原子消费 invitation 并取得第二个 Endpoint。Connection 创建时 A 端存活、B 端为 invitation；invitation 被丢弃即通知 A 对端放弃；A 先关闭使 invitation 终态且 attach 失败；attach 先完成则 B 存活，此后 A 关闭通知 B。映射、权限、输出空间或预留失败均不消费 invitation。

## 映射、Connection 与关闭

Connection 独占共享帧与两端参与方关系。每个端点绑定所在进程的 object-owned VM reservation；普通 VM 操作不能替换或占用该 reservation。端点 lease 关闭时立即撤销本端映射；一端关闭不突然拆除幸存端映射，幸存端以 `PEER_CLOSED` 得知页内对端数据已不可信并按协议进入 Broken。当且仅当两端参与方都已关闭，Connection 才归还帧。

关闭不可复活。attach 在 Connection 的单一线性化点竞争 invitation 与关闭：提交前的任何失败完全回滚，提交后不再有可失败步骤。进程退出 drain 与显式关闭使用同一语义。

## 门铃与对象状态

每个 Endpoint 是可等待对象。对端调用 `TunnelNotify` 时，内核置本端 `DATA`；它只是提示重新检查控制块，不计数、不证明数据存在。端点拥有者只在页内协议达到无进展条件后，调用专用 `TunnelAcknowledgeData` 确认本端 `DATA`，再重查控制块并以 WaitMany 等待 `DATA`、`PEER_CLOSED` 或 `CLOSED`。不存在通用信号清除，终态位不能确认。

隧道机制方向中立：一页和两端点不区分读写。单工、双工、门铃语义及确认时机由页内协议定义；双工通信使用两条反向单工隧道。

## 边界

创建、attach、通知、等待和关闭都要求 Endpoint 或 invitation 的合法 role 与 rights。内核只验证对象关系、映射生命周期和状态发布；不验证 attach 者业务身份，也不解析共享页。隧道没有全局 id、按 PID 查找的端点或永久 registry。
