# 设备体系设计：领域模型、驱动管理与资源收束

> 状态：**首批设备闭包的施工设计已闭合；固定基线独立 Review 的四项发现已修订复核，尚未实施或运行验收**。本篇是唯一活跃设计与后续实施记录；当前授权推进设计至可实施，代码施工与新提交仍分别确认。当前方案取代[早期讨论快照](archived/ref-2026-10-23-device-design-rounds.md)中的候选和待选方向。服务装配、平台保留内存、系统终局仍由各自计划拥有；准入范围及证据见[当前接力](#当前进度与接力)。

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
| FunctionAccess | 在明确设备范围取得已发布功能入口；不含 Claim、BindingUse 或资源表；可与观察权装在同一 sender context |
| ClaimSource | 在已授权范围 Claim/StopClaim、恢复本主体原轮控制；由 srv_driver 持有 |
| ClaimSourceAdmin | 封本来源并停止其已提交绑定；交给 init，申请角色不能解封 |
| BindingControl | 查询/停止这一轮，发行 BindingUse、安装不透明激活观察入口和功能关联；srv_driver 持有 |
| BindingUse | 逐名取本轮资源、观察本轮停止；驱动持有，不默认能 Claim、复位其他设备或管理来源 |
| Provision | 本登记单元的准备/停止/结果接口；仅 srv_device 持有调用角色；该接口不附带 Claim、其他 BindingControl 或来源管理权 |

这些是领域权利集合，不要求每个角色另造 mailbox。provider 以内核认证 sender_context_id 查本地不可伪造的授权、范围和付款绑定；payload 里的角色名、PID、DeviceId 都不能替代该绑定。DeviceAdmin 可签发缩小范围的观察/功能/登记/Claim 来源，派生不能扩权；首批授权范围为已存在祖先及其后代，来源被封或祖先 Withdraw 后禁止新的业务取得，旧停止/观察控制仍可收尾。来源签发 Unknown 的装配收束由服务计划拥有，不恢复丢失结果后无条件再签发。

纯 DeviceObserver 的 OAL 视图只给发现信息，不能用 Boundary 绕过 FunctionAccess。`OpenFunction(DeviceId, expected_epoch, name)` 在本地提交点复核当前绑定、功能边和取得范围，返回该已安装入口的别名；名字为单分量，不在管理者内代理功能 RPC。停止后拒绝新取得，已经在途或交付的入口由真实功能 provider 终止。通过 OAL 接续取得同一功能时执行相同权限检查；函数入口的 KOID/context 身份用于重复安装比较，不比较各进程的本地 Handle 数值。

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

Provision backing 可以且必须持有自身供应、资源控制和清理所需的实际能力；例如 PCI provider 持窗口及 DMA/IRQ/MMIO 控制，USB provider 持父绑定和 transport 状态。“调用角色不附带其他权限”不限制实现 owner 的合法持有。登记/占用决定仍归 srv_device，backing 不通过一个 Provision 请求自行 Claim 新设备。

Provision 状态为 `{closed_epoch, active: None | (epoch, Preparing|Prepared|Stopping)}`。epoch 使用原 Binding 的 claim_epoch，不另发硬件轮号；closed_epoch 记录该入口已关闭的高水位，e≤closed_epoch 可 O(1) 判定旧轮，不保留逐轮历史结果。空闲 StopClaim 不调用 provider，因此相邻实际 Prepare 的 epoch 可以不连续。每个已发行 Provision 入口保留小壳至入口寿命结束；NotFound 不作为安全完成，provider 消失返回 Unavailable。srv_device 若在期限内未收到 Prepared/Rejected/Failed，提交原 Binding.Stop；只有收到 Closed 并完成本轮收束、进入 Released 后，匹配者才可依既定候选政策选择下一轮，超时本身不触发自动无限重试。

| 操作/结果 | 合同 |
|---|---|
| `Prepare(e)` | Binding 已提交后调用；只建立一份 active 准备。相同 e 返回原进度/结果，旧已关闭轮返回 Closed，前轮未关闭不得准备下一轮 |
| `Rejected(e,reason)` | 没有遗留效果，provider 已将 closed_epoch 推至 e；可以安全收束该轮 |
| `Failed(e,reason)` | 可能有供应/硬件效果，转 Stopping；不能据失败直接释放 |
| `Prepared(e,count)` | 整表已冻结；逐索引 `GetPreparedResource(e,index)` 返回同一表项的 capability 别名及 name/type/rights/metadata；重复不重新分配业务资源 |
| `Stop(e)` | 幂等；无 active 时可先封闭经认证调用者发行且 e>closed_epoch 的轮次，迟到 Prepare 不再开始；已有相同 e 则取消准备、停止资源并保留责任，存在另一 active 时拒绝越轮。封准入不要求立刻撤掉协作停止仍需的 MMIO/IRQ |
| `ConfirmStopped(e)` | srv_device 转交本轮受权的协作停止事实；重复无新效果，旧轮不作用于新轮。只改变停止所需证据，不自行产生 Closed，provider 仍须完成资源控制与退休 |
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

驱动激活阶段为 Starting/Ready/Stopping/Terminal(ok|error)；两种终态统一，区别仅在有无错误原因，后续处理路径相同。它不复制设备绑定状态。Ready 只表示本次功能可受理，Terminal 报告是协作证据，硬件是否安全仍由 Provision/资源控制确认。ReportStopped/ConfirmStopped 报告链**只在声明了 TrustedPhysical DMA 主控角色的设备**停止路径中使用：管理者通过 BindingControl 的 `ReportStopped` 转交，srv_device 向原 Provision 调用 ConfirmStopped；两者按 BindingRef 幂等，停止期间仍可调用。非 DMA 设备的停止仅走 Provision.Stop 控制侧路径，不需要此报告链。受信物理主控角色必须在装配时具备该证据提交权限，不能从声明/tag 中自授；无此权限的报告仅是诊断。

`StopActivation(ref,deadline)` 幂等，禁止该激活新业务，推进既有请求收尾；其后迟到 Activate 不重新启动同一 ref。重复记录保留至该 Binding 已 Released 且控制关系退休，容量随真实记录付款。一个进程死亡时停止其全部绑定；进程复用节省启动成本，也明确扩大故障影响范围。

Activate 本地接受后即可返回 Starting 和该激活的观察入口，不把一条 RPC 长期扣作 Ready 的唯一交付渠道。管理者先订阅、再重复同一 Activate 读取阶段/Ready 根，按观察序号确认；复用 librpc 序号订阅和 NotificationObserver，保留同一 BindingUse，不重新取得另一轮。libdevice 只保存不透明观察入口，不解释 libdriver 阶段。worker 接受时把请求任务链接到该激活记录，并观察 BindingUse 的停止；管理者失联也不能留下无 owner 的初始化。

终态记录在 Binding 已安全 Released、全部已接请求/观察任务及导出控制关系退休后才能回收。回收后迟到的 Activate 必须先查询所附 BindingUse 的实际阶段；Closed/Stopping 拒绝，新数值 ref 或 payload 不能重建使用权。查询在途任务也先持原记录，不能出现“旧查询成功回复尚在途，记录已删后重新激活”的窗口。只有已认证可信主控明确报告本轮硬件静止，管理者才转交 ReportStopped；Starting/Ready/Terminal(ok|error) 本身均不是这个报告。

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

TrustedPhysical 下，已由装配准入为可信主控的驱动可提供本轮硬件静止报告；报告经实际激活入口认证、绑定到 BindingRef，按上述 ReportStopped→ConfirmStopped 转交，不能由 payload 的可信标志自授资格。PCI Provision 依据该报告、已封口 DmaDomain 的所有 mapping 退休完成和自身 bus-master/IRQ/MMIO 收束确认 Closed，不自行解释 xHCI 寄存器。DmaDomain 提供 Idle/终态观察，只有已封口且无活动映射才成立，不能将瞬时映射数为零当成域停止。驱动报告后永久关闭本轮提交；这是可信基座合同，不能推广为不可信驱动说 stopped 就安全。驱动死亡或报告缺失时没有替代证据，走保留与故障终局。

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

当前 [main](../os/kernel/src/main.rs) 在解析后立即 `release_device_tree`，因此不能把现有 FDT 借用直接交给 init。目标将原 DTB reservation 移交 boot owner，直到有界 Bootstrap Work 将 `totalsize` 个有效字节复制到零填充、固定长度的资金化 MemoryObject；临时原页随后归还。用户只收到 READ/WAIT/DUPLICATE/TRANSIT/GRANT，不收到 WRITE/EXECUTE/MANAGE；不得暴露 DTB 首尾页的无关字节，也不保留指向已归还原页的引用。

初始出生块保留现有通用 Handle 数组和 initfs payload；平台描述单独置于只读 MemoryObject。`shared/erhino_shared::startup::initial` 同步增加该描述对象的固定索引，其后为平台根句柄。描述对象含版本、有效长度、DTB 区间和定宽记录 `(resource_kind, dt_node_offset, handle_index)`；所有偏移、乘法、重复项及实际 Handle kind/rights 均校验。平台几何以相应 control 查询为准，记录不再复制一份可写资源账本。HANDLE_COUNT 的固定五项假设在 kernel/libprocess/srv_init 及测试中一并替换成基础项加实际记录数，不保留旧槽位兼容解析。

根对象、初始 cap 表容量、描述对象、DTB copy 及 boot 终态观察全部准备后才 Start init；失败沿原 boot failure 收尾，不发布部分可用平台。启动包的 initfs tar 仍是程序输入，kernel 不解析驱动声明。srv_init 从已有 tar 条目构造不可变的声明/映像配对来源，将所选程序转为只读映像能力交给 srv_driver；两者不通过运行期可变路径重新配对。完整初始描述只给 init，后继进程只得到对应只读描述及明确缩权的实际资源角色。

### 启动与枚举

1. 内核仅解析启动/内存/中断所需平台事实；启动交付只读 DTB、经校验的 IO/IRQ/DMA 根和启动映像集给 init。DT 文本不是能力，用户态解析不能自铸任意物理授权。
2. init 先保有控制根、故障执行预算和管理 Job，再启动 srv_device/srv_driver，交付各自角色。管理基座和必需 PCI/xHCI 驱动映像可从 boot 来源取得，不依赖尚未启用的设备存储或 UART 日志。
3. 平台适配器登记 PCI host 和静态事实。总线语义的 DT 子树交给该总线解释，中央登记核心不解析 USB/SPI/I²C 私有地址。
4. srv_driver 绑定 PCI host，启动 drv_pci_generic。它取得限定 ECAM/IO 窗口、子资源派生和 interrupt-map 路由信息；枚举首批 bus 0，拒绝未支持桥拓扑。BAR 配置期间封相应解码，按设备 size/alignment 在已授窗口分配，准备完成前不启 bus mastering；范围/页重叠仍由资源层强制。
5. PCI provider 登记 xHCI function，并兑现它的 MMIO、INTx 和 DmaUse。srv_driver 匹配并启动 drv_usb_xhci，读取能力、建立 ring/context、按规范复位/初始化后启用控制器。不得凭函数指针或字符串跨越资源授权。
6. root hub 检测连接、复位端口、通过 xHCI slot/address 流程和 USB 标准请求读取/校验描述、选择配置。地址、配置和完整设备生命周期归 USB owner；在首批选定接口粒度后登记 boot keyboard 接口，不能同时允许外部整机驱动独立占用同一设备。
7. srv_driver 由 Watch 发现接口；boot 来源中已有支持声明但 HID worker 尚未运行，故现在启动，Claim→ResourcesReady→Activate→Ready。应用从设备 functions 或装配别名取得 HID 键盘入口，直接到 HID provider，不经管理者转发按键数据。

PCI 资源准备中的硬件步骤归 PCI Provision backing；xHCI 初始化后的 bus-master/关停由相关 control owner 协作。不以清 PCI bus-master 位单独证明所有既往写入已经结束；正常 xHCI 停止仍须 halt 和在途责任确认。控制器崩溃时首批不实现一个复制 xHCI 状态的救援驱动。

首批资源名字与实际角色固定为：PCI host 取得 `ecam`、`mmio-window`、`irq-control`、`dma-control`、`registrar`；xHCI function 取得 `registers`、`interrupt`、`dma`、`pci-function`；HID 取得 `usb-interface`。控制角色仅交给被装配为总线/provider 的程序，不能从“名字相同”推断权限。typed 解码核对实际 kind/rights；PCI/USB 领域 metadata 由对应库解释，libdevice 不识别这些名字。

`pci-function` 是 PCI provider 拥有的本轮受限会话，允许幂等 `EnableBusMaster`，不开放任意 PCI config 写。Prepare 在关闭 MEMORY decode/BME 时探测并分配 BAR，安装地址后启 MEMORY decode，使 xHCI 可访问寄存器，但保持 BME 关闭。xHCI 验证 halt/reset/CNR、设置已 pin 的 ring/context 后，调用该会话启 BME，再按规范置 R/S 开始运行；PCI provider 在启用提交点复核本轮未 Stop。正常停止先由 xHCI 完成 halt，再由 PCI provider 清 BME/关 INTx 并撤资源。错误路径可提前清 BME 限制后续事务，但不能把它当成已有 DMA 安全结束。启用回复 Unknown 重试同一会话，不重新初始化或发行另一域。

### USB transport 与键盘功能

USB Provision 创建**已限定设备/接口及端点的会话**，表就绪前完成，不在 GetResource 时首次开 session。接口驱动可提交允许的 interface/endpoint 控制请求和 interrupt IN，不能自行 SET_ADDRESS、SET_CONFIGURATION、复位整条总线或访问其他接口。描述和实际会话在同一次资源回复中配对。

USB 请求通过所属协议提交 Setup/端点、方向、长度、数据 owner 和期限；host 校验接收范围并承担从提交至硬件完成/取消的责任。首批小控制/键盘报告走有界消息，host 自己的稳定 DMA 缓冲承接硬件，叶驱动不 pin 自己的任意页。请求取消可能与完成竞态，回复 Unknown 不释放 host 缓冲；session.Stop 阻止新请求，按 xHCI endpoint/slot 命令和完成事件收束旧请求。拔出只是物理事实，DMA transfer/event ring 不因设备消失立即释放。

首批每个接口会话至多一个未确认请求，`Transfer(seq, request)` 在预付任务/缓冲就绪后提交，再写 TD/doorbell；相同 seq 和输入重试返回原进度/结果，不发第二个 USB 事务，输入不同 Conflict。完成结果保留至客户端提交下一 seq，下一 seq 同时确认前一结果；前一请求尚活跃则 Busy。`Cancel(seq)` 可先封尚未开始的当前轮，已开始则进入真实 endpoint 取消；旧 seq 不影响后续。取消 Unknown 时继续原 seq 或停止整个会话，不能丢 DMA 缓冲后重发。seq 不回绕，记录/结果字节和 host 缓冲均在接受前付款；session.Stop 负责最后未确认结果及在途责任。此限制来自首批串行 boot 控制/键盘轮询需求，未来并行传输另以实际队列容量扩展，不预建通用事务日志。

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

本节只安排后续授权后的实施，不创建空 crate 或临时 ABI。中间编辑可以不完整，交付须满足完整消费者与收束链。

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

设备路线通过后，执行 `just check`、受影响 host 测试及既有完整 `just acceptance`（已含 clippy，不重复跑）；DT、启动与安全点变化由其平台/故障组合回归。设备路线纳入聚合时同步构建手册和唯一超时配置，不能通过缩减 core 业务换取设备验收。这里是未来执行门，本轮没有新增命令或运行证据。精确注入若无法由正式设施隔离实现，保留具体未覆盖项并阻止相应强保证宣告，不用普通成功运行抵消。

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

本轮接手基线为 `refactor/system-redesign` / `e6d11a1`，工作树原为干净。用户授权按计划推进到可以实施；本轮完成设计收敛、固定基线独立审查及四项发现的修订复核，未实施生产代码。

**首批施工设计已闭合，可以按“实施依赖与删除条件”进入代码实施。** 当前范围是 QEMU virt/PLIC、PCI xHCI、运行期 USB boot keyboard 热插、真实功能客户端及正常/故障收束；不代表全设备生态、通用服务编排或持久化关机已经闭合。本轮设计文档提交已获授权，代码实施仍需单独授权。

### 施工准入核对

| 原收敛切面 | 本轮定稿及证据 | 准入判断 |
|---|---|---|
| 领域与实际使用 | 设备→绑定→功能、父总线资源、FunctionAccess 与观察/Claim 分权；明确真实 provider 持权 | 所选角色、依赖方向和消费者明确 |
| 协议责任链 | 登记/占用/Provision 共用合法轮次、超时只停旧轮、Activate 观察/迟到回收、USB 请求去重及取消 | 四项独立 finding 已按原反例关闭；无已知未决提交/收尾 owner |
| 内核资源 | RAM/MMIO 来源分开，发布许可与强制撤销、预付槽、树准入/锁序、Pin 地址暴露及独立写许可 | 原 VM/Backing/等待/维护接点已亲读；不留给施工者选择互不兼容的生命周期 |
| 平台与装配 | DTB 保活/只读交付、启动根与实际权限、PCI BME、USB 停止、根 DONE 观察与系统终局 | 固定规范和本机 11.1.2 差异已核定；首批信任边界及拒绝范围明确 |
| 迁移与验证 | 真实路径表、删除旧 dtb/五句柄总数假设/日志驱动占位、device workload/QMP/故障判据 | 按依赖整链实施，无临时 adapter 或假设备前置 |

以上结论是设计可施工，不是实现验收。新资源 ABI、srv_device/srv_driver、驱动库/程序、设备 Watch、外部 IRQ 和 DTB 用户态交付仍未实现。ABI 数字编号、Rust 内部文件拆分和按实际 Layout 推导的容量由实施确定；不得因此改变已定权限、提交点、失败收束或不支持范围。实现发现规范反例时回到本计划修订，不以临时分支绕过。

### 检查、限制与下一步

主代理亲自检查 `MemoryObjectCore/MemoryObjectState`、VM Views/Operation/Backing、bootstrap、ProcessShell/reclaim、trap/sched/锁序、startup ABI、OAL Namespace 及实际 RPC 来源身份接点；explorer 只辅助盘点构建/迁移路径。一次 advisor 咨询用于 VM/资源接线取舍；独立 reviewer 用新上下文审查固定 `9ef1858` 及 `e6d11a1` 修订，原发现及主代理复核见[审查档案](archived/review-2026-10-23-device-system-design.md)。未把咨询、并行草案或原基线审查冒充对最终所有增量的独立审查。

交付前结构检查覆盖 owner、权限与付款分离、不可逆提交、旧回复/输出 fault、清理准入、最后 Close、根死亡、锁序、单步工作界、实际消费者和删除门；追加核对 ReadSegments 与 Retire 竞争、旧 VA 空洞重用、Source 封口后的范围完整性。所选首批范围没有留下已知阻塞设计项；条件后续均在本篇延期表或原领域计划，未把通用服务/动态保留内存/IOMMU 扩展硬塞入首批。

验证包括文档本地链接/标题锚点、围栏与 diff 空白检查，以及 QEMU 版本/设备属性查询。最终文档核验覆盖 10 份 Markdown、211 个本地链接与 64 个标题锚点，围栏和 `git diff --check` 通过；归档移动后的引用也已核对。本轮未运行状态模型、编译、host/target 测试或客体 QEMU，硬件与并发运行门尚待真实实现执行；不把设计路径推演报告为测试通过。

下一步取得代码实施授权后，从启动事实与资源责任切面开始，按本篇顺序连续接通设备领域、管理装配、PCI/xHCI/HID 和实际验收，更新 impls 与本记录。提交须按协作约定展示摘要并取得授权；本轮设计增量提交后按 REVIEW 固定该提交/序列登记一次独立审查，不重复登记已经关闭的四项原 finding。

### 历史基线

`9ef1858` 保存首轮设备目标基线，原文档验证为 11 份 Markdown、182 个本地链接、47 个锚点及围栏/diff 检查。`e6d11a1` 删除 Provision.Query、QueryActivation 并合并激活终态；其中“超时换新轮”和 backing 持权禁令已由本轮修正，不能再作为施工指令。早期讨论只从[历史快照](archived/ref-2026-10-23-device-design-rounds.md)追溯；当前依据始终是本篇正文。
