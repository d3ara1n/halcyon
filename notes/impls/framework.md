# 用户态框架分层实现

`libkernel` 拥有 RV64 ecall、错误解码、内核对象/IPC/MM/process 原语和安全资源 owner；`libthread` 拥有 `ResultCell`、固定容量 `ThreadRegistry`、线程工厂、trampoline 与协作式 reap；`libruntime` 拥有 `_start`、gp 初始化、全局 allocator、panic/OOM 处理、入口线程接管和应用入口交接。`libprocess::context` 负责 StartupSeed 校验与 StartupContext/InitialGrants 采用；入口 descriptor 与 Control 经 StartupAdoption 交给 runtime，不进入应用 Context。

目标结构由 [`../ideas/framework.md`](../ideas/framework.md) 拥有，迁移范围、验证证据和历史取舍由 [分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md) 拥有。普通库只依赖知识 owner，不依赖 `libruntime`；用户程序显式链接 `libruntime`，由 `_start` 先完成运行时准备，再通过 `start` lang item 接收 rustc 的 `main`。

## 当前职责分布

- `shared/erhino_shared` 拥有调用号、固定宽 ABI、对象角色、rights、请求/结果布局和共享校验。
- `user/libraries/libkernel` 拥有 `call`、IPC/MM、MemoryObject/MemoryPool、Process/Thread 原语、时间和系统控制；`ThreadControl` 是唯一关闭 owner。
- `user/libraries/libprocess::context` 解析并采用启动 grants；StartupContext 只保留进程身份、payload 和 affine InitialGrants，入口 descriptor 经 StartupAdoption 交由 libruntime/ThreadRegistry；高层进程装载/监督仍归 `libprocess`。
- `user/libraries/libthread` 的 `ResultReceiver::Drop` 只放弃结果引用，不等待或触碰 record；显式 `join` 才观察 DONE。结果发布有独立状态字，线程退出不再提供安全逃生路径。
- `user/libraries/libruntime` 的 `_start` 保持 a0/a1，建立 gp，初始化 heap，采用入口 ThreadControl 并按 reservation 建立普通 `MappedRegion` owner；`start` lang item 将 `()`/`Result<(), E: Debug>` 分别收束为 ThreadExit/ProcessAbort，panic/OOM 也由 runtime 调用 ProcessAbort。
- 所有服务、驱动和验收程序均为普通 `fn main`，显式依赖 runtime/thread/kernel owner；不存在旧目录、旧 alias 或 facade。
- 用户 target 与共同 linker 位于 `user/platforms/`；linker `ENTRY(_start)`，两个 target 使用 `riscv64-elf-ld` 以生成受审计的 SystemV ELF ABI。

## 当前启动和资源接口

- `libruntime` 不公开 `finish` 或应用桥接宏；`start` lang item 接收 rustc 生成的 `main` shim，并负责将结果转换为 ThreadExit 或 ProcessAbort。
- runtime 把入口 descriptor 与 Control 转为入口 Registry record，StartupContext 放入私有一次性槽；应用经 `libruntime::startup::take` 领取一次。应用随后消费 `StartupContext::into_parts()`，身份、payload 与 affine `InitialGrants` 分别移交；grant 只能按索引 `take` 或 role `try_adopt`，未领取项由 owner Drop 关闭。
- StartupBlock 映像保持进程寿命；入口栈 reservation/usable 几何被转换为普通 region owner，Registry 不区分入口线程和运行期线程的回收路径。
- `ThreadRegistry::reap` 以固定槽、轮转游标和有界预算推进，不设置专职 driver。栈解除映射遇到 `ObjectBusy` 返回等待并由下一次协作推进；`PreparedThread` 在 Start 前独占未启动资源，对 Registry 不可见，Start 失败返还 self，Drop 关闭 Control 后以已 DONE record 登记；T/F 不在非用户线程 Drop。
- Capability、Endpoint、MappedRegion 的失败/Drop 行为仍分别由 IPC/MM 实现篇拥有；跨模块组合不复制这些 owner。

## 当前验证边界

host 单测覆盖 ResultCell 仲裁、reap 候选扫描与启动表的校验辅助函数；adopt 失败返还 Seed 由源码结构论证（失败分支先于任何 owner 构造），不由测试观察。QEMU 验收覆盖真实启动、线程创建/放弃/回收与 race 矩阵。
