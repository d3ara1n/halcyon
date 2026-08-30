# Runnel

Runnel 是 Halcyon 的官方流式数据交换协议：运行在[隧道](tunnel.md)共享区间上的**单工 SPSC FIFO 字节流**。本规范与[共享内存协议公共契约](shared-memory.md)共同构成互操作要求；固定宽度、little-endian、对齐、Acquire/Release、角色视图、不信任对端和 Broken 规则均为规范义务。

Runnel 只回答字节流，不携带记录、Handle、MemoryObject 注册或 buffer ownership。需要记录边界、scatter/gather 或零拷贝缓冲交接时使用并列的 [BufferQueue](buffer-queue.md)，不能把 descriptor ring 解释成 Runnel 的内部实现。

## RNL2 布局

共享区至少一页，固定使用前 128 B 控制块，剩余全部为环形数据区。Tunnel 返回的规范化映射长度为 `total_bytes`，`data_offset = 128`，`capacity = total_bytes - data_offset`。控制块和数据区同权时不拆成独立控制页。

| 偏移 | 类型 | 字段 | 唯一写者 |
|---|---|---|---|
| `0x00` | 对齐原子 `u32` | `MAGIC = 0x324C4E52`（小端字节为 `RNL2`） | 初始化者，最后发布 |
| `0x04` | 对齐原子 `u32` | 低 16 位 `VERSION = 2`，高 16 位 `header_bytes = 128` | 初始化者 |
| `0x08` | 对齐原子 `u64` | `total_bytes` | 初始化者 |
| `0x10` | 对齐原子 `u64` | `capacity` | 初始化者 |
| `0x18` | 对齐原子 `u64` | `head`：累计写入字节数 | 生产者 |
| `0x20` | 对齐原子 `u64` | `tail`：累计读取字节数 | 消费者 |
| `0x28` | 对齐原子 `u32` | `eof`：流结束标记（0/1） | 生产者 |
| `0x2C` | 对齐原子 `u32` | `flags = 0` | 初始化者 |
| `0x30–0x7F` | — | 保留，必须为零 | — |

初始化者清零完整控制块，以原子 relaxed store 写入版本、几何、游标、EOF 与 flags，最后以 release store 发布 MAGIC。attach 方先以 acquire 读取 MAGIC，再用原子 load 各取得一次版本和几何，验证保留区、`total_bytes` 与本地 Tunnel 映射完全相等、`capacity = total_bytes - 128` 且容量位于协议硬上限内，随后把几何冻结为本地 shadow；正常传输不再读取共享几何字段。对端以后篡改这些初始化字段不会改变本地寻址，若经诊断复检发现则 Broken。RNL1 不构成兼容面，升级时两侧同步替换。

## 游标、读写与 EOF

游标是 `u64` 自由计数而非数组下标，差值按模 2^64 回绕：`used = head -% tail`，且必须在 `0..=capacity`。capacity 必须小于 2^63，使局部 shadow 所验证的前进距离无歧义。空当且仅当 `head == tail`，满当且仅当 `used == capacity`。

生产者独占 `head` 和 `eof`，消费者独占 `tail`；双方不得写对方字段。生产者先写数据，再 release 发布 head；消费者 acquire 读取 head 后读数据，再 release 发布 tail；生产者 acquire 读取 tail 后才覆写。双方保存最近一次已接受的对端游标：生产者只接受 tail 前进不超过此前 outstanding，消费者只接受 head 前进不超过此前 free；更新 shadow 后仍须满足 `used <= capacity`，每次复制长度不得超过本地验证容量。违反任一条件即 Broken。

生产者在最后一次 head 发布后以 release 置 `eof = 1`，此后不再写入。消费者先 acquire 读取 EOF，再 acquire 读取并冻结最终 head；只有已观察 EOF 且 tail 追上该最终 head 才是正常 EOF。`PEER_CLOSED` 或页内错误不是 EOF。

## 门铃与阻塞循环

会阻塞的封装在每次写入、腾出空间或发布 EOF 产生正进展后，都必须在等待或返回前通知对端；批量实现至少保证空到非空、满到非满和 EOF 等关键转换得到通知。双方明确约定永不阻塞的纯轮询模式可以省略门铃。门铃只提示状态改变，真实可读量和可写量始终来自控制块。

阻塞角色反复执行：检查控制块；无进展则 `TunnelAcknowledgeData`；重新检查；仍无进展才以 WaitMany 等待 `DATA | PEER_CLOSED | CLOSED`；醒来后从头检查。消费者只在读至空后确认 DATA，生产者只在无空间时确认并等待腾空提示。

## 分工

| 关注点 | 归属 |
|---|---|
| 多页 backing、映射、端点、邀请、关闭与 `PEER_CLOSED` | Tunnel |
| DATA、WaitMany 与门铃调用 | 对象状态和等待机制 |
| 布局、角色、游标、内存序、EOF 与 Broken | Runnel |
| 记录、region 注册、descriptor 与 buffer 交接 | BufferQueue |
| 连接身份鉴权与请求语义 | 上层服务协议 |
