# 正式服务装配与监督设计

状态：**静态常驻服务基座及交付后结构清理已完成，所选验收与清理后回归通过；代码未提交，后续能力按条件触发**。本案拥有启动依赖、就绪、监督恢复与进程授权装配；基座交付证据从[实施接力](#静态基座实施接力2026-10-28)进入，后续清理与验证见[交付后结构清理](#交付后结构清理)，目标由[常驻基座设计](#静态常驻服务基座设计2026-10-28)拥有。发布发现由[OAL/服务分案](todo-2026-10-07-fal-service-redesign.md)拥有，系统终局与停止政策由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有，设备独立组合门及观察项由[设备计划](todo-2026-09-device-system-design.md#整体审查修复后的接续2026-10-28)拥有。

## 目标与非目标

把正式长期服务、资源管理者和驱动组合成有明确启动、授权、存活、恢复和停止责任的系统。能够回答谁启动谁、依据什么判断就绪、何时可向消费者交付能力、失败由谁接管、何时重启或升级为系统停止。

本轮静态基座不新增服务管理进程、manifest 文件格式、通用服务框架或独立电源管理服务。长期职责采用下节已确认的根监督与服务管理分层；不因需要正式系统装配就重写已完成的 Process/Job、Thread owner 或用户态执行基座。

### 已确认的 init 与服务管理职责

init 负责初始基础服务的装配，并在初始化后继续承担该必需集合的根监督。由 init 直接管理的基础服务崩溃，进入系统故障终局；这里的“系统恐慌”是系统级失败政策，沿既有终局与硬件安全收束合同执行，不新增由用户进程 fault 触发内核 panic 的路径。正常停止仍按已提交的停止意图和各领域完成事实判断，不把预期退出误作崩溃。

未来独立服务管理服务（暂称 `srv_service`，组件名称尚未定稿）本身也是由 init 启动和监督的基础服务，其崩溃同样进入系统故障终局。由它启动和管理的普通服务采用它自己的失败、重启、停止及恢复政策；普通服务崩溃不因其位于 init 的后代进程树中就升级为系统故障。根监督集合与受托服务集合按明确的管理责任划分，不把所有后代 DONE 都解释为基础服务死亡，也不为同一实例保留两套竞争的生命周期管理状态。服务管理器的具体授权、预算、管理协议及故障收束在接入时闭合，本轮不提前实现。

初始基础服务组合未来由 initfs 提供 init 专属服务描述，init 解释该描述并完成装配，其中包括服务管理服务。描述格式或脚本目前不设计，现阶段在 init 源码中硬编码选定部署。描述来源与长期监督职责分别建模；改为从 initfs 读取不意味着 init 退出根监督，也不要求把已启动基础实例的管理权整体转交服务管理器。

上述分层是已确认的长期边界。当前静态常驻基座继续实现 init 对选定基础宿主的启动、授权、持续监督和停止；该根监督可以长期保留，不能随普通服务管理需求增长而扩成通用服务管理器。`srv_service` 与 initfs 服务描述均为后续接入范围，本轮不声称已经具备。

## 当前事实与前置

原装配盘点基线为`f174d8e`，首批设备施工为`549a9e7..f3337eb`；第二轮在其上替换独立线程及重复握手，当前实现由root同一Assembly持续推进。启动与监督事实见[startup实现](../notes/impls/startup.md)、[设备实现](../notes/impls/device.md)、[framework实现](../notes/impls/framework.md)。

- `srv_init` 的固定 workload、RequiredLaunchSet、RootSupervisor 与已有 pm/fs 消费者承担验收装配和真实监督责任；不能把它们当成没有可用基座。
- 对象链、自动回收、Job 封口、进程出生/执行绑定、来源登记及运行时分层已完成相应交付。本案不恢复 ProcessDrain 或用户必须持续驱动的内核回收。
- 当前已有 Registry/服务记录；发布成功、服务就绪、endpoint 可用和进程存活分别表达不同事实。System 与验收部署共用宿主装配和监督，System 不包含验收消费者。
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

常驻基座的完成门由下方选定范围具体化；设备首批的进程分工、静态来源、授权和持续监督已有实现与所选验证，不代表一般服务热升级、持久化停止或配置脚本已设计完成。现有同进程fs/registry装配由发布分案接通，不要求提前拆出srv_service。第二轮实施证据由机制收敛报告归档，设备原独立门仍由主计划维护。

## 当前接力

本轮依据 `99f5e9b` 目标设计与 `caa1f5e` 职责澄清完成选定静态基座，验证、结构检查与证据边界见[唯一实施记录](#静态基座实施接力2026-10-28)。用户随后授权的[结构清理](#交付后结构清理)及针对性回归也已完成，没有待接续的本轮施工步骤。应用、管理器、initfs 描述、恢复和持久化按本篇条件表接手；本案继续拥有这些独立后续，暂不整体归档。当前代码未提交，也未授权 commit/push；获准提交后再登记固定代码交付的可选独立 Review。原[固定设计的独立 Review](todo-2026-10-28-service-foundation-design-review.md)仍仅对应原设计提交、可选未执行；HighNewt 的进行中检查属于协作结构检查，不冒充独立 Review。

设备第二轮两条责任链已随 `c5d005d` 提交；整体审查 G1–G5、C1 及公共 loader 修复已随 `e23afdb` 交付。可选独立审查、精确动态覆盖及候选出生观察仍由[设备修复后接续](todo-2026-09-device-system-design.md#整体审查修复后的接续2026-10-28)拥有，不重做 root/worker 等待和装载迁移。持久化、仓库安装更新、服务恢复与通用配置的条件后续见本篇延期表。

## 首批设备的持续监督接线

首轮固定检查点采用独立观察线程与WaitSet，和主线程通过停止/关闭/撤防确认协调；该形态已由第二轮替换。原合同与运行证据保留在[首批历史记录](archived/ref-2026-10-27-device-delivery-history.md#init设备管理者的独立观察与终局施工合同)，当前合同由下节拥有。

## 第二轮持续监督设计（2026-10-27）

本轮一致性重构已完成：同一loader、ELF验证及Image构造、实际装配等待和持续管理观察已接入root，旧线程已删除，所选运行验证与结构检查通过。施工过程与验证证据保存在[收敛交付档案](archived/review-2026-10-27-device-implementation.md)。选择 RootSupervisor 作为唯一监督事实和终局权限 owner，装配仍可顺序表达，但所有依赖等待都通过明确的 root 提交/等待接点推进全部责任。既有 Process/Job/Pool、Runtime/Assembly、RPC调用owner与 collector 继续复用，不建立 supervisor 服务或通用工作流。

### 已接通的装配范围

[`RootSupervisor::turn_and_wait`](../user/services/srv_init/src/supervisor.rs) 持一份 Assembly，分流 Job、Batch、Read、管理者观察及装配等待的来源，每轮推进各自有限工作，再汇总可运行状态、最早期限和本次等待结果。顺序装配在等待外部结果期间继续推进这些责任。

持续监督约束覆盖必需实例开始受监督后仍可达的路径，当前接点如下。第二轮最初覆盖两设备管理者，静态基座将同一约束扩展到文件宿主；首轮 `f3337eb` 的阻塞路径盘点与迁移过程保留在收敛档案。

| 实际路径 | 当前源码 | 接入方式 |
|---|---|---|
| 两管理者 bootstrap 等待、设备管理 RPC | [device.rs](../user/services/srv_init/src/device.rs)、[driver.rs](../user/services/srv_init/src/driver.rs) | root 的 receive/wait 与 Exchange；原 CallOperation 的来源进入统一等待槽 |
| 来源停止查询、DONE 与管理能力关闭 | [supervision.rs](../user/services/srv_init/src/supervision.rs) | 同一管理记录确认来源 Closed 与实际 DONE，注销后交原 Collector |
| srv_fs 就绪、发布及注册队列探针 | [hosting.rs](../user/services/srv_init/src/filesystem/hosting.rs)、[acceptance.rs](../user/services/srv_init/src/filesystem/acceptance.rs)、[registry.rs](../user/services/srv_init/src/filesystem/registry.rs) | 宿主报告进入持续监督；顺序 RPC 用 Exchange，Copy/Watch 保留原 Dispatcher，验收探针等待也推进 root |
| 报告、进程与验收阶段等待 | [supervisor/waiting.rs](../user/services/srv_init/src/supervisor/waiting.rs)、[main.rs](../user/services/srv_init/src/main.rs) | 实际来源与绝对期限进入 root；有限 Collector 同样通过 root 等待，独立 ABI 创建自检在基础宿主出生前运行 |
| ELF 装载与映像构造 | [supervisor/loading.rs](../user/services/srv_init/src/supervisor/loading.rs) | 同一 Preparation/ImageCopy 每轮推进一步；失败显式交回原收集账本 |
| 显式并发 Retry 的测试线程 | [driver.rs](../user/services/srv_init/src/driver.rs) | 保留并发调用用途；主线程通过 root 等实际 ThreadControl DONE，再 try_join 领取结果 |

表内范围包括当前默认 workload 中的验收代码，它们在管理者运行期间执行，不能因名字带 test 就排除活性影响。未来新增 root 路径同样不得直接 Caller::call、独立 WaitSet::wait 或 sleep 到外部响应；普通领域库的同步 API 可供其它调用者保留，root 使用对应分步入口，不往通用 Caller 注入 pump 回调。

### 唯一控制记录与调度

root 在启动必需实例前按选定部署预付 Record、DONE/私有报告观察容量、控制运输槽及故障收束存储。Record 持本实例的 PreparedProcess 或 Running Control、单调停止意图/期限及真实 DONE 快照；设备 SourceAdmin 与文件 Bootstrap 保存各自领域事实。Job/Pool 关闭权复用 root 原能力账本，领域装配记录保留实例关联和业务入口，不复制进程控制权或建立平行账本。

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

设备管理者正常停止先在同一 Record 提交意图/绝对期限，再封相应 Job/来源并发管理 Stop。来源 Closed 通过现有 QuerySourceStop 的原谓词确定；这里可保留仅在收束期间的有限、定时任务重查，不能使用阻塞 sleep，也不为两个来源另造长期观察账本。整个系统运行稳定时没有这项周期工作。

root 取得真实来源 Closed 后，关闭自己已经无用的 admin/入口；收到 DONE 必须核 Completed 且无异常 detail，随后实际 detach 来源、交原 collector、关闭控制与 Pool。DONE 先到时保留快照等待原来源证明或期限；不把顺序差异当失败，也不从 DONE 反推安全。不存在 release_admin/disarm/ack/join 的线程间协议。

故障终局仍按关机计划：同一个 root owner 不可逆封装配，持唯一 Job/平台 Seal/reset authority，有限尝试来源封口和执行收集，然后 Reboot/SystemFailure；reset 拒绝终止 init，由 boot DONE 停驻。不存在两个线程争 Terminal，也不等待无静止证明的 DMA 或全额退款。终局 RPC/观察使用原绝对升级期限的较早界，不能伪造 collector 已超时。

### 费用、删除门与验收

管理记录、DONE/私有报告来源和有限控制调用槽来自 root 预付账户，容量由实际部署实例及并存装配操作推导；普通请求不能占控制清理预留。loader 的页面暂存按真实容量付款，其阶段结束才退；角色和等待来源全部退休后再关最后控制。没有后台监督线程的栈、ThreadControl 或 changed/ack Notification 费用。

上表的等待与装载已接回 root，device_supervision 的独立线程、Shared 锁、双 Notification、终局抢占、admin ACK、撤防及 join/reap 已删除。后续新增装配路径须遵守同一接入约束，不能恢复独立观察者来补偿主线的阻塞等待。

验证覆盖正常两管理者收束；另一业务等 RPC/报告/进程结果时管理者死亡；Start 后最早死亡；Stopping 中异常；完整 source Closed 与 Completed 的两种先后；loader 尚有大量页未处理时死亡观察仍推进；reset 拒绝后 init DONE 与原 DMA 保留。复用原故障消费者和现有执行测试，停止旧线程专属 ACK/join 窗口的复测，保留等价的责任合同。check/clippy、正式组合及证据版本按构建手册执行，本轮实现已覆盖相应正常与故障路线，具体证据和未覆盖边界见机制收敛报告，不外推任意交错。

## 运行期驱动出生的分步接线（2026-10-28）

本轮整体 Review 的 C1 已纳入授权范围。采用输入 owner 与无内部借用游标：ELF Validation 持输入，ISA/架构字符串只持原映像范围和游标；借用与拥有输入共用同一个验证器。ImageView 独立保存长度和只读 MappedRegion，映射强引用保活 backing，不借用 Image，也不复制完整映像。

Preparation 保留借用入口，增加封闭的拥有型输入（ImageView、payload Vec、逐项 OwnedGrant）。两种输入共用同一阶段机。拥有型构造立即接管输入，初始校验错误冻结在原 Preparation，由第一次 advance 返回；取消和 close_inputs 显式释放，失败保留原 owner。ProcessGrant 成功的同一步解除 OwnedGrant 本地关闭责任，GrantOutcome 继续记录是否消费；借用入口的调用者责任不变。完成/成功取消后才允许关闭输入，取消 Collector 与装配者映射清理分别确认。

原 ProcessTask 的出生按 Admitting→Preparing→Prepared/附着DONE→Running 推进：每次调用一次公共 Preparation.advance；准备完成保存实际PID和Control，关闭输入映像成功、DONE来源实际 registered 后才 Start。Grant前后失败或Stop立即取消原Builder、保留同一Collector；源仍安装/注销或Run仍关联时不驱动Collector关闭Control。取消Collector的PID先进入原Record，确保结果可定位。出生绝对期限不刷新；正常Stop不产生候选失败原因，Start成功后仍沿既有Released/Detach→Kill/Collector链。

Pool、bootstrap mailbox与缓冲由原worker owner贯穿出生/退出；Preparing与Prepared不同时持有同一进程Control，运行或取消之后才由SuperviseTask独占它。没有新增线程、运行体、loader或恢复账本。一次map/unmap内核调用仍可能等待VM事务，本目标消除整份ELF装载和同步回收循环，不声称Task单步有严格墙钟时延上界。

验证目标包括拥有输入的验证器跨步移动、原ELF/ISA fixtures及首错；运行期已有绑定时新worker出生，正常Stop与DONE仍可推进；取消/失败区分Grant前、Grant后、DONE安装中及运行期，真实USB复用与故障路线复用原消费者。本轮已通过与未逐窗注入的边界分别见整体报告及设备主计划的修复后接续，不能把本段目标当成全部运行通过。

## 静态常驻服务基座设计（2026-10-28）

本轮为设计任务。用户授权推进下一步，并在必要时与 BrightReed 讨论以收敛设计、避免不必要的机制扩张。以下为主会话核对代码并经两轮讨论后的收敛结论，不冒称逐项由用户选择；未改变已有设备安全、目录发布或内核退出合同。基线 `f6b712e`，进入时工作树干净。ideas 未扩大审定，impls 继续描述现有实现。

### 目标、用途与现状证据

交付目标是**静态常驻基座**：init 持续监督必需服务，向受权消费者交付真实入口，直至自身政策提交正常停止或进入系统故障终局。常驻生命周期独立于验收脚本结束；空闲时等待真实来源，不靠心跳、循环探针或永久 runnable 维持存活。首批给后续应用铺路，暂不承诺用户可见工作流。

| 当前事实 | 源码依据 | 对本轮的约束 |
|---|---|---|
| init 默认执行确定验收并在结束后关机 | [main](../user/services/srv_init/src/main.rs) 的 `main/run/launch_test_services` | 增加明确的系统部署入口，不能只删除最后 reset 或把失败清理循环当常驻运行 |
| fs 主实例同时持独立文件与 registry provider；次实例只有文件并注册 `fs.secondary` | [fs boot](../user/services/srv_fs/src/server/boot.rs)、[部署容量](../user/services/srv_fs/src/lib.rs) | A/B 两组付款入口与双实例是验收配置；领域实现和收束路径由所有配置共用 |
| 文件 backend 出生为空目录，数据在内存中；当前没有磁盘后端 | [MemoryBackend::with_bindings](../user/libraries/libfs/src/backend.rs) | 不增加启动归档挂载、驱动仓库、flush 或断电恢复的承诺 |
| pm 的域没有 CREATE；实际负载是 Tunnel/流控及 test_target 收束 | [pm](../user/services/srv_pm/src/main.rs)、init 的 `pm_grants` | 保留其验收用途，不把二进制名称当成新增正式进程管理服务的理由 |
| 两设备管理者已在 Start 前挂 DONE，root 等待与 loader 会推进监督 | [supervision](../user/services/srv_init/src/supervision.rs)、[loading](../user/services/srv_init/src/supervisor/loading.rs) | 扩展已有 root 监督责任，不增加线程、独立 supervisor 或新的执行框架 |
| fs bootstrap 目前从首条消息取 PID，等待 Ready/Stopped 使用无限期限 | [filesystem](../user/services/srv_init/src/filesystem.rs) 的 `accept_fs_provider/confirm_primary_fs_ready/collect_fs_provider` | 迁移为出生前绑定身份、持续 DONE 观察与有限部署期限；这些接点属于本轮必要闭包 |

首批系统部署选定 init、srv_device、srv_driver、按需驱动和一个 srv_fs（包含 registry）。srv_device、srv_driver 与 srv_fs 三个长期子宿主均为必需；没有键盘或当前平台没有可用设备不是启动失败，驱动仍按原声明和实际硬件启用。fs 初始为空内存目录，registry 初始可为空，boot 映像继续直接从现有启动来源取得。init 是这些管理/发布根的实际接收者；未来应用通过下述授权交付路径接入，不能以尚无应用为理由伪造示例常驻消费者。

系统部署不启动 srv_pm、secondary fs、test_fs、test_target 或压力负载。现有 core/stress/memory/device 验收仍按各自合同选角色；复用同一个服务装配和生命周期，所需 A/B 额度及跨 provider 场景保留。新增显式 `system` 部署选择及对应 just 运行入口，打包仅携其必需映像；选择与既有验收 feature 互斥并在构建入口拒绝冲突。具体 recipe/feature 名称由施工时的 Justfile 同步定义，内核与 BootPackage 不增加部署政策。

### 领域、宿主与四类关系

fs 与 registry 继续共宿主，独立 mailbox、授权、状态和领域协议。这个实例被部署政策选为必需基座；本轮没有业务状态丢失后的重建、旧 capability 处理与重连保证，因此宿主故障进入系统终局。易失内存本身不推出“所有内存文件服务死亡都必须停系统”，该政策只属于 init 的这一部署实例。拆成两个进程在当前共同失败政策下不增加保证，暂不引入其进程、付款、监督与停止成本。

| 关系 | 常驻基座的 owner / 边界 |
|---|---|
| 进程与 Job 管理 | init 持 services 上级及文件宿主的 ProcessControl；设备三 Job 关系保持原合同。文件宿主是 services 直接成员，无创建子进程的 Job grant；无需为单个无后代宿主再建子 Job |
| 内核付款 | 文件宿主用 init 派生的独立双维 Pool，ProcessCreate 与交付给宿主的 CREATE grant 指向同一 Pool；init 保留观察/关闭 owner。root 自身的监督、运输及启动临时分配付 root，不从服务账反推管理权 |
| 用户态费用 | fs/registry 继续在宿主 Budget 中分领域；基础设施账户与入口账户不同。系统部署只发行一组实际入口；验收主实例发行两组。不是新增不可信域硬容量隔离 |
| 授权 | 私有 bootstrap sender、停止 Notification owner、文件管理 receiver、registry receiver 与 Pool grant 由 init 出生交付。文件/目录/发布根回到 init，普通消费者不拿根管理、停止 signaler、Pool CREATE 或 ProcessControl |
| 业务供应 | 文件/目录无设备依赖；驱动从 boot 来源装载，不等待 registry；消费者在实际所需入口就绪后启动。停止先退消费者，再退它依赖的 provider，Job 树不替代供应关系 |

`libprocess` 继续拥有出生、DONE 与 Collector；`libexecution` 拥有等待和执行；`libfs`、`libservice` 各自拥有业务。root 的装配政策留在 srv_init，不把服务名单、重启政策或停止图放入这些公共库。

| 目标模块 / 关键持有 | 修改边界 |
|---|---|
| `srv_fs::startup`、`srv_fs::deployment` | 前者拥有宿主 profile 编解码与 grant/报文数量合同，后者集中容量和阶段期限；`server::boot` 组合原领域 provider |
| `srv_init::supervision` | 本地 Record 持唯一进程阶段与 DONE 来源，系统 Terminal 归 root。设备来源/平台封口仍调用 `device`、`driver` 的领域接点 |
| `srv_init::filesystem::{Host,Bootstrap}` | Host 持业务根与实例关联；Bootstrap 拥有私有报告、release/Pool 关联和就绪/停止事实，由同一 Record 持有。生命周期句柄不再复制到对外 Host，文件业务与批次退款验收位于 acceptance 子模块 |
| `srv_init::deployment` | 拥有静态 profile 的映像选择、装配、常驻推进与内部停止入口；main 选择正式或验收编排，验收 `run` 调用相同宿主装配入口 |
| 原 `supervisor` 与公共库 | RootSupervisor 继续唯一持 Assembly、能力账本和收集责任；已有 loader/RPC/Collector 及领域协议复用，不增加公共服务状态机或 facade |

### 文件宿主出生、就绪与授权交付

`srv_fs` 的库目标增加宿主私有 `startup` 模块，供同包二进制与 init 共用配置编解码。只表达三个确定 profile：System（文件与 registry、一组入口）、AcceptancePrimary（同宿主、两组入口）、AcceptanceSecondary（文件、一组入口、原发布接收）。payload 固定为两个 little-endian u32：version=1，profile=0/1/2；拒绝其它值、长度与旧空串/`secondary` 解释。五个应用 grant 槽仍为 bootstrap sender、release Notification owner、文件管理 receiver、registry receiver 或 secondary publication inbox、实际 Pool。发送/接收两侧同次迁移，不保留兼容解析。

profile 仅选择有限部署政策；它不是授权。采用前核实际角色、权限、槽数和预期 receiver/Pool；不因 profile 或自报 PID 获权。System 与 Primary 共用初始化和退出；入口数量及账户额度从 profile 导出的同一个配置计算，不保留固定 A/B 数组之外另一套系统实现。

文件宿主的启动责任按以下顺序推进：

1. root 准备实例记录、Pool、bootstrap receiver 与接收存储、停止 signaler、目标 grants、DONE 来源容量及失败收束承载，再创建/装载进程。私有 sender 不交无关进程；真实 sender context 在移交前查询保存在记录，预期 PID 来自 PreparedProcess。
2. loader 每步用原 Preparation；完成后把唯一 PreparedProcess 交同一持续观察记录，DONE 实际 registered 后才 Start。准备失败/停止通过原取消 Collector 交接；GrantOutcome 仅决定未消费能力的释放，不充当回滚证明。
3. 宿主准备 backend、入口角色、控制/业务 Ingress、release 观察、退休及运输任务。必要来源全部获得实际安装回执后，才能交根并发 Ready；`spawn` 成功或来源请求已排队不能替代安装回执。Ready 不依赖任何验收请求。
4. 沿用四种现有根消息：文件、文件管理、目录、发布 authority；每条空正文，System 各一能力、Primary 各两能力。Ready/Stopped 均空正文、零能力。Secondary 仍按现有管理根及 Register 完成合同交付，数量由同一 profile 定义。接收者按种类收齐，不依赖不同来源通知被领取的顺序；重复种类、额外能力和错误身份均拒绝并走原实例失败收束。
5. 每条消息核出生记录的 PID 与 `sender_context_id`，能力经 typed 采用并核角色/输出权限。根只先存入私有实例记录；收齐合法 Ready、真实进程仍 Running、root 准入仍开放后才允许向消费者交付。Ready 与 DONE 同轮出现时不得开放入口；检查后死亡仍由持续监督接管，不能承诺无故障窗口。

bootstrap 使用已有消息 kind，不增加通用 Lifecycle RPC、Ready ACK 或按名字反查管理入口。System 最多四根+Ready+Stopped 六条，须以编译检查证明不超过共享 ABI mailbox 容量；每条能力数同样核 ABI 上限。初始消息额度有独立准入，正常报告不被公共业务积压挤占；发送失败是部署失败，不静默忽略。

首批授权交付的真正入口是 root 持有的根能力，不要求注册 `fs.memory` 或自造固定服务名。文件访问通过既有 fs Derive 收窄路径与 FsRights，不能只去掉 sender 的 GRANT 位就声称业务只读。目录客户端只在部署政策允许其发现整个本部署目录时，取得已发行 DirectoryAccess 的别名：保留原 Scope::Root 与 DESCRIBE/ENUMERATE/GET_ENTRY/WATCH 权限，不含注册/发布权。当前 service 协议没有远程目录 Derive/Restrict，不宣称逐客户端收窄目录可见范围；本轮不为此新增接口。确需发布的提供者通过现有 DelegateName 取得 exact-name 发布权，文件 Bind 管理根和 registry 根发布权留 init。派生请求若结果未知，停止该未完成消费者装配并由原请求/能力寿命退休，不盲签第二份权限。

入口准备、目标 Pool 与进程监督准入完成后，经现有出生 grants 或受认证的一次运输交付。Grant/Send 成功才解除发送侧唯一 owner；失败保留未消费 Packet/Capability，取消移交原 Collector。root gate 在派生、Start 和后续交付前复核；已提交的交付/迟到结果归原记录收束，不因等待调用返回错误而遗忘。Namespace 由消费者本地持有，没有全局路径隐式重绑。现有 init/test_fs 验收通过同一出口取得文件派生、目录访问与 exact-name 发布角色，各按真实权限验证；未来应用接入时再确定目录布局和业务授权。

### 一个实例 owner、持续监督与停止事实

root 内将当前设备专属的进程观察骨架扩为本地长期实例记录，供两管理者与文件宿主共同使用；不是新增公共服务框架。记录独占 PreparedProcess/Running Control/Collector 的阶段持有及其 DONE 来源，绑定不复用的本次实例身份（实际进程、Control、私有 bootstrap context）。领域记录保有各自入口与停止结果，通过稳定记录关联；不另存第二份 ProcessControl owner 或平行 PID 账本。

| 阶段 | 可观察含义与允许动作 |
|---|---|
| Preparing | 输入、Building owner 与失败取消责任已准入；无对外入口 |
| Starting | DONE 已挂，Start 已提交；接私有根与 Ready，尚不交普通消费者 |
| Running | 本宿主就绪成立；root 可按实际授权装配消费者，DONE 持续可观察 |
| Stopping | 原停止意图与绝对期限已提交，禁止新交付；原业务与进程终态分别收集 |
| Retiring | 正常停止所需事实已齐，或准备取消已交 Collector；先实际注销所有借用 Control/receiver 的来源，再关闭对应 owner |
| 记录释放 | 原 Collector、输入映射、入口/报告与 Pool 的本地持有均已结束；不伪称跨域仍持有的 backing 已退款 |

阶段只表达责任转换；Ready、Stopped、DONE 等具体事实从各自 owner 得到，不复制 registry 的 Active/Withdrawn 为监督状态。异常状态由 root 的唯一故障原因和系统终局承担，不再造恢复事务。所有事件由 root 同一 Assembly/Runtime 推进，无新共享锁、观察线程或跨线程 ACK。

文件宿主正常停止复用现有 release Notification。root **先**写 Stopping 意图与截止时间，再 signal；Notification 重复提示只维持停止事实。宿主封新文件/管理/目录业务，已准入调用、流、映射、注册、Watch 与来源仍沿原 owner 退休；清理接口不因封普通准入而失效。root 取消自己持有的客户端操作并释放已经无用的文件/目录/发布别名，不能一边保活它们一边等宿主报空。宿主在 Runtime/Assembly 和领域账确实收空后发 Stopped，再正常退出。

root 保存两项独立单调事实：认证的 Stopped，以及真实 DONE 快照。正常完成必须同时具备 Stopped 与 Dead/Completed/NONE detail；之后注销来源、交唯一 Collector、关闭报告接点与 Pool owner。DONE 先被处理时保留原 receiver，继续取已经入队的 Stopped；Stopped 先到时仍等 DONE。异常 DONE 立即升级，不能被 Stopped 掩盖；Running 期间未提交停止的 Completed 也是必需实例意外退出。超时前有界复查真实 DONE 和原 mailbox，已存在的完成事实优先于期限；缺报告不无限等待，也不用 DONE 代替业务收束。

停止先于 Ready 时，root 保留原接收责任，接到的根只用于释放，迟到 Ready 不开放交付；已运行宿主沿同一停止路径退出，未 Start 的进程取消 Building。异常身份/格式使部署失败，收到的未采用能力仍由消息 owner 关闭。停止报告不增设提交确认、结果重传或持久事务。

设备的完成谓词仍为原来源 Closed、Binding/Provision 安全及 Process Completed，不能套用 fs 的 Stopped 替换；共同观察骨架只汇合已由领域确认的结果。原 `device_supervision::Terminal` 的唯一终局持有提升到 root 系统层，现由 `supervision::Terminal` 实现；平台封口和设备来源清理由原领域接点提供，fs 失败也到这一出口，不再因“不是设备错误”进入无限失败服务循环。

### 发布未知与恢复边界

System 的初始根直接交付，没有首次按名注册事务。验收 secondary 及未来确需发现的服务仍使用既有单步 Register；启动者/提供者保留原调用和 RegistrationControl，实际发布成功才可宣告该名字的发布完成。Registry 不维护进程健康、不启动服务、不替调用者恢复。

对于本轮的必需实例，启动失败、Ready 超时、运行期意外退出或停止失败都封装配并进入系统故障终局；不自动重启。已发送 Register 的未知结果不重放，不用名字存在推断自己成功，不恢复被 Replace 撤下的旧注册；原控制与运输最后释放后的目录退休继续由 registry 承担。整个必需部署失败时停止其 owner，未决结果不阻止有界故障升级。可选业务服务如何继续运行、显式重启与重新授权不在本次承诺中。

后续若承诺恢复，必须先选定可丢弃/可恢复状态、新实例授权、旧会话结果与重连政策。客户端已有 Namespace/endpoint 不被注册替换自动改变；显式 GetEntry 得到新实例也不允许自动重放旧实例上的未知变更。这些边界复用发布分案，不新增两阶段 Register、代理入口或透明重连层。

### 准入、费用、期限与调度

静态部署描述只列实际映像、宿主 profile、必需角色和付款政策；先校验重复/缺失映像及输入，再发布子进程。root 记录及来源容量按本部署会并存的长期实例计算：System 三个宿主，验收可另含 secondary；不能把“固定三项”藏成公共库上限。每个长期实例独立预留 DONE 和必要的 bootstrap 观察，已有单次 RPC 等待槽/Collector 容量依实际并发复用；不为每实例创建线程、独立 Assembly 或一套 RPC 引擎。

文件 Pool 的页预算覆盖实际 ELF runs、ABI 主栈/出生映射及显式配置的堆/数据工作集；metadata 预算覆盖实际映射、对象、能力、来源与退休峰值。部署值集中在 srv_fs 的容量政策，施工按装载布局及现有工作集核算、验证正常准入和拒绝回退，不从复制设备 Pool 的魔数推导保证。用户态 task/source/缓冲预算由 profile 的真实入口、请求、流、注册/Watch 并存上界推导并 checked arithmetic；停止/报告和退休承载先于公开入口预留。允许超出部署容量的新业务正常返回资源错误，不承诺物理堆耗尽后任意新分配成功。

文件宿主启动和停止各取一个部署级绝对期限；初始政策均为 10s 的故障检测窗口，分别从准备开始和停止意图提交起计，集中由 srv_fs 部署配置拥有。这是本轮选择的运营等待政策，不是从 Collector 重试次数推导的硬件界或 WCET；实施在所选 QEMU/节流条件核验，若有界进展触限按构建手册增大并记录。各 RPC 使用该阶段剩余期限与领域单次期限中较早者，部分根/Ready/Stopped/DONE 不刷新期限。设备既定阶段期限保持各自唯一来源；稳定 Running 无周期健康探针。系统停止的聚合与故障升级期限由关机案拥有。

所有新路径沿 `RootSupervisor::turn_and_wait` 接入 receive/feed、有限轮转与最早期限；DONE/已知故障先于新的装配动作。每个实例每轮至多处理一个 bootstrap 消息或推进一次来源/进程操作，批量授权/清理用游标续进；收到无限业务提示不能挤占监督和清理机会。没有就绪工作时交 Assembly 停驻。启动/取消、调用出错、Collector 失败均保留原责任，不能把局部返回错误解释为可丢弃实例记录。

### 正常停止接点、迁移与验证门

系统正常停止的唯一政策 owner 为 init。首批只提供 root 内部停止入口，验收编排通过它停止同一个部署；普通服务和消费者没有系统停止权，也没有外部关机客户端。系统模式不会因无应用或短任务自然结束自行停机。未来外部管理界面需显式授权协议，不能通过全局目录、PID 或关闭任意 sender 获得停止权。完整聚合由[静态常驻基座停止](todo-2026-09-system-shutdown-orchestration.md#静态常驻基座停止2026-10-28)拥有。

施工按同一闭包依次完成，当前没有依赖新增内核机制的未决前置：

1. srv_fs 集中 profile/启动契约及容量；System 单组入口与验收 A/B 共用初始化、业务和释放。同步 init 发送/解析及现有 test_fs 的实际额度读取，删除旧字符串启动解释和固定两组假设。
2. root 把长期实例的进程持有/来源与唯一终局归到同一 owner，接通 fs 的预期身份、有限准备、Start 前 DONE、Ready 门和持续监督；移除这些实例在普通批次中的重复 Control 持有及仅阶段末收集路径。普通短任务仍可用原 Batch/Collector。
3. 正式部署与验收编排分别选择同一装配入口，加入常驻驱动循环和内部停止入口；验收仍用真实 fs/registry/设备消费者。删除为适配旧生命周期保留的无限 bootstrap 等待及独立 fs 释放流程，不复制另一套“system provider”。
4. 系统停止接入文件宿主和消费者清理，再接既有设备停止与唯一 reset；root 失败接点只保留共同终局。同步 Justfile 打包、配置冲突检查、构建手册、startup/service 等实际 impls；只有代码落地后回填实现事实。

| 要捕获的错误 / 独立合同 | 最小验证边界与判据 |
|---|---|
| 默认流程仍随测试结束退出，或常驻只是空循环 | 显式 system 部署观察真实三宿主就绪、无测试进程、持续 DONE 监督及无待办时停驻；有限观察不宣称无限活性 |
| Ready 过早、身份自认或停止后迟到交付 | 共用启动解码及真实 fs 宿主；错误 context/PID/能力布局拒绝，来源准入后才 Ready，Stop 先到后无新消费者 Start/交付 |
| 文件启动/RPC等待饿死其他监督 | 复用已有外部 GDB 故障路线，在实际 fs 准备或等待时终止必需实例；一次进入 root 终局，不等待原业务超时才发现 |
| DONE/Stopped 领取顺序造成误判或漏报 | 原实例记录和 fs 报告：两种顺序都需双事实才收集；缺 Stopped 到期失败；异常 DONE 或未 Stop 的 Completed 不成功。来源退休回执前不关闭借用 owner |
| 业务饱和占用停止承载，或提前退款 | 原 test_fs/流/Copy/Watch 保持非空并停止，普通请求被封、在途 owner 退休、报告与实际 Completed 匹配；无跨域持有的工作集在 Collector/最后 owner 后核原 Pool，不能仅靠日志“预算空” |
| 原 A/B、secondary 发布与设备安全接线被改变 | 原 core 与 device 消费者沿同一生命周期完成，既有 Unknown/名称替换/热插合同保留；按实际修改触发精确补证，不重跑无关历史异常 |

host 验证优先复用解码、来源与生命周期设施；新增用例仅针对表中可观察反例，不另造 provider/运行体。实施收尾执行 `just check`、`just clippy` 及构建手册要求的受影响 core、平台、设备停止/故障组合；系统部署的有限观察实例由操作者清理，外部收割不是正常停止通过证据。设计提交时未运行代码验收；后续实施证据由下方实施记录拥有。

### 条件后续与设计交付检查

| 延期能力 | owner、触发与完成条件 |
|---|---|
| 用户可见应用及外部正常停止请求 | 服务装配/应用接入者；出现真实应用或管理 UI 时，确定目录/名字、最小授权、停止调用权及真实消费者。此前仅声明基座，不制造占位客户端 |
| 按名字限定目录可见范围的远程发行 | libservice 与 OAL/服务目录改造计划；真实消费者不能被授予现有整个目录访问角色时，先闭合目录授权发行协议及来源/付款/撤销，再接入该消费者。当前不以 sender 运输缩权冒充业务范围缩权 |
| 服务恢复、可丢弃实例及客户端重连 | 本计划；出现必须在某服务失败后继续可用的需求时，闭合状态恢复、旧权力/未知操作与新实例边界并验证真实故障。当前必需实例失败政策到该合同替换时才删除 |
| 独立服务管理服务 | 本计划；长期分工按[已确认职责](#已确认的-init-与服务管理职责)，后续接入时闭合受托授权、付款、普通服务失败政策与系统停止接点，并验证普通服务崩溃不升级系统终局、管理器自身崩溃由 init 接管。本轮保留 init 的根监督，不新增管理器占位进程 |
| initfs 中的 init 专属服务描述 | 本计划；启动描述工作启动时确定格式、输入校验及装配映射，替换源码硬编码的初始组合；验证相同描述对应相同初始授权和依赖，失败仍由 init 收束。此前不增加脚本解释器或通用 manifest 框架 |
| 独立 registry 宿主 | 本计划；独立信任/付款/升级/可用性需求成立时重审，须有相应真实装配与删除旧布局的证据；领域独立不是拆进程的充分理由 |
| 正式受托进程管理服务 | 进程领域/服务装配；真实工作负载需要委托启动或持续管理政策时设计。现有 srv_pm 验收保留，不先更名或包装成正式管理 API |
| 持久文件系统、仓库及安装更新 | 文件扩展/设备仓库与关机各原计划；真实数据/仓库消费者出现前闭合介质、来源、flush/恢复和停止依赖。内存目录不冒充 bootfs |

外部对照用于检查职责分离，不能直接引入对方框架：systemd **v257** 的 [Type/Restart](https://www.freedesktop.org/software/systemd/man/257/systemd.service.html) 区分进程启动、READY 和重启政策，[unit ordering](https://www.freedesktop.org/software/systemd/man/257/systemd.unit.html) 区分依赖与次序；Genode Foundations **24.05** 的 [3.2.1–3.2.4](https://genode.org/documentation/genode-foundations-24-05.pdf) 说明父级路由、服务公告和 session 寿命，未据此获得自动重连保证。Fuchsia 官方 lifecycle 在线页只作发现材料，未固定版本，不作为本轮合同证据。Halcyon 的选择来自上述实际 owner、目标与故障边界。

BrightReed 的只读讨论与主会话判断收敛为：可先交付有真实授权出口和生命周期的铺路基座；共同故障政策使当前拆宿主没有独立收益；不自注册、不新增生命周期 RPC；文件停止按独立 Stopped/DONE 事实汇合。定稿走查发现目录没有远程缩权发行接口，主会话亲核 service Request/AccessRights 与 Root::prepare 后，将承诺收窄为既有目录角色交付，不为填充方案新增 RPC。主会话亲核上述关键代码，拒绝把易失状态当作普遍的故障终局理由。

交付前结构检查：范围与非目标、正常/失败/取消/退出、授权与付款、唯一进程持有、来源退休、停止与终局已有确定归属；没有新增锁序或内核 ABI，公共目录协议不扩展。持久化、恢复和外部客户端是有触发的能力后续，不是本轮未闭合 owner。设计只对选定基座闭合，D01/D02/D07–D09 的其它未来范围保留。文档验证与提交状态由本节末尾追加；提交后按 REVIEW 登记固定交付的可选独立审查，不把本轮讨论当作独立 Review。

设计轮文档验证：5 份变更均为 plans 文档；33 个新增本地链接、20 个标题锚点与代码围栏检查通过，`git diff --check` 通过。BrightReed 的意见仅作为设计收敛与所核源码事实，未执行独立 Review。该轮未修改代码、impls 或 ideas，未运行编译、单测、QEMU。用户随后授权提交，设计固定为 `99f5e9bb3b9afd436b0e3aed4acd3662bc0aa698`，可选独立审查已另行登记；仅本地提交，未 push。该设计轮证据不替代下方实施证据。


## 静态基座实施接力（2026-10-28）

授权基线 `caa1f5e`，分支 `refactor/system-redesign`，进入工作树干净。用户经 HighNewt 确认整轮实施；FairRiver 主写代码、impls 与本记录，HighNewt 只读结构检查；后续代码未授权 commit/push。本轮继续采用上述同一闭包与验证门，不拆成独立交付。

已完成 Profile/Capacity、同一 Runtime 的必要来源回执门、本地 Instance/Record 的唯一 Control 与 Start 前 DONE/私有 bootstrap 观察、真实 Running 交付门、System 静态装配与记录集合驱动的停止。所有普通 ABI 创建自检已前置；验收 primary 保持运行至全局停止，secondary 保留非空 Copy/Watch 的局部退休。原 partial-admission/失败隔离使用已完成 pm 与已实际 detach 的 secondary Control 交原 Batch，未复制 Control。正式根由 root 持有并由共同停止释放；停止、来源注销、Collector 和根 Job 收集均纳入冻结总期限，完成后保留 Completed。

开发证据：`just build_user` 通过，日志 `artifacts/run/service-foundation-build.log`。旧中间态两次 core 因错误的 Pool rights、根 Packet 的 At0 期限触发共同故障 Reboot；不是有界正常推进撞限，未扩大路线期限。现场分别为 `artifacts/failed-acceptance-20261028-143329-13556.log`、`artifacts/failed-acceptance-20261028-143449-14860.log`。修正后 core debug 开发路线通过：`artifacts/run/20261028-143850-15788-acceptance-virt-debug.log`；该现场早于根终局 owner、总停止期限及根收集收口，只作为中间开发证据。

主要落点：`srv_fs::startup::Profile` 与 `deployment::Capacity` 拥有布局/预算推导；`srv_init::filesystem::{launch,hosting}` 拥有出生交付与认证报告；`supervision::{InstanceTask,Record}` 拥有长期进程与实际来源；`deployment::{system,Stop}` 拥有部署集合与停止聚合。旧 `device_supervision`、空 payload/secondary 字符串解释、独立 fs 阶段末收集和无限失败清理路径已删除。impls 已同步 startup/service/fs/device 的当前结构。


### 最终验证与覆盖边界

以下均保留完整日志；基座交付汇总为 `artifacts/service-foundation/results.txt`，中间返回码原表保留在同目录 `attempt-results.txt`。平台聚合首次未完整通过，不能把它的总返回码当最终失败或通过；sifive_u 原成功与 nofd 修正后成功分别列出。本表记录结构清理前的交付验证，不把早期开发检查重复计算为完整验收；后续清理涉及的回归由[清理记录](#交付后结构清理)单列，未变化的专项继续引用本表。

| 验证门 | 最终结果与日志 |
|---|---|
| `just check` / 十一分面 `just clippy` | 通过；`artifacts/run/service-foundation-check-final.log`、`artifacts/run/service-foundation-clippy-final.log`，分面详情在 `artifacts/lint/` |
| srv_fs host 协议与容量 | 两项通过；`artifacts/run/service-foundation-host.log`，显式 `aarch64-apple-darwin`，验证错误 payload/旧格式拒绝与 profile 容量关系 |
| release core | 通过；`artifacts/run/20261028-144615-23859-acceptance-virt-release.log` |
| debug stress | 通过；`artifacts/run/20261028-144710-25912-virt-stress.log`，保留原业务/额度/退休场景 |
| sifive_u / nofd 完整 core | 分别通过；`artifacts/run/20261028-145143-35848-acceptance-sifive_u.log`、`artifacts/run/20261028-150032-40110-virt-nofd.log` |
| 正式 System 有限来源观察 | 通过；`artifacts/service-foundation/system-observe-fixed.log`，证据 `artifacts/system-observe/20261028T070958.721125Z-44790/evidence.json` |
| device 非空业务与正常停止 | 通过；`artifacts/run/20261028-151116-45246-acceptance-device-release.log` |
| Driver 在 fs 装载期间故障 | 通过；`artifacts/run/20261028-151211-47371-acceptance-device-manager-fault-Driver-loading.log` |
| Filesystem Start / 业务等待 / 停止中故障 | 分别通过；`artifacts/run/20261028-152358-50998-acceptance-device-manager-fault-Filesystem-start.log`、`artifacts/run/20261028-152553-51993-acceptance-device-manager-fault-Filesystem-wait.log`、`artifacts/run/20261028-154047-56311-acceptance-device-manager-fault-Filesystem-stopping.log` |
| ColdReboot 被拒后的根终局与 DMA 保留 | 通过；`artifacts/run/20261028-154551-58001-acceptance-device-reset-rejection.log` |
| 两个领取顺序及完成优先 | 通过；`artifacts/run/20261028-162414-88845-acceptance-fs-stop-order.log`，证据 `artifacts/fs-stop-order/20261028T082417.710357Z-89215/evidence.json` |

System 证据限于三必需宿主真实就绪、无验收进程、root 在已 attached 的真实来源上 Infinite 等待且没有 deadline timer，以及恢复运行后的有限存活观察；不宣称四 hart 全 idle 或无限活性。外部进程最终被工具清理，不作正常停止证据。正式停止与故障路线复用同一装配，其全局停止实际推进仍存活的 primary→Driver→Device，保留设备独立安全完成谓词。

双领取顺序的准确结论：secondary 正常领取 Stopped 先（`DONE_seen=false, stage_expired=false`）；primary 的实际 Dead=3 与原 DONE token Hint 已形成，外部仅延迟 root 调度后，在原阶段到期复查中 DONE 先（`DONE_seen=true, stage_expired=true`）。真实 tick 增量107435400、timebase10MHz，即10.74354s，达到原10s窗口。该次 root 在文件收集之后的 Driver 停止实际 snapshot 为199765330000ns，是 primary 领取时刻的后继上界，严格小于冻结总期限298768822000ns，尚余99003492000ns；证据 JSON 核同一日志第380/386/390/430行的顺序及最后 Requested Shutdown。未改 Guest 状态、结果、期限、Runtime 输入预算或产品通知顺序，不泛称控制了两种普通通知交付顺序。

Filesystem Start 故障现场按同PID26实际非法指令与查询 snapshot（state=3/cause=5/detail0=8589934593/detail1=0）绑定交付拒绝，记录 `DONE_consumed=false`；不是伪称监督器已领取 DONE。wait/stopping 按真实 DONE 核对。各路线仍要求实际注入、唯一系统终局、Job/平台封口和 QMP真实 RESET，取证在第一轮 RESET 后结束，不跨 boot 拼证。Reset 拒绝只证明同一组映射仍由稳定 owner 保留及该路线四 hart 停驻，不证明硬件已静止。

### 开发失败、工具尝试与修复复核

nofd 原现场 `artifacts/failed-acceptance-20261028-145255-38743.log` 在基础服务出生前报 `DMA cancellation retained write permission`。原自检仅等 WaitSet READABLE/CLOSED 提示；亲核 WaitSet、MemoryObject seal/publish 与最后 Close 退休后，改为等同一对象真实 EXECUTABLE 再 query 写许可0，并保留 plain Seal/DONE 后续。HighNewt 确认属于本轮必要消费者闭包；未改内核/DMA合同，未扩大期限。修正后的 nofd 成功在上表。该证据证明 LastClose 触发取消后的写许可释放，不单凭该电平宣称全部 mapping/Pool 已退款。

System 工具初次使用错误 syscall 号而未命中，修为实际 Debug=1 后取证通过；中间用户主动收割重启的返回码不记成产品失败。Filesystem Start 工具初版只认 DONE、错过更早真实 Running 拒绝并沿到下轮 boot；已按上述实际 snapshot 分支修复。stopping 初次过晚 attach 错过窗口；其后全 syscall 观察曾干扰旧 Watch 期限，未作为预期 fault 通过；改用原 debug_print 停止意图预置断点后，才取得上表完整成功现场。没有为这些工具失败修改业务期限或弱化 fault 判据。

双顺序工具尝试现场保留在 `artifacts/fs-stop-order/`：`20261028T074703.261826Z-59230` 未保持 raw vCont 断点；`20261028T075402.328185Z-67082` 发现同问题后主动结束；后续 `20261028T075625.660123Z-72010`、`20261028T080648.296768Z-81540` 已见实际 Dead/Hint，但旧 bootstrap 提示仍先被 Runtime 的原输入预算领取，未证明 DONE-first。最终改为上述原 expiry recheck 的调度取证，不改 Runtime/产品预算；其间 `20261028T082356.995355Z-88782` 的 timebase 十六进制解析 preflight 失败未启动 Guest。工具 raw vCont 必须 always-inserted 的规则已入工具手册。所有尝试的外部进程均已清理；最终成功后未继续追自然反转窗口。

交付前结构检查及 HighNewt 协作检查均已复核以下原 findings：首次根/secondary Register 前共用必要来源就绪谓词；五槽角色/权限和出生 grant 对齐；唯一进程 owner、真实 Running 与 Start 前 gate/期限；正式 root 根持有及停止中迟到根立即释放；设备 Closed 与 fs Stopped 各自单一真值；总停止期限覆盖 detach/Collector/根收尾且 Completed 明确；正式 Pool 非零仅观察；System ELF audit 只核实际打包清单。

PreparedProcess finding 已修复并经 HighNewt 静态复核：出生准备完成后的发布失败、`start_manager` 移交前 gate/槽拒绝，以及 Stop-before-Start 的 `cancel_unstarted` 均显式复用 `cancel_prepared`→原 Collector；取消提交失败在 owner 尚存时直接共同终局，不返回触发 PreparedProcess 同步 Drop/INFINITE 等待。未改公共库析构合同，未建第二套清理框架。正常 Disposition/Cleanup 与退款重试保留；移除的是旧无限 failure/reset 重试路径。

结构收口按 REVIEW 核契约、消费者、付款、取消/迟到结果、来源实退、旧路径及 impls。长期控制仍是 Record 的单一所有权，同一 Assembly 推进，无新共享锁、锁序或公共内核 ABI；没有新增用户 fault→kernel panic。host 用例捕获私有启动格式/容量关系，GDB 工具只作为既有真实消费者的外部取证，不造运行体。语法、文档链接及 `git diff --check` 通过。本轮承诺范围无未闭合责任；未穷举每种错误身份、配额和 Stop-before-Ready 交错，这些按对应校验/owner路径静态复核，不冒称均有动态注入。真实硬件、持久化、恢复、外部关机与普通服务管理仍按原条件表；代码提交后才登记固定代码 Review。

最终离线收口：四个修改/新增 Python 工具及九段内嵌 GDB Python 解析通过；315个本地文档链接、74个标题锚点及本记录全部证据路径存在性通过，Justfile解析通过。最后 `just check`、十一分面 `just clippy` 返回码均为0；无遗留 QEMU/GDB 进程。COMPASS、总纲与关机案已移除本轮失效的施工下一步，保留条件后续；没有追加已通过的 QEMU 组合。上述检查日志在 `artifacts/service-foundation/offline-checks.log`、`final-checks.status`；当前全部改动仍未提交，未 push。

### 部署容量核算与证据口径

`startup::Profile` 唯一选择入口与报文数量；`deployment::Capacity` 的基础 task 项为150、基础 source 项为217，另加每入口实际根数（带 registry 为4，secondary为2）。System 为154 task/221 source，AcceptancePrimary 为158/225，Secondary 为152/219；这是宿主运行体容量，不是 init 必需集合的公共上限。每条根带 entries 个能力，System/Primary四根+Ready+Stopped共6条，Secondary管理根+Ready+Stopped共3条，均编译核 ABI 上限。

文件 Pool 页数直接累加 validated ELF runs（含 BSS）、ABI主栈、一页当前出生块与4096页显式堆/数据工作集。System debug 两个 runs 为260页，主栈2048页，出生1页，合计6405页；Ready实际分配2365页，余量4040页。4096页是选择的16MiB运行工作集，并非 allocator 或全部领域额度同时成功的承诺：fs、registry各2MiB用户态Bytes账户限制领域记录/数据，另为 Runtime/Assembly 预付输入、请求/回复临时缓冲、16个流会话（各3页Tunnel）、容器增长/退休并存及allocator粒度留空间。领域费用、映射页和metadata属于不同计量维度，不将它们简单相加声称物理硬保证；超额仍可返回原资源错误。

metadata选择1MiB（131072个8字节槽）的主要依据：独立进程VM固定ledger为4096个96B区域项与16个88B事务项，启动日志给出最低394624B；另有ProcessShell/Thread、cap表、映像/栈/出生/堆/流的MemoryObject、backing、region及Mailbox、Lifetime、Notification和退休责任。debug ELF只读尺寸样本（`artifacts/service-foundation/layout-sizes.log`）为WaitSet Member104B、Registration80B、WaitSet376B、Shell384B、Thread1040B；登记另收费AVL链接/Charge和对象头，不能只计这些payload。最宽225来源的payload约41KiB，来源整体规划留128KiB；16请求最大收/发payload基数约128KiB，角色/Watch/流、cap表扩张与退休并存另留余量。System Ready实际metadata65443槽=523544B，距1MiB余525032B，可容纳这些主要动态项；验收核正常准入、原Quota/恢复和最后owner退款。这是选定配置的容量与检查量级，不是任意全部上界同时饱和都可成功的证明，也不是所有allocator碎片的上界。

MemoryPoolSnapshot没有逐Pool高水位。Ready、停止前与Collector后均为时点样本，不记成峰值；不加内核ABI或周期探针。无外域owner的secondary验收在最后Collector后核双维0；正式聚合只报告余额，不以0账裁决服务完成或设备安全。

## 交付后结构清理

用户在上述验收交付后授权整理代码并更新计划、文档。基线仍为 `caa1f5e` 加本轮未提交实现，已有实现和证据全部保留。代码/工具对照快照为 `artifacts/service-foundation/cleanup-baseline-20261028-180612.tar.gz`（包含新增文件、原 diff 与删除清单），文档对照位于 `artifacts/service-foundation-cleanup/docs-before/`。FairRiver 主写代码及必要工具同步，HighNewt 审阅接口和责任链并主写本文与 impls。

清理范围来自实际调用与成形结构：文件 Host 与 Bootstrap 重复保存生命周期句柄；`retire_filesystem` 的布尔参数隐藏正式 Collector 与验收批次两种控制权去向；长期监督的启动返回值借出 Control，却复用表示出生移交的 `Spawned`；验收退款检查混在正式宿主模块；旧 `serve_forever` 仅转系统故障，以及固定两管理者、旧验收次序等失效描述。

同轮删除未使用的 `_job` 参数和 Secondary 单入口的等价判断，将 reset authority 负测与最终系统动作分开，修正部署局部名称与过时阶段日志。保留有独立合同的 Reboot/重复停止分支、实际 Await 查询、Copy 自有期限以及取消/来源退休状态；不以当前调用者少为理由删除这些契约，也不为统一外观引入通用服务框架。

保持现有外部语义及责任顺序：生命周期关联归原 Bootstrap/实例记录，Host 只保留实例关联和业务根；停止意图先于 signal，Stopped/DONE 汇合和实际来源注销先于 Control 转交。正式收集和验收批次交接采用明确接点，验收在取出 Control 前预留接管容量；取消仍复用 `cancel_prepared`，不恢复同步析构等待。启动只返回借用身份或 PID，进程 owner 留在 Record；不增加公共框架、平行账本或新状态机。

代码清理与验证已完成，相对快照修改10个源码文件，独立差异为 `artifacts/service-foundation-cleanup/code.diff`。`start_instance` 仅返回 PID，`instance_snapshot` 借原 Control 查询；Host 不再保存 bootstrap/release/Pool 句柄。`begin_filesystem_stop` 和 `close_filesystem_report` 统一使用原 Bootstrap，后者显式要求来源已实退；`detach_filesystem` 返回唯一 Supervised，正式调用立即交 Collector，验收 `track_stopped_host` 在预留容量后接管并继续原 Batch。验收退款组织已迁入 acceptance，reset 负测与正式动作分离，旧失败包装和未使用参数已删除。

结构复核已确认 Control 转交之间没有新增可失败操作、报告接点仍在来源实退之后关闭，Pool 的验收零账仍在 Batch 收集之后判断。原 InstanceTask 的消息领取、到期复查、异常 DONE 与终局封口判据未改；不新增状态机或改变公开契约。impls 已按职责归属回填，计划中的两管理者固定容量与 ABI 后置描述已校正。

清理后验证：

| 验证 | 结果与日志 |
|---|---|
| `just check` / 十一分面 `just clippy` | 返回0；`artifacts/service-foundation-cleanup/check.log`、`clippy.log`。check 保留原内核两项未使用依赖警告，本次未改变内核依赖 |
| `just acceptance-virt` | release 完整 core 通过，覆盖 secondary 批次接管/退款、primary 正式 Collector、设备停止及原 reset 负测；`artifacts/run/20261028-181307-2974-acceptance-virt-release.log` |
| `just system-observe` | 三必需宿主及 root 的真实 Infinite 来源有限观察通过；`artifacts/run/20261028-181130-1970-system-observe.log`，现场 `artifacts/system-observe/20261028T101134.594621Z-2353/evidence.json` |

误用 `just virt-release` 的首轮正常退出仅保留为演示开发证据（`artifacts/service-foundation-cleanup/core-release.log`），不计严格 core 验收；其后使用上表正确入口补证。源码接口残留搜索、文档本地链接/锚点和 `git diff --check` 已核对；文档最终增量为3篇、90个本地链接及19个锚点，全部通过（`artifacts/service-foundation-cleanup/docs-final.log`）。工具未修改，其解析与未变化专项沿用原交付证据。

未改变消息/到期判据、硬件/平台或故障收束，因此未重跑双顺序及全部平台/故障组合；未变化的专项和 host 协议用例继续由上表交付证据拥有，不记为清理后重新运行。当前无本轮新增未闭合责任，未来能力和独立 Review 的条件保持原记录，未提交或 push。
