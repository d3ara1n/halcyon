# notes

设计文档，按视角分层（约定见 AGENTS.md「心智模型」）：

- **[ideas/](ideas/)** —— 方向性设计：系统**应该是什么**。自顶向下的概念、边界与构想，动笔时机应领先于代码，允许与当前实现不一致；未来文档网站只收这一层。
- **[impls/](impls/)** —— 实现记录：实际是**怎么做的**。自底向上，引用具体模块、结构与路径，随代码演进同步修订，过时即改或删。
- **images/** —— 图源。

同一主题允许两篇并存（如 `ideas/task.md` 与 `impls/task.md`），是同一系统的两个视角而非前后阶段：判断方向意图读 ideas/，判断实现现状读 impls/ 与代码本身。

## 索引

| 主题 | idea | impl |
|------|------|------|
| 内核调用（syscall / remote call） | [call](ideas/call.md) | [call](impls/call.md) |
| 任务模型（进程 / 线程 / 调度） | [task](ideas/task.md) | [task](impls/task.md) |
| 启动资源交付（StartupBlock / launch 事务） | [object](ideas/object.md)、[service](ideas/service.md) | [startup](impls/startup.md) |
| 内存管理 | — | [mm](impls/mm.md) |
| 内核内部机制（中断 / 锁 / 唤醒） | — | [internals](impls/internals.md) |
| 执行环境（boot / trap / 上下文） | — | [execution-context](impls/execution-context.md) |
| 对象、Handle、启动授权与服务寻址 | [object](ideas/object.md) | [IPC 对象实现](impls/ipc.md) |
| 通用 RPC（mailbox 上的调用形态） | [rpc](ideas/rpc.md) | [FAL 实现](impls/fal.md) |
| 文件系统（FAL / 命名空间 / 走路） | [fal](ideas/fal.md) | [fal](impls/fal.md) |
| 等待、对象状态与 Notification | [wait](ideas/wait.md)、[signal](ideas/signal.md) | [IPC 对象实现](impls/ipc.md) |
| IPC 总览与消息控制面 | [ipc](ideas/ipc.md)、[message](ideas/message.md) | [IPC 对象实现](impls/ipc.md) |
| 通用 RPC（信封、关联与并发） | [rpc](ideas/rpc.md) | — |
| 共享内存数据面 | [shared-memory](ideas/shared-memory.md)、[tunnel](ideas/tunnel.md)、[runnel](ideas/runnel.md) | [IPC 对象实现](impls/ipc.md) |
| 用户态文件系统（VFS / FAL / 服务框架） | [fs](ideas/fs.md)、[fal](ideas/fal.md)、[framework](ideas/framework.md) | — |
| 设备租借 | [device](ideas/device.md) | — |
| 服务进程 | [service](ideas/service.md) | — |
| ECS | [ecs](ideas/ecs.md) | — |
