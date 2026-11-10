# init 组态与统一服务契约

状态：**IF2 代码迁移、host/target 验证与交付前结构检查已完成；未提交，未运行 QEMU。** 静态入口、显式付款和出生命名空间已接通三个正式宿主；代码/静态交付不等于 IF2 的正式运行验收。当前入口见 [IF2 交付与接力](#if2-交付与接力)，前一轮运行证据仅为历史基线。 前一轮运行阻塞从[历史记录](#首次运行阻塞2026-10-29)追溯。本篇是本任务唯一工作记录，拥有目标设计、实施顺序、验证门、债务与接力。上游职责边界（init 根监督、`srv_service` 分工）由[服务装配计划](todo-2026-10-07-service-assembly.md#已确认的-init-与服务管理职责)拥有；系统停止政策由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有；设备安全事实由[设备计划](todo-2026-09-device-system-design.md)拥有。

## 目标与非目标

目标：init 编译为可复用的固定二进制，由 initfs 中的**组态文件**（formation）、映像与服务设定选择持续运行的服务系统。IF2 不再支持有限 task 组态自然完成后由 init 结束系统；关机/重启由受授权电源操作触发。历史验收部署已随客体测试设施移除，不再作为当前消费者。

前提：上游交付完整通用出生环境，领域服务自行采用真实入口、权限和付款；init 不链接、不解码任何领域协议，也不等待业务就绪。

已确认的决定：

- 组态文件是数据（实例、映像、资源、能力路由、初始 Namespace），不是脚本。init 不执行消息编解码、条件或循环，不解释领域协议。
- 系统服务与后续用户服务共用通用出生和入口契约，不按 `srv_` 前缀或启动者拆库，不要求统一 init/start callbacks 或 Ready 握手。监督政策由其实际 owner 决定：init 基础集合退出进系统终局，未来 `srv_service` 可有自身恢复政策。
- 命名采用本项目自己的领域词，不沿用其他系统的惯例目录或术语（如 `etc`、`manifest`、`rc`）。

非目标：`srv_service` 的实现、用户服务描述的格式、服务恢复与重启、持久化。本任务只保证契约与组态 schema 核心可被 `srv_service` 复用。

## 部署结束语义

最新决定由[服务装配 U2.4](todo-2026-10-07-service-assembly.md#启动依赖澄清-u2讨论中)拥有：系统组态是持续存在的服务形态，关机/重启是操作，不是任务执行结果。IF2 删除 `finish`、`kind=task`、task 完成聚合及其驱动的 init 正常停止路径；业务任务完成后可凭授权请求电源管理服务，init 不汇总任务结果。

以下仅记录被替换的现有实现与历史验证，不再是 IF2 目标：

- `stay`：初始装配后继续根监督，任务结束或系统空闲都不使 init 退出。
- `shutdown` / `reboot`：有限组态的全部 task 正常完成后，按停止依赖收束 resident 和可撤销路由，再请求系统关机或重启。当前校验要求至少一个 task；错误及超时仍走系统故障终局。
- 不提供 `exit`：内核依赖 process-zero 存活，init 正常结束的对外语义是系统结束，不是留下其他进程继续运行。初始进程身份及当前 DONE → runtime stop 的实现区别由[服务装配职责](todo-2026-10-07-service-assembly.md#process-zero-与-pm-的职责澄清)拥有。

上述旧路径在 IF2 开工前存在于 `Formation::validate`、`Executor::supervise/stop_all` 和 init `main`，现已删除；该段只保留前一轮语义和证据，不代表当前代码。正常电源操作由[电源服务计划](todo-2026-09-power-management-service.md)与[关机编排](todo-2026-09-system-shutdown-orchestration.md#服务系统定位与职责修订)承接。pm 入口、应用包与普通用户进程视图属于服务装配计划，不在本组态 schema 中提前增设身份分类。

## IF2：静态入口与 OAL 组态装配

本轮接续同一任务，不另开平行装配计划。目标方向由[服务装配 U2](todo-2026-10-07-service-assembly.md#启动依赖澄清-u2讨论中)最新用户决定覆盖本篇旧的「就绪与导出」及实例顺序装配方案；旧章节保留为前一轮设计/实现证据，不再直接作为 IF2 目标。用户已授权先完成本轮服务组态代码任务；电源管理、正常关机编排和新增故障处理明确后置，不再作为 IF2 开工前置。worker 接手本轮实施；开工时提交和 QEMU 均未获授权，后续审阅/清理及条件提交的最新授权见下方当前接力，QEMU 仍未授权。

拟交付：组态声明预建资源、付款 Pool、角色/权限及初始 OAL 命名空间；init 创建并接线资源，向 Building 进程交付完整出生环境后发布，不依赖服务 Export/Ready。fs、device、driver 三个现存正式宿主同步采用预建入口，删除旧动态导出和就绪装配链。静态入口的领域授权及付款关联由相应服务解释并采用，init 不链接领域实现。组装或发布失败按系统致命失败处理，不承诺继续装配。

代码闭包：

1. `shared/erhino_shared`、内核资源工厂和 `libkernel`：按本次预建清单闭合显式 Pool 付款，包括对象主体、sender/Lifetime 等实际持久派生存储，不能只给 MailboxCore 增加付款参数而遗留隐式根费用。临时装载存储仍可由 init 付款；ProcessCreate/Map/Write 与一进程一地址空间不改范式。
2. `shared/formation` 与 host 编译/打包：资源与入口引用、Pool/权限声明、Namespace 描述、二进制格式及校验同次演进；删除依赖运行期 Export 和业务 Ready 的装配规则。资源构造引用的合法性不等于服务业务 Wants/Requires。
3. `libprocess` 出生与 `liboal`：明确命名空间描述的归属、编码和实际能力对应关系；使用方采用上游视图，不由 init 调用领域业务来生成视图，不新增可变 backing 占位机制。
4. `srv_init` 与现有服务消费者：预建/交付资源，采用真实 receiver、sender 身份及领域授权记录，再正常接收已排队消息；init 分别推进发布与 DONE 监督，不逐实例等待 Ready。删除原 Report::Export/Ready、Reporter 对应路径及无用状态/配置，不能保留并行旧装配路径。
5. 同步正式组态、实际 ABI 消费者、相关 host 校验与 impls；必要正式运行由用户指定，不新增 guest harness。

实施先在本篇完成责任链审计，闭合具体 schema、出生采用协议和资源付款清单，再按依赖落代码；范围内内部设计自行决定，不重复请求逐步授权。用户明确本轮先交付服务组态，正常电源管理及新故障/救援机制后置：删除 finish/task 自然完成政策及旧 init 正常停止编排、Export/Ready/Stopped 启动管理报告链，不为保留它们另造适配层。既有设备领域的停止/撤销安全机制不因 init 协议删除而误删；未来正常关机由原电源/关机计划接通，IF2 明确不提供正常关机操作。

装配/发布或基础服务意外退出沿现有系统致命终局底线处理；允许为新的静态资源账本调整必要接点，但不新增恢复、重试、救援流程，不以“失败处理以后做”为由破坏内核资源守恒、初始进程 DONE 保护和活 DMA 保留。init 不等待业务 Ready，不按逆启动顺序推导业务停止。

非目标：电源管理服务、正常关机编排、新故障/救援机制、正式 pm、Service Unit/应用启动服务、包格式、全权 Tier 1 政策、COW/fork、共享 MemorySpace；init/组态文件改名未授权，不顺带执行。pm 功能建议须在这条装配前置闭合后重新核对出生接口。

### IF2.1 接手审计与具体设计（2026-11-10）

基线再次核实为 `master / 68843ce`，20 份已有 Markdown 修改/新增，无产品代码变更。负责人直接读规则、IF2/U2 与实际执行器；四个只读子任务分别盘点付款工厂、fs/registry、device/driver、formation/birth/Namespace。以下是本轮实施依据，不再从旧章节补齐接口；内部选择在已授权范围内闭合，没有新增需用户裁决的架构前提。

#### 资源与组态模型

- formation 格式升为 v2，不解码或适配 v1。删除 `Finish`、`Kind`、`Export`、`Input.revoke`、实例 `after/start_timeout_ms/stop_timeout_ms` 及实例业务拓扑排序；host 遇到旧字段明确拒绝。所有实例都是根监督的持续服务，不按 task 结果结束系统。
- 保留 `Budget { work_pages, metadata_bytes }`、具名 `Pool { name, budget }`、`Job { name, parent, payer }`；实例仍带 `name/image/job/pool/inputs/settings`，新增 `namespace`。具名 Pool 与实例 Pool 不重名，付款者引用 `init`、具名 Pool 或实例 Pool；所有 Pool 可在任何目标进程存在前建立，不从服务存活推导付款账户是否存在。
- 新增 `Resource { name, payer, kind, terminal_revoke: bool }`；`ResourceKind::Mailbox { rights }` 或 `Sender { mailbox, badge, rights }`。Mailbox 主输出为唯一 receiver；Sender 主输出为普通 sender，副输出为配对 Lifetime。每项资源有显式 payer，sender/Lifetime 不默认沿用 mailbox payer。该清单只包含本轮实际静态入口；Notification、MemoryObject、WaitSet 不作为组态新资源工厂，现有运行期工厂不因“可能预建”而扩范围。
- `Source` 保留平台描述/平台根列表、initfs Image、Job、Pool、SelfPool，删除 Export，新增 `Resource { name, output: Main | Lifetime }`。源字符串 `init/object.<资源名>` 与 `init/lifetime.<sender资源名>` 对应两个输出；不再接受 `<实例>/<导出>`。资源名沿用点分段语法；名字及其后缀都只作引用，不推导业务协议。
- `Input { name, source, rights: Option<Rights> }` 只表达能力接线；`NamespaceBinding { prefix, source, rights }` 只允许实际 sender 主输出。每实例 TOML 的 `namespace."/前缀" = { from = "init/object.<名>", rights = [...] }` 显式描述完整初始视图。没有服务间 Wants/Requires、Ready/probe 或运行后拼树。
- 构造依赖只来自 Job 父链和 Sender 对 Mailbox 的引用；先建 Pool/Job/所有 Mailbox，再 mint 全部 Sender。实例按确定声明顺序逐个构造/发布；相互 sender 引用合法，不形成实例启动环。静态检查验证名字、资源/付款/Job引用、唯一 receiver/Lifetime 去向、对象合法 rights 和所有衰减边；实例的普通输入不得占 `oal.entry.*`。
- receiver 恰有一个实例输入 owner，必须 move，不能 duplicate；Lifetime 至多一个提供方输入 owner（无采用消费者时由 init 施工壳释放）。sender 可按同一身份复制；不同领域角色必须独立 mint，别名不产生新 authority。创建母本内部保留施工所需 GRANT，sender 母本另持 DUPLICATE；目标只收到声明权利，不因施工便利升级。
- 输入展开数为普通 inputs（含平台根实际展开）加 Namespace binding 数；最终必须不超过 `PROCESS_MAX_GRANTS`。平台实际数量、初始授权表和 payload 长度在发布前 preflight；出生页数按实际 StartupBlock header、授权表、入口线程描述及 payload 推导，不继续假设固定一页。超过额度、缺失映像和非法引用在副作用前拒绝；实际对象分配或发布失败走致命终局。

#### 纯描述、出生帧与 Namespace 的唯一归属

新增纯逻辑 `shared/oal`（目录、package、crate identifier 均为 `oal`），拥有规范绝对/相对路径规则、Namespace birth 描述/codec、`oal.namespace` section 名和 `oal.entry.N` 输入命名。其消费者为 formation 与 liboal；不依赖用户运行时、内核调用、formation 或 libprocess。这避免 shared 反向依赖 user、复制路径规则或用回调隐藏反向知识。现有 liboal path/view 委托纯规则并映射错误；链接 target 的宽松语义不收紧。

`libprocess::birth` 升为 v2：命名 capability 表、独立 settings、通用 named data sections。`encode(inputs, settings, sections)` 对输入和 section 各按名字规范排序，拒绝重复和非法/截断/溢出；`Birth::take_data(name)` 一次领取，`finish` 同时拒绝未采用的 section 与 capability。libprocess 不认识 OAL。

`oal::namespace::Description` 只含 `prefix → input name`，不含 Source、裸 handle、PID 或领域政策；按 prefix 排序，拒绝重复前缀、重复输入及非法路径，支持 Unicode 和现有普通字符，无任意路径长度上限。init 用共同规则为每实例生成隐藏能力输入及描述 section。`liboal::Namespace::adopt_birth(&mut Birth)` 领取 section、核实际 MailboxSender/发送权利后建立本地完整绑定；缺省 section 表达空视图。失败不交付半成品。liboal 对 libprocess 的依赖从知识使用者指向提供者，不在 libprocess 反向引入 OAL。

三个正式宿主先采用 Namespace 和本领域输入，再 `Birth::finish()`，并在整个服务生命期持有该 Namespace。Namespace 是前缀路由视图，不承诺 `/` provider 的枚举自动合成其他绑定点。它不共享可变 backing，也不使业务已经可用。

#### 内核付款与运输采用

本轮修改现有 MailboxCreate、MailboxMintSender 的 ABI，增加付款 Pool 选择；共享定义、syscall 分派和 libkernel 同次演进。真实 Pool handle 必须具 CREATE，不消费该授权；唯一保留的本地便利选择是明确的“当前进程 Pool”值，不是失败后的 fallback。`Mailbox::create/create_in`、`mint/mint_in` 只组合到同一 syscall/内核构造实现，不保留第二套旧工厂。IF2 预建账本一律调用显式 `_in`。

| 占用 | 付款与释放责任 |
|---|---|
| Mailbox Core、固定队列、Receiver 壳及维护账户 | 该 Mailbox 资源的显式 Pool；最后 receiver 关闭后仍由原内核维护清队列，sender 等实际引用消散后释放 |
| Sender、配对 Lifetime、其 WaitQueue/维护与持久存储 | 该 Sender 资源的显式 Pool；未安装失败完整回滚，最后 authority/use 后 CLOSED，存储随实际引用退休 |
| Send 消息/Delivery 与能力运输暂存 | 继续由真实发送进程付款，不混入入口 Pool |
| 本地 handle 表项 | 继续由实际持有进程的 capability table 付款 |
| init ImageCopy 缓存、装载/ProcessWrite 暂存 | init 自有共享映像缓存及短寿命装载支出，不迁费、不冒充目标页付款 |
| 目标进程/空间/Map | 既有 ProcessCreate(job,pool) 绑定的目标 Pool，不公开 MemorySpace、不共享页表 |

先在 handle 表锁外解析付款授权，再沿原构造器传 `Arc<MemoryPool>`；不在 Mailbox/Notification/Lifetime/WaitSet 来源锁内新增收费或退款。沿原锁阶及预付维护工作推进，Close/DONE 不等同全部外部对象退款。不得因对象转授或提供方退出提前退还仍由对象持有的费用。

`libkernel::ipc::message` 增加完整 pair 采用/校验：sender 的 related_object_id 等于真实 receiver ID，Lifetime 的 related_object_id 等于 sender ID，kind/rights 均核实；失败保留完整 owners。领域库复用该运输知识，本地重复 context 仍由领域采用边界检查。

#### 提供方采用与早到消息

上游已可发送的 sender 不再满足旧“尚未公开”隐含前提。共同接点是：采用全部真实 receiver/pair → 建好本地 Budget/Funding/backing/领域角色 → Lifetime 来源实际登记、表项安装 → 释放只作身份凭据的 sender → 开放对应 Ingress Receive。消息此前留在内核真实队列，沿既有容量/费用/期限和 RPC 背压；不加启动期缓存、Permission 后重试或全局屏障。这个本地授权安装门不向 init 汇报，不等远端业务或平台发现完成。

- fs：输入 `file.receiver`、`file.management.receiver`、可选 `service.receiver`；每 entry 的 sender 为 `file.root.N`、`file.management.N`、`service.directory.N`、`service.authority.N`，Lifetime 分别为 `file.root.lifetime.N` 等（序号始终末段）。各角色 settings 明确业务 rights、输出运输上限和 service Scope，原 watches/sessions 继续建立 entry 的可信同账户 Funding。根仍为现有 MemoryBackend NodeRef。复用 GrantTable/ManagementTable 的 prepare_minted，补全 pair 与重复 context 校验；libservice Root 增加 prepare_minted 并复用原来源/安装/退休 owner。重算 MAX_ENTRIES：旧“32导出/4”不是依据，以实际出生授权约束核定，System 仍一入口。
- 非 registry fs 的 `service.authority` 仍是对外主动 Register 的客户端授权，区别于本宿主采用的 `service.authority.N`。注册/撤出及未知回复责任保留，与本地 Ingress 并发推进，不能等注册结果才处理入站。
- device：输入 `device.receiver`，settings.sources 的第 N 项采用 `device.source.N`、`device.source_lifetime.N`、`device.admin.N`、`device.admin_lifetime.N`。私有 root 和平台 Provision 仍可内部 mint；静态来源复用原 Source owner，在所有静态来源安装完成前不收件。平台扫描/登记不再是来源发布门，正常 Watch 观察空树到增量变化。动态 IssueSource、child registrar、Binding/Use/Provision/硬件静止不改语义。
- driver：保留 control/observer 两只独立 Mailbox 的队列隔离。输入为 `driver.control.receiver/sender/lifetime` 与 `driver.observer.receiver/sender/lifetime`；核配对并在 World 建好真实 context/Funding 后才能创建 Ingress。无业务消费者的 Lifetime 验证后显式关闭，不新增“最后客户端关闭则停止管理器”语义。Manager 观察 device、扫描匹配与被动服务并发；worker BOOT_READY 是领域出生验证，必须保留，不能机械删除所有 Ready 字样。
- 每个领域采用前拒绝重复 context（包括 service Directory/Authority 跨表冲突），不能依赖底层 ordered table 的结构断言。采用失败沿基础服务致命退出底线，既有 root/Prepared/Lifetime owners 释放本地占用，不伪造业务成功。

#### 正式 System、失败和删除门

System 每个宿主显式取得 `/`（文件根）、`/services`（registry Directory）、`/devices`（独立 observe-only `view` Source）、`/drivers`（driver observer）。driver 自身另收原有 claim/child_register/report_stopped Source；不把该权力或任何 admin/control 放入普通 Namespace。device 的 settings.sources 配置 driver 与 view 两份独立来源，使用既有根 Funding 政策；Pool 选择与用户态软账户分别建立。

init 在所有实例 Pool 和静态入口存在后可逐个发布，不等业务 Ready。每次 Start 前持续登记内核 DONE；此前已运行实例的 DONE 在后续有界构造间隙同样消费。失败时保持现有“封 services → 有界 best-effort Revoke → 封平台根 → 有界收集 → 冷重启”底线，不新增恢复/重试/救援。静态 Sender 资源以显式 `terminal_revoke = true` 声明支持既有通用 revoke 协议并委托给故障终局使用；仅 Sender 可设置，静态校验其 WRITE/WAIT/TRANSIT/DUPLICATE 权利。terminal 从施工开始保存这些标记资源的必要别名；不从名字或 `.admin` 后缀推导协议/授权。不等服务回应才登记，不读领域 settings，不等待 Closed，不把进程死亡当作硬件静止。未声明终局角色的 sender/receiver 施工母本在接线完成后释放，不人为保活 Lifetime。

删除：formation旧字段/格式、实例拓扑业务排序、动态导出收集、所有 init Ready/Export/Stopped 编解码及 Reporter、instance.report/stop 自动资源与观察、三宿主专供其使用的 Stop/Release/Publication 状态、finish/task聚合及 init.stop_all 正常关机链。保留 libservice::instance::revoke（设备/原终局仍消费）、运行期 registry Register/Withdraw、所有实际领域退休/撤销和 driver worker出生协议。init 组件名和 system.toml 文件名不改。

#### 实施与验证顺序

1. 下层：Mailbox 付款 ABI/工厂及 pair 校验；共享 oal 与 formation v2；birth v2/Namespace采用。并行编辑按文件独占，交界接口按本节统一。
2. 上层：负责人迁移 init preflight/资源账本/发布/DONE/terminal 与正式 System；fs/service、device/driver 分组改采用和删除旧链。所有方在共同契约接通后再组合验证，不按单文件交付。
3. 编译与 host：`just check`/`clippy` 选择 formation-host、新纯 oal、libprocess/liboal/libfs/libservice/libdevice/libdriver 及三个宿主/init；共享 ABI 影响内核和真实 libkernel 消费者，做 kernel 与实际 user target 检查。host 单测覆盖 namespace codec/路径、birth sections、formation资源/付款/权限/所有权/循环接线及旧字段拒绝；复用既有 libfs/service/device 机制测试，不建设 guest harness。
4. 新增测试捕获独立契约错误：非法/重复Namespace输入、birth未采用section、receiver重复move、错付款/资源引用、sender权限提升、实例引用环被误当启动环、配对身份不符或重复角色。不能把实现内部顺序照抄为期望；真实 syscall/跨进程早到消息与硬件责任只能代码核查并明确运行未覆盖。
5. 按 REVIEW 做最终结构检查，回填 impls 与本记录；QEMU无授权不运行，不提交。成功编译/host不称正式运行验收；若未完成，记录实际断点与欠缺，不归档/宣告交付。

### IF2 施工过程与初始开发检查（2026-11-10）

- 初始开发检查时：Mailbox 显式付款 ABI/工厂及 pair 核验；负责人已改 init preflight、静态资源账本、单独发布与 DONE、显式 terminal_revoke 接点，迁移 System 声明并删除 finish/正常停止链。shared/oal、formation/birth/Namespace 与三宿主采用当时正在并行接通；最终状态见下方交付与接力。
- 分工：A（sub-14）独占 Mailbox ABI/kernel/libkernel；B（sub-15）独占纯 oal、formation、birth 和 liboal；C（sub-16）独占 fs/service 采用及相关 impls；D（sub-17）独占 device/driver 采用及相关 impls。负责人独占 init/System/StartupBlock几何、记录和组合验证。无并行构建。
- 工具链：rustc `1.101.0-nightly (19833af72 2026-10-20)`，commit `19833af7224424e8b53e288097d56b6af43d5bcf`，host `aarch64-apple-darwin`，LLVM `23.1.4`。
- 开发检查：`just check kernel` 通过，日志 `artifacts/checks/20261110T074130.022698Z-74935/kernel.log`；`just clippy kernel` 通过，日志 `artifacts/checks/20261110T074131.484150Z-75004/kernel.log`。对应当前 Mailbox/shared ABI/StartupBlock 几何，不覆盖用户态组合或运行。
- 当时断点：等待三个消费者组完成后统一做 host/target 编译与契约测试；定向只读检查 init 与付款失败责任链正在进行。后续还需完整闭包结构检查、impls/文档一致性和残留清理。未运行 QEMU、未提交；内核编译不能替代真实显式付款/早到消息/硬件验证。

### IF2 组合验证与开发失败记录（2026-11-10）

实际范围覆盖共享 Mailbox ABI、通用出生和全部正式宿主，因此用户态 target 选择 `user` 分面检查所有真实 ABI 消费者；没有运行无影响的全仓回归或 QEMU。所有 host 由 just 显式传 `aarch64-apple-darwin`。

| 检查 | 结果 / 日志 |
|---|---|
| `just check user` | 通过：`artifacts/checks/20261110T074335.900067Z-75519/user.log` |
| `just clippy user` | 通过：`artifacts/checks/20261110T074542.412603Z-76359/user.log` |
| `just clippy formation-host`、`just clippy oal` | 通过：`artifacts/checks/20261110T074543.341151Z-76393/formation-host.log`、`artifacts/checks/20261110T074543.949574Z-76438/oal.log` |
| `just test-host formation` | 15 项（11 library、4 compiler）通过：`artifacts/checks/20261110T074240.550519Z-75148/formation.log` |
| `just test-host oal`、`erhino_shared` | 3 / 32 项通过：`artifacts/checks/20261110T074242.656501Z-75201/oal.log`、`artifacts/checks/20261110T074243.388658Z-75233/erhino_shared.log` |
| `just test-host libkernel` | 10 项及 2 doctest 通过：`artifacts/checks/20261110T074244.147691Z-75265/libkernel.log` |
| `just test-host libprocess -- --features management` | 25 项通过：`artifacts/checks/20261110T074245.291695Z-75309/libprocess.log` |
| `just test-host liboal`、`libfs`、`libservice` | 27 / 43 / 14 项通过：`artifacts/checks/20261110T074622.049800Z-76774/liboal.log`、`artifacts/checks/20261110T074623.550992Z-76827/libfs.log`、`artifacts/checks/20261110T074624.710456Z-76861/libservice.log` |
| `just test-host libdevice`、`libdriver` | 58 / 7 项通过：`artifacts/checks/20261110T074625.556882Z-76894/libdevice.log`、`artifacts/checks/20261110T074627.612439Z-76958/libdriver.log` |
| `just test-host srv_fs -- --lib` | 3 项通过：`artifacts/checks/20261110T074645.377237Z-77080/srv_fs.log` |
| `just build_kernel` | 编译、链接及 ELF 审计通过：`artifacts/checks/if2-build-20261110T074716Z/build_kernel.log` |
| `just make_boot_package` | System v2 schema 编译、七个正式用户 ELF 审计、initfs 与 BootPackage 生成通过：`artifacts/checks/if2-build-20261110T074716Z/make_boot_package.log`；声明序 device → driver → fs，不以该序表达业务依赖 |

开发失败原日志保留：

- `just check user` 首次退出 101，`artifacts/checks/20261110T074259.969541Z-75400/user.log`：负责人 init 仍引用建议名 entry_input/BIRTH_SECTION，与实际 input_name/SECTION_NAME 不符，以及 TerminationDetail 未实现 Display。已按实际 owner API 修正并改 Debug；删除几何提取后剩余 StartupBlockHeader import。libkernel manifest 中无实际使用的 talc 同步清除，不留未用依赖；重跑 user check 通过。
- `just clippy user` 首次退出 101，`artifacts/checks/20261110T074409.901112Z-75740/user.log`：完整 receiver/pair 采用使 ManagementTable::prepare_minted 具有八个独立参数。保留真实边界，以带中文理由的单项 expect 标明，不为 lint 新造无意义 adapter/参数壳；重跑 clippy 通过。
- `just test-host srv_fs` 退出 101，`artifacts/checks/20261110T074628.484056Z-76994/srv_fs.log`：选择了整个 package，连带为 native 编译仅支持 RV64 的服务 binary，产生已 cfg 排除模块错误；不是三个纯 Config 用例失败。改选既有 library 边界 `-- --lib` 后三项通过，不给正式服务新增客体 harness 或 host runtime。

System file/management 的 output 明确保留 READ/WRITE/SIGNAL/WAIT/MANAGE/TRANSIT/DUPLICATE/GRANT，既有流 Open/CreateOpen 需完整 invitation 权利，不将“静态 sender 的发送权利”误当成全部派生输出 ceiling；Directory 保持目录运输范围，Authority 另允许其显式发布/委托输出。设置均由 fs 解释，不由 init 推导。

本轮新增测试核实资源 owner/权限/付款引用、实例环形接线合法、旧字段拒绝、Namespace纯描述/路径和birth section消费、pair真实身份及fs显式角色政策。现有领域 host 回归同时验证协议与纯机制；不能证明真实 syscall 选Pool、早到消息跨进程采用或硬件/DMA行为。正式运行没有授权，尚无 IF2 的 QEMU 运行证据；此前 Ready/Stopped 路线不是该版本的运行证据。

### IF2 结构检查与完成条件（2026-11-10）

负责人按 REVIEW 核对最终 diff、职责、付款与消费、旧路径删除和 impls；最终 git diff --check 通过，变更/新增 Markdown 的 175 处本地链接与锚点检查通过，Mailbox 新 ABI 的实际调用者核对无旧签名残留。用新上下文只读代理交叉核查，不冒充固定提交独立 Review：

- sub-18：init/StartupBlock/Preparation、Mailbox ABI 与内核持久付款，未发现可达 finding。核对 Grant 消费后被 DONE 监督中断也不 double-close、receiver move、施工母本释放、显式 terminal_revoke、Start 前 DONE、付款授权在来源锁外和原自动退休。
- sub-19：纯 oal、formation/compiler/birth/Namespace 与 System，未发现可达 finding。核对唯一真值/依赖方向、格式边界、权利衰减、唯一 move、实例互指合法、section 一次采用与残留拒绝；前缀路由不聚合根枚举属于已声明边界，不作假缺陷。
- sub-20：三宿主、libfs/libservice 采用、System 和 impls，未发现可达 finding。核对本地安装门不提前 Receive、跨角色重复context拒绝、pair身份、witness sender释放、CLOSED/revoke区别、运行期注册并发，以及 Binding/硬件静止、worker BOOT_READY 保留。
- 简化审阅：没有保留并行旧接线、probe-ready、启动期重试缓存或重复进程账本；类型与状态由通用资源、出生描述、本地领域 owner 分别拥有。Sender witness 是运输配对采用的短期 pin，不作为永久第二控制路径。新增 shared/oal 由 host formation 与运行期 liboal 共同消费，未用 facade、回调或源码路径包含隐藏逆依赖。

| 本次条件 | 对应事实 |
|---|---|
| 承诺与非目标 | 静态资源、显式付款、完整初始前缀视图和持续根监督；不提供正常电源操作、pm、恢复或聚合根枚举 |
| 必需前置与消费者 | 付款工厂/ABI、formation/compiler、birth/Namespace 与 fs/device/driver 全部接通，正式 System 实际编译/打包通过 |
| 正常与失败责任 | 常驻监督无“任务正常完成”；装配/发布/DONE 失败沿原致命底线；未消费输入由唯一 owner 关闭，已消费输入不再本地关闭，内核持原自动回收与活 DMA；运行期领域退休/撤销/退款保留 |
| 删除 | 旧 v1/Finish/Kind/Export/revoke-after/timeout 字段、报告链和自动 Stop/Stopped/正常关机链无兼容出口；实际运行期业务 Stop/Register 不误删 |
| 验证与说明 | 上述 target、clippy、239 项 host 测试（含两项 doctest）、ELF 审计和组态打包通过；相关 impls 回填，结构检查无 finding |
| 未覆盖 | 没有 IF2 正式 QEMU 运行，不能把编译、历史 Ready/Stopped 证据或源码顺序证明等同跨进程/硬件运行验收 |

### 主会话交付核对

主会话在 worker 停止编辑后复核工作树、静态资源/付款与 init 发布入口、旧链删除及交付边界，未重新运行已通过的代码检查。读取验证表的 17 份日志，确认文件存在并从测试结果复算得到 239 项通过；核读 BootPackage 构建日志，确认 System v2 与七个正式用户 ELF 审计完成。补正 COMPASS、电源和关机计划中仍称当前保留正常停机链的导航，保留历史证据不改写。最终检查 26 份变更/新增 Markdown 的 209 处本地链接/锚点及代码围栏通过，git diff --check 通过。此为交付核对，不扩大前三组源码结构检查范围，也不替代 QEMU 运行证据。

### IF2 交付与接力

代码迁移与静态交付完成，未提交。全部原 20 份 Markdown 改动保留；本轮没有正式 pm、电源服务、恢复/救援、init 或 system.toml 改名。当前工作树不再是 IF2 半迁移结构，KNOWN_ISSUES 的临时 IF2 施工条目移除；全项目其他重审登记保持原义。本计划保留活跃入口，不将“静态交付”归档成已经完成正式运行验收。

用户随后已授权核实设计落实与剩余安排，并在过度设计/过度防御审查和必要机制清理完成、验证及结构条件满足后提交，由主代理执行最终提交。本轮设计落实/延期审计、独立简化审阅与实际 finding 清理已经收口，不再把“选择 QEMU”写成唯一下一步，也不重复索取已给出的条件提交授权。sub-21/22 的简化审查中断且没有保存报告，不计通过；重派的 sub-23/24 已形成 [S1–S3 简化报告](archived/review-2026-11-10-if2-simplification.md)。修复、定向验证及主代理同组复核均已完成，S1–S3 关闭归档；原 findings、修复和日志由该报告唯一拥有，先前三组 IF2 结构检查不替代该轮审阅。提交授权不包含 push、发布或 QEMU。

正式运行仍由用户另选范围，不主动运行 QEMU，也不恢复已删除 guest harness。若选择运行，观察基础宿主发布及常驻、命名空间输入与本地采用、driver worker出生/设备使用，以及失败时原终局/内核安全底线，保存首错版本/组态/日志；命令由 BUILD-AND-TEST 拥有。真实 syscall 选 Pool/实际费用和退款、早到排队消息的跨进程交错与硬件/DMA行为是验证缺口，不是尚未实现的 IF2 功能；唯一记录继续由本篇拥有。责任链修改或真实反例触发定向取证，不自动重跑无关且已通过的 host 检查。

明确后置能力及既有条件后续按下表承接；本表只登记边界、唯一 owner 与接通门，不复制各计划设计，也不自动授权下一功能施工。正常电源能力当前未接通，不能通过 init 退出代替。

| 能力 / 类别 | 唯一 owner 与工作记录 | 触发 | 完成标准 |
|---|---|---|---|
| 正常关机/重启服务入口（用户明确后置） | [电源管理服务](todo-2026-09-power-management-service.md) | 用户选择接续电源功能；开放正常电源请求前闭合 | 受权请求、动作/并发仲裁与已接管意图由服务稳定持有，交接关机完成事实后执行最终动作，失败有明确归属；以真实请求接通为准，不以服务退出代替 |
| 跨服务正常收束与根监督衔接（用户明确后置） | [关机编排](todo-2026-09-system-shutdown-orchestration.md#服务系统定位与职责修订) | 电源服务开放正常操作前；持久服务接入另触发 drain/flush | 新 owner 下的封口、在途/供应顺序、领域完成、预期退出认证、期限及故障升级接通；最终 reset 前的许可来自真实责任完成，不用 DONE 冒充硬件静止 |
| pm 与 driver 受托创建（既有后续，非 IF2 遗漏） | [服务装配](todo-2026-10-07-service-assembly.md#下一功能任务建议pm-与驱动进程委托) | 用户确认下一功能范围；引用新出生接口前重核详细设计 | 按该案闭合映像/委托授权、预算、取消到 DONE/退款并接通真实 driver 消费者，删除对应直接创建/竞争控制路径；sealed Image 不冒充 OAL 文件装载 |
| 服务管理与故障后继续可用（条件后续，非当前恢复承诺） | [服务装配条件表](todo-2026-10-07-service-assembly.md#条件后续与设计交付检查) | 出现真实独立管理或失败后必须继续可用的需求，且获相应范围授权 | 该案核定普通服务/基础集合责任，闭合恢复状态、旧权力/未知结果与新实例边界，接通真实故障消费者；在此之前保留现有基础实例失败终局，不宣称已有救援服务 |

应用/driver bundle、持久存储/仓库、远程目录范围发行等属于各领域既有条件后续，不是 IF2 拆出未做完的功能；从服务装配条件表及对应领域入口按真实消费者接手，本轮不虚构排期。COW/fork 只作概念说明；公开共享 MemorySpace 不是本轮采用的范式；init/组态改名及全权 Tier 1 政策没有形成当前施工承诺。聚合根枚举明确不在前缀路由交付内；这些均不登记为 IF2 已承诺延期债务。

#### 当前保留的工作集成本推导债务

这是前一轮已有债务，不是 IF2 新增正确性欠账，也没有因静态接线完成而消失。当前唯一活动登记在本节，后面的旧债务表仅保留当时位置/写法。

- 现状/位置：正式宿主 Pool 工作集在 `user/formations/system.toml` 声明，driver worker 仍按 `libdriver::boot::WORKER_POOL` 派生；fs 的部署工作集常量仍在 `srv_fs::deployment`。这些值沿用既有部署，尚无把映像/运行期页、实际 metadata/能力/维护成本统一关联的工作集推导；各角色的业务份额不因此被要求相同。
- 唯一承接 owner：[内核内存预算计划的入站接力](todo-2026-09-14-kernel-memory-budget.md#入站接力正式部署工作集成本)。已核该计划的来源/实际成本、双维 Pool 准入、委派与退款合同；[内存重构](todo-2026-10-01-memory-redesign.md)拥有 backing/View/地址空间，并已将资金预算及硬容量问题指向预算分案，因此不再作为二选一承接者。此为工作集成本/资金边界，不将 metadata 槽误作可兑现堆字节或 allocator 成功保证。
- 触发/保留条件：不可信创建域、紧凑部署、额度不足实证，或首次对指定工作集作精确容量承诺时接续；触发前可保留当前可信正式部署的声明，不承诺尚未证明的硬容量。
- 目标/删除门/验证：预算 owner 先核实际来源和各责任成本，形成可追溯的公共推导；同步正式组态与真实 worker/宿主消费者，消除同一可推导值的重复真值。按所承诺的工作集核正常/拒绝/跨域存活及最后退款；仅数值相同或当前编译通过不能关闭。涉及独立域硬隔离时另满足该预算计划原完成门，不把它作为 IF2 已交付能力。

本轮文档承接核对只修改本篇、服务装配和 COMPASS 的当前入口，未改源码。已核预算与内存计划的实际职责后选定工作集债务的单一预算 owner；上述三篇新增链接共 47 处核对通过，git diff --check 通过。未重跑代码检查、未运行 QEMU、未提交；中断的简化审查仍不计通过。

提交前最终核定：本轮 IF2 承诺均已接通；明确后置能力、既有工作集债务、候选构想与运行证据缺口已分别登记，工作集预算 owner 已补入站接力。主代理逐项复核 S1–S3 简化实现及五份定向验证/打包日志，原授权与资源责任保持，无待修 finding。最终 28 份变更/新增 Markdown 的 217 处本地链接/锚点与代码围栏、git diff --check 均通过；没有源码变更触发的新内核或 host 补测需求，未运行 QEMU。

用户的条件提交要求已满足，主代理执行本次代码/设计提交；随后按 REVIEW 登记可选固定提交 Review。此轮协作与简化检查不替代该登记；提交不包含 push 或运行授权。

### IF2 开工与协作

开工基线 `master / 68843ce`；工作树已有本会话 20 份 Markdown 修改/新增，均保留，不当作外部改动回滚。当时未有本轮产品代码改动。worker Agent「pm 下一步任务讨论」（资源 qr6NxXjcCzZoe8Cw）改为本轮实施负责人；独占当前代码与本计划的编辑协调，若使用子代理，按明确文件/契约边界分配，禁止交叉写入。主会话不同时修改其工作文件。

负责人直接读取本篇 IF2 和服务装配 U2.1–U2.4，不依赖旧交接中的 pm 优先、finish 保留或引导切换假设。实现前记录完整设计；施工中按代码更新 impls，交付前完成必要 host/check/clippy 与结构检查，发现关键前提变化再回主会话。开工时不主动运行 QEMU、未授权提交；后续条件提交按当前接力的最新授权执行。未完成时记录断点、未闭合责任、验证及下一步，不能将阶段检查冒充交付。

当前状态：**IF2 代码及静态交付已收口，正式运行未覆盖；详见 IF2 交付与接力。** 正常电源操作与新故障处理不构成本轮阻塞，后续入口仍由各原计划拥有。

## 当前接力

从 [IF2 交付与接力](#if2-交付与接力)接手。先前代码/静态验证与结构检查已完成，正式运行未覆盖、未提交；最新授权下的设计落实/延期审计与 S1–S3 简化清理已收口，现由主代理执行条件提交。下面旧命名/契约/完成门与运行记录属于前一轮历史实现，其中 Export/Ready、finish/task 和正常 Stop/Stopped 均已由 IF2 替换，不再作为当前接口或下一步指令。

## 命名

本节为定稿，实施中不再另议；新增名字按本节语法派生。

### 术语

| 中文 | 代码 | 含义 |
|---|---|---|
| 组态 | `formation` | 一份完整的初始系统组成：实例、资源、路由与结束动作 |
| 部署 | 组态源文件名 | System 或某一验收形态；一个部署对应一份组态 |
| 实例 | `instance` | 组态中的一个进程；同一映像可有多个实例 |
| 种类 | `resident` / `task` | 常驻（须 Ready、须正常停止）/ 任务（以 Completed 结束即成功） |
| 出生 | `birth` | 进程启动时收到的命名输入与设定 |
| 输入 / 导出 | `input` / `export` | 实例收到的命名能力 / 实例交出的命名能力 |
| 路由 | `route` | 把来源接到某实例的某输入 |
| 撤销 | `revoke` | 停止或终局时收回某条路由派生的全部使用 |
| 设定 | `settings` | 实例私有的类型化配置值，由服务自己解释 |

### 名字语法

- 实例名、输入名、导出名、job 名统一语法：小写 ASCII 段 `[a-z][a-z0-9_]*`，段间用 `.`，总长不超过 32 字节。
- 首段是名字语义的所属领域：`instance.*` 属 `libservice::instance`；`platform.*` 属 `erhino_shared::platform`；`memory.*` 属内核内存对象；`device.*` 属 libdevice；`driver.*` 属 libdriver；`file.*` 属 libfs；`service.*` 属 libservice 目录。新领域按其库名取首段。
- 列表：末段为十进制序号，从 0 连续无空洞，如 `platform.root.0`。`Birth::take_all("platform.root")` 按序取全部；组态中把列表前缀接到列表前缀即整表路由，序号保持。
- 撤销管理端的名字为被管导出名加 `.admin`，如 `device.source.driver.admin`。
- 组态内引用来源写作 `<实例名>/<导出名>`。保留实例名 `init`（init 内建来源）与 `self`（引用方实例自身），不得作为实例名。
- 组态源中的种类、对象类型与权限取值用 snake_case：`mailbox_sender`、`notification_owner`、`memory_object`、`job`；权限 `read`、`write`、`wait`、`signal`、`manage`、`create`、`duplicate`、`transit`、`grant`。

### 文件、组件与构建

| 对象 | 名称 |
|---|---|
| initfs 组态文件 | 根目录 `formation`（二进制，魔数 `ERFORMTN` + u32 版本 1） |
| initfs 映像 | `image/<组件名>`，由现有 `bin/` 改名，与 `libprocess::image`、`StartupImage` 用词一致 |
| 出生帧 | payload 魔数 `ERBIRTH\0` + u32 版本 1 |
| 组态 crate | `shared/formation`：no_std 库；`host` feature 下同名 bin `formation` 为编译器 |
| 设定 crate | `shared/settings` |
| 出生模块 | `libprocess::birth`，类型 `Birth` |
| 实例契约模块 | `libservice::instance`（不复用 libservice 现有目录“服务描述”类型） |
| 内核级验收 task | `test_kernel` |
| 组态源 | `user/formations/<部署名>.toml` |
| 部署名 | 沿用现有 workload：`system`、`core`、`stress`、`memory`、`device`、`device-reuse`；原 feature 覆写各自成为部署：`device-retry-race`、`device-candidates`、`device-function-session`、`irq-retirement`、`irq-exit`、`mmio-publication`、`mmio-forced`、`dma-races`、`dma-shared`、`dma-quota`、`dma-install-failure`。部署名不是 Rust 组件，保留连字符 |
| Justfile | 变量 `FORMATION`（环境 `ERHINO_FORMATION`，默认 `core`）取代 `ERHINO_ACCEPTANCE_WORKLOAD` 与 `INIT_FEATURES`；`make_formation` 编译组态，`make_initfs` 按组态引用的映像组装 |

新增名字均为完整英文领域词，符合 [user/README](../user/README.md) 命名章节，无需登记缩写。源格式用 TOML：编译器只在 host 运行，需要注释、表与数组，Rust 生态有成熟解析器；init 内没有文本解析器。依赖版本按全局规则固定。

### 保留名与内建来源

| 类别 | 名字 |
|---|---|
| 生命周期输入 | `instance.report`（mailbox_sender，write\|wait\|transit）、`instance.stop`（notification_owner，read\|wait） |
| 报告消息 | `Export`、`Ready`、`Stopped` |
| 撤销协议 | `Revoke`、`QueryRevoked` → `Open` \| `Revoking` \| `Closed` |
| init 内建来源 | `init/platform.description`、`init/platform.root`（列表）、`init/image.<组件名>`（只读 MemoryObject）、`init/job.services`、`init/job.<名>`（组态 `[job.<名>]` 声明）、`init/pool.<名>`（组态 `[pool.<名>]` 声明，CREATE 别名） |
| 自身来源 | `self/memory.pool`（本实例 Pool 的 CREATE 别名） |
| 全局结束动作 | `finish = "stay" \| "shutdown" \| "reboot"` |

`SystemReset` 不是来源，不可路由。

### 领域输入与导出名

| 宿主 | 输入 | 导出 |
|---|---|---|
| srv_device | `instance.*`、`memory.pool`、`platform.description`、`platform.root` | 设定 `sources` 表每项 `<名>` 产生 `device.source.<名>` 与 `device.source.<名>.admin`；根来源不导出，由 srv_device 自持 |
| srv_driver | `instance.*`、`memory.pool`、`device.source`、`driver.workers`（worker Job）、`driver.image.<组件名>` | `driver.control`、`driver.observer` |
| srv_fs | `instance.*`、`memory.pool`；不承载 registry 时加 `service.authority` | 每入口 N：`file.root.N`、`file.management.N`；承载 registry 时加 `service.directory.N`、`service.authority.N` |

驱动实现键即映像组件名：删除声明中独立的 `key` 字段，设定条目以 `implementation = "drv_usb_hid"` 指定实现，srv_driver 据此取输入 `driver.image.drv_usb_hid`。实现键、映像路径与输入名由同一个组件名派生，不再有三份名字。

### System 组态源示例

数值按现有常量换算，示例只定结构与名字：

```toml
finish = "stay"

[job.drivers]
parent = "services"
pool = "driver"            # 由 driver 实例的 Pool 付款

[instance.device]
image = "srv_device"
kind = "resident"
pool = { work_pages = 0, metadata_bytes = 0 }   # 按现值换算
start_timeout_ms = 5000
stop_timeout_ms = 40000
inputs."memory.pool" = "self/memory.pool"
inputs."platform.description" = "init/platform.description"
inputs."platform.root" = "init/platform.root"
exports."device.source.driver" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."device.source.driver.admin" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
settings.sources = [{ name = "driver", scope = "root", access = ["observe", "claim", "child_register", "report_stopped"] }]

[instance.driver]
image = "srv_driver"
kind = "resident"
pool = { work_pages = 0, metadata_bytes = 0 }   # 含 worker 工作集
stop_timeout_ms = 40000
inputs."memory.pool" = "self/memory.pool"
inputs."device.source" = { from = "device/device.source.driver", revoke = "device/device.source.driver.admin" }
inputs."driver.workers" = "init/job.drivers"
inputs."driver.image.drv_pci_generic" = "init/image.drv_pci_generic"
inputs."driver.image.drv_usb_xhci" = "init/image.drv_usb_xhci"
inputs."driver.image.drv_usb_hid" = "init/image.drv_usb_hid"
exports."driver.control" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."driver.observer" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
settings.declarations = [
  { implementation = "drv_pci_generic", identity = "<PCI host identity 字节>", priority = 1, capacity = 1, reusable = false, child_registrar = true, trusted_physical = true },
  { implementation = "drv_usb_xhci", identity = "<xHCI identity 字节>", priority = 1, capacity = 1, reusable = false, child_registrar = true, trusted_physical = true },
  { implementation = "drv_usb_hid", identity = "<boot keyboard identity 字节>", priority = 1, capacity = 2, reusable = true, child_registrar = false, trusted_physical = false },
]

[instance.fs]
image = "srv_fs"
kind = "resident"
pool = { work_pages = 4096, metadata_bytes = 1048576 }
start_timeout_ms = 10000
stop_timeout_ms = 10000
inputs."memory.pool" = "self/memory.pool"
exports."file.root.0" = { kind = "mailbox_sender", rights = ["write", "wait", "transit", "duplicate", "grant"] }
# file.management.0、service.directory.0、service.authority.0 同上
settings.registry = true
settings.entries = [{ watches = 24, sessions = 16 }]
```

`instance.report`、`instance.stop` 由 init 为每个 resident 自动供给，不写进组态。identity 字节的书写形式（十六进制字符串）由 `formation` 编译器的设定编码规则定义；领域常量（如 `libpci::host::HOST_IDENTITY`）在 host 单测中与 System 组态逐字节比对，不在两处各写一份真值后不核对。

## 现状（代码盘点，2026-10-28）

srv_init 约 17.3k 行，直接依赖 `srv_fs`、`test_fs`、`libdevice`、`libdriver`、`libpci`、`libusb`，可选 `test_device`；验收约占 55–60%（粗估）。三个常驻宿主做同一件事，写法各自独立：

| 环节 | srv_device | srv_driver | srv_fs |
|---|---|---|---|
| 出生槽位 | 0 报告、1 Pool、2 平台描述、3+ 平台根 | 0 报告、1 Pool、2 worker Job、3 ClaimSource、4+ 映像 | 0 报告、1 停止通知、2 路由 receiver、3 登记 receiver、4 Pool |
| payload | `DEVPLATF` + 根数 | init 用 Rust 常量编码的声明表 | `Profile` 枚举 |
| 就绪 | 一条：registry mailbox KOID + 2 句柄 | 一条：两 mailbox KOID + 2 句柄 | 四种根授权消息（按 kind 区分角色，非句柄上限所迫）+ 空 Ready，位掩码 15 或 2 |
| 就绪后 | init 发 6 次 RPC，含权限拒绝、Watch 拒绝、枚举、OAL 等验收断言 | 无 | 无 |
| 等待 | 阻塞 `receive_until` 5s | 阻塞 `receive_until` 5s | 并入监督轮询 10s |
| 停止 | init 封 observer、等 Watch Revoked、封根、轮询 `QuerySourceStop`；服务无 Stopped | init 发 Stop RPC op1、封 ClaimSource、轮询 `QuerySourceStop`；服务无 Stopped | init signal 停止通知，服务发 `PROVIDER_STOPPED` |
| Pool | 固定 8 MiB + 主栈 | `WORKER_POOL` ×（有 ECAM 5 份，否则 1 份） | ELF load runs + `HEAP_PAGES` |

部署选择散在服务内：`srv_fs::Profile` 决定入口数、是否承载 registry、报告消息数；驱动匹配、优先级、信任位写在 init。故障终局在 `supervision::Terminal::run` 中硬编码 libdevice `SealSource`。部署由 `system` 与十余个 `acceptance-*` feature 选择，Justfile 按 workload 决定 initfs 条目。

### 已核实事实

| 项 | 结论 | 出处 |
|---|---|---|
| 平台上限 | `MESSAGE_HANDLE_MAX = 8`、`MAILBOX_CAPACITY = 16`、`PAYLOAD_MAX = 4096`、`PROCESS_MAX_GRANTS = 64`；出生 payload 无固定字节上限，仅受启动块 `u32` 几何约束 | `shared/erhino_shared/src/message.rs`、`proc.rs`；`libprocess/src/preparation.rs` |
| seat 释放 | 只有 `release_binding` 移除活跃 seat，要求 binding 处于 Stopping、依赖 binding 已释放、提供方进度为 Closed/Rejected。资源已暴露时，清理等待消费者 `ReportQuiescent`（`REPORT_STOPPED` 权限）。封源或关闭最后 Lifetime 只发起停止，不释放 seat | `libdevice/src/binding_work.rs:124,588,645`、`registry.rs:1059`、`srv_device/src/server/platform.rs:222` |
| `QuerySourceStop == Closed` | 该来源子树影响的活跃 seat 为空。正常停止路径下，它蕴含所有已暴露资源的 binding 均已由执行者报告静止；执行者异常死亡时不保证最终 Closed，可能一直 Stopping 或 srv_device 因期限故障 | `libdevice/src/registry.rs:537,575` |
| fs 预建 receiver | 登记 receiver（registry profile）与 secondary 的 publication inbox 共用槽 3；secondary 依赖 init 在 Start 前把 publication authority 投入 inbox。路由 receiver 成为 fs 管理 receiver。没有找到必须由 init 建立的付款或身份理由 | `srv_init/src/filesystem/launch.rs:176`、`srv_fs/src/server/boot.rs:156,202` |
| 就绪 KOID 消费者 | device incarnation 与 driver `ManagerReady` 两个 KOID 只有 init 解码 | `srv_device/src/server/source.rs:257`、`srv_driver/src/server/manager.rs:398` |
| driver Stop op1 | 除 init 外，`test_device` 只把它发给 observer 端作为权限负探针 | `test_device/src/drivers.rs:62` |
| 平台描述 | 格式属于 `erhino_shared::platform`：头含 `resource_count`，资源记录含 kind、DT 节点偏移与初始槽号；不含 ECAM 标志。ECAM 识别需要 libpci 解析 DTB | `erhino_shared/src/platform.rs`、`libpci/src/platform.rs:250` |
| worker Job | manager Job 与 worker Job 是 `services` 下的兄弟；manager 死亡不封、不杀 worker。系统终局封 `services` 并由 JobCollector 有界收集全部后代 | `srv_init/src/driver.rs:285`、`supervision.rs:125` |
| Pool 委托寿命 | 派生 Pool 的整份委托只在其全部计费退休后才回到父 Pool。`MemoryObject::create`、`DmaDomainControl::create_control/create_use` 隐式向调用进程绑定的 Pool 计费，无显式付款参数。故意跨 reset 保留的 DMA 映射会使创建者所在 Pool 的整份委托一直保活 | `libkernel/src/memory_object.rs:37`、`os/kernel/src/resource/dma/object.rs:195,304`；切面 A 开发 core 日志 `artifacts/failed-acceptance-20261029-082357-44659.log` |

## 分层与知识归属

依据 [libraries/README](../user/libraries/README.md)“不按调用者是不是系统服务拆出重复知识库”。

| 层 | owner | 内容 |
|---|---|---|
| 出生输入 | `libprocess::birth` | payload 帧中的命名输入表与设定段；进程领域，task 与测试进程也可用 |
| 实例契约 | `libservice::instance` | 保留输入、Export/Ready/Stopped 消息、撤销协议；子进程侧 helper；监督方 launcher（libexecution 任务，init、`srv_service`、验收控制 task 共用） |
| 组态 schema | `shared/formation` | 二进制编解码、静态校验；`host` feature 下为 TOML 编译器 |
| 服务设定 | `shared/settings` | 布尔、i64 整数、UTF-8 字符串、列表、表的规范编码与只读访问；表键按字节序唯一排列，解析时一次校验全树，访问不再失败于格式；不认识任何服务键。字节值（如驱动 identity）以整数列表表达，由服务按自己的键解释 |
| 领域语义 | libfs、libdevice、libdriver 等 | 导出名常量、设定键及含义；导出能力的业务协议不变 |
| 执行器 | srv_init | 组态执行、根监督、系统终局；只依赖 libkernel、libprocess、libservice、libexecution、libbudget、librpc、erhino_shared、tar、elf、formation |

`libservice::instance` 只依赖 libprocess、libexecution、librpc、libkernel，不依赖 OAL 与 registry；不注册目录的服务只用该模块。libdevice 依赖 `libservice::instance` 的撤销协议并在 SourceAdmin 端实现；libservice 不认识设备。

## 契约一：出生

- payload 帧：魔数 `ERBIRTH\0`、version、输入表（名字 → 槽号）、设定段（`settings` 字节）。名字按[名字语法](#名字语法)，表内唯一。对象类型与权限以 `adopt` 时内核 query 的实际结果为准，表中不记录。
- `Birth::adopt(StartupContext)` 提供：`take(name, kind, min_rights)`；`take_all(prefix)` 取列表输入（如 `platform.root`）；`settings()`；`finish()`。`finish()` 拒绝未取用的输入，使组态与实现不一致在出生时失败。
- `instance.report` 与 `instance.stop` 的类型与权限见[保留名](#保留名与内建来源)。其余输入（`memory.pool`、Job、映像、平台根）均为普通命名输入。
- 不需要生命周期的 task（如 `test_fp`）可空 payload、不使用 `Birth`。
- 不采用 ELF note 携带契约：输入/导出不一致在出生与就绪时被捕获，note 需要额外构建步骤并依赖 strip 保留，收益仅是提前到构建期发现。

## 契约二：就绪与导出

- 子进程在 `instance.report` 上依次发若干 `Export { name }`（每条恰一个句柄），最后发 `Ready { export_count, summary }`。一导出一能力，例如 device 的预限定来源与其管理端是 `device.source.driver` 与 `device.source.driver.admin`。`summary` 为可选领域字节，init 不解释、可转交消费者。
- init 通用校验：sender pid 与 `instance.report` 通道 context；导出名属于组态为该实例声明的集合且不重复；实际 kind 与 rights 不低于声明；Ready 时计数相符、声明导出全部到齐、进程仍 Running 且未 DONE。
- 删除 KOID/incarnation 交叉核对与位掩码（仅 init 消费，见已核实事实）。
- 就绪等待并入监督轮询，每轮有界领取；init 持续排空 `instance.report`（容量 16）。子进程按启动期限 `try_send`，超期即启动失败。

## 契约三：停止

- init 对 `instance.stop` 发 signal；`instance.stop` 关闭（init 消失）同样视为停止请求。服务自行收尾，发 `Stopped`，以 Completed 退出。init 要求 Stopped 与正常 DONE 两项事实，期限取组态 `stop_timeout_ms`，超期进系统终局。
- srv_fs：现有停止通知与 `PROVIDER_STOPPED` 改用统一消息。
- srv_driver：删除 Stop RPC op1，改为响应 `instance.stop`；在 Binding Released、worker 收集之后发 Stopped。`test_device` 的 op1 权限负探针随之删除。
- srv_device：收到 `instance.stop` 后封自身根来源、等根来源退休、发 Stopped。原由 init 执行的封源、Watch 终止确认与 `QuerySourceStop` 轮询移入服务内部。

## 契约四：可撤销导出

- 组态可在一条路由上声明 `revoke = "<实例名>/<导出名>.admin"`。该导出必须是实现撤销协议的管理端，只有两个操作：`Revoke`（幂等）与 `QueryRevoked → Open | Revoking | Closed`。
- 语义：`Closed` 表示提供方账本中经该来源派生的活跃使用为空。“活跃使用”及其与硬件静止的关系由领域拥有。libdevice 的实现即现有 SealSource 与 QuerySourceStop；依已核实事实，正常停止路径下 Closed 蕴含已暴露资源均已由执行者报告静止，可作为放行门。
- init 的两处使用：
  - 正常停止：消费者 Stopped + DONE 之后撤销其收到的可撤销路由，轮询至 Closed，再停止提供方。Revoking 超过提供方 `stop_timeout_ms` 进系统终局。
  - 系统终局：对已收到的全部撤销管理端发 `Revoke`，替代 `Terminal::run` 中的 libdevice `SealSource`。终局不等待 Closed，仍按现行封 `services`、有界收集、冷重启。
- 签发方式：需要给其他实例的受限来源，由提供方按自身设定在 Ready 前签发为预限定命名导出。srv_device 按设定 `sources` 表签发 `device.source.<名>` 与 `device.source.<名>.admin`（scope 与 access 写在该表项）；System 中 driver 使用 `device.source.driver`。init 只路由，不再运行期调用 `IssueSource`；原“签发 Unknown 时封原根”分支随之删除，签发失败即 device 未就绪，走启动失败终局。

## 组态文件

### 字段

| 位置 | 字段 | 含义 |
|---|---|---|
| 顶层 | `finish` | 全部 task 正常完成后的动作 |
| `[job.<名>]` | `parent`、`pool` | 父 Job（`services` 或另一具名 job）；付款 Pool 所属实例 |
| `[pool.<名>]` | `work_pages`、`metadata_bytes` | init 从根 Pool 派生并持有至系统结束的具名 Pool，不随任何实例收集而关闭；用于寿命超出单个实例的计费（如故意跨 reset 保留的计费）。额度取其承载进程的部署工作集：能从公共契约推导的按推导，否则沿用项目已有的单进程部署工作集常量并在债务登记，不另拟数值 |
| `[instance.<名>]` | `image` | initfs 内 `image/<组件名>` 的组件名 |
| | `kind` | `resident` 或 `task` |
| | `job` | 可选，具名 job；缺省时 init 在 `services` 下为该实例新建 Job |
| | `pool` | `{ work_pages, metadata_bytes }`；映像页由 ELF load runs 推导后另加 |
| | `inputs."<输入名>"` | 来源字符串，或 `{ from, rights, revoke }`（`rights` 只衰减） |
| | `exports."<导出名>"` | `{ kind, rights }`，为最小要求 |
| | `settings.*` | 内联设定，编译时编码进 `formation` |
| | `start_timeout_ms`、`stop_timeout_ms` | 期限，单位毫秒 |
| | `after` | 无数据依赖时的顺序约束，列出实例名 |

来源只有[保留名与内建来源](#保留名与内建来源)所列的 `init/*`、`self/*` 与 `<实例名>/<导出名>` 三种。

### 结束与 initfs 布局

- 全部 task 正常完成后执行 `finish`。task 失败与 resident 意外退出一律进系统终局。
- initfs 只含 `formation` 与 `image/*`。设定内联进 `formation`，不另设设定文件目录。

### 静态校验（任何副作用之前）

路径存在且唯一；实例名、输入名唯一；引用存在；引用的导出已声明；rights 只衰减不提升；依赖无环；预算加和不溢出且不超过根 Pool；实例与路由数不超过实现上限。编译器与 init 调用同一校验函数。

### 执行

- 按依赖拓扑序启动，组态顺序作为确定性 tie-break。resident 的导出在其 Ready 之后才可路由给后继。
- 停止按反拓扑序，每步为“消费者 stop → Stopped + DONE → 撤销其可撤销路由至 Closed → 下一个”，替代 `deployment.rs` 固定的 Consumers/Files/Devices/Root 阶段。停止总预算为各实例 `stop_timeout_ms` 之和加根预算。
- Supervisor 运行体、记录与来源数由组态在执行前计算并一次准入，替代 `Instance` 枚举、`deployment::instances()` 与 `file_hosts()`。
- 组态解码结果与 initfs 映像在 init 生命周期内持有。

## 领域迁移

- **srv_fs**：删除 `Profile`。设定 `registry`（布尔）与 `entries`（每入口 watches/sessions 份额）决定形态，`Capacity` 由设定计算。管理与登记 receiver 改由 fs 自建（未找到须由 init 预建的理由）。四种根改为[领域导出名](#领域输入与导出名)所列命名导出。secondary（`registry = false`）的 publication authority 改为输入 `service.authority`，路由自 primary 的 `service.authority.N`，删除 Start 前投递与 parent pid 核对。
- **srv_driver**：声明表进入设定 `declarations`（implementation、identity、priority、capacity、reusable、child_registrar、trusted_physical），映像为输入 `driver.image.<组件名>`，`image_bytes` 由 sealed 对象有效长度得出。worker Job 为输入 `driver.workers`。worker 工作集计入 driver 实例 `pool.work_pages`，由部署决定；srv_driver 启动时校验额度覆盖声明，不足即启动失败。init 不再识别 ECAM。
- **srv_device**：输入 `platform.description` 与列表 `platform.root`；导出设定 `sources` 声明的预限定来源及其管理端。根来源由 srv_device 自持、不导出：`instance.stop` 时它自行封根并等子来源退休，故 init 不再持有 root 入口、observer 与 Watch，终局撤销只针对已路由的来源管理端。
- **init 平台输入**：平台描述头经 `erhino_shared::platform` 解析，按 `resource_count` 与 `handle_index` 取根。终局按实际 object kind 封 MMIO/IRQ/DMA 根，只需 libkernel。删除 libpci 依赖。

## 验收迁移

- 内核级检查（IPC、内存映射、时间、Job/Pool、tunnel、Once、race、MMIO/IRQ/DMA 根、Building/Job 管理）移入新 task `test_kernel`，接收派生 Pool/Job、所需映像与平台根别名，由它启动 `test_target`、`test_hammer` 与 `srv_pm`。断言 sender PID 等于 `init_pid` 的检查改为对照自身 pid。平台根别名只出现在验收组态，用 `after` 保证与 srv_device 使用串行。
- 寿命到 reset 的计费不得进入可回收 Pool。`test_kernel` 另收输入 `memory.pool.retention`（路由自 `init/pool.retention`）。DMA `process_retirement` 中故意跨 reset 保留的模式（exposed、output-fault）由 `test_kernel` 以保留模式自启动的子进程执行：子进程出生时绑定 retention Pool，原 fixture 的 scope/use/MemoryObject 创建、目标启动与核验整体在子进程内运行，判据不变。验收断言：该子进程收集后，`test_kernel` 主 Pool 在其自身收集后双维归零；retention Pool 的残留恰为被保留映射的计费。其余 fixture 若存在跨收集保留的计费，按同一规则归入保留域。
- 服务级检查由已有 `test_device`、`test_fs` 承接：device/driver 自检（权限拒绝、Watch、枚举、OAL）移入 `test_device`；fs/registry/copy/secondary 生命周期移入 `test_fs`，secondary 由 `test_fs` 用 `libservice::instance` launcher 启动。正式装配路径不再包含探针。
- 留在 init 的是自身不变量：startup capture、idle ownership、dispatcher funding、终局与 reset。reset 负路径检查删除：attenuated rights 拒绝属于内核契约，由 `test_kernel` 持衰减别名验证。
- 删除全部 init feature；Justfile 由 `FORMATION` 选择 `user/formations/<部署名>.toml`，initfs 映像集合由该组态引用的 `image` 与 `init/image.*` 决定，不再手列。

## 实施顺序

### 切面 A：内核级验收移出（独立先行）

1. 新建 `test_kernel`，搬入内核级检查：按领域分为 `ipc_checks`、`ipc_thread_checks`、`memory_checks`、`job_checks`、`tunnel_checks`、`rpc_checks`、`time_checks`、`once_checks`、`race_checks` 与 MMIO/IRQ/DMA 资源检查，测试本地 owner 为 `harness`，出生采用与保留模式子进程在 `startup`。
2. 现有 init 以现有 spawn 启动 `test_kernel`，按 feature 传模式字节。
3. 验证：`just check`、`just clippy`，现有 core/stress/memory/device 路线结果与切面前一致。

该传模式路径是登记的债务，见[债务](#债务)。

### 切面 B：统一契约与组态（一次耦合迁移）

按依赖自底向上，中间步骤不要求 QEMU 通过：

1. `shared/settings`、`shared/formation`（含编译器），host 单测覆盖编解码与全部静态校验负路径。
2. `libprocess::birth`，host 单测覆盖帧解码、重名、`finish()` 拒绝未取用输入。
3. `libservice::instance`：消息、撤销协议、子进程 helper、launcher；host 单测覆盖导出校验与撤销状态机。
4. srv_fs、srv_device（含 SourceAdmin 撤销协议与预限定导出）、srv_driver 迁移。
5. srv_init 执行器替换 `deployment.rs`、`device.rs`、`driver.rs`、`filesystem/`；平台输入去 libpci；Terminal 改用撤销协议。
6. 测试侧按 AGENTS“验证”中的过渡设施口径，只做保持必要可用的最小迁移：`test_kernel` 改由组态启动并从设定取模式；`test_device`、`test_fs` 只承接仍有证据价值的服务级验收，迁移代价高于价值的检查直接删除，并在本篇写明失去的覆盖。
7. `user/formations/` 写出 system 与各验收部署；Justfile 改为编译组态、按部署组装 initfs，`bin/` 改为 `image/`。

开发证据：步骤 1–3 的 host 单测；步骤 5 后 System 路线。

### 收口

按 [BUILD-AND-TEST](BUILD-AND-TEST.md) 选择受影响 package/分面的 check、clippy、host 测试及必要的组态/服务验收；不以全部历史路线为固定门。按 [REVIEW](REVIEW.md) 做交付前结构检查并写回本篇。回填 `notes/impls`（服务、init、device 相关篇），更新 KNOWN_ISSUES 矫正区与 COMPASS。

## 删除清单（切面 B 完成时）

- srv_init：`deployment.rs` 的 `instances`/`file_hosts`/`system` 与固定停止阶段；`supervision::Instance` 及 `Record` 的 `filesystem`、`device_closed`、`admin` 特化字段；`device.rs`、`driver.rs`、`filesystem/hosting.rs`、`filesystem/launch.rs`；服务级验收代码；全部 feature；对 `srv_fs`、`test_fs`、`test_device`、`libdevice`、`libdriver`、`libpci`、`libusb` 的依赖。
- srv_fs：`startup::Profile`、`PROVIDER_*`/四种根 kind 的旧报告。
- libdevice：`startup` 槽位常量与 `Delegation`。
- libdriver：`boot` 槽位常量、`ManagerReady`、`Declaration` 的 payload 编码（声明类型可保留为设定解析结果）。
- srv_driver：Stop RPC op1；`test_device` 中对应负探针。
- Justfile：按 feature 与 workload 选择 initfs 条目的分支。

## 债务

本节保留前一轮债务登记的历史位置和承接措辞。工作集成本推导仍然有效，其当前唯一活动登记、触发/删除门及单一预算 owner 见 [IF2 当前保留债务](#当前保留的工作集成本推导债务)；不再按原表“或”字选择两个并行 owner。

| 项 | 现状 | 目标 | 触发与删除条件 |
|---|---|---|---|
| 正式部署工作集无单一真值 | srv_fs、srv_device、srv_driver 各自以独立常量定义部署 Pool 工作集（`deployment::METADATA_BYTES`/`HEAP_PAGES`、device 的 8 MiB、`libdriver::boot::WORKER_POOL`），没有由内存领域给出的公共推导。测试侧的 WORKER_POOL 借用已随测试进程删除 | 内存领域提供单进程工作集的公共推导，正式服务的同类常量收敛为一个真值；切面 B 中额度改由组态 `pool` 字段声明，数值仍沿用这些常量 | 触发：出现须精确核定单进程工作集的需求（不可信创建域、紧凑部署或额度不足的实证）。由[内存重构](todo-2026-10-01-memory-redesign.md)或[内核内存预算](todo-2026-09-14-kernel-memory-budget.md)承接 |

## 完成门

1. 同一 init ELF（比对哈希）分别在 System 与各验收 initfs 下运行。
2. System：三宿主、按需驱动、常驻监督成立；正常 shutdown 按反拓扑序停止，经撤销门放行 device 停止。
3. 两个 srv_fs 实例由组态定义，取代 `Secondary` 枚举。
4. 组态负路径（host 单测）：缺失路径、重名、未声明导出、依赖环、超预算、rights 提升，均在副作用前拒绝。
5. 运行期负路径：导出缺失或超时、Ready 前退出、Stopped 缺失、Revoke 超时，均进系统终局且不遗忘已启动实例。
6. 删除清单全部完成；srv_init 的 Cargo 依赖只剩执行器所列。
7. 交付前结构检查写回本篇，impls 回填。

## 前一轮接力记录（历史）

切面 A、B 的实施与运行过程保留在下方历史记录。2026-11-01 用户决定只保留 host 自动测试，专用测试进程、组态和 runner 已由[设施清理](archived/todo-2026-11-01-host-only-validation.md)移除；原运行结果仍只对应当时版本，不构成当前可运行路线。

切面 B 已实施并提交。首次实跑暴露的生命周期输入转授阻塞已按[原条目](#首次运行阻塞2026-10-29)修复；后续 memory 与 services 独立通过，取得 task 出生/正常退出及三宿主 Ready、Stopped/DONE、撤销链与关机证据。其余普通组结果由[验收记录](archived/todo-2026-10-29-acceptance-groups.md#逐条运行与修复2026-10-29)拥有。下一步仅按实际需求补充未覆盖的组态负路径与专项，不恢复已删消费者或固定全量验收。

最终结构：

- **纯逻辑层**：`shared/settings`（设定编码）、`shared/formation`（组态模型、校验、拓扑序与 host 编译器；多个源按序深合并，后者覆盖前者，编译时另写出引用映像清单）、`libprocess::birth`（出生帧与 `Birth` 采用器，`take_list` 接受一组允许 kind）、`libprocess::image::Image::adopt_sealed`（按封存对象整页长度采用）、`libservice::instance`（报告线格式、`Reporter`、`STOP_SIGNALS`、撤销协议线格式，请求体含操作码与绝对期限）。
- **srv_fs**：设定 `registry`/`entries`/`publish` 取代 `Profile`；自建管理与 registry receiver；导出 `file.root.N`、`file.management.N`，registry 宿主另导出 `service.directory.N`、`service.authority.N`。
- **srv_device**：根来源自持不导出；设定 `sources` 预签发 `device.source.<名>` 与 `.admin`；`Family::Stop` 观察 `instance.stop`，停止时释放根与未发布来源的 sender 并封根；`request.rs` 第四支以请求方 admin context 服务 `Revoke`/`QueryRevoked`。`libdevice::startup` 已删。
- **srv_driver**：声明表由 `libdriver::boot::Catalog::from_settings` 解析（identity 为字符串，删 `image_bytes`、槽位常量与 `ManagerReady`）；映像为 `driver.image.<实现键>`；Stop RPC op1 删除，改为 `Family::Stop`；就绪经 `Reporter` 导出 `driver.control`/`driver.observer`。
- **srv_init**：`platform`（按 `erhino_shared::platform` 取根，不依赖 libpci）、`executor`（组态执行：具名 Pool/Job、实例 Pool 按 ELF runs+主栈+出生页+`work_pages`、出生帧、报告校验、`after`、反拓扑停止与撤销门）、`terminal`（封 services、对已收撤销管理端发 Revoke、封平台根、有界收集、冷重启）。全部 feature、领域依赖与旧装配/服务级验收已删。
- **组态与构建**：`user/formations/system.toml` 为正式部署源；Justfile 以 `ERHINO_FORMATION_SOURCES` 选源，`make_initfs` 编译组态并只装入其引用映像，根目录为 `formation` 与 `image/<组件>`。

失去的覆盖（按 AGENTS 过渡设施口径删除，不登记债务）：`test_fs`、`test_device` 与 init 内服务级验收（fs/registry/copy/secondary 生命周期、device/driver 自检、function-session、reuse/candidates/retry-race、manager/controller fault、reset-rejection、fs-stop-order）；init 根监督的 failure-isolation、dispatcher funding、startup capture、idle ownership 自检；test_kernel 的 SystemReset 负路径与 held Pool 跨进程报告比对。失效宿主观察器、旧锚点和分组重建由[验收清理](archived/todo-2026-10-29-acceptance-groups.md)统一承接；本篇不再要求恢复已删除消费者。

当前证据边界：正常出生与停止已有历史运行证据，多 fs 实例与运行期负路径仍未覆盖；随正式应用接入和具体问题取证。System/验收 init ELF 身份比对的旧完成门随验收部署删除而退出，不记为已通过。设施移除与覆盖变化见[清理记录](archived/todo-2026-11-01-host-only-validation.md)，自动验证按构建手册选择 host 边界。

### 接线与责任

- **init 侧**：`AcceptanceImages` 在任何子进程出生前一次核完验收归档（缺失或重复即拒绝）。切面 A 的 `kernel_acceptance::KernelAcceptance::run`（源码见当时提交 `413f384`；该模块在切面 B 已删） 用原 Preparation/spawn 与 root 账本启动 `test_kernel`，并由 init 原短任务路径启动 `test_fp`；两者均须 Dead/Completed/NONE。`finish` 以二者为输入执行 RootSupervisor 失败隔离自检（原 QuotaExceeded 首因、恰好一个健康成员、InvalidPolicy 后同一 Collector 恢复及隔离 Job 收集判据不变），再核主 Pool 页与 metadata 的 allocated/reserved/delegated 全零并关闭，核 held Pool 自报告后逐字不变；此后才准入 srv_device，平台根别名与设备使用由此串行。
- **nofd 降级**：判据集中在 `run_companion`，严格为 test_fp spawn `System(NotSupported)`、grants `Retained` 且 `cleanup_error == None`；此时以单进程 Collector 收集 test_kernel 并输出自检不适用原因。FP 映像仍必需，其余出生错误或非 Completed 均失败。memory 不运行 test_fp 与该自检。
- **test_kernel 侧**：按领域分模块（见[切面 A 第 1 步](#切面-a内核级验收移出独立先行)），`main` 只负责启动子进程与编排顺序，`startup` 负责出生采用与保留模式子进程，[`harness`](../user/tests/test_kernel/src/harness.rs) 持能力与子进程 owner。它只持 services 下独立验收 Job 的子树，不持根 Job；根拓扑遍历、startup capture、idle ownership、Dispatcher funding 与 RootSupervisor 自检留 init。pm 收到 `libprocess::DERIVED_CONTROL_RIGHTS` 的委托域 JobControl。辅助均在测试包内，不新增公共库 API。
- **held Pool**：RootSupervisor 持有通用 held Pool 集合（`hold_pool`），从根 Pool 派生并持有至系统结束，不随实例收集关闭；对应组态的 `[pool.<名>]`。retention Pool 是其唯一当前使用者。
- **保留 fixture**：DMA exposed/output-fault 的父 fixture 与目标全部在 test_kernel 以保留角色自启动的子进程中执行，出生绑定 retention Pool。test_kernel 在最后一个保留子进程收集后核残留页数恰为 `RETAINED_PAGES_PER_FIXTURE` 之和，再经私有 sender 报告 104 字节全快照；init 只持通用判据，不认识 fixture 细节。metadata 残留记实测，不设任意上界。
- **memory 停止**：`deployment::instances()` 对 memory 为空集合，`stop_providers` 只停止声明集合中的 Driver/Device，声明了却缺记录报原错误；实例监督运行体建立过才退休。该声明集合在切面 B 由组态替换，不另登记债务。此修正针对 `bfb4fdd` 起即存在的缺陷。

### 工作集来源与每部署份数

每份为原 [`libdriver::boot::WORKER_POOL`](../user/libraries/libdriver/src/boot.rs) 的 4096 页与 1 MiB metadata；份数按场景同时存活峰值取 max，不全部累加，由 `kernel_acceptance::kernel_pool_budget` / `held_pool_budget` 唯一计算。无公共单进程工作集推导的限制由[债务](#债务)承接。主域并存依据：[`main::launch_children`/`run`](../user/tests/test_kernel/src/main.rs)、[`once_checks::run`](../user/tests/test_kernel/src/once_checks.rs)、[`race_checks::race_matrix`](../user/tests/test_kernel/src/race_checks.rs) 及资源检查的 `process_retirement`；MMIO/IRQ/DMA 目标子域付款在 `mmio_checks.rs:198`、`irq_checks.rs:197`、`dma_checks.rs:292`。

| 部署/recipe 形态 | 主 Pool 份数 | 同时存活场景/依据 | retention |
|---|---:|---|---|
| core / acceptance-virt、virt-nofd | 4 | 普通阶段主+pm+目标为 3；Once 阶段主+pm+独立 2 MiB metadata 域为 4，取 max | 2 份，两项各保留 2 页 |
| device / device-reuse | 4 | 内核检查同 core；设备消费者在主域收集后才准入，不与主测试域共存 | 同 core |
| stress / virt-stress | 5 | 主+pm+两 hammer+1 并发目标；Once 仍 4，取 max | 同 core |
| memory / virt-vm-fault | 5 | 借用 stress 组合：主+1 hammer 是 stress 并存集合的子集，执行同一 raw storm（1023 个 raw 线程并存、第 1025 次准入 ReachLimit，见 test_hammer:737–764、proc.rs:71）；按进程计两份漏掉线程成本 | 无 |
| mmio-publication / mmio-forced | 4 | 同 core；退出专项逐个派生一份 WORKER_POOL 给目标，不与 Once 域并存 | 同 core |
| dma-races / dma-shared / dma-quota | 4 | 同 core；目标逐个派生一份 WORKER_POOL，只有当前目标子域并存 | 同 core |
| dma-install-failure | 4 | 同 core；单一安装失败目标一份 WORKER_POOL，无保留模式 | 无 |
| irq-exit | 4 | 同 core；两个变体顺序执行，各派生一份 WORKER_POOL | 同 core |
| irq-retirement | 4 | 内核检查同 core；真实 function/IRQ 场景留 init 服务阶段 | 同 core |

主 Pool 除 READ/CREATE/DUPLICATE/GRANT 外需 TRANSIT：race 的 ACTION_CREATE 经消息把付款授权交给 hammer。held Pool 别名不含 TRANSIT。工作集不足一律报告为模型错误，不自动扩额。

### Pool 断言迁移口径

| 检查 | 迁移后的基线与判据 |
|---|---|
| test_process_funding | 原本派生独立 16 页/1 MiB Pool，现以测试 Pool 为父；WrongObjectType、缺 CREATE 拒绝、Map 后 allocated>baseline、abandon 后等于 baseline 均保留 |
| test_pool_budget | 双维回滚、无隐式 quota、父关闭、完整退款与活 View 付款链保留；起点 depth 多 1，链短 1，但仍到同一 MAX_DEPTH 并核 ReachLimit |
| memory mapping | 非 stress 对照测试 Pool 的 allocated 前后相等；stress 不查询的原条件保留 |
| ipc_thread_checks committed_kill | 对照测试 Pool 前后页数相等，原条件不变 |
| tunnel close | 非 stress 核关闭后 allocated 下降且退款 >0；stress 不查询 |

没有必须限定 depth 1 根 Pool 才成立的原断言，没有删除退款/拒绝检查。两处 sender PID 自检对照 test_kernel 自身 pid，子进程 SpawnRequest.parent 为其真实直接启动者。

### fixture 收集后寿命核查

| fixture | 主域外仍活计费/依据 |
|---|---|
| DMA exposed / output-fault | 明确保留，位于 retention；Mapping、Backing/MemoryObjectCore、WritePermit、域节点与保留表，合计 4 页 |
| DMA unexposed | 最后 Close 取消、scope DONE、writer 0/Executable，主 Pool 最终双维零 |
| dma-races / shared / quota / install-failure | 每个目标 Pool 在 CPU/Control/父对象关闭后全零；shared 另核父 RAM 精确退款 |
| mmio-publication / forced | 逐目标核范围失效、scope DONE、Control 关闭及独立 Pool 零；MMIO 映射不占 RAM backing |
| irq-exit | 目标 Control 关闭、独立 Pool 零、use/Line 贷款与来源退休后 DONE |
| irq-retirement | function_checks 仍在 init 服务域，不持主测试 Pool 计费 |
| race / memory suite | 目标/hammer 与 Job 退出后收集；主 Pool 双维零作为组合门 |

### A-F4 最终验证

冻结点 A-F4：541 个产品/工具输入（`user/`、`os/`、`shared/`、`tools/`、Justfile、rust-toolchain），SHA-256 `3815d17e48bcf484a2ba2df13ff487a9e546e271e950c012f7aeea1e649bc07a`；每条 recipe 前后核对，全部相等。`just check`、十一分面 `just clippy` 与 rustfmt 检查均返回 0。结果与运行时刻见 `artifacts/init-formation-a/final-F4/results.json`，下列链接为各 recipe 的完整构建/验收日志。

| just recipe | workload / 构建 | srv_init feature | rc | 日志 | 说明 |
|---|---|---|---:|---|---|
| acceptance-virt | core / release | — | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-virt.log) |  |
| virt-stress | stress / debug | acceptance-stress | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-stress.log) |  |
| virt-vm-fault | memory / debug | acceptance-memory → acceptance-stress | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-vm-fault.log) |  |
| acceptance-device | device / release | acceptance-device | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device.log) |  |
| acceptance-device-reuse | device-reuse / debug | acceptance-device-reuse | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-reuse.log) |  |
| acceptance-device-candidates | device-reuse / debug | acceptance-device-candidates | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-candidates.log) |  |
| acceptance-device-function-session | core / debug | acceptance-device-function-session | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-function-session.log) |  |
| acceptance-fs-stop-order | core / debug | — | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-fs-stop-order.log) |  |
| acceptance-mmio-publication | core / debug | acceptance-mmio-publication | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-mmio-publication.log) |  |
| acceptance-mmio-forced | core / debug | acceptance-mmio-forced | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-mmio-forced.log) |  |
| acceptance-dma-races | core / debug | acceptance-dma-races | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-races.log) |  |
| acceptance-dma-shared | core / debug | acceptance-dma-shared | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-shared.log) |  |
| acceptance-dma-quota | core / debug | acceptance-dma-quota | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-quota.log) |  |
| acceptance-dma-install-failure | core / debug | acceptance-dma-install-failure | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-install-failure.log) |  |
| acceptance-irq-exit | core / debug | acceptance-irq-exit | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-irq-exit.log) |  |
| acceptance-irq-retirement | core / debug | acceptance-irq-retirement → acceptance-device-function-session | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-irq-retirement.log) |  |
| virt-nofd | core / release，无 D64 域 | — | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-nofd.log) |  |
| system-observe | system / debug | system | 0 | [日志](../artifacts/init-formation-a/final-F4/system-observe.log) |  |
| acceptance-device-retry-race | device-reuse / debug | acceptance-device-retry-race | 124 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-retry-race.log) | 既有观察器缺陷，已[移交设备计划](todo-2026-09-device-system-design.md#retry-观察器缺陷2026-10-29)，不计通过；同样失败于 retry.log:12 读取 `self`，外层 617 行 `Retry late-query window not established` |

通过路线的判据由 Guest 内硬失败保证：主 Pool 未归零、held Pool 报告后变化或保留页数不符，均使 test_kernel panic 或 init 进系统终局，wrapper 据此判失败；wrapper 在成功时删除原始 Guest 输出，因此 A-F4 不保留逐路线采样值，A-F3 的采样（保留域 4 页/884 槽）保存在 `final-F3/`。stress 16/16 矩阵、memory 1024 成员 raw storm 与真实 RetryFault、nofd 不适用锚点、System 三宿主常驻等待均由各自 wrapper 锚点核验。

### 交付前结构检查

按 [REVIEW](REVIEW.md) 核对最终结构，没有待修 finding：

- **责任/授权**：真实 sender/parent 身份、独立验收 Job、root 账本与终局对失败/取消的接管保持；test_kernel 收集与主 Pool 关闭先于 srv_device 准入；held Pool 由 root 持有至系统结束，未以遗漏退款保活主域。
- **知识归属**：init 不认识 DMA fixture 细节，只持通用判据；fixture 期望页数由 test_kernel 自核。部署模式与保留子进程角色分为两个字段，不共用位空间。pm 委托域权利引用 `libprocess::DERIVED_CONTROL_RIGHTS`，不复制。`SUPERVISOR_RIGHTS`/`JOB_FULL_RIGHTS` 在公共库中没有同用途常量，两侧各自定义；pm 与 test_kernel 的 `WRITABLE_WAKE_*` 是切面前即有的双端约定，srv_pm 不应为此依赖测试 crate，保持现状。
- **收口/简化**：srv_init 的八个旧模块、`kernel_checks`、`LaunchedServices`、`RunFailure`、验收专用 `retention_pool` 字段已删除；test_kernel 去掉 init 时代命名（`Checks`、`root`、`root_pool_allocated`、`kill_and_supervise`、`RunFailure`）与纯转发方法。init 的 ReadWorld/ReadDuty 经核没有只服务已删路径的字段。
- **安全/并发**：新增 unsafe 的采用/移交及只读映像存活依据成立；保留子进程 spawn 前释放 Context 锁；不新增跨进程锁或内核临界区；未改共享 ABI 或硬件协议。
- **工具**：GDB 观察器按 test_kernel 真实 PID 与 `test_kernel::irq_checks::process_retirement` 符号附着；`check-vm-fault` 在 Guest 明确 panic/reboot 后停止；nofd 脚本期望不适用锚点。业务判据未改。

未做硬件或性能/WCET 验收；sifive_u、hetero、Boot/IPI/VM-repair/独立 DMA-retention 等未列路线未运行。当时的私有出生帧已由切面 B 替换，测试侧 WORKER_POOL 借用随后随客体设施删除；正式工作集推导仍由[债务](#债务)承接。

### 施工过程索引

过程证据保留在 `artifacts/init-formation-a/`，均不计交验：

- **付款寿命**：首轮 core 暴露保留 DMA 把主池整份委托保活，test_fs QuotaExceeded（`artifacts/failed-acceptance-20261029-082357-44659.log`）；据此建立 held/retention Pool 与保留子进程，设计修订见[已核实事实](#已核实事实)「Pool 委托寿命」。
- **工作集口径**：撤回逐项证明内存内部布局与单 64 KiB arena 的要求，额度改为 WORKER_POOL 组合；memory 两份在 raw storm 前 QuotaExceeded（`artifacts/vm-fault/20261029T005457.159966Z-78248/fault-qemu.log:352`），改借 stress 五份。
- **memory 停止缺陷**：A-F2 暴露 `driver assembly record missing`（`artifacts/failed-acceptance-20261029-090925-95909.log`），按声明集合修复。
- **工具定稿**：运行中修改 wrapper 导致一次 stress 外层 rc2，此后规定冻结期间不改 tools/Justfile/产品代码；同期平台时钟异常复发按 [KNOWN_ISSUES](../KNOWN_ISSUES.md#历史偶发异常仅观察) 记入原设备记录。
- **A-F3**：整理前的功能冻结点，19 条路线结论与 A-F4 一致，证据在 `final-F3/`；随提交前整理作废，由 A-F4 取代。
- **Retry 归因**：00274bf 干净基线同样在观察器读取无 `self` 的 `commit_retry` 帧时失败，ELF 差异来自 ignored lockfile 重新解析与路径等环境差异；证据在 `retry-attribution/`，已[移交设备计划](todo-2026-09-device-system-design.md#retry-观察器缺陷2026-10-29)。

## 首次运行阻塞（2026-10-29）

运行入口与脚本修复证据由[验收分组记录](archived/todo-2026-10-29-acceptance-groups.md#首次运行取证2026-10-29)拥有。正式代码基线 `2314f40`，本轮未改产品代码。memory/basic 的原始日志 `artifacts/acceptance/20261029T094343.227369Z-91322/guest.log` 显示：init 开始准备实例 kernel 后打印 `instance preparation failed: kernel, System(RightsDenied)`，随即进入系统失败终局、封 services/平台根并提交 SystemFailure Reboot；尚无 test_kernel Start。runner 判失败并清理，无超时或清理错误。

静态核实的确定性权限矛盾：

- `srv_init::executor::start` 以 `REPORT_RIGHTS` 铸造报告 sender，以 `STOP_RIGHTS` 创建停止 owner，然后将原 handle 作为出生 grants 的前两项。前者为 WRITE/WAIT/TRANSIT，后者为 READ/WAIT，均无 GRANT；内核 mint/create 按传入权利铸造。
- `Preparation` 的 Grant 阶段经 `ProcessGrant` 转授；`os/kernel/src/task/process.rs` 逐项 `Invocation::claim(..., Rights::GRANT)` 后才验证目标权利子集并提交移交。因此该路径到 Grant 阶段必在第一项 report 被拒绝，stop 同样不满足；普通组态输入则已按 `rights | GRANT` 创建别名。
- `SUPERVISOR_RIGHTS` 属于 Control 允许集合；Job 与 Pool 均有 CREATE，Builder 铸造包含 MANAGE。这些调用链未发现同类声明矛盾。运行日志只给通用准备错误，没有阶段字段，不能将上述静态定位表述为运行断点已确认的失败阶段。

责任 owner 为本计划的 srv_init 出生装配。用户后续授权修复并逐条运行：report sender 与 stop owner 的源权利补 GRANT，HandleGrant 目标仍为现有 REPORT_RIGHTS/STOP_RIGHTS；失败时 owners 自动关闭，Consumed 后仍按原代码移交唯一 owner，不放宽内核检查。

修复后 `just check srv_init`、`just clippy srv_init` 通过，日志分别为 `artifacts/checks/20261029T095211.461840Z-92539/srv_init.log`、`20261029T095211.960153Z-92578/srv_init.log`。独立运行 memory/basic 取得 PID 2 Start、领域完成、正常 DONE、全部收集和 Requested Shutdown；随后 services/basic 的 device/driver/fs 全部 Ready，再按 fs→driver→device 正常 Stopped/DONE 后关机。该出生阻塞已关闭，逐条运行结果由[验收记录](archived/todo-2026-10-29-acceptance-groups.md#逐条运行与修复2026-10-29)拥有；这些正路径不替代组态完成门中的负路径证据。

交付前结构核查：改动只增加源能力的转授权，HandleGrant 目标权限、所有权移交、Consumed 处理、Preparation 失败收束和内核检查均保持；未新增 unsafe、锁或回调，impls 已回填。任务/常驻两类真实消费者验证通过，不为两处权限表达式补镜像单测。本次产品修复已提交为 `fe0ed70`；[可选独立 Review](todo-2026-10-29-init-grant-fix-review.md)已登记，未执行。
