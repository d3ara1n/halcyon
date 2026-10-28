# 设备体系设计：领域模型、驱动管理与资源收束

> 状态：**第二轮交付及原组合门已完成；最新整体 Review 的 G1–G5、C1 与文档偏差已修复、复核并完成所选组合验证，已提交为 `e23afdb`。** 首批设计增量与第二轮代码登记已合并，[统一报告](archived/review-2026-10-28-device-driver-overall.md)保存发现与修复证据；当前接续从[修复后接续](#整体审查修复后的接续2026-10-28)进入。本篇继续拥有领域目标、验收合同及条件后续。第二轮历史处置见[收敛交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)及[生命周期档案](archived/review-2026-10-27-device-lifecycle.md)。DMA＋IPI、全 hart idle 和真实 IRQ 门不重开；时钟及候选出生的未归因异常仅观察。服务装配、平台保留内存和系统终局由各自计划拥有。

第二轮已实施合同及原验证由[机制收敛档案](archived/review-2026-10-27-device-implementation.md)保存；其后的[生命周期与观察收口](archived/review-2026-10-27-device-lifecycle.md)已完成修复、复核及所选组合验证，该档案保存增量目标和证据。[唯一收口表](#当前唯一收口表2026-10-26)保存已关闭的 DMA＋IPI、全 hart idle 门及未归因的独立时钟异常；本次调查见[时钟复查](#独立时钟异常复查2026-10-27)。impls按当前代码回填；历史施工与审查交接段保留当时事实，不从其中的旧目标恢复施工。

## 目标与范围

设备应能在用户层被发现、认识、授权使用和管理；驱动聚焦硬件与领域协议，应用取得功能能力。设备目录是活的领域对象视图，包含存在、绑定、功能与状态，不能退化为静态 DT 数据或一个写入 PID 的标签。

通用目标覆盖平台设备、总线子设备、动态插拔、共享供应和功能组合。首批完整实现路线选定 **QEMU virt + PCI xHCI + 运行期 USB boot keyboard 热插**：插入前 HID 驱动未运行，系统发现设备、选择并启动驱动，应用取得键盘功能；拔出、失败和停止走同一责任链。打印机是结构检验场景，真实 USB 打印协议不属于首批交付。固定证据见[平台取证](ref-2026-10-23-device-platform-contracts.md)。

首批使用启动映像集提供驱动声明与可执行映像。磁盘仓库格式、安装、更新和来源认证按用户要求延期；不因此延期运行期按需激活。首批不实现通用 HID、外置 USB hub、PCI bridge/PCI 热插、MSI、IOMMU、非一致 DMA、动态保留区/reusable、透明管理者恢复。它们不能被静默当成已支持能力；具体触发与完成门见文末。

遵守 less is more：只有设备实例、一次绑定及所属领域的实际资源/会话；不建通用 Lease、ClaimScope、第二个 HAL/DAL 发现框架、驱动 Runtime、通用分布式事务或持久恢复账本。已有 OAL、RPC、执行、等待、进程与预算机制继续按其合同使用。

## 第二轮收敛设计（2026-10-27）

本轮由用户授权继续审查与重构设计。以下是主代理依据实际代码、架构咨询及责任链推演作出的目标裁决，当前已按此实施；代码、验证与剩余边界由[机制收敛报告](archived/review-2026-10-27-device-implementation.md)承接，原验收门的当前关闭/开放状态由本篇唯一收口表维护。没有增加复合USB、IOMMU、透明重启或通用驱动框架的承诺。

### 本轮选择与适用边界

选用“稳定逐轮 owner＋完整状态观察＋原轮 Use 直接提交受限静止事实”。srv_driver 继续拥有选择、进程关联与故障政策；srv_device 不识别程序、PID 或 libdriver 的激活阶段。停止的唯一业务入口为原 Binding 的 Stop/StopClaim/来源封口，驱动按原 Binding 观察结束访问，不再需要管理者逐层重发 StopActivation。

采用直连的依据是：可以在已有 BindingUse 的认证 context 上增加一项冻结的窄权限，不需要新参与者登记、报告对象、恢复账本或第三个协调者。原来 `Activation snapshot→Managed→ReportStopped` 的转述链因此消失；`Binding→Provision` 的供应协调仍由设备领域拥有。普通硬件报告不能因进程终态被合成。

| 比较项 | 保留协调者转述 | 本轮选用结构 |
|---|---|---|
| 停止传播 | Binding Stop 后还向 worker 发 Stop，再查询激活状态 | 原轮 Binding Watch 交完整快照，驱动据此停止 |
| 静止事实 | 管理者核声明、读 snapshot、发 ReportStopped | 已授权的 Use context 直接报告自身原轮，管理者不解释硬件证据 |
| 供应进展 | 重复 Prepare/Stop 读状态，单独 ConfirmStopped | 原 provider 保存状态并通知；Stop 带单调的消费者静止事实 |
| 能力发行 | 每次 IssueUse/IssueRegistrar 可新建角色，Unknown 只能保守停止 | 一个 Binding 的逻辑消费角色和子登记来源固定，重复请求恢复原别名 |
| 生命周期 | RPC 请求内首次准备，Instance/Engine 在旁等待 | 逐轮实例拥有准备资格与实际执行；RPC 只读写该实例 |

这些差异允许改变现有用户态协议；相关生产者、消费者和验收一起迁移，不保留兼容命令。共享内核 ABI、DMA 安全模型、MMIO 强制退休、IRQ token 契约不因此修改。

### 1. 稳定记录与阶段持有

Registry/Seat 仍是设备存在和当前占用的唯一真值。Binding 的任务在 Claim 提交前准入，持有供应调用、冻结表、暴露事实和释放责任。当前业务事实与运输 txid 分开：Query 不形成业务阶段，不保存 Query 结束后回跳的 resume；已提交的意图、完成事实和尚缺条件决定下一动作。

| owner | 必须保存的阶段资源 | 退休提交点 |
|---|---|---|
| Binding 业务部分 | Provision、Collecting/Frozen、原轮观察、单一供应调用及停止期限 | provider 明确 Closed、真实依赖 Binding 已 Released、原调用已脱离后，Registry 同 actor 提交 Released |
| 驱动逐激活实例 | 冻结的 Use/Registrar 身份、唯一首次核验、Binding 观察、实际 Engine 与本轮 IO | 无访问效果的拒绝完成本地收尾；已开始访问者先停止并收束实际 IO，再等原 Binding Released |
| Provision | 一个 active epoch 的准备/停止效果、冻结输出、进度通知出口 | 实际供应资源与迟到硬件责任安全结束，更新 closed_epoch |
| 终态回执 | 固定原轮身份、只读结果、旧角色/Lifetime、已接请求与实际存储费用 | 最后对应角色和已接运输退休；不阻止业务 Released |
| ProcessTask | 稳定 InstanceId、进程/Pool、启动与 DONE、从 runs 推导的关联 | 全部绑定安全脱离后收集；不等待外部旧功能别名永久消失 |

Binding 在 Released 后消耗式移出并释放业务 Box，原任务可继续承载紧凑回执和真实 Lifetime；不能只切 enum 而保留完整 Work/provider。终态接口只操作本回执，拒绝 Get/新派生/新功能安装；Query/重复 Stop 仍针对原轮。只有仍会用于范围/授权查询的独立对象保留 Registry 身份引用，回执不为已结束业务额外保留来源/设备引用。费用按实际释放的存储退款，尾部记录仍有独立数量/字节上限。

驱动的首次 Activate 在任何远端核验前完成本地认证、冻结输入、预付实例/任务/观察容量，并原子登记唯一 Preparing owner。Preparing 已承担接纳后的责任，但没有访问硬件的许可。原 BindingUse 的首次权威核验和 Watch 安装均由它持有，只有确认原轮 ResourcesReady、观察承载已可用后才开始 Get/IO。并发重复请求只引用该 owner，不各自发首次查询；输入/入口身份不一致返回 Conflict。

核验拒绝、超时或 Stop 先到时，原 Preparing 永久关闭提交资格；取消/退休查询和所有已接请求后进入拒绝回执，不需要先造 Activation 才能退休。若从未取得/使用资源，不为本地退场等待一个伪造的“成功激活”；已经发生 IO 的记录必须走正常停止。记录从索引摘除前须排除其旧查询再提交的可能；所有重入 Activate 都重新核原 Use，中央已经停止的旧轮不能复活。未被接受的请求只释放自身候选，不改变已存在实例。

### 2. BindingUse 与子 Registrar 是原轮出口

一个 Binding 对应一个逻辑消费 owner；capability 别名可供该 owner 内部委派，不能解释成多个独立的静止报告者。`IssueUse` 首次成功把一个固定 context 和母本归入原 Binding；重复调用返回同一身份别名，不能再发行第二个独立 Use。必要角色/执行存储在首次提交前预付，未提交失败不设置已发行；提交后回复丢失可按原 Control 恢复。

真实 GetResource 只接受该原轮 Use；Control 保留管理、查询和零资源能力的发现投影，不再直接取得硬件资源。这样资源出口与消费报告指向同一逻辑消费角色。自行消费硬件的 init 验收也必须先取得 Use，不能保留测试特权分支。原 Use 的内部别名和派生访问由同一可信消费 owner 负责，报告必须覆盖全部委派访问。

`IssueRegistrar` 同样为一个父 Binding 固定一对 Registrar/SourceAdmin。其范围、父轮和来源权限由原 Control 与 CHILD_REGISTER 派生，重复只交原别名；一个 Registrar 可按原 publication_seq 发布许多子实例，不需要每次请求发行新来源。通用 GrantSource 的任意派生不据此变成可恢复事务。

Stopping 后只允许恢复已经存在的原轮出口，且它们的新使用门已经关闭；不得首次发行 Use 或 Registrar。Released 后关闭母本，不再取得新别名；已在途和外部别名按原身份退休。母本不与服务记录组成递归强引用环；子来源仍用 BindingRef 数字关系，既有实际来源/设备引用负责其存储寿命。

这改变原“每次 IssueUse 独立发行”的用户态语义，迁移全部实际调用者。多 Binding 进程复用继续按各自 BindingRef/context 隔离，不把同进程的 Use 合并。

### 3. 停止要求与受限报告

登记增加不可变的 `StopRequirement`，由真实供应 owner 在受权 Register 时声明，重复 Register 必须相同：

- `ProviderOwned`：供应者能独立终止已交付会话的实际访问并证明安全；普通 USB 接口使用此形态。
- `ConsumerQuiescence`：供应停止需要原消费 owner 先结束直接访问；平台 PCI host 和 PCI xHCI 使用此形态。

停止要求不再从 Frozen 中“是否出现 DmaUse/DmaDomainControl”猜测。它描述供应安全条件，不授予报告权；声明错误的 provider 仍违反自己作出的 Closed 承诺。新平台或总线必须按真实停止能力选形态，不能靠隐藏 DMA 于另一种 capability 来改变责任。

`ConsumerQuiescence` 的 IssueUse 要求原 ClaimSource 具有 REPORT_STOPPED 的委派权限，否则拒绝发行；服务在该固定 Use 角色上记录 `REPORT_QUIESCENT` 操作权。受信驱动的选择和这项权限的交付由已认证装配/srv_driver 决定，驱动声明、payload、PID 均不能自授。ProviderOwned 的 Use 没有这项权利；HID 不取得物理停止报告权。

首次对实际 BindingUse 的核验回复同时包含服务端保存的停止要求与该角色操作掩码，worker 在任何 IO 前核对所需报告权限；这是已认证接口的角色描述，不由程序配置猜测。后续每次报告仍检查服务端真实 context/权限，客户端缓存不能授权。Binding Watch 只携原轮业务状态，不把操作权当成全局可转让字段。

`ReportQuiescent` 使用实际 Use context 定位原 Binding，无任意目标参数。只在原 Binding 已 Stopping 时接纳，原轮内幂等、单向；来源封口不撤掉已承担清理的报告权限。Released 旧角色仅返回本轮已经结束，不作用于后来 epoch。报告的含义是“该消费 owner 及其委派访问已经永久结束”；typed API 为 unsafe 证据边界，不能由 Terminal/CPU DONE/关闭句柄自动调用。

Binding 在同 actor 封 Get 后仍可依据自己的不可逆 `resources_exposed=false` 证明消费访问从未产生，无需等待不存在的 worker 报告。该位在真实资源别名离开唯一出口前置位，回复缺失不能清回 false。消费代码若能实际证明本地未访问硬件或已经完成停止，即使 Get 运输结果 Unknown 也可提交真实报告；不能仅凭“未收到成功回复”推导这一事实。

### 4. 观察返回完整状态，运输不制造业务阶段

Binding Watch 的 Query/Ack 回复原子配对 `{sequence, pending, terminal_reason, BindingSnapshot}`。建立时固定 BindingRef 并持原 Shared Binding，因此 Released 后读它的终态回执，不跟随后来的 Seat。Scope Watch 仍只提供范围变化信息，枚举按原弱一致合同；不把一棵树塞进快照。

使用现有 librpc::subscription::Protocol 的类型化 Snapshot。Binding 与 Activation 的快照进入宿主已有的有版本缓存后即可 Ack 该序号并唤醒所属 owner，不必等后续 RPC/功能发布完成才确认“已消费”；通知不是要求逐项执行的命令队列。版本不能倒退，停止/终态不能被旧快照覆盖，首个失败理由不被后来普通更新清除。

保留既有 Subscription 任务作为独立等待/运输 owner；先去掉它与 Managed 间的 reading/changed 往返。它们的源/txid 有实际用途，不为减少任务数强迫多个 ConsumerTask 共享一个会被全部 drain 的 Input。Managed 消费缓存事实，公共 Drivers 视图直接投影这些事实及选择/进程关联，不另写一套生命周期阶段。

worker 的 Binding 观察同样独立于硬件 RPC，先于资源访问成立。用户来源失效、Stop 和 Released 都沿此通道；普通生命周期不再 1ms/2ms Query。订阅建立失败时不得开始 IO，已经在 IO 的观察失败转本地停止/明确故障，绝对期限仍负责升级。QueryBinding 保留显式检查和有限 Unknown 恢复，不承载正常常态轮询。

### 5. Activate 接纳、观察与结果分离

worker 管理入口只接受：

| 操作 | 输入与提交语义 | 回复 |
|---|---|---|
| Activate | 原 BindingRef、固定 identity、Use、可选原 Registrar；认证成功并预付唯一 Preparing owner 后接纳 | 原轮接纳快照与观察入口；不等待硬件 Ready |
| QueryActivationResult | 原 BindingRef；仅真实 WorkerControl manager context 可调用，不再携 Use/Registrar | 原实例状态；Ready 时配对同一个功能根、名字和描述 |
| 公开激活 Observe/Watch/Ack | 原观察 context，零业务 capability 输入 | 有序状态快照；不授予功能能力 |

移除独立 StopActivation 命令。正常停止、取消未知 Activate 和替换实现都先提交原 Binding Stop；worker 从绑定观察和首次核验收束，不由管理者再发第二种停止命令。实例自主失败时立即封新业务并发布带首个错误原因的 Stopping 快照，管理者据此立即提交原 Binding Stop，不等 Terminal 或静止报告；实例继续持有原 IO 与子供应清理，访问永久结束后才报告 Quiescent，实际本地清理完成后发布 Terminal。管理者失效由 init 原 SourceAdmin/Job 的故障政策收束。这样不会让父实例等子清理完成才公布错误、子又等父 Binding Stop 的通知环。

Ready 的 function capability 只能经 WorkerControl 的结果出口交管理者；公开激活观察不得借机取得功能，Devices/FunctionAccess 的授权门仍在。管理者按原轮取得一次功能结果并幂等安装，Ready→Stopping 交错时丢弃迟到根，不发布。普通 Starting/Stopping/Terminal 和静止进展直接读 Snapshot，不重新 Activate。

Activate 的 Unknown 恢复只针对同一原轮和冻结输入。决定取消时只停止原 Binding，不用恢复回执才能停止。没有取得观察入口时可按同一 Activate 恢复；Stop 后迟到 Activate 的本地/远端核验永久阻止新 IO。RPC 的取消与结果注销先完成，才归还该运输的预留。

### 6. Provision 采用通知和单调停止意图

每个 Binding Work 在 Claim 准入时预付一对 Notification 和一个本地等待来源。Work 持 Owner 并先挂好等待，provider 只取得 SIGNAL 角色；Prepare(e, signaler) 在准备效果之前固定该轮通知身份。同 e 重复 Prepare 必须指向同一通知对象，不换 sink；closed_epoch 仍封住迟到 Prepare，不保留无限历史。

provider 在 Preparing→Prepared、失败、停止进展成为可读事实及 Closed 的提交点发提示。Work 在准备查询前排空自身通知，再读取 `QueryProvision(e)` 的完整进度；查询在途来的新提示留作下一次读取。通知可合并、不是事实或计数；snapshot/closed_epoch 才是判据。首次 Prepare 回复可以直接带现有进度，后续无变化时不重发 Prepare。

`StopProvision(e, consumer_quiescent)` 把停止意图与已有消费静止事实一次提交：phase 单向进入 Stopping，true 只能累积，后来 false 不能撤回；true 可随第一次 Stop 到达。停止要求为 ProviderOwned 时 provider 自行收束；ConsumerQuiescence 要求 true 或已独立取得足够硬件证据后才完成。它仍拥有 Prepare 自身效果、真实子责任、IRQ/MMIO/DMA 退休。

移除 ConfirmStopped 以及中央 stop_acknowledged→confirmed 的专用串行握手。中央先发 false 以尽快封供应，后来取得真实报告时再发 true；回复缺失读取同轮进度或在原绝对期限内重提同一单调意图，不新建轮次。明确 Closed 才进入中央依赖释放检查；Unavailable/NotFound/超时从不变成 Closed。通知失败关闭观察或触发明确故障，不能因此释放硬件。

NotificationSignaler 的最后关闭不关闭接收 Owner 的电平，不能用通知 CLOSED 充当 provider 死亡检测。继续保留既有 Provision 入口 Lifetime/供应进程监督和准备、停止的绝对期限。排空提示后的 Query 若失败，必须有限恢复或进入明确故障，不能假设以后还有一条提示而永久 Parked。

Prepare/Stop 的通知出口来自唯一调用者 Work，无动态 Watch 注册、Ack RPC 或任意多观察者表。实际 provider 的普通准备/停止进度仍按其既有 Runtime 推进；所有输出副本和效果继续归同一原轮。外部别名与迟到运输不能使新轮采用旧结果。

### 7. USB 完成通知与必要轮询分开

原 USB session 保留单 seq、输入和结果的合同。HID 在第一条 Transfer 前创建并挂好本 session 的通知 Owner；第一条 Transfer(seq=1, request, signaler) 在实际提交 TD 前把通知身份连同原请求保存。重复同 seq 核输入和通知身份；后续 seq 沿同一通知，不重复建来源。若第一条操作是 Cancel，同样交该通知以封该 seq；Query 不创建请求或改变通知身份。

Controller 的真实结果写入 `current.result` 后发提示。HID 排空提示后 Query 原 seq；结果已先完成也必须可读。下一 seq 只在旧结果已取得后提交，沿原规则确认旧结果；旧提示最多引起一次新查询，不能确认后来结果。Stop/取消/断开既结束接纳，也产生可查询终态并提示。

通知 Owner 消散或 signal 失败只说明客户端观察不可达，不能释放 TD。实际 TD/地址/结果仍由原 session/Controller 持有，沿 Provision.Stop 和原硬件命令收束。只读 Query 不无条件唤醒硬件 Engine。

xHCI 需要读 CNR/HCRST/HCH、reset/枚举阶段尚无可用事件的推进，保留按具体硬件条件和绝对期限安排的轮询。进入稳定运行后靠 IRQ/请求/真实资源 DONE 推进，不把全部 Engine 永久挂在一个 1ms 节拍上。各 Stage 的唤醒来源须在迁移时逐项核定，不能先删 timer 再补偶然唤醒。

### 8. 三条端到端责任轨迹

| 轨迹 | 业务推进与最终 owner | Unknown/失败边界 |
|---|---|---|
| 正常停止 | Control.Stop 封中央 Get 并发布 Stopping；Work 向 provider 提交 Stop(false)，worker 从原 Watch 停自身及下游 IO；可信 worker 以原 Use 报告；Work 提交 Stop(true)；provider 结束实际资源并公布 Closed；Registry 核实际依赖后 Released；worker/Managed 脱离，ProcessTask 收集 | 任一等待仅以对应 owner 的真实完成事实推进；期限到界进入原故障政策，不先归还 DMA |
| Activate 已接纳、回复丢失 | worker 的 Preparing owner 仍持首次核验/观察；管理者可以同输入恢复，也可直接 StopClaim；迟到核验见 Stopping 不 Get，已访问者正常停止并报告；原 Binding 独立持供应责任 | 不另签 Use/Registrar、不换 epoch；未取得回执不是无消费者证明。原调用/旧请求必须脱离后才丢相关记录 |
| 父已停止访问、子尚未 Released | 父 Engine 停新业务但保留供应协议服务；子硬件/会话由真实 provider 完成 Closed，中央释放子轮；父只有结束子清理仍需的父资源访问后才可报告，报告不必等子软件 Binding Released；中央父轮仍按实际依赖门等待，父进程关联保留到父 Released | 自主失败先公布 Stopping/错误以传播停止；不等父进程 DONE 才释放子、不因父 Terminal 杀父进程；静态拓扑子节点不自动变为供应依赖 |

当 ProviderOwned 的 HID 子消费者死亡时，xHCI 的 session owner 仍能取消/停止 TD，中央按供应安全释放，不要求死 HID 的报告。可信 xHCI/PCI 实际 owner 死亡而无硬件静止证明时，保持 DMA pin/写许可及原故障终局，不新增透明恢复。

来源树和设备树的准入、撤出及真实依赖谓词暂保留为单一真值。当前按两类关系推导的释放集合有独立语义；为少扫几次而增加永久供应 DAG、镜像成员表或第二份计数不构成本轮收敛。通知负责唤醒已有父任务，最终释放仍在同一 actor 原谓词复核。

### 9. 付款、容量与有界执行

| 承载 | 准入点及付款者 | 结构容量与释放 |
|---|---|---|
| 中央 Binding | Claim 前，原来源 domain/execution 账户；内核对象另付所在进程 Pool | 一份业务 owner、一份供应通知/等待、最多一条在途供应 RPC，外加按 Limits 预付的资源表；Released 释放业务存储 |
| 稳定 Use / 子来源 | 首次发行前，原 Binding 来源账户 | 每 Binding 一个 Use、至多一对子 Registrar/Admin；母本随停止/释放规则关闭，旧运输照寿命付款 |
| Binding/Activation Watch | 发行前由原调用角色账户准入 | 每订阅一等待来源、一 RPC 槽和有界快照；管理者每 Binding 至多两种订阅，worker 每激活一 Binding 订阅 |
| 驱动实例 | Activate 接纳前，worker 原实例与执行预算 | 一份 Preparing/Engine owner、业务 RPC 与绑定观察各自有承载；报告在业务调用已脱离后可复用其槽，绝不抢未退休 txid |
| USB 通知 | 第一条请求提交前，HID 付 Owner/来源，provider 付存储与接收句柄 | 每真实 session 一份通知身份，TD/结果容量沿原配置；无按键时不增加周期请求 |
| 终态回执 | 原准入预留覆盖，原 owner 继续付款 | 按实际小记录、角色、Lifetime、已接请求计费；不凭逻辑 Released 提前退款 |

生产者的 signal/Query/Stop 失败不丢已提交 owner。预留的是控制/清理执行、存储和有限在途槽，不能宣称底层物理堆或所有 Receive 安装必成；运输失败以原有限收尾或明确故障处理，禁止为成功率无限重发。公开观察流量不能借报告/停止角色占控制预留；报告认证必须在选择付款账户前完成。

任务每步只做一个协议提交、一次有界资源推进或一个记录处理；通知、业务和退出均回到现有 Runtime 公平推进。库不另建执行器。用户态仍以 actor 单一所有权为主；本轮不新增跨线程共享状态或内核锁阶。

### 10. 实施顺序、删除门与验证

两条责任链已按以下顺序完成一致性迁移。这里保留范围及删除门，后续接手以当前唯一收口表为准：

1. **设备逐轮控制链**：同步 libdevice/libdriver/libusb 协议与类型；重构中央 Binding/固定出口/完整 Watch；接 provider 通知与单调 Stop；接驱动 Preparing owner、Binding 观察、直接报告、结果取得；接 srv_driver 事实推进及真实消费者。旧 StopActivation、ConfirmStopped、Control 直接 Get/报告、按快照重发 Activate、周期生命周期/USB 查询和重复 Describe 已删除。`srv_init::function_checks` 使用原 Use Get/Report，并保留旧 Use 验证关闭，不把新 Control 的 Permission 当成旧轮失效证据。
2. **装配监督链**：由[服务装配的第二轮设计](todo-2026-10-07-service-assembly.md#第二轮持续监督设计2026-10-27)拥有；阻塞等待、loader 与原 root 已接通，双线程协调已删除。

C5 的 DMA 未发布空根构造回退按内核独立契约实施：末次 retained 条目失败时，趁唯一局部 owner 仍持 Node，在锁内 seal/finish/remove 该零成员单根、锁外释放；Node Drop 见已无记录不发布 Root Work。仅适用于未发布空根，真实派生树和已暴露 Mapping 的维护/保留不改。

验证以可观察合同为依据：同轮重复发行身份不变；Preparing 拒绝及迟到查询不复活；两个 Binding 不能互报；普通 Use 拒报告；来源 Seal 后 Watch 仍交最终快照；旧 Ack 不清新变化；Prepare/Stop/Activate/结果回复缺失仍归原轮；父子停止无循环；无输入 USB 不重复发请求且完成早于等待不丢；旧入口尾部不持供应 owner；公开压力不挤占清理执行。复用现有 host 协议/Registry 测试、真实 PCI/xHCI/HID 与 A/B/C 消费者。

本次实现及所选组合验证已完成，全 hart idle 与 DMA＋IPI 门已补证关闭，具体版本和限制见唯一收口表。时钟异常仅观察。后续修改按实际影响执行 check/clippy 和必要组合，不恢复已删除机制的专属验收门，也不把设计推演替代运行证据。

### 外部机制对照及适用限制

- [Genode Foundations 26.05：异步通知](https://genode.org/documentation/genode-foundations/26.05/architecture/Inter-component_communication.html) 明确 signal 可合并、无 payload，处理次数不能当事件次数，需要另读当前状态；[session 寿命](https://genode.org/documentation/genode-foundations/26.05/architecture/Recursive_system_structure.html) 由 server 拥有 RPC 对象身份。这里用于核对“owner 保存结果、通知只提示”，不移植其销毁 server 等同关闭 session 的政策。
- [Linux 6.12 devres](https://www.kernel.org/doc/html/v6.12/driver-api/driver-model/devres.html) 明确 managed resources 只负责 freeing，其他检查仍归驱动；[v6.12 dd.c](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/base/dd.c?h=v6.12) 的 device_remove 先于 device_unbind_cleanup/devres_release_all。它支持软件释放与硬件停止分别论证，不支持用 Drop 推导 DMA 静止。
- [Fuchsia driver dispatcher](https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads) 与 [C++ driver rubric](https://fuchsia.dev/fuchsia-src/development/drivers/developer_guide/cpp-rubric) 分别描述 dispatcher 停止顺序与无法确认设备停止时保留 pin。两者为 2026-10-27 取回的滚动页面，且 DriverBase/DriverBase2 命名跨页不同，仅作非规范对照，不作为本项目固定硬件合同或子设备停止顺序的证据。

### 第二轮提交与调查接力

第二轮代码交付统一包含设备控制链、持续监督及分步 ELF/loader/Image、未发布 DMA 空根局部回滚、后续生命周期与观察收口，以及实际消费者、验收工具和说明回填。代码基线为 `a5a1a7893872e73d410eecb38b7ea21aed7d1616`，固定目标为 `c5d005dc7e6c64a74f0bb7e5874494c4532db4b0`；[原独立 Review 登记](archived/todo-2026-10-27-device-convergence-review.md)保留该次范围，现与首批设计增量一起由[最新代码整体报告](archived/review-2026-10-28-device-driver-overall.md)承接。归档报告中的“尚未提交”保留当时事实，不作为当前接力状态；第二轮仅本地提交，未 push。

最近版本已通过 `just check`、十分面 Clippy、137项相关 host 测试及 core/reuse/candidates/QueryUse/GetResource/Master 六条真实路线，具体证据与精确交错未动态注入的边界见[生命周期交付档案](archived/review-2026-10-27-device-lifecycle.md#最终组合验证与交付边界)。提交整理已核源码与该版本指纹一致，不重复运行已通过路线；较早第二轮平台/监督/IRQ 证据按原档案的版本和覆盖边界复用，不冒充本轮逐项重跑。

本次 DMA＋IPI 单项调查已完成，后续从以下当前状态接手：

1. **DMA 保留＋IPI 失败组合（已关闭）**：本次第四轮完整路线退出 0，同次 InitialWatch DONE、合格 WFI 目标的失败返回/消费、后续原广播及同组 Mapping/Backing 跨四 hart 停驻保留已取得证据。原 selector/WFI 与映射集合判据不变；[调查记录](archived/ref-2026-10-27-device-delivery-history.md#dmaipi-组合调查与关闭2026-10-27)保存工具修正、原失败和覆盖限制，不重复运行本门。
2. **独立平台时钟异常（仅观察）**：本轮[复查](#独立时钟异常复查2026-10-27)仍未复现。用户决定不复发就不继续调查，观察状态与重开条件统一见 [KNOWN_ISSUES](../KNOWN_ISSUES.md#历史偶发异常仅观察)；本篇保留原证据及复发后的取证参考，不占当前排期，不将未复现改写为修复，也不将该异常算作 DMA＋IPI 失败。

后续 `956685b` 只包含 DMA＋IPI 验收工具修正、证据收口及导航更新，未改变产品实现或共享 ABI。2026-10-28 按用户要求将原两份待审登记合并，整体 Review 固定最新 `d19fdfe`；G1–G5、C1 已完成修复，证据保存在统一报告，接续转至[修复后接续](#整体审查修复后的接续2026-10-28)，不重复第二轮旧迁移。通用长期服务、持久化关机及其他扩展仍按各自触发条件设计。

## 领域、运行角色与 OAL

| 事实或责任 | 唯一 owner | 外部表达 |
|---|---|---|
| 设备被接纳、名字/父关系、占用和绑定阶段 | srv_device 的设备 backing | Devices 视图、设备领域查询与控制 |
| 物理端口、总线地址、协议状态、资源效果 | 实际平台/总线/供应 provider | 领域描述、受限资源入口和功能 backing |
| 支持声明与对应映像 | 启动来源，未来为驱动仓库来源 | 配对取得的不可变声明与映像能力 |
| 选了哪个实现、运行在哪个进程、激活/停止进度 | srv_driver | Drivers 运行实例视图及设备上的观察关联 |
| 系统装配、授权委托、管理者监督 | init | 实际 Job/Process、管理入口与控制根 |
| 打印、USB、键盘等业务状态 | 对应功能 provider | 可经多处挂载访问同一真实功能 |

OAL 是虚拟投影，不是全系统实际树。srv_device 可以拥有真实登记树，hub 可以拥有自己的真实端口树；挂载与 Boundary 连接它们。不能另起与 OAL 平行的通用路径、发现和接续机制。领域操作可以解析自身的设备路径，也可直接使用实际入口，不强制先 Lookup。

示意布局：`/System/Devices/<拓扑>/binding` 观察当前绑定与激活关系，`functions` 接续实际功能；`/System/Drivers/<运行实例>/bindings` 表达同一进程的多个激活关系；`/Printers/office` 等使用视图可以指向同一个功能 provider。这些是装配名字，不是权限来源，也不是必须同步的三棵可写设备状态树。Devices 自身既可用于匹配，也可按入口授权交付功能。

公开观察入口不泄漏 BindingControl、MMIO 或 USB transport。取得能力走显式领域方法，回复将实际 capability 与元数据配对；同一路径经不同授权入口可有不同可操作范围。路径/tag/ObjectInfo 都是瞬态定位或观察，不是身份、占用或授权。使用现有 Registry 时遵守其平铺叶条目与 GetEntry 合同，不将 `/Services/Printers/...` 当作现成的嵌套服务树。

## 模块与依赖

| 目标位置 | 拥有内容 |
|---|---|
| `user/libraries/libdevice/` | DeviceId、登记、观察、Claim/Binding、Provision、功能关联及 OAL 投影支持 |
| `user/libraries/libdriver/` | 支持声明、候选选择、激活/停止协议、具名类型化资源解码；消费 libdevice |
| `user/services/srv_device/` | 中央登记和绑定 backing，组合平台导入；不依赖 libdriver、不启动驱动 |
| `user/services/srv_driver/` | 匹配、启动/复用、激活、发布与驱动监督，组合 libdriver/libdevice/libprocess |
| `user/libraries/libpci/`、`libusb/`、`libhid/` | 各自总线/功能的类型、限制、协议及可复用实现；不替 srv_device 保存占用 |
| `user/drivers/drv_pci_generic/` | 首批 PCI host 枚举、BAR/INTx 装配与子设备 Provision |
| `user/drivers/drv_usb_xhci/` | xHCI、root hub、USB 枚举和接口 transport |
| `user/drivers/drv_usb_hid/` | Boot keyboard 接口驱动与键盘功能 provider |
| `shared/dtb/` | 从 `os/dtb/` 迁移可复用纯解析，保持组件名；内核与用户态共享，不复制 parser |
| `shared/erhino_shared/`、`os/kernel/`、`user/libraries/libkernel/` | 同步定义、实施和封装特权 MMIO/IRQ/DMA 对象与操作 |

`shared/dtb` 已迁移，`libdevice` 已落实登记/绑定/供应状态、冻结资源表、授权/登记 backing、RPC 协议与组合绑定任务；实际 `srv_device` 已接入 init 的启动和正常停止。真实 PCI/xHCI/HID、功能客户端及管理监督已接通；当前能力与证据边界见唯一收口表及延期能力表。库不依赖具体 libruntime，不自行开线程或消息泵。libdriver 提供普通类型和可组合任务，驱动将绑定、IRQ、请求来源放入既有执行/等待机制。先显式按名解码，不预建 derive/schema 系统。libhub 只有实际共同领域知识形成时才抽取。

平台解析与资源组合位于知识使用侧：srv_device 的登记核心只调用 libdevice Provision 合同；平台适配器可组合 DT/资源库，具体总线驱动可组合供应库。不能通过回调把 USB 私有操作塞入 libdevice。

## 身份、登记与真实使用单元

`DeviceId = { registry_incarnation:u64, serial:u64 }`。incarnation 来自 srv_device 根 mailbox 的内核 KOID；serial 非零、单调、不复用。只承诺当前内核运行期引用，不承诺跨启动持久身份。KOID 与不回绕事实见[对象](../os/kernel/src/task/object.rs)、[分配器](../shared/monotonic_id/src/lib.rs)。任何数值 ID 都不授予权限。

登记冻结来源、父实例、静态匹配 identity、是否可 Claim、使用粒度与资源表容量上界。描述的可变运行状态另列，不能原地改变活动绑定的匹配依据或资源形状。撤出后重新出现产生新 DeviceId；普通状态变化、路径别名或改名不自动换 ID。

一个可 Claim 实例是一个独立独占使用单元，至多一个未 Released 的绑定。纯目录/聚合可以不可 Claim。接口是否建节点由实际独立绑定需求决定；整机与重叠接口不能无仲裁地同时暴露为独立席位。可以选一种粒度，或由真实父 provider 持粗粒度，再交付隔离的子会话。共享 IRQ、复位影响不能简单靠合并不相关设备解决；物理冲突和影响分别由真实资源/供应 owner 判断。

平台登记权可直接建立静态聚合，不为建目录制造假 Binding。hub 的子登记权由父绑定授权交付；libdevice 定义登记能力，不要求登记者提交 libdriver 类型。父关系必须指向已有合法祖先，不允许环。来源封口/祖先 Withdraw 在提交时直接阻止后代新 Claim，异步遍历只负责清理，不承担准入正确性。

### 登记操作与 Unknown

每个 Registrar context 保存单调 `publication_seq` 高水位及尚需保留的实例关系。调用者按此 context 串行登记：当前 Register 得到确定结果，或通过 WithdrawPublication 封闭这一轮后，才发行下一 seq。一个 context 可以登记许多同时存在的实例，但只有一个新的登记结果未决。

| 操作 | 提交与重试合同 |
|---|---|
| `Register(seq,parent,name,identity,limits,provision)` | 校验权限/来源/父关系、单分量名字唯一性、容量及付款后，一步分配 DeviceId 并发布实例；无远端准备。新 seq 必须是高水位的下一值 |
| 重复 `Register(seq,...)` | 活跃关系存在则返回同一 DeviceId/控制别名；静态输入及 Provision endpoint 身份必须相同，否则 Conflict；多余能力副本关闭 |
| `WithdrawPublication(seq)` | 当前下一 seq 尚未 Register 时也可关闭它并推进高水位；迟到 Register 不复活。已有实例则转 Withdrawn 并安排原绑定停止；旧已关闭 seq 返回 Closed |
| 配额/校验拒绝 | 不提交实例、不消费 seq；明确放弃时由 WithdrawPublication 关闭该轮 |
| `SealRegistrar` | 不可逆封新登记及本来源子实例准入，预算内逐个撤出；已提交清理继续，不因调用者死亡而取消 |

高水位以下已删除关系的 Register 返回 Closed，不重新登记。未来未发行 seq 返回 Invalid，不允许跳跃封住任意后续登记。seq 耗尽仍可撤出/清理，不能新登记。Registrar 的来源绑定退出时由 srv_device 主动封口；单纯 Close 某个 sender 副本不等于撤销。

`Present → Withdrawn` 单向；Withdrawn 不再新建 Claim，但旧 Binding 的控制/观察仍有效。存在未完成绑定、迟到准备或已发行控制壳时按真实寿命保留其记录及付款。新 ID 不证明旧物理资源空闲，Provision/资源 owner 仍须拒绝冲突使用。

## Claim、轮次与角色

每个可 Claim 实例有非零单调 `claim_epoch`，`BindingRef = (DeviceId, epoch)`。申请主体由内核认证的 `sender_context_id` 确定；同一 sender 的副本是同一主体，新 context 不是。PID 只能诊断，不能代替主体或绑定。

| 角色 | 操作范围 |
|---|---|
| DeviceAdmin / RegistrarAdmin | 受托登记来源、授权发行/封口、查询与停止其范围；init 保留外层管理 |
| DeviceObserver | 发现、查询、订阅；不含 Claim 或资源使用 |
| FunctionAccess | 在明确设备范围取得已发布功能入口；不含 Claim、BindingUse 或资源表；可与观察权装在同一 sender context |
| ClaimSource | 在已授权范围 Claim/StopClaim、恢复本主体原轮控制；由 srv_driver 持有 |
| ClaimSourceAdmin | 封本来源并停止其已提交绑定；交给 init，申请角色不能解封 |
| BindingControl | 查询/停止这一轮，取得固定原轮 Use/子 Registrar、安装不透明激活观察入口和功能关联；srv_driver 持有 |
| BindingUse | 逐名取本轮资源、观察本轮停止；只有按第二轮停止要求明确授权的角色可报告原轮消费静止，不含 Claim、来源管理或其他轮权限 |
| Provision | 本登记单元的准备/停止/结果接口；仅 srv_device 持有调用角色；该接口不附带 Claim、其他 BindingControl 或来源管理权 |

这些是领域权利集合，不要求每个角色另造 mailbox。provider 以内核认证 sender_context_id 查本地不可伪造的授权、范围和付款绑定；payload 里的角色名、PID、DeviceId 都不能替代该绑定。DeviceAdmin 可签发缩小范围的观察/功能/登记/Claim 来源，派生不能扩权；首批授权范围为已存在祖先及其后代，来源被封或祖先 Withdraw 后禁止新的业务取得，旧停止/观察控制仍可收尾。来源签发 Unknown 的装配收束由服务计划拥有，不恢复丢失结果后无条件再签发。

纯 DeviceObserver 的 OAL 视图只给发现信息，不能用 Boundary 绕过 FunctionAccess。`OpenFunction(DeviceId, expected_epoch, name)` 在本地提交点复核当前绑定、功能边和取得范围，返回该已安装入口的别名；名字为单分量，不在管理者内代理功能 RPC。停止后拒绝新取得，已经在途或交付的入口由真实功能 provider 终止。通过 OAL 接续取得同一功能时执行相同权限检查；函数入口的 KOID/context 身份用于重复安装比较，不比较各进程的本地 Handle 数值。

`Claim(DeviceId,expected_epoch)` 先本地准入并提交唯一占用，再由稳定任务推进资源准备。提交前预留 Binding、控制出口、准备任务和清理跟踪；没有远程调用夹在本地提交里。提交后回复失败不回滚，责任不再依赖申请者继续调用。

绑定阶段：`Preparing → ResourcesReady → Stopping → Released`；Preparing 可直接停止。ResourcesReady 仅表示整张资源表就绪，不表示驱动功能已经 Ready。Stop 封闭新使用，停止期间不回到前态；全部资源安全与迟到责任闭合后才 Released/推进 epoch。释放还须等待依赖原 BindingRef 的活 Binding 收束：从申请来源祖先的 Source.binding，以及设备/设备祖先登记来源的相同关系推导，精确匹配 DeviceId 与 epoch；单纯拓扑后代、独立根 Registrar 登记且无原轮来源依赖的静态节点不在此集合。Preparing/ResourcesReady/Stopping 阻止释放，Released 的控制/观察壳不阻止。Registry 在释放的同一 actor 提交点复核，不保存依赖表。

| 当前事实 | 请求 | 结果 |
|---|---|---|
| Present、Vacant(e)，来源有效 | a Claim(e) | 提交 Binding(e,a,Preparing)，返回 control 别名与当前阶段 |
| 本地准入失败 | Claim(e) | Quota/Invalid 等未提交结果；仍为 Vacant(e) |
| 已有 Binding(e,a)，含 Stopping 或实例已 Withdrawn | a Claim(e) | 同一 control 与当前阶段；不重复准备/激活 |
| 已有 Binding(e,a) | b Claim(e) | Busy，不交控制 |
| 当前轮已超过 e | Claim(e) | Stale；库不自动换新 epoch 重试 |
| Vacant(e) | 合法 StopClaim(e) | 关闭空闲轮并推进，Closed(e)，不制造 Binding 或准备 |
| 本主体 Binding(e) | StopClaim(e) / control.Stop | 进入/保持 Stopping(e)，返回停止已接管；最终结果须另观察 |
| 他人 Binding(e)，无管理权 | StopClaim(e) | NotYours/Permission |
| 已超过 e、或 Withdrawn 且该发行轮没有未完成绑定 | StopClaim(e) | Closed(e)，不影响新轮 |
| 未发行未来轮、错误 incarnation、服务不可达 | 请求 | InvalidEpoch/StaleInstance/Unavailable；不能当 Closed |

最后一轮可正常清理，u64 耗尽后拒绝新 Claim、不回绕。已发行控制只指向原轮，可保留付费的 Released 观察壳。Unknown 且继续使用时重试原 Claim；决定取消时直接 StopClaim 原轮，不能先换新轮或再 Claim 制造准备。停止/观察的权限在来源封口后保留，封口只禁止新承诺。

## Provision 与冻结资源表

每个登记单元有**一个 Provision owner**。它可组合多个供应者，内部顺序、部分失败和回滚由该具体 backing 拥有；libdevice 不设计供应 DAG、k 方事务或时钟私有操作。无可用供应时返回明确拒绝，或由匹配者在真实领域变化后再尝试，不隐式无限重试。

Provision backing 可以且必须持有自身供应、资源控制和清理所需的实际能力；例如 PCI provider 持窗口及 DMA/IRQ/MMIO 控制，USB provider 持父绑定和 transport 状态。“调用角色不附带其他权限”不限制实现 owner 的合法持有。登记/占用决定仍归 srv_device，backing 不通过一个 Provision 请求自行 Claim 新设备。

Provision 状态为 `{closed_epoch, active: None | (epoch, Preparing|Prepared|Stopping)}`。epoch 使用原 Binding 的 claim_epoch，不另发硬件轮号；closed_epoch 记录该入口已关闭的高水位，e≤closed_epoch 可 O(1) 判定旧轮，不保留逐轮历史结果。空闲 StopClaim 不调用 provider，因此相邻实际 Prepare 的 epoch 可以不连续。每个已发行 Provision 入口保留小壳至入口寿命结束；NotFound 不作为安全完成，provider 消失返回 Unavailable。srv_device 若在期限内未收到 Prepared/Rejected/Failed，提交原 Binding.Stop；只有收到 Closed 并完成本轮收束、进入 Released 后，匹配者才可依既定候选政策选择下一轮，超时本身不触发自动无限重试。

| 操作/结果 | 合同 |
|---|---|
| `Prepare(e,signaler)` | Binding 已提交后调用，按第二轮通知合同固定本轮结果提示出口；只建立一份 active，相同 e/输入恢复原进度，旧轮 Closed，不越过未关闭前轮 |
| `Rejected(e,reason)` | 没有遗留效果，provider 已将 closed_epoch 推至 e；可以安全收束该轮 |
| `Failed(e,reason)` | 可能有供应/硬件效果，转 Stopping；不能据失败直接释放 |
| `Prepared(e,count)` | 整表已冻结；逐索引 `GetPreparedResource(e,index)` 返回同一表项的 capability 别名及 name/type/rights/metadata；重复不重新分配业务资源 |
| `StopProvision(e,consumer_quiescent)` | 幂等且意图单调；无 active 时封住迟到 Prepare，已有相同轮停止自身效果，另一 active 拒绝越轮；消费者静止 true 只累积，独立 ConfirmStopped 删除，详见第二轮合同 |
| `QueryProvision(e)` | 读取原轮进度与已接纳静止事实，不新建准备、不推进硬件；Closed 仍由实际 owner 提交，常态变化靠原通知 |
| `Closed(e)` | 对该轮准备效果、派生 use、在途硬件访问与实际释放条件的保证；不是仅关闭 endpoint 或超时 |

调用权限限定到本实例的 srv_device 调用 context；普通驱动与 payload 中的角色标志不能发行 Provision 轮次。无 active 时，Prepare 接受该已授权发行者的 e>closed_epoch，先把 closed_epoch 推至 e−1（未曾准备的跳过轮均关闭），再建立 active(e)；有 active 时只接受相同 e，其他新轮返回 Busy。srv_device 只为本地已提交 Binding 发 Prepare，先收到旧 active 的 Closed 才发送下一轮；公开 Claim/StopClaim 的未来轮校验仍由 srv_device 强制。这样空闲取消不会卡住后续准备，旧在途 Prepare 也不能越过高水位复活。准备可能改变硬件，必须从一开始归原 Binding；不能把有副作用工作提前到 Claim 提交前。

srv_device 在 Prepared 后逐项取得表，校验唯一单分量名字、capability 角色、固定导出 rights 和登记上界。领域私有 metadata 是不透明字节。只有全部项到齐且绑定仍 Preparing 时，才一次转 ResourcesReady；cap/metadata 必须在同一回复配对。校验或 Receive 安装失败走原 Binding.Stop，不公布残表。

`GetResource(name)` 仅从这张本地冻结表取别名，固定权限，不接受临时扩权或新资源申请，不产生 Boundary。名字不是路径脚本，额外子分量为 NotDirectory。绑定 Resources 视图由同表生成，观察权不等于 Get 权。该选择取代早期允许 GetResource 远程接续的候选；OAL 拓扑与功能 Boundary 不受影响。

Get 可重试的含义仅为“不重复分配业务资源”；仍产生消息、Delivery、句柄和元数据费用。首批只接纳可别名角色，没有 taken 位、归还再取或仿射转移。供应/总线 session 的独占与终态由自身协议保证，管理者保留副本不改变其合同。

Stopping 时立即拒绝新 Get。晚到 Prepared/表项副本仍归旧 Binding，关闭副本并继续 Stop 收尾，不能交给新驱动。Closed 后迟到别名必须已经不能重新产生使用，仍按 RPC 收尾释放运输费用。provider 死亡只结束等待，不等于效果已结束；无硬件/资源证据便保持隔离并升级故障。

## 驱动发现、按需激活与设备绑定

init 启动管理基座并委托唯一自动选择范围；srv_driver 订阅设备、读取支持声明，统一选择。srv_device 不认识驱动程序，运行中的驱动也不另行全局匹配抢占。管理员可以共存并停止绑定，srv_driver 的激活进度不能替代设备侧占用事实。

### 声明输入与选择

首批来源是 init 交付的只读 boot 映像集，接口提供 `implementation_key`、协议版本、匹配规则、明确配置优先级、进程复用容量、资源需求和对应映像能力。声明与映像在同一来源操作中配对，采用后不可被路径替换；不先读声明、再按可变文件路径取程序。格式由后续仓库设计拥有，首批可用编译期记录关联启动包内映像，不提前设计磁盘 schema。

匹配规则为所属领域的静态 identity 精确条件组合，例如 platform compatible、PCI class、USB interface class/subclass/protocol；不建立任意脚本求值器。srv_driver 依赖对应协议库解释这些值。静态指定实现与配置优先级属于同一选择流程；同优先级有多个有效候选时报告 Ambiguous，等待明确政策，不能以启动/Claim 竞速选择。

一个实例与当前静态声明集合的一次选择，只逐个尝试有序候选一次；失败需旧绑定安全 Released 才尝试下一候选。资源不足、依赖未就绪等待实际变化或显式管理重试，不紧循环。声明变化默认只影响未绑定实例，替换活动实现需显式 Stop/重绑定政策。首批 boot 声明静态，不新增仓库 Watch。

### 激活与功能关联

1. srv_driver 选择候选并 Claim；未知结果恢复原轮或 StopClaim。资源就绪前不激活程序使用硬件。
2. 取得 ResourcesReady 后，按声明选择可复用且有容量的进程，或用受托 Job/libprocess 启动。首批 PCI host、xHCI 各为隔离进程；HID worker 可承载多个接口，容量来自实际预算配置。
3. 以 `Activate(BindingRef,BindingUse,identity)` 交给驱动。worker 按 BindingRef 去重；相同请求返回原激活状态，不再初始化一次。身份/入口不一致返回 Conflict。一个未完成激活有一份任务及预留状态，回复丢失由管理者重试同一 Activate 恢复（Activate 幂等，相同 BindingRef 返回原激活状态及当前阶段）。
4. 驱动逐名解码资源、检查类型和元数据、探测/初始化。已准入 IO 后自主失败先公布 Stopping/首错，由管理者停止原绑定；真实清理完成后才报告 Terminal。尚未准入 IO 的准备拒绝按第二轮独立出口收尾，探测碰过硬件后不能当作无效果拒绝。
5. 驱动 `Ready(function_root,description)` 交付一个 OAL 功能根。srv_driver 凭 BindingControl 安装该根及激活观察关联；同一根的重复安装幂等，不同根 Conflict。只允许 ResourcesReady 且尚未 Stop 的本轮安装；迟到 Ready 不发布，直接收尾。
6. 按装配需要发布 Registry 或用途视图。发布不是 Binding/设备/Registry 的全局事务；必选出口失败则该能力激活失败并停止，可选出口失败只降级诊断。首批必要出口为设备 functions，Registry 是可选别名。

驱动公开激活阶段为 Starting/Ready/Stopping/Terminal(ok|error)，其底层准备 owner 和拒绝回执按第二轮阶段持有合同区分。Ready 只表示功能可受理，Terminal 不证明硬件安全。停止要求、Use 窄权限及 ReportQuiescent 由[第二轮收敛设计](#第二轮收敛设计2026-10-27)唯一定义，取代管理者转述 ReportStopped 与独立 ConfirmStopped；不从声明/tag 或资源类型自授报告权。

停止统一提交原 Binding，不另发行 StopActivation。逐激活 owner 消费原轮观察收束；有访问效果的业务等真实 Released，拒绝准备与终态回执分别按第二轮合同退休。进程死亡停止其全部绑定，复用节省启动成本也扩大故障影响范围。

Activate 接纳后返回 Starting 和原观察入口；状态由有序快照读取，Ready 功能根由受管理入口认证的 QueryActivationResult 取得，不为观察进展重复运输 Use/Registrar。libdevice 只保存不透明激活观察入口，不解释 libdriver 阶段。worker 的原轮 owner 在开始 IO 前完成 Binding 核验和观察接线；具体提交、Unknown 与拒绝边界由第二轮合同拥有。

有访问效果的实例与无效果的拒绝准备分别按第二轮合同进入终态承载；最终回收仍须原请求/观察任务和导出关系全部退休。回收后迟到 Activate 必须重新核实际 Use；旧查询的提交资格在摘记录前永久关闭，数值 ref 不能复活使用权。硬件静止只由有权且持实际证据的原轮 Use 报告，任何公开激活阶段本身都不构成该报告。

停止首先封本地 Get、卸下功能边并通知相关 provider 停止准入，同时发起可选 Registry Withdraw；远端撤名失败不阻塞硬件停止。已取得的 session 由实际 provider 拒绝/收束，删目录不会撤销它。消费者重连必须显式取得新入口，不能把同路径新设备当作旧会话延续。

## 观察、分页与消息表达

设备 Watch 是本案新领域合同，复用已有通知/订阅原语，不宣称已有设备 Watch。订阅授权范围内已存在的根；等待未来子树时订阅祖先，不引入另一个未来节点对象。

顺序为 **先订阅 → Query(seq) → 枚举/读取 → Ack(旧 seq)**；期间有变化便仍可读并再扫描。seq 覆盖登记/撤出、绑定各阶段、功能边安装/撤除，也覆盖尚未 Released 的 Withdrawn 实例。它不聚合 hub 私有端口视图变化；后者由所属协议观察。

OAL 继续按名分页、跨页弱一致，无读者快照，不因每次热插使游标 Invalid。供设备管理内部候选扫描的领域枚举按 serial 向前，首批次携带当时最高 serial 作为有限终点；删除项跳过，后来的新项下一次扫描。状态在扫描期间变化由 seq 保持提示，已看到的候选可立即处理，不等待一份全局一致快照。Claim 总在提交点复核。每个 Watch 的 seq 不回绕，耗尽时该订阅终止并显式重订阅，不能伪造无变化。

用户态 wire 复用 librpc 请求/回复、期限、Unknown 和带序号观察支持；各领域独占自己的 opcode/version。整数定宽、小端编码，DeviceId 为两 u64，BindingRef 再加一 u64；名字/metadata 长度前置并 checked 校验，capability 用槽位对应，要求实际 handle_count 精确匹配。禁止将 Rust 内存布局直接作为 wire。单次文本/metadata 上界由现有消息载荷减 RPC/领域头和固定字段推导，单条回复逐项交付一个资源，不能把 8 个 handle 的运输上限变成设备总资源上限。超界在登记/解码时拒绝，整体大描述用只读 MemoryObject，长度和预算先准入。

领域错误至少区分 Permission、Invalid、Conflict、Quota、NotFound、StaleInstance、StaleEpoch、Busy、NotReady、Closed、Unavailable、Unsupported；运输错误保留 Unsent/Unknown。Closed 是对应领域明确承诺的旧轮完成，不能由 NotFound、timeout 或 endpoint DEAD 转换。ABI 数字编号及结构 padding 在实现时按所属协议统一分配，不改变本篇角色和结果语义。

## 控制根、付款与故障

init 在被监督故障域外保留真实平台根、ClaimSourceAdmin、管理/执行预算及 Job/Process 控制。srv_device 持受托资源控制及登记/绑定账本，srv_driver 持 ClaimSource/BindingControl/驱动 Job，普通驱动只持本次 use。总线枚举者可额外取得限定父窗口/子登记权，属于明确角色，不把整个平台控制交给每个驱动。

`SealAndStopClaimSource` 先原子关闭来源新 Claim，再由已准入任务分批停止所有已提交绑定，包括 Preparing 和控制回复丢失的绑定。原主体恢复旧 control/StopClaim 仍可收尾。来源永久封口；接替 srv_driver 使用新 context，不透明继承。全部 Released 和消息/观察壳最终退款是不同事实。

`StopClaim` 的 wire 保留 `Closed` 与 `Binding(Stopping snapshot)` 的区别，并校验原 BindingRef，不以空成功混合停止已接管和安全完成。`QuerySourceStop` 只接受真实 SourceAdmin context，来源封口后仍可调用；从同一 Registry 推导 `Open/Stopping/Closed`，不保存来源的第二份停止阶段。有效封口要求该来源或其来源祖先已经 Seal，未封口绝不 Closed。完成集合包括 claimant 属于该来源树的全部活 Binding，以及该来源树登记的设备/设备祖先撤出而受影响的活 Binding；其他 Claim 来源占用这些设备同样阻止 Closed。Preparing/ResourcesReady/Stopping 都未完成，只有原 Work 经过供应安全与迟到责任闭合后的 Released 才可消除阻挡。

查询只读、不推进清理、不等待远端、不分配；来源/设备身份由现有引用保留，SourceAdmin 及已接 Delivery 保活定位壳。祖先有效封口即可阻止子来源的新承诺，后续 Claim/Register/派生的 commit 仍同门复核，因此查询 Closed 后不会有迟到新责任加入。旧控制/观察壳退款不阻止 Closed。沿现有配置上界，最坏遍历所有设备的设备祖先及各登记/claimant 来源祖先，结构成本为设备数、设备深度、来源深度的乘积及有序表查找，不误写为单纯“设备数×来源数”。服务按实际管理角色使用已有控制预留；manager/init 复用现有期限/定时进行有界重查，Unknown/NotFound/Unavailable/超时不转换为 Closed，不新增来源 Watch 或成员账本。

| 占用/工作 | 准入与付款 | 释放条件 |
|---|---|---|
| 实例/来源/观察壳 | 发行/登记前由受托管理账户付记录和字节费用 | 来源/实例不再有活责任且已发行入口退休 |
| Binding/准备/停止任务 | Claim 提交前预留；表项数、名字与 metadata 总字节上界来自登记 | 安全 Released 后按任务/控制壳实际寿命退费 |
| Provision 硬件准备和供应会话 | provider 自身的真实账户；一单元最多一个 active 轮 | 效果/会话实际安全退出；不足返回明确拒绝 |
| Get/准备结果消息与句柄 | 普通 Sender 的运输由实际发送进程 Pool 付款；Once 的运输由 make_send_once 发行进程 Pool 付款，包括输入 scratch、payload/handles 和 Delivery 尾部；Receive 安装仍由接收进程 Pool 付款 | 各存储按真实寿命退款；领域费用另按授权账户，不冒充内核安装预付 |
| DMA backing、映射与隔离 | 持原 MemoryCharge/MetadataCharge，控制侧稳定 owner 保活 | 可信设备完成/静止后显式退休并完成必要同步 |
| 故障推进 | init/管理者装配时预留执行与控制容量，不借业务请求临时续费 | 原责任完成或系统终局；超时不提前退款 |

内核 Charge 强持实际 Pool，付款进程死亡不摧毁余额 owner。用户态 Charge 不能充当跨进程持久账本。容量按实际表布局与配置计算：单绑定管理预留覆盖 `Binding固定项 + 资源项上界×项成本 + 名字/metadata总字节上界 + 固定数量准备/停止/观察任务`；设备数、观察者、driver 激活数、消息在途各有单独额度。拒绝超界，不在承诺后无界扩容。清理链接在对象准入时备好，停止不因业务额度用尽而丢责任。
### 设计修订：Once 的一次运输付款授权

2026-10-25：Drivers 公开回复采用客户端签发的 Once。原实现把运输费用记入回复者 Pool，已入队未读消息及接收后仍持有的 Delivery 可在 Outbox 退出后继续收费，因此两个入口、请求预留和回复期限不能独立保证管理者 Native 额度。统一调整 MailboxOnceSender 对象付款语义：make_send_once 的发行者以自身进程 Pool 委托至多一次成功的 ABI 有界运输，唯一 payer 从 Once 已有 `_metadata.payer()` 取得，不增付款字段、Pool 或额度账本。普通 MailboxSender 仍由实际发送进程付款。

Send 先做不分配的长度检查，再取得普通 Sender 授权/use 或 Once 独占 Invocation，并核类型、WRITE、Pending 后选定 payer；输入复制、raw moves、验证/claim/rights 数组、payload/handles（含 Delivery 槽）及 Delivery 全由同一 payer 付实际费用，uaccess、分配和退款不在表锁或邮箱锁内。uaccess 仍访问当前发送进程，付款身份不改变发送者 PID/context 或授权。入队与 Pending→Sent 保持同一提交点；入队前 Quota/OOM/uaccess/非法移动/队满/过期等失败按原 Invocation 返还 Once 和移动能力，Close/Frozen 已撤返还资格时不复活槽。失败可重试，每次独占调用并退瞬态费用，不承诺总 CPU 预算。

费用随消息正文/数组和 Delivery 各自真实寿命保留；发行者死亡、Pool 句柄关闭或 Once 消散不提前退款。Receive 表安装仍付接收者账户。不预付最大 4KiB，不保证回复必成；发行者额度不足沿原 Outbox Quota→Abandoned，不能升级服务 fatal 或无限重试。Unsent 仅描述回复发送，业务可能已提交，调用者 Unknown 合同不变。当前保证为零 cap 的公开回复积压不累计占 srv_driver NativePool；业务对象、全局 allocator 硬字节隔离与全局分配必成不在此保证内。两入口、任务/来源和请求管理预留继续保留。

验证使用真实不同 Pool 的发行者/回复者，覆盖未读 ReplyPort、仅保留 Delivery 的接收尾部、最终原账退款、发行者 Quota 及失败返还；复用已有队满/uaccess/Close/Frozen 和普通 Sender 用例。实际 Drivers Observer 积压时核 Control 查询/停止仍可推进，同进程 Once 检查不能替代跨 Pool 证据。

| 故障 | 继续持有责任者与结果 |
|---|---|
| 叶驱动死亡 | srv_driver 停止该进程全部绑定；存活总线 provider 收束会话，安全后可重绑定 |
| DMA 控制器驱动死亡 | 不等待 stopped；控制侧封准入。首批无独立硬件隔离、不能证明停止时保留全部相关 pin，进入系统故障终局 |
| srv_driver 死亡 | init 封旧来源、停止其执行；srv_device/资源 owner 收束。旧绑定安全退出后才可按政策启新管理者，否则终局 |
| srv_device 死亡 | init 封系统新工作，停止受影响执行，保留实际根；不重建占用表，进入系统故障终局 |
| init 死亡或最终 reset 失败 | 按[系统终局合同](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)进入不可恢复停止；活跃 DMA 的 pin 不随句柄回收释放，等待外部复位 |

停止/故障路径不能形成同步互等，异步通知允许向上；正确性不依赖已失效子服务的回应。正常停止可以先协作、期限后升级，进程终态已知时直接走无配合路径。期限仅防停滞，不证明硬件已停止。

## 特权资源合同

设备实例/Binding 仍是用户态对象；内核只提供硬件访问与安全 owner。共享 ABI 角色由 shared、kernel、libkernel 同步演进。下列合同定义实际对象角色，当前接通范围与验证边界由实施续接记录拥有。

### MMIO

`MmioControl` 拥有经启动平台校验的物理 IO 区间、可派生范围与固定访问属性；`CreateUse(range,rights)` 产生一次绑定可撤销的 `MmioUse`，`Map(use,vm,offset,length)` 建立受其跟踪的映射。范围 checked、按页归一，不能将同一页的独占寄存器伪装成两个隔离区间，也不能把普通 RAM 重新标成 IO 授权。共享页交给真实 provider 或单一 owner。

根 control 与子 control/use 构成实际派生关系；根 Seal/Revoke 覆盖全部后代，单 use Revoke 只撤该 use 的全部地址空间映射。提交封闭新派生/Map，原映射撤销复用 VM 维护、远端 shootdown 与退休 owner；完成在所有相关翻译失效后报告。Close 只释放 handle，不代替 Revoke；先撤 MMIO 可能妨碍驱动静止硬件，顺序由 Provision 合同决定。

MMIO control 的最后 capability 消散会提交该节点的封口/撤销，use 最后 capability 消散提交自身映射撤销；均由预付内嵌 Work 推进，Close 返回不表示翻译已失效。仍存活的上级 control 保留观察和收尾权。正常 Provision 必须保有协作停止所需控制/use 到硬件静止后，不能靠最后 Drop 隐式安排停止顺序。此行为接入对象自身 on_last_capability，公共 Close 不认识设备类型。

首批映射属性依 QEMU virt 的平台 IO/PMA，不假设存在 Svpbmt；不映射为可执行，不制造普通 Rust 引用表示设备寄存器。libkernel 提供受范围限制的 volatile 访问；寄存器语义和必要 I/O/内存 fence 由驱动按固定规范负责。CPU fence 不是 DMA 完成或非一致缓存维护。

### IRQ

首批 PLIC 在内核，外部 SEIP 接入 trap。`IrqControl` 限定真实 source 与允许的目标 context，派生独立 `IrqUse`；同一真实 source 首批一个消费 owner，共享线配置明确拒绝，不把它误当两条独立 IRQ。目标 hart 在准入时固定，首批不提供迁移 API。

接口为 `IrqUse.Take() → token | Empty`、`Ack(token)`、查询/WaitSet READABLE；control 的 Seal 封新派生，Revoke 强制结束 use。token 包含 use 代次与本次中断序号，不回绕。别名共享同一消费状态，不产生第二个硬件订阅。

trap 每次最多 claim 一个 source，不分配、不做用户 RPC。内核记录 in-service，发布可读提示；gateway 在 complete 前阻止同源再次转交。Take 领取本次提示，驱动检查/服务实际设备后 Ack，内核核对 token 再 complete。IRQ 是提示，不承诺逐边沿无损计数。PLIC 的 claim、Ack 和撤销共用控制器短锁，在锁内完成寄存器与 source 状态转换，锁外通知既有等待机制；不得保有退出线程强引用或绕过 WaitSet 订阅生命周期。

现有 idle 在 SIE=0 下 WFI，醒来不必进入 trap，所以不能只增加 trap 的 SEIP 分支。接入时同时在 run 安全点、idle 睡前复检和醒后路径识别本 hart 外部 pending，调用同一个最多领取一源的推进原语；在内核持续维护时也保留这一服务机会。没有 runnable 工作可以睡眠不等于没有设备请求，PLIC/SEIE 保持可唤醒，已记录可读事件和等待提示按既有来源/维护队列阻止丢唤醒。每次领取后返回既有工作仲裁，不在安全点无界排空 IRQ。

撤销先封用户领取/Ack，若有 in-service 则在 enable 仍成立时 complete，再禁用 source；这几个步骤在同一 PLIC 短锁内完成。首批 source 仅在所选一个 context enable，所有 hart 的 claim 路径取得同一控制器锁，所以 complete→disable 窗口不会被另一 claim 插入，不靠循环排空电平源。真实控制权限全部消散时也由内核执行此有界封源步骤，不等待已死使用者 Ack。之后该 use 不再交付事件，旧 token 返回 Closed。PLIC pending 可能使新绑定看到一次陈旧提示，新驱动重新检查硬件；不能由旧 Ack 完成新轮。批量派生/等待者处理走既有维护预算。

### DMA

首批是显式的 `TrustedPhysical + Coherent` 模式，没有 IOMMU 隔离。取得物理 DMA 地址的控制器驱动属于系统可信基座；描述范围与 capability 不能阻止恶意驱动向硬件写任意物理地址。普通 USB/HID 子驱动只拿受限 transport，不取得 DMA 根。非一致或要求不可信 DMA 隔离的配置返回 Unsupported。

`DmaDomainControl` 持设备/父域、模式、映射登记与封口状态；派生 `DmaUse`。`Pin(memory,range,direction)` 校验内存权限、范围和冻结条件后创建 `DmaMapping`，先返回 mapping 能力；`ReadSegments(mapping,offset,capacity)` 才分段交付设备地址与长度。方向明确以设备为主体：DeviceReads 要求 READ，DeviceWrites 要求 WRITE，Bidirectional 要求两者。映射持独立 BackingUse；设备可写时还持 MemoryObject 写许可及归还目标，阻止 Seal 完成后仍有设备写入。它们与原 charge 的寿命独立于驱动 VM。范围 use/写许可由[内存领域](todo-2026-10-01-memory-redesign.md#设备-dma-接入合同)提供和强制，设备层不复制一套页占用或 W^X 状态。

**地址可能已交付的 mapping 由内核稳定 owner 持有，最后 Close、VM 退出、付款进程死亡和控制 handle 全部消失均不 unpin。** 控制消散封新准入，未完成映射进入有账的静态孤立保留；它不是可运行维护债务，不进入 ready 队列或等待一个永远不会来的完成。首批没有重新取得孤立控制的协议，只由外部复位结束这种保留。尚未交付地址的准备可以由原 owner 安全取消；正常可退休映射使用已有维护基座有界释放。此规则也覆盖 init 回收控制句柄的窗口，不能依赖全 hart 已停驻才保证 backing 不被复用。

正常 `Retire(mapping)` 可由创建它的受信 DmaUse 角色提交，或由其上级 control 提交；前者不能退休另一 use 的映射。因而 xHCI 可以在运行期间回收已完成的 transfer 缓冲，不必逐次调用 PCI provider。`Seal(domain)` 只阻止新 Pin/使用，不自动证明旧 DMA 消失。libkernel 的安全 wrapper 不把 Drop 包装成完成，提交可信物理 DMA 退休是显式、须论证的边界。根 control 可封全部派生，binding 子 control 只作用于自己的映射。无 IOMMU 时根不能凭空撤销物理写能力；缺少 quiescence 证据保持 pin，系统故障复位，不谎报 Revoke 已完成。

TrustedPhysical 下，已由装配准入为可信主控的驱动通过第二轮合同规定的原 Use 窄权限报告本轮静止，不能由 payload 的可信标志自授资格。PCI Provision 依据中央转交的单调消费静止事实、已封口 DmaDomain 的所有 mapping 退休完成和自身 bus-master/IRQ/MMIO 收束确认 Closed，不自行解释 xHCI 寄存器。DmaDomain 提供 Idle/终态观察，只有已封口且无活动映射才成立，不能将瞬时映射数为零当成域停止。驱动报告后永久关闭本轮提交；这是可信基座合同，不能推广为不可信驱动说 stopped 就安全。驱动死亡或报告缺失时没有替代证据，走保留与故障终局。

首批用固定长度 MemoryObject 页构造 ring/数据，CPU View 仍可 Unmap/降权，DMA 的独立 use 保证实际页不释放，设备写许可保证对象不能提前变为 Executable；不禁止与 DMA 安全无关的 VM 操作。CPU 与设备并发访问按驱动协议划分所有权并使用适当屏障。页级 pin 不代表整片物理连续，返回 SG 段。xHCI 小 ring/DCBAA/context 使用满足规范对齐与边界的页内区域，数据跨 64 KiB 按 TRB 分段。不另建 raw 虚拟地址转物理地址的旁路。

### 并发与有界推进

用户态各 backing 采用单一状态 owner；操作只在短提交段检查/改状态，不跨 await 持锁。远端完成带 DeviceId/epoch，在原任务上核对后消费。批量撤出/来源停止有持久游标和已付任务，每次推进额度由执行预算限定，调用者退出不丢任务。

内核资源 gate 只仲裁准入/封口和成员链接，不在其锁内进入 VM、Pool、WaitSet 或驱动协议。映射准入先预备容量，再在资源/VM 各自 gate 验证并提交，竞态失败回滚未发布项；撤销先封 gate、摘取有界成员，再锁外推进 VM/IRQ/DMA owner。DMA backing 使用责任以实际内存对象原语保持，停止状态不代替引用。地址空间退休仍由 VM 自身锁序与 shootdown 合同拥有。

大范围 Pin、SG 地址枚举、批量派生撤销不得在一个不可抢占 syscall 中扫完整集合。Pin 的准备/提交/回滚复用已提交工作 owner 和等待完成模式，设备地址在准入成功后才可交付；地址段按有界游标读取。同步快路径只处理有结构上界的固定步骤，退休复用原映射的预付记录，不临时分配逐页 Work。

IRQ 顶半部最多一次控制器 claim 和一项状态发布，驱动批量处理 event ring 受每次任务预算约束；未消费完则自保持 runnable，不能等待新的 IRQ 才继续。持续设备风暴仍占其已准入执行预算；未 Ack 的 source 不反复入内核。没有 IRQ/时间推进保证时进入既有运行故障终局，不能宣称公平性靠无限重试取得。

### VM 来源、发布与强制撤销

现有 [Views](../os/kernel/src/vm/views.rs) 强制持 `BackingUse`，[PhysicalExtent](../os/kernel/src/vm/backing.rs) 只拥有 Funded/Boot RAM。目标在原 VM 来源分发点统一演进为 `MappingOrigin` 和 `MappingUse`：前者在 `memory_space` 表达 Anonymous/Object/Mmio 的纯值身份、偏移和属性，后者在 kernel 分为 `Ram(BackingUse)` 与 `Mmio(MmioViewUse)`。MMIO 不进入 RAM SegmentMap、不产生 MemoryObject 写许可、不归还帧库存；几何预约、PTE 发布、fault、shootdown、空表清理和空间退出仍只有既有 VM 一条路径。原 BackingView/MapBacking 命名及全部消费者同步替换，不加 facade。

每次 MMIO Map 有非复用 MappingId 和稳定记录；View 切分保留原 MappingId，预备片段节点随实际存储付款。`MmioViewUse` 只持该次 Map 的片段责任和 checked IO 偏移；最后片段在翻译同步完成后交还责任。资源成员表持稳定 Map 记录；长期记录只保留 VM/Views 的弱定位，不能常驻 VmAccessLease 阻止地址空间退出。Weak 升级失败不证明已撤销，完成必须来自明确片段退休回执。

Map 责任阶段为 `Preparing → PublishGranted → Live → Revoking → Detached`，准备失败走 `RollingBack → Detached`；另有单向 stop_requested，VM 内部的 PTE/fence 阶段不复制到该记录。

1. 准备 Map 记录、VM Operation、资源链接、片段与撤销承载，在资源 gate 内登记 Preparing；尚未进入资源账本不得发布 PTE。
2. 资源锁外完成 VM 预约和准备。最终在资源 gate 内复核祖先准入，授予 PublishGranted；这是 Map 与 Seal/Revoke 的顺序点。
3. 解开资源锁，再进入 VM gate，执行既有最终检查、用户结果写回和必成提交。写回失败仍由原 Operation 回滚，真正结束后才 Detached。
4. Seal/Revoke 先于许可时拒绝发布并回滚；许可先行时，原 Map 可以完成发布，但 Revoke 必须等它回滚或完成后撤销。正在分步发布的 CommittedChange 不半途丢弃。
5. 主动 Unmap 和整空间退出沿原 VM 退休，归还同一片段责任；资源撤销不再释放第二次。Revoke 完成须无仍可发布的事务、无存续片段且相关翻译已失效。

控制侧 Revoke 不能调用会因业务配额或临时分配失败的普通 Unmap。每个 VM 出生时预付一个强制退休事务槽及 dependency 槽；每次 MMIO Map 预付排队节点，片段节点内含本 MappingId 的链接。该槽 FIFO 消费被撤销映射，每次冻结一个来源的修改准入，等已有预约完成/回滚，再退休一个完整片段。等待挂真实完成来源，不忙轮询；后续普通 Protect/切分不能抢占已冻结来源。

首批 MMIO 使用基础页 PTE，不使用大页：完整片段删除无需为拆大页临时分配表帧。强制槽使用固定容量的完整片段 Remove 入口及已有修改状态机，逐步发布、fence、退休和清表。切界准备失败可能留下等价片段，因此不能假定一次 Map 永远只对应一个 View。也不能按原 VA 范围全量 Unmap：用户已经释放并另映射的空洞不再属于该 MappingId。空间 Frozen 后不启动新单映射事务；已启动事务结束，余项交整空间退休，回执仍回原资源记录。

普通内核数据访问必须同时验证 ledger 来源为 RAM，再使用现有 translate 校验与 SUM 直访。uaccess、syscall 输入输出、IPC 数据缓冲、结果 cookie、初始栈和上下文不得把 MMIO 当普通内存。驱动 MMIO 只经专用 volatile 访问；MMIO 不可执行，也不能作为 DMA Pin 的 MemoryObject 来源。此检查不新增软件遍历页表的拷贝路径。

实施接线明确：上述来源判断消费 VM 的实物 View 来源账本，而非只读几何区域表。Protect/Unmap 分步更新可能暂时摘掉几何旧条目，此时原 PTE 仍可存在；实物 View 在发布 PTE 前登记，并保留到翻译同步后，能够覆盖该窗口。AnySpace 与 ProcessBody/退休 owner 共享同一个 Views，不新建并行来源表；内核数据访问按 ADDRESS_SPACE→VM_VIEWS 检查来源，再 translate 与 SUM 直访。结果槽的几何预约仍另行拒绝 MMIO。

### 资源树的准入与关闭

MMIO/IRQ/DMA 各真实根使用同一职责形状的树内同步：稳定 NodeId、不可变 parent、直接 child/member 链、单向封口状态、close_seq 和已付清理游标。辅助机制只拥有树和准入，不解释 PCI、Binding 或具体硬件停止；不新建通用供应图或通用资源事务服务。

Seal 只封新承诺并返回封口已接管，不把既有 MMIO 映射或 IRQ Take/Ack 立刻撤掉；这些使用仍可服务协作停止。MMIO/IRQ Revoke 包含 Seal 并推进实际撤销，完成另等；DMA 使用 Seal 加显式受信 Retire，没有伪装成硬件阻断的通用 Revoke。最后控制消散触发各自失效路径，不能把三类资源的物理完成条件合并成一个 Drop。

每根一把 RESOURCE_TREE 短锁，不嵌套父子锁。任意深祖先检查分步进行：先取 close_seq，沿稳定 parent 每步检查固定数量节点，最终同锁比较序号并链接新 child/member 或授予发布许可。任一祖先已封则拒绝；序号变化则本次提交前准备返回 Busy 并回滚，不在不可抢占内核中重试。每次 Seal/Revoke 在同锁下先封自身并推进 close_seq，再安排后台游标，故未遍历到的后代也不能取得新许可。同根无关分支关闭可能造成保守 Busy，这是显式准入失败，不改已提交工作的完成保证；用户态只在真实状态变化或明确管理请求后重试。序号耗尽封全根新准入，清理继续。

树节点的查询/观察权、派生权和停止权按实际 control/use 角色检查；`READ` 查询、`WAIT` 观察、`CREATE` 受限派生/Pin、`MANAGE` 对授权范围封口/退休，复制与运输继续要求既有 rights。MMIO 映射另核 use 的 READ/WRITE 上限且绝无 EXECUTE；DmaMapping 的地址读取权限只随所属 use/控制发行，不由有权读普通 MemoryObject 推导。ABI 的 kind/调用号由 shared 统一分配，不能以 payload type/tag 绕过实际对象角色。每份派生准入、查询结果安装和退役均使用真实 Pool，不把用户态领域预留等同于内核句柄安装预付。

清理每步处理一个直接 child 或 member；责任在安全完成前不从集合消失。子项完成只向父交一次回执，父在遍历结束、所有子项和成员完成后关闭。节点释放也分步，禁止递归 Drop 整棵树。DMA 孤立保留阻止域 Idle/完成，但不把静态保留加入 runnable 队列。

| 接缝 | 锁与移交 |
|---|---|
| 树准入、发布许可、封口和成员回执 | RESOURCE_TREE 单独短锁；锁内不分配、不调用 VM/Pool/WaitSet |
| 取得 RAM use | 既有源授权门 → BACKING；不持资源锁 |
| DMA 写许可取得/归还 | MEMORY_OBJECT；不持资源锁或 VM 锁 |
| VM 几何、View、PTE | ADDRESS_SPACE → VM_VIEWS → BACKING；MMIO 分支不取 BACKING |
| 资源通知 VM / VM 回传退休 | 原锁内移交稳定描述，解锁后进入另一 owner |
| PLIC claim/Ack/禁源 | 同一控制器短锁及内嵌 source 状态；不嵌套资源树/VM/WaitSet，锁外发送提示 |
| Work 发布、付款/退款与等待提示 | 沿既有 owner 路径，位于资源/PLIC 锁外 |

新 rank 的数值只在 `sync::ranks` 分配，以上允许边是施工依据。IRQ 派生准备先取得资源准入许可，再在 PLIC 锁下登记；Revoke 若已先行，必须等待该已登记准备撤回/禁源，不留“树已关闭而 PLIC 后启用”的遗漏窗口。

### DMA Pin 的提交与结果寿命

每次 Pin 的稳定记录拥有独立 BackingUse、可选 `(MemoryObjectCore, WritePermit)`、域链接、付款、完成源和退休 Work。先准备这些承载，再在有效源授权下取得 use；设备写方向通过 MemoryObject 原状态锁执行 `reserve_writes(1)`，不伪造 CPU View 或 replacement。取得 use 后遇 Seal 拒绝写许可，由原记录分步回滚；许可先成功则 Seal 等待其退休。

阶段为 `Preparing → ReadyUnexposed → MayBeExposed → Retiring → Retired`；未暴露分支可 `Cancelling → Cancelled`。Pin 完成仅安装 mapping 能力，不含物理地址。安装失败、未暴露 mapping 最后 Close 或域封口可以安全取消；提交后的任务不依赖 Pin 调用者存活。

ReadSegments 在任何设备地址可能写入用户缓冲之前，先将记录标为 MayBeExposed。输出 fault、部分写入、超时或未知结果均不能撤销这一事实；之后唯有显式受信 Retire 可以释放。即使调用者没有读到完整结果，也按可能已暴露处理。设备地址按原 use 的物理段和逻辑 offset 有界枚举，不预建全量 SG 数组；每次最多输出请求容量与用户缓冲允许范围中的较小值，checked 校验且返回下一 offset。不以段数或分页形状充当持久身份。

地址交付也经过资源树最终准入：同锁取得本次交付许可并标记 MayBeExposed，封口先行则不授许可；许可先行则属于域已跟踪的在途交付。每次 ReadSegments 持短期交付 lease，Retire 封闭新读取后等既有 lease 退出才释放 use，不能一边复制地址一边退款复用对应页。lease 只覆盖有界输出，不由用户持有或续租；退出/fault 由内核释放。TrustedPhysical 退休调用者还须保证已经取得的地址不会再次交给硬件，内核不能撤回用户缓存的物理地址。

Retire 在本 use 或上级 control 的实际权限下幂等提交，禁止后续地址读取；受信调用者保证设备不再访问该范围，经平台必要同步后归还写许可，再逐步释放 use，最后公布 Retired。其他 CPU/DMA use 仍持页时不要求整份 backing 归零。Domain Idle 只在已 Seal、无准备/回滚/退休工作、无未退休及孤立 mapping 时成立。资源记录不持 ProcessBody/VM 强借用，不阻止进程 DONE；原 Pool/charge 独立存活。

libkernel 将 TrustedPhysical Retire 作为显式 unsafe 边界，注明该范围全部硬件访问已完成及不会再提交；Drop 不调用它。DMA 共享页也不暴露可与设备并发的普通 Rust 引用：驱动以原始地址、明确的 CPU/设备所有权阶段和规范屏障访问，具体共享访问规则沿用[固定取证](ref-2026-09-shared-memory-access.md)。pin、volatile 和 CPU fence 各自不能证明硬件完成。

## 首批端到端平台路线

固定参考为 QEMU v11.1.0 virt、PLIC、coherent PCI DMA；首批实际环境选已核对差异的 **11.1.2**，见[版本取证](ref-2026-10-23-device-platform-contracts.md#本机-1112-与固定-1110-的适用性)。固定 `qemu-xhci,msi=off,msix=off`，运行期在 `xhci.0` 插拔 `usb-kbd`。just 路线显式关闭默认网络/多余 PCI 设备，列出唯一所需 PCI function，并核对 interrupt-map 使 xHCI INTx 独占；实际有共享时 PCI provider 拒绝，不因默认机器配置悄悄绕过首批限制。具体启动参数由实施时 just 路线拥有，DT 与机器配置必须一致；本次仅查询版本/属性，未运行客体。

### 平台事实与初始能力交付

`board` 的平台 admission 唯一拥有可授权的物理事实：完整消费 RAM/保留区排除，再校验受支持的 PCI host ECAM、CPU 侧 MMIO window、PLIC source/context 和 DMA 模式。只对已识别、启用且语义受支持的节点铸根；未知设备仍可保留在只读 DTB，不由一个 `reg` 自动获得物理授权。PLIC/CLINT、固件与内核保留区不交给用户态映射。PCI `ranges` 需区分 bus 地址与 CPU 地址，BAR 填 bus 地址，MMIO control 授 CPU 区间；不能默认两者相同。首批只消费 memory BAR，PIO/桥窗口/非恒等 DMA 翻译明确 Unsupported。

原 DTB reservation 移交 boot owner，直到有界 Bootstrap Work 将 `totalsize` 个有效字节复制到零填充、固定长度的资金化 MemoryObject；临时原页随后归还。用户只收到 READ/WAIT/DUPLICATE/TRANSIT/GRANT，不收到 WRITE/EXECUTE/MANAGE；不得暴露 DTB 首尾页的无关字节，也不保留指向已归还原页的引用。该保活与复制接点已进入本轮实现，实际状态见下方实施记录。

初始出生块保留现有通用 Handle 数组和 initfs payload；平台描述单独置于只读 MemoryObject。`shared/erhino_shared::startup::initial` 同步增加该描述对象的固定索引，其后为平台根句柄。描述对象含版本、有效长度、DTB 区间和定宽记录 `(resource_kind, dt_node_offset, handle_index)`；所有偏移、乘法、重复项及实际 Handle kind/rights 均校验。平台几何以相应 control 查询为准，记录不再复制一份可写资源账本。HANDLE_COUNT 的固定五项假设在 kernel/libprocess/srv_init 及测试中一并替换成基础项加实际记录数，不保留旧槽位兼容解析。

根对象、初始 cap 表容量、描述对象、DTB copy 及 boot 终态观察全部准备后才 Start init；失败沿原 boot failure 收尾，不发布部分可用平台。启动包的 initfs tar 仍是程序输入，kernel 不解析驱动声明。srv_init 从已有 tar 条目构造不可变的声明/映像配对来源，将所选程序转为只读映像能力交给 srv_driver；两者不通过运行期可变路径重新配对。完整初始描述只给 init，后继进程只得到对应只读描述及明确缩权的实际资源角色。

本轮确定平台描述 v1 使用 48 字节定宽小端头与 16 字节资源关联项，布局真值在 `shared/erhino_shared::platform`。DT 节点位置是相对 blob 的真实 BEGIN_NODE token 偏移，不能只按该位置的 u32 数值识别节点。第 i 条资源记录引用 `BASE_HANDLE_COUNT+i`，因此每个 root 槽只出现一次；同一 DT 节点允许多个同类资源（PCI host 的 ECAM 与 MMIO window），不能误用 `(kind,node)` 去重而丢失真实资源。实际 kind/rights 核验仍须随资源对象与平台装配接通。

### 启动与枚举

1. 内核仅解析启动/内存/中断所需平台事实；启动交付只读 DTB、经校验的 IO/IRQ/DMA 根和启动映像集给 init。DT 文本不是能力，用户态解析不能自铸任意物理授权。
2. init 先保有控制根、故障执行预算和管理 Job，再启动 srv_device/srv_driver，交付各自角色。管理基座和必需 PCI/xHCI 驱动映像可从 boot 来源取得，不依赖尚未启用的设备存储或 UART 日志。
3. 平台适配器登记 PCI host 和静态事实。总线语义的 DT 子树交给该总线解释，中央登记核心不解析 USB/SPI/I²C 私有地址。
4. srv_driver 绑定 PCI host，启动 drv_pci_generic。它取得限定 ECAM/IO 窗口、子资源派生和 interrupt-map 路由信息；枚举首批 bus 0，拒绝未支持桥拓扑。BAR 配置期间封相应解码，按设备 size/alignment 在已授窗口分配，准备完成前不启 bus mastering；范围/页重叠仍由资源层强制。
5. PCI provider 登记 xHCI function，并兑现它的 MMIO、INTx 和 DmaUse。srv_driver 匹配并启动 drv_usb_xhci，读取能力、建立 ring/context、按规范复位/初始化后启用控制器。不得凭函数指针或字符串跨越资源授权。
6. root hub 检测连接、复位端口、通过 xHCI slot/address 流程和 USB 标准请求读取/校验描述、选择配置。地址、配置和完整设备生命周期归 USB owner；在首批选定接口粒度后登记 boot keyboard 接口，不能同时允许外部整机驱动独立占用同一设备。
7. srv_driver 由 Watch 发现接口；boot 来源中已有支持声明但 HID worker 尚未运行，故现在启动，Claim→ResourcesReady→Activate→Ready。应用从设备 functions 或装配别名取得 HID 键盘入口，直接到 HID provider，不经管理者转发按键数据。

PCI 资源准备中的硬件步骤归 PCI Provision backing；xHCI 初始化后的 bus-master/关停由相关 control owner 协作。不以清 PCI bus-master 位单独证明所有既往写入已经结束；正常 xHCI 停止仍须 halt 和在途责任确认。控制器崩溃时首批不实现一个复制 xHCI 状态的救援驱动。

首批冻结供应资源名字与实际角色固定为：PCI host 取得 `ecam`、`mmio-window`、`irq-control`、`dma-control`；xHCI function 取得 `registers`、`interrupt`、`dma`、`pci-function`；HID 取得 `usb-interface`。控制角色仅交给被装配为总线/provider 的程序，不能从“名字相同”推断权限。typed 解码核对实际 kind/rights；PCI/USB 领域 metadata 由对应库解释，libdevice 不识别这些名字。

子登记是独立的实际授权，不是 Provision 供应项。srv_driver 在原轮 ResourcesReady 后凭 BindingControl 调用 `IssueRegistrar`，先将 Registrar/SourceAdmin 纳入同一激活记录，再在该轮统一 Activate 中显式交付 Registrar。PCI 和 xHCI/root-hub worker 按实际总线装配角色取得它；HID/普通叶驱动不取得。SourceAdmin 由 srv_driver 原记录保存，init 保留外层来源管理。重复 Activate 核对同一 Registrar endpoint/context 身份与 BindingUse，不比较本地 handle；额外别名关闭。IssueRegistrar 按第二轮合同固定原轮对子，重复调用只恢复同一身份；Unknown 可恢复该对子或停止原轮，不能另签替代来源。当前 Managed 对调用失败选择停止，不代表协议不具备幂等恢复能力。父轮停止即时阻止子新登记，父清理服务保留到子责任结束。

`pci-function` 是 PCI provider 拥有的本轮受限会话，允许幂等 `EnableBusMaster`，不开放任意 PCI config 写。Prepare 在关闭 MEMORY decode/BME 时探测并分配 BAR，安装地址后启 MEMORY decode，使 xHCI 可访问寄存器，但保持 BME 关闭。xHCI 验证 halt/reset/CNR、设置已 pin 的 ring/context 后，调用该会话启 BME，再按规范置 R/S 开始运行；PCI provider 在启用提交点复核本轮未 Stop。正常停止先由 xHCI 完成 halt，再由 PCI provider 清 BME/关 INTx 并撤资源。错误路径可提前清 BME 限制后续事务，但不能把它当成已有 DMA 安全结束。启用回复 Unknown 重试同一会话，不重新初始化或发行另一域。

### USB transport 与键盘功能

USB Provision 创建**已限定设备/接口及端点的会话**，表就绪前完成，不在 GetResource 时首次开 session。接口驱动可提交允许的 interface/endpoint 控制请求和 interrupt IN，不能自行 SET_ADDRESS、SET_CONFIGURATION、复位整条总线或访问其他接口。描述和实际会话在同一次资源回复中配对。

USB 请求通过所属协议提交 Setup/端点、方向、长度、数据 owner 和期限；host 校验接收范围并承担从提交至硬件完成/取消的责任。首批小控制/键盘报告走有界消息，host 自己的稳定 DMA 缓冲承接硬件，叶驱动不 pin 自己的任意页。请求取消可能与完成竞态，回复 Unknown 不释放 host 缓冲；session.Stop 阻止新请求，按 xHCI endpoint/slot 命令和完成事件收束旧请求。拔出只是物理事实，DMA transfer/event ring 不因设备消失立即释放。

首批每个接口会话至多一个未确认请求，`Transfer(seq, request)` 在预付任务/缓冲就绪后提交，再写 TD/doorbell；相同 seq 和输入重试返回原进度/结果，不发第二个 USB 事务，输入不同 Conflict。首 seq 的 Transfer 或 Cancel 还必须携带同一完成 Signaler，Query 不携能力；通知身份、提交和读取遵守[第二轮完成通知合同](#7-usb-完成通知与必要轮询分开)。完成结果保留至客户端提交下一 seq，下一 seq 同时确认前一结果；前一请求尚活跃则 Busy。`Cancel(seq)` 可先封尚未开始的当前轮，已开始则进入真实 endpoint 取消；旧 seq 不影响后续。取消 Unknown 时继续原 seq 或停止整个会话，不能丢 DMA 缓冲后重发。seq 不回绕，记录/结果字节和 host 缓冲均在接受前付款；session.Stop 负责最后未确认结果及在途责任。此限制来自首批串行 boot 控制/键盘请求需求，未来并行传输另以实际队列容量扩展，不预建通用事务日志。

接口控制请求首批只开放该接口的 HID SET_PROTOCOL 和所需只读描述/状态读取，逐项检查 recipient、wIndex、方向与长度；不把任意 class request 都放行。SET_ADDRESS/SET_CONFIGURATION/SET_INTERFACE、设备复位、其他接口和端点访问由 USB owner 拒绝或自行管理。共享 EP0 由 host 串行仲裁，取消某接口请求时保留其他接口已接受请求的 owner，不让叶驱动操作 endpoint ring。

Endpoint 取消遵循固定 xHCI 合同：发 Stop Endpoint，持续消费 Stopped Transfer Event 和 Command Completion，核定实际终点；需要跳过剩余 TD 时在 Stopped/Error 状态发 Set TR Dequeue Pointer，并等完成使旧缓存失效后再恢复或退役相关缓冲。Stop Endpoint 不释放硬件 endpoint 资源，不能只看命令已提交就回收 ring。整个设备移除时先把活动 endpoint 收到 Stopped/Idle、收完其完成事件和定向命令，再 Disable Slot；成功后清 DCBAA 槽并释放对应 context/ring。单接口停止而其他接口仍活跃时不得 Disable 整个 slot。原 TD/命令地址在相关完成事件消费前不复用，软件 epoch 不能修复硬件事件仍指旧地址的问题。

控制器正常 halt 前持续排空事件并停止 endpoint/command 工作；HCH 说明控制器已停，不表示 Event Ring 已空，也不把被中止请求改成成功。停机后残余完成由 host 按旧请求 owner 收束，随后才退 DMA。命令失败、Event Ring Full 导致无法证明收束或规范期限耗尽时保留相关 pin 并升级故障，不猜测硬件已经不再引用。ring 容量由页内 TRB 布局和实际内存预算计算，接受请求前预留 TD/command/event 消费状态，不能满后覆盖未消费项。

命令调度在业务额度之外保留一个串行清理命令槽；Stop Endpoint、Set TR Dequeue Pointer、Disable Slot 按依赖复用该槽，不能被业务提交占满。ring/context/事件消费与停止任务在控制器/接口准入时已付款并 pin；进入停止不再申请新 DMA backing 或要求已经封口的域重新 Pin。清理期间仍消费事件和 timer，单次处理额度有界；规范失败走既定故障终局。
HID 驱动校验 boot keyboard 描述并 SET_PROTOCOL(0)，消费 8 字节报告。功能协议提供当前按键集合、有效/不确定状态及单调报告序号，`Watch/Query/Ack` 报告变化；这是状态输入，允许合并提示，不承诺每次短暂按键边沿无损。需要无损事件流的消费者必须另行接入有容量/溢出合同的队列，首批不声称具备。ErrorRollOver 作为无效/不确定输入报告，不伪造全部释放；断开公布终态，不假造最后一帧键盘报告。LED 输出和通用 report descriptor 非首批承诺，Unsupported 明示。

正式验收客户端使用同一 libhid、真实 driver/provider 和设备发现；可新增 `test_device` 验收进程，不能为演示另造假 bus、测试专用驱动运行时或静态模拟热插来源。应用无需知道 xHCI 进程名，也无需扫描 Drivers。

### 正常停止与无配合故障

叶绑定：设备侧封新用/功能边 → USB session 停止准入并收束该接口在途请求 → host 确认 endpoint/slot 相应完成 → 撤销 session/use → Provision Closed → Binding Released。父绑定停止先封子登记/新 Claim，按预算停止后代；子停止仍可调用父的清理路径，父不能过早关闭其清理入口。

控制器正常停止：封新提交并停止子会话 → xHCI 完成/取消在途工作 → 清 R/S、等待 HCH（必要的 reset 仅按规范顺序）→ 确认不再设备访问后退休 DMA → IRQ complete/disable → MMIO 撤销 → Provision Closed。具体清理依赖图由 provider 实现，不能用全系统固定的“先撤 MMIO”序列代替。初始化中断亦根据已产生的硬件效果走同一 owner。

控制器死亡/无响应且没有独立隔离证据时：封资源新准入，停止执行，保留 DMA pin/控制/费用，进入系统故障终局；不报告 Released、不重分配其物理资源。共享影响超出单绑定时，真实 provider 决定受影响集合并停止相应使用，不能伪装成仅撤单个软件 cap。

## 场景核对与设计边界

| 场景/交错 | 方案中的闭合点 |
|---|---|
| 登记回复丢失，撤出先于登记到达 | registrar seq/highwater，重复同实例或 Closed，不复活旧登记 |
| 两个实现匹配、多个持权管理员 | 一个自动选择 owner、明确优先政策；占用提交互斥；管理员 Stop 通过设备状态被观察 |
| Claim 回复丢失、取消先到、旧请求晚到 | 原 epoch 重试/StopClaim，下一轮不可被旧命令修改 |
| 准备触碰硬件后失败、部分能力接收失败 | 先提交 Binding、单 Provision owner、残表不可见、原 Stop 责任不丢 |
| 父消失、同端口重新插入 | 新 DeviceId；来源封口阻止后代新 Claim；真实资源未释放仍 Busy |
| 驱动未启动时热插 | srv_driver Watch→boot 声明选择→启动→同一 Activate；不要求预启所有驱动 |
| 管理员停止期间迟到 Ready/Get | 状态提交检查拒绝新边/资源；旧会话由 provider 停止 |
| 持续插拔和分页 | 弱一致有限扫描 + seq 重扫，不因每次变化重启游标 |
| 叶驱动与 DMA 主控分别崩溃 | 前者由存活 host 收束；后者无隔离证据时系统终局保留 pin |
| srv_driver/srv_device/init 分别死亡 | 来源封口 / 外层根终局 / boot 终态观察与独立于 handle 的 pin owner |
| Registry 失联、名字仍可见 | 不阻塞物理停止，不承诺可用；重连显式取得新入口 |
| 额度不足、序号耗尽、无响应 | 承诺前拒绝；已承诺清理继续；不回绕、不以超时退款 |

通用模型已覆盖共享供应/父子组合的 owner，但首批不实现每种供应协议。选定路线的 DMA 安全保证限于受信控制器与一致内存；不能据 QEMU 通过宣称恶意 DMA 隔离或真实硬件缓存正确。root 控制域可封全部派生，不能凭软件撤销创造平台没有的硬件阻断能力。

## 部署期限与已确定的边界

当前部署期限各有实际owner和起算点，不是硬实时上界；用后续完整链证实当前工作集余量，不按递增数值宣称全链必在限内。重复Stop/查询不刷新阶段期限，硬件halt/reset/端点限值不变：

| 期限 | owner / 起算点 | 覆盖与到期责任 |
|---|---|---|
| 准备20s | Managed首次Claim前 | Claim→Ready→出生/激活→具名功能安装；依据已观测debug成本约10s及运输/调度余量，到期停止原轮 |
| 出生10s | ProcessTask开始loader前 | 同一次出生与认证boot；失败封复用、唤醒实际关联Run |
| 普通RPC5s | srv_driver每次调用 | 与阶段剩余期限取min；Unknown留原调用/轮次，不重签 |
| 中央Binding停止20s | srv_device Work首次Stop | provider Stop/受权停止报告/Closed及实际依赖释放；到期保留并升级，不伪造Released |
| Managed停止30s | 原Run进入Stopping | 观察中央释放及原订阅/RPC脱离，保留进入故障终局的余量 |
| 装配观察40s | init首次可使srv_driver退出的Stop前 | 同一绝对界传入Stop、Seal、QuerySourceStop、sleep上限与manager DONE；到期仍持原记录交故障入口 |
| native收集 | 原RootSupervisor既有监督策略 | 在上述DONE之后分别collect_process/collect_job；不冒充包含于40s，另记完成时间 |

### 发现与资源视图的约束

OAL 非根续页使用空 path，从 cursor 恢复原 view 并重新检查授权范围。封 ClaimSource 的同次提交覆盖功能隐藏及独立 Watch 序号；Watch 错误回复先核空正文和零槽，保留 Permission/Quota/Closed，不统一按成功回复的槽数校验。

Resources OAL 直接投影原 Binding 的 Frozen 表，Descriptor 与显式 Get 的实际能力配对；视图读取不交付能力、不产生第二份资源树。每次续页重新核同一 BindingRef、入口权限和整表可用性；Preparing 不暴露残表，Stop 后拒新视图读取，旧 context 不转向新轮。具体实现见 [设备实现](../notes/impls/device.md)。

### 条件性证据责任

| 项目 | 当前事实与 owner | 触发与完成标准 |
|---|---|---|
| Resources OAL 在 Stop / 旧 context 下的精确运输交错 | libdevice/srv_device 的资源视图；原记录有 host 页/名字边界及真实 Describe/Get 消费者，未单独取得这两个运输窗口的现场证据 | 视图或停止合同修改、实际反例出现，或已有正式设施可隔离该窗口时核对：停止后不再取得新视图，旧 context 不观察新轮，错误不伪造 Released；普通成功不替代该窗口。原边界见[投影取证](archived/ref-2026-10-27-device-delivery-history.md#resources-oal投影接线选择) |

本项承接原施工记录中已有的证据限制，不新增系统能力或重复已关闭的资源门。复合 USB 接口及其他范围外能力按延期表处理；历史时钟异常仅观察。

### 验收门与可观察判据

| 风险 / 独立合同 | 最小验证边界与必须观察的结果 |
|---|---|
| 登记与占用不复活 | libdevice 状态测试：撤出先于 Register、空闲 StopClaim 跳轮后合法 Prepare、同/异主体竞争、旧 epoch 晚到；只允许一个实际绑定及单向终态 |
| Unknown 与能力交付 | 真实 RPC/Delivery 消费者丢弃 Claim/Prepare/Get/Activate 回复并停止原轮；表未齐不可见，旧 Ready 不安装，旧角色不能控制新轮，任务与费用归原 owner |
| 映射撤销不误删或漏删 | 既有 VM 对象测试接真实 MmioUse：发布许可两侧 Revoke、分步发布中退出、部分 Unmap 后空洞映射 RAM、业务配额耗尽；Revoke 完成后无旧 IO 翻译，新 RAM 保留 |
| RAM/IO 来源边界 | 用真实 IO View 作为 syscall/IPC/结果输出地址应拒绝，普通 RAM 路径保留；MMIO X 权、RAM 伪装 IO、越页扩权均拒绝 |
| IRQ 正确与活性 | 真实 PLIC：全 idle 插入、Take 后旧 Ack 与 Revoke 交错、控制 Close、等待取消/线程退出、持续事件；不丢持续就绪、不重复 complete 新轮、普通任务仍推进 |
| DMA 安全与费用 | Pin 与 Seal 竞争、未暴露取消、ReadSegments 部分输出失败、最后控制 Close、重复 Retire、驱动 VM 退出；可能暴露地址后仍 pin，原 charge 不提前退，正常退休只退实际释放份额 |
| 真正按需热插 | 启动无 usb-kbd 且 HID worker 未运行；QMP 插入后出现新 DeviceId、HID 进程及 Ready；客户端经 FunctionAccess 取得功能，收到受控按下/释放状态；拔出后 session 终止且 Binding Released |
| 重插与复用 | 同端口重插获得新 DeviceId；向旧控制/会话发请求不能作用于新设备；两接口复用 HID worker 时停止其中一个不停止另一个 |
| 父子正常停止 | 有真实 interrupt IN 在途时封来源；子清理仍能使用父服务，随后主控 halt、DMA Retired、Provision Closed、Binding Released；最后 Job/DONE 与 SystemReset |
| 无配合终局 | 分别终止 HID、xHCI、srv_driver、srv_device、init；前者由存活 host 安全收束，其余按实际证据决定收束或有账隔离/故障终局，不能统一期待退款 |
| 根终局与 reset 错误 | 复用正式 GDB 注入设施在真实调用点返回 reset/IPI 错误，观察封准入、boot DONE 观察与停驻；只宣称实际观察的 hart，不把 request 当全 hart 屏障 |

实施新增 `just acceptance-device [mode]` 和对应 device workload：固定 QEMU virt/PLIC/唯一 xHCI 配置、私有 QMP socket、实际映像指纹及 DTB，外部控制器负责插拔/输入和日志判定，结束负责清理本次进程与 socket。此入口是现有验收设施的设备路线，不是测试专用服务或虚拟总线。功能输出应报告 DeviceId/BindingRef/序号和关闭结果，不能仅凭 “driver started” 判通过；主机 QMP 成功、客体功能就绪和客体收束分别判定。

设备路线通过后，执行 `just check`、受影响 host 测试及既有完整 `just acceptance`（已含 clippy，不重复跑）；DT、启动与安全点变化由其平台/故障组合回归。设备路线及唯一超时配置由Justfile和构建手册同步拥有，不能通过缩减 core 业务换取设备验收；实际执行状态见当前唯一收口表。精确注入若无法由正式设施隔离实现，保留具体未覆盖项并阻止相应强保证宣告，不用普通成功运行抵消。

## 延期能力的唯一登记

| 能力 | 现状与 owner | 触发、接通/删除条件与验证 |
|---|---|---|
| 文件系统驱动仓库、安装更新/来源认证 | 当前仅 boot 来源；驱动管理/服务装配 | 需要系统镜像之外安装驱动时设计；仍配对声明/映像，验证替换竞态与唯一选择；不删除 boot 必需来源 |
| 批量 Get/预装资源 | 当前逐名本地 Get；libdevice/libdriver | 实测初始化往返成为问题后优化，不增加事务/转移语义；比较时延与同样失败合同 |
| 多域恢复/管理者冷重启 | 当前中央单域、srv_device 死亡终局；装配/设备 | 实际需缩小故障范围且可证明全部硬件静止时，设计独立根/账本重建；新身份，不复活旧 use |
| IOMMU、非一致 DMA | 当前 Unsupported；资源/平台 owner | 目标平台需要时补映射隔离、缓存/翻译同步、失效与崩溃证据；此前不能启用不可信 DMA |
| 动态 reserved-memory、memory-region、reusable | 首批路线不消费；平台保留内存计划 | 平台或设备实际引用前完成独立合同/实施/回收验收；不把未知语义压成永久排除或普通 DMA 页 |
| 外置 hub、更多总线/共享 IRQ、clock/reset/pinctrl | 当前由领域扩展承接；不预建通用供应图 | 出现真实消费者时按同一 Provision owner 组合，验证共享影响与父子退出，不能只补描述不补授权 |
| USB 打印、通用 HID/无损输入流 | 当前 boot keyboard 状态功能；对应功能协议 | 需要真实应用能力时定义领域会话、容量、完成/取消和实际消费者；设备/驱动目录结构不重做 |
| 仿射资源 | 目录当前仅可别名角色；libdevice 与资源 owner | 第一项必须转移且不能别名的真实资源出现时设计，不用 Taken 位提前承担不存在的寿命 |

## 当前进度与接力

当前交付与独立 Review 从[提交与调查接力](#第二轮提交与调查接力)接手；已关闭门见下表，未覆盖的独立窗口见条件性证据责任。首轮施工与多轮调试过程已归入[交付历史](archived/ref-2026-10-27-device-delivery-history.md)，不从旧断点恢复施工。

### 当前唯一收口表（2026-10-26）

本表拥有当前剩余门与证据分类；[交付历史](archived/ref-2026-10-27-device-delivery-history.md)保留各轮施工/失败事实与路径，不再从历史“尚未接入”重复产生施工任务。原验收合同仍由正文拥有。待证项不是已证缺陷，不能用常规成功或另一个操作的证据替代。

**独立异常记录：实际平台时钟失败，根因未定，仅观察。** IRQ第三轮platform clock failed及原143主动收割保留，旧首因值不可恢复。[时钟复查](#独立时钟异常复查2026-10-27)仍未复现；按用户决定不复发就不继续调查，统一从 [KNOWN_ISSUES 观察项](../KNOWN_ISSUES.md#历史偶发异常仅观察)接手。原证据不改写为修复，DMA＋IPI 已补证关闭，不把时钟异常记为该机制失败。

**审查代码责任。** 第二轮目标、一致性重构及所选验证保留原交付事实，见[收敛档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)。最新整体 Review 的 G1–G5 与 C1 已按[统一报告](archived/review-2026-10-28-device-driver-overall.md)完成修复及复核，本表不复制 findings；本轮精确动态覆盖限制由[修复后接续](#整体审查修复后的接续2026-10-28)拥有。全 hart idle 与 DMA＋IPI 门保持下表原证据；未归因异常仍仅观察。

**已通过，可复用。** 下表的轮次名称、简称、旧源码行号及未附链接的证据均从[完整历史记录](archived/ref-2026-10-27-device-delivery-history.md)查阅，不构成当前待办。

| 已关闭门 | 原证据入口与限制 |
|---|---|
| DMA保留与IPI失败组合 | [本次第四轮完整证据](../artifacts/dma-ipi-investigation/20261027T113617Z/run-4-summary.json)、[实际指纹](../artifacts/dma-ipi-retention/20261027T115226.316524Z-91149/evidence.json)，`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just virt-dma-ipi-retention`退出0。hart1→hart0原request中的FAILED由原wrapper/caller返回并消费，恢复全部hart调度后hart2→hart0原STOPPED广播返回并消费Ok(0)，四hart进入park；同Mapping9/10、各自同Backing范围0..2/use/write/Idle跨五点不变。原selector/WFI及保留判据不放宽，工具窗口与栈读取修正、历史失败及边界见[调查记录](archived/ref-2026-10-27-device-delivery-history.md#dmaipi-组合调查与关闭2026-10-27)。不排除timer/其他唤醒，不证明硬件DMA静止或永久IPI失败活性 |
| 全 hart idle 与真实 IRQ 唤醒（第二轮结构） | `THROTTLE=100 just acceptance-irq-idle` 退出 0，[入口](../artifacts/run/20261027-142222-48689-acceptance-irq-idle.log)、[原共同快照与 IRQ 证据](../artifacts/device-function/20261027T062229.574930Z-49086/evidence.json)。第六次共同取样四 hart 同时 idle_mask=15、current=0、ready_len=0、remote_head=0、timer_pending=false，maintenance_ready=0，PC 位于原 post-check/WFI 合格点；保留未来 timer。QMP 插入前 source33/gen5 无 pending，插入后同源 pending 且 hart0 SIP.SEIP，随后同源 claim；timer_coincident=false。两轮真实 Take/Ack、旧 token Closed/新代拒旧 token 与完整 core 收口通过。旧八轮事实保留；不声称已证明旧失败的唯一根因。已删除的 PCI 周期 Query 专属探针不再作为前置。  统一root后再次退出0：[新版入口](../artifacts/run/20261027-161721-99758-acceptance-irq-idle.log)、[共同快照](../artifacts/device-function/20261027T081730.356125Z-245/evidence.json)，verified=true；仍为四hart current=0/idle_mask=15，ready/remote/timer_pending/maintenance为空，同source33/gen5由pending=false到真实插入后pending/claim，无timer共现，完整core通过。 |
| 平台供应、真实PCI/USB/HID、单键盘热插与多Binding进程复用 | 上文正常release、observer-reuse-third及原A/B/C证据；直接端口多设备不代表复合USB接口 |
| 运行期监督/最早Start/Stopping、HID及真实主控死亡、reset拒绝保留 | 各角色原fault与三阶段DMA日志；不宣称死亡后硬件静止或全额退款 |
| Resources/Drivers公开投影与管理余量、Once发行者运输付款 | once-drivers-release-first、真实跨Pool/未读/Delivery/发行者死亡及积压下Control；非全局堆硬隔离 |
| 候选有限尝试、负候选交接与描述符未就绪取消、正向Retry | candidate-negative-sixth、candidate-cancel-second、retry-positive-first；不代表两个有效HID实现，不替代本表Retry竞态 |
| 六项真实回复缺失处置与旧function别名 | 各真实DeadlineExpired/Sent错误及Master同context/ref/绝对期限恢复；function-session-second；不外推所有丢包位置或全部精确退款 |
| MMIO许可两侧Revoke、IRQ在途撤销/跨generation | mmio-window-third、irq-retirement-first；前者活VM完整footprint与独立Pool零，后者新use已taken时旧Ack零complete，均不代表其他待证窗口 |
| DMA四指定竞争与独立账、双Pin同Backing逐份退款、Quota拒绝/同Use恢复 | dma-races-second、dma-shared-first、dma-quota-first及各原客体/指纹；CPU后142尾部与最后真实owner关闭后两维零分别记，不泛化OOM/IPI |
| 操作者进程清理 | 七项真实子进程反例及随后三条DMA路线实际无残留；不抵消原第八轮泄漏失败 |
| MMIO普通额度满时强制槽仍可用 | [host-os原测试输出](../artifacts/device-closing/host-os.log)263、`planner.rs:2011–2050`实际占满配置8个普通事务后仍准入0key、完成及全回滚；原+1预付、内核唯一Limits16→transaction_capacity17/依赖17、Forced出生plan预付及同一reserve_forced_remove接线已亲核。未实测QEMU同时满16槽，按独立纯容量合同不新增必过压力场景；不覆盖足迹冲突/所有资金失败/Frozen/IPI |
| MMIO真实来源/uaccess与空洞重用 | [function-session-second客体104/414](../artifacts/qemu-acceptance-20261025-205432-57658.log)：真实ECAM身份、IO结果缓冲MemoryNotAccessible、EXEC RightsDenied、排他范围AddressConflict、空洞RAM在Revoke后保值、原IO VA重新准入及正常收口。纯容量host不替代这些硬件IO事实 |
| MMIO非空未开始队列跨Frozen交接 | [第十一轮精确窗口](../artifacts/mmio-forced/20261027T011409.591234Z-73035/evidence.json)：原Queued PID26/scope30064771098、maps5/6、held Idle/activeNone/FIFO[5]，原Revoke成功后真实Kill、access_open=false且held phase未变；[客体409–411](../artifacts/failed-acceptance-20261027-091406-73023.log)同PID/scope的members0/Killed、CPU后82槽及Control关闭后Pool全零。结合forced.rs295–317的唯一Frozen交接/退出租约路径审定闭合，不要求旧VM再被advance时仍活；第十一轮整体exit1不改，后样本不归给旧VM。该轮未覆盖的Publishing及整轮收口见下一行 |
| MMIO已提交Publishing跨Frozen及两Map完整收口 | [第十二轮实际窗口](../artifacts/mmio-forced/20261027T014534.141674Z-94993/evidence.json)、[完整core客体](../artifacts/qemu-acceptance-20261027-094529-94982.log)、[身份/清理](../artifacts/device-closing/mmio-forced-twelfth-final-version.json)：PID27/scope34359738394、maps7/8、原active7/同Execution Publishing的phase与active原字节跨Revoke成功→真实Kill Frozen保持，既有VM identity27/access_open=false；放行后417–419同PID/scope members0/Killed、Pool20565 CPU后82槽/Control后两维零，427域收集/439显式shutdown，整轮exit0。结合forced.rs278–317先推进已提交engine再处理Frozen的唯一责任链闭合，不写成逐指令观察。Queued附带实际Idle/FIFO[5]/identity26窗口及同账收口；原第十一轮exit1不改，两组地址/satp实际复用而identity不同，不据此反推旧轮后样本 |
| Seal提示未达不授予新Claim | [定向host](../artifacts/device-closing/retry-seal-host.log)exit0，在实际Registry owner的Seal/SourceStop Closed后，新claim及late commit均Closed；静态亲核Retry只删本地runs/置dirty、不写来源或authority，实际srv_device Claim→Work提交同actor必经Registry.claim/commit_claim source门。未实测RPC提示延迟，不将其当新的必过门；提示不承担授权，成功Retry只重置纯记录 |
| Retry迟到记录保护 | [第四轮原观察](../artifacts/device-retry/20261026T123128.086138Z-73093/evidence.json)/[完整正常收口](../artifacts/qemu-acceptance-20261026-203038-73077.log)：两begin同原Rc/ref/impl、先352 reset、迟到353 current=None/NotFound、同Tid7/registry槽2 Live→Free及exit0；直接运行仅removed-record。监督亲核原强Rc防ABA/独立ref-impl快照、commit同actor身份→settled→字段→remove、Manager唯一replacement/rollback、新Rc准入和Outbox结束跳过commit，结合既有fallback证据审定闭合原717合同。其余拒绝分支为源码责任证明，不写成直接运行通过，不新增每枝QEMU |
| root构造与Building整批安装指定分配失败 | root-funding-first的两原观察/指纹/账见[首轮历史](archived/ref-2026-10-27-device-delivery-history.md#未发布dma根的首轮历史证据)；局部构造/预约付款回退及无cap交付、Gate Failed/四park、安装后五份Node Work保持Queued成立。不是五根全额退款；retained-entry特定Root Work证据见下一行 |
| DMA未发布root retained-entry失败局部回滚（第二轮替代） | `THROTTLE=100 just virt-platform-funding dma-retained`退出0，[入口](../artifacts/run/20261027-135625-37995-virt-platform-funding.log)、[原账证据](../artifacts/boot-failure/20261027T055629.589219Z-38357/platform-dma-retained-evidence.json)：verified/root_birth_refunded=true，真实OutOfMemory，原Pool的pages allocated=4、metadata allocated=63936构造前后相同，未发布零成员根不排正常Root Work。四hart Gate Failed时仅原先三MMIO/一IRQ owner待清理。首轮Queued专属证据保留于交付历史记录，不再要求已删除路径；不关闭已暴露DMA＋IPI或整批全额退款。 |
| DMA晚期安装指定分配失败 | [dma-install-second指纹/原观察](../artifacts/dma-install-failure/20261026T071054.289613Z-82520/evidence.json)，完整ReadyUnexposed后指定releases Buffer16B失败，先前handles Buffer已实付；实际OutOfMemory/无cap无暴露，原Work members/write归零、独立Pool最后全零及core/shutdown通过。不称物理堆耗尽，acquire_step非空前缀OOM本不可达 |
| IRQ直接已park退出与含Line成员Set随宿主退出 | [IRQ第六轮原观察](../artifacts/irq-exit/20261026T100635.591971Z-41077/evidence.json)PID28/source32/gen6 Waiting/epoch2、原Line loan/节点链；PID29/gen7实际Running但cap slot4/gen1/rights69持WaitSet KOID19019，唯一Member实际挂原Line。两者Kill后各同generation原Line空链/Idle/强根回原两owner，[客体405–409](../artifacts/qemu-acceptance-20261026-180630-41066.log)Pool19003/19011两维全零及scopeDONE，整轮core/shutdown通过。第四/第五轮WaitOne最小证据仍复用，旧整体失败不改写；不代表Set宿主Waiting/所有等待形态或强公平 |
| IRQ持续事件有界推进/普通任务服务机会 | 监督已亲读原248/250/276/455及真实接线，按[IRQ持续事件证据映射](archived/ref-2026-10-27-device-delivery-history.md#本次离线收敛root有限诊断与irq持续事件证据映射)的结构+host+集成组合审定关闭独立差额：单claim/单Line节点锁外Delivery、SEIP Requeue→原安全点→M单advance_one→有限机会rotor、正式xHCI 16事件/more→Runnable，加PLIC/signal/rotor既有host和真实READABLE业务进展/连续Take-Ack/退出证据。7相关源文件与既有验证版本一致；未执行任意高频/同hart指定比例/无限时长压力，不宣称其运行通过。原持续事件合同保留，全idle共同快照与外部唤醒仍单列 |

当前收口方式：第二轮设备控制链、root 持续监督及生命周期增量保持原交付事实，DMA＋IPI 组合与全 hart idle 门均已关闭。[整体 Review](archived/review-2026-10-28-device-driver-overall.md)的缺陷修复与机制收敛已完成，后续按[修复后接续](#整体审查修复后的接续2026-10-28)的具体触发进入，不恢复已关闭门或历史施工。未归因异常仅观察，历史失败保留原样。

### 独立时钟异常复查（2026-10-27）

用户本轮重新安排时钟调查；基线 `956685b`，接手工作树干净。本轮核原始日志、内核采样/换算/停止链、固定规范与 QEMU 源码，并运行一次已有首因路线；没有修改产品、消费者或调查工具。**结果仍为未复现、根因未定，不关闭异常。**

原失败客体 `failed-acceptance-20261026-161223-12528.log:265` 确有 `Runtime stopped: platform clock failed; doorbell failure mask 0x0`，其前最后时间为启动后约 144.4694 秒；原 IRQ Entry 尚未认证到 init，Query/Kill 点未启用。本机 `artifacts/**/*.log` 检索只找到这一份实际时钟终局日志。原元数据记录施工基线 `549a9e7` 和 ELF `7773d62e…`，不是当时干净提交的复现；当前完整 ELF 不同，相关 clock/sbi/shared time 源码相对首轮提交 `f3337eb` 无差分。旧首次 prior/raw/geometry 和写前 failed 未保存，仍不可恢复。

源码核定：

- `ClockState::read` 仅在原始样本比采样前取得的 prior 小至少两 tick，或 `ClockGeometry::snapshot` 转换失败时写 failed。RPC 期限到期、非法 deadline、单次 `after_ns` 越界不会写平台 failed，不能用普通业务超时解释该终局。selftest 持独立实例。
- 已采旧样本延迟到另一 hart 发布之后才继续，不会因此误触回退判断：比较的是采样前保存的 prior，后续用 fetch_max 发布。`sbi::read_time` 保留编译器内存屏障和 `fence rw,i; rdtime; fence i,rw`，对应固定 Zicsr「CSR Access Ordering」；Zicntr 允许的跨 hart 一 tick 差异已纳入判断。没有发现应补 fence 或放宽阈值的依据。
- 换算用 u128。正常 10 MHz 计数的 u64 纳秒范围约 584 年，原约 144 秒的正常读数远未接近边界；这只排除正常时间自然耗尽，不能排除异常 raw、geometry 或内存损坏。

QEMU 固定源码核定（本机版本 11.1.2；本地参考仓库 tag 对象 `ceaea6cb3e2a7d966f9ca97c7ebb5b88cd764dd0`）：

- [`target/riscv/tcg/csr.c::read_time`](https://github.com/qemu/qemu/blob/v11.1.2/target/riscv/tcg/csr.c#L1678) → [`hw/intc/riscv_aclint.c::cpu_riscv_read_rtc`](https://github.com/qemu/qemu/blob/v11.1.2/hw/intc/riscv_aclint.c#L51) → QEMU_CLOCK_VIRTUAL → 无 icount 的 MTTCG 后端 `cpu_get_clock`。ACLINT 另有 time_delta，reset/mtime 写入可修改它；不能宣称它永远为零，旧现场也未取其值。
- [`system/cpu-timers.c`](https://github.com/qemu/qemu/blob/v11.1.2/system/cpu-timers.c#L74) 对 offset/enabled 成对更新用 seqlock，同一读侧重试拒绝跨更新的快照；[`system/cpus.c`](https://github.com/qemu/qemu/blob/v11.1.2/system/cpus.c#L296) 在 VM stop 时冻结，start 时补偿后续走。在宿主单调时钟成立、没有额外 mtime/reset 写入的前提下，未找到正常暂停/恢复导致因果读数回退的源码路径。这不是对旧运行现场或 QEMU 全部实现的排除证明。
- [`get_clock`](https://github.com/qemu/qemu/blob/v11.1.2/include/qemu/timer.h#L838) 在启动探测成功时使用 CLOCK_MONOTONIC，否则回退 gettimeofday；本轮未采运行期 use_rt_clock，不把正常 macOS 支持推作旧现场证据。SIGSTOP/SIGCONT 不调用上述 VM stop/start，正常宿主时钟继续前进；`tools/qemu-throttle.sh:8` 所称客体感知为“时间暂停”不适用于这条无 icount 路径。这项注释偏差不证明它造成旧时钟故障，本轮未改工具。

运行证据：`just investigate-clock` 退出 0，耗时约 117.36 秒；[入口](../artifacts/run/20261027-201225-92604-investigate-clock.log)、[外层结果](../artifacts/clock-investigation/20261027T121225Z/result.json)、[源码身份](../artifacts/clock-investigation/20261027T121225Z/source-identity.json)、[运行状态及 ELF 指纹](../artifacts/clock-first-cause/20261027T121232.006197Z-92988/run.json)、[探针状态](../artifacts/clock-first-cause/20261027T121232.006197Z-92988/first-cause.json)。实际 rustc `1.101.0-nightly (19833af72 2026-10-20)`，kernel `f084cf97906c26e31ca61e8822bb4070d315254139373fc151211c34ee3d081b`。THROTTLE100、4 hart、无 icount、冷启动 -S、只装两个错误路径点；未恢复原用户 Entry 断点及 50% 节流。`run.status=not-reproduced`、`probe.status=not-triggered`；[客体](../artifacts/clock-first-cause/20261027T121232.006197Z-92988/qemu.log) 90 行时钟自检、395/397 行原两个 IRQ 消费者退款/DONE、417 行 Shutdown accepted。本次 QEMU/GDB/runner 已全部退出，未连续重跑；它不是旧配置的单变量对照，也不是时钟修复验证。

**本轮最终处置。** 用户确认不复发就不继续调查；本项与 W05-04 一并登记为 [KNOWN_ISSUES 历史偶发异常](../KNOWN_ISSUES.md#历史偶发异常仅观察)，不阻塞当前工作，不再安排定向对照、重复运行或预防性代码修改。该入口拥有观察状态与重开条件，本篇继续保存原始证据和触发后的调查责任；未复现不等于修复。

以下仅作复发后的取证参考，不是待执行任务：当前缺口是首次失败分支和原样本，再次出现终局时先保存原 prior/raw/geometry、写前 failed、hart/调用栈及环境，再区分 QEMU time_delta/宿主时钟异常与内核状态写入。确需恢复调试交错时，在同一 GDB 保留两个首因点并记录用户 Entry 的地址/PID过滤命中，保持 THROTTLE100；节流对照须单独确定，不能同时改变多个变量后单项归因。增量探针和脱离调试器的一次性首因记录均未实施，不改变失败政策、不钳制严重回退，也不在持锁采样路径临时增加日志。

## 本轮修复设计（2026-10-28）

整体报告 G1–G5 与 C1 已实施，修复与复核事实由该报告保存。本轮沿用 actor 单一所有权，不新增内核锁、硬件能力或通用运行体。

- **就绪提交与停止**：Work 先完成本地 Frozen 候选，再由 Registry 同 actor 检查祖先/来源并提交 ResourcesReady；只有成功才发表资源表。此提交点的 Closed 进入原 Work.stop，候选别名释放、provider 继续持实际供应责任；其他错误仍按原故障政策处理。停止仍清 next_get、固定首次期限，不依赖 maintenance 抢先运行。
- **发现退场**：Manager 在领取发现 RPC 前先兑现 world.stopping；取消原 txid 并等 Dispatcher 真实完成，迟到结果只释放，不再枚举/选择。尚未获停止提示时，形状合法的 Enumerate Closed 表示原来源已封，进入同一单调停止。注销通知来源后释放 Scope Watch；所有 Managed/Process 仍按原 Released/Detach/收集合同结束。
- **稳定终态 Watch**：Binding Watch 出生输入使用宿主从已认证 Control/Use 读取的 Snapshot，不能用裸 BindingRef 当授权证明。活动快照在 Registry 同 actor 复核；Released 快照配对原角色付款者，建立从出生即终态的订阅，之后不随新 epoch 更新。终态出生不再依赖 source/device 表项，不延长 Registry 供应 payload 的寿命；原 Work 的业务 Box、供应 sender、Frozen 和身份引用仍按 Released/来源注销释放。Watch 自身持实际 Charge，宿主角色持 Shared 回执及 Lifetime，最后关闭事件唤醒原尾部 owner。此处以终态回执的独立寿命替代延长设备记录的候选，避免简化观察却重新保活供应。
- **USB 停止**：先收在途定向命令；无 slot 直接退休页，有未结束/未领取 TD 时针对该 TD 的真实 dci 停止与 SetDequeue；没有 TD 的已空闲端点集合直接 Disable Slot，不从 keyboard 描述符推导实际配置状态。保留命令完成检查、停止事件与 DMA 静止前提。
- **页退休**：Page 的显式退休按 DMA DONE 后解除 CPU 映射，失败保留原 Page 并传播给原 Engine；Device/Controller 只有每页显式完成后才进入 Closed。Disable Slot 后将仍需交付的 USB 完成结果从 TD/页消费为小型结果回执，允许页先退休、结果后领取；不能在页解除后回读 DMA 缓冲。构造期 Drop 仍可释放 CPU 映射，但清理失败必须终止进程交 VM 回收，不吞错。

G1/G3 的 Registry 纯逻辑回归依据来源封口、稳定原轮和旧 Ack 合同；G2/G4/G5 复用真实停止、热插和退款消费者，不另造模拟服务。具体精确交错如需扩展既有外部取证工具，保持正式 ABI、原角色与安全判据。

## 整体审查修复后的接续（2026-10-28）

本轮 G1–G5、C1、D1 与组合验证发现的 bootstrap 就绪通知接点已完成修复及交付前复核，详细依据、静态/host/QEMU 结果和原始失败保存在[整体报告](archived/review-2026-10-28-device-driver-overall.md)。代码与说明已统一提交为 `e23afdbfe0924058ce498a9b8994d70b10f451e3`，基线为 `d19fdfe4d9f655d20cc8f46d8d0cb570b879f05f`；[固定交付独立 Review](todo-2026-10-28-device-driver-fixes-review.md)仅作可选登记、尚未执行，不阻碍后续工作；如选择执行，范围覆盖本轮所有修复、拥有输入公共 loader、就绪通知、准备窗口及实际消费者。提交整理核 18 个产品文件与交付指纹一致，没有重复已通过的编译或 QEMU；仅本地提交，未 push。归档报告中的未提交状态保留归档时事实。当前没有剩余产品修复或半重构迁移，不重做已交付的 root/worker 分步装载。

Managed 从 Claim 至功能安装采用一次绝对 40s 部署准备窗口：本轮分步装载允许其他 actor 职责持续推进，实际 PCI 正常链已接近原 20s，因此留同量调度余量。它不是性能保证，回复不刷新期限；worker 出生/bootstrap 10s、单次 RPC 5s、停止与硬件期限保持原值。bootstrap entry 的认证提交立即唤醒关联 Managed，通知来源注销独立完成。

以下是验证覆盖缺口，不是已证实仍存在的代码缺陷。普通成功路线不替代这些窗口；保留至对应责任链再次修改、出现相同反例，或既有外部工具支持安全触发时补证。触发后在本记录追加证据，顺序为先核原身份/owner，再施加交错或失败，最后检查真实终态和原账；不另建测试专用服务。

| 覆盖范围 | owner 与当前依据 | 补证判据及关闭条件 |
|---|---|---|
| G1/G2：准备末尾封来源、Manager Enumerate 在途停止 | libdevice Work 与 srv_driver Manager 维护者；就绪提交点 Closed→原 Stop、发现退场先于旧调用消费的源码论证，加 Registry 回归与真实停止组合 | 在真实原请求窗口封来源/Stop，无迟到发表或管理者故障；旧调用退休、Binding Released、进程收集与原账结束 |
| G4：SetConfig/Configure 两个无 TD 间隙拔出 | libusb xHCI 维护者；不再从描述符推导 dci，候选取消及实际热插通过 | 原 slot/DMA/CPU 映射退休，主控及另一连接保持活跃；两个窗口分别有实际阶段证据 |
| G5：CPU Unmap 提交前分配失败 | libusb Page/Engine 维护者；错误保留 Page，显式退休未完成不发布 Closed，真实正常退出通过 | 真实失败保持原映射 owner，重试完成或明确进程终局承担原账；不能以 DMA DONE 代替 CPU 完成 |
| C1：Grant 前后、DONE 安装中和运行后的取消交错 | libprocess 与 srv_driver ProcessTask 维护者；类型与提交点检查、拥有输入 host、真实候选取消与公共 root loading 故障路线 | 活跃 Binding 的 DONE/Stop 在另一 worker 装载期间仍推进；各窗口只有一个 Control/Collector owner，来源注销先于最终关闭，原 Pool 退款。root loading 路线不替代拥有输入逐窗取消 |

### 候选出生异常（仅观察）

2026-10-28 的一次 candidates 运行中，HID process 5 未记录 Start 就以 Unavailable 结束原 Run；PID 16 随后被收集、原 Pool 两维归零，供应 Closed，整轮最终退出 124。原始入口和客体见[整体报告的现场](archived/review-2026-10-28-device-driver-overall.md#候选路线的一次未归因出生异常)。旧现场缺少首次失败分支、阶段和期限，根因未定。

已在原出生 owner 增加 Preparation 阶段/GrantOutcome、出生 deadline 及 Start 错误诊断；同一路线完整重跑通过，worker 准入至 bootstrap 约 1.2–2.3s，保持原 10s 出生期限。没有证据将异常归因于限额或宣告已修复，不继续安排重复压力或预防性补丁。观察状态统一见 [KNOWN_ISSUES](../KNOWN_ISSUES.md#历史偶发异常仅观察)。

复发时由本计划接续，先保存 `driver preparation deadline expired` / `driver bootstrap deadline expired` / `driver worker Start failed` 的实际首因、phase/GrantOutcome、now/end、process/PID、原 Pool 和同轮 Binding/供应状态，再决定是否需要修复。安全收集的旧证据不能证明所有出生错误均正常；无首因的 124 不算超时性能要求，也不因后一次通过而改写原失败。
