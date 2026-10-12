# 正式服务装配与监督设计 Review

## 审查结论

本次是第三批设计 Review，审查对象为
`plans/todo-2026-10-07-service-assembly.md` 当前正式服务装配与监督设计，
以及其直接引用的启动、任务、框架、服务目录、关机和设备接点。
未修改生产代码、设计计划、COMPASS、Review 收口计划或其他报告。

结论：**暂不能进入服务装配施工。**

当前计划已经正确划分了本案与服务目录、关机编排、设备体系的边界，也明确拒绝
恢复公开 Starting/Ready/Draining 两阶段注册。但是，正式装配的核心责任链仍停留在
“待闭合”的门槛列表：首批正式长期服务、就绪证明、Pool/执行付款归属、服务供应、
重发布 authority、失败升级和停止交接尚未形成唯一可实现的契约。

代码和实现说明可以复原一套**验收剧本**，不能复原一套正式服务编排契约：
`srv_init` 通过 `RequiredLaunchSet` 记录映像出现和进程 spawn，`srv_fs` 有独立的
provider-ready 消息，`srv_pm` 没有同类服务就绪报告，驱动当前只有启动/日志事实；
随后 init 驱动文件服务消费者、Job 收束和一次 `SystemReset`。这些事实不能直接提升为
服务健康、设备静默或通用恢复协议。

## Findings

### SA-01 / P1：首批正式服务、依赖和就绪证明没有形成可施工矩阵

**性质：设计缺口，部分由当前实现证据暴露。**

**位置 / 章节**

- `plans/todo-2026-10-07-service-assembly.md`「设计门」第 1 项、
  「完成门与验证边界」。
- `notes/impls/startup.md`「init/pm 当前政策」。
- `user/services/srv_init/src/main.rs:308-317,378-398,495-628`。
- `user/services/srv_fs/src/server/boot.rs:762-773`。
- `user/services/srv_pm/src/main.rs:879-997`。

**可达或可推导路径**

1. `srv_init::launch_test_services` 用 `RequiredLaunchSet` 检查 initfs 映像是否存在，
   并在 `spawn` 返回成功后调用 `mark_started`。
2. `srv_fs` 之后通过 bootstrap mailbox 发送 `PROVIDER_READY_KIND`；该 ready 只在
   文件根、目录、authority、management 和（secondary 时）Register 控制都已建立后发送。
3. `srv_pm` 启动后直接建立 Runtime 和 mailbox task，未向 init 报告“业务可服务”的独立
   ready；其 JobDriver 只在委托域收束时使用。`drv_spi_sifive` 在当前范围也没有可核验的
   服务就绪/硬件可用报告。
4. `REQUIRED_STARTED` 包含 `REQUIRED_PM_TARGET`，但不包含 `REQUIRED_FS_CONSUMER`；
   `test_fs` 作为映像被记录后由后续剧本单独 spawn。该集合因此表达的是当前验收拓扑，
   不是首批正式服务的启动依赖和可观察能力集合。
5. 计划没有列出每个正式服务的：服务身份、业务入口、必须依赖、可选依赖、就绪谓词、
   发布完成条件、健康丧失条件和对应的监督动作。也没有说明验收进程、一次性 target、
   `test_hammer` 是否完全排除在正式装配之外。

**违反的当前设计原则或契约**

- 本案设计门要求从真实 `srv_init`、`srv_fs`、`srv_pm` 消费者选择首批正式长期服务，
  并明确必需/可选能力和就绪判断。
- 服务目录的 `Register` 只是发布者声明业务已可服务；目录不替装配者判断健康，
  也不维护 Starting/Ready/Draining 真值。
- 进程创建成功、Job 可枚举或一次 workload 完成，均不是业务入口可用或设备安全静默
  的证明。

**影响**

施工者无法确定应启动哪些正式服务、何时向消费者交付入口以及哪个失败应使 stage
失败、降级为可选能力或升级为系统停止。若直接把现有 `mark_started` 或
`PROVIDER_READY_KIND` 当作通用就绪协议，会把 spawn/初始化完成误报为服务健康；若
把一次 `test_fs` 报告或 pm-domain 收束当作服务健康，则正式服务能力没有独立完成门。
这也使后续重启和验证矩阵无法确定观察对象。

**建议归属**

由本案计划 owner 闭合。新增一张首批正式服务矩阵，至少覆盖 `srv_fs` 文件 provider、
服务目录 provider、`srv_pm`、首批设备/驱动接点和真实长期消费者；逐项写明依赖 DAG、
交付入口、必选/可选、就绪谓词、发布完成点、健康丧失、失败升级和验证证据。
验收 target、一次性 workload 和 `test_hammer` 只能作为验证消费者，除非本案明确把
它们纳入正式服务集合。

### SA-02 / P1：进程、Job、Pool、执行付款、授权交付和服务供应的 owner 图未闭合

**性质：设计缺口，当前实现仅能证明验收剧本的局部交接。**

**位置 / 章节**

- `plans/todo-2026-10-07-service-assembly.md`「设计门」第 2、4 项。
- `notes/impls/startup.md`「init/pm 当前政策」。
- `user/services/srv_init/src/main.rs:339-454,469-494,654-740`。
- `user/services/srv_pm/src/main.rs:879-925`。
- `user/services/srv_fs/src/server/boot.rs:250-457`。

**可达或可推导路径**

1. init 创建 `services` 和 `pm_domain` Job，保留 `pm_domain` 的直接收束副本，
   同时把另一份 JobControl 交给 pm；pm 只拥有委托域控制，异常退出由 init 监督。
2. `srv_fs` 接收 bootstrap、release、route、registration 和 MemoryPool grant；
   init 还为 secondary 创建 publication inbox，并把 authority 通过消息再次交给 provider。
3. `srv_fs` 自己建立文件/服务 backing、Execution account、Runtime、来源和发布控制；
   `srv_pm` 则在进程内部创建自己的 `Budget`/`AccountView`，启动时没有与 init 的服务
   付款账本对应的显式交接。
4. 计划只要求“分开画进程/Job 管理、付款 Pool、授权交付与服务供应关系”，没有给出
   这些关系的方向图、付款来源、运行时额度与 kernel MemoryPool 的边界，或服务死亡后
   谁仍持有重启所需的 authority。

**违反的当前设计原则或契约**

- 机制、授权与政策必须分离，且每项资源和操作有唯一 owner/authority。
- 已提交责任不能因调用者、上级 alias 或服务进程消散而丢失。
- 付款 Pool、执行预算、业务入口和监督 control 不能以“都由 init 创建”或“都由服务
  自己持有”模糊替代；每次交接必须有明确的责任转换点。

**影响**

现有交接可以支撑固定验收顺序，却不能据此推导正式服务的单向 authority 图。特别是：
pm 的 Job 收束权、init 的直接兜底权、fs 的服务目录/发布 authority、provider 的
业务入口和各自付款账户之间没有规定谁能重建谁、谁能撤销谁、谁为重启期间的旧入口/
清理工作付款。没有该图，容易形成 supervisor 依赖 provider 提供监督入口、provider
又依赖 supervisor 重新发布的循环，或在服务死亡后只剩 ProcessControl 而没有供应/
发布 authority。

**建议归属**

由本案 owner 闭合 authority/付款/供应三张相互对应的图：Process/Job control、
kernel MemoryPool、Execution/Runtime account、服务访问入口、发布 authority、注册
control、供应资源和监督恢复承载分别列 owner、authority、payer、观察者和接管者。
明确 init、pm、fs provider、服务目录 provider 之间只允许的单向交接，并写明父
authority 消散、子 capability 已交付、服务进程死亡和恢复进程重建时的责任保持方式。
不在本案复制 `libservice` 的注册业务语义；只定义装配侧使用这些能力的责任转换。

### SA-03 / P1：Register 未知结果、旧注册退休和重新发布没有唯一恢复责任

**性质：设计缺口。服务目录已有合同，本案尚未把它接成恢复政策。**

**位置 / 章节**

- `plans/todo-2026-10-07-service-assembly.md`「装配接点」、
  「设计门」第 3 项。
- `plans/todo-2026-10-07-fal-service-redesign.md`「注册提交与完成状态」
  （尤其 Register 成功回复延迟、回复丢失和未知发布结果）。
- `user/services/srv_fs/src/server/boot.rs:640-773`。

**可达或可推导路径**

1. provider 在业务根和目录入口准备后异步提交 `Register`；注册成功后才向 init
   发送 provider ready。
2. FAL/service 合同允许 Register 已提交但回复丢失，且明确不自动重试、不按名字删除、
   不恢复替换前注册；Replace 后新注册控制消散还可能使名字为空。
3. 当前装配计划只点名“Replace 已提交但回复丢失”的未知结果，没有定义首次
   CreateOnly、provider 在 Register 后立即死亡、控制未交付、旧 endpoint 已发布但
   新进程准备完成、或旧注册退休尚未完成时的装配状态。
4. `srv_init` 仍以 provider ready 消息和后续 `GetEntry`/Watch 观察为主；计划没有规定
   监督者持有什么不可消散的恢复 authority，如何知道旧注册已经 Withdrawn/不可再用，
   以及重新发布成功前消费者如何处理旧 endpoint。

**违反的当前设计原则或契约**

- 服务目录只拥有 Register/GetEntry/Withdraw/QueryRegistration 的发布发现契约；
  本案拥有启动、就绪、恢复和业务 drain，不能把恢复责任留在目录协议的隐含状态中。
- 已发布 endpoint、旧注册、回复丢失和未知结果必须分别有 owner；不能通过重试名字、
  恢复旧注册或自动重绑来猜测提交结果。
- 重启不得复活失效 endpoint 或重复授予权力。

**影响**

服务死亡或启动超时后，系统可能只有一个已 Dead 的 ProcessControl 和一个未知的目录
名字状态，无法判断应等待旧注册退休、重新创建进程、重新取得发布 authority，还是升级
为系统失败。消费者可能继续持有旧入口而被错误地视为已重连，或者新进程在旧注册尚未
消失时错误使用 Replace。该缺口直接阻断“服务死亡、重新发布、重连”施工。

**建议归属**

由本案 owner 定义装配恢复状态和责任转换，不改写服务目录的 Active/Withdrawn 两态：
`Spawned/ReadyPending`、`Published`、`FailurePending`、`RetireOld`、`Restarting`、
`Degraded` 等只能是 supervisor/provider 的内部责任记录，不作为目录公开状态。
对每个 Register 结果明确“已确认提交 / 已确认拒绝 / 未知”的持有人、Query/Watch
可用性、控制消散后的下一步、旧 endpoint 的失效证明和新发布的授权来源；明确
消费者重连只由其自身 WatchName→GetEntry→本地绑定流程触发，不自动重绑已有 Namespace。

### SA-04 / P1：失败、就绪超时、服务死亡与停止没有统一的有界状态机和升级边界

**性质：设计缺口。**

**位置 / 章节**

- `plans/todo-2026-10-07-service-assembly.md`「设计门」第 3、4 项、
  「完成门与验证边界」。
- `plans/todo-2026-09-system-shutdown-orchestration.md`第 8-22 行。
- `plans/todo-2026-09-device-system-design.md`第 15-21、38-49、76-82 行。
- 当前验收路径：`user/services/srv_init/src/main.rs:1311-1343,1959-1995`、
  `user/services/srv_init/src/supervisor.rs:1845-1905`。

**可达或可推导路径**

1. 当前 init 在验收失败时把服务 Job、进程、RPC/endpoint cleanup 交给
   `RootSupervisor`，然后尝试 `SystemReset(SystemFailure)`；正常路线在一次 workload
   后通知 fs provider 停止、等待消费者 DONE、观察 provider 关闭，再提交
   `SystemReset(Shutdown, Requested)`。
2. 这条路径能表达当前测试剧本的收束顺序，但没有通用的服务停止请求、业务 drain、
   endpoint 撤出、已提交请求收束、服务进程退出、供应释放和升级条件。
3. 关机计划明确 `SystemReset` 不通知服务、不等待资源，并且通用优雅关机状态机尚未
   设计；设备计划明确进程 DONE、驱动报告或维护批次完成都不能证明 IRQ/DMA/硬件动作
   已静默。
4. 服务装配计划只说“确定服务停止入口及与监督的交接”，没有定义启动失败、就绪超时、
   服务死亡、重连失败、重复重启、监督者失联、停止超时和 `SystemReset` 被拒绝时的
   责任链和有限重试/升级门。

**违反的当前设计原则或契约**

- 正常、失败、取消、退出、超时、恢复和停止必须使用同一责任链，并有结构性上界。
- 进程 DONE 只证明进程对象生命周期；Job Dead 只证明 Job 收束；普通 workload 成功
  只证明该 workload。它们不能代替业务健康、服务 drain 或设备安全静默。
- 系统关机状态机由关机计划拥有，设备安全停止由设备计划及固定硬件契约拥有；本案
  必须给出交接点而不能以其中任一局部事实冒充全局完成。

**影响**

施工无法选择监督者的内部阶段、等待对象和超时后的唯一升级路径。最危险的错误是：
收到 Process DONE 就释放服务/设备资源，或收到 `PROVIDER_STOPPED` 就直接认为设备和
外部事务安全；相反，若服务停止责任只依赖调用者继续发送 release/withdraw，也会在
调用者死亡时遗失已提交清理。`SystemReset` 失败时当前 init 只是继续 idle/监督，未形成
正式系统失败的可观察终态。

**建议归属**

本案先定义服务级停止接口和 supervisor 交接：封闭新请求、停止发布、业务 drain、
撤出/观察已发布入口、等待服务报告和进程终态的顺序，以及每一步的 owner、deadline、
重试和升级结果。关机计划随后定义系统范围聚合条件和最终 reset 许可；设备计划定义
驱动/供应/IRQ/DMA 静默证明。本案只消费这些证明，不能自行定义硬件静默或全局关机
状态。

### SA-05 / P1：施工准入所需的接口、责任转换、删除清单和验证门尚未落地

**性质：设计缺口，伴随证据缺口。**

**位置 / 章节**

- `plans/todo-2026-10-07-service-assembly.md`「设计门」第 4、5 项、
  「完成门与验证边界」。
- 当前计划全文没有首批服务部署图、责任转换表、迁移删除表或服务装配验证矩阵。
- `notes/impls/startup.md`第 69-99 行仅记录当前验收/实现事实，不是本案目标契约。

**可达或可推导路径**

1. 计划要求在施工前形成“首批装配的接口、责任转换、迁移删除清单与验证门”，
   但当前正文只有待论证问题和服务目录接点，没有给出这些交付物。
2. 现有实现中，启动 grant 的索引语义、bootstrap mailbox 的消息种类、provider-ready/
   provider-stopped、pm mailbox、JobControl、service Register control 和消费者
   report 各自存在，但没有被提升为一份冻结的跨组件接口清单。
3. 当前实现还包含验收专用路径：`test_target`、`acceptance`、`test_fs`、pm-domain
   target、一次 shutdown。计划没有列出哪些是正式路径、哪些必须在正式装配施工时删除或
   保留为验证消费者，也没有列出旧 launch/监督路径的删除条件。
4. 计划因此无法给出最小验证集合：启动缺失、ready timeout、Register unknown、旧
   endpoint、provider death、consumer reconnect、stop timeout、supervisor failure
   和设备静默各应观察什么终态。

**违反的当前设计原则或契约**

- 进入施工前，当前承诺范围的接口、owner、责任转换、旧路径删除条件和验证门必须
  唯一、可据以实施。
- 局部代码存在、静态 lint 或一次验收 workload 不能替代跨组件责任链和正式消费者接通。
- 未登记的过渡结构不能进入下一阶段。

**影响**

不同实施者会自行决定 supervisor 是否为新进程、ready 消息是否成为正式协议、Pool
是否由 init 或服务持有、旧 registration 如何清理以及失败是否重启。即使代码可以
编译，也无法判断是否完成本案承诺，容易在验收路径上保留第二份服务健康真值或把
测试专用收束误当正式监督。

**建议归属**

由本案 owner 补齐四份施工输入后再开工：

1. 首批正式服务部署/依赖/能力矩阵；
2. 每个交接点的接口与责任转换表（含付款、authority、未知结果和消散后 owner）；
3. 现有 `srv_init` 验收装配到正式装配的迁移与删除清单；
4. 按正常、失败、取消、超时、死亡、重连、重新发布、停止和升级组织的最小验证矩阵。

其中服务目录协议、对象链/执行/RPC/运输和全局关机/设备细节只引用其 owner 的契约，
不在本案复制或重新 Review。

## 当前事实与证据边界

- `notes/impls/startup.md` 描述的 `RequiredLaunchSet`、RootSupervisor、pm-domain、
  fs 双 provider 和验收监督是真实当前实现/说明事实；本报告没有把它们改写成未来
  正式服务设计。
- `srv_fs` 已有 provider-ready/provider-stopped 和单步 Register 的实际接点；服务目录
  的 Active/Withdrawn、未知发布结果、不自动恢复旧注册等合同属于
  `plans/todo-2026-10-07-fal-service-redesign.md`，本报告只检查装配如何消费它们。
- `srv_pm` 当前拥有委托 JobControl，并在进程内用 JobDriver 收束委托域；init 保留直接
  收束权作为验收兜底。该现状不等于正式长期服务的供应、健康和恢复 owner 已经确定。
- `SystemReset` 的对象能力、基础 Process/Job/Runtime/RPC/SourceRegistration 责任链及
  其既有 Review finding 不在本报告重复。若本案施工消费到这些 finding，应引用其唯一
  报告的处置状态。
- 本次未运行构建、host 测试或 QEMU；这是设计 Review，代码事实来自源码和当前实现说明。

## 排除项与交界

- 不审 FAL/service Register/GetEntry/Withdraw/Watch 的业务协议自身；SA-03 只审装配侧
  对未知结果、控制寿命和重发布的责任接入，不复制服务目录 Review 的 finding。
- 不审对象链、Process/Job、执行、RPC、运输和时间固定提交的基础正确性；SA-02/SA-04
  只指出正式服务如何消费这些机制的责任缺口。
- 不审全系统关机状态机；SA-04 只指出服务停止入口与关机/设备计划的交界未定义。
- 不审设备 MMIO/IRQ/DMA 细节；SA-01/SA-04 只要求服务矩阵引用设备计划提供的静默/
  供应证明，不把进程终态当作硬件证明。

## 最终处置建议

在 SA-01 至 SA-05 关闭前，不应开始本案代码施工，也不应以新增 supervisor 进程、
manifest、两阶段注册或测试专用运行体作为临时补全。关闭条件是：首批正式服务和真实
消费者可复原；authority/Pool/供应/监督图无循环且每次责任转换有 owner；Register
未知结果、旧注册退休、重连和重发布有单一恢复政策；服务停止与关机/设备证明的交界
明确；接口、删除和验证矩阵足以让施工者无需临场作架构决定。
