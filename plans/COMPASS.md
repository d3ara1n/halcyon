# 罗盘

> 跨会话导航：方向、位置、戒律。只存上下文不排任务——走法由目标与架构自然序决定，每次收口时维护。

## 方向

构建 notes/ 所描述的系统：RISC-V 微内核 eRhino（内核 erhino_kernel，用户态 rinlib）。方向性结论（细节见 notes/ 对应篇）：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务，内核路径恒短，内核态不可打断是推论不是选项（ideas/kernel.md「协作式内核」）。
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
| [`todo-2026-09-explicit-system-reset.md`](todo-2026-09-explicit-system-reset.md) | 当前主线：先完成 power 方向设计，再以 capability 授权的用户态显式 reset + 独立 power 服务替代 quiescent 自动关机，并迁移验收终态 |
| [`todo-2026-09-user-memory-mapping.md`](todo-2026-09-user-memory-mapping.md) | 显式复位后的下一前置：完整设计并实现 Running Map/Unmap、映射账本、guard、remote TLB shootdown 与有界回收；完成后解除 ThreadSpawn 阻塞 |
| [`todo-2026-09-ipc-data-plane-design.md`](todo-2026-09-ipc-data-plane-design.md) | 未来设计项：内存映射与多线程压力线收口后，再以真实负载审视 MemoryObject、帧移交、Tunnel/Runnel 与描述符协议，不预写实现结论 |
| [`todo-2026-08-26-review-carryover.md`](todo-2026-08-26-review-carryover.md) | 归档 review 中未闭合承接项的唯一跟踪点：设备/中断接入重审与 ThreadSpawn 后 IPC 压力验证线 |
| [`todo-2026-08-27-mechanism-generalization-review.md`](todo-2026-08-27-mechanism-generalization-review.md) | 机制层泛化改造（15c7811/9c03251/95deea6）的未来审查计划：Lock Ladder、per-hart Timeout queue、MappingLease、公理层与文档自洽的复核轴 |
| [`todo-2026-08-26-bootstrap-launcher-review.md`](todo-2026-08-26-bootstrap-launcher-review.md) | 机会型任务：BootPackage / launcher 十切片代码审查，有空就做，不阻塞主线 |
| [`todo-2026-08-28-thread-teardown-review.md`](todo-2026-08-28-thread-teardown-review.md) | 生命周期 step 7（多线程 teardown barrier，提交 d741880）的未来审查计划：成员表不变量、游标取消锁序、汇编出口归一、写回复检即杀 |
| [`todo-2026-08-28-domain-eligibility-review.md`](todo-2026-08-28-domain-eligibility-review.md) | 生命周期 step 8（调度域 eligibility，提交 1d7dc92）的未来审查计划：域推导、绑定冻结、路由与多域停机 |
| [`todo-2026-08-28-persistent-init-review.md`](todo-2026-08-28-persistent-init-review.md) | 生命周期 step 6（持久 init/pm 委托域）的未来审查计划：authority 边界、收束兜底与 quiescent 相容性 |
| [`todo-2026-09-thread-model.md`](todo-2026-09-thread-model.md) | 线程资源模型三批计划：批一及事务复审已收口；批二受用户内存映射前置阻塞，前置完成后重审栈/guard/join 资源契约再实施 |
| [`todo-2026-08-29-early-quiescent-shutdown.md`](todo-2026-08-29-early-quiescent-shutdown.md) | 负载存活时误判 quiescent 停机；不再局部修补谓词，由显式系统复位计划删除 quiescent → SRST 路径并迁移验收承接收口 |

常驻手册：[`REVIEW.md`](REVIEW.md) 规定设计与代码两类 Review 的事后审查纪律（不进入任务流程、不阻碍验收）；`DEBUG-PLAYBOOK.md` 与 `TOOLING-PITFALLS.md` 分别记录调试和工具纪律。

## 挂起项

无计划文档、纯等触发条件的延后项素引（收口时扫描本表：条件到即转正式计划或并入主线；本表只存索引，真值在详情列所指处）。有计划文档的排队看活跃计划表，会消灭的问题看 `KNOWN_ISSUES.md`，review 承接看 carryover——三者不在此重复。

| 事项 | 触发 | 详情 |
|---|---|---|
| CPU 预约对象（budget/period、pick 边界配额过滤） | 不可信域接入 | `ideas/task.md`「Job」 |
| fence.i 代码代次优化 | **已满足**（step 7 active 集合）；另一半条件「开销实测可见」未验 | `impls/task.md`「调度」 |
| 显式 affinity / 跨域迁移 ABI | 多线程（ThreadSpawn）/迁移纪元 | `archived/todo-2026-08-28-domain-eligibility.md` 决策 3–4 |
| initfs manifest / 服务编排 | 需要正式服务编排 | `ideas/bootstrap.md` |
| ld-erhino 动态链接（PT_INTERP、共享库） | 无明确触发，构想态 | `ideas/bootstrap.md` |
| F-only/Q/V/TSO 档位建模 | 真实需求出现 | `impls/execution-context.md` |
| TLS ABI（用户 tp 置零中） | 需要 TLS 时 | `impls/task.md` |
| 多用户 / ACL | 多用户需求 | `ideas/object.md` |
| ASID 分配 + 定向 shootdown 优化 | 地址空间切换开销实测 | `impls/task.md` |

## 位置

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、BootPackage initial ELF bootstrap、syscall 面 Debug/Exit/Extend/Sleep、进程回收、timer/IPI 通路）与执行环境重构（a9a65cb）。IPC 前地基工程已完成（hart 身份统一、锁内存序、所有权单向化、uaccess 集中化、Extend 字节 sbrk 语义；见 `plans/archived/2026-09-pre-ipc-groundwork.md`）。IPC 对象 / Handle 重建也已完成：进程本地 HandleTable、WaitContext、显式 Mailbox/Notification、原子 Handle move、Endpoint/Invitation 与 Acquire/Release Runnel 已贯通，实施档案见 [已归档计划](archived/2026-08-ipc-object-foundation.md)，实现现状见 `notes/impls/ipc.md`。
- 当前：**显式系统复位与 power 服务**——先形成 power 方向设计并确认，再以 capability 授权的用户态 reset 替代调度器 quiescent 自动关机；原有业务/资源验收锚点全部保留，终态改由 init 完成收束后命令独立 power 服务提交。计划见 [`todo-2026-09-explicit-system-reset.md`](todo-2026-09-explicit-system-reset.md)。
- 后续前置：完整化 Running Map/Unmap、映射账本、guard、remote TLB shootdown 与有界回收；该项收口后重审 ThreadSpawn 的用户栈、结果槽与 join handle 契约。批一 Start 拆解与事务复审已经收口，不因批二阻塞而回退。
- 已完成：**竞态矩阵覆盖增强已收口（2026-09）**——锤侧延迟变体（`Cmd.aux` 转正为执行前延迟，奇数轮锤延迟 10ms），kill-vs-exit/fault/abandon 双侧终因均有胜出记录，全验证线 10/10；实施档案见 [archived/todo-2026-08-28-race-matrix-coverage.md](archived/todo-2026-08-28-race-matrix-coverage.md)。
- 已完成：**完整进程生命周期 step 1–10 已收口（2026-08-28）**——per-hart 索引最小堆 Timeout queue、WaitContext 稳定 token 注销、任意非零预算 ProcessDrain、Invitation 非等待角色与 fail-closed QEMU acceptance 已落地；原启动大事务随后演进为独立 Grant/Attach 与纯发布 Start，当前实现以本节批一事务复审结论为准。实施档案见 `archived/todo-2026-08-26-process-lifecycle.md` 与 `archived/todo-2026-08-28-step10-correctness.md`。
- 已完成：**完整进程生命周期 step 2–6 已落地**——step 2–4（ProcessControl 前移、lifecycle 顶级锁状态机、全局进程表退役、跨 hart kill、硬上界 ProcessDrain、init 监督闭环）已过统一代码 Review（[archived/review-2026-08-27-process-lifecycle-code-review.md](archived/review-2026-08-27-process-lifecycle-code-review.md)）；step 5（Job 管理面）已实施收口：JobSeal/Query/Enumerate/Derive、有序成员表、链锁封口、完成传播与 libprocess 递归 job_kill；**step 6（持久 init 监督政策与 pm 委托域）已实施收口（2026-08-28）**：init 建 root → services → pm_domain/acceptance 拓扑，委托域 JobControl 经 StartupBlock grants 授 pm（MANAGE|READ|WAIT，无 CREATE）而 init 保留复制件作直接收束权；pm 对域内 Running 靶走 枚举→派生（铸造）→kill→drain→seal；失败路径整树 job_kill(services)；init 全部收束后常驻管理端点、不自终止，终态交 quiescent 静默停机（公理入档 ideas/bootstrap.md）；拓扑快照两处打印供调试。FAL 剩余面（DirectoryGrant、每订阅者 watch、跨进程 provider）仍排在其后。
- **机制层泛化改造已落地（2026-08-27）**：以 [archived/review-2026-08-27-mechanism-generalization.md](archived/review-2026-08-27-mechanism-generalization.md) 为纲的四批改造——① impls 失同步修复与 KernelRequest 正名（每机制恰一篇拥有的归属纪律入 README）；② Timeout queue per-hart 化（唤醒所有权结构化；本轮已演进为稳定 token 的索引最小堆）；③ RAII 收束契约（tunnel `MappingLease`、`phys_to_virt` 栈区 debug 断言、ideas/object.md「收束分层」公理替代 close fanout 枚举证明）；④ **Lock Ladder**：`sync::ranks` 秩表 + per-hart 秩栈断言（同秩链段 key 递增：链锁 jid、表嵌套 pid；talc 经 `RankedRawSpinlock` 类型级注入；bootstrap 专用帧经 formal entry 切换），锁序契约按实测重写并修正旧基线三错（lifecycle 方向、drain_gate/HEAP/POOL 未入档、AddressSpace 双层）。全部负载 debug 构建验证无违规；reserve/commit/rollback 协议四要素成文。
- FAL/RPC 首批已落地——方向 C 拍板（无中央 VFS、symlink 无 hardlink、Lookup 三值应答）；librpc（RpcPrefix/同步 Caller）、libfal（线协议/memfs/provider）、libfs（前缀表/走路引擎）与 fs 真路径验收线达成（`54d3e02`/`bf32c1c`，实现现状见 `notes/impls/fal.md`）。
- IPC ABI 基座已重构：Entry 保存 immutable badge，MessageHeader 区分 sender_pid/sender_badge，Mailbox owner 可 mint sender；TRANSIT/GRANT 分离 buffered message 与 Building 期 direct grant；send-once target/transit alias 已拒绝。完整审查见 [`archived/review-2026-08-26-notes-design.md`](archived/review-2026-08-26-notes-design.md)，实现现状见 `notes/impls/ipc.md`。
- StartupBlock v2 与 BootPackage 启动链已落地：outer 为 Header + 实际 child Handle 数组 + 可零 padding + opaque payload；内核只解析 fixed envelope 与唯一 init ELF，payload 以只读页交给 init（映入即收编 owned backing）。实现现状见 `notes/impls/startup.md`。
- 对照负载：`user/systems/` 与 `user/drivers/` 五服务（fs/init/pm/drv_spi_sifive + D64 验证负载 srv_fp）。fs 经用户态 FAL 真路径完成创建/枚举/属性/符号链接/偏移读写，验收线已过（`bf32c1c`）；旧 fs ABI 尸体已清，KNOWN_ISSUES 桩条目已消解。
- 用户态 launcher 基座已落地：root Job/JobControl、affine ProcessBuilder、Building-only Map/Write/Grant/Attach、纯发布 ProcessStart、ProcessControl 与公共 `libprocess` 已贯通；组装失败统一执行 builder close → ProcessDrain → control close，Grant 是否已消费由 `SpawnFailure` 显式报告。init 以临时 ustar 政策启动其余负载，内核不含 tar/service policy；initfs manifest/archive 仍在需要正式服务编排时另案设计。方向见 `notes/ideas/{task,bootstrap}.md`，实现现状见 `notes/impls/{startup,task}.md`。
- 下一自然序：显式系统复位与 power 服务 → 用户内存 Map/Unmap 完整化 → ThreadSpawn 批二/批三 → 以压力与真实负载启动 IPC 数据面设计 → FAL 剩余面（DirectoryGrant、跨进程 provider、服务发现）→ 设备/中断接入 → 异构（分片计划见活跃计划表）。**step 7（ThreadSpawn 前多线程 teardown barrier）已实施收口（2026-08-28）**：线程成员表（tid 寻址、离场即摘）取代单值记录，等待取消锁外游标化（零分配），归一收敛到 trap 汇编非 Resume 出口，KNOWN_ISSUES 写回 panic 面消解（deliver_output 复检即杀 + 分发出口终止检查）；ThreadSpawn 接入面清单入档计划篇。**首次 release 验证暴露并修复 trap 入口 x5 破坏**（SPP 检查在保存前用 t0，每次用户 trap 覆写用户 x5；修复经已保存的 t5 中转，寄存器纪律入档 execution-context.md，调查档案 [archived/review-2026-08-28-release-trap-entry-x5.md](archived/review-2026-08-28-release-trap-entry-x5.md)）；自此 release 验证线纳入阶段收尾必跑（`just virt-release`）。**step 8（capability-derived 调度域 eligibility 与 D64 开放）已实施收口（2026-08-28）**：域按需求满足签名等价类推导（`os/sched_domain` 纯逻辑 crate，host 可测）、boot 冻结、Start 提交点绑定进程，多域默认落最弱兼容域；reserve/commit/rollback 上收 `SchedClass` trait（F2 勾销）；D64 兼容谓词修正（FLEN 恰 64，Q 排除）；验证面新增 `srv_fp` D64 负载（gc target：fsqrt/fmadd 位型、FPR/fcsr 跨 trap 往返、轮转复检）与 `virt-hetero`/`virt-nofd` 多域 DTB 变体（`tools/make-hetero-dts.py` + `ERHINO_DTB`）；virt/virt-release/hetero/nofd/sifive_u/host 全绿，方向公理入档 ideas/task.md「线程」。生命周期 step 10 文档终态已收口。**step 9（多核竞态验证矩阵）已实施收口（2026-08-28）**：`srv_hammer` 双锤负载（HAMMER 执行器/TARGET 竞态靶，`libprocess::race` 线协议）+ init 竞态矩阵段 10 场景（kill vs kill/Exit/fault/Start/park/abandonment、并发 Create+枚举乱序窗口、seal vs 并发 Create、双 Drain ObjectBusy 仲裁、最后 control 消散派生兑底）；验证中修复内核 `dealloc_bounded` 完成路径 off-by-one（最后一跳用满预算时 work_done 超 max 违约，host 回归补齐）与 sifive_u BootPackage 装载窗口（尾部 32MB→64MB，零内存代价）；virt/virt-release/hetero/nofd/sifive_u 矩阵 10/10、host 全绿。**多线程内核前置已经就绪，公开 ThreadSpawn 仍受用户内存映射机制阻塞**——teardown barrier、线程成员表、active 位图、每线程 FP 状态与域绑定无需回退；完整 Map/Unmap、guard 与 remote TLB shootdown 收口后，ThreadSpawn 才重开用户态资源契约与实现。BootPackage / launcher 基座已过机制层审查（[`archived/review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)，F1 payload 收编 owned backing、F3 Pid 拓宽 u64 已实施）；initfs 内部协议在需要正式服务编排时单独设计。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 文档即决策：方向性结论进 notes/，本文件只导航；每轮收口于「决策入档、代码全绿（debug 与 release）已提交、下一步自然序明确」。
