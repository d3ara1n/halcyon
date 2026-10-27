# 正式服务装配与监督设计

状态：**设备首批装配已获实施授权；平台供应、两管理者私有启动/持续监督、真实PCI/xHCI/HID、单键盘热插、多Binding复用和管理者故障终局已有设备计划列明的组合证据；首批候选及精确费用/Unknown等门仍未完成，通用正式服务装配尚未全部闭合**。本案拥有启动依赖、就绪、监督恢复与进程授权装配；发布发现由[OAL/服务分案](todo-2026-10-07-fal-service-redesign.md)拥有，终局由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有，设备施工与全部证据由[设备分案](todo-2026-09-device-system-design.md#当前进度与接力)统一记录。

## 目标与非目标

把正式长期服务、资源管理者和驱动组合成有明确启动、授权、存活、恢复和停止责任的系统。能够回答谁启动谁、依据什么判断就绪、何时可向消费者交付能力、失败由谁接管、何时重启或升级为系统停止。

本案不预设新增 supervisor 进程、manifest 文件格式、通用服务框架或独立电源管理服务。静态装配与动态配置是待论证的方案选择；不因需要正式系统装配就重写已完成的 Process/Job、Thread owner 或用户态执行基座。

## 当前事实与前置

原装配盘点基线：2026-10-07，`refactor/system-redesign` / `f174d8e`；设备施工从 `549a9e7` 继续，当前未提交实现及验证由设备主计划拥有。当前启动与监督事实见 [startup 实现](../notes/impls/startup.md)、[task 实现](../notes/impls/task.md)、[framework 实现](../notes/impls/framework.md)。

- `srv_init` 的固定 workload、RequiredLaunchSet、RootSupervisor 与已有 pm/fs 消费者承担验收装配和真实监督责任；不能把它们当成没有可用基座。
- 对象链、自动回收、Job 封口、进程出生/执行绑定、来源登记及运行时分层已完成相应交付。本案不恢复 ProcessDrain 或用户必须持续驱动的内核回收。
- 当前已有 Registry/服务记录；发布成功、服务就绪、endpoint 可用和进程存活之间的正式系统协议尚未完成本轮详细设计。
- 设备的实例、受权使用/绑定、供应依赖和硬件安全责任已按首批范围定稿。virt 的三个 MMIO 根、PCI INTx 根和 DMA 根已交付 init，实际管理者委托及非空消费者已接通。设备驱动装配不能仅凭进程 DONE 推断 DMA 已静止。
- srv_device由init的独立Job/Pool和ProcessControl监督记录承载；真实私有bootstrap核context/PID及入口关联，发行受限观察并接枚举/OAL/Watch。平台委托、真实供应及依赖者已接，两管理者Start前准入/运行期/Stopping观察和唯一故障终局亦已接。正常停止按原来源Closed、监督admin副本关闭确认、真实Completed及原Job/进程/Pool收集交接；具体范围与未覆盖门见[设备主记录](todo-2026-09-device-system-design.md#当前进度与接力)。
- boot 已在初始进程启动前接入独立 DONE 观察；init 的最终 reset 被拒绝时自终止，由该观察触发 runtime stop。已暴露 DMA 的实际 pin 与写许可继续保留，相关证据见设备主计划；这不替代设备 halt 或正常停止编排。
- 关机编排与电源管理已有独立记录；本案只确定服务的停止入口及其与监督的交接，不复制全系统关机状态机。

发布发现的唯一目标由[OAL/fs/服务目录改造计划](todo-2026-10-07-fal-service-redesign.md#服务目录)拥有：业务就绪后单步 Register、显式 GetEntry、控制入口 Withdraw/QueryRegistration 和名字 Watch；公开状态为 Active/Withdrawn，准备/资源退休为内部责任。设计及实现状态见该记录，不作为既有启动事实。目录不维护 Starting/Ready/Draining 健康阶段，不把 endpoint 可用当健康证明。真实启动、就绪判断、业务 drain 和恢复归本案。

装配接点：首轮仍由 srv_fs 的同一 Runtime 容纳文件与 registry 两个独立 provider，分别交付实际入口；消费者在自己的 Namespace 绑定服务目录，通过 GetEntry 取得业务入口，服务替换不会自动重绑。启动者应在业务就绪后注册，注册成功后才宣告发布完成；若 Replace 已提交但回复丢失，旧注册不恢复，新控制最终消散后名字可能为空，按发布分案的未知结果合同处理。需要可恢复发布事务或不断档切换时，由本案证明需求再闭合，不把两阶段注册偷偷恢复为默认前置。

## 驱动启用与设备管理的装配边界

用户已明确要求运行期设备出现时能发现并启动尚未运行的驱动；采用 init 启动并授权专职 srv_driver，后者统一匹配、按需启动/复用和激活监督。设备关系与 Claim 的详细目标见[设备计划](todo-2026-09-device-system-design.md#驱动发现按需激活与设备绑定)。srv_device 只拥有设备登记与受权占用，不认识驱动可执行文件。

init 交付受托 Job/进程控制范围、付款来源、设备 owner 派生的管理授权以及启动必需映像来源，监督 srv_driver；srv_driver 使用既有 libprocess 启动并监督自己的驱动运行实例。启动时设备与后续热插设备走同一匹配/绑定流程：进程不存在时启动，已有可用进程时复用，随后交付同一种绑定激活请求。不保留 init 和已运行驱动各自竞争自动 Claim 的双轨。具体 WorkerConfig 同次出生交付容量；srv_driver 的 ProcessTask 唯一持进程/Pool/监督，Managed 只持逐 Binding 工作，关联从原 Run 推导，不按 PID 或数组槽复用身份。只有全部关联安全脱离后才能收集进程；当前 PCI/xHCI 隔离，HID 容量二并允许复用。

未来驱动仓库以文件系统中的映像和支持声明作为匹配输入，格式、安装、更新机制按用户要求后续设计。首批由 init 交付 boot 配对声明/映像来源，包含 PCI、xHCI、HID；HID 仅在接口出现时启动。根控制器及仓库所依赖的存储驱动必须能从启动来源取得，仍由同一选择/激活流程处理。静态指定实现、明确优先级、歧义拒绝与有界候选尝试由设备计划的驱动合同拥有，不在 init 再匹配一次。

`/System/Drivers` 由 srv_driver 的运行实例与绑定激活事实提供 OAL 视图，设备节点直接关联绑定及功能，不复制设备占用表。[设备控制根合同](todo-2026-09-device-system-design.md#控制根付款与故障)规定 init 在子故障域外保留平台根和来源管理；srv_driver 失败先封旧来源，srv_device 失败进入系统故障终局、不重建占用表。[根终局](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)规定 reset 错误及 init 死亡处理；DMA 保活独立于句柄和管理进程寿命。

srv_driver私有Ready精确交两个sender及两个真实Mailbox KOID：Control只留init，Observer独立认证并只接受零cap/noBoundary的只读OAL。两个已有Mailbox/Ingress仍由同一World/Runtime推进，公开队列与总请求预留不能挤占管理准入；worker仍交原单个激活入口。运行关联只用InstanceId=(Control Mailbox incarnation,出生前serial)，不以PID或路径恢复权限。实际消息付款另按内核对象合同：普通Sender付当前发送进程，Once由make_send_once发行者Pool委托一次有界运输及Delivery尾部，Receive表安装付接收者；服务用户态请求/回复缓冲及执行来源仍付服务预留，不宣称所有管理分配必成。

### 首批装配提交与监督

- init 在创建各管理进程前预付监督槽、控制入口、根能力保存与故障任务；驱动受托 Job 的实际上级仍由 init 持有。srv_driver 的启动权限不得逃出该 Job/预算。
- 先启动并取得 srv_device 的真实管理/观察入口，再派生 ClaimSource 给 srv_driver、SourceAdmin 留 init。来源签发产生 Unknown 时，init 不发行第二份替代来源继续执行：停止该未完成装配，使用仍持有的 srv_device 根管理封闭其全部已发行来源，再决定安全重试或故障终局。首批不引入可恢复授权发行事务。
- 管理者 bootstrap 通过 init 预先交付的私有接点送回实际入口；只有相应进程和入口均就绪才启动依赖者，启动期限失败按原监督记录停止，不留未登记的孤儿进程。
- 运行期按声明隔离 PCI host/xHCI，HID worker 可在已准入容量内服务多个绑定。启动中的实现有唯一激活 owner，重复设备提示合并到原记录；实例消失时先停止对应 Binding，不因启动回复迟到再次激活。
- 首批必要功能出口为 Devices/functions；Registry 是可选装配别名。发布可见不等于驱动健康，停止时撤名不阻塞资源链；业务就绪由实际 Activate/Ready 合同决定。
- srv_driver 接替者只用新 Claim context；旧执行与所有相关 Binding 安全结束后才恢复选择。无停止证据时进入终局，不定时无限重启。init 停止意图先封统一装配 gate，之后不会启动新的管理者或 worker。

上述选用静态启动配置与动态设备激活，不要求先设计 initfs 编排脚本或通用 supervisor。管理记录按实际存储及预留任务付款，失败保留原 owner；授权签发/进程启动的不确定结果不能靠丢句柄当作已回滚。

### 首批装配图与就绪合同

首批使用编译期装配配置；启动映像集是输入，不承担运行期监督。下表的 Pool 是实际内核付款来源，用户态 libbudget 账户仅约束所属管理者的记录、任务和字节。每次进程出生前由启动者预留启动记录、就绪接点和终态观察，Process/Job 管理关系不由付款关系推导。

| 运行角色 | 启动与监督 owner | 启动交付的最小权限和付款 | 对依赖者的就绪事实 |
|---|---|---|---|
| srv_init | kernel boot owner 观察初始 Shell DONE | 根 Job、SystemReset、根 Pool、只读启动映像/DTB、实际平台控制根 | 保有控制根与监督记录后才能启动管理基座 |
| srv_device | init 的管理 Job；init 持 ProcessControl | 自身 Pool、私有 bootstrap sender；平台适配器持明确委托的资源控制及登记范围 | 私有接点返回 DeviceAdmin、DeviceObserver 和平台登记控制；仅表示接收与清理任务已可运行 |
| srv_driver | init 的管理 Job；init 持 ProcessControl 与 ClaimSourceAdmin | 独立 Pool、受托驱动 Job、ClaimSource、DeviceObserver、boot 配对映像/声明来源；无 SystemReset | 已装入静态声明、挂好设备订阅、能接收 Stop；不要求 HID worker 已启动 |
| PCI / xHCI worker | srv_driver 的驱动 Job；srv_driver 持各 ProcessControl，init 保留外层 Job | 各自 Pool、私有激活接点；BindingUse 按激活交付。PCI 额外得到限定窗口、子资源派生和子登记权；xHCI 得到受信 DMA use | 进程 bootstrap 只返回激活入口；功能就绪另等该 Binding 的 Ready |
| HID worker | 同一驱动 Job；按已准入容量复用 | 自身 Pool、激活接点、接口 BindingUse；无物理资源根和受信 DMA 报告权限 | 每接口独立 Ready；一个接口失败不伪造其他接口终态 |
| 正式键盘消费者 / test_device | init 的消费者 Job | 自身 Pool、DeviceObserver 与限定 FunctionAccess、结果回报接点 | 从实际 functions 取得键盘入口后消费 libhid；不由 PID 或目录出现推定 Ready |

正常服务不从通用 Registry 发现自己的 bootstrap 控制入口。init 为每次启动预建独占 sender context/receiver，把 sender 通过出生块交给目标；回复身份与已保留的启动记录关联，迟到回复只能回到原记录。收到入口后同时核对目标 Process 尚未终态；后续死亡由预先安装的观察接管，不能凭一次检查保证永久存活。启动失败、就绪超时或停止先到时，记录保持 Stopping，关闭新激活，停止该实际 Process，并等待其终态；迟到入口只释放，不发布为新实例。

管理者就绪后平台适配器登记 PCI host；PCI Ready 后才可能发现 xHCI；USB 接口出现才激活 HID。这些是功能供应关系，不能替代进程 Job 树。设备业务不依赖 srv_fs/Registry 就绪：必要功能出口是 Devices/functions，通过已有 OAL Namespace 绑定实际入口；可选 Registry 别名失败只影响发现别名。已有 core workload 的 fs/pm 验收装配保留原 owner，不把它们误列为本次硬件启动前置。

首批不自动重启失效的 srv_device 或 DMA 主控。srv_driver 失效后，init 封旧 ClaimSource、停止受托驱动 Job；只有全部旧 Binding 已安全 Released 才允许显式管理请求创建新来源和新管理者。初次启动与接替都经过同一装配 gate；没有明确请求不循环重启。若清理需要已死主控的硬件证据，直接按关机计划升级。

PCI 与 xHCI/root-hub 的子登记能力在原绑定 ResourcesReady 后由 srv_driver 凭 BindingControl 调用 IssueRegistrar 取得，随同一 Activate 显式交付；不属于 Provision 冻结硬件表。srv_driver 在运输前将 Registrar/SourceAdmin 纳入原激活记录，SourceAdmin 留该记录，init 外层来源管理覆盖其死亡后的封口。HID/普通叶角色没有该 grant。签发 Unknown 时停止原轮，父轮失效阻止所有可能已发行子来源继续登记；不重签替代授权。激活重试使用同一能力身份，迟到/停止请求及父子清理遵守设备计划的统一合同。

平台根、PCI 配置写权限和 TrustedPhysical DMA 属于系统可信基座：init、设备管理/平台适配器、认证并转交静止报告的 srv_driver、PCI provider、xHCI 主控均必须可信。不能只把 xHCI 标为可信，却把可重配 BAR/bus-master、派生 DMA 或提交可信静止事实的上游当作不可信。HID 与功能客户端的边界来自受限 USB 会话；路径、支持声明和程序自报标志不能授予上述权限。

## 设计门

消费者停止证据沿原 Binding/Provision 传递。PCI host 的 DMA control 使 PCI provider 属于受信物理供应链：它须在后代安全收束、最后一次 ECAM/配置访问结束后报告，不等待自身 Released。管理者认证该原轮事实后转交 ReportStopped，平台收到 ConfirmStopped 后仍独立完成资源退休。未曾交付资源且原 Binding 已 Stopping 时，srv_device 原 Work 可根据唯一资源出口的不可逆交付事实确认未产生消费者访问；晚到 Activate/Use 无法越过已封 Get 门。未知激活和进程死亡不清除已暴露事实，证据不足沿故障终局保留，不追加并行监督协议。

1. 明确首批正式长期服务及可观察能力、启动依赖、必需与可选能力；从真实需求选择静态或可配置装配。
2. 分开画进程/Job 管理、付款 Pool、授权交付与服务供应关系。确定上级退出、失联和权限消散后的接管责任。
3. 闭合启动失败、就绪超时、服务死亡、重新发布、重连与失败升级；重启不得复活失效 endpoint 或重复授予权力。
4. 与 FAL/服务分案核定发布发现接点，与设备分案核定驱动供应和隔离事实，与关机编排核定停止接点。各项契约只有一个拥有计划。
5. 形成首批装配的接口、责任转换、迁移删除清单与验证门后，才进入代码施工。

上述设计门在设备首批范围内已由装配图、私有就绪/Unknown/监督合同及设备主计划的迁移/验证门闭合，可以随设备施工；通用正式服务继续按这些门设计，不把未来热升级、持久化和配置需求列为本次设备的前置。

## 完成门与验证边界

设计交付须能依据计划实现选定装配，明确正常、失败、取消、退出、恢复和停止责任。系统能力交付须有真实长期服务接入、替换范围内的旧装配路径删除，并在相应平台验证启动、就绪、故障恢复和停止组合；不能仅用一次验收 workload 跑完宣称正式编排已完成。

本案通用正式装配方案仍未闭合；设备首批的进程分工、静态来源、授权和失败政策已有上文目标，不代表一般服务热升级、持久化停止或配置脚本已设计完成。现有同进程 fs/registry 装配由 OAL/fs/服务目录计划接通，本案不要求提前拆出 srv_service。设备施工的代码、开发验证与未覆盖项只在设备主计划记录；已接通的管理宿主启动/停止不代表平台供应、驱动接入或完整运行期故障监督已完成。

## 当前接力

设备首批继续按本篇装配合同和[设备主计划当前交接点](todo-2026-09-device-system-design.md#当前交接点2026-10-25)实施。真实平台/驱动接入、两管理者持续故障观察及只读Drivers已接，下一施工为主计划拥有的有限候选/显式Retry与精确费用/Unknown等剩余门；不重做已过装配基线。通用正式服务继续闭合本篇设计门，仓库格式/安装更新保持延期；不恢复双轨匹配、透明继承旧来源或由srv_device启动驱动。

## 首批设备的持续监督接线

具体采用init内一条libruntime用户线程及独立WaitSet；原RootSupervisor账本/Assembly仍单主线程owner，不共享可变账本或新增通用supervisor。libprocess的同一loader提供未启动准备owner与Start，观察必须在Start前挂好；普通spawn组合二者。两管理者的缩权Control、正常停止意图/期限、真实Completed及SourceStop::Closed交接各属于同一固定记录。srv_device根admin监督副本先明确关闭并确认，主线才关最后root/admin；失败不得由消费Manager析构丢责任。具体已定契约及实现证据由[设备主记录](todo-2026-09-device-system-design.md#init设备管理者的独立观察与终局施工合同)拥有。
