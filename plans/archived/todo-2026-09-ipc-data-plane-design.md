# IPC 数据面演进设计

> 状态：**设计已完成（2026-09）**。开始点为 `adef9a7`；用户已确认 MemoryObject-first、显式 MemoryPool、删除 Job 预算职责，以及额度池与全局物理库存分离。实施由 [`../todo-2026-09-memory-object-data-plane.md`](../todo-2026-09-memory-object-data-plane.md) 承接；外部事实摘要见 [`../ref-2026-09-ipc-data-plane-systems.md`](../ref-2026-09-ipc-data-plane-systems.md)。

## 驱动问题

旧 Tunnel 固定单页，Runnel v1 把 4 KiB、128 B 控制块和 3968 B 容量写死。单页不可扩展本身就是结构缺陷，不再要求当前负载先证明容量不足。与此同时，描述符环只有在引用独立预注册区域时才产生零拷贝收益；若只引用 Tunnel 内联字节，只会减少容量并增加状态。

公开可转移 MemoryObject 又暴露了更早的前置：backing 可以脱离创建进程存活，因而物理内存必须有独立于进程和 Job 的费用来源。Job 若同时统计资源，会与 capability 持有物形成双重真值。

## 已确认决策

| # | 决策 | 结论 |
|---|---|---|
| D1 | 数据面主结构 | MemoryObject-first：先统一多 extent backing 与公共对象，再让 Tunnel 复用内部 core |
| D2 | 库存预算 | 显式 MemoryPool capability；root 由平台事实铸造，子池从父池转移额度 |
| D3 | Job 职责 | Job 只做创建、成员与收束域；不绑定默认资源包，不统计内存或 CPU 配额 |
| D4 | Pool 与物理库存 | Pool 持页额度，唯一 FramePool 持实际空闲 extent；backing 持 `FrameTracker[] + MemoryCharge` |
| D5 | 费用归属 | ProcessCreate 消费 Pool Handle 建立进程绑定；页表、匿名 backing、Tunnel/MemoryObject 按方向文档支付；capability 跨 Job 转移不改来源池 |
| D6 | Tunnel | 固定长度、有页数与 extent 上限的多页逻辑连续 backing；不要求物理连续，不预留固定控制页 |
| D7 | Runnel | v2 继续只做单工 SPSC 字节流；动态总长度、128 B header、`u64` 游标；不含 descriptor ring |
| D8 | BufferQueue | 与 Runnel 并列；Mailbox 注册 MemoryObject，descriptor 使用 `region + generation + offset + length`，submit/completion 双环交接 |
| D9 | 所有权强度 | 普通 BufferQueue 只承诺协议所有权；DMA/CPU 强制排他、pin、IOMMU 与撤销留给设备资源设计 |
| D10 | capability 模型 | 不重做通用 capability；增加 MemoryPool/MemoryObject 的 kind、role 与 rights 即可 |

## 方案比较

### 直接扩 Tunnel

改动最短，但会让 Tunnel 与公共 MemoryObject 各自拥有 backing 分配、extent 投影和映射准备；描述符也无法自然引用外部缓冲。删除公共 backing module 后复杂度会在两个调用者中重现，因此否决。

### MemoryObject-first

MemoryPool、ObjectBacking、AddressSpace ObjectView 各有一个 interface；Tunnel 只增加参与方、门铃与 lease，Runnel 只增加字节流，BufferQueue 只增加注册与交接。复杂实现集中在可 host 测的 pool/planner 与统一 AddressSpace seam 后，外部 interface 仍小，采用。

### 内核多区域 IOBuffer

可以由内核强化 region discipline 和部分 TOCTOU 防护，但会把队列协议、region table 和长状态推入不可打断内核路径。当前没有必须由内核解析 descriptor 的硬件或安全契约，否决；未来特殊 DMA 对象必须从设备契约独立推导，不能复活为通用 IPC 机制。

## 外部事实如何影响选项

Zircon、QNX、seL4、L4Re、Genode 与 Twizzler 均把小控制消息和共享数据区分开；稳定引用使用对象 offset 或预注册 region，而不是本地 Handle/VA。Genode packet stream 的 submit/ack、managarm 的 chunk+index queue 说明 descriptor 结构取决于完成顺序，单环不是普遍答案。Hubris lease 依赖同步 rendezvous 才能原子撤销；RedLeaf/Theseus 的语言所有权依赖单地址空间或受控语言环境，不能直接升级为不可信进程共享页的硬保证。Zircon IOBuffer 展示了内核多区域方案的收益与复杂度，但不构成 Halcyon 把协议放进内核的独立理由。

## 后续自然序

1. 实施 MemoryPool 与进程资源绑定，迁移所有用户可放大的帧分配路径；
2. 公开 MemoryObject 并把多页 ObjectView 接入统一 MemoryChange；
3. Tunnel 迁移为多页共享 core，Runnel 升级 v2；
4. 闭合 FAL DirectoryGrant、跨进程 provider、服务发现与 Open 流；
5. 在真实驱动数据面前实现 BufferQueue，并与 MMIO/IRQ/DMA 设计共同决定硬件所有权。
