# IPC 数据面演进设计

> 【未来设计计划】MemoryObject 的 backing/mapping 分层、固定长度与硬容量上限、mapping 独立保活、`Mutable → Sealing → Executable`/WritePermit 单向 seal，以及统一 AddressSpace/MemoryChange seam 已由 `notes/ideas/{mm,object}.md` 拥有，本计划不重议。这里等待真实负载后设计公共对象 ABI、帧移交、描述符协议及 Tunnel/Runnel 的数据面组合。触发顺序：用户内存映射机制完整化 → ThreadSpawn 与 IPC 压力线收口 → 启动本设计。

## 驱动问题

当前 Tunnel 提供单页共享映射、两端参与方、门铃与关闭；Runnel 在该页上定义
单工 FIFO 字节流。它已经满足现有控制面与 8 KiB 跨进程流验证，但尚未回答
更大数据集、记录边界、页级 backing 复用、共享与独占移交等需求。

不能仅因「描述符环更常见」替换现有字节环，也不能把原始帧、映射、
capability 运输和队列协议合并成一个新 syscall。开始设计前必须先用 FAL、
ELF/文件缓存、驱动 I/O 或 IPC 压力结果明确负载和所需语义。

## 设计问题

### 1. 数据语义

- 调用者需要字节流、定长元素、记录队列还是 scatter/gather；
- 数据是复制、双方共享、只读发布，还是内核强制的独占移交；
- 背压、EOF、取消、peer close、损坏后的 Broken 分别属于哪一层；
- 性能目标来自 syscall 次数、复制量、容量还是 cache/TLB 行为。

### 2. 机制分层

在已确认的内存对象、AddressSpace 与映射事务契约下，重新证明以下模块的接口：

- Mailbox 的控制消息与 capability move；
- Tunnel 的参与方、门铃和生命周期如何消费 object-owned mapping lease；
- Runnel 的 FIFO 字节流协议；
- 固定长度 MemoryObject 如何按真实负载组织共享、只读发布和派生 view；
- 描述符协议如何引用预注册区域，而不是把进程本地 Handle 数值写进共享内存。

删除测试：若拿掉某一模块，其复杂度是否会散回多个调用者；若不会，该模块
只是传递层，不应存在。

### 3. MemoryObject 数据面用法与帧移交

MemoryObject 的稳定前提是：对象持 backing、mapping lease 借 view；capability move 只移动建立新 view 的 authority，不撤销旧 view；普通 close 的 backing 工作量受硬容量上限约束；`Mutable → Sealing → Executable` 与覆盖 reserved/published/retiring 的 WritePermit 遵守 backing 级 W^X。

本计划仍须由负载决定：

- 公共对象创建、seal、查询及映射 ABI 的最小 interface；
- 多对象 scatter/gather 与容量上限如何呈现给协议；
- shareable/immutable backing 与 affine exclusive ownership 是否需要不同对象或 role；
- 独占帧移交所需的 duplicate 禁止、lease 归零、撤销和回收线性化；
- DMA/设备 ownership 与 CPU mapping 的同步关系。

未经上述完整设计，不增加“帧移交”第二机制或占位 syscall。

### 4. Tunnel/Runnel 演进

- 多页 extent 是否只扩大字节流容量，还是控制页与数据页分区；
- 描述符环若只引用同页内字节，是否反而降低容量且没有零拷贝收益；
- 新的记录/BufferQueue 协议应与 Runnel 并列，还是能在不放大调用者接口的
  前提下作为其内部实现；
- 版本、角色唯一写者、Acquire/Release、shadow 校验、门铃确认闭环和 Broken
  是否仍可独立证明。

## 工作流程

1. 固定开始 commit，收集 ThreadSpawn 后 IPC 压力线与目标负载数据；
2. 从 `references/INDEX.md` 所列规范和补充的成熟系统官方资料建立事实表；
3. 给出至少两种完整设计，比较接口深度、所有权、收束上界和验证成本；
4. 只把负载推出的新方向写入对应 ideas 拥有篇，不重复定义既有 MemoryObject/映射契约；
5. 按确认后的模块归属拆出独立实施计划；
6. 实现后才写 `notes/impls/`，不以计划或构想冒充现状。

## 前置依赖

- `todo-2026-09-user-memory-mapping.md` 已收口，统一 AddressSpace/MemoryChange、lease 与跨 hart 完成 seam 稳定；
- `todo-2026-09-thread-model.md` 批二、批三已收口；
- carryover IPC 压力线给出并发和资源守恒证据；
- 至少一个真实消费者证明现有 Tunnel/Runnel 不足，而非仅有构想。

## 本计划完成标准

本计划是设计计划，不以代码数量收口：

- 需求与负载证据足以区分 stream、record、shared 和 exclusive transfer；
- 每个机制只有一个拥有的 ideas 主题，接口、所有权和失败语义无重叠；
- 用户已确认架构选择；
- 实施任务按真实模块拆分并有完整依赖、验证和回滚面；
- COMPASS 只在实施任务形成后把相应机制转为当前自然序。
