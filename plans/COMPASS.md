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
- **纯 capability 授权**：无进程权限等级；平台根由内核按事实铸造、init 决定策略；Handle 以 TRANSIT/GRANT 区分消息暂存与直接跨表安装，badged sender 承载用户态 grant（`notes/ideas/object.md`）。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求渐进替换（如调度域/类）。

## 活跃计划

plans/ 根目录只放活跃计划（todo 与含未闭合承接项的 review 同为计划）；调查收口即移入 `archived/`，`ref-*` 是死的参考资料。当前全部活跃计划：

| 文件 | 概要 |
|---|---|
| [`todo-2026-08-system-audit.md`](todo-2026-08-system-audit.md) | 重写版系统审查 7 分片：01 SBI 与 02 trap/上下文已完成收口，03–07 待做 |
| [`todo-2026-08-26-process-lifecycle.md`](todo-2026-08-26-process-lifecycle.md) | 下一自然序主线：完整进程生命周期与终止屏障——step 1–7 已实施收口（step 7 多线程 teardown barrier 与写回 panic 面消解，review 计划见 [todo-2026-08-28-thread-teardown-review.md](todo-2026-08-28-thread-teardown-review.md)）；剩余 step 8–10：D64 eligibility、多核验证矩阵、文档终态收口 |
| [`todo-2026-08-26-review-carryover.md`](todo-2026-08-26-review-carryover.md) | 归档 review 中未闭合承接项的唯一跟踪点：设备接入重审、IPC 压力验证线、notes 结构整改、F2/F4 注记 |
| [`todo-2026-08-27-mechanism-generalization-review.md`](todo-2026-08-27-mechanism-generalization-review.md) | 机制层泛化改造（15c7811/9c03251/95deea6）的未来审查计划：Lock Ladder、per-hart 期限表、MappingLease、公理层与文档自洽的复核轴 |
| [`todo-2026-08-26-bootstrap-launcher-review.md`](todo-2026-08-26-bootstrap-launcher-review.md) | 机会型任务：BootPackage / launcher 十切片代码审查，有空就做，不阻塞主线 |
| [`todo-2026-08-28-thread-teardown-review.md`](todo-2026-08-28-thread-teardown-review.md) | 生命周期 step 7（多线程 teardown barrier，提交 d741880）的未来审查计划：成员表不变量、游标取消锁序、汇编出口归一、写回复检即杀 |

常驻手册：[`REVIEW.md`](REVIEW.md) 规定设计与代码两类 Review 的事后审查纪律（不进入任务流程、不阻碍验收）；`DEBUG-PLAYBOOK.md` 与 `TOOLING-PITFALLS.md` 分别记录调试和工具纪律。

## 位置

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、BootPackage initial ELF bootstrap、syscall 面 Debug/Exit/Extend/Sleep、进程回收、timer/IPI 通路）与执行环境重构（a9a65cb）。IPC 前地基工程已完成（hart 身份统一、锁内存序、所有权单向化、uaccess 集中化、Extend 字节 sbrk 语义；见 `plans/archived/2026-09-pre-ipc-groundwork.md`）。IPC 对象 / Handle 重建也已完成：进程本地 HandleTable、WaitContext、显式 Mailbox/Notification、原子 Handle move、Endpoint/Invitation 与 Acquire/Release Runnel 已贯通，实施档案见 [已归档计划](archived/2026-08-ipc-object-foundation.md)，实现现状见 `notes/impls/ipc.md`。
- 当前：**完整进程生命周期 step 2–6 已落地**——step 2–4（ProcessControl 前移、lifecycle 顶级锁状态机、全局进程表退役、跨 hart kill、硬上界 ProcessDrain、init 监督闭环）已过统一代码 Review（[archived/review-2026-08-27-process-lifecycle-code-review.md](archived/review-2026-08-27-process-lifecycle-code-review.md)）；step 5（Job 管理面）已实施收口：JobSeal/Query/Enumerate/Derive、有序成员表、链锁封口、完成传播与 libprocess 递归 job_kill；**step 6（持久 init 监督政策与 pm 委托域）已实施收口（2026-08-28）**：init 建 root → services → pm_domain/acceptance 拓扑，委托域 JobControl 经 StartupBlock grants 授 pm（MANAGE|READ|WAIT，无 CREATE）而 init 保留复制件作直接收束权；pm 对域内 Running 靶走 枚举→派生（铸造）→kill→drain→seal；失败路径整树 job_kill(services)；init 全部收束后常驻管理端点、不自终止，终态交 quiescent 静默停机（公理入档 ideas/bootstrap.md）；拓扑快照两处打印供调试。FAL 剩余面（DirectoryGrant、每订阅者 watch、跨进程 provider）仍排在其后。
- **机制层泛化改造已落地（2026-08-27）**：以 [review-2026-08-27-mechanism-generalization.md](review-2026-08-27-mechanism-generalization.md) 为纲的四批改造——① impls 失同步修复与 KernelRequest 正名（每机制恰一篇拥有的归属纪律入 README）；② 期限表 per-hart 化（唤醒所有权结构化，热路径少一把全局锁）；③ RAII 收束契约（tunnel `MappingLease`、`phys_to_virt` 栈区 debug 断言、ideas/object.md「收束分层」公理替代 close fanout 枚举证明）；④ **Lock Ladder**：`sync::ranks` 秩表 + per-hart 秩栈断言（同秩链段 key 递增：链锁 jid、表嵌套 pid；talc 经 `RankedRawSpinlock` 类型级注入；bootstrap 专用帧经 formal entry 切换），锁序契约按实测重写并修正旧基线三错（lifecycle 方向、drain_gate/HEAP/POOL 未入档、AddressSpace 双层）。全部负载 debug 构建验证无违规；reserve/commit/rollback 协议四要素成文。
- FAL/RPC 首批已落地——方向 C 拍板（无中央 VFS、symlink 无 hardlink、Lookup 三值应答）；librpc（RpcPrefix/同步 Caller）、libfal（线协议/memfs/provider）、libfs（前缀表/走路引擎）与 fs 真路径验收线达成（`54d3e02`/`bf32c1c`，实现现状见 `notes/impls/fal.md`）。
- IPC ABI 基座已重构：Entry 保存 immutable badge，MessageHeader 区分 sender_pid/sender_badge，Mailbox owner 可 mint sender；TRANSIT/GRANT 分离 buffered message 与 direct ProcessStart；send-once target/transit alias 已拒绝。完整审查见 [`archived/review-2026-08-26-notes-design.md`](archived/review-2026-08-26-notes-design.md)，实现现状见 `notes/impls/ipc.md`。
- StartupBlock v2 与 BootPackage 启动链已落地：outer 为 Header + 实际 child Handle 数组 + 可零 padding + opaque payload；内核只解析 fixed envelope 与唯一 init ELF，payload 以只读页交给 init（映入即收编 owned backing）。实现现状见 `notes/impls/startup.md`。
- 对照负载：`user/systems/` 与 `user/drivers/` 四服务。fs 经用户态 FAL 真路径完成创建/枚举/属性/符号链接/偏移读写，验收线已过（`bf32c1c`）；旧 fs ABI 尸体已清，KNOWN_ISSUES 桩条目已消解。
- 用户态 launcher 基座已落地：root Job/JobControl、affine ProcessBuilder、Building-only Map/Write、事务化 ProcessStart、ProcessControl 与公共 `libprocess` 已贯通；init 以临时 ustar 政策启动其余负载，内核不含 tar/service policy。进程生命周期改造已实施（见上）。剩余方向：内核提供 JobSeal/分页枚举，递归 JobKill 由 init/pm 等用户态管理者组合（step 5），持久 init 是当前系统配置的 root supervisor；D64 仍等待调度域 eligibility（step 8）。initfs manifest/archive 另案设计。方向见 `notes/ideas/{task,bootstrap}.md`，实现现状见 `notes/impls/{startup,task}.md`。
- 下一自然序：完整进程生命周期与用户态多线程屏障 → FAL 剩余面（DirectoryGrant、跨进程 provider、服务发现）→ 设备/中断接入 → 异构（分片计划见活跃计划表）。**step 7（ThreadSpawn 前多线程 teardown barrier）已实施收口（2026-08-28）**：线程成员表（tid 寻址、离场即摘）取代单值记录，等待取消锁外游标化（零分配），归一收敛到 trap 汇编非 Resume 出口，KNOWN_ISSUES 写回 panic 面消解（deliver_output 复检即杀 + 分发出口终止检查）；ThreadSpawn 接入面清单入档计划篇。下一项是 step 8：capability-derived 调度域 eligibility，再开放 D64（届时 F2 ready-marker 预留域路由承接项触发）。BootPackage / launcher 基座已过机制层审查（[`archived/review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)，F1 payload 收编 owned backing、F3 Pid 拓宽 u64 已实施）；initfs 内部协议在需要正式服务编排时单独设计。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 文档即决策：方向性结论进 notes/，本文件只导航；每轮收口于「决策入档、代码全绿已提交、下一步自然序明确」。
