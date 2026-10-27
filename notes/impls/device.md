# 设备资源内部机制

当前内核包含层级资源树、MMIO/IRQ control/use capability、DMA domain/use/mapping、分步派生/Map/Pin 与逐项退休。受支持的 QEMU virt 启动配置交付真实 ECAM、32 位及 64 位 PCI memory window 控制根、PCI INTx 控制根和 TrustedPhysical/Coherent DMA 根；其他平台没有这些根。srv_device 的平台登记/供应已接统一驱动管理、真实 PCI 与 xHCI worker；PCI 访问 ECAM，xHCI 经限定供应取得 BAR/INTx/DMA，真实 slot/EP0 描述符及 endpoint 配置使用稳定 DMA/ring。HID 经每轮限定 USB session 消费 boot 协议和八字节输入；正式功能应用和两轮热插已接。直接端口两连接、同进程两活 Binding、单接口退出和旧入口尾部隔离使用同一进程/逐绑定 owner；复合 USB 多接口不在当前解析实现内。init 的两管理者独立观察与故障终局已接，具体权限和完成边界见下文。用户不能从裸物理地址构造能力。MMIO View、发布与强制退休由 [mm 实现](mm.md#映射来源与-mmio-退休原语)拥有；请求执行由 [call 实现](call.md#分步请求)拥有。

## 用户态设备领域机制

[`libdevice`](../../user/libraries/libdevice/src/lib.rs) 提供 `no_std` 领域记录、授权 backing、协议和可组合的绑定任务，不依赖 libruntime，不自行创建运行体。srv_device 接收认证消息并驱动这些机制；设备枚举、Watch、功能关联与 OAL 投影已接入服务；平台 Provision、PCI、xHCI 与 HID 消费者组合到各自原运行体。传入数字或链接该库不会取得任何权限。

[`identity`](../../user/libraries/libdevice/src/identity.rs) 的 DeviceId 为非零 mailbox incarnation 与非零单调 serial，BindingRef 加非零 epoch，分别固定编码为 16/24 字节小端值。Ids 的 scan_end 给出一轮 serial 扫描的有限终点，新登记留给下一轮。数值不会回绕或复用，不表达跨启动持久身份。

[`publications`](../../user/libraries/libdevice/src/publication.rs) 为每个 Registrar 的认证 context 保存登记高水位和 AVL 关系表。prepare 持实际节点、记录与字节 Charge，失败不消耗序号；commit 再核原 context、下一序号、封口和容量，失败返还原候选。Withdraw 可以先于恰好下一序号的 Register，旧候选不能复活；旧已删关系只返回 Closed。Seal 同步封新取得，后台按序号逐项撤出；移除要求已 Withdrawn，额外绑定、迟到任务和控制壳的退休前提由宿主负责。移除所得 Publication 继续持自己的存储费用直到实际释放。

[`binding::Seat`](../../user/libraries/libdevice/src/binding.rs) 保存单个可 Claim 设备的唯一当前占用：设备存在性、已发行轮次、主体和阶段。主体来自上层认证 context 表，同主体重复 Claim 返回原轮，异主体 Busy。commit 明确区分 Created/Existing，只有前者移交新预付任务，后者退回新候选、沿用原准备 owner。来源已封或设备 Withdraw 后仍允许原主体恢复已提交控制；ResourcesReady 前必须收齐资源表。Stop 封新使用，原供应明确 Closed 且迟到任务收束后宿主才可 release；Released 快照可留作旧控制壳。Withdraw 后不发行新轮，epoch 耗尽也不回绕。实际范围及来源祖先准入由宿主同门验证，Seat 不替它签发权限。

[`provision::Provision`](../../user/libraries/libdevice/src/provision.rs) 保存单个供应单元的 closed_epoch 和至多一份 active。Prepare 可以跳过没有调用供应者的空闲取消轮，将高水位推进至 e−1；已有 active 时只接受原轮重试。Stop 可先于 Prepare 封原轮，存在另一 active 时拒绝越轮停止。Prepared 的资源数冻结，停止后拒绝迟到 Prepared。Failed 保持 Stopping 和首个错误；ConfirmStopped 只记录本轮受权协作事实，不产生 Closed。finish 的前提是实际供应 owner 已安全结束硬件/资源及迟到责任，运输失败、进程 DONE 和超时不满足该前提。

[`resource`](../../user/libraries/libdevice/src/resource.rs) 的 Collecting 在 Claim 前按登记 Limits 预留项数组及名字/metadata 缓冲，同时冻结实际单项回复头大小，持相应实际字节 Charge；每次 push 不再分配。Prepared count 不得超过上界，索引须连续、名字为唯一单分量、总文本不得越界，每项还须能装入扣除 RPC/回复头后的载荷；全部项到齐后才能消费为 Frozen，部分表没有公开资源取得接口。Frozen 保存实际 capability owner、固定导出描述和不透明 metadata，查询不产生新业务资源。宿主在 Get 时仍须核本轮阶段与范围，停止后拒绝新取得；析构只关闭能力别名，不能代替 Provision 停止。

单项 wire 使用 RPC Reader/Writer，含索引、内核 kind、固定导出 rights、供应协议标识、两份 u32 长度前缀及字节段；解码检查 reserved、尾差和恰好一个 capability 槽。单项大小按 PAYLOAD_MAX 减 RPC 和外层领域头校验；总表可多于一次消息的 handle 数。首批只接受列明的可别名内核资源和 MailboxSender 供应入口；实际 HandleDescribe 须含对应 kind、导出权及管理者重复交付所需 DUPLICATE/TRANSIT，固定导出权本身不必包含这两位。语义核验及 capability 与描述的配对仍须由实际接收任务完成。

## 设备登记 backing 与授权

[`registry::Registry`](../../user/libraries/libdevice/src/registry.rs) 单独拥有来源表、来源管理入口索引和按 serial 排序的实例表，由一个 actor 独占。所有上下文都是宿主从实际内核 sender 或已接消息中确认的身份；bootstrap 仅允许一次，后续来源必须从已有 ADMIN 角色派生。Grant 与对应 SourceAdmin 使用两个不同 context，管理入口只能定位并封闭原来源，不能当作 Claim 或登记角色。派生冻结同一付款账户、权利子集和已存在子树范围；新增源及管理索引全部预付后才链接。

每个来源保存不可变 parent/source scope、权利、付款、登记高水位、生命周期观察状态及宿主任务号。来源树与设备树通过数字身份关联，不用递归强引用链。准入沿来源祖先与设备不可变父链迭代复核；封口或祖先 Withdraw 即刻阻止新登记/Claim/ResourcesReady，无需等待维护扫描到叶节点。StopClaim、release_binding 与维护共用 synchronize_withdrawal，在推进 epoch 前同步祖先/登记来源的有效撤出，避免扫描前关闭或释放旧轮时发行未来轮。仅申请来源封口不会撤出设备，其他有效来源仍可 Claim 后续轮次。扫描每次处理一个 serial，有限终点由 scan_end 提供；它将受影响席位置为停止并返回原清理任务号，不分配新任务或把任务号当成已执行的证明。

实例记录持有名字、不可变 identity、资源 Limits、Provision 的实际 owned payload，以及从真实 HandleDescribe 得到的 sender object/related mailbox 身份。名字和 identity 的字节费用在分配前准入，实际容量多出的部分在提交前补齐；节点及序号关系各持对应费用。prepare 不发布，commit 再核范围、祖先、名字、容量和原登记序号，两个预付节点在同一独占步骤链接。两个相同 Register 候选交错提交时，后者返回 Existing 原 DeviceId，释放多余 payload/费用；静态输入或 Provision 身份不一致则 Conflict。观察快照不包含 Provision 能力。

Claim 仍由 Seat 拥有唯一当前状态。Created 同步记录宿主已经准入的绑定任务号，并持有来源与设备的身份引用；Existing 保持原任务，不重复推进准备。来源封口后原主体可恢复旧控制和停止；释放当前 Binding 后归还这两份引用。实际 BindingControl/Use 的 Lifetime、已发布观察壳及在途 RPC 仍须由宿主另持 retain/release 引用，不因当前席位释放而提前退款。

设备退休要求本登记已经 Withdrawn、原控制 Lifetime 已关闭、没有子实例/身份引用/活动绑定，并由宿主确认原迟到任务已收束；它返回真实 Provision payload 给调用者继续释放。来源退休要求两种角色 Lifetime 都关闭、已封口、无派生来源、无登记记录或任务引用。子来源保留父来源身份，子实例保留父实例身份，子树来源额外保留其 scope 节点，因此旧范围验证不会穿过已经删除的祖先。role_closed 只对应实际 Lifetime 的最后角色关闭，不对应任意一个 sender 副本的 Close。

实际组合者是 srv_device。Registry 的任务号来自已准入的 Runtime 请求，来源观察、回复和清理唤醒由下面的宿主拥有；功能边、Watch/OAL 和父 Binding 约束的子 Registrar 共用同一 backing，具体边界见后文。

## RPC 与长期绑定任务

[`protocol`](../../user/libraries/libdevice/src/protocol.rs) 分开拥有设备与 Provision 的协议 ID。32 字节头包含版本、opcode、状态、正文长度、reserved 和 Deadline；字段均以 Reader/Writer 定宽小端处理。解码检查整帧边界、保留位和实际句柄数。Register 同时检查其后 Device 查询所需的较长回复是否能装入消息；完整大描述的 MemoryObject 形式尚未实现，当前超界明确拒绝。

设备请求按角色处理来源派生/封口、登记/撤出/查询、Claim/StopClaim、绑定查询、Use 发行、逐名资源取得、停止及受权停止报告。Register 入站恰有一个 Provision sender；管理服务通过实际 HandleDescribe 配对其身份。Provision 请求保持原 epoch，Get 还保持原 index；回复校验 opcode、epoch、数量及单项 capability。Progress::Closed 和 Prepare 的 Rejected 是明确安全结果，Error::Closed、NotFound、Unavailable 和运输 Unknown 都不等于它们。

[`binding_work::Work`](../../user/libraries/libdevice/src/binding_work.rs) 在 Claim 提交前准备完整 Collecting、共享控制状态、费用及任务存储，Created 后持有额外 Registry 身份引用。它借用宿主现有 Dispatcher，每次最多一条实际调用；Dispatcher 已注销原调用来源才可 take。取消在本地终结等待后仍向同一 Provision 发 Stop，资源、已收表项和设备责任继续留在原 Work。收齐资源时再核来源/祖先准入，表冻结与 ResourcesReady 连在同一 actor 步骤内。

Shared Binding 不复制当前占用状态：活动期从 Registry/Seat 查询，安全释放后保存原 Released 快照。BindingControl 的恢复母本保持到 Released；旧 Control、Use 和已接请求各持共享引用，Work 在这些引用全部退休后才归还自身来源/设备身份引用。Get 只在当前绑定仍可用时复制固定导出权限，停止立即拒绝新取得。临时发送 alias 可带 TRANSIT，最终 Packet 使用描述中冻结的导出 rights。

停止报告同时要求真实来源报告权与已取得的 DMA use/control。Work 先收到原 Stop 的进展，才转交 ConfirmStopped；硬件静止仍由真实 provider 判定。准备/停止各有绝对期限，重查只限制无新证据的轮询；[`timing`](../../user/libraries/libdevice/src/timing.rs) 同时公布阶段结束与重查的较早期限，及时报告不受旧退避阻挡。Ready/Released 不再保留准备期限。无法取得安全 Closed 时 Work 保留责任并报告故障，由宿主升级，不能析构成成功释放。

## srv_device 宿主

[`server`](../../user/services/srv_device/src/server.rs) 的 World 独占 mailbox、Registry、角色表和 Dispatcher；任务族包含 ingress、原请求、根来源、运输、维护和平台任务。ingress 收到消息后使用 RequestContext 的 sender_context_id 查角色与付款，经 TaskSubmission 准入后才推进业务。输入 Vec 移到请求 owner，不额外复制正文。Outbox 在提交前准入，回复完成后归还消息费用；长期来源、登记和绑定 owner 留在原任务。

[`request`](../../user/services/srv_device/src/server/request.rs) 将原请求转为来源、登记、Binding、Use 或 Watch owner。观察快照不附资源控制；BindingUse 可以查询、观察和 Get，不能签发 Use、子 Registrar、停止或报告。登记和 Binding 的控制母本用于原轮恢复，进入相应终态后移除；Lifetime 的 authority/use 双零包含已接 Delivery，旧消息不会因客户端关掉最后本地副本而失去收尾记录。Observed 只接受相同 kind 的登记回执，底层 SourceRegistration 保留严格断言；注销仍按实际 SourceId 匹配。

BindingControl/Use 的 OAL 根由 [`resource/oal`](../../user/libraries/libdevice/src/resource/oal.rs) 直接投影原 Frozen，按名字逐项分页。ObjectInfo 只给原 BindingRef 和既有 Descriptor，零 cap、无 Boundary；领域 opaque metadata 仍随显式 Get 配对。游标在同一原轮中以冻结项索引恢复最后交付名字，不保存会话或目录副本；旧轮游标拒绝。根与叶的编码布局决定唯一资源名上界，Entry 准入共用该值。Preparing 返回 Busy，停止/释放拒新读取为 Revoked；观察失败不产生 Released。只读投影不触 resources_exposed，实际能力仍只走原 Binding::resource。

[`source`](../../user/services/srv_device/src/server/source.rs) 在根/派生角色交付前建立两份实际 Lifetime 观察；来源关闭只通知一次，避免把静态关闭状态持续转成 runnable。维护每步扫描一个设备或来源，沿原任务号唤醒；停止不再分配另一份清理任务。宿主的复合任务区分 Work 定时通知和 Outbox 自身期限，只有回复期限真正到达才取消回复。

服务配置分别限制来源、设备、Binding、Use、业务请求和控制请求。业务/管理账户分开限制 Task、来源 InputBytes 和字节；只有实际管理或控制角色可用控制预留，伪造 opcode 不扩大配额。长期记录付原来源账户；停止/封口/报告请求由服务管理预留承担，Funding 在每个账户内保证领域和执行身份相同。未准入请求的拒绝回复由已有 ingress 承担运输推进，不新建运行体。

不可恢复的来源/运输或绑定安全故障使管理进程显式失败。init 的 [`device_supervision`](../../user/services/srv_init/src/device_supervision.rs) 通过独立用户线程和 WaitSet 持续观察固定两个管理者，主线程同步 Caller/wait 不负责推进该观察；RootSupervisor 的原 Vec、Runtime/Assembly 和收集账本仍只由主线程访问。两管理者使用同一 loader 的 PreparedProcess，在 Start 前登记缩权 Process READ|WAIT 和原观察记录；srv_driver 的 SourceAdmin 同时交付，srv_device 的根 admin 在认证私有 bootstrap 后补入。记录和单调 gate 由短锁保护，系统调用与等待在锁外。

正常停止在第一个可使管理者退出的请求之前提交原实例意图和绝对期限，DONE 必须核验 Completed/无异常 detail。DONE 先到时保留真实快照等待同次来源 Closed；观察线程关闭其 SourceAdmin 副本并交确认，主线程才关闭 srv_device 最后根/admin。逐项移除 DONE 来源、关闭观察 cap 后才撤防和 join；join 后沿原线程 registry 有界 reap 到只剩主线程。Shared 仍保留故障终局所需的 Job、平台 Seal 和 reset 副本，不把观察撤防或线程记录退休写成所有监督权限、内核 metadata 均已退款。Manager 留在原 RootSupervisor，stop 失败保留记录及未交付责任。

主线程故障与观察线程异常共同争用一个不可逆 FailureTerminal 和唯一动作 owner：先封 services Job，向仍可响应的原 SourceAdmin 有界 Seal，封真实平台根，使用原 JobCollector 枚举/停止受影响执行。总升级界约束实际 wait，但不伪造原观察请求提前超时；到界或不可继续时保留未完成事实，提交 Reboot/SystemFailure（内核 ColdReboot），reset 拒绝则终止整个 init，由 boot 的 DONE 观察停驻。该路径不等待全部 Pool 退款或 Job DONE，不进入旧 Shutdown/无限清理循环。进程失败、JobSeal、Kill 提交或 reset 都不证明 DMA 静止；平台根与内核保留责任继续按各自合同存活。

[`srv_driver`](../../user/services/srv_driver/src/server.rs) 以原 Runtime/Dispatcher 组合设备 Watch、有限扫描、静态声明选择与逐轮 Managed。设备占用仍只由 srv_device 的 Seat 决定。init 的 [`driver`](../../user/services/srv_init/src/driver.rs) 将具体声明与 sealed Image 在同次出生输入中配对，保有外层 SourceAdmin/Job/Pool。Managed 独占原 BindingControl/Use、Ready 后发行的 Registrar/SourceAdmin、激活调用和两类订阅；实际 Released 且订阅/RPC 脱离后解除进程关联，不执行进程收集。来源封口提前终结 Watch 后，Managed 仍使用原 Control 与原期限定时查询 Released。

运行实例统一使用libdriver::view::InstanceId=(Control Mailbox incarnation,出生前checked serial)，原Run与ProcessRecord共用同一身份，PID/路径不授予权限。Drivers视图直接读取原processes/runs，按名弱一致分页，实例下的bindings给原BindingRef及实际阶段/Released观察；无目录副本或读者会话。Control与只读Observer由两个真实Mailbox/认证context承载，仍在同一World/Runtime内使用原Ingress/Request/Outbox；公开队列满不占Control队列。总请求事实outstanding_requests给认证Control保留两席，原任务/来源和两个Ingress存储按配置预付。公开回复占用界为min(原调用期限,5s服务界)，wire回原期限，零cap且无Boundary；Native Once运输由发行者Pool付款，服务用户态请求/回复缓冲仍由服务总请求预算承担。

boot Ready精确交两sender槽及各自Mailbox KOID，init核实际kind/rights/context后保Control并只委托Observer；具体worker原单slot boot不变。原Manager首次停止唤醒公开Ingress，实际注销来源后关公开Mailbox，关闭失败保留owner；旧回复由原Outbox期限/来源收尾，不等外部Observer别名消失。观察错误/成员NotFound只表示弱一致视图中该关联不可见，不证明硬件Released。

驱动选择使用同一Catalog的严格降优先级游标，Run保存当前implementation及首个导致停止的失败原因，不保存候选集合。真正Released且已Detach的失败Run才可自动尝试下一唯一候选；同级歧义拒绝，Quota/Resource/Busy/NotReady或未提交Claim的终态等待显式政策，不因普通Watch变化重试。正常停止不产生自动fallback原因。Manager重新读取当前DeviceInfo并以最新epoch建立下一Managed，保留原Run直到原任务/调用/订阅/进程关联退场。

init的显式并发Retry消费者仍复用同一编解码/发送/采用函数：主线原Control与一份WRITE|WAIT别名分别由独立Caller使用，新线程不持RootSupervisor或管理记录。结果无论成功失败均join/reap，数量只作辅助检查。只读取证从同一ResultReceiver的Tid/control关联原registry唯一Live槽及stack owner，join/reap后核仍存活registry内同槽为Free；不再解引用已退的control/stack。每次RPC各发一次，Unknown不重发；是否实际命中迟到提交由只读原Request/Run取证决定，不以客户端一成一败代替。

Control的Retry(DeviceId)是非幂等本地选择重置，仍由原Request/Outbox持一次Dispatcher只读Query及原Run/ref/implementation；提交前复核同一记录已Terminal/task=0且无关联、设备取样仍present/无活绑定、本World未Stopping。成功只清纯选择记录，不保证远端来源仍开放或新激活成功；原Claim提交点才是授权门。Outbox先终结时取消原Query，等其实际注销后才退休请求。最坏管理查询数纳入CALLS原配置及来源预付，不另建Retry队列/历史账本。

[`ProcessTask`](../../user/services/srv_driver/src/server/worker.rs) 唯一持进程出生、Pool、ProcessControl、私有 boot/DONE 来源与原 SuperviseTask。关联使用出生前建立的 InstanceId；同 actor 根据原 runs 推导已关联容量，覆盖等 boot 和 Activate Unknown，不保存成员表或成员计数。显式 reusable 政策与每进程 capacity 独立；PCI/xHCI 隔离，HID 容量为二。无成员后先封关联，再 Kill/收集；观察回调在旧来源完全注销后移交 collector。进程 DONE 封复用并经原 runs 游标唤醒其全部成员，普通 HID 由存活 USB owner 收硬件，物理 worker 无安全证据时失败保留。

[`libdriver::worker`](../../user/libraries/libdriver/src/worker.rs) 的 WorkerControl 持唯一进程邮箱、认证管理入口、静态实现与原 Budget 活跃额度；ActivationSlot 持逐 BindingRef 的 Activation、独立 function/observation 身份和 Lifetime。Activate/Stop 校验原 Use/Registrar endpoint/context，receipt 交 observation 及 Ready 时的 function；Observe::Watch 返回独立控制 sender 和 NotificationObserver 两槽，每个观察者独立确认。原 Engine 在 Terminal 后仍观察真实 Released，随后关闭 Use/Registrar 与本地 sender 母本，原 Lifetime 来源继续处理外部别名/Delivery 最后退休。旧 context 只对应旧轮，不因新活跃额度可用而复用。

Activate在采用Use/Registrar前先核manager context及实现固定identity/Registrar角色，不匹配返回Permission；诊断记录原BindingRef与匹配结果，调用进程身份由正式日志提供。拒绝不表示Prepare无硬件效果，原Binding仍沿供应停止/安全释放/管理脱离收束。

[`drv_pci_generic`](../../user/drivers/drv_pci_generic/src/server.rs) 从四项平台供应取得受限控制，核真实几何与 coherent DMA 模式后创建自身 ECAM use/mapping，按有界游标读取 bus 0 配置。首批只接受一个 xHCI、拒桥拓扑。子供应复用 Provision/Collecting/Frozen，BAR/INTx/DMA 子资源由该 PCI owner 保存；pci-function 每个供应轮使用独立真实 sender context，只允许当前 Prepared 原轮启 BME。停止报告前结束全部子硬件责任并解除 ECAM，之后服务仅保留软件清理。

PCI 的原 Engine 先经本轮 Use Describe 具名资源，核零槽和原 BindingRef，再用相同名字调用原 Get；回复的 Descriptor 必须与描述一致，实际 capability 的 kind/rights、HostInfo 和物理几何仍独立核验。两类调用共用原 Dispatcher/pending 记录。停止先到不再从描述发起 Get，合法 Revoked/Closed 仅触发原停止路径，硬件静止仍由 Host 实际收束证明。

[`drv_usb_xhci`](../../user/drivers/drv_usb_xhci/src/server.rs) 和 PCI 共用 [`libdriver::worker`](../../user/libraries/libdriver/src/worker.rs) 的激活/观察认证，实际 Runtime 仍由程序组合。Controller 的硬件知识在 [`libusb::xhci`](../../user/libraries/libusb/src/xhci.rs)：等 CNR、halt/reset、逐页 pin/取得地址并初始化环，原 pci-function 确认 BME 后才发布 DCBAAP/CRCR/ERSTBA 并 Run，Ready/运行同时检查 HCE/HSE。共享页只作固定宽度 volatile 访问并 fence，不构造与设备并发的普通 Rust 引用。Controller 按硬件能力、页几何与两份已付连接存储共同限制 MaxSlotsEn；根端口/slot/context/TD 由各自原 Device 记录持有，全局 command ring 仍串行，独立命令准入游标避免快速完成饿死另一连接。事件按实际 pointer/slot 分派；按有限步骤执行 EnableSlot、AddressDevice、描述符、配置和 endpoint 设置。实际 worker 持续消费有界事件与 IRQ 提示；停止先收 endpoint/SetDequeue/DisableSlot 并清 DCBAA/退休其页，再 halt/收残事件/退休控制器页，最后解除 IRQ 观察及寄存器映射、报告消费者停止。接口供应使用原 Provision/Collecting/Frozen 和每轮独立 session context，HID 只取得 `usb-interface`。USB seq 保留具体输入及结果，Unknown 不换序号；取消复用原 StopEndpoint/SetDequeue，接口终结再 DisableSlot/逐页 DONE。

物理连接的存在直接从Controller原Device的disconnected与真实PORTSC CCS判定；重新Prepare期间keyboard描述符尚未生成不触发登记撤出。interface的ready/描述符仍用于实际供应准备。停止只消费同connection原记录的Stage::Closed，该阶段保留原端点/slot及实际页退休保证；缺记录返回错误，不能以描述符缺失或观察失败当Closed。


Engine原pending的Call::Master携带一次绝对期限。Sent且cause为ServiceClosed/Timeout时，先退原RPC/Delivery，再以同Controller.function/ref/context与同deadline重发；不重建Controller、环或DMA域。已观察Stopping时不新发Master，下一步进入原halt/Retire；Frame、取消、shutdown和其他错误不自动重发。期限耗尽或清理失败按原故障保留，成功回复仍严格核opcode/error/空正文/零cap；硬件halt/reset的规范限值独立。

[`srv_init::function_checks`](../../user/services/srv_init/src/function_checks.rs) 是显式验收配置的真实控制器消费者：自动装配仅保留PCI声明，init另取限定该设备Subtree的OBSERVE/CLAIM/REPORT_STOPPED来源，无CHILD_REGISTER；实际资源仍经BindingControl唯一Get出口交原Inputs采用。每轮保留真实Watch与DMA只读观察，复用同Controller的Run、HCH/事件清理、四份ring DMA退休与Unmap后才ReportStopped、等待Released。旧function及Control owner跨轮保留，旧Control不恢复资源，新Prepared拒旧function context；Stop ACK只代表中央提交，同context返回认证Closed才证明provider停止门已传播。此模块不实现USB枚举、供应或自动选择，也不修改正常产品声明。

该消费者的IRQ变体在真实插入返回READABLE后保留原电平，先由原BindingControl完成认证QueryBinding并核原轮ResourcesReady，再probe原use仍READABLE才Take/Ack。两侧都不由观察调用消费token，后续停止及旧generation检查继续同一路径；这不为其他actor增加IRQ能力，也不从一次请求推导长期/同hart公平性。

[`libhid`](../../user/libraries/libhid/src/lib.rs) 拥有 boot keyboard 八字节报告、有效/不确定/断开状态及单调序号。有效报告将 usage 排序去重；ErrorRollOver、错误保留字节和断开保留最后有效键集并使状态无效，真实全零报告才表示全部释放。[`drv_usb_hid`](../../user/drivers/drv_usb_hid/src/server.rs) 组合原 Runtime/Dispatcher 与统一 Activate，SET_PROTOCOL(0) 完成才 Ready，随后用同一接口会话序号消费 Interrupt IN；活跃期不再插入优先周期 Binding Query，停止由管理者原观察/认证 Stop 与 bus session/Provision 收束传递。每个 Binding 的 Instance 独占 ActivationSlot、Keys、USB seq 和 Engine；同一 Runtime/Dispatcher 共享进程，不保留单激活分派。它不取得 Registrar、MMIO、IRQ 或 DMA。功能 Query/Watch/Ack 只交状态，独立 Watch 持自己的确认游标、Notification 和控制 Lifetime；旧 Ack 不清后来报告的 pending。停止封功能新取得并取消原传输，断开终态不生成伪报告。认证/旧轮/拒绝错误仅结束本地提交并保存错误，原 provider 仍持 TD；它们不置 consumer_stopped，实际中央 Released 仍等待供应安全结果。Instance 的原任务继续持有付费终态尾部与真实 Watch Lifetime 来源，按最后关闭事件注销退款；已 Released 尾部不持 Use/Registrar 或活跃额度。最多三十二条实例记录，每实例键盘 Watch 与激活观察各至多八个；满额明确拒绝新准入，不驱逐既有责任。


平台适配器 [`server/platform`](../../user/services/srv_device/src/server/platform.rs) 持 init 委托的真实平台控制别名，通过原 Dispatcher 向原 Register 请求登记 PCI host，登记控制与 Lifetime 仍由同一 Request owner 维护。私有就绪等待该登记回执。Provision sender 使用独立的实际认证 context，原 ingress/Request 直接分派 Prepare/Get/Stop/ConfirmStopped；供应状态复用 Provision，部分表与整表复用 Collecting/Frozen，不保存第二份设备占用事实。

一轮准备每步创建一个 ECAM、non-prefetchable memory window、IRQ 或 DMA 子控制；派生成功先存原 owner，再复制能力进表，后续失败仍停止原轮。metadata 由 [`libpci::host`](../../user/libraries/libpci/src/host.rs) 以固定小端格式解释，保留 CPU/bus 两份窗口几何及实际 INTx 路由。停止先 Seal，等待原轮消费者停止事实，然后撤销 IRQ/MMIO 并等实际 DONE，DMA 已封域亦须 DONE，最后销毁表并提交 Provision Closed。平台没有另存后代等待集合；Registry 变更仍唤醒原平台任务。

Registry 从申请来源祖先及设备祖先登记来源的 Source.binding 推导原 BindingRef 的真实依赖，释放提交点拒绝尚有活依赖的父轮；独立来源登记的静态拓扑后代不自动成为依赖。Work 收到供应安全结果后以原 Next::Release 保留该事实，等待依赖 Released，不重复供应 RPC。原停止期限继续有效，后代变化经维护扫描唤醒 Stopping 父任务。只有中央释放完成，驱动管理才可回收父执行/清理服务；该门不证明 provider 硬件安全。

Observed 直接消费 SourceRegistration 的 Installing/Removing 状态；观察来源尚在安装或注销时任务保持 Runnable，收到真实回执稳定后才 Parked。Runtime 的登记回执本身不承担自动唤醒业务任务。

## 设备发现、Watch 与父绑定来源

[`registry/discovery`](../../user/libraries/libdevice/src/registry/discovery.rs) 在同一 Registry 中提供按 serial 的有限枚举。初次请求的 after/end 都为零，返回当时最高 serial；后续沿原 end 逐项推进，空结果的 next 等于 end。范围每次按实际来源复核，删除跳过，新发行 serial 留给下轮；枚举包含仍保留记录的 Withdrawn 实例，查询再读取存在性和绑定阶段。

Watch 可针对授权范围或已取得的 BindingControl/Use 原轮。记录保存来源、目标、原任务、序号和领域费用，并额外保留 source/device 身份。设备登记、撤出、占用/Ready/Stop/Release 和功能关联改变在原提交点推进匹配观察；单独封 ClaimSource 时也同步卸下其功能并推进独立观察者的序号，不等维护补出变化；来源撤销终止范围 Watch，绑定 Watch 在原轮 Released 时终止，不跟随新 epoch。[`watch::Sequence`](../../user/libraries/libdevice/src/watch.rs) 从 1 开始，旧 Ack 不清新 pending，未来或零 Ack 拒绝；MAX 只用于最后终态，原因和序号不再改变。Query/Cancel/Ack 的终态记录一直保留到实际控制 Lifetime 和已接 Delivery 退休。

[`server/watch`](../../user/services/srv_device/src/server/watch.rs) 只投影通知电平。原 Watch 请求任务持控制 Lifetime，角色表持 NotificationOwner/Signaler，客户端只取得 NotificationObserver。维护逐个唤醒原 Watch 任务，pending 非零时按序号 signal；Ack 当前序号后由同 actor 排空通知。绑定 Watch 的角色额外持 Shared Binding，注销后唤醒原准备任务，不能提前释放旧控制壳费用。客户端 [`client::Watch`](../../user/libraries/libdevice/src/client.rs) 先按成功/失败分别校验正文和能力槽，再采用实际角色，并实现既有 librpc subscription::Protocol，不另建执行器；合法的 Permission/Quota/Closed 回复保留原领域错误，畸形回复才返回 Invalid。

CHILD_REGISTER 是明确的来源权限；普通 CLAIM 不隐含它。经已认证 BindingControl 发行子 Registrar 时，Registry 核父轮 ResourcesReady 和原主体权限，子来源仅获父设备子树内 REGISTER/OBSERVE，并保存父 BindingRef。父 Stop、来源封口或 epoch 改变立即拒绝后代新准入；原 Source 任务随后完成本地封口和清理，旧来源不会随父设备重新绑定恢复。子来源的 scope 引用保留父设备身份。

## 功能与 OAL 投影

[`registry/functions`](../../user/libraries/libdevice/src/registry/functions.rs) 按 BindingRef 保存具名功能根、不可变描述及实际 endpoint 身份；同名同身份/描述重复安装幂等，冲突拒绝。每项在安装前支付 Function 和真实节点/文本容量费用。不透明激活观察入口使用设备记录内预付的单槽，不解释 libdriver 状态。BindingControl 才能安装；显式 OpenFunction 复核 FunctionAccess、来源、范围和本轮可用性，GetActivation 则要求观察权。进入停止时新取得立即失败，功能记录由原状态变更清除；关闭入口别名不等于 provider 已结束业务。

[`registry/oal`](../../user/libraries/libdevice/src/registry/oal.rs) 直接投影登记记录。全局根按设备名字列实例；设备节点下固定为 `binding`、`children`、`functions`，真实子实例位于 children，避免设备名字与固定视图冲突。子树来源的入口根就是该设备节点。binding 是含原设备 ID、轮次、存在性与阶段的观察叶；functions 按具名功能根列边界。纯观察者可以读取拓扑和名字，但取得 `functions/<name>` Boundary 必须经过同一个 OpenFunction 授权门；Boundary 精确保留剩余路径，随后由客户端访问实际 provider。

OAL 逐名分页，每页当前交付至多一项；游标携 registry incarnation、视图身份和最后名字，续页按 OAL 合同使用空路径并从游标恢复原视图，再复核来源权限及范围。删除项或新登记不使同一目录的游标失效；原实例退休后拒绝该游标，同名新实例不会接管旧游标。名字的准入上界从一项完整 ObjectInfo、重复名字/游标和实际消息容量推导，保证非终页可前进。目录页和 ObjectInfo 不包含控制或资源能力；BindingUse的Resources投影见上文，能力仍由GetResource取得。

## 平台 IO 准入

[`shared/dtb::pci`](../../shared/dtb/src/pci.rs) 解析 ECAM、bus-range 和 PCI ranges。PCI 地址为三 cell、size 为两 cell，parent 地址按父宽度解析；IO/Memory32/Memory64、prefetchable 和 bus/CPU 两份区间分别保留。ECAM 必须对齐并覆盖所声明总线，tuple 必须完整、长度非零且加法不溢出。解析器只提供事实，不铸造权限，也不把缺省 dma-ranges 解释为通用恒等合同。

[`board/io.rs`](../../os/kernel/src/board/io.rs) 是物理授权准入者，位于零堆启动阶段。当前只识别 riscv-virtio 的启用根、恒等 simple-bus 父与启用的 pci-host-ecam-generic；验证固定 QEMU 机器的 ECAM、32 位和当前高位窗口，拒绝多 host、未知翻译、IOMMU/MSI 拓扑和非一致声明。PIO tuple 单独核对但不铸成 MMIO。该 profile 的窗口白名单排除 PLIC/CLINT、固件/内核私有 IO；另检查窗口不与全部 RAM、永久 reservation 或其他已准入窗口重叠，不通过切掉洞扩大或改变授权。

IoRoots 的 MMIO 数组至多三个槽，对应这台机器的 ECAM 与两个 memory window；IRQ 与 DMA 各有独立可选平台描述，容量不混同。DMA 根只在固定 coherent、恒等、无 IOMMU profile 下存在。未识别平台或不存在/禁用 host 保留零根；已识别但不支持的启用配置拒绝启动。高位窗口目前只接受 16 GiB 起、16 GiB 长的 profile，更大 RAM 引起的窗口移动不静默沿用旧值。Boot 与 init 的能力交付和校验见 [启动说明](startup.md#只读平台描述)。

## 层级资源树

[`resource_tree`](../../os/resource_tree/src/lib.rs) 是内核侧 `no_std` 纯机制库。`ResourceTree<V,C>` 拥有单个真实根的全部节点，调用者以一把 RESOURCE_TREE 门串行访问，同时在该门内提交实际领域成员和发布许可。它不取得 VM、Pool、等待来源或硬件锁，不执行硬件关闭，也不自行启动 Work。

节点使用全局不复用 NodeId，存储在带已付款节点的 AVL 表中。每项保存不可变 parent、直接活子链的前后链接及首尾、撤销游标、未回执成员数、单向 sealed/revoking/finished 和领域载荷。父子只有数字链接，不以 Arc 互持。一次查找/修改由 AVL 高度界定；任意深祖先关系不在一次调用中遍历。

`PreparedNode` 在门外铸造身份并按实际 Layout 付款；准入成功仅移动已付节点。`link_child` 失败完整返还候选，由调用者在门外释放。已完成节点经 `remove_finished` 摘出为不同类型 `RetiredNode`，不能再插回树复活原身份；解构载荷和退款都在门外。ResourceTree 的析构要求节点表已经为空，不以整树 Drop 替代逐节点退休。

## 准入与封口

`begin(subject)` 保存本根身份、subject、当前 close_seq 和待检查节点；`advance` 每次只核验一个节点是否封口并沿不可变 parent 前进。遍历结束仍不授予领域许可，调用者必须在实际提交的同一门内 `validate`。准入票不能用于另一根，未查完返回 Incomplete；序号变化返回 Busy，不自行重复遍历。实际成员链接通过 `add_member` 同时完成最后复核和责任计数。

Seal 只关闭新准入，保留已有成员；Revoke 包含封口，并初始化直接子撤销游标。两种首次状态变化都推进 close_seq，因此后台尚未触及的后代也不能获得新的许可。序号耗尽以后全根拒绝新准入，旧成员和清理仍可继续；节点 status 的 sealed 表示该节点本地封口事实，不代替祖先准入检查。

`next_child_to_revoke` 每次移交一个直接子项；调用者在同门取得对应稳定领域描述，再在门外通知其预付清理 owner。返回游标结束不代表子项已完成。子项可以在遍历前或遍历中自行结束，摘链会同步修复父游标，避免保存指向已摘节点的下一步。

## 完成与存储寿命

领域 owner 在实际责任安全结束后调用一次 `finish_member`，不能以弱引用升级失败或客户端消散代替物理完成。`finish(node)` 还要求本节点已封口、活子链为空、成员数为零；具体硬件条件由调用方在进入本门前证明。首次完成摘除父的活子链接并返回父 NodeId，供调用者门外通知父 owner 复检；重复完成不再交父回执。

根出生先分配未初始化 Arc 存储，再写入完整 ResourceTree，避免 Arc 分配失败直接析构非空树。领域节点尚未成功出生时的失败，在门内封闭并摘除唯一未发布根，存储带到门外退款；DMA节点已经出生后的保留条目失败由下文Root Work接管。派生候选尚未入树时可直接释放 PreparedNode；已链接对象的失败由其原撤销 owner 收束。

完成节点可以继续保留观察壳和付款存储，但已经不再阻止父完成。父可先退休，已完成子项的 immutable parent 只保留历史身份；对其新准入先因自身 sealed 被拒绝，不再访问已经移除的父。节点物理存储仍由其最后 owner 分步摘除和门外退款。

## MMIO capability 与派生

[`resource/mmio/object.rs`](../../os/kernel/src/resource/mmio/object.rs) 拥有不同实际类型 MmioControl 和 MmioUse，各自持 ObjectHeader、Terminal Holders 和 MmioNode。control 可用 CREATE 派生子 control/use，use 只能映射；READ 查询、WAIT 观察 DONE、MANAGE 封口/撤销，READ/WRITE 同时约束映射和可派生访问上限。EXECUTE 不在任何 MMIO 类型的 rights 上限中。请求 rights 须是来源实际 rights 和目标类型上限的子集。

派生在 handle 门内取得父节点与祖先准入票，锁外按实际 Pool 预付节点、区间索引、观察来源和内嵌 Work，并铸造未交付的 capability。原线程的请求每轮检查一个祖先；最后 RESOURCE_TREE 门内复核票据并同时链接子节点与范围。直接活子区间在父载荷的有序表中排他，前驱/后继查询拒绝重叠；范围按完整页校验，不把同页寄存器授予两个独立子项。父子包含仍允许，别名和同 use 的多次 Map 共享原权利。

新 capability 通过 handle::install 安装，句柄值直接由原 ecall 的 a1 交付。提交前失败或线程退出释放未安装 capability 和准备；已链接后安装失败经 on_last_capability 提交同一节点的撤销。复制/运输/最后持有继续使用公共 capability 机制，公共 Close 不分设备类型。

## MMIO 撤销与 Map 成员

[`MmioNode`](../../os/kernel/src/resource/mmio/node.rs) 持不可变物理范围/写上限、根 Arc、NodeId、独立 DONE 来源、内嵌 Work 和通知槽。根表只持节点 Weak；父子没有强引用链。MmioMappings 的根宿主、节点、准备和区间索引各按真实布局付款。唯一裸物理输入是平台使用的 unsafe root 构造器，要求排除 RAM、固件和内核私有 IO；派生和 Map 只接受已经拥有的节点及 checked 子范围。

Seal 只封新准入；Revoke 或最后 capability 消散提交封口，并初始化直接子与直接 Map 成员的持久游标。Work 每轮只将一个子节点交其撤销 owner，或将一个 Map 交 VM 的预付 FIFO。实际成员留在链上直到 Detached，子节点留在父活子链上直到其完成；提前完成会修复对应游标。没有下一项但尚有责任时，Work 先 arm 再复检并停驻；新回执通过 WakeToken 唤醒。通知槽锁内提交 activate/wake，终态先关闭通知槽，潜在最后根在锁外释放；此 Work 不直接等待 VM fence。

[`MmioMapping`](../../os/kernel/src/resource/mmio.rs) 强持所属节点，节点表只存 Map Weak 和纯生命周期状态。Preparing 登记与发布许可分别经过逐步祖先核验及同门最终复核。Map 首次 Detached 时摘直接成员链、归还树成员责任并锁外通知节点，记录存储可以继续保留。节点完成同时摘父活子链及排他区间，锁外退款后发布自身 DONE 并通知父复检；观察壳不阻止父完成。节点最后析构只移除一个已完成节点；未链接准备或空根出生失败也只有单节点回滚。

## 用户态 IO owner

Map原Admission已遍历完成后发生根sequence变化，最终grant复核返回ObjectBusy，原预约和Preparing成员沿回滚归还；新准入遇已封节点返回ObjectClosed。发布许可成功后即使Revoke到达，仍由原已接受操作完成PTE发布，再经同一ForcedRetirement删除，不能撤回原提交。显式验收沿原32页test_target消费者暂停许可两侧；子进程独立Pool区分活VM页表/容量与CPU DONE后Control观察壳费用，最后关闭才归零。

原`mmio_checks::process_retirement`的Frozen变体仍由同一test_target持两份32页Map、父方提交Revoke后Kill，目标独立Pool，真实scope DONE/members0及Killed/Control关闭后要求原账归还。外部工具分别选择原ForcedRetirement栈上的Idle/非空FIFO及Publishing/原active，不要求另一映射同时排队；step已take的Execution在此期间属于原Work栈，不因self.execution暂空而丢失。实际活VM以既有identity区分，held Execution跨Frozen窗口保存后即停止跟踪旧地址，最终由同PID/scope的原消费者回执核收束。父Kill封口后立即恢复Work，已提交事务先完成，未开始部分交整VM退休；不用等待DONE来安排恢复，不改VM字段或增加通知接口。

[`libkernel::mmio`](../../user/libraries/libkernel/src/mmio.rs) 提供 typed control/use、派生、查询、Seal/Revoke，以及复用普通 Map 提交协议的 MmioRegion<U>。map借用use，map_owned由region直接持use；私有字段只由这两个原Map入口构造，共用范围/对齐/写权限检查及volatile 8/16/32/64位访问，分片Unmap只开放借用形态，不产生普通Rust引用。

父 control 或别名仍能撤销映射，旧 VA 随后可能被另一映射复用。因此 volatile 访问与显式 Unmap 是 unsafe 边界：驱动必须排除本轮并发撤销、别名 Unmap 和地址复用，并满足寄存器宽度/副作用/屏障合同。借用region析构只丢几何，拥有region析构关闭其use，最后cap可触发异步撤销；两者都不等于同步Unmap/DONE，更不证明硬件停止。map_owned失败返还原use，整段unmap失败返还原region；持有use到硬件静止的顺序仍由驱动协调。volatile 和 CPU fence 都不能证明硬件 DMA 已结束。

## PLIC 平台与中断机会

[`shared/dtb::plic`](../../shared/dtb/src/plic.rs) 解析控制器 reg/source 上界、CPU interrupt-controller phandle 与 interrupts-extended；context 的位置是硬件序号。固件标为 `0xffffffff` 的不可用项仍占位置，不压缩编号。`board/irq` 只接固定 virt 的单 PLIC，核对寄存器域不与 RAM/永久 reservation 重叠且被内核直映射覆盖；PCI 四设备、四 pin 的完整 swizzle 表必须指向同一 PLIC，授权仅为 source 32–35。已准入 hart 必须各有唯一 S context。

[`controller`](../../os/kernel/src/resource/irq/controller.rs) 在 registry 冻结后建立 slot→context 表和唯一控制器；全局构造不跨 hart 写 context。每 hart 在固件 cold/warm 初始化完成、共同 CSR 基线和 stvec 建立后，进入 `rt::hart_formal_entry` 的接管点：只清自己的已准入 S context enable、写 threshold=0 并读回核验，最后追加 SEIE；任一读回失败沿原启动 Failed 终段。接管完成才发布 Online，boot 等全员 Online 后才发布 Ready，因此清 enable 不会破坏已激活用户源。trap SEIP、run 安全点、idle 登记后的睡前复检与 WFI 醒后共用 `service`，只在本 hart SEIP pending 时最多 claim 一源，然后回到既有仲裁。SIE 关闭不妨碍 WFI 被局部启用的外部 pending 唤醒。

PLIC 锁拥有寄存器和 source 槽，源只在指定 context 启用；寄存器以 volatile 访问，前后 `fence iorw,iorw` 保序。启用先设 enable、最后设 priority=1；停止在 complete→disable 后将 priority 归零。固定 QEMU 11.1.2 只写 enable 不重算输出，priority 写也使已 pending 来源按新状态重评。静态 source 表容量来自固定平台的有效源 1–95（0 是空 claim）。源槽持实际 Line 强根直到禁源并摘除，所有释放、维护发布和提示在锁外完成。

## IRQ 使用、观察与撤销

[`resource/irq`](../../os/kernel/src/resource/irq/mod.rs) 复用 ResourceTree 的分步祖先准入、封口和子回执。control 拥有连续 source 范围及允许 target 位图，子项只能缩小；use 恰含一源一 target，同层 source 范围互斥，不因目标不同而共享。每个节点、Line、capability 和观察来源均由申请者 Pool 按布局付款，控制根由启动 Pool 付款。树只持节点 Weak，父子没有递归 Arc 链。

派生预付后先在 RESOURCE_TREE 门链接，再在独立 PLIC 门 activate。撤销会先封同一 Line，包括尚未 activate 的准备，所以先行撤销后迟到登记返回 Closed；安装失败或调用者退出由最后 capability 路径提交节点撤销。Seal 只禁新派生，已存在 use 仍可 Take/Ack；Revoke 和最后 Close 通过节点 Work 逐子清理，父节点等全部真实子回执再 DONE。

[`os/plic::Service`](../../os/plic/src/lib.rs) 保存不复用 generation、单调 claim sequence、in-service、taken 和 closed，是消费状态的唯一真值。claim 记录一次在途硬件领取并置 READABLE；Take 只消费提示，返回 sequence，别名不能第二次领取；Ack 验证 generation/sequence/taken 后 complete。序号耗尽时 complete 当前硬件 claim 并禁源、关闭来源，后续显式撤销仍可回收节点。

Line 内嵌 WaitQueue，其电平、订阅链与 Service 同受 PLIC 锁保护，不用锁外延迟电平覆盖终态。control 单独持 SignalSource；use 直接使用 Line 的唯一观察链，不额外分配完成来源。Work 每步复制一个弱目标的在途 Hint，锁外发送；WaitSet 持节点与来源，Remove/退出走原 detach，不由中断保存线程/VM 强根。停止时同门封 Take/Ack，若 in-service 则保持 enable 完成领取，再 disable 和摘源槽；此时只发布 CLOSED 位，来源的终态标志仍为 false。树 finish/释放父占用之后才发布 DONE 并关闭观察来源，等待 DONE 的调用者才能立即复用 source。旧 token 返回 Closed，新 use 的 generation 不复用。可能残留的硬件 pending 只是提示，新驱动仍须检查设备。

[`libkernel::irq`](../../user/libraries/libkernel/src/irq.rs) 提供 typed control/use。Query 的 use generation 固定，Take 只经 a1 返回 sequence，封装组合为 IrqToken；不在消费后向用户缓冲写结果。Ack 通过寄存器交完整 token。READ 用于 Query，Take 要求 READ|WRITE，Ack 要求 WRITE，WAIT 观察，CREATE 派生，MANAGE 停止。内核 IRQ 停止不证明设备或 DMA 静止，驱动的协作停止仍由设备协议负责。

[`irq_checks`](../../user/services/srv_init/src/irq_checks.rs)的显式退出消费者把真实use的READ|WAIT别名与就绪信号交给原test_target；直接变体等待Line，集合变体挂一个真实Line成员后在持续观察作用域中持有WaitSet，以Receive复检候选，空提示后等待再复检。注册产生的初始候选不等同真实Line就绪；本消费者不预期硬件事件，非空记录、意外集合状态及API错误均失败。父方保留use母本，按原ProcessKill/终因/DONE关闭唯一Control，等目标实际Pool全部归还后才再Query原use、显式Revoke并等来源DONE；不以目标CPU完成或最后use Close代替观察loan的退休。就绪信号只交接阶段：直接变体另核实际Waiting/Line来源，集合变体另核真实子cap所持Set与Line成员，宿主恰好Waiting时再核两层来源；不新增产品线程查询接口。

## DMA 稳定 pin 与地址交付

[`resource/dma`](../../os/kernel/src/resource/dma/mod.rs) 的 DmaDomainControl 派生子 control/use；DmaUse 的 CREATE 接受 Pin，DmaMapping 只提供查询、地址读取和 DONE 观察。mode/coherency 固定为显式 TrustedPhysical/Coherent，根只由 board 已验证的 QEMU 无 IOMMU、恒等 PCI DMA 平台发行。真实设备地址不具撤销性，取得它的驱动必须属于受信主控基座；不提供 raw VA→PA 或 MMIO→DMA 旁路。

[`object.rs`](../../os/kernel/src/resource/dma/object.rs) 在普通原线程 Pending 中逐个检查祖先、取得 backing 段。来源只接受真实 MemoryObject，方向以设备为主体：Reads/ Writes/Bidirectional 分别核 READ/WRITE/两者；DmaUse 也必须包含相应方向权利。先预付 mapping、成员条目、稳定保留条目、观察与回滚 Work，再登记 Preparing。MemoryObject 的独立取得/写许可由 [内存 owner](memory-object.md#消费者边界)提供。每轮最多给一段增加 use 引用，退出或错误由同一 mapping Work 取消前缀。Pin 完成只安装能力，不输出任何物理地址。
最终安装仍由调用线程Pool支付cap表预约。`handle::install`在reserve失败后先释放表锁，再释放未安装capability；最后DmaMapping capability触发原stop，ReadyUnexposed的完整use/write交回预付Work。Backing切界后的acquire_step只推进既有段，不再分配。显式安装失败消费者要求实际OutOfMemory、没有返回新mapping handle、use成员/write已退，随后沿原CPU/Control/父MemoryObject关闭核独立子账。宿主工具只在认证原Pin安装栈内，让尚未执行的一次allocator请求返回null；不修改已分配指针，不表示物理堆耗尽。


[`os/dma::MappingState`](../../os/dma/src/lib.rs) 是阶段、封新读取与在途 reader 数的唯一真值：Preparing→ReadyUnexposed→MayBeExposed→Retiring→Retired，取消分支为 Cancelling→Cancelled。状态位于域的成员表，与祖先最终准入共 RESOURCE_TREE 门。ReadSegments 先通过分步祖先检查，再同门授予本次读取、标 MayBeExposed；随后用短期 Exposure 保持 use，按逻辑 offset 经 physical_span 枚举。输出描述最多一页，存储由调用 Pool 预付；payload 和下一 offset 都由原线程经 translate+SUM 写回。任意输出失败仍保持 MayBeExposed，reader 计数在锁外输出结束或错误退出时归还。

域内成员表只存 Weak、状态与双向链接；全局 RETAINED 表强持已登记 mapping，NODES 表强持尚有真实责任的域节点。它们的条目均在出生时预付，物理/层级完成后摘除并门外退款。Mapping 强持所属 Node，Node 强持 Root，Root 只有弱定位，不形成强引用环。最后 mapping Close 只自动取消未暴露准备；MayBeExposed 在没有任何 handle、进程或 VM 的情况下继续持 BackingUse、设备写许可和原 charge，静态保留不激活 mapping Work。最后 control/use Close 传播 Seal，不伪造硬件 Revoke。

域封口每步处理一个直接子或 mapping，已有地址暴露只封新交付。游标结束而仍有静态责任时，Node Work 返回 Idle；NODES 提供存活根，不靠永久 WakeToken 或持续 ready 保活。真实回执会重新发布原节点，全部子项/成员收束后才 DONE。公开快照的 members 是直接 mapping 责任数，域的 DONE 仍按整棵活责任子树判定。

Retire 接受创建该 mapping 的原 DmaUse 或同根真实上级 control 的 MANAGE authority，其他 use 被拒绝。祖先检查逐步进行，不因 Seal 失效。为保留已完成中间 control 的不可变身份，树中分别计算活子责任和存储子引用；完成只摘活子链，Node Drop 标记存储引用消散，Root 内嵌回收 Work 每步删除一个已完成、无存储子引用的记录，再向直接父递减。这样晚到重复 Retire 仍可校验完整祖先链，且不递归析构深树。

[`Node::root`](../../os/kernel/src/resource/dma/mod.rs)在Node成功出生后才分配NODES保留条目；该条目失败时，临时Arc与局部Node按原Drop路径封口/finish、标记dropped并链接reclaim，随后发布已预付Root Work。它与Node出生前失败的同步空根回滚不同；Root Work仍持原树和费用责任，运行停止时不能把Queued接管当作退款已经完成。

Retire 先封新 reader，既有 Exposure 退出后原 mapping Work 才释放。取消时把未完成 acquisition 收束成实际前缀；退休时按 coherent 平台合同排序，再归还原 MemoryObject 写许可、逐段推进 BackingRelease。全部实物责任归还后同门移除成员/回执、摘稳定保留条目，最后发布 DONE。独立 CPU/DMA use 可继续保有同页，不要求整份 backing 归零；观察壳仍按自身存储寿命付款。

[`dma_checks`](../../user/services/srv_init/src/dma_checks.rs)与[`test_target`](../../user/tests/test_target/src/main.rs)的显式竞争消费者复用上述Pin/Seal/Read/Retire/退出路径。子进程只读View为同一Backing保留第一页切界，Pin可取得非空未完成前缀；读取变体用原线程库的第二线程持真实Exposure，主线程Retire后沿原Query/许可/DONE观察再交接读取完成。地址只被测试读取，不交硬件。各目标使用独立Pool，父方继续拥有原MemoryObject；CPU完成后仍存在的原Control/Backing切界存储按真实寿命付款，父对象及相关能力关闭后才要求子账归零。

共享变体对同一MemoryObject取得两份独立mapping，核完整SG相同后逐份Retire、等待DONE并关闭角色；原Notification只交接有限观察阶段。第一份完成时第二份仍持地址与写许可，父MemoryObject的root use继续持物理页。父/子Pool分别观察RAM来源和新增mapping/切界元数据，最后原父对象关闭才释放其实际物理份额。额度变体仅把目标实际付款Pool的READ观察角色交给目标；目标持真实Notification角色制造Quota，再释放压力沿同一DmaUse重新Pin/取消。原成员/write及最终账务仍由同一资源机制决定，不以测试余额替代安全状态，不新增资金或资源账本。

外部取证在原Pending续体从Request参数和advance_in_space的Admitted owner核Thread及SATP；这一段位于调度器set_context之前，不能把HartLocal.current为空解释成无请求owner。具体请求执行面由[call实现](call.md#分步请求)拥有。四竞争窗口取证只安排原CPU次序，不更改许可、成员或返回值；指定安装分配失败另按上述有限宿主故障注入执行。

[`libkernel::dma`](../../user/libraries/libkernel/src/dma.rs) 提供 typed 角色和分页 SG 输出，不产生设备共享页的普通 Rust 引用。Retire 是显式 unsafe 边界，要求设备访问已经结束且缓存的地址不会再次提交；Drop 只 Close，不能代替该证明。首批没有恢复孤立控制的协议；缺少退休证据的页/费用保留至外部复位。
