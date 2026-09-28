# 用户态框架分层实现

当前实现已完成本次运行时分层迁移。`libkernel` 拥有 RV64 ecall、错误解码、内核对象/IPC/MM/process 原语和安全资源 owner；`libthread` 拥有 `ResultCell`、固定容量 `ThreadRegistry`、线程工厂、trampoline 与协作式 reap；`libruntime` 拥有 `_start`、gp 初始化、全局 allocator、panic/OOM 处理、入口线程接管和应用入口交接。`libprocess::context` 持有 StartupBlock 的一次采用与初始 capability owner。

目标结构由 [`../ideas/framework.md`](../ideas/framework.md) 拥有，迁移范围、验证证据和历史取舍由 [分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md) 拥有。普通库只依赖知识 owner，不依赖 `libruntime`；用户程序显式链接 `libruntime`，由 `_start` 先完成运行时准备，再进入 runtime 生成的应用桥接。应用源码只写 `libruntime::entry!(app_main)`，不直接命名 ABI 符号。

## 当前职责分布

- `shared/erhino_shared` 拥有调用号、固定宽 ABI、对象角色、rights、请求/结果布局和共享校验。
- `user/libraries/libkernel` 拥有 `call`、IPC/MM、MemoryObject/MemoryPool、Process/Thread 原语、时间和系统控制；`ThreadControl` 是唯一关闭 owner。
- `user/libraries/libprocess::context` 解析并采用启动 grants，保留进程身份、payload、入口 descriptor 和 capability 槽；高层进程装载/监督仍归 `libprocess`。
- `user/libraries/libthread` 的 `JoinHandle::Drop` 从不等待；显式 `join` 才观察 DONE。结果发布有独立状态字，线程退出不再提供安全逃生路径。
- `user/libraries/libruntime` 的 `_start` 保持 a0/a1，建立 gp，初始化 heap，采用入口 ThreadControl 并按 reservation 建立普通 `MappedRegion` owner；`entry!` 生成的应用桥接把 `()`/`Result<(), _>` 交给 `finish`，分别收束为 ThreadExit/ProcessAbort，panic/OOM 也由 runtime 调用 ProcessAbort。
- 所有服务、驱动和验收程序均为 `no_main`，显式依赖 runtime/thread/kernel owner；不存在旧目录、旧 alias 或 facade。
- 用户 target 与共同 linker 位于 `user/platforms/`；linker `ENTRY(_start)`，两个 target 使用 `riscv64-elf-ld` 以生成受审计的 SystemV ELF ABI。

## 当前启动和资源接口

- `libruntime` 没有名为 `main` 的应用入口 API。`finish` 只负责把已经执行完的应用结果转换为 ThreadExit 或 ProcessAbort；各 binary 使用 `libruntime::entry!(app_main)` 生成桥接函数。
- `__halcyon_main` 是该宏生成的内部 ELF 链接符号：`_start` 位于 runtime crate，而真正的应用函数位于 binary crate，两个 crate 需要一个稳定的链接边界。它不是用户需要实现、调用或依赖的业务 API；源码中只应出现 `entry!`。将来若入口改为链接器保留的 section/trait 注册机制，替换的也是这个内部桥，不改变应用函数契约。
- runtime 入口先由 `libprocess::context::StartupContext::adopt` 一次领取所有初始 grants；入口 descriptor 指向的 Control 转为入口 Registry record，普通程序 grant 通过 context owner 一次转交或按 capability 权限显式派生。
- StartupBlock 映像保持进程寿命；入口栈 reservation/usable 几何被转换为普通 region owner，Registry 不区分入口线程和运行期线程的回收路径。
- `ThreadRegistry::reap` 以固定槽、轮转游标和有界预算推进，不设置专职 driver。栈解除映射遇到 `ObjectBusy` 返回等待并由下一次协作推进；`PreparedThread` 在 Start 前持有未启动资源，Start 失败返还 self 由 Drop 取消；T/F 不在非用户线程 Drop。
- Capability、Endpoint、MappedRegion 的失败/Drop 行为仍分别由 IPC/MM 实现篇拥有；跨模块组合不复制这些 owner。

## 当前验证边界

host 单测、`just check`、`just clippy`、用户 ELF audit、Base64/D64 `_start`/gp 入口反汇编门和 `just acceptance` 均已通过。acceptance 覆盖 virt stress 16/16、virt core/release、sifive_u、nofd 及 panic/alloc/fatal boot-failure；QEMU DTS 工具自身的既有 warning 不影响验收判定。
