# 五批 Review 统一报告：当前有效性与后续责任

状态：**九份独立报告的统一核定已完成。本篇是这组 Review 的唯一活跃报告和后续处置入口；原报告只作历史取证。** 本次没有修改生产代码，也没有启动服务、设备、平台内存或关机施工。

## 结论与基线

以 `refactor/system-redesign` / `992515aa8f9f83ff7aece8dd037d8a29a6fd354f` 及本轮开始时已有的计划整理为基线，核对五批九份报告中的 **42 个原编号**。生产代码目录 `os/`、`shared/`、`user/` 和 `tools/` 起点没有未提交改动。固定提交报告的历史结论与当前有效性分别记录，后续实现不能倒填为旧提交已经正确。

当前没有从这九份报告中确认需要立即修复的生产代码缺陷。三条原 P1 代码结论分别因可达路径不成立、要求了合同未承诺的终态、以及后续机制替换而退出当前修复队列；理由见 H01—H03。这不是全项目无缺陷宣告，也不是对其他提交和未覆盖窗口的验收。

仍影响后续工作的内容归为 **9 组设计闭合责任（D01—D09）和 7 组条件触发的证据/历史责任（E01—E07）**。前者是尚未设计完成的能力进入施工前需要回答的问题，后者是已有能力的精确验证边界。不能将它们合计成“当前系统有若干 P1 bug”，也不能为了关闭报告立即实现所有未来能力或补建故障测试设施。

本篇拥有跨报告去重、有效性判定、责任分工和处置/复核状态。专题计划继续拥有详细目标设计、实施过程与原验证证据；本篇链接使用这些事实，不建立第二套可独立维护的专题设计或延期账本。原作者/协调者名字是历史协作身份，后续 owner 按所属模块与计划维护者确定，不假定已退出的会话继续承担任务。

## 已退出当前修复队列的判断

### H01：基础对象链 F1 的停驻路径不成立

**原结论：** `SourceRegistration::registered` 在 `CancellingInstall` 后只转 `RemovePending`，没有提交 Remove 或重新调度，因此监督任务可永久停驻。

**本轮更正：** 该局部状态事实存在，但原路径遗漏了 Runtime Gate 顺序和停止时已经建立的 ready 资格，不能据此推出报告所称的永久停驻。不是本轮修复代码后关闭。

- 当前 [Runtime](../user/libraries/libexecution/src/runtime.rs) `advance_one`（867—890）在 `pending_gate` 存在时先推进 Gate，之后才弹任务；普通 `SuperviseTask::advance` 不会在自己的 Register 回执尚未处理时再次执行取消分支。
- 可以插入的 `shutdown_turn` 会调用 [WorkQueue::stop](../user/libraries/libexecution/src/work_queue.rs)（406—443）。它执行任务 `stop` 后调用 `schedule(id)`，保留下一次任务推进资格。
- Gate 成功 `registered` 后返回 false，只决定 `gate.refused`；[finish_advance](../user/libraries/libexecution/src/work_queue.rs) 的 Parked 分支没有撤销先前已经排入的 ready 项。任务下一次推进取得真实 token，提交 Remove；[ObservationSlot::poll](../user/libraries/libprocess/src/observation.rs)（219—231）在实际注销回执之后才交付结果。
- 固定树 `ac9d7d9` 中已经存在同一 `stop → schedule` 和 Gate 优先协议，以及 `registration_cancelled_before_install_receipt_is_retired_before_task_drop` 回归；因此这不是 `a1ca124` 之后才补上的保证。
- 本轮执行该既有 host 用例，**1 项通过，38 项过滤，exit 0**。完整日志：[source-cancel-host.log](../artifacts/review-consolidation/source-cancel-host.log)。用例命中“Register 已暂存、停止先于安装回执”的窗口，最终任务/来源清空且 Runtime/Assembly close 成功。

**证据边界：** 用例使用现有 host SourceOps，未声称真实 guest 的任意交错已穷尽。调用者绕过 Runtime 正常停止协议直接修改任务值、又不安排推进，不属于原报告证明的正式监督路径；本轮没有确认此类路径的代码反例。今后改变 Gate、stop/schedule 或 ready 收尾时，以该独立寿命合同复核，不新增一条当前 P1。

### H02：基础对象链 F2 将故障终局误作全 hart 完成屏障

**原结论：** IPI 失败后无 timer 的 hart 可保持 WFI，不能消费 `runtime_stop` 或设置 PARKED，因而系统停止责任未完成。

**本轮更正：** WFI 可继续保持的代码事实成立；“必须全部进入 Rust park、继续退休/退款”的要求不属于已确认的故障合同。该结论不作为当前代码缺陷，不以补重试 actor、轮询 timer 或强制回收来修复。

- [对象链修复档案](archived/review-2026-10-07-object-chain.md#r4-b1不可恢复门铃终态)已经明确选择不可恢复 `runtime_stop`：不保证失去门铃的无 timer hart 被再次唤醒；任意后续唤醒先检查停止事实，再 park；资源留到重启，不推进正常回收，不保证设备/DMA 撤销。
- [runtime_stop.rs](../os/kernel/src/runtime_stop.rs) 的 REQUESTED/STOPPED 是全局不可逆事实，PARKED 只记录实际进入 Rust park 的 hart；[sched::idle](../os/kernel/src/sched.rs)在 WFI 前后检查停止。[当前实现说明](../notes/impls/internals.md#ipi-门铃与-remote-call)明确“不把 PARKED 当全 admitted 完成 barrier，不等它参与退款”。
- `git diff ac9d7d9 HEAD -- os/kernel/src/runtime_stop.rs os/kernel/src/sched.rs` 为空；本轮没有用后续代码替固定树补保证。
- 既有真实 IPI 注入验证的是“后续真实广播唤醒后先停驻”，不证明无未来门铃也全部 PARKED；[构建手册](BUILD-AND-TEST.md#验收入口)与该边界一致。

**未来影响：** 设备隔离、共享供应释放或完整全 hart 屏障不能消费这个 fail-stop 当作完成证明。若未来明确要求门铃不可恢复时仍完成设备收束或全体确认，应先在设备/平台/系统终局设计中确定新合同；归 D04/D08，不能把新需求反写成当前 F2 bug。

### H03：时间/运输 F-01 已被后续 Deadline 接线替换

**原结论：** 固定 `a2aabed` 的 Runnel `Transport::wait`、批量读写没有绝对 Deadline，guest 用零毫秒等待进入 Infinite，init/pm 可能无法按期限退出。

**当前判定：** 后续提交 `dda8a5b` 已引入 Deadline 接线及 `absolute_wait_timeout_preserves_progress_and_role` host 回归；旧提交反例保留为历史。当前 API 提供携带 Deadline 的 `write_all_until` / `read_exact_or_eof_until`，Channel/Transport 持同一绝对期限，guest 最终调用 `wait_until`。详见 [librunnel](../user/libraries/librunnel/src/lib.rs)。当前 PM/初始化的正式流推进改为 Runtime 任务与非阻塞分步读写，有限业务期限由任务持有；文件/Copy 消费者也由其各自任务推进。

保留无限等待便利函数本身不构成缺陷；只有承诺有限期限的消费者错误调用它才构成反例。当前原报告点名的必要调用路径已被替换，不能仅凭旧函数名字仍存在重开该 P1。精确映射失败清理仍归 E03，不能以 H03 关闭代替验证。

### H04：来源 Busy、普通叶能力 Close 与非零 Copy 取消的旧缺口已缩减

以下不是新增的“已修 bug”，而是对原证据要求是否仍适用的核定：

- **合法 WaitSet Remove 的真实 Busy/Quota 注入不适用。** [WaitSet::remove](../os/kernel/src/task/wait_set.rs)（250—260）有锁摘 token、锁外 Drop/kick，不分配且没有 Busy/Quota 分支；未知 token 为 ObjectNotFound，Runtime 按已退休处理。通用 SourceOps 的失败保 owner/回执/费用已有 host 边界；不能要求当前内核制造不存在的资源拒绝。对应 FAL F-01、执行 E-02 的这一部分退出；真实映射撤销失败与异步回执组合仍归 E03。
- **独占、合法、非映射 Handle/Delivery Close 的资源性失败不适用。** [FAL 主计划再核](todo-2026-10-07-fal-service-redesign.md#提交前机制收敛)第 1674—1678 行、C08/C10 已区分叶能力 Close 与映射 Unmap。旧 close-retry 暂存删除不能被旧报告恢复为施工要求；错误 raw handle 与进程已经退出是另一失败合同。
- **Copy 的非零目标 accepted 后取消有历史实现及运行记录。** 当时的 `srv_init/src/filesystem/copy.rs` 消费者经真实 ReadAt 确认非零前缀，再取消并核最终前缀、并发隔离和退款；该消费者在组态切面 B 已删，旧验收锚点由[验收清理](todo-2026-10-29-acceptance-groups.md)删除，不再作为当前可运行覆盖。[主计划第二轮验证与第三轮最终收口](archived/ref-2026-10-27-oal-fs-delivery-history.md#第三轮最终收口)记录最终 C10/C11 聚合覆盖该门，原证据路径为 `artifacts/convergence3/acceptance-summary.json`。该原始产物当前不在工作区，运行结果按已交付工作记录取证，未重新逐 guest 验证。FAL F-05 中“未接通/未运行”的表述不适用于其主固定目标 `a1ca124`；不因此宣称所有取消交错已覆盖。坏帧/Delivery 特定失败仍归 E04。
- **普通 RPC queued/sent unknown、迟到旧 txid 和 sealed live waiter 不能笼统记成“没有组合证据”。** 当前消费者/主计划已有实际场景；旧执行 E-01、运输 E-03 仅保留尚未定位到证据的精确 Wake 拒绝、异常输出及平台边界，见 E04/E07。后续不能为了重验已覆盖部分扩大任务。

## 仍影响未来施工的设计闭合责任

D01—D09 下方保留 2026-10-12 核定时的问题、触发与关闭门；当前状态同时读取本节追加的范围复核。原 P1 表示依赖未决契约的施工前置，**不表示尚未开始的设计违反了既有生产能力合同**。原 RM-07 的 P2 验证门也保留其证据性质，不因合并进入 D09 就升格为生产 P1。

### 设备首批施工准入后的范围复核

本轮设备设计以 `e6d11a1` 为接手基线收敛，当前目标见[设备施工准入](archived/ref-2026-10-27-device-delivery-history.md#施工准入核对)。固定基线独立 Review 的原始问题及针对性复核见[设备审查档案](archived/review-2026-10-23-device-system-design.md)；该档案的 D01—D04 是另一组编号，不能与本报告同号条目混淆。以下只更新设计责任，不表示代码交付：

| 本报告责任 | 当前范围处置与依据 | 剩余触发 |
|---|---|---|
| D01 / D02 | [首批装配图](todo-2026-10-07-service-assembly.md#首批装配图与就绪合同)明确管理者/驱动/消费者、Job/Pool/授权、私有就绪、Unknown 收尾、显式接替和功能发布；设备首批设计闭合 | 通用服务恢复、持久化停止和配置装配仍由原案设计 |
| D03 | DeviceId/BindingRef、认证来源、Claim/Provision/激活和迟到收束已定；设备独立审查关于跳轮/重试/持权的三项发现已修订复核 | 多管理域和透明恢复不在首批，出现需求再触发 |
| D04 | MMIO 发布许可/强制撤销、PLIC/idle、DMA 暴露/孤立保留、受信 PCI/xHCI 及控制根已形成可实施合同 | 实施必须取得实际 IRQ/DMA/故障证据；IOMMU/非一致 DMA 继续 Unsupported |
| D05 | [保留区首批边界](todo-2026-09-platform-reserved-memory-lifecycle.md#首批设备路线的边界)确认所选路线只用普通固定长度 MemoryObject，不消费动态 region | 本条原能力尚未触发，不能登记为已实现或完整设计关闭 |
| D06 | 普通 RAM DMA 的 use/写许可/实际退休由[内存接点](todo-2026-10-01-memory-redesign.md#设备-dma-接入合同)闭合；reusable 与可撤回 loan 未纳入首批 | 原 reusable 设计门保留，真实消费者出现前闭合 |
| D07 / D08 | [设备系统终局](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)给出准入封闭、完整责任集合、父子停止、Cold reset 错误与 boot 根观察 | 通用持久化服务 drain/flush 等原范围继续由关机案承接 |
| D09 | [实际迁移面](archived/ref-2026-10-27-device-delivery-history.md#实际迁移面与删除门)及[验收判据](todo-2026-09-device-system-design.md#验收门与可观察判据)已对应所选首批能力 | 未来扩大承诺按各案补门；当前门仍待代码实现及运行验证 |

因此不再以旧 D0/D1 或“维护基座未闭合”阻止首批设备施工，也不关闭本报告全部未来责任。E01—E07 的既有能力精确证据缺口未因本轮设计获得新的运行证据。

### D01：正式服务集合、就绪与授权/付款/供应关系

**2026-10-28 选定范围复核：** 下方原发现保持当时事实。[静态常驻基座设计](todo-2026-10-07-service-assembly.md#静态常驻服务基座设计2026-10-28)已闭合 init、两设备管理者及单个 fs/registry 宿主的静态集合、独立付款、私有认证、真实来源就绪与消费者授权交付。init 为真实根接收者，当前交付定位为应用接入前的铺路基座；pm/A-B 仅作验收配置，不凭名称扩成正式服务。D01 在该范围设计关闭，代码未实施；真实应用目录/授权政策仍按新消费者接入触发。

**来源：** 服务装配 SA-01、SA-02。**唯一领域 owner：** [服务装配计划](todo-2026-10-07-service-assembly.md)「设计门」1—2。

该计划仍只登记范围，正式长期服务/真实消费者、必需/可选依赖和业务就绪谓词未列成可实现契约。当前 RequiredLaunchSet 证明映像存在和 spawn，fs 有自己的 provider-ready；它们没有证明所有服务的健康。进程/Job control、kernel Pool、用户执行付款、发布 authority 和服务供应是不同关系，不能由“init 创建了一切”替代。

**可推导影响：** 进程启动成功后过早交付入口，或服务死亡后监督者只剩 ProcessControl、缺少重建/供应 authority。**触发：** 选定首批正式长期服务并准备替换验收装配。**关闭门：** 从选定能力推导服务/消费者集合，逐事实确定就绪、交付、付款与接管 owner，证明权力可达且依赖无循环。不预设新 supervisor 进程、manifest 或固定服务名单。

### D02：服务级恢复、未知发布结果与停止交接

**2026-10-28 选定范围复核：** [常驻基座的实例/发布合同](todo-2026-10-07-service-assembly.md#静态常驻服务基座设计2026-10-28)明确必需实例失败终局、不自动重启，直接 bootstrap 根无需自注册；已有 Register 未知不重放、旧 Namespace 不自动重绑。fs 停止意图先提交，认证 Stopped 与真实 Completed 独立汇合，来源退出后才移交唯一 Collector。该范围设计关闭，未取得新运行证据；可恢复服务、外部管理客户端和透明重连保持明确条件后续。

**来源：** SA-03、SA-04；关机 SO-03 的服务交界只引用本条。**owner：** 服务装配「装配接点」「设计门」3—4。

单步 Register 和 Active/Withdrawn 已有正式合同。装配尚未定义首次发布未知、替换回复丢失、旧 endpoint 退休、重新发布、消费者重连、服务拒绝/死亡和停止超时的唯一政策。

**可推导影响：** 从服务名存在误判健康，自动重放未知变更，复活旧入口，或进程 DONE 后误认业务已经 drain。**触发：** 正式服务恢复/重启/停止能力接入。**关闭门：** supervisor/provider 的责任状态、有限期限/升级、原 control/authority 持有、消费者显式重连和服务停止报告可实施；系统范围聚合归 D07/D08，目录 wire 不重设。

### D03：设备身份、关系和跨域事务

**来源：** 设备 D-01、D-03。**owner：** [设备计划](todo-2026-09-device-system-design.md) D1/D3。

当前 D0/D1 仍为组织层细化，实例、绑定代次、管理域、分配范围、租约/session 的身份与引用关系未定；跨管理域预留只保留取得顺序/回滚候选，没有提交、未知结果、重复/迟到和取消的端到端合同。

**可推导影响：** 旧 Ready/取消回执作用于新绑定，半准备资源无人持有，或跨域互相等待。**触发：** 选定设备能力进入领域协议/资源接口设计。**关闭门：** 每个真实事实有唯一 owner，范围与代次足以拒绝旧引用；选定跨域责任转换和线性化点并穿行部分失败/失联路径。具体 identity 类型、事务算法由设计选择，不把原报告建议的字段或状态名字当作已确认方案。

### D04：设备 authority、隔离成本、硬件安全与 IRQ

**来源：** D-02、D-04、D-05；RM-06 的硬件部分归本条。**owner：** 设备 D2/D4/D5；固定平台/供应合同由对应领域提供。

驱动/管理者死亡后的控制与恢复权、IRQ mask/ack/rearm/代次/风暴和停止/隔离证明尚未闭合。设备计划第 19 行仍把维护执行承载说成“尚待闭合”，该前置已由维护计划/对象链完成；需要设计的是设备消费者与可信事实，不是再造执行器。

**可推导影响：** 只有进程控制没有隔离 authority，或把 Process DONE、VM fence、维护 Work Done 当成 DMA/IRQ 静止，提前复用 backing/供应。**触发：** 首批 MMIO/IRQ/DMA/供应能力选择。**关闭门：** 按实际平台能力取得固定证据，明确 manager/driver 消散后的执行根、付款和保守隔离；不支持能力明确拒绝，设备停止证明不能由通用 IPI/runtime_stop 代替。无需提前支持所有总线/IOMMU。

### D05：平台 region 输入、动态放置与一次布局发布

**来源：** 保留内存 RM-01—RM-04。**owner：** [平台保留内存计划](todo-2026-09-platform-reserved-memory-lifecycle.md)「目标模型」「决策门」；DT parser/planner/启动供给是接点。

节点与多 reg extent、phandle/names/compatible 的身份和拒绝边界、动态排序/候选选择及存储上界、整体布局提交点、region authority 与 direct-map/FramePool/system/root Pool 的单一物理真值尚未选定。当前 parser 已实现静态 reg/no-map，动态/reusable 明确 fail closed；不是当前代码遗漏了已承诺能力。

**可推导影响：** 部分布局可见，物理页同时属于设备 region 与普通供给，或 capability 粒度越过设备引用范围。**触发：** 接入 dynamic reservation/region capability 或正式设备 region 引用。**关闭门：** 从支持的 DT 输入得到确定布局，所有 fallible 准备在发布前闭合；发布集合、重入/失败边界、owner 与 charge 投影可核对。排序/容量来自资源模型，不照抄旧限额，也不强求原报告示例策略。

### D06：reusable loan/reclaim 与 DMA 内存生命周期

**来源：** RM-05、RM-06 的内存部分、设备 D-06。**owner：** 平台保留内存计划；硬件隔离完成事实消费 D04。

可撤回 loan 的准入、backing 可丢弃/迁移条件、收回提交、CPU View/PTE 与 DMA pin/缓存/翻译屏障、设备移除/超时/owner 消散后的 quarantine 尚未定。普通 MemoryPool grant 不可撤销，不能作为 loan。

**可推导影响：** reclaim 超时就把 extent 重新标可用，设备仍按旧物理地址访问；或 loan 与普通 charge 混用而无法停止新 use。**触发：** 正式承诺 reusable 或可撤回 DMA 内存。**关闭门：** 分层稳定 owner 和完成谓词闭合，有/无 IOMMU 分别给出支持边界；完整 extent 安全归还前不重新借出，无证明时保活隔离而非伪退款。静态专用区能力可按独立范围先闭合，不能因此宣称 reusable 完成。

### D07：系统意图、准入封闭、范围与跨域收束

**2026-10-28 选定范围复核：** [静态常驻基座停止](todo-2026-09-system-shutdown-orchestration.md#静态常驻基座停止2026-10-28)确定 init 内部唯一意图、同一步本地 gate 与 services JobSeal、原记录覆盖在途装配、消费者→文件/目录→设备的供应收束及冻结总期限。外部请求协议和持久化 flush 不在承诺中；该非持久部署范围设计关闭，实施与组合验证由服务装配唯一记录承接。

**来源：** 关机 SO-01—SO-03。**owner：** [关机计划](todo-2026-09-system-shutdown-orchestration.md)「必须闭合的语义」15—19。

计划目前是未来设计清单，没有关机意图的授权/提交点、与服务/委托/外部工作的准入交接、范围稳定性证明、业务/文件/设备/供应/进程/维护的依赖与聚合终态。init 的固定 workload 只证明自己的责任集合。

**可推导影响：** 快照之后出现逃逸新责任，超时后把进程/Job 终态当系统完成，或根调用者死亡遗失已提交业务收束。**触发：** 首个需要长期运行/持久化/设备停用协议的正式服务前，或承诺通用优雅关机前。**关闭门：** 证明准入封闭与范围完整，每个领域事实有唯一 owner/等待与升级责任，强制终止后的资源边界可核对。是否使用 epoch、广播、拓扑遍历或其他机制仍需论证，不把原报告示例实现冻结为要求。

### D08：系统终局政策、SystemReset 与根监督者失效

**2026-10-28 选定范围复核：** 常驻基座复用已交付的设备/boot 终局，将唯一终局持有提升为 root 系统责任，文件必需宿主故障进入同一出口；正常 Shutdown 与故障 ColdReboot、不足硬件证据的 DMA 保留、reset 拒绝与 init DONE 边界不扩大。具体见[关机设计](todo-2026-09-system-shutdown-orchestration.md#静态常驻基座停止2026-10-28)。选定范围设计关闭，新增接线尚未实施，不能作为新的故障运行证据。

**来源：** SO-04、SO-05；基础 F2 的未来边界归本条/D04。**owner：** 关机计划「必须闭合的语义」20—22。

shutdown/reboot/system failure 的必经/可绕过步骤、隔离资源/已提交责任的剩余 owner、最终 reset capability 持有人与许可、返回错误/不支持/竞争的监督终局，以及根监督者意外退出合同尚未设计。SBI reset 是最终叶动作，不判断业务收束。

**可推导影响：** 为紧急复位跳过必要硬件隔离，或 reset 返回后只保留一个 idle 进程却没有明确系统状态。**触发：** D07 的正式系统终局承诺。**关闭门：** 分政策定义必须消费的事实与允许保守保留的资源，区分根存活提交和意外退出；若需要恢复/重建根先独立证明能力，不临时补内核 reaper。未来电源管理服务不预设为执行主体。

### D09：跨领域前置、选定能力与可判定交付门

**2026-10-28 选定范围复核：** [服务装配的迁移/验证门](todo-2026-10-07-service-assembly.md#静态常驻服务基座设计2026-10-28)已列宿主 profile、私有就绪、持续观察、付款、停止聚合、旧路径删除和真实验收消费者；无需新内核机制、测试服务或通用执行框架。该范围设计门关闭，施工和验收门仍开放；D05/D06 等未触发能力及所有 E 类证据边界不变。

**来源：** SA-05、D-07、D-08、RM-07、SO-06。**owner：** 各专题拥有自身施工门，本篇拥有交界去重。

这些原编号大量重复“接口/责任转换/部署/验证门未闭合”，合并为一个共同检查切面，但不把全部领域合成一个巨型先决条件。服务、设备、平台内存和关机各自计划尚未形成选定能力的完整合同；保留内存 fixture 只有数量/场景要求，缺乏实际可观察 oracle。

**可推导影响：** 从库名、模块图或某次验收反推能力就绪，代码中临时发明 topology/authority/停止 owner，或者通过近似 fixture 宣称所有 DMA 能力完成。**触发：** 相应设计转施工/交付。**关闭门：** 对选定范围列出接口与责任转换、必要前置及唯一 owner、明确 Unsupported/延期边界、旧路径删除和最小观察证据；实际 fixture 从选定合同推导，不要求预先指定某个 hash、固定状态名或人为测试数量。

## 已有能力保留的精确证据与历史责任

以下 E 条目没有本轮确认的代码反例。原 P2 的缺证属性保留，采用条件触发而非立即补测：只有正式隔离/故障接缝出现、相关源码合同变化或真实故障暴露时推进。已经覆盖的正常/故障组合不重跑；不能以“报告未列出测试”证明不存在验证，也不能用缺少测试反推代码有 bug。

### E01：Notification N6 的 mint/安装/接受后 Close 与逐壳费用

**归并：** N6-E01—E03、FAL F-03。**owner：** [Notification N6 再核](todo-2026-10-10-notification-observer-roles.md#n6-注入前置再核2026-10-11)。

源码有 Authorization/BodyLease、MetadataCharge/Arc、cap reserve→写回→commit 的稳定回滚；Observer 只强持 core，不增加 Owner holders。正常角色、Startup/Owner 退出、CLOSED 和 Watch 已有真实证据。仍缺 mint-specific quota/allocator/KOID、输出 fault/安装 Frozen、接受后 Close 的精确窗口，以及 observer 壳/core/handle/WaitSet/维护引用各自实际退款观测。

**未来影响：** mint/付款/install 修改时正常角色回归不能捕获错 payer、半安装或提前退款。**触发/完成：** 既有隔离目标与正式工具具备可控失败点和付款观测后，分阶段确认未公开资源释放、源 pending/closed 不被复活/破坏、物理最后 owner 后退款。不得耗尽 init、全局 KOID 或新增测试专用 ABI 来填门。

### E02：异步 Bind/Boundary 的母撤销与迟到能力

**归并：** FAL F-02；设备/服务自己的 authority 设计不并入此代码验证条目。**owner：** [FAL 主计划](todo-2026-10-07-fal-service-redesign.md) G05-19/G05-21 与第 1677 行再核。

稳定范围/ceiling、活动位、同名替代和 queued-unknown 已有 host/guest 证据。普通母 sender Close 不等于对已接受请求 revoke；已准入 use 仍有效。未定位的是 provider 生命周期受控 revoke 在下游已发送后、迟到 Derive 返回前的组合。

**触发/完成：** provider 停止/恢复提供正式可控暂停点，或该路径真实故障出现时，核原 snapshot/pin/Charge、Revoked 结果、原 txid 临时能力收束及替代 Binding 不被删。没有公开 revoke 方法时不为测试增加该方法。

### E03：Runnel/Copy 实际映射失败与来源退休组合

**归并：** 运输 E-02、FAL F-01 的映射部分。**owner：** librunnel 映射 owner、VM 故障接点及 FAL 主计划 F 条目；Outbox 自身的异常输出/来源责任归 E04，不为它附加映射 owner。

当前叶 capability Close 不再是可恢复资源失败，MappedEndpoint 的 `unmap` 仍可能失败并返原映射 owner。host 已有失败 owner/费用和通用来源回执证据，正常 Create/Attach/流/Copy/provider 退出已接通。真实已消费 Attach 后输出/协议错误、实际映射撤销故障与数据来源注销/原费用持续持有的精确组合没有稳定现场证据。

**触发/完成：** 正式 VM/mapping 失败入口出现或实际错误时，核真实来源先退休，保同一映射/费用且不连续 Runnable 自旋，安全撤销后退款。只处理当时可达的映射责任；不重建已删除的普通 Close 重试槽。

### E04：RPC/Outbox/下游坏帧、Wake 拒绝及异常输出收束

**归并：** 执行 E-01/E-02 的剩余窗口、FAL F-05 的坏帧/Delivery 部分、运输 E-03 的 RPC 交界。

**owner：** librpc Dispatcher/Outbox；领域晚到/Unknown/accepted 事实由 FAL 主计划 G05-22/F 负责。当前 Dispatcher 有 sent/unknown、旧 txid 丢弃、完成队列、`wake_pending` 和无 waiter 退休；Outbox 的 Sent/Abandoned 不回滚业务，真正来源退出后才完成。正式 queued/unknown、sealed waiter、协议错型携能力及 Copy 旧 txid 已有消费者证据，不能重新登记成整条路径没有验证。

**剩余边界：** command 缓冲容量导致 Wake 暂不可投递、目标已经退休的 Wake 回执、Bind 特定带能力迟到坏帧、Delivery/输出故障与 Outbox 停止/来源退出/真实费用的组合尚无逐窗口 guest 证据。法定 WaitSet Remove 不存在的 Busy、普通 reply/Delivery Close 资源性失败不属于该门。

**触发/完成：** 正式 transport/Delivery/Gate 故障可受控，或相关真实故障/合同变化出现时，使用原 Dispatcher/Outbox/消费者，核完成仍可领取或由原 owner 退休、业务 Unknown 不重放、替代者不受影响、来源/付款最后退出。无需为所有旧提交补一套旧运行体。

### E05：typed Packet 的真实失败返还

**归并：** 运输 E-01。**owner：** libkernel Packet/SendOnce 与正式消息消费者。

源码显示 `try_send` 失败返回原 Packet，`try_reply` 另返回 SendOnce；现有时间/满箱负向部分仍用 raw syscall，typed 成功/角色运输不能替 typed wrapper 失败的完整 owner 返还证据。

**触发/完成：** 修改 Packet/reply-once/安装输出责任，或既有正式消费者可控满箱/过期/关闭时，核 payload、transit capability、SendOnce 原 owner 完整且可继续使用/合法关闭，提交后不伪造撤回。无代码反例时不把它升级成消息运输 P1。

### E06：W05-04 的历史未知 Watch 准入错误

**归并：** FAL F-04。**owner：** [历史问题重新核定](todo-2026-10-07-fal-service-redesign.md#历史问题重新核定2026-10-11)及其唯一 W05-04。

主计划保留的原日志摘录只证明 secondary Watch 饱和循环出现非 Quota 分支，没有 Err/count/clock。当前每次准入各有独立有限 deadline，删除了最多 24 次共用一个 5s deadline 的候选结构；原错误是否由该结构导致无法恢复。Watch/RPC/mint/收费准入仍存在，没有足够删除证明关闭全部未知原因。主计划记录的后续 40 份 release/5 份最终 guest 成功只证明对应运行，不能排除低概率错误；这些历史 guest 本轮没有原文件可供重新核验。

**触发/完成：** 相同 panic/非 Quota 反例出现时保存 Err、count、last_ns/deadline 与 Lifetime/费用顺序，定位当前机制并用反例复核；或未来整体替换准入责任链时提供必要条件删除和保留行为消费者证明。没有复现触发时不靠重复跑矩阵收口，不恢复旧同步 Copy。旧 midflight Copy 已按整体机制替换关闭，不是本条当前 bug 或根因。

### E07：时间消费者与真实平台边界的条件证据

**归并：** 运输 E-03 的平台部分。**owner：** 公共时间/平台合同及其实际期限消费者。

ClockGeometry/Deadline/ClockState 的 host/selftest、At(0)、绝对 Sleep、同期限重试和已提交 Send 有证据；当前 Runnel 有期限接线与累计进度 host 回归，RPC 有业务取消/未知组合。真实 Endpoint 的流等待到期并保留累计进度没有定位到单独 guest 场景；真实弱序多 hart 的所有高水位/epoch 边界、特定平台计时可靠性和新期限消费者交界也未声明全覆盖。

**触发/完成：** 有限期限流消费者的等待/超时合同变化、新硬件/clock backend、epoch 边界或跨 hart 时间合同变化，或真实期限/clock 故障时，以实际消费者确认超时累计进度和继续使用边界，以独立平台事实证明 monotonic/failure/期限语义。QEMU 测量、GDB 暂停和普通成功不能作为 WCET/精确 CPU 份额证据；当前结构性有限机会保证不因本条未补而撤销。

## 42 个原编号的处置映射

下表完整覆盖原报告编号；原始文字、固定提交和当时证据只保留在只读档案，不复制为另一份活跃 findings。表中的“更正”保留原判断而退出队列，“历史已替换”不否定旧提交反例，“拆分”表示同一原编号有已消除与仍条件有效的部分。

| 原报告 / 编号 | 当前处置 | 唯一入口 |
|---|---|---|
| 基础对象链 F1 | 更正：遗漏 Gate 优先及 stop→schedule，原停驻路径未成立 | H01 |
| 基础对象链 F2 | 更正：无 timer WFI 是已选 fail-stop 允许的保守终局，不承诺全体 PARKED/退款 | H02；未来增强 D04/D08 |
| FAL F-01 | 拆分：合法 Remove/叶 Close 的资源拒绝假设不适用；真实映射/回执窗口保留 | H04、E03 |
| FAL F-02 | 条件有效：准入后显式 revoke/迟到 Derive，不把普通母 Close 当 revoke | E02 |
| FAL F-03 | 重复：归 Notification N6 | E01 |
| FAL F-04 | 历史未知仍有当前准入基础，候选共享期限已删，根因未证 | E06 |
| FAL F-05 | 拆分：非零 accepted 取消/一般 sealed waiter 已补；坏帧/Delivery 精确窗口保留 | H04、E04 |
| 时间/运输 F-01 | 历史反例有效，当前必要 API/消费者路径已替换 | H03 |
| 时间/运输 E-01 | 条件有效：typed Packet guest 失败返还 | E05 |
| 时间/运输 E-02 | 重定向到当前 MappedEndpoint/Attach/来源合同，旧清理布局不恢复 | E03 |
| 时间/运输 E-03 | 拆分：Runnel/一般 RPC 组合已有后续证据；精确异常与平台边界保留 | H03/H04、E04/E07 |
| 执行/库 E-01 | 拆分：取消/unknown/旧 txid 已有后续消费者；Wake 精确拒绝保留 | H04、E04 |
| 执行/库 E-02 | 拆分：普通 Close/Remove 资源失败不适用；真实异常退出/付款交界保留 | H04、E04 |
| 服务 SA-01 | 设计待闭合：选定服务/依赖/就绪 | D01 |
| 服务 SA-02 | 设计待闭合：authority/Pool/执行付款/供应 | D01 |
| 服务 SA-03 | 设计待闭合：未知发布/重发布/重连 | D02 |
| 服务 SA-04 | 设计待闭合：服务恢复/停止；系统聚合去重 | D02，系统部分 D07/D08 |
| 服务 SA-05 | 共同施工门，非独立生产 bug | D09 |
| 设备 D-01 | 设计待闭合：身份与唯一关系 | D03 |
| 设备 D-02 | 设计待闭合：可达 authority/隔离付款/死亡接管 | D04 |
| 设备 D-03 | 设计待闭合：跨域事务 | D03 |
| 设备 D-04 | 设计与平台证据待闭合 | D04 |
| 设备 D-05 | 设计待闭合：IRQ 硬件/订阅交界 | D04 |
| 设备 D-06 | 重复：region/reusable/DMA 内存责任由平台计划拥有 | D06，输入布局 D05 |
| 设备 D-07 | 共同跨领域接点 | D09 |
| 设备 D-08 | 共同施工/验证门 | D09 |
| Notification N6-E01 | 精确 mint 失败/费用证据条件有效 | E01 |
| Notification N6-E02 | 安装/输出 fault/Frozen 证据条件有效 | E01 |
| Notification N6-E03 | 接受后 Close/逐壳物理退款证据条件有效 | E01 |
| 保留内存 RM-01 | 设计待闭合：动态放置/上界 | D05 |
| 保留内存 RM-02 | 设计待闭合：DT 输入/region 引用粒度 | D05 |
| 保留内存 RM-03 | 设计待闭合：整体布局提交/发布 | D05 |
| 保留内存 RM-04 | 设计待闭合：物理 owner/authority 投影 | D05 |
| 保留内存 RM-05 | 设计待闭合：reusable 状态/收回 | D06 |
| 保留内存 RM-06 | 分工去重：硬件证明归设备，region 收回消费证明 | D04/D06 |
| 保留内存 RM-07 | P2 设计/验证门，按选定能力推导 oracle | D09 |
| 关机 SO-01 | 设计待闭合：意图/准入/范围稳定性 | D07 |
| 关机 SO-02 | 设计待闭合：领域收束依赖/聚合 owner | D07 |
| 关机 SO-03 | 分工去重：服务结果由装配定义，系统期限/升级由关机聚合 | D02/D07 |
| 关机 SO-04 | 设计待闭合：三类终局政策/绕过证明 | D08 |
| 关机 SO-05 | 设计待闭合：reset 许可/失败/根退出 | D08 |
| 关机 SO-06 | 共同跨专题前置/触发/完成门 | D09 |

## 自然接续顺序与完成条件

本轮只整理报告，不授权新的代码施工或选择新的系统架构。

1. **按当前范围接续。** 设备已交付范围从设备主计划接手，不从旧 D0/D1 恢复施工。2026-10-28 的静态常驻基座按服务装配记录接续实施，不等待所有条件证据补齐。平台 region 的 D05 按实际消费者触发，需要 reusable 时才推进 D06 的具体承诺。
2. **闭合真实交界。** 设备硬件停止/供应事实和服务停止入口形成后，由关机 D07/D08 自顶向下聚合。关机的触发发生在正式长期服务需要收束之前，不要求先实现全部设备、持久文件系统或独立电源管理服务。
3. **对选定范围过 D09。** 不把全部未来能力当一个串行大门。必要身份/authority/提交/生命周期/硬件事实未决时阻塞依赖它的施工；不相关的扩展保留明确 Unsupported/条件后续。
4. **按 E01—E07 的实际触发补证。** 复用正式设施和消费者，命中对应错误后再选择最小验证。当前没有触发时保持证据限制，不为归档报告制造测试专用服务、替代执行器或宽松断言。

D 条目关闭需要所属计划记录已确认合同和相应设计走查，并在本篇追加原编号对应的处置/复核依据；能力未实施时只标“设计闭合”，不得标系统交付。E 条目关闭需要精确证据或必要机制删除证明；一般验收成功/有限次未复现不能替代。若后续证据推翻本轮 H 判定，保留本轮推理并追加更正，不抹去历史。所有有效责任闭合或有不破坏当时承诺的唯一条件后续后，本总报告才可归档。

## 原始范围与档案

| 批次 / 原报告 | 固定对象或设计对象 | 本轮作用 |
|---|---|---|
| [基础对象链](archived/review-2026-10-12-substrate-object-chain.md) | `ac9d7d9`，次级 `606b59d` | 保留 F1/F2 原判；当前更正 H01/H02 |
| [FAL 服务交付](archived/review-2026-10-12-fal-service-delivery.md) | `a1ca124` 与验收 `a06e455` | 已有能力与精确缺证；按实际子路径缩减 |
| [时间与运输](archived/review-2026-10-12-transport-time.md) | `c6e0a84`、`3060dd8`、`a2aabed` | 区分旧反例与当前替换、转接证据 |
| [执行与库](archived/review-2026-10-12-execution-library.md) | `a3891b00 → 5de2780f → e0b5c459 → 4e18e5e`，`96ee03b` | 不恢复旧 Runtime/库布局，只保留当前可达证据责任 |
| [服务装配](archived/review-2026-10-12-service-assembly.md) | 当前装配设计 | D01/D02/D09 |
| [设备体系](archived/review-2026-10-12-device-system.md) | 当前 D0/D1、D2—D6 门 | D03/D04/D06/D09 |
| [Notification N6](archived/review-2026-10-12-notification-n6.md) | 当前 mint/安装/付款及 N6 前置 | E01 |
| [平台保留内存](archived/review-2026-10-12-platform-reserved-memory.md) | 当前 dynamic/region/reusable 设计 | D05/D06/D09，与 D04 分工 |
| [关机编排](archived/review-2026-10-12-shutdown-orchestration.md) | 当前通用关机设计清单 | D07/D08/D09 |

[五批执行记录](archived/todo-2026-10-12-review-convergence.md)只说明当时如何分批和形成原报告；其中“第一批两项 P1 开放”是统一核定之前的历史事实，不是当前施工导航。原报告移入历史目录不表示原 findings 全部修复，表示活跃责任已集中到本篇。既有历史档案不重新打开。

## 本轮验证与未覆盖

- 主代理亲读关键 Gate/停止/ready、ObservationSlot、runtime_stop/idle 代码，以及当前 Runnel Deadline、WaitSet Remove、服务/Notification/平台内存说明与相关主计划。辅助只读 explorer 提供旧运输/执行提交到当前路径与证据的位置，关闭判定由主代理承担。
- 唯一新代码运行证据是 H01 对应的既有 host 用例；没有新增测试、修改源码、运行 QEMU 或重复完整验收。该定向执行用于核对明确的原反例，不作为对象链全量复验。
- `a1ca124` 的 C09/C10/C11、N6 和 W05-04 证据按原版本读取，不把成功路线倒填为精确故障已注入。原普通路线 guest 日志按当时策略删除的部分只有聚合/严格检查证据，不补造现场。
- 当前工作区没有 `artifacts/convergence2/`、`convergence3/` 或 `historical-closure/` 的原验收产物。历史通过/失败次数与指纹由所属计划的已交付记录提供，本轮直接核对的是当前场景、断言、验收锚点和版本差异；未独立复算那些旧日志的 SHA 或逐 guest 计数。此限制与真正尚未执行的精确注入分别记录，不把工作区缺少历史日志改称生产代码失败，也不重复执行整套验收来恢复档案。
- 文档检查核对 42 个编号映射、唯一活跃报告、归档保真、当前链接/锚点及导航，不改写历史代码位置为当前位置。原报告中的 plain-text 源路径/行号保留当时含义。
- 本轮文档核验：42 个原编号均唯一映射，D/E 分组分别为 9/7；九份原报告归档后的 SHA-256 与原文一致；259 个本地链接/锚点及代码围栏通过，生产目录 diff 为空。提交前另外修正较早归档登记的目录迁移链接，暂存文档的本地路径检查通过。活跃文档使用默认空白检查；原始时间/运输报告自带的末尾空行原样归档，该档案检查只排除 `blank-at-eof`，其余空白规则照常检查。核验与来源摘要保存在 `artifacts/review-consolidation/{provenance,verification,relocated-links}.json`。
- 本轮不重新取证硬件规范、审查设备寄存器、不作 WCET/物理 CPU 份额证明，也不宣称跨平台的任意弱序、迟到或失败窗口已覆盖。
