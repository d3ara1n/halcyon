# notes

设计文档按视角分层；编写、更新或审查时遵循 [AGENTS.md「心智模型」](../AGENTS.md#心智模型) 的文档职责：

- **[ideas/](ideas/)**：线稿。粗粒度描述系统有哪些模块、概念与类型，以及它们之间的关系；可以触及细节但不展开，后续设计与实施的填充可以覆盖线稿轮廓。只在设计阶段修改；
- **[impls/](impls/)**：回头描述代码已经实现的当前面貌，供架构审计免读代码。按组件与机制解释模块、数据与控制流、所有权、生命周期、并发、失败行为及当前限制，脱离任务背景仍可独立阅读；不承载设计裁决、目标选择、质量评价或完成宣告；
- **images/**：图源。

施工以代码核实当前事实，审计读 impls；重构目标以所属计划中已确认的详细设计为依据。ideas 只表达线稿，处于施工或重构范围时默认不可据以补齐目标设计。设计决策、状态机、完整契约和实现方案写在计划，不进 ideas。

粒度判据：正常实施可能推翻的内容不属于 ideas。实施中遇到必须推翻线稿的情况，即回到设计阶段，由计划记录设计轮次后再改 ideas；单纯实施只回填 impls。

线稿的精简、重排或职责清理不构成重设计审定。哪些篇章的哪些范围已重新设计、依据哪份详细计划以及还缺什么，由计划登记；未登记的范围不作目标依据。本轮登记入口为[系统重构总纲](../plans/todo-2026-09-29-system-redesign.md#重构设计依据与-ideas-审定登记)。设计审定与代码/impls 的实现状态分别记录。

## 归属纪律

每个机制在 idea 与 impl 视角各有唯一拥有篇；其他篇只描述本主题如何使用该机制并链接拥有篇，不重复定义其不变量。施工过程与验证结果由 `plans/` 对应计划或审查档案承载。更新 impl 时改写机制所在章节，保留一致的当前视图；源码与计划链接分别用于核实和追溯，正文自行解释关键实现。

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
| ObjectSignals、WaitOne 与持久 WaitSet | [wait](ideas/wait.md) | [IPC 对象](impls/ipc.md) |
| 公共单调时间与绝对期限 | [time](ideas/time.md) | [time](impls/time.md) |
| Notification | [signal](ideas/signal.md) | [IPC 对象](impls/ipc.md) |
| Tunnel 与共享内存 | [tunnel](ideas/tunnel.md)、[shared-memory](ideas/shared-memory.md) | [Tunnel](impls/tunnel.md) |
| Runnel | [runnel](ideas/runnel.md) | [Runnel](impls/runnel.md) |
| BufferQueue | [buffer-queue](ideas/buffer-queue.md) | — |
| 通用 RPC | [rpc](ideas/rpc.md) | [rpc](impls/rpc.md) |
| 服务进程与用户态框架 | [service](ideas/service.md)、[framework](ideas/framework.md) | [service](impls/service.md)、[startup](impls/startup.md)、[runtime](impls/runtime.md)、[rpc](impls/rpc.md) |
| 用户态库组合与可替换 Rust runtime | [framework](ideas/framework.md) | [framework](impls/framework.md) |
| 用户态内核接口、安全 owner 与失败交付 | [kernel-interface](ideas/kernel-interface.md) | [framework](impls/framework.md)、对应对象实现篇 |
| 开放发现与命名空间 | [oal](ideas/oal.md) | [fs 中的 OAL 接续](impls/fs.md) |
| 文件领域、流与 Copy | [fs](ideas/fs.md) | [fs](impls/fs.md) |
| 设备声明、发现/PnP、驱动接管、租约与资源机制 | [device](ideas/device.md) | [资源内部机制](impls/device.md)、[MMIO/VM 接线](impls/mm.md#映射来源与-mmio-退休原语) |

应用层构想不属于系统契约，单独位于 [`ideas/applications/`](ideas/applications/)；当前包含机器人 [ECS](ideas/applications/ecs.md)。
