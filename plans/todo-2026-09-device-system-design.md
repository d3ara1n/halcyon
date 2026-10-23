# 设备体系设计：领域模型、驱动管理与资源收束

> 状态：**设备领域与首批平台的设计基线已形成，后续继续收敛和完善；尚未冻结施工方案、未实现或运行验收**。本篇是唯一活跃设计与后续实施记录；当前授权方案、文档整理及提交，不授权代码施工。当前方案取代[早期讨论快照](archived/ref-2026-10-23-device-design-rounds.md)中的候选和待选方向。服务装配、平台保留内存、系统终局仍由各自计划拥有，本文记录它们在设备闭包内的当前接点。

## 目标与范围

设备应能在用户层被发现、认识、授权使用和管理；驱动聚焦硬件与领域协议，应用取得功能能力。设备目录是活的领域对象视图，包含存在、绑定、功能与状态，不能退化为静态 DT 数据或一个写入 PID 的标签。

通用目标覆盖平台设备、总线子设备、动态插拔、共享供应和功能组合。首批完整实现路线选定 **QEMU virt + PCI xHCI + 运行期 USB boot keyboard 热插**：插入前 HID 驱动未运行，系统发现设备、选择并启动驱动，应用取得键盘功能；拔出、失败和停止走同一责任链。打印机是结构检验场景，真实 USB 打印协议不属于首批交付。固定证据见[平台取证](ref-2026-10-23-device-platform-contracts.md)。

首批使用启动映像集提供驱动声明与可执行映像。磁盘仓库格式、安装、更新和来源认证按用户要求延期；不因此延期运行期按需激活。首批不实现通用 HID、外置 USB hub、PCI bridge/PCI 热插、MSI、IOMMU、非一致 DMA、动态保留区/reusable、透明管理者恢复。它们不能被静默当成已支持能力；具体触发与完成门见文末。

遵守 less is more：只有设备实例、一次绑定及所属领域的实际资源/会话；不建通用 Lease、ClaimScope、第二个 HAL/DAL 发现框架、驱动 Runtime、通用分布式事务或持久恢复账本。已有 OAL、RPC、执行、等待、进程与预算机制继续按其合同使用。

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

上述新目录均为目标，当前没有实现。库不依赖具体 libruntime，不自行开线程或消息泵。libdriver 提供普通类型和可组合任务，驱动将绑定、IRQ、请求来源放入既有执行/等待机制。先显式按名解码，不预建 derive/schema 系统。libhub 只有实际共同领域知识形成时才抽取。

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
| ClaimSource | 在已授权范围 Claim/StopClaim、恢复本主体原轮控制；由 srv_driver 持有 |
| ClaimSourceAdmin | 封本来源并停止其已提交绑定；交给 init，申请角色不能解封 |
| BindingControl | 查询/停止这一轮，发行 BindingUse、安装不透明激活观察入口和功能关联；srv_driver 持有 |
| BindingUse | 逐名取本轮资源、观察本轮停止；驱动持有，不默认能 Claim、复位其他设备或管理来源 |
| Provision | 本登记单元的准备/查询/停止/结果副本；仅 srv_device 持有调用角色 |

`Claim(DeviceId,expected_epoch)` 先本地准入并提交唯一占用，再由稳定任务推进资源准备。提交前预留 Binding、控制出口、准备任务和清理跟踪；没有远程调用夹在本地提交里。提交后回复失败不回滚，责任不再依赖申请者继续调用。

绑定阶段：`Preparing → ResourcesReady → Stopping → Released`；Preparing 可直接停止。ResourcesReady 仅表示整张资源表就绪，不表示驱动功能已经 Ready。Stop 封闭新使用，停止期间不回到前态；全部资源安全与迟到责任闭合后才 Released/推进 epoch。

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

Provision 状态为 `{closed_epoch, active: None | (epoch, Preparing|Prepared|Stopping)}`。高水位表示该 provider 的硬件轮次已安全关闭，区别于 srv_device 的占用轮次，不是重复占用表。每个已发行 Provision 入口保留小壳至入口寿命结束；NotFound 不作为安全完成，provider 消失返回 Unavailable。

| 操作/结果 | 合同 |
|---|---|
| `Prepare(e)` | Binding 已提交后调用；只建立一份 active 准备。相同 e 返回原进度/结果，旧已关闭轮返回 Closed，前轮未关闭不得准备下一轮 |
| `Rejected(e,reason)` | 没有遗留效果，provider 已将 closed_epoch 推至 e；可以安全收束该轮 |
| `Failed(e,reason)` | 可能有供应/硬件效果，转 Stopping；不能据失败直接释放 |
| `Prepared(e,count)` | 整表已冻结；逐索引 `GetPreparedResource(e,index)` 返回同一表项的 capability 别名及 name/type/rights/metadata；重复不重新分配业务资源 |
| `Stop(e)` | 幂等；未有 active 的合法下一轮也能先封闭，迟到 Prepare 不再开始；已有 active 则取消准备、停止资源并保留责任。封准入不要求立刻撤掉协作停止仍需的 MMIO/IRQ |
| `ConfirmStopped(e)` | srv_device 转交本轮受权的协作停止事实；重复无新效果，旧轮不作用于新轮。只改变停止所需证据，不自行产生 Closed，provider 仍须完成资源控制与退休 |
| `Query(e)` | 返回 Preparing/Prepared/Stopping/Closed；e≤closed_epoch 可 O(1) 返回 Closed，不保留逐轮历史结果 |
| `Closed(e)` | 对该轮准备效果、派生 use、在途硬件访问与实际释放条件的保证；不是仅关闭 endpoint 或超时 |

调用权限限定到本实例及由 srv_device 发行的合法轮次，不接纳任意跳跃 e。准备可能改变硬件，必须从一开始归原 Binding；不能把有副作用工作提前到 Claim 提交前。

srv_device 在 Prepared 后逐项取得表，校验唯一单分量名字、capability 角色、固定导出 rights 和登记上界。领域私有 metadata 是不透明字节。只有全部项到齐且绑定仍 Preparing 时，才一次转 ResourcesReady；cap/metadata 必须在同一回复配对。校验或 Receive 安装失败走原 Binding.Stop，不公布残表。

`GetResource(name)` 仅从这张本地冻结表取别名，固定权限，不接受临时扩权或新资源申请，不产生 Boundary。名字不是路径脚本，额外子分量为 NotDirectory。绑定 Resources 视图由同表生成，观察权不等于 Get 权。该选择取代早期允许 GetResource 远程接续的候选；OAL 拓扑与功能 Boundary 不受影响。

Get 可重试的含义仅为“不重复分配业务资源”；仍产生消息、Delivery、句柄和元数据费用。首批只接纳可别名角色，没有 taken 位、归还再取或仿射转移。供应/总线 session 的独占与终态由自身协议保证，管理者保留副本不改变其合同。

Stopping 时立即拒绝新 Get。晚到 Prepared/表项副本仍归旧 Binding，关闭副本并继续 Stop/Query，不能交给新驱动。Closed 后迟到别名必须已经不能重新产生使用，仍按 RPC 收尾释放运输费用。provider 死亡只结束等待，不等于效果已结束；无硬件/资源证据便保持隔离并升级故障。

## 驱动发现、按需激活与设备绑定

init 启动管理基座并委托唯一自动选择范围；srv_driver 订阅设备、读取支持声明，统一选择。srv_device 不认识驱动程序，运行中的驱动也不另行全局匹配抢占。管理员可以共存并停止绑定，srv_driver 的激活进度不能替代设备侧占用事实。

### 声明输入与选择

首批来源是 init 交付的只读 boot 映像集，接口提供 `implementation_key`、协议版本、匹配规则、明确配置优先级、进程复用容量、资源需求和对应映像能力。声明与映像在同一来源操作中配对，采用后不可被路径替换；不先读声明、再按可变文件路径取程序。格式由后续仓库设计拥有，首批可用编译期记录关联启动包内映像，不提前设计磁盘 schema。

匹配规则为所属领域的静态 identity 精确条件组合，例如 platform compatible、PCI class、USB interface class/subclass/protocol；不建立任意脚本求值器。srv_driver 依赖对应协议库解释这些值。静态指定实现与配置优先级属于同一选择流程；同优先级有多个有效候选时报告 Ambiguous，等待明确政策，不能以启动/Claim 竞速选择。

一个实例与当前静态声明集合的一次选择，只逐个尝试有序候选一次；失败需旧绑定安全 Released 才尝试下一候选。资源不足、依赖未就绪等待实际变化或显式管理重试，不紧循环。声明变化默认只影响未绑定实例，替换活动实现需显式 Stop/重绑定政策。首批 boot 声明静态，不新增仓库 Watch。

### 激活与功能关联

1. srv_driver 选择候选并 Claim；未知结果恢复原轮或 StopClaim。资源就绪前不激活程序使用硬件。
2. 取得 ResourcesReady 后，按声明选择可复用且有容量的进程，或用受托 Job/libprocess 启动。首批 PCI host、xHCI 各为隔离进程；HID worker 可承载多个接口，容量来自实际预算配置。
3. 以 `Activate(BindingRef,BindingUse,identity)` 交给驱动。worker 按 BindingRef 去重；相同请求返回原激活状态，不再初始化一次。身份/入口不一致返回 Conflict。一个未完成激活有一份任务及预留状态，回复丢失由管理者 QueryActivation 恢复。
4. 驱动逐名解码资源、检查类型和元数据、探测/初始化。失败报告 Failed，由管理者停止原绑定；探测碰过硬件后不能当作无效果拒绝。
5. 驱动 `Ready(function_root,description)` 交付一个 OAL 功能根。srv_driver 凭 BindingControl 安装该根及激活观察关联；同一根的重复安装幂等，不同根 Conflict。只允许 ResourcesReady 且尚未 Stop 的本轮安装；迟到 Ready 不发布，直接收尾。
6. 按装配需要发布 Registry 或用途视图。发布不是 Binding/设备/Registry 的全局事务；必选出口失败则该能力激活失败并停止，可选出口失败只降级诊断。首批必要出口为设备 functions，Registry 是可选别名。

驱动激活阶段为 Starting/Ready/Stopping/Stopped 或 Failed 诊断；它不复制设备绑定状态。Ready 只表示本次功能可受理，Stopped 报告是协作证据，硬件是否安全仍由 Provision/资源控制确认。管理者通过 BindingControl 的 `ReportStopped` 转交报告，srv_device 向原 Provision 调用 ConfirmStopped；两者按 BindingRef 幂等，停止期间仍可调用。需要将受信物理主控报告作为静止依据时，相关受托角色必须在装配时具备该证据提交权限，不能从声明/tag 中自授；无此权限的报告仅是诊断。

`StopActivation(ref,deadline)` 幂等，禁止该激活新业务，推进既有请求收尾；其后迟到 Activate 不重新启动同一 ref。重复记录保留至该 Binding 已 Released 且控制关系退休，容量随真实记录付款。一个进程死亡时停止其全部绑定；进程复用节省启动成本，也明确扩大故障影响范围。

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

| 占用/工作 | 准入与付款 | 释放条件 |
|---|---|---|
| 实例/来源/观察壳 | 发行/登记前由受托管理账户付记录和字节费用 | 来源/实例不再有活责任且已发行入口退休 |
| Binding/准备/停止任务 | Claim 提交前预留；表项数、名字与 metadata 总字节上界来自登记 | 安全 Released 后按任务/控制壳实际寿命退费 |
| Provision 硬件准备和供应会话 | provider 自身的真实账户；一单元最多一个 active 轮 | 效果/会话实际安全退出；不足返回明确拒绝 |
| Get/准备结果消息与句柄 | 发送/复制占发送进程实际 Pool，Receive 安装占接收进程 Pool；领域费用另按授权账户 | 按真实 Delivery/handle 收尾；不宣称领域预留已支付内核未来安装费用 |
| DMA backing、映射与隔离 | 持原 MemoryCharge/MetadataCharge，控制侧稳定 owner 保活 | 可信设备完成/静止后显式退休并完成必要同步 |
| 故障推进 | init/管理者装配时预留执行与控制容量，不借业务请求临时续费 | 原责任完成或系统终局；超时不提前退款 |

内核 Charge 强持实际 Pool，付款进程死亡不摧毁余额 owner。用户态 Charge 不能充当跨进程持久账本。容量按实际表布局与配置计算：单绑定管理预留覆盖 `Binding固定项 + 资源项上界×项成本 + 名字/metadata总字节上界 + 固定数量准备/停止/观察任务`；设备数、观察者、driver 激活数、消息在途各有单独额度。拒绝超界，不在承诺后无界扩容。清理链接在对象准入时备好，停止不因业务额度用尽而丢责任。

| 故障 | 继续持有责任者与结果 |
|---|---|
| 叶驱动死亡 | srv_driver 停止该进程全部绑定；存活总线 provider 收束会话，安全后可重绑定 |
| DMA 控制器驱动死亡 | 不等待 stopped；控制侧封准入。首批无独立硬件隔离、不能证明停止时保留全部相关 pin，进入系统故障终局 |
| srv_driver 死亡 | init 封旧来源、停止其执行；srv_device/资源 owner 收束。旧绑定安全退出后才可按政策启新管理者，否则终局 |
| srv_device 死亡 | init 封系统新工作，停止受影响执行，保留实际根；不重建占用表，进入系统故障终局 |
| init 死亡或最终 reset 失败 | 按[系统终局合同](todo-2026-09-system-shutdown-orchestration.md#设备管理故障的接点)进入不可恢复停止；活跃 DMA 的 pin 不随句柄回收释放，等待外部复位 |

停止/故障路径不能形成同步互等，异步通知允许向上；正确性不依赖已失效子服务的回应。正常停止可以先协作、期限后升级，进程终态已知时直接走无配合路径。期限仅防停滞，不证明硬件已停止。

## 特权资源合同

设备实例/Binding 仍是用户态对象；内核只提供硬件访问与安全 owner。新增共享 ABI 角色由 shared、kernel、libkernel 同步演进。下列名称表示目标对象角色，不是现有 ObjectKind。

### MMIO

`MmioControl` 拥有经启动平台校验的物理 IO 区间、可派生范围与固定访问属性；`CreateUse(range,rights)` 产生一次绑定可撤销的 `MmioUse`，`Map(use,vm,offset,length)` 建立受其跟踪的映射。范围 checked、按页归一，不能将同一页的独占寄存器伪装成两个隔离区间，也不能把普通 RAM 重新标成 IO 授权。共享页交给真实 provider 或单一 owner。

根 control 与子 control/use 构成实际派生关系；根 Seal/Revoke 覆盖全部后代，单 use Revoke 只撤该 use 的全部地址空间映射。提交封闭新派生/Map，原映射撤销复用 VM 维护、远端 shootdown 与退休 owner；完成在所有相关翻译失效后报告。Close 只释放 handle，不代替 Revoke；先撤 MMIO 可能妨碍驱动静止硬件，顺序由 Provision 合同决定。

首批映射属性依 QEMU virt 的平台 IO/PMA，不假设存在 Svpbmt；不映射为可执行，不制造普通 Rust 引用表示设备寄存器。libkernel 提供受范围限制的 volatile 访问；寄存器语义和必要 I/O/内存 fence 由驱动按固定规范负责。CPU fence 不是 DMA 完成或非一致缓存维护。

### IRQ

首批 PLIC 在内核，外部 SEIP 接入 trap。`IrqControl` 限定真实 source 与允许的目标 context，派生独立 `IrqUse`；同一真实 source 首批一个消费 owner，共享线配置明确拒绝，不把它误当两条独立 IRQ。目标 hart 在准入时固定，首批不提供迁移 API。

接口为 `IrqUse.Take() → token | Empty`、`Ack(token)`、查询/WaitSet READABLE；control 的 Seal/Revoke 可强制结束 use。token 包含 use 代次与本次中断序号，不回绕。别名共享同一消费状态，不产生第二个硬件订阅。

trap 每次最多 claim 一个 source，不分配、不做用户 RPC。内核记录 in-service，发布可读提示；gateway 在 complete 前阻止同源再次转交。Take 领取本次提示，驱动检查/服务实际设备后 Ack，内核核对 token 再 complete。IRQ 是提示，不承诺逐边沿无损计数。PLIC 的 claim、Ack 和撤销共用控制器短锁，在锁内完成寄存器与 source 状态转换，锁外通知既有等待机制；不得保有退出线程强引用或绕过 WaitSet 订阅生命周期。

现有 idle 在 SIE=0 下 WFI，醒来不必进入 trap，所以不能只增加 trap 的 SEIP 分支。接入时同时在 run 安全点、idle 睡前复检和醒后路径识别本 hart 外部 pending，调用同一个最多领取一源的推进原语；在内核持续维护时也保留这一服务机会。没有 runnable 工作可以睡眠不等于没有设备请求，PLIC/SEIE 保持可唤醒，已记录可读事件和等待提示按既有来源/维护队列阻止丢唤醒。每次领取后返回既有工作仲裁，不在安全点无界排空 IRQ。

撤销先封用户领取/Ack，若有 in-service 则在 enable 仍成立时 complete，再禁用 source；这几个步骤在同一 PLIC 短锁内完成。首批 source 仅在所选一个 context enable，所有 hart 的 claim 路径取得同一控制器锁，所以 complete→disable 窗口不会被另一 claim 插入，不靠循环排空电平源。真实控制权限全部消散时也由内核执行此有界封源步骤，不等待已死使用者 Ack。之后该 use 不再交付事件，旧 token 返回 Closed。PLIC pending 可能使新绑定看到一次陈旧提示，新驱动重新检查硬件；不能由旧 Ack 完成新轮。批量派生/等待者处理走既有维护预算。

### DMA

首批是显式的 `TrustedPhysical + Coherent` 模式，没有 IOMMU 隔离。取得物理 DMA 地址的控制器驱动属于系统可信基座；描述范围与 capability 不能阻止恶意驱动向硬件写任意物理地址。普通 USB/HID 子驱动只拿受限 transport，不取得 DMA 根。非一致或要求不可信 DMA 隔离的配置返回 Unsupported。

`DmaDomainControl` 持设备/父域、模式、映射登记与封口状态；派生 `DmaUse`。`Pin(memory,range,direction)` 校验内存权限、范围和冻结条件后创建 `DmaMapping`，返回分段的设备地址与长度。映射持独立 BackingUse；设备可写时还持 MemoryObject 写许可及归还目标，阻止 Seal 完成后仍有设备写入。它们与原 charge 的寿命独立于驱动 VM。范围 use/写许可由[内存领域](todo-2026-10-01-memory-redesign.md#设备-dma-接入合同)提供和强制，设备层不复制一套页占用或 W^X 状态。

**活跃 mapping 由内核稳定 owner 持有，最后 Close、VM 退出、付款进程死亡和控制 handle 全部消失均不 unpin。** 控制消散封新准入，未完成映射进入有账的静态孤立保留；它不是可运行维护债务，不进入 ready 队列或等待一个永远不会来的完成。首批没有重新取得孤立控制的协议，只由外部复位结束这种保留。正常可退休映射才使用已有维护基座有界释放。此规则也覆盖 init 回收控制句柄的窗口，不能依赖全 hart 已停驻才保证 backing 不被复用。

正常 `Retire(mapping)` 可由创建它的受信 DmaUse 角色提交，或由其上级 control 提交；前者不能退休另一 use 的映射。因而 xHCI 可以在运行期间回收已完成的 transfer 缓冲，不必逐次调用 PCI provider。`Seal(domain)` 只阻止新 Pin/使用，不自动证明旧 DMA 消失。libkernel 的安全 wrapper 不把 Drop 包装成完成，提交可信物理 DMA 退休是显式、须论证的边界。根 control 可封全部派生，binding 子 control 只作用于自己的映射。无 IOMMU 时根不能凭空撤销物理写能力；缺少 quiescence 证据保持 pin，系统故障复位，不谎报 Revoke 已完成。

TrustedPhysical 下，已由装配准入为可信主控的驱动可提供本轮硬件静止报告；报告经实际激活入口认证、绑定到 BindingRef，按上述 ReportStopped→ConfirmStopped 转交，不能由 payload 的可信标志自授资格。PCI Provision 依据该报告、已封口 DmaDomain 的所有 mapping 退休完成和自身 bus-master/IRQ/MMIO 收束确认 Closed，不自行解释 xHCI 寄存器。DmaDomain 提供 Idle/终态观察，只有已封口且无活动映射才成立，不能将瞬时映射数为零当成域停止。驱动报告后永久关闭本轮提交；这是可信基座合同，不能推广为不可信驱动说 stopped 就安全。驱动死亡或报告缺失时没有替代证据，走保留与故障终局。

首批用固定长度 MemoryObject 页构造 ring/数据，CPU View 仍可 Unmap/降权，DMA 的独立 use 保证实际页不释放，设备写许可保证对象不能提前变为 Executable；不禁止与 DMA 安全无关的 VM 操作。CPU 与设备并发访问按驱动协议划分所有权并使用适当屏障。页级 pin 不代表整片物理连续，返回 SG 段。xHCI 小 ring/DCBAA/context 使用满足规范对齐与边界的页内区域，数据跨 64 KiB 按 TRB 分段。不另建 raw 虚拟地址转物理地址的旁路。

### 并发与有界推进

用户态各 backing 采用单一状态 owner；操作只在短提交段检查/改状态，不跨 await 持锁。远端完成带 DeviceId/epoch，在原任务上核对后消费。批量撤出/来源停止有持久游标和已付任务，每次推进额度由执行预算限定，调用者退出不丢任务。

内核资源 gate 只仲裁准入/封口和成员链接，不在其锁内进入 VM、Pool、WaitSet 或驱动协议。映射准入先预备容量，再在资源/VM 各自 gate 验证并提交，竞态失败回滚未发布项；撤销先封 gate、摘取有界成员，再锁外推进 VM/IRQ/DMA owner。DMA backing 使用责任以实际内存对象原语保持，停止状态不代替引用。地址空间退休仍由 VM 自身锁序与 shootdown 合同拥有。

大范围 Pin、SG 地址枚举、批量派生撤销不得在一个不可抢占 syscall 中扫完整集合。Pin 的准备/提交/回滚复用已提交工作 owner 和等待完成模式，设备地址在准入成功后才可交付；地址段按有界游标读取。同步快路径只处理有结构上界的固定步骤，退休复用原映射的预付记录，不临时分配逐页 Work。

IRQ 顶半部最多一次控制器 claim 和一项状态发布，驱动批量处理 event ring 受每次任务预算约束；未消费完则自保持 runnable，不能等待新的 IRQ 才继续。持续设备风暴仍占其已准入执行预算；未 Ack 的 source 不反复入内核。没有 IRQ/时间推进保证时进入既有运行故障终局，不能宣称公平性靠无限重试取得。

## 首批端到端平台路线

固定参考为 QEMU v11.1.0 virt、PLIC、coherent PCI DMA；固定 `qemu-xhci,msi=off,msix=off`，运行期在 `xhci.0` 插拔 `usb-kbd`。just 路线显式关闭默认网络/多余 PCI 设备，列出唯一所需 PCI function，并核对 interrupt-map 使 xHCI INTx 独占；实际有共享时 PCI provider 拒绝，不因默认机器配置悄悄绕过首批限制。具体启动参数由实施时 just 路线拥有，DT 与机器配置必须一致；本次未运行模拟器。

### 启动与枚举

1. 内核仅解析启动/内存/中断所需平台事实；启动交付只读 DTB、经校验的 IO/IRQ/DMA 根和启动映像集给 init。DT 文本不是能力，用户态解析不能自铸任意物理授权。
2. init 先保有控制根、故障执行预算和管理 Job，再启动 srv_device/srv_driver，交付各自角色。管理基座和必需 PCI/xHCI 驱动映像可从 boot 来源取得，不依赖尚未启用的设备存储或 UART 日志。
3. 平台适配器登记 PCI host 和静态事实。总线语义的 DT 子树交给该总线解释，中央登记核心不解析 USB/SPI/I²C 私有地址。
4. srv_driver 绑定 PCI host，启动 drv_pci_generic。它取得限定 ECAM/IO 窗口、子资源派生和 interrupt-map 路由信息；枚举首批 bus 0，拒绝未支持桥拓扑。BAR 配置期间封相应解码，按设备 size/alignment 在已授窗口分配，准备完成前不启 bus mastering；范围/页重叠仍由资源层强制。
5. PCI provider 登记 xHCI function，并兑现它的 MMIO、INTx 和 DmaUse。srv_driver 匹配并启动 drv_usb_xhci，读取能力、建立 ring/context、按规范复位/初始化后启用控制器。不得凭函数指针或字符串跨越资源授权。
6. root hub 检测连接、复位端口、通过 xHCI slot/address 流程和 USB 标准请求读取/校验描述、选择配置。地址、配置和完整设备生命周期归 USB owner；在首批选定接口粒度后登记 boot keyboard 接口，不能同时允许外部整机驱动独立占用同一设备。
7. srv_driver 由 Watch 发现接口；boot 来源中已有支持声明但 HID worker 尚未运行，故现在启动，Claim→ResourcesReady→Activate→Ready。应用从设备 functions 或装配别名取得 HID 键盘入口，直接到 HID provider，不经管理者转发按键数据。

PCI 资源准备中的硬件步骤归 PCI Provision backing；xHCI 初始化后的 bus-master/关停由相关 control owner 协作。不以清 PCI bus-master 位单独证明所有既往写入已经结束；正常 xHCI 停止仍须 halt 和在途责任确认。控制器崩溃时首批不实现一个复制 xHCI 状态的救援驱动。

### USB transport 与键盘功能

USB Provision 创建**已限定设备/接口及端点的会话**，表就绪前完成，不在 GetResource 时首次开 session。接口驱动可提交允许的 interface/endpoint 控制请求和 interrupt IN，不能自行 SET_ADDRESS、SET_CONFIGURATION、复位整条总线或访问其他接口。描述和实际会话在同一次资源回复中配对。

USB 请求通过所属协议提交 Setup/端点、方向、长度、数据 owner 和期限；host 校验接收范围并承担从提交至硬件完成/取消的责任。首批小控制/键盘报告走有界消息，host 自己的稳定 DMA 缓冲承接硬件，叶驱动不 pin 自己的任意页。请求取消可能与完成竞态，回复 Unknown 不释放 host 缓冲；session.Stop 阻止新请求，按 xHCI endpoint/slot 命令和完成事件收束旧请求。拔出只是物理事实，DMA transfer/event ring 不因设备消失立即释放。

HID 驱动校验 boot keyboard 描述并 SET_PROTOCOL(0)，消费 8 字节报告。功能协议提供当前按键集合及单调报告序号，`Watch/Query/Ack` 报告变化；这是状态输入，允许合并提示，不承诺每次短暂按键边沿无损。需要无损事件流的消费者必须另行接入有容量/溢出合同的队列，首批不声称具备。ErrorRollOver 作为无效/不确定输入报告，不伪造全部释放。LED 输出和通用 report descriptor 非首批承诺，Unsupported 明示。

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

本节只安排后续授权后的实施，不创建空 crate 或临时 ABI。中间编辑可以不完整，交付须满足完整消费者与收束链。

1. **启动事实与特权资源**：共享 DT parser/只读 DTB 交付、平台根、MMIO/IRQ/DMA 对象和 shared/libkernel 同步；DMA 稳定 pin、根终态观察、PLIC 等待/撤销接入既有内存和维护 owner。平台保留内存按[所属计划](todo-2026-09-platform-reserved-memory-lifecycle.md)核定首批不使用的能力并明确拒绝。
2. **设备领域闭包**：登记/来源、Claim/StopClaim、Provision、冻结表、Watch 和功能边；类型/状态、提交、额度与清理一起实现，不先造只展示数据的设备树冒充管理。
3. **驱动管理与装配闭包**：boot 配对声明来源、唯一匹配、受托启动/复用/监督、激活、停止、发布；同步[装配](todo-2026-10-07-service-assembly.md)和[故障终局](todo-2026-09-system-shutdown-orchestration.md)，不保留 init/驱动自主抢占的旁路。
4. **实际总线与消费者**：PCI→xHCI/USB→HID→正式功能客户端；每层接通 Provision/停止，不以假设备代替真实热插。os/dtb 迁至 shared 后删除旧位置，更新真实消费者；清除被替换的裸地址/启动槽位接线，不为历史兼容留 adapter。
5. **组合验收与文档**：按构建手册执行 just check/clippy 与实际 QEMU 路线，更新 impls、范围登记及本篇工作记录。尚未替换的 drv_spi_sifive 占位不冒充已有能力，不为本路线顺带实现其他平台。

验证按风险选择：纯身份/轮次/解析在 host 边界；跨 provider Unknown、late reply、Quota 使用真实 RPC 设施；MMIO 撤销跨 hart、IRQ 旧 token、DMA pin/charge 在原对象测试处验证；热插使用 QMP 和实际 USB，观察未运行→启动→功能→拔出/Released；包含重插旧请求、叶/主控/管理者故障、reset 错误及根终态。故障终局的正确结果可以是有账隔离和停止，不能强求退款伪造成功。QEMU 不覆盖真实非一致 DMA、真实 xHCI halt 延迟与 IOMMU。

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

2026-10-23：在 `refactor/system-redesign`、代码核对基线 `efd29c3` 上完成当前目标方案整理，主代理承担架构选择，ClearHare 参与边界与反例检查，研究子任务核实固定平台资料。前轮身份/Claim/控制根结论已并入正文；本轮补齐登记 Unknown、Provision、冻结表、观察/激活/发布、MMIO/IRQ/DMA、首批热插与故障终局。历史讨论不再作为下一步导航。

**下一步继续在本篇及所属专题收敛和完善设计，再决定施工准入。** 用户本轮确认先整理提交，后续根据计划和方案继续讨论；提交是保存可追溯的设计基线，不表示全部接口已最终冻结或已授权写代码。当前所有新资源 ABI、srv_device/srv_driver、驱动库/程序与设备 Watch 均未实现；现有 trap 尚无外部设备 IRQ 接入，DTB 用户态交付亦待接线。完整能力完成必须有真实消费者和故障验证，不能用本文提交替代。

### 后续收敛顺序

1. **领域与使用体验**：以设备→绑定→功能、应用取得功能、驱动取得父总线资源三条路径审阅当前表达，核对独立概念是否必要；有反例时修订本篇，不新建平行方案。
2. **协议与责任链**：逐项审阅 Register/Claim/Provision/Activate/Stop 的角色、提交点、Unknown、迟到与付款，进一步明确接口细节；保持设备、驱动、总线各自的事实 owner。
3. **资源与终局的可实现性**：对照内存、等待、维护和平台固定证据核定 MMIO/IRQ/DMA 接入、受信边界、静止证明及根死亡；跨专题变更回写唯一 owner。
4. **首批范围与复杂度**：复核 PCI/xHCI/USB 键盘路线是否仍是合适的完整消费者，检查所需模块、资源成本与不支持项；当前路线是本版设计选择，允许据新证据调整。
5. **整体验收与施工准入**：完成固定基线独立 Review 的处置，并据实际修订更新实施依赖、删除清单和验证门；代码施工仍单独授权。

这些是后续审阅重点，不是已确认缺陷清单。独立 Review 的发现进入其活跃报告，设计决定仍回填本篇及所属专题；当前主计划保持活跃，不按任务全部完成归档。

设计检查核对：事实唯一 owner、依赖方向、身份与权限分离、提交/未知/取消、晚到结果、付款与背后物理寿命、故障终局、平台限制、旧路径删除及验收范围。最后核对修正了受信 DMA 退休执行者/报告传递、PLIC claim 与撤销串行、静态孤立保留、MemoryObject 写许可接线、INTx 独占配置和 idle 外部唤醒接点；这些均已进入正文，不另留未处置 findings。

此前文档验证：11 份 Markdown 的 182 个本地链接、47 个标题锚点和代码围栏检查通过，`git diff --check` 通过；提交整理后重新核对修改范围的链接、锚点与格式。证据为源码阅读、固定官方资料与设计路径推演，未运行状态模型、构建、测试或 QEMU；没有代码或 impls 修改。按 [REVIEW](REVIEW.md)在本次设计基线提交后登记固定提交独立审查，不将讨论中的对抗核对替代独立 Review。
