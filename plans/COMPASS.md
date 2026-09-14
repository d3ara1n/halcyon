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
- **单一内存事务核**：匿名与对象来源、Running 与 Building authority、结果 cookie 与 view 发布都是同一事务的字段维度，不是平行编排；新增来源或输出是加一个维度，不是加一套 plan/complete（`notes/impls/mm.md`「用户地址空间」）。
- **AddressSpace 是内存所有权 seam**：进程先有稳定 Unbound 身份，Building 期以一次性 ProcessBindMemory 附入 PoolBinding 与页表后转为 Bound；区域账本是映射真值，匿名 mapping 自有 affine extents，共享字节使用固定长度、容量有界的 MemoryObject；所有变更走 validate/reserve/commit/publish/synchronize/retire，跨 hart 完成以 epoch + Remote Call 确认闭合（`notes/ideas/mm.md`）。
- **资源能力与 Job 正交**：Job 只做创建、成员与收束；MemoryPool 只支付 page-backed storage，KernelMemoryBudget 支付内核 metadata，CPU 由预约对象支付，设备由各自 capability 授权。ProcessCreate 只建 Building 空壳，资源经独立操作附入；capability 跨 Job 转移不改资源来源（`notes/ideas/{task,mm,object}.md`）。

## 活跃计划

公共时间与公共对象已完成原交付；时间提交 `c6e0a84`，实现见 `notes/impls/{time,ipc}.md`。分支级设计审视另识别了公共操作与回收边界收束需求，不把原交付通过视为现有结构必须保留；本轮只更新设计与计划，没有实施重构或新增验收通过声明。

**当前下一任务**：[消息运输 → 流运输/Runnel → 通用执行与准入 → 完整 RPC/Outbox](todo-2026-09-13-service-runtime-prerequisites.md) 前两闭包已实施并提交（消息运输 `3060dd8`，流运输 `a2aabed`，未来复核见 [消息运输 Review](todo-2026-09-14-message-transport-review.md) 与 [流运输 Review](todo-2026-09-14-stream-transport-review.md)）；下一实施为通用执行与准入闭包，先按 `AGENTS.md`「标准施工流程」完成接手、规模审计与设计闭包（Runtime 任务、WaitSet 注册、期限、Park/Wake、Close/Drain 停驻、监督接管边界及 Runnel 登记接入面与三条件观察结果形态）再编码。验收概率判定已改为确定性终因覆盖，历史 Tunnel 静默截断按墙钟敏感的偶发现象归档于 [`ref-2026-09-acceptance-timing-flake.md`](archived/ref-2026-09-acceptance-timing-flake.md)；当前无开放验收 todo，若新现场命中归档中的触发条件再立案。ProcessDrain 由受信任管理者在 REAPABLE 后推进，普通应用不持续轮询，保留该契约；自动全程回收、预算激励和重新设计全监督拓扑均不是当前前置。

后续串行位置：[共享包契约与归属](archived/todo-2026-09-13-workspace-package-ownership.md) 已完成 → [内核等待/请求/退休结构收束](todo-2026-09-14-public-operation-ownership.md) → [FAL 后端/授权闭包 → 业务操作](todo-2026-09-fal-service-capabilities.md)。共享包整理是独立的小型 workspace 迁移，不阻塞当前执行前置整体开工；只有实际证据表明某个缺失能力阻断当前闭包，才提升对应完整机制并同步依赖。每个机制包含真实消费者迁移、失败/退出和旧路径删除，组合验收是完成门。

无消费者的 Runnel 观察草稿面（`register`/`peer_attached`/`prepare_wait`/`all_consumed`，随基线 `d22b9d7` 入库）已在流运输闭包中删除，已立案终态访问缺陷随之消失；原始 ABI 工厂同批删除，typed create/attach 成为唯一构造入口，init 创建侧已迁移；观察/登记/取消语义移入通用执行闭包，由 Runtime 首个真实消费者共同定形。Delivery 独立身份与 Peek 已在消息闭包裁决保留。共享包整理已作为独立小型任务开工：`shared/` 现组织为 workspace，`erhino_shared` 与跨层纯逻辑库（含 `elf`、`tar`、`monotonic_id`、`ordered_table`、`timer_queue`、`metadata_admission`）各自保持独立 package。

当前开发分支为 `task/fal-service-capabilities`，从本地 `master` 的 `5d406a4` 分出；本次设计审视基线为 `bf48cab`。交接先读 [FAL 总计划的开发分支与交接](todo-2026-09-fal-service-capabilities.md#开发分支与交接)：运输/执行与 FAL 仍是草稿；验收可靠性首轮已收口，历史时间敏感现场保留只读归档。后续按机制闭包逐项提交，最终经授权合并，不自动 push。历史验证日志/诊断产物只在本机 artifacts，异机需按该交接节重跑。

plans/ 根目录保留活跃专题计划与含未闭合 findings 的 Review 报告。专题 todo 拥有当前实施，Review 保留目标提交证据与复核清单，二者不重复安排同一问题。已完成调查/复核进入 `archived/`，`ref-*` 是只读参考资料。当前全部活跃入口：

| 文件 | 概要 |
|---|---|
| [`todo-2026-09-13-fal-integration-baseline-review.md`](todo-2026-09-13-fal-integration-baseline-review.md) | 未来 Review：固定 `d22b9d7` 的 公共对象前置 交付与混合集成边界，不把 公共时间前置/运输/RPC/服务执行前置/FAL 草稿视为已完成能力 |
| [`todo-2026-09-13-monotonic-time-rpc-deadline-review.md`](todo-2026-09-13-monotonic-time-rpc-deadline-review.md) | 未来 Review：固定 `c6e0a84` 的公共时钟、绝对期限、运行期停止与真实消费者边界，不把跨 epoch/RPC/FAL 责任混入复核 |
| [`todo-2026-09-frame-source-selftest-review.md`](todo-2026-09-frame-source-selftest-review.md) | 未来代码 Review：固定复核 `606b59d` 的库存来源、boot-held affine owner、完整清零、切分退款与 child 来源保活，不阻塞 FAL 主线 |
| [`todo-2026-09-design-audit-followup-review.md`](todo-2026-09-design-audit-followup-review.md) | 未来 Review：固定复核 `4b27ce6` 与 `8aa7bc2` 的 RX 同步、重复工作删除、对象来源保活和 Sealing 收缩，不重开 A–E program |
| [`todo-2026-09-fal-service-capabilities.md`](todo-2026-09-fal-service-capabilities.md) | FAL 业务暂停：先运输/执行/RPC，随后共享包与内核执行结构收束，再恢复后端、授权与业务 |
| [`todo-2026-09-monotonic-time-rpc-deadline.md`](archived/todo-2026-09-monotonic-time-rpc-deadline.md) | 时间前置 公共时间前置 已完成并归档：精确时钟、MonotonicNow、绝对 Wait/Sleep/Send、运行期协作停止与现有消费者；由执行/业务任务消费完整 Deadline |
| [`todo-2026-09-14-message-transport-review.md`](todo-2026-09-14-message-transport-review.md) | 未来 Review：固定复核 `3060dd8` 的 typed 运输层、消费式 Packet、take/restore 重试与消费者迁移失败路径，不重开流闭包设计 |
| [`todo-2026-09-14-stream-transport-review.md`](todo-2026-09-14-stream-transport-review.md) | 未来 Review：固定复核 `a2aabed` 的观察草稿面删除、raw 工厂删除、srv_init typed 创建迁移与终态访问边界，不预审通用执行闭包的观察接入形态 |
| [`todo-2026-09-13-service-runtime-prerequisites.md`](todo-2026-09-13-service-runtime-prerequisites.md) | 四闭包：消息运输与流运输/Runnel 均已完成待 Review → 通用执行/准入（下一实施）→ RPC/Outbox；typed owner 已收口，事件驱动登记/观察/取消并入通用执行闭包 |
| [`todo-2026-09-14-public-operation-ownership.md`](todo-2026-09-14-public-operation-ownership.md) | 执行前置及共享包之后收束内核等待/请求/退休结构；保留 ProcessDrain，不作为当前整体开工前置 |
| [`todo-2026-09-14-user-memory-owner-lifecycle.md`](todo-2026-09-14-user-memory-owner-lifecycle.md) | 独立延期：执行基座和当前 FAL 基础交付后，遇到长期动态 mapping/正式 reaper 需求时统一映射、堆和栈 owner；当前运输清理不得转延期 |
| [`todo-2026-09-14-kernel-memory-budget.md`](todo-2026-09-14-kernel-memory-budget.md) | 独立延期：按不可信分配/创建域的 metadata 隔离需求触发，默认排 FAL 基础与映射 owner 后；不用于激励 pm Drain |
| [`todo-2026-09-fal-extended-operations.md`](todo-2026-09-fal-extended-operations.md) | 等真实消费者触发：递归 Copy/Delete、快照/持久性/原子替换、capability 属性 Copy、递归/可重放 Watch、append/组合 Open；不隐含在基本 FAL 完成中 |
| [`todo-2026-09-platform-reserved-memory-lifecycle.md`](todo-2026-09-platform-reserved-memory-lifecycle.md) | 未来规范支持：动态 `/reserved-memory` 放置、region identity/设备引用与 `reusable` 可撤回借用；须在正式设备/DMA 资源接入前完成 |
| [`todo-2026-09-power-management-service.md`](todo-2026-09-power-management-service.md) | 未来设计项：独立用户态电源管理服务需重新设计职责、拓扑、协议与 capability 分配 |
| [`todo-2026-09-system-shutdown-orchestration.md`](todo-2026-09-system-shutdown-orchestration.md) | 未来设计项：闭合用户态从关机意图到最终 reset 的服务收束政策，不预设执行主体、拓扑或协议 |
| [`todo-2026-08-26-review-carryover.md`](todo-2026-08-26-review-carryover.md) | 等设备/中断/DMA 接入触发的唯一 review 承接项 |
| [`todo-2026-09-kernel-final-architecture-review.md`](todo-2026-09-kernel-final-architecture-review.md) | 等 MemoryObject 主线、多页 Tunnel、Runnel v2 与主要用户态消费者完成，并在统筹批次 A–E 收口后执行的最终架构 review |
| [`todo-2026-09-14-architecture-audit-findings.md`](todo-2026-09-14-architecture-audit-findings.md) | 审计发现承接清单（A 设计层 / B 阶段矛盾 / C 容量依据 / M 内存分配 / D 文档定性）；不占活跃串行位，逐条审视后各自并入归属专题或单独立案 |

公共对象前置 公共对象前置 已完成并[归档](archived/todo-2026-09-13-public-ipc-wait-prerequisites.md)，实现与验证真值见 `notes/impls/ipc.md`：Native 坏输出/新轮停驻旧取消、通知历史/终态摘槽/非空压力、真实双接收线程与 forced Full、跨进程提交后 kill 和 GDB 已装 Close/active 窗口已补，旧 findings 复核关闭；core/128MiB/release/nofd/启动失败、七面 lint、163 项 host 通过。后续验收可靠性收口已补运行身份、阶段观测和确定性终因覆盖；历史 Tunnel 墙钟截断仅保留只读归档，不作为开放缺陷。原交付不包含本次公共操作边界重构；当前下一步以本节顶部接手顺序为准。

A–E 的五份专题实施计划、九份报告、Review program 与系统审计总计划均已归档。最终修复基线为 `228b6a5`，WiseHare/OliveWillow 定点复核通过，无开放 finding；过程与验证边界见 [`Review program 档案`](archived/todo-2026-09-review-program.md)。

常驻手册：[`REVIEW.md`](REVIEW.md) 规定设计与代码两类 Review 的事后审查纪律（不进入任务流程、不阻碍验收）；`DEBUG-PLAYBOOK.md` 与 `TOOLING-PITFALLS.md` 分别记录调试和工具纪律。

## 挂起项

无计划文档、纯等触发条件的延后项素引（收口时扫描本表：条件到即转正式计划或并入主线；本表只存索引，真值在详情列所指处）。有计划文档的排队看活跃计划表，会消灭的问题看 `KNOWN_ISSUES.md`，review 承接看 carryover——三者不在此重复。

| 事项 | 触发 | 详情 |
|---|---|---|
| CPU 预约对象（budget/period、pick 边界配额过滤） | 不可信执行域接入 | `ideas/task.md`「线程」 |
| fence.i 代码代次优化 | active 集合条件已满足；另一半「开销实测可见」需先具备 dispatch 计时手段（Zicntr）且占比可见 | `impls/task.md`「调度」 |
| 显式 affinity / 跨域迁移 ABI | 真实多域硬件成为运行环境或出现多域放置需求（ThreadSpawn 前置已落地）；接入边界公理见详情 | `ideas/execution-context.md`「调度域」、`archived/todo-2026-08-28-domain-eligibility.md` 决策 3–4 |
| initfs manifest / 服务编排 | 需要正式服务编排 | `ideas/bootstrap.md` |
| ld-erhino 动态链接（PT_INTERP、共享库） | 无明确触发，构想态 | `ideas/bootstrap.md` |
| F-only/Q/V/TSO 档位建模 | 真实需求出现 | `impls/execution-context.md` |
| TLS ABI（用户 tp 置零中） | 需要 TLS 时 | `impls/task.md` |
| 多用户 / ACL | 多用户需求 | `ideas/object.md` |
| ASID 分配 + 定向 shootdown 优化 | 地址空间切换开销实测 | `impls/task.md` |
| 过渡 admission/Remote 槽高水位可观测性 | KernelMemoryBudget 立案或容量重校需求出现 | `ideas/mm.md`「MemoryPool 与 backing charge」 |
| Unmap 调用者唤醒点前移（Synchronize 即返回、retire 后台化） | 内存操作延迟实测成为瓶颈（需先具备测量面） | `ideas/mm.md`「MemoryChange 事务」 |

## 位置

以下为历史批次与验证证据的导航，段内保留的“下一步”描述属于当时交接；当前任务与顺序以「活跃计划」顶部和对应唯一 todo 为准。

- 已完成：boot/高半区启动协议、帧池（os/frame_pool）、堆、Sv39 页表（os/page_table）、板级解析（os/dtb）、任务模型（trap 路径与 trap 锚、域—类调度、进程/线程、BootPackage initial ELF bootstrap、syscall 面 Debug/Exit/MemoryMap/MemoryUnmap/MemoryProtect/Sleep、进程回收、timer/IPI 通路）与执行环境重构（a9a65cb）。IPC 前地基工程已完成（hart 身份统一、锁内存序、所有权单向化、uaccess 集中化；见 `plans/archived/2026-09-pre-ipc-groundwork.md`）。IPC 对象 / Handle 重建也已完成：进程本地 HandleTable、WaitContext、显式 Mailbox/Notification、原子 Handle move、Endpoint/Invitation 与 Acquire/Release Runnel 已贯通，实施档案见 [已归档计划](archived/2026-08-ipc-object-foundation.md)，实现现状见 `notes/impls/ipc.md`。
- 已完成：**统一内存事务核与公共 MemoryObject 已收口（2026-09）**——切片 1–6D 与切片 7 全部完成。Running/Building/bootstrap/object view 四条路径收敛为单一 `MemoryChangePlan`/`PreparedMemoryChange`（source/authority/output/image_end/view 五个字段维度），四套平行 plan/complete/commit 类型与 Tunnel 两份回滚矩阵已删除，proc.rs 净减约 700 行。对象 view 的权限真值从 `ObjectViewAuthorization` 流出（原 `ReadWrite` 硬编码已清），公共 MemoryObject 经 `MemoryObjectCreate/Query/Seal(0x55-0x57)`、`ObjectSignals::EXECUTABLE` 与 `MemoryMapRequest.source` 接入，AddressSpace 持 per-object view owner 使对象独立于 Handle 存活。实施途中修正一个真实前置缺口：含 W 的 object view 被部分 Unmap/降权时存活片段需要后继 permit，因此 Unmap/Protect 采用两段式（Validate 定几何并冻结对象来源 → 锁外取得 permit → 重入 Reserve）；Sealing 允许只延续原写范围的切分/收缩，仍拒绝只读范围升权，失败回滚不再回查易失的 live view owner。实施档案见 [`archived/todo-2026-09-memory-object-unification.md`](archived/todo-2026-09-memory-object-unification.md)，历史 A 批复核见 [`Review program 档案`](archived/todo-2026-09-review-program.md)；本轮后续修复由 [`设计审查后续 Review`](todo-2026-09-design-audit-followup-review.md) 单列复核，不重开旧 program。实现现状见 `notes/impls/{mm,memory-object,tunnel}.md`。
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
- 下一自然序：A–E Review program 已收口：`228b6a5` 完成启动 Failed 广播、nofd 验收及 Tunnel 精确失败/交错验证，并修复补证暴露的输出终止锁序缺陷；两位独立 reviewer 确认全部关闭，九份报告与统筹/系统审计计划归档。多页 Tunnel/Runnel v2（切片 8/9）实现与完整验收已完成，事后审查纳入统一架构 Review：有界几何 ABI、Endpoint 与 raw cleanup 安全边界、独立物理 cursor、共享访问平台契约、正进展通知及部分完成错误已贯通真实消费者；RNL1、单页接口和重复 Prepare 编排已删除。工程限额按独立依据重校：guard 12KiB 派生审计阈值，两平台每 hart 栈统一 256KiB，不为经验门槛拆帧。切片 10 已完成实现与完整验收：raw allocation adapter 与通用 tracker 已删除，boot-held 直接拥有几何；正式 funding 自检、全范围清零、切分双账本退款与 child 来源保活已贯通。切片 10 已提交为 `606b59d`，未来固定提交复核见 [`库存来源代码 Review`](todo-2026-09-frame-source-selftest-review.md)；数据面专题已归档，交付与证据见 [`数据面档案`](archived/todo-2026-09-memory-object-data-plane.md)。下一步先按实施前审视后的前置任务推进，FAL 业务暂停；原整体方向为：公共时间/绝对期限与 Mailbox 发送授权、Lifetime/Delivery/HandleQuery、WaitSet 前置 → libsrv/RPC/稳定授权后端 → 跨进程 provider、Record/服务发现、正式 Open、Watch 与 Move/Copy → BufferQueue 和设备/中断/DMA → 异构。时间专题是本次正式跨进程服务的必需前置；内核扩展用于消除补偿机制，业务与长工作仍在用户态。公共前置已进入施工，typed 运输/阶段错误、异步 RPC、libsrv 和正式 FAL 消费者仍未接通，内核 just check 无警告、六个公共逻辑包 59 项 host 测试及现有用户态构建/virt core/默认时限 stress 16/16 已通过；Native 请求继续已接真实 ProcessDrain：出生预付可复用完成存储、固定目标/预算、暂停/epoch 取消与未安装清理；Job 摘除前预付 Finalization 独立根避免 Control/Caller 消散丢终段传播，定点 reviewer 已复核所有权与释放唤醒 P1。新路径 core 通过，stress 300s 诊断完整 16/16/reset，QEMU 实测 162.657s，默认 stress 重校为 300s；150s 在 Tunnel close round7 截断及更早未分类超时保留，不用通过轮次替代逐次归因。WaitSet 单 actor/ticket 已共同接普通 Close、ProcessDrain/unpublished，Register/Rearm operation 责任已登记，旧 Seal/Drain 调用号/结果及用户编排已删；控制面 notify/finish/actor 同一安全点总预算 16，CLOSED 来源不接迟到持久安装并有界摘槽。srv_init 新增非空/self/cross/rearm 验收，actor stress 16/16/reset；旧 callback 跨轮 outcome P1 已复核关闭，CLOSED/Rearm seen 滞留 P1 已复核关闭；续发现退休跳过较早历史快照 P2，由普通通知/退休共用 select_snapshot 修复且复核关闭。启动自检新增旧轮回调、Closed seen、来源关闭后迟到安装、历史快照、安装操作门和 Rearm 前/后×Remove/Close 4/4，机制级顺序排列及20类准入退款/core通过，不声称实际多hart或独立槽池库存。本轮补齐正式Taken队列槽交回前后×Remove/Close4/4、未安装reply Abandoned/Done直接断言与mandatory独立责任、PendingClose/FinishDependency早晚park唤醒2/2、仅队列actor根及最后Weak失效、actor/Kernel finish/Object构造压力3/3和20类准入/三固定槽库存退款。独立test_target持256项WaitSet被kill后max_work=1 Drain4165批，stress重跑16/16/reset，128MiB sifive_u core/预期reset失败收割通过，七面clippy全部通过；机制自检不冒充实际Waiting/active-hart退出链或跨表并发库存。新生产路径启动组合已补真实Bound space/域ReadyBatch与wait.install/termination debt/ThreadDeparture：已安装Close kill、Native FINISH_DEBTS和UNPUBLISHED_DEBTS零工作park/wake、2/8捕获+恢复隔离finish恰6步、waiter二轮复用/旧epoch拒绝，Pool及全部债务许可退款；三测试finding复核关闭，128MiB core/release core通过，单hart主动推进不冒充真实多hart。最新全stress300s round7截断、GDB诊断223.446s和普通166.765s均走完24Tunnel矩阵后15/16已知flake；三GDB现场为wait投递/Map preflight/unmap发布，尚不归因原截断，均THROTTLE100不采默认50推测。下一序三类公平压力/消息回滚与真实多hart退出/完成坏输出补证，用户将15/16概率覆盖与Tunnel静默截断完善统一收口为确定性覆盖与墙钟敏感偶发现象，历史证据及重开条件见[验收时间敏感归档](archived/ref-2026-09-acceptance-timing-flake.md)，不作为当前开放缺陷。通知publish/Pending增及槽交回/Pending减已与queue同锁；新增三类同时pending保底进度/共享预算压力首组并核backlog全部Signaled与退款。消息所有权确定性组合已补：真实Bound/域准入共用task.selftest，独立sender/duplicate/迁移once身份badge与成功消费、队列/received Delivery保来源、Lifetime已装CLOSED通知；预留Busy/隐藏followers、真实Unmap payload页后partial header回滚并唤醒WSet、重试完整内容/业务及Delivery KOID保留/旧编号跨新能力仍Stale、full失败同时保once+move、closed-owner生产拒收并关最后transit。三项测试P2复核关闭，最新virt/128MiB/release core及七面clippy通过，Pool/准入/control/deferred退款；仅Ready前主动排列，不冒充实际多hart。本轮已完成上述剩余公共正确性门并归档公共对象前置；公共时间前置 时间前置随后完成并归档：公共时钟、绝对期限、运行期协作停止与真实期限消费者验证，提交 `c6e0a84`；最后六项P2复核关闭，core/128MiB/release/nofd/启动失败/七面clippy与163host通过。下一序运输/RPC/服务执行前置→FAL；完整stress历史墙钟敏感现场已完成首轮验收收口并归档；任务依赖以 FAL 总计划导航，公共对象/时间/执行各自计划安排闭合范围与验证门，设计按证据修正，不能把文档确认当成前置完成。最终全局架构 Review 仍等待数据面及主要消费者完成；系统关机编排与独立电源管理服务保持各自未来计划，不在本轮实施。**step 7（ThreadSpawn 前多线程 teardown barrier）已实施收口（2026-08-28）**：线程成员表（tid 寻址、离场即摘）取代单值记录，等待取消锁外游标化（零分配），归一收敛到 trap 汇编非 Resume 出口，KNOWN_ISSUES 写回 panic 面消解（deliver_output 复检即杀 + 分发出口终止检查）；ThreadSpawn 接入面清单入档计划篇。**首次 release 验证暴露并修复 trap 入口 x5 破坏**（SPP 检查在保存前用 t0，每次用户 trap 覆写用户 x5；修复经已保存的 t5 中转，寄存器纪律入档 execution-context.md，调查档案 [archived/review-2026-08-28-release-trap-entry-x5.md](archived/review-2026-08-28-release-trap-entry-x5.md)）；自此 release 验证线纳入阶段收尾必跑（`just virt-release`）。**step 8（capability-derived 调度域 eligibility 与 D64 开放）已实施收口（2026-08-28）**：域按需求满足签名等价类推导（`os/sched_domain` 纯逻辑 crate，host 可测）、boot 冻结、Start 提交点绑定进程，多域默认落最弱兼容域；reserve/commit/rollback 上收 `SchedClass` trait（F2 勾销）；D64 兼容谓词修正（FLEN 恰 64，Q 排除）；验证面新增 `test_fp` D64 负载（gc target：fsqrt/fmadd 位型、FPR/fcsr 跨 trap 往返、轮转复检）与 `virt-hetero`/`virt-nofd` 多域 DTB 变体（`tools/make-hetero-dts.py` + `ERHINO_DTB`）；virt/virt-release/hetero/nofd/sifive_u/host 全绿，方向公理入档 ideas/task.md「线程」。生命周期 step 10 文档终态已收口。**step 9（多核竞态验证矩阵）已实施收口（2026-08-28）**：`test_hammer` 双锤负载（HAMMER 执行器/TARGET 竞态靶，`libprocess::race` 线协议）+ init 竞态矩阵段 10 场景（kill vs kill/Exit/fault/Start/park/abandonment、并发 Create+枚举乱序窗口、seal vs 并发 Create、双 Drain ObjectBusy 仲裁、最后 control 消散派生兑底）；验证中修复内核 `dealloc_bounded` 完成路径 off-by-one（最后一跳用满预算时 work_done 超 max 违约，host 回归补齐）与 sifive_u BootPackage 装载窗口（尾部 32MB→64MB，零内存代价）；virt/virt-release/hetero/nofd/sifive_u 矩阵 10/10、host 全绿。**ThreadSpawn 三批与用户内存 8B 已收口（`bdc83ef` / `004cae5`）**——teardown barrier、成员表、active 位图、每线程 FP 状态与域绑定直接复用；Running spawn 使用独立 Spawning 状态，用户态以普通匿名 mapping 建立双 guard 栈并由 JoinHandle 结构化收束，线程级 result obligation 与进程 mandatory_ops 保持不同职责；竞态矩阵扩至 16/16，carryover IPC 压力四线与 `sifive_u` 连续十轮通过，实施计划已归档。BootPackage / launcher 基座已过机制层审查（[`archived/review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)，F1 payload 收编 owned backing、F3 Pid 拓宽 u64 已实施）；initfs 内部协议在需要正式服务编排时单独设计。

## 戒律

- 内核态路径保持短；出现「必须内核抢占」的需求 = 工作放错了地方，修方向不修模型。
- 公平性靠数据结构性质（FIFO 等），不靠记账字段——旧内核死因是记账字段无写入点。
- 用户可触发的 fault 一律杀进程绝不 panic 内核；syscall 未知号返回错误。
- 全局状态按三层纪律（hart 私有走 tp / 对象走锁 / 全局 OnceLock+Spinlock），禁 static mut。
- 用户内存访问：SUM 直访 + translate 前置校验，不软件遍历页表拷贝。
- 框架先行、实现从简：结构一次到位，实现按需求渐进替换（如调度域/类）。
- 共享 ABI 改动内核与用户态两侧同步，不留单边。
- 施工统一遵循 `AGENTS.md`「标准施工流程」；本文件只维护方向、位置、计划入口、完成证据和残留导航，不重复定义任务审计、设计、实施与验证流程。
- 文档即决策：方向性结论进 notes/，本文件只导航；收口记录完成证据、剩余责任与下一步自然序。提交需用户另行授权，未提交不妨碍审视与验证。
