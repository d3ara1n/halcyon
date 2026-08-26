# 罗盘

> 跨会话导航：方向、位置、戒律。只存上下文不排任务——走法由目标与架构自然序决定，每次收口时维护。

## 方向

构建 notes/ 所描述的系统：RISC-V 微内核 eRhino（内核 erhino_kernel，用户态 rinlib）。方向性结论（细节见 notes/ 对应篇）：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务，内核路径恒短，内核态不可打断是推论不是选项（impls/internals.md「内核中断模型」）。
- **调度 = 域—类—执行点三层**：异构 hart 即多域；策略在类内整体可替换；扩展是横向加项不是改结构（ideas/task.md、impls/task.md「调度」）。
- **单一归属不变量**：线程任意时刻恰处于一个容器——类队列 / hart current / 无容器（impls/task.md）。
- **唤醒所有权**：timer = 自己的确定期限，IPI = 他方请求，无主唤醒不存在（impls/internals.md「唤醒所有权」）。
- **异步 syscall = 内核请求 + wake**：内核永不等待，阻塞表达为 Waiting，完成即唤醒（ideas/call.md）。
- **ABI 演进两侧同步**：shared/ 不冻结，内核与 rinlib 一起改。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求渐进替换（如调度域/类）。

## 位置

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、initfs/ELF 装载、syscall 面 Debug/Exit/Extend/Sleep、进程回收、timer/IPI 通路）与执行环境重构（a9a65cb）。IPC 前地基工程已完成（hart 身份统一、锁内存序、所有权单向化、uaccess 集中化、Extend 字节 sbrk 语义；见 `plans/archived/2026-09-pre-ipc-groundwork.md`）。IPC 对象 / Handle 重建也已完成：进程本地 HandleTable、WaitContext、显式 Mailbox/Notification、原子 Handle move、Endpoint/Invitation 与 Acquire/Release Runnel 已贯通，实施档案见 [已归档计划](archived/2026-08-ipc-object-foundation.md)，实现现状见 `notes/impls/ipc.md`。
- 当前：FAL/RPC 首批已落地——方向 C 拍板（无中央 VFS、symlink 无 hardlink、Lookup 三值应答）；librpc（RpcPrefix/同步 Caller）、libfal（线协议/memfs/provider）、libfs（前缀表/走路引擎）与 fs 真路径验收线达成（`54d3e02`/`bf32c1c`，实现现状见 `notes/impls/fal.md`）；已过统一 review（结论 [review-2026-08-25-fal-rpc.md](reviews/review-2026-08-25-fal-rpc.md)，修复批已落地；锚授权与 Delegate 传输映射随跨进程批次）。IPC 不保留 PID Send、全局 tunnel id、阻塞 Receive、ObjectKind/id Wait 或旧 signal 兼容层；旧实现风险档案见 [IPC 三面 review](review-2026-08-ipc.md)，现行对象层已过统一 review（结论 [review-2026-08-ipc-object.md](review-2026-08-ipc-object.md)，无当前可达缺陷，多线程写回 panic 面入 KNOWN_ISSUES）。
- 启动资源交付已定型：StartupBlock（只读映射快照 + 槽位化 Handle 安装）替换 `StartupMailbox` 过渡，实施档案见 [已归档计划](archived/todo-2026-08-process-startup-resources.md)，实现现状见 `notes/impls/startup.md`；归档 handover（内核只 spawn init + `TAG_INITFS_ARCHIVE`）留服务化阶段。
- 对照负载：`user/systems/` 与 `user/drivers/` 四服务。fs 经用户态 FAL 真路径完成创建/枚举/属性/符号链接/偏移读写，验收线已过（`bf32c1c`）；旧 fs ABI 尸体已清，KNOWN_ISSUES 桩条目已消解。
- 自然序往后：FAL 剩余面（流数据面 Open+Runnel、跨进程客户端、服务发现与 Handle[T] 铸造）→ 服务化（pm 接管 spawn；用户态多线程、运行时监听与退出语义）→ 设备/中断接入 → 异构（效能核多域、实时核 AMP）。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 文档即决策：方向性结论进 notes/，本文件只导航；每轮收口于「决策入档、代码全绿已提交、下一步自然序明确」。
