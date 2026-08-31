# 罗盘

> 跨会话导航：方向、位置、戒律。只存上下文不排任务——走法由目标与架构自然序决定，每次收口时维护。

## 方向

构建 notes/ 所描述的 Halcyon：以 RISC-V 微内核 eRhino（内核二进制 `erhino_kernel`）为核心，用户态以 `rinlib`、系统服务与跨组件契约共同构成。方向性结论（细节见 notes/ 对应篇）：

- **微内核 ↔ 协作式互为因果**：长工作一律在用户态服务，内核路径恒短，内核态不可打断是推论不是选项（ideas/kernel.md「协作式内核」）。
- **调度 = 域—类—执行点三层**：异构 hart 即多域；策略在类内整体可替换；扩展是横向加项不是改结构（ideas/task.md、impls/task.md「调度」）。
- **单一归属不变量**：线程任意时刻恰处于一个容器——类队列 / hart current / 无容器（impls/task.md）。
- **唤醒所有权**：timer = 自己的确定期限，IPI = 他方请求，无主唤醒不存在（impls/internals.md「唤醒所有权」）。
- **异步 syscall = 内核请求 + wake**：内核永不等待，阻塞表达为 Waiting，完成即唤醒（ideas/call.md）。
- **ABI 演进两侧同步**：shared/ 不冻结，内核与 rinlib 一起改。
- **纯 capability 授权**：无进程权限等级；平台根由内核按事实铸造、init 决定策略；Handle 以 TRANSIT/GRANT 区分消息暂存与直接跨表安装，badged sender 承载用户态 grant（`notes/ideas/object.md`）。
- **框架先行、实现从简**：整体系统设计为先，搭框架再填充——结构一次到位，实现按需求渐进替换（如调度域/类）。
- **AddressSpace 是内存所有权 seam**：进程先有稳定 Unbound 身份，Building 期以一次性 ProcessBindMemory 附入 PoolBinding 与页表后转为 Bound；区域账本是映射真值，匿名 mapping 自有 affine extents，共享字节使用固定长度、容量有界的 MemoryObject；所有变更走 validate/reserve/commit/publish/synchronize/retire，跨 hart 完成以 epoch + Remote Call 确认闭合（`notes/ideas/mm.md`）。
- **资源能力与 Job 正交**：Job 只做创建、成员与收束；MemoryPool 只支付 page-backed storage，KernelMemoryBudget 支付内核 metadata，CPU 由预约对象支付，设备由各自 capability 授权。ProcessCreate 只建 Building 空壳，资源经独立操作附入；capability 跨 Job 转移不改资源来源（`notes/ideas/{task,mm,object}.md`）。

## 活跃计划

plans/ 根目录只放活跃计划（todo 与含未闭合承接项的 review 同为计划）；调查收口即移入 `archived/`，`ref-*` 是死的参考资料。当前全部活跃计划：

| 文件 | 概要 |
|---|---|
| [`todo-2026-08-system-audit.md`](todo-2026-08-system-audit.md) | 重写版系统审查 7 分片：01 SBI 与 02 trap/上下文已完成收口，03–07 待做 |
| [`todo-2026-09-power-management-service.md`](todo-2026-09-power-management-service.md) | 未来设计项：独立用户态电源管理服务需要另行设计，当前不预设职责、拓扑、协议或 capability 分配 |
| [`todo-2026-09-system-shutdown-orchestration.md`](todo-2026-09-system-shutdown-orchestration.md) | 未来设计项：闭合用户态从关机意图到最终 reset 的服务收束政策，不预设执行主体、拓扑或协议 |
| [`todo-2026-08-30-user-memory-mapping-review.md`](todo-2026-08-30-user-memory-mapping-review.md) | 未来审查：用户内存切片 1–8B（`1cd6ab2` 至 `bdc83ef`）及压力收口 `004cae5` 的统一 AddressSpace、失败原子、Remote shootdown、lease/backing 与 ThreadSpawn/join 组合所有权 |
| [`todo-2026-08-30-remote-call-review.md`](todo-2026-08-30-remote-call-review.md) | 未来审查：`6199985` 的固定槽/RVWMO/epoch 基座及 `004cae5` 的真实同 AddressSpace stale-translation 与后续 Retire 组合 |
| [`todo-2026-09-memory-object-data-plane.md`](todo-2026-09-memory-object-data-plane.md) | 当前主线：平台供给、系统储备、MemoryPool、funded frame、空壳 ProcessBindMemory、root bootstrap、资金化 owner/admission、owner-aware 页表协议与分批 deferred retire 已完成；下一步完成 root/全部用户页表与匿名 backing 全面资金化，随后进入公共 MemoryObject/多页 Tunnel/Runnel v2 |
| [`todo-2026-09-deferred-retire-review.md`](todo-2026-09-deferred-retire-review.md) | 未来审查：提交 `7225673` 的显式 Retiring、固定容量 work debt、owner hart/Pending 电平、table/backing/metadata 分批退休、Tunnel/ProcessDrain 统一接管与最终完成顺序 |
| [`todo-2026-09-process-memory-binding-bootstrap-review.md`](todo-2026-09-process-memory-binding-bootstrap-review.md) | 未来审查：提交 `7c76097` 的 Unbound Process、一次性 Bind、Building 截止、metadata 壳寿命、funded root、root Pool capability 与 bootstrap payload owner 闭包 |
| [`todo-2026-09-funded-owner-page-table-lifecycle-review.md`](todo-2026-09-funded-owner-page-table-lifecycle-review.md) | 未来审查：提交 `c522e50` 的 funded owner 守恒分解、切片 6 metadata admission、owner-aware 页表事务、空表剪枝、Remote ack owner 保活、可恢复 drain 与栈边界 |
| [`todo-2026-09-funded-frame-broker-review.md`](todo-2026-09-funded-frame-broker-review.md) | 未来审查：提交 `48227c8` 的双账本事务顺序、仿射回滚、清零发布边界、固定 extent storage、栈 guard 与 raw/adopt 类型隔离 |
| [`todo-2026-09-memory-pool-review.md`](todo-2026-09-memory-pool-review.md) | 未来审查：提交 `4715f3a` 的 root 额度闭包、Pool 线性 token/自然退款、metadata sponsor、Handle 发布原子、rights/ABI 与 rinlib affine owner |
| [`todo-2026-09-platform-memory-ledger-review.md`](todo-2026-09-platform-memory-ledger-review.md) | 未来审查：提交 `198e665` 的 Devicetree admission、物理分类守恒、no-map 双重排除、transition/direct-map 静态预算与双平台启动闭包 |
| [`todo-2026-09-system-supply-reserve-review.md`](todo-2026-09-system-supply-reserve-review.md) | 未来审查：提交 `0a944c7` 的固定 workspace planner、system typed tickets、FramePool/heap 物理隔离、静态容量证明与三平台分类闭包 |
| [`todo-2026-09-platform-reserved-memory-lifecycle.md`](todo-2026-09-platform-reserved-memory-lifecycle.md) | 未来规范支持：动态 `/reserved-memory` 放置、region identity/设备引用与 `reusable` 可撤回借用；须在正式设备/DMA 资源接入前完成 |
| [`todo-2026-08-26-review-carryover.md`](todo-2026-08-26-review-carryover.md) | 归档 review 中未闭合承接项的唯一跟踪点：设备/中断接入重审 |
| [`todo-2026-08-27-mechanism-generalization-review.md`](todo-2026-08-27-mechanism-generalization-review.md) | 机制层泛化改造（15c7811/9c03251/95deea6）的未来审查计划：Lock Ladder、per-hart Timeout、MappingLease 与文档自洽；共享事务骨架触发条件已满足，由当前 AddressSpace/MemoryChange 主线承接 |
| [`todo-2026-08-26-bootstrap-launcher-review.md`](todo-2026-08-26-bootstrap-launcher-review.md) | 机会型任务：BootPackage / launcher 十切片代码审查，有空就做，不阻塞主线 |
| [`todo-2026-08-28-thread-teardown-review.md`](todo-2026-08-28-thread-teardown-review.md) | 未来审查：step 7 `d741880` 与 ThreadSpawn `bdc83ef`/`004cae5` 的成员表、离场屏障、ThreadDeparture、join 发布和锁序 |
| [`todo-2026-08-28-domain-eligibility-review.md`](todo-2026-08-28-domain-eligibility-review.md) | 生命周期 step 8（调度域 eligibility，提交 1d7dc92）的未来审查计划：域推导、绑定冻结、域内 idle 与 IPI 路由 |
| [`todo-2026-08-28-persistent-init-review.md`](todo-2026-08-28-persistent-init-review.md) | 生命周期 step 6（持久 init/pm 委托域）的未来审查计划：authority 边界、收束兜底与显式 reset 后的 supervisor 语义 |

常驻手册：[`REVIEW.md`](REVIEW.md) 规定设计与代码两类 Review 的事后审查纪律（不进入任务流程、不阻碍验收）；`DEBUG-PLAYBOOK.md` 与 `TOOLING-PITFALLS.md` 分别记录调试和工具纪律。

## 挂起项

无计划文档、纯等触发条件的延后项素引（收口时扫描本表：条件到即转正式计划或并入主线；本表只存索引，真值在详情列所指处）。有计划文档的排队看活跃计划表，会消灭的问题看 `KNOWN_ISSUES.md`，review 承接看 carryover——三者不在此重复。

| 事项 | 触发 | 详情 |
|---|---|---|
| CPU 预约对象（budget/period、pick 边界配额过滤） | 不可信执行域接入 | `ideas/task.md`「线程」 |
| fence.i 代码代次优化 | **已满足**（step 7 active 集合）；另一半条件「开销实测可见」未验 | `impls/task.md`「调度」 |
| 显式 affinity / 跨域迁移 ABI | 多线程（ThreadSpawn）/迁移纪元 | `archived/todo-2026-08-28-domain-eligibility.md` 决策 3–4 |
| initfs manifest / 服务编排 | 需要正式服务编排 | `ideas/bootstrap.md` |
| ld-erhino 动态链接（PT_INTERP、共享库） | 无明确触发，构想态 | `ideas/bootstrap.md` |
| F-only/Q/V/TSO 档位建模 | 真实需求出现 | `impls/execution-context.md` |
| TLS ABI（用户 tp 置零中） | 需要 TLS 时 | `impls/task.md` |
| 多用户 / ACL | 多用户需求 | `ideas/object.md` |
| ASID 分配 + 定向 shootdown 优化 | 地址空间切换开销实测 | `impls/task.md` |

## 位置

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、BootPackage initial ELF bootstrap、syscall 面 Debug/Exit/MemoryMap/MemoryUnmap/MemoryProtect/Sleep、进程回收、timer/IPI 通路）与执行环境重构（a9a65cb）。IPC 前地基工程已完成（hart 身份统一、锁内存序、所有权单向化、uaccess 集中化；见 `plans/archived/2026-09-pre-ipc-groundwork.md`）。IPC 对象 / Handle 重建也已完成：进程本地 HandleTable、WaitContext、显式 Mailbox/Notification、原子 Handle move、Endpoint/Invitation 与 Acquire/Release Runnel 已贯通，实施档案见 [已归档计划](archived/2026-08-ipc-object-foundation.md)，实现现状见 `notes/impls/ipc.md`。
- 当前：**MemoryPool、MemoryObject 与多页 IPC 数据面**——平台供给账本（`198e665`）、物理隔离系统储备（`0a944c7`）、strict count-solvent MemoryPool、funded frame broker、切片 4+5 的 Unbound Process/一次性 ProcessBindMemory/root Pool bootstrap、切片 6A/6B 的资金化 owner/admission 与 owner-aware 页表生命周期协议，以及切片 6C 的分批 deferred retire（`7225673`）已经收口；当前自然序是 root/全部用户页表资金化 → 匿名 backing 资金化，其上再开放公共 MemoryObject、多页 Tunnel 与 Runnel v2。实施计划见 [`todo-2026-09-memory-object-data-plane.md`](todo-2026-09-memory-object-data-plane.md)，方向见 `notes/ideas/{kernel,mm,object,task,bootstrap}.md`。
- 后续设计：用户态系统关机编排与独立电源管理服务分别立案，二者不互相预设执行主体、职责、拓扑、协议或 capability 分配。计划见 [`todo-2026-09-system-shutdown-orchestration.md`](todo-2026-09-system-shutdown-orchestration.md) 与 [`todo-2026-09-power-management-service.md`](todo-2026-09-power-management-service.md)。
- 已完成：**显式系统复位已收口（2026-09）**——eRhino 自有 reset ABI、primordial `SystemReset` capability、init 直接提交与 SBI 显式映射已落地；调度器不再从 quiescent 推断关机，idle 只负责 WFI 与唤醒路由；virt 五线与 sifive_u 平台失败返回均通过。实施档案见 [`archived/todo-2026-09-explicit-system-reset.md`](archived/todo-2026-09-explicit-system-reset.md)，旧竞态调查见 [`archived/todo-2026-08-29-early-quiescent-shutdown.md`](archived/todo-2026-08-29-early-quiescent-shutdown.md)。
- 已完成：**竞态矩阵覆盖增强已收口（2026-09）**——锤侧延迟变体（`Cmd.aux` 转正为执行前延迟，奇数轮锤延迟 10ms），kill-vs-exit/fault/abandon 双侧终因均有胜出记录，全验证线 10/10；实施档案见 [archived/todo-2026-08-28-race-matrix-coverage.md](archived/todo-2026-08-28-race-matrix-coverage.md)。
- 已完成：**完整进程生命周期 step 1–10 已收口（2026-08-28）**——per-hart 索引最小堆 Timeout queue、WaitContext 稳定 token 注销、任意非零预算 ProcessDrain、Invitation 非等待角色与 fail-closed QEMU acceptance 已落地；原启动大事务随后演进为独立 Grant/Attach 与纯发布 Start，当前实现以本节批一事务复审结论为准。实施档案见 `archived/todo-2026-08-26-process-lifecycle.md` 与 `archived/todo-2026-08-28-step10-correctness.md`。
- 已完成：**完整进程生命周期 step 2–6 已落地**——step 2–4（ProcessControl 前移、lifecycle 顶级锁状态机、全局进程表退役、跨 hart kill、硬上界 ProcessDrain、init 监督闭环）已过统一代码 Review（[archived/review-2026-08-27-process-lifecycle-code-review.md](archived/review-2026-08-27-process-lifecycle-code-review.md)）；step 5（Job 管理面）已实施收口：JobSeal/Query/Enumerate/Derive、有序成员表、链锁封口、完成传播与 libprocess 递归 job_kill；**step 6（持久 init 监督政策与 pm 委托域）已实施收口（2026-08-28）**：init 建 root → services → pm_domain/acceptance 拓扑，委托域 JobControl 经 StartupBlock grants 授 pm（MANAGE|READ|WAIT，无 CREATE）而 init 保留复制件作直接收束权；pm 对域内 Running 靶走 枚举→派生（铸造）→kill→drain→seal；失败路径整树 job_kill(services)；init 正常路径在全部收束后提交显式 reset，平台拒绝时常驻管理端点保持 root supervisor；拓扑快照两处打印供调试。FAL 剩余面（DirectoryGrant、每订阅者 watch、跨进程 provider）仍排在其后。
- **机制层泛化改造已落地（2026-08-27）**：以 [archived/review-2026-08-27-mechanism-generalization.md](archived/review-2026-08-27-mechanism-generalization.md) 为纲的四批改造——① impls 失同步修复与 KernelRequest 正名（每机制恰一篇拥有的归属纪律入 README）；② Timeout queue per-hart 化（唤醒所有权结构化；本轮已演进为稳定 token 的索引最小堆）；③ RAII 收束契约（tunnel `MappingLease`、`phys_to_virt` 栈区 debug 断言、ideas/object.md「收束分层」公理替代 close fanout 枚举证明）；④ **Lock Ladder**：`sync::ranks` 秩表 + per-hart 秩栈断言（同秩链段 key 递增：链锁 jid、表嵌套 pid；talc 经 `RankedRawSpinlock` 类型级注入；bootstrap 专用帧经 formal entry 切换），锁序契约按实测重写并修正旧基线三错（lifecycle 方向、drain_gate/HEAP/POOL 未入档、AddressSpace 双层）。全部负载 debug 构建验证无违规；reserve/commit/rollback 协议四要素成文。
- FAL/RPC 首批已落地——方向 C 拍板（无中央 VFS、symlink 无 hardlink、Lookup 三值应答）；librpc（RpcPrefix/同步 Caller）、libfal（线协议/memfs/provider）、libfs（前缀表/走路引擎）与 fs 真路径验收线达成（`54d3e02`/`bf32c1c`，实现现状见 `notes/impls/fal.md`）。
- IPC ABI 基座已重构：Entry 保存 immutable badge，MessageHeader 区分 sender_pid/sender_badge，Mailbox owner 可 mint sender；TRANSIT/GRANT 分离 buffered message 与 Building 期 direct grant；send-once target/transit alias 已拒绝。完整审查见 [`archived/review-2026-08-26-notes-design.md`](archived/review-2026-08-26-notes-design.md)，实现现状见 `notes/impls/ipc.md`。
- StartupBlock v2 与 BootPackage 启动链已落地：outer 为 Header + 实际 child Handle 数组 + 可零 padding + opaque payload；内核只解析 fixed envelope 与唯一 init ELF，payload 由 boot-held owner 直接转为 root-funded immutable lease backing，init 同时取得与内部 PoolBinding 同源的 root MemoryPool capability。实现现状见 `notes/impls/startup.md`。
- 对照负载分置于 `user/services/`、`user/drivers/` 与 `user/tests/`：当前服务为 `srv_init`、`srv_fs`、`srv_pm`，驱动为 `drv_spi_sifive`，验收进程为 `test_fp`、`test_hammer`、`test_target`。`srv_fs` 经用户态 FAL 真路径完成创建、枚举、属性、符号链接与偏移读写；旧 fs ABI 尸体已清，KNOWN_ISSUES 桩条目已消解。
- 用户态 launcher 基座已落地：root Job/JobControl、affine ProcessBuilder、显式 MemoryPool、一次性 ProcessBindMemory、Bound 后的 Building-only Map/Write/Grant/Attach、纯发布 ProcessStart、ProcessControl 与公共 `libprocess` 已贯通；组装失败统一执行 builder close → ProcessDrain → control close，Grant 是否已消费由 `SpawnFailure` 显式报告。init 以临时 ustar 政策启动其余负载，内核不含 tar/service policy；initfs manifest/archive 仍在需要正式服务编排时另案设计。方向见 `notes/ideas/{task,bootstrap}.md`，实现现状见 `notes/impls/{startup,task}.md`。
- 下一自然序：页表/匿名 backing 全面资金化 → 公共 MemoryObject/多页 Tunnel/Runnel v2 → FAL 剩余面（DirectoryGrant、跨进程 provider、服务发现与 Open）→ BufferQueue 与设备/中断/DMA 接入 → 异构；系统关机编排与独立电源管理服务分别保留为未来设计项，不阻塞当前自然序。**step 7（ThreadSpawn 前多线程 teardown barrier）已实施收口（2026-08-28）**：线程成员表（tid 寻址、离场即摘）取代单值记录，等待取消锁外游标化（零分配），归一收敛到 trap 汇编非 Resume 出口，KNOWN_ISSUES 写回 panic 面消解（deliver_output 复检即杀 + 分发出口终止检查）；ThreadSpawn 接入面清单入档计划篇。**首次 release 验证暴露并修复 trap 入口 x5 破坏**（SPP 检查在保存前用 t0，每次用户 trap 覆写用户 x5；修复经已保存的 t5 中转，寄存器纪律入档 execution-context.md，调查档案 [archived/review-2026-08-28-release-trap-entry-x5.md](archived/review-2026-08-28-release-trap-entry-x5.md)）；自此 release 验证线纳入阶段收尾必跑（`just virt-release`）。**step 8（capability-derived 调度域 eligibility 与 D64 开放）已实施收口（2026-08-28）**：域按需求满足签名等价类推导（`os/sched_domain` 纯逻辑 crate，host 可测）、boot 冻结、Start 提交点绑定进程，多域默认落最弱兼容域；reserve/commit/rollback 上收 `SchedClass` trait（F2 勾销）；D64 兼容谓词修正（FLEN 恰 64，Q 排除）；验证面新增 `test_fp` D64 负载（gc target：fsqrt/fmadd 位型、FPR/fcsr 跨 trap 往返、轮转复检）与 `virt-hetero`/`virt-nofd` 多域 DTB 变体（`tools/make-hetero-dts.py` + `ERHINO_DTB`）；virt/virt-release/hetero/nofd/sifive_u/host 全绿，方向公理入档 ideas/task.md「线程」。生命周期 step 10 文档终态已收口。**step 9（多核竞态验证矩阵）已实施收口（2026-08-28）**：`test_hammer` 双锤负载（HAMMER 执行器/TARGET 竞态靶，`libprocess::race` 线协议）+ init 竞态矩阵段 10 场景（kill vs kill/Exit/fault/Start/park/abandonment、并发 Create+枚举乱序窗口、seal vs 并发 Create、双 Drain ObjectBusy 仲裁、最后 control 消散派生兑底）；验证中修复内核 `dealloc_bounded` 完成路径 off-by-one（最后一跳用满预算时 work_done 超 max 违约，host 回归补齐）与 sifive_u BootPackage 装载窗口（尾部 32MB→64MB，零内存代价）；virt/virt-release/hetero/nofd/sifive_u 矩阵 10/10、host 全绿。**ThreadSpawn 三批与用户内存 8B 已收口（`bdc83ef` / `004cae5`）**——teardown barrier、成员表、active 位图、每线程 FP 状态与域绑定直接复用；Running spawn 使用独立 Spawning 状态，用户态以普通匿名 mapping 建立双 guard 栈并由 JoinHandle 结构化收束，线程级 result obligation 与进程 mandatory_ops 保持不同职责；竞态矩阵扩至 16/16，carryover IPC 压力四线与 `sifive_u` 连续十轮通过，实施计划已归档。BootPackage / launcher 基座已过机制层审查（[`archived/review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)，F1 payload 收编 owned backing、F3 Pid 拓宽 u64 已实施）；initfs 内部协议在需要正式服务编排时单独设计。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 文档即决策：方向性结论进 notes/，本文件只导航；每轮收口于「决策入档、代码全绿（debug 与 release）已提交、下一步自然序明确」。
