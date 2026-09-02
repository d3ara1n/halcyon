# notes

设计文档按视角分层（约定见 AGENTS.md「心智模型」）：

- **[ideas/](ideas/)**：系统应该是什么。自顶向下描述概念、边界和契约，允许领先于实现；
- **[impls/](impls/)**：系统实际怎么做。引用结构、模块和路径，随代码演进同步修订；
- **images/**：图源。

同一主题的 idea 与 impl 是并列视角，不是前后阶段。判断方向读 ideas，判断现状读 impls 与代码。

## 归属纪律

每个机制在 idea 与 impl 视角各有唯一拥有篇；其他篇只描述本主题如何使用该机制并链接拥有篇，不重复定义其不变量。idea 不记录当前施工状态，impl 不保存历史选型或未来实施方案。

## 索引

| 主题 | idea | impl |
|---|---|---|
| 内核边界、协作式执行与内部机制 | [kernel](ideas/kernel.md) | [internals](impls/internals.md) |
| 系统复位 authority、语义与平台映射 | [system-reset](ideas/system-reset.md) | [internals](impls/internals.md) |
| 对象、Capability、Handle、运输与根授权 | [object](ideas/object.md) | [MemoryObject](impls/memory-object.md)、[IPC 对象](impls/ipc.md) |
| Bootstrap、init、StartupBlock 与 launcher | [bootstrap](ideas/bootstrap.md) | [startup](impls/startup.md) |
| Job、进程、线程、生命周期与调度类 | [task](ideas/task.md) | [task](impls/task.md) |
| Hart、执行需求、调度域、用户上下文与 trap | [execution-context](ideas/execution-context.md) | [execution-context](impls/execution-context.md) |
| 内存所有权、地址空间与映射 | [mm](ideas/mm.md) | [mm](impls/mm.md) |
| 系统调用与 remote call | [call](ideas/call.md) | [call](impls/call.md) |
| IPC 总览与消息控制面 | [ipc](ideas/ipc.md)、[message](ideas/message.md) | [IPC 对象](impls/ipc.md) |
| WaitMany 与 ObjectSignals | [wait](ideas/wait.md) | [IPC 对象](impls/ipc.md) |
| Notification | [signal](ideas/signal.md) | [IPC 对象](impls/ipc.md) |
| Tunnel 与共享内存 | [tunnel](ideas/tunnel.md)、[shared-memory](ideas/shared-memory.md) | [Tunnel](impls/tunnel.md) |
| Runnel | [runnel](ideas/runnel.md) | [Runnel](impls/runnel.md) |
| BufferQueue | [buffer-queue](ideas/buffer-queue.md) | — |
| 通用 RPC | [rpc](ideas/rpc.md) | [rpc](impls/rpc.md) |
| 服务进程与用户态框架 | [service](ideas/service.md)、[framework](ideas/framework.md) | [startup](impls/startup.md)、[rpc](impls/rpc.md)、[fal](impls/fal.md) |
| 文件系统 namespace 与 FAL | [fs](ideas/fs.md)、[fal](ideas/fal.md) | [fal](impls/fal.md) |
| 设备资源授权 | [device](ideas/device.md) | — |

应用层构想不属于系统契约，单独位于 [`ideas/applications/`](ideas/applications/)；当前包含机器人 [ECS](ideas/applications/ecs.md)。
