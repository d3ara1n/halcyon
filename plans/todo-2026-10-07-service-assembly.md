# 正式服务装配与监督设计

状态：**设备首批装配已具备施工合同，通用正式服务装配仍未全部闭合，未开始代码施工**。本案承接[系统重构总纲](todo-2026-09-29-system-redesign.md#剩余切面与接续顺序)的服务装配主线，拥有正式系统的启动依赖、就绪、监督恢复与进程授权装配；发布发现协议由[OAL/服务分案](todo-2026-10-07-fal-service-redesign.md)拥有，全系统停止协议由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有，设备装配事实与供应依赖由[设备分案](todo-2026-09-device-system-design.md)拥有。

## 目标与非目标

把正式长期服务、资源管理者和驱动组合成有明确启动、授权、存活、恢复和停止责任的系统。能够回答谁启动谁、依据什么判断就绪、何时可向消费者交付能力、失败由谁接管、何时重启或升级为系统停止。

本案不预设新增 supervisor 进程、manifest 文件格式、通用服务框架或独立电源管理服务。静态装配与动态配置是待论证的方案选择；不因需要正式系统装配就重写已完成的 Process/Job、Thread owner 或用户态执行基座。

## 当前事实与前置

核对基线：2026-10-07，`refactor/system-redesign` / `f174d8e`。当前启动与监督事实见 [startup 实现](../notes/impls/startup.md)、[task 实现](../notes/impls/task.md)、[framework 实现](../notes/impls/framework.md)。

- `srv_init` 的固定 workload、RequiredLaunchSet、RootSupervisor 与已有 pm/fs 消费者承担验收装配和真实监督责任；不能把它们当成没有可用基座。
- 对象链、自动回收、Job 封口、进程出生/执行绑定、来源登记及运行时分层已完成相应交付。本案不恢复 ProcessDrain 或用户必须持续驱动的内核回收。
- 当前已有 Registry/服务记录；发布成功、服务就绪、endpoint 可用和进程存活之间的正式系统协议尚未完成本轮详细设计。
- 设备的实例、受权使用/绑定、供应依赖和硬件安全责任仍在设计。设备驱动装配不能仅凭进程 DONE 推断 DMA 已静止。
- 关机编排与电源管理已有独立记录；本案只确定服务的停止入口及其与监督的交接，不复制全系统关机状态机。

发布发现的唯一目标由[OAL/fs/服务目录改造计划](todo-2026-10-07-fal-service-redesign.md#服务目录)拥有：业务就绪后单步 Register、显式 GetEntry、控制入口 Withdraw/QueryRegistration 和名字 Watch；公开状态为 Active/Withdrawn，准备/资源退休为内部责任。设计及实现状态见该记录，不作为既有启动事实。目录不维护 Starting/Ready/Draining 健康阶段，不把 endpoint 可用当健康证明。真实启动、就绪判断、业务 drain 和恢复归本案。

装配接点：首轮仍由 srv_fs 的同一 Runtime 容纳文件与 registry 两个独立 provider，分别交付实际入口；消费者在自己的 Namespace 绑定服务目录，通过 GetEntry 取得业务入口，服务替换不会自动重绑。启动者应在业务就绪后注册，注册成功后才宣告发布完成；若 Replace 已提交但回复丢失，旧注册不恢复，新控制最终消散后名字可能为空，按发布分案的未知结果合同处理。需要可恢复发布事务或不断档切换时，由本案证明需求再闭合，不把两阶段注册偷偷恢复为默认前置。

## 驱动启用与设备管理的装配边界

用户已明确要求运行期设备出现时能发现并启动尚未运行的驱动；采用 init 启动并授权专职 srv_driver，后者统一匹配、按需启动/复用和激活监督。设备关系与 Claim 的详细目标见[设备计划](todo-2026-09-device-system-design.md#驱动发现按需激活与设备绑定)。srv_device 只拥有设备登记与受权占用，不认识驱动可执行文件。

init 交付受托 Job/进程控制范围、付款来源、设备 owner 派生的管理授权以及启动必需映像来源，监督 srv_driver；srv_driver 使用既有 libprocess 启动并监督自己的驱动运行实例。启动时设备与后续热插设备走同一匹配/绑定流程：进程不存在时启动，已有可用进程时复用，随后交付同一种绑定激活请求。不保留 init 和已运行驱动各自竞争自动 Claim 的双轨。具体进程配置与受托合同尚未实现。

未来驱动仓库以文件系统中的映像和支持声明作为匹配输入，格式、安装、更新机制按用户要求后续设计。首批由 init 交付 boot 配对声明/映像来源，包含 PCI、xHCI、HID；HID 仅在接口出现时启动。根控制器及仓库所依赖的存储驱动必须能从启动来源取得，仍由同一选择/激活流程处理。静态指定实现、明确优先级、歧义拒绝与有界候选尝试由设备计划的驱动合同拥有，不在 init 再匹配一次。

`/System/Drivers` 由 srv_driver 的运行实例与绑定激活事实提供 OAL 视图，设备节点直接关联绑定及功能，不复制设备占用表。[设备控制根合同](todo-2026-09-device-system-design.md#控制根付款与故障)规定 init 在子故障域外保留平台根和来源管理；srv_driver 失败先封旧来源，srv_device 失败进入系统故障终局、不重建占用表。[根终局](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)规定 reset 错误及 init 死亡处理；DMA 保活独立于句柄和管理进程寿命。

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

平台根、PCI 配置写权限和 TrustedPhysical DMA 属于系统可信基座：init、设备管理/平台适配器、认证并转交静止报告的 srv_driver、PCI provider、xHCI 主控均必须可信。不能只把 xHCI 标为可信，却把可重配 BAR/bus-master、派生 DMA 或提交可信静止事实的上游当作不可信。HID 与功能客户端的边界来自受限 USB 会话；路径、支持声明和程序自报标志不能授予上述权限。

## 设计门

1. 明确首批正式长期服务及可观察能力、启动依赖、必需与可选能力；从真实需求选择静态或可配置装配。
2. 分开画进程/Job 管理、付款 Pool、授权交付与服务供应关系。确定上级退出、失联和权限消散后的接管责任。
3. 闭合启动失败、就绪超时、服务死亡、重新发布、重连与失败升级；重启不得复活失效 endpoint 或重复授予权力。
4. 与 FAL/服务分案核定发布发现接点，与设备分案核定驱动供应和隔离事实，与关机编排核定停止接点。各项契约只有一个拥有计划。
5. 形成首批装配的接口、责任转换、迁移删除清单与验证门后，才进入代码施工。

上述设计门在设备首批范围内已由装配图、私有就绪/Unknown/监督合同及设备主计划的迁移/验证门闭合，可以随设备施工；通用正式服务继续按这些门设计，不把未来热升级、持久化和配置需求列为本次设备的前置。

## 完成门与验证边界

设计交付须能依据计划实现选定装配，明确正常、失败、取消、退出、恢复和停止责任。系统能力交付须有真实长期服务接入、替换范围内的旧装配路径删除，并在相应平台验证启动、就绪、故障恢复和停止组合；不能仅用一次验收 workload 跑完宣称正式编排已完成。

本案通用正式装配方案仍未闭合；设备首批的进程分工、静态来源、授权和失败政策已有上文目标，不代表一般服务热升级、持久化停止或配置脚本已设计完成。现有同进程 fs/registry 装配由 OAL/fs/服务目录计划接通，本案不要求提前拆出 srv_service。未修改代码、ideas 或执行运行验收。

## 当前接力

设备首批按本篇装配合同与设备计划的统一实施依赖接手，代码仍需另行授权。更广正式服务继续闭合本篇设计门；仓库格式与安装更新保持延期。不恢复双轨匹配、透明继承旧来源或由 srv_device 选择/启动驱动的方案，也不把设备闭包的完成范围扩写为通用服务装配已完成。
