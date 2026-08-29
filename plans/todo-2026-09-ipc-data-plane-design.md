# IPC 数据面演进设计

> 【未来设计计划】只登记问题与设计流程，不预写方向结论，不代表
> MemoryObject、帧移交、描述符环或多页 Tunnel 已进入 ideas/impls。触发顺序：
> 用户内存映射机制完整化 → ThreadSpawn 与 IPC 压力线收口 → 以真实负载启动
> 本设计。

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

重新证明而非预设以下模块的接口：

- Mailbox 的控制消息与 capability move；
- Tunnel 的映射关系、参与方、门铃和生命周期；
- Runnel 的 FIFO 字节流协议；
- 可能的 MemoryObject 对 backing、视图和派生区间的持有；
- 可能的描述符协议如何引用预注册区域，而不是把进程本地 Handle 数值写进
  共享内存。

删除测试：若拿掉某一模块，其复杂度是否会散回多个调用者；若不会，该模块
只是传递层，不应存在。

### 3. MemoryObject 与帧移交

设计必须区分：

- 对象持帧、多个 mapping lease 借视图；
- capability move 只移动 authority，不自动证明旧映射已经消失；
- shareable/immutable backing 与 affine exclusive ownership；
- 独占移交需要的 duplicate 禁止、lease 归零、撤销和回收线性化；
- 可 TRANSIT 对象的 close 固定上界与多页 backing 有界 drain 如何相容；
- W^X、代码发布、DMA/设备所有权和进程退出时的回收顺序。

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
4. 将用户确认的方向写入 `notes/ideas/{mm,object,tunnel,runnel}.md` 或新增主题篇；
5. 按确认后的模块归属拆出独立实施计划；
6. 实现后才写 `notes/impls/`，不以计划或构想冒充现状。

## 前置依赖

- `todo-2026-09-user-memory-mapping.md` 已收口，映射/lease/TLB seam 稳定；
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
