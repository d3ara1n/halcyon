# 正式服务装配与监督设计

状态：**设备第二轮持续监督已实施并通过所选组合验证；通用正式服务装配仍待后续设计，原设备独立门未全部关闭**。本案拥有启动依赖、就绪、监督恢复与进程授权装配；已实现合同见[第二轮持续监督设计](#第二轮持续监督设计2026-10-27)，本轮实现、证据与边界见[机制收敛报告](archived/review-2026-10-27-device-implementation.md#第二轮交付与边界)。发布发现由[OAL/服务分案](todo-2026-10-07-fal-service-redesign.md)拥有，终局由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有，原设备开放门见[唯一收口表](todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)。

## 目标与非目标

把正式长期服务、资源管理者和驱动组合成有明确启动、授权、存活、恢复和停止责任的系统。能够回答谁启动谁、依据什么判断就绪、何时可向消费者交付能力、失败由谁接管、何时重启或升级为系统停止。

本案不预设新增 supervisor 进程、manifest 文件格式、通用服务框架或独立电源管理服务。静态装配与动态配置是待论证的方案选择；不因需要正式系统装配就重写已完成的 Process/Job、Thread owner 或用户态执行基座。

## 当前事实与前置

原装配盘点基线为`f174d8e`，首批设备施工为`549a9e7..f3337eb`；第二轮在其上替换独立线程及重复握手，当前实现由root同一Assembly持续推进。启动与监督事实见[startup实现](../notes/impls/startup.md)、[设备实现](../notes/impls/device.md)、[framework实现](../notes/impls/framework.md)。

- `srv_init` 的固定 workload、RequiredLaunchSet、RootSupervisor 与已有 pm/fs 消费者承担验收装配和真实监督责任；不能把它们当成没有可用基座。
- 对象链、自动回收、Job 封口、进程出生/执行绑定、来源登记及运行时分层已完成相应交付。本案不恢复 ProcessDrain 或用户必须持续驱动的内核回收。
- 当前已有 Registry/服务记录；发布成功、服务就绪、endpoint 可用和进程存活之间的正式系统协议尚未完成本轮详细设计。
- 设备的实例、受权使用/绑定、供应依赖和硬件安全责任已按首批范围定稿。virt 的三个 MMIO 根、PCI INTx 根和 DMA 根已交付 init，实际管理者委托及非空消费者已接通。设备驱动装配不能仅凭进程 DONE 推断 DMA 已静止。
- srv_device由init的独立Job/Pool及root管理记录承载；真实私有bootstrap核context/PID及入口关联，发行受限观察并接枚举/OAL/Watch。两管理者的DONE来源先于Start，运行期/Stopping观察与唯一故障终局均由root推进。正常停止由同一owner确认来源Closed、关闭原admin、核真实Completed、注销来源并交原Collector，无admin副本/ACK或撤防握手；具体边界见设备唯一收口表。
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

平台适配器先登记 PCI host，srv_device 的私有就绪等待真实登记回执；srv_driver 随后观察并激活 PCI，PCI Ready 后才可能发现 xHCI，USB 接口出现才激活 HID。这些是功能供应关系，不能替代进程 Job 树。设备业务不依赖 srv_fs/Registry 就绪：必要功能出口是 Devices/functions，通过已有 OAL Namespace 绑定实际入口；可选 Registry 别名失败只影响发现别名。已有 core workload 的 fs/pm 验收装配保留原 owner，不把它们误列为本次硬件启动前置。

首批不自动重启失效的 srv_device 或 DMA 主控。srv_driver 失效后，init 封旧 ClaimSource、停止受托驱动 Job；只有全部旧 Binding 已安全 Released 才允许显式管理请求创建新来源和新管理者。初次启动与接替都经过同一装配 gate；没有明确请求不循环重启。若清理需要已死主控的硬件证据，直接按关机计划升级。

PCI 与 xHCI/root-hub 的子登记能力在原绑定 ResourcesReady 后由 srv_driver 凭 BindingControl 调用 IssueRegistrar 取得，随同一 Activate 显式交付；不属于 Provision 冻结硬件表。这对子 Registrar/SourceAdmin 固定为原轮出口，Unknown 可恢复同一身份，或停止原轮；不新签替代来源。SourceAdmin 仍留管理记录，init 外层来源管理覆盖其死亡后的封口；HID/普通叶角色不取得该 grant。迟到/停止请求与父子清理按设备第二轮合同。

平台根、PCI 配置写权限和 TrustedPhysical DMA 属于系统可信基座：init、设备管理/平台适配器、选择受信实现并委派报告权的 srv_driver、PCI provider、xHCI 主控均必须可信。直接报告缩短转交链，不缩小这些实际权限的可信边界。不能只把 xHCI 标为可信，却把可重配 BAR/bus-master、派生 DMA 或委派报告权的上游当作不可信。HID 与功能客户端的边界来自受限 USB 会话；路径、支持声明和程序自报标志不能授予上述权限。

## 设计门

消费者停止与供应安全按[设备第二轮合同](todo-2026-09-device-system-design.md#第二轮收敛设计2026-10-27)传递：可信消费 owner 经原轮 Use 直接报告，Binding 将单调静止事实交原 Provision；srv_driver 不再转述。PCI host 在后代硬件责任、最后一次 ECAM/配置访问结束后报告，不等自身 Released，供应仍独立完成资源退休。未暴露内生证明、Unknown 与进程死亡不等于安全、证据不足进入故障终局的边界保留。

1. 明确首批正式长期服务及可观察能力、启动依赖、必需与可选能力；从真实需求选择静态或可配置装配。
2. 分开画进程/Job 管理、付款 Pool、授权交付与服务供应关系。确定上级退出、失联和权限消散后的接管责任。
3. 闭合启动失败、就绪超时、服务死亡、重新发布、重连与失败升级；重启不得复活失效 endpoint 或重复授予权力。
4. 与 FAL/服务分案核定发布发现接点，与设备分案核定驱动供应和隔离事实，与关机编排核定停止接点。各项契约只有一个拥有计划。
5. 形成首批装配的接口、责任转换、迁移删除清单与验证门后，才进入代码施工。

上述设计门在设备首批范围内已由装配图、私有就绪/Unknown/监督合同及设备主计划的迁移/验证门闭合，可以随设备施工；通用正式服务继续按这些门设计，不把未来热升级、持久化和配置需求列为本次设备的前置。

## 完成门与验证边界

设计交付须能依据计划实现选定装配，明确正常、失败、取消、退出、恢复和停止责任。系统能力交付须有真实长期服务接入、替换范围内的旧装配路径删除，并在相应平台验证启动、就绪、故障恢复和停止组合；不能仅用一次验收 workload 跑完宣称正式编排已完成。

本案通用正式装配方案仍未闭合；设备首批的进程分工、静态来源、授权和持续监督已有实现与所选验证，不代表一般服务热升级、持久化停止或配置脚本已设计完成。现有同进程fs/registry装配由发布分案接通，不要求提前拆出srv_service。第二轮实施证据由机制收敛报告归档，设备原独立门仍由主计划维护。

## 当前接力

第二轮两条责任链已完成本次迁移，不再重做独立线程替换或loader分步化。设备原剩余门从其唯一收口表接续；本案在正式长期服务、恢复或持久化停止提出实际需求时，按上述设计门闭合通用装配。仓库格式/安装更新、透明重启、通用manifest继续延期，不恢复双轨匹配或由srv_device启动驱动。

## 首批设备的持续监督接线

首轮固定检查点采用独立观察线程与WaitSet，和主线程通过停止/关闭/撤防确认协调；该形态已由第二轮替换。原合同与运行证据保留在[首批历史记录](todo-2026-09-device-system-design.md#init设备管理者的独立观察与终局施工合同)，当前合同由下节拥有。

## 第二轮持续监督设计（2026-10-27）

本轮一致性重构已完成：同一loader、ELF验证及Image构造、实际装配等待和持续管理观察已接入root，旧线程已删除，所选运行验证与结构检查通过。施工过程与验证证据保存在[收敛交付档案](archived/review-2026-10-27-device-implementation.md)。选择 RootSupervisor 作为唯一监督事实和终局权限 owner，装配仍可顺序表达，但所有依赖等待都通过明确的 root 提交/等待接点推进全部责任。既有 Process/Job/Pool、Runtime/Assembly、RPC调用owner与 collector 继续复用，不建立 supervisor 服务或通用工作流。

### 已接通的装配范围

[`RootSupervisor::turn_and_wait`](../user/services/srv_init/src/supervisor.rs) 持一份 Assembly，分流 Job、Batch、Read、管理者观察及装配等待的来源，每轮推进各自有限工作，再汇总可运行状态、最早期限和本次等待结果。顺序装配在等待外部结果期间继续推进这些责任。

范围以“两管理者开始受监督后仍可达”为界，当前接点如下。首轮 `f3337eb` 的阻塞路径盘点与迁移过程保留在收敛档案，不再用旧源码行号描述当前实现。

| 实际路径 | 当前源码 | 接入方式 |
|---|---|---|
| 两管理者 bootstrap 等待、设备管理 RPC | [device.rs](../user/services/srv_init/src/device.rs)、[driver.rs](../user/services/srv_init/src/driver.rs) | root 的 receive/wait 与 Exchange；原 CallOperation 的来源进入统一等待槽 |
| 来源停止查询、DONE 与管理能力关闭 | [device_supervision.rs](../user/services/srv_init/src/device_supervision.rs) | 同一管理记录确认来源 Closed 与实际 DONE，注销后交原 Collector |
| srv_fs 就绪、发布及注册队列探针 | [filesystem.rs](../user/services/srv_init/src/filesystem.rs)、[registry.rs](../user/services/srv_init/src/filesystem/registry.rs) | 顺序 RPC 用 Exchange；Copy/Watch 保留原 Dispatcher；待领取回复通过 root 观察实际就绪 |
| 报告、进程与验收阶段等待 | [supervisor/waiting.rs](../user/services/srv_init/src/supervisor/waiting.rs)、[main.rs](../user/services/srv_init/src/main.rs) | 实际来源与绝对期限进入 root；有限 Collector 同样通过 root 等待，独立 ABI 自检在管理者收集后运行 |
| ELF 装载与映像构造 | [supervisor/loading.rs](../user/services/srv_init/src/supervisor/loading.rs) | 同一 Preparation/ImageCopy 每轮推进一步；失败显式交回原收集账本 |
| 显式并发 Retry 的测试线程 | [driver.rs](../user/services/srv_init/src/driver.rs) | 保留并发调用用途；主线程通过 root 等实际 ThreadControl DONE，再 try_join 领取结果 |

表内范围包括当前默认 workload 中的验收代码，它们在管理者运行期间执行，不能因名字带 test 就排除活性影响。未来新增 root 路径同样不得直接 Caller::call、独立 WaitSet::wait 或 sleep 到外部响应；普通领域库的同步 API 可供其它调用者保留，root 使用对应分步入口，不往通用 Caller 注入 pump 回调。

### 唯一控制记录与调度

root 在启动任何管理者前预付固定两份 ManagerRecord、DONE/就绪观察容量、控制运输槽及故障收束存储。ManagerRecord 持本实例的 ProcessControl、实际 SourceAdmin、单调停止意图/期限及真实 DONE 快照；Job/Pool关闭权复用root原能力账本，领域装配记录只保留该实例的借用关联和私有就绪/入口。不存在第二套Shared/Record、监督admin副本或新Job/Pool账本。

实际 ProcessControl 在 Start 前挂入 root 的持续 DONE 来源；同一 owner 管能力关闭与来源 detach，不需向另一线程交还 capability。服务私有入口仍核真实 context/PID/Process；srv_device 的 SourceAdmin 在私有就绪后纳入同一记录，之前由原 Process/Job/reset 负责故障兜底。

在原 root 循环中增加有明确类型的管理/装配 duty，加入统一 receive/feed、期限汇总及轮转，不另起相互嵌套的 wait 循环。每轮所有活 duty 获至多一个有界推进机会，管理 DONE 与故障判断在普通装配动作前处理；持续业务 ready 不能饿死管理观察。知道故障后封新装配并只推进原终局责任，不能返回业务脚本。

顺序包装只做“提交操作→保存返回标识→await 该结果”，await 仍推进所有 duty；业务结果由实际任务保存，不以回调修改被等待栈。任务的调用/回复、来源注销和取消均有稳定 owner；一个 await 失败不销毁其它已提交责任。

### Loader 的必要前置

libprocess 以显式 `Preparation` 持有同一个 loader；普通 prepare/spawn 循环驱动，root 每轮推进一步。同步入口与分步入口共用验证、映射、写入及取消责任，不能把完整同步 prepare 包在 Task::advance 中当作有界推进。

准备 owner 持真实 Building 进程、稳定映像输入/游标、尚未消费的 grants、已经发生的 GrantOutcome 和失败清理责任。ELF 验证/规划分步遍历，映像/出生块复制按页推进；Map每步仅一笔，上界直接采用现有ABI `PROCESS_MAP_MAX_BYTES`，其它步骤各为固定出生操作。复制页粒度来自现有VM几何，不以任意大映像大小作为一次step的上界。Image::copy沿同一“未sealed构造owner→逐页映入/写/解除→seal”寿命拆分；构造和转交方不映入整份只读映像，实际读取者另持有限寿命的view。

施工修正依据：每次ProcessMap创建独立backing与region，逐页Map会把8MiB主栈从8笔放大成2048笔，真实装配在Start前即耗尽原metadata预算。Map的分步单位因此取原ABI单次上界，保留结构工作界，避免以扩大Pool预算维持映射对象膨胀；不新增内核异步Map协议。这改变内部批粒度，不改变出生、权限或取消合同。

现有顺序OAL/文件/服务客户端通过RPC的显式Exchange接口使用宿主调用入口：普通Caller保留同步行为，root使用原CallOperation的分步推进与实际来源，来源挂入原Assembly。该接口仅表达请求/回复运输，不携带supervisor类型或pump回调；root自身拥有明确推进循环。无DUP来源仍以原借用存活，不为接入调度复制权限，不另建一套领域编解码。

Create 前预留准备与取消责任；所有可失败步骤保留原 Building owner。失败/取消交原 Process 取消与 collector 机制，不能遗忘已消费 grants 或把请求停止当全部退款。Start 仍只消费完整 PreparedProcess，并必须在 DONE 来源建立之后执行。封口后的迟到 Start 由原 Job/Process 门拒绝。

分步装配已覆盖 root 上的实际启动调用者，是持续监督的前置约束。它保证每次用户态推进有结构工作界，不承诺确定墙钟延迟；内核平台/时钟故障仍按原终局处理。后续批量优化须依据实际装配成本，并保持明确的单步成本界。

### 正常停止与故障

正常停止先在同一 ManagerRecord 提交意图/绝对期限，再封相应 Job/来源并发管理 Stop。来源 Closed 通过现有 QuerySourceStop 的原谓词确定；这里可保留仅在收束期间的有限、定时任务重查，不能使用阻塞 sleep，也不为两个来源另造长期观察账本。整个系统运行稳定时没有这项周期工作。

root 取得真实来源 Closed 后，关闭自己已经无用的 admin/入口；收到 DONE 必须核 Completed 且无异常 detail，随后实际 detach 来源、交原 collector、关闭控制与 Pool。DONE 先到时保留快照等待原来源证明或期限；不把顺序差异当失败，也不从 DONE 反推安全。不存在 release_admin/disarm/ack/join 的线程间协议。

故障终局仍按关机计划：同一个 root owner 不可逆封装配，持唯一 Job/平台 Seal/reset authority，有限尝试来源封口和执行收集，然后 Reboot/SystemFailure；reset 拒绝终止 init，由 boot DONE 停驻。不存在两个线程争 Terminal，也不等待无静止证明的 DMA 或全额退款。终局 RPC/观察使用原绝对升级期限的较早界，不能伪造 collector 已超时。

### 费用、删除门与验收

管理记录、DONE/就绪/故障来源和有限控制调用槽来自 root 预付账户，容量由实际两管理者及并存装配操作推导；普通请求不能占控制清理预留。loader 的页面暂存按真实容量付款，其阶段结束才退；角色和等待来源全部退休后再关最后控制。没有后台监督线程的栈、ThreadControl 或 changed/ack Notification 费用。

上表的等待与装载已接回 root，device_supervision 的独立线程、Shared 锁、双 Notification、终局抢占、admin ACK、撤防及 join/reap 已删除。后续新增装配路径须遵守同一接入约束，不能恢复独立观察者来补偿主线的阻塞等待。

验证覆盖正常两管理者收束；另一业务等 RPC/报告/进程结果时管理者死亡；Start 后最早死亡；Stopping 中异常；完整 source Closed 与 Completed 的两种先后；loader 尚有大量页未处理时死亡观察仍推进；reset 拒绝后 init DONE 与原 DMA 保留。复用原故障消费者和现有执行测试，停止旧线程专属 ACK/join 窗口的复测，保留等价的责任合同。check/clippy、正式组合及证据版本按构建手册执行，本轮实现已覆盖相应正常与故障路线，具体证据和未覆盖边界见机制收敛报告，不外推任意交错。
