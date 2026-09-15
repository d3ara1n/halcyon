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

初始化者通过共享访问原语初始化完整控制块，以指定宽度原子 relaxed store 写入版本、几何、游标、EOF 与 flags，保留字节置零，最后以 release store 发布 MAGIC；完成后才向上层交出 Invitation。attach 方先以 acquire 读取 MAGIC，再用原子 load 各取得一次版本和几何，验证版本/header 长度、flags、`total_bytes` 与本地 Tunnel 映射完全相等、`capacity = total_bytes - 128` 且容量位于协议硬上限内，随后把几何冻结为本地 shadow。读方忽略保留字节，未知 flags 拒绝；正常传输不再读取共享几何字段。对端以后篡改这些初始化字段不会改变本地寻址，若经诊断复检发现则 Broken。RNL1 不构成兼容面，升级时两侧同步替换。

## 游标、读写与 EOF

游标是 `u64` 自由计数而非数组下标，差值按模 2^64 回绕：`used = head -% tail`，且必须在 `0..=capacity`。capacity 必须小于 2^63，使局部 shadow 所验证的前进距离无歧义。空当且仅当 `head == tail`，满当且仅当 `used == capacity`。

累计进度不能直接对 capacity 取余用于寻址：一般容量不整除 2^64，整数回绕会改变余数。每个角色独立持有从零开始的本地物理 cursor，仅按该角色实际完成的字节数对 capacity 取余推进；累计进度只用于可读量/可写量和对端前进量检查。复制从物理 cursor 起至多分两段，数据流跨整数回绕仍保持连续。

一条连接上的每个角色只建立一次，不支持从任意旧进度重建角色。attach 的生产者必须验证本方尚未写入或发布 EOF；attach 的消费者必须验证本方尚未读取，允许创建方生产者已发布不超过容量的首批数据/EOF。角色从零产生这一前提使两端物理位置可归纳一致，无需再在共享区复制一套位置真值。

生产者独占 `head` 和 `eof`，消费者独占 `tail`；双方不得写对方字段。生产者先写数据，再 release 发布 head；消费者 acquire 读取 head 后读数据，再 release 发布 tail；生产者 acquire 读取 tail 后才覆写。双方保存最近一次已接受的对端游标：生产者只接受 tail 前进不超过此前 outstanding，消费者只接受 head 前进不超过此前 free；更新 shadow 后仍须满足 `used <= capacity`，每次复制长度不得超过本地验证容量。违反任一条件即 Broken。

生产者在最后一次 head 发布后以 release 置 `eof = 1`，此后不再写入。消费者先 acquire 读取 EOF，再 acquire 读取并冻结最终 head；只有已观察 EOF 且 tail 追上该最终 head 才是正常 EOF。已经观察到 EOF 后，其回落或最终 head 再变化均为 Broken。`PEER_CLOSED` 或页内错误不是 EOF。

## 门铃与阻塞循环

会阻塞的封装在每次写入、腾出空间或发布 EOF 产生正进展后，都必须在等待或返回前通知对端；所有可与阻塞封装混用的公开读写入口遵守同一规则。只有先建立带明确排序证明的等待意图或事件代次握手，才能按空到非空、满到非满等边沿省略通知；观察环状态发生边沿本身不构成无丢唤醒证明。双方明确约定永不阻塞的纯轮询模式可以省略门铃，但不能与阻塞模式无约定混用。

对端仍为 Invited 时没有现存接收者需要唤醒，Attach 的首次检查必须发现此前发布的数据/EOF；这与对端已关闭是不同状态。门铃只提示状态改变，真实可读量和可写量始终来自控制块。

数据进度一经发布不能因后续通知失败而回滚。公开操作必须同时报告错误和已经完成的字节数，批量操作累计整次调用的进度，避免调用者重试整段造成重复数据。EOF 已发布后的通知失败同样不撤销 EOF。

所有驱动方式共用非阻塞推进与等待准备：检查控制块；无进展则 acknowledge DATA；重新检查；仍无进展才等待 DATA、PEER_CLOSED 或 CLOSED。阻塞门面使用 WaitMany，服务事件循环使用 WaitSet；醒来都从真实控制块重查。消费者只在读至空后确认 DATA，生产者只在无空间时确认并等待腾空提示。

角色独占 Endpoint，向执行框架只交付安全的观察注册能力，不导出原始 Handle 或映射访问权。建立阶段可观察 Tunnel 的 PEER_ATTACHED，实际协议验证仍由 attach 方完成。Attach 错误必须明确区分 Invitation 尚未消费与已消费后协议失败，清理 owner 随错误保留。

等待条件是三个互不相同的观察事实——可写空间出现、已发布 EOF 后对端消费至最终 head、对端映射建立——不能合并为一个布尔结果；每次唤醒后从控制块重查并返回带领域载荷的类型化结果。登记按当前等待条件选择信号：对端建立是持久电平，满足后不再重复登记；数据条件随 acknowledge→重查→再等待循环逐轮重新登记。登记寿命由执行核心拥有，角色只声明当前等待什么；业务不手工编排 acknowledge、重 arm 或 token。

生产者的“已发布 EOF 且对端已消费全部字节”可以作为上层完成的一个前置事实；后端写入、持久性、最终状态和取消仍由上层控制协议决定。Runnel 不把传输 EOF 或 Endpoint 关闭转换成文件操作成功。

## 分工

| 关注点 | 归属 |
|---|---|
| 多页 backing、映射、端点、邀请、Attach 状态与关闭 | Tunnel |
| DATA、WaitMany、WaitSet 与门铃调用 | 对象状态和等待机制 |
| 布局、角色、游标、内存序、EOF 与 Broken | Runnel |
| 记录、region 注册、descriptor 与 buffer 交接 | BufferQueue |
| 连接身份鉴权与请求语义 | 上层服务协议 |
