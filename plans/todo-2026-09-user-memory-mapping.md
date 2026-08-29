# 用户内存映射机制完整化

> 【后续待实施计划】显式系统复位收口后、ThreadSpawn 批二前的必经前置。
> 本篇只登记未来工作，不代表方向设计已经完成；启动本任务时先写
> `notes/ideas/mm.md` 的完整设计并经确认，再实施，最后才同步 impls。

## 驱动问题

当前 Running 进程只有字节粒度 sbrk 语义的 `Extend`；`ProcessMap` 只服务
Building 组装。地址空间以平坦 `frames` 保存全部 owned backing，以
`external_mappings` 保存 Tunnel 借入 VA，尚无运行期匿名 Map/Unmap、独立
映射区域所有权、guard hole 管理或通用跨 hart TLB shootdown。

这使 ThreadSpawn 虽可接受用户提供的 sp，却无法完整兑现「栈大小、guard、
放置和回收是用户态政策」：从 Extend 堆切一块只能复用 allocator 内存，不能
建立 guard，也不能独立解除映射。不得把该降级固化为线程 ABI 的默认基础。

## 设计阶段（启动本任务时）

先从需求独立推导并确认 `notes/ideas/mm.md`，至少覆盖：

1. **映射与 backing 分层**：虚拟区间、PTE 视图、owned backing、对象借入视图
   各自的唯一所有者；AddressSpace 账本不再以析构偶然顺序表达关系。
2. **运行期接口**：匿名 Map/Unmap 的地址选择、对齐、权限、长度上界、冲突、
   部分解除、失败原子性与返回值；与 Building-only ProcessMap/Write、Extend
   的关系必须统一，不保留三套互相重叠的浅机制。
3. **guard 语义**：guard 是未映射 reservation 而不是填充页；线程库只消费
   通用内存接口，不引入线程专用内核栈分配。
4. **并发与 TLB**：同一地址空间多 hart 活跃时，PTE 发布、权限收窄和解除的
   remote shootdown 请求、确认屏障、失败/终止竞争；不得依赖 ASID=0 的偶然
   全量刷新掩盖正确性。
5. **回收上界**：单次 map/unmap 工作量有硬上限；释放帧走可恢复游标或结构性
   有界数据结构，不重新引入 FrameTracker::Drop 无界扫描到用户触发路径。
6. **W^X 与代码发布**：匿名数据、栈、可执行映射和权限转换的允许集合；动态
   代码发布所需的线程静止与 `fence.i` 代次不由普通 Map 绕过。
7. **未来对象映射**：只规定 MemoryObject 将来接入时必须满足的 seam 和所有权
   约束，不在本任务里加入未设计对象、占位 ABI 或虚假的 backing 变体。

架构级选项确认前不写代码。设计完成后另列精确 ABI 和状态机实施切片。

## 实施范围（设计确认后细化）

- shared/rinlib 的 Running 期用户内存 ABI；
- AddressSpace 映射账本与 reserve/commit/rollback；
- anonymous map、unmap、guard reservation 与权限检查；
- remote TLB shootdown 的有界请求/确认机制；
- 进程终止、并发 unmap、映射失败和帧归还的统一收束；
- host 纯逻辑测试与 RISC-V 多 hart 验证负载；
- `notes/impls/{mm,internals,task}.md` 在实现后同步实际结构。

## 明确不做

- 不在本任务里实现 MemoryObject、帧 capability、COW、文件缓存或描述符环；
- 不为 ThreadSpawn 增设内核分配用户栈的专用 syscall；
- 不用固定预映射栈池替代通用运行期映射机制；
- 不因当前 ASID 恒零而跳过远端失效协议的结构设计。

## 对 ThreadSpawn 的解除条件

只有以下条件全部成立，`todo-2026-09-thread-model.md` 批二才能重开：

- 用户态可建立带未映射 guard 的次线程栈；
- join 后可在内核确认线程离场之后安全解除/复用该映射；
- 同地址空间多 hart 下解除映射不会留下 stale TLB；
- 栈、wrapper 上下文、结果槽和 join handle 的所有权可由安全 rinlib 接口完整
  表达，不要求泄漏、隐式 detach 或内核代管用户 allocator。

## 完成标准

- ideas 设计经确认后实现，代码与 impls 同步收口；
- Map/Unmap 正常、冲突、OOM、部分解除、并发和终止路径均保持账本/帧守恒；
- guard fault 走用户 fault 终止，不升级为内核 panic；
- remote shootdown 有真实双 hart 验证，不以单核样例代替；
- debug/release、virt/hetero/nofd/sifive_u 与 host 验证全绿；
- ThreadSpawn 计划解除阻塞并重新审定用户态资源契约。
