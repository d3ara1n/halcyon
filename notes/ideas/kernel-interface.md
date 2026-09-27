# 用户态内核接口与资源所有权

libkernel 拥有用户态对 eRhino 内核的调用、安全资源表示和单次操作的责任转移。它不拥有内核内部事务机器，也不承担程序语言环境、任务执行循环或服务政策。与 runtime、进程和线程库的依赖由 [framework](framework.md) 规定；内核对象、授权、关闭与退休的基础语义由 [object](object.md) 拥有。

本篇规定目标接口契约，不表示 libkernel 已实现。当前分层差距与具体 API/迁移表由 [分层计划](../../plans/todo-2026-09-runtime-kernel-library-separation.md) 记录。

## 接口层次

| 层次 | 责任 | 禁止泄漏到上层的负担 |
|---|---|---|
| 私有调用后端 | 寄存器约定、ecall、错误解码、输入输出交付 | 调用者不复制 asm、错误编号或结果初始化规则 |
| 原始边界 | 极少数需要接管原始地址、执行入口或外部 Handle 的操作 | unsafe 前提必须写明，不以一个裸数值伪造 owner |
| 安全资源接口 | 构造、借用、消耗、观察、转换与失败后返还 | 不要求上层猜测调用后哪些能力已失效 |
| 领域组合 | libprocess、libthread、RPC 等持有多步工作和政策 | 不回调到 libkernel 要求其启动任务、重试循环或监督其它对象 |

原始入口默认保持私有；只有 runtime 平台入口、线程执行地址或其它确有独立需要的消费者，才使用窄化的公开 unsafe 接管接口。不导出一个能绕过所有 owner 的安全 raw syscall 总入口。共享 ABI 继续拥有固定宽数值，libkernel 不造第二份调用号、rights 或错误码。

## 值、借用与 owner

- **数值和描述**：Handle、对象身份、Pid/Tid、role、rights 与描述快照可以复制，但复制它们不产生新的关闭、运输或资源生命周期责任。
- **借用**：操作从存活 owner 借用 capability 或观察视图。借用不能变成消费、关闭或新 owner；对原始数值的资源接管必须跨 unsafe 边界。
- **非映射能力 owner**：Capability 线性持有一个进程表项的关闭责任。它不实现隐式 Clone；显式 duplicate 由内核核验 DUPLICATE 和 rights 衰减，并返回一个新的表项 owner。相同 object_id 的两个合法表项仍是不同 owner。
- **role 包装**：Mailbox、sender、通知角色、Pool、MemoryObject、进程/线程控制等包装消费同一 owner，不新建或复制权限。带 role 的类型减少误用，不取代每次内核 authority 检查。
- **本地映射 owner**：Endpoint 或 MappedRegion 的责任还包含地址空间、借用和同步边界；不能擦除为任意 Capability，再绕开 mapping 清理。共享区继续使用 [shared-memory](shared-memory.md) 的外部可变访问模型，不因有 owner 就产生普通可变切片。

从创建/duplicate/接收等受控路径取得的新表项，可直接建立 owner；从启动入口取得的一组初始表项按 [bootstrap](bootstrap.md) 的一次采用契约建立。HandleQuery 只能核实 role/rights，不能证明关闭权未被其它 Rust 对象接管。必须先有独占责任的来源证明，再做安全的 role 转换。

role 转换在入口验证一次。输入 role 不符、未满足该类型明确要求的最低 rights 或查询失败时，完整返还原 owner；不关闭、不吞掉它。不能为构造一个通用控制 owner 就默认要求该 role 的全部权限，未获授权的具体操作仍由内核拒绝。已核实的不可变 entry 描述可随同 owner 传递，避免重复查询；对象信号、队列状态、可用性等动态事实不可据此缓存成真值。库不能把 rights 自动补齐，也不要求目标侧 owner 必须保留来源侧曾用的 GRANT/TRANSIT 权限。

## 消耗与失败形状

接口按实际提交契约选择返回形状，不用一个通用 Result 错误码抹平责任差异：

| 操作形状 | 成功 | 失败 |
|---|---|---|
| 借用查询/非消费操作 | 返回值或更新结果 | 原 owner 仍在调用方；值是否更新按对应原子契约 |
| 新建资源 | 返回完整新 owner 组 | 未产生可达新 owner；若存在已发布资源则进入下一种形状 |
| 消费输入后返回资源 | 输入已转移/关闭，交付新 owner | Commit 前返还全部输入；不得将调用方的输入默默 Drop |
| 可能部分发布或提交后发现交付问题 | 明确交付运行结果或清理 owner | 区分未提交与已提交；已提交的能力不伪装成可重试输入 |
| 有预算的推进 | 返回 progress 和仍需推进的责任 | 已完成步骤不回滚，未完成 owner 与进度一起保留 |
| 成功不返回 | 进程/线程或系统终局已经提交 | 仅在真实拒绝时返回错误；不得继续按成功结果运行 |

Packet/SendOnce 的发送失败返还各自 owner；绑定 Pool、向 Building 进程 Grant、在 Building 进程 ThreadCreate、消费 ProcessBuilder 的 Start、消费 Invitation 的 Attach 分别遵守其提交点。Grant 与 Building ThreadCreate 输出的是目标进程的表项数值，不是调用者可关闭的本地 Capability。接口不得把目标数值误包装为本进程 owner。

Close 与业务停止不同。非映射表项的最后一次关闭可按内核预付退休契约完成；关闭 JobControl 不杀死成员，关闭 ProcessControl 不等于 ProcessDrain，关闭 Pool 不撤销已经分配的资源。用户态组合拥有的 drain/停止责任要先显式接管或交付，不能让 generic Capability 的 Drop 冒充已完成。

需要处理失败的 close/unmap/结束接口消费 owner，并在拒绝时返回它。合法非映射表项的必成 Close 可供 Drop 使用；违反 unsafe 独占前提或内核不变量不伪装成可恢复业务错误。涉及用户映射与业务协商的 Drop 行为按所属契约处理，不能统一吞错、无限重试或启动隐藏 reaper；当前映射退休缺口仍由其唯一计划承接。

## 分配、停驻与取消

用户堆分配、内核资源分配和等待是三个维度。无用户堆分配不等于 syscall 不会失败，也不等于不会把调用线程转入 Waiting。

| 路径 | 用户堆与完成形态 |
|---|---|
| 基本 query、duplicate、Notification signal/take、Debug、时间值、错误解码 | 不需要用户堆；按各自单次调用完成规则返回 |
| 匿名映射/解除/权限变化、含退休的 HandleClose | 安全基础路径不分配用户堆，但可能停驻，遵守内核事务和结果记录寿命 |
| Send/Receive、预备好的包/接收缓冲 | 业务非阻塞；满/空/准入失败直接返回，deadline 只约束提交，不隐式等待重试 |
| Packet、ReceiveBuffer、批量观察结果等便利容器的准备 | 显式可失败分配；接收前准备容量，不能在能力已经安装后才为 owner 分配空间 |
| WaitMany、WaitSet wait、SleepUntil | 显式等待，期限不在循环里重置；结果状态不等于资源已消费 |
| Drain、Attach 和其它可能停驻的对象操作 | 必须按各自契约保留输入、结果槽与清理责任，不按函数名字推断同步性 |

allocator 初始化/扩容必须只使用不分配用户堆的那组接口。同步 ecall 的输出缓冲跨内核 Waiting 仍由调用帧保活；用户线程的终止不能越过内核挂接的 committed 结果义务。普通异步任务不能在一个尚未返回的 syscall 期间释放该帧，取消等待也不回滚已提交资源动作；完成形态详见 [call](call.md)、[mm](mm.md)。

libkernel 不自动轮询 Mailbox、不因 Retry/Busy 创建 executor task，也不把每个内核操作包装成自有 async runtime。librpc/libexecution/libprocess 按领域组合等待、期限与重试。由内核对象维持的退休责任则继续由内核按预算推进，不能转嫁为调用者必须不断调用。

## 模块出口与独立事实

| 领域出口 | libkernel 提供 | 上层仍拥有 |
|---|---|---|
| object / capability | entry 描述、borrow、duplicate、Close、合法接管与转换 | 业务身份解释和管理政策 |
| memory / mapping | Pool、MemoryObject、region、Map/Unmap/Protect、共享访问 view | allocator arena 策略、用户栈、文件缓存 |
| process / thread 原语 | 控制能力、Create/Bind/BindExecution/Grant/Start、Query/Drain 单步、ThreadCreate/ThreadStart/ThreadExit、ProcessTerminate/ProcessAbort | ELF、ProcessBuilder 组合、监督、线程环境、Registry 与结果 |
| ipc / notification / wait | Mailbox、sender、Delivery、Packet、通知角色、Tunnel、WaitSet 与等待 | RPC、Runnel/BufferQueue、事件任务、服务注册 |
| startup view | 平台交付的稳定字节 view 和仅读验证边界 | libprocess 的身份/payload 解释、初始能力采用与分配 |
| time / system / debug | 单调时间、期限、self 终止、受权系统复位和零堆诊断 | 语言终止政策、关机编排、日志服务 |

ProcessControl/ThreadControl 的安全包装只拥有内核表项，不附带用户栈、closure 或进程监督责任。libthread 以共享引用持有同一 ThreadControl owner（公开 Thread 与 Registry record 共享，最后引用 Close），libprocess 的监督组合持有 ProcessControl；不同层不能对同一表项建立两份关闭 owner。

接口实现时为每个出口记录可分配、可停驻和消费形态，不靠调用者记忆隐含前提。通用接口不预占尚未设计的设备 syscall；设备层在后续整体设计完成后加入同一资源责任规则。
