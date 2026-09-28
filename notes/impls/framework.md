# 用户态框架分层实现

当前实现尚未完成目标分层。`user/rinlib` 同时承载 RISC-V `ecall` 入口、系统调用包装、内核对象/Handle/IPC/内存/等待安全封装，以及 Rust `lang_start`、全局 allocator、panic/OOM 和退出处理。公共库中的多个 riscv64 代码路径因此直接依赖 `rinlib`；`libprocess` 还无条件依赖它并读取 `rinlib::env::pid()`。目标拆入 libkernel、libthread 和 libruntime，并细化 libprocess 的 context/management 边界；当前代码继续使用旧名称，直到迁移完成。

目标结构由 [`../ideas/framework.md`](../ideas/framework.md) 拥有，迁移范围、顺序和施工门由 [分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md) 拥有。目标 runtime 可以单向组合进程上下文、线程与内核资源库；当前尚无 libkernel/libthread/libruntime、context 功能拆分、显式 entry! 或新线程环境契约，不能按目标结构描述实际代码。

## 当前职责分布

- `shared/erhino_shared` 拥有调用号、固定宽 ABI、对象角色、rights、请求/结果布局和共享校验；没有 ecall 汇编。
- `rinlib::call` 拥有当前唯一用户态 ecall stub、错误解码和 syscall wrappers。
- `rinlib::ipc`、`mm`、`process`、`memory_object`、`memory_pool`、`thread`、`time`、`system` 混合提供内核资源包装和用户态高层组合。
- `rinlib::rt` 私有，仅在 RV64 启用，拥有 lang start、allocator、panic/OOM 和 main 终止；它调用同 crate 的当前资源包装。
- `rinlib::env` 解析 StartupBlock 并保存进程 PID、parent PID 和启动块视图；`libprocess` 目前读取其中的 PID 构造出生块。
- `libprocess`、`librpc`、`librunnel`、`libfal`、`libfs`、`libservice`、`libexecution` 的部分目标代码当前依赖 rinlib 的 IPC、MM、time 或 process API；`libbudget` 当前不依赖 rinlib。
- `shared/erhino_shared/src/proc.rs:276-300` 当前还包含 Rust `Termination`/`ProgramResult`；用户 `rinlib::rt` 转换 main 返回值，`os/kernel/src/rt.rs` 则只借其泛型约束，实际调用 main 后 park。该语言政策尚未迁出 shared。
- 两个用户 target JSON 和共同 `linker.ld` 仍在 `user/rinlib/`；`user/.cargo/config.toml`、target 的 `-T rinlib/linker.ld` 和 Justfile 点名这些路径。脚本中的 `ENTRY(main)` 仍与 rustc 入口包装相连，目标的平台资产与 runtime 入口拆分尚未实施。
- `rinlib::rt` 的旧注释提到未来信号监听线程，但当前实际入口只初始化环境和堆，再调用 main；现有 Notification 的 signal/take 不包含异步 handler 或自动监听器。目标选择按消费者需要组合，不把该注释当成已完成或必需的运行机制。

这些是实现事实，不是对目标结构的授权。设计与实现的差距必须随分层计划的单一迁移闭包消除，不能在此通过 facade 或 re-export 伪装成已完成。

## 当前启动和资源接口的限制

- `rinlib::env` 的 `startup_handles()` 和 `startup_payload()` 返回静态原始视图；没有 StartupSeed、一次采用表或初始 Handle 的单次领取状态。当前各调用点各自承担 raw → owner 的 unsafe 接管前提，HandleQuery 不能证明唯一关闭责任。
- 普通 `libprocess::map_birth_block` 将出生块映射为 READ|WRITE，ThreadCreate 后一次写入；内核 bootstrap 的启动映像是只读。普通程序当前没有与目标一致的硬件只读约束，不能把 init 的保证直接扩写成全体 launcher 的事实。详细构造见 [startup](startup.md)。
- Notification、Job/Process 操作当前主要为 raw Handle 函数；ThreadControl 内联在 rinlib JoinHandle。目标安全包装尚未建立，不能将接口草案写成可调用 API。
- Capability 只覆盖非本地映射表项；Endpoint 带本地映射与借用约束，MappedRegion 不是 Handle。它们的失败/Drop 行为不同，现状分别由 IPC/mm 实现篇拥有。
- Mailbox Send/Receive 当前业务非阻塞；Packet/ReceiveBuffer 的预付分配、WaitMany/WaitSet 的显式等待，以及含内核退休的 Close/Map 停驻是不同事实。迁移不把带 Deadline 的发送改为底层重试循环。

## 当前验证边界

本篇只记录现状。没有新增 `libkernel`，没有迁移 Cargo 依赖、目录、入口配置或 ABI；目标分层尚未通过构建或 acceptance。固定提交 Review 和历史实现文档中的 `rinlib` 名称保留为当时事实，当前实现说明也保留旧名以便核对源码。
