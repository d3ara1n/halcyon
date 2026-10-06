# 用户态框架分层实现

`libkernel` 拥有 RV64 ecall、错误解码、内核对象/IPC/MM/process 原语和安全资源 owner；`libthread` 拥有 `ResultCell`、固定容量 `ThreadRegistry`、线程工厂、trampoline 与协作式 reap；`libruntime` 拥有 `_start`、gp 初始化、全局 allocator、panic/OOM 处理、入口线程接管和应用入口交接。`libprocess::context` 负责 StartupSeed 校验与 StartupContext/InitialGrants 采用；入口 descriptor 与 Control 经 StartupAdoption 交给 runtime，不进入应用 Context。

目标职责以[分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md)的已确认契约为准；执行装配契约与接续由[对象链计划](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。普通库只依赖知识 owner，不依赖 `libruntime`；用户程序显式链接 `libruntime`，由 `_start` 先完成运行时准备，再通过 `start` lang item 接收 rustc 的 `main`。

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

## libexecution 的执行与扁平装配

[`source.rs`](../../user/libraries/libexecution/src/source.rs) 拥有集合原语协议、实际集合状态与 Registration 借用；Runtime 只依赖该提供者。SourceOps 合并 Add/Remove/Receive/Wait/时钟/Close 的既有内核与 host seam，没有每域 SourceOps facade。Registration 是真实 Rc 存储借用，不可复制或自行创建。

[`Assembly`](../../user/libraries/libexecution/src/assembly.rs) 实际拥有一个扁平集合、唯一接收缓冲、停驻与关闭责任。所有 fallible 存储/预算准备在消费集合前完成，失败返还集合。bind 提供登记借用；无借用才可 close，失败保留 owner。单运行体 run 检查绑定一致且没有其他运行体，每轮一个有界 Receive 批与任务量子；多运行体由外层一次接收后分流，不把集合注册进另一个集合。

[`Runtime`](../../user/libraries/libexecution/src/runtime.rs) 拥有任务、来源、请求 Gate、预付输入 FIFO 与来源退休 timer。任务 ID 进程内单调不复用，用作 cookie 命名空间；SourceId 为 token/task。同一集合 token 不复用，feed 同时核本表 token/cookie，未知、已删、退休或迟到记录丢弃。SourceEntry.pending 只存合并电平，交付时重建当前身份/种类；重复提示不移动 FIFO 或另分配输入。CLOSED reason 规范化为 CLOSED 提示，业务重查来源状态，不保留历史 rearm 错误。

来源持久；没有 armed/generation/Rearm、运行体接收缓存/notified/wait_item 或独立 run。Add 先付 source/输入额度、pin timer 与有序表节点，再调用实际来源注册并必成 adopt。Remove 先核 SourceId 所属任务，未知/已删项不调用共享 Remove；失败保留同一条目、额度与 timer 重试。Finalizing 等自己的来源注销成功后才归还任务；同一集合其他 Runtime 不受正常任务失败或正常 close 影响。

[`WorkQueue`](../../user/libraries/libexecution/src/work_queue.rs) 使用预付任务槽和纯任务推进。Finalizing FIFO 复用原槽链指针，新完成追加尾部，每次头项同时选择一份来源注销、任务退休或等待源回执。Runtime 将它与独立来源 retry timer 交替，不再按任务/来源 key 扫描回绕；blocked 回到尾部，持续新准入不能插队。元工作计数防全部阻塞时自旋；业务期限与元重试期限分别持节点，不互相改写。成功 advance 进入 Gate 前、来源注册/拒绝/注销回调后同步任务期限，取消期限不会在后续 Gate 时制造超时。Gate 的 pending 输入从实际槽查询，无重复 has_pending 真值。

非空 Runtime Drop 或接收 owner 提前 Drop 使整个装配失效；准入/推进/shutdown 在业务回调前拒绝，异常 Drop 仅清理自身登记，最后集合/进程边界接管。业务错误通过原 Runtime Hold/退休保留任务，不使用此异常路径。Framework 不建立孤儿来源队列或第二清理 actor。

RootSupervisor、fs 双 Runtime 已共用各自 Assembly，pm 与同步 RPC 使用实际扁平来源；init/test 使用持久注册、Process DONE 与独立 Endpoint/View。Pump 的失败 carrier 同时保留 Runtime 与真实接收 owner，先退休任务，再关集合；生成前的原始集合与已装配集合用同一所有权枚举区分，不复制接收责任。

## 当前验证边界

host 设施覆盖 ResultCell 仲裁、reap 扫描、启动表校验和执行装配的来源身份/期限/公平/失败 owner。目标消费者使用当前 ABI；类型检查不代替安装/取消/退出交错与实际付款证明。当前证据和未覆盖风险只由所属计划记录。
