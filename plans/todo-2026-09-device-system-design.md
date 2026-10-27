# 设备体系设计：领域模型、驱动管理与资源收束

> 状态：**第二轮机制收敛、生命周期收口及所选组合验证已完成，本次统一保存代码交付；首批完整闭包仍有独立门未关闭。** 本篇拥有详细目标、实施与原开放验收门；第二轮发现、处置与验证见[收敛交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)及[生命周期收口档案](archived/review-2026-10-27-device-lifecycle.md)，原固定对象见[登记档案](archived/todo-2026-10-27-device-implementation-review.md)。当前交付与后续安排见[提交与调查接力](#第二轮提交与调查接力)。服务装配、平台保留内存和系统终局仍由各自计划拥有；首轮检查点与证据边界见[历史审查交接](#审查交接2026-10-27)。

第二轮已实施合同及原验证由[机制收敛档案](archived/review-2026-10-27-device-implementation.md)保存；其后的[生命周期与观察收口](archived/review-2026-10-27-device-lifecycle.md)已完成修复、复核及所选组合验证，该档案保存增量目标和证据。[唯一收口表](#当前唯一收口表2026-10-26)继续维护DMA＋IPI待证与独立时钟异常，原全hart idle门已在替代结构下关闭。impls按当前代码回填；历史施工与审查交接段保留当时事实，不从其中的旧目标恢复施工。

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

正式实施仍执行规定的 check/clippy 与必要设备/故障/退款组合。全 hart idle 门在常态工作已静默后重新取同一时点与真实唤醒证据；DMA＋IPI 和时钟异常不因本设计关闭。原 Describe/Get、线程 ACK/join、未发布根异步退休等机制专属门，只有旧路径已删除且替代合同有证据时才登记被替代。

### 外部机制对照及适用限制

- [Genode Foundations 26.05：异步通知](https://genode.org/documentation/genode-foundations/26.05/architecture/Inter-component_communication.html) 明确 signal 可合并、无 payload，处理次数不能当事件次数，需要另读当前状态；[session 寿命](https://genode.org/documentation/genode-foundations/26.05/architecture/Recursive_system_structure.html) 由 server 拥有 RPC 对象身份。这里用于核对“owner 保存结果、通知只提示”，不移植其销毁 server 等同关闭 session 的政策。
- [Linux 6.12 devres](https://www.kernel.org/doc/html/v6.12/driver-api/driver-model/devres.html) 明确 managed resources 只负责 freeing，其他检查仍归驱动；[v6.12 dd.c](https://git.kernel.org/pub/scm/linux/kernel/git/torvalds/linux.git/tree/drivers/base/dd.c?h=v6.12) 的 device_remove 先于 device_unbind_cleanup/devres_release_all。它支持软件释放与硬件停止分别论证，不支持用 Drop 推导 DMA 静止。
- [Fuchsia driver dispatcher](https://fuchsia.dev/fuchsia-src/concepts/drivers/driver-dispatcher-and-threads) 与 [C++ driver rubric](https://fuchsia.dev/fuchsia-src/development/drivers/developer_guide/cpp-rubric) 分别描述 dispatcher 停止顺序与无法确认设备停止时保留 pin。两者为 2026-10-27 取回的滚动页面，且 DriverBase/DriverBase2 命名跨页不同，仅作非规范对照，不作为本项目固定硬件合同或子设备停止顺序的证据。

### 本轮设计检查与接力

对新目标的限定接口检查覆盖唯一 Use 直接报告、Preparing 拒绝/Unknown/父子收束，以及 Provision/USB 通知的先完成后等待、回复缺失和跨轮寿命。发现自主失败若等到本地清理后才公布 Terminal，可能阻断子停止传播；已修订为立即公布 Stopping/首错，管理者立即 Stop 原 Binding，再等实际清理。主代理核对了 PCI 现有先 Withdraw 子登记再等待供应的路径，以及 ConsumerQuiescence 的真实报告等待条件。通知生产者消散不等于接收 Owner CLOSED 的边界亦已明确。

上述限定设计检查在施工前完成，随后实现与所选运行验证已完成，证据见[收敛交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)。协议两侧与实际消费者已同步，不保留旧 wire 兼容层。ideas 本轮未重写，不能把旧线稿当作本目标的额外来源；原开放验收门继续按[唯一收口表](#当前唯一收口表2026-10-26)处理。

### 第二轮交付后整理

用户在第二轮交付后授权整理代码、完善文档和计划。整理时基线为 `refactor/system-redesign` / `a5a1a789`，当时未提交或 push；归档报告和原验收指纹保持当时事实，不把整理后的代码冒充原路线的逐字节版本。当前交付状态由下节拥有。

- 代码：root RPC 的推进、就绪检查和等待错误统一交原 CallOperation::abort；准备失败与 Start 失败共用取消后的 Collector 交接；Image 的逐页清理复用同一个保留失败 owner 的 Unmap 函数。通用等待基础设施失败直接进入 root 故障终局，不再借用 loader 命名入口。没有增加状态、协议或 owner。
- 接口与实现说明：补齐 Preparation 完成/首错/取消/Drop、GrantOutcome 的消费语义和 ImageView 的读取寿命；startup、runtime、rpc 按实际类型、接点和失败责任回填。修正旧出生块写法、Job 拓扑及“尚无驱动启动”的过时说明。
- 计划：当前第二轮接点与首轮历史交接明确分开；服务装配和系统总纲不再登记已完成的线程替换、loader 迁移或驱动接线。原 DMA＋IPI 及独立时钟异常仍只由本篇唯一收口表承接。
- 验证：`just check`、全部十个 clippy 分面、user 格式检查、libprocess/librpc 的 21 项 host 测试及 debug core 回归均退出 0。日志见 `artifacts/refactor/tidy-{check,clippy,fmt,host,core}.log`，core 实际入口为[本轮运行](../artifacts/run/20261027-175453-50937-acceptance-virt-debug.log)。host 仅覆盖其可执行的纯逻辑，分步装载与根调用通过目标编译、源码等价核对和真实 core 检查；未重复全部设备故障组合，也不扩大原验收声明。

本轮源码指纹与原交付的六文件差异见[整理版本](../artifacts/refactor/tidy-source-version.json)。交付前核对三条 RPC 错误分支仍使用原 cause/投递阶段，取消仍先保留 GrantOutcome 再交原 Collector，Unmap 失败仍放回原映射；文档链接、标题锚点和 diff 空白按本次修改检查。无需新增平行任务；未来固定提交的独立审查覆盖第二轮完整改动及本次整理。

### 第二轮提交与调查接力

本次代码交付统一包含第二轮设备控制链、持续监督及分步 ELF/loader/Image、未发布 DMA 空根局部回滚、后续生命周期与观察收口，以及实际消费者、验收工具和说明回填。代码基线为 `a5a1a7893872e73d410eecb38b7ea21aed7d1616`；固定提交在提交后的独立 Review 登记中记录。两份归档报告中的“尚未提交”保留归档时的历史事实，不作为当前接力状态。

最近版本已通过 `just check`、十分面 Clippy、137项相关 host 测试及 core/reuse/candidates/QueryUse/GetResource/Master 六条真实路线，具体证据与精确交错未动态注入的边界见[生命周期交付档案](archived/review-2026-10-27-device-lifecycle.md#最终组合验证与交付边界)。提交整理已核源码与该版本指纹一致，不重复运行已通过路线；较早第二轮平台/监督/IRQ 证据按原档案的版本和覆盖边界复用，不冒充本轮逐项重跑。

用户已确认后续主线转入剩余问题调查，按以下顺序接续：

1. **DMA 保留＋IPI 失败组合**：从[唯一收口表](#当前唯一收口表2026-10-26)的原第五轮断点开始，先核当前产物及已修正诊断脚本，再取同次 InitialWatch DONE、同一真实 IPI call 的失败返回/消费、后续原广播及同组 Mapping/Backing 跨四 hart 停驻保留。保持 selector/WFI 和原映射集合判据；证据缺口不自动升级为产品缺陷，不另开重复计划。
2. **独立平台时钟异常**：按[有限首因调查方案](#实际时钟失败的有限首因调查方案)接续，明确本次要区分的假设和有限观察窗口。历史未复现不算修复；本项不与 DMA＋IPI 的成败互相替代。

调查前读取 DEBUG-PLAYBOOK，涉及 GDB/平台再读 TOOLING-PITFALLS。独立代码 Review 单独登记固定交付，不替代上述取证，也不重复已结束的设备迁移、全 hart idle 验收或线程监督改造。通用长期服务/持久化关机及其他扩展仍按各自触发条件设计。

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

`shared/dtb` 已迁移，`libdevice` 已落实登记/绑定/供应状态、冻结资源表、授权/登记 backing、RPC 协议与组合绑定任务；实际 `srv_device` 已接入 init 的启动和正常停止。具体范围与未接消费者见下方施工记录，其余设备领域新目录仍为施工目标。库不依赖具体 libruntime，不自行开线程或消息泵。libdriver 提供普通类型和可组合任务，驱动将绑定、IRQ、请求来源放入既有执行/等待机制。先显式按名解码，不预建 derive/schema 系统。libhub 只有实际共同领域知识形成时才抽取。

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
4. 驱动逐名解码资源、检查类型和元数据、探测/初始化。失败报告 Terminal(error)，由管理者停止原绑定；探测碰过硬件后不能当作无效果拒绝。
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

子登记是独立的实际授权，不是 Provision 供应项。srv_driver 在原轮 ResourcesReady 后凭 BindingControl 调用既有 `IssueRegistrar`，先将 Registrar/SourceAdmin 纳入同一激活记录，再在该轮统一 Activate 中显式交付 Registrar。PCI 和 xHCI/root-hub worker 按实际总线装配角色取得它；HID/普通叶驱动不取得。SourceAdmin 由 srv_driver 原记录保存，init 保留外层来源管理。重复 Activate 核对同一 Registrar endpoint/context 身份与 BindingUse，不比较本地 handle；额外别名关闭，原授权不重签。IssueRegistrar 不幂等，结果 Unknown 时停止原装配/绑定并封原父轮，收束可能已发行的来源，不再签替代来源继续激活。父轮停止即时阻止子新登记，但父清理服务保留到子责任结束。

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

## 实施依赖与删除条件

本节安排已授权首批闭包的实施依赖，不创建空 crate 或临时 ABI。中间编辑可以不完整，交付须满足完整消费者与收束链。

1. **启动事实与特权资源**：共享 DT parser/只读 DTB 交付、平台根、MMIO/IRQ/DMA 对象和 shared/libkernel 同步；DMA 稳定 pin、根终态观察、PLIC 等待/撤销接入既有内存和维护 owner。平台保留内存按[所属计划](todo-2026-09-platform-reserved-memory-lifecycle.md)核定首批不使用的能力并明确拒绝。
2. **设备领域闭包**：登记/来源、Claim/StopClaim、Provision、冻结表、Watch 和功能边；类型/状态、提交、额度与清理一起实现，不先造只展示数据的设备树冒充管理。
3. **驱动管理与装配闭包**：boot 配对声明来源、唯一匹配、受托启动/复用/监督、激活、停止、发布；同步[装配](todo-2026-10-07-service-assembly.md)和[故障终局](todo-2026-09-system-shutdown-orchestration.md)，不保留 init/驱动自主抢占的旁路。
4. **实际总线与消费者**：PCI→xHCI/USB→HID→正式功能客户端；每层接通 Provision/停止，不以假设备代替真实热插。os/dtb 迁至 shared 后删除旧位置，更新真实消费者；清除被替换的裸地址/启动槽位接线，不为历史兼容留 adapter。
5. **组合验收与文档**：按构建手册执行 just check/clippy 与实际 QEMU 路线，更新 impls、范围登记及本篇工作记录。删除日志驱动占位及其强制装配，不为本路线顺带实现其他平台。

验证按风险选择：纯身份/轮次/解析在 host 边界；跨 provider Unknown、late reply、Quota 使用真实 RPC 设施；MMIO 撤销跨 hart、IRQ 旧 token、DMA pin/charge 在原对象测试处验证；热插使用 QMP 和实际 USB，观察未运行→启动→功能→拔出/Released；包含重插旧请求、叶/主控/管理者故障、reset 错误及根终态。故障终局的正确结果可以是有账隔离和停止，不能强求退款伪造成功。QEMU 不覆盖真实非一致 DMA、真实 xHCI halt 延迟与 IOMMU。

### 实际迁移面与删除门

以下按 `e6d11a1` 源码核对。表内新路径表示施工目标，不能据此判断目录已经存在。每个切面同时接通生产消费者和失败路径；自然编辑顺序不要求中间状态独立交付。

| 切面 | 当前实际接点与目标 | 同闭包删除 / 保留边界 |
|---|---|---|
| DT 纯解析及平台输入 | `os/dtb` 移到 `shared/dtb`；更新 os/shared workspace、`os/kernel/Cargo.toml` 和新用户消费者；`board/main/rt/boot` 交付平台事实及 DTB owner | 删除旧 dtb 目录/依赖和解析后立即归还 DTB 的旧时序；保留已有 RAM/静态保留区单一供给账本 |
| 启动 ABI | `shared/erhino_shared/src/startup.rs`、`boot/bootstrap.rs`、`libprocess::context/management`、`srv_init` | 同步平台描述及句柄数组校验；去掉初始五句柄固定总数假设，不改普通进程 payload 为内核设备语义 |
| 特权对象与等待 | shared 的 object/call/mem、kernel syscall/task/resource、libkernel typed owner；PLIC 新模块接 trap/sched/rt 的 SEIE 与安全点 | 新角色与调用两侧同改；公共 Close 不加设备分流；IRQ 不新增私有线程订阅表 |
| VM 与内存 | `memory_space`、`vm/{operation,change,views,space,retire}`、`memory_object`、uaccess | 原来源类型及所有分发点整体迁移；删除“所有 View 都是 RAM”的假设，不复制页表事务、不写裸物理映射旁路 |
| 用户设备与驱动 | `libdevice/libdriver/libpci/libusb/libhid`、`srv_device/srv_driver`、三个真实驱动和消费者 | 同一真实登记/激活入口；不保留 init/驱动各自全局抢 Claim 的备用路线 |
| boot 映像装配 | `Justfile::build_user/make_initfs`、`srv_init::StartupImage/RequiredLaunchSet` 与用户 workspace | 删除无硬件行为的 `drv_spi_sifive` 占位及其 REQUIRED_DRIVER 强制启动/输出锚点；这不删除已有 SPI 能力，因为现有程序只有日志。不为 sifive_u 实现新 SPI |
| 构建与验收 | Justfile host 库列表、INIT_FEATURES、DTS、`tools/qemu-acceptance.sh`、既有 GDB/QMP 支持 | 新设备 workload 使用同一正式装配/驱动；core/stress/platform 原有业务覆盖保留，旧占位锚点随占位删除 |
| 实现说明 | notes/impls 的 startup/task/mm/kernel-interface、device 所属篇及 notes 索引 | 仅代码落实后回填；本轮不提前把目标写入 impls，也不据此审定整篇 ideas |

当前内核无用户设备裸地址接口，唯一旧驱动是日志占位；“删除旧路径”不意味着另造一个旧 MMIO/IRQ/DMA ABI 再迁移。完整设备能力尚不存在，新增资源实现必须直接使用目标合同。

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

第二轮设备控制链、root 持续监督与分步装配已完成，原首批闭包仍保留 DMA＋IPI 组合证据缺口和独立时钟异常；当前入口为[唯一收口表](#当前唯一收口表2026-10-26)。本轮整理及补充验证见[交付后整理](#第二轮交付后整理)。

> 下方首轮施工、交接及审查交接保留当时事实与授权，不是当前施工指令；其中旧接口、旧工作树状态和“下一步”不再生效。当前目标以第二轮收敛设计为准，原验收证据及剩余门只由唯一收口表维护。

本轮代码实施从 `refactor/system-redesign` / `549a9e7` 的干净工作树开始。用户已授权按完善后的方案落地，范围为本篇首批设备闭包，依次接通启动事实/特权资源、设备领域、驱动管理装配、真实 PCI/xHCI/HID 与验收。独立设计增量 Review 仍按原登记排期，不将其记为已完成；提交和 push 均未获本轮授权。

### 当前交接点（2026-10-25）

**在设备管理与发现层完成当前范围验证、平台 Provision 接入之前交接。** 用户明确要求在合适边界整理后换会话。本次检查确认没有正在编辑一半的平台注册/硬件初始化路径，没有运行中的子代理、QEMU 或 GDB；保持已验证实现，不为交接另开一段未完成的硬件施工。首批设备能力仍未完成，本计划和矫正区继续保留。

#### 接手基线与工作树

- 仓库 `/Users/chien/Projects/halcyon`，分支 `refactor/system-redesign`，HEAD `549a9e78400f9e59f3bdf516ef4d54f101846178`。**本会话设备施工全部仍在工作树，含大量未跟踪新文件；HEAD 不包含这些实现。** 后续在同一工作树接手，不能从 HEAD 新建一个干净 checkout 就当作当前实现，也不能清理未跟踪目录。
- 首批完整闭包的代码实施授权持续有效；本次只暂停交接，不撤回平台/驱动后续授权。提交、push 和发布尚未授权；暂存区为空。
- 改动除设备代码外，还包含 `os/dtb → shared/dtb` 的完整迁移、nightly feature/lint 适配和既有验收工具修正。`git diff --stat` 不计未跟踪文件；须同时查看 `git status --short` 与 `git ls-files --others --exclude-standard`。按本篇各续接记录理解这些改动，不以“看起来不属于新服务”回退。
- 交接盘点为 185 个变更文件，其中 83 个未跟踪（其余含已跟踪修改与删除）；没有暂存或提交。本次工作树文件清单、状态及 SHA-256 保存在 `artifacts/device-handoff/worktree-manifest.json`，用于识别接手后的漂移，不代替源码或本篇。`artifacts/` 是本机证据，不提交，也不是重建代码的备份；关键状态和下一步均在正文。
- 本次设备新增范围已收集的审查 finding 均已修复；全项目其他活跃 Review 仍归各自记录。最近 sub-20 的两项 P2/一项 P3 已修复，反例、修复及复核见[发现层续接](#设备发现观察与授权续接)。这些施工中检查不等于固定提交的完整独立 Review；原设计增量 Review 仍按 COMPASS 登记等待，不重复做一轮相同首审。

#### 当前可直接复用的代码

| 切面 | 当前事实与代码入口 | 证据/限制 |
|---|---|---|
| 平台事实与根 | `shared/dtb/src/{pci,plic}.rs`；`shared/erhino_shared/src/platform.rs`；`os/kernel/src/board/{io,irq}.rs`；`boot/bootstrap.rs` | virt 有真实三 MMIO 根、PCI INTx 根及 coherent TrustedPhysical DMA 根；其他现有平台零根。描述中的数字不是 authority |
| 特权资源与内存责任 | `os/kernel/src/resource/{mmio,irq,dma}/`；`vm/{mapping,forced,operation,change,views}.rs`；`user/libraries/libkernel/src/{mmio,irq,dma}.rs` | MMIO 撤销/VM 退休、IRQ 代次/Take/Ack、DMA pin/暴露保留/受信 Retire 已接；真实 xHCI IRQ/DMA 尚无消费者 |
| 领域状态与权限 | `user/libraries/libdevice/src/{registry,binding,provision,publication,resource,protocol}.rs` | Registry 单 actor；原 sender_context 授权、publication_seq、BindingRef、整表冻结、停止与旧轮控制均可复用；不要重建一份设备表 |
| 准备与停止 owner | `libdevice/src/binding_work.rs::{Work,Binding}` | Claim 原任务长期持有准备/停止 RPC 与残表；原 Provision 安全 Closed 后才 Released；Work 还等旧 Control/Use/Watch/在途请求引用退休 |
| 发现与功能 | `libdevice/src/registry/{discovery,functions,oal}.rs`、`watch.rs`、`client.rs` | 有限枚举、Watch、具名功能根/激活观察、OAL 同门授权；子来源固化父 BindingRef。非空场景有 host 反例，尚无驱动运行证据 |
| 服务宿主 | `user/services/srv_device/src/main.rs`、`server.rs::{World,Family,run}`、`server/{request,source,watch}.rs` | 已有实际 Runtime/Assembly/Outbox/Dispatcher。入口当前只处理 device/OAL，尚不受理平台 Provision 协议；新增供应任务须组合到既有运行体 |
| init 装配 | `user/services/srv_init/src/platform.rs::PlatformDescription`、`device.rs::Manager::{launch,stop}`、`main.rs::launch_test_services`、`supervisor.rs::RootSupervisor` | 原根仍在 init；Manager 只交 bootstrap 与 Pool 两个出生 grant。实际私有就绪、空根枚举/OAL/Watch、封口/正常停止均已跑通 |
| 构建/验收 | `user/Cargo.toml`、`Justfile::make_initfs`、`tools/qemu-acceptance.sh` | srv_device 已列 workspace、initfs 和必需启动集合；当前没有 srv_driver/libdriver/libpci/libusb/libhid、真实 PCI/xHCI/HID 驱动或 acceptance-device 路线 |

实现审计视图从 [device](../notes/impls/device.md)、[startup](../notes/impls/startup.md)、[mm](../notes/impls/mm.md) 进入。代码是唯一真值；不要从未审定 ideas 或本篇历史段落推导当前行为。

#### 下一施工单元与第一批接线位置

**2026-10-25 设计修订（施工中）**：亲读 IssueRegistrar→check_child_source→binding_usable 后，确认原“registrar 属于 Frozen 表项”的表述与 ResourcesReady 签发门形成循环。经协作监督确认，采用上文子登记激活授权合同；保留整表冻结、CHILD_REGISTER 与原 BindingRef 失效门，不放宽 Preparing 的签发检查，不制造占位 sender。该调整同样覆盖 PCI 与 xHCI，发送/采用/Unknown/停止消费者随统一驱动激活一起接通。

本轮平台输入与真实登记已接通：内核初始槽检查仍由 `Description::parse` 严格执行，只读事实另以 `Description::decode` 解析；服务委托由 libdevice 的 16 字节用户态出生头给出版本/根数，固定按描述记录序号映射到 `ROOT_BASE+i`，不传可推导的索引表。PCI 平台几何/能力核验收敛到 libpci，由 init 与服务共同使用，init 保留真实根。`InitialGrants::len` 保留完整数组几何，`remaining` 从原 Option 槽计算实际剩余应用能力数，排除已由 runtime 采用的入口线程 Control；服务用实际能力数核委托并逐槽采用，不能伪造内核数组长度。

#### 2026-10-25 平台登记续接（施工中）

**消费者停止证据修订**：平台 Frozen 含 DmaDomainControl，PCI provider 属于已授权的受信物理供应链。平台不能只等后代退出就撤 ECAM；它须先消费原轮消费者停止事实，且仍独立核资源实际完成。原 Binding 增加不可逆的 resources_exposed 事实，唯一公开 Control/Use GetResource 出口在能力可能进入 Outbox 前置位；回复失败/Unknown/Close/进程退出均不复位；Resources OAL 的零能力目录/Describe 不标记暴露，若未来增加实际能力取得，必须复用此唯一出口，不能另建遗漏标记的能力路径。原 Work 只有在 Binding 已 Stopping、Get 已封且未曾交付资源时，才沿 Stop 已接管→ConfirmStopped 发送内生的“无消费者访问”证明；Preparing/残表取消不依赖整表冻结后才计算的 dma_report，也不伪造外部 ReportStopped 或扩大调用者权限。

已暴露轮只接受认证原轮 worker 的永久停止事实。consumer_stopped 表示所有仍需该轮 ECAM/MMIO/IRQ 或新 DMA 提交的消费者侧硬件清理均已结束；不能由 Terminal 推导。PCI worker 在这些后代硬件责任、最后一次 ECAM/配置访问及在途清理结束后报告，不等待自身 Binding Released；报告后保留的子服务只处理不再使用这些资源的迟到拒绝、既有结果和软件收尾。平台 Revoke 晚于这份事实，且独立等待资源 DONE。Activate Unknown 保持原 ref/use/Registrar；Stop 前发生的 Get 保守记为已暴露，Stop 先行则迟到 Get 拒绝。PCI/xHCI/管理者死亡而缺少安全证据时，封来源/执行/资源准入并进入故障终局，不由 Process DONE 伪造 DMA 静止。该修订在真实 PCI 激活前实施；host 与实际消费者分别验证，不将推演计为运行证据。

**父轮执行寿命修订**：供应安全与父清理服务可回收是不同事实。Work 在有效 Closed/明确无效果 Rejected 后进入原 Next::Release，保留供应安全事实，等待上述真实依赖全部 Released；重复 Stop 不退回供应 RPC，不再重发 Prepare/Get/Stop/ConfirmStopped，原停止绝对期限继续有效，耗尽进入故障而非释放。后代变化经原 dirty/有界 maintenance 扫描唤醒 Stopping 父任务，直接 StopBinding 且 Claim 来源仍开放同样覆盖；无变化不持续唤醒。平台 Adapter 不再复制拓扑后代扫描，保留真实消费者报告与资源 DONE 两道物理门；中央释放门只保护父执行/清理服务寿命，不能替代 provider 自身安全 Closed。

- 已接消费者/行为：init 经原 RootSupervisor 复制只读描述及受限平台根，Grant 成功或已消费失败均按原账本兑现；srv_device 共同核 DT token/几何/kind/rights，实际平台任务经原 Dispatcher 与 Register 请求登记 PCI host，原 Request 持登记控制和 Lifetime。服务私有就绪等待平台登记回执，init 核真实 DeviceId/incarnation/name/identity/四项硬件表上界，OAL 根按有根/零根实际装配验证；原空根假设已删除。
- 实现接点：`libpci::{platform,host}` 拥有 PCI 平台能力核验和冻结 metadata；`srv_device::server::platform` 在原 Family 中组合登记和供应工作，`Request` 接 Provision 协议。Prepare 按原 Provision 轮次逐步派生真实 ECAM/window/IRQ/DMA 子控制并冻结表；Stop 先 Seal，收到原轮消费者停止事实后撤资源/等 DONE，Closed 不由进程终态推导。原 Registry discovery change 唤醒平台任务，不依赖停止调用者重试。
- 运行失败与修复：第一轮 `virt.exit=124` 是服务将含 runtime 入口控制空槽的 `len` 当作应用授权数引发 panic 后主动收割；由上述 remaining 接口修复。第二轮 `virt-second.exit=124` 是 Register owner 安装控制/provider 观察来源后直接 Parked；Runtime 登记回执不自动唤醒，回复无法推进。现 Observed/原 SourceRegistration 的 Installing/Removing 期间保持 Runnable，稳定后 Parked，根 Source 同规，不新增并列状态。
- 开发证据：`artifacts/device-platform/host.log/.exit=0`，libdevice 与 libpci host 反例通过；目标 `srv_init`/`srv_device` Clippy `target-dev-second.exit=0`。修复停驻后的 release core `virt-third.exit=0`，入口 `artifacts/run/20261025-024647-28533-acceptance-virt-release.log`，成功客体原始日志 `artifacts/qemu-acceptance-20261025-024649-28860.log`，覆盖真实非空登记/身份/OAL/Watch、来源封口、管理者正常退出与原 core 收束。后续有限扫描 end、错误传播及 remaining host 断言为开发增量，随下一真实消费者检查，不重复 core 作为已完成基线。
- 剩余责任由下节实际 PCI 续接记录承接。平台登记与供应接线已经推进到真实驱动消费者；完整首批能力仍未完成，不由源码存在或局部 Ready 宣告交付。

#### 2026-10-25 实际 PCI 消费者续接（施工中）

init 从同一 tar 取得 srv_driver 与 drv_pci_generic，使用 libdriver::boot 的有限具体声明逐项配对 libprocess::image 的 sealed MemoryObject/只读映射。原 RootSupervisor 持驱动装配记录、SourceAdmin、管理者及 worker Job/Pool；Grant 成功和 Consumed 失败沿原账本兑现。srv_driver 已纳入 workspace/initfs，原 drv_spi_sifive 日志占位及装箱路径已删除。运行期 init 存活时监测 srv_driver/srv_device 死亡并即时封口的接线仍未完成；现仅在原根故障入口封已持有的驱动来源，不能把 CPU 收集记为供应安全。init 自身 DONE→boot 停驻已有基线，不另开监督机制。

srv_driver 在原 Runtime/Dispatcher 内通过 Watch→有限枚举→具体声明唯一选择→原轮 Claim/整表 ResourcesReady→IssueUse/IssueRegistrar→真实出生→统一 Activate 接通 PCI。SourceAdmin 留在同一 Managed 记录；只读映像使用原 loader，不新增仓库或加载器。Managed 的进程收集复用 libprocess::SuperviseTask 的 advance_in；其动态 Observation.id 回调只在旧 boot/DONE 来源实际注销后移交，不能假定固定 kind。来源封口可提前终结 Binding Watch，停止后使用原 Control、绝对期限和定时重查观察 Released。选择的一次候选规则、完整 Drivers OAL、运行期死亡消费者及费用/Unknown 故障门仍须随本闭包收口。

drv_pci_generic 使用同一 Runtime 组合激活、观察、平台资源 Get、ECAM 扫描和子供应。固定四项平台能力逐项核实际角色、rights、几何及 TrustedPhysical/Coherent 模式。PCI function 资源表使用原 Provision/Collecting/Frozen，拥有 BAR、IRQ/DMA 子控制；受限 pci-function 会话只允许本轮 EnableBusMaster，不开放任意配置写。当前只实现首批 bus 0、单 xHCI 控制器，桥和额外 xHCI 明确拒绝；尚未装入真实 xHCI 或产生 DMA。PCI 普通停止在全部子硬件供应 Closed 后解除 ECAM、报告永久消费者停止，仍保留不触硬件的软件清理入口至中央 Released。

开发运行证据与失败：

- `assembly-target-second/third/fourth.exit=0`：目标 srv_init/srv_driver/drv_pci_generic Clippy；`host-parent-release.exit=0` 与 `target-parent-release.exit=0` 覆盖精确父轮依赖门及原监督复用的开发检查。
- `virt-pci-first/second.exit=124` 是实际 Activate/StopActivation 从已消费 RPC 槽 0 取 Use 导致 Resource，随后收集回调误用固定 kind 导致 panic 的主动收割。真实业务输入已改从槽 1 起；回调已按不重叠所有权移交。原始日志 `artifacts/failed-acceptance-20261025-045209-44980.log`、`artifacts/failed-acceptance-20261025-045314-45632.log` 保留。
- `virt-pci-third.exit=124` 已真实完成 Watch/Claim、整表 Ready、PCI Activate、四资源 Get、ECAM 映射、bus 0 slot 0 fn 0 的 1b36:0008/class060000 枚举及激活 Ready。整体失败于后段原 MMIO 进程退休验收再次派生同一 ECAM 范围，正确触发排他冲突。现将真实驱动/设备正常停止移至该验收前，全部原场景保留。原始日志 `artifacts/failed-acceptance-20261025-045420-46184.log`；不将 PCI 枚举混称发现/接通 xHCI。
- `virt-pci-fourth/fifth.exit=124` 为管理者正常退出等待失败后的明确失败收割。第五轮实际到达 consumer_stopped、Binding Released、worker 收集；原始日志 `artifacts/failed-acceptance-20261025-045738-49060.log`。原 Manager 设置 finished 后返回 Parked，使外层尚未接管 shutdown 就进入无限 Wait；已改在该接管点保持 Runnable。第四轮原始日志 `artifacts/failed-acceptance-20261025-045536-47482.log` 同样保留，未把退出码当纯超时调期限。

`virt-pci-sixth.exit=0`：入口 `artifacts/run/20261025-045851-50170-acceptance-virt-release.log`，真实客体 `artifacts/qemu-acceptance-20261025-045854-50530.log`。完整到达 PCI 四项平台资源取得/ECAM 枚举与 Ready、consumer_stopped/解除 ECAM、Binding Released、worker 收集、srv_driver 正常退出、Source Closed 与 Job 收集、srv_device 正常退出；随后原 MMIO process retirement 重新派生原范围、DMA process retirement、acceptance 域收集及最终 shutdown 全部通过。它是当前实际 PCI 消费者的开发运行证据；无 xHCI 装机、IRQ、真实 DMA 或热插证据。

当前续接以本节之后的正式键盘路线记录为准：PCI/xHCI/HID/正式客户端已接通单键盘两轮正常热插，继续多接口进程复用、RootSupervisor/Managed 运行期故障消费者及原费用/Unknown/精确交错门。以上仍是同一首批闭包，不构成整体系交付。

#### 2026-10-25 实际 xHCI 续接（施工中）

virt 启动配置已增加 slot 1 的 qemu-xhci，显式 msi/msix=off、nic=none；DTS 保持实际 PLIC interrupt-map/ranges，function 从 ECAM 枚举而非从文本授予。drv_usb_xhci 与配对声明/只读映像已纳入 workspace/initfs/init 同次委托，走原 Watch/Claim/Ready/IssueUse/IssueRegistrar/Activate。PCI 与 xHCI 已删除各自重复的激活认证/观察实现，共用 libdriver::worker 的 Control/Activate；该组件只持原领域事实并组合 Dispatcher/Outbox，不创建运行体，不解释硬件，也不保存中央占用。

libusb 拥有具体 USB Setup/boot 描述符以及 xHCI coherent 页/ring/寄存器。Controller 先等 CNR，再 halt/HCRST，核能力和 PAGESIZE，逐页创建 MemoryObject/CPU View、Pin 并真实取得设备地址，完整初始化后经 pci-function 启 BME，再发布 DCBAA/CRCR/ERST、Run。事件按 cycle/类型/原 command 指针匹配，有界消费；IRQ 只是提示。停止先由同一 owner 完成 USB endpoint/slot/TD/command 清理，usb_idle 门核实际 Closed 后才进入控制器 halt；HCH 后仍收残事件，逐页 Retire/等 DONE，注销 IRQ 观察再解除寄存器映射，最后报告消费者停止。受限 USB session 和真实 HID 消费者已接，功能应用、热插和多接口复用仍在施工。

**立即生效的证据纠正**：`virt-xhci-fourth.exit=0` 只保留当时路线退出、软件收束与所观察的 Run/HCH/Retire 事实，撤回功能性 Controller Ready/成功命令处理结论。真实键盘开发运行发现 USBSTS.HCE=1；原实现未在 Ready 检查 HCE/HSE，且在 BME 关闭时发布 ERSTBA。固定 QEMU 在该高 DWORD 写入即 DMA 读 ERST；取证见平台资料补充。最终顺序为完整 pin/CPU 初始化→原会话 EnableBusMaster 确认→发布 DCBAAP/CRCR/ERSTBA→Run，Ready 和运行期同时拒 HCE/HSE，不靠清错误位或忽略无事件继续。启 BME 后配置错误/取消仍属于原轮潜在硬件访问责任；Unknown 恢复/停止原会话，不重建域/环，不因尚未 Run 退回无效果。

`pci-function` 已从复用设备 Provision sender 改为每轮 Prepare 在硬件效果前独立 mint 的受限 function context，整表导出该轮入口，EnableMaster 在同一 actor 核当前 context 与 Prepared，Closed 丢入口。旧 alias 不得在下一轮 Prepared 启 BME；无逐轮表/兼容分支。`pci-session-target.exit=0` 为目标 Clippy；普通新入口的成功运行不能替代旧 alias、新轮与停止后启用、Unknown 同轮重试的权限反例。

USB 根端口/slot/EP0/配置已接入 Controller 原 owner，保存实际 context/ring、原 TD/command 地址和有限期限；停止先收 endpoint/SetDequeue/DisableSlot，再清 DCBAA/逐页退休，最后才进入控制器 halt。当前 `virt-usb-enum-fourth.exit=0`（`artifacts/qemu-acceptance-20261025-054106-63466.log`）以**启动即插真实 usb-kbd**作开发证据：HCE=0，实际 PortStatusChange、EnableSlot/AddressDevice Command Completion，Device 8/18 字节及 Config 9/完整描述符的 Transfer Event，SET_CONFIGURATION/ConfigureEndpoint 与 boot interface 0/IN endpoint 0x81；正常停止的 StopEndpoint/SetDequeue/DisableSlot、HCH/Retire DONE、两层 Released/收集及原 core 完整收束。该轮也覆盖新 pci-function 入口正常调用；不证明旧轮权限反例。IRQ Take/Ack 尚未按实际 token 单独核证；受限 session、HID/应用和最终 QMP 无键盘启动→热插仍未接通，不计首批完成。

受限接口供应复用原 Provision/Collecting/Frozen：每轮独立 session context，只导出 `usb-interface`；请求保存原 seq、具体 BootProtocol/InterruptIn 输入和一份结果，Unknown 不换 seq。取消复用物理 Device 原 StopEndpoint→SetDequeue 路径，只有实际完成后交付取消；接口 Stop 再 DisableSlot/清 DCBAA/等页 DONE。旧 session context 不进入新 Prepare。请求准入由原 Budget 的 root/业务/认证控制账户承担，无 active_requests 镜像计数。`usb-provider-target-second.exit=0`、`virt-usb-provider-first.exit=0`（客体 `artifacts/qemu-acceptance-20261025-061731-66659.log`）覆盖真实 serial 3 登记/撤出及原正常回收链，尚无该轮的 HID Claim。

`libhid` 已拥有 boot 报告状态/键盘面貌和 Query/Watch/Ack 协议；ErrorRollOver、坏保留字节与断开将状态标为不确定并保留最后有效键集，不伪造释放帧。`drv_usb_hid` 通过统一 Activate 和原 Runtime/Dispatcher 取得真实受限会话，SET_PROTOCOL(0) 完成后 Ready，未取得 Registrar/物理根。独立 Watch 保留自己的确认游标，通知只提示当前状态；USB TD 和 DMA 始终归 xHCI。`hid-host-first.exit=0` 的两个独立 HID 合同反例覆盖有效键排序/去重、ErrorRollOver/断开不伪造释放及真实零报告释放；`hid-target-second.exit=0`，`virt-hid-first.exit=0`（客体 `artifacts/qemu-acceptance-20261025-062331-68859.log`）实际到达 HID Claim→整表→会话取得→真实 SET_PROTOCOL Transfer Event→HID Ready；正常停止实际取消 NAK TD、USB Provision Closed、三层 Released/worker 收集和原 core 收束。键盘未输入按键，因此当前不宣称 HID 状态输入/OAL 应用完成。

### 正式键盘路线与逐 hart PLIC 接管

`test_device` 只取得 Root 范围的 OBSERVE/FUNCTIONS，有限枚举真实设备并凭原 BindingRef OpenFunction；它从真实父链构造 OAL 路径，核 Boundary 与显式取得入口的 KOID/context 完全相同，并验证键盘面貌。每步 Query/Watch 核有效键集及新序号后才允许主机发送下一步。客户端的原 Subtree Watch 保留被测实例到 present=false 且 Seat 无活 Binding，避免把 StaleInstance/endpoint DEAD 当安全释放。旧功能入口在新实例就绪后只能明确 Closed/ServiceClosed 或仍为断开终态，Unknown 不算反例通过。

`just acceptance-device` 的 device workload 在原 core 前装配正式客户端；QMP 操作者复用原验收/节流/退出设施，使用项目内私有 socket，等待客户端无键盘及 xHCI Runtime 实际 Waiting 后才插入。主机每次 device_del 同时等待匹配 DEVICE_DELETED（兼容事件先于/后于回复），与客体原 Seat 释放分别取证；两轮显式使用相同 QMP port1/客体 port5。该等待点只证明该 Runtime 无 runnable 任务，不声称所有 hart 同时 idle。

实际失败保留：`virt-device-first.exit=2` 为构建入口未接受新 workload，尚未运行客体；`virt-device-second.exit=124`（`artifacts/failed-acceptance-20261025-063306-71815.log`）已完成第一轮按键/释放/停止，第二轮自动分到 port6，发现循环把有限扫描中刚退休项的 StaleInstance 当 fatal。修复只在发现 Query 跳过它并保持原 after/end，不把它当 Released。`virt-device-third.exit=124`（`artifacts/failed-acceptance-20261025-063536-72734.log`）已在同 port5 重插，但旧外部入口保活的 Pool 尾部超出原预算，worker 出生 Quota 后重复选同一候选，管理发现亦误拒旧 StaleInstance。原 Run 现保存已尝试选择结果，已清理的硬件/进程/Pool/订阅不为此保活；多候选后续有限尝试和复用仍须完成，不建立第二份尝试表。Declaration.capacity 保持每进程绑定容量，未改作全实现进程/Run 上限；当前仍是一进程一激活。

Pool 配置统一消费 `libdriver::boot::WORKER_POOL`：每份为 ABI 8 MiB 主栈加 8 MiB 映像/堆/执行存储、1 MiB kernel metadata。init 付款域按 manager 常驻、三份活 worker、一份允许的退休重叠及一次串行 loader 写入临时峰值配置为 80 MiB/6 MiB；峰值从 libprocess 原 1 MiB 写入分块按 metadata slot 换算，不能由已委托给目标的 Pool 代付。此前 80 MiB/5 MiB 在 debug 退休重叠缺少调用者临时额度的证据与修正见下方记录。这是有界工作集，更多外部旧入口仍可导致明确拒绝，不强制提前退款或无限加额。`virt-device-seventh.exit=0`（`artifacts/qemu-acceptance-20261025-065339-80134.log`）逐责任观察到首个 HID 进程已收集时 allocated_pages=0、allocated_metadata=514，随后客户端实际关闭旧 function alias；最后客户端/物理 Binding/进程/Job 均收束后，原父 Pool 的 delegated_pages=0、delegated_metadata=0。页已退与旧能力尾部保留分别记录，预算增加未替代退款证据。

`virt-device-fourth.exit=1`（`artifacts/failed-acceptance-20261025-064124-74022.log`）和 `virt-device-fifth.exit=1`（`artifacts/failed-acceptance-20261025-064544-76062.log`）都完成两轮同端口按键/释放/OAL/删除/Seat 释放及原 core/shutdown，仍因未取得 IRQ token 判失败。第五轮 boot hart1，source33 target slot0 的 priority/pending/enable 均有效，但 QMP 只读 S context1 threshold=7；未读有副作用的 claim。固定 OpenSBI v1.8.1 的 warm context 初始化会重写该值，取证和链接见[平台资料](ref-2026-10-23-device-platform-contracts.md#opensbi-v181-的逐-hart-plic-接管)。

实现修正将原每 hart 接管移至 `rt::hart_formal_entry` 的共同 CSR/stvec 之后、publish_online 之前，只清自己的 S context enable/threshold 并读回，再置 SEIE；全局 Controller 只建立唯一 owner/slot→context，删除跨 hart 早写及 sched 重复接管。沿固件完成→本 hart 接管→Online→全员 Online/Ready→用户授权的原屏障，不增加 ready 位、运行期重设或 IRQ 补偿。`irq-handover-host/check/target.exit=0`；`virt-device-sixth.exit=0`（`artifacts/qemu-acceptance-20261025-064822-78040.log`）实际 boot hart3、source33 target slot0，IRQ 来源在 timer 前唤醒原 Engine并 Take/Ack，完整两轮同端口 serial3/4、a 按下 seq2/释放 seq3、主机删除/原 Seat 释放、三层收集与 core/shutdown通过。第七轮追加退款及旧入口严格结果验证也通过；它是当前单键盘正常链正式路线证据，仍不证明整个首批闭包完成。

当前转[审查交接](#审查交接2026-10-27)，停止继续施工/验收。MMIO第十二轮exit0后整组关闭；DMA+IPI第五轮资格诊断RuntimeError导致未到目标，最后纯诊断修正仅离线验证，不改变原失败或门状态。两组待证（DMA+IPI、全idle/PCI）及时钟异常独立未定，仍由下方唯一表承接；不自动第六轮、idle或聚合。当前为可审查施工checkpoint，首批闭包未交付、不归档。用户已授权主代理整理提交及本地Review登记，未授权push；实施者只交冻结清单，不执行commit。

**2026-10-26 用户确认的本轮推进安排**：验收取证一轮仍无法确定问题时，将当前事实、已有证据、待核问题、对应owner、复现条件和完成判据留在本计划的所属条目，转入最终集中验收，先推进其他独立工作。已明确且影响后续正确性或施工的实现错误及时沿原责任链修复。当前全hart idle/PCI轮询尚无确定缺陷结论，按此安排暂存；不继续以多轮探针适配占据主线。延期项保持未验证状态，最终验收逐项处理，已有通过证据按受影响范围复用。

### 多绑定进程复用的施工合同

本轮修订将原 Managed 中的两种实际寿命整体分离，复用现有 Runtime/Dispatcher/libprocess；不保留另一个单激活协议、复用 adapter 或并行设备占用表。以下是已确认目标，代码未落地前不回填为 impls 事实。

- **进程 owner**：srv_driver 同一 Runtime 新的具体 ProcessTask 接过原 Worker 的 Pool、ProcessControl、私有 boot、DONE 来源和原 SuperviseTask。ProcessRecord 的本地单调 ProcessId 在 OS 出生之前建立，保存实现、任务、实际 PID/入口和可接纳状态；OS spawn 只发生在 ProcessTask 真正准入之后。出生失败、任务准入失败及停止先到均回到已关联原 Run；没有 PID 不等于没有原装配责任。
- **关联与容量**：Run 仅保存原 BindingRef、候选选择、ProcessId 关联及唯一 Released 事实。关联提交在同一 actor 内复核进程未封新关联、静态实现/复用政策和由原 runs 推导的容量，覆盖出生中、等 boot、停止中及 Activate Unknown；不再另存成员表或活跃计数。Declaration.capacity 只限制每进程绑定容量，另以明确静态 reusable 政策区分 PCI/xHCI 隔离与 HID 复用。进程付款总额仍由真实 Pool 约束。
- **单轮 owner**：Managed 独占本轮 Claim/BindingControl/Use/子 Registrar 管理、Activate、功能安装、两类订阅和原调用 Unknown。只有本轮安全 Released、RPC/清理及订阅实际脱离，才关闭这些能力并解除 ProcessId 关联、唤醒 ProcessTask；不再 Kill 或等待 Process collected。原已尝试候选结果可以保留在纯 Terminal Run，设备撤出/退休或管理来源停止后删除；其清理用原 wake_cursor/Query，不另建定时扫描。多候选只能在旧轮安全 Released 后有限尝试下一候选。
- **进程收束**：ProcessTask 先封新关联，再确认无仍关联成员，才 Kill、注销出生/DONE 来源并交原 collector；无空闲进程缓存政策。晚到关联与封口由同 actor 串行确定顺序。DONE 是进程事实，立即封复用并沿原 runs 游标逐个唤醒其成员；普通 HID 的成员逐轮 Stop，由存活 USB owner 收硬件，受信 PCI/xHCI 缺可信硬件静止证据则保留/故障终局。进程完成回执只更新唯一 ProcessRecord；删除 Run.collected 等单绑定进程收集镜像。管理者完成同时要求 Run 工作和 ProcessTask 都已收束。
- **统一 worker 入口**：libdriver::worker 拆为唯一 WorkerControl（mailbox/认证管理 context/实现配置）与每 BindingRef 的 ActivationSlot（原 Activation、独立 function/observation context 和通知）。同一个 Activate/Stop/receipt/Observe 合同显式定位 Slot；PCI/xHCI 配置容量 1，HID 配置容量 2。worker 出生配置由 libdriver 的具体版本化 WorkerConfig 编码，同次真实 Spawn 交付，manager 和 worker 采用同一 capacity；运行中 payload 不可改装配政策。槽及必要任务/输入在查询原 BindingUse 或交付入口之前准备；在途请求持同一稳定记录，迟到查询不重新插回已退休槽。
- **HID 工作与尾部**：每接口 Instance 独占 Keys、USB seq/结果和 Engine，在同一 mailbox/Dispatcher/Runtime 内独立推进。确认中央 Released 并退出本轮初始化/USB/RPC 工作后归还活跃额度；function/observation/每个 Watch 的旧 context 永不复用。旧入口只保留断开快照、序号、独立确认游标和必要身份，继续支付软件尾部，不持硬件责任。尾部采用本 worker 的独立记录预算（首批 32 条、每实例至多 8 个 Watch），按真实 Pool 存储和执行/来源费用准入；耗尽可明确拒绝新工作，不能驱逐既有清理或把旧尾部计成另一个活跃绑定。
- **尾部最后关闭**：保留 function/observation/Watch 控制的真实 Lifetime；停止发行入口后先关闭本地 sender 母本，再观察外部别名/已接 Delivery 完成，避免母本自保活。复用原请求/任务生命周期承担观察来源和退款；活跃 Engine 已退出时仍有原尾部 owner 推进。整个进程已可收集时允许关闭 mailbox，让旧入口明确 ServiceClosed；内核 Pool 费用仍随真实最后引用保留。
- **USB 物理 owner**：Controller 从单 Option<Device> 整体迁移为有限直接端口/slot 记录；首批按两份实际连接付款，并将 MaxSlotsEn、DCBAA/doorbell 边界和上下文/ring 数量共同限制为硬件能力、页几何与付款配置的最小值。每份记录仍持原 command/TD 地址、slot、缓冲和停止期限；事件按真实 pointer/slot 回到原 owner，相关完成消费前不复用地址。
- **命令与清理活性**：仍只有原全局 command ring 和有限串行在途命令；等待意图保留在原 Device 阶段，命令准入轮询游标只在实际授出命令后推进，避免忙等游标与快速完成造成饥饿。每轮先更新所有有限连接的停止意图/期限，再有界选择可推进 owner；业务不能无限预占队列或消耗临时清理 DMA 额度。单连接 Provision Closed 后才可忘记其已安全物理记录，不因其他活接口阻挡无关软件壳退休。
- **USB 停止粒度**：接口/端点只终结自身会话；同 slot 若仍有合法活接口，不得 Disable Slot。整个物理设备停止才收完所有 endpoint/命令、Disable Slot、清 DCBAA/逐页 DONE。直接端口多设备的证据不冒充复合 USB 设备支持；当前描述符支持范围及未覆盖组合继续如实记录，不能因改 Vec 宣称通用多接口。

当前统一迁移已接 ProcessTask/Managed 与三个实际 worker 的 WorkerControl/ActivationSlot；HID 的 Instance 分别持有 Keys、USB seq、Engine 和独立 Watch。原 Engine 同时作为付费尾部 owner，在 Terminal 后继续完成 Use 查询、实际 Released、sender 母本关闭、Lifetime 安装/注销和最后 Delivery 退休；旧 context 不复用。每步最多推进一个 Slot 来源变更与一个键盘 Watch 来源变更，关闭风暴不耗尽 Runtime 原 Gate 的 16 个操作槽。`process-owner-target-second.exit=0`、`multi-physical-target-third.exit=0`、`multi-hid-target-second.exit=0`、`multi-owner-target-second.exit=0` 为实际消费者目标 Clippy 开发证据，均非运行证明。

首次新版 `virt-multi-owner-first` 客体 `080004-90853` 已到达首轮真实按键/释放、USB 清理与 Seat Released，并在同 PID13 接纳第二连接；随后原 Managed 未消费的 activation_changed 覆盖 Released→Detach，将已释放轮反复推进 Activate（明确 Closed）。本轮主动停止，保留失败日志，不按纯挂死翻倍。修正限定为已 Released 的 Run 不再由激活通知进入 Activate；来源注销与 Detach 仍由原 owner 推进，无重试表/新状态。`multi-detach-target.exit=0`；修后 `virt-multi-owner-second.exit=0`，`just acceptance-device` release 的汇总 `artifacts/run/20261025-080117-93221-acceptance-device-release.log` 明确 profile device 通过；成功客体细节按默认策略清理，不虚构路径，不为补日志重复运行。此次仅原单键盘路线回归，同 PID A/B/C 并发与死亡仍待后续真实场景；后续显式 KEEP_LOG。impls 已按当前代码同步进程/逐轮/尾部事实。

正式多绑定场景复用同一个 `test_device`、原 Devices/FunctionAccess/OAL 和 QMP 脚本，出生 payload 只选空（原两轮）或 `reuse`（A/B/C）；`acceptance-device-reuse` 使用正式 debug ELF，经真实PID的用户 trap 入口模拟一次非法指令，保存 ELF SHA256/目标/frame/寄存器证据，不给 worker 加自杀入口。GDB 全停不用于计时/公平性证明。该场景以本轮真实 xHCI Ready、尚无 HID 出生作为插入门，不要求 debug Runtime 出现 Waiting；原 release 路线保留 Waiting/IRQ 门。两者不冒充全 hart idle。固定 QEMU 的后 realize 键盘优先路由证据见平台资料；按 B→删除 A 后仍 B→插 C 后 C 的顺序操作，客体逐步确认真实状态，并 Query B 拒串改。

当前失败与定位分层保留：`virt-reuse-first.exit=2` 仅构建入口遗漏 workload 白名单，未运行客体；`second.exit=143`（081158-96596）原 5s 管理期限内 PCI 出生后主动停止、资源未暴露并安全收束；`third.exit=143`（081411-903）诊断 10s 后 PCI/xHCI Ready，但主机仍等原 release 的 Waiting 门，主动终止；`fourth.exit=143`（081639-5982）已按 Ready 插入，HID 得 session 后被活跃期优先周期 QueryBinding 饿死 SET_PROTOCOL；`fifth.exit=124`（082024-13147）删除该重复观察后 HID Ready、A 正式功能可用、slot2 完整枚举，因同父固定目录名 keyboard 冲突而被原错误门收割。主动停止或明确失败不按纯挂死翻倍。第六轮正在验证修复，尚不记 A/B/C 通过。

期限纠正：BUILD 手册自动翻倍规则属于 QEMU 路线防挂死期限，不能据此自动修改服务内部政策。srv_driver 原公共 `deadline()` 的 5→10s 当前为诊断增量，覆盖 Claim 到激活、原停止和调用期限，未改 xHCI reset/halt/端点硬件限值。第四轮记录同一绝对起止：PCI 13.513497→22.068121s（end 23.514085s），出生 18.274409、认证 boot 18.313241；xHCI 22.490464→30.795456s（end 32.490595s），出生 27.743271、boot 27.789790。两者有实际出生/资源/枚举推进，原 5s 不覆盖该 debug 成本；HID 的 33.375907→43.379913s 到期则是独立饥饿，不能归咎慢或再加期限。后续凭完整阶段证据确定最终内部期限。

HID 已删除活跃期周期 Use Query/next_check，初次 Activate 仍核真实 Use，GetResource 仍有停止门，管理者原 Binding Watch→认证 Stop 与 bus Provision Stop 负责停止；原 seq 的 Pending/Unknown 不换序，provider 收完原 TD 才交明确结果。Terminal 后仍查询实际 Use→Released 再退活跃额度/母本。USB Permission/StaleEpoch/Closed 等错误不当安全 TD 结果：记录原错误 Terminal、consumer_stopped=false，停止本地提交，原 provider 保留并收束 TD；只有明确 Complete/Cancelled/Progress::Closed 或从未取得 session 才发布消费者停止事实。PCI/xHCI 自身停止观察保留。多连接登记名改为原 connection 的 keyboard-<connection>，满足 Registry 同父开放实例名字唯一；它只是目录分量，功能名仍 keyboard，授权仍是原 capability/BindingRef。相关目标检查 `multi-hid-progress-target`、`multi-hid-error-target`、`multi-usb-names-target` 退出均 0。

第六轮 `virt-reuse-sixth.exit=1`（082350-15374）已取得 A/B/C 同 PID13、A 原轮 Released 后保 function/键盘 Watch、B 继续按下/释放、C 独立输入与 A 终态；GDB 在注入前读取虚拟高地址失败，未修改目标现场，不是死亡验证。调试读取改复用 qemu_gdb.PHYSICAL_READ，直接映射偏移来自当轮 ELF，Thread/Arc/Shell 字段来自 DWARF，不写 repr(Rust) 固定偏移。第七轮 `virt-reuse-seventh.exit=124`（082716-18804）才真正按 PID13/frame0xffffffc080a85cd0/hart4 注入 cause2；B/C 各自 StopEndpoint/DisableSlot/逐页 DONE→供应 Closed→Seat Released，PID13 一次收集。观察到该进程页已退、旧能力尾部 metadata947，未将其冒充全部退款；原 core 后父设备停止在 init 旧5s总界失败，整体仍失败。注入证据位于 `artifacts/device-fault/20261025T002848.019382Z-18815/`。

第八轮 `virt-reuse-eighth.exit=124`（115757-35218）未到注入：HID 33.921开始、39.527出生/39.605 boot，真实SET_PROTOCOL Ready后在InstallActivation越过43.921s准备截止，44.032s停止；有持续实际推进，不是第四轮饥饿。原OpenFunction竞态断言未输出具体error/槽数，因此不能倒推实际错误。客户端现严格先核op22及空错误正文/零槽，仅合法NotReady/Closed/NotFound/StaleEpoch/StaleInstance表示发现候选失效并保持游标推进；Permission/Invalid/运输失败仍不吞，候选跳过不证明Released。

第九轮 `virt-reuse-ninth.exit=124`（120809-38103）复用、C输入后Query B不串改、真实fault后的B/C Released及单次收集再次成立，证据 `artifacts/device-fault/20261025T040946.054617Z-38114/` 同时含kernel/BootPackage SHA256与原trap现场。父stop start259.245986/end299.245830s，尚未到40s；根PCI Work在stop_end264.627085s附近停于Confirm（reported/stop_acknowledged=true，confirmed=false）耗尽中央旧5s窗口，fault=Unavailable。故整轮仍失败，不能把硬件已停和局部死亡链当总完成。

当前部署期限各有实际owner和起算点，不是硬实时上界；用后续完整链证实当前工作集余量，不按递增数值宣称全链必在限内。重复Stop/查询不刷新阶段期限，硬件halt/reset/端点限值不变：

| 期限 | owner / 起算点 | 覆盖与到期责任 |
|---|---|---|
| 准备20s | Managed首次Claim前 | Claim→Ready→出生/激活→具名功能安装；依据已观测debug成本约10s及运输/调度余量，到期停止原轮 |
| 出生10s | ProcessTask开始loader前 | 同一次出生与认证boot；失败封复用、唤醒实际关联Run |
| 普通RPC5s | srv_driver每次调用 | 与阶段剩余期限取min；Unknown留原调用/轮次，不重签 |
| 中央Binding停止20s | srv_device Work首次Stop | provider Stop/报告/Confirm/Closed及实际依赖释放；到期保留并升级，不伪造Released |
| Managed停止30s | 原Run进入Stopping | 观察中央释放及原订阅/RPC脱离，保留进入故障终局的余量 |
| 装配观察40s | init首次可使srv_driver退出的Stop前 | 同一绝对界传入Stop、Seal、QuerySourceStop、sleep上限与manager DONE；到期仍持原记录交故障入口 |
| native收集 | 原RootSupervisor既有监督策略 | 在上述DONE之后分别collect_process/collect_job；不冒充包含于40s，另记完成时间 |

用户报告过外部cyber_policy/Daybreak提示，但当前代理可见记录没有对应拒绝tool_call，具体动作和执行前后边界不可定位；不归因于任何已执行代码/测试，不试探改写请求。已核无本次QEMU/GDB残留，已完成代码和失败/局部证据保留；继续已授权本机客体验证，只有实际可见受阻动作才标注未执行。

### init设备管理者的独立观察与终局施工合同

该设计已由协作监督裁定，接续同一闭包。RootSupervisor的Vec、Runtime/Assembly、收集账本只由主线程访问；具体观察线程通过libruntime::thread（下层libthread）和独立WaitSet得到调度，不要求同步Caller/wait推进原Assembly。它只持缩权的两管理者Process READ|WAIT owner和必要故障authority，固定两份观察/正常退出交接记录及单调装配gate。锁仅保护短记录提交，spawn/Start/RPC/wait/join均在锁外。receive_into使用固定数组不等于内核Receive无分配，错误直接升级。

libprocess原loader分成affine未Start准备owner和显式Start，普通spawn仍组合同一实现。Prepare保留原GrantOutcome和取消/清理责任；Start前准备线程、WaitSet、固定存储、Job/reset权限，将真实缩权Control挂入DONE并提交原观察记录后才Start。初始管理者可以在bootstrap之前死亡；srv_device根SourceAdmin在私有认证回执后立即移交独立副本，之前以Process/Job/reset兜底；srv_driver原SourceAdmin可在Start前持好。不新增设备专用loader回调或另一spawn路径。gate封口后的迟到创建/Start由真实JobSeal拒绝，封口前已接纳记录仍纳入同一故障责任集合。

正常停止前登记原实例意图及绝对期限，不整体撤防；DONE总核真实cause/detail。Completed先于主线SourceStop::Closed时保留待核验，等同次证明或期限；异常在Stopping中仍升级。srv_device先取得根Closed，再请求观察线程显式关闭自己的根admin并确认，主线才关闭最后root/admin，否则监督副本会自阻服务退出。两管理者逐项确认原证据、注销来源/关闭cap后才撤防、join并核线程registry实际收束；不把join自身当栈/metadata退款。device Manager记录迁入原RootSupervisor持有，stop按借用推进，失败原记录恢复，不由消费self或run局部析构丢owner。

FailureTerminal仅一个动作owner，主线与观察线程争同一个不可逆gate/authority。先JobSeal封迟到工作，来源仍可响应时有界Seal原来源，死/Unknown不重建表或无限等回复；平台根Seal只封新使用且保留可能活DMA。受影响执行复用原JobCollector枚举/停止，设升级截止，不复制worker表，不把JobSeal/ProcessKill提交写成全系统已停止。无需等所有Pool退款或Job完成即可Reboot+SystemFailure（内核ColdReboot）；reset错误立即终止整个init→boot DONE停驻。原serve_forever的Shutdown/无限清理循环不作为此路径，任何一方不得并行第二次reset。观察线程只在正常明确撤防后返回，错误不留ResultCell等主线日后取；panic/OOM/用户fault本身已终止init。

验证沿原实际消费者：主线程长同步wait分别杀两个管理者、Start后立即死、Stop回复前DONE、Stopping异常死、admin副本的关闭确认不自阻正常退出、故障真实QMP ColdReboot边界及reset拒绝后init DONE/DMA保留。第七/九轮只覆盖普通HID死亡，不能代替上述管理者/物理主控故障门。
实现与验证：`PreparedProcess` 使用原 loader/GrantOutcome，普通 spawn 组合 prepare/start；具体观察线程已接两管理者 Start 前 DONE、私有 bootstrap 后根 admin 交付、正常来源 Closed→admin 关闭 ACK→真实 Completed→撤防、唯一 FailureTerminal。终局同步观察等待取原请求期限与总升级界的 min，到总界直接升级，不向原 Collector 提交伪造的提前 TimedOut。正常 join 后原线程 registry 实际退到仅 main，Shared 仍持 Terminal 的 Job/平台 Seal/reset 副本，供后续主线失败使用，不代表所有监督 cap 或 metadata 归零。

`observer-host-first.exit=0`：现有 libprocess/libthread host 共9项通过。`device-observer-target-third.exit=0` 是前一版目标 Clippy 开发证据。`observer-release-first` 已实际完成两轮热插，但 test_device 正常分支末尾误无条件进入 reuse，主机正常脚本无后续插入，故主动停止并保留 `artifacts/device-platform/observer-release-first.reason`；不属于外层撞限或观察线程故障。删除末尾调用；显式 reuse payload 分支执行后 return，不落普通两轮。

`QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-device`：`observer-release-second.exit=0`；入口 `artifacts/run/20261025-132647-58261-acceptance-device-release.log`，客体 `artifacts/qemu-acceptance-20261025-132650-58634.log`。Device/Driver 均 Start 前准入，正常两轮数据/IRQ 后，Driver stop30.479s→DONE31.388s，role1 admin 关闭 ACK、Completed、撤防；Device 根 Closed 后 role0 admin 关闭、Completed、撤防；观察线程 registry 仅 main，worker Pool delegated 两维为0，原 core 和 explicit shutdown 完整通过。该证据不是运行期管理者死亡或故障 ColdReboot 证据。

新版复用第一轮 `observer-reuse-first.exit=1`：客体 `artifacts/failed-acceptance-20261025-132808-60528.log` 到达 A/B/C 同进程、A 尾部/B 继续、C 输入和真实 HID fault 后两 Seat Released/一次进程收集；后续原 core 仍推进，280s 主机 QMP route 外层期限触发 `device QMP route deadline exceeded`。按用户规则将 Justfile 唯一 `VIRT_DEVICE_TIMEOUT` 280→560 并立即重跑，实际命令已核 `--timeout 560`；不修改设备内部政策、不缩场景。

`observer-reuse-second.exit=124`：客体 `artifacts/failed-acceptance-20261025-133320-70357.log`，同一局部复用/死亡链成立，原 FS 测试在最后 shared Binding quota 后进入有限 binding::check，init 等 COMPLETE 的 `test_fs::command::PHASE_TIMEOUT_MILLIS=120000` 到界。该值是双向命令/报告/最终收集共用的测试防挂死预算，不是 FS 业务租约或设备停止期限；超时前连续完成双 provider、读写/配额/退款/Rename 等场景，没有具体停滞证据。按共同验证口径将此唯一值 120→240s，并加实际阶段起止及 binding 场景开始/结束绝对时间；后续验证不改 FS RPC、硬件和设备20/30/40政策。

`just acceptance-device-manager-fault Driver`：`observer-driver-fault-first.exit=0`，入口 `artifacts/run/20261025-133816-78584-acceptance-device-manager-fault-Driver-wait.log`，真实注入/ELF 指纹 `artifacts/device-fault/20261025T053854.215121Z-78985/`。主线程已进入原客户端同步 wait，实际 pid9 用户 fault 后观察线程 query 到 DONE/cause5，唯一 FailureTerminal 提交 JobSeal、原来源 Seal 和全部五平台根 Seal；原 JobCollector 推进后 Reboot/SystemFailure，主机单独收到 QMP guest RESET，再发送 quit 清理。主线随后因 client 退出失败没有发出第二个 reset；不是以主机终止冒充重启，也不证明活 DMA 退款。Device 管理者同场景继续取证。

`just acceptance-device-manager-fault Device`：`observer-device-fault-first.exit=0`，入口 `artifacts/run/20261025-133935-79437-acceptance-device-manager-fault-Device-wait.log`，真实注入 `artifacts/device-fault/20261025T054012.722784Z-79805/`。pid3 真 fault 后，观察线程核 cause5；两来源因管理者已死准确记录 Seal unconfirmed，未重建来源或冒充 Closed，Job 和五平台根实际 Seal、原执行收集、唯一 Reboot/SystemFailure 与 QMP guest RESET 成立。没有以本轮 CPU 收束或 reset 推导 DMA 退款。

`QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-device-reuse`：`observer-reuse-third.exit=0`，入口 `artifacts/run/20261025-134052-80242-acceptance-device-reuse.log`，客体 `artifacts/qemu-acceptance-20261025-134056-80609.log`。完整 A/B 同PID、停A保旧入口而B继续、C同PID独立输入、实际HID fault→B/C各自Released→单次收集，随后原FS/core、三层停止、两管理者admin ACK/Completed/撤防、线程registry仅main、worker Pool两维delegated=0及Shutdown均完成。FS COMPLETE wait126.760159→254.898013s（128.137854s），最后binding场景247.123746→254.895095s（7.771349s），与旧120s失败构成测试预算不足的正反证据；不再扩大此调查。Driver stop304.408787→SourceClosed312.178171→DONE312.661029s，当前20/30/40部署工作集有观察余量，不作为WCET。直接双键盘复用不证明复合USB接口支持。

Start最早窗口的首次操作 `observer-device-start-first.exit=1`：操作者在真实Job::start_commit_gate按DWARF取process参数，但该借用Arc位于当前内核栈窗口，误当ELF直映地址换算，GDB明确读错并在注入前失败；没有用户fault或成功证据。已将借用参数改为命中hart当前有效栈映射读取，后续Shell/Thread稳定直映数据仍按原物理读路径；保留 `artifacts/device-fault/20261025T054704.590408Z-91658/`，第二轮继续。

当前新版独立复用、两管理者长等待/最早Start/Stopping故障、真实xHCI DMA的reset拒绝保留与提取后core DMA回归均已通过；调用者出生峰值和Copy测试预算不足已由具体失败及修后运行闭合。当前按下文Drivers公开投影接线继续，完整设备闭包仍保留候选、费用/Unknown及精确交错门，未执行或操作者失败的场景不计通过。

最早Start门：`observer-device-start-second.exit=0`（入口 `artifacts/run/20261025-134808-92130-acceptance-device-manager-fault-Device-start.log`，原始 `artifacts/device-fault/20261025T054811.379646Z-92493/process-fault.log` 与同目录evidence）；`observer-driver-start-first.exit=0`（入口 `artifacts/run/20261025-134918-92779-acceptance-device-manager-fault-Driver-start.log`）。调试器从VM启动前预装真实Job::start_commit_gate断点，以DWARF参数取原Shell/PID，记录PRE_START后才允许该目标第一次真实ecall cause8→2；不是准入日志之后attach推定首条。两者均Start前DONE已注册、fault/DONE、唯一封口终局与QMP guest RESET。Driver门同时覆盖Start前已持SourceAdmin、Device门覆盖私有根admin尚未交付的早期兜底。

reset拒绝：`observer-reset-rejection-fourth.exit=0`；入口 `artifacts/run/20261025-140342-3360-acceptance-device-reset-rejection.log`，[三阶段原始GDB观察](../artifacts/device-fault/20261025T060421.224698Z-3792/process-fault.log)与[ELF/BootPackage指纹](../artifacts/device-fault/20261025T060421.224698Z-3792/evidence.json)。本轮主线client同步wait、xHCI DCBAA=0x86537000/CRCR=0x86538001/ERST=0x8653a000/events=0x86539000已实际交付，早于后段FS/core退休消费者；因此GDB初始非空集合是这四份实际xHCI环页，非无关遗留core pin。拒绝真实ColdReboot/SystemFailure SBI ecall并返回NotSupported后，init abort→boot DONE→四hart停驻，三阶段保持id4..7、同backing/页范围0..1、实际usage/write、无acquisition/release且Work Idle，无QMP RESET，再由主机quit清理。证明物理安全未知时责任/写许可不因init死亡或停驻消失，不证明设备DMA已停止。

观察体由原check-dma-retention提取为qemu_dma_gdb，core仍严格两mapping/每份两页与8192字节，设备模式用实际非空集合并逐项校验。首轮reset操作已取得同四页三阶段细节，但主线合法先赢唯一gate，旧主机额外等待observer DONE日志导致滞留，主动清理并留reason，整入口未记通过；后两次是预算诊断代码的编译错误、没有QEMU执行，修正库查询接点后第四轮实际通过。

Stopping开发失败：`observer-driver-stopping-first/second` 均未到故障注入，原正常debug第二轮HID出生失败；第二轮入口 `artifacts/run/20261025-140516-4126-acceptance-device-manager-fault-Driver-stopping.log` 精确记录父Pool metadata available63609/allocated67463/delegated524288 slots，8B/slot，故可用508872B；ProcessWrite请求614978B返回QuotaExceeded，Grant Retained、cleanupNone，新子Pool取消后全空。旧pid13已收集但旧入口使其Pool仍有514 slots尾部和整份委托，不能用allocated_pages=0声称预付已返。两个失败均主动清理且保留reason，不属于外层撞限。

成本修正只补调用者临时付款：ProcessCreate/initial_map/目标grant表用新子Pool，调用者handle安装、ProcessWrite临时MetadataBuffer及Grant输入缓冲使用caller Pool。LOADER_WRITE_BUFFER_BYTES保留原1MiB分块的唯一真值并公开给装配计费；实际硬件配置为manager常驻+三活worker+一次退休尾部的原五份WORKER_POOL，再加一次串行loader写入峰值metadata，页数仍80MiB，metadata5→6MiB；零ECAM根不预付不可能发生的worker出生峰值。不是第六份worker、无限出生保证、capacity改义或Quota旁路。`observer-budget-target-third.exit=0`；当前原失败 `observer-driver-stopping-third` 正在运行，仍核新子出生、后续真实停止意图下异常终局。

Stopping第三轮（入口140909-5941）已实际越过原Quota点：父metadata可用194662 slots，新子仍完整4096页/131072 slots，第二轮HID Ready。随后原FS并发Copy旧断言只说明健康任务未Complete，原Outcome未打印，未到停止注入；保留失败并清理。第四轮 `observer-driver-stopping-fourth.exit=1`（入口 `artifacts/run/20261025-141935-10374-acceptance-device-manager-fault-Driver-stopping.log`）具体Outcome为Transport(Timeout)/cleanup_pending=false，local_read和transported均23040，source_terminal Completed/Ok，destination_terminal None；原取消任务已通过Cancelled/非零前缀断言。不能把健康任务超时写成取消传播，也不能从transported推断目标已提交全部字节。

Copy脚本的整次FollowBudget与绝对期限同为原5s测试防挂死预算；只在该Copy模块以唯一常量翻倍至10s，保留一次起算且不在ReadAt后续期，全局FS RPC/provider租约和设备政策不变。第五轮实际健康Complete、目标confirmed/terminal齐全：任务234.792706→finish241.410482s约6.617776s；ReadAt同步238.299861→238.357710s仅57.849ms，总体debug Copy成本越原5s，不能把主要耗时归因这段暂停。该测试预算不作性能/WCET要求。

`observer-driver-stopping-fifth.exit=0`，入口 `artifacts/run/20261025-143011-13263-acceptance-device-manager-fault-Driver-stopping.log`，[故障原始现场](../artifacts/device-fault/20261025T063503.155044Z-13669/process-fault.log)与同目录evidence。真实Driver Stop intent之后、Stop RPC完成之前fault pid9，独立DONE核异常cause5，唯一Terminal→原封口→Reboot/SystemFailure→QMP guest RESET，异常不因正常停止意图被忽略。本轮PCI四资源Describe/Get配对、两轮键盘、原FS组合和修正后的调用者出生峰值均有实际证据；这是预期故障路线，不代替完整正常shutdown。继续Device停止交接窗口和提取后原core DMA观察体回归，已过正常release/完整reuse不重复。

`observer-device-stopping-first.exit=0`，入口 `artifacts/run/20261025-143900-17382-acceptance-device-manager-fault-Device-stopping.log`，原始 `artifacts/device-fault/20261025T064344.463812Z-17762/process-fault.log` 及同目录evidence。Driver先正常SourceClosed/Completed/撤防，Device正常停止意图后真fault/cause5，主线终态核验亦报错，唯一Terminal与QMP RESET成立，未把异常当正常退出。

提取后原core工具回归 `observer-core-dma-first.exit=0`，入口 `artifacts/run/20261025-144520-19374-virt-dma-retention.log`；[原始GDB观察](../artifacts/dma-retention/20261025T064526.502321Z-19744/gdb.log)和[指纹](../artifacts/dma-retention/20261025T064526.502321Z-19744/evidence.json)。保持原严格两份mapping/每份两页8192B，id9/10、range0..2跨reset拒绝/init DONE/四hart park完全相同且Idle；原断言没有为共享观察体放宽。当前监督增量的上述正常/早期/运行期/停止故障门与工具回归已完成，不重复；原精确Unknown/费用/公开投影等剩余门继续由本篇承接。

#### Resources OAL投影接线选择

BindingControl/Use 的 OAL 根直接是本轮 Frozen 的只读资源目录。根及每个叶项的 ObjectInfo 携带原 BindingRef；叶项仅投影现有 Descriptor 的 kind/固定 rights/protocol，不返回能力或 Boundary，完整 opaque metadata 仍与显式 Get 的真实能力配对。纯 Devices Observer 不因此获得资源目录入口。Stop/来源封口后拒新资源视图读取；Preparing 不发布残表，Released/旧 context 不重定向新轮。实际能力仍只能从 Binding::resource 离开原 owner，其不可逆暴露位与原确认合同保持不变。

枚举按冻结名字顺序每页至多一项，游标只保存原 BindingRef 与原表项索引，不存服务端会话或第二份树。每次恢复重新核相同原轮、当前入口及整表可用，索引用于从同一 Frozen 恢复已交付名字；游标不授予权限。资源名上界从 OAL 含固定游标/一项摘要的最大页运输布局推导，Entry 编码/接收共用该唯一上界，使已准入名字总能交付，不增加任意重排/分表机制。验证覆盖单项续页、跨轮游标、叶子额外路径、超长名拒绝以及零cap/停止门，真实 worker 随后消费相同视图。

实现已接：Frozen只读投影与Binding/Use角色分派，Entry准入共享页布局推导的名字上界；Preparing→Busy、Stopping/Released→Revoked、跨原ref或坏游标→CursorInvalid、未知叶→NotFound、叶后续分量/Enumerate叶→NotDirectory。查询/停止原路径仍可用，任何观察错误不产生Released。PCI的原pending记录先Describe再Get，核原ref/零槽及Descriptor配对，实际cap kind/rights/HostInfo/几何继续独立验证；Stop先到不再从Describe发Get，原停止owner收部分能力。`resources-host-consumers.exit=0`覆盖libdevice/libdriver/libpci/libusb/libhid，含新两项独立页协议与边界名测试；`resources-consumer-target-second.exit=0`。143011-13263真实路线逐项记录四资源Describe/Get matched，且ECAM/后续xHCI和两轮键盘成立。资源视图Stop/旧context的精确运输交错仍随原剩余门补证，不以host或正常数据点替代。

#### Drivers公开投影接线合同

libdriver拥有运行实例观察身份与ObjectInfo编解码；原ProcessId整体替换为InstanceId=(管理Mailbox incarnation,出生前checked serial)，同一World使用这一个身份关联Run/ProcessRecord，不并存映射表。PID、serial、路径与tag只观察，不授予权限。srv_driver从原processes/runs直接投影：根按名字给process-<16位小写hex serial>，实例下有bindings，绑定叶按binding-<设备serial hex>；实例给原key/PID/声明容量/实际关联数/原接受与CPU收集事实，绑定给原BindingRef/管理阶段/实际Released观察。直接来源关系仍只在原Run.process，成员消失可跳过或NotFound，不保存快照/第二成员表，不由CPU DONE推导Released。

Control与Observer采用两个已有Mailbox、两个原Ingress实例及同一World/Runtime/Request/Outbox。依据kernel CoreState::admit只看共用16槽、不按context保留，独立队列使公开满载不能挡Control运输。boot Ready精确携两sender槽和两Mailbox KOID；发送/采用逐项核实际kind/rights/认证context，init保留Control、只委托Observer；worker既有单slot boot不变。OAL零cap/noBoundary，Observer发管理opcode仍Permission，不能凭协议/tag自授角色。

沿原总请求事实更名outstanding_requests，真实control context认证后才可用管理余量；公开最多REQUESTS减去一项停止和一项状态观察的两份预留，Control可用总上限，不加业务镜像计数。TASKS/WAIT_SOURCES与两个Ingress的Receive/MessageStorage、原Outbox来源按实际多一入口同步预付；其余任务仍受原DEVICES/PROCESSES/请求配置，公开流量不能消费这两份底层任务/来源准入。公开Outbox取min(调用者绝对期限,原5s服务回复占用界)，wire仍回原deadline；5s是资源占用政策而非性能要求，INFINITE不无限保活已接受回复。

停止沿原Manager第一次stop_notified唤醒公开Ingress，先移除其原来源再关闭公开Mailbox owner，关闭失败返还原owner并升级；不等待外部Observer别名关闭。旧Outbox由原期限/关闭回执收尾，停止/满额时未准入公开消息关闭原回复权，不另建拒绝运行体；私有Control仍可观察/推进停止。真正服务退出仍由原Run/Process/请求/RPC责任收口，观察别名不新增退出前提。公开绑定期名称是弱一致定位，旧Context/游标不能跨manager incarnation。

真实消费者同步test_device的实际出生grant与Namespace：Devices/FunctionAccess继续原scope，另取只读Drivers。A/B同实例及PID、A退/B存、C加入同实例及Bindings关系由该真实视图核对，主机PID日志保辅助证据；另核Observer不能Stop、零cap、旧实例消失/停止及公开满额下Control仍可用。无新fixture或测试worker，所有协议发送/采用/失败/Grant消费与退出关闭随原owner统一迁移。

代码已接上述两入口、InstanceId及直接OAL投影，init严格采用两KOID/两槽并只向test_device交Observer。客户端通过实际视图核A/B/C关联与Observer拒Stop；另由init保留两个真实回复Mailbox，只交其Sender给客户端发行Once，每箱填16条零cap回复。客户端退出后init仍不读取回复，先执行原Control Query，并保持邮箱至原Control Stop/管理者收集结束；这验证已发送的公开回复不成为服务退出条件，不引入测试服务或另一运行体。验收客户端的具体槽由test_device库拥有，Devices/Drivers两业务槽与两个回复授权槽分别校验；worker单槽boot不变。

付款开发证据：`once-check-first.exit=0`（just check）、`once-consumers-target-third.exit=0`及`once-backlog-target-first.exit=0`、`once-notification-target-first.exit=0`为目标Clippy；`once-drivers-host-first.exit=0`覆盖libdevice/libdriver/libkernel/librpc现有host用例。实际付款验证复用原test_target及原loader/RootSupervisor：init账户A拥有目标Mailbox、原Sender并实际Send，独立进程账户B发行Once且保留观察壳；额度压低后核Quota返原Once，再发送四份完整正文，分别观察未读/只留Delivery/关闭Delivery费用，最后原进程收集后B两维零；普通Sender仍不借B付款。未运行的交错不得由这些结构或编译结果宣称通过。

`once-drivers-reuse-first`整体exit124是内部夹具准备错误，`artifacts/failed-acceptance-20261025-163037-32250.log`647报Once ready grant failed：本地signaler未DUP却复制，付款主体未执行；修正仅本地母本增加DUP、子grant仍SIGNAL。此前同日志已取得Drivers instance3/PID13的A/B/C实际关联/退出与HID故障解绑，保留局部证据。`once-drivers-reuse-second.exit=124`（`artifacts/failed-acceptance-20261025-163730-41801.log`110）在设备/付款主体之前暴露旧Notification测试立即Receive(2)假定两提示已同时到达；实际来源Closed与有界Work逐项hint不是同一完成点。测试沿同一5s绝对防挂死预算使用原WaitSet.wait/receive逐项核两token Closed并remove，不加sleep、不改内核等待合同。两失败均非外层路线撞限，未翻倍生产/路线期限。

`once-drivers-reuse-third.exit=124`（`artifacts/failed-acceptance-20261025-164010-42824.log`）完整A/B/C公开视图/HID fault/各原Seat Released、32份Once逐份Sent、Control Query与Driver Stop/收集期间未读积压已成立；付款主体652-654给独立Pool25364的before63831→queued65887→Delivery尾63827→retired63767 metadata slots及最终两维零。整体在728失败于新夹具原Pool辅助entry未显式关闭，未通过root所有权终检；修正使用原root.close_control(pool)，不放宽终检。该版尚未覆盖发行者在未读时先死亡，不能与下一版本证据合并。

新版`once-drivers-release-first.exit=0`，入口[完整运行](../artifacts/run/20261025-164600-52071-acceptance-device-release.log)、[客体原始日志](../artifacts/qemu-acceptance-20261025-164610-52561.log)。付款夹具改到原launch取得target image后运行、结束即关原账本Pool；A拥有Mailbox/原Sender并执行Send，B真实发行。Quota时携带能力的原owner/rights保留（失败在payload分配前，不冒充中途move claim回滚全覆盖）。B在四份回复仍未读时实际Kill并由原collector收集，Pool2384 pages=0而metadata=2120 slots；Receive后只保留四Delivery为60 slots，全部finish后为0，最终两维allocated/delegated均零，普通Sender仍A付款。queued65887/before63831是原进程存活阶段，不要求与retired0相等。153-154两箱各16份回复逐份Once DONE，init保留全部未读至406 Control Query和613 Driver正常Stop/收集完成后；随后关闭两个接收owner。两轮同端口按键/IRQ/原Seat释放、worker委托两维0、两管理者正常admin/DONE撤防、registry只留main及最终shutdown全程通过。Native结果不承诺allocator硬字节隔离或所有回复必成，用户态Request/缓冲/来源仍服务总量预留付款。

`drivers-view-host-first.exit=0`含最长实例key/完整游标/单项ObjectInfo的实际OAL运输上界、跨incarnation/原Binding epoch与Terminal不等Released反例。`once-retention-target-first.exit=0`为新版目标Clippy。完整`once-drivers-clippy`前四面通过，user-host只报libpci::host/libdriver::boot测试mod后仍有实现的布局lint；测试块移至文件末尾，第二轮结果另记，不修改行为。
#### 有限候选与显式Retry的接线合同

当前Run的implementation是同一不可变Catalog内严格降优先级游标。Catalog.select统一接受previous；previous越界或其identity不匹配即Invalid，下一个最高优先级仍须唯一，同级不以索引/key破平局。原Run记录首个有政策意义的失败原因，随后Stop/取消清理不覆盖；正常Withdraw、来源Stop或管理员Stop不产生自动fallback。Manager只在原Managed已Terminal/task=0、Binding安全Released或明确未提交Claim、两类订阅/RPC退场且process关联解除后，重新QueryDevice核原DeviceId仍present/无活Binding及最新可用epoch，再用原Run推进下一候选；不沿旧Use/ref更换实现，不保存attempt集合或第二占用表。

Quota/Resource/Busy/NotReady保持Terminal，不因普通discovery_dirty、Watch或自身清理通知重试同候选。没有精确相关资源变化来源时只等明确管理Retry，不加轮询。Control新增非幂等Retry(DeviceId)，Observer无此权限；请求先在原Request/Outbox准入，再借原Dispatcher QueryDevice。Query前保留同一原Run、BindingRef与候选索引，返回后在同actor重核仍Terminal/task=0、无已提交Claim责任/订阅RPC/进程关联，且记录和候选未被替换；另一Retry或fallback后的记录不能被迟到结果清掉，不存操作历史。成功只表示本地纯选择记录重置已提交并唤醒原Manager，不保证来源仍开放、设备仍可占用或新激活成功。QueryDevice的present/epoch只是取样，不是跨RPC冻结事实；后续Claim在srv_device提交点最终复核真实来源/设备门。重复请求可Busy/NotFound/Conflict；Unknown禁止盲重发，调用者先观察实际状态再作新的明确政策决定。本actor已进入World.stopping即拒绝。远端Seal先发生但通知尚未到本actor时，允许纯终态记录被清除，之后原Claim仍必拒绝新责任；Watch只推进本地停止，不充当安全门，不新增来源查询或跨服务协调器。

首批每类只有一个真实实现；独立host验证完整有限顺序、下层歧义及previous非法，Manager最小边界核终态原因/Released与Detach门。第二候选真实集成未执行时明确保留边界，不造假驱动、不以单实现正常路线抵消该缺口。

本轮接线补充：自动fallback只消费真正Released且已Detach的失败Run；Claim被明确拒绝/Unsent而未提交的选择终态不自动换实现，只允许管理Retry。准备总窗口耗尽记录NotReady并保留终态，不能把调度/依赖成本误判成实现不适配而自动换候选。原Run首个导致停止的原因由fail保留，后续正常停止与清理不覆盖；Retire提交前核本地Control/Use/激活入口/调用/订阅/进程关联均已脱离。Retry只在原Request中持一次待核QueryDevice txid及原Run/ref/implementation，不加任务、账本或查询Watch；Outbox先结束时取消原查询并等Dispatcher实际注销后才退Request计数。CALLS按最多REQUESTS个管理Retry查询扩容，RPC_SOURCES/WAIT_SOURCES沿原配置派生，避免只留逻辑请求数而遗漏底层准入。

`candidate-host-first.exit=0`包含严格降序、最高u32优先级、零优先级最后一项、下层歧义及previous越界/identity错配；`candidate-target-first`、`candidate-retry-target-first`和`candidate-consumers-target-first`均目标Clippy exit0。实际消费已接Observer发正确Retry正文仍Permission，以及init Control对真实活PCI Binding的Busy反例；`candidate-reuse-first`当前运行中，成功重置纯Terminal、迟到Query防串记录和Seal提示未达反例尚待最小边界验证，不能由编译或host选择顺序替代。

`candidate-reuse-first.exit=0`，入口[组合运行](../artifacts/run/20261025-171321-56346-acceptance-device-reuse.log)、[客体日志](../artifacts/qemu-acceptance-20261025-171327-56772.log)。Observer正确Retry正文仍Permission；真实A/B/C依次投影到instance3/PID14，A退后B仍在、C加入（248/307/347/417），HID fault后各自原Seat Released且PID14只收集一次，485完整复用通过。492真实Control Retry对仍活PCI Binding返回Busy，494 Query及696原Stop/收集期间仍保留两箱32条未读回复；Once2413发行者死亡2120→Delivery60→0、正常监督/两维worker退款及最终shutdown均通过。此版本不包含随后‘自动fallback必须真Released、准备超限归NotReady’的窄政策修正；该修正不从无Claim拒绝或部署成本推导候选不适配，后续目标/针对门另记。该运行只验证单实现与拒绝门，不证明正向Retry或实际第二候选。

显式负装配变体`acceptance-device-candidates`复用原reuse/QMP运行体，compile feature只追加真实PCI映像与USB identity的高优先级不适配声明（无Registrar/物理角色），正常三声明与五份Pool不变。原libdriver::Activate先核真实manager context，再在take Use/Registrar与任何资源Get/MMIO前核固定identity/Registrar角色；不匹配明确Permission，诊断同时保留原BindingRef及两个匹配结果，实际日志PID区分该负候选与正常PCI父进程。拒绝不取消provider Prepare已有USB硬件责任；原Stop/Provision/Binding释放门与Managed清理继续完整收束。主机逐条核该候选没有越权角色或硬件使用，原USB供应Closed→Binding Released→Managed纯终态→同DeviceId更高epoch Claim→真实HID输入，高候选同选择轮不再试，实际旧PID只收集一次、最终原Pool退完。这不表示存在第二个有效HID实现，也不给worker添加验收模式。`candidate-negative-target-first.exit=0`、脚本ast解析与Just dry-run是开发证据，实际组合结果另记。

负装配入口前两次未执行客体：`candidate-negative-first`在Just把带斜线feature值解释为路径时失败（[原日志](../artifacts/run/20261025-175816-70180-acceptance-device-candidates.log)），`candidate-negative-second`在通用output转写命令时丢失该值的引号而失败（[原日志](../artifacts/run/20261025-181140-71690-acceptance-device-candidates.log)）。修正在设备验收私有recipe内传递原feature参数，不更改通用output协议；第三轮才实际构建/运行。dry-run不作候选或硬件证据。

第三轮`candidate-negative-third.exit=124`在srv_driver出生时严格拒绝声明/映像槽不匹配（[原日志](../artifacts/failed-acceptance-20261025-181246-72587.log)141），init发送端原来固定三映像，改为同一images集合逐项生成grants并预留全部槽，接收校验不放宽。第四轮`candidate-negative-fourth.exit=1`（[原日志](../artifacts/failed-acceptance-20261025-181453-73805.log)）实际到达pid14原USB Claim/Ready/Activate，249由既有固定identity/角色门明确Permission，原USB供应仍完成StopEndpoint/SetDequeue/DisableSlot/退休至268 Closed；269主机仍预期不可达的提交后Unsupported而判失败。删除该不可达PCI分支及仅供它的identity getter，保留原Permission与权限门，在原门补精确拒绝诊断供第五轮核证；不将该轮Closed外推Released/真实fallback。

第五轮`candidate-negative-fifth`已到负候选pid14 Permission、原USB Closed(epoch1)/Released/Managed Retire/进程及Pool零、同DeviceId3新epoch2真实HID Claim，但重新Prepare期间原Device::new还没有keyboard descriptor，发现循环将interface(None)误当物理撤出，325 Withdraw后新Binding以未暴露取消安全收束；未到真实HID输入，按已确认逻辑错误主动收割，原日志`artifacts/failed-acceptance-20261025-181725-76231.log`保留。修正连接存在从原Device.disconnected+真实PORTSC CCS推导，不新增状态；描述符未就绪不撤出原登记。

第六轮`candidate-negative-sixth.exit=0`，入口[完整组合](../artifacts/run/20261025-182125-82484-acceptance-device-candidates.log)、[客体日志](../artifacts/qemu-acceptance-20261025-182130-82913.log)。249/400/597三个实际不适配PCI PID14/16/17逐项在原身份/角色门拒绝，原USB Closed→Binding Released→Managed脱离后278/428/625分别在原DeviceId3/4/5从epoch1推进epoch2，真实HID单PID15完成A/B/C独立输入、A退/B存/C复用及fault后的两Binding收束/一次收集。无负候选Get/MMIO/Registrar或物理角色，所有旧PID各收集一次。960原worker Pool两维delegated=0，972观察线程registry仅main，998显式shutdown；1006主机核全部三条候选链。该版不含随后描述符缺失时取消的usb_stopped修正，不将成功重枚举当取消证据。

同一语义修正覆盖停止：Controller::usb_stopped只查询同connection原Device的Stage::Closed，该阶段仍由原端点/slot与逐页退休流程产生；缺记录返回错误，绝不当安全Closed。Supply停止直接消费该事实，删除无消费者的Interface.stopped投影。`candidate-cancel-target-first.exit=0`；同一显式候选QMP路线追加第一连接新epoch Prepare实际Reset->WaitReset后拔出，要求原Stop的descriptor_ready=false、原供应Closed→Released/Managed退场和主机DEVICE_DELETED，再同端口重插接原A/B/C及退款。未命中描述符缺失窗口即失败，不降判据。运行结果分版本见下文。

取消首轮`candidate-cancel-first.exit=1`（[原日志](../artifacts/failed-acceptance-20261025-182736-94924.log)）：286同connection1/新epoch2确已Reset->WaitReset，287-289主机删除完成，293-299真实逐页退休/Closed；300主机因缺descriptor_ready=false诊断拒绝。断开先由原Preparing的usb_prepare返回Closed并经Provision.failed进入Stopping，早于外部Stop RPC；统一log_stopping同时记录这一路径及原父停止/Stop，不增加状态。实际已清理与整体失败分别保留，不作timeout。

`candidate-cancel-second.exit=0`，入口[取消组合](../artifacts/run/20261025-182958-96752-acceptance-device-candidates.log)、[客体日志](../artifacts/qemu-acceptance-20261025-183002-97151.log)。原DeviceId3 epoch1负候选安全释放后，279-286同connection1/epoch2重新Prepare的Reset->WaitReset才触发拔出；291真实Stopping的descriptor_ready=false，随后原Closed/Binding Released/Managed退场至303，主机DEVICE_DELETED另行确认，再同端口重插。新DeviceId4/5/6的负候选分别安全交接真实HID，完整A/B/C输入/旧尾部/HID故障/单次收集、1083原Pool委托两维零、1095main-only、1121shutdown通过。旧身份没有因重新准备或同端口重插恢复。该运行覆盖描述符尚未就绪时取消；不拿成功重枚举代替它。

正向Retry增量复用普通reuse路线与init原Control：客户端完成HID fault后的各Binding Released/Detach并关闭旧入口、由原collector收集后，init观察原manager Pool仅剩两份物理worker委托，再有限枚举仍连接且无活Binding的最新键盘C，发一次Retry。原回复严格核PID/opcode/空正文零cap，Unknown不重发。成功只证明本地纯终态选择重置，不承诺随后Claim/激活成功；QMP保持B/C连接直到该结果才删除。活PCI Busy和Observer Permission反例继续保留。无管理cap交给只读客户端，不新增观察owner、选择表或进程。退款等待与枚举共用一次绝对30s验收防挂死界；Retry沿该绝对剩余界，服务RPC/硬件政策不变。`retry-positive-target-first.exit=0`，实际运行待前述主控故障门结束。

`retry-positive-first.exit=0`，[完整入口](../artifacts/run/20261025-184040-10438-acceptance-device-reuse.log)、[客体日志](../artifacts/qemu-acceptance-20261025-184044-10804.log)。493/495原C device5 epoch1实际Released/Managed退场，499/500客户端分别核B/C后退出并收集；502活PCI Retry仍Busy，503同actor提交原device5/epoch1纯选择重置，504 init一次成功收空正文/零cap回复后主机才删除B/C。原真实A/B/C同PID14及fault收束/一次收集、Observer积压下Control Query/Stop、715原worker两维委托零、727main-only及753shutdown通过。不将成功重置外推新Claim/新功能成功；迟到Query防串新记录、远端Seal提示未达的精确交错仍单列未覆盖。

受信主控死亡门沿原执行/物理责任链补证：`acceptance-device-controller-fault`读取本轮xHCI Ready的真实PID及已交付环地址，在原trap硬断点装好后以真实插键盘唤醒可能等待IRQ的进程；只把该PID的真实ecall现场改为非法指令异常。原ProcessTask收到DONE、Managed在原Binding尚未Released且受信执行者已死时进入故障，日志区分该事实与Driver后续死亡。DMA观察体仍只读内核原RETAINED非空集合，逐项核usage/write、无acquisition/release、Work Idle，跨真实ColdReboot拒绝、init DONE与四hart停驻比较同id/backing/range；若插入导致新增slot页则按实际集合核，不锁定四页。该门不产生consumer_stopped/Retire，不以CPU死亡或停驻证明硬件静止。原core模式严格两mapping/各两页断言保持，外部GDB停顿不作期限/公平性证据。当前仅工具接线，运行结果另记。

`controller-fault-first.exit=0`，[完整入口及客体输出](../artifacts/run/20261025-183901-9694-acceptance-device-controller-fault.log)、[原始三阶段观察](../artifacts/device-fault/20261025T103939.404171Z-10097/process-fault.log)、[ELF/BootPackage及环地址指纹](../artifacts/device-fault/20261025T103939.404171Z-10097/evidence.json)。主线程已在原同步wait，245本轮pid13真实DCBAA/CRCR/ERST/events地址交付，246 Ready；断点先安装后248真实插键盘，250仅pid13发生cause2用户fault。251原Managed记录其DONE、原device2/epoch1未Released且reported=false，252 Driver失败、253真实Driver DONE、254唯一Terminal；Job/两source/五平台根封口后267请求真实Reboot/SystemFailure，调试器返回NOT_SUPPORTED，275 init终止、276/277 boot DONE/停驻。原观察体记录mapping4..7同backing/range0..1、usage/write许可保留、无acquisition/release且Work Idle，跨reset拒绝/init DONE/四hart停驻一致；无QMP RESET。此门证明受信主控CPU死亡不会制造静止/退款，不能表述设备已停止DMA，也不外推响应时限。

#### 已提交操作的回复缺失与费用取证

共用原QEMU/GDB设施，正常收束项用原debug core装配，故障项沿原唯一终局；不造provider/worker或产品验收分支。外部观察在真实服务的Send入口核RPC Response/version/txid、领域成功opcode/正文及实际PID；只改该份运输SendHeader.deadline为At(0)，domain正文deadline/ref/cap内容保持。实际Once独占claim及其context另从原发送栈/Delivery创建参数核实，业务提交由原owner记录/成功正文对应的实际对象事实证明。必须观察该Send真正返回DeadlineExpired；若先MailboxFull/Closed/Quota或未命中则该门失败，不能只凭写了At0判通过。一次命中即撤注入，原消费者继续恢复或停止。该方法模拟**业务已提交、回复在入队前被运输放弃**，不声称回复已Sent/排队后丢失；原请求必须是Sent错误，实际cause可为ServiceClosed/Timeout，Unsent不算Unknown。

| 操作及真实提交点 | 本轮丢失的实际回复 / Unknown观察者 | 原轮身份与恢复/停止owner | 预期费用/边界 |
|---|---|---|---|
| Claim：srv_device的Work.commit/Registry Seat提交原BindingRef并发布唯一Work | 首个成功Claim快照及一份BindingControl别名回复；srv_driver Managed得到CallPhase::Sent错误，尚无Control | 保留原ClaimSource/DeviceId/epoch；Managed不得新Claim掩盖未知占用，Driver故障交init原SourceAdmin/Job封口，原Work停供应或保留进入终局 | 回复别名与原Outbox按实际关闭；Seat/Work/供应费用不因丢Control提前退。故障终局不承诺全部退款 |
| Provision.Prepare：真实平台/总线provider已提交Prepared冻结形状，Prepare硬件效果归该provider | Prepared/count零cap回复；中央Work得到Sent错误 | 同Provision sender/context/原epoch发Stop，清迟到Prepare，再沿未交付消费者证明ConfirmStopped；provider仍核全部实际资源DONE才Closed | Frozen及子控制真实清理后退；零cap回复无成功运输尾部，原Request/来源仍要注销。最终原Pool退款需正常路线证明 |
| Provision.Get：provider从本轮Prepared Frozen复制一个真实供应别名 | 具名索引Descriptor+一cap回复；中央Work得到Sent错误，可能仍未收齐整表 | 同provider/epoch/index停原轮，原Collecting已收前缀与迟到alias由Work/RPC/Outbox收束；中央不得伪造完整表/Ready | 只关闭运输alias不替代物理责任。此处不证明消费者出口resources_exposed；需与下一行分开取证 |
| BindingUse.GetResource：Binding::resource核可用性并先置resources_exposed，才把真实资源alias交Outbox | 真实PCI消费者的首项ECAM Descriptor+一cap回复；PCI Engine观察Sent错误 | 位保持true，回复丢失/Drop不复位；原Use/ref不得以未收到能力内生确认。物理worker若按当前路径失败，由Driver/init保留并终局，不能把CPU DONE当ReportStopped | alias/Delivery瞬态按真实退休，原MMIO/供应壳与未知物理责任不提前释放；该门不能由Provision.Get或只读Describe替代 |
| Activate：真实worker按原Use Query的Ready快照提交ActivationSlot，输入固定原ref/Use及同Registrar身份 | 首份成功receipt/observation入口（Ready时还含function）；Managed得到Sent错误 | 同进程、同BindingUse/Registrar停原Activation，原Binding Stop及provider安全收束；不新签Registrar、不换Slot/新epoch补偿。迟到Activate仍受原Use门 | 未交付receipt alias按原Outbox退休；活跃额度只在真实Released退，外部旧尾部照付费，Managed脱离后进程owner才收集 |
| Master：PCI provider在本轮Prepared且真实function context匹配后写BME | 空正文零cap成功回复；xHCI Engine得到Sent错误，启BME结果Unknown | **基线代码**原先直接失败保留；本轮已在原pending的Call::Master(Deadline)接线，仅Sent的ServiceClosed/Timeout在清完原RPC owner后，以原function context/ref、同Controller/环/域和同绝对deadline重发。Frame/取消/shutdown不自动重发，已观察Stopping则沿原halt/Retire。**本轮同会话恢复已取证**，不刷新总期限；停止后与旧context必须拒绝 | BME可能已启用，未Run也不推出无效果；已暴露pin/write/charge继续由原owner持有。成功恢复不重新分配环/域，失败无静止证据保留 |

旧pci-function alias在新Prepared/Stop后的真实context拒绝另列门；不能由Master的正常新入口、payload epoch比较或本表替代。各项测试/日志路径与实际phase/cause、原ref/context及费用观察逐项补入，尚未执行不计通过。回复Send的DeadlineExpired只证明该次入队前失败，不证明所有中途move rollback；后者复用既有队满/uaccess/Frozen反例，最终核实际剩余门。

开发边界：`unknown-target-first.exit=0`包括本轮Master原pending期限/分类、原消费者Sent/txid诊断和暴露事实诊断，不能由早期九面Clippy覆盖。`unknown-claim-first`在Just逻辑表达式要求lists语法时失败，未构建/运行；改为已有嵌套if。`unknown-claim-second.exit=1`（入口`artifacts/run/20261025-192137-23931-acceptance-device-unknown-Claim.log`、原探针`artifacts/device-unknown/20261025T112150.272846Z-24357/reply-loss.log`）在读取用户栈时失败，尚未选中/写At0。trap已切内核页表，调试器改从真实Thread.satp解析目标用户地址，核U/W及叶几何，仅写原运输header的16字节deadline；不改guest页表/CSR，不进入产品uaccess。共享trap DWARF读取段抽到qemu_gdb，与已通过controller探针逐行去空行比较相同。

**Claim回复缺失已取证**：`unknown-claim-third.exit=0`，[入口与完整客体](../artifacts/run/20261025-192519-24871-acceptance-device-unknown-Claim.log)、[原探针](../artifacts/device-unknown/20261025T112529.571916Z-25232/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T112529.571916Z-25232/evidence.json)。207原Work已提交incarnation2178/device1/epoch1，选中pid3成功Claim/txid6/一cap回复，真实Once context2452、发行Pool2367；同线程原Send在respond_error实际返回DeadlineExpired，RPC/domain/body hash及move输入未变，未入队。209 Managed得到原txid6的Sent/ServiceClosed，尚无Control即210失败，212真实Driver DONE→213单Terminal→原Job/两Source/五根Seal。222-223原Work仍持同ref、未暴露且无报告，经原准备停止接管；随后唯一Reboot请求及QMP guest RESET。末尾Pool仍有委托/责任，不宣称全部退款或供应安全Closed；进程故障没有变成新Claim/新epoch补偿。

**真实消费者GetResource回复缺失已取证**：`unknown-get-resource-first.exit=0`，[入口与完整客体](../artifacts/run/20261025-193240-26237-acceptance-device-unknown-GetResource.log)、[原探针](../artifacts/device-unknown/20261025T113250.725357Z-26614/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T113250.725357Z-26614/evidence.json)。218原Binding ref=device1/epoch1在ECAM alias离开前resources_exposed=true；选中pid3的GetResource成功一cap回复/txid3，真实Once context2846、发行PCI Pool2712，实际DeadlineExpired且正文/move输入未变。220 PCI真实pid11得到该txid3的Sent/ServiceClosed后失败，224原trusted owner DONE仍未Released/reported=false，227停止时暴露位仍true，225/230 Driver失效/唯一终局，原Source/平台根封口后QMP真实RESET。没有内生确认或安全Released，也没有因未收到alias清回false。该门不等于Provision.Get；未入队回复无Delivery接收尾部，物理/绑定/Pool责任按故障保留，不冒充全退款。
**Prepare回复缺失已取证**：`unknown-prepare-first.exit=0`，[完整入口](../artifacts/run/20261025-193655-27442-acceptance-device-unknown-Prepare.log)、[客体](../artifacts/qemu-acceptance-20261025-193658-27815.log)、[原探针](../artifacts/device-unknown/20261025T113706.456255Z-27826/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T113706.456255Z-27826/evidence.json)。原Prepared/count4零cap回复pid3/txid3/Once2482/发行Pool2159实际DeadlineExpired；154中央Work原ref收到Sent/ServiceClosed，155未暴露停止，159 Released、160 Detach、161原Work Confirmed/StopACK后退休；352 main-only、378 shutdown及原委托两维零。

**Provision.Get回复缺失已取证**：`unknown-provision-get-first.exit=0`，[完整入口](../artifacts/run/20261025-194104-33056-acceptance-device-unknown-ProvisionGet.log)、[客体](../artifacts/qemu-acceptance-20261025-194108-33433.log)、[原探针](../artifacts/device-unknown/20261025T114115.884978Z-33444/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T114115.884978Z-33444/evidence.json)。ECAM/index0一cap回复pid3/txid4/Once2531/发行Pool2170实际DeadlineExpired；154中央Work Sent/ServiceClosed，未收齐Frozen也未向消费者暴露，原Stop/Confirm不依赖表齐后dma_report，156 Released、158原Work退休；349 main-only、375 shutdown及原委托两维零。

**Activate回复缺失已取证**：`unknown-activate-first.exit=0`，[完整入口](../artifacts/run/20261025-194622-39040-acceptance-device-unknown-Activate.log)、[客体](../artifacts/qemu-acceptance-20261025-194626-39436.log)、[原探针](../artifacts/device-unknown/20261025T114634.032960Z-39447/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T114634.032960Z-39447/evidence.json)。真实PCI pid11提交device1/epoch1、seq1 Starting后，一capreceipt/observation回复txid28/Once2785/发行Pool2367实际DeadlineExpired。163 Managed同txid Sent/ServiceClosed；165未暴露停止，174 Released、176 Detach/Unavailable、177原Work退休、178原pid收集，最终362 main-only、388 shutdown及委托两维零。该门覆盖Starting收据回复缺失，不泛化为所有后续已暴露阶段。

**Master同会话回复缺失恢复已取证**：`unknown-master-first.exit=0`，[完整入口](../artifacts/run/20261025-195821-45230-acceptance-device-unknown-Master.log)、[客体](../artifacts/qemu-acceptance-20261025-195825-45625.log)、[原探针](../artifacts/device-unknown/20261025T115833.152283Z-45640/reply-loss.log)、[指纹/字段](../artifacts/device-unknown/20261025T115833.152283Z-45640/evidence.json)。205 xHCI以原device2/epoch1/context3305/deadline40076183000提交，206 PCI真实写BME；pid11/txid6零cap成功回复的真实Once4168/发行Pool3648同Send返回DeadlineExpired。208消费者Sent/ServiceClosed，209相同ref/context/绝对deadline再提交，210原provider幂等写BME成功，211才一次发布原环地址/Run；没有新域/环或进程替代。371实际HCH/事件清理/DMA DONE/解除映射，374子Released、387父Released、403 main-only、429 shutdown及委托两维零。

以上六门均核原RPC/domain正文及move输入未变、真实Once context/payer和同Send实际DeadlineExpired；不以其他失败码或写入At0代替命中。正常收束四门的最终委托零是组合退款证据，不冒充每份中间资源的精确计量；Claim/GetResource按已定故障终局，不计局部恢复或全退款。

#### 旧pci-function真实能力跨轮验收

采用独立显式装配，只自动选择真实PCI；init在既有资源验收位置持限定Subtree ClaimSource/SourceAdmin，仅OBSERVE/CLAIM/REPORT_STOPPED，无CHILD_REGISTER。每轮取真实四项Frozen并复用libusb::Controller完成Run、HCH/残事件/全部暴露DMA DONE及解除映射，之后才ReportStopped/等待Released；不复制硬件实现、增加provider或改变产品控制协议。两轮必须同DeviceId、不同epoch和真实function context。

保留旧alias跨Released到新Prepared，旧context必须Permission且不得改变新轮BME；新context真实启BME成功。原context在provider实际接受Stop后必须Closed，中央Stopping回复只证明中央提交，允许传播期间同context仍成功；沿真实认证Closed有界等待，不新增跨服务同步合同。只读GDB观测实际ECAM command在新Prepared旧调用前后BME=0、新alias成功后=1，并核实际sender context/ref及请求回复；数字仅供观察，不授予权限。未知或错误不制造静止事实，沿原唯一故障终局保留。运行尚未执行。

开发检查`function-target-second/third.exit=0`；新增验收仅采用原Inputs/Controller与原RPC/Watch，没有第二硬件实现。`function-session-first.exit=1`，[入口](../artifacts/run/20261025-205202-55733-acceptance-device-function-session.log)、[失败客体](../artifacts/failed-acceptance-20261025-205207-56126.log)、[原只读探针](../artifacts/device-function/20261025T125216.099619Z-56137/function.log)：客体第一轮已真实BME提交，探针错把RPC请求也限定零cap而漏掉请求的一份reply Once，无法关联回复。仅修探针按Request一槽/Response零槽，未改产品门。该轮整体失败，不是timeout；不能把已到BME当跨轮或停止通过。

`function-session-second.exit=0`，[完整入口](../artifacts/run/20261025-205430-57298-acceptance-device-function-session.log)、[客体](../artifacts/qemu-acceptance-20261025-205432-57658.log)、[只读原始观察](../artifacts/device-function/20261025T125440.419776Z-57669/function.log)、[ELF/BootPackage及字段指纹](../artifacts/device-function/20261025T125440.419776Z-57669/evidence.json)。真实PCI pid11，同Device2 epoch1/context2909与epoch2/context3410；171/187两次Prepared，175/192实际Controller Run和四份ring DMA。新Prepared后txid73旧context返回Permission，command1026→1026即BME保持0；txid74新context成功1026→6即BME1。原txid51/79认证Closed之前各有Stop传播期间的真实成功，不把中央ACK当远端Stop提交。180/197观察该原context Closed后，181/199由原Controller实际Closed、DMA DONE/members0、Unmap；随后原ReportStopped成功才183/201 Released，202/204两份原Work以reported/confirmed/StopACK=true退休。203原Source Closed及旧function/Control显式关闭；后续PCI/管理者正常收集、main-only、acceptance collected、root辅助能力清理和432 shutdown，最终root metadata delegated0。此为完整组合退款证据，不替代逐份资源中间metadata计量。无QEMU/GDB残留。旧function跨轮/Stop门关闭，不重复本路线。

#### MMIO发布许可两侧的真实Revoke取证

复用mmio_checks::process_retirement及test_target::mmio_target的32页真实Map与父控制Revoke，显式验收追加两种编排，保留原最后Close/退出消费者。子进程以现有Notification报告Map前就绪并等待父gate；父放行后提交真实Revoke。外部GDB只安排hart顺序，不写许可、PTE、返回值或对象状态。

| 原提交窗口 | 实际暂停位置与锁前提 | 预期原机制结果 |
|---|---|---|
| 最终许可之前 | MmioPublication::grant入口，尚未取得资源根门；原Preparing已登记，父Revoke先正常提交并返回 | 原grant.validate按根sequence失效明确ObjectBusy；原准备与引用回滚，scope最终DONE/members0，同use DONE后新Map必须ObjectClosed |
| 最终许可之后、PTE发布之前 | vm::operation::commit在grant_publication成功后、782/783取得VM锁之前；资源根门已释放 | 已接受Map继续原提交/发布，之后原ForcedRetirement删除完整IO片段；不把Revoke先后改写成撤销已接受事务 |

两个窗口先核真实执行Thread/PID、原Mapping/footprint与同父控制身份，再正常推进父Revoke并放行原Map。scope DONE且成员零时子进程仍Running，调试器沿该Thread真实用户SATP核原footprint全部PTE无效，然后原Kill/DONE收集。子进程使用已有MemoryPool/libprocess付独立预算，记录Map前、结果后、进程Dead但Control仍在及最后Control关闭后的两维；最终该Pool allocated/delegated零才能关闭。普通root余额、未读日志或CPU死亡不代替精确退休事实。暂停点不持资源根/VM锁；vCont与全局时间边界见固定资料，不作期限/公平性证明。尚未运行，Retry迟到Query/Seal提示未达、其他MMIO/IRQ/DMA/root准入与逐份退款门继续单列开放。

目标Clippy `mmio-window-target-second.exit=0`。`mmio-window-first.exit=1`在外部探针初始化失败：[入口](../artifacts/run/20261025-211601-65654-acceptance-mmio-publication.log)、[原探针](../artifacts/mmio-publication/20261025T131614.010349Z-66161/publication.log)。连接时所选hart仍处固件bare页表，直接对内核高半区执行反汇编不可读，尚未命中Map/Revoke窗口。机器码/DWARF核验移到同一当轮ELF的离线GDB，生成code-windows.log与绑定指纹；运行期状态/PT仍用原物理读取。另统一GDB DWARF方法名与nm角括号的表示差异。未改客体状态或放宽结果，该轮整体失败，不计timeout或资源机制失败。

`mmio-window-second.exit=124`为客体断言触发的主动收割：[入口](../artifacts/run/20261025-211754-67512-acceptance-mmio-publication.log)、[失败客体](../artifacts/failed-acceptance-20261025-211758-67883.log)、[原时序/窗口](../artifacts/mmio-publication/20261025T131805.579364Z-67894/publication.log)。真实pid28/Mapping9、footprint0x50000..0x70000，父hart1/子hart2，先停原grant入口，再只恢复父hart取得真实Revoke成功返回，再放子执行；其后首个Map结果断言失败，未到退休完成。不是外层route deadline，不翻倍。复核resource_tree::check_sequence/validate表明Revoke先推进根sequence，已完成遍历的Admission明确Busy→ObjectBusy，区别于新准入已封节点的Closed；初版夹具要求ObjectClosed不准确。补实际错误日志，before只认原ObjectBusy，并在原use DONE后重试一次新Map严格核ObjectClosed；after仍要求成功。内核门不改，不泛收错误，原失败保留；尾部GDB target-running输出不单独认定失同步。

`mmio-window-third.exit=0`，[完整入口](../artifacts/run/20261025-212540-74689-acceptance-mmio-publication.log)、[客体](../artifacts/qemu-acceptance-20261025-212544-75076.log)、[原时序/对象/PT观察](../artifacts/mmio-publication/20261025T132552.145143Z-75087/publication.log)、[当轮机器码](../artifacts/mmio-publication/20261025T132552.145143Z-75087/code-windows.log)、[指纹与原窗口](../artifacts/mmio-publication/20261025T132552.145143Z-75087/evidence.json)。before为真实pid28/Mapping9、父hart2/子hart3，停在grant入口0xffffffc08033b788、root锁调用0xffffffc08033b7e6之前，实际父Revoke成功返回后409原Map ObjectBusy，410真实use DONE后新Map ObjectClosed。after为pid29/Mapping10、父hart3/子hart1，原grant调用0xffffffc0802ee8d8成功分支之后0xffffffc0802ee90e、VM锁0xffffffc0802ee92e之前；父Revoke返回后417原Map真正成功，原范围0x50000..0x70000。

两轮均在原scope Query真实DONE/members0时、父Kill前沿原Thread.satp核全部32页PTE无效；客体412/419分别确认仍Running，然后才原Kill/DONE。独立Pool19255/19265的Map前与Revoke后均为2136页/63562 metadata，CPU DONE后为0页/82 metadata，最后Control关闭后allocated/delegated两维均零。存活期未要求页表/handle容量消失，CPU观察壳最后关闭才退的82另有实测；不以整个root大余额替代。原最后Close/退出消费者及后续DMA/core保留，441 shutdown、root metadata delegated0通过；无QEMU/GDB残留。此门关闭精确许可两侧Revoke及这两个独立子Pool的收束，不关闭非空强制队列/Frozen、满业务额度、root准入资金/OOM、IRQ/DMA其他交错和Retry精确竞态。

#### IRQ在途撤销与跨代确认取证

沿原function_checks的真实Controller消费者增加显式IRQ验收变体；同一PCI provider/原Binding供应/原Controller停机，不给worker新权限，不另造IRQ或事件循环。QMP真实插入产生端口事件，init对实际IrqUse等待READABLE并Take，不用软件伪造claim/token。

| 原窗口 | 实际owner与动作 | 必须观察的结果 |
|---|---|---|
| Take后原provider Revoke | 第一轮保留真实已taken token，Controller仍完成HCH/事件清理/DMA DONE/unmap，再ReportStopped；PCI原IRQ control负责Revoke | 原Line.close的outstanding=true，在同PLIC锁真实complete一次再disable；Controller halt路径Take为空，不能替夹具提前Ack原token；原use/Binding随后DONE/Released |
| 新generation已有真实在途且已Take，提交旧token | 旧IrqUse/token跨Released保留到第二轮，实际新IRQ事件Take后才对旧use及新use调用旧token | 旧use明确ObjectClosed；新use明确IllegalArgument，Native原状态in_service/taken仍成立且不得硬件complete；原真实新token随后Ack成功且对应complete恰一次 |
| 全hart idle后的实际唤醒 | 与上述机械安全窗口分开记录；仅在原四hart idle、无当前用户执行/待推进事实的只读证据成立后插入 | GDB全停不等于idle，Waiting日志不等于idle；若无法取得真实全idle快照保留未证，不制造ready位/补偿轮询；调试暂停不作时限或公平性证明 |

第一轮ReportStopped仅在原Controller保证所有后续MMIO/新DMA责任已结束后发生；保留的token/旧alias只用于原provider完成IRQ撤销之后的拒绝反例，不再承担合法硬件清理。IRQ根/控制/完成责任仍归原provider，未因token留存换成CPU死亡证明。正常尾部显式关闭旧角色，退款边界沿实际Pool/来源。Retry迟到Query/Seal提示未达、其他MMIO/IRQ/DMA/root准入/费用门继续开放，尚未运行不计通过。

`irq-retirement-target-first.exit=0`；`irq-retirement-first.exit=0`，[完整入口](../artifacts/run/20261025-214348-83502-acceptance-irq-retirement.log)、[客体](../artifacts/qemu-acceptance-20261025-214353-83892.log)、[Native原始观察](../artifacts/device-function/20261025T134401.475503Z-83903/function.log)、[指纹与在途字段](../artifacts/device-function/20261025T134401.475503Z-83903/evidence.json)。177-179第一轮真实QMP插入后source33/target0/generation5 sequence1 Take并保留，原Line.close的owned/outstanding=true；Native complete写返回后才disable。第二轮200-204实际generation6/sequence1已in_service/taken，再以真实旧5/1调用：旧use ObjectClosed且零complete，新use IllegalArgument且原6/1状态保持/零complete，随后原6/1 Ack成功且恰一次complete。两代sequence相同，排除了仅靠无事件/未Take或序号不同产生拒绝的误判。两轮HCH/事件清理/DMA DONE/unmap/ReportStopped/Released、主机删除分别取证，217来源Closed/旧别名显式关闭，445 shutdown及root delegated0。该门关闭在途Revoke与跨代Ack，不宣称全hart idle；无QEMU/GDB残留。

全idle增量只改变原外部操作者的第一轮插入门，不改产品调度或驱动：在原sched::idle实际检查/装填完成后直接跳WFI的非阻塞j处暂停，必须同时读到四个已准入hart的current_thread=0、PC在实际检查后jmp/WFI/紧随WFI、各域实际ready队列为空及idle位、maintenance ready=0、各hart timer_pending=false/remote inbox为空、SIP无SSIP/STIP/SEIP。只读来源实际Controller/contexts，插入前源pending=0/enable/priority/threshold成立；保持CPU暂停时QMP真实device_add，核pending及目标SEIP真实置位并记录timer共现，再恢复CPU沿原同源claim/Wait/Take推进；若恢复时STIP也到期，如实记录，不宣称纯SEIP独立唤醒。GDB全停本身不是上述idle事实；不要求timer全卸载，不以暂停时长证明响应界。未满足快照条件就不计覆盖，不增加产品ready位或补偿轮询。后续各轮的实际覆盖见下文，当前尚未取得合格共同快照。

all-idle首轮`irq-idle-first.exit=1`，[入口](../artifacts/run/20261025-215956-91471-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-215959-91850.log)、[探针](../artifacts/device-function/20261025T140007.279277Z-91861/function.log)：只到首BME，未到Run/插入门，实际外层QMP route deadline560耗尽。按用户规则Just唯一默认翻倍1120。直接在WFI设断点存在GDB单步越过阻塞WFI、其他CPU暂停的工具隐患，不能从这轮推出产品失活；改按当轮实际机器码选装填/检查后直接跳WFI的非阻塞j，不改guest PC/CSR或step策略。

第二轮`irq-idle-second.exit=1`，[入口](../artifacts/run/20261025-221611-10563-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-221615-10952.log)、[探针](../artifacts/device-function/20261025T141622.070321Z-10964/function.log)：176真实Run、177等待门，178夹具本轮60s等待未得到插入通知而失败、进入唯一终局。该轮不是1120外层撞限；最初仅据探针尾部/入口误分类已纠正，未执行2240版本，唯一外层默认保持1120。全idle尚未覆盖。逐hart切换/读取移出Breakpoint.stop回调：回调只返回正常stop，主GDB循环在同一次全停后读取快照再继续原执行；各次不累加hart事实，有限诊断明确PC/current、ready/maintenance/remote、timer/SIP或读取失败。没有用加内部期限代替定位，不改产品调度/时钟，原IRQ在途/跨代通过证据保持。

第三轮`irq-idle-third.exit=124`，[失败客体](../artifacts/failed-acceptance-20261025-223911-43538.log)、[原探针](../artifacts/device-function/20261025T143919.318293Z-43549/function.log)：读链先明确拒绝非idle的hart0/PID9，下一帧在其他vCPU当前页表下懒读内核typed global失败（0xffffffc08040ac80），由探针失败触发收割，不是1120外层耗尽。逐CPU只materialize寄存器和已确认idle帧的domain指针，恢复原kernel-idle hart并强制读回寄存器选择正确RSP context后才读内核globals/HartLocal/队列；不改guest satp。第四轮`irq-idle-fourth.exit=124`，[失败客体](../artifacts/failed-acceptance-20261025-225358-58082.log)、[探针](../artifacts/device-function/20261025T145406.477974Z-58098/function.log)：生成脚本缩进错误，未有效执行观察。生成脚本完整Python段在启动GDB前增加ast解析，调用检查失败即停止后续启动，保留原失败，不计任何窗口覆盖。

第五轮`irq-idle-fifth.exit=1`，[入口](../artifacts/run/20261025-225807-58833-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-225810-59213.log)、[探针/周期证据](../artifacts/device-function/20261025T145818.583645Z-59233/function.log)：读取链已成立，候选真实拒绝包括PID9/3非idle、maintenance=1、SSIP pending和ready=1，但未取得同一共同快照，最终夹具60s等待结束后唯一终局；不是1120外层耗尽，也不能推出系统从不全idle。用户随后关心发热，主代理/监督者即时ps/pgrep均确认当时无just/QEMU/GDB/限速器残留；只证明当时无残留，不否定前轮高负载。用户已澄清可能无关，原授权继续。

第六轮`irq-idle-sixth.exit=1`，[入口](../artifacts/run/20261025-232005-82073-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-232009-82469.log)、[探针](../artifacts/device-function/20261025T152016.956051Z-82480/function.log)、[周期证据](../artifacts/device-function/20261025T152016.956051Z-82480/evidence.json)：仅future-timer成功分支实际记录29312次命中，其中current-precheck27864、maintenance566、pc873、ready6、interrupts3；这些是保存时的计数，不额外推断收割前未保存的候选。真实Run/等待后仍由夹具60s等待结束触发单终局，不是1120外层耗尽。16603行原日志主要是GDB线程切换，约95%候选未过current门；Send等全局断点仍持续截断真实RPC。该证据只说明此采样未取得合格快照，不能推出系统从不全idle。

工具收敛保持完整门：实际IRQ等待target出现后保存并暂禁无关function Send/Delivery/IRQ辅助断点，先前真实Master事实保留；真实QMP插入后恢复后续取证点。两条实际post-check j均可采样，同一次全停先按固定DWARF地址批量读原current/maintenance、当前domain共同idle位，过廉价拒绝门才在正常stop后逐CPU检查PC/CSR/queue/remote及真实TimerQueue.first。以该次全停首读time比较期限，未来timer允许，不用后续调试耗时反复评价旧状态。布局只初始化一次，每1024命中保存计数，continue输出仅去重复switch行，原事实与异常保留；不拼接旧快照。夹具ROUND由60s翻倍120s，依据是上述有效读取与持续推进，仍为整次验收轮的防挂死预算，硬件halt/reset和服务20/30/40政策不变。THROTTLE只限制QEMU，GDB开销独立；不改系统应用/安装或产品调度/时钟，外层唯一1120保持。Retry迟到Query/Seal、MMIO/DMA/资金/OOM等原剩余门继续开放。

第七轮`irq-idle-seventh.exit=1`，[入口](../artifacts/run/20261025-233427-92425-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-233432-92809.log)、[原探针](../artifacts/device-function/20261025T153439.729917Z-92820/function.log)、[周期计数](../artifacts/device-function/20261025T153439.729917Z-92820/evidence.json)：实际禁用了无关点，但两j仍保存231424命中、current219165/maintenance4007/共同idle位8251拒绝，未进完整CSR门；夹具120s等待结束而失败。回调false仍触发远端all-stop/step-over，廉价筛选不能消除这一根本测量扰动，不再重复密集j策略。

第八轮`irq-idle-eighth.exit=124`，[入口](../artifacts/run/20261025-235356-16395-acceptance-irq-idle.log)、[失败客体](../artifacts/failed-acceptance-20261025-235401-16800.log)、[原始采样](../artifacts/device-function/20261025T155408.799846Z-16811/function.log)、[448次快照与12帧进度](../artifacts/device-function/20261025T155408.799846Z-16811/evidence.json)。原等待target出现后关闭j/辅助断点，工具线程仅在GDB continue期间向该GDB PID发一次SIGINT，请求普通all-stop；停止后取消/join宿主计时者再读快照，期间不改guest。实际保存448次共同stop，447帧PC不合格，1791行日志；前12帧记录当前CPU PC及原PID3/9/11的保存sepc/a7/a0，保存trap不冒充当前PC。夹具120s通知缺失→init失败终局，外层脚本按失败锚点主动收割才为124，不是1120路线撞限。尚无all-idle证据；下一只做少量原PCI Query完成→同advance下一submit的诊断，用暂停前input.now_ns/原next_poll，不用断点后的持续time制造过期，也不删除安全观察。

第八轮事后清点发现独立QEMU/throttle组16819/16820仍在，已核当轮私有socket/argv/PGID并CONT+TERM收完，[原ps/清理记录](../artifacts/device-platform/irq-idle-eighth-cleanup.log)。此前各即时无残留结论不外推此轮。未定位该轮最早哪项清理中断；共享`qemu_gdb.stop_group`现统一按本次专有组CONT/TERM→实际PID/PGID有限核空→必要KILL→再核空，EPERM仅在组实际不存在时忽略，不能由leader退出代替组消散。设备操作者短清理段拒重复终止，ExitStack保证VM、GDB、QMP和私有文件全部执行收尾，清理异常独立留存而保留原失败。七项隔离真实子进程反例通过，[完整日志](../artifacts/device-platform/device-process-cleanup-third.log)：正常组、STOP子进程、wrapper先退后代仍在、TERM拒绝需KILL、前序callback异常后其余owner仍收完、实际活组EPERM报错、组确已消散才忽略EPERM；早期三项日志保留为历史。随后DMA竞争、共享、额度三条实际路线均确认无本次QEMU/GDB/操作者残留及清理错误，不冒充第八轮或all-idle通过。


#### DMA指定竞争与独立付款观察

沿原`dma_checks::process_retirement`与`test_target`的真实MemoryObject/DmaUse继续补窗口；父方仍持原control/MemoryObject，子进程用已有WORKER_POOL承担自身出生与Pin/读取请求，失败和未Start仍按原GrantOutcome关闭。新增显式验收配置只安排已有消费者，不新增provider、Runtime或控制协议。地址只由测试读取，绝不交硬件，因而测试中受信Retire的静止前提独立成立。

| 原窗口 | 外部时序与实际提交点 | 必须观察的事实 |
|---|---|---|
| Pin写许可之前Seal | 子Pin已登记原mapping/BackingAcquisition，在`reserve_dma_write`进入原对象锁前暂停；父原Seal真实返回后放行 | 原MemoryObject已Executable且write_permits=0，原Pin明确ObjectBusy；准备记录自动取消，原域DONE/members0，不交mapping或地址 |
| Pin写许可之后Seal | 子原Pin已把一份真实WritePermit写入原Owned，仍未完成cap交付；父Seal真实返回后放行 | MemoryObject保持Sealing/write_permits=1，原Pin成功且ReadyUnexposed；子退出只取消未暴露mapping，原许可退后Executable，原域DONE |
| 已准入地址reader与Retire | 原Read已expose/reader+1，在锁外枚举/输出前暂停；同一真实mapping的受信Retire提交并观察 | phase=Retiring/readers=1时不得DONE/退write或BackingUse；释放reader后原读取真实完成、reader=0才退休/DONE，不能以Retire syscall返回代替完成 |
| 非空Pin准备中退出 | 原BackingAcquisition已取得非空前缀而未完成，在不持资源/Backing/VM锁处暂停；父正常Kill提交后放行 | 目标实际Dead/Killed，原Pin取消由既有Work收完非空前缀/写许可，原域DONE/members0；未曾暴露，不留故障保留代替正常取消 |

断点必须用当轮DWARF和反汇编核实锁外位置，记录真实PID/Thread/MappingId/MemoryObject identity/Backing范围；vCont只安排原CPU运行，不写许可、页表、状态或结果。MemoryObjectCreate当场由父进程Pool付RAM/原对象，MemoryObject的root use仍在时Pin退休只减uses，不退父物理页。子Pool付Pin/Read Pending、mapping/来源/预付切界等元数据；SegmentMap换入子付款arena后该费用可合法活到父Backing清空。逐阶段按“Pin/reader完成→CPU DONE→父MemoryObject/相关View能力关闭→子Pool两维零”记录，不要求存活期容量尾部瞬退；共享Backing不能以大root余额或两个不同Backing冒充共享退款。目标第三轮Clippy exit0，离线当轮ELF窗口/生成脚本语法核验通过；实际结果见下，不替代多Pin共享退款、付款/OOM、IPI保留组合或Retry精确交错等其余门。

`dma-races-first.exit=1`，[入口](../artifacts/run/20261026-075821-30527-acceptance-dma-races.log)、[失败客体](../artifacts/failed-acceptance-20261026-075827-30936.log)、[原探针](../artifacts/dma-races/20261025T235835.420492Z-30947/races.log)：父原Seal已选，vCont命中首个Pin窗口后错误要求HartLocal.current非零而失败，尚未放行父操作，不计Pin/Seal窗口。源码事实是`sched.rs`在set_context之前持真实t/运行准入推进Pending；`request::advance_in_space`切入该owner的SATP，current合法为0。探针改从原Pending::advance(thread)与advance_in_space的Admitted owner交叉核同一Thread/PID/frame/SATP，Hart字段仅诊断；原syscall仍核已安装owner，不改内核补current。此事实也不降低all-idle完整PC门。

`dma-races-second.exit=0`，[完整入口](../artifacts/run/20261026-080754-37711-acceptance-dma-races.log)、[客体](../artifacts/qemu-acceptance-20261026-080757-38092.log)、[原窗口](../artifacts/dma-races/20261026T000805.398066Z-38103/races.log)、[当轮ELF/boot指纹与真实对象](../artifacts/dma-races/20261026T000805.398066Z-38103/evidence.json)、[DWARF及反汇编](../artifacts/dma-races/20261026T000805.398066Z-38103/code-windows.log)。四窗口分别成立：

- PID31/identity19427/mapping11/backing817，许可尚未取得，父Seal真实返回Executable/write0，原Pin返回ObjectBusy，随后域DONE。
- PID32/identity19441/mapping12/backing830，Owned已持write、cap未交，父Seal真实返回Sealing/write1，原Pin继续成功ReadyUnexposed；正常Kill取消后write0/Executable。
- PID33/identity19455/mapping13/backing843，原acquisition严格前缀`0<next1<hi2`且未完成；父正常Kill真实返回，再放行准备，原Work收前缀/许可，域DONE。
- PID34/identity19469/mapping14的两个真实Thread：reader已expose后暂停，原Retire真实返回；保持reader时实际Query/许可/DONE探测为Retiring/readers1/write1/DONEfalse，原软件交接通知提交点之后才释放reader。4096字节真实Segment交付完成后，readers0/write0/Retired/DONE。通知只是有限测试编排，不代替Exposure或Retire事实。

四Pool19424/19438/19452/19466在CPU DONE后均为pages0、metadata142，未据此宣称全部退款；关闭原Control和父MemoryObject后各自allocated/delegated/reserved两维全零，四scope均DONE/members0，453显式shutdown及root delegated0。内核高地址和SATP可由allocator复用，身份由原PID/MemoryObject identity/MappingId固定，不把相同地址当同一对象。第二轮profile core完整通过，QEMU/GDB/私有传输目录全部退，未出现清理sidecar错误。此组只关闭上述四窗口与各自子账，不关闭双Pin共享Backing逐份退款、资金/OOM或保留+IPI组合。

多Pin共享退款使用同一`test_target`的显式`dma-shared`消费者，不重跑上述四个精确窗口：父同一MemoryObject交给一个独立子Pool目标，子CPU只读View和两份不同DmaMapping共同持同一两页Backing。两份真实SG须逐项相同且覆盖全部两页、两份独立写许可；父Seal后按原Notification逐段允许子Retire/关闭第一份、第二份。第一份退后另一Pin仍MayBeExposed且地址不变、write1/Sealing，第二份退后write0/Executable；每段必须先有真实DONE再关闭原mapping角色。父在固定软件交接区分别观察子Pool元数据下降，而原父RAM页分配保持不变；父MemoryObject仍持root时不把Pin退休记成物理退款。最后CPU/Control和真实父对象都关闭、子Pool两维零后，核同一父Pool在该无其他RAM准入区恰退该MemoryObject页数。原父对象付RAM/基础存储，子Pin付新增mapping/请求/切界；存活Backing arena尾部保留到真实清空。新增的是有限消费者编排，没有新资源机制或测试运行体；首轮实际结果见下。

`dma-shared-first.exit=0`，[完整入口](../artifacts/run/20261026-084130-47340-acceptance-dma-shared.log)、[客体](../artifacts/qemu-acceptance-20261026-084136-47761.log)、[当轮指纹](../artifacts/qemu-acceptance-20261026-084136-47761.meta)。409-421为稳定软件交接区：同MemoryObject identity19386，两角色KOID19396/19397，两SG完整覆盖原两页且均为0x820fa000/0x820fb000；写许可2→1→0，第一份DONE/Close后B保持MayBeExposed/地址不变。子Pool19383的metadata从63868→63750→63632，分别关联各份DONE/角色关闭；它们只证明这两段费用归还，不独自充当全部/物理退款。两段父Pool1 pages7035不变，CPU DONE后子metadata142尾部仍在；关闭原Control/父MemoryObject后子账两维allocated/delegated/reserved全零，父RAM7035→7033恰该对象2页。scopeDONE/members0、438显式shutdown/root delegated0、无QEMU/GDB/操作者残留及无清理sidecar异常成立。该共享双Pin门关闭，不重跑；不外推Quota/OOM或保留+IPI组合。

额度拒绝消费者仍使用原目标及独立Pool：只增加该Pool的READ观察grant，真实Notification角色持有到调用者metadata额度拒绝，再归还一份角色后尝试原Pin，记录明确Quota、原MemoryObject许可/use成员收束以及释放压力后同一来源重新Pin成功。Vec只持实际cap owner，按原Pool可用槽数给出有限上界，不模拟credit或修改Pool/allocator。拒绝后实际容量尾部允许继续付款，最后CPU/Control/父MemoryObject关闭才核原子账零。该场景证明真实额度拒绝的取消/恢复与最终账务，不把未知失败前缀升级成精确中途/OOM证据。

`dma-quota-first.exit=0`，[完整入口](../artifacts/run/20261026-095314-61526-acceptance-dma-quota.log)、[客体](../artifacts/qemu-acceptance-20261026-095319-61945.log)、[当轮指纹](../artifacts/qemu-acceptance-20261026-095319-61945.meta)。410-416为原独立Pool19409的真实观察：持1020份Notification owner、归还一份后原Pin明确Quota；失败前后metadata available均116、allocated均130956。释放压力后同一Use新Pin实际ReadyUnexposed，再正常取消到members0/write0；CPU DONE后metadata142尾部，关闭父对象/Control后allocated/delegated/reserved两维全零，scopeDONE/members0。433显式shutdown/root delegated0、入口profile core passed及无本次进程残留成立。只关闭真实Quota拒绝/恢复与本次原账收束，不关闭精确已接受前缀的付款失败、allocator OOM或IPI组合；`dma-quota-target-first.exit=0`仅作目标Clippy开发证据。

root资金/OOM待验条目：DMA`Node::root`的Self::new失败走未发布同步seal/finish/remove；Self::new成功后retained entry失败则沿最后Node Drop seal/finish→原Root预付Work/稳定强根→逐项remove_finished。后者不能只因先建树后分配就判为非空Tree析构缺陷，当前未修改内核。最终窗口需核Err返还的Arc clone、本地node/root析构及RootWork原账回收，区别同步回滚与异步已接管；已有boot::load入口panic/alloc handler取证不覆盖该位置。MMIO/IRQ相邻Root owner同样按真实Drop/未发布错误值路径核，不新增guard/状态层。该精确失败取证留最终集中验收，尚未通过。

完整Clippy收口：第二轮已过前五面，user-target仅报设备验收专用observer_handle在默认feature下未用；将该访问器和test_device具体槽依赖与acceptance-device同门，未改运行语义。随后按Justfile相同flags只执行尚未过的user-target/stress/memory/FP，`once-clippy-user-target/user-stress/user-memory/user-fp.exit`均0。结合`once-drivers-clippy-second.log`前五面，九分面均通过，不重复无关已过面。

主验证链为两个真实 Binding 同 PID A/B→停止 A 并保留旧 function/Watch，B 继续输入→同 PID 接受 C 且 A 仍为终态→终止该 HID 进程→B/C 由存活 host 安全收束→仅一次进程收集；同时保留原单键盘重插/IRQ/退款路线为受影响回归。费用耗尽、旧轮、Unknown、晚到消息与死亡窗口仍属原闭包，不另造 fixture 运行体。

受影响回归：`irq-handover-boot-failure.exit=0`；`acceptance-platforms` 首轮 sifive_u 在原 128 MiB 机器因空设备场景预付 80 MiB worker 工作集而失败（`failed-acceptance-20261025-065644-82394.log`）。init 改由原真实 ECAM 根是否存在选择硬件五份 WORKER_POOL 或仅 manager 一份，不按 MODEL 分支、不改变声明/发现/平台业务。第二轮 sifive_u 已通过；nofd 随后暴露旧批次验收假设两份 ledger 尾部记录，删除空 drv 且 FP 正确降级后该点只余 pm。原部分准入/InvalidPolicy/健康推进/collector 恢复场景已移到两个实际 FS provider 业务关闭后的显式 PID/control 交接，删除重复单独 collect；`platform-handoff-target.exit=0`、`irq-handover-nofd-second.exit=0`。不重复已通过 boot-failure/sifive_u；当前正式设备基线和上述差异保留，后续多接口改动再按影响回归。
`usb-host-first.exit=0` 验证 USB/HID 固定描述符字段和坏长度/零推进拒绝；`usb-target-second.exit=0`、`worker-common-first.exit=0`、`xhci-target-third.exit=0` 是对应目标 Clippy 开发证据。`virt-xhci-first.exit=1` 与 `virt-xhci-second.exit=124` 均为失败，客体日志分别 `artifacts/failed-acceptance-20261025-051352-53226.log`、`artifacts/failed-acceptance-20261025-051603-54608.log`：实际发现 1b36:000d/class0c0330，登记 xHCI、冻结四资源，xHCI 取得资源/寄存器、coherent 页和 Run；正常停止实际到达 HCH/残事件/DMA DONE/解除映射、PCI function Closed、两层 consumer_stopped，第二轮也到达两层 Released/worker 收集。后续 srv_device 仍明确失败，正在按原任务/回调定位，不能把局部停止或控制器 Run 记为整体验收通过。没有真实 USB transfer、IRQ/DMA 往返或热插证据。

以下保留首段施工时的接续依据；平台供应/PCI/USB/HID 正常链已按上文接通，当前施工以下节未闭合责任及上方多绑定记录为准。原施工依赖是**平台授权交付与真实供应，接上驱动管理/PCI 消费者，再向 xHCI/HID 展开**；编辑顺序不构成额外阶段交付，不为先让局部测试通过而造临时 provider、空驱动或另一个 Claim 匹配者。

1. **先闭合用户态平台输入的重新绑定。** 亲读 `srv_init::platform::PlatformDescription::adopt/description`、`device::Manager::launch`、`srv_device::main`。init 当前只暴露 `ecam_handle/irq_handle/dma_handle` 给既有验收，尚未提供完整三窗口/root 遍历或委托接口；根 owner 必须继续留在 init。服务目前硬性要求空 payload，只有 slot 0 bootstrap、slot 1 Pool。新增用户态启动/配置格式由实际用户态知识 owner 定义，并同步发送、采用、Grant 失败/消费和监督账本；不要把具体服务配置塞进通用内核出生 ABI。
2. **平台描述不能直接当成子进程槽表。** `shared::platform::Description::parse(bytes,handle_count)` 当前校验的是 kernel→init 的 `BASE_HANDLE_COUNT + resource_count`，每项 `handle_index` 固定为该初始数组的位置。virt 原描述含五个资源项；服务实际 grant 数和槽位不同。保留只读描述事实与实际 capability 的逐项关联，明确区分初始槽校验和用户态委托槽校验；不能把原 handle_index 直接用于服务 grants，不能靠传虚构 handle_count 跳过绑定验证，也不能修改原只读对象。需要拆分通用格式解析与初始槽绑定校验时，同步 shared/kernel/init/服务实际消费者。
3. **接真实平台 Provision owner。** 平台适配器组合 DT/资源库，验证实际委托 kind/rights/几何与 DT 节点；它拥有 PCI host 的登记来源、准备/停止任务和每轮子控制/use。复用 `libdevice::provision::Provision` 的 Prepare/Get/Stop/ConfirmStopped 及 `resource::{Collecting,Frozen}`，通过真实认证 Provision sender 接 `binding_work::Work`。目前 srv_device 只有管理 mailbox 的 device/OAL 分派，不能只给 Register 塞一个永远无响应的 sender。将供应 ingress/请求/清理加入原 Family/World 和容量付款，不另开测试运行体。
4. **资源描述由使用领域拥有。** libdevice 只核名字、内核角色、固定 rights 和不透明 metadata；PCI host 的静态 identity、ECAM/bus-range/window/INTx 与 DMA 元数据由 PCI/平台知识使用侧统一定义，并由发送和实际 PCI 消费者共同使用。具体编码、服务启动槽位及适配器内部类型尚未实施，属既有合同内的实现选择；下一会话落定后写回本篇，不必重新问授权。受信 PCI provider 需要限定的子控制派生能力，普通 leaf worker 只取 BindingUse；不要把平台根直接给每个驱动。
5. **接统一驱动选择与运行监督。** 按[驱动发现/激活合同](#驱动发现按需激活与设备绑定)和[服务装配](todo-2026-10-07-service-assembly.md#首批装配图与就绪合同)，由 init 交付 boot 声明/映像配对来源、受托驱动 Job/Pool、ClaimSource/观察入口，SourceAdmin 留在外层。srv_driver 唯一选择并通过 libprocess 启动实际 drv_pci_generic；Claim/Activate 未知结果恢复或停止原轮，不能改为 init 或运行中驱动竞争自动 Claim。运行期管理者死亡的即时封口/停执行链也在此处接通。
6. **替换当前边界检查与占位。** 平台导入后删除 `device.rs` 中“枚举必须为空”和 `check_discovery` 的空页假设，改为核真实平台 DeviceId/身份；保留 Watch、权限、终态 Ack 与退款检查。`device_manager.stop(root)` 当前在 `main.rs` 的短寿命服务批次收集验收之前，因为旧 `verify_batch_handoff` 会取服务等待自然退出；不能把尚未 Stop 的长期管理者重新交给该验收。真实驱动装配时同步删除日志占位 drv_spi_sifive、REQUIRED_DRIVER、initfs 项和旧日志锚点，保留既有 core/stress/platform 的其他业务。

本段施工须一起处理准备失败、部分 root/Grant 交付、来源签发 Unknown、回复丢失、停止先到及进程死亡。原 root/control 记录和已经提交的责任先存在，再交使用能力；不能将未取得安全证据的 DMA 责任退款。平台 provider 的一次空表 Prepared 不等于实际 PCI host 供应完成，验收必须覆盖它承诺的真实资源和消费者。

#### 不可丢失的约束与剩余验证门

| 未闭合责任 | owner / 接通触发 | 完成判据 |
|---|---|---|
| 平台导入、Provision 与非空 Register/Claim/Watch/OAL | srv_device 平台适配器、init 装配、实际 PCI 消费者；下一施工单元 | 实际 DeviceId/资源表可取得；Prepare/Stop/迟到回复和退款由原 owner 收束；空根假设删除 |
| BindingUse Resources OAL、具名资源解码、功能/激活实际消费者 | libdevice/srv_device、libdriver/具体领域库；首个实际 worker 接入 | 与冻结表单一真值、Get 固定权限及停止门一致；纯 Observer 不得取得资源；功能通过真实 provider 运行 |
| 运行期管理故障、父子正常停止与 xHCI 静止 | init/srv_driver/平台与总线 provider；接通真实驱动时同步 | 管理者死亡不重建占用表；子清理仍能调用父清理入口；HCH 等真实证据后才能受信 Retire；否则保留并进入终局 |
| 资源机制精确交错/费用失败 | VM、MMIO/IRQ/DMA owner；最终设备组合验收前 | 原[验收门](#验收门与可观察判据)及 MMIO/IRQ/DMA 续接所列窗口取得证据；常规成功路线不替代 |
| 真实 IRQ/DMA、USB 热插、旧轮重插与故障 | libpci/libusb/libhid、三个驱动和正式功能消费者 | QMP 插入前 HID 未运行，插入后按需启动/Ready/输入，拔出及重插旧角色不串轮，真实停止或故障保留 |

- 对外身份是 DeviceId/BindingRef 与内核认证 context；不能用 PID、路径或 payload 标签授予权限。CHILD_REGISTER 不等于无范围 REGISTER；已发行子 Registrar 不随父新 epoch 复活。
- `Progress::Closed` / Prepare 的明确 Rejected 才是供应安全结果；Error::Closed、NotFound、超时、endpoint 关闭和 Process DONE 都不能转换成这个证明。DMA 暴露后的稳定保留与管理者/句柄寿命分离，不能为了退出测试通过把它撤掉。
- PLIC completion 顺序和 QEMU 11.1.2 的限制、PCI BME 与 xHCI halt/缓存完成条件以[固定平台取证](ref-2026-10-23-device-platform-contracts.md)为据。DTS、真实xHCI命令行和QMP路线已经同步接通，具体证据见上方；未识别平台继续零设备，不为 sifive_u 顺带实现 SPI。
- 最近三项发现已关闭，后续修改仍保留其反例：OAL 非根续页必须用空 path 从 cursor 恢复原 view并重核范围；封 ClaimSource 在同次提交覆盖功能隐藏和独立 Watch 序号；Watch 错误回复先核空正文/零槽，保留 Permission/Quota/Closed。不要恢复后台补序号或统一两槽检查。

#### 接手时可用的验证证据

| 证据 | 最近结果、位置与边界 |
|---|---|
| 设备领域 host | `artifacts/device-discovery/host-layout.log/.exit`，51 项通过；包含 sub-20 原反例、父轮约束、OAL 授权与序号合同。整数 payload 不证明真实硬件/非空 RPC 已跑通 |
| 静态检查 | `device-discovery/check.exit=0`、`clippy-final.log/.exit=0`；最终八分面 Clippy 包含 user host/target/stress/memory/FP |
| 真实 core | `device-discovery/virt-review.log/.exit=0`；审查修正后的真实空根枚举/OAL/Watch/权限拒绝/终态 Ack/正常退出。之后仅 OAL 摘要 scratch 容量按实际布局推导，host/Clippy 补验通过，短名字编码未变 |
| 压力与平台 | `device-discovery/stress.log`、`platforms.log`、`routes.exit=0`；16/16、sifive_u、nofd。发生在最后纯层审查修正之前，相关修正由 host 和上述 core 补验 |
| 上一资源/装配范围故障 | `artifacts/device-rpc/faults-final.log` 中 boot/IPI/VM fault/repair 通过；原组合因 DMA 验收断言中断，修正续跑 `retention-validation.exit=0`，不能把原组合 exit=1 改记为成功 |
| DMA 终局保留 | `artifacts/dma-retention/20261024T171526.378604Z-3082/` 的 evidence/gdb/qemu 日志：reset 拒绝、init DONE、四 hart 停驻三处均同两 mapping、4 页、2 写许可、Work=Idle；地址未交硬件，不证明 DMA 已静止 |
| 结构/文档/清理 | `device-discovery/final-check.log`；108 个本地链接/锚点、格式、脚本语法、diff 空白通过；无暂存与 QEMU/GDB。交接整理另见 `artifacts/device-handoff/final-check.log` |

`artifacts/device-*` 中 first/second/failed 日志保留开发过程，不能只挑最近文件名判断成功。具体失败原因及修复版本从相应续接段定位。继续开发先按受影响面选择检查，不无理由重跑已有基线；代码/ABI/平台改变后依 [BUILD-AND-TEST](BUILD-AND-TEST.md) 执行必要门：

```sh
(cd user && cargo test -p libdevice --target aarch64-apple-darwin)
just check
just clippy
QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-virt
```

本段交接时尚无 `acceptance-device`，后续已由本篇上方接通真实QMP操作者、原消费者和组合路线；当前从唯一收口表继续，不再沿历史接线清单重新实施。**不需要重新做全仓盘点或重跑 sub-20 首审。**

本次交接整理核对了上述源码接点、已有退出码和证据文件，清理当前导航与历史“下一步”的歧义，未改变 Rust 实现、公开协议或运行配置；不重复执行已通过的编译/host/QEMU。最终只检查本次文档的本地链接、标题锚点、围栏、diff 空白以及工作树/进程状态，结果保存于 `artifacts/device-handoff/final-check.log`。

### 本轮代码事实与未闭合责任

启动输入接点已实施：`os/dtb` 整体迁至 `shared/dtb`，内核及 init 共用 parser；`DeviceTreeReservation` 从 main 移交 boot/Bootstrap，逐目标页复制有效字节，复制完成或失败后归还可回收原页。MemoryObject 使用 root Pool 付款，init 在实际启动路径查询类型/权限、映射只读描述并核验 DTB 与零 padding。基础初始图增加平台描述槽，总数按基础项加资源数计算；通用 libprocess 采用逻辑原本支持实际数组长度，无需加入设备语义。

续接已实施 VM 来源/基础页/RAM 访问检查、MappingUse/MmioViewUse、稳定 Map 记录与每空间预付强制 Work/FIFO，以及层级树、MMIO control/use、分步请求/返回续体、逐子/成员清理 Work、共享 ABI 与 libkernel。QEMU virt 的三个实际 MMIO 根已由 board 准入、Bootstrap 铸造并交给 init，真实 ECAM Map/撤销已有运行证据；细节与边界见下方记录，不能由此宣称完整设备可用。

**首批设备闭包仍未完成，不构成设备能力交付。** virt真实平台根、srv_device平台供应、srv_driver、PCI/xHCI/USB/HID与正式Devices/FunctionAccess/OAL客户端已接通；单键盘热插、直接端口双键盘多Binding、正常及故障监督、投影与Once付款、候选/取消、回复缺失恢复和资源竞争/退款已有证据按[当前唯一收口表](#当前唯一收口表2026-10-26)复用。MMIO Forced/Frozen、IRQ退出及DMA资金门已关闭；仍缺DMA+IPI与全idle/PCI，时钟异常独立未定。当前转审查交接，不能据采样失败改产品，也不自动继续取证；早期接线说明保留历史边界。复合USB接口未覆盖，旧drv_spi_sifive已删。用户已授权本次commit与本地Review登记，由主代理执行；push未授权。

### 审查交接（2026-10-27）

本节保留首轮转审查时的交接事实。施工检查点为 `f3337ebd789a79c8cad23c419b690b702e8f96bb`，原[Review 登记](archived/todo-2026-10-27-device-implementation-review.md)固定了对比基线与切面；该审查与后续第二轮处置已在[收敛档案](archived/review-2026-10-27-device-implementation.md)结束。以下清单描述该固定检查点，不代表当前开放门或再次提交授权。

- **当前承诺与实际接通**：virt/PLIC、真实PCI→xHCI→USB boot keyboard运行期热插，正式Devices/FunctionAccess/OAL应用、同进程多Binding、正常停止与故障保留链均有真实消费者。只读现状入口为[device impl](../notes/impls/device.md)，再按链接读mm/call/ipc/startup；复合USB接口未覆盖，不能以直接端口多设备证据代替。
- **已有验证及版本**：下方已关闭表与“成形自查与集中验证记录”拥有原路径。`just check`、host三workspace、十面Clippy/stress及release/platforms/faults分段通过，原聚合124/续跑0保留；聚合源码937项基线不冒充最终树再跑。后续定向消费者/工具按各轮冻结记录，最新MMIO第十二轮完整core/两窗/最终账通过，运行时944项无差分。最后IPI诊断改动只有离线检查，未运行第六轮。
- **开放项**：DMA+IPI组合缺同call失败返回/消费与后续原广播的完整关联；全hart idle/PCI观察未取得合格共同快照，原轮询推进只有待核假设；独立clock异常只有一次实际失败与一次不同环境未复现。具体owner/最小判据/原证据均在下方唯一表，不能因工具困难删除门或升级成已证产品bug。
- **发现分类**：真实产品接线问题（逐hart PLIC接管、未暴露取消、真实父轮释放/后代服务寿命、逐轮function与BME、USB重Prepare存在性/无描述符停止、Once付款等）已按相应记录实现并取证；test_target Set单次wait即析构是已修fixture缺陷；近期DWARF/多ELF/缓存/退休地址及异常留存属于工具问题，最后IPI纯诊断修正未跑。现无另行确认而尚未修的产品机制缺陷清单，不表示独立Review不会发现缺陷。失败原样保留，不把工具或fixture错误记成产品错误。
- **审查重点**：先沿Registry/Binding Work的授权与父子释放、ProcessTask/Managed/ActivationSlot的不同寿命、可信硬件停止与exposed DMA保留、Once付款及各层费用尾部、init出生前观察/正常撤防/唯一终局、VM Forced/IRQ有界推进检查唯一owner和重复状态。已有结构检查是输入而非独立Review替代；详细切面、已知证据与报告接力由[固定实现 Review](archived/todo-2026-10-27-device-implementation-review.md)登记，不把待证项或审查登记称为已交付。
- **工作树与归属**：施工起点HEAD为549a9e78400f9e59f3bdf516ef4d54f101846178；实施者提交前tracked/untracked状态、文件哈希、删除项及291路径清单见[交接快照](../artifacts/device-closing/review-handoff/snapshot.json)。主代理核对清单后，仅额外移除三个新增Cargo.toml末尾空行；[暂存核验](../artifacts/device-closing/precommit-verification.json)记录此无语义差异，最终实现身份以施工提交为准。原DTB迁移、工具链allocator_ext/lint适配及验收修正属于已接受基线；[libfs邻接核验](../artifacts/device-closing/handoff-adjacent-scope.json)五文件纯fmt、lib.rs仅feature替换+fmt，未发现其他任务遗留。根目录临时25行GDB索引已[原字节移入证据目录](../artifacts/device-closing/handoff-evidence-relocation.json)，未入源码提交；artifacts本机证据不冒充仓库发布内容。

上述施工检查点提交与本地 Review 登记已经落实，未 push，保留为历史追溯。当前导航由 COMPASS 指向第二轮交付及原剩余门；不能从本节恢复旧的线程协调、撤防或全 hart idle 待办。

### 当前唯一收口表（2026-10-26）

本表拥有当前剩余门与证据分类；前文各轮施工/失败记录保留事实与路径，不再从历史“尚未接入”重复产生施工任务。原验收合同仍由正文拥有。待证项不是已证缺陷，不能用常规成功或另一个操作的证据替代。

**独立异常记录：实际平台时钟失败，根因未定。** IRQ第三轮platform clock failed及原143主动收割保留，旧首因值不可恢复。一次THROTTLE100/冷启动/仅两错误点调查正常而未命中，仅not-reproduced；多项环境差异不能单项归因或宣布修复。该异常继续单列，不无限阻塞其他独立工作；后续只按明确单项安排，GDB使用100，若再现终局即收割留证，不机械增时或多轮追。下方剩余门不由本例关闭/重算。

**已实现，待集中证据。**

| 门 / 独立合同 | 原 owner 与最小验证边界 | 已有证据、剩余判据 |
|---|---|---|
| DMA保留与IPI失败组合 | 内核稳定Mapping/Backing owner、原停止/IPI协议 | third缺失败返回、第四轮资格未知各自保留。第五轮exit1，[原GDB](../artifacts/dma-ipi-retention/20261027T020938.232656Z-2391/gdb.log)到实际IPI ecall，但首候选诊断RuntimeError导致资格/注入/消费/InitialDONE/四park均未完成，不能反推第四轮。最后纯诊断修正已通过[离线生成/反例](../artifacts/device-closing/dma-ipi-fifth-offline/validation.json)，先保存标量、分别记录帧查询异常、原错先锁定且不重复采栈，必需门未改；修正版未运行。后续明确启动此门时，沿原实际 InitialWatch DONE、同 call 失败返回/消费、后续原广播及同组 mapping 跨四 hart 停驻完成取证；不放宽 selector/WFI，不以第二轮普通通过替代。 |

**审查代码责任。** 第二轮目标已写入本篇与服务装配计划，一致性重构、所选组合验证及交付前结构检查已完成；确认缺陷、设计处置和验证证据见[收敛交付档案](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)，本表不复制 findings。全 hart idle 门已在替代结构下通过并移入下表；DMA＋IPI 与独立时钟异常继续开放，首批整体闭包尚未交付。

**已通过，可复用。**

| 已关闭门 | 原证据入口与限制 |
|---|---|
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
| root构造与Building整批安装指定分配失败 | root-funding-first的两原观察/指纹/账见下节；局部构造/预约付款回退及无cap交付、Gate Failed/四park、安装后五份Node Work保持Queued成立。不是五根全额退款；retained-entry特定Root Work证据见下一行 |
| DMA未发布root retained-entry失败局部回滚（第二轮替代） | `THROTTLE=100 just virt-platform-funding dma-retained`退出0，[入口](../artifacts/run/20261027-135625-37995-virt-platform-funding.log)、[原账证据](../artifacts/boot-failure/20261027T055629.589219Z-38357/platform-dma-retained-evidence.json)：verified/root_birth_refunded=true，真实OutOfMemory，原Pool的pages allocated=4、metadata allocated=63936构造前后相同，未发布零成员根不排正常Root Work。四hart Gate Failed时仅原先三MMIO/一IRQ owner待清理。首轮Queued专属证据保留于下方历史记录，不再要求已删除路径；不关闭已暴露DMA＋IPI或整批全额退款。 |
| DMA晚期安装指定分配失败 | [dma-install-second指纹/原观察](../artifacts/dma-install-failure/20261026T071054.289613Z-82520/evidence.json)，完整ReadyUnexposed后指定releases Buffer16B失败，先前handles Buffer已实付；实际OutOfMemory/无cap无暴露，原Work members/write归零、独立Pool最后全零及core/shutdown通过。不称物理堆耗尽，acquire_step非空前缀OOM本不可达 |
| IRQ直接已park退出与含Line成员Set随宿主退出 | [IRQ第六轮原观察](../artifacts/irq-exit/20261026T100635.591971Z-41077/evidence.json)PID28/source32/gen6 Waiting/epoch2、原Line loan/节点链；PID29/gen7实际Running但cap slot4/gen1/rights69持WaitSet KOID19019，唯一Member实际挂原Line。两者Kill后各同generation原Line空链/Idle/强根回原两owner，[客体405–409](../artifacts/qemu-acceptance-20261026-180630-41066.log)Pool19003/19011两维全零及scopeDONE，整轮core/shutdown通过。第四/第五轮WaitOne最小证据仍复用，旧整体失败不改写；不代表Set宿主Waiting/所有等待形态或强公平 |
| IRQ持续事件有界推进/普通任务服务机会 | 监督已亲读原248/250/276/455及真实接线，按下方“IRQ持续事件证据映射”的结构+host+集成组合审定关闭独立差额：单claim/单Line节点锁外Delivery、SEIP Requeue→原安全点→M单advance_one→有限机会rotor、正式xHCI 16事件/more→Runnable，加PLIC/signal/rotor既有host和真实READABLE业务进展/连续Take-Ack/退出证据。7相关源文件与既有验证版本一致；未执行任意高频/同hart指定比例/无限时长压力，不宣称其运行通过。原持续事件合同保留，全idle共同快照与外部唤醒仍单列 |

当前收口方式：第二轮设备控制链、root持续监督及生命周期增量已经完成，按[提交与调查接力](#第二轮提交与调查接力)统一保存交付并转入剩余问题调查。原 DMA＋IPI 组合与独立时钟异常继续由本表承接；已关闭的全 hart idle 门见上表。历史失败保留原样，不从机制收敛推导首批全部验收交付。

#### 未发布DMA根的首轮历史证据

第二轮用局部回滚替代了下述正常Root Work接管；以下保留当时事实，不能据此恢复旧验证门。

[第八轮原观察](../artifacts/boot-failure/20261026T115952.260274Z-67134/platform-dma-retained-evidence.json)、[最终版本/独立清理](../artifacts/device-closing/root-retained-eighth-final-version.json)，仅retained、THROTTLE100、exit0。真实104/8及原Pool1 payer/精确OutOfMemory，同Root最后Node5 Drop后member sealed/finished/dropped、原Root Work Queued；首park及四不同hart逐次head/tail raw=`01000000000000000500000000000000`且fresh=5，Work raw03/fresh Queued、Root票号2/2/维护12/12、tree1/mappings0，全集合五项owner Queued。metadata64061→64074→64010/页4保持只证局部回退，停驻不能宣称五根全退；不重写旧第七轮失败/原head缺失或归因动态Type

#### 成形自查与集中验证记录

2026-10-26按REVIEW进行本次成形自查，亲读实际代码而非沿历史目录推断完成。核对范围为：srv_driver原Request/Retry提交与Outbox取消、Managed原轮停止/ReportStopped/Released/Detach、ProcessTask成员准入/封新关联/出生与唯一收集、libdriver ActivationSlot/真实能力认证及Lifetime尾部；libdevice资源唯一出口、消费者未暴露证明、Next::Release保留供应安全事实、Registry精确BindingRef来源依赖及SourceStop集合；init固定两管理者观察/正常admin关闭/撤防/join与唯一故障终局；Once运输真实payer和失败Invocation归还；xHCI原Master绝对期限/恢复分类、USB连接与descriptor无关的真实Closed门、VM强制退休Frozen分支。

- owner/权限：Run只保存选择与单Binding，进程/Pool/collector在ProcessTask，关联从原Run推导。中央Seat仍是唯一占用真值，父释放依据原Source.binding精确轮次，未把设备拓扑当清理依赖。Registrar来自ResourcesReady后真实签发，HID不获物理根；Retry成功没有授予硬件访问。
- 失败/取消：Claim或能力出口结果Unknown不得用空结果/CPU终态补Released；已暴露位不可复位。原Work确认供应安全后只等实际依赖释放，不重复供应RPC。Managed在原Binding安全Released且调用/两类订阅脱离后解除进程关联，ProcessTask无成员后才Kill/收集；出生/来源拒绝仍由原Run停止或外层SourceAdmin故障收束。
- 并发/停止：观察线程只持缩权cap和固定记录，不共享RootSupervisor账本；短锁外做RPC/wait/close。正常意图仍核真实Completed/cause/detail和同次来源Closed；异常只由一个gate取得终局authority。总故障截止对原Collector等待取min，不伪造原观察超时；正常join保留Terminal的Job/平台/reset副本供后续故障，不误记全部authority已退。
- 资源/付款：Once从独占claim的实际对象取发行者Pool，input/scratch/handles/Delivery同源，出队与接收尾部分别持charge；普通Sender仍原发送者。共享DMA Pin退休不释放父MemoryObject root，最后真实owner与Backing arena清空才退原账；真实Quota证据不泛化allocator OOM。MmioRegion拥有use不排除上级Revoke，原unsafe访问前提仍依实际停止顺序。
- 机制收敛：未新增选择表、依赖图、信用账、私有驱动运行时或并行停止协议。旧日志驱动/旧DTB目录/单激活分派已替换；各有限测试变体只选择原消费者，GDB取证/注入留宿主工具，未加产品测试opcode。未发现本次必须修正的可达缺陷；未取得证据的窗口继续列上表，不以静态推演报通过。

初次最终静态/host验证：`just check`退出0，日志`artifacts/device-closing/check-first.log`；按BUILD手册的os非kernel workspace、shared workspace、用户公共库host命令分别退出0，日志`host-os.log`/`host-shared.log`/`host-user.log`与同名`.exit`在同目录。实际passed合计分别268/119/254（含各原测试组）；没有选择用户服务binary为host target。此处不替代十面Clippy、QEMU聚合、上表集中精确证据及最终文档复核；后续修改按实际影响补测，不无理由重复以上通过项。

IRQ去重依据：`irq_checks.rs:84-93`实际`WaitSet::remove→Member::drop→Line::detach`，不是尚缺的基本取消；`function_checks.rs:479-489`提供WaitOne入口及真实IRQ后的等待完成/Take，单看这次调用不能证明曾park。退出路径亲核`reclaim::step_sweep_members`先由`termination_step`交出Waiting owner，放BodyLease后再`sched::reap→wait::detach`；后者依次退timer、在Line原CONTROLLER锁内摘节点、锁外退Hint/source loan、结束原epoch。通用Notification和hammer/race已有同一通用收束/持续电平/rotor证据，但source不是Line，故不冒充本表三个IRQ专属窗口；未发现需要加状态或新通知机制的源码缺陷。不因辅助盘点出现额外序号边界就制造新验收组合，既有host/真实启源/逐hart读回仍按其原范围复用。

集中验证进展：`acceptance-first.exit=124`原结果保留，日志`artifacts/device-closing/acceptance-first.log`。其中十面Clippy逐面通过，日志副本在`device-closing/clippy/`；debug stress完整通过（103659-75473入口）。release core在原外层35s到期，客体`artifacts/failed-acceptance-20261026-104219-86236.log`仍推进FS/Tunnel/IPC，未见内部panic/终局；按用户政策唯一`VIRT_RELEASE_TIMEOUT`35→70。该轮收尾无QEMU/GDB后，只修main里function_checks::run换行，`fmt-user-second.exit=0`，不使无关已过证据失效。失败release及未运行组合由`acceptance-remaining-first.log/.exit`接续，最终exit0：release104657-87896、sifive_u104733-89405、nofd104818-91136、boot-failure104852-92657、IPI104900-93119、VM fault104905-93527、VM repair105003-95769、DMA retention105058-96604各入口均Passed；该组结束确认无QEMU/GDB。与原十面Clippy/stress合并，通用聚合所需各段现已通过，不把原124改写为0，也不据此关闭上表精确门。源码937项哈希见`version-before-acceptance.json`，纯fmt/外层配置/文档差分见`version-before-continuation.json`，均在`artifacts/device-closing/`。

DMA保留+IPI集中窗口采用原严格core两mapping/各两页观察体，复用同一次reset拒绝→真实InitialWatch收到init DONE→原runtime_stop传播。只对第一次request广播中实际非发送者、真实WFI且无quantum期限的目标跳过一次原IPI ecall并返回SBI_ERR_FAILED；记录sender/target实际slot/raw、mask/base、实际SIP/SIE/TimerQueue/quantum及真实返回。核同一原非空exposed集合在IPI前后和四hart park仍同id/backing/range、write存在、无acquisition/release且Work Idle；随后原STOPPED广播到同目标实际返回0也须观察。无quantum不代表无其他timer/SSIP，不能宣称后续IPI是唯一唤醒。工具`virt-dma-ipi-retention`复用原check-dma-retention与观察体，只增加显式宿主变体，无产品状态/运行体；默认与变体完整生成AST/Just dry-run均0，非新版默认路线已运行证据。寄存器/PC改写只用于原SBI失败注入，不碰mapping/页表/目标时钟/PC；不把未提交硬件的地址当真实DMA静止证据。

`dma-ipi-first.exit=1`，[入口](../artifacts/run/20261026-105910-97975-virt-dma-ipi-retention.log)、[客体/原GDB](../artifacts/dma-ipi-retention/20261026T025913.842658Z-98353/qemu.log)、[原脚本](../artifacts/dma-ipi-retention/20261026T025913.842658Z-98353/retention.gdb)。真实触发是GDB外层360s TimeoutExpired，客体仍在FS推进，尚未到reset/目标IPI，未形成窗口。新增返回断点过早全程启用，导致常态SBI/TIME也截断，已修成只在选中本次IPI后启用；初始disabled并离线核完整生成AST。该轮无QEMU/GDB残留。按外层规则唯一DMA验收预算360→720，`dma-ipi-second`续跑中，未取得结果不记通过。若目标/工具仍不定，则保留具体事实、转下一独立门，不以多轮适配独占主线。

`dma-ipi-second.exit=1`，[入口](../artifacts/run/20261026-110724-99125-virt-dma-ipi-retention.log)、[原GDB](../artifacts/dma-ipi-retention/20261026T030727.718489Z-99502/gdb.log)、[客体](../artifacts/dma-ipi-retention/20261026T030727.718489Z-99502/qemu.log)、[当轮脚本](../artifacts/dma-ipi-retention/20261026T030727.718489Z-99502/retention.gdb)、[失败产物身份](../artifacts/dma-ipi-retention/20261026T030727.718489Z-99502/failed-evidence.json)。本轮不是720s超时：reset前与真实InitialWatch DONE两处mapping9/10、各range0..2、原Backing/write和Idle相同；首次request的线程4、mask1/base0处，新helper在Rust语言环境用C式带单引号global表达式，报`overlong character literal`。尚未注入IPI，组合未覆盖，两快照仅局部事实。三处表达式静态改为Rust限定名，无QEMU的当轮ELF `set language rust; whatis`逐个解析exit0（`dma-ipi-rust-expression.log/.exit`）；Python AST不验证GDB表达式，whatis也不等同运行期完整布局/读链。该修正版未运行，不启第三轮追工具。失败指纹在此轮结束、任何后续构建前采集，JSON明确这一来源，不伪称事前记录；工具今后执行前即固定kernel/boot/script指纹，失败也保留。QEMU/GDB均已退，TCP私有监听无存活owner，日志/脚本是保留证据，不含待清的QMP socket。该门回到集中待验，从同一次已确定request断点接续；不改变产品机制。

IRQ集中增量沿原`acceptance-irq-retirement`两轮实际插入，Wait返回真实READABLE后先不Take/Ack，用原BindingControl发认证QueryBinding，核原ref/ResourcesReady/零cap成功回复，再probe原use仍READABLE才Take。Host按同round/device/epoch/source/generation关联进展且要求先于原token；没有新feature/服务/协议/运行体。`irq-readable-target.exit=0`为受影响user-resources目标Clippy；`irq-readable-first.exit=0`，[入口](../artifacts/run/20261026-112554-2137-acceptance-irq-retirement.log)、[客体](../artifacts/qemu-acceptance-20261026-112559-2534.log)、[当轮指纹/机械IRQ事实](../artifacts/device-function/20261026T032607.709034Z-2545/evidence.json)。179/180和201/202分别在device2的epoch1/gen5、epoch2/gen6取得认证查询回复后才Take seq1；217两轮实际停止/来源Closed、422观察线程main-only、445辅助cap关闭、455root delegated0及完整shutdown通过，无QEMU/GDB残留。仅关闭本真实READABLE保持期间另一正式actor完成请求的最小证据；不外推曾park、同hart/rotor长期公平或持续高频IRQ负载，原强条件差额继续开放。

两个退出消费者复用原`irq_checks`、`test_target`与libprocess/独立WORKER_POOL，显式`acceptance-irq-exit`只安排原资源消费者：父从实际平台范围派生一个use，先Query固定generation并保留母本，只把READ|WAIT的alias与SIGNAL就绪cap交给目标。直接变体WaitOne挂Line；集合变体先把真实Line挂进一成员WaitSet再等待Set，二者不能混同source。ready不证明park：只读取证在原ProcessKill最终封口前核实际MemberState::Waiting、Thread/epoch/SATP/原来源；集合变体另核Member确实挂原Line。CPU实际Killed/DONE与唯一Control关闭后，先等原子Pool两维allocated/delegated/reserved零，再Query仍保有的原use；原Line应只剩Node字段和Controller源槽两份强根、无子节点，借此核摘链/loan已退，随后才显式Revoke并等use/控制scope DONE。父母本避免用最后cap Close自动撤销掩盖取消责任；此前后不是瞬时物理退款断言，也不把子Pool零替代父Line引用事实。没有产品查询op、私有运行时或假IRQ，原field/代次和付款owner不变。初次目标检查101仅漏测试目标Rights import，修正后`irq-exit-target-second.exit=0`；当前外部脚本/入口接线中，未运行不计通过。首轮如Waiting/工具不定则保留具体事实转独立门，不连续追打。
IRQ退出首轮`irq-exit-first.exit=1`，[入口](../artifacts/run/20261026-121042-15202-acceptance-irq-exit.log)、[客体](../artifacts/failed-acceptance-20261026-121047-15600.log)、[原探针/证据](../artifacts/irq-exit/20261026T041055.764052Z-15611/evidence.json)。只取得初始source32/generation6、Line强根2/空链/Idle；`IrqUse::take`先Query generation，探针把这次准备Query误认退出后采样而失败，两个退出窗口均未到，不是产品错误或外层超时。静态修成只有同generation的实际Kill记录已核后才认最终Query；未重跑。`irq-exit-resources.exit=0`为全部原资源feature的目标Clippy；运行后QEMU/GDB及私有文件均已清理。按用户节奏转独立资金门，不连续追此探针。

DMA资金可达性亲核（2026-10-26）：

| 原接点 / 付款owner | 可达失败及责任 | 本轮最小证据边界 |
|---|---|---|
| Pin入口：`dma/object.rs:305–336`，调用线程Pool | Backing storage、写许可Buffer、PreparedMapping、capability母本与Pending预付均在登记前；失败沿原局部owner退款 | 已过Quota只说明其实际早拒绝位置，不外推所有分配 |
| `Backing::begin_acquire` / `SegmentMap::begin_acquire` | 原预付两个切界槽；begin成功后acquisitions租约由Pending持有，取消由原Mapping Work接管前缀 | DMA四竞争窗已有真实非空前缀退出及原账零 |
| `memory_space/backing.rs:285–313 acquire_step` | 只查原段、checked_add use并推进游标；没有分配或付款。所谓“非空前缀后allocator OOM”在该循环不可达，UseLimit不是OOM | 修正验收表述，不增加产品分配或伪造错误来命中 |
| ReadyUnexposed后`dma/object.rs:384–387 → handle::install → Table::reserve`，仍由原调用线程Pool付款 | handles及releases两个Buffer和表增长仍可失败；reserve失败先放表锁再release_all，最后DmaMapping capability触发stop，由预付Work释放完整usage/write。仅Pin::Drop的Preparing分支不足以解释此处 | 指定一次尚未执行的真实分配返回null；严格关联原Pin/Thread/Pool/安装栈/Layout，优先releases Buffer，实际OutOfMemory且无新handle/无地址暴露，scope members0/write0及最后真实owner退账 |
| root构造/retained entry/安装 | DMA构造失败同步rollback；retained entry失败由最后Node Drop发布预付Root Work。尚无已证bug | 原root资金窗口继续开放；本次Pin指定分配失败不能代替root或真实物理堆耗尽 |

本次指定分配失败只在宿主既有GDB设施安排，不改产品allocator/错误返回/付款状态；绝不将已经成功分配的指针改null。若首轮安装栈或具体分配定位不定，保留原事实转下一独立门。

root资金接点进一步亲核：`boot/bootstrap.rs:161–168`先为整份Cap Vec付metadata/分配，再逐项构造真实MMIO/IRQ/DMA根；每个失败分支显式`release_all(caps)`。`handle::mint → Cap::mint`仅校验rights并取得原Holders，不分配、不选资金owner，因此不设“mint中Quota/OOM”假窗口；新根首次mint的rights来自固定合法常量。真正后续资金接点为`bootstrap.rs:271–280`对Building进程表的reserve（两个Buffer及可能表增长）；此时尚无用户Start或能力逃逸，失败放弃原caps并由publish外层freeze_and_seal。构造侧MMIO/IRQ由Node Drop与status检查配合同步回滚空树，DMA retained entry失败由原Root Work持有异步回收。启动全局失败可能停止运行态维护，须区分“退款已完成”与“原预付稳定owner仍持责任”；不得把boot park单独写成全额退款。该行是源码可达性/owner审计，root实际失败注入仍未覆盖。

晚期安装指定分配失败首轮：`dma-install-first.exit=124`，[入口](../artifacts/run/20261026-125001-32987-acceptance-dma-install-failure.log)、[客体](../artifacts/failed-acceptance-20261026-125006-33377.log)、[原工具日志](../artifacts/dma-install-failure/20261026T045013.722825Z-33388/install.log)、[原指纹/窗口](../artifacts/dma-install-failure/20261026T045013.722825Z-33388/evidence.json)。PID28/Pool19456/mapping8已真实ReadyUnexposed，完整range0..2、use/write持有、reader0，原Thread/SATP及合法current0齐全；尚未注入。探针在releases allocator停点按demangled CapTable/`::reserve::`匹配DWARF帧失败，GDB退出后Pin正常成功而目标触预期OutOfMemory断言。故不是产品回退失败、不是1120外层超时，不能翻倍或记该门通过。静态改按原capability/src/table.rs与reserve函数联合定位，补失败前栈落盘及Backing身份核验；修正版未运行，集中待复核。原资源分面Clippy/fmt/diff exit0；QEMU/GDB与本轮私有文件已清理。

root最小首轮复用原boot失败工具安排两个独立变体，不增加产品接口：

| 窗口 | 实际选择与起止 | 成立判据与退款边界 |
|---|---|---|
| 首MMIO根Node构造 | `MmioNode::root`合法范围检查后、预付树前取原Pool；在同一Node::new的Arc实际未执行分配处指定失败；观察bootstrap原MMIO构造Err分支 | 真实OutOfMemory、原空根结构已析构/同步回滚、此段Pool账恢复、无rootcap交付；随后原boot失败Gate Failed/四hart park。不是入口alloc handler，也不修改已分配指针 |
| 全批Building cap安装 | 全部真实平台根已mint，bootstrap原reserve前；其releases Buffer真实分配失败，在原Err分支核表/Pool | 实际OutOfMemory、未提交任何槽、两个预约Buffer的付款已退；原release_all/freeze接管所有已构造root。全局Failed若不再推进原维护，只核稳定Work/引用owner仍在，不能称全部root退款 |

两变体只用一次真实allocator入口失败，分别记录调用栈、实际Layout/原Pool、before/after及当轮ELF/包指纹；原panic/门/四park设施及专有进程组清理复用，不重跑已通过三种boot入口故障。

`root-funding-first.exit=0`，[入口](../artifacts/run/20261026-130159-48605-virt-platform-funding.log)、[构造原观察](../artifacts/boot-failure/20261026T050204.934959Z-48974/platform-node-gdb.log)、[构造指纹/账](../artifacts/boot-failure/20261026T050204.934959Z-48974/platform-node-evidence.json)、[安装原观察](../artifacts/boot-failure/20261026T050204.934959Z-48974/platform-install-gdb.log)、[安装指纹/账](../artifacts/boot-failure/20261026T050204.934959Z-48974/platform-install-evidence.json)。构造在实际首ECAM范围0x30000000/256MiB的Node Arc 208B/align8未执行入口注null，Pool1 metadata63413→63537→63413；安装在11cap的releases176B/align8入口注null，64085→64118→64085、表occupied0。构造实际返回OutOfMemory；安装内部实测AllocationFailed、boot对外错误映射OutOfMemory。两者均由原boot错误至Gate Failed及四个不同hart park。安装失败原3MMIO/IRQ/DMA五份Node Work真实Queued并保留到四park；只关闭这两个局部资金窗口，不称五根全退。ELF7773d62e、包a6b31496事前指纹固定，两场结束无QEMU/GDB或运行器残留。

DMA retained-entry另列首轮：在`dma/mod.rs:197`已完成Node构造及根树弱链接、尚未进入NODES时，实际`PreparedEntry<Arc<Node>> → OwnedBox`分配失败。该错误值保有的Arc clone先退，再由局部Node最后Drop同步seal/finish、enqueue_reclaim，发布原Root自身的预付Work强根；必须观察这个Root Work及原reclaim成员，不能用上一安装失败的五份Node Work代替。该轮boot全局失败可以核责任稳定保留，不要求运行态维护已完成或账已全退。只选原entry的实际Layout/同Pool，一次未执行allocator失败，具体接点/退款边界与其余两种root窗口分开。

`root-retained-first.exit=1`，[入口](../artifacts/run/20261026-130630-50154-virt-platform-funding.log)、[原观察](../artifacts/boot-failure/20261026T050633.583941Z-50528/platform-dma-retained-gdb.log)、[事前指纹/前置](../artifacts/boot-failure/20261026T050633.583941Z-50528/platform-dma-retained-evidence.json)。真实到Node::root197、node5/root0xffffffc08087d4f0、cap_count10/Pool1，仍未选择原分配/未注入。完整pretty类型过滤未认目标OwnedBox，探针持续截获后续全局分配（16705行），直到运行期同类候选缺Root祖先报错；不是产品回退错误，也不算外层到期。静态改以funded_storage源码+try_new和当次Root祖先选取，候选最多保存四份原栈，Root返回未命中即停，断点静默；修正版未运行。与DMA安装失败/IRQ退出/IPI表达式修正版一起集中留在上表，当前不二轮追打。两项已通过root门保持原版本证据，无QEMU/GDB或该运行器残留。

MMIO Frozen首轮的两个最小提交窗（复用原`mmio_checks::process_retirement`/`test_target`两次32页Map、父Revoke→Kill，无新Runtime）：

| 窗口 | 当轮实际停点与原owner | 放行与成立判据 |
|---|---|---|
| FIFO未开始 | `ForcedRetirement::advance`prologue后、upgrade VM前的当轮266地址，原step栈Execution.phase=Idle、active=None，FIFO包含至少一份原mapping，且所见项为原mapping集合的子集；保留原两Map消费者，不要求同时排队 | 父真实Revoke成功返回、Kill完成原VM封口，核access_open=false后立即恢复原Work；未开始mapping交整VM退休。快照外另一Map责任仍须原scope DONE/members0、VM/CPU收束、Control关闭/独立Pool零闭合；两项同时积压若观察到只属额外证据 |
| 已提交Publishing | 当轮反汇编核到`step`中advance返回后、重新取得queue锁前地址（源码421仅索引），原栈Execution.phase=Publishing、active是原mapping并已提交；不强求另一mapping同时在FIFO | 不能停404处持VM锁再跑Kill。父封口后立即恢复，不等待DONE；核同一已提交phase在Frozen后完整结束/回Idle，再处理未开始交接。原两Map的全部责任最终仍由scope/VM DONE、进程/Control/账收束证明 |

只安排实际CPU顺序，不改VM/许可/队列/PC/时钟。若未到所选最小窗口或缺独立可推进父CPU，记录真实状态为窗口未形成；首轮不定即转项。暂停持有的Execution/访问租约属于原Work，不据`self.execution=None`判丢owner，也不在恢复前等scope/进程DONE。

MMIO Frozen启动/首轮边界：`mmio-forced-first.exit=2`，[入口](../artifacts/run/20261026-132218-52617-acceptance-mmio-forced.log)，Just output插值丢features引号，未构建/未运行；改原private config recipe后，`mmio-forced-second.exit=1`才是实际首轮，[入口](../artifacts/run/20261026-132609-52876-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-132614-53274.log)、[原探针](../artifacts/mmio-forced/20261026T052622.386335Z-53285/forced.log)、[事前指纹/空窗口](../artifacts/mmio-forced/20261026T052622.386335Z-53285/evidence.json)。机器码确认266地址0xffffffc080224578、421地址0xffffffc08022576a在advance返回与下一Queue锁之间；客体405–407仅证目标pid26/两Map就绪与原Revoke已返回。探针选择/读链发生MemoryError0x56，pending/windows均未落下，故两个Work窗均未形成；不把前置正常core/真实Revoke当Frozen证明，不称产品缺陷或1120外层超时。静态选择点改到Revoke279 prologue后/Root锁前、Kill537 prologue后/封口前，增加有限选择阶段落盘；这只是修正后待复核，原异常的精确字段仍未定位。受影响目标Clippy0、fmt0、完整生成AST0；结束再次核无QEMU/GDB/运行器残留，不立刻第三轮。

MMIO额度与来源边界已分开去重：`Space::new`预付`max_transactions+1`，普通门排除0key后按limits检查；强制reserve不占普通额度、不再分配。内核唯一16配置进入AnySpace，VmShell dependencies按真实capacity17付存储，Forced出生即付plan并只走同一强制入口。复用已过host满普通槽测试及真实退休消费者关闭纯容量合同；不新增“QEMU同时满16”门。来源/uaccess另用完整function-session-second客体104/414及`mmio_checks::run`实际错误断言/重用事实，不由host包办IO；它们不覆盖Frozen/足迹冲突/allocator失败/IPI。

Retry迟到Query首轮最小场景：复用原device-reuse的C已Released/Managed退场/解除进程关联且HID子Pool委托已归还后的单次显式政策位置。仅显式验收feature用两个独立Caller发两次真实Control Retry：原主线持原Sender，一条libruntime线程只持其WRITE|WAIT副本、预付线程环境与有限返回结果；不借RootSupervisor/Manager记录，不新增产品闸门、延迟或重试。普通路线仍一次Retry。主线无论自身调用成功/失败均先join并核子线程结果，原registry reap与最终观察线程main-only/原Pool收口继续负责尾部；Unknown不盲重发。

只读取证须实际见两条请求都在同一settled Rc/ref/implementation上完成begin，然后一次成功删除纯记录，另一条Query返回到commit。若第二条仅在begin已NotFound，则窗口未形成；若commit只见旧记录已删除，无新Run出现，仅关闭重复删除子情形。新Rc/同Rc新ref/implementation必须分别有实际更替及原guard拒绝证据，不由两RPC一成一败外推。Seal提示的授权边界按上表独立证据闭合，不混入本窗口。GDB只在本次HID故障注入工具退出后接入同一客体，固定当前Driver ELF/包，按实际Driver PID、用户sscratch→原HartLocal/Thread/SATP与原Request/RPC txid核身份；不改寄存器/状态来选择调度次序。首轮两begin或迟到点未形成即记录转项。

`retry-race-first.exit=1`，[入口](../artifacts/run/20261026-135650-61409-acceptance-device-retry-race.log)、[客体](../artifacts/failed-acceptance-20261026-135655-61822.log)、[原观察](../artifacts/device-retry/20261026T055836.689695Z-61833/retry.log)、[ELF/包与原记录](../artifacts/device-retry/20261026T055836.689695Z-61833/evidence.json)。真实Driver PID9同一Thread/SATP、两次begin为同settled Rc859200/Device5 e1/impl2、txid339/340，339在同原记录上commit并重置。探针先把原活PCI Busy回复（未关联本C的begin、txid=null）计入两响应，看到339成功就提前结束，340迟到commit未到；不是并发产品缺陷/超时，也没有两客户端收齐或子线程退款证据。静态仅计关联本目标begin的响应，未重跑。当前夹具数量只作辅助，新增Caller无论结果均join/reap；实际退休由同一ResultReceiver的Tid/control关联原registry唯一Live槽及stack owner，再核同槽Free，不再解引用已退control/stack。Native日志只报告joined/数量，异常结果也经过原reap，Unknown不重发。目标Clippy/fmt/完整生成AST通过；所有QEMU/GDB/本轮运行器已清理。首轮局部事实与普通reuse已过完整组合分开，不因新增fixture复跑旧组合。

Seal提示安全门复核：`retry-seal-host.exit=0`，原`source_closed_projection_cannot_be_followed_by_late_commits`补一条实际Registry新claim，在Seal/SourceStop Closed之后与原late commit均返回Closed。该例原设备仍来自另一有效ROOT登记，拒绝不是靠清除本地选择实现。静态亲核Retry.commit只移除World.runs/置dirty，Claim从srv_device Request508到Work.commit535/BindingWork291同actor进入Registry.claim923/commit_claim951核真实source。提示不承担权限，允许本地纯记录先重置；没有新跨服务原子点/查询/Watch owner。未实测RPC提示延迟，按独立授权owner的host与真实消费者接线闭合，不新增该时序为必过门。

#### 六项统一复核批次（2026-10-26）

本批冻结[943项源码/工具清单](../artifacts/device-closing/version-before-recheck-batch.json)，HEAD仍为549a9e7；[批次状态](../artifacts/device-closing/recheck-batch-state.json)逐项保存实际命令/退出码/无残留检查。仅各跑一轮，工具未定保留原门并转下一项，不追加循环；全hart idle/PCI观察不混入本批。运行期间不改源码/工具或并行构建覆盖产物。线程退休按同一Tid/control与原registry唯一Live槽/stack owner关联，join/reap后核同一仍存活registry槽Free；不解引用已退control/stack，数量只辅助。

- `root-retained-second.exit=1`：[入口](../artifacts/run/20261026-150603-72759-virt-platform-funding.log)、[原观察](../artifacts/boot-failure/20261026T070609.320153Z-73150/platform-dma-retained-gdb.log)、[指纹/前置](../artifacts/boot-failure/20261026T070609.320153Z-73150/platform-dma-retained-evidence.json)。Node5/root0xffffffc08087d4f0、cap_count10/Pool1，metadata64061；下一allocator在保存候选栈前被Root祖先校验拒绝，未执行注入/未证回退或Root Work。离线[机器码](../artifacts/device-closing/root-retained-post-batch-disassembly.log)核2742a8在2742ce调用entry之前，不能解释为停点已越过分配；根因保留未定。原组清理后无项目QEMU/GDB。
- `irq-exit-second.exit=124`：[入口](../artifacts/run/20261026-150611-73202-acceptance-irq-exit.log)、[客体](../artifacts/failed-acceptance-20261026-150615-73566.log)、[原观察](../artifacts/irq-exit/20261026T070624.263902Z-73577/evidence.json)。前置FS已完成，Copy健康任务在10s总测试预算内只推进12288字节，实际Outcome为Transport(Timeout)，256.001016s主线收到完整取消/失败结果后主动终局；IRQ consumers尚未到，sources/targets均空。此为前置验收预算/RPC失败，非1120外层耗尽，未关闭任何IRQ退出门；不由exit124机械翻倍设备政策。原组清理后无项目QEMU/GDB。
- `dma-install-second.exit=0`：[入口](../artifacts/run/20261026-151033-82088-acceptance-dma-install-failure.log)、[客体406–413](../artifacts/qemu-acceptance-20261026-151042-82508.log)、[原探针](../artifacts/dma-install-failure/20261026T071054.289613Z-82520/install.log)、[指纹/完整窗口](../artifacts/dma-install-failure/20261026T071054.289613Z-82520/evidence.json)。PID28/Pool15980/MemoryObject15983/mapping8，Pending真实owner与SATP成立，hart_current=0合法；完整range0..2/write/readers0/ReadyUnexposed后，真实handle::install→CapTable::reserve第二个releases Buffer的16B/align8尚未执行allocator返回null，首个handles和第二Buffer charge都已实付。syscall实际OutOfMemory，子未取得handle/未expose，原Work members0/write0、scope DONE；CPU后metadata142尾部，关Control/父MemoryObject后子Pool两维allocated/delegated/reserved零。profile core及显式shutdown通过，原组无残留。只关闭这次指定分配失败与回退，不称真实物理OOM/所有资金点。
IRQ前置Copy预算留证：本次并发取消/健康任务分别起于244.141205/244.149005s，原绝对end254.141205/254.149005s；同步ReadAt249.982152→250.768370s（0.786218s）。健康任务最后local_read/transported12288、destination_confirmed0、两端terminal None，Outcome Transport(Timeout)，未据此宣称目标提交全部数据。本批结束后再区分整次测试预算与具体RPC期限，本批冻结不改预算。

- `mmio-forced-third.exit=1`：[入口](../artifacts/run/20261026-151442-90405-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-151450-90829.log)、[原观察](../artifacts/mmio-forced/20261026T071515.828533Z-90840/evidence.json)。尚未到MMIO消费者，Copy健康任务215.738014→225.738014s总测试预算内持续推进至23040字节，但destination_confirmed0/两terminal None，实际Transport(Timeout)后226.282198s主动失败；同步ReadAt221.411331→221.515796s（0.104465s）。两个windows仍空，inferior退出后No registers是工具收尾，未复现/解决原MemoryError0x56，不证明Forced/Frozen失败。原组无残留，本批不改预算、不重跑。

- `retry-race-second.exit=1`：[入口](../artifacts/run/20261026-151839-98646-acceptance-device-retry-race.log)、[客体](../artifacts/failed-acceptance-20261026-151845-99085.log)、[原探针](../artifacts/device-retry/20261026T072114.464059Z-99096/retry.log)、[当前指纹/阶段](../artifacts/device-retry/20261026T072114.464059Z-99096/evidence.json)。真实A/B/C与HID Fault、B/C Released/Detach/一次收集后，活PCI Retry Busy已有局部证据；新增caller源码点0x24ee6当前内联帧为core::Option::is_some，frame.read_var(worker)失败，begins/commits均空，仅1无关响应。未取得两Caller结果/目标迟到交错/同registry槽退休；不是产品Retry或线程回收失败，不是超时。保留PC/栈和Init/Driver/包指纹，本批不现场改或重跑；原组无残留。

- `dma-ipi-third.exit=1`：[入口](../artifacts/run/20261026-152130-4886-virt-dma-ipi-retention.log)、[原观察](../artifacts/dma-ipi-retention/20261026T072134.509958Z-5250/gdb.log)、[客体](../artifacts/dma-ipi-retention/20261026T072134.509958Z-5250/qemu.log)、[事前指纹](../artifacts/dma-ipi-retention/20261026T072134.509958Z-5250/evidence.json)。真实request sender slot/raw1向非sender slot/raw0、mask1/base0，在requested1/stopped0且目标WFI/quantum0/timerFirst空/timerPending=false/sip0时执行单次ecall跳过与-1返回值注入；所选失败返回没有进入IpiReturn日志，`failed IPI return was not observed`，因此不关闭组合门。后续STOPPED=1原广播对同target error0成立。两原mapping9/10、同backing/range0..2/Idle在reset拒绝前、initDONE、注入前及四park时一致，单独保留成立，不表示DMA静止或所有IPI失败安全；不扩大为唯一唤醒原因。原组无残留，未外层超时，本批不再迭代。

本批六项结果为1/124/0/1/1/1；指定DMA晚期分配失败关闭，其余明确分类为：IRQ/MMIO共同前置Copy失败而未到目标，Root/Retry为目标工具识别失败，DMA+IPI已有注入前后部分事实但缺选中返回。当前无新已证产品缺陷。全部本轮QEMU/GDB结束，943项冻结清单除本计划回填外无变化，见[批次最终状态](../artifacts/device-closing/recheck-batch-final-state.json)。不自动开启下一整批，不将整体批次exit1改为成功。

本批之后的最小下一步集中登记如下；这不是新机制或新增验收范围：

| 剩余组 | 当前准确断点 / 最小下一步 | 产品缺陷判断 |
|---|---|---|
| root retained-entry | root197前置与entry实际调用已定；先离线核`in_source`对当前Root祖先的识别及完整候选栈保存，目标返回即止损，再核指定失败后的原Root Work而非Node Work | 未确认，当前为工具缺证 |
| IRQ两退出/强公平差额 | 先统一处理前置Copy测试预算；之后沿已修的实际Kill身份读Line Waiting或WaitSet成员+宿主Waiting/摘链loan。长期/同hart公平不由两次Query外推 | 未到目标；强公平仍缺独立证据 |
| MMIO Forced/Frozen | 同一Copy前置先闭合；保留queued224578/Publishing22576a两个机器点及原Revoke/Kill身份，真实到场后才评价原0x56静态修正 | 未到目标，原读链原因未定 |
| Retry迟到Query与caller退休 | 固定Init ELF的0x24ee6当前为内联Option；先离线核拥有worker的外层帧/有效作用域，不更改产品或以数量替代Tid/control/registry槽 | 工具缺证，无产品结论 |
| DMA保留+IPI | 实际选中请求和后续原发送已取到；先离线核跳过ecall抵达return地址时断点执行/每sender原call关联，补实际-1观察，保持既有映射集合断言 | 返回缺证，不改停止机制 |
| 全hart idle/PCI观察 | 本批未运行；先有限采原input.now_ns/next_poll/完成→同advance提交，才判断物理停止观察是否有重复Runnable；共同idle仍须同一四hart完整快照及真实claim/Take | 尚未证明产品轮询缺陷 |

#### 批次后的离线修正（2026-10-26，无客体运行）

本段明确晚于上述冻结批次，不回写其运行结果。沿[原ELF身份](../artifacts/device-closing/offline-fixes/elf-identities.json)找到与Retry/IRQ运行记录hash完全相同的缓存srv_init，内核仍7773d62e…；没有重建来替代原ELF、没有启动QEMU或执行目标GDB脚本。当前改动仅为Copy验收预算与三个具体探针，不新增产品接口/状态/监督。

- **共同Copy前置**：亲核`filesystem/copy.rs::task`同一`COPY_SCENARIO_TIMEOUT_MILLIS`构造起点绝对deadline与FollowLimits，CopyTask的RPC/session同用原总界。IRQ/MMIO两轮都有实际推进，主线ReadAt分别仅0.786218/0.104465s，不能把主要成本归给同步暂停；日志不足以区分任务到期与同总界RPC谁先产生Timeout。按用户测试防挂死规则，唯一常量10→20s，不逐步骤续期，不改provider业务租约、通用RPC helper或设备20/30/40政策。两次原未完成结果保留，20s版本尚未运行。
- **Root归属**：`in_source`不再只看当前内联SAL，而核实际命名frame定义scope的function/source；候选完整栈在祖先过滤前有界落盘，checkpoint保存真实caller返回PC、Root返回未注入即止损。离线核2742a8及2742d2属于原Node::root、定义scope拥有pool；不能据此断言已定位原运行祖先失败的具体原因，更不宣称回退已证。
- **Retry变量**：原0x24ee6的worker、0x25010的caller_id均在同retry_concurrent外层词法scope，工具沿实际外层frame找精确symbol并`symbol.value(frame)`，不从当前core内联帧猜变量。读取失败保存inline链；原两begin/ref/implementation/Rc与同Tid/control/registry槽核验保持。只证明作用域布局，实际变量读取与交错仍待运行。

**Retry第三轮开跑前核验。** 原`acceptance-device-retry-race` feature经Just仅准备当前产物（[构建日志](../artifacts/device-closing/retry-race-third-build.log)exit0），Init5066bdd5…、Driver62499ed5…、kernel7773d62e…；[本轮scope/生成检查](../artifacts/device-closing/retry-third-offline/validation.json)exit0，worker源码178实际点24ee6/scope24ee6–253be、caller_id源码207实际点25010/scope24efc–253be均由真实retry_concurrent拥有，[声明记录](../artifacts/device-closing/retry-third-offline/scopes.json)沿worker声明的registry字段核ThreadRegistry<NoEnvironment>。这些绝对点仅属本轮产物，每次运行仍从实际ELF生成；未调用runtime Symbol.value的部分不以离线通过代替。

亲读探针Run读数只在单次停点物化为标量字典，不跨停点保存Run的Value/Type；另将caller_registry原(addr,registry.type,slot)收敛为(addr,slot)，两端按当前ELF的静态ThreadRegistry声明新读，记录声明objfile/字段布局。原Tid/control身份、唯一Live槽与后续同槽Free仍严格，不解引用已退休control/stack。没有产品更改、调度闸门、延迟或额外Retry；不更改原两begin/实际commit/响应关联判据。完成冻结后只运行THROTTLE100原reuse变体一轮，记录实际分支与caller退休，未形成窗即结束，不串MMIO/IPI/idle。

**Retry第三轮结果：现场外层帧缺失，目标交错未覆盖。** `THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-device-retry-race`退出1：[入口](../artifacts/run/20261026-201153-68726-acceptance-device-retry-race.log)、[客体](../artifacts/failed-acceptance-20261026-201157-69111.log)、[原证据](../artifacts/device-retry/20261026T121245.406377Z-69122/evidence.json)、[原GDB](../artifacts/device-retry/20261026T121245.406377Z-69122/retry.log)。非外层期限耗尽、非内部失联/硬件时限失败；GDB在精确caller变量查找报错后退出，主机verify未取得目标证据而终止本轮，没有正常shutdown结论。

实际pid1/Thread/SATP门通过后的Caller点24ee6、sequence2、hart0，保存thread18446743800988150096/frame18446743800988150160/SATP0x8000000000080200。可见frame链只有`Option<subscription::Failure<libdevice::Error>>::is_some`（24ee6）与`Option<Result<libdriver::boot::Declaration,Error>>::expect`（24e64），没有retry_concurrent外层，故worker Symbol按原严格门拒绝。begins/commits/目标responses为空，只有1条无关活PCI响应；没有两begin/一次reset/迟到commit实际分支，也未读到caller Tid/control/Live槽或其退休，不能关闭removed-record或其他子情形。

本轮Init ELF单独加载时scope存在、但live同时加载kernel/Driver/Init后未得到该外层帧，离线存在不等同运行值可读。实际静态ThreadRegistry声明objfile为srv_driver，size48/字段布局已落盘；该差异单独不判原因，随后以同hash ELF真实加载/移除重现的证据见下段，明确属于同用户VA符号冲突，不写成Run动态Type回退。第三轮未保存的额外现场寄存器/栈不能事后补回；原整体失败和未覆盖交错维持，不改产品或增加调度门。

客体325/441实际确认A/B/C同PID14及独立输入，451–510记录真实HID fault、B/C原Binding Released/Managed退场、PID14仅一次收集，513活PCI Retry拒绝；这些是本轮局部行为证据，514的探针验证失败使整轮停止，不等于并发Retry或完整正常收束通过。实际kernel7773d62e…、Driver62499ed5…、Init5066bdd5…、BootPackage e7cf4c8b…（运行Just重新生成，非离线前置9807bd8c…）、脚本2688dc45…、工具4d0e3adb…，按本轮实际内容核hash。

[最终944项/独立清理](../artifacts/device-closing/retry-race-third-final-version.json)核执行期无源码/工具差分，just/QEMU/GDB/cpulimit/helper无残留，当前运行创建的私有run目录/文件均已清，原日志和证据保留。四组待证与时钟异常各自维持，不因A/B/C局部成功减少Retry门，也不由工具失败增加产品状态/防御层。本次仅此一轮，未启动MMIO/IPI/idle或第四轮，不重跑已过聚合/root。

**Retry同VA符号归属修正（第三轮后，仅离线）。** 监督与本轮离线复核均在当前同hash kernel7773d62e…/Driver62499ed5…/Init5066bdd5…上按旧顺序同时加载：`block_for_pc(24ee6)`实际返回Driver objfile的`Option<subscription::Failure>::is_some`；以已加载objfile的绝对文件名移除Driver后，同PC立即归Init retry_concurrent且worker声明可见。[完整切换/声明记录](../artifacts/device-closing/retry-symbol-switch-offline/switch-scopes.json)保留旧冲突与后续各点结果，第三轮实际错误帧与这个独立重现相符。修正宿主符号选择，产品保持原合同。

`qemu_driver_retry_gdb.py`保留kernel，五个断点仍原绝对地址。每次停住先运行原sscratch→Thread/Process PID与真实SATP的纯地址门，未认证即继续且不读用户Frame；认证后仅保留实际用户ELF，按已加载objfile绝对路径移除另一映像，再调用invalidate_cached_frames。随后在单次observe_user函数内新取Frame/词法Symbol/Type/Value，返回后只留下JSON标量及caller_registry的地址/槽号，避免卸载时还持上个用户objfile的调试对象。Init的准入和退休两点分别重新lookup/核registry声明objfile为Init；不改guest/satp/时钟/调度、原响应或交错判据，不建立通用调试框架。

[离线生成与同ELF加载检查](../artifacts/device-closing/retry-symbol-switch-offline/validation.json)exit0：[原GDB核验输出](../artifacts/device-closing/retry-symbol-switch-offline/switch-audit.log)实际执行Init24ee6(worker)→Driver151bc(begin/self)→Init25010(caller_id)→Driver15320(commit/retry/world/info)→Driver1558e(reply/self/error)→Init24ee6的切换，所有函数/symbol都归期望用户objfile，两次Init点的ThreadRegistry声明也确属Init。完整生成AST通过，原owner/record/reference/optional/变量查找等严格函数、最终分类AST及五个绝对点与第三轮实际脚本一致。此处只核声明/选择，未连接QEMU，无runtime变量值、真实栈展开或卸载时断点连续性运行证据；两begin/迟到分支/caller退休仍未证。工具a6ec8841…，本段不启第四轮或串其他专项，产品check/Clippy及已过聚合不因Python工具修正重复运行。

**Retry第四轮：removed-record真实交错与caller退休通过，整条正常收口。** `THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-device-retry-race`退出0：[入口](../artifacts/run/20261026-203034-72694-acceptance-device-retry-race.log)、[完整客体](../artifacts/qemu-acceptance-20261026-203038-73077.log)、[原探针](../artifacts/device-retry/20261026T123128.086138Z-73093/retry.log)、[原字段与分类](../artifacts/device-retry/20261026T123128.086138Z-73093/evidence.json)。原同VA符号修正已在本轮实际生效，Driver→Init→Driver→Init切换后，两个Caller点与Driver begin/commit/reply均读取到原函数/声明，registry两端objfile都为Init；未改guest、调度、RPC或产品门。

两个begin为txid352/353、sequence3/4，均Rc6467840、Device(incarnation2176,serial5)e1、implementation2、Terminal/task0/无process/两Watch0。352在sequence5 commit时current仍原记录，sequence6响应success；353到sequence7才commit，current实际None，sequence8响应NotFound。两begin均早于首次成功，迟到回复没有再次清除记录；verified=true/coverage=removed-record。此运行不覆盖新Rc已出现、同Rc的ref/implementation已变化等分支，不能仅因一成一败概括全部交错。

Caller原Tid7、control_arc21231328、registry21210624/槽2，在sequence2明确Live并持实际stack/control；join/reap后的sequence9同槽Free且同caller_id=7，未解引用已退Control/stack。客体514也确认main=None/worker=Some(NotFound)、caller_tid7、joined=true及retained_records2，数量仅辅助原身份取证；515确认纯选择重置。原HID PID14被真实fault、B/C安全Released/Detach且仅收集一次；完整core随后通过，729 worker委托两维零、727/738两管理者正常撤防、741 registry仅main、755 acceptance域collected、766–767显式shutdown。

[最终944项/实际身份与独立清理](../artifacts/device-closing/retry-race-fourth-final-version.json)核执行期无源码/工具差分；kernel7773d62e…、Driver62499ed5…、Init5066bdd5…、本轮重新生成BootPackage cb5be827…、实际脚本533191d3…、工具a6ec8841…，不能混第三轮或离线脚本的包/hash。just/QEMU/GDB/cpulimit/helper零残留，私有run目录/文件均退，第三轮原失败保持。此次不启动第五轮或MMIO/IPI/idle，不重跑root/聚合。

**原迟到保护合同的其余分支：按责任链评估，不机械拆运行门。** 原717要求迟到Query不能清除替换后的记录；Request在[control.rs](../user/services/srv_driver/src/server/control.rs)137–175先clone原强Rc并独立保存BindingRef/implementation，整个Query期间旧allocation仍有强owner，因此新的Rc不能因旧allocation被回收而产生地址ABA。该模块178–219的commit在同actor同步提交内先取current/核ptr_eq，再核settled与保存字段，最后211才remove/置dirty，无await或中途执行其他World任务；不同Rc会在196–197 Conflict返回，原Rc的新活轮在201–202 Busy返回，新轮再次Terminal但ref/impl变化则204–206 Conflict返回，这些拒绝均先于任何删除。Outbox已结束的完成结果在267–270跳过commit并沿原Dispatcher注销，取消不旁路保护。

[Manager::consider](../user/services/srv_driver/src/server/manager.rs)62–112是原Run的ref/impl唯一替换点：有prior时复用Rc整体写replacement，无prior时新Rc插表；spawn准入失败精确恢复rollback，接受后新阶段Starting，后续Terminal仍由Managed安全Released/Detach门拥有。[Run::settled/can_fallback](../user/services/srv_driver/src/server.rs)109–125及既有Catalog host的严格有限游标/真实负候选新epoch证据说明变更来源，不把这些host/集成证据写成commit的未运行分支已取值。上述实际唯一mutator/强owner/同actor无副作用校验，连同本轮真实RPC迟到删除反例，形成可闭合原记录保护合同的分层证据；保留直接运行覆盖仅removed-record的限制，不因实现有多个分支自动增加新运行体/每枝QEMU。此为已完成的静态评估，不新增状态/重试机制或产品修复。

监督随后亲核上述第四轮完整序列与关键代码，审定关闭原迟到记录保护合同；实际运行仅removed-record，其余拒绝分支按源码责任证明的边界保持。矩阵移入已过区，当前三组剩余门及独立时钟异常继续分开，无需追加Retry/root或每枝QEMU。

**MMIO Forced/Frozen第五轮前有限修正。** 亲读`qemu_mmio_forced_gdb.py`原vm_type=vms[0].type、execution_type=execution.type后，在Kill返回处再次cast，确实跨越Phase/Option转换沿用了运行值类型。本次只保存原VM/栈Execution数值地址，每个停点以当前ELF静态`kernel::vm::forced::Execution`与`kernel::vm::change::VmShell`声明新读；既有VM/scope/mapping/PID身份、vCont父子运行顺序、入口Idle/active=None/两FIFO、Publishing/active+第二FIFO以及后置handoff门保持。选中窗口与Frozen复核补Phase/active完整原字节，后者先落实际access_open/地址/阶段，再核原Frozen与held phase；不改内核、消费者、返回值、PC、SATP或时钟，不宣称历史0x56已解决。

[离线核验](../artifacts/device-closing/mmio-forced-offline-fifth/validation.json)exit0：[声明/作用域](../artifacts/device-closing/mmio-forced-offline-fifth/declarations-scopes.json)沿ForcedRetirement.execution的Option字段及vm的Weak字段链分别核Execution1128B/VmShell760B，非从旧运行值名字反查。当前Queued224578/Publishing22576a由本ELF重新定位，advance调用22575a后/下一Queue锁前与Revoke实际锁前关系仍由生成器机器码门严格核验；ProcessKill原外层caller与两处Execution/self声明也有效。完整AST与原vCont/owner/遍历helper对比通过，无远端连接或客体运行。

原mmio-forced feature经Just构建0（[日志](../artifacts/device-closing/mmio-forced-fifth-build.log)）；实际test_target源码8f28da60…/debug ELF b9e0c0bd…按原strip-debug派生后等于initfs条目a38c0c60…，BootPackage内Init和payload分别等于实际ELF/完整initfs。当前Init01451872…、kernel7773d62e…、工具07e7a74a…，运行仍将重新记录实际包/脚本身份。完成冻结后按授权仅THROTTLE100/KEEP_LOG单项第五轮；clock终局/前置失败或工具未定均保存首因和清理，不自动第六轮或其他专项。

**MMIO Forced/Frozen第五轮：识别两Map与VM，工具指针门失败，窗口未形成。** `THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`退出1：[入口](../artifacts/run/20261026-205413-80313-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-205417-80695.log)、[原探针](../artifacts/mmio-forced/20261026T125420.960234Z-80706/forced.log)、[原字段](../artifacts/mmio-forced/20261026T125420.960234Z-80706/evidence.json)、[执行/清理](../artifacts/device-closing/mmio-forced-fifth-state.json)。本次不是1120s路线期限耗尽，clock专用旁观harvest=null仅表示该wrapper未触发，不据此概括其他QA清理。原GDB错误为expected direct-mapped kernel pointer，主机verify据verified=false结束；不改期限、不将其归成已证内核缺陷。

本次Revoke锁前2be2b6已在原init consumer身份下选中mmio-forced-queued：PID26、scope30064771098、原Thread18446743800988149744，scope mapping table实际两份physical与scope一致/各32页、同VM18446743800990012400/SATP9223372036855301933、mapping ids5/6，以上pending已保存。此处首次VM按静态声明新读有效；held Execution尚未读取，不能由前置通过验证整个Phase/Option转换读链。随后未保存revoke_returned、kill_frame、phase/work/queued_ids等窗口字段，windows=[]/verified=false，Publishing消费者未到。

客体406–408证明queued target admitted、独立Pool21044在mapped-before-revoke时pages2149/metadata63828（delegated0）及原Revoke实际返回；但该日志不能补上探针未建立的held Work→Kill Frozen→恢复顺序。错误指针数值及抛错当时PC/调用栈未在GDB异常前保存，225是Python块的end行而非具体失败指令，不能从最后pending推定哪份响应/哪段owner读取出错；旧0x56本轮未出现也不当作已解决。最终scope DONE/CPU Killed/Pool零及原队列交接均未证，不用正常core前半程或错误前余额代替。

[最终944项/本轮身份与独立清理](../artifacts/device-closing/mmio-forced-fifth-final-version.json)核执行期无源码/工具差分；kernel7773d62e…、Init01451872…、实际新包04867282…、脚本9e3fec8b…、工具07e7a74a…；test_target源码8f28da60…/debug b9e0c0bd…/实际条目a38c0c60…，BootPackage Init/payload对应当前实际ELF/initfs。just/QEMU/GDB/cpulimit/helper及本次私有run文件零残留，原证据保留。当前仅报告该准确工具缺口/阶段，无新增产品状态、无第六轮或其他专项；三组待证与独立时钟异常维持。

**MMIO响应参数读取与六停点location修正（第五轮后，仅离线）。** 同hash kernel7773d62e…的旧respond_ok入口357858/respond_error入口357b08尚未执行prologue，而DWARF frame分别指向本函数sp+16/sp+32，故入口直接read_var(frame)的顺序明确错误。第五轮原坏指针未存，不能据此回推它一定触发了该次异常。本次每次按实际ELF的源码840/846重新定位，生成时核DWARF栈槽与实际a0参数store先于选点、此前无frame业务写；本ELF新点357862/357b14，参数store分别35785c/357b10，均在sepc修改前，不跨重编硬编码地址，也不从a0猜执行身份。原parent_frame/sepc匹配和direct-map拒绝门保持。

[六点与祖先参数的初始化证据](../artifacts/device-closing/mmio-response-offline/location-proof.json)区分词法scope、实际location与初始化指令：

| 当前停点 | 实际读取位置与初始化依据 |
|---|---|
| Revoke 2be2b6 | self=sp+136，2be2b4参数store已执行；祖先object::revoke的thread=其sp+224，2401a4保存，调用Node::revoke前已初始化 |
| Kill封口 24f178 | self=sp+64，24f170保存；祖先process::kill的caller=其sp+264，2f18dc保存，freeze_and_seal调用前已初始化 |
| Queued 224578 | self/execution指针=sp+3680/3688；prologue先构造a2=sp+2648，再于22456a/56e保存到a2+1032/+1040，均早于停点 |
| Publishing 22576a | self=sp+152已存；Execution值=sp+176/1128B，Option<Execution>::expect以该输出地址返回初始化值，再将相同局部传给advance，停点在其返回/下一Queue锁前 |
| respond_ok 357862 | frame=sp+16，35785c已保存；此点尚未执行第一条frame业务写，sepc未变 |
| respond_error 357b14 | frame=sp+32，357b10已保存；转换调用之前，sepc未变 |

上述绝对数值只绑定本轮实际ELF，保留[原始scope/祖先机器码](../artifacts/device-closing/mmio-response-offline/six-locations-and-ancestors.log)与[响应前后机器码](../artifacts/device-closing/mmio-response-location-before-fix.log)；不把scope存在本身当值可读。`partial`在vCont回复匹配/切换真实CPU、flush register cache后显式invalidate_cached_frames，外层再新取frame；原CPU/Thread/PID/mapping与锁外窗口门未弱化。

本脚本每次停点先落PC、QEMU hart/GDB线程、函数/阶段和当前mode；具体observed_word在调用原共享word前落实际address、来源变量、原GDB frame/Thread或UserContext地址，拒绝/读取错误还标出原异常。主观察段捕获一般异常后先保存last_stop/最近word与bt（若bt不可取则明确诊断缺失），再原样raise，不旁路direct-map检查，不变更共享word/physical_word或产品。只保留当前停点/读取诊断，不加无限日志账本。

[离线检查](../artifacts/device-closing/mmio-response-offline/validation.json)exit0：完整生成AST、六点/祖先location初始化、共享word/physical_word AST不变、vCont后清帧与外层异常重抛均核。仅以明确标注的离线零地址哨兵触发原word无访存拒绝，验证地址/来源/原错误会先落盘（[诊断输出](../artifacts/device-closing/mmio-response-offline/diagnostic-audit.log)），不作为客体坏指针样本。工具b2aa368a…；未连接/启动QEMU，无新runtime值或第六轮，两个Forced/Frozen窗口仍未证，不改第五轮整体失败或三组/独立时钟边界。

**MMIO第六轮主动收尾与审定修正。** `THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`最终143：[入口](../artifacts/run/20261026-212521-87667-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-212525-88049.log)、[收割前完整快照](../artifacts/device-closing/mmio-forced-sixth-before-harvest.json)、[本次身份核实/TERM记录](../artifacts/device-closing/mmio-forced-sixth-tool-harvest.json)、[最终原证据](../artifacts/mmio-forced/20261026T132528.527254Z-88063/evidence.json)、[原GDB](../artifacts/mmio-forced/20261026T132528.527254Z-88063/forced.log)。helper PID88063/PGID87645且实际check-device.py --mmio-forced参数一致，才发TERM并交原finally收专有组；本次未触1120s路线界。clock wrapper harvest=null与这次明确tool-harvest分开，143不是自动加时依据。

原Revoke响应已在新点实际读到并保存revoke_returned=true，原PID26/scope两mapping5/6及VM18446743800990154896保持；随后原Kill入口的frame与parent_frame同为18446743800988149808，sepc同为1374282。此时父停在Kill入口等合格Work窗口，worker尚未持到，windows空。原工具响应分支无“尚未Revoke完成”的限制，会把将来的同frame/sepc Kill应答先认成Revoke并continue；这是按已保存标量和源码证明的独立工具识别错误，但本轮尚未执行该Kill响应，不能把它当未持到候选的实际原因。修正只用现有revoke_returned/kill_frame阶段事实，不新增镜像状态或改共享身份/错误门。

原GDB共42487条vCont-stop、多为全局advance224578，未保存每次是否原VM/实际phase/active/FIFO拒绝。不能由数量认定产品循环、已经推进或没有合格窗口；raw maintenance vCont在当前Work断点仍启用时再次恢复的工具跨越风险须独立离线核。收割前failure=null；原最终JSON的Remote communication error/Connection reset及No stack发生于本次QEMU收尾后，属于尾部，不能覆盖原未形成窗事实或伪造指针错误。

[第六轮最终944项/身份与独立清理](../artifacts/device-closing/mmio-forced-sixth-final-version.json)核执行期无差分，kernel7773d62e…/Init01451872…、实际包f92d9343…、脚本32d5b9ee…、工具b2aa368a…；test_target b9e0c0bd…→条目a38c0c60…与包payload对应。原全局进程/本次私有文件零残留，样本和尾部均保留，完整CPU/scope/退款与两个窗口仍未证。

**本轮审定：恢复原292/294最小合同。** 非空未开始责任交接与已提交事务完整结束是两份原合同，不要求原两Map在同一时点均在一个FIFO。上方窗口表已同步：Queued核Idle/activeNone及非空原mapping子集；Publishing核原active已提交，不强求另一项同时queued。两Map消费者继续保留；没有出现在该快照的Map仍必须由最终原scope/VM DONE、进程/Control与账闭合，不能丢责任。双项积压只作额外事实，不作为必过门。本次只修具体工具/方案并做离线检查，无第七轮，不改许可/PTE/产品或时钟。

**raw恢复的有限离线修正。** 固定GDB18.1的`cli_packet_command→send_remote_packet`只put/get，不经过正常infrun的断点跨越；固定QEMU11.1.2的`gdb_continue_partial`中c只resume、s启单步，`check_for_breakpoints_slow`明确单步覆盖BP_GDB，否则当前PC命中即再停。亲读原函数及固定链接见[本次复核](../artifacts/device-closing/mmio-sixth-offline/review.md)与[固定源码记录](../artifacts/device-closing/mmio-vcont-source/SOURCE-EVIDENCE.md)。这解释原恢复方式的确定风险，不由它补回第六轮缺失的VM/Phase/FIFO，也不宣称原42487次均是同一原因。源码核查到此停止，不扩调试框架。

脚本只对刚处理且本次原准许运行集合内的当前CPU执行一条已核指令，其余CPU保持暂停；原held父/Work仍按现有阶段排除。生成器按实际ELF核六点，Queued只准原栈Execution的sb a1,1031(a2)，respond_ok只准原frame的sd a2,80(a0)，其他四点只准auipc ra；未知形态、WFI、锁原子/等待、调用/分支及其他store直接拒绝。单步后必须SIGTRAP、原CPU及PC=反汇编派生next，任一不符先保存包/原值后停止，不发continue；成立后才恢复原CPU集合并重新清cache/取frame。没有手改PC/寄存器/内存/时钟/单步flags，没有提前跨未观察CPU。

原不合格候选下一次允许恢复时会经上述一条原指令跨越，或明确止损；每模式最多128次Work检查只限制宿主取样成本，不是内核时间/推进保证。每原因保存首份实际Phase/active/FIFO及数量，不同VM只留身份而不遍历其执行，去掉逐包输出；最终两Map全部责任仍由原消费者的scope DONE、VM/CPU退休、Control/Pool断言闭合。

[完整生成](../artifacts/device-closing/mmio-sixth-offline/generate.log)与[离线验证](../artifacts/device-closing/mmio-sixth-offline/validation.json)均exit0：同hash7773d62e…六点/祖先初始化证据复用，真实第六轮同frame/sepc的阶段反例、非空原子集、共享word/physical_word AST不变、单CPU原指令跨越/held排除/CPU或下一PC不符即停止/清帧均核。后四项属于明确标注的控制替身，不是QEMU运行证明。当前工具67590250…；新脚本/源码最终身份以[离线版本清单](../artifacts/device-closing/mmio-sixth-offline/final-version.json)为准。没有新QEMU/远端执行，两个真实Frozen窗仍未证，不重跑聚合或其他门；不改第六轮143及旧0x56精确因果未知的事实。

下一单项仅`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`第七轮，事前944项/实际产物与离线验证固定在`artifacts/device-closing/version-before-mmio-forced-seventh.json`；仅验证上述真实跨越及两个最小合同窗口、原两Map完整收束。运行期源码/工具/产物冻结，原地址从实际ELF生成而非沿旧常量；若CPU/指令不符或128次Work宿主采样耗尽，保存实际首拒绝/原值，记工具未取成窗，不作产品性能/失活结论，不自动第八轮，不串IPI/idle或已过路线。

`mmio-forced-seventh.exit=1`：[入口](../artifacts/run/20261026-222525-7373-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-222529-7759.log)、[原GDB](../artifacts/mmio-forced/20261026T142533.186525Z-7770/forced.log)、[原证据](../artifacts/mmio-forced/20261026T142533.186525Z-7770/evidence.json)、[运行状态](../artifacts/device-closing/mmio-forced-seventh-state.json)。GDB按原Device admission后attach，六点已设置；首个last_packet只有c:p1.1/2/3/4，没有response，原gdb.error为error while fetching packet from remote target，随后check-device以MMIO evidence incomplete退出。selection/pending/last_stop/word/step_over/candidate_samples均未生成，windows空；没有执行单步、没有候选拒绝、没有达到128或1120s，无clock终局/本次人工收割，不能据此推产品失活或指令跨越失败。错误处理中保存的bt仅是GDB当时可返回的栈，不当作实际目标owner或本轮停止PC证明。

原普通全CPU恢复也改走maintenance put/get，当前失败就发生在这一初始路径；它与原infrun的正常运行等待不同是静态事实，但本轮没有具体remote读错误/超时的原返回值，暂不指定原因或增加预算。最小后续离线核验是该固定GDB的raw包等待停止回复/返回错误路径，不现场改代码或开第八轮，不以本轮给单步/原两个Frozen窗口补证。

[本轮最终944项与独立清理](../artifacts/device-closing/mmio-forced-seventh-final-version.json)核执行期无源码/工具差分，tool67590250…、实际脚本dc41d456…、kernel7773d62e…、Init01451872…；本次实际BootPackage34b3eca5…的Init/payload与当前构建逐字匹配。test_target源码8f28da60…/debug ELF b9e0c0bd…，initfs实际strip条目a38c0c60…与事前一致，未把旧包身份混作本轮。QEMU/GDB/just/节流器/对应helper及私有run目录均零残留。首批仍三组待证，时钟未知另列，原已过门不变；一轮后停项报告，没有新已证产品缺陷。

第七轮后的局部修正：普通无held CPU分支误改成partial(全部CPU)是已确认的工具范围回归，现仅恢复原gdb.execute('continue')。初始、Revoke后无held、Frozen解除持有后的运行都回到正常infrun；只有确实排除父/Work的三种原分支仍用partial/已核单步。没有改remotetimeout、产品或原错误/身份/候选门；该范围回归不等于已经确认第七轮具体远程读失败原因。所有恢复后仍沿外层清frame cache并新取frame，不跨停点沿用动态状态。

[本次生成](../artifacts/device-closing/mmio-seventh-offline/generate.log)、[离线分支/控制检查](../artifacts/device-closing/mmio-seventh-offline/validation.json)exit0：六点/共享word/原frame碰撞门不变，控制替身分别核initial/no-held-after-revoke/frozen-release走normal continue，held-parent/held-work/both-held走原允许的partial集合，以及后续清frame/新读。工具17b2347b…、离线脚本3ecb27ce…，不把该检查当实际远端跨越。新版及既有产物固定后仅执行第八轮`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`，记录在`version-before-mmio-forced-eighth.json`；若失败保存实际包/阶段/原前置与候选，清理后报告，不现场第九轮或串其他项。

`mmio-forced-eighth.exit=1`：[入口](../artifacts/run/20261026-223303-9079-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261026-223307-9485.log)、[原GDB](../artifacts/mmio-forced/20261026T143311.111974Z-9496/forced.log)、[完整原值](../artifacts/mmio-forced/20261026T143311.111974Z-9496/evidence.json)、[运行状态](../artifacts/device-closing/mmio-forced-eighth-state.json)。普通continue真实越过前次首包失败并到目标。pid26/scope30064771098、同VM18446743800987519408/satp9223372036855301933、mapping5/6及原Revoke响应关联；客体405准入、407真实Revoke返回。实际last_step_over为原respond_ok357862的sd a2,80(a0)，同p1.4停止、after357864、step_over_counts=1，代码内SIGTRAP/同CPU/next门通过；成功包正文随后被last_packet覆盖，只有这份实际stopped/after/计数保留，不声称另存了其完整RSP回复。

Queued第一次实际候选已持到：Execution18446744072636646336在原Work栈，工具原phase=Idle/activeNone门通过，FIFO只有原mapping5，原scope两Map仍为5/6；self.execution母槽为空的唯一持有门亦通过。原phase/active字节完整保存在pending，candidate_samples=1，无拒绝；这证明最小非空子集前置成立，不要求mapping6同时queued，也不证明其最终收束。随后真实Kill入口在hart3，parent_frame==kill_frame18446743800988149808、sepc同1374282；本次最后raw包只有c:p1.4、尚无response，即error while fetching packet from remote target。没有Kill成功返回/Frozen/Work放行后接管，windows空；Publishing及原scope/进程/Pool最终收束均未到，不能用本轮局部单步/候选关闭两窗口。

该错误不是128次宿主取样/1120s路线撞限，无本轮人工收割或clock终局。bt是取包失败时GDB可返回的栈，不能据它认定运行中的父仍停在同一PC、持锁等待或产品失活；具体远端读/恢复原因仍未定位。按一次安排停项保存，不扩大期限、追加探针/产品改动或自动第九轮/其他路线。

[最终944项/产物与独立清理](../artifacts/device-closing/mmio-forced-eighth-final-version.json)核执行期零源码/工具差分，tool17b2347b…、实际脚本e585463b…、kernel7773d62e…/Init01451872…、本轮包9c39f95a…；Init/payload逐字匹配。test_target源码8f28da60…、debug ELF b9e0c0bd…、实际strip条目a38c0c60…保持原身份。QEMU/GDB/just/节流器/helper及私有run目录均零残留。三组待证与独立时钟未知不变；无新已证产品缺陷，原第七轮失败仍保留。

**第八轮后的有限离线复核（无第九轮）。** `freeze_and_seal→freeze/finish_freeze→close_entry/seal_exit/notify_slots→publish_reclaim`亲读涉及原Shell、VM/forced、退休和回收的共享锁/通知；暂停其他非held hart不是原交错要求。工具已只删parent-only恢复分支：Work被持住且未Frozen时，全CPU减原Work hart（含父和其余非held hart）；父暂持住但Work未到时仍全CPU减父；冻结后/无held回原正常continue。两对象同hart的独立推进拒绝、真正held排除、原身份与Frozen/完整事务/最终scope两Map收束判据均不变。此修正不证明第八轮真实持锁死锁，不更改产品。

固定GDB gdb-18.1-release的`send_remote_packet`12681–86调用默认forever=false/is_notif=null的getpkt，返回负数才抛本次字符串；getpkt10835–10914使用remote_timeout读起包，最多三次超时/损坏包尝试后-1，read_frame还可因中途/校验问题返回-1。正常wait_as9084–9098在blocking/等待配置允许时走forever，起包用watchdog或-1，包内字节仍有remote_timeout。按原-nx/连接设置对当前已安装GDB18.1离线实读remotetimeout=2、watchdog=0、non-stop=false、tcp connect-timeout=10，原脚本没有改前两项；这些是宿主协议读取/连接边界，不是产品期限，也不推出原第八轮实际超时或精确耗时。旧样本无底层read_frame分类/逐包elapsed，不能在超时与损坏包等原因中指定其一。未扩大remotetimeout、watchdog或内核期限。原源码片段/配置/链接见[有限复核](../artifacts/device-closing/mmio-eighth-offline/review.md)、[原等待代码](../artifacts/device-closing/mmio-eighth-offline/remote-wait-source.log)、[实际离线配置](../artifacts/device-closing/mmio-eighth-offline/gdb-configuration.log)。

每份实际raw s/c发出前追加packets（请求、序号、此前确认的stop），之后保存成功原response或原异常及host_elapsed_seconds；与原Work观测共用128宿主上限，记录已满则在下一包发出前止损，不覆盖旧成功原文，不作产品失活判据。last_packet保留原入口而不再独占历史。每次恢复先撤销“当前stop”事实，旧现场只在packet.origin_stop保留；raw真实SIGTRAP/CPU/PC确认后才保存新stop，普通continue仍清frame新读。未确认最新halt时failure只标现场缺失，不调用旧bt，最后word标为历史；错误原样抛出，不因诊断替换。elapsed含该次raw恢复调用/确认，不作性能或锁等待证明，无重试/新运行体或通用框架。

[完整生成](../artifacts/device-closing/mmio-eighth-offline/generate.log)与[离线检查](../artifacts/device-closing/mmio-eighth-offline/validation.json)exit0：四hart控制反例核持Work恢复父+其他两hart、持父排除父、普通/冻结后走normal；原六点/身份/最小条件/共享word AST及同CPU下一PC门保留。生成函数控制替身另核成功s和失败c原文同时保留、原异常/elapsed、未halt不读旧bt、128已满前拒绝下一包且不覆盖。当前tool0c53b921…、脚本044c316b…，本次没有实际runtime值或远端连接，原第八轮一条跨越证据另存不混。最小未证仍为新恢复集合/逐包记录实际可用→Kill成功/Frozen→两个最小窗口完整交接及原scope两Map/进程/Pool收束；方案报回后再定单项，不自动开跑。

[本次离线最终版本/清理](../artifacts/device-closing/mmio-eighth-offline/final-version.json)与第八轮结束944项比较仅具体工具和本计划变化；kernel/Init/test_target及实际包9c39f95a…均未变，无新QEMU/远端执行、相关owner/私有文件零残留，diffcheck0。新恢复集合/逐包保存/未确认halt诊断均只有离线证据；保留第七/八轮的失败总结果及第八轮真实一条跨越/Queued候选，不给两个Frozen窗或最终退款补证。

下一单项仅第九轮`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`，事前版本`artifacts/device-closing/version-before-mmio-forced-ninth.json`。按实际包/CPU集合、原Kill/Frozen、两个窗口及原两Map最终账分别判断；新的宿主elapsed/应答/原错误不自动等价于产品期限或锁活性结论。运行期冻结，一次后核清理/产物报告，不第十轮、不串IPI/idle或已过项。

`mmio-forced-ninth.exit=1`：[入口](../artifacts/run/20261027-081757-59283-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261027-081801-59670.log)、[完整原包/参数/指纹](../artifacts/mmio-forced/20261027T001804.849295Z-59682/evidence.json)、[原GDB](../artifacts/mmio-forced/20261027T001804.849295Z-59682/forced.log)、[运行状态](../artifacts/device-closing/mmio-forced-ninth-state.json)。本次GDB实际记录18.1/remotetimeout2/watchdog0/non-stop=false/connect10。四包均收到原T05并确认halt，response与host_elapsed_seconds完整保留：包1/3/4的c集合为p1.1/.2/.4（只排除原held Work p1.3/hart2，其他非held被允许运行）；包2为原s:p1.4，357862→357864同CPU。三个c分别停在父p1.4的respond_ok、Kill入口24f178、respond_ok；不是parent-only集合，也未再取包报错，不能据本轮成功回包反推旧包失败具体原因。

原pid26/scope30064771098、mapping5/6/同VM、Revoke成功；Queued首份实际样本Idle/activeNone、FIFO[5]，原栈Execution唯一持有门通过，原两Map不要求同时queued。包4从原Kill入口恢复，在respond_ok357862确认halt（elapsed约0.00094025s只作宿主事实）后，GDB读取的frame参数为0x17，observed_word先保存来源respond frame.sepc及地址0x117，再由共享word原direct-map门拒绝expected direct-mapped kernel pointer。对应当前已确认停点的GDB展开返回respond_ok(frame=0x17,...)，祖先dispatch.frame显示0xffffffc08089a830；这些是GDB本次读值/展开事实，缺本次SP/实际store槽等独立证据，不认定内核传入坏frame或指定DWARF/缓存原因。没有旁路字段、用寄存器/祖先猜身份或放宽地址门。

失败发生在核响应frame/sepc之前，不能凭父CPU到respond_ok宣称原Kill响应已认证，更未读取frozen_read；windows空。Publishing、Work放行后的Frozen责任交接、scope/CPU/Control/Pool完整收束未到，最终仍未通过。candidate_samples=1、packet_count=4，非128或1120s撞限，无clock终局/人工收割，不调期限、不自动重试或第十轮，不串其他路线。四包详细原文与确认halt在原JSON保持，不再用last_packet覆盖唯一成功证据。

[最终944项/实际身份/独立清理](../artifacts/device-closing/mmio-forced-ninth-final-version.json)核执行期零差分，tool0c53b921…、实际脚本e2f73ca4…、kernel7773d62e…/Init01451872…、本轮包dc7b051c…；Init/payload逐字匹配。test_target源码8f28da60…/debug b9e0c0bd…/实际条目a38c0c60…一致，QEMU/GDB/just/节流器/helper及私有run目录零残留。原第七/八轮整体失败与其局部证据保留，当前三组待证及独立时钟未知不变；没有新已证产品缺陷。

**第九轮后的响应快照离线方案（无第十轮）。** 同hash7773d62e…的原机器码与DWARF分别核：ok357862前已存frame到sp+16、ret到sp+24，prologue32，停点a0仍是frame；error357b14前已存frame到sp+32、err到sp+24，prologue48，357b12已改a0=sp+24，因此不能把error点a0当frame。两者DW_AT_frame_base均DW_OP_reg2，frame位置分别fbreg+16/+32，GNU RISC-V CFI在两点分别CFA=sp+32/48（error的ra=CFA-8）。系统dwarfdump有MCRegInfo警告，保留原警告并只采其明确数值表达式，与GDB声明/scope和机器码/CFI交叉，不声称它完整识别了目标架构。证据：[静态证明](../artifacts/device-closing/mmio-ninth-offline/static-proof.json)、[两点机器码/作用域](../artifacts/device-closing/mmio-ninth-offline/generated/code-windows.log)、[CFI](../artifacts/device-closing/mmio-ninth-offline/response-cfi.log)、[声明](../artifacts/device-closing/mmio-ninth-offline/declarations.log)。

固定QEMU v11.1.2的core第一组base_reg=0，RV64 XML按序给sp=2/a0=10/a1=11/pc=32；Hg设置g_cpu，p只读取该CPU gpr/pc并按目标字节序返回。代码只采用函数级事实，不采用辅助报告的旧错因推断。新诊断先保存GDB所选CPU/pc、newest/selected Frame状态、表达式寄存器/Frame.read_register、实际frame Symbol.value/原read_var与dispatch祖先；这些都明确标为GDB/展开层值。随后显式Hg同CPU，有界取实际target.xml/core XML核RV64第一feature和真实编号，再以p2/pa/pb/p20取原sp/a0/a1/pc，保存原应答和LE数值；不是凭GDB别名猜编号。raw pc必须是当前响应点，验证虚拟调试模式后，以raw SP读本点已核的32/48字节原prologue范围，保存frame/ret或err槽字节与解释。记录查询前后read_var差异但不据此改变原门。来源/固定链接与完整步骤见[有限复核](../artifacts/device-closing/mmio-ninth-offline/review.md)及[QEMU固定源码](../artifacts/device-closing/mmio-response-source/SOURCE-EVIDENCE.md)。

快照仅原两个mode、revoke/kill阶段，dispatch.thread须等于已选父Thread；target_pid来自此前真实目标记录。每mode/phase最多一份，共最多四份，未知/重复阶段拒绝，不全局堆日志。GDB诊断字段不可用会明确记录并继续收独立raw对照，不能转成身份/成功证据；原native_frame=int(frame.read_var('frame'))及原word/direct-map/sepc核验仍在快照之后原样执行，不使用a0/原始栈槽或祖先frame作fallback。Hg仅调试器CPU选择并保留原应答，p/m为有界只读诊断；不写guest寄存器/PC/SATP/内存/PTE/调度/时钟，不修改共享word。所有Frame/Symbol/值在当次停点转普通数值/字符串，不跨停点缓存运行态Type。

[完整生成](../artifacts/device-closing/mmio-ninth-offline/generate.log)、[原控制/生成检查](../artifacts/device-closing/mmio-ninth-offline/validation.json)与[响应诊断检查](../artifacts/device-closing/mmio-ninth-offline/snapshot-validation.json)均exit0：两布局不同/a0角色、Frame声明/原地址门不变、诊断在word前、原目标/阶段限定、XML编号/CLI应答解析、未知XML拒绝、原查询错误保留均核。解析/控制替身和本地ELF检查不是实际qXfer/p/m/Frame API值；tool22a7398c…、脚本99033cdd…。第九轮缺失的原SP/参数槽/独立寄存器不可恢复；只有后续实际CPU/PC及raw寄存器/已核槽独立一致时，才有依据进一步判断GDB/DWARF层差异，当前不指定根因或称已修。最小未证是新快照实际收全/对照可靠，再回原Kill身份/Frozen、两个完整窗口与原两Map最终收束。当前只报告离线结果，不开第十轮/其他路线。

[本次离线最终版本/清理](../artifacts/device-closing/mmio-ninth-offline/final-version.json)与第九轮结束944项比较仅具体工具/本计划变化；kernel/Init/test_target及实际包dc7b051c…未变，无新QEMU/远端执行、相关owner及私有文件零残留，diffcheck0。原始运行和新离线证据分开保存，不清第九轮失败、不称诊断或DWARF问题已运行解决。

下一单项仅第十轮`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`，事前版本`artifacts/device-closing/version-before-mmio-forced-tenth.json`。目标优先收原最多四份真实响应的raw CPU/PC/寄存器/栈槽与GDB字段；有差异只记录，原严格门仍决定能否继续，不现场fallback。若快照完整且原门自然通过，再核原两窗及两Map最终收束；运行期冻结，不额外改读取/恢复，结束清理/版本核实后报告，不第十一轮或串项。

`mmio-forced-tenth.exit=1`：[入口](../artifacts/run/20261027-085810-66973-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261027-085813-67351.log)、[两份完整响应/原包/指纹](../artifacts/mmio-forced/20261027T005817.326487Z-67362/evidence.json)、[原GDB](../artifacts/mmio-forced/20261027T005817.326487Z-67362/forced.log)、[运行状态](../artifacts/device-closing/mmio-forced-tenth-state.json)。只到Queued的revoke/kill两阶段（最多四份中的两份；Publishing未到），两份均收全：编号由首份实际target/core XML核验后共用，Hg、p2/pa/pb/p20、虚拟模式确认、原32字节m栈读全部有原应答，无诊断缺失/查询错误；实际raw恢复4包亦全部T05。当前响应同p1.3/PC357862，held Work为p1.4（hart3），三个c集合为p1.1/.2/.3，仍只排除真正held Work。

| 本轮真实响应 | raw SP（GDB表达式与Frame.read_register同值） | raw a0 / raw SP+16槽 / dispatch祖先frame | Symbol.value及Frame.read_var（raw查询前/后） |
|---|---|---|---|
| Revoke | 0xffffffffc00cca70 | 均0xffffffc08089a830 | 均0xffffffc08089a830 |
| Kill | 0xffffffffc00ccab0 | 均0xffffffc08089a830，实际槽0xffffffffc00ccac0 | 均0x17 |

两份newest/selected Frame均level0/type0/valid=true/unwind_stop_reason0，Symbol所属函数/objfile均是当前kernel/respond_ok；raw a1与其参数槽均0。第十轮因此提供了此前缺失的独立一致值：该停点实际a0及已执行参数store的原槽内容是合法frame，0x17不是它们的当前值；GDB变量取值路径与这些原始事实不一致已确证。但具体frame-base求值、目标内存读取/缓存等子机制尚未定位，不能写成已修GDB或确认内核传坏frame，也不由本轮反推第九轮未保存的动态值。

原两Map5/6、Queued Idle/activeNone/FIFO[5]前置及Revoke返回保持。新快照不参与放行：查询后原Frame.read_var仍0x17，随后原observed_word在frame.sepc地址0x117拒绝，原失败保存。没有拿raw a0/slot或祖先frame fallback，没有改读取/恢复/产品或期限。原Kill sepc身份/Frozen未核，两完整窗口、Publishing和scope/CPU/Control/Pool最终收束未到；本轮对照完成不等于整条验收通过。sample1、raw恢复4包，非128/1120s撞限，无clock终局/人工收割；一次后停项，不自动第十一轮或串其他路线。

[最终944项/实际身份/独立清理](../artifacts/device-closing/mmio-forced-tenth-final-version.json)核执行期零差分，tool22a7398c…、实际脚本4612280c…、kernel7773d62e…/Init01451872…、本轮包7b64d2ac…；Init/payload逐字对应。test_target源码8f28da60…/debug b9e0c0bd…/实际条目a38c0c60…保持身份，QEMU/GDB/just/节流器/helper与私有run目录零残留。三组待证与独立时钟未知保持，旧失败与各自证据不清，无新已证产品缺陷。

**第十轮后补齐原GDB缓存生命周期。** 固定gdb-18.1-release的infrun4457–4461/4695–4700/5353–5357在运行事件处理路径调用target_dcache_invalidate；maintenance send_remote_packet12681–83仅put/get，没有这步。本机-nx实际stack-cache/code-cache均on，help与本地ELF命令执行确认maintenance flush dcache清理目标内存缓存（含stack-cache/code-cache），不涉及guest缓存策略。原差异与该缺口一致，但不宣称已还原第十轮具体内部cache事件。原文和命令证据见[固定代码片段](../artifacts/device-closing/mmio-tenth-offline/normal-vs-raw-source.log)、[命令输出](../artifacts/device-closing/mmio-tenth-offline/dcache-command.log)、[来源/边界](../artifacts/device-closing/mmio-tenth-offline/lifecycle-proof.json)。

代码只在raw_resume确认T05并切到实际停止CPU后加入maintenance flush dcache，随后原flush register-cache/invalidate_cached_frames和PC读取保持。新Frame/原变量/目标内存读取都在该步之后；不禁用缓存、不新增读取fallback、不改guest状态、产品、原身份/地址门、恢复集合或期限。原有有限RSP/GDB/栈对照继续用于实际一致性取证。[完整生成](../artifacts/device-closing/mmio-tenth-offline/generate.log)、[AST/控制与顺序检查](../artifacts/device-closing/mmio-tenth-offline/validation.json)、[对照原门检查](../artifacts/device-closing/mmio-tenth-offline/snapshot-validation.json)均0，tool eb632e98…/离线脚本c62cfec3…；只属离线证据，原第十轮exit1不改。

下一仅第十一轮`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`，冻结版本`artifacts/device-closing/version-before-mmio-forced-eleventh.json`，实际核刷新后的对照是否一致；仍按原严格门验证两窗/两Map最终收束。若仍有差异或工具未成窗，保存准确原值/包/阶段并清理后报告，不第十二轮或串其他项，不现场再改读取。

`mmio-forced-eleventh.exit=1`：[入口](../artifacts/run/20261027-091402-72632-acceptance-mmio-forced.log)、[客体](../artifacts/failed-acceptance-20261027-091406-73023.log)、[原响应/前后观测/指纹](../artifacts/mmio-forced/20261027T011409.591234Z-73035/evidence.json)、[原GDB](../artifacts/mmio-forced/20261027T011409.591234Z-73035/forced.log)、[运行状态](../artifacts/device-closing/mmio-forced-eleventh-state.json)。raw确认停机并失效dcache后，Queued revoke/kill两份中Symbol.value、Frame.read_var（查询前/后）、raw a0、raw原frame槽均自然一致为0xffffffc08089a830；原word/sepc身份门通过，没有fallback。第十轮变量差异在这两份新样本未再出现，支持补齐缺失缓存生命周期的实际效果，但不外推旧缓存的每项内部事件或未运行的其他样本。原Kill确认后记录frozen=true、frozen_read.access_open=false、原Held Execution.phase=Idle及原字节，角色/停止身份未放宽。

后续恢复观察在global advance224578/hart0，工具报original VM reopened after Frozen；这一断言消息不直接证明产品重开。原pending仍是Queued pid26/scope30064771098、mapping[5,6]、vm0xffffffc08087e2d0/work0xffffffc08087e830；post_frozen_observation虽地址相同，实际phaseIdle/activeNone/FIFO[7]，不是旧mapping集合。客体409已报告旧Queued pid26 Killed/members0，410其Pool20840 pages0/metadata82，411关闭后两维全0；412已启动新Publishing pid27/scope34359738394/Pool20848，414其Revoke也已返回。探针没有存不可复用VM/Work身份，不能由相同地址把后样本归给旧VM；地址复用/跨轮误配的风险有具体现场，旧原始身份无法事后补回，不猜产品重开或真实锁死。

当轮工具未落下其额外要求的后续frozen_handoff，windows仍空；该项原取样要求在下方审定中收敛，Queued已有精确Frozen窗口与完整消费者收束可组合闭合。Publishing消费者虽已进入，探针仍未切到其post点/原轮，精确Publishing窗口及整轮core/shutdown未验。candidate_samples=2、raw恢复4包且全部T05，非128/1120s撞限，无clock终局或人工收割；整体exit1保留，不自动第十二轮/其他路线。

[最终944项/产物与独立清理](../artifacts/device-closing/mmio-forced-eleventh-final-version.json)核运行期零差分，tool eb632e98…、实际脚本ce60affb…、kernel7773d62e…/Init01451872…、本轮包34b1d6f1…；Init/payload逐字对应。test_target源码8f28da60…/debug b9e0c0bd…/实际条目a38c0c60…保持身份，相关QEMU/GDB/just/节流器/helper与私有run目录零残留。原第十轮exit1与它自己的独立差异证据保留；当前三组待证与独立时钟未知不变，无新已证产品缺陷。
**第十一轮后审定：观察寿命与完成组合。** 原Frozen合同不要求已收束的VM在下一advance仍存活。Queued由原held Idle/activeNone/非空队列跨真实Kill Frozen、同PID/scope最终回执及原forced.rs295–317唯一交接路径闭合；不继续追旧地址，也不倒推第十一轮后样本的真实identity。Publishing仍须取得原active已提交/同phase跨Frozen的真实窗口；原advance先处理非Idle引擎再进Frozen分支，配合消费者最终DONE/账目证明完成，静态不替代该窗口。

本次仅离线修改具体探针：Mapping选择与当前活Work读取均核VmShell现有单调identity（change.rs39/48/133），不加产品身份；原Work持有Execution时核Frozen并保存精确窗口，随后禁旧Work断点、清本地观察引用并恢复下一目标Revoke。不得在旧对象释放后继续解引用其地址，也不得把本地窗口完成当作scope/CPU/Control/Pool已完成；宿主按同PID/scope关联原消费者回执。原弱VM引用没有活强owner时不读data。两Map消费者及原最终责任/账目门保持，不为取证延长对象寿命。生成/静态控制检查仅属离线证据，未启动第十二轮。

离线落实结果：`tools/qemu_mmio_forced_gdb.py`删除post_frozen_observation/committed_progress_after_frozen及旧地址“重开”断言；强根为0、不同VM或同地址异identity只记非目标候选，held目标仍严格核同identity、原phase/active与access_open=false。Weak强根只解释本次全停快照，每次重读，不将strong>0当跨恢复保活；保存的窗口只有标量/原字节。`tools/check-device.py`同时核窗口PID/scope，原Revoke→DONE/Killed→Control关闭后Pool零→最终shutdown的组合门保留。没有产品/消费者/期限改动。

[当前ELF声明布局](../artifacts/device-closing/mmio-eleventh-offline/declarations.json)独立确认VmShell.identity及Weak→ArcInner strong/data；[完整生成](../artifacts/device-closing/mmio-eleventh-offline/generate.log)、[原恢复控制/缓存顺序](../artifacts/device-closing/mmio-eleventh-offline/validation.json)、[响应原门](../artifacts/device-closing/mmio-eleventh-offline/snapshot-validation.json)、[观察寿命反例](../artifacts/device-closing/mmio-eleventh-offline/lifetime-validation.json)均exit0。有限替身核同地址异identity拒绝、strong0不读data/每快照重查、窗口取毕不读旧地址/禁旧点并可选下一Revoke，以及PID/scope必须同时匹配；这些不是实际Publishing运行证据。工具hash10ae7a67…；[本次944项版本与清理](../artifacts/device-closing/mmio-eleventh-offline/final-version.json)记录仅本计划/两宿主工具变化，实际kernel/Init/BootPackage/test_target不变、无新客体/残留。最小未证门仍为Publishing原active已提交跨Frozen窗口与同PID/scope原最终回执组合，需另行安排单项；Queued不重开，不自动第十二轮，IPI/idle及时钟未知保留。

监督审定后安排第十二轮，唯一命令`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-mmio-forced`，执行前固定`artifacts/device-closing/version-before-mmio-forced-twelfth.json`。只补Publishing原active/Execution跨真实Kill Frozen及原同PID/scope最终收束；Queued附带运行不重开已证子门。探针verified只指两窗口，整轮由宿主原DONE/退款/shutdown组合判定。运行期冻结源码/工具/实际产物；若身份/快照/前置不定，保存准确事实并清理报告，不自动第十三轮、不串IPI/idle或其他路线，不改产品/阈值。

`mmio-forced-twelfth.exit=0`：[入口](../artifacts/run/20261027-094525-94568-acceptance-mmio-forced.log)明确profile core passed；[原客体](../artifacts/qemu-acceptance-20261027-094529-94982.log)、[原窗口/包/响应](../artifacts/mmio-forced/20261027T014534.141674Z-94993/evidence.json)、[原GDB](../artifacts/mmio-forced/20261027T014534.141674Z-94993/forced.log)。两模式各sample1、raw恢复6包，无候选拒绝/failure/clock终局/主动收割。Queued PID26/scope30064771098、maps5/6/Idle/activeNone/FIFO[5]，Publishing PID27/scope34359738394、maps7/8/原active7、held Publishing；两者都在Revoke真实成功及原Kill返回后核Frozen/access_open=false，原Execution0xffffffffc00863c0的phase/active字节各自未变。VM0xffffffc080800b50、Work0xffffffc080cba730、Execution及satp0x800000000008072d实际复用，既有identity分别26/27；本轮正确分轮，不反推第十一轮原后样本身份。Weak当前强根分别3/4只解释各全停快照，未视作跨恢复保活。

探针捕获窗口后禁旧点、结束旧地址跟踪并放行；原宿主按mode+PID+scope关联。客体407–412 Queued Pool20557、413–419 Publishing Pool20565，各原Revoke→members0/Killed/scopeDONE、CPU后pages0/metadata82、Control关闭后allocated/delegated两维全零。Publishing窗口verified提示在其最终退休前，后者确由原消费者独立完成，未把提示当退款。427 acceptance domain collected、辅助cap清理及439 Requested Shutdown成立；原forced.rs278–317非Idle先完成引擎、再入Frozen分支/退出租约责任与真实窗口/最终回执组合闭合Publishing，MMIO整组关闭，非逐指令运行覆盖声明。前十一轮失败各自保留，不重跑已过专项/聚合。

[最终944项/本轮产物/独立清理](../artifacts/device-closing/mmio-forced-twelfth-final-version.json)核运行期零差分；tool10ae7a67…/实际脚本b7f63113…、kernel7773d62e…/Init01451872…/本轮BootPackage494d692c…。test_target源码8f28da60…、debug b9e0c0bd…与strip-debug后的initfs实际条目a38c0c60…一致于冻结链，包内Init/payload逐字核验。相关QEMU/GDB/just/节流器/helper和私有run文件零残留。仅本计划回填后停项，不第十三轮/其他路线；唯一收口表现剩全idle/PCI与DMA保留+IPI两组，实际时钟未知独立，未归档/提交。

- **IPI原调用消费**：每次从当前ELF定位并核指令/指纹；本ELF ecall2d48c2后c6/c8真实存error/value，raw观察点2d48ca与注入落点c6不同。registry35f444在send_ipi返回及Result::is_err执行后、分支前；raw点只记录原寄存器/存栈错误并保留call，消费点核同caller SP、原Result::Err(SbiError::Failed)/is_err后才完成关联。未消费call遇下一send即报错，不静默覆盖；其他普通SBI不进入IPI关联。立即读回注入值不算调用者消费，本次无运行证明。
- **IPI资格离线增量**：只在原Injection实际收到IPI extension的ecall处统计固定watch/fid/mask-base/精确registry caller/REQ-STOP/非发送者/request祖先/target/WFI/quantum门；每门只保存第一份拒绝及总计数，首个IPI保留有限当前栈与原寄存器/停止标志，未执行到的门明确未取样，不伪造false。当前kernel7773d62e…离线声明证实发送指令/返回PC-1/PC的registry精确名均为kernel::registry::try_ipi_slots，mask/targets/slot/hartid范围覆盖；运行Frame.name/unwind/实际过滤原因仍不可事后恢复，不据此宣布根因。该脚本由GDB正常continue/Breakpoint.stop恢复，不发执行型raw vCont；physical_word仅切调试器内存模式，不套用MMIO的手工执行缓存缺口。原返回点默认关闭、选中后才启用、一次注入/同call消费/禁止覆盖及当前ELF动态机器码解码保持。离线完成后先报告最小运行门，不立即第五轮或串idle。

资格增量仅改`tools/qemu_dma_ipi_gdb.py`：诊断首栈最多24帧，缺词法的纯诊断帧保存原GDB错误/空scope，不用于替代精确caller；`mask/targets/slot/hartid`读取只添事实或准确不可读原因，不参与新的授权/匹配fallback。每个固定门仅首拒绝留完整栈，重复只计数；原WFI拒绝集合最多留4份，未降低原WFI/quantum门，以上限值仅约束宿主日志成本。首候选/每门快照复制当时gates，避免后续门更新污染原时点。异常先输出原candidate/资格摘要，实际目标缺失仍沿原严格错误终止，不当普通不匹配；返回观察点仍初始关闭，没有启动期全局返回截断。

[声明名/词法范围](../artifacts/device-closing/dma-ipi-qualification-offline/caller-scopes.json)与[变量location](../artifacts/device-closing/dma-ipi-qualification-offline/caller-locations.log)以当前kernel7773d62e…独立核发送指令35f436、unwind返回PC-1=35f439/PC35f43a及消费35f444；registry的mask/targets/slot/hartid各在sp+184/192/200/224，机器码在发送前均已存，Runtime Frame/寄存器不可能离线补回。声明名完全符合当前精确选择字符串，故不改selector，也不把此静态事实写成已找到第四轮过滤原因。[当前机器码/派生点](../artifacts/device-closing/dma-ipi-qualification-offline/points.json)保持原ecall2d48c2/raw2d48ca/consumed35f444/result-slot0，每次仍从实际ELF重新解码；[生成AST](../artifacts/device-closing/dma-ipi-qualification-offline/generation.json)、[资格反例](../artifacts/device-closing/dma-ipi-qualification-offline/validation.json)及Just dry-run均exit0。资格替身只验证逐门首拒绝/重复计数/有限栈、诊断缺scope不变匹配、目标缺失仍严格；原IpiReturn/IpiConsumed/retained/Injection与第四轮实际脚本AST逐项未变，不把离线反例当真实IPI证据。

恢复路径亲核`check-dma-retention.py:48`普通continue及原Breakpoint.stop，不存在MMIO式raw s/c执行旁路；IPI代码只切CPU并flush register-cache、设置原一次注入寄存器和调试器物理读取模式，没有新增跨停点Type/Frame缓存，也不无关追加dcache或改返回判据。最小后续运行仍是一轮原`virt-dma-ipi-retention`：先记录实际IPI候选是否到达与首拒绝，只有原资格成立才沿同一次失败raw→Result/is_err消费→同目标后续原STOPPED广播→同映射/写许可四park保留闭合；未选中按新事实记录，不能借保留单项关闭组合。当前仅离线，未执行第五轮/idle，版本与无残留见[离线收尾](../artifacts/device-closing/dma-ipi-qualification-offline/final-version.json)；MMIO已过不重跑，两组剩余及时钟独立状态不变。

监督审定后仅安排第五轮`THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just virt-dma-ipi-retention`；执行前固定`artifacts/device-closing/version-before-dma-ipi-fifth.json`。本轮实际资格/首拒绝与第四轮未知原因分开记录，合格才核原同call失败返回/消费、同目标后续STOPPED原广播及同暴露映射写许可四park。运行期冻结，未选中/读取不定一轮后保存并清理，不现场改selector/放宽WFI、不自动第六轮或串idle，不重跑MMIO/聚合。

`dma-ipi-fifth.exit=1`：[入口](../artifacts/run/20261027-100934-2031-virt-dma-ipi-retention.log)、[原GDB](../artifacts/dma-ipi-retention/20261027T020938.232656Z-2391/gdb.log)、[原脚本](../artifacts/dma-ipi-retention/20261027T020938.232656Z-2391/retention.gdb)、[客体](../artifacts/dma-ipi-retention/20261027T020938.232656Z-2391/qemu.log)。12–13真实reset拒绝前mapping9/10、各原Backing/range0..2/Idle，随后返回NotSupported；14行Python RuntimeError: Cannot locate block for frame，16行实际Thread1停在sbi_call ecall2d48c2，DWARF显示eid Ipi/fid0/arg0=1/arg1=1。这不是已保存raw寄存器，不能据它补watch/REQ-STOP/registry/request/target/WFI资格结论。首候选打印前纯诊断未完成，无FIRST_CANDIDATE/FIRST_REJECT/ELIGIBILITY/INJECT/RAW_RETURN/RETURN，实际资格原因仍未取到，不借此解释第四轮。

新ipi_snapshot对Frame.block等纯诊断只捕gdb.error，本轮实际异常类别为RuntimeError；外层异常记录又调用同一snapshot，因此不能保证原错/准确帧先落盘。现有日志没有失败帧PC/完整栈，不能确定是哪一个栈帧或原block值，也不把静态判断写成已经复现其内部原因。随后原FINISH报incomplete DMA terminal evidence；没有InitialDONE/四park/独立DMA_RETENTION_PASSED或组合通过，客体虽到原core DMA退休和显式shutdown、NotSupported，不替代调试窗口。不是720s外层到期/业务超时/clock终局或已证产品失败；一轮后停止，未现场修改工具/selector/判据，不第六轮或串idle。

[当轮944项/产物与独立清理](../artifacts/device-closing/dma-ipi-fifth-final-version.json)核运行期零差分、0owner/0私有文件；tool81e4962b…/实际脚本59b99c7c…、kernel7773d62e…/本次core Init384b1d40…/BootPackage8af8fae0…，包内Init/payload逐字对应，test_target源码8f28da60…/debug b9e0c0bd…/strip条目a38c0c60…链保持。QEMU由该次helper正常finally TERM清理，不记成客体硬件安全或IPI成功。后续最小缺口仍是先让纯诊断可靠保存该类异常及原资格，然后才能观察同call返回/消费/广播与保留；现仅记录待办，保持两组待证/时钟另列，不新增产品机制。

第五轮后仅离线修正诊断：首候选与各门首拒绝先持有/打印已得标量，再原位补有限可选栈；name、PC、scope、older与newest查询分别保存gdb.error/RuntimeError原类型/消息，失败scope明确为空，已取得字段不因后续查询失败丢失。实际caller/target/注入门仍严格，纯诊断不可用不作为身份fallback。最外层先设置原failure并输出已有candidate/已存栈，不再次调用snapshot；raw/consume同样先锁定原错，可选bt失败另记而不覆盖。只验证本次RuntimeError反例、必需门错误仍终止、无重复采集、生成/default+IPI AST与原门差分；暂不第六轮/idle，不改产品/期限，不将诊断修复记为资格通过。

该小改已有限闭合：[生成/default+IPI AST](../artifacts/device-closing/dma-ipi-fifth-offline/generation.json)与[诊断反例](../artifacts/device-closing/dma-ipi-fifth-offline/validation.json)均exit0，tool34cb6122…；原raw/consume成功与必需校验体、retained/Injection、资格条件及动态机器码定位未变，仅可选诊断和失败保存顺序改变。反例保存本次RuntimeError无scope、name/PC/部分栈、原标量先输出、必需caller/寄存器错误仍停止、外层不重复采栈、bt失败不盖原错，均为离线替身而非真实IPI资格证据。修正版未运行，第五轮exit1/第四轮原因未知保持；按用户交接决定不再接续运行，源码/工具最终身份见审查交接快照。
- **MMIO/IRQ**：只对原Revoke/Kill/advance/step、IRQ Query/ProcessKill离线核函数和变量scope，工具未追加产品闸门/新消费者；未到目标的共同Copy失败不能验证0x56修正。all-idle/更强公平保持原状态。

[DWARF原记录](../artifacts/device-closing/offline-fixes/dwarf-scopes.json)、[九项scope约束](../artifacts/device-closing/offline-fixes/scope-contracts.json)、[IPI实际指令](../artifacts/device-closing/offline-fixes/ipi-observation-code.log)/[派生观察点与hash](../artifacts/device-closing/offline-fixes/ipi-observation-points.json)、[Result类型](../artifacts/device-closing/offline-fixes/ipi-result-type.log)均已保存。三个Python模块AST、三个Root模式/Retry完整生成脚本及DMA/IPI setup/finish AST通过；[生成核验](../artifacts/device-closing/offline-fixes/generate-checks.log)明确未执行目标脚本。受影响user-resources分面[Clippy](../artifacts/device-closing/offline-fixes/copy-target-clippy.log)exit0、[fmt](../artifacts/device-closing/offline-fixes/copy-fmt.log)exit0；不重复已过聚合。

后续需要的最小运行门仍是：Root只跑dma-retained特定失败；Retry只核原并发Caller/迟到提交及同记录退休；DMA+IPI只核选中raw返回/消费与后续广播及原映射保留；IRQ和MMIO分别进入其两个原退出/Forced窗口，并顺带核共同20s前置，不能用正常core替代。all-idle/强公平另行集中，不自动启动新一批。当前没有新已证产品缺陷，首批闭包继续开放。

随后单项复核安排：仅`just virt-dma-ipi-retention`，验证上述raw/Result消费关联与原真实目标、后续广播、同映射保留/四park；Root/Retry/IRQ/MMIO及聚合不启动。若实际字段读取/enum解码/关联仍失败，保存原call/错误与真实栈后结束该项，不连续重跑。运行使用本次实际构建产物（含Copy20s），每次从该ELF重新解码观察点并记录hash；旧缓存只作离线来源对照。执行前版本固定在`artifacts/device-closing/version-before-dma-ipi-fourth.json`，结果随后回填，不预记通过。

`dma-ipi-fourth.exit=1`：[入口](../artifacts/run/20261026-155356-9534-virt-dma-ipi-retention.log)、[原GDB](../artifacts/dma-ipi-retention/20261026T075401.594995Z-9921/gdb.log)、[客体](../artifacts/dma-ipi-retention/20261026T075401.594995Z-9921/qemu.log)、[实际指纹](../artifacts/dma-ipi-retention/20261026T075401.594995Z-9921/evidence.json)、[执行/清理](../artifacts/device-closing/dma-ipi-fourth-state.json)。本轮实际kernel7773d62e…、boot9ca72dda…、scriptba314b23…，观察点从该ELF重新核定为2d48ca/35f444；工具setup已执行，但没有INJECT/RAW_RETURN/RETURN，终止于`no eligible original request-broadcast IPI target`。`rejected=[]`只表明没有进入现有WFI/quantum拒绝记录，早期资格过滤没有逐段落盘；不能据此断言系统未发送IPI，不能把原因猜成caller帧识别或STOPPED竞争。实际raw变量读取、enum解码及消费关联仍未到，不关闭组合门，不现场继续改/跑。

原mapping9/10、各自同Backing/range0..2/Idle在reset拒绝前、initDONE、四park三次一致（GDB12/14/16），只有单独DMA_RETENTION_PASSED成立，不把它替代组合，也不声明DMA静止。实际Copy20s本轮已采用：健康任务93.948753→113.948753s绝对预算，97.199147s结果Complete，23040字节local/transported/confirmed一致且两端Completed/Ok；同步ReadAt95.496733→95.556814s。此为本轮前置成功事实，不外推IRQ/MMIO目标或时限上界。原943项执行期间无差分、无项目QEMU/GDB残留，见[结束版本/清理](../artifacts/device-closing/dma-ipi-fourth-final-version.json)。停止此单项并汇报，其他路线未启动，当前六组剩余门不变。

DMA+IPI后续只登记离线待核：如再补候选诊断，须有限记录watch/fid/mask、实际caller、REQ/STOP及目标状态各资格门的原因/数量，有明确候选上界；不增加断点或长运行猜原因，不让无关SBI主导。当前未实施这项诊断，不开第五轮。

下一单项仅`just virt-platform-funding dma-retained`，不重跑已过platform-node/install；执行前版本为`artifacts/device-closing/version-before-root-retained-third.json`。沿当前ELF生成，特定分配只注一次，核失败后原Root reclaim/Queued与四park；若祖先/分配/回退观察仍不定，保留原候选栈/指纹并清理后报告，不自动串其他项。

`root-retained-third.exit=1`：[入口](../artifacts/run/20261026-160037-10609-virt-platform-funding.log)、[原GDB](../artifacts/boot-failure/20261026T080040.984832Z-10984/platform-dma-retained-gdb.log)、[完整候选/指纹](../artifacts/boot-failure/20261026T080040.984832Z-10984/platform-dma-retained-evidence.json)、[执行/清理](../artifacts/device-closing/root-retained-third-state.json)。真实Node5/root0xffffffc08087d4f0/Pool1、cap_count10、metadata64061；唯一候选栈包含原funded_storage::OwnedBox::try_new、ordered_table::PreparedEntry、DMA entry、Node::root2742d2及bootstrap。Root真实返回PC27f988触发止损，未形成allocation/injected字段，未执行指定失败，也未证最后Node Drop/特定Root Work。候选选择的scope或Layout具体拒绝条件未落盘，信息不足不作归因；不能把未注入当产品回退失败。新kernel7773d62e…/boot6f9a2611…已固定，GDB日志14行即退出，未扩散捕后续全局分配。清理0owner，代码/工具未改，不连续重跑或串其他项。剩余六组保持原证据边界。

Root另有离线待办（未落地）：按已保存OwnedBox返回PC255d6e与pc-1核实际定义scope、原运行脚本与当前生成脚本一致性及所有过滤分支。已知同hash静态Node类型104B/align8、payer为MetadataFunding指针，不能据此恢复旧a0/a1/运行frame.block。待补最多4份候选的实际pc/scope匹配、a0/a1、expected布局、payer匹配阶段及唯一拒绝理由，不能只存栈后静默continue；不新增产品分配/失败接口。当前仅保存了返回点机器码，未实现上述诊断，不记已修、不起第四轮。

下一单项仅`just acceptance-irq-exit`：共同Copy20s已有本轮实际全量Complete前置，原IRQ两读取点静态scope已核；执行前版本固定到`artifacts/device-closing/version-before-irq-exit-third.json`。工具只补失败时准确字段阶段/当前frame及backtrace、Kill前实际member状态/source/链身份，不改产品、线程或协议。WaitOne与WaitSet两层来源、Kill后摘链/loan/最后原账及Revoke DONE分别判定；一轮未定保存结果并清理后报告，不串其他路线。

`irq-exit-third.exit=143`：[入口](../artifacts/run/20261026-161218-12151-acceptance-irq-exit.log)、[客体265](../artifacts/failed-acceptance-20261026-161223-12528.log)、[原GDB](../artifacts/irq-exit/20261026T081231.056688Z-12539/exit.log)、[实际指纹](../artifacts/irq-exit/20261026T081231.056688Z-12539/evidence.json)、[主动收割原因](../artifacts/device-closing/irq-exit-third-harvest.json)、[执行](../artifacts/device-closing/irq-exit-third-state.json)。新kernel7773d62e…/init61d89028…/boot98cd9b8a…；工具实际核Query3360aa、Kill2f19a6及本轮init入口12ac6，仍没有IRQ_EXIT_NATIVE_CONSUMER_ENTERED，sources/targets均空。两个退出场景都未命中，因此没有其Waiting/Line或Set成员/摘链loan/独立账证据，未记通过。

前置Copy确用20s绝对141.860258→161.860258s；同步ReadAt144.430371→144.469400s，取消任务完成其本地收尾，健康任务尚无最终Outcome。随后明确内部终局`Runtime stopped: platform clock failed; doorbell failure mask 0x0`，不是Timeout错误或1120外层耗尽；不能把根因泛归Copy/IRQ/调试器。内核时钟失败原因尚未定位，属本次实际失败，不构成已证待修机制缺陷。为结束已停止客体，确认本轮helper身份后只向PID12539发送TERM，经既有ExitStack清理，命令实际143；GDB随后incomplete仅未成窗尾部。按[精确可执行文件清理核验](../artifacts/device-closing/irq-exit-third-final-cleanup.json)无QEMU/GDB残留，不依据macOS固定宽度comm判断；不重跑、不改时限、不串Root/IPI/其他路线，报告后再选后续。

#### 实际时钟失败的有限首因调查方案

该实际失败阻塞后续验证判断，六组原未证保持独立。`clock::STATE.failed`的源码写入只有正常read回退分支54与snapshot转换错误闭包60；selftest使用独立ClockState。`runtime_stop::check`在锁外消费failed并报告终局，末尾字符串没有首次prior/raw/error来源信息。旧客体已清理，无内存/core快照，原prior/raw/effective raw、geometry、首次hart/PC/调用栈及Entry跨PID命中次数不可离线恢复；磁盘ELF的BSS初值不能冒充这些运行值。

同hash7773d62e…的[源码/机器码](../artifacts/device-closing/clock-first-cause/failed-write-code.log)、[DWARF作用域/布局](../artifacts/device-closing/clock-first-cause/dwarf-scopes.json)、[实际Release store/rdtime](../artifacts/device-closing/clock-first-cause/atomic-store-code.log)、[Release跳表](../artifacts/device-closing/clock-first-cause/release-store-dispatch.log)已核：

| 接点 | 本ELF机器地址与可读性 |
|---|---|
| 平台状态 | STATE VA40dd88/PA8040dd88，high_ticks+0/high_ns+8/failed+16；GEOMETRY VA40dda0/PA8040dda0，value+0/state+24，实际几何为frequency/origin/max三u64 |
| 回退首因 | 正常read_time实例221ba8–221d86，221cb0在store调用221cb4之前，参数已是failed目的地址/true/Release；同scope有self、geometry、prior51、sample raw52 |
| 转换首因 | 正常实例错误闭包221fd2–221ffc，221fe4在store调用221fe8之前，有error；祖先read的221c68及pc-1都有prior51、sample raw52、effective raw58及geometry，按具体symbol/声明行区分同名raw |
| 实际写入 | 两调用同经Atomic<bool>::store29ffce→atomic_store<u8,false>，Release分支37e5ce，fence rw,w在37e5d2，sb在37e5d6；sbi::read_time本身已有fence rw,i→rdtime→fence i,rw，不能猜缺保序 |

[原IRQ启用点](../artifacts/device-closing/clock-first-cause/irq-active-points.log)：Query3360aa/Kill2f19a6在Entry之前禁用，Entry12ac6启用并核真实PID1，未观察到entered。其他进程相同VA可能触发Entry检查，旧日志没有计数，不能称无调试停顿，也不能把尚未启用的IRQ读链归为原因。实际[命令元数据](../artifacts/failed-acceptance-20261026-161223-12528.meta)显示50%节流、4hart、virt/rv64/zkr、无icount，GDB由Device admission日志后attach，并非冷启动-S；meta throttle=unknown不覆盖命令中明确的50。TOOLING要求GDB用100，原50是调试配置偏差，仍非时钟回退根因证据。

本次已确认只执行一次调查，复用原IRQ消费者构建/4hart/无icount，采用THROTTLE100、冷启动-S及单一GDB，仅安装上述两个错误路径硬件点，不装IRQ Entry/Query/Kill；与原配置的差异逐项记录，不声称完全同配方。具体调查工具复用既有just构建/专有VM组清理，不加产品hook、线程或通用探针框架。

1. 按实际ELF指纹重新定位正常实例与两store前机器点；原atomic目的地址须等于平台STATE.failed，同时核实际self/STATE身份，排除selftest。
2. 第一个合格错误路径全停快照记录写前failed、分支、mhartid/PC/SATP/原调用栈、同次局部prior、sample raw52、转换分支effective raw58与error、传入geometry和全局GEOMETRY/发布位、全局high_ticks/high_ns。局部prior与当下全局高水位不是同一时点，分别命名保存，不互换。原current_thread可为0，不据此否定内核续体owner。
3. 固定本读帧432B及转换帧32B只读原始字节作为DWARF再解码依据；变量读取失败保存准确字段/栈及已有快照，不能填0或以断点后新time替代旧样本。写前failed已真则只记迟到观察，不冒充首因。
4. 一次快照后让原store与原终局自然执行，主机收原platform clock failed与证据；不写寄存器/内存/阈值/clamp，不单步或人为改时钟。没有命中而到正常终态，只记本轮未复现；其他内部失败、取样错误或外层截止分别记录并清理，不多轮追。
5. 先完成静态生成/指纹冻结再运行，结束后清理本次QEMU/GDB/私有资源并回报，不自动恢复其他专项。本次不是IRQ退出门验证，即使消费者正常结束也不以无IRQ来源快照关闭原门。

具体调查入口已实现为`just investigate-clock`与`tools/check-clock-failure.py`：使用原just构建及acceptance-irq-exit feature，不改消费者；独立有限宿主调查只复用既有符号/物理读取/专有组回收，单一GDB仅含两个错误路径点，不调用原IRQ探针。一次首因快照经原store后须收到实际platform clock failed终局；无命中须原两个消费者及正常Shutdown结束才记not-reproduced。取样错误、其他客体失败与外层截止均保留准确状态并清理，不自动追加运行。宿主与GDB分别拥有run.json/first-cause.json，后者原子替换落盘，短清理段屏蔽重复终止并收专有VM组/调试进程。

本次静态检查：工具模块与两完整生成Python块AST通过，生成脚本只有两次FailurePoint构造且无guest寄存器写；Just dry-run明确THROTTLE100/4hart/无icount。原同hash指令/作用域证据仍只证明布局，不代表实时变量可读或已定位时钟根因。执行前源版本冻结到`artifacts/device-closing/version-before-clock-first.json`；仅一次调查，结果随后回填，其他专项暂停。

`clock-first.exit=0`只表示一次调查正常完成：[入口](../artifacts/run/20261026-165440-24421-investigate-clock.log)、[运行状态/指纹](../artifacts/clock-first-cause/20261026T085443.991451Z-24806/run.json)、[未命中状态](../artifacts/clock-first-cause/20261026T085443.991451Z-24806/first-cause.json)、[原GDB](../artifacts/clock-first-cause/20261026T085443.991451Z-24806/gdb.log)、[客体](../artifacts/clock-first-cause/20261026T085443.991451Z-24806/qemu.log)。run.status=not-reproduced、probe.status=not-triggered，没有任何首次失败的运行变量快照；kernel7773d62e…/init61d89028…与原IRQ失败ELF相同，实际boot88f949a9…/script2e1ceb8b…已固定。

实际采用100%、-S冷启动、两个错误路径点且不含IRQ点；与原50%、Device admission后attach、用户Entry启用有多项差异，不能据此把旧异常归给节流或断点。客体406–409：WaitOne PID28/source32/gen6/Pool20400与WaitSet PID29/gen7/Pool20408均沿原消费者关闭，两维allocated/delegated/reserved零、scope DONE；428/429显式Shutdown并QEMU正常退出。只证明本轮实际消费者业务完成，不证明曾Waiting/Line成员身份，也不替代IRQ原门或时钟修复。

[结束版本与清理](../artifacts/device-closing/clock-first-final-version.json)核944项在执行期间无差分，实际kernel/init/boot与本轮指纹一致，QEMU/GDB残留零；未自动追加任何路线。当前独立时钟异常仍根因未定，六组原未证保持原边界；该例无新产品修复，后续按本记录安排，不重复本次未复现调查。

随后单项`THROTTLE=100 just acceptance-irq-exit`，原IRQ工具/消费者均保持当前版本，仅按手册修正调试节流；仍为Device admission后attach、Entry/Query/Kill原启用关系，无-S/无clock点，不能与clock专线混写。执行前固定`artifacts/device-closing/version-before-irq-exit-fourth.json`；只补两个真实Waiting/来源/链及退出收束，时钟终局再现立即经原helper清理收割，任一目标工具失败保存准确字段/原frame后停止该项，不自动串路线。

IRQ第四轮实际`exit=1`：[入口](../artifacts/run/20261026-172200-26369-acceptance-irq-exit.log)、[状态/清理](../artifacts/device-closing/irq-exit-fourth-state.json)、[原GDB](../artifacts/irq-exit/20261026T092207.960444Z-26766/exit.log)、[完整观察/指纹](../artifacts/irq-exit/20261026T092207.960444Z-26766/evidence.json)、[客体](../artifacts/failed-acceptance-20261026-172204-26755.log)。THROTTLE100/Device admission后attach、无-S且仅原IRQ点；kernel7773d62e…/init61d89028…/bootfcc2e72b…，944项冻结版本执行期间无源码/工具变化。没有时钟终局/外层超时/主动收割，结束QEMU/GDB零残留。

第一子门已成立：PID28/source32/gen6/epoch2实际Waiting，原Thread.wait_entry持Line loan、无timer且active_wait，thread节点owner为原Line链；退出后该generation原Line空链/Work Idle/strong回2。此两根只指本取样点仍活的Node字段与Controller源槽，不泛化Line任意时刻计数。客体406–407原Kill/DONE/Control关闭、Pool21309两维allocated/delegated/reserved零后才做第二Line Query，随后原Revoke/两个DONE成立。该最小门复用，不重跑。

第二子门未证：PID29/source32/gen7在实际Kill前原MemberState=Running，旧工具以“not actually parked”提前拒绝，未读取Set/Member、未见第二次closed/退款；保留原失败，不据Running否定Set责任。**审定纠正**：正文248/455的该项是含Line成员的真实WaitSet随宿主退出，不要求宿主同刻park；直接Waiting已由第一种覆盖。Set门须从原Body真实已安装cap定位Set，核唯一Member实际附原Line，退出后核Member/source loan与原账归还；若宿主恰好Waiting，再附加核线程源为该Set、线程节点挂Set.source链。不得把“调用过wait”当Waiting，也不再为Set强凑park时序。

本次仅离线修改`tools/qemu_irq_exit_gdb.py`：沿原Body.handles有限初始化slots枚举Occupied，跳过纯整数空/预约/Claimed/Retired态；ObjectAuthority.__0是KernelObject fat Arc，用当前ELF WaitSet静态vtable识别具体类型，再由WaitSet.this的实际DWARF类型推导ArcInner<WaitSet>，不调用guest方法、不猜drop符号/已失效类型别名。Set别名按真实ArcInner合并，保存cap slot/generation/rights及Set KOID/Member/source/chain；只接受唯一附原Line的单成员Set。Running宿主不再被拒；Waiting附加条件保持严格。退出后仅查询父仍持有的Line，不解引用已经退出的Set/Member；原父消费者已在Control关闭且子账全零后Query，再Revoke/DONE，无需新增查询/线程/产品状态。

离线验证`python3 -B artifacts/device-closing/irq-set-offline/verify-offline.py`退出0：[类型布局](../artifacts/device-closing/irq-set-offline/dwarf-layout.json)、[静态初始化日志](../artifacts/device-closing/irq-set-offline/setup-audit.log)、[验证/源码与ELF指纹](../artifacts/device-closing/irq-set-offline/validation.json)。完整两段生成Python AST通过；实际同hash ELF的KernelObject vtable=0xffffffc0803c21e0、ArcInner类型、Cap/Body/Member/Entry与Rights字段已核。初次离线生成发现nm符号带`<WaitSet>::new`，已按真实符号修正；这不涉及guest运行。仅证明类型/脚本静态关系，尚无实际cap值、Set/Member读取或第二退出运行证据；未启动第五轮，Root/IPI/MMIO/Retry均未追加。

IRQ第五轮单项`THROTTLE=100 just acceptance-irq-exit`实际`exit=1`：[入口](../artifacts/run/20261026-175328-33469-acceptance-irq-exit.log)、[运行状态](../artifacts/device-closing/irq-exit-fifth-state.json)、[GDB/准确原栈](../artifacts/irq-exit/20261026T095337.359704Z-33867/exit.log)、[完整观察](../artifacts/irq-exit/20261026T095337.359704Z-33867/evidence.json)、[客体](../artifacts/failed-acceptance-20261026-175332-33852.log)。仍为100%/Device admission后原attach、无-S/clock点；kernel7773d62e…/init61d89028…/boote73f5a16…，工具使用离线修正版，本轮没有修改。没有时钟终局/外层撞限/主动收割，harvest=null。

PID28/source32/gen6/tid1实际Waiting/epoch2，原Line source loan与node链吻合；同generation第二Query为空链/Work Idle/strong回Node+Controller两份。客体406–407本轮Pool19409两维allocated/delegated/reserved零、scopeDONE，复用既有直接退出门，不扩大到第二种。第二目标PID29/source32/gen7在Kill提交前MemberState=Ready；原Body cap表initialized_slots=5/occupied=3，工具完成有限Occupied枚举后cap_scan.sets为空，未匹配静态预期vtable。失败在`process::kill`原PC0xffffffc0802f19a6，完整原caller/frame/bt已保存。提示not unique在本例实际为零匹配，不是多Set；没有读取实际Set KOID/Member/Line挂链，也未取得第二退出后Line/原账，因此本子门仍开放。

本轮没有逐项保存cap raw/vtable，旧现场退出后无法离线恢复各项具体值；不能仅据零匹配判定产品没有Set或对象生命周期错误，也不能将静态vtable存在升级为实际cap匹配。下一若获安排，先离线检查静态对象识别假设与生成点，再用有限逐cap诊断定位拒绝门，不能无边界扫描/降低成员链判据；本次不现场修工具、不启动第六轮。Root/IPI/MMIO/Retry均未串行启动，独立时钟异常仍未定位。

[结束版本/实际产物与清理](../artifacts/device-closing/irq-exit-fifth-final-version.json)核944项与执行前冻结完全一致、实际kernel/init/boot与GDB证据一致，真实ps核QEMU/GDB/cpulimit及本次helper/runner零残留；本段文档回填在该快照之后。现成路线附带WaitOne未新增feature/运行体，没有新产品缺陷结论；整轮失败与已闭最小子门分别保留。

**Set退出夹具生命周期修正（第五轮后，仅静态）**。亲读原test_target分支与内核add/Receive确认可达错误：register无条件enqueue候选使集合READABLE；libkernel::WaitSet::wait只等集合电平，单次wait可在真实Line未就绪时合法返回。原分支随即离开if作用域、正常Drop(set)并到panic，不再保持本门要求的Set/Member owner。此为验收消费者fixture缺陷，不能据此确认第五轮确实走到哪一步或确定零vtable匹配的实际因果；旧exit1/未证不改写。

`irq_exit_target`改为在Set创建/注册/ready之后进入持续观察作用域：固定单项ReadyRecord数组沿原receive_into复检实际来源，零记录后等待集合提示，READABLE再回Receive。Set为loop外同一owner且循环无正常退出；不Remove/Close、不新建集合、不加sleep/线程/同步门。该来源按原场景不预期硬件事件，非零ReadyRecord、集合CLOSED/其他意外状态或Receive/Wait错误均失败，不能吞掉真实事件。直接WaitOne分支及其严格Waiting判据不变。只同步父消费者注释、构建手册该路线语义和impls实际行为，没有改内核/公共库API/产品机制，也没有给vtable识别叠补丁。

静态核验：[生命周期/原API依据](../artifacts/device-closing/irq-set-fixture/lifecycle-review.json)为人工源码审阅，不是运行测试；register初始候选、Receive实际电平过滤、Wait仅集合提示和Drop真实Close分别有独立源码依据。目标命令经现有`just output`执行`cargo clippy -p test_target --bin test_target -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings`，[target-clippy-second.exit=0](../artifacts/device-closing/irq-set-fixture/target-clippy-second.log)；`cargo fmt -p test_target -- --check`[fmt-final.exit=0](../artifacts/device-closing/irq-set-fixture/fmt-final.log)。初次检查因output的bash -c命令参数未保留单参数而在仓库根找不到Cargo.toml，未编译；改用项目内具体检查脚本后Clippy通过。随后fmt仅要求一处assert换行，修正后通过；保留原失败记录，不归因fixture。

本轮没有起第六轮或其他QEMU/GDB；新test_target生命周期的运行覆盖、Set实际cap/Member→原Line挂链及Kill后的原账/DONE仍待必要单项。旧kernel/init/boot产物属于第五轮，静态Clippy不冒充重建运行产物；后续若安排运行须重新构建并固定当轮身份。Root诊断/IPI与独立时钟记录仍保持原未定范围。

**IRQ第六轮：修后真实消费者与两个退出子门通过。** `THROTTLE=100 just acceptance-irq-exit`退出0，[入口core passed](../artifacts/run/20261026-180624-40644-acceptance-irq-exit.log)、[状态](../artifacts/device-closing/irq-exit-sixth-state.json)、[原GDB](../artifacts/irq-exit/20261026T100635.591971Z-41077/exit.log)、[完整对象观察](../artifacts/irq-exit/20261026T100635.591971Z-41077/evidence.json)、[客体](../artifacts/qemu-acceptance-20261026-180630-41066.log)。仍为100%/Device admission后原attach，原只读工具未改，无-S/clock点；无时钟终局/主动收割或外层撞限。

消费者实际身份见[完整装箱链](../artifacts/device-closing/irq-exit-sixth-consumer-identity.json)：源码8f28da60…、新test_target debug ELF b9e0c0bd…，按Just已有strip-debug规则生成派生副本，与initfs的bin/test_target a38c0c60…及staging逐字节一致；BootPackage实际payload与该initfs、init段与实际ELF分别逐字节一致。本轮kernel7773d62e…/init61d89028…/bootc626effa…；init未变不能替代消费者身份，debug与strip文件hash不同是原构建合同，不记构建错误。身份检查不写运行产物，派生副本只在本次证据目录。

实际PID28/tid1/source32/gen6为Waiting/epoch2、无timer且active_wait，Thread source loan及节点链属于原Line；后置同generation/同Line空链、Work Idle、strong回原Node+Controller两根。第二PID29/tid1/source32/gen7为Running/epoch1，waiting=false；真实Body cap表5初始化槽/4占用，slot4/gen1/rights69指向WaitSet KOID19019（ArcInner0xffffffc0809d1740/body0xffffffc0809d1750）。唯一Member attached=true，来源为原gen7 Line，node0xffffffc0808c9290的owner精确等于原Line chain0xffffffc080801328；没有以Running拒绝，也没有声称宿主已park或核过线程到Set的第二层。Kill后父仍持原use，后置只读同gen7原Line为空链/Idle/strong回2；未解引用已经退出的Set/Member。

客体405–409：两目标分别实际Kill/终因/DONE/Control关闭并等Pool19003、19011的页与metadata allocated/delegated/reserved全零，再取原Line Query、Revoke并核两个DONE；409退出验收通过。417验收domain collected、426root辅助cap清理、429显式Shutdown和入口core passed成立。该证据关闭两个最小退出门及本次fixture修复的运行回归，不外推强公平/全idle，也不据此给第五轮零匹配确定运行归因。

[执行期版本与最终清理](../artifacts/device-closing/irq-exit-sixth-final-version.json)核944项与冻结无差分，实际消费者/boot/kernel/init身份仍一致，独立ps核QEMU/GDB/cpulimit/本次helper与runner零残留，私有meta已清。旧失败全部保留；本轮后仅更新文档，不自动串其他专项或重复已过聚合。IRQ剩余仅原更强公平差额，Root诊断/IPI/时钟等保持各自原未定界；整体设备闭包仍开放。

下一单项只复核MMIO Forced/Frozen（第四轮），THROTTLE100，原两个32页Map消费者不改。开跑前亲读发现既有工具在ProcessShell::freeze_and_seal停点回找process::kill外层后读取不存在的`thread`，实际参数为`caller: &Thread`；只修这一明确工具变量名，不设缺字段fallback/猜owner，不改产品。离线[当前ELF作用域](../artifacts/device-closing/mmio-forced-offline-fourth/scopes.json)、[生成/机器点与AST核验](../artifacts/device-closing/mmio-forced-offline-fourth/validation.json)退出0，runtime_values_available=false：实际kernel7773d62e…，advance266=0xffffffc080224578，step锁外Publishing=0xffffffc08022576a，在advance调用返回后/下一Queue锁前；Revoke279=0xffffffc0802be2b6、freeze_and_seal537=0xffffffc08024f178均在参数落栈后。地址由当前ELF重新定位、同值不代表沿用历史常量；queued参数/step实际栈Execution/self及外层kill的caller/类型已核。只生成完整脚本/AST，不连接客体。

该单项仍要求两个真实mapping/原FIFO与栈Execution，原Revoke成功返回后执行Kill并核access_open=false才放行Work；不等待scope/CPU DONE才恢复，不改许可/页表/PC/时钟或降低判据。实际生成点、运行消费者/boot/kernel/init另随本轮冻结；目标工具不定一轮后停项，不自动追加运行，IRQ/聚合及其他专项不串行。

**MMIO Forced/Frozen第四轮：前置运输失败，窗口未覆盖。** `THROTTLE=100 just acceptance-mmio-forced`退出124，[入口](../artifacts/run/20261026-181817-50398-acceptance-mmio-forced.log)、[原客体](../artifacts/failed-acceptance-20261026-181831-50810.log)、[状态/时钟收割字段](../artifacts/device-closing/mmio-forced-fourth-state.json)、[原GDB](../artifacts/mmio-forced/20261026T101857.685852Z-50821/forced.log)、[空窗口与生成点](../artifacts/mmio-forced/20261026T101857.685852Z-50821/evidence.json)。本轮明确100%、原工具只含已冻结caller变量名修正、原两个Map消费者不改；没有运行其他专项。

客体152–159先记录真实Provision.Get（BindingRef serial1/epoch1、index3、txid7）phase Sent/cause Timeout，随后原Binding转Stopping且resources_exposed=false、平台资源退休等待；file-watch consumer Transport(Timeout)、init client subscription stopped before cycle→原device failure terminal，helper记录MMIO publication entered device failure terminal。尚未到test_target MMIO admitted或原Revoke选择；selection/pending/windows均为空，未观察实际FIFO/Execution、Revoke返回、Frozen或映射/子账窗口。不能从本例判定Forced缺陷、新caller读取成功或原0x56修正有效；此前Copy健康完成也不替代本例别的前置RPC成功。

124分类以源码和原日志为准：`qemu-acceptance.sh`终态集合包含acceptance failed，239–243命中后主动TERM原runner并设status=124；本轮约65s而非1120路线期限耗尽。独立clock观察runner的harvest=null只表示没走clock专用收割，不等于QA未主动收割。GDB先看到inferior exited，之后No registers是原终态收割后的工具尾部，不作首因。现有日志不足确定这些运输Timeout的原停滞/调试影响，不猜已证内核bug，不机械扩大产品RPC/停止或路线预算。

[实际消费者装箱身份](../artifacts/device-closing/mmio-forced-fourth-consumer-identity.json)关联修后test_target源码8f28da60…/debug ELF b9e0c0bd…→按原strip-debug得initfs条目a38c0c60…；BootPackage实际payload/实际init分别逐字节匹配本轮initfs/ELF。kernel7773d62e…、本轮MMIO配置init01451872…/boota7d823f1…与GDB指纹一致；生成地址由本轮ELF重建，机器点相同不冒充沿用旧地址。

[结束版本与独立清理](../artifacts/device-closing/mmio-forced-fourth-final-version.json)核944项执行期无差分，实际消费者与boot/kernel/init仍匹配；独立ps核本次helper/runner/QEMU/GDB/cpulimit零残留，私有meta已清。只完成这一轮，按安排保留未覆盖并停项，不现场改探针、不继续第五轮、不串Root/IPI/Retry等；IRQ已过门及原聚合证据保持原范围。

#### 本次离线收敛：Root有限诊断与IRQ持续事件证据映射

MMIO第四轮归档后没有运行新QEMU。亲核其工具初始只有Revoke点启用，Kill/OK/error/Work点均禁用；现有前置Timeout不能据此归因为“高频全局返回断点”。该本轮首因/工具影响尚未确定，旧未证窗口不改判，产品期限与路线预算均未动。

**Root候选诊断已实施，未运行。** `qemu_platform_funding_gdb.py`仅为retained模式增加至多4份结构化候选：在祖先筛选前先落实际allocator PC/a0/a1与候选项，再存完整原栈的frame.block、PC和PC−1定义scope及原Root/OwnedBox/bootstrap匹配布尔；PC−1只作诊断，不作为替代owner/fallback。期待布局从当前ELF中领域具体Prepared.retained的实际类型推导，不凭pretty字符串匹配；本ELF为104B/align8，祖先失败也保留该期待值。实际候选value Layout独立记录，payer包括未到/读参数/比较阶段、原参数类型/指针、期待与实际Pool地址/KOID/匹配结果；raw/layout/payer读取失败保存具体阶段与error，不将读失败写成付款不匹配。每份候选只有首个拒绝原因，或明确selected；候选达到4仍不命中即停，原Root返回未注入同样立即止损。没有扩大分配范围或新增产品付款状态。

[离线核验](../artifacts/device-closing/root-candidate-offline/validation.json)退出0，三种完整生成脚本AST通过；只执行retained脚本构造Breakpoint前的静态类型初始化，不连接客体。实际kernel7773d62e…与第三轮相同：[原完整栈各返回PC/PC−1作用域](../artifacts/device-closing/root-candidate-offline/return-pc-scopes.json)确认OwnedBox原返回PC0xffffffc080255d6e两侧均属funded_storage::try_new，value是实际ordered_table::Node<Arc<DMA Node>>、104B/align8，payer为MetadataFunding指针。新生成脚本的in_source函数AST与第三轮真正执行的脚本相同，原set-a0-null/return-PC注入命令亦相同；本次只添加有限诊断与明确止损，不声称旧frame.block实时值等于离线block_for_pc，更无法恢复旧寄存器/实际payer。Node/install已过分支不重跑，特定Root Work/退款仍未证；所有生成证据明确offline_generation_only/runtime_values_available=false。

**IRQ持续事件证据映射（供监督审定，未改原契约）。** 原248要求一次claim/短锁/既有通知/不得保留退出线程；250要求安全点/idle同一有界服务与返回仲裁；276要求事件批预算与未Ack源不反复入内核；455要求不丢持续就绪、不重复complete新轮且普通任务推进。其命题和实际owner如下；“未做长期/同hart/任意高频负载”仅是证据适用限制，不自动产生新的无限测试门。

| 原合同的具体命题 | owner与实际调用链 | 现有证据及适用边界 |
|---|---|---|
| 同源已claim而未Take/Ack时责任/READABLE不消失、不重复交硬件领取 | plic::Service的in_service/taken/closed与唯一CONTROLLER锁；claim发布Line电平，Take才清READABLE，Ack核原token后complete | [host-os](../artifacts/device-closing/host-os.log)369–373实际三个PLIC测试；真实irq-retirement-first在途/跨generation原Native complete计数。irq-readable-first客体179/180、201/202在未Take前取得认证Query并复probe仍READABLE，再Take。host不冒充硬件，Native与真实RPC补各自边界 |
| 单次硬件/通知推进有界，持续提示不能在内核安全点无界排空 | controller::service118–171无循环最多读一次claim；Line::step323–335至多一个queue节点/锁外Delivery；signal::step每次一个节点或边界，电平未变不重置requested/游标 | 原调用链静态检查；[host-os](../artifacts/device-closing/host-os.log)21–23覆盖signal边界/插入/删除/电平；真实IRQ收到/Take/Ack和两个退出门直接经过Line。不是对任意硬件/锁停顿的性能上界 |
| 持续IRQ活动下仍回普通执行仲裁，已有普通任务能推进 | 用户SEIP分支trap115–118只service一次后Requeue；sched480→maintenance::service491，M每次advance_one后回仲裁；Rotor有限机会/负债选择U与M，不因零费用/失败选择永占；idle睡前/醒后复用同一service | [host-os](../artifacts/device-closing/host-os.log)155–165实际rotor份额/欠账/有限机会/零费用与选择失败测试，加当前内核静态接线；[irq-readable-first客体](../artifacts/qemu-acceptance-20261026-112559-2534.log)两次READABLE保持期间原srv_device完成Query；不把多hart上一次RPC说成同hart长期比例测量 |
| 持续事件在驱动批次间仍前进，而非必须等新IRQ才续处理 | 正式xHCI Engine::hardware198–237每批最多16个event、未排空返回more，排空才Ack；业务受原Runtime/进程执行预算，IRQ本身不另建执行通道 | [正式两轮热插客体](../artifacts/qemu-acceptance-20261025-064822-78040.log)同gen5实际seq1…31等连续Take/Ack，期间PID10真实按下/释放/OAL业务推进；结合原有界服务/仲裁结构，不宣称任意频率压力或WCET实测 |
| 退出不残留线程强根/订阅loan | 原Wait/Member::drop/Line::detach与真实父母本；不再属于持续事件缺口 | 第六轮直接Waiting与Set成员随Running宿主退出/空链/独立账已通过，见已关闭表；不重复施工 |
| 全hart idle仍能由真实外部pending推进 | formal逐hartPLIC接管、sched idle前后服务、真实PLIC/SEIP | 这是250/455已有独立门，不能由host或前述普通任务进展替代；共同快照/真实插入→claim/Take仍未证，继续单列 |

身份审计[irq-fairness-mapping.json](../artifacts/device-closing/irq-fairness-mapping.json)确认PLIC、signal、rotor及controller/maintenance/sched/trap共7份当前源码与既有host/聚合版本完全一致，未追加测试。建议把“IRQ更强公平差额”按上述组合证据收口，不另要求原合同未定义的单hart指定份额、任意中断频率吞吐或无限时长压力；这不是删除持续事件合同，也不把静态论证写成QEMU压力运行。若审定认为现有组合仍缺某个可达持续事件活性命题，应只保留其具体前提/owner/最小有限判据；当前先报映射，矩阵标待审定，不擅自关门或新建消费者。

监督审定已通过上述证据组合：独立“IRQ更强公平差额”关闭，不增原合同未定义的同hart指定份额/任意频率或无限压力门；不是删除持续事件合同，也不是将静态当作做过硬件压力。当前唯一矩阵已移入可复用表，待证减为五组，时钟未知另列、全idle明确门继续开放，IRQ退出不重新登记。随后只安排root retained-entry一个mode一轮，使用本次有限候选诊断，实际无法恢复的旧寄存器不再猜测；不重跑node/install或其他专项，结果/未定均收尾后报告。

**root retained-entry第四轮：确定诊断已取得，指定失败未到。** `THROTTLE=100 just virt-platform-funding dma-retained`仅此mode，退出1：[入口](../artifacts/run/20261026-184324-55304-virt-platform-funding.log)、[原GDB](../artifacts/boot-failure/20261026T104329.003605Z-55679/platform-dma-retained-gdb.log)、[原客体](../artifacts/boot-failure/20261026T104329.003605Z-55679/platform-dma-retained-qemu.log)、[有限候选/原作用域与指纹](../artifacts/boot-failure/20261026T104329.003605Z-55679/platform-dma-retained-evidence.json)、[状态](../artifacts/device-closing/root-retained-fourth-state.json)。本轮只运行retained，未重跑node/install；cold-S/4hart/100%，kernel7773d62e…/boot0e3f4e9a…/init384b1d40…，实际生成script0ebb3b74…。

实际Gate0/Node5/Pool1、原root0xffffffc08087d4f0/cap_count10，唯一候选在未执行allocator入口PC0xffffffc0803599e8的a0=104/a1=8，与目标期待104/8相符。原OwnedBox返回PC0xffffffc080255d6e的owned_box=true、Node::root2742d2的root=true、bootstrap3565b2/2a2ac2的bootstrap=true均为本次真实frame匹配，已经缩小前轮未知门；不以静态block假定这些值。随后新增全栈诊断走到汇编尾帧_enter_hart_high/PC0xffffffc080352b1c，frame.block抛Cannot locate block for frame，候选rejection=observation-error、stage=frame.block definition scope。真实value Layout仍null、payer stage=not-reached，未selected/未注入，未出现真实OutOfMemory/原Node最后Drop/特定Root Work或四park证据。该失败属于诊断把非筛选汇编尾帧也强制要求词法块，不是产品/付款失败；不能拿raw104/8关闭此门，也不倒推第三轮实际拒绝原因。

[结束版本/产物与独立清理](../artifacts/device-closing/root-retained-fourth-final-version.json)核944项与冻结无执行期差分，实际kernel/boot/script一致；独立ps核本次helper/runner/QEMU/GDB/cpulimit均零。只此一轮、没有超时/其他专项；本轮事实归档后按监督已授权范围离线修这一具体诊断缺陷，非参与判定尾帧记录scope_unavailable/原GDB错误，参与Root/OwnedBox/bootstrap或value/payer的帧继续严格，不能全局吞错/伪造匹配或fallback身份。修后只作离线scope/生成AST与差分检查，暂不第五轮。

**上述尾帧诊断阻断已离线修正，未再运行。** retained候选对各帧先调用原in_source完成Root/OwnedBox/bootstrap的严格判定，同一匹配结果供诊断与必要选择复用。凡名字参与这三类谓词，原frame.block/定义source仍必须可读，value/payer也无fallback。只有不参与三类筛选的辅助帧在frame.block抛精确RuntimeError“Cannot locate block for frame.”时，记录frame_scope=null/scope_unavailable原错误并继续纯诊断；其他错误仍原样抛出，未全局吞GdbError。PC/PC−1没有词法块也如实记null，不当作身份或伪造匹配；原注入/付款/产品语义不变。

[新离线核验](../artifacts/device-closing/root-tail-offline/validation.json)退出0；[当前ELF作用域](../artifacts/device-closing/root-tail-offline/return-pc-scopes.json)使用第四轮实际保存的帧PC，确认_enter_hart_high352b1c与pc−1都无DWARF词法块，而原255d6e的OwnedBox/value104/8/MetadataFunding以及Root/bootstrap作用域仍可核。该汇编帧实际frame.block错误来自第四轮保存的真实error，不伪装为离线调用过live frame；实际owner匹配true亦保留本轮来源。三种完整脚本AST/静态初始化通过，新in_source函数AST与第四轮真正脚本相同、原寄存器注入命令相同。新版只落工具诊断，无第五轮/其他QEMU；真实value/payer、指定OutOfMemory、原Node最后Drop与特定Root Work接管仍未到，raw104/8不替代这些责任。

**root retained-entry第五轮：实际value/payer已到，严格Layout门拒绝，指定失败未到。** `THROTTLE=100 just virt-platform-funding dma-retained`只此mode，退出1：[入口](../artifacts/run/20261026-185433-57087-virt-platform-funding.log)、[原GDB](../artifacts/boot-failure/20261026T105437.803743Z-57452/platform-dma-retained-gdb.log)、[原客体](../artifacts/boot-failure/20261026T105437.803743Z-57452/platform-dma-retained-qemu.log)、[完整原观测](../artifacts/boot-failure/20261026T105437.803743Z-57452/platform-dma-retained-evidence.json)、[状态](../artifacts/device-closing/root-retained-fifth-state.json)。当前actual kernel7773d62e…/boot2c43ba8b…/init384b1d40…/script824a7745…，工具89731c3a…执行期不改；node/install与其他专项未运行。

本轮Gate0/Node5/Root0xffffffc08087d4f0/Pool1，未执行allocator入口3599e8实际a0=104/a1=8；领域Prepared.retained链期待104B/align8。尾帧_enter_hart_high现如实scope_required=false、frame_scope/PC/PC−1均null、scope_unavailable保存原错误，未再阻断；原OwnedBox255d6e与Root/bootstrap定义仍匹配。实际candidate.read_var(value).type的全名与领域链相同，但其sizeof返回97、align8；payer的实际MetadataFunding指针0xffffffffc00c8898解析到Pool0xffffffc080801510/KOID1，与原期望地址/身份完全匹配。候选唯一拒绝为allocator-Layout-mismatch，stage=raw allocator Layout；这组差异是本次真实GDB观测，未解释成Rust实际分配97B或产品付款错误。

拒绝后保持原分配正常执行，在原Root返回点27f988记root_returned=true并由原require(injected)停止，故仍未selected/未注入/verified=false；未进入真实OutOfMemory、同Root最后Node Drop/预付Work接管或四park/稳定责任与局部退款观察。不能用104/8、payer一致或尾帧修复成功关闭资金失败门，也不将第五轮值倒推第三轮。下一如安排离线调查，须区分该帧value调试类型与领域实际类型链的布局来源，保留原97与104值，不能先强制取静态104、按align补齐或放宽匹配来命中。

[最终执行版本/产物/独立清理](../artifacts/device-closing/root-retained-fifth-final-version.json)核944项执行期无差分，当前kernel/boot/init与实际生成脚本身份均保存；独立ps核helper/runner/QEMU/GDB/cpulimit零残留。没有超时、未自动第六轮、未串其他项、未改产品/资金语义；本轮后只回填记录和报告，五组待证与时钟未知各自保留。

**Root分配Layout真值修正（第五轮后）。** 同hash ELF的原OwnedBox返回PC255d6e中value声明Symbol.type为104B/align8，info scope明确length104；声明字段height在byte96、size1，97等于最后字段末端，而声明总长度包含尾部padding。实际Box::try_new_uninit_in在2c0c8a调用2da650的Layout::new<MaybeUninit<该Node>>，机器码直接li a0,8/li a1,104/ret，随后原allocator实际a0=104/a1=8；领域Prepared.retained类型链独立同为104/8。它们共同证明本产物的真实分配布局，不能由旧runtime Value.type返回97判产品布局错误；未声称已查明GDB内部重建97的全部原因。

`value_declaration`只在已认证的实际OwnedBox帧词法块向上找到funded_storage::try_new定义，并要求唯一参数value声明；从其Symbol.type取得分配布局，精确核对领域链的类型/size/align，再用同一声明核真实a0/a1。runtime Value.type97保留在value_layout中作为诊断，新declared_layout/declared_scope单列真值与定义；无机械round-up、硬编码104替换或放宽区间。原Root/bootstrap、payer、Ready门及限定一次未执行分配的注入/退款/Work判据均保持。

[静态核验](../artifacts/device-closing/root-declared-layout-offline/validation.json)退出0：[实际声明/字段](../artifacts/device-closing/root-declared-layout-offline/declared-layout.json)、[独立机器码与info scope](../artifacts/device-closing/root-declared-layout-offline/layout-allocation-code.log)。三种完整生成AST、retained构造Breakpoint前的真实声明/类型初始化通过，原in_source函数AST和寄存器注入命令与第五轮实际脚本一致；其中机器码绝对地址证明只限本hash7773d62e…，生产脚本仍从本轮真实帧解析而不跨重编沿用常量。没有新产品/ABI/资金语义改动，旧运行证据保留。完成差分和冻结后仅安排retained第六轮一次，THROTTLE100；真实指定失败/最后Drop/特定Root Work与退款/四park仍待本轮实际结果，不重跑node/install或聚合。

**root retained-entry第六轮：真实指定失败与同Root接管已到，错误观察局部变量缺失。** `THROTTLE=100 just virt-platform-funding dma-retained`只此mode退出1：[入口](../artifacts/run/20261026-190938-59699-virt-platform-funding.log)、[完整原观察](../artifacts/boot-failure/20261026T110942.472621Z-60074/platform-dma-retained-evidence.json)、[原GDB](../artifacts/boot-failure/20261026T110942.472621Z-60074/platform-dma-retained-gdb.log)、[原客体](../artifacts/boot-failure/20261026T110942.472621Z-60074/platform-dma-retained-qemu.log)、[状态](../artifacts/device-closing/root-retained-sixth-state.json)。当前kernel7773d62e…/bootca12ad38…/init384b1d40…/实际scriptab65dfd8…，现工具bbf0248e…；执行期间保持冻结，未运行node/install或其他路线。

本轮OwnedBox真实定义scope255ce2–255e18的唯一value参数Symbol.type=104/align8，严格等于领域链及实际allocator3599e8的a0=104/a1=8；runtime Value.type=97/align8单独保留诊断，不用于分配门。原payer MetadataFunding实际指向Pool1/0xffffffc080801510，与Root期待一致；三类owner/Ready门均满足，候选selected=true且只对该尚未执行的真实104B分配注入null，injected=true。此前metadata allocated64061→该入口64074、available2032620→2032607，差13个8B槽，与本次实际charge吻合；只是收费快照，不是退款证据。

随后真实原Node最后Drop在原maintenance::kick处被观察：Root地址必须等于原0xffffffc08087d4f0，reclaim helper实际核tree唯一节点、mappings0、reclaim_head=Node5、该成员sealed/finished/dropped以及原Root Work状态Queued，root_drop_observed=true并打印PLATFORM_DMA_LAST_NODE_DROP。该证据直接属于本次失败Root，而非用其他五份Node Work替代；证明该时刻预付Root Work稳定接管，不证明已经执行/退费，也不等同四park后的全集合仍保留。

原错误观察点bootstrap265/PC0xffffffc0803565e4处frame.read_var(error)报Variable error not found，发生于actual_error与after账目保存/parking启用之前，所以probe.actual_error仍空、verified=false，未取四个不同hart的Gate Failed park与终态Queued复核。该时刻live错误帧的bt未保存，只有固定错误checkpoint/源码和异常行，不能事后伪补。原客体96行却明确输出`initial process construction cannot be admitted: OutOfMemory`，故指定失败/实际OOM有native运行证据，不得把probe局部读取失败写成OOM未发生。当前仅以准确字段/停点标工具缺口，不根据source行号猜error何时在scope中，不现场修/第七轮。

[最终944项/实际产物与独立清理](../artifacts/device-closing/root-retained-sixth-final-version.json)核执行期无差分，原kernel/boot/init与实际GDB脚本hash一致，独立ps核本次helper/runner/QEMU/GDB/cpulimit零残留。整体exit1保留：局部退款、四park/终态责任集合均未观察，不能因同Root接管或native OOM就宣称全根退款/整门通过；没有超时、产品/ABI/资金语义改动，五组原待证与时钟未知继续分开。本次只收这一轮，旧node/install及IRQ门不重跑。

**retained错误取样点修正（第六轮后）。** 同hash7773d62e…中旧bootstrap265/3565e4还在从result载入error，3565e8/ea才将值存到局部；error真实词法作用域为3565ee–35662e，声明SystemCallError。retained模式仅将源码索引改266，每次从实际ELF重新定位（本ELF3565ee），不跨重编硬编码绝对地址。运行到该点先存真实frame函数/PC/bt，再严格定位bootstrap::publish所拥有的唯一error局部、核声明类型并用Symbol.value(frame)读取；其他模式仍原点/原读取，产品错误/退款和注入门均不变。

[离线作用域与生成核验](../artifacts/device-closing/root-error-scope-offline/validation.json)退出0：[新旧scope与机器码](../artifacts/device-closing/root-error-scope-offline/scope-code.log)、[赋值指令](../artifacts/device-closing/root-error-scope-offline/error-assignment-code.log)、[精确error声明记录](../artifacts/device-closing/root-error-scope-offline/declared-layout.json)。实际生成新点位于上述作用域；旧点调用相同声明查找会明确拒绝，没有fallback。三种完整AST/静态声明初始化通过，原in_source及寄存器注入命令不变。静态不能替代live Symbol.value或后置Pool账/四park；完成冻结后仅retained第七轮一次，THROTTLE100，若仍未定就保留准确阶段收尾，不自动第八轮。

**root retained-entry第七轮：精确错误/局部账已取，首park后终态读取未闭合。** `THROTTLE=100 just virt-platform-funding dma-retained`只此mode退出1：[入口](../artifacts/run/20261026-192904-61860-virt-platform-funding.log)、[完整原观察](../artifacts/boot-failure/20261026T112907.844582Z-62230/platform-dma-retained-evidence.json)、[原GDB](../artifacts/boot-failure/20261026T112907.844582Z-62230/platform-dma-retained-gdb.log)、[原客体](../artifacts/boot-failure/20261026T112907.844582Z-62230/platform-dma-retained-qemu.log)、[状态](../artifacts/device-closing/root-retained-seventh-state.json)。本轮kernel7773d62e…/boot8f4baeaf…/init384b1d40…/实际script3836c514…，工具22bf94a3…冻结；其他模式/路线未运行。

严格声明/领域链/allocator104-8、原Pool1 payer匹配、真实指定一次分配失败及同Root最后Node Drop/reclaim/Queued接管再次成立。新错误观察点3565ee实际命中，精确error声明范围3565ee–35662e、SystemCallError及bootstrap::publish owner与真实frame/bt均已保存，Symbol.value(frame)实际为OutOfMemory，error_observation.stage=recorded。前置metadata allocated64061/available2032620→allocator时64074/2032607→错误观察64010/2032671，页allocated4/available256225保持，reserved/delegated均0；这是本失败局部owner回退，分配入口至错误点减少64 metadata槽、相对初始点净减少51槽，不把各组件混成104B单项退款或声称全根归零。

GDB随后实际打印`PARKED thread=1 gate=2`，进入同Root终态reclaim复核。tree.nodes.len=1/mappings.len=0的第一条件通过，随后reclaim_head存在且对应Node5的联合条件失败，报original root reclaim does not contain the dropped Node。该点原head数值/variant未在断言前保存，不能区分None/其他id/调试解释差异；JSON parked和queued_roots在后续save前仍空，只能用原log证明一个park，不能冒充四hart完成。原Root在最后Drop时的有效reclaim/Queued接管仍是正面证据，不能因终态读取失败改写其未发生，也不能以此宣布终态责任丢失或产品bug。

已定位但未证实的一处工具接缝：retained_root最初保存(root.address,root.type)，该GDB Type来自当时reclaim_head=None/record Idle的运行值；终态按它cast后读取。是否存在值特化类型导致误读，尚无该点raw/canonical对照，当前仅保存此具体线索，不强改状态、忽略断言或用旧Queued值替代终态。若后续获安排，应在同Root身份下先核实际静态类型与原字节/字段再判责任，不凭这条消息推定原因。

[最终944项/实际产物与独立清理](../artifacts/device-closing/root-retained-seventh-final-version.json)核执行期无差分，kernel/boot/init/实际脚本身份固定；独立ps核本次helper/runner/QEMU/GDB/cpulimit零残留。整轮exit1、verified=false及未完成四park/终态保留继续准确记录，无超时/产品或资金语义改动；不自动第八轮、不串其他专项，五组待证/独立时钟异常维持原边界。

**第七轮后的canonical观察与先存后判修正（仅离线，未第八轮）。** `tools/qemu_platform_funding_gdb.py`的retained_root只保存原数值地址，nodes只保存原地址与由源码确定的三类Node/DMA Root声明名；每个停点重新lookup当前ELF静态类型并从地址读取，不再跨停点保存运行值Type。静态声明来自`resource/dma/mod.rs`的Root/Node、`resource/irq/mod.rs`的Node及`resource/mmio/node.rs`的MmioNode；实际ELF记录了objfile、总长与字段链offset/type/size，并沿DMA Node.root的真实Arc字段声明核Root与独立lookup一致（本ELF Root280B）。实际原Root地址和NodeId由各自原frame/owner独立核，不由类型名字推导身份。

reclaim在任何责任断言前先落原Root/NodeId、阶段、thread/PC、tree/mapping数量、head/tail的完整raw字节及fresh canonical解释、Work状态raw/解释、Root及maintenance门的next/serving票号、member标志与当前栈；读取中断保留已取得字段和具体失败阶段。四park逐次先记实际thread/gate/是否重复及原observed事实，再做原准入和全集合检查；工作记录也先存真实状态而非预填Queued。require失败先存原因，原注入/候选/payer/error/Queued等门未放宽。票号仅说明锁门持有/等待，不据此猜hart，也不在GDB等持锁hart或把锁忙当通过。

源码责任解释仍以[Root::step](../os/kernel/src/resource/dma/mod.rs)的398–432行为准：持Root锁先更新head/tail，随后remove_finished，故中间head变化与树数量可以来自真实执行；maintenance Running时强根由advance_one的局部work持有，Queued链与Blocked根由原WorkRecord拥有。新快照须联合实际锁/状态/栈判定，不把固定head断言当全部owner证明；若真实Work已推进，依据原责任链解释，不能改产品、吞差异或伪造Queued。第七轮未保存的原raw/锁/终态不可事后恢复，缓存Type导致该次误读仍未证实。

[离线核验](../artifacts/device-closing/root-canonical-offline/validation.json)退出0：三个mode生成脚本AST、当前ELF声明/字段布局初始化均通过；[retained声明记录](../artifacts/device-closing/root-canonical-offline/platform-dma-retained/declarations.json)保留类型来源、字段偏移和maintenance Queue声明。与第七轮实际脚本比较，严格in_source、retained候选/精确error Symbol规则、寄存器注入命令均未变。生成前缀只载入ELF静态元数据，未remote/启动客体，runtime_values_available=false/new_guest_runs=0；无须重跑产品check/Clippy或聚合。新运行值读取、完整局部账/同Root跨四park责任仍是最小未证门；本次不启第八轮，不改产品/资金/停止合同，不串其他专项。

**root retained-entry第八轮：真实raw/fresh与四park补齐，指定失败门关闭。** `THROTTLE=100 just virt-platform-funding dma-retained`只运行此mode，退出0：[入口](../artifacts/run/20261026-195948-66764-virt-platform-funding.log)、[完整原观察](../artifacts/boot-failure/20261026T115952.260274Z-67134/platform-dma-retained-evidence.json)、[原GDB](../artifacts/boot-failure/20261026T115952.260274Z-67134/platform-dma-retained-gdb.log)、[原客体](../artifacts/boot-failure/20261026T115952.260274Z-67134/platform-dma-retained-qemu.log)、[运行状态](../artifacts/device-closing/root-retained-eighth-state.json)。当前kernel7773d62e…、init384b1d40…未变；Just正常重生成的实际BootPackage为9d347061…（不是开跑前旧8f4baeaf…），实际GDB脚本2c003d19…、工具30fe67e8…均以本轮保存的内容取hash，不混旧产物身份。

实际声明/领域链/allocator104B-8及Pool1 payer通过原严格门，指定一次尚未执行分配返回null；精确error Symbol读到OutOfMemory，客体96行也输出相同错误。原Root0xffffffc08087d4f0/Node5与实际最后Drop的Node地址0xffffffc08087d750独立核验。最后Drop时tree1/mappings0、head/tail fresh均5、成员sealed/finished/dropped、Work Queued；Root票号2/2、维护队列4/4，记录了真实原接管，不以其他Node Work替代。

首个park实际thread4/Gate2，随后thread3、2、1各只到一次；四次各自完整保存同Root快照：head与tail的16字节raw均`01000000000000000500000000000000`，各次fresh解释均Some(Node5)，tree1/mappings0；Work状态raw03/fresh Queued，Root锁票号next=serving=2、maintenance门next=serving=12，原成员三标志均true。五项最终清理owner（同DMA Root、三MMIO Node、一IRQ Node）每次原记录都为Queued，verified=true/failure不存在。此前缺失的首park实际raw/终态状态与后续四park在本轮已补齐；没有锁忙自动接受、等待持锁hart或内核状态改写，不从本轮成功补造第七轮旧raw或宣布其具体读取原因。

本轮原账仍为metadata allocated64061→allocator入口64074→精确error后64010，available2032620→2032607→2032671；pages allocated4/available256225保持，reserved/delegated均0。入口至错误点净退64 metadata槽、相对初始点净退51槽，仅记录局部构造/失败责任回退；含同Root在内的五项清理Work在全局失败停驻后仍Queued，因此不称全根退款、维护已经执行或物理页全退。[最终944项/实际产物与独立清理](../artifacts/device-closing/root-retained-eighth-final-version.json)核执行期无源码/工具差分，运行体收尾后just/QEMU/GDB/cpulimit/helper均无残留，runner退出且私有TCP监听随owner关闭。

该指定retained-entry失败/最后Drop稳定接管/局部账与四park保留门现关闭，node/install不重跑，原第七轮整体失败保留。没有新增产品/ABI/付款机制，不自动第九轮或串其他专项；当前唯一矩阵剩四组，时钟异常单列未定，首批闭包仍不归档/宣告完成。

开发工具链为 `rustc 1.101.0-nightly (19833af72 2026-10-20)`。浮动 nightly 将 `allocator_api` 部分稳定化并改用 `allocator_ext`；本轮同步现存 feature 声明，清理新 lint 揭示的重复 attribute、测试来源多余 Box::pin 与监督数组索引循环，未改变对应对外合同。验证证据随本节记录，不能从旧设计检查推导代码通过。

### 本轮开发验证与结构检查

以下均是当前启动输入接点的开发证据，不是设备闭包的完整验收。2026-10-24 执行；当前改动未提交。

| 验证 | 结果与证据 |
|---|---|
| `just check` | 通过，`artifacts/device-implementation/check.log` |
| `just clippy` | 最终八个分面通过，`artifacts/device-implementation/clippy-final.log`；详细日志仍由 `artifacts/lint/` 保存 |
| shared host `dtb`、`erhino_shared` | 显式 `aarch64-apple-darwin`，55 项测试通过，`artifacts/device-implementation/host.log`；新增反例覆盖固定小端几何、溢出/截断、资源槽重复/总数不符，以及属性数据伪装节点 token |
| capability 既有 signal 测试 | 显式 host target，3 项通过，`artifacts/device-implementation/capability-host.log`；复核本轮移除测试来源多余 Box::pin 后的节点寿命 |
| 既有 core/platform 路线 | debug stress、release core、sifive_u、nofd 均通过；前三项见 `artifacts/device-implementation/acceptance.log`，nofd 续跑见 `nofd.log` |
| 既有 fault 路线 | boot panic/alloc/fatal、IPI failure、VM fault、VM repair 均通过，`artifacts/device-implementation/faults-final.log` |
| 迁移与文档 | 12 个 DT 文件完整迁移，仅 parser 节点查询与对应测试有内容差异，旧目录不存在；新增本地文档链接及 `git diff --check` 通过 |
| 进程清理 | 三个验收启动器均退出，核对无剩余 QEMU/GDB 进程 |

`just acceptance` 原聚合在 nofd 的 DTS 生成前退出，不能把该次命令退出码写成成功：宿主 `/usr/bin/python3` 为 3.9.6，`make-hetero-dts.py` 的 union 注解需延迟求值。本轮加入 `from __future__ import annotations` 后仅续跑未通过路线，不重复已通过的 core/sifive。故障验收另发现 GDB 18.1 的原 breakpoint commands/continue 链在第一个 park 后返回脚本尾部，连续两次只收集一个 hart；改用已有 IPI 验收采用的 Python Breakpoint.stop 观察方式，仍要求四个不同线程全部在 Gate Failed 后抵达 park。最终三类 boot 注入和全部故障路线均实际通过，没有降低判据。原失败日志保留于 `artifacts/run/20261024-025731-11105-virt-nofd.log`、`artifacts/run/20261024-032159-15040-virt-boot-failure.log` 和对应重试记录。

主代理的当前结构检查核对了 DTB 原页 owner/永久 reservation 排除、页内复制上界、root Pool 费用与失败析构、MemoryObject 初始化发生在 Start 前、实际只读 rights 和尾页零填充、通用出生块消费者及旧目录删除。未新增用户内存访问旁路；初始化 helper 仅在 boot 独占合同下经实际 backing 定位页，正常 Map/Unmap 仍走原 VM。描述中的资源索引校验不能替代未来实际控制对象的 kind/rights 检查，该责任已在上表登记。DTB 复制过程中精确 OOM/并发失败注入、特权资源和真实热插尚未验证；既有 boot 注入发生在 boot::load 入口，不能冒充覆盖了新复制阶段的全部失败窗口。

### VM 来源与强制退休基础续接

用户再次明确继续，沿同一首批闭包授权推进；本段没有新增交付范围。以下是代码现状：

- 原 `MapBacking`/`BackingView` 已整体改为 `MappingSource`/`MappingOrigin`，同步纯层、内核 VM 及真实测试消费者，无别名出口。新增 Mmio 来源携 MappingId/offset；完整或部分 Protect/Unmap/切分保留同一身份，MMIO 不取得 MemoryObject 写许可、不能 EXECUTE，也不能承担普通结果写回 lease。
- `MemorySpace` 出生额外付一个强制事务槽，`VmState.dependencies` 按相同实际容量准备，普通业务仍受原 `max_transactions` 限额。`ChangeStorage::prepare_forced_remove` 准备固定计划；`freeze_mapping` 封该来源的新修改/切分/迟到预约，已预约工作可以完成。`reserve_forced_remove` 只接受仍匹配 MappingId 的完整片段，复用既有 Prepared→Committed→Retiring→complete，不走可能因业务配额满而失败的普通 Unmap。
- 强制槽使用内部保留 ChangeKey(0)，普通编号仍从 1 单调发行。槽在上一项 complete 前不复用，避免普通序号耗尽使清理丢失承载；来源 MappingId 仍必须由资源 owner 唯一发行、不复用。强制 Remove 没有可访问的 fault target，不把该槽号当用户身份或已结束 Mapping 的身份。
- 页表 `PublishCursor::map` 显式携 `LeafSize`，普通 RAM 仍选择最大合法叶，MMIO 来源选择 BasePage。原最坏表帧上界已经按每级触及的表计算，基础页同样需要并预付中间表，不将其供给误计为零。单基础页删除可用空表帧供给完成，不因拆大页临时分配。
- 普通 uaccess/初始上下文使用同一实物 Views 判定 RAM，再检查 PTE。来源索引与几何修改/翻译退休的时间关系由上文接线说明拥有；`Views::prepare_map` 拒绝将 RAM BackingUse 配成 Mmio 来源。

本段纯层交付时尚无内核强制 Work/FIFO 或 MmioViewUse；后续已由下方各续接记录接通控制/使用、完整树和真实平台根。本段仅保留当时的纯层证据，当前接点由最新续接记录拥有；PLIC、DMA、管理服务和真实驱动链继续由同一主计划承接，不另开平行任务。

测试依据为正文的隔离与释放合同，新增六项反例：MMIO 执行/结果 lease 拒绝；VA 空洞重用不被旧 MappingId 撤销；业务事务满额仍可强制退休；冻结拒绝迟到预约且保留已接受工作；退休门内不分配、不退款；对齐大区间仍使用基础页并可无新增表帧删除一页。它们复用原 memory_space/page_table host 测试与付款设施，不新增测试服务或虚拟设备。来访实物 View 可以从几何片段内部开始，强制预约按地址定位当前完整片段后核 MappingId；该形状也纳入满额退休用例，避免为失败切界留下的等价细片另建路径。

| 本段验证 | 结果及日志 |
|---|---|
| 纯层 host | 显式 `aarch64-apple-darwin`，memory_space 53 项、page_table 33 项全部通过；`artifacts/device-resources/host.log`，最终定位细化后的 memory_space 补测见 `memory-final.log` |
| 内核与静态门 | `just check`、最终 `just clippy` 八分面通过；`artifacts/device-resources/check.log`、`clippy-final.log` |
| 组合回归 | `just acceptance` 退出 0，包含 stress、release core、sifive_u、nofd、boot/alloc/fatal、IPI、VM fault/repair；`artifacts/device-resources/acceptance.log` |
| 最终来源检查 | 实物 Views 检查在首轮 stress 运行期间替换了几何查询；后续 release/platform/fault 构建已包含该接线，另补最终 debug stress 通过，见 `artifacts/device-resources/stress-final.log` |
| 文档与清理 | 本地链接及 diff 空白检查通过；本次验收进程全部退出，无残留 QEMU/GDB |

本段结构复核检查了来源与权限分离、已接受事务跨冻结推进、专用槽与普通额度隔离、固定存储循环使用、片段身份/偏移和旧 VA 重用、来源生命周期覆盖发布窗口，以及 ADDRESS_SPACE→VM_VIEWS 的现有锁序。普通 RAM 消费者保持实际接通；没有增加公开 MMIO syscall 或让 RAM BackingUse 代持 IO。新增强制退休原语尚未经过内核资源 Work、远端 Revoke 与真实 MMIO 的运行验证，DMA/IRQ 仍未实现；以上回归不能抵消这些接线缺口。本段不归档、不登记设备闭包完成，也不新增固定提交 Review，当前代码仍未提交。

### MMIO 实物责任与预付执行者续接

用户再次要求继续，沿原设备闭包实施，未另行提交。`vm/mapping` 将 RAM 与 MMIO use/split/release 接到同一 View 索引与准备/回滚/退休通路；`vm/mmio` 的稳定记录持根成员门、弱 VM、原操作完成来源、片段回执及 Map 准入时预付的 FIFO 节点。`MmioMapState` 是纯层状态唯一真值，原 Map 操作结束与全部片段责任归还共同决定 Detached，切界准备也计入责任。发布许可在 VM 门外取得，已提交责任不因随后 Stop 丢失。

`ForcedRetirement` 随每个 VmShell 出生预付；FIFO/链首及片段链接都在原 Map/View 准备期取得。非空队列按 MappingId 冻结来源，依赖原 Map 或既有修改完成时真实停驻，逐完整片段复用 `ChangeContext` 的发布、fence、退休与清表协议；强制路径使用内联单游标，完成后取回固定计划存储。没有复制第二套 VM 状态机、临时 Unmap 分配或扫描全空间寻找某个 Mapping。

进程 Frozen 时从 VmShell 摘下常驻执行者、封排队并调度收尾：已提交片段继续完成，未提交项由整空间退休接管。退出租约作为执行者最后字段，晚于其队列、计划、运输和费用退出，避免提前放行 VM 回收/DONE。等待挂钩不会保活退出线程，Map 记录不长期持 VM 强借用；Weak 升级失败不会冒充 Detached。

**当时的外部能力边界**：本段完成时公共 MemoryMap 尚无 MMIO capability，只有根成员状态与预留授权口。后续已接入实际 control/use、任意深祖先、最后 capability 消散和平台根，具体状态见下方最新续接。原 vm/mmio 文件已经迁至 resource/mmio，VM 不再拥有控制角色。

| 未接通入口 / 当前代码位置 | owner、接通及删除条件 | 验证门 |
|---|---|---|
| 原 `MmioMappings::new/seal/next_mapping` 与 `MmioMapping::request_stop` | 已由后续 Node/Control/Use 替换单根专用入口，实际平台根接通，相关未使用注记已删除 | 已有运行结果及剩余精确交错/费用门统一见最新平台续接 |
| `Operation::prepare_mmio` | 后续已接实际 MmioUse 分支，改为只接受节点与准入票的安全内部 API；裸物理 unsafe 只留平台 root 构造 | 正常 Map、输出 fault、Stop 早于/晚于发布许可、用户内存拒绝 MMIO |
| 非空强制队列与实际 MmioViewUse | 后续已由实际 ECAM 来源及正常 test_target 生成成员 | 旧 VA 重用、非空退休与退出组合已有证据；指定跨 hart/冻结交错和费用故障仍不能由普通成功运行替代 |

新增四项纯层生命周期测试依据已确认的发布/退休合同：Stop 早于许可；许可后 Stop 仍完成原提交；迟到切界责任保留至归还；普通 Unmap 与输出失败的终态。普通 RAM 的生产消费者已经迁移到 MappingUse，进程退出实际经过新执行者的封口/末尾退款路径。

施工中对当前工作树进行了一次只读辅助审查（sub-3）；确认两个 P1，均由主代理修复并对照原交错复核：

| 原 finding 与可达交错 | 修正与反例验证 |
|---|---|
| 入队/关闭无条件 publish：Blocked 时 activate panic；Running→fence 停驻窗口置 notified，下一轮可能越过未完成 ack | 用 MmioExecutorState 区分 Idle、Active、普通依赖、Fence、Closed；入队不唤醒队首等待，关闭只用 token 打断普通观察；迟到 hint 不触碰 fence；三处同步后退休检查改为所有构建启用的断言 |
| VM 先 Frozen、stop 尚未安装退出租约时执行者提前 Done，随后 stop 激活终态 | 安装退出租约后才提交 closing；只有 closing 与空队列同门成立才 Done；空队列复检与实际维护通知在同一队列门内提交，排除迟到 activate |

另外四项 host 反例直接组合真实 `maintenance::record::Record`，覆盖 fence 发布前后入队/关闭、普通依赖关闭、Frozen 与退出租约交接间隙、Idle 判定后生产者与终态关闭交错。未另造维护调度模拟器。原 90 项 host 与首次完整 acceptance 是修正前的开发证据；修正后 94 项 host 和 `just check` 已通过，日志为 `artifacts/device-mmio/host-final.log`、`check-final.log`。修正后完整 `just acceptance` 退出 0，八分面 Clippy、debug stress、release core、sifive_u、nofd、boot/alloc/fatal、IPI、VM fault/repair 全部通过，见 `acceptance-final.log` 与 `acceptance-final.exit`；验收进程均已退出，无残留 QEMU/GDB。该审查不是已登记固定设计提交 Review 的执行，也不宣称修正后又经过独立复审。

主代理对照原 finding 复核了队列门内维护通知、Blocked 的票据唤醒、fence 唯一完成者、Frozen 与 closing 的交接点、最终退款顺序和 VM→Views / 资源门外移交。Map/use、实物索引和固定计划由单一 owner 持有；公共旧 RAM 消费者继续使用同一修改引擎。当前仍没有合法 MmioUse 生产非空队列；上表列出的真实 IO、跨 hart Revoke 和非空队列退出运行门没有被 RAM 路线补证。

### 层级资源树续接

存储细化与实施已完成：新增 `os/resource_tree` 纯机制，稳定 NodeId、不可变父关系和直接活子链由单一已付款有序表拥有；祖先准入每步只检查一个节点，最终在同一外部门比较 close_seq。成员责任计数与实际领域记录由同门增减；Seal 与 Revoke 分开，子完成只摘链并返回一次父回执，已完成观察壳不阻塞父完成。PreparedNode 与 RetiredNode 是不同类型，退休节点不能重新插入；节点存储带出门外逐项退款，Drop 要求表已空，不递归析构层级树。该细化保持正文外部语义，未新增另一个任务或测试运行体。

`MmioMappings` 原独立 sealed 标志已删除，其根封口、Preparing 登记、发布许可和 Detached 回执改用 ResourceTree；成员责任在首次 Detached 归还，不等稳定记录最后 Arc 消散。本段当时只接单根；下方续接已删除 `root_admission`，改为实际节点的分步祖先检查。IRQ/DMA 将复用树准入，硬件停止条件仍由各领域 owner 拥有。

| 验证 | 证据与边界 |
|---|---|
| 新增资源树 host 测试 | 显式 `aarch64-apple-darwin`，6 项通过，`artifacts/device-tree/host.log`；覆盖未遍历祖先封口、无关分支令旧票 Busy、序号耗尽仍可清理、跨根票拒绝、子完成修复撤销游标/仅一次父回执、257 层逐步检查和逐节点门外退款 |
| 最终编译与 lint | `just check` 通过，`artifacts/device-mmio/check-tree.log`；`just clippy` 八分面通过，`artifacts/device-tree/clippy.log`。workspace、内核 path 依赖与名称一致；既有 host workspace 入口自动覆盖新库 |
| 运行覆盖边界 | 本段当时的完整 acceptance 先于层级树适配，尚无真实树消费者；后续平台根与运行证据另见最新续接，不把旧运行结果冒充后续验收 |
| 文档与清理 | 新增/修改内容的 32 个本地链接及标题锚点通过，`git diff --check` 通过，无剩余 QEMU/GDB；`artifacts/device-tree/final-check.log` |

主代理对该增量的结构检查覆盖跨根票据不可混用、封口后旧票失效、父身份不可改、成员回执与记录存储寿命分离、游标遇子项自行完成、退休节点不能重新插入，以及门内不分配/退款。节点查表沿 AVL 高度界定，祖先链每步一项，清理没有递归层级 owner；领域实际成员仍由同门内的唯一记录决定，不以计数替代实际停止条件。只有单根 Map 账本被接入，实际子能力、后台清理和返回续体的缺口继续由下段承接，不记为已交付。

续接位置经主代理亲读核定，explorer 仅辅助定位：`KernelObject`/`Holders(Terminal)` 已提供最后持有回调；`handle::install` 已提供 reserve→同步用户结果写回→commit/rollback；`MemoryMap` 的准备目前在 syscall 栈上的 Operation 中完成，只有提交/回滚进入 ChangeWork。`WaitPlan::completion` 只交付单位成功值，**没有跨调度轮次保存祖先准备并交付失败/新句柄/原 Map 续体的现成路径**。不能把 `prepare/finish` 内的同步循环叫作跨轮次祖先检查，也不能在其中无界遍历父链。

该接点由下节继续实施；授权仍覆盖原首批闭包。当前下一步与未闭合责任以下节为准，不再从单根适配开始。

### 分步请求与 MMIO 控制使用续接

用户继续授权后，主代理亲读 syscall、等待/执行资格、VM 准入、SUM、capability 安装/最后持有与现有资源树，确定并实施下列内部接线：

- 请求由原 Thread 的唯一执行资格持有，按具体载荷 Layout 支付 Box，不创建内核线程或独立用户指针写入 Work。每次调度仍经过 Shell/VM enter，再以目标 satp 执行一个有界步骤；恢复 kernel root + SFENCE 后才 leave 和排队/等待。只有请求完成才推进原 ecall。该选择保留原 SUM 访问合同，避免把内核 root 下的普通 WaitPlan 续体当作可直接写用户地址的上下文；固定 RISC-V ISA v20250508 supervisor 的 satp/SFENCE 合同已经核对。
- 未提交准备由请求 Drop 归还；已有 Operation 在 Drop 时将全部准备移交出生预付的回滚 Work。提交后由既有 ChangeWork 和独立完成来源推进，不要求原线程继续运行。普通 RAM MemoryMap 也走该真实请求入口，用现有生产消费者核对切换、输出与退出；没有另造验收请求。
- MMIO 成员与授权从 `vm/mmio.rs` 整体迁至 `resource/mmio.rs` 及 node/object 模块，没有重导出旧路径。实际 MmioControl/MmioUse 使用 Terminal Holders；最后 capability 消散提交自身节点撤销。control 的 CREATE 派生单页归一的子 control/use，rights 同时受来源实际 rights 与目标类型上限约束，任何 MMIO 对象均无 EXECUTE。
- 派生在准备期预付节点/Work/观察源和排他区间项，每轮检查一个祖先，最终同资源门比较 close_seq 并链接。直接活子区间按前驱/后继拒绝重叠，直到子项真实完成才摘除/退款；父子包含和同 use 的别名/多次 Map 不混同独立范围授权。节点链接后句柄安装失败经最后持有路径撤销，不反向拆已提交责任。
- MMIO Map 分为 Start、Admit、Publish：第一次祖先检查完成后登记 Preparing 并准备既有 VM 事务，第二次检查完成后同门 grant，再由原线程写 payload/cookie 与必成提交。Busy 不在内核重试；任意阶段失败或退出仍归原 Operation。节点的清理 Work 每轮通知一个直接 child 或 Map；游标结束仍等真实回执，arm→复检停驻，终态关闭通知槽后才 Done。
- shared 新增 MmioControl/MmioUse kind 和 CreateControl/CreateUse/Query/Seal/Revoke 调用，kernel 与 libkernel 同步。libkernel 的 MmioRegion 不自动 Unmap；显式 Unmap 和 volatile 访问要求驱动证明撤销排除与地址未重用，防止安全析构或迟到访问命中复用 VA。独立来源及页表/退休机制未复制。

`Operation::prepare_mmio` 已改成节点+准入票的安全内部接口，原单根专用入口和相应未使用注记已删除。本段结束时唯一未接入口为平台 root 构造；下方续接已由 board/Bootstrap 接通并删除该注记。裸物理 unsafe 仍要求实际平台校验，不开放用户自铸 authority。

本轮编译发现自动 trait 求解的新类型图超过默认递归深度，编译器预算调至 256，未以 unsafe auto-trait 实现代替求解。首次压力路线在客体启动前被 ELF 审计拒绝：MapCall::advance debug 单帧 0x3530 超过 0x3000。拆成 start/admit/publish/complete，各阶段共用请求内唯一 Operation；没有增加栈或 guard。最终 `just build_kernel` 通过，全 ELF 最大帧 0x2970，见 `artifacts/device-request/build-stack.log`；原失败保留 `artifacts/run/20261024-154713-10264-virt-stress.log`。这不是超时或客体故障，不调整运行期限。

一次新上下文辅助 reviewer（sub-5）只读检查本轮请求、MMIO 节点/成员、派生/最后 Close、通知停驻、发布和 libkernel VA 重用边界，按阶段拆分后的当前代码未发现有充分证据的新 finding。未运行真实 MMIO；该结果不替代平台接线后的运行门，也不是已登记固定设计提交 Review 的完成。主代理继续承担实现与交付前复核。

主代理的结构复核核对了请求槽不跨 yield 持 BodyLease、临时目标 satp 只覆盖持 RunPermit 的有界步骤、归一先于 leave、输出 fault 的执行 owner、请求退出先于 Departure、Control/Use 实际类型与 rights 子集、准备失败和安装失败的不同 owner、排他区间保留到子完成、Node 通知不激活 Blocked/Done、成员/父回执只交一次、资源锁外 VM/Work/付款移交，以及旧 vm/mmio 和单根 helper 的删除。Map 的写回/提交只保留一处，节点树和实际成员链各拥有不同事实，没有新增第二套 PTE/退休状态机。上述结构检查不替代尚未运行的真实 MMIO 交错；本轮 ELF 证据是单帧审计，不重新签发旧固定 ELF 的 M 累计栈证书。

本轮开发证据：`just check`、阶段拆分后八分面 `just clippy` 通过，分别为 `artifacts/device-request/check-final.log`、`clippy-after-stack.log`；memory_space/page_table/resource_tree 显式 host 共 100 项通过，libkernel 6 项单测和 2 项 compile-fail doc-test 通过，见 `host.log`、`libkernel-host.log`。请求引擎早期普通 RAM debug core 已通过（`virt.log`）；最终依次执行 `just virt-stress`、`just acceptance-virt`、`just acceptance-platforms`、`just acceptance-faults`，全部通过且顺序启动器退出 0，见 `routes-final.log`、`routes-final.exit`。该组合覆盖 debug stress、release core、sifive_u、nofd、boot panic/alloc/fatal、IPI、VM fault/repair；已通过的 Clippy 不在此组合重跑。新增/修改文档 40 个本地链接及标题锚点通过，`git diff --check` 通过；验收进程已退出且无 QEMU/GDB 残留，见 `docs-check.log`、`process-check.log`。

当时的下一接点已由下节实施；当前继续原设备闭包第一个依赖切面的 IRQ/DMA 与剩余验证门，不重做已接通的 MMIO 平台输入。

### 平台 MMIO 根与真实消费者续接

本轮沿既有授权接通实际来源。`shared/dtb::pci` 只解析 ECAM/bus-range/ranges 事实，保留 PCI bus 与 CPU 两套地址、PIO/memory32/memory64 和 prefetchable；完整 tuple、cell 宽度、总线覆盖、非零长度及溢出均检查。`board::io` 在零堆启动期识别受支持且启用的 QEMU virt PCI host，核父 simple-bus 恒等翻译及固定机器窗口，排除全部 RAM、永久 reservation、私有 IO 和根间重叠。PIO 只验证描述、不铸 MMIO 根；非支持平台保持零根，已识别但不支持的启用布局拒绝启动。固定三槽由 ECAM、mem32、mem64 的硬件形状决定，不作为通用设备数上限。

固定事实补证见[PCI 平台准入取证](ref-2026-10-23-device-platform-contracts.md#2026-10-24pci-平台准入取证)。QEMU RV64 高窗口大小恒为 16 GiB，base 为 RAM 顶向 16 GiB 对齐；本仓 1 GiB 配置对应 `0x400000000 + 0x400000000`。当前 profile 不接受大 RAM 引起的窗口移动，也不由 dma-ranges 缺省推导通用恒等 DMA；后者事实只来自固定 QEMU 无 IOMMU 实现。当时尚未发行 DMA authority，后续接线见 DMA 续接。

main 将 IoRoots 与 DTB owner 一起移交 boot。Bootstrap 以真实记录数构造头、关联项和 cap 表，先准备完整平台输入，再 Start init；铸根/安装失败释放已铸 cap 后沿原失败 owner 收束。三个 MMIO 根使用 root Pool，输出只记录 kind/node offset/handle index，CPU 几何仍由 control Query 拥有。根构造未使用注记已删除。libprocess 的启动 role 白名单补入 MmioControl/MmioUse；init 逐项采用 typed root，核真实 kind/完整 rights、初始状态、DT CPU 几何和重复根，保存在启动输入中。

真实验证复用原 init VM 剧本和 test_target，没有假 IO、测试服务或新运行体：

- 在实际 ECAM 只读 PCI host 配置 dword，观察 `0x00081b36`；不写 BAR、解码、BME 或设备寄存器。
- 创建子 control/use，拒绝同页范围重叠；使用真实 IO 作为普通 Map 结果缓冲被拒绝，RX 映射被拒绝。部分 Unmap 后将中间页改映 RAM，父 Seal 拒绝新 Map，Revoke/DONE 后新 RAM 内容保留，旧 IO VA 可重新映入，完成后原物理范围可再次派生。
- 通过正式 startup Grant 给 test_target 真实 MmioUse；子进程建立两份各 32 页的 IO View。一种模式在进程继续存活时关闭最后 use，父在不先 Revoke 的情况下复用原范围；另一种模式保留 View，父接续提交 Revoke 和 ProcessKill，并等待两种实际 DONE。该调用序列覆盖调度竞争，不宣称已经精确观测某个 Frozen/发布许可交错或特定远端 hart ack。
- libkernel 增加只返回几何的 range 与 unsafe 部分 Unmap，继续要求调用者排除撤销/VA 重用。验收脚本对具有真实资源根的完整 workload 强制要求两条 MMIO 成功锚点，零根平台和独立 memory workload 不伪造这些锚点。

辅助 reviewer sub-7 对本轮平台/发行/采用代码未发现新的充分证据授权缺陷，但确认一项 P2 验收误判：只看 Dead 会把子进程故障退出当作最后 Close 自主退休。主代理已按原反例修正，在父方 Revoke/Kill 前要求 Running/None/零 detail，在 DONE 后要求 Dead/Killed/PROCESS_CONTROL_KILL；意外 Faulted/Aborted 不再可能打印通过。原 full routes 结果属于修正前证据，修正后补测单独记录，不宣称已独立复审。

开发证据：shared dtb/erhino_shared 显式 host 共 60 项通过（新增 5 个 PCI 几何反例）；libprocess/libkernel 共 11 项单测和 2 项 compile-fail doc-test 通过（新增 startup MMIO role 采用测试），见 `artifacts/device-platform/host-final.log`、`user-host.log`。最终 `just check` 与 P2 修正后八分面 Clippy 通过，见 `check-final.log`、`clippy-reviewed.log`。首轮真实 MMIO debug core 通过，随后完整 stress/release core/sifive_u/nofd/boot/alloc/fatal/IPI/VM fault/repair 顺序回归退出 0，见 `virt.log`、`routes.log`、`routes.exit`。补强终因断言后的 stress/release core 也通过，启动器退出 0，见 `review-check.log`、`review-check.exit`；生产机制未再改动，不重复其他已经通过的路线。

主代理结构检查核对了平台事实与 authority 分离、PIO/CPU/bus 地址区分、支持形状与排除范围、单次铸根、页内描述写入与 DTB 原页寿命、cap Vec 全部失败释放、StartupContext role 白名单和 init 单次采用、后继 Grant 运输、测试中不写设备寄存器、几何 token 在撤销后不再 Unmap，以及目标存活/终因断言对原 P2 反例的排除。新增/修改文档的 47 个本地链接及标题锚点、`git diff --check` 通过；本轮验收启动器全部退出，没有 QEMU/GDB 残留，见 `artifacts/device-platform/final-check.log`。

未闭合责任继续由本篇承接：精确许可两侧 Revoke、业务配额耗尽、非空队列与 Frozen 的指定交错、root 铸造/安装中途资金拒绝和 OOM、资源 metadata 逐项退款计量仍未由本轮运行覆盖；既有 boot 注入在 boot::load 入口，不能冒充上述窗口。当前 MMIO 基础已有真实消费者证据，仍不是设备体系闭包完成。当时的 PLIC/IRQ 下一接点已由下节接续；当前下一步与验证门以下节为准。本篇不归档，未提交或 push。

### PLIC/IRQ 平台与机制续接

沿既有授权继续第一依赖切面。实现细化：IRQ control 持连续 source 范围和允许 target 位图；target bit 表示本次启动冻结的 admitted hart slot，由 DT `interrupts-extended` 经 CPU interrupt-controller phandle/hartid 对应到实际 PLIC context。子 control 缩小范围，use 恰含一源和一个 target；同层 source 范围排他，不以不同 target 绕过同源独占。PCI 根仅含经完整 interrupt-map 核对的四条 INTx，不交付 PLIC 寄存器或其他平台源。

共享 ABI 的 token 是非零单调 use generation 与 claim sequence；Query 提供固定 generation，Take 在结果寄存器返回 sequence，libkernel 组合完整 token。这样 Take 不在消费后写用户缓冲，避免输出 fault 遗失唯一 Ack 凭证。Ack 接受完整 token，别名共享消费状态。事件纯状态与序号耗尽由 `os/plic` 拥有，硬件寄存器/短锁、观察链和实际 source 槽由 kernel controller 拥有。

子节点先在树门链接，再在 PLIC 门登记/启源；撤销在同一 PLIC 门关闭尚未登记的 Line，因此先行 Revoke 可以阻止迟到 activate。已领取事件先 complete 再 disable；关闭后摘除全局源槽，实际 Arc 与提示均在锁外处理。观察直接复用 WaitQueue，状态与来源电平同 PLIC 锁提交，避免锁外晚到 READABLE 覆盖 CLOSED。树清理保持每步一个直接子项，全部回执后 DONE。

主代理检查发现并修复创建失败清理：ResourceTree 要求空树析构，既有 MMIO root 在 Arc 分配或后续 Node 分配失败时可能留下初始空根并触发断言。本轮同步改为先取得未初始化 Arc 存储再装树，后续 Node 失败时封闭/摘除唯一未发布根，门外退款；没有将该 failure 当作硬件成功。精确资金/分配故障注入仍属于本篇未闭合门。

首次 QEMU 在 board 阶段拒绝 PLIC description。静态 fixture 与实际固件输入不同：通过现有 ELF/QEMU/GDB 只读断点抓取 `artifacts/device-irq/runtime.dtb`，确认 OpenSBI 1.8.1 把 M context 的 interrupt 11 改成 `0xffffffff`，并未移除该 context。parser 接受固件禁用标记但保持位置，board 只授 interrupt 9 对应的 S context；新增反例要求 supervisor indices 仍为 `[1,3,5,7]`。失败为 panic 后主动收割，不是撞限，未修改期限；原证据 `artifacts/failed-acceptance-20261024-184719-74388.log`，诊断脚本及日志在 `artifacts/device-irq/dt-probe*`，进程/socket 已清理。

辅助 reviewer sub-10 确认一项 P2：Line.close 在硬件禁源后就发布 DONE/终态，而父树 allocated 位尚未释放；另一 hart 等 DONE 后立即复用 source 会被旧占用拒绝，Query 也可能还未显示 DONE。主代理已修正为硬件关闭只发布 CLOSED 非终态提示，树 finish/释放占用后再发布 DONE 并关闭观察来源；序号耗尽同样不冒充树回执完成。对照原交错复核，新 use 最早在旧 source 槽摘除且父范围释放后准入，旧 Line 的最终通知只操作自身观察链，不再触碰新 source。没有把修正后代码称为再次独立审查通过。

init 的现有 core/stress workload 已接真实 IRQ 根：范围缩减/同源排他、空 Take/伪造 Ack、Seal 保留旧 use、Revoke/DONE/旧 token 拒绝、WaitSet 注册/取消/完成、观察壳保留时 source 重用、最后 use Close 后由观察者等待完成再重用。未另建服务、设备或运行体。这些调用实际操作 PLIC enable/disable，但不产生或伪造硬件 IRQ；真实 claim/Take/Ack、all-idle 唤醒、持续中断与指定交错仍须由后续 xHCI 路线补证。修正固件 sentinel 后首次 debug core 已通过（`virt-2.log`、`virt-2.exit`），该次构建早于 P2 终态修正，不能替代最终回归。

主代理进一步核对 researcher sub-9 提供的固定 QEMU 源码：enable 写不调用输出更新，若在 enable 前写 priority，启用前已 pending 的源可能不唤醒目标。已改为 source 槽登记→enable→priority=1；停止仍为同锁 complete→disable，再 priority=0。没有为模拟器另建 IRQ 队列或循环排空；该顺序也符合硬件合同，寄存器访问使用显式 I/O fence。source 数量的口径同步校正为 ndev=95、内部 num-sources=96 含保留 0。外部事实由[PLIC 与固件交接取证](ref-2026-10-23-device-platform-contracts.md#2026-10-24plic-与固件交接取证)拥有；实际 pending/热插场景仍不以源码取证冒充运行证据。

结构简化移除 IrqUse 不使用的独立 SignalSource，只保留 Line 内嵌的同锁观察链；control 仍有独立 DONE 来源。主代理核对了 actual source/context 权限、同源互斥、逐祖先 close_seq、commit→activate 与先行关闭、代次/序号不复用、complete-before-disable、锁外退款/提示、弱观察目标、最后 capability 与进程退出、树回执先于公开 DONE、无递归父子 owner，以及公共调用/封装/启动角色同步。没有新增裸物理 syscall 或测试运行体。

本段开发验证于 2026-10-24 完成，当前改动未提交：

| 验证 | 结果与实际边界 |
|---|---|
| shared host | dtb/erhino_shared 65 项通过；新增 5 个 PLIC 解析反例覆盖真实 context/路由、顺序变化、错误父/重复路由、未知 phandle 和固件禁用项。显式 `aarch64-apple-darwin`，`artifacts/device-irq/host-shared-final.log` |
| os host | plic/resource_tree 共 9 项通过，新增 3 个消费状态反例覆盖 Take/Ack 唯一性、关闭/代次隔离和序号耗尽；显式 host，`host-os.log` |
| user host | libkernel/libprocess 共 11 项单测、2 项 compile-fail doc-test 通过；显式 host，`host-user.log` |
| 最终静态检查 | `just check`、八分面 `just clippy` 通过，最终 PLIC 顺序版见 `plic-order.log`；新增 Rust 格式、脚本语法、`git diff --check` 通过 |
| 完整路线回归 | P2 修正后的 stress、release core、sifive_u、nofd、boot panic/alloc/fatal、IPI failure、VM fault/repair 依次通过，`routes.log` 保存每条路线入口/结果；该组合不是最终 priority 顺序的全量重跑 |
| 最后增量验证 | 移除 use 多余完成来源后的 debug stress 通过，`final-validation.log`；priority 顺序修正后 debug/release core 均通过，`plic-order.log`、`plic-order.exit=0`。已通过且不受最后寄存器顺序影响的平台/故障路线未重复运行 |
| 文档与清理 | 121 个本地 Markdown 链接及标题锚点通过；本轮诊断/验收 QEMU、GDB 全部退出，临时 socket 已清理，无暂存文件；`final-check.log` |

完整 virt workload 的脚本要求 IRQ 成功锚点；memory 专项和零根平台不伪造该锚点。以上运行证明真实 PLIC 寄存器入口、角色/付款/等待/收束与已有系统工作可以组合，不证明真实 claim/in-service 硬件交错、DMA 安全或完整设备能力。构建自带的 ELF 单帧审计通过，不重新签发维护累计栈证明。

当时的 DMA/boot 下一接点已由下节接续，当前接力以下节为准。IRQ 的 all-idle 插入、真实 in-service 的 Take/Ack 与 Revoke、控制者/线程退出时未确认事件，以及持续 IRQ 下普通工作服务机会，随实际 PCI/xHCI 消费者接通验证；精确 MMIO/费用故障门继续由本篇前述条目拥有，不视为已经关闭。本篇仍为施工记录，不归档，不登记设备能力完成。

### DMA 稳定使用、退休与根终态续接

按既有授权继续。亲读确认 `SegmentMap::acquire` 会扫描全部覆盖段，不能直接把大 Pin 包成一个同步调用。新增内存域的分步取得游标：在有效 MemoryObject 授权门内取得短期 backing 准入责任，保持完整域 root use 不被最终退休推进；每次只给一个实际段增加引用。全部完成交付一份 BackingUse；失败/退出将已经取得的前缀交同一 Pin 的预付 Work 回滚，再归还准入责任。Backing 最终 Work 在存在这种准入责任时保持 Idle，最后归还者重新发布，不忙轮询、不持进程/VM。设备写许可仍通过 MemoryObject 原状态锁的 reserve_writes(1)，独立于 CPU View。

Pin 的准备使用原线程 Pending，每轮只推进一个祖先或一个 backing 段；准备期间退出由预付 mapping Work 接管，地址尚未交付时可安全取消。最终只安装 mapping capability，设备地址由另一次有界 ReadSegments 取得。该次地址交付先在资源门核祖先并标 MayBeExposed、取得短期读取计数，再锁外按实际 BackingUse 枚举/写出；Retire 等该计数清零才归还写许可和 use，输出 fault 不退回未暴露状态。

所有已登记 mapping 的实际强根放在内核专属 RETAINED 表；所有尚有活责任的域节点由 NODES 表强持。两种条目均在出生时预付，物理/层级完成后摘除并退款。域内表只记录 Weak、状态和成员链接，因此没有 Root↔Mapping/Node 强引用环。最后 handle/进程消散不能释放 MayBeExposed 的真实 use；没有退休动作的静态保留不发布 runnable Work。准入、完成与退款各自只有一个 owner。

上级 Retire 必须在域封口后仍能按不可变祖先关系授权。DMA 域表因此分别记录活责任子链接和存储子引用：活子完成即回执，已完成父记录只在所有后代观察/映射身份也释放后才能摘除。节点 Drop 只标记自身存储引用消散，根内嵌回收 Work 每步摘一项并递减直接父存储引用，不递归析构、不复制每条完整祖先路径。这个保留只维持授权身份与付款，不阻止域在实际映射全部退休后 DONE。

上述机制已落地到 `os/dma`、`resource/dma`、MemoryObjectCore/Backing、shared 和 libkernel，原有 CPU VM 仍走同一 backing/许可 owner。MemoryObjectSnapshot 的 write_views 已同步改为 write_permits，准确包含 CPU/DMA 独立写者；所有实际消费者和布局断言一起更新，没有别名出口。virt 启动交付第五个 DMA 根，init 核模式、一致性和初始状态；通用 startup role 表同步接三类 DMA capability。

本轮辅助审查 sub-12 记录两类 P1，均已修正：非空 DMA 祖先不能只靠 Idle Work 保活，主代理在审查期间加入 NODES 稳定 owner，该变更获审查当前源码复核；Backing acquisition 的最后归还若在锁外延迟 publish，可能激活已 Done 的 root Work，主代理将首次 root 发布、最后 acquisition 通知与终态统一到 BACKING 门，门铃仍锁外。对照原交错复核，不存在读出 wake 后被另一 hart 结束再迟到 activate 的窗口。未宣称第二项修正后另做了独立复审。

`boot/watch.rs::InitialWatch` 在初始图准入时预付，只持 Shell 独立来源。Bootstrap 在 ProcessStart 前 attach/publish，装载完成后独立停驻等待真实 DONE；hint 仅唤醒，Work 在锁外确认终态后提交 runtime_stop。init 最终 reset 被拒绝时保留诊断并自终止，原 idle_forever 路径已删除。这个终局不遍历设备、不冒充 DMA 静止，未改变正常成功 reset 不返回的叶合同。

真实消费者复用 init/test_target：正常 Pin/分页 SG、CPU Unmap 后独立写许可仍阻止 Seal、拒绝新 DMA writer、不同 use 的 Retire 拒绝、上级及中间 control 消散后的重复 Retire、未暴露最后 Close 取消；子进程分别携未暴露/已暴露 mapping 被父 Kill，以及在 ReadSegments 地址输出处发生精确 UserAccess fault。前两种正常 Kill 均核 Running/None→Dead/Killed/PROCESS_CONTROL_KILL，输出 fault 核 Faulted/UserAccess/地址 `u64::MAX`，意外退出不能算通过。两份已暴露 mapping 各保留两页和一份写许可，刻意留至本次隔离 QEMU 复位；它们没有被提交给任何设备，不要求故障路径退款。

新增 `just virt-dma-retention` 使用现有真实 core 消费者和 QEMU/GDB 设施，在最后 SBI reset ecall 返回 NotSupported；初始进程实际 DONE 触发 boot observer。核对 reset 拒绝前、初始 DONE、四 hart 全停驻后三处同一组 mapping id/backing/range：两份、共四页、两份设备写许可保持，mapping Work 全为 Idle，原 MemoryObject root release 已结束、无 acquisition/release 游标。首次脚本只匹配类型名而未覆盖 GDB 的 Rust impl 模块栈帧名，终局实际触发但脚本拒绝证据；修正名称识别后全部断言通过，未放宽保留或停驻判据。该路线加入 acceptance-faults，使用完整独立日志/ELF hash并负责进程清理。

主代理交付前结构检查覆盖源授权与方向、root 准入责任、每段取得/取消前缀、写许可与 Seal、同门地址暴露/reader 屏障、输出 fault、静态保留/实际强根/付款、深祖先身份回收、临界区与维护发布边、最后 capability/VM/进程退出、初始观察预付与 attach-before-Start、reset 拒绝及旧 idle 路径删除。没有复制物理库存、W^X 状态、VM 退休或另建测试服务；新的保留表只拥有真实领域存活责任。

本段开发验证于 2026-10-24 完成，证据均在本次工作树，未提交：

| 验证 | 结果与日志 |
|---|---|
| 内存/DMA/树 host | 显式 `aarch64-apple-darwin`，dma/memory_space/resource_tree 共 72 项通过；新增 3 项暴露/读者屏障/取消状态反例和 2 项分步引用/取消前缀守恒反例。`artifacts/device-dma/host-os.log` |
| ABI 与用户库 host | erhino_shared 30 项、libkernel/libprocess 11 项、2 项 compile-fail doc-test 通过；字段改名后的全部消费者已编译，`host-shared.log`、`host-user.log` |
| 最终检查 | `just check`、八分面 Clippy 通过，`final-validation.log`；原中间编译错误保存在 `check.log`，已按现有 Buffer 切片接口修正 |
| 普通运行与完整回归 | 首轮 debug core 通过，`virt.log/virt.exit=0`；接进程场景后 stress、release core、sifive_u、nofd、boot/alloc/fatal、IPI、VM fault/repair 顺序通过，`routes.log/routes.exit=0` |
| 根故障终局 | `just virt-dma-retention` 最终通过，`final-validation.exit=0`；固定证据 `artifacts/dma-retention/20261024T121044.876817Z-34576/{gdb.log,qemu.log,evidence.json}`，三处保留快照与四 hart 停驻均齐全。首轮脚本拒绝保留在 `20261024T120748.388741Z-33529/` |
| reset 拒绝的真实平台 | 自终止政策接入后的 sifive_u core 补测通过，`sifive-final.log/sifive-final.exit=0`；不由 virt 调试注入替代平台拒绝路径 |
| 文档与清理 | 198 个本地 Markdown 链接/标题锚点、diff 空白、脚本语法和 Justfile 解析通过；QEMU/GDB 无残留，专项 Python 编译缓存已清理，无暂存文件。`final-check.log` |

完整回归后仅追加 reset 失败终局、共享字段语义命名及相应专项/平台检查，没有无理由重复已通过的普通路线。GDB 停顿不作为调度公平性或性能证据，单帧 ELF 审计也不替代维护累计栈证明。

第 2/3 个依赖切面已由下节开始接续，当前断点以下节为准。资源机制的精确 Pin/Seal、reader/Retire、准备中退出及付款/OOM 交错，以及 DMA 保留与 IPI 传播失败的同次组合，还需补运行证据；先前 MMIO/IRQ 未闭合门仍在本篇，真实 IRQ/DMA、设备 halt/故障/Cold reboot 与热插必须由实际驱动路线覆盖。本篇保持施工态，不归档、不宣称首批设备闭包完成。

### 设备领域机制续接

用户要求文档、计划随施工保持更新后继续。核对发现装配计划仍把设备权限/供应责任标作设计中，并要求重复申请代码授权；已同步为已授权施工及真实五根、boot DONE 的当前事实。设备主计划中“后续授权后实施”和历史准入段的未实现列表也分别改为当前依赖与明确历史事实；未把通用装配或设备管理服务登记为已完成。

当前实施 `user/libraries/libdevice`，独占设备身份、登记序号、Claim 席位、Provision 轮次和冻结资源表。DeviceId/BindingRef 使用定宽小端编码，数值不授予权限；Seat 持唯一当前占用，原主体可在来源封口/Withdraw 后恢复原控制，旧控制不能切换到新 epoch。Withdraw 后不再发行未来轮，最后一轮可以收束但不回绕。Provision 接受实际发行者跳过空闲取消轮后的 epoch，Stop 可先封原轮；Failed/ConfirmStopped 都不能代替实际 Closed。

Publications 保存每个认证 Registrar context 的高水位与仍存活关系，预备节点和领域费用先准入，commit 再核来源身份/封口/seq；取消先到时拒绝迟到候选而不复活登记，Quota 不消费 seq。Seal 即时禁止准入，逐项清理另由原 owner 执行。Collecting 在 Claim 前预留整张表的项与文本容量，逐项校验名字、固定导出描述及总字节，收齐才可得到不可变 Frozen；类型声明须与实际 HandleDescribe 核对，母本保留复制/运输权并不要求把这些权利授给驱动。资源表总项数不受单条消息 handle 数限制，单项 wire 恰配一个 capability，消息大小从实际 RPC/外层头推导。

本阶段只完成基础记录和状态机制，授权/登记 backing 已由下节继续落实；它们尚不是 srv_device 服务，真实稳定准备/停止任务、RPC 方法、设备 Watch/OAL、实际管理进程装配仍未接通。库不自开运行体；后续由 srv_device 的同一本地提交同时安装记录/预付任务并调用这些机制，不能先回复 Claim 再补清理 owner。实现说明同步在 [device](../notes/impls/device.md#用户态设备领域机制)。

结构检查明确了两处责任边界：commit_claim 返回 Created/Existing，只有 Created 能移交新预付任务，重复请求须退新候选并复用原 owner；Withdraw 后不再推进到一个未实际发行的新 epoch。辅助只读审查 sub-14 发现一项 P2：Collecting 只核领域文本额度，可能冻结无法塞入单项 RPC 的表项。已将实际回复头加入 reserve/push 的运输校验，并加入“领域额度足够但运输超界拒绝、恰好填满运输载荷接受”的反例。主代理对照反例修复和复核，未宣称修复后再做独立复审。

验证为本轮领域基础机制的开发证据：

| 检查 | 结果与边界 |
|---|---|
| `cargo test -p libdevice --target aarch64-apple-darwin` | 20 项通过，覆盖旧轮/同异主体、空闲取消、Withdraw、耗尽、供应跳轮/先停后备、登记撤出与跨来源候选、额度/费用、部分表/运输上界及 wire；`artifacts/device-domain/host-final.log`、`host-final.exit=0` |
| `just check` | 通过，`artifacts/device-domain/check.log`、`check.exit=0`；这是 os 检查，不单独充当用户库编译证据 |
| `just clippy` | 八分面通过，`artifacts/device-domain/clippy-final.log`、`clippy-final.exit=0`；host 清单加入 libdevice，user-target 显式含 `--lib --bins`，实际编译尚未被程序链接的 no_std 库 |
| 中间检查 | 首轮 Clippy 对完整返还残表的 Result 大错误值报 lint；已用带理由的 expect 保持原 capability/存储/费用 owner，不为错误路径再分配 Box。原输出保存于 `clippy-user-host-first.log` |
| 文档与工作树 | 5 份本轮文档的 70 个本地链接/标题锚点及代码围栏通过，`docs-check.log/docs-check.exit=0`；`git diff --check` 与 Justfile 解析通过。未启动 QEMU/GDB 或临时服务，无暂存改动 |

主代理结构检查覆盖权限与数字身份边界、当前占用单一真值、候选/提交区分、清理不借新业务配额、记录和实际能力的析构/费用顺序、wire 不直接使用 Rust 布局，以及无运行体/无具体驱动反向依赖。基础机制的实际消费者仍待服务接线，因此本次不运行无法触及新代码的 QEMU；前一轮内核运行结果不冒充本轮服务证据。

该接点已由下节继续实施，当前断点以下节为准。已核现成接点：`librpc::RequestContext/Outbox/Dispatcher`、`libservice::RoleTable` 的来源事实、`libexecution::Runtime` 和 `libprocess::spawn`/init RootSupervisor 的预付监督；新 bootstrap 要按装配合同核独占 context，不能只复制旧 fs 接点的 PID 比较。子进程出生 grants 容量、Dispatcher 每调用两来源和原注册控制退休均沿实际基座计费，不新增另一套运行框架。

原 MMIO/IRQ/DMA 精确交错与硬件消费者缺口继续保留。当前没有 srv_device/srv_driver 可执行服务、真实驱动或设备 workload，不登记设备领域闭包或系统能力已完成；本篇不归档，代码未提交。

### 设备登记与授权 backing 续接

继续既有授权。服务端 backing 使用单 actor 独占的来源/实例/绑定表；transport 的 sender_context_id 只用于查本地已发行角色，不从 payload、PID 或名字推导权限。每个来源冻结范围、权利与同一付款账户，派生只能收窄；来源管理入口指向原来源，封口不删除旧停止/观察权限。实例父关系不可变，来源封口与祖先 Withdraw 的准入在操作提交点同步复核，后台预算遍历只通知清理。来源、实例祖先和旧绑定的身份记录必须保留至引用它们的控制、任务及派生关系全部退休，不能删掉路径后再试图从新登记重建旧权限。

登记节点、出版序号关系和绑定清理记录都先预付，提交只链接；原 Claim 请求须已进入实际 Runtime，提交时记录该稳定任务身份，回复失败不移交或取消责任。重复 Claim 的 Created/Existing 结果决定是否接受新候选。资源表、功能根和激活观察入口分别由原 Binding 持有；Preparing/Stopping 期间不导出表项或功能，释放必须等待真实 Provision Closed 及本轮在途责任收束。服务端实现不得另建消息泵、私有调度器或把纯状态库当成已经运行的服务。

当前代码已新增 `libdevice::registry::Registry<C>`，C 承载实际 owned Provision payload；来源/管理入口索引、不可变拓扑、登记高水位与实例表、缩权派生及祖先准入共同在单 actor backing 内复核。SourceAdmin context 不授予 Claim/登记；观察快照不含实际 Provision。登记的两份预付节点同门提交，重复候选返回原 ID 与 Existing，不产生第二份实例或任务。名字/identity 动态存储在分配前准入，额外容量补齐真实费用，失败不消费登记序号。

Created Claim 同步保存原稳定任务号并持来源/设备身份引用，重复请求不替换任务号；真正的 Runtime TaskSubmission、BindingControl/Use Lifetime 和在途 RPC 仍待服务宿主接入，当前不能用非零任务号冒充执行准入。来源封口与祖先撤出即时挡新准入，逐项 maintenance 返回原绑定或登记清理任务号；没有另造执行器。来源树/设备树通过数字身份与计数保留，scope/子实例/活动绑定及旧控制未退休时拒绝删除祖先。

辅助只读审查 sub-15 发现一项 P2：祖先已撤出但维护尚未扫描到后代时，空闲 Stop 或活动绑定 Release 可能推进后代 Seat 的 epoch。主代理将有效撤出同步集中为 synchronize_withdrawal，由 StopClaim、release_binding 和维护共同使用；同步覆盖祖先撤出/登记来源封口，单独封申请来源则不撤出设备。新增两个反例分别覆盖扫描前空闲 Stop 和 Stop 后、Release 前撤出祖先，两者都覆盖跨来源登记及封 Registrar。修复后原未来轮返回 InvalidEpoch；已有反例继续确认仅申请来源封口时其他来源可接下一轮。主代理按原序列复核，不将本次修复记作另一次独立复审。

本段最终开发证据：

| 检查 | 结果与边界 |
|---|---|
| libdevice host | 显式 `aarch64-apple-darwin`，29 项通过；`artifacts/device-registry/host-final.log`、`host-final.exit=0`。覆盖观察/管理角色不能抢 Claim、派生越界、撤出与候选提交、重复登记费用、原绑定任务恢复、身份退休及审查所指扫描前窗口 |
| 编译与 lint | `just check` 通过，`check.log/check.exit=0`；修复后八分面 `just clippy` 通过，`clippy-final.log/clippy-final.exit=0`，包含用户 RISC-V 库。中间 27 项及首轮 lint 日志保留为开发过程 |
| 结构复核 | 主代理核对来源/实例/绑定身份保留、纯观察不导出 Provision、两个候选提交重试、预算失败无发布、原任务不替换、停止/释放同步有效撤出、无递归引用链及无新运行体。没有改变已确认外部范围 |
| 文档与清理 | 3 份相关文档的 53 个本地链接/标题锚点及围栏通过，`docs-check.log/docs-check.exit=0`；`git diff --check` 通过。无本轮 QEMU/GDB 或临时服务，无暂存改动 |

该断点已由下节继续接线；本节证据仍只覆盖 backing，不由此推导真实运输或服务已验收。原资源与硬件消费者缺口仍保留，本篇保持施工态，未提交。

### 设备管理服务、RPC 与装配续接

继续原授权，新增实际 `srv_device` 进程，并纳入 user workspace、initfs 显式映像清单和 init 的必需启动集合。服务使用已有 Assembly/Runtime、SourceRegistration、Outbox 与 Dispatcher；没有新增执行器、测试服务或假设备。init 在独立管理 Job/Pool 中启动它，预留 Process 监督槽，私有 bootstrap 同时核 sender_context_id、PID、实际 mailbox 关联和两种根角色，随后发行正式观察来源并验证其不能派生权限。来源签发结果未能确认时封旧根，不发行替代来源。

`libdevice::protocol` 定义设备与 Provision 的独立协议标识、32 字节定宽头、严格 opcode/长度/保留位/句柄数校验，注册输入同时满足后续完整查询回复的容量。Prepare/Get/Stop/Confirm 共用原 epoch；明确 Progress::Closed/Prepare 的 Rejected 才能证明本轮供应安全，运输错误、NotFound、Error::Closed 均不能转换为该证明。请求/回复不使用 Rust 内存布局。实际服务按认证 context 分派来源、登记控制、BindingControl 与 BindingUse，纯观察不导出使用权。

原 Claim 请求在真实 Runtime 准入后转成长期 owner，持 `binding_work::Work`、控制 Lifetime、Dispatcher 调用和冻结表。Created 前预付控制、表、记录和任务存储；回复完成或丢失不结束该责任。每轮只持一个实际 RPC，取消先等 Dispatcher 退休本地调用来源，再向原 Provision Stop；真实 Closed 后才 Released。原任务继续等待旧控制/使用入口及已接请求的共享引用退休，最后归还 Registry 身份引用。无安全证据的失败保留责任并升级服务故障，不以 Drop 或超时退款。ReportStopped 还要求来源的真实报告授权与已取得的 DMA 角色，不能从驱动声明自授。

业务与管理请求在服务内使用独立账户、Task 配额、来源输入及字节预算；只有实际控制角色可用管理预留，单纯自报 Stop opcode 不可。长期记录仍付原来源账户，控制请求由管理者预留账户承担，两种 Funding 内部各自保持领域/执行同账户。输出失败前后的责任不借用新业务额度。init 的服务页预算按公共 ABI 的 8 MiB 主栈再加 8 MiB 映像/执行/领域存储空间计，metadata 另给 1 MiB 对应槽位；不是把 timeout 当性能要求或用进程退出冒充 DMA 静止。

审查与开发过程：

- sub-17 找到两项 P2：停止报告被旧重查等待挡住、空闲重查漏掉阶段期限。已集中为 timing 门：有效期限取阶段结束/重查时间较早者，新报告不受旧轮询退避限制；Ready/Released 不继续公布准备期限。增加独立时间线反例。宿主还区分 Work 的定时通知与 Outbox 自身期限，防止一次准备重查提前取消 Claim 回复。
- sub-18 找到 P1 登记回执误分派：根 kind 4 回执广播到等待 kind 5 的 owner，正常启动会断言；真实第三轮运行复现。已在 Observed 按 kind 分派，保留执行器严格断言；原根双登记与后续 Outbox 重登使用同一分派边界。修复均由主代理对照原序列复核，未宣称另做独立复审。
- 初轮构建漏入 initfs 的 srv_device 已补入显式装配。初轮客体在服务启动前遇到原 ProcessCreate 全局 Pool 退款断言，后续两轮未复现；全局计数混合其他异步退款，当前改为专用付款 Pool，核初始 allocated=0、实际 Map 后正扣费、Building 退休后精确回到 0，保留差值诊断，不据首次现象归因内核。新 Pool 首次只预留 256 KiB metadata，release 聚合在 ProcessCreate 拒绝：固定区域/事务账本本身就需 394624 字节，尚未含 View 和预付退休。已按完整空进程存储成本预留 1 MiB metadata，页预算仍为 16 页，修正后 release core 通过。
- 第二轮服务 spawn 失败，静态核对发现原 8 MiB 总页预算不足以容纳 ABI 的完整 8 MiB 栈和约 0.6 MiB 映像，现已按上述资源模型修正并保留具体 spawn 错误信息。这些失败的退出码 124 来自失败锚点主动收割，不是有界推进撞限。
- 平台续跑的 sifive_u 通过；nofd 在 `verify_batch_handoff` 的监督恢复中失败（`artifacts/failed-acceptance-20261025-010433-86538.{log,meta}`）。原验收取最后两个服务等待自然退出，test_fp 不启动时包含长期 srv_device；此时管理者还未收到 Stop，监督等不到正常终态。现把设备管理者来源封口、正常退出和账本收集放在短寿命服务监督验收之前；没有删去 handoff 测试或延长期限。最后顺序版用 nofd、debug stress 及故障路线复核。

当前开发证据：`artifacts/device-rpc/host-final.log` 中 libdevice 39 项、libkernel/libprocess 11 项单测及 2 项 compile-fail doc-test 通过；包含原机制、两个时间线反例和八项 wire/Funding 反例。`check-final.exit=0`，聚合八分面 Clippy 和 debug stress 通过；第四轮 debug core 也已通过，入口日志为 `artifacts/run/20261025-003557-67358-acceptance-virt-debug.log`，当时未保留成功客体原始日志。隔离付款域容量修正后的 release core 通过（`release-retry.exit=0`），入口为 `artifacts/run/20261025-010306-83321-acceptance-virt-release.log`，已启用成功客体日志保留。最后顺序版的八分面 Clippy、nofd 和 debug stress（16/16 矩阵）均通过，日志为 `clippy-order.log`、`nofd-retry.log`、`stress-final.log`，对应 exit 均为 0。sifive_u 在付款修正后、停止顺序前已通过（`platforms-final.log`）；其后 nofd/stress 覆盖最后的公共停止顺序改动。boot panic/alloc/fatal、IPI failure、VM fault、VM repair 均通过（`faults-final.log`）。DMA retention 初轮在目标窗口前遇到下面记录的验收断言和清理异常，原组合 exit=1 保留；修正后的 DMA retention 与最终八分面 Clippy 均通过（`retention-retry.exit=0`、`clippy-retention.exit=0`）。最终 DMA 现场为 `artifacts/dma-retention/20261024T171526.378604Z-3082/`：reset 拒绝前、init DONE 和四 hart 停驻后三处均为同两条 mapping、4 页、2 个写许可，Work=Idle；同时包含设备管理者正常停止的四个锚点。所有要求的路线已分别通过，仍按各次实际版本保留证据，不把这些结果称为尚未接通硬件的设备能力验收。

前三轮失败日志及元数据分别保留在 `artifacts/failed-acceptance-20261025-{001136-63386,002239-64849,003023-66527}.{log,meta}`；新付款域容量不足的失败为 `artifacts/failed-acceptance-20261025-005707-82830.{log,meta}`。原聚合 `acceptance.exit=124` 保留，不改写为成功；修正后从失败路线续跑，已通过且不受付款验收修改影响的 stress 不重复。

本段完成时服务尚未导入平台设备、接设备 Watch/OAL 与功能边，也没有 srv_driver 或硬件驱动消费者；后续发现与观察接线由下节承接，平台根仍由 init 保留。平台 Provision、父 Binding 约束的子 Registrar、选择/激活/停止装配与真实 PCI/xHCI/HID 接线继续属于同一闭包。当前验证首先闭合实际管理基座启动、权限、来源封口与正常退出，不能替代 Register/Claim/Provision 的真实硬件和精确运输故障门。

DMA retention 的初轮现场为 `artifacts/dma-retention/20261024T171248.959139Z-2011/`。客体停在“封域后读地址必须恰为 ObjectClosed”的旧断言，尚未到新服务或保留窗口；该日志未记录实际错误变体，不能声称已经观测到 ObjectBusy 或地址泄露。静态合同核对表明 ResourceTree 的分步 `advance/validate` 在 close sequence 变化时明确返回 Busy，DMA 映射为 ObjectBusy；祖先封口逐层传播可触发这一路径。验收改为只接受 ObjectClosed/ObjectBusy 且输出 Segment 的两个哨兵字段均不变，任何成功或其他错误仍失败；随后仍核 MayBeExposed、写许可和实际 Retire，不降低安全要求。调试器 detach 后 QEMU 已退出，宿主 killpg 的 PermissionError 掩盖了原失败；共享清理 helper 仅在亲生进程已结束时容许这一竞态，活进程的权限错误仍抛出。独立短进程检查覆盖活组终止回收、已退出 EPERM、活组 EPERM 三条路径，见 `process-cleanup-check.log`，无残留测试进程。

主代理结构复核已沿真实 ingress→请求准入→领域提交→Outbox/Dispatcher→Lifetime 注销→记录退休检查身份与付款边界，核对管理预留不会被普通请求占用、来源双回执按 kind 分派、准备/停止期限和回复期限分开、原轮 Unknown 不重建 owner，以及 init 的 Pool/Job/Process 账本和 Grant 消费状态。宿主复用既有 Runtime 和 RPC，没有平行消息泵、驱动私有操作回调或旧角色兼容出口；当前已发现的问题均在原路径修正。未运行的 Register/Claim 供应链及硬件停止不计为已验证；实际旧驱动占位随后续驱动装配删除，仍由本篇拥有。

本段提出的有限枚举/Watch、功能/OAL 和父 Binding 子来源接线已由下节承接。当前下一步、未闭合消费者与验证门以下节为准；原 Register/Claim 的运输 Unknown、迟到回复、接收安装失败及业务额度耗尽后的管理路径仍随正式消费者取证，不以假 provider 或空设备补门。计划保持施工态。

文档、脚本与收尾：同步 device/startup 实现说明、服务装配计划、库归属、构建手册和矫正区；102 个本地文档链接/标题锚点、Rust 格式、脚本语法及 `git diff --check` 通过。本轮 QEMU/GDB 和隔离清理测试进程全部退出，暂存区为空，未提交或 push；见 `artifacts/device-rpc/final-check.log`。

### 设备发现、观察与授权续接

继续既有首批闭包授权；接手仍为 `refactor/system-redesign` / `549a9e7`，保留前述未提交实现。本轮沿真实 srv_device 接通有限 serial 枚举、Watch、功能/OAL 投影及父 Binding 限定的子登记来源；不引入新执行器或模拟供应者。

实施接点与约束：

- Registry 独占枚举与观察状态。首批枚举以当前最高 serial 为有限终点，续页保留该终点，删除跳过，后来的登记留待下轮。范围及读取权限每次复核，不把游标作为授权。
- 每份 Watch 独立持非回绕 sequence/pending/终因，并付记录、任务、来源观察与字节费用；登记、撤出、绑定阶段和功能变化在原本地提交中推进相关 Watch。宿主通过原长期请求及 NotificationOwner/Signaler 投影通知，向客户端只交 NotificationObserver 和该 Watch 的控制 sender；Query/Ack/Cancel 不授予设备使用权。旧 Ack 保留后来变化，未来 Ack 拒绝，耗尽保留最后一个终态序号。
- 来源封口立即终止对应观察准入；已发行控制仍能查询终态。记录在控制 Lifetime 与已接请求退休后释放其原 source/device 引用和费用，所有通知和清理均复用现有 Runtime 任务。
- 功能入口及激活观察只按原 BindingRef 安装；新取得同时复核本轮 ResourcesReady、来源、范围及 FunctionAccess。停止立即隐藏功能边，已交付入口的业务收束仍归实际 provider。OAL 复用 Lookup/Enumerate/Boundary，不复制领域树或把纯观察升级为功能授权。
- 子 Registrar 来源保存父 BindingRef，不能因设备下一轮重绑而恢复；准入须核原轮可用。来源引用保留父设备身份，父停止同步阻止后代新承诺，原维护任务推进撤出与清理。公开源权限显式区分子登记发行与无限定的根登记，普通 Claim 不隐含该权限。

实现已接 `registry/{discovery,functions,oal}`、独立 Watch 序号、协议 op 14–24、srv_device 原请求/Watch 通知投影及 init 的真实观察流程。DeviceResource 增加 Watch/Function，容量分别为 32/64；执行槽、输入存储和角色表从这些真实上界同步推导。Ack/Cancel 只有已核定的 Watch 控制角色能使用管理预留。子来源新增显式 CHILD_REGISTER 权限，普通 CLAIM 不取得它。

OAL 的具体投影为根按名列设备，每个设备有 binding/children/functions；真实子实例在 children 下，因此不禁止设备取名 binding 或 functions，也不制造重名歧义。Function 的具名 entry 是驱动提供的 OAL 功能根；InstallFunction 同时冻结名字、描述、实际 endpoint，OpenFunction 返回同一 entry，functions/name Boundary 同门复核后交付它。InstallActivation 保存独立不透明观察入口及协议号，不解释驱动阶段。OAL 当前每页至多一项，名字/游标容量由消息边界推导，保留弱一致、删除后继续和原视图身份校验。

开发验证复用现有 host 反例和真实 init/srv_device：范围隔离、有限终点、旧 Ack、序号耗尽、Withdrawn 的停止/释放变化、父轮停止及旧来源不复活、FunctionAccess 与 Boundary 同门、停止后禁止取得及旧功能不随重绑恢复均有独立合同反例。开发初版 47 项 host 和八分面 Clippy 通过（`artifacts/device-discovery/host-fourth.log`、`clippy-third.log`）；此后 wire、视图重名和审查反例已纳入最终 51 项。真实 release core 已通过（`virt-first.exit=0`，入口 `artifacts/run/20261025-015601-8823-acceptance-virt-release.log`）：init 实际订阅、Query、枚举空根、Ack/通知清空、OAL 根及正常停止的 Revoked/终态 Ack 均执行。该版本的 debug stress（16/16）、sifive_u 和 nofd 也已通过，`routes.exit=0`，各路线入口见 `stress.log`、`platforms.log`；`just check` 通过。以下审查修正后，host 共 51 项通过（`host-review.log`）；审查修正后的八分面 Clippy 与 release core 均通过（`clippy-review.exit=0`、`virt-review.exit=0`，入口 `artifacts/run/20261025-020540-20345-acceptance-virt-release.log`）。最后将 OAL 摘要暂存容量从固定余量改为按一个 trait/tag 及实际值宽度推导；该调整后的 host 51 项和八分面 Clippy 通过（`host-layout.log`、`clippy-final.log`），既有短名字运行路径的编码不变。其他未受修正影响的已通过路线不重复，本轮未重跑与用户态发现接线无关的 boot/IRQ/DMA/VM 故障注入。

只读增量审查 sub-20 发现并由主代理按原序列修复三项问题：

1. P2 非根目录续页：原实现把空路径续页误当入口根，再与非根游标比较而拒绝。现从 cursor 的 incarnation/原视图身份恢复 Device/Children/Functions，再核当前 OBSERVE 范围；非法/已退休身份拒绝。同一测试按真实 OAL 合同以空路径取完非根三页，并以另一子树来源尝试同一游标，核权限拒绝。
2. P2 单独 ClaimSource 封口：原代码只给登记来源失效的设备推进观察，功能准入已关闭时独立观察者仍可能 Ack 旧序号。现来源封口同次检查实际 claimant 来源，立即卸下功能/激活边并推进 Watch；后台只负责原 Binding.Stop。反例在任何维护调用前核登记仍 Present、Seat 仍 Ready、功能费用已归还、旧 Ack 保留新 pending。
3. P3 Watch 客户端错误丢失：原创建采用先要求两槽，合法零槽错误被改成 Invalid。现由纯 protocol::decode_watch 统一先辨成功/错误，错误要求空正文零槽并保留原状态，成功创建要求两槽，Query/Ack/Cancel 要求零槽。host 覆盖 Permission/Quota/Closed 和附带多余能力的畸形回复；init 还从真实 Watch 控制入口尝试越权订阅并核 Permission。主代理补紧 Exhausted 必须使用 MAX 序号的 wire 验证。

修复由主代理对照原反例复核，未宣称另做独立复审。当前平台导入仍为空；空根断言只验证当前实际装配，导入平台 PCI host 时必须替换为真实枚举内容，不保留测试专用空视图。Binding Watch、子 Registrar、功能安装及非空 OAL 的真实驱动消费者尚未接通，host 的整数 payload 不能冒充这些运行证据。BindingUse 的 Resources OAL 视图、供应/驱动管理与完整运行期监督继续随正式消费者接通；原 GetResource 已有固定权限接口。源码与 impls 如实记录上述边界，不宣布首批设备能力完成。

下一步由实际平台装配承接：将 init 保留的只读平台描述和限定根委托给 srv_device 平台适配器，登记真实 PCI host 并提供原轮 Provision；原 init 空根断言随该导入删除，改核实际 DeviceId/身份与资源表。随后接 libdriver/srv_driver 的选择、激活、协作停止和故障监督，PCI/xHCI/HID 与真实功能消费者；libdevice/srv_device 的 BindingUse Resources OAL 投影随首个实际 BindingUse 消费者补齐并核权限。硬件合同和未覆盖的精确运输/费用/停止窗口继续由本篇原验证门拥有，不需要再次申请已授权范围内的代码实施。

本轮交付前结构检查已核对：Registry 唯一持有领域事实，Notification 只投影 pending；功能与 OAL 不复制目录树或绕过 FunctionAccess；父 BindingRef 与原来源引用阻止旧轮复活；控制请求按实际角色使用管理预留，Watch 退休归还原 source/device/Binding 引用。原有 Runtime、Dispatcher、Outbox 和生命周期来源继续承担运输与清理，没有新增运行体、假设备或临时供应服务。108 个本地文档链接/标题锚点、Rust 格式、脚本语法及 `git diff --check` 均通过；本轮 QEMU/GDB 全部退出，暂存区为空，未提交或 push。证据见 `artifacts/device-discovery/final-check.log`。

### 设计交付基线（历史）

本轮接手基线为 `refactor/system-redesign` / `e6d11a1`，工作树原为干净。用户授权按计划推进到可以实施；本轮完成设计收敛、固定基线独立审查及四项发现的修订复核，未实施生产代码。

**首批施工设计已闭合，可以按“实施依赖与删除条件”进入代码实施。** 设计范围是 QEMU virt/PLIC、PCI xHCI、运行期 USB boot keyboard 热插、真实功能客户端及正常/故障收束；不代表全设备生态、通用服务编排或持久化关机已经闭合。当时仅获设计文档提交授权；当前代码授权与实施状态以上方记录为准。

#### 施工准入核对

| 原收敛切面 | 本轮定稿及证据 | 准入判断 |
|---|---|---|
| 领域与实际使用 | 设备→绑定→功能、父总线资源、FunctionAccess 与观察/Claim 分权；明确真实 provider 持权 | 所选角色、依赖方向和消费者明确 |
| 协议责任链 | 登记/占用/Provision 共用合法轮次、超时只停旧轮、Activate 观察/迟到回收、USB 请求去重及取消 | 四项独立 finding 已按原反例关闭；无已知未决提交/收尾 owner |
| 内核资源 | RAM/MMIO 来源分开，发布许可与强制撤销、预付槽、树准入/锁序、Pin 地址暴露及独立写许可 | 原 VM/Backing/等待/维护接点已亲读；不留给施工者选择互不兼容的生命周期 |
| 平台与装配 | DTB 保活/只读交付、启动根与实际权限、PCI BME、USB 停止、根 DONE 观察与系统终局 | 固定规范和本机 11.1.2 差异已核定；首批信任边界及拒绝范围明确 |
| 迁移与验证 | 真实路径表、删除旧 dtb/五句柄总数假设/日志驱动占位、device workload/QMP/故障判据 | 按依赖整链实施，无临时 adapter 或假设备前置 |

以上结论是当时的设计准入，不是实现验收；当时新资源 ABI、srv_device/srv_driver、驱动库/程序、设备 Watch、外部 IRQ 和 DTB 用户态交付均未实现，当前事实以上方施工记录为准。ABI 数字编号、Rust 内部文件拆分和按实际 Layout 推导的容量由实施确定；不得因此改变已定权限、提交点、失败收束或不支持范围。实现发现规范反例时回到本计划修订，不以临时分支绕过。

#### 检查、限制与下一步

主代理亲自检查 `MemoryObjectCore/MemoryObjectState`、VM Views/Operation/Backing、bootstrap、ProcessShell/reclaim、trap/sched/锁序、startup ABI、OAL Namespace 及实际 RPC 来源身份接点；explorer 只辅助盘点构建/迁移路径。一次 advisor 咨询用于 VM/资源接线取舍；独立 reviewer 用新上下文审查固定 `9ef1858` 及 `e6d11a1` 修订，原发现及主代理复核见[审查档案](archived/review-2026-10-23-device-system-design.md)。未把咨询、并行草案或原基线审查冒充对最终所有增量的独立审查。

交付前结构检查覆盖 owner、权限与付款分离、不可逆提交、旧回复/输出 fault、清理准入、最后 Close、根死亡、锁序、单步工作界、实际消费者和删除门；追加核对 ReadSegments 与 Retire 竞争、旧 VA 空洞重用、Source 封口后的范围完整性。所选首批范围没有留下已知阻塞设计项；条件后续均在本篇延期表或原领域计划，未把通用服务/动态保留内存/IOMMU 扩展硬塞入首批。

验证包括文档本地链接/标题锚点、围栏与 diff 空白检查，以及 QEMU 版本/设备属性查询。最终文档核验覆盖 10 份 Markdown、211 个本地链接与 64 个标题锚点，围栏和 `git diff --check` 通过；归档移动后的引用也已核对。本轮未运行状态模型、编译、host/target 测试或客体 QEMU，硬件与并发运行门尚待真实实现执行；不把设计路径推演报告为测试通过。

设计交付提交为 `a0f01e2359749ab2bc2e882016919a28ea3bc9a1`；[设计增量独立 Review](todo-2026-10-23-device-implementation-design-review.md)固定该提交，尚未执行，不重复登记已经关闭的四项原 finding。当前从上方实施断点接续，不再等待代码授权；后续新提交仍按协作约定展示摘要并取得授权。

#### 历史基线

`9ef1858` 保存首轮设备目标基线，原文档验证为 11 份 Markdown、182 个本地链接、47 个锚点及围栏/diff 检查。`e6d11a1` 删除 Provision.Query、QueryActivation 并合并激活终态；其中“超时换新轮”和 backing 持权禁令已由本轮修正，不能再作为施工指令。早期讨论只从[历史快照](archived/ref-2026-10-23-device-design-rounds.md)追溯；当前依据始终是本篇正文。
