# BufferQueue

BufferQueue 是运行在 [Tunnel](tunnel.md) 上、与 [Runnel](runnel.md) 并列的单向记录与缓冲交接协议。它面向块 I/O、网络包和其它已经具有记录边界的数据，不替代字节流。控制面通过 Mailbox 运输 MemoryObject capability；共享区只保存稳定的 region 引用和固定宽 descriptor，绝不保存进程本地 Handle、虚拟地址或裸指针。

## Region 注册

每端维护只存在于本进程的 region table。注册由控制面完成：一方通过 Mailbox TRANSIT 一个具适当 MAP/READ/WRITE rights 的 MemoryObject Handle，并携带候选 region id、generation、对象长度和协议权限；接收方验证、映射并显式应答后，该 `(region, generation)` 才能出现在 descriptor 中。Handle 数值和本地映射地址不进入共享区。

region id 允许复用，但 generation 必须变化。注销先停止新提交，等待所有引用该 generation 的 descriptor 完成或被协议终止，再由双方确认后解除映射和关闭 Handle；任何陈旧 generation、越界 offset/length 或未完成注册都使队列 Broken。关闭 Tunnel 终止整条 queue，不把页内注销标志当作 backing 回收 authority。

## Submit 与 completion

一条 BufferQueue 包含两个有界 SPSC 定长元素环：submit ring 由提交方写、处理方读；completion ring 由处理方写、提交方读。共享 header 固定声明版本、总长度、两个环的 offset/element size/capacity 与 feature 位；每个游标有唯一写者，发布、取得、shadow 校验、门铃与 Broken 遵守共享内存公共契约。

首版 descriptor 至少表达：

```text
region:u32 + generation:u32 + offset:u64 + length:u64 + cookie:u64 + flags:u32 + reserved
```

`offset + length` 必须无溢出且位于本地已注册对象范围；`cookie` 只由上层关联请求，不构成 authority。一个 descriptor 引用单个连续对象子范围。scatter/gather 由有硬段数上限的一组 descriptor 表达，组的开始、结束与失败原子性由具体协议版本明确；首版实现可以只开放单段而不改变 region seam。

submit 成功发布前，buffer 属提交方；处理方 acquire 接受合法 descriptor 后取得协议所有权，直到对应 completion release 发布；提交方 acquire completion 后才重新使用或注销该范围。completion 必须回显足以拒绝陈旧或伪造完成的 region generation、cookie 与结果长度。submit ring 满和 completion ring 满都是普通背压，不允许覆盖未消费项。

处理方只有在已经为该 submit 保留一个 completion 槽后，才算 acquire 接受 descriptor 并取得 buffer 的协议所有权；若没有 completion 容量，必须停止消费 submit 并施加背压。completion reservation 与已接受 descriptor 一一对应，不能因处理失败、取消或 peer 行为而丢弃。初始协议令 completion capacity 不小于最大在途 submit 数，避免以隐藏队列补偿环容量。

## 所有权边界

BufferQueue 的 ownership 是双方协议承诺，不是内核撤销：只要两端仍持 RW mapping，恶意或损坏进程仍能越权访问 buffer。安全封装以 affine 用户态类型阻止本端正常代码在在途期间访问，并把对端违约转为 Broken，但不能宣称获得硬件强制独占。

需要 CPU mapping 与设备 DMA 之间的强制排他、IOMMU 更新、cache maintenance、pin、撤销或驱动崩溃回收时，必须使用设备资源设计提供的 DMA 对象和 lease。普通 MemoryObject、Tunnel 或 Rust move 不能替代这些硬件契约，也不为“帧移交”增加第二套内核 syscall。

## 门铃与失败

双方只在从无进展变为可能进展的关键转换上通知：submit 从空变非空、completion 从空变非空，以及对端可能因腾出环槽继续。门铃可合并且不携带 descriptor 数量；等待者确认 DATA 后重查两个环，再进入 WaitMany。未知必需 feature、共享几何改变、游标非法前进、descriptor 越界、completion 不匹配或 peer close 都使本端不可逆 Broken。

## 边界

内核不创建 BufferQueue 对象，不解析 descriptor，不登记 region table，也不保证协议 ownership。Tunnel 只提供共享控制区、门铃与生命周期，MemoryObject 只提供 backing identity、映射和 seal，Mailbox 只运输注册 capability。三者组合后，BufferQueue 在用户态隐藏环布局、注册状态、背压和交接规则；删除该模块会让这些复杂度散回每个驱动和服务调用者，因此它是独立协议模块，不是 Runnel 的配置项。
