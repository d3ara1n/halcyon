# notes

设计文档按视角分层（约定见 AGENTS.md「心智模型」）：

- **[ideas/](ideas/)**：系统应该是什么。自顶向下描述概念、边界和契约，允许领先于实现；
- **[impls/](impls/)**：系统实际怎么做。引用结构、模块和路径，随代码演进同步修订；
- **images/**：图源。

同一主题的 idea 与 impl 是并列视角，不是前后阶段。判断方向读 ideas，判断现状读 impls 与代码。

## 索引

| 主题 | idea | impl |
|---|---|---|
| 对象、Capability、Handle、运输与根授权 | [object](ideas/object.md) | [IPC 对象](impls/ipc.md) |
| 进程启动与 StartupBlock | [object](ideas/object.md)、[service](ideas/service.md) | [startup](impls/startup.md) |
| 任务、Job、进程、线程与调度 | [task](ideas/task.md) | [task](impls/task.md) |
| 执行环境（boot / trap / hart capability） | [task](ideas/task.md) | [execution-context](impls/execution-context.md) |
| 内存管理 | — | [mm](impls/mm.md) |
| 内核内部机制（中断 / 锁 / 唤醒） | — | [internals](impls/internals.md) |
| 内核调用（syscall / remote call） | [call](ideas/call.md) | [call](impls/call.md) |
| IPC 总览与消息控制面 | [ipc](ideas/ipc.md)、[message](ideas/message.md) | [IPC 对象](impls/ipc.md) |
| 等待、对象状态与 Notification | [wait](ideas/wait.md)、[signal](ideas/signal.md) | [IPC 对象](impls/ipc.md) |
| Tunnel、共享内存与 Runnel | [tunnel](ideas/tunnel.md)、[shared-memory](ideas/shared-memory.md)、[runnel](ideas/runnel.md) | [IPC 对象](impls/ipc.md) |
| 通用 RPC | [rpc](ideas/rpc.md) | [FAL/RPC 现状](impls/fal.md) |
| 服务进程与用户态框架 | [service](ideas/service.md)、[framework](ideas/framework.md) | [startup](impls/startup.md)、[FAL/RPC 现状](impls/fal.md) |
| 文件系统、FAL 与私有 namespace | [fs](ideas/fs.md)、[fal](ideas/fal.md) | [FAL/RPC 现状](impls/fal.md) |
| 设备授权 | [device](ideas/device.md) | — |
| ECS 应用构想 | [ecs](ideas/ecs.md) | — |
