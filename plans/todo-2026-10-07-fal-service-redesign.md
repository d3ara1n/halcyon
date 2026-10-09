# OAL、fs 与服务目录改造计划

状态：**本轮范围的设计收口完成，D0 已关闭；P1—P4 代码迁移与实现验收未开始。** 本篇是唯一活跃工作记录。目标、协议、生命周期、迁移和验收均由本篇拥有；设计审查的原始发现、修订及复核见[工作记录](#工作记录与当前证据)，下一步见[当前接力](#当前接力)。设计完成不等于代码能力已经交付。

历史讨论独立保存在[第一至第七轮](archived/ref-2026-10-07-oal-fs-service-design-rounds.md)与[第八至第九轮](archived/ref-2026-10-09-oal-fs-service-rounds-8-9.md)。其中被替代的初稿、状态与“下一步”不参与本篇目标解释。外部系统事实见[文件操作取证](ref-2026-10-09-cross-provider-file-operations.md)。

## 交付目标与范围

把现有 libfal/libfs/libservice 中混合的发现、文件和服务协议职责拆开，交付 liboal 发现协议、libfs 文件协议及 libservice 注册取得协议，接通现有 provider、装配与验收消费者。所有领域数据、状态、实际路径遍历与操作由具体 provider/backing 实例持有和执行；协议库不在 backing 之上另建状态层或执行层。

| 范围 | 本案承诺 |
|---|---|
| OAL | 用户态树状视图的声明、provider、Directory、目录发现与入口传递、Lookup/Enumerate、ObjectInfo、Trait/tag 表达及 Boundary 接续；不解释领域对象或承载领域操作 |
| fs | 内存文件 provider 的目录/Property/Stream、授权、小 I/O、流完成、名字变更、订阅和 Copy |
| 服务目录 | 名字发布授权、注册/发布/替换/撤出、GetEntry、名字/目录变化订阅及入口退休 |
| 实际接通 | srv_fs、srv_init、现有 test_fal 及 libfs/libservice 内部消费者；验收进程随迁移改名为 test_fs |
| 支撑面 | 必要的 RPC 编码工具和 provider ingress 分派、Cargo/构建/initfs/验收入口、对应实现说明与导航 |

不在本案实现磁盘/网络文件系统、音乐播放器、完整设备/驱动资源协议、正式长期服务监督或新 RPC/Runnel 运行体。内存后端不承诺持久化；设备/驱动仅检验 OAL 的扩展接点，其领域设计由[设备分案](todo-2026-09-device-system-design.md)拥有，启动和健康监督由[装配分案](todo-2026-10-07-service-assembly.md)拥有。

## 已确认的架构边界

1. OAL 承接 FAL 的通用树状结构与使用方式，领域专属解释和操作抽入各领域库。Directory 属于 OAL；provider 暴露内部对象的视图，Lookup/Enumerate 由其实际 backing 或计算过程即时生成 ObjectInfo/Boundary。库不维护一棵 OAL backing 树；客户端持有目录入口、路径上下文并跨 provider 发现，不构成另一份 backing。
2. Trait（特质）本身只是开放的数字标记；OAL 不赋予领域含义、操作承诺或组合约束。消费者按自己引入的领域知识匹配所期望的标记组合，忽略其余标记；provider 按领域请求实际提供行为或返回错误。tag 是创建时间等带值属性，与 Trait 区分，二者同属 ObjectInfo 面貌。ObjectInfo 是观测时产生的瞬态视图，不承诺可读写、稳定身份、固定形状或跨次一致性。
3. 实际路径解析作用于 provider 的 backing；库可以提供遍历和接续工具。“只有一套”指没有 OAL/fs 各自独立的目录真值或执行层。OAL 拥有通用目录使用以及 ProviderBoundary/LinkBoundary 的客户端接续：前者交付入口继续，后者转发路径继续。LinkBoundary 不限定为文件符号链接，任何 provider 都可生成；是否存在真实链接对象及其领域操作由 provider/领域解释。
4. 普通 Get/Read/Write 按本次实际目标执行，不要求先 Lookup，也不携带上一查询的 expected_identity；观察不产生锁定、占用或每进程隔离视图。
5. liboal/libfs/libservice 提供领域知识、接口/数据形状和可复用工具，不替所有 provider 统一决定具体行为、策略或实现。fs 是抽象的文件领域知识，实现 fs 不要求 provider 本身是磁盘文件系统：网络数据、数据库或 music_player 转接的歌曲均可通过 fs 暴露；底层来源及如何兑现请求归 provider。libfs 也提供遍历、文件操作与实现支持，实例状态由实际使用者/backing 持有，不因使用库再出现一份真值。
6. provider 可以组合多种领域协议，也可以消费其它 provider 来实现自己的协议；播放器的多协议摘要/内部 Open 转发仅是设计检验例。
7. 真实能力、付款和已提交责任保持各自 owner。可见的标签不产生权限，撤名不证明已有业务资源已经回收，Close 不冒充 Finish/Sync。
8. 普通路径使用以 namespace 和路径为入口；OAL 持有客户端 walk 上下文，领域客户端拥有 RPC 编解码与调用，真实范围和授权归 provider。删除客户端 DirectoryGrant 同义包装及普通调用对 Position/预先 Lookup 的要求；复用既有真实 Endpoint owner。受限入口签发与业务需要的稳定目录授权保留为显式领域能力，客户端路径前缀不产生隔离。

## 基线、前置与方案状态

基线为 2026-10-09 的 `refactor/system-redesign`，HEAD `6187ce194b26881076f0eba209f997b808fd0c3a`。本轮接手前已有前两轮设计及关联导航修改，已完整存入快照并在其上收敛；不覆盖其它任务。仅更新 plans 与关联导航，不把目标提前回填成 ideas/impls 的已实现事实。

代码与[当前 FAL 实现说明](../notes/impls/fal.md)描述旧实现；[KNOWN_ISSUES](../KNOWN_ISSUES.md#矫正区)已登记矫正范围，不把旧结构当新方案的正确性依据。现存 RPC、Runnel、Process/Job 和 SourceRegistration 已有实现基础，本案不重新启动已完成的对象链或运行时分层任务。

本案从既有 FAL/fs 的协议和 backing 实现做职责分离。库定义契约与可复用代码，实际实例由 provider 持有；不能把“代码位于某库”解释成“运行时多一个拥有对象真值的层”。共同 PathHeader、OAL 通用 Continue 业务封装、框架先定位再调用领域 handler、强制 Locate 前置均撤出当前方案。CreateOpen、共享流、Copy 保留部分结果等仍只是 fs/backing 的领域设计，不是 OAL 的职责。

当前共享运输常量为 `PAYLOAD_MAX = 4096`、`MESSAGE_HANDLE_MAX = 8`，来源是 [message.rs](../shared/erhino_shared/src/message.rs)，它们是现状依据而非在新库复制的常量。旧 `PATH_MAX = 512`、链接/步数限额不能未经论证照搬。完整报文必须计入 RpcPrefix、路径/领域头、参数、响应和实际能力槽。

## 设计决策与施工门

本轮用户授权完整整理并补全设计。以下技术选择将此前的初稿收敛为同一实施目标，不把它们追记为第八轮用户逐项确认的决定。代码施工仍不属于本轮任务。

| 决策 | 本轮确定的目标及理由 |
|---|---|
| 发现模型 | 沿用已确认的瞬态 ObjectInfo、开放 Trait/tag、两种 Boundary、namespace/路径直接领域调用；不再引入定位握手、对象照片锁定或统一业务执行层 |
| Trait/tag 表示 | 开放的 namespace/local 数字标识；tag 的值为长度定界字节。各领域拥有含义和编号，OAL 只验证结构 |
| 文件原子移动 | Rename 同时移动/改名，原子域是同一实际文件 backing；跨域明确 CrossFilesystem，不把 mailbox/进程相同当成充分条件 |
| Copy 与高层移动 | 本轮提供 Stream 与纯数据 Property 到同种新文件的 Copy，允许跨 provider，保留部分目标并报告未知结果。跨文件系统复制后删除的高层 Move、递归目录复制及额外元数据保存不在本轮；不能用 Copy 后按路径 Remove 偷补这些能力 |
| 服务目录 | 平铺名字，服务条目为叶；GetEntry 显式取得入口和完整协议描述，由客户端选择协议并决定本地绑定 |
| 发布 | 服务就绪后单步 Register，公开 Active/Withdrawn；替换的未知结果与迟到回复按注册完成合同处理，不恢复旧注册 |
| 枚举 | 本案两个 backing 按名字分页，跨页弱一致；不创建读者快照。远端边界项只返回名字和“需另行 Lookup”，不伪造目标摘要或批量导出能力 |
| 装配 | 首轮在 srv_fs 中组合两个独立 provider/mailbox/backing，同一 Runtime 推进；客户端分别绑定文件和服务目录。正式进程拆分及监督归装配分案 |

跨文件系统高层 Move 的不承诺是交付边界，不是留给实施者临场选择的策略。其未来需求必须同时确定并发源修改/替换、删除授权、目录递归、属性、部分失败及恢复合同；没有这些条件时 Rename 返回 CrossFilesystem，Copy 单独报告结果。它们不阻塞本轮已明确的文件原语。

D0 的关闭条件为：本篇线上表示、方法/权限、接续、提交、取消/失联、资源退休、模块/消费者迁移和验收门均无影响本轮承诺的未决项；结构审查发现已逐项处置。Rust 文件内的拆函数、容器选择与字段排布可按本篇自行决定，但不能改变这些合同、增加旧协议兼容层或建立第二份真值。审查和证据记录见文末。

## 目标设计

以下承载当前目标契约；后面的线形、状态和迁移表是这些边界的实施细化。方案状态由上一节统一记录。

### 职责与依赖

| 旧机制 | 目标归属与处置 |
|---|---|
| 通用目录、路径表达、Lookup/Boundary 与发现使用 | OAL 承接 Directory、用户态树状声明、provider、目录入口传递、ProviderBoundary 和通用 LinkBoundary 接续；provider 解析自己的 backing，fs 拥有符号链接对象的具体操作 |
| 固定 NodeInfo/NodeSummary | 由 OAL 的瞬态 ObjectInfo 替代；领域 Trait/tag 由各领域定义，OAL 不设统一 NodeKind、组合合法性或强制对象身份/版本字段 |
| Property/Stream、目录变更、文件权限及流控制 | libfs 定义领域接口和实现支持；实际存储、授权表与会话归文件 provider。删除客户端 DirectoryGrant 包装，真实受限入口签发和稳定目录授权不随之删除 |
| ServiceRecord 伪装为 Property/Record，Read 时导出 endpoint | 删除该表达；服务摘要为 OAL 投影，取得入口是 libservice 的显式操作 |
| 服务名字授权、发布控制、就绪/撤出、endpoint 失效观察 | libservice 定义契约，registry backing 持有并推进实际状态，直接产生 OAL 视图，不增加协议层状态或 FAL 投影 NodeStore |
| Directory 特制 ExportPolicy、属性中的能力值及 affine Take | 不进入 OAL/tag 或 fs Property；真实能力交付由领域操作和 RPC 承担，旧探针随迁移移除 |
| 文件 Watch、Copy | 保留有独立用途的文件能力，按下文归 libfs；服务变化订阅归 libservice，不让服务目录冒充文件 |
| 通用 LE Reader/Writer | 移到 librpc 的协议编码工具模块，由 OAL/fs/service 直接使用；不为共享字节游标保留 libfal 或建立 fs→service 依赖 |
| RPC、Runnel、来源登记、预算 | 复用各自 owner；不新建通用 OAL 运行体或第二种流运输 |

目标依赖为 `libfs → liboal + librunnel`、`libservice → liboal`，二者均可直接消费 librpc/libexecution/libbudget/libkernel；libservice 不依赖 libfs。music_player 可以同时消费 liboal/libfs 并实现自己的协议。设备/驱动库按相同方式定义标签和领域操作，不使 OAL 增加领域分支。

核实依据：现有 [文件后端](../user/libraries/libfal/src/backend.rs) 的 lookup/resolve 共用 lookup_child；[目录枚举项](../user/libraries/libfal/src/protocol.rs) 已有 DirectoryEntry.name/info；[客户端代码](../user/libraries/libfs/src/resolve.rs) 的 Found/Delegate/Link、prefix 和 Position 承载现有路径接续。通用换入口、相对/绝对转发及路径上下文由 OAL 承接，解除对文件 NodeKind/NodeSummary 的依赖；fs 保留符号链接内容读取、创建等领域操作。现有 RPC 和 Move 准入仅作既有运输/授权事实。

### 库与 provider 的使用关系

liboal/libfs/libservice 提供各自领域的知识和可互通的形状，具体 provider 按自身需求使用。库可以提供类型、编解码、客户端封装、遍历等基础算法及实现协议所需的工具；它们不是必须逐层经过的运行时，也不替 provider 统一选择数据组织、共享/独占或其它业务策略。

用户给出的 `impl Provider for MyProvider` 只是表达接口关系的伪代码，不是已选定的 Rust trait、async 签名、分派或模块布局要求。具体写法由实施者结合现有代码选择，无须再把这些常规工程选择交用户确认。

例如文件 provider 可以直接使用 libfs 提供的遍历工具操作自己的 backing，并将结果按 liboal 的 ObjectInfo/Boundary 形状暴露；也可以在适合自身后端时提供专用实现。复用算法和操作封装不等于多出一个拥有领域真值的 fs 层。

RPC 接收设施只负责协议识别、解码、调用方法和发送回复；调用关系分别为 `OAL client → MyProvider::lookup → backing` 和 `fs client → MyProvider::read → backing`，不存在 `OAL 执行层 → fs 执行层 → backing` 的串联关系。查找本身当然是一次协议调用，但 OAL 不接收、解释或执行文件/服务业务操作。

协议无领域状态，不等于运行程序没有调用上下文或缓冲。回复缓冲、临时请求、客户端持有的入口和流进度由实际运行实例持有；它们不构成另一份可独立更新的对象真值。若库提供可复用 backing 实现，其实例仍是 provider 选用的 backing，不能再为它配一份所谓 OAL/fs 层状态。

### OAL 入口与路径

入口是能接受相应协议请求的真实通信能力，客户端沿用既有 typed sender owner；transport 提供已认证的来源，provider/backing 解释其真实范围及权限。OAL Namespace 只保存路径前缀到入口的本地绑定，walk 持调用期间的路径上下文；不建立通用 DirectoryGrant 或 FsRights 模型。文件、播放器、registry 按各自知识授权和操作 backing，真实收窄入口由发行者兑现。

OAL 描述目录、名字、子项和跨 provider 的路径关系，并提供通用发现工具。路径如何在内存树、文件系统、注册表或播放列表中解析，由 provider 实现；客户端按公共 ProviderBoundary/LinkBoundary 维护接续所需的目录入口和路径上下文，不复制 provider 内部对象树。文件链接或播放列表别名都可产生相同的 LinkBoundary。

Boundary 中可携带继续发现的目标入口及剩余路径关系；接收者据此向相应 provider 发起后续查询。这是协议结果的使用，不是 OAL 遍历远端内部结构。只有 provider 知道应该返回摘要、边界或错误；客户端不根据 tag 自行猜测底层位置。

若边界涉及目录权限衰减，仍由具体 provider 与真实发行者兑现后再交付入口；不能只发送过宽能力和一份要求客户端自律的 rights。相关请求和临时能力由实际 provider/调用者持有并沿已有运输契约清理，OAL 不维护 grant 表。

### 客户端 API 与 Namespace

Namespace 直接持 `Arc<MailboxSender>`，不添加只有 endpoint 字段的 Grant/DirectoryGrant。`bind(prefix, entry)` 要求规范绝对分量前缀，重复前缀返回 Exists；`replace_binding` 显式替换，`unbind` 返回原 owner。匹配取分量边界上的最长前缀，`/a` 不匹配 `/abc`；没有匹配入口返回本地 NoBinding。根绑定不是必需。绑定变更需独占访问，单次调用借用固定 Namespace，不在同一次接续中混用修改前后的本地绑定。

Namespace 的前缀映射只负责本地路由，不自动创建父目录、合并远端枚举或制造目录摘要。程序知道自己声明的绑定，可直接访问该前缀；需要通过远端目录发现的名字仍由实际 provider 提供。首轮 `/System/Services` 是启动装配明确交付的绑定，不把“所有本地绑定自动出现在根枚举”列为已经提供的能力。

对外路径可以绝对或相对；相对路径以调用者显式提供的本地 cwd 为基点，默认 cwd 为视图根。重复斜杠、`.` 与 `..` 在视图中按分量词法规范化，越过视图根返回 InvalidPath；被词法抵消的分量不访问远端。本案不承诺 POSIX 对 `missing/../x` 或“符号链接后再跟 ..”的逐分量物理寻路规则。链接返回的 target 在其指定父位置展开后采用同一视图规则。这里的路径处理不做 Lookup，不赋予文件语义。

| 客户端表面 | 必须保持的参数与行为 |
|---|---|
| `Namespace::{bind,replace_binding,unbind}` | 已有入口与本地前缀；不发 RPC，不收窄或扩大真实授权 |
| `oal::Client::{lookup,enumerate}` | 借用 namespace 和既有 RPC，配置接续限额；首次枚举收 path/max_entries，Page 可返回 EnumerationCursor，由其 next 方法续页；只调用 OAL |
| `fs::Client::{get<T>,set,read_at,write_at,open,create,create_open,remove,rename,read_link,set_length,sync,derive_directory,watch}` | path 与本操作参数；rename 接收两个用户路径，内部取得目标父目录真实授权；get 的 T 只参与本地解码 |
| `fs::Client::copy` | source/destination、明确的 `CopyKind::Property/Stream`、目标 node ceiling 及期限；不为自动猜种类强制 Lookup |
| `service::Client::{get_entry,watch_directory,watch_name}` | namespace/path；入口可直接用于领域客户端，是否绑定由应用决定 |
| `service::{Publisher,RegistrationControl}` | 使用实际发布/控制 sender；Register/Withdraw/Query 不按 namespace 定位控制对象 |
| `StreamControl` / 各领域 `WatchControl` | 持有真实控制能力并封装该能力的业务操作，不是普通目录入口的同义包装 |

这些参数角色与副作用是接口合同；同步/异步返回写法按既有 librpc/libexecution 实现，不新建 Runtime。需要直接使用已取得业务入口的消费者，可用单根 Namespace 或领域的入口相对调用；不得强制它先查询 ObjectInfo。

walk 私有状态保存当前入口、该入口在视图中的根位置、未完成路径、上层入口帧及剩余预算。ProviderBoundary 推入新帧；LinkBoundary 在父位置拼接 target 和 remaining，保留适用的现有帧，越过入口根时回到上层帧，视图绝对目标从 Namespace 重选。调用结束释放临时帧；显式 bind 才延长入口的本地存活。不会将私有帧重新包装成普通应用必需的 Position。

领域请求能否通过某入口取决于该 provider 实现的协议。只有 OAL 的 provider 不必转发未知领域 RPC；返回 Unsupported 后不由库自动补一次 Lookup 再重放。装配通过明确绑定和 GetEntry 保证正式消费者可达，OAL 标记不承诺任意领域操作在同一路径上可达。

### 路径请求与领域接续

Lookup、Read、Open、GetEntry 分别是各协议的请求，直接交给实现它的 provider 方法。已经知道领域的调用者可以直接发领域请求，不要求先握手或先取得摘要；provider 在本次操作中验证实际目标。OAL 发现帮助未知调用者认识对象，不成为所有领域操作的必经执行层。

在同一进程/入口复用接收设施时，可以根据协议标识调用不同的 Provider 实现；也可以按实际授权角色设置不同入口。这属于具体 provider 的装配和 RPC 分派，不引入统一 PathHeader 或 OAL 的 `Done/Continue(domain_response)` 业务信封。

文件操作遇到跨 provider 情况时如何继续、是否内部转发、哪些结果可以重试，由 fs 协议及其具体实现负责；服务和播放器同理。领域可以复用 OAL 的 Boundary 表达，但 OAL 不接管其请求、能力输入或副作用。本案 fs 的接续结果只发生在领域操作提交前，具体合同见路径接续细则。

### 发现操作与目录枚举

liboal 的提供端接口以 Lookup/Enumerate 为基础。Lookup 的结果为 `Found(ObjectInfo)` 或 `Boundary`，失败返回协议错误；具体实现可内部查询/遍历 backing 后生成结果，也可直接计算这个视图。没有 Locate、通用前置遍历或定位后统一执行业务的要求。

Boundary 保留换入口和路径转发两种通用表达，客户端负责跟随，不把 Boundary 当作 object。Lookup 中途 LinkBoundary 用于继续未完成路径；末项符号链接返回其 ObjectInfo，fs::ReadLink 等操作按领域协议读取实际链接。专辑别名的末项视图由播放器直接生成，OAL 不把别名强制解释成符号链接。ProviderBoundary 仍可换入口后查询目标入口根；最终应答 provider 生成 ObjectInfo，不合并沿途 Trait/tag。

枚举由 provider 对自己的 backing 执行，返回子项名字和可选 ObjectInfo；远端边界项以摘要缺省表示，绝不把 Boundary 当作对象或伪造目标面貌。分页游标由 backing 解释；OAL 只定义请求/响应及完整、结束、失效和过大摘要等结果。枚举 path 的末项链接若需指向被枚举目录，可以返回 LinkBoundary；Lookup 的末项仍只返回对象面貌。目录变化不默认生成每个消费者的快照或镜像树。客户端与提供端的响应/查询预算沿各自请求生命周期落实，不以通用走路状态机代替。

### ObjectInfo、Trait 与 tag

ObjectInfo 是 provider 在 Lookup/Enumerate 时生成的面貌描述，包含 Trait 标记和 tag 属性；它不是持久对象、统一对象存储或接口契约注册表。创建和解码可以检查报文边界、长度等表示完整性，不检查领域标记的组合是否合法，也不验证面貌是否真实。没有可靠时间等属性的 provider 可省略相应 tag；OAL 不为补全视图创造 backing 状态。

Trait 是可比较的数字标记，不含 OAL 解释的操作清单、互斥类型规则或组合约束。标记定义由各领域库拥有；客户端是否理解某标记取决于引入的领域知识。消费者按当前用法匹配所需子集，未知或无关标记不影响匹配，不要求 Trait 集合恰好等于某个封闭对象类型。数字标识使用下文的开放 MarkerId；不能把 Rust 闭合枚举变成接收端拒绝未知领域标记的理由。

OAL 允许承载 `Property+Integer+Array+Boolean+String` 等任意标记组合，但这不使无期望形状参数的 fs Get 自动获得多种返回值分派能力。Get 返回 provider 实际提供的值，`Get<T>` 在消费者处解码或转换，不相容时返回类型/范围错误。provider 须避免宣称其 Get 实际无法提供的相矛盾形状；OAL 不阻止、证明或修复虚假声明。`Property+Boolean+mime::Mp3` 同样不触发 OAL/fs 的 MIME 合法性检查。不同协议/操作各自拥有的返回值仍可并存。

tag 表达创建时间、长度等带值属性，和 Trait 一起组成照片。解释 tag 的值同样需要相应知识；ObjectInfo 自身不提供通过修改 tag 改写 backing 的通用操作。Directory 属于 OAL 的树状发现表达；文件、服务、MIME、播放器等领域分别拥有自己的标记与属性。

观察不会固定对象的形状、名字、接口可用性或实际内容。provider 可以动态计算甚至随机产生视图，已知消费者也可以不经 Lookup 直接发领域请求；provider 收到后执行或返回错误。provider 对业务数据与行为负责，OAL 不以摘要构造器、全域形状验证器或发现握手为其背书。

完整 ObjectInfo 须适合有界响应，过大的领域正文通过领域协议取得；这一运输边界不产生领域形状约束。线形与首批编号见下文；没有可靠时间来源的本案 provider 不发创建时间，不伪造时间或为补齐 tag 新增持久状态。

### fs 基础接口

libfs 拥有文件类型、协议、编解码、客户端组合与可复用实现。下列成功语义是使用该操作时必须兑现的合同；provider 可明确 Unsupported，不能成功后改变该合同。共享内存 backing、稀疏存储与内部收束方式是本案具体实现，其他 provider 不必采用同一存储结构。Get 只发送路径，不发送客户端 T 或期望形状；T 仅用于实际返回值的本地解码。

| 操作 | 明确语义 |
|---|---|
| `Get<T>` / `Set<T>` | 完整读取/替换有类型 Property；检查本次实际目标。Set 可改变 Property 值形状，随后的摘要自然改变；不是修改 tag |
| ReadAt/WriteAt | 对 Stream 在明确偏移执行有界字节访问；返回实际字节数，支持短读/短写；普通 Write 不隐式创建或截断 |
| Open | 按当前路径建立指定方向/范围的文件流会话，返回控制入口和 Runnel offer；不带先前 Lookup 的 expected_identity |
| Create | 在已定位父目录中独占创建 Directory、Property、Stream 或 SymbolicLink；名字已存在报 Exists |
| CreateOpen | 独占创建 Stream 并在同次成功结果取得其流会话；用于避免 Copy 在创建与按名打开之间操作到别人替换的文件 |
| Remove/Rename | 对实际目录名字操作；非空目录不递归删除。Rename 校验两个真实目录授权，在同一文件 backing 原子提交；跨原子域返回 CrossFilesystem，不暗中 Copy/Delete |
| ReadLink | 本操作固定不跟随末项链接，返回目标字节；没有通用 NoFollowFinal 线上标志，不要求先取得摘要 |
| SetLength/Sync | 前者改变逻辑长度、扩展部分读为零；后者单独请求持久化，后端不能兑现时明确 Unsupported，不能把内存接收当作持久完成 |
| DeriveDirectory | 由文件 provider 签发受限目录 grant，范围和文件权利不超过来源；发现权限和 FsRights 分别检查 |
| Watch | 显式订阅文件/目录变化，返回独立控制入口；不使用共享目录 grant 的裸订阅 ID 充当唯一管理权限 |

Property 值的初版编码支持 Bool、Signed64、Unsigned64、Float64、UTF-8 String、Blob 及这些类型的同质一维数组；不包含 capability/ServiceRecord 或通用递归 Record。`Get<usize>` 接受 Integer 并做范围检查，标量/数组不相容则 TypeMismatch。值编码由 fs 拥有，ObjectInfo 只给平铺形状摘要；完整值受一次 RPC 原子替换/返回的容量约束，超出返回 ValueTooLarge。

文件 provider 的 backing 持有 FsRights、实际授权根、grant 表和可信付款绑定。旧 GrantTable 的真实能力校验用于 Rename，还须检查源 REMOVE、目的 CREATE/替换权限、目录类型和循环条件。Create/Rename 的路径处理由 backing 按 fs 协议执行，不经 OAL PathHeader、Locate 或框架预先解析；libfs 定义这些方法的参数与保证，provider 完成实际校验和提交。

Get/Set 对一份实际值线性化；小 ReadAt/WriteAt 与其它对同一文件的操作以本次有界数据提交为单位串行化。共享流按块推进，不承诺整个文件或整个流的原子快照；重叠写以实际提交次序决定结果。实现可用单一 owner 或后端短临界区，不能持锁跨远端等待。长数据处理按预算分段，文件名变更不把已选定的会话改指其它对象。

本项目当前内存文件 provider 的迁移方案采用共享访问会话和独立进度，不转移文件归属。这是具体实现的选择，不是 libfs 对所有 provider 的统一要求，更不是 OAL 的要求；其它 provider 按自身需要采用相应行为，并通过所使用的接口/结果形状表达。

首批实现支持矩阵：内存文件 provider 提供目录 Create/Remove/Rename/Derive、Property Get/Set、Stream 小 I/O/Open/CreateOpen/SetLength、链接 Create/ReadLink 及文件 Watch；Sync 明确返回 Unsupported，不宣称内存文件持久化。服务目录实现发现、GetEntry、注册控制和服务订阅，不实现文件接口。播放器仅是组合检验例，可实现只读 fs Open 与自己的播放协议，不因此必须实现 Set/Create。Unsupported 表示对象/provider 不提供该操作，Permission 表示实现提供但本次授权不允许，两者分开。

### fs 流、订阅与 Copy

复用现有 Offered → Active → Terminal 的控制过程及 Runnel 单工数据面。Open 成功先交付控制入口/offer；客户端建立映射并按 Runnel 准备后 Start，provider 才执行传输。offer 有有限建立期限，未 Start 的退出/超时释放预备占用。Active 会话按显式期限及端点存活收束，控制入口持有已选择的后端文件。

读流的范围由 Open 时的起点及上界确定；内容可以被其它写入改变，截断可能导致提前 EOF，不伪装成固定内容快照。写流的 accepted 是已进入后端的前缀，transported 是运输进度，两者不混淆。Finish 等待 EOF 被正确排空及业务处理完成；Cancel 保留已接受前缀，并在已提交的后端工作确定收束后给出终态，不能在仍可能提交写入时提前宣布最终计数。读取方向的对端接收计数只表达已经确认的消费进度，Copy 以自己的实际读出量另行计数。Sync 的持久性要求与 Finish 分开。映射/来源撤销失败由会话退休 owner 继续处理，不能因回复、取消或关闭控制入口就提前退款。

Watch 由文件协议拥有，服务目录另有服务变化订阅。两者复用底层通知/来源登记和单调计数工具，不再共同依赖文件 NodeStore。每次变化递增 sequence 并合并事件位；Query 不清空，Ack(sequence) 仅在未出现更晚变化时清除，否则保持通知，避免 Query/清除之间丢唤醒。事件是重查提示，不是完整日志。fs Watch 绑定所选后端文件/目录，其退休、明确撤销订阅授权或 provider 停止给出终态；服务按名字/目录订阅的终止条件见下文，不能因某次 registration 退休就终止它。独立控制入口关闭后取消订阅并最终释放资源。

消费者顺序固定为：先建立订阅，Query 订阅得到 sequence，再读取/枚举实际状态，最后 Ack 这个旧 sequence 并等待通知；不能在状态读取之后取一个更晚的 sequence 来确认，否则会吞掉中间的变化。序列耗尽时明确终止订阅，不回绕复用。

Copy 是 libfs 客户端组合：Open 源、CreateOpen 目标、按预算搬运、分别完成两端。源内容不是默认快照，目标是本次创建并打开的文件；名字随后改变不把会话转向替代者。结果分开记录本地读出量、提交到运输的量、最后确认的目标 accepted，以及两端可缺失的终态；只有收到目标可信终态才能报告精确最终 accepted。失联时最后确认值只是下界，最终写入/完成可以未知，关闭映射不补足这个知识。初版保留已经创建的空文件或部分结果，不自动按原路径删除。源、目标会话及映射由 Copy owner 全部收束；文件本身仍归后端，保留结果不是泄漏。

上述为 Stream 分支。Property 分支执行一次完整 GetValue，再以同一值的原始编码 Create(Property)，不转换客户端类型、不复制 capability，也不伪造 Stream 计数。源值取 Get 的线性化快照；源随后变更不影响已取得的值。目标 Create 原子提交完整值，失败可能为 NotCreated 或 Unknown，不存在“半个属性值”。两分支均要求目标名字空闲，不复制源的 node ceiling、时间、授权或其它元数据；目标 ceiling 由调用者明确给出，成功只表示本次数据操作完成。

发送 CreateOpen 后丢失结果时报告创建结果未知，不自动重试、按名字清理或宣称回滚。这个选择替代旧 CreatedTarget 的默认条件删除路径；它不需要跨 Lookup/Get 身份匹配。是否另提供显式临时文件/原子发布由独立文件需求触发，不是当前 Copy 成功或失败责任的缺口。

### 文件操作边界的依据

[外部取证](ref-2026-10-09-cross-provider-file-operations.md)核实了 QNX、Hurd、Genode、MINIX 和 POSIX/Linux 的不同边界。它们支持分别建模原子 rename、复制/删除组合与数据复制加速；不把跨进程 IPC 本身当作原子性的保证或障碍。

本案 Rename 的目标目录授权必须由源处理实例识别，并指向同一 MemoryBackend 实例。相同 mailbox 上的不同 backing 仍返回 CrossFilesystem；同一 backing 的不同受限入口在各自授权均满足时可以原子 Rename。验证依据是可信 context 解析得到的实际 backing/目录引用，不是客户端可写的数字域 ID。无法在本地可信授权表识别的外部目录能力返回 CrossFilesystem；不会向远端发送“试一下”的变更请求。普通重命名、同域跨目录移动和跨 provider Copy 均在本轮支持矩阵内。

### 服务目录

本节定义本轮服务目录目标；详细完成与失败合同见[注册提交与完成状态](#注册提交与完成状态)。服务目录可被 OAL 发现，但其注册对象不作为文件，也不承诺把服务业务子树自动接入目录。

#### 当前事实与职责

核对基线为 `6187ce194b26881076f0eba209f997b808fd0c3a`。当前已有独立 `libservice::Registry` backing，但没有独立服务目录进程：`srv_fs::ServiceBackend` 组合内存文件与 Registry，后者仍持 `NodeStore<Projection>`、FAL root/rights/watch 及编码后的 ServiceRecord。`libservice::client` 当前只封装 DelegateName，其余注册 RPC 与状态任务分布在 srv_fs/srv_init。这些是待迁移事实，不作为必须保留旧分层的理由。

目标是独立的服务目录 provider：自己的服务名字、注册记录、访问/发布授权、订阅与退休 owner；同时实现 OAL 发现和 service 领域操作。两种协议直接读取同一份 registry backing，不再建立文件节点或持久化 ObjectInfo。libservice 不依赖 libfs/libfal，不要求发布的服务是文件系统，也不理解 FsRights。

首轮保留现有进程装配：srv_fs 组合文件 provider 和 registry provider，二者拥有各自 mailbox/context 和 backing，同一 libexecution Runtime 推进请求与退休；删除按 NodeRef 判断后端及要求 registry 实现文件 Backend 的连接。独立 provider 不等于新增 srv_service 进程；正式进程拆分、启动依赖、健康与恢复仍由装配分案拥有。

#### OAL 视图与取得后的使用

- registry 的根是 `oal::Directory`，枚举当前可见服务名；首轮每个 registry 是一个平铺目录，名字为单个分量。层次通过客户端 Namespace 组合不同目录入口，不增设服务目录内的文件树。
- `Lookup(name)` 从当前名字绑定生成 `service::Entry` Trait 和注册标识/协议列表等 tag；未发布或已撤出的名字返回 NotFound。目录项描述服务注册，不能把业务对象的 fs/music Trait 原样贴到这个条目上。
- 服务条目是叶对象：`Lookup(name/child)` 返回 NotDirectory。取得业务入口由 service::GetEntry 显式完成，不通过 Lookup 隐式导出 Endpoint，也不要求服务目录解释或转发任意未知领域 RPC。OAL 本身仍支持两种 Boundary，本选择仅规定首轮 registry 的条目语义。
- 客户端把服务目录入口绑定到本地 `/System/Services` 后，可直接调用 `service::get_entry(namespace, path)`，不先 Lookup。返回的入口若接受根相对路径，可绑定到 `/Music` 等业务路径，之后 fs/music 客户端直接调用真实 provider；不带路径的工厂/控制入口直接交给相应领域客户端。
- 服务名被替换不会自动替换消费者已有的 Namespace 绑定。消费者收到变化提示后，自行重新取得并决定重连；已投递业务请求不因服务换代自动重放。

`ObjectInfo`、GetEntry 描述和枚举页均从记录即时生成。GetEntry 在一次受保护步骤中选择同一注册的描述和入口副本，保证二者配对；此前的 Lookup 不参与条件匹配。协议描述是发布者的声明，registry 验证其编码和能力运输条件，不验证被发布服务是否真的实现相应业务。客户端按自己的领域知识选择协议/版本，GetEntry 不承接业务协商。

#### 发布、控制与订阅接口

| 接口 | 语义及授权 |
|---|---|
| DelegateName | 持根发布 authority 者签发限定名字、Create/Replace 权利的发布入口；不因目录可见而获得发布权 |
| Register(CreateOnly/Replace) | 发布者在业务就绪后提供名字、协议描述与可重复共享的真实入口；一次提交名字可见并返回 RegistrationControl。CreateOnly 要求空位；Replace 需 Create\|Replace，可在空位创建或原子替换现有注册 |
| GetEntry(path) | 从本次可访问的 Active 注册取得配对的 RegistrationId、协议描述和入口副本；不创建业务会话 |
| Withdraw / QueryRegistration | 通过该注册自己的控制入口撤出或查询；旧控制者不能删除替代者，无按名字操纵别人注册的旁路 |
| WatchDirectory / WatchName | 通过服务目录入口订阅目录或名字变化，返回独立控制与通知入口；WatchName 允许名字尚不存在并跨替换持续 |
| QueryWatch / AckWatch / CancelWatch | 通过订阅控制入口管理该订阅；遵守先 Query 序号、读实际状态、再 Ack 旧序号的顺序 |

Register 本身表示发布者声明已可服务。当前 secondary 在旧 Register 成功后立即 PublishReady，二者之间没有业务初始化步骤，因此本案删除“注册后等待调用者 Publish”的公开阶段和建立租约。公开注册状态只保留 Active / Withdrawn(reason)；准备和资源退休是 provider 私有状态。Starting、Ready 健康状态及业务 Draining 不再由目录维护；启动者决定何时注册，业务停止与监督由各 owner 承担。

发布入口必须已允许重复共享，真实领域权限在交付 registry 前由发行者收窄。registry 只复制允许的运输权限，不隐式调用文件 Derive、不替消费者建立隔离会话。需要独立业务授权时发布领域准入入口，消费者取得后显式调用。服务访问、发布权限与控制能力分开；付款继承可信入口/发布上下文，裸账户数字不成为授权。

WatchName 绑定目录范围内的文本名字，WatchDirectory 绑定目录；服务替换和单次注册撤出只产生变化提示。订阅在取消、订阅入口授权明确撤销、目录停止或计数耗尽时终止；注册记录退休不使名字订阅终止。通知合并而非完整日志，Query 不清零，Ack 旧序号只在没有新变化时清除通知。

#### 库与宿主的具体拆分

| 位置 | 目标知识 / 实现 |
|---|---|
| libservice::protocol | service 操作、RegistrationId/Info、协议描述、权限/错误与 wire；直接用 librpc 编码工具及 liboal Boundary/ObjectInfo 表达，不含文件类型 |
| libservice::client | GetEntry、注册/控制与 Watch 封装，入口沿既有 typed sender 持有；路径方法复用 OAL Namespace/walk，控制方法直接按能力调用，无 DirectoryGrant |
| libservice::registry | 可复用的具体 registry backing：名字索引、Registration、访问/发布权及订阅记录；采用者拥有实例，没有第二份 OAL 投影 |
| libservice::provider | 与现有 Runtime/RPC 配合的 ingress/请求、注册控制、来源观察、订阅和退休支持；消息仅按真实 context 角色与协议分派，不在每个宿主复制状态机 |
| srv_fs | 创建两个 provider 实例与实际资源账户、接收入口，装配单 Runtime 和停止顺序；不再拥有 ServiceRecord schema 或 registry 的另一份注册表 |
| srv_init / test_fs | 启动与授权交付、Subscribe→GetEntry→真实领域调用；绑定服务目录与已取得业务入口，检验公开行为 |

具体模块可按现有类型调整，但协议/客户端/可复用 registry 支持与宿主装配的知识边界固定。不新造执行框架，不以回调让 libservice 反向依赖文件 provider。

### 提交与资源交接

| 责任 | 准入/提交点 | 失败、退出与最终释放 |
|---|---|---|
| Lookup/枚举/Boundary 回复 | provider 方法读取/遍历自己的 backing 或即时计算视图，必要时准备继续入口 | 请求和回复资源归实际运行实例；未交付/未采用能力沿 RPC 清理；客户端可持调用期间的接续状态，不产生 OAL backing 树或叶对象会话 |
| Property/目录变更 | 内存/配额准备完后，由后端原子提交值或名字变更 | 提交前可回滚准备；提交后回复失败不回滚，不自动重放 |
| 文件 Open/CreateOpen | 预留文件/会话/运输资源后建立可交付结果；CreateOpen 的名字插入是创建提交点 | 丢失回复时控制/offer 经寿命观察及期限退休；已提交文件保留，结果未知须报告 |
| Registration | Register 在所有入口观察、回复及退休承载就绪后原子提交名字；成功控制属于该次注册 | 提交后回复失败不回滚；最后控制引用消散或业务入口关闭后由 registry 撤出自己的绑定，持续持有退休责任 |
| GetEntry | 选定 registration 并准备对应入口副本 | 交付失败清理副本；成功后由消费者持有，发行者拥有业务资源；撤名不提前退款仍存引用 |
| 订阅/流会话 | 各领域显式建立资源并注册既有来源观察 | 端点关闭/超时触发当前 owner 退休；清理待办继续被 provider 持有，真实释放后退款 |

协议库不重建 RPC、执行或来源登记。实际宿主负责把收到的消息调用到相应 provider 方法，backing 持有并推进 root、会话、registration、订阅等领域状态。复用现有 RpcPrefix、回复授权、能力槽和 PreparedResponse/Outbox 的运输规则，不给 ObjectInfo/tag 或抽象协议层另建账户/对象状态。

请求任务的阶段为接收/解码、准备、提交、回复、清理；它们是实际 provider 的任务状态，不是新 OAL 执行层。完整验证头、正文、能力个数/种类/真实 context 后才进入业务。异步准备前持有授权与目标引用，返回时复核仍需成立的授权、名字条件与期限；变更提交前已预留回复和退休承载，提交后不能再因编码/容器分配失败改发“未发生”的错误。

librpc 的本地取消/超时不会发送一个全局事务取消消息。provider 通过真实回复权的 CLOSED 观察和 Header Deadline 判断是否放弃未提交准备；最终提交前检查已经观察到的关闭与期限。关闭与提交竞态仍可能已提交，调用者只凭超时/取消不能断言未发生。提交后 Outbox 失败不回滚；无恢复控制的变更返回未知结果，有独立控制的操作按该控制 Query/Cancel。任何重试都必须基于本协议明确未提交的 Boundary/拒绝，或其自身幂等控制合同。

合法 typed 非映射 handle 的关闭按 libkernel 既有必成合同处理；对映射、来源登记和其它允许可恢复失败的操作，错误值须保留完整 owner 并进入已有 Runtime 的退休推进。运行实例停止先封新准入，再收束任务、控制、来源及退休；进程异常退出沿内核对象链处理，不承诺死去的 provider 还能答复终态查询。

### 路径接续细则

单次线上路径为 UTF-8 相对路径，空串表示入口根；`/` 分隔，不含 NUL、空段、`.`、`..`，不以 `/` 起止。名字为一个非空分量。`*`、`?`、反斜杠没有通配或转义作用，不沿用旧 FAL 对这些普通字符的额外禁令。长度取决于本次完整报文的可用字节，不再额外规定 512 字节上限。库不在解码时访问对象或推断领域类型。

Boundary 只表达下一步如何继续发现，保留两种公共形式。ProviderBoundary 对应旧 Delegate；LinkBoundary 对应旧 Link，推广为通用路径转发。两者均由 OAL 解释，具体 provider 决定何时返回；不根据 fs、播放器或其它领域标记分派第三方边界。以下字段和覆盖规则是本轮线形合同；错误偏移、非分量边界或不匹配后缀均为损坏回复，不能触发接续。

| 形式 | 字段与能力 | 使用规则 |
|---|---|---|
| ProviderBoundary | `consumed_bytes:u32, remaining:Text`，回复 slot 0 为新入口 | consumed 截止在原路径分量末尾且非零，前缀和 remaining 连续覆盖原路径；目标从已交付入口的授权根处理 remaining。空 remaining 合法；空路径不能继续换入口 |
| LinkBoundary | `parent_bytes:u32, target:Text, remaining:Text`，无能力 | parent 为转发分量父前缀，parent 与 remaining 之间必须恰好覆盖原路径的一个非空分量及相应分隔符；remaining 为原路径精确后缀；相对 target 从该父位置展开，视图绝对 target 从客户端视图根重启，再拼接 remaining。Lookup 中的 LinkBoundary 用于中途接续，末项返回 ObjectInfo；具体领域操作的转发按其协议决定。`.`/`..` 由客户端路径上下文处理 |

OAL 的客户端 walk 持有 prefix、入口、相对/绝对转发、`.`/`..` 和跨入口返回上下文。空或含 NUL 的 target 非法，target 可包含 `/`、`.`、`..`；所有偏移必须是 UTF-8 与分量边界。消费者无需引入 fs 才能跟随 LinkBoundary。查询 `Latest-Releases/Song` 时，播放器可返回 parent 为空、target=`Lady-Gaga-2026`、remaining=`Song`，客户端在播放器入口根下继续；直接 Lookup `Latest-Releases` 则返回专辑/目录 ObjectInfo。末项符号链接同样返回 ObjectInfo，不引入观察 Boundary 本体的对象接口。转发与换入口可交替，循环由已有请求/展开/期限预算终止。

fs 中所有单目标路径操作的响应明确包含 `Boundary` 分支，且只可在尚未创建、修改、建立流/订阅或交付操作结果时产生。ReadAt 的 Boundary 也不能夹带已读取的数据。provider 的 handler 使用同一 backing 遍历接口，在本次请求里得到本地目标或边界；本地目标直接执行，不再重做一次外部 Lookup。fs 客户端收到明确 Boundary 后，以新 txid 向目标发送同一领域操作及改写后的路径；已投递后的超时、损坏回复和失联不等价于 Boundary，不自动重放变更。

带输入能力的 Rename 采用现有“两份真实目录授权”结构：客户端先用 **fs DeriveDirectory** 取得目标父目录 grant，再向源入口发送源路径、目标单个名字及该 grant 的临时副本。源 provider 在一次提交中校验两个目录及本次源项；跨原子域返回 CrossFilesystem。这不是 OAL 定位前置，也不宣称两次调用之间目标父目录名字不变：派生 grant 明确选择了目录，之后按该目录操作。源路径遇边界可接续；每次运输使用调用者保留 grant 的临时副本，旧请求负责清理收到的副本，不能将唯一输入移走后盲目重发。

两种 Boundary 不附带由客户端执行的授权 mask；交付的真实入口必须已经满足出口限制。跨领域装配采用 Namespace 及 provider 自己兑现的出口政策；本案文件路由的具体签发与管理见下一节，不再保留一个未定义的通用 Bind API。

### 文件 provider 的路由装配

通用前缀/入口数据和 Boundary 接续归 liboal；下列 Bind/Unbind 则是本案文件 provider 的可选管理协议，拥有 FsRights 衰减，定义在 libfs。不能把它当作所有 OAL provider 的统一授权 API。跨领域树由客户端 Namespace 直接组合；registry 不实现文件 Bind，也不为它伪造 fs DeriveDirectory。

文件目录成员只有一份索引，成员指向本地 NodeRef 或 Binding；Binding 保存目标发行者入口、按出口上限预签发的共享入口、FsRights/运输上限及付款/退休 owner。它不是持久 ObjectInfo 或第二棵 OAL 树。枚举 Binding 返回名字且 info 缺省；Lookup 包含末项时均换到真实目标入口，最后由目标产生摘要。Create 遇已占用绑定名返回 Exists；Remove/Rename 的末项若为 Binding 返回 Busy，管理员显式 Unbind 才删除装配关系。

管理入口与普通文件 grant 分开铸造，context 绑定本地管理根、允许出口上限和可信账户。仅从管理 sender 收到请求才有权限；切换消息 kind 不升权。Bind/Unbind 只解析本地父目录，禁止跨 Boundary 或链接转发管理操作；空路径无效。父目录在异步准备期间由实际引用保活，提交时复核管理授权和当前名字条件。

Bind 请求提供 `path:Text, max_rights:u32, output_transport:u64, replace:u32`，业务槽 1 是目标目录的发行者 sender，该授权必须支持 fs DeriveDirectory。准备阶段通过目标 `DeriveDirectory("", max_rights, stored_sender_transport, output_transport)` 验证范围并取得共享出口；后两个参数均为完整 u64 Rights，stored_sender_transport 至少为 WRITE|WAIT|TRANSIT|DUPLICATE，且同样受目标发行者的出口上限约束。任何准备失败不安装绑定，已产生的 grant/回复 owner 沿原 RPC 清理。replace=0 要求空位，replace=1 可创建或替换现有 Binding，但不能遮盖本地文件节点。一次提交修改父目录的成员索引及 Watch，再退休旧 Binding；回复丢失为结果未知，不自动回滚/重放。

跨 Binding 时，文件 provider 先校验本地遍历及本次操作授权，再计算 `effective = source_grant.rights ∩ binding.max_rights`，运输出口同样求交。若共享出口的实际领域/运输限制已足够窄，可直接复制；否则由 Binding 持有的真实发行者入口执行 DeriveDirectory 签发更窄入口，然后返回 Boundary。不得把完整上限入口与客户端自律的 rights 一起发走。内部派生沿原请求剩余期限、账户和有限预算；回来后复核原 grant 与 Binding 仍有效，失败则清理结果。普通接续不统一要求每次新派生；两端衰减需求相同的调用复用共享出口。

Unbind 请求只有 path，缺失返回 NotFound，本地文件节点返回 TypeMismatch；成功只移除对应 Binding。两操作成功体为空，不返回 Boundary、不含业务能力；op=1 Bind、2 Unbind，头、status 和期限格式同下文，协议 kind 使用文件宿主管理值。客户端保留输入发行者入口，只运输临时副本。解除/替换绑定不召回先前已交付入口，远端文件会话仍由发行者管理。绑定的两份真实入口、来源和可变存储在释放后退款，关闭失败仍由文件 provider 退休队列持有。

每次调用创建 `FollowBudget { calls, expanded_bytes, deadline }`。客户端保存宿主提供的非零调用/内存额度和有限超时政策，方法开始时结合调用者期限建立本次总 deadline，不把一个绝对期限永久复用到所有调用。首次 RPC、初始规范化及后续展开均先扣相应调用/内存预算；Rename/Copy 的内部阶段共用原调用剩余额度与期限，不逐阶段重置。不复制旧 40/256 魔数，耗尽返回本地 BudgetExceeded 并清理临时入口。每个提供端请求受完整报文长度限制；远端派生/内部转发另有有限任务预算和下游 owner。没有跨进程全局路径锁。

### 线形与操作表

以下是目标线形，不是旧源码已经实现的 ABI。每个协议拥有自己的 Header/Request/Response；共享的是 librpc 的 LE 字节工具、RpcPrefix，以及 liboal 的摘要/Boundary 数据类型。相同头布局不引入统一业务 dispatcher 或通用 PathHeader。

| 协议 | 消息 kind / 版本 | 接收入口 |
|---|---|---|
| OAL | `0x4f41_4c31` / 1 | 实际 provider 暴露发现的入口 |
| fs | `0x4653_3031` / 1 | 文件目录 grant、流/订阅控制入口 |
| service | `0x5352_5632_5245_4701` / 2 | 独立 registry 的目录访问、发布 authority、注册/订阅控制入口 |
| 文件宿主管理 | `0x4653_3031_5254_4501` / 1 | 独立管理 sender；本篇定义的 Bind/Unbind |

文件 provider 的 mailbox 接受 OAL/fs，registry 的独立 mailbox 接受 OAL/service；各自从内核 sender context 查询真实角色与访问范围。换消息 kind 不产生额外权限。二者可由同一 Runtime 调度，但 registry 的目录访问、发布和控制记录不进入文件 GrantTable，不按文件 NodeRef 分派。这里只复用现有消息运输。

文件管理协议仅由文件 provider 的独立管理角色处理，Bind/Unbind 的 wire 已在路由装配节定义；它不是普通文件入口的隐含权限。服务目录使用客户端 Namespace.bind：srv_init 分别交付真实文件入口和 registry 目录入口，消费者绑定 `/System/Services`；文件宿主不代理 service/未知领域请求。

RpcPrefix 保持 16 字节；随后各协议头均为 `version:u16, op:u16, status:u32, body_len:u32, reserved:u32=0, Deadline:16`，共 32 字节。请求 status=0；回复回显 op、txid 和请求期限。op 先按原始 u16 读取，以便对未知操作返回 Unsupported。请求 slot 0 固定为 librpc 回复权；下表请求业务槽从 1 开始，回复槽从 0 开始。不使用的槽必须为空，错误回复不带能力。

本文使用 `Text/Bytes = len:u32 + len 个字节`，无隐含对齐；Text 必须合法 UTF-8。`OptionalU64 = present:u32 + value:u64`，present 只能是 0/1，缺省时 value=0。Deadline 直接使用 shared 的既有 16 字节编码及合法性校验。所有加法、乘法、偏移和 u64→usize 转换做溢出/范围检查。

ObjectInfo 的线形是 `trait_count:u32 + trait_count 个 MarkerId + tag_count:u32 + tag_count 个 (MarkerId, value:Bytes)`。MarkerId 为 `namespace:u128, local:u32`，均 LE，无对齐；Trait 和 tag 使用各自键空间。未知数字和未知 tag 内容按长度保留或跳过；计数/长度溢出、重复 Trait、重复 tag 键、尾部残留或截断拒绝为 Invalid。排序没有语义，禁止的是同一键出现两次，不是任何领域标记组合。ObjectInfo 及 tag 内均不携带 capability。

| namespace（128 位固定数字） | Trait local | tag local / 值编码 |
|---|---|---|
| OAL：`0x48414c43594f4e00000000004f414c00` | 1 Directory | 首轮没有必需 tag |
| fs：`0x48414c43594f4e000000000046530000` | 1 RegularFile，2 Property，3 Stream，4 SymbolicLink，5 Boolean，6 Integer，7 Float，8 String，9 Blob，10 Array | 1 ByteLength：单个 LE u64；适用于流的逻辑字节长度，不把 Property 编码大小当相同含义 |
| service：`0x48414c43594f4e000000000053525600` | 1 Entry | 1 Registration：16 字节 RegistrationId；2 Protocols：下文 ProtocolDescriptor 列表 |

数字常量分别定义在所属库，OAL 不匹配 fs/service 的 namespace。Integer 包括 Signed64/Unsigned64，正负和范围由实际值编码决定；Array 与相应标量 Trait 组合，仅描述面貌。MIME、播放器及后续领域使用自己的稳定 namespace/local，不修改 OAL 的闭合枚举；不为尚未实现的领域预建空库或登记运行时插件。

本案 MemoryBackend 的具体投影固定为：Directory → OAL Directory；Property → fs RegularFile/Property/对应标量标记，数组再加 Array；Stream → fs RegularFile/Stream 和 ByteLength；SymbolicLink → fs SymbolicLink。不把属性内容或链接正文塞进 tag，不输出权限作为能力证明。四类最大 info 长度分别为 28、88、80、28 字节；Binding 的枚举 info 长度为 0。这是该 backing 的投影规则，不是 OAL 的类型组合校验。registry 根只有 Directory，条目为 Entry + Registration/Protocols 两个 tag。

面貌示例：目录可带 OAL Directory；内存属性可带 fs RegularFile/Property 及其值形状标记；歌曲可同时带 fs Stream、mime::Mp3 和 music_control 标记。它们只是 provider 输出视图的示例，不是 OAL 的 backing 类型映射表；同一对象可支持多个领域协议/操作，但同一个 Get 不按未发送的客户端 T 选择返回形状。服务入口等真实能力通过对应领域操作取得，面貌本身不产生能力。

`ProtocolDescriptor = id:u64, min_version:u16, max_version:u16, addressing:u32`，addressing=0 为无路径领域入口、1 为入口根相对寻址；列表为非零 count:u32 加固定 16 字节条目，同 id 不重复，0 < min_version ≤ max_version，未知 addressing 拒绝。它是发布描述，不是 OAL Trait 的接口证明。Register 冻结描述，GetEntry 返回同一注册完整描述；客户端按自己所知协议选择版本交集，registry 不接受返回值期望或执行业务协商。

| OAL op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 Lookup | path:Text | result:u32，0=ObjectInfo、1=ProviderBoundary、2=LinkBoundary；仅 ProviderBoundary 回复槽 0 |
| 2 Enumerate | path:Text, cursor:Bytes, max_entries:u32 | result:u32；0=Page，1/2=Boundary。Page 为 next_cursor:Bytes, done:u32, count:u32，随后重复 name:Text + info:Bytes；info 长度 0 表示此名字是边界项、摘要需另行 Lookup，非零是完整 ObjectInfo。Page 无业务能力，ProviderBoundary 带回复槽 0 |

枚举分初次与续页：初次 cursor 为空、path 指定目录；续页 cursor 非空且 path 必须为空，由本次真实接收入口解释游标中的目录位置，禁止同时传非空 path/cursor。OAL 不保存扫描会话。本案两个 backing 均按名字 UTF-8 字节序输出，区分大小写、不做 Unicode 规范化。文件 cursor 为 `{provider_incarnation:u64, directory_serial:u64, after_name:Text}`；incarnation 取文件 mailbox 内核对象 ID，目录 serial 不复用。续页按 serial 选择原目录，并重新核对它仍存活、仍在本次 grant 根的可遍历范围内且具有 Enumerate 权限；祖先遍历校验与普通路径遵守同一节点 ceiling/授权规则，不把数字 ID 当授权。目录在授权范围内改名仍可续页，退休、移出范围或无法确认可遍历范围返回 CursorInvalid；入口整体撤销/失权仍为 Revoked/Permission。新目录不会复用旧 serial。registry cursor 为 `{registry_incarnation:u64, after_name:Text}`，按当前 Root/ExactName 范围筛选；跨实例 cursor 返回 CursorInvalid。

静态目录分页完整且无重复；并发插入在游标前的名字可被本轮遍历遗漏，删除/替换按本页观察，不承诺快照或全局 epoch。`max_entries=0` 无效，done 只取 0/1，done=1 时 next_cursor 为空；done=0 必须有至少一项及前进后的非空 cursor。容量或 max_entries 达界时只输出整项。首项装不下返回 EntryTooLarge，不返回永远无进展的空页；单次 Lookup 摘要装不下返回 SummaryTooLarge。客户端只在确需边界项摘要时另发 Lookup，该调用使用自己的预算，不由一次 Enumerate 隐式扇出。

EnumerationCursor 私有持有实际应答入口及 provider opaque cursor；续页向该入口发送空 path、该 cursor 和 max_entries，不重发长原路径，也不重新选择已经修改的 Namespace 绑定。它只服务分页，不是普通操作的 Position 前置、不产生目录授权或固定内容；首次请求没有能力导出以外的新 Derive 要求。续页返回 Boundary 是协议错误，客户端不携旧 cursor 跟随；位置失效应由原 provider 返回 CursorInvalid。重新开始遍历才重新解析用户路径。cursor 释放关闭其入口引用，本案 provider 不为它建立扫描会话或保活目标目录。

| fs op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 Get、2 Set | path:Text；Set 追加 value:Bytes | Get 返回 value:Bytes；Set 为空 |
| 3 ReadAt、4 WriteAt | path:Text, offset:u64；ReadAt 追加 count:u32，WriteAt 追加 data:Bytes | `IoResult { count:u64, end:u32, cause:u32, data:Bytes }`；只有 ReadAt 含 data |
| 5 Open、6 CreateOpen | path:Text, direction:u32, offset:u64, length:OptionalU64, session_deadline:Deadline, tunnel_bytes:u32；CreateOpen 再加初始 FsRights:u32 | StreamOffer；回复槽 0=控制 sender、1=Runnel invitation |
| 7 Create | path:Text, kind:u32, rights:u32, initial:Bytes | 空；kind=1 目录、2 Property、3 Stream、4 SymbolicLink。目录/Stream 的 initial 为空，Property 是值编码，链接是 UTF-8 target |
| 8 Remove、10 ReadLink、12 Sync | path:Text | Remove/Sync 空；ReadLink 为 target:Text |
| 9 Rename | source:Text, destination_name:Text, replace:u32 | 空；请求槽 1=目标父目录 grant；replace 只取 0/1 |
| 11 SetLength | path:Text, length:u64 | 空 |
| 13 DeriveDirectory | path:Text, rights:u32, sender_transport:u64, output_transport:u64 | 空；回复槽 0=实际收窄后的目录 grant |
| 14 Watch | path:Text, mask:u64 | WatchInfo；回复槽 0=独立控制 sender、1=通知 waiter |
| 15 Start、16 QueryStream、17 Finish、18 Cancel | 空，直接发到流控制入口 | StreamInfo，不含能力 |
| 19 QueryWatch、21 CancelWatch | 空，直接发到订阅控制入口 | WatchInfo |
| 20 AckWatch | sequence:u64 | WatchInfo |

fs 路径操作成功体前均有 result:u32，0=表中结果，1/2=Boundary；控制操作没有该字段。Create/Remove/Rename/ReadLink 不跟随末项链接，其余路径操作跟随；创建操作只解析父路径。CreateOpen 固定创建空 Stream 并以 Write 方向打开，不允许借这个入口创建只读流；结果丢失语义见下文。Rename replace=1 可替换兼容种类，目录必须为空，目录不能移入自身子树；不兼容类型返回 TypeMismatch。同一目录同一名字是成功的无操作。

Open 的 direction=1 Read/2 Write，offset 是绝对文件偏移。Read 的末端为 `max(offset, min(Open 时文件长度, 可选的 offset+length 上界))`，offset 超过 EOF 时是空读流；Write 初版要求 length 缺省，按 EOF 结束，CreateOpen 固定 offset=0。tunnel_bytes=0 采用宿主当前 Runnel 配置，非零值须满足既有 Runnel 布局及额度；这不是文件数据的最大长度。有限 offer_deadline 取请求剩余期限、宿主 offer 配置期限及有限 session_deadline 的最早者。每个 provider 可以不支持某种 Open 形态并明确返回 Unsupported，不能成功回复后悄悄改变所承诺的方向/范围。

Property 编码为 `scalar:u32, array:u32, count:u32, payload`；scalar=1 Bool、2 Signed64、3 Unsigned64、4 Float64、5 String、6 Blob，array 只取 0/1，标量 count 必须为 1。数值逐项定宽 LE，Bool 每项一字节且仅 0/1；String/Blob 每项是 Text/Bytes。空数组保留 scalar 标识。Float64 保留 IEEE 位模式；`usize` 的转换属于 libfs，负数和越界报 Range，不把平台 usize 写上线形。Get 不要求请求方提供先前摘要。

StreamOffer 精确为 `session_id:u64, direction:u32, tunnel_bytes:u32, start:u64, read_end:u64, offer_deadline:Deadline`，共 48 字节。session_id 取本次控制 sender 的 context ID，非零，仅关联会话；Read 要求 read_end≥start，Write 的 read_end=0。StreamInfo 为 `session_id:u64, state:u32, outcome:u32, reason:u32, reserved:u32=0, accepted:u64, transported:u64`，共 40 字节。state 为 Offered=1、Active=2、Terminal=3；reason 为 None=0、Completed=1、Cancelled=2、Expired=3、PeerClosed=4、Backend=5、ProviderStopped=6、Revoked=7。非 Terminal 必须 outcome=Ok/reason=None；终态 Completed 对应 Ok，Cancelled/Expired/PeerClosed/ProviderStopped/Revoked 分别对应 Cancelled/Expired/Unavailable/Unavailable/Revoked，Backend 对应实际非 Ok fs status。内部 Stopping 不新增 wire 状态。

Runnel 仅采用当前 v2，不携带无选择的 stream_protocol 字段。WatchInfo 为 `sequence:u64, pending:u64, state:u32, reason:u32`，state=1 Active/2 Terminal；Ack 未来序号 Invalid，旧序号只返回当前状态。fs 的事件位为 Create=0、Delete=1、Rename=2、Modify=3、Unbound=4；service 为 Registered=0、Withdrawn=1、Replaced=2；两者 Initial=62、Terminal=63。fs 建立请求的 mask 只允许业务位，Initial/Terminal 自动包含。service 的 WatchDirectory/WatchName 初版关注全部业务位，没有额外 mask 字段。service 同名 Replace 是一次 Replaced 变化；fs 目录 Watch 仅监视直接名字变化，不递归监视子文件内容。

| service op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 DelegateName | name:Text, publish_rights:u32 | 空；回复槽 0=限定名字的发布 authority |
| 2 Register | name:Text, mode:u32, descriptors:Bytes, export_transport:u64；Header Deadline 必须有限 | RegistrationInfo（提交快照）；请求槽 1=已允许共享的 endpoint，回复槽 0=RegistrationControl |
| 3 GetEntry | path:Text | result:u32；0=`RegistrationId + descriptors:Bytes`、1/2=Boundary；成功取得或 ProviderBoundary 的回复槽 0=实际入口 |
| 4 Withdraw、5 QueryRegistration | 空，发到注册控制入口 | 当前 RegistrationInfo，不带能力 |
| 6 WatchDirectory、7 WatchName | directory_path:Text；WatchName 追加 name:Text | result:u32 + WatchInfo 或 Boundary；建立成功槽 0=控制、1=通知 waiter，ProviderBoundary 仅槽 0 |
| 8 QueryWatch、10 CancelWatch；9 AckWatch | 与既定 Query/Ack/Cancel 体相同，直接发到订阅控制入口 | WatchInfo；编码及语义由 service 拥有，不依赖 libfs |

`RegistrationId = {registry_incarnation:u64, serial:u64}`，前者取该 registry mailbox 的内核对象 ID，后者由 provider 单调分配、不回绕；耗尽只拒绝新注册，不妨碍撤出旧注册。它用于描述和区分注册，不是授权。`RegistrationInfo = RegistrationId + state:u32 + reason:u32`，实际控制身份来自不可伪造 sender context。Register mode=0 CreateOnly、1 Replace；每次提交创建新注册，控制不支持再 Publish 或重新激活 Withdrawn。一个注册持一个可复制 MailboxSender，该入口可接受多个协议；控制只属于目录注册，与业务会话独立。Register 回复表示提交时的 Active 快照，回复到达前可能已经被替换，须用该控制 Query 观察后续状态。

三个领域协议及文件管理各自固定 status：0 Ok，1 NotFound，2 NotDirectory，3 Permission，4 Invalid，5 Unsupported，6 Exists，7 NotEmpty，8 Revoked，9 Conflict，10 Resource，11 Quota，12 Busy，13 Cancelled，14 Internal，15 CrossFilesystem，16 TypeMismatch，17 Range，18 Expired，19 CursorInvalid，20 SummaryTooLarge，21 EntryTooLarge，22 ValueTooLarge，23 Unavailable。各自仅产生适用项，枚举归各自协议，librpc 不拥有业务错误。已识别 kind 的未知 op/版本不执行；头和回复权可信时以本协议支持的版本返回 Unsupported，op/txid 回显。无法安全解码头、能力错型或完全未知的 message kind 按既有 RPC 清理拒收，不猜测陌生领域的回复格式。客户端将未知版本/status、损坏回复或运输失联作为明确的调用失败，不据此重放变更。IoResult 的 cause 使用 fs status，end=0 可继续、1 EOF、2 Error；短 I/O 可同时携带后续错误，不能用空错误体抹掉已提交数量。

每个领域体容量为 `B = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN`，当前 4048；只引用共享常量，不在新库复制 4096/8。若有 T 个 Trait 和各值长 V 的 tag，`ObjectInfo_len = 8 + 20*T + Σ(24+V)`；Lookup Found 为 `4+ObjectInfo_len`。任意长度/计数先做 checked 运算，确认所需字段能由剩余正文提供，再分配；不能按未验证的 count 预分配。完整解码后才触及 backing，没有隐式结构体 padding，bool/枚举/保留位按各自定义校验。

分页逐项计入 result、cursor、done、count、名字和 info 的全部前缀。MemoryBackend 的共同成员准入维护 `44 + 2*name_len + max_info_len ≤ B`，所有引入或改变名字的 Create、CreateOpen、Bind、Rename 都必须使用同一个计算；Rename 用源节点种类所有允许形状的 max_info_len 检查目标名字，Property 按数组等最大形状计入，Bind 的 info 为 0。超限在提交前返回 EntryTooLarge，不修改旧名字、目标或文件；不能仅在构造枚举回复时才发现本地合法变更破坏该条件。registry 对应为 `36 + 2*name_len + info_len ≤ B`。还须满足各操作完整请求和 Lookup 容量，约束均由协议推导，不另设 PATH_MAX。

任何 provider 输出非终页前还须保证 `12 + next_cursor_len ≤ B`，使空 path 的下一次 Enumerate 请求可以编码。文件 cursor 长为 `20+name_len`，续页请求为 `32+name_len`；registry 对应 `24+name_len`，由本案上述成员准入保证。初次长路径不再重复占用续页正文。外部/动态 provider 无法给出可表达摘要、条目或可用游标时按 SummaryTooLarge/EntryTooLarge 返回，不截断或输出无法继续的非终页。

Register 预检请求、Lookup/单项续页以及 GetEntry 的 `24+descriptors_len≤B`；后者与 RegistrationId/描述/能力必须匹配。Set/Create 的完整 Property 必须能经 Get 返回，超出 ValueTooLarge。ReadAt 每次最多返回 `min(requested_count, B-24)` 字节，剩余由调用者继续；WriteAt 的数据受完整路径/偏移/Bytes 请求容量约束。偏移加长度、大小转 usize 和各流累计计数均 checked；不能回绕，数据操作中遇上界按 IoResult/终态保留此前已完成数量。

协议表中未使用的能力槽必须为空，错误回复不带业务能力。OAL 页面没有业务能力；ProviderBoundary 恰好 1 个。fs Open/CreateOpen 与 Watch 的成功结果恰好 2 个，其余按表。service Register 业务输入仅 slot 1，GetEntry 成功恰好 1 个，Watch 成功恰好 2 个。先按结果变体校验角色/数量，再把临时 capability 转成相应 typed owner；损坏帧沿 RPC 清理全部收到的能力。

运输字段直接使用 shared 的 `Rights(u64)`，sender_transport/output_transport/export_transport 在线上均为 LE u64，不截断成 u32；FsRights 与服务访问/发布位仍为 u32。未知 Rights 位返回 Invalid。output_transport 是该授权向外交付能力的完整 Rights 上限，不是旧实现仅含 TRANSIT|GRANT 的附加转授位。实际输出按本操作的角色和明示请求/交集计算，必须满足该角色的必需位并处于上限内；不相容时提交前 Permission，不默补被上限禁止的业务位。

可调用 sender 的必需位为 WRITE|WAIT|TRANSIT；无显式运输位参数的新建 sender（发布 authority 或流/注册/订阅控制）默认再取上限允许的 DUPLICATE。通知 waiter 的必需位为 WAIT|TRANSIT，同样仅在上限允许时带 DUPLICATE；Runnel invitation 使用 librunnel v2 的既有角色/权限合同。GRANT 不自动附加，只能按操作的显式运输请求交付且仍受上限约束。内部需重复导出的母本持 DUPLICATE，不等于接收方必然获得该位。控制 sender 的 context 必须绑定新建的控制记录，不能复制普通目录 sender 充当控制。

DeriveDirectory 的 sender_transport 是新入口自身的完整权限，也是一次输出，必须是母 grant 的 output_transport 子集；新 grant 的 output_transport 也必须是母上限子集。二者分开验证，不以“出口上限合法”跳过入口自身权限检查，显式请求的位超限不静默削减。Bind 准备共享出口和后续按需派生遵守相同规则；source 与 Binding 出口交集按完整 mask 计算。service export_transport、发布 authority 与 DirectoryAccess 的上限也均为完整 mask；GetEntry 的副本权限取 export_transport 与访问入口上限的交集，仍须含 WRITE|WAIT|TRANSIT，否则 Permission。

运输字段加宽后的完整请求体长度分别为：DeriveDirectory `24+path_len`，Bind `20+path_len`，Register `20+name_len+descriptors_len`；均以 B 检查。它们不改变分页游标或 D-F1/D-F2 公式。旧 Issuance::validate_policy 的 TRANSIT|GRANT-only 检查、srv_fs/srv_init 的对应装配参数和 host 用例须同次迁移；新上限按实际输出角色所需位与明确允许的可选位配置，不能为了通过检查无条件改成 Rights::ALL，不保留旧语义兼容分支。

### 实际授权与资源责任

OAL 没有共同 ACL、账户分类或 grant 表。接收者从真实 sender context 找到自己已签发的授权，再决定能否产生摘要、边界或执行领域方法。下面的表是当前两个 backing 的实现设计；采用这些支持类型的进程拥有其实例。

| 实际类型 / 索引 | 拥有的事实与授权 | 付款与释放 |
|---|---|---|
| 文件 `MemoryBackend` / `NodeStore` | 目录成员、值/数据、内部 NodeRef、版本和名字变更；保留实际 backing，不为 OAL 建副本 | 文件创建者所用 grant 的可信账户支付 Node/Bytes；名字解绑只减命名引用，最后使用引用释放后分批退休 |
| 文件 `GrantTable` / `GrantState` | context→root、FsRights、输出运输上限、账户、Active/Revoked | 迁移现有 Grant/WaitSource/Bytes charge 和槽位 Permit。派生继承母 grant 账户；revoke 立即拒绝新使用，不提前退款，最后 sender 的 Lifetime CLOSED 加来源撤销后移除 |
| 文件 `StreamEntry` | 控制 context、选定 NodeRef、方向/范围、计数、期限、Runnel 角色及待清理 owner | Open 所用 grant 的账户承担 Offer/Stream/Bytes/WaitSource；建立成功后由 entry 接管，物理映射/来源释放后相应退款 |
| 文件/服务各自 `Subscription` | 独立控制 context、观察目标、sequence/pending、通知及来源；只索引自己的领域目标 | 建立时入口绑定的账户付 Watch/Bytes/WaitSource；终态释放观察目标，剩余控制记录仍付费，最后控制副本关闭才回收记录 |
| 服务 `DirectoryAccess` | context→Root 或 ExactName 范围、Describe/Enumerate/GetEntry/Watch 权利、输出运输上限、可信账户 | registry 自己的有界访问表；装配签发并绑定 ServiceResource 账户，Lifetime 管理条目。没有文件根 NodeRef/FsRights |
| 服务 `NameAuthority` | context→Root/ExactName、Create/Replace/Delegate 发布权限和账户 | 承接 AuthorityTable；子 authority 权限不超父项。关闭父别名不撤销已交付的子控制；明确撤销只禁止该 authority 后续注册 |
| 服务 `Registration` / `names` | Registration 拥有名字/描述、endpoint、授权快照、控制/入口观察及退休责任；names 仅索引 Active 注册 | 发布 authority 账户支付 Registration、Bytes 和来源；索引引用同一注册，不存 ServiceRecord/NodeStore。提交前预留名字和退休承载 |
| 请求、Outbox、下游请求 | 收到的输入、回复权、临时能力、编码缓冲、尚未交付结果 | 对应入口的可信账户承担领域 Request/Outbox/Bytes；执行任务另用同一付款身份绑定的 libexecution 视图。Outbox/ReplyCleanup 原 owner 负责失败重试 |

`FsRights` 位固定为 Traverse=0、Enumerate=1、ReadProperty=2、WriteProperty=3、ReadStream=4、WriteStream=5、Create=6、Remove=7、Watch=8、Describe=9、Derive=10（均表示 `1 << bit`）。未知位拒绝，底层运输 Rights 使用 shared 真值。Lookup 需 Traverse/Describe；枚举需 Traverse/Enumerate，条目摘要属于枚举结果，不额外要求 Describe。Get/Set、ReadAt/WriteAt、Read/Write Open 使用各自读写位；ReadLink 使用 ReadProperty；SetLength 使用 WriteStream；Sync 对流使用 WriteStream、对 Property 使用 WriteProperty。Create/CreateOpen 检查父目录 Create，CreateOpen 同时检查新 Stream 的写权限。Remove 检查父目录 Remove；Rename 检查源父 Remove、目标父 Create，替换还需目标父 Remove。DeriveDirectory 要求目标目录 Derive、范围位于母 root 内，权限/运输出口均不得扩大。

文件 grant 表示调用者权限，`MemoryNode.ceiling` 表示对象自身允许的操作，实际操作按二者求交；中间目录逐项检查 Traverse，末项目标或父目录按上表检查所需位。Create 的 rights 字段就是新节点 ceiling，不是 capability 授予，也不表示创建者获得这些位；它可包含创建者当前 grant 没有的已知位，因为当前调用者仍受 grant 限制。新 ceiling 固定到节点，不提供本轮未设计的 chmod/通用 tag 写入。DeriveDirectory 的新权限不超过父 grant 与所选目录 ceiling，派生本身须被二者允许；ObjectInfo 不携带可替代真实授权的 rights。

目录 ceiling 同时限制从该目录签发的子树授权，ReadProperty/ReadStream 等位在目录上不表示它本身是 Property/Stream；创建可供派生使用的目录时须按预期子树权限设置这些位。MemoryBackend 维护每个目录的唯一弱父关系，成员插入/移除/Rename 在同一提交中更新它，用于 Rename 防环与续页授权范围检查；根无父，不支持目录硬链接。弱父不保活祖先，父项失效不能被当成验证通过。目录引用与内部 serial 仍属于真实 NodeStore，不扩展成 OAL 对象身份或扫描会话。

调用准入取得真实授权记录的使用引用；最后 sender 消散只终止该入口的新请求，已准入请求、已派生授权和独立控制继续按自身寿命结束。显式 revoke 则令尚未提交请求在最终复核时失败，并停止由该授权建立的活动流/订阅，不回滚已提交结果。已派生子 grant 是独立授权，不因父项别名关闭或单项 revoke 自动失效；批量撤销须由发行者明确遍历其管理范围，不能假称已有递归 revoke。保留的授权/撤销记录在请求和控制引用全部释放后收费回收。

服务 DirectoryAccess 的 Describe/Enumerate/GetEntry/Watch 与发布 authority 的 Create/Replace/Delegate 分开。根发布权可 DelegateName，ExactName 不再扩大范围；CreateOnly 需 Create，Replace 需 Create|Replace（含空位创建），Register 在最终提交时重新检查 scope、权利与名字条件。已交付控制不依赖父 authority 的某个别名持续存活。Register 校验母本是可重复共享的 MailboxSender，实际运输权含 WRITE/WAIT/TRANSIT/DUPLICATE；export_transport 至少含 WRITE/WAIT/TRANSIT，且不超出母本与发布 authority 上限。GetEntry 还受目录访问入口的输出上限限制，不自动附加 GRANT，不改变领域权限；描述是否真实由发行者负责。共享副本沿用原 context/账户，不伪称逐客户端隔离或撤销。控制和订阅操作只按自己的 sender context 认领资源，正文 RegistrationId 不授予控制。

服务访问位按 Describe=0、Enumerate=1、GetEntry=2、Watch=3，发布位按 Create=0、Replace=1、Delegate=2 分配；未知位拒绝。DirectoryAccess 的 Root/ExactName 是发行者在装配时签发的独立访问角色，根的 Lookup/Enumerate 只反映其可见范围；ExactName 越界请求 Permission，无需泄露对应名字是否存在。GetEntry 对目录根返回 TypeMismatch，对多分量服务子路径返回 NotDirectory。WatchDirectory 监视当前访问范围，WatchName 必须在该范围内且名字可尚不存在。NameAuthority 的 DelegateName 仅允许 Root + Delegate，返回指定单分量的 ExactName，权限和出口上限不超父项；子 authority 和已交付注册控制独立存活。没有从 Describe 或 GetEntry 自动获取发布权限的路径。

Rename 的同域判定先从输入能力的可信 `related_object_id` 确认 mailbox，再用真实 sender `object_id` 查本地目录授权及实际 backing；请求正文或 badge 不替代这个校验。同域但角色错误/撤销分别返回 Permission/Revoked，非同域返回 CrossFilesystem。同一目录同一名字成功无操作；目录只可替换空目录，非目录之间可以替换，目录与非目录不兼容。源目录不能移入自身后代，父链检查最多访问该 backing 的 NodeLimit 个节点，宿主配置须把这一步成本计入单次工作上界。目录名解绑后只要还有有效目录授权仍保留对象和数据；继续在已解绑目录创建内容仍正常计费，最后使用引用释放后由 provider 退休，不把 namespace 可见性误当引用寿命。

首批计费不引入“请求传裸 account id”或新的付款 capability 协议。共享发现/目录入口下的请求、流和订阅继承该入口的可信账户，别名共用额度；装配需要隔离时签发不同账户绑定的入口。服务 registration 的 endpoint 预先由发行者收窄；registry 的 GetEntry 只产生一次运输副本，不在 provider 新建 grant。以后确需取得者支付时，由对应领域新增真实付款授权契约；当前不假称已经取得者付费。

费用按真实占用计算：固定记录分配字节、预留可变容量及索引/退休节点；可共享名字存储由一个实际 owner 计费，索引和 OAL 摘要不各存一份领域真值。ServiceResource 保留 Authority/Registration/Bytes/WaitSource，增加 Access/Watch 及必要请求/Outbox 准入；具体布局统一按实际 owner 计算，不能对同一存储重复收费。Runtime 的 Task/InputBytes/来源额度与 service 账户来自同一可信付款上下文。每注册在提交前至少准备控制 Lifetime、业务入口 CLOSED 两个来源及最终关闭承载；每 Watch 准备控制 Lifetime、通知对端关闭来源及唤醒承载。请求/回复来源依现有 RPC 生命周期另计。容量是宿主的真实预算，控制壳继续收费直到关闭，释放的 endpoint/来源分别退款。

预扣 quota 不等于已经预留内存。插入名字、安装控制、替换索引和登记退休的提交步骤不得分配：使用实际可失败的 reserve、预建索引节点或已付费槽位，在准备阶段取得所需容量。并发准备须各持可兑现的槽位/容量 permit，不能只因 try_reserve 成功就允许其它请求耗掉同一份空闲容量；提交仅消费自己的预留。没有 reserve 合同的容器不能凭“已扣 Bytes”宣称提交必成；可更换内部布局，但须同时计入保留容量与提交工作界。释放一条记录不自动退还仍被容器持有的容量费用。P2/P3 须在工作记录写明每个实际容器的预留与释放方式。

订阅和控制的保留可能由持有者延长，但始终计费且有全局/账户上界。grant、stream、watch、registration 的 cleanup 节点、必要的终态记录与唤醒承载均在公开前准备；提交后不得因再次申请容器失败而丢掉责任。准备失败关闭已经铸造的入口/通知/映射；close 或来源撤销失败时，完整 owner 留在原任务的退休状态继续推进，不在错误值被丢弃时泄漏。

并发沿现有 provider 执行方式：真实 backing 由本地单 owner/短可变借用串行访问；borrow 内完成选择、验证和提交，不持有它跨 IPC、映射或等待。RPC/流任务保存自己的准备对象和实际 NodeRef；异步准备回来后重新验证仍需成立的授权及当前名字条件，再提交。OAL 摘要只是这次借用生成的数据，不登记观察者。不同 provider 没有跨进程锁序；Rename 只在一个 backing 提交，远端派生先交出本地借用。

每次 poll 有显式工作预算：路径/值扫描受单个报文长度限制；流最多搬运缓冲可容纳的一个块；退休最多处理传入 budget 项。watch 状态标记最多扫描准入的 `W_limit` 条，内存容量及这一次扫描成本由宿主配置约束；提交前预留足够的 WakeBatch，标记在提交步骤完成，唤醒投递满时保留游标分批兑现。这样 Query 可立即看到变化，延迟的是通知投递。停止时先封准入，再让已提交 entry 和退休任务推进；临时任务消散或外部不再调用均不能删除清理责任。

源码复核的复用边界：当前 [Registry 退休](../user/libraries/libservice/src/registry.rs) 的 retire_registrations 在 endpoint.close 错误时保留母本但向上传播 Err，最终可到达 [runtime.turn 的 expect](../user/services/srv_fs/src/server.rs)；[Watch close_signaler](../user/libraries/libfal/src/provider.rs) 则丢弃返回的 owner，转入 typed Drop 的必成关闭边界。迁移不能把这两种写法当作“可恢复退休错误已有重试”的证据。目标任务对可恢复来源/映射/退休错误保留 owner、记录下一次唤醒并重试，轮转游标防止一项失败阻挡其它退休项；合法 typed 非映射 Handle 关闭的结构性必成约束仍以 libkernel 为准，不通过制造非法 handle 来伪造可恢复故障。

### 文件提交与完成状态

| 事件 | 提交/结果规则 | 未交付、取消与后续责任 |
|---|---|---|
| Get/Set | Get 拷出一次完整值；Set 准备新值后一次替换，旧值转退休 | Get 无活能力；Set 提交后回复丢失为结果未知，不撤回已经可见的值 |
| ReadAt/WriteAt | 返回 count 是本次实际读取/接受的数量；ReadAt data 长度必须等于 count。零请求返回零成功，EOF 明确 end=EOF；错误可带已完成前缀 | 本次内存实现先准入所需存储再提交一个有界块，分配失败 count=0；不将局部数据包装成完全失败 |
| SetLength | 在 backing 的一次提交改变长度；增长零填，缩短撤去超出部分的可见数据 | 活跃读流的上界不扩大，之后遇截断可提前 EOF；活跃写流仍按自己的 offset 提交，后续写可再次扩长。accepted 是历史接受量，不是当前内容仍保留多少 |
| Create/CreateOpen | 先准备节点、目录插入、会话/运输、回复及退休承载，再插入名字；CreateOpen 同一步将流绑定到所建 NodeRef | 提交前任一步失败不创建；提交后失败保留文件，未交付控制/offer 关闭或到期退休。不会按原路径删除后来被替换的对象 |
| Remove/Rename | 同一 backing 一次变更名字/引用；Rename 不移动已有会话的目标 | 最后名字解除不关闭仍持有的流/目录 grant；父目录 watch 收到名字变化，目标 watch 与实际目标寿命关联 |
| Sync | 当前内存 backing 在执行前返回 Unsupported | 无假持久性保证；不把 Finish 或关闭 sender 当作 Sync |

I/O 的参数/权限/类型错误在执行前用 Header.status 返回；一旦进入数据处理，其完成情况用 Header.status=Ok 的 IoResult 表达，cause 记录处理结果，防止通用错误解码丢掉部分进度。非零请求既无进度又非 EOF 时返回 Busy/具体错误，不无限返回零字节“成功”；count 不得超过请求量。

SetLength 复用 [Data 的稀疏块表示](../user/libraries/libfal/src/data.rs)：扩长只改逻辑长度，未分配洞读为零，不按目标长度同步分配/清零。缩短在一次有界提交中更新长度/内部版本并清零最后保留块的尾部（最多一个块），将 `Trimming { first_discarded_block, cursor }` 责任留在实际 Data；之后每次退休推进删除预算内的尾块，释放后才退款。Trimming 期间 Read 只见新长度，新的 WriteAt/SetLength 返回 Busy，活动写流停驻于该 backing 的变化唤醒，不继续提交或复活尾块；清理完成后清除标记并唤醒写流。缩短前准备的写因内部版本不符必须重新准备。清理承载在首次可能需要它的节点准入时预留，截断后调用者消失仍由 provider 推进；节点整体退休接管同一 Data，不并行清理两次。这是内存 backing 的短期写入背压，不是观察对象产生的锁或 OAL 状态。

创建返回空结果是有意的：成功表示已创建，不隐含必须再制造一份 ObjectInfo；需要摘要的消费者单独 Lookup。CreateOpen 只返回实际会话，省掉“创建→查询身份→条件 Open”的旧组合。

| 流内部状态 | 触发 / 转移 | 对控制请求的行为 |
|---|---|---|
| Preparing | 路径与权限校验、资源准备；失败进入原请求退休，成功安装 Offered | 不对外可见，不允许业务数据提交 |
| Offered | offer 有限期限；成功 Start→Active；超时/Cancel/控制消散/停止→Stopping | Start 校验 Runnel 握手后原子启动；重复 Start 在 Active 返回现状，不重置进度 |
| Active | 每块确认后端接受量及运输量；EOF 且排空→Stopping(Completed)，取消/失效/后端错→Stopping(对应原因) | Query 返回单次一致的计数；Finish 登记一个有界等待者，额外等待者返回 Busy；请求等待期限不延长 session 期限 |
| Stopping | 停止新块，等待已提交后端操作确定，关闭数据面、撤销来源并重试失败 owner | wire 仍报告 Active（未启动则 Offered）；Query 不冒充最终计数。Cancel 立即返回当前 StreamInfo，Finish 可等终态，等待者退出不取消清理 |
| Terminal | 已不会再发生后端写入，冻结 outcome/reason/计数；重清理资源可仍在退休 | Query/Finish/Cancel 幂等返回终态；Start 返回 Conflict；保存收费的最小控制记录，控制最后关闭后最终回收 |

业务计数冻结与映射物理释放是两道条件：可以先有 Terminal，再有映射关闭重试，但不能有 Terminal 后仍能提交数据。写方向 `accepted≤transported`；读方向 transported 是已放入隧道的字节、accepted 是对端已确认消费的部分。Copy 的本地读出量独立统计，不拿读流 producer 的计数冒充自身已经读到。

Finish 在 Offered 返回 Conflict，在 Active/Stopping 最多登记一个等待者，在 Terminal 立即返回。等待者的回复端关闭或请求期限到期会清空等待槽；它不改变 session_deadline，也不自动取消会话。Cancel 不争用 Finish 的等待槽：先提交停止请求，再立即返回当前 Info，可重复；只有 Terminal Info 的计数才是最终值。Start 在 Stopping/Terminal 返回 Conflict。控制的最后副本关闭也触发停止。session_deadline 可为领域允许的无限期限，但 Offered 建立期限始终有限；调用期限、建立期限与会话期限不互相替代。内存 backend 没有异步写入，Stopping 可快速完成；未来后端不能沿用“发出取消即完成”的假设。

文件 Watch 持目标的弱身份及通知登记，不用强 NodeRef 把目标永久保活。名字解除发 Unbound 提示（mask bit 4），实际 backing 在最后命名/使用引用消失并开始退休时发 Terminal；目录名字事件为 Create/Delete/Rename，内容为 Modify（沿用 bit 0—3），Terminal 固定最高位。明确 revoke 引起该授权建立的活动流/订阅停止，已经提交的数据不回滚。仅关闭父 grant 的一个副本不触发 revoke。

两种领域的订阅都由独立控制 context 管理，不能使用共享根 context 加裸 ID 操作他人订阅。sequence 从 1 开始，建立时置 Initial 重查提示（bit 62）；Initial/Terminal 自动包含，不允许请求方借 mask 排除。Query 不清，Ack 只确认给定旧序列，通知复位和 pending 判断由同一 owner 完成。CancelWatch 是幂等终止，不关闭仍由调用者持有的控制记录；Terminal 记录仍可 Query，终态通知保持到确认/关闭。发生新变化时先更新计数再置通知；`u64::MAX` 保留给终态，Active 不能达到该值，从而耗尽转终态仍有一个未被确认的新序列。reason 的数字为 Active=0、TargetRetired=1、ProviderStopping=2、Cancelled=3、Revoked=4、SequenceExhausted=5，service 的 TargetRetired 指范围目录退休。provider 停止后的控制不可用由传输 Closed 表达，不保证停止进程还能回复最后一次查询。

Copy 结果分为 `Complete` 与 `Failed { creation: NotAttempted|NotCreated|Created|Unknown, progress, cause, cleanup_pending }`。StreamProgress 含 `local_read, transported, destination_confirmed, source_terminal:Option<StreamInfo>, destination_terminal:Option<StreamInfo>`；未确认的目标下界为零，只有目标 Terminal 给出最终 accepted。Stream Complete 要求两端 Completed 且目标接受量等于本地搬运量。PropertyProgress 只含已取得的值编码长度和目标创建确认，不伪造流会话/计数；Property Complete 要求 Create 成功应答。两种 Complete 都不承诺源持续不变、元数据复制或持久化。

Copy 在发起业务前完成既有 Runtime 的任务、来源与清理容量准入；原任务从开始到实际清理结束始终是 owner。取消或失败先停止搬运并关闭两端控制的本地引用，业务结果可先交付，但不能立即完成的映射/来源/回复清理继续保留在同一已准入任务中，由后续 advance 推进，最后才返回 Step::Complete。取消意图同样只令原任务转入收束，不依赖临时再申请一个清理任务。cleanup_pending 仅是诊断状态，不向应用转嫁唯一清理责任；进程退出沿已有进程/内核对象链收束。禁止无 owner 的 Drop、靠下次 Copy 顺便清理或新建执行框架。目标文件保留，不恢复 CreatedTarget 按路径回删。

### 注册提交与完成状态

本节定义 registry 的完成合同。公开状态为 Active=1、Withdrawn=2；reason 为 None=0、Explicit=1、Replaced=2、ControlClosed=3、EndpointClosed=4、ProviderStopping=5。Withdrawn 是名字不可再选择的业务终态，不能据此推断观察来源和存储已全部释放；第一次撤出原因固定，不重新激活。

Registry 只持一份名字索引和 Registration 记录，以及真实需要的访问/发布权、订阅、请求和退休 owner。Preparing 属于未提交请求；Retiring 属于 provider 继续推进的资源责任，不是用户必须驱动的公开阶段。库可提供这些实现，运行实例和账户由宿主持有。

| 事件 | 提交与可见行为 | 失败、并发与清理 |
|---|---|---|
| Register 准备 | 验证名字/描述/真实入口/发布权，预留记录、索引、控制、两项来源、回复和退休承载；不得提前插入可见名字 | 异步准入完成前由请求 owner 持有全部资源；有限请求期限到期或取消可放弃，名字不改变 |
| Register(CreateOnly) 提交 | 在一次 backing 可变借用中复核 scope/权利/期限/入口关闭状态，名字空闲才插入 Active 注册，更新相关 Watch 序号和事件位 | 名字已有则 Exists；失败关闭准备的控制和入口并退休来源，不留下 Starting 记录 |
| Register(Replace) 提交 | 检查 Create/Replace 权，一次把名字指向新 Active；旧注册标记 Withdrawn(Replaced)，进入自己的退休队列 | 撤旧与插新之间没有可观察空位；旧控制晚到的 Withdraw 只能操作旧注册，不能删新绑定 |
| Register 成功回复延迟 | 回复保存该次提交的 RegistrationId 与 Active 快照，携同一注册的控制 | 后来被替换仍发送原成功回复；不能因注册已 Withdrawn 就取消其 Outbox。客户端可用控制 Query 得到后续终态 |
| Withdraw | 通过控制将本注册置 Withdrawn；只有 names[name] 仍指向它时才删绑定并发出变化 | 幂等；回复丢失可用同一控制 Query/再次 Withdraw，不按名字操作替代者 |
| 控制最后引用消散 / 业务入口关闭 | 已登记来源触发撤出自己的绑定并继续退休 | 不依赖注册者再调用。晚到旧事件不可撤出新注册；母本关闭不等于召回消费者副本 |
| GetEntry | 一次受保护步骤验证目录 scope、选取 Active 并取得描述及真实入口副本，随后退出借用发送 | 同时撤出时按选择点前后决定能否取得；已选择副本可在撤出后交付，发送失败只清理这份副本 |
| Retiring | 先撤销业务入口观察，再释放 registry 母本及相关收费；控制仍存活则保留最小 Withdrawn 壳和其来源 | 可恢复失败保留 owner 和下一次唤醒，轮转推进其它记录；不能 Err 退出任务后遗失责任，不能提前退款 |
| 终态控制关闭 | 控制 Lifetime 来源撤销完成后，最后壳和费用释放 | 后续访问关闭入口遵守运输 Closed；不承诺进程死亡后还能查询 registry 终态 |

**回复丢失的边界。** 已提交 Register 的结果可能未知，不能自动重试、按名字删除或恢复替换前的旧注册。尤其 A→B 的 Replace 已提交、B 的控制未交付而最终释放时，B 会自动撤出，名字可以变为空；不回滚到 A。B 短暂可见期间发出的业务入口仍由其发行者约束。本案不增加恢复 token、预分配操作控制或去重事务；需要“未知发布也能恢复归属”时须另有独立需求。

控制寿命包含回复消息中的真实引用：Outbox 未送达而失败时清理未交付控制，最后引用消散后撤出；已入活接收队列的控制仍真实存活，不能承诺 RPC 期限一到就撤名。registry 只长期持 Lifetime 观察能力，不额外保留使控制永不消散的 sender。准备及回复期限不会自动变成 Active 注册的租约。

**订阅与分页。** 名字更改和匹配 Watch 的 sequence/pending 更新在同一 backing 提交步骤完成；发送通知可以分批，Query 必须立即反映已提交变化。WatchName 跨注册替换存活，WatchDirectory 覆盖该访问范围；seq 耗尽则利用预备终态承载结束该订阅，不回绕且不阻止本次撤名。复用当前 Query→读状态→Ack 旧序号规则。注册 serial 耗尽只封新准入，枚举使用 last_name 游标，因此不需要全局 root epoch 或未来撤名版本预约。

并发使用 registry 单 owner/短可变借用；不持借用跨 RPC、等待来源登记或回复投递。请求解码/字符串复制受一次报文上限限制，分页按报文容量与调用预算停止，通知只扫描准入上限内的订阅，退休每次推进给定预算。停止先封新准入，再逐项撤名、终止订阅、撤来源与释放引用；控制/入口关闭事件始终有可推进 owner。发布授权和目录访问入口寿命观察同样保留到清理结束，不由文件 provider 的退休队列代管。

**责任链验证门。** 复用现有注册任务、provider 关闭和回复隔离测试设施，覆盖：重复名字、替换后旧控制、B 被 C 替换后迟到的 B 成功回复、Replace 提交后控制丢失导致空名且不恢复 A、控制仍在消息队列中时保持真实持有、已选 GetEntry 与 Withdraw 交错、WatchName 在名字不存在及 A→B→C 中持续有效、服务目录入口关闭与停止清理、静态及并发分页、额度耗尽和来源准入失败。测试按这些外部合同迁移，不保留仅为 Starting/Publish 状态本身存在的探针。

### 源码迁移映射

| 现有源码 / 实际调用 | 迁移或删除 |
|---|---|
| `libfal/src/node.rs` 的路径/NodeInfo 与 `libfs/src/resolve.rs` 的结果类型 | 通用路径字节/Boundary/ObjectInfo 归 liboal；内部文件种类仍归 libfs backing。删除公共固定 NodeSummary/Position.info 与普通请求的 Expected |
| `libfs/src/{prefix,resolve}.rs`、`client.rs` | 通用 prefix/Namespace、两种 Boundary 和私有 walk 上下文归 OAL；删除客户端 DirectoryGrant/Grant 包装及公共路径调用必须先取得 Position 的要求。Namespace 直接采用已有 Endpoint owner，fs 持自己的领域 RPC 并复用路径推进工具 |
| `libfal/src/{authority,grant,backend,store,data,resource,provider,watch,route}.rs` | 文件存储/操作归 fs，通用目录使用归 OAL，真实 grant/context/付款/退休归实际 provider。Issuance 改完整 u64 出口上限并同步装配参数/host 测试；不保留旧两位 forwarding 检查。边界复用足够窄的入口或由发行者签发，不整包搬进 fs |
| `libfal/src/{protocol,client}.rs`、`protocol/stream.rs` | 文件请求与流/值编码归 libfs；发现请求归 liboal；有 owner 的失败/清理类型跟随实际调用保留 |
| `libfal/src/bytes.rs` | Reader/Writer/DecodeError 移入 librpc 编码工具；fs/service/oal 同次改依赖，不留旧重导出 |
| `libfal/src/value.rs`、`libservice/src/record.rs` | fs 承接纯值编码；删除 Record/Handle/ExportPolicy/Take 与服务记录文件化，不给 tag 添加旁路 handle |
| `libservice/src/{protocol,authority,registry,resource,client}.rs` | 改 service v2 的单步 Register、GetEntry、RegistrationControl 与名字 Watch；删除 Starting/Publish/公开 Draining 和 FAL NodeStore 投影，Registry 直接生成 OAL 面貌；补齐领域客户端与服务 provider 支持 |
| `srv_fs/src/server.rs`、`server/{registration,authority}.rs` 的注册循环 | 可复用注册/来源/退休任务迁入 libservice 的 provider 支持；宿主只装配独立 registry mailbox/backing。删除 ServiceBackend 的 registry NodeRef 分流、文件 Backend 约束和重复控制索引；来源错误与退休 owner 保留至真实清理 |
| `srv_fs/src/server/stream.rs` 及文件 ingress | 保留文件领域处理并迁入实际 fs owner；不再承接服务 Record Read、目录入口特殊导出或服务 Watch |
| `srv_fs` secondary、`srv_init` 名字授权与入口发布 | 业务根已可服务且共享权限已收窄后单次 Register，拿控制后维持其寿命；删除随后 PublishReady 往返，保留“发布成功后才宣布启动就绪”的真实顺序。失败/未知交监督者，不自动重放 Replace |
| `srv_init` 与 `test_fal` 的 Subscribe→Read/ServiceRecord→Lookup | 改 WatchName→GetEntry→本地 Namespace.bind→领域调用；服务目录和文件入口分别装配。迁移回复隔离、退出、替换和名字订阅场景，删除只证明 Starting 状态存在的探针 |
| `libfs/src/client/copy.rs` 及真实 Copy 验收 | CreateOpen 与显式部分/未知结果；保留 Runnel/退出/清理 owner，删除默认 CreatedTarget 条件删除和条件 Open |

OAL 的 Object Abstraction Layer 名称及组件身份在 P1 同步写入 user/README；libraries/README、notes 索引与 impls 在实际迁移后反映代码，不在设计交付时把未来状态写成事实。Provider 接口可按同步 backing 方法与现有任务式长操作分别组织；不强迫所有方法实现一个异步 trait，也不把遍历/收束算法复制到每个进程。

### 设计走查

| 反例 / 既有审阅问题 | 本方案的闭合点 |
|---|---|
| 看过的名字被另一进程替换 | 普通操作直接处理本次目标；类型/权利不合返回错误，兼容替代者可以成功，没有 expected_identity 或 OAL 锁 |
| 完全未知领域对象 / 音乐对象同时像文件 | OAL 传递 Trait/tag 面貌，消费者按自己引入的领域知识匹配期望子集并调用；未知或无关标记忽略，不检查全体组合是否合理，也不保证 provider 的声明真实 |
| 普通 Read 必须先 Lookup、走两遍远端 | fs 请求自身遇边界才接续，本地最终 provider 在同次调用里选定目标并执行；未知对象初次 discovery 仍是显式可选行为 |
| Rename 携带目标授权、接续时丢失唯一输入 | 临时 grant 副本运输，原 owner 保留；只对明确未提交的 Boundary 重试；跨 provider 不伪装原子搬移 |
| 读服务记录使 provider grant 随读者增长 | GetEntry 只复制已经签发的共享入口；需要逐客户端资源时直接调用领域准入协议。共享账户、撤销及 watch context 的限制不隐藏 |
| Query 与通知复位之间发生新变化 | sequence 在状态提交时更新，Ack(old) 不吞掉新变化，通知投递可以分批但状态不能后补 |
| 服务 A→B→C 后旧控制者撤名、名字订阅消失 | 名字删除核对实际 RegistrationId；Watch 绑定名字，旧控制只管理旧注册 |
| Replace 提交但新控制未交付 | 明确允许新注册最终撤出后名字为空，不恢复旧注册；报告未知，不自动重放 |
| B 的成功回复晚于 C 替换 B | 成功回复仍交付 B 的提交快照与控制，随后 Query 可见 Withdrawn(Replaced)；不以终态丢弃该回复 |
| RPC 期限过去但控制仍在接收队列中 | 活引用仍计费，只有最后控制引用消散才触发自动撤出，不伪造到期回收 |
| Copy/写流回复丢失却报告精确接受量 | 已确认下界与可缺失的终态分开，CreateOpen 可 Unknown；Cancel 不在后端仍可提交时冻结计数 |
| 巨大 SetLength 无界清零、截断后再扩长露出旧数据 | 稀疏扩长；缩短同步遮蔽范围并清零边界块，尾块分批释放，完成前写入/扩长背压 |
| 撤名/Terminal 提前退款，关闭失败 owner 消失 | 业务可见性、终态、实际释放分别记录；预留退休承载，原任务继续持有并推进失败资源 |

走查结论限于设计与源码映射：没有保留旧通用执行候选的必要性，也没有发现需要新增内核原语才能实现本案的前置。旧审阅关于 Copy 未知计数、名字 watch 寿命和真实目标目录授权的发现均由上表对应契约承接；原 midflight Copy 故障仍需在代码阶段按验证门处置，不能因设计走查关闭。

## 目标模块与实际消费者

以下是知识归属和编辑范围，不要求每个表格行成为独立交付阶段。新目录只有在实施时才创建；本文不将计划中的类型/模块写成已经存在。

| owner / 位置 | 目标职责与迁移面 |
|---|---|
| `user/libraries/liboal/` | Namespace、通用树状声明、Directory、真实入口绑定、两种 Boundary、私有 walk 上下文、ObjectInfo/Trait/tag 和 provider 支持；不持 backing 树、通用 grant 表或领域业务报文 |
| `user/libraries/libfs/` | 文件领域知识、接口/值类型、编解码、符号链接对象与文件操作支持；复用 OAL 的目录使用及两种通用 Boundary，不拥有独立领域边界分派或重复状态层 |
| `user/libraries/libservice/` | service 协议与客户端、可复用 registry/provider 支持、独立访问/发布/控制/Watch 与退休实现；无 libfal/libfs 依赖、无文件投影或自建运行体 |
| `user/libraries/librpc/` | 通用 LE 编码工具及已有请求/回复能力清理；不增加领域权限、OAL 路径或文件类型知识 |
| `user/services/srv_fs/` | 装配文件和 registry 两个 provider owner，各自 mailbox/context/账户及 backing，同 Runtime 推进；无基于文件 NodeRef 的服务分派 |
| `user/services/srv_init/` | 根入口/名字发布权交付、服务发现、流与退出验收驱动；消费新协议和真实能力，不解码旧 ServiceRecord |
| `user/tests/test_fal/ → user/tests/test_fs/` | 复用现有真实进程验收，覆盖文件主线及所消费的 OAL/服务接点；改名同步 workspace、package、target、initfs 和启动路径 |
| 文档与构建 | `user/Cargo.toml`、相关 Cargo 依赖、Justfile、启动/验收引用；实现后同步 libraries/README、notes 索引及对应 impls |

旧消费者已核实集中在上述 provider、init、验收进程及两库内部；`srv_fs` 当前声明的 libfs 依赖未被其源码使用，不能把这视作已接通目标层。音乐播放器、设备/驱动仅作接口组合推演，不新增测试专用服务或假业务运行体。

装配接通按真实就绪顺序进行：primary 建立文件与 registry 的独立入口、授权/账户及 ingress/退休任务，再向 srv_init 交付对应根入口和发布授权；双方同步现有启动消息的能力角色，不继续把单一 FAL root 当两个领域入口。secondary 先把自己的文件请求与维护任务接入 Runtime，再由同一 Runtime 的发布任务推进 Register；不能在服务入口已经可见时让唯一服务线程阻塞于同步注册等待。收到 Register 成功后保留控制并沿现有启动流程报告就绪，失败/未知按现有启动失败与退出路径处理，不在本案补建正式监督者。消费者按 WatchName→QueryWatch→GetEntry→Ack 的顺序观察名字，需要使用取得的业务入口时再绑定；原文件与服务协议消费者成组迁移。

## 实施顺序

一次交付覆盖完整责任链。下列 P1—P3 是相互依赖的迁移工作包，中间状态不要求独立通过整体验收；不为临时通过检查创建旧协议 adapter、双编码或假消费者。局部不编译的迁移继续按目标接完；暂停时准确记录断点和已实施部分。

| 工作包 | 前置 | 成组迁移的内容 | 完成证据与删除门 | 当前状态 |
|---|---|---|---|---|
| P0 设计收口 | 本计划及当前源码 | 完成本篇协议、责任链、迁移及验证设计，执行结构审查并处置 findings | 当前范围无未决设计；独立审查两项发现经修订和作者复核关闭，见文末 | 已完成，D0 关闭 |
| P1 协议接口与 provider 对接 | P0 | 拆出 liboal 的发现表达/接口，分开各领域协议及编解码，接收端直接调用真实 provider 方法 | Lookup 和文件调用都落在同一实际 backing；没有抽象层状态、框架前置走路或 OAL 业务封装 | 未开始 |
| P2 文件领域 | P1 的接口与入口 | 将文件协议独立到 libfs，承接现有实际文件 backing、客户端及 init/test 调用；由 provider/backing 实现小 I/O、流、目录、Watch、Copy 及收束 | 目标支持矩阵接通；普通操作无旧 Lookup 条件；不在协议层复制 backing，旧能力属性探针退场 | 未开始 |
| P3 服务目录领域 | P1 的 OAL/RPC 表达；与 P2 联合验证文件入口发布 | 实现 service 协议和 registry 完成合同，迁入 provider 生命周期支持，改宿主/消费者 Register、WatchName、GetEntry 与 Namespace 绑定；删除旧投影和两阶段注册 | 替换/未知回复/控制队列引用/Watch/退款用例通过，libservice 不依赖 libfs/libfal | 未开始 |
| P4 组合收口 | P2、P3 接通 | 改名 test_fs，统一 Cargo/启动/initfs/验证锚点，删除 libfal 和剩余旧入口；回填实际 impls，完成结构检查与聚合验收 | 删除清单清零、验证矩阵有证据、历史缺陷处置明确；符合本篇交付条件后才归档 | 未开始 |

实际编辑可按公共类型 → 核心实现 → 消费者 → 清理顺序交错推进 P1—P3。不得将仅新增源码、局部编译或阶段结束登记为 OAL/fs/服务系统能力完成。根入口、会话、registration 和订阅的已提交清理始终由真实 owner 持有，不等待调用者回来补最后一步。

## 旧路径删除清单

| 待替换项 | 替代物与删除条件 |
|---|---|
| libfal package/目录、Cargo/import/构建引用 | 文件协议归 libfs、发现协议归 liboal；真实实现和状态由具体 provider/backing 承接，共享工具归实际 owner 后删旧包，不留旧名 facade |
| 混杂的发现/文件表示与抽象层走路假设 | 协议表达分别归各库；真实遍历保留在 provider/backing；只删除重复/错误的层，不为拆分搬出第二个 resolver |
| 固定公共 NodeInfo/NodeSummary、统一文件 NodeKind | ObjectInfo/tag 接通后删除旧公共表示；具体文件 backing 内部需要的种类和状态不在删除范围 |
| 客户端 DirectoryGrant/Grant、包装拆包及普通路径调用的必需 Position | Namespace 使用已有真实 Endpoint owner，领域 API 接受 namespace/路径，私有 walk 接管接续；应用无需事先 Lookup。真实 provider 授权表、受限能力及有业务依据的稳定引用保留 |
| ServiceRecord/FAL Record/只读文件投影 NodeStore | registry 直接生成 OAL 摘要，GetEntry 返回入口；所有服务发现调用迁移后删除 |
| ExportPolicy 中 Directory 特判、Handle Property、affine Take | 正式能力交付走领域协议；移除原专用探针及 handler，不以保留测试为由保留机制 |
| 常规路径调用的 expected_identity 与旧 CreatedTarget 自动条件清理 | 当前路径请求、CreateOpen 会话和明确 Copy 结果接通后删除；不误删后端内部身份、registration 标识或订阅序列 |
| 旧共享 grant 下裸订阅 ID 的管理授权 | 独立订阅控制入口接通后删除；通知/单调计数等真正公共机制仍复用 |
| 两阶段 Register/PublishReady、公开 Starting/Draining/Terminal、建立租约、QueryName 与按名 Withdraw | 单步 Register/CreateOnly/Replace，控制入口 Withdraw/QueryRegistration，公开 Active/Withdrawn；旧 opcode、状态任务分支、epoch 预约及专用探针同时删除 |
| registry 文件 Backend、NodeStore<Projection>、ServiceRecord、FAL Watch/GrantTable 及宿主 registry hook | 独立 registry OAL/service provider、Registration backing、名字 Watch 和真实角色表；GetEntry 显式交付入口，宿主只装配 |
| test_fal 的当前 package/产物/启动名和失效锚点 | test_fs 和对应消费者/验证器同次切换，不留名称补偿 alias；历史日志与档案保留原名 |

最终删除检查同时看代码、Cargo 图、initfs/启动和有效文档。真实文件 backing 需要的 NodeRef/store、grant 表、流会话等继续按实际职责保留或重构；它们的实例属于 provider，不是要被清空的数据或要重新在 libfs 抽象层建立的副本。

## 验证矩阵与执行入口

开发证据和完整验收分开记录。优先迁移现有 host 单测、srv_init 和 test_fs 场景；下表给出需要捕获的独立错误，不为每个阶段另造 harness。

| 契约 / 风险 | 最小可信验证边界与预期 |
|---|---|
| 编码、未知 tag 与帧边界 | host 协议测试：按定义构造完整/截断/超长/重复键/未知标记/非法能力槽；未知 tag 可跳过，损坏帧返回错误，不触发业务 |
| 完整运输上限 | u64 编解码不截断高位，未知 Rights 位拒绝；Derive 的入口自身权限和新出口分别不能超母上限；缺少角色必需位时提交前 Permission；GetEntry 按双方完整上限求交，DUPLICATE/GRANT 不越权。覆盖 Bind 内部派生和旧仅转授位装配参数的替换，不以 Rights::ALL 绕过失败 |
| 接口直接调用与单一 backing | 同一具体 provider 实现 OAL/fs 接口；分别请求 Lookup/Read，验证 backing 数据变化自然反映到摘要，接收胶水不预先遍历或保存对象镜像 |
| 普通路径 API 与入口授权分离 | 复用现有消费者，Get/Open 从 namespace/路径直接发领域请求，跟随两种 Boundary 后由当前 provider 执行；不要求 DirectoryGrant/Position 或前置 Lookup。独立受限入口仍由 provider 校验，修改客户端路径不能扩大能力范围 |
| Boundary 和受限授权 | 复用现有 provider/host 接续设施：只依赖 OAL 的客户端跟随路径转发与换入口交替，不需要领域 Boundary 解码；覆盖相对/视图绝对目标、未消费后缀、播放器式目录别名、空剩余路径和循环预算；真实能力不放大，取消/回复丢失无遗留入口 |
| 领域路径与能力请求 | Rename 校验真实双目录授权、同 backing 与环；同 mailbox 不同 backing 拒绝。Bind 先准备真实受限出口，不把可越权母本发给消费者；覆盖共享出口复用、必要派生、替换/撤出和父授权失效。结果丢失不自动重放 |
| 枚举与只读摘要 | 静态目录完整无重复；长路径/长名字第一页能返回也必须能续页。覆盖本文 D-F1/D-F2 边界、Create/CreateOpen/Bind/Rename 的共同名字准入、Property 形状变化、伪造/过期 cursor、目录移出授权范围和入口撤销；摘要不产生能力，续页不增加 Derive 要求 |
| Property、小 I/O 与文件会话 | 类型/范围/短结果、同一文件共享流、SetLength 交错、建立失败和退出；截断→清理→扩长读零、清理期间写流可恢复且无旧数据复活；实际计数及支持/权限错误符合接口 |
| Copy 与结果未知 | 迁移 Property/Stream 及跨 provider 场景；验证完整属性原子创建、流前缀取消、CreateOpen/Finish 回复丢失、provider 停止和 Copy 任务消散后 Runtime 接管清理。已确认下界不冒充最终值，不按名字删替代者，不自动升级为跨域 Move |
| 服务发布与替换 | registry 单测及既有真实消费者：Register 一次可见、旧控制不删新注册、B→C 后 B 迟到回复仍可 Query、Replace 回复丢失后不恢复 A、GetEntry 描述与能力配对 |
| 服务目录使用与分页 | 只用 OAL 可枚举根/查看 ServiceEntry，末项服务条目不冒充业务目录；GetEntry 后按协议使用/绑定，名字替换不偷偷更新已有绑定；静态分页完整、并发弱一致、游标不能扩权 |
| 名字订阅与丢唤醒 | 从未发布到 A→B→C 的连续变化；Query → 读状态 → Ack 间插入变化仍可观察；registration 退休不终止名字订阅 |
| 资源与退出 | 正式 provider/消费者退出和响应丢失路径；入口、来源、映射、终态记录在真实释放后退款，无无人持有的退休责任 |
| 依赖和旧路径 | Cargo/import、实际调用、构建/启动及有效文档检查；libservice 不依赖 libfs/libfal，liboal 不解释领域状态 |

host 检查按受影响包选择并显式指定 host target；例如目标包建成后在 user workspace 使用 `cargo test -p liboal -p libfs -p libservice -p librpc --target aarch64-apple-darwin`。实施前按实际 cfg/包测试能力核对，不把该示例当已执行结果。

责任链接通后按[构建与验证手册](BUILD-AND-TEST.md)执行 `just check` 和跨机制聚合 `just acceptance`；后者已包含 clippy 及规定平台/退出路线，无新风险时不重复单跑同一套检查。`just check` 只覆盖 os 及其依赖，不能代替用户态验证。本案未改调度域契约时不额外要求 hetero。长日志、进程清理和证据边界遵守手册。

新增测试须对应上表的独立契约；不会因改变 API 而机械保留旧断言，也不会因为删去旧锚点就宣称覆盖了新行为。受影响 impls 在代码接通后同步回填，运行结果和阶段状态只记录在本计划。

## 未完成责任与延期边界

| 项目 | 唯一 owner / 当前事实 | 触发、完成标准与保留期限 |
|---|---|---|
| 新实现的结构检查与独立代码 Review | 设计 D0 已关闭，尚无本轮 Rust 实现可审查 | P4 按 REVIEW 完成交付前结构检查；代码交付后另登记固定提交/序列的独立 Review，不能以本轮设计审查替代 |
| 新协议与实际资源链尚未实现 | 本计划 P1—P4；本轮只整理设计 | D0 关闭后按源码迁移表接通真实两端；以完整验证矩阵和旧路径删除为完成标准 |
| 迁移期间仍存旧路径 | 本计划 P1—P4；代码尚未改，未产生新的半重构区域 | 同一切面接通后删除对应旧实现；P4 前清零。暂停时记具体文件/责任/验证断点并同步矫正区，不创建兼容桥接 |
| 旧 Copy 间歇失败 | [原问题记录](todo-2026-09-29-fal-midflight-copy-flake.md)独占现象与根因证据；当前根因仍未知 | 新路径若复现或涉及复用机制，属于本次 Copy 正确性闭包；交付前提供修复或旧责任链被替换且等价场景已验证的证据，在原记录说明处置，不用一次通过宣称原问题根因已解决 |
| 设备/驱动与正式装配 | 各所属分案；不在本次实现 | 它们开始接入时使用本案协议；本案只保证扩展接点，不能宣称硬件资源或长期服务能力已交付 |
| 范围外文件能力 | [文件扩展计划](todo-2026-09-fal-extended-operations.md)唯一登记高层跨域 Move、递归/元数据、持久化等触发与完成标准 | 本篇当前边界完整；实施不得借 Copy/Delete 隐式实现未设计的移动事务，也不重复维护该计划的延期清单 |

其它临时表示、清理缺口或范围外残留如在实施中出现，按项目债务规则加入本节，写明 owner、位置、触发、删除条件和验证。未经登记不得作为跨阶段默认前提；当前承诺内的正确性欠账不能延期。

## 完成条件与记录维护

- D0 已关闭，当前目标、接口与资源责任完整，实际支持矩阵与非目标相符。
- OAL、fs、service 的请求/响应两侧及 srv_fs/srv_init/test_fs 接通；基础库存在或局部检查通过不能替代真实消费者接入。
- 正常、失败、取消、超时、退出和退款责任逐项兑现，已提交工作不依赖消散的调用者。
- 删除清单清零，Cargo 身份一致，未留下旧库 facade、重复对象真值或未登记过渡路径。
- 相应验证矩阵、`just check`/聚合验收和交付前结构检查有明确结果；旧 Copy 问题按原记录处置，未覆盖项如实记录。
- 实现说明按当前源码回填，COMPASS 和关联计划指向有效入口；ideas 的线稿清理/审定与代码完成分开登记。

全部条件满足才归档本活跃计划；提交、push 和发布仍按各自授权边界执行。代码交付后的独立 Review 按[REVIEW](REVIEW.md)登记固定提交或序列，不替代本次交付前检查。纯文档整理不登记代码独立 Review。

## 工作记录与当前证据

2026-10-09 第十轮：依据本轮“完整整理、补全设计、避免实施走偏”的授权，将前两轮快照归档，主文保留唯一目标；补齐表示、路径/分页、文件原子边界、服务协议、授权/失败/退休和实施门。前轮的局部检查与咨询保留在历史快照，不自动计作本轮完整审查。仅文档修改，未运行 Rust 编译、host 或 QEMU，P1—P4 未开始。

第十轮独立 reviewer 对完整目标及实际源码前置进行只读结构审查，发现以下两项 P1 设计阻塞；修订后由作者逐项复核原反例及新增授权路径，后续 MellowPond 的协作核对另见下节。这些均为设计证据，不是实现验收。

| Finding / 原始反例 | 目标修订 | 复核与状态 |
|---|---|---|
| D-F1：分页响应能装下，原路径与 cursor 却可能装不进续页请求。路径 2200 字节、子目录名 1825 字节时，首次请求 2212、首个非终页 3722 均合法，旧续页为 4057 > 4048 | 续页 path 为空，cursor 由原入口解释目录；客户端只持原入口与 opaque cursor。提供端每页验证实际目录仍在当前可遍历授权范围，不产生新 grant 或 OAL 对象身份 | 按字段编码复核，新续页为 1857 ≤ 4048；非终页还约束 12+cursor_len≤B。枚举所有当前可准入名字长度均满足。**已关闭** |
| D-F2：CreateOpen/Rename 未执行可枚举名字约束。2000 字节 Stream 名可分别以 2052/2013 字节请求引入，但非终页需 4124 > 4048 | 共同成员准入覆盖 Create/CreateOpen/Bind/Rename，按源种类最大摘要计算；超限在变更前返回 EntryTooLarge | 编码长度与 4124 的反例已复核，新共同准入拒绝该名字；Property 最大形状和 Binding 空摘要分别计入同一公式。**已关闭** |

审查未提出重开 OAL 原则或新增内核原语的阻塞；注册替换/迟到回复、队列控制寿命、Watch、流业务终态与物理释放分离已有合同。作者同时核对续页数字不授予权限、目录弱父关系/退休、输出能力角色、真实容量预留、路径预算及实际消费者映射。当前承诺范围内无已知未闭合设计选择，D0 关闭；常规模块内部实现与最终资源布局仍由施工按这些合同确定。

文档证据：7 份 Markdown 的 117 个本地链接/锚点、活跃表格列数、代码围栏、namespace 编号及外部导航入口检查通过；新历史快照逆向还原相对链接后的 SHA-256 与整理前正文一致；独立审查反例的逐字段编码及容量不变量计算通过，`git diff --check` 通过。没有 Rust 编译、host/QEMU 或新代码的验证证据。

### 与 MellowPond 的实施接手核对

2026-10-09，按用户要求与 MellowPond 完成双向实施接手核对。对方完整阅读主计划、复算 D-F1/D-F2，并抽查 RPC 前缀、真实 sender 身份和容器准入；明确认可目标/非目标及责任链，确认没有实质实施阻碍、没有需要用户裁决的分歧或设计点，接受组织 P1—P4。双方共同确定以下实施口径，已同步正文：

1. liboal、libfs、libservice 是三个知识 owner，不构成 OAL→fs→backing 的串行执行层。Rename 的 mailbox 关系校验只是必要条件，仍要用可信 context 解析实际目录授权和相同 backing；跨域、角色错误、撤销分别按 CrossFilesystem/Permission/Revoked 处理。
2. [Runtime](../user/libraries/libexecution/src/runtime.rs) 的 stop 只冻结意图、advance 继续退休，spawn 失败返还 task owner，未排空的 close 返还运行体。Copy 的原已准入任务持续持有清理，业务结果可先交付，最后才 Complete；取消不临时申请第二个清理任务。
3. [RegistrationClientTask](../user/services/srv_fs/src/server.rs) 已经通过 DownstreamState/dispatcher 非阻塞推进；迁移时删除 PublishReady，Register 成功直接持控制进入 Published，同时替换旧 ExportPolicy/ValueProtocol/FalRights 构造。现有机制足以支撑新就绪顺序。
4. object ID API 已核实：[object::query](../user/libraries/libkernel/src/ipc/object.rs) 返回 HandleDescription，[MailboxSender::description](../user/libraries/libkernel/src/ipc/message.rs) 可取得 sender 身份；[GrantTable](../user/libraries/libfal/src/grant.rs) 已在 new/prepare_minted 中取得 mailbox object_id、校验 related_object_id 并使用 sender object_id。StreamOffer.session_id 与 incarnation 不需要新增内核接口，也不留作 P1 的未决前置。
5. 互核发现旧 output_transport 仅接受 TRANSIT/GRANT，不能直接沿用。新输出上限统一为完整 Rights，运输字段与 shared 一致为 u64；入口自身权限、新授权出口及实际交付角色分别受限。已同步 DeriveDirectory/Bind/Register 线形、请求长度、旧检查/装配参数迁移和验证要求；不使用兼容分支或无条件 Rights::ALL。
6. try_reserve 不单独证明提交无分配。每个并发准备持有自身可兑现的容量/槽位，P2/P3 按实际容器闭合并记录；这些是实施责任，不是需要用户另行选择的架构方案。

接手核对结论：D0 保持关闭；本轮只澄清设计与移交，未改 Rust、未执行代码验收。用户安排实施代理后，由 MellowPond 按下节组织施工及回填记录。

交接补文经 MellowPond 再次核对，明确回复“与共识一致，无偏差”，并独立复算三种加宽请求的长度。补文后 7 份 Markdown 的 122 个本地链接/锚点、活跃表格列数和代码围栏检查通过；逐字段编码复核确认 DeriveDirectory/Bind/Register 的新长度及 u64 高位不截断，`git diff --check` 通过。这里只验证设计表示；未知 Rights 位的运行时拒绝及实际授权/清理仍须在 P1—P4 按验证矩阵兑现。

## 当前接力

当前协调与主计划维护 owner 为 **MellowPond**，已经明确接受接手；等待用户将实施代理交给它后组织 P1—P4，本会话不继续并发编辑或启动代码。交接基线为 HEAD `6187ce194b26881076f0eba209f997b808fd0c3a`，工作树中的计划/导航/参考快照改动尚未提交，应完整保留；提交及外部写入仍须各自授权。

下一施工入口为 P1，按公共表示/编码 → provider 对接 → 文件与服务责任链 → 消费者 → 旧路径删除的依赖顺序，允许 P1—P3 交错编辑。实施代理先核对 HEAD/工作树，再读本篇完整目标、迁移/删除清单和验证矩阵；不重开已确认原则，不从历史初稿恢复规则。执行结果、具体容器预留、发现与修订、暂停断点及验收证据都写回本篇唯一记录。

实施者可决定 Rust 内部写法、容器和文件拆分，但不能自行恢复 DirectoryGrant/强制 Lookup、服务文件投影、两阶段发布、按路径 Copy 回删或跨域移动补偿。若源码证明本篇的关键前置不成立，先在本篇记录反例并修正相关责任链及验证门，不叠加兼容 adapter 让中间态通过。设计门已关闭不豁免实施中的结构检查、实际消费者接通与验收。
