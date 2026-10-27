# OAL、fs 与服务目录改造计划

> 2026-10-27 活跃资料清理前的完整工作记录快照。本文只保留当时的设计轮次、施工、失败和验证事实；当前合同与条件后续由[OAL/fs 计划](../todo-2026-10-07-fal-service-redesign.md)拥有。

状态：**本轮实施与 C01—C11 收敛已提交，用户已确认交付结束及遗留验证边界。** 本篇保留未闭合专项验证的唯一承接与历史证据，不再作为待施工队列。精确注入窗口与历史未知根因继续按原分级保留，不由普通验收核销。目标、协议、生命周期、迁移和验收由本篇拥有，当前接续见[接力](#当前接力)，本轮修改与验证见[机制收敛](#提交前机制收敛)。

历史讨论独立保存在[第一至第七轮](ref-2026-10-07-oal-fs-service-design-rounds.md)与[第八至第九轮](ref-2026-10-09-oal-fs-service-rounds-8-9.md)。其中被替代的初稿、状态与“下一步”不参与本篇目标解释。外部系统事实见[文件操作取证](../ref-2026-10-09-cross-provider-file-operations.md)。

## 交付目标与范围

把现有 libfal/libfs/libservice 中混合的发现、文件和服务协议职责拆开，交付 liboal 发现协议、libfs 文件协议及 libservice 注册取得协议，接通现有 provider、装配与验收消费者。所有领域数据、状态、实际路径遍历与操作由具体 provider/backing 实例持有和执行；协议库不在 backing 之上另建状态层或执行层。

| 范围 | 本案承诺 |
|---|---|
| OAL | 用户态树状视图的声明、provider、Directory、目录发现与入口传递、Lookup/Enumerate、ObjectInfo、Trait/tag 表达及 Boundary 接续；不解释领域对象或承载领域操作 |
| fs | 内存文件 provider 的目录/Property/Stream、授权、小 I/O、流完成、名字变更、订阅和 Copy |
| 服务目录 | 名字发布授权、注册/发布/替换/撤出、GetEntry、名字/目录变化订阅及入口退休 |
| 实际接通 | srv_fs、srv_init、现有 test_fal 及 libfs/libservice 内部消费者；验收进程随迁移改名为 test_fs |
| 支撑面 | 必要的 RPC 编码工具和 provider ingress 分派、Cargo/构建/initfs/验收入口、对应实现说明与导航 |

不在本案实现磁盘/网络文件系统、音乐播放器、完整设备/驱动资源协议、正式长期服务监督或新 RPC/Runnel 运行体。内存后端不承诺持久化；设备/驱动仅检验 OAL 的扩展接点，其领域设计由[设备分案](../todo-2026-09-device-system-design.md)拥有，启动和健康监督由[装配分案](../todo-2026-10-07-service-assembly.md)拥有。

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

代码与[当前 FAL 实现说明](../../notes/impls/fal.md)描述旧实现；[KNOWN_ISSUES](../../KNOWN_ISSUES.md#矫正区)已登记矫正范围，不把旧结构当新方案的正确性依据。现存 RPC、Runnel、Process/Job 和 SourceRegistration 已有实现基础，本案不重新启动已完成的对象链或运行时分层任务。

本案从既有 FAL/fs 的协议和 backing 实现做职责分离。库定义契约与可复用代码，实际实例由 provider 持有；不能把“代码位于某库”解释成“运行时多一个拥有对象真值的层”。共同 PathHeader、OAL 通用 Continue 业务封装、框架先定位再调用领域 handler、强制 Locate 前置均撤出当前方案。CreateOpen、共享流、Copy 保留部分结果等仍只是 fs/backing 的领域设计，不是 OAL 的职责。

当前共享运输常量为 `PAYLOAD_MAX = 4096`、`MESSAGE_HANDLE_MAX = 8`，来源是 [message.rs](../../shared/erhino_shared/src/message.rs)，它们是现状依据而非在新库复制的常量。旧 `PATH_MAX = 512`、链接/步数限额不能未经论证照搬。完整报文必须计入 RpcPrefix、路径/领域头、参数、响应和实际能力槽。

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

核实依据（设计基线 `6187ce1` 的迁移起点，旧文件已删除）：当时 `libfal/src/backend.rs` 的 lookup/resolve 共用 lookup_child；`libfal/src/protocol.rs` 已有 DirectoryEntry.name/info；`libfs/src/resolve.rs` 的 Found/Delegate/Link、prefix 和 Position 承载路径接续。当前实现见 [文件 backing](../../user/libraries/libfs/src/backend.rs)、[OAL 协议](../../user/libraries/liboal/src/protocol.rs) 与 liboal 的 walk/namespace/view/client。通用换入口、相对/绝对转发及路径上下文由 OAL 承接，解除对文件 NodeKind/NodeSummary 的依赖；fs 保留符号链接内容读取、创建等领域操作。现有 RPC 和 Move 准入仅作既有运输/授权事实。

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

[外部取证](../ref-2026-10-09-cross-provider-file-operations.md)核实了 QNX、Hurd、Genode、MINIX 和 POSIX/Linux 的不同边界。它们支持分别建模原子 rename、复制/删除组合与数据复制加速；不把跨进程 IPC 本身当作原子性的保证或障碍。

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

Create/CreateOpen 的条件是所选名字在提交时仍为空、原父仍处于该授权根的可遍历范围并允许 CREATE；不同名字的并发准备互不使对方失效，不要求整个父目录保持未变。每项持独立成员 permit/预建索引与 Node。提交继续增加目录内部版本，故还须复核版本未耗尽；同名后来被占用为 Conflict，原授权撤销为 Revoked，移出范围为 Permission，耗尽为 Resource。准备时已存在的名字仍按 Create 契约返回 Exists，不更换选中的父实例或自动重试。

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
| 14 Watch | path:Text, mask:u64 | WatchInfo；回复槽 0=独立控制 sender、1=NotificationObserver（由 provider 持有的 Owner[M] 铸造，见[通知观察角色前置](../todo-2026-10-10-notification-observer-roles.md)） |
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
| 6 WatchDirectory、7 WatchName | directory_path:Text；WatchName 追加 name:Text | result:u32 + WatchInfo 或 Boundary；建立成功槽 0=控制、1=NotificationObserver，ProviderBoundary 仅槽 0 |
| 8 QueryWatch、10 CancelWatch；9 AckWatch | 与既定 Query/Ack/Cancel 体相同，直接发到订阅控制入口 | WatchInfo；编码及语义由 service 拥有，不依赖 libfs |

`RegistrationId = {registry_incarnation:u64, serial:u64}`，前者取该 registry mailbox 的内核对象 ID，后者由 provider 单调分配、不回绕；耗尽只拒绝新注册，不妨碍撤出旧注册。它用于描述和区分注册，不是授权。`RegistrationInfo = RegistrationId + state:u32 + reason:u32`，实际控制身份来自不可伪造 sender context。Register mode=0 CreateOnly、1 Replace；每次提交创建新注册，控制不支持再 Publish 或重新激活 Withdrawn。一个注册持一个可复制 MailboxSender，该入口可接受多个协议；控制只属于目录注册，与业务会话独立。Register 回复表示提交时的 Active 快照，回复到达前可能已经被替换，须用该控制 Query 观察后续状态。

三个领域协议及文件管理各自固定 status：0 Ok，1 NotFound，2 NotDirectory，3 Permission，4 Invalid，5 Unsupported，6 Exists，7 NotEmpty，8 Revoked，9 Conflict，10 Resource，11 Quota，12 Busy，13 Cancelled，14 Internal，15 CrossFilesystem，16 TypeMismatch，17 Range，18 Expired，19 CursorInvalid，20 SummaryTooLarge，21 EntryTooLarge，22 ValueTooLarge，23 Unavailable。各自仅产生适用项，枚举归各自协议，librpc 不拥有业务错误。已识别 kind 的未知 op/版本不执行；头和回复权可信时以本协议支持的版本返回 Unsupported，op/txid 回显。无法安全解码头、能力错型或完全未知的 message kind 按既有 RPC 清理拒收，不猜测陌生领域的回复格式。客户端将未知版本/status、损坏回复或运输失联作为明确的调用失败，不据此重放变更。IoResult 的 cause 使用 fs status，end=0 可继续、1 EOF、2 Error；短 I/O 可同时携带后续错误，不能用空错误体抹掉已提交数量。

每个领域体容量为 `B = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN`，当前 4048；只引用共享常量，不在新库复制 4096/8。若有 T 个 Trait 和各值长 V 的 tag，`ObjectInfo_len = 8 + 20*T + Σ(24+V)`；Lookup Found 为 `4+ObjectInfo_len`。任意长度/计数先做 checked 运算，确认所需字段能由剩余正文提供，再分配；不能按未验证的 count 预分配。完整解码后才触及 backing，没有隐式结构体 padding，bool/枚举/保留位按各自定义校验。

分页逐项计入 result、cursor、done、count、名字和 info 的全部前缀。MemoryBackend 的共同成员准入维护 `44 + 2*name_len + max_info_len ≤ B`，所有引入或改变名字的 Create、CreateOpen、Bind、Rename 都必须使用同一个计算；Rename 用源节点种类所有允许形状的 max_info_len 检查目标名字，Property 按数组等最大形状计入，Bind 的 info 为 0。超限在提交前返回 EntryTooLarge，不修改旧名字、目标或文件；不能仅在构造枚举回复时才发现本地合法变更破坏该条件。registry 对应为 `36 + 2*name_len + info_len ≤ B`。还须满足各操作完整请求和 Lookup 容量，约束均由协议推导，不另设 PATH_MAX。

任何 provider 输出非终页前还须保证 `12 + next_cursor_len ≤ B`，使空 path 的下一次 Enumerate 请求可以编码。文件 cursor 长为 `20+name_len`，续页请求为 `32+name_len`；registry 对应 `24+name_len`，由本案上述成员准入保证。初次长路径不再重复占用续页正文。外部/动态 provider 无法给出可表达摘要、条目或可用游标时按 SummaryTooLarge/EntryTooLarge 返回，不截断或输出无法继续的非终页。

Register 预检请求、Lookup/单项续页以及 GetEntry 的 `24+descriptors_len≤B`；后者与 RegistrationId/描述/能力必须匹配。Set/Create 的完整 Property 必须能经 Get 返回，超出 ValueTooLarge。ReadAt 每次最多返回 `min(requested_count, B-24)` 字节，剩余由调用者继续；WriteAt 的数据受完整路径/偏移/Bytes 请求容量约束。偏移加长度、大小转 usize 和各流累计计数均 checked；不能回绕，数据操作中遇上界按 IoResult/终态保留此前已完成数量。

协议表中未使用的能力槽必须为空，错误回复不带业务能力。OAL 页面没有业务能力；ProviderBoundary 恰好 1 个。fs Open/CreateOpen 与 Watch 的成功结果恰好 2 个，其余按表。service Register 业务输入仅 slot 1，GetEntry 成功恰好 1 个，Watch 成功恰好 2 个。先按结果变体校验角色/数量，再把临时 capability 转成相应 typed owner；损坏帧沿 RPC 清理全部收到的能力。

运输字段直接使用 shared 的 `Rights(u64)`，sender_transport/output_transport/export_transport 在线上均为 LE u64，不截断成 u32；FsRights 与服务访问/发布位仍为 u32。未知 Rights 位返回 Invalid。output_transport 是该授权向外交付能力的完整 Rights 上限，不是旧实现仅含 TRANSIT|GRANT 的附加转授位。实际输出按本操作的角色和明示请求/交集计算，必须满足该角色的必需位并处于上限内；不相容时提交前 Permission，不默补被上限禁止的业务位。

可调用 sender 的必需位为 WRITE|WAIT|TRANSIT；无显式运输位参数的新建 sender（发布 authority 或流/注册/订阅控制）默认再取上限允许的 DUPLICATE。Watch 通知角色为 NotificationObserver：provider 持唯一 NotificationOwner 负责 Take/清除与关闭，经 NotificationMintObserver 铸造只读观察 READABLE|CLOSED 的 Observer 交付客户端；必需位 WAIT|TRANSIT，仅在上限允许时带 DUPLICATE，不交付 Owner、不带 READ/SIGNAL/MANAGE。客户端在同一订阅异步处理在途时按 SourceRegistration remove/注销回执暂移来源并持有待处理责任，完成后重登记复查；角色、ABI、付款与等待合同由[通知观察角色前置](../todo-2026-10-10-notification-observer-roles.md)拥有。Runnel invitation 使用 librunnel v2 的既有角色/权限合同。GRANT 不自动附加，只能按操作的显式运输请求交付且仍受上限约束。内部需重复导出的母本持 DUPLICATE，不等于接收方必然获得该位。控制 sender 的 context 必须绑定新建的控制记录，不能复制普通目录 sender 充当控制。

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

费用按真实占用计算：固定记录分配字节、预留可变容量及索引/退休节点；可共享名字存储由一个实际 owner 计费，索引和 OAL 摘要不各存一份领域真值。ServiceResource 保留 Authority/Registration/Bytes/WaitSource，增加 Access/Watch 及必要请求/Outbox 准入；具体布局统一按实际 owner 计算，不能对同一存储重复收费。Runtime 的 Task/InputBytes/来源额度与 service 账户来自同一可信付款上下文。每注册在提交前至少准备控制 Lifetime、业务入口 CLOSED 两个来源及最终关闭承载；每 Watch 在提交前准备控制 Lifetime 来源及唤醒承载。通知的 NotificationOwner 与 Signaler 均由 provider 持有，Owner 的关闭是 provider 自己的退休动作，不另登记观察来源；客户端 Observer 的复制或消散不终止订阅，也不可被观察，订阅终止只由控制寿命、显式 Cancel、目标退休或 provider 停止决定。请求/回复来源依现有 RPC 生命周期另计。容量是宿主的真实预算，控制壳继续收费直到关闭，释放的 endpoint/来源分别退款。

预扣 quota 不等于已经预留内存。插入名字、安装控制、替换索引和登记退休的提交步骤不得分配：使用实际可失败的 reserve、预建索引节点或已付费槽位，在准备阶段取得所需容量。并发准备须各持可兑现的槽位/容量 permit，不能只因 try_reserve 成功就允许其它请求耗掉同一份空闲容量；提交仅消费自己的预留。没有 reserve 合同的容器不能凭“已扣 Bytes”宣称提交必成；可更换内部布局，但须同时计入保留容量与提交工作界。释放一条记录不自动退还仍被容器持有的容量费用。P2/P3 须在工作记录写明每个实际容器的预留与释放方式。

订阅和控制的保留可能由持有者延长，但始终计费且有全局/账户上界。grant、stream、watch、registration 的 cleanup 节点、必要的终态记录与唤醒承载均在公开前准备；提交后不得因再次申请容器失败而丢掉责任。准备失败关闭已经铸造的入口/通知/映射；close 或来源撤销失败时，完整 owner 留在原任务的退休状态继续推进，不在错误值被丢弃时泄漏。

并发沿现有 provider 执行方式：真实 backing 由本地单 owner/短可变借用串行访问；borrow 内完成选择、验证和提交，不持有它跨 IPC、映射或等待。RPC/流任务保存自己的准备对象和实际 NodeRef；异步准备回来后重新验证仍需成立的授权及当前名字条件，再提交。OAL 摘要只是这次借用生成的数据，不登记观察者。不同 provider 没有跨进程锁序；Rename 只在一个 backing 提交，远端派生先交出本地借用。

每次 poll 有显式工作预算：路径/值扫描受单个报文长度限制；流最多搬运缓冲可容纳的一个块；退休最多处理传入 budget 项。watch 状态标记最多扫描准入的 `W_limit` 条，内存容量及这一次扫描成本由宿主配置约束；提交前预留足够的 WakeBatch，标记在提交步骤完成，唤醒投递满时保留游标分批兑现。这样 Query 可立即看到变化，延迟的是通知投递。停止时先封准入，再让已提交 entry 和退休任务推进；临时任务消散或外部不再调用均不能删除清理责任。

源码复核的复用边界（设计基线 `6187ce1` 的旧结构，已随迁移替换）：当时 `libservice/src/registry.rs` 的 retire_registrations 在 endpoint.close 错误时保留母本但向上传播 Err，最终可到达 `srv_fs/src/server.rs` 中 runtime.turn 的 expect；`libfal/src/provider.rs` 的 Watch close_signaler 丢弃返回的 owner，转入 typed Drop 的必成关闭边界。这两种写法不是“可恢复退休错误已有重试”的证据。目标任务对可恢复来源/映射/退休错误保留 owner、记录下一次唤醒并重试，轮转游标防止一项失败阻挡其它退休项；合法 typed 非映射 Handle 关闭的结构性必成约束仍以 libkernel 为准，不通过制造非法 handle 来伪造可恢复故障。

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

SetLength 复用 [Data 的稀疏块表示](../../user/libraries/libfs/src/data.rs)：扩长只改逻辑长度，未分配洞读为零，不按目标长度同步分配/清零。缩短在一次有界提交中更新长度/内部版本并清零最后保留块的尾部（最多一个块），将 `Trimming { first_discarded_block, cursor }` 责任留在实际 Data；之后每次退休推进删除预算内的尾块，释放后才退款。Trimming 期间 Read 只见新长度，新的 WriteAt/SetLength 返回 Busy，活动写流停驻于该 backing 的变化唤醒，不继续提交或复活尾块；清理完成后清除标记并唤醒写流。缩短前准备的写因内部版本不符必须重新准备。清理承载在首次可能需要它的节点准入时预留，截断后调用者消失仍由 provider 推进；节点整体退休接管同一 Data，不并行清理两次。这是内存 backing 的短期写入背压，不是观察对象产生的锁或 OAL 状态。

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

下表左列是设计基线 `6187ce1` 的迁移起点（纯路径，多数旧文件已随迁移删除），不是当前源码链接；当前面貌以 notes/impls 与实际源码为准。

下表左列是设计基线 `6187ce1` 的迁移起点（旧路径，非当前源码链接）；P1—P4 已按右列完成迁移，当前面貌以 notes/impls 为准。

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
| P1 协议接口与 provider 对接 | P0 | 拆出 liboal 的发现表达/接口，分开各领域协议及编解码，接收端直接调用真实 provider 方法 | Lookup 和文件调用都落在同一实际 backing；没有抽象层状态、框架前置走路或 OAL 业务封装 | 已完成；旧中间态运行记录见历史施工段 |
| P2 文件领域 | P1 的接口与入口 | 将文件协议独立到 libfs，承接现有实际文件 backing、客户端及 init/test 调用；由 provider/backing 实现小 I/O、流、目录、Watch、Copy 及收束 | 目标支持矩阵接通；普通操作无旧 Lookup 条件；不在协议层复制 backing，旧能力属性探针退场 | 已完成；G05 剩余项按最终协议和条件性证据记录承接 |
| P3 服务目录领域 | P1 的 OAL/RPC 表达；与 P2 联合验证文件入口发布 | 实现 service 协议和 registry 完成合同，迁入 provider 生命周期支持，改宿主/消费者 Register、WatchName、GetEntry 与 Namespace 绑定；删除旧投影和两阶段注册 | 替换/未知回复/控制队列引用/Watch/退款用例通过，libservice 不依赖 libfs/libfal | 已完成；遗留精确窗口见当前接力 |
| P4 组合收口 | P2、P3 接通 | 改名 test_fs，统一 Cargo/启动/initfs/验证锚点，删除 libfal 和剩余旧入口；回填实际 impls，完成结构检查与聚合验收 | 删除清单清零、验证矩阵有证据、历史缺陷处置明确；符合本篇交付条件后才归档 | 已完成；`a1ca124`、C01—C11、最终 acceptance 已核对 |

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

责任链接通后按[构建与验证手册](../BUILD-AND-TEST.md)执行 `just check` 和跨机制聚合 `just acceptance`；后者已包含 clippy 及规定平台/退出路线，无新风险时不重复单跑同一套检查。`just check` 只覆盖 os 及其依赖，不能代替用户态验证。本案未改调度域契约时不额外要求 hetero。长日志、进程清理和证据边界遵守手册。

新增测试须对应上表的独立契约；不会因改变 API 而机械保留旧断言，也不会因为删去旧锚点就宣称覆盖了新行为。受影响 impls 在代码接通后同步回填，运行结果和阶段状态只记录在本计划。

## 未完成责任与延期边界

| 项目 | 唯一 owner / 当前事实 | 触发、完成标准与保留期限 |
|---|---|---|
| 新实现的结构检查与独立代码 Review | P4 交付前结构检查已完成；固定提交 `a1ca124` 的独立 Review 已由统一报告完成核定 | 后续只有新的代码提交或明确新增设计交付时，按 REVIEW 登记增量审查 |
| 新协议与实际资源链尚未实现 | P1—P4、C01—C11 已接通真实两端和正式消费者 | 主体完成；N6、映射撤销/Bind 精确交错与 W05-04 仍按本计划和统一报告的条件门承接 |
| 迁移期间仍存旧路径 | 旧 libfal、旧协议表示、旧 Cargo/启动身份和旧测试入口已清理 | 当前不再保留迁移中间态；新残留只有在后续修改出现时按债务规则登记 |
| 旧 Copy 间歇失败 | [原问题档案](todo-2026-09-29-fal-midflight-copy-flake.md#2026-10-11-历史证据复核与最终处置)独占现象与处置证据；原精确根因仍未知，旧责任链已整体替换 | 已按旧同步Copy/pump/有限轮退休/合并断言删除、真实新消费者接通及分版本验证关闭；新路径若复现由当前责任链承接，不把机制替换宣称为已定位旧根因 |
| 设备/驱动与正式装配 | 各所属分案；不在本次实现 | 它们开始接入时使用本案协议；本案只保证扩展接点，不能宣称硬件资源或长期服务能力已交付 |
| 范围外文件能力 | [文件扩展计划](../todo-2026-09-fal-extended-operations.md)唯一登记高层跨域 Move、递归/元数据、持久化等触发与完成标准 | 本篇当前边界完整；实施不得借 Copy/Delete 隐式实现未设计的移动事务，也不重复维护该计划的延期清单 |

其它临时表示、清理缺口或范围外残留如在实施中出现，按项目债务规则加入本节，写明 owner、位置、触发、删除条件和验证。未经登记不得作为跨阶段默认前提；当前承诺内的正确性欠账不能延期。

## 完成条件与记录维护

- D0 已关闭，当前目标、接口与资源责任完整，实际支持矩阵与非目标相符。
- OAL、fs、service 的请求/响应两侧及 srv_fs/srv_init/test_fs 接通；基础库存在或局部检查通过不能替代真实消费者接入。
- 正常、失败、取消、超时、退出和退款责任逐项兑现，已提交工作不依赖消散的调用者。
- 删除清单清零，Cargo 身份一致，未留下旧库 facade、重复对象真值或未登记过渡路径。
- 相应验证矩阵、`just check`/聚合验收和交付前结构检查有明确结果；旧 Copy 问题按原记录处置，未覆盖项如实记录。
- 实现说明按当前源码回填，COMPASS 和关联计划指向有效入口；ideas 的线稿清理/审定与代码完成分开登记。

主体交付条件已满足；本篇因仍拥有 N6、映射撤销/Bind 精确窗口和 W05-04 等条件性证据责任而保持活跃，不把普通验收或有限次未复现写成已关闭。后续新的代码提交、设计交付或触发性故障按[REVIEW](../REVIEW.md)登记和接续；提交、push 和发布仍按各自授权边界执行。

## 工作记录与当前证据

以下设计轮次与 P1—P4 批次按发生时点保留；其中“当前”“下一步”“未完成”描述该批当时的断点，不构成现行施工指令。现行责任、范围和验证以[当前接力](#当前接力)、末节 P4 分级处置及[提交前机制收敛](#提交前机制收敛)为准，历史失败不改写成通过。

2026-10-09 第十轮历史记录：依据当时“完整整理、补全设计、避免实施走偏”的授权，将前两轮快照归档，主文保留唯一目标；补齐表示、路径/分页、文件原子边界、服务协议、授权/失败/退休和实施门。前轮的局部检查与咨询保留在历史快照，不自动计作本轮完整审查。当时仅文档修改，未运行 Rust 编译、host 或 QEMU，P1—P4 尚未开始；后续施工结果见当前接力和末节。

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
2. [Runtime](../../user/libraries/libexecution/src/runtime.rs) 的 stop 只冻结意图、advance 继续退休，spawn 失败返还 task owner，未排空的 close 返还运行体。Copy 的原已准入任务持续持有清理，业务结果可先交付，最后才 Complete；取消不临时申请第二个清理任务。
3. [RegistrationClientTask](../../user/services/srv_fs/src/server.rs) 已经通过 DownstreamState/dispatcher 非阻塞推进；迁移时删除 PublishReady，Register 成功直接持控制进入 Published，同时替换旧 ExportPolicy/ValueProtocol/FalRights 构造。现有机制足以支撑新就绪顺序。
4. object ID API 已核实：[object::query](../../user/libraries/libkernel/src/ipc/object.rs) 返回 HandleDescription，[MailboxSender::description](../../user/libraries/libkernel/src/ipc/message.rs) 可取得 sender 身份；[GrantTable](../../user/libraries/libfs/src/grant.rs)（当时位于 `libfal/src/grant.rs`） 已在 new/prepare_minted 中取得 mailbox object_id、校验 related_object_id 并使用 sender object_id。StreamOffer.session_id 与 incarnation 不需要新增内核接口，也不留作 P1 的未决前置。
5. 互核发现旧 output_transport 仅接受 TRANSIT/GRANT，不能直接沿用。新输出上限统一为完整 Rights，运输字段与 shared 一致为 u64；入口自身权限、新授权出口及实际交付角色分别受限。已同步 DeriveDirectory/Bind/Register 线形、请求长度、旧检查/装配参数迁移和验证要求；不使用兼容分支或无条件 Rights::ALL。
6. try_reserve 不单独证明提交无分配。每个并发准备持有自身可兑现的容量/槽位，P2/P3 按实际容器闭合并记录；这些是实施责任，不是需要用户另行选择的架构方案。

接手核对结论：D0 保持关闭；本轮只澄清设计与移交，未改 Rust、未执行代码验收。用户安排实施代理后，由 MellowPond 按下节组织施工及回填记录。

交接补文经 MellowPond 再次核对，明确回复“与共识一致，无偏差”，并独立复算三种加宽请求的长度。补文后 7 份 Markdown 的 122 个本地链接/锚点、活跃表格列数和代码围栏检查通过；逐字段编码复核确认 DeriveDirectory/Bind/Register 的新长度及 u64 高位不截断，`git diff --check` 通过。这里只验证设计表示；未知 Rights 位的运行时拒绝及实际授权/清理仍须在 P1—P4 按验证矩阵兑现。

### P1 第一批实施记录（2026-10-09，RubyRaven）

MellowPond 核对理解摘要后批准本批 a/b/c，实施基线为 `4523bbf`、工作树干净；本批只完成公共编码和发现表示的基础切面，**P1 未完成，实际 provider/消费者尚未接通新 OAL 协议**。设计正文与当前接力由 MellowPond 维护，本条只登记代码事实、执行证据和断点。

**代码与归属。**

- 将 `libfal/src/bytes.rs` 真正移入 `librpc/src/bytes.rs`，增加 LE u128、u32 长度段和仅校验 UTF-8 的 Text 读取原语。工具无 Deadline/Rights/路径语义；保留 u16 长度段是仍在使用它的旧协议所需的字节原语，不是新协议双编码或兼容 adapter。
- `libfal::{protocol,route,value}`、`libservice::{protocol,record}`、`srv_fs::server` 同批改用 librpc 的唯一工具位置；删除旧 `pub mod bytes` 与旧文件，没有重导出。对应源码迁移表的公共字节工具项，旧 libfal 包及其其它领域机制按既定 P2/P3/P4 继续替换。
- 新建身份一致的 `user/libraries/liboal`，仅依赖 librpc/erhino_shared；`info` 拥有开放 MarkerId、唯一 OAL Directory 常量、ObjectInfo/tag，`path` 验证线上相对路径，`boundary` 拥有两种接续表达和相对于原路径的完整覆盖验证，`protocol` 拥有 OAL 头、Lookup/Enumerate、Page、请求/回复编码与实际能力槽位图校验。没有 fs/service 常量、NodeKind、领域 backing、通用业务信封或执行层。workspace 和 user/README 同步组件身份与名称展开。

**公开表示、构造与资源。** ObjectInfo/Page 是完整验证后的借用报文视图，MarkerId/Tag/Header/Request/Boundary/DirectoryEntry 等为固定字段或借用数据的值类型；未引入拥有 heap 容器的 OAL 对象。ObjectInfo 的 traits/tags 和 Page 的 entries 迭代器只从不可变的已验证私有构造视图产生；重复 Trait/tag 键、未验证 count/length、截断和尾差不能越过构造入口。未知数字和 tag 值原样保留，不检查领域组合。提供端用 `info::{encoded_len,encode}`、`protocol::{encode_page,encode_request,encode_response}` 写入调用方给定的可变缓冲；所有编码先验证完整长度/结构和容量，再开始写入，没有分配、索引插入或背后状态。B 引用 shared PAYLOAD_MAX、RPC PREFIX_LEN 和本协议 HEADER_LEN 推导。ObjectInfo/Page 不保活领域对象；报文 owner 须在视图使用期间保持缓冲存活。

本批没有新增生产容器、容量 permit 或提交/退休状态，只有借用 slice 和局部游标；因此 P2/P3 的每容器预留/收费/并发兑现门未在本批实施，也未把本地编码检查当作提交必成证据。ObjectInfo 重复键检查在完整报文上作有界扫描，输入先受 B 约束；不根据远端 count 申请内存。Page 先检查 count 的最小剩余编码长度，再逐项验证。请求能力检查要求实际槽 0 为既有 RPC 回复权且无业务输入；回复按结果变体要求恰好槽 0 的 ProviderBoundary 或零业务能力，typed owner 转换和拒收清理由下一批实际 RPC 接收者接通。

**开发验证（rustc `1.100.0-nightly (c54751567 2026-08-22)`）。** 以下均非系统验收，日志位于仓库 artifacts/oal-p1：

| 检查 | 完整命令 | 结果 / 日志 |
|---|---|---|
| Host 单测 | `(cd user && cargo test -p liboal -p librpc -p libfal -p libservice --target aarch64-apple-darwin)` | 退出码 0；liboal 16、librpc 16、libfal 34、libservice 17，合计 83 个单测通过，doc tests 通过；`artifacts/oal-p1/host-tests-pass.log` |
| Host lint | `just --command bash -c 'cd user && cargo clippy -p liboal -p librpc -p libfal -p libservice --all-targets --target aarch64-apple-darwin -- -D warnings'` | 退出码 0；`artifacts/oal-p1/host-clippy.log` |
| 现有用户态 target 消费者 | `just --command bash -c 'cd user && cargo check --workspace --bins --exclude test_fp --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem'` | 退出码 0；实际自定义 RV64IMAC target，含 srv_fs 的工具 import；`artifacts/oal-p1/user-target-check.log` |
| 新 OAL target 库 | `just --command bash -c 'cd user && cargo check -p liboal --lib --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem'` | 退出码 0；现有 bins 尚未依赖 liboal，故单独检查；`artifacts/oal-p1/oal-target-check.log` |
| 删除证明 | `rg -n '(libfal::bytes\|crate::bytes\|pub use[^;]*bytes)' . --glob '*.rs'` | rg 退出码 1 表示全仓 Rust 源码零匹配；`artifacts/oal-p1/removed-bytes-grep.log` |
| Diff 结构 | `git diff --check` | 退出码 0 |

新增用例各自注释其独立合同和目标错误，覆盖原语的手写 LE 字节、开放数字/未知值、重复键与恶意 count/length、线上路径、UTF-8 偏移与精确后缀、帧头/期限/操作回显、能力槽、分页初次/续页互斥、done/进展/空摘要及整体容量。固定帧测试按计划字段独立组帧，不以生产编码器互相 round-trip 作为唯一依据。未覆盖真实 provider 的目录范围/游标身份、授权衰减、两端能力清理或业务生命周期，这些仍属于 P1 后续与 P2/P3 的既定验证门。

**初次失败与处置。** `host-tests.log` 退出 101：一个页容量夹具少计 4 字节 cursor 内容，实际开销为 result/cursor_len/done/count 16 + cursor 4 + 两个项长度前缀 8 = 28；实现正确返回 EntryTooLarge。`host-tests-final.log` 退出 101：新增 u128 golden 用例的数字常量少分组而与手写字节不一致，已改为分组明确的固定常量。两者均为测试夹具错误，未发现或修改设计合同，原失败日志保留；修正后上述最终 host 检查通过。

**本批结构检查及断点。** 按 REVIEW 检查工具唯一归属、公开表示私有验证入口、输入扫描上界、能力槽合同和 Cargo 依赖；生产代码没有 unsafe/heap 容器/锁，也没有能力 owner 或取消任务。迭代器 expect 的不可达依据是只能从已完整验证的不可变视图构造，原语 writer 的容量断言由调用者完整长度预检成立。没有新增兼容债务或合同反例。待接通表示仍由本计划 P1—P4 拥有：实施者下一批承接 Namespace/私有 walk/调用预算与 OAL 客户端，随后成组切换领域协议和真实 provider 分派；至 P4 前接通消费者并删除旧领域路径，按本篇验证矩阵验收。libraries/README 与领域 impls 按协调要求留到实际接通后回填，不把本批局部编码能力写成系统完成。未运行 QEMU、just check 或聚合 acceptance；本批未 commit/push。

### P1 第二批实施记录（2026-10-09，RubyRaven）

MellowPond 批准 Namespace、视图词法路径、私有 walk/预算、同步 OAL Client 与分页 cursor；确认既有 RPC 接口足够，并要求回复完整验证一次、生产/host 共用一份入口泛型状态算法。本批已完成这些基础接口，**P1 仍未完成**：新 OAL 尚无实际 provider/验收消费者，fs/service 线上协议及 provider 未在本批改动。用户要求 P1—P4 全部完成后一次提交；本批及第一批均保留工作树，没有 commit/stash/reset。

**代码事实与合同映射。** 新增 `liboal/src/{budget,view,namespace,walk,client}.rs` 和 `walk/tests.rs`，并调整 `lib.rs`/Cargo。Namespace 与公开 Walk 只使用真实 `Arc<MailboxSender>`；liboal 因真实入口类型与 target 客户端新增 libkernel 依赖，仍不依赖 fs/service。Namespace 的 bind 只接受规范绝对前缀，重复返回本地 Exists；replace_binding 只替换已有精确项（缺失 NoBinding），成功返还旧 owner；unbind 返还原 owner，失败的 bind/replace 完整返还输入 Arc。最长匹配按分量边界，根绑定可缺省。本地操作不发 RPC、不查询 backing、不制造目录摘要。

view 在显式规范 cwd 下做绝对/相对词法规范化，默认 cwd 由 Client CallOptions 取 `/`；折叠重复斜杠与 `.`/`..`，越过视图根 InvalidPath，被抵消分量没有 RPC。初始扫描/容量在执行扫描前扣预算。私有 `State<E>` 的唯一泛型是入口句柄，生产用 Arc sender、host 纯逻辑用数字入口，二者执行相同算法，没有 cfg(test) 算法或 mock 运输。帧只持入口与视图根，State 持规范路径及原 FollowBudget。ProviderBoundary 推帧，空剩余路径查询新入口根；LinkBoundary 从规定父位置拼 target/remaining，相对目标保留未越过的帧，越根退上层；若曾越根又返回旧前缀也不重新复用已退出的旧帧。绝对目标从 Namespace 重选，最初绑定之外没有上层帧时也重选。公开只提供 target/follow 推进，不暴露 Position/帧，也不解码领域响应。

**三项责任核对。**

- 预算交接：FollowLimits 是调用方给定的非零 calls/expanded_bytes/有限相对 timeout，每次方法起始结合当前时钟与调用者期限建立较早的有限 deadline；整条 Boundary 链始终用同一 FollowBudget。before_call 先扣次数，路径/帧准备先扣字节，follow 先核期限，不复用旧 40/256 常量。Walk 的 budget_mut 允许内部阶段继续扣原余额，into_budget 释放路径帧并把剩余预算移交下一条路径，供 Rename/Copy 接入；该 getter 本身不宣称两领域消费者已接通。cursor.next 是显式的新方法调用，按本次调用期限建立自己的预算，不在同一 Boundary/内部阶段重置。
- 入口释放：Walk::before_call 和 follow 出错均清空帧及暂存路径，新建/准备失败的 Arc 随局部 owner 释放，into_budget/Walk Drop 同样释放帧引用。Client 的运输/业务/损坏回复错误直接返回，栈上的 Walk/Reply 同步收束；异常槽或错误回复不先取 slot0，HandleSet Drop 清理全部收到能力。Namespace 显式绑定与成功 EnumerationCursor 仍持它们自己的 Arc，这些真实 owner 不被误关。RPC CallError 的未完成 request/ReplyCleanup 或 SenderAdoptFailure 仍由显式错误值持有，不丢弃其 owner；本批没有映射/来源退休责任。
- Namespace 固定借用：bind/replace/unbind 要求 `&mut Namespace`，Client/Walk 持 `&Namespace`；字段没有内部可变性，编译期排斥 walk 使用中修改绑定或混用新旧表。

**RPC 与结果 owner。** Client 借用已有 Caller，发送只调用 Caller::call_cancelable，不自建等待器/Runtime；Header Deadline 与实际调用使用同一本次预算期限，取消能力只借用到方法结束。先按响应变体核能力槽，再在 take 前 Query 真实 sender role 与 WRITE|WAIT|TRANSIT，之后用现有 MailboxSender::from_capability 转 typed owner（该既有边界再次 Query；未为减少 Query 改内核/RPC）。转换失败保留其返还的能力 owner。只有明确 Boundary 接续；Unsupported、超时、取消、损坏回复或失联不补 Lookup、不重放。

Lookup/EnumerationPage 保存 RPC 原 payload 与验证产生的私有布局，没有自引用/unsafe；构造时 decode_response 完整验证一次，后续只按保存布局提供借用 ObjectInfo/Page。**Page 的构造不是只检查外层：Page::decode 对每个非空 info 调用 ObjectInfo::decode，完整核过 Trait/tag 唯一性、计数/长度、截断/尾差。** 后续 entries 只提取已验证的字段，view_validated 不重复全量键检查。原 payload 和布局不对外可变，保证该省略成立。EnumerationCursor 只持原应答入口 Arc 与 opaque bytes；next 发空 path/cursor 到该入口，不读 Namespace，不缓存路径、不跟随续页 Boundary；释放 cursor 释放入口引用，provider 无扫描会话。

**本地容器预留与释放（不替代 P2/P3 backing 提交 permit 门）。**

| 容器 / owner | 准备和并发兑现 | 释放边界 |
|---|---|---|
| Namespace 的 Vec<Binding> 与每项 String | bind 先用 String/Vec try_reserve_exact 准备，再插入；整个操作独占 `&mut Namespace`，没有异步准备者共享空闲槽。replace 不分配 | unbind/replace 返还真实 Arc，名字 String 按 owner 释放；Vec 保留容量仍属于 Namespace，直到它 Drop，不宣称卸载一项就退款 |
| State 的 Vec<Frame<Arc<MailboxSender>>>、路径/root String | 每次帧/路径准备先扣原 expanded_bytes，Frame 用实际布局大小，字符串用输入/所需容量，再 try_reserve_exact；单调用独占 State，不存在并发消耗它的预留。被抵消分量和退出帧不返还已花的展开预算 | 退帧/失败/阶段结束释放引用及字符串；保留 Vec/String 容量仍归 Walk，最终 Drop 释放。预算是本次累积展开上界，不冒充 allocator 的硬收费 |
| 单次请求 Vec 与既有 RPC Reply payload | 请求按完整 encoded_len 做 fallible reserve，再写入；RPC Request/Reply/能力 storage 由既有运输 owner 承接，完整报文受共享 ABI 上界约束 | 每次 Boundary 后旧 Reply 释放；最终 payload 移入 Lookup/Page owner，借用视图不另存摘要或镜像 |
| EnumerationCursor 的 opaque Vec / 新入口 Arc | 先扣 cursor 长度与记录布局，再 fallible reserve/copy；新 sender Arc 用 Arc::try_new，失败按合法 typed 非映射叶 owner 的既有 Drop 合同收束 | next 消费旧 cursor；失败/释放时 Vec/Arc Drop，新 cursor 仅在成功页结果显式持有 |

上述是进程本地调用和绑定内存，采用进程所选 allocator，不在 OAL 建账户表；P2/P3 的真实 backing/控制/退休容器仍须逐项实现可兑现 permit 与实际收费，不能据本表宣称已满足。

**开发证据与覆盖边界。**

| 检查 | 完整命令 | 结果 / 日志 |
|---|---|---|
| Host 单测 | `(cd user && cargo test -p liboal -p librpc -p libfal -p libservice --target aarch64-apple-darwin)` | 退出码 0；OAL 27 / RPC 16 / FAL 34 / service 17，合计 94 单测及 doc tests 通过；`artifacts/oal-p1-batch2/host-tests.log` |
| 最终 host lint | `just --command bash -c 'cd user && cargo clippy -p liboal --all-targets --target aarch64-apple-darwin -- -D warnings'` | 退出码 0；`artifacts/oal-p1-batch2/host-clippy-final.log` |
| target 编译开发检查 | `just --command bash -c 'cd user && cargo check -p liboal --lib --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem'` | 退出码 0；`artifacts/oal-p1-batch2/target-dev.log` |
| 最终 target lint（含最新预算交接出口） | `just --command bash -c 'cd user && cargo clippy -p liboal --lib --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings'` | 退出码 0；`artifacts/oal-p1-batch2/target-clippy-final.log` |
| 结构/范围 | `git diff --check`；检查新 Cargo、公开入口类型和实际 RPC 调用位置 | 退出码 0；无 fs/service/RPC/provider 编辑、无兼容 adapter/运行体 |

新增 11 个 host 用例依据本篇合同覆盖规范绝对前缀/词法路径、分量最长匹配/根可缺省、Provider/Link 交替、空后缀、相对越根退帧/越根再返回、绝对重选、调用/展开循环预算和期限新建/较早值/溢出。**这些是同一生产算法的纯逻辑 host 证据**，没有真实 RPC 或 kernel sender。Namespace 的 bind/replace/unbind 真正 owner 操作、Walk 的真实 Arc 清理、Caller 往返、sender role/rights 交接、结果 owner/cursor 生命周期目前仅有源码责任检查及自定义 RV64IMAC target 编译/lint，未以 host 数字入口当作这些行为的证据。第一批 Page/info 的全量结构检查用例继续通过；没有新造 mock Runtime、服务或测试运行体。

开发 host-dev.log 退出码 0、OAL 27 用例通过，但提示 allocator_api 在 host 未使用，已将 feature gate 限于 riscv64；此后最终 host/target lint 无 warning。预算交接出口在最后结构检查后补入，所以最终 lint 覆盖它；该纯所有权 getter 没有新增镜像测试。无合同反例、新兼容债务或额外延期。仍保留的旧 libfs prefix/resolve/Position、旧领域协议与 provider 由本篇既定 P1 后续/P2/P3 同切面切换并删除，最迟 P4 清零；新 OAL 的真实能力/游标/接续链在实际消费者接通后按验证矩阵复核。下一批建议开始成组领域协议切换及同 backing 的 OAL/provider 对接，再推进文件/registry 责任链。未运行 QEMU、just check/acceptance，未更新 impls 为系统完成事实；当前接力仍由 MellowPond 维护。

### P1 第三批实施记录（2026-10-09，RubyRaven）

MellowPond 批准 fs v1 编解码、文件 backing 的 OAL 直接视图、共同成员准入、实际消息 kind 分派、完整运输上限及容器预留登记，并追加要求同批移出/删除被承接的旧源码。原 P1/P2/P3 是联合迁移闭包，中间消费者允许不编译；本批不宣告系统能力或 P1 完成。当前没有 commit/stash/reset，用户的全部完成后一次提交决定继续有效。

**当前代码事实。** `libfs` 不再依赖 libfal；authority/node/protocol/value/backend/store/data/grant/resource 以 git mv 移入目标 owner 后修改。fs v1 定义 op 1—21、全部固定响应及纯值，独立 Bind/Unbind codec 已具备。OAL Lookup/Enumerate 直接读取唯一 MemoryBackend；目录项包含 Local/Binding，不存投影 ObjectInfo。真实内部 serial 与弱父关系供范围验证，cursor 为 mailbox incarnation + directory serial + after_name；续页空 path、UTF-8 顺序、当前范围及权限逐页复核。共同成员准入覆盖现有 Create/Binding 安装/Rename，后续 CreateOpen 接入同一函数。

`provider::Request` 以 ReceivedMessage 的真实 kind 选择 OAL/fs；未知 kind、损坏头及错型能力返还完整 RPC owner。未知版本/op 在可信头及回复权下返回本协议 Unsupported。任务收费并持真实 AccessSnapshot，先准备完整 Packet/Outbox 和 CLOSED 来源，再借用 backing 解码/执行；变更预编码成功体并最终复核期限/授权后无分配 commit。已经提交的回复丢失不回滚。已接本地 Get/Set/ReadAt/WriteAt/Create/Remove/Rename/ReadLink，流/Watch/SetLength/目录派生任务留在既定 P2 后续。Sync 的内存实现 Unsupported。Binding 的共享入口仅在完整领域/运输上限已经足够窄时导出，需异步更窄签发时目前 Unsupported，不泄露母本；管理接收者和实际来源/退款接线仍未具备。

`srv_fs/server.rs` 已删除旧 ServiceBackend 的 registry/NodeRef 分流、旧 Ingress/serve_v2、Read/Delegate/Take 等旧文件请求路径，现有 World/FileTask/Ingress 直接持新文件 backing 并调用新 Request；没有为中间编译恢复旧分支或 facade。根配置使用 sender/Runnel 实际角色需要的完整出口位并集，未使用 Rights::ALL。srv_init 的 Bind helper 已改完整 u64 输出上限、fs 管理 Header 和实际输入运输位，其其余旧文件/服务剧本尚未迁移。

**每容器预留与收费。**

| 容器 | Prepare 的实体预留与收费 | 并发兑现及提交 |
|---|---|---|
| NodeStore.records | Counter 的独立 Permit、Node/Bytes Charge、预建 PreparedEntry<Record>、真实 NodeRef/Lease | 每个同时准备各占一槽，prepared+resident 不超过上限；commit 只 insert_prepared，最后引用进入预算退休 |
| Directory.members | 全 backing 成员 Counter；独立新成员 Permit，名字 capacity + AVL entry Bytes Charge 和 PreparedEntry | 同目录改名先准备新索引后转移源槽；Binding 替换转移旧槽；版本复核保证可插入位置，不共享一个 capacity promise |
| Data.blocks | 新块单独占 Counter Permit；块及索引 Bytes Charge、预建块/entry | 覆盖旧块转移其槽，版本复核后只移除/插入已准备记录；退休逐块归还 |
| GrantTable.grants | 每个准备独立 Permit，Grant/WaitSource/Bytes Charge，Rc<GrantLease> 与预建 entry；存储费包括 lease | install 只消费预建 entry；调用者必须先登记 Lifetime，记录及请求使用引用释放前不退款 |
| Request/Outbox/Ingress | 请求持 Request/Outbox/Bytes/WaitSource Charge；完整 Packet/能力空间提前准备；Ingress 在 Receive 前持 MessageStorage | 来源由既有 Runtime/Outbox 登记/撤销；实际 backing 变更前来源已 admitted；没有新扫描会话、通用执行容器或持久摘要 |

本地变更提交没有已知容器分配。尚未接线的 Lifetime、Binding 下游、会话、订阅、后台退休及进程停止 owner 必须在既定后批完成，不能把 library helper 的存在当作这条运行责任链已经交付。

**开发证据。** 日志均在 `artifacts/fs-p1-batch3/`。最终 host tests 退出码 0：libfs 22、liboal 27、librpc 16 项；包含独立 LE 字节/固定长度、完整 u64 高位拒绝、纯值恶意形状、同 backing 投影、D-F1 原反例（2200 路径/1825 名字，3722 首页/1857 续请求）、D-F2 2000 Stream 名提交前拒绝、目录移出范围/实例变化、prepared 冲突和 revoke、真实 backing 防环/跨 backing、新 sender/new output 各自不放大，以及下述结构审查反例回归。没有 Kernel sender/IPC 模型或测试专用运行体；纯 backing host fixture 沿原测试方式构造本地授权。

- Host lint：`just --command bash -c 'cd user && cargo clippy -p libfs -p liboal -p librpc --all-targets --target aarch64-apple-darwin -- -D warnings'`，退出码 0，`host-clippy-final.log`。
- RV64IMAC lint：同三库 `--lib --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings`，退出码 0，`target-clippy-final.log`；已包含 grant/provider。
- 目标开发 check：libfs 退出码 0，`target-dev.log`。host 的 check 由 all-targets clippy 覆盖；host 未执行 Kernel IPC，target 未运行 QEMU 测试。

**迁移编译断点（均有真实退出码 101 日志，不造 adapter）。**

| 消费者 | 当前失败入口及原因 | 接通/恢复责任 |
|---|---|---|
| srv_fs | `src/main.rs:51` 仍调用已经删除的 `server::run`；新的 World/FileTask/Ingress 类型体可检查，但启动 Runtime、根 Lifetime/退休、primary/secondary 发布及独立 registry 未重新装配 | 第四批服务 v2/registry 与独立根装配，联合 P2 文件生命周期任务 |
| libservice，继而 srv_init/test_fal | `protocol.rs:5`、`record.rs:6`、`registry.rs:16/18/1000` 仍 import 旧 PATH_MAX/FalRights/Value/NodeStore/Backend/watch API；新 owner 不提供兼容出口 | 第四批按服务 v2/真实 registry 替换旧 Record 与文件投影，同批切消费者 |
| srv_init/test_fal 自身后续断点 | 依赖错误当前先阻断；源内仍用旧 libfal Client/协议及 libfs prefix/resolve/Position/DirectoryGrant，新 libfs 不导出这些接口；Bind 调用还需新的 FsRights | P2 普通文件客户端与正式调用切换；同批删除旧 prefix/resolve/Position，不推迟至 P4 |

srv_fs 已确认最终只剩 main 的 run 接手断点，见 `srv_fs-migration-final.log`；初次检查的误 import/allocator feature 已修复。srv_init/test_fal 的依赖断点见各 `*-migration-check.log`。当前未运行完整 just check/clippy、QEMU 或 acceptance，不能把局部库 lint 当联合迁移验收。

**删除/移出清单。** 对照实施基线 HEAD：authority 97、node 131、backend 2252、protocol 1418、value 1064 行从旧 libfal 路径移出/重写；data 218、store 600、grant 409、resource 34 行以 rename 显示，其源码只在 libfs 生效。`libfal/protocol/stream.rs` 的282行双编码删除；本会话第一批已移出的 bytes 205行仍计入总 diff，不能算第三批重复删除。srv_fs 的 mixed server 实现最终 diff 删除侧5545行，新增侧275行，取而代之为具体文件任务族。原 libfal 公共导出和512路径限额移除，不重导出新库。

暂存旧文件：libfal client/provider/watch/route（不再编译/导出，P2 客户端/生命周期及第四批装配切面结束删除）；libfs client/prefix/resolve/client/copy（不再被新 lib.rs 编译，P2 消费者切换同批删除）；srv_fs server/authority/registration/stream 旧任务文件（没有旧 server module 声明，第四批注册或 P2 流任务重组后删除）。旧 libservice 代码由第四批整体替换，其未决债务不另建第二份计划。旧源码只供核实，不作为目标参考。

**结构检查与修订。** 作者按 REVIEW 核对唯一 owner、权限/真实目录引用、模板尺寸、容器预付、取消/失败与所有 unsafe 后，另请 reviewer 对新文件基础和真实 ingress 做只读审查。原始发现为3项P1、2项P2，全部在本批修复；这不是固定提交后的独立 Review。

| 原始反例 | 本批修复及复核 |
|---|---|
| 先准备 X/A→D，另一提交把 Y/Z→A，X/D 的版本仍旧匹配却会形成目录环 | Prepare/Commit 复用同一 NodeLimit 父链验证，最终独占借用重新防环；弱父无法确认返回 Conflict。手写交错回归确认拒绝、名字/内容保留 |
| Remove/替换准备时目录为空，其内部随后 Create 不修改父版本 | Commit 重新检查被删除/替换目标为空；两种反例均返回 NotEmpty 并保持原名字和新增内容 |
| 长符号链接只收费固定节点/短名字，正文没有 Charge | Link body 独占 target capacity Bytes Charge；长正文 prepared 增费与放弃退款回归通过。Binding 同样用收费的共享状态持撤销位/真实入口，最后状态引用释放才退该费 |
| Runtime Task 满载后 ingress 永久重试、无限持有限请求/自旋 | pending 暴露原 deadline，超时立即收束未登记 owner；spawn 拒绝在 ingress 原任务沿已准备 Outbox 发 Quota，不重试业务，等待回复时暂停 mailbox READABLE。停止先退回复来源；ingress MAX_SOURCES=2（mailbox+拒绝回复），实际启动接线仍待第四批 |
| 只检查 Offer 自身形状，Read/CreateOpen 可接受错误方向、offset/范围/期限 | 成功解码结合原 Open/CreateOpen 核对方向、start、可选 read 上界和请求/session deadline；独立损坏回复用例通过 |

最终 host 三库22/27/16测试、host及RV64三库 -D warnings lint 均通过；srv_fs 真实新任务体复查后仍只剩已登记 main::run 装配断点。已通过检查不继续重复；后续新增/迁移再按对应风险补测。准入满载的实际 Kernel/WaitSet 交错、Typed能力/RPC清理、Grant Lifetime 最终提交复核、Bind异步衰减/来源退休、流/Watch/Copy与多域启动仍由本篇既定P2/第四批验证门拥有，未取得运行证据之前不报完成。

静态实现说明已由 fs 篇拥有当前文件机制，旧 fal 篇清除失效生产链；libraries/README 与 notes 索引同步知识归属，KNOWN_ISSUES 已登记实际半迁移范围。下一施工从第四批 service v2/registry/独立根及 P2 剩余任务继续，复用当前 sole backing/RPC入口，不恢复旧消费者接口、不重做已通过的纯编码测试。没有 commit/push；P1和整个任务仍未完成。

## 当前接力

提交前 C01—C11 的调查、代码/文档修改及验证均由 **GrandOwl 亲自完成**；**MellowPond、RubyRaven** 仅提供先前事实与历史线索，没有编辑或测试。分支 `refactor/system-redesign`，施工基线 `4523bbf`；交付提交为 `a1ca124fdb70c1f20034f3e7d24d587f4323c98e`，[独立 Review 统一核定](../review-2026-10-12-consolidated.md)已完成，未发现新的可达代码 finding，仍有效的精确证据缺口按总报告映射由所属专题承接。最新证据为 `artifacts/convergence3/acceptance-summary.json`：libfs43/librpc16 共59项 host、目标 lint、just check、原失败 nofd 路线、七面 clippy 与完整 just acceptance（784.55秒）通过，验收期间源码指纹不变。C10 删除下游派生与 Copy 的普通叶能力清理暂存，C11 修复聚合发现的同目录不同名字 CreateOpen 被父目录整体版本误判 Conflict，并补提交前当前授权范围和版本耗尽检查；没有串行化两个 Copy、自动重试或放宽验收。首轮 nofd 失败、host 修前确定反例及各自源指纹保留。convergence/convergence2、P4 与旧40门仅作为各自基线的历史证据，不视为当前源码重复验证。N6、映射撤销与 Bind 特定迟到交错的注入前置及 W05-04 历史未知错误继续保留；旧 midflight Copy 已按[历史重新核定](#历史问题重新核定2026-10-11)的整体机制替换依据关闭归档，不宣称已定位旧根因。**用户接受上述遗留由文档承载，本轮施工结束；后续只按明确验证触发、新需求或提交后独立 Review 接续。** 主计划因仍拥有专项验证条目保留为活跃承接记录，不将未覆盖项归档为已关闭。

恢复工作先核对工作区与本轮源码指纹，保留全部原实施和规则/工具改动。只在出现新修改、新失败或具体未解决风险时补相应验证；现有精确注入窗口不以额外普通运行冒充关闭。本轮提交已获授权并完成；后续新提交仍需单独授权。固定交付的独立 Review 已完成并统一核定；后续仅按总报告中的有效责任与本计划的明确触发接续。

实施者可决定 Rust 内部写法、容器和文件拆分，但不能自行恢复 DirectoryGrant/强制 Lookup、服务文件投影、两阶段发布、按路径 Copy 回删或跨域移动补偿。若源码证明本篇的关键前置不成立，先在本篇记录反例并修正相关责任链及验证门，不叠加兼容 adapter 让中间态通过。设计门已关闭不豁免实施中的结构检查、实际消费者接通与验收。

### 历史施工记录：P3 第四批开发断点（2026-10-09，RubyRaven）

以下内容是当时的开发中间态，保留用于追溯，不代表当前状态。后续 Notification 前置、P4 施工和最终聚合已完成，当前结论以本篇开头状态和“当前接力”节为准。

**当前实现与删除。** service v2、Registry、DirectoryAccess/NameAuthority、请求/根 Lifetime、Maintenance 和客户端已替换旧 ServiceRecord/NodeStore 投影。Registry 直接生成 OAL 面貌；Register 单步提交，控制 Lifetime 与业务 endpoint CLOSED 两个来源先准入，最终 probe/权限/期限复核后公开。srv_fs 文件/registry 分别持 mailbox/backing，共用 Runtime/Assembly；secondary 用 Dispatcher Register/Withdraw。普通 fs Client、srv_init 和现名 test_fal 已用 Namespace/Walk 及领域请求，旧 Position/DirectoryGrant 没有兼容出口。

删除清单对照 HEAD，仅列路径；总 diff 包含前三批，不将整段累计行数算为第四批新删除：

- 删除旧 libfal 包及 Cargo workspace 成员；剩余 client/provider/watch/lib.rs/Cargo.toml 已删除。原 bytes、protocol/stream、route 及已迁入 libfs 的 backing 等沿前三批记录承接，当前目录无 Rust/Cargo 文件。
- 删除旧 libfs prefix.rs、resolve.rs、client/copy.rs、client/copy/pump.rs；client.rs 已整体换成直接领域路径客户端，不保留旧实现。
- 删除旧 srv_fs server/authority.rs、registration.rs、stream.rs；旧 mixed ServiceBackend/serve_v2 已由前三批移除。
- 删除 libservice record.rs/ServiceRecord、libfal 依赖；libservice 不依赖 libfs；liboal 不解释领域类型。
- 删除 init/test_fal 中旧流、Watch、Copy、Bind、能力属性/Take 剧本代码；没有注释保留、cfg 开关、占位日志或放宽验收。
- Justfile host 库 lint 的 libfal 项换为 liboal；tools/qemu-acceptance.sh 保持严格且无 diff。test_fal 改名、启动/initfs/验证锚点统一仍属 P4。

静态说明回填 notes/impls/{fs,service,fal}.md 及 notes 索引、libraries/README；KNOWN_ISSUES 保留矫正范围。

**三类开发证据。**

| 分类 | 实际边界 |
|---|---|
| 已切新协议且真实运行到 | 三次 virt 均启动 primary srv_fs，收到文件/registry/authority 根，调用 service WatchName；第一次仅有拓扑不完整，第二次定位为 Invalid，第三次确认 Permission。未进入独立文件消费者或任何 FS1/SRV2 成功路径 |
| 仅编译/lint 通过，未实际运行 | GetEntry/本地绑定、直接文件属性/数组/链接/小 I/O/分页、注册 A→B→C/旧控制不删新注册/旧 Ack、secondary 出生后 authority IPC 交付及显式 Withdraw、正常退出收束、generic cancellable RPC。最后权限/寿命/启动修订发生在第三次 virt 后，按协调者指示未重跑 |
| 第五批需补证 | 下列 G05 场景；相应真实能力接通并复用现有消费者后再验证，不把已删场景记为已通过 |

最终静态日志位于 artifacts/service-p3-batch4/：user-workspace-target-final3.log（RV64IMAC workspace bins，按构建手册除独立 gc 的 test_fp）、user-workspace-target-clippy-final4.log、libraries-target-clippy-final.log、user-gc-clippy-final.log、service-host-clippy-final.log 均退出 0；service-host-tests-final.log 退出 0，10 项纯编码/授权容器/序号测试通过。最后 lifecycle 根关闭/revoke 同类修订已包含在上述最终 target check/lint。host 没有模拟 Kernel sender/IPC；新控制运输默认值与索引移除/显式 revoke 用例分别来自第 410/437 行合同，不将源码当前输出当预期。早先开发失败日志保留，不冒充最终通过。98 个当前说明/导读本地链接目标、第四批表格列数与 git diff --check 通过。未完成聚合 just check/clippy、完整 acceptance 或交付前结构检查。

| QEMU 开发运行 | 完整驱动日志 | 保留的 guest 日志 | 退出码与第一失败 |
|---|---|---|---|
| virt 1 | artifacts/service-p3-batch4/qemu-virt-dev.log | artifacts/failed-acceptance-20261010-010949-99864.log | 124；required topology incomplete，未到文件/服务验收 |
| virt 2 | artifacts/service-p3-batch4/qemu-virt-dev2.log | artifacts/failed-acceptance-20261010-012041-1817.log | 124；WatchName Status(Invalid) |
| virt 3 | artifacts/service-p3-batch4/qemu-virt-dev3.log | artifacts/failed-acceptance-20261010-012614-3061.log | 124；WatchName Status(Permission) |

三次均为 HEAD 4523bbf 上本次 dirty 实施中间态，日志时间及修订顺序如上；不是固定提交证据。路线在 init 失败后发 SystemFailure 复位，再被 90s watchdog 判失败。不是“只缺旧 FAL 成功锚点”；第一业务失败在 Watch 建立之前。runner、QEMU 和 throttle 进程已结束。

**反例、已修复 findings 与复核。**

| finding / 合同 | 修复与证据 |
|---|---|
| service 在 Outbox 接管必需回复槽 0 后，用剩余 HandleSet 将合法请求判缺槽；WatchName Invalid | provider::slots 恢复已验证且由 Outbox 持有的原回复槽 0，仍逐项检查真实业务槽。第三次真实 WatchName 不再 Invalid，走到下一通知前置 Permission |
| DelegateName 硬附 GRANT，Register 控制硬附 DUPLICATE，违反第 410 行 | Snapshot::control_transport 检查必需 WRITE/WAIT/TRANSIT，并仅取上限允许的 DUPLICATE；不附 GRANT。DelegateName/Register 都使用它。10 项 host 测试包含缺必需位、可选复制位、GRANT 不自动附加 |
| 无 GRANT 的 scoped authority 不能用 process::grant 出生安装 | 不改协议运输字段。secondary 出生取得独立 publication inbox receiver，payload 明确 Secondary；init 出生后用 AUTHORITY_GRANT_KIND/空正文/唯一 scoped sender 投递。child 以 10s 截止接收，核对真实 parent_pid、kind/槽/角色及无 GRANT，采用后关闭 inbox。init sender 留在 supervisor 至投递完成，失败由已有子进程 control 收束。仅静态证据 |
| RoleTable.remove 与 Root Lifetime CLOSED 被当作 revoke，违反第 437 行 | remove 只去索引；service Root 另存显式停止/revoking 标志，普通 CLOSED 不更改已准入使用引用。Authority 控制 owner 只有显式停止才 revoke。同类文件 Root 修订为 CLOSED 不 revoke、stop 才 revoke。纯表测试核对新请求拒绝、使用引用保费/保权、显式 revoke 及子授权独立；Kernel 交错仍未运行 |
| bootstrap MailboxFull 未说明责任 | StartupBlock 独占交付 bootstrap peer，初始空队列；primary 仅三个根+Ready，最多 4 条未消费，静态断言 4≤共享 MAILBOX_CAPACITY=16。Stopped 在 init 接受 Ready 后；secondary Ready 后 Stopped。无其他 producer，不以重试形成无界队列。唯一 publication inbox 也只投递一次；其他发送错误沿既有原 owner/进程失败收束。尚无完整运行证据 |
| secondary 停止原来只弃注册控制，缺显式撤出 | 封口后在同一个 Dispatcher 发 Withdraw；收到 Withdrawn 或有限期限/失联收束后封 Runtime。失联仍关闭原控制以让目录 owner 自动撤出，绝不按名字删替代者或恢复旧注册。实际退出验证仍待通知前置 |

根 GRANT 的原取证仍有效：process::grant 跨表出生安装要源 GRANT。primary 三个根的 GRANT 母本只在 Root 准备/启动 Packet/receiver 交付链中存在；World 不保留母本，普通消费者副本去 GRANT。secondary 的 delegated authority 现在没有 GRANT，改走已有 IPC，不把根装配理由推广成 DelegateName 默认附加理由。

首轮子 reviewer 因 upstream HTTP/2 stream failed 中止，原记录为结构审查未完成。2026-10-10 已按协调要求补齐非 Watch 审查，得到 2P1+4P2，结论未通过，触发/责任见本节末续记；原部分输出保留，非 Watch 修复如上。Watch 控制 mask 与最后 owner 移除后的 Maintenance 唤醒仍待前置后成组复核，不据子审查完成宣告整体通过。

**通知前置反例与待裁决候选。** os/kernel/src/task/notification.rs 的 NotificationOwner::allowed_rights 只含 READ|WAIT|MANAGE|GRANT（第 207 行），不可 DUPLICATE/TRANSIT；NotificationSignaler 只含 SIGNAL|TRANSIT|GRANT|DUPLICATE（第 226 行），没有 WAIT；NotificationObservation 允许 WAIT/DUPLICATE/TRANSIT，但 allowed_signals 只有 CLOSED（第 238/241 行）。所以不能从现有角色构造计划 Watch 回复槽 1 的“可 IPC 交付、可观察 READABLE 的 waiter”。provider 当前 create 不支持的 Owner rights，真实返回 Permission。不能用观察 CLOSED、客户端轮询、假能力或放宽验收替代。

用户已裁决选择 A，并授权内核改动：provider 持唯一消费 Owner，交付只读 READABLE/CLOSED 观察角色；要求与现有等待机制一致、整体干净。GrandOwl 起草独立内核前置计划，MellowPond 审定后交本案实施者；审定前不改内核或 Watch 接线。下表保留候选的反例对照，其中 B 未选：

| 候选 | 完整责任链与取舍 | 受影响范围 / 验证门 |
|---|---|---|
| A：内核新增 NotificationWaiter 只读事件角色 | Owner 保持唯一消费/业务寿命责任；从 Owner 派生 WAIT+TRANSIT、按明确规则可 DUP/GRANT 的 waiter，观察 READABLE+CLOSED，派生/关闭 waiter 不延长或结束通知业务。既有 NotificationObservation 仍只观察关闭。内核新对象/能力/观察存储由真实 funding pool 付款 | 新 shared ObjectKind/syscall、Kernel 派生和事件允许集合、libkernel 封装；fs/service Watch 维持两输出槽。验证 source 注册、跨进程传输、只读、消费与新事件、Owner 关闭、waiter 最后副本、失败退款；复用正式进程/IPC 验收 |
| B：客户端提供 Notification Signaler | client 自建 Notification，保留 Owner(READ+WAIT)，Watch 请求槽 1 给 provider Signaler(SIGNAL+TRANSIT)。回复仅控制 sender；服务状态 Query/Ack/Cancel 仍由控制 owner 管理。provider 不观察通知对端，订阅取消/寿命由控制 Lifetime 兑现；客户端消费通知，通知对象由客户端进程付款 | fs/service Watch 请求与回复槽/线形、客户端 Notification owner、provider 保存 signaler 和 Ack/重置的协议流程需成组重设计；不能只把 Owner 换 Signaler 而保留 server NotificationTake。验证 Query→重查→Ack 的竞争与丢唤醒、终态通知、调用者退出/失败与退款 |

A 的具体观察角色、既有 NotificationObservation 的整合方式、权利与 syscall 由独立内核前置计划统一闭合，不把候选名字或新增 syscall 当已审定 ABI。B 的客户端付款/消费方案未选，不再作为下一步。Watch 控制 mask、明确 revoke/独立控制寿命和最终维护唤醒在前置交付后成组接通。

**第五批验证缺口与接通条件（共同 owner：RubyRaven，协调 MellowPond）。** 每项最迟在 P4 组合收口前关闭；“接通”指当前协议的真实 backing/来源/RPC/退休消费者齐全，不恢复旧 FAL API 或能力属性。现有脚本保持严格，第四批没有对锚点做可选化。

| ID | 删除的旧函数/场景 | 对应矩阵行 | 第五批接通及补证标准 |
|---|---|---|---|
| G05-01 | exercise_affine_take、inline repeatable handle read/Take | 编码、真实资源与退出 | 不恢复属性能力/affine 语义；改验纯值拒绝能力槽、GetEntry/新控制 typed 能力配对及失败 owner 收束。废弃锚点仅在 P4 按最终消费者统一替换 |
| G05-02 | exercise_property_watch、inline Watch authorization | 名字订阅/资源退出、文件 Watch 合同 | fs 独立 Watch 控制与已裁决通知接通；真实 mask/权限、Create/Modify/Delete/Unbound/Terminal、Query/Ack/Cancel、最后控制退款 |
| G05-03 | exercise_resource_quota_recovery | 资源与退出 | 正式文件/控制来源接通后饱和 Node/Bytes/Grant/Watch/会话等实际额度，释放真实 owner 后退款且再次准入；不以容量常数替代观察 |
| G05-04 | exercise_directory_move_and_enumerate 中旧 Move 部分，inline cross-provider Move rejection | 领域路径与能力请求、枚举摘要 | 新 Rename 双目录真实授权及派生接通；同域改名/替换/目录防环、跨 backing 拒绝、分页与引用保活。当前已保留简单 OAL 分页消费者，未运行 |
| G05-05 | exercise_existing_values | Property、小 I/O | 按纯值新模型补 bool/i64/u64/f64/String/Blob/数组的实际类型/范围/短结果及变形摘要；新消费者已有 u64/数组/链接部分，仅编译 |
| G05-06 | exercise_unattached_offer | Property、小 I/O 与文件会话；资源退出 | 新 Open 会话接通；未 Start 期限、来源与 invitation/控制退休退款 |
| G05-07 | exercise_discarded_offer | 同上 | 丢弃 Offer 双 owner 后会话取消/映射退休，不保留旧 DeferredStub |
| G05-08 | exercise_prestart_eof | 同上 | Offer→Start gate 和 EOF 接通；建立前不产生业务，空读端正常 EOF 与关闭责任 |
| G05-09 | exercise_bounded_stream_read | Property、小 I/O 与会话 | primary/secondary 新 Open 范围与分批读取、实际数量/EOF/错误、完整收束 |
| G05-10 | exercise_create_uncertainty | Copy 与结果未知；资源退出 | 新 CreateOpen/建立事务接通；成功提交后回复丢失不重放，控制/pin 关闭和范围内删除责任正确 |
| G05-11 | exercise_stream_contents | Property、小 I/O 与会话 | 实际共享流、offset/read_end、冻结读范围、SetLength 交错和稀疏扩长零填充 |
| G05-12 | exercise_conditional_open | 同上 | 若独立条件建立合同继续保留，用新控制/实际 identity 验证，不恢复公开 FAL NodeInfo/版本探针；未承诺条件则由协调者核销而不造 API |
| G05-13 | exercise_abandoned_open_reply | 资源退出、编码回复配对 | 控制/invitation 到未领取回复邮箱后调用者消散，真实来源与映射最终退休 |
| G05-14 | exercise_cross_provider_copy、inline Property Copy/stream Copy | Copy 与结果未知 | 新 Property 原子创建及跨 provider Stream Copy 接通；完整属性、实际前缀、失败和只留同域 Rename |
| G05-15 | exercise_created_target | Copy 与结果未知 | CreateOpen 创建新目标及拥有具体结果/pin，取消只清自己创建实例，不按路径删替代者 |
| G05-16 | exercise_copy_variants | 同上 | Conflict、空文件/空范围和取消等实际行为；终态数量/结果与完整退款 |
| G05-17 | exercise_copy_after_progress_cancel | 同上 | 真实非零 accepted/transported 前缀后取消，未知结果不冒充最终值，Runtime 继续接管清理 |
| G05-18 | exercise_copy_during_provider_shutdown、inline in-flight shutdown | 同上；资源退出 | provider 停止、Copy 任务消散、原 owner/Runtime 继续关闭会话/退款；旧 midflight flake 由原计划在新消费者触发时承接，不宣告已修 |
| G05-19 | inline Delegate/restricted Open、fs Watch authorization | 普通路径/入口授权、Boundary 受限授权 | DeriveDirectory 与会话/Watch 接通；目录/FsRights/完整运输上限不放大，原母别名关闭不误 revoke |
| G05-20 | init exercise_open_streams | 文件会话；资源退出 | 对控制 Start/Query/Finish/Cancel、满载/EOF/未知回复完成正式 runtime 验证，不恢复旧 Open/Lookup 前置 |
| G05-21 | init bind_fs_route、stage/finish_stalled_delegate、cancel_stalled_delegate_while_provider_runs、reply_to_cancelled_delegate | 领域路径/能力请求；Boundary、退出 | Bind/Unbind 管理接收、真实受限出口、双来源、替换/撤出/父撤销接通；取消/迟到回复、未消费 suffix、循环预算与完整上限均验证 |
| G05-22 | init stage/finish_abandoned_reply | 编码/能力配对；资源退出 | 复用正式入口和既有 public_ipc 设施构造回复未领取/迟到隔离，核对临时能力和 Delivery 退出责任；不另建测试服务 |

**第四批仍需接通/复核的责任。** Notification 通用三角色/typed迁移已实施并取得 N1—N7 分层证据，N6 专属故障时点仍仅源码审计；service Watch/registry_checks 及普通 fs/退休已经实际运行至 reset，严格路线因 G05-20 旧流锚点缺失仍未通过。非Watch六项保留原始反例与修复证据；跨独立账户等未单独验证的组合不由普通成功路径推定。原 init probe_registration_reply_boundaries/probe_registration_endpoint_retirement 的未知Replace回复、B→C后B迟到、不恢复A、控制队列引用/endpoint来源及退款仍未迁完，属原P3，不挪G05；registry_checks已跑过但仅覆盖其部分。当前接力为前置报告收口并交协调正式排Watch剩余/W04-01及第五批流/Copy/Bind；目标消费者补齐后再满足严格锚点和组合验收，不归档、不报整体完成、不做阶段提交。

### 第四批通知错误修复与非 Watch 结构审查（2026-10-10）

**授权与当前方向。** 用户已选 A，provider 持 Owner 清除、向外导出只读 READABLE/CLOSED 观察角色；内核修改已授权，但由 GrandOwl 起草独立前置计划，MellowPond 审定后才交本案实施者。当前没有 Kernel/shared ABI 或 Watch 接线/脚本改动；两个与方案无关的错误收束先实施。以下是开发检查与当前 dirty 工作树的只读结构审查，不是固定提交的独立 Review，也不宣告第四批通过。

**通知错误修复。**

| 项 | 实现与责任 | 验证及边界 |
|---|---|---|
| Ack 通知已空时误报 Resource | watch.rs 的清除操作只将 ObjectNotAvailable 当作已空成功；其他错误仍经原 Status 映射返回。未来序号仍 Invalid，旧序号不清新变化 | 原序号测试增加相同序号及更旧序号重复 Ack 均保持已确认快照。Kernel NotificationCore::take 的空状态返回 ObjectNotAvailable 已读源码核实；未运行真实 Ack syscall |
| 永久 ObjectClosed 无界重试 | Maintenance 分开处理 ObjectClosed：Watch 记录标记 notification_closed，转 Cancelled 终态并释放 DirectoryAccess 观察目标引用；后续 signal 不再调用已关闭对象，也不设置 retry。原终态原因不被重复覆盖。其他错误仍保留重试责任 | 控制 Lifetime 仍归原 Request；终态控制、通知 handles 和真实保留存储继续计费，最后控制关闭/来源注销后移除并退款。没有删除仍有人持有的终态记录，没有把未发出的信号记成已发送。目标/lib 检查覆盖这些状态分支，实际 Kernel 关闭交错待前置 |

开发日志均在 artifacts/service-p3-batch4/：service-host-tests-notification-errors.log（10 项通过，退出 0）、service-host-lint-notification-errors.log、libraries-target-lint-notification-errors.log、user-workspace-lint-notification-errors.log（各退出 0）。没有合成 Kernel capability、sender、IPC 或新的测试运行体；未重复 QEMU，通知前置仍未交付。

**补齐的非 Watch 结构审查。** 上轮 HTTP/2 失败审查没有完整结论；本轮按协调要求完成非 Watch 源码审查。范围为 service codec/authority/Registry/Request/root、普通 fs Client、srv_fs 双 mailbox 同 Runtime/出生后 authority 交付/Register/Withdraw/退休和 init/test_fal 消费者。排除 Watch 角色/接线及这次两个已处理错误，也不把 G05 未接通本身当 finding。主实施者亲自核对触发代码与合同。原结论为 **2P1+4P2，审查未通过**；保留以下原始反例，六项修复与复核见后续处置表，整体运行未验收。

| ID / 级别 | 位置与可达反例 | 违反的合同、修复目标与复核标准 |
|---|---|---|
| R04-01 / P1 | srv_fs/server/lifecycle.rs:285 用 !progress.done 作 ready；NodeStore::retire_step 在队列为空但根仍活时返回 work_done=0/done=false，所以 Retire 常态一直 Runnable。停止时删最后节点得 done=true 后立即 Parked，下一次 advance 才撤来源，缺必然唤醒 | ready 应表示立即退休工作，以 has_retire_work 判断；每次推进后重新检查 stopping/is_empty，清空后继续撤来源并等实际注销回执再 Complete。仍有使用引用但无队列时等引用释放通知。复核正常空队列停驻、不自旋，以及最后一节点释放后来源撤销、Runtime Drained/Stopped |
| R04-02 / P1 | test_fal/main.rs:34 的 wait_command 直接 expect(notification::take)。SECONDARY_DISCOVERED 后消费者先等 CONTINUE，而 init 要先做两 provider/registry 检查；没有 pending bit 时内核合法返回 ObjectNotAvailable，导致正常交错 panic。COMPLETE→OBSERVE_SHUTDOWN 同样可达 | 仅 ObjectNotAvailable 表示尚无命令并进入等待；其他错误/CLOSED 明确失败收束。复核命令晚到、先到与停止三条正常控制路径，不改 Watch 协议 |
| R04-03 / P2 | service/provider.rs:150 付款只从 Access/Authority 取得。RegistrationControl context 不在两表中，QueryRegistration/Withdraw 落宿主 state.account；registry.rs:248 的注册费用却由原发布 authority 账户支付，Registration/Withdrawn 壳没有可供请求取用的 AccountView | 注册及终态控制壳保留可信付款绑定；接收者按真实 context 找该账户，不依赖父 authority 活着，也不接受正文账户数字。复核不同付款入口注册、查询、撤出及终态请求的 Request/Outbox/Bytes/WaitSource 费用一致 |
| R04-04 / P2 | service/provider.rs:148 的 accesses.get(id).ok()/authorities.get(id).ok() 丢掉 Revoked；显式撤销但未关闭的合法目录/authority 随后被当作不存在，返回 Permission 并失去原付款绑定 | 保留查表拒绝原因及可信付款，区分错型/越权与明确撤销；普通关闭不 revoke 的修复保持。复核旧入口显式撤销后的 Describe/Enumerate/GetEntry/Register/DelegateName 返回 Revoked，陌生 context 仍 Permission |
| R04-05 / P2 | libfs/client.rs:305 在完整目标规范化之前 rsplit_once，再 validate_name。/new/、/new/. 按词法应为 /new，却提前 InvalidPath；basename 未进入完整展开额度，仅 parent 走 Walk，可绕预算 | 在同一 FollowBudget 中规范化完整 destination，再拆父路径和名字；目标派生与源 Rename 继续同一剩余预算。复核尾斜线、点项、规范化根的非法名字，以及长 basename 不能绕过展开限额；不以尚未接通 DeriveDirectory 作为否定此已实现算法的理由 |
| R04-06 / P2 | libfs/client.rs:482 对每个合法文件请求用 vec![0;len]，allocator 耗尽会进入进程 allocation error handler，无法返回已有 ClientError::System(OutOfMemory) | 先 fallible try_reserve_exact，再 resize/编码；失败发生在 RPC 投递前，不发送或重放变更。复核所有直接文件请求使用同一可恢复分配路径，必要证据不另造故障运行体 |

六项由第四批/P3 与普通 fs Client 现有责任链承接，owner RubyRaven、协调 MellowPond；不延期为 G05，不另立重复记录。协调确认六项均属已有合同偏离，授权立即修；以下处置已落实源码及最小边界回归。真实启动/停止与付款交错仍须在前置和 Watch 接通后由正式 QEMU 消费者补证，不能以静态修复宣告第四批通过。

审查支持的当前结构事实：新控制 context 来自实际 MintedSender object_id，内核 envelope 填 sender_context_id，Rename 通过实际 kind/related mailbox/context 查本地授权；Register 三个相关来源先准入、最终复核、预建索引提交；Replace 后旧控制只撤自己的 ID，成功回复保留原 Active 快照，队列中控制由真实 message capability 引用保活；普通关闭与显式 revoke 分开，secondary 通过专用 inbox 和 Dispatcher 发布/撤出。上述为源码结论，host/target 静态检查不证明延迟回复、控制队列、来源撤销、退款和退出的实际组合行为。整体交付前检查与正式运行仍未完成。

**R04 修复→复核（同轮，协调者授权）。**

| finding | 实际修复 | 复核与未覆盖 |
|---|---|---|
| R04-01 | MemoryBackend 的 RetirementState 直接从 has_retire_work/is_empty 导出 Ready/Waiting/Empty；Retire 只在 Ready runnable，停止后 Empty 保持 runnable 至下一轮 source.remove，实际注销回执后 Complete，不再用 progress.done 判 ready | 独立实际 backing 回归保持存活根且 work_done=0/done=false，调度活动必须 Waiting；封口仍持引用 Waiting，释放后 Ready，分批清空后 Empty。无 Kernel/任务模型。作者复核 Empty→撤来源→注销回执分支；实际 Runtime 自旋/Stopped 组合仍待 QEMU |
| R04-02 | wait_command 仅将 ObjectNotAvailable 当尚无命令，随后等 READABLE/CLOSED；整轮用同一 deadline，CLOSED/到期明确失败，不重置时间或无限空转 | 作者逐分支核对命令先到/后到/关闭/超时，target check/lint 覆盖；真实调度交错待 QEMU |
| R04-03 | Registration 与 Withdrawn 壳持独立 AccountView；provider 按真实 control context 从 Registry 取原账户支付请求/Outbox/Bytes/WaitSource，不依赖已退休 authority | 作者核对 prepare_register 的 authority.account 冻结、ActiveBody 退休后账户仍在 record、accept 的优先解析与最终记录退款。未造 Kernel sender 验证，跨账户 RPC 运行证据待 N8/P3 |
| R04-04 | RoleTable::binding 只解析索引里的可信使用/付款，不丢掉 inactive 记录；Request 操作沿原 Snapshot::check 报 Revoked。get 先识别 context 再判 sealed/active，陌生 context Permission，业务路径 NotFound 保持 | 新纯授权回归验证明确 revoke 后绑定账户不变且 check=Revoked，陌生 context（含 seal 后）仍 Permission，旧普通关闭回归继续通过。所有 service 操作实际响应待运行 |
| R04-05 | Rename 先以原 FollowBudget 调 view::normalize 完整 destination，再拆规范父目录/名字，原剩余预算移交目标派生与源请求；member 名字准入仍由 backing 的公共 admission 检查 | 既有词法回归增加 /new/ 和 /new/.→/new，短展开预算拒绝长 basename；作者复核完整输入计费、规范根空 basename 拒绝和预算无 reset，实际 Derive/Rename 待接通 |
| R04-06 | 共用 fs 请求缓冲以 Vec::new→try_reserve_exact→resize，失败在创建 RpcRequest/投递前返回 ClientError::System(OutOfMemory) | 客户端此时没有已收到的回复权/输入能力，Caller::call 未执行，caller 直接取得明确未投递 OutOfMemory，不等待 Closed/Timeout，不 abort/重放。另行核对服务端建立前失败：libfs Request::accept 的 Resource(ResponseFailure) 完整持有 RequestContext 的 SendOnce、HandleSet 输入槽与 Delivery，由 srv_fs Ingress::advance 的 Err(rejected)→drop(rejected) 沿既有 RPC typed owner/Capability Drop 关闭；reply Pending→Abandoned→ReplyObserver PEER_CLOSED，Delivery 释放 LifetimeUse，输入槽逐项关闭。已取出拒收的 caller 观察 ServiceClosed，期限先到而未处理则 Timeout，不生成业务 Resource 回复。target lint 与作者复核清理链；未新建 OOM 注入环境 |

受影响 host 测试日志 artifacts/service-p3-batch4/nonwatch-findings-host-tests2.log：fs23、OAL27、service11，退出0。新增退休/撤销付款回归复用实际 backing/授权容器，不创建 Kernel fake 或新运行体。最终 nonwatch-findings-user-check3.log、nonwatch-findings-user-clippy.log、nonwatch-findings-host-clippy.log、nonwatch-findings-library-clippy.log 均退出0；user bins 按既有 imac 路线除独立gc test_fp，其此前gc lint证据仍有效，依赖未受本轮影响。初次 missing AccountView import 与 WaitResult 字段误用的失败日志保留。未重跑QEMU、未改Kernel/shared或验收脚本、未commit/push。六项源码反例已修并完成针对性复核；第四批整体结构/运行门仍不宣告通过。下一步按已审定 Notification 独立前置计划进行完整角色迁移，再接主计划 Watch；原未交付能力及 N8 验证不提前登记完成。

### Notification 前置接入后的 N8 开发证据（2026-10-10）

通用角色/typed/共享 ABI 的实现、N1—N7（含 N6 未注入窗口）、工具链与删除清单只由 [前置工作记录](../todo-2026-10-10-notification-observer-roles.md#实施工作记录角色迁移与开发证据2026-10-10rubyraven) 拥有。本篇记录业务接缝；以下不是第四批整体验收通过。

**Watch 真实接入。** libservice provider 不再为 NotificationOwner 请求 DUPLICATE/TRANSIT；只持 READ|WAIT|MANAGE Owner 和 SIGNAL Signaler，从 Owner[M] 铸造 WAIT|TRANSIT 的 NotificationObserver，输出受目录出口上限约束。Watch 控制 sender 使用母出口允许的 W|WAIT|TRANSIT 与可选 DUP，不自动 GRANT。provider WatchTable 继续独占 Owner 清位，客户端使用 typed Observer 的只读 READABLE/CLOSED 等待。libfs 客户端已切 typed Observer 唯一转换边界，但文件 provider Watch 仍 Unsupported，留 G05-02，不报文件 Watch 交付。

| 三类覆盖 | 事实与锚点 |
|---|---|
| 已切新协议并实际执行 | core/stress guest：SRV2 init WatchName Query GetEntry passed、FS1 init direct file paths passed（两个provider）、SRV2 init replacement control and stale Watch Ack passed、SRV2 independent WatchName Query GetEntry binding passed、SRV2 independent endpoint withdrawal passed、SRV2 init endpoint withdrawal passed、FAL2 cancellable RPC phases passed、FS1 provider supervision reclaimed both provider processes；两路线之后完成其余正式负载和reset |
| 仅编译/源码，未相应运行 | fs Watch Observer输出采用、DeriveDirectory/流/Copy/Bind等未闭合线上任务；跨实际独立付款账户的控制请求费用与每个退休时点不由普通成功路径推定已验。既有Watch正式消费者当前同步调用，WaitOne返回后才发RPC，没有持续Observer登记跨异步在途请求；第五批新增真正异步消费者时必须暂移来源并保留处理责任 |
| 第五批及原第四批缺口 | G05-01—22仍由原表唯一承载，流/Copy/Bind未核销；注册Unknown Replace回复、B→C后B迟到、不恢复A及endpoint/控制退休反例仍为原P3责任，不能挪到G05冒称完成 |

**正式路线命令与失败归属。** just virt 的 artifacts/notification-observer/virt1.log 和 virt2.log 均退出1，完整guest分别为 artifacts/failed-acceptance-20261010-092956-28815.log 与 failed-acceptance-20261010-093610-33881.log。just virt-stress 的 artifacts/notification-observer/virt-stress1.log 退出1，完整guest为 artifacts/failed-acceptance-20261010-093106-30013.log；真实自动退休、Startup Observer、16/16矩阵和reset都在guest日志中。共同阻断锚点 FAL2 Open control and terminal cases passed after 16384 bytes 属 **G05-20：被删除的 init exercise_open_streams，Open/StreamControl Start/Query/Finish/Cancel**；已有唯一登记，无须新增平行缺口。脚本没有可选化、放宽或删锚点，等目标流状态机/正式消费者接通后由真实输出满足。当前不执行聚合acceptance去重复同一已知缺口，不把guest完成写成路线通过。

容器预留与原第四批permit记录不变；Notification新typed壳没有额外容器或提交分配，观察壳/来源实际Kernel计费由前置拥有。新增 raw仅正式ABI夹具，生产src无raw通知调用；不保留Owner可运输伪路径。当前没有阶段提交、stash/reset/push。

**Watch 源码 finding W04-01（本篇唯一记录，源码已修，独立账户运行待 W04-02）。** 原 Request::accept 未从 WatchTable 解析 WatchControl 的原账户，QueryWatch/AckWatch/CancelWatch 落宿主 state.account；通知关闭释放 access 后也不能从目标恢复 payer。第五批已在 Watch 记录冻结独立 AccountView，并以真实控制 context 的 control_account 查询接入 Request/Outbox/Bytes/WaitSource 付款；目标引用释放与终态壳付款分开。AccountView 与原 Charge 随记录实际退休释放，没有新增提交分配。源码复核 prepare 使用 access.account、通知关闭只释放 access、control_account 在终态仍返回原账户；普通同账户运行不证明独立账户付款。owner RubyRaven、协调 MellowPond；真实双账户门由 W04-02 承接，不把 Notification 通用前置或普通 Watch 锚点写成全合同完成。

### 第五批 Watch 付款与原 P3 探针（2026-10-10）

**授权与顺序。** MellowPond 已排第五批：W04-01→原 P3 回复边界探针→文件 Watch 与异步消费者→DeriveDirectory→Bind/Unbind→StreamControl→Copy。每接通目标能力恢复相应 G05 正式消费者，严格脚本不放宽；不 commit、stash、reset、push。

**W04-02：独立账户真实运行前置缺口。** 当前 libservice/root.rs::Root::prepare 固定使用 state.account 签发 DirectoryAccess/NameAuthority；srv_fs/server/boot.rs 的 Budget::new(&limits, 1) 只配置一个账户；libexecution/runtime.rs 的 queue.insert 与来源 input 费用固定使用 self.account。只将根绑定账户 B 而 Runtime 留 A，会违反本篇 429/447 的领域费用与执行 Task/InputBytes/来源由同一可信付款身份承担的合同；第二个 Runtime 或测试服务也不在授权范围。MellowPond 已确认缺口并与 GrandOwl 对齐执行付款身份的结构归属，在正式排批前不修改 libexecution。目标是同组闭合 Root 独立账户参数、boot 多账户装配及执行任务可信付款；owner 为 MellowPond/GrandOwl 前置设计、RubyRaven 实施。触发条件为前置合同审定并正式排批；补证须复用既有 init/provider 装配，建立不同于宿主的真实账户入口，核对 Watch/注册控制 Query/Ack/Cancel、终态释放目标后的请求及执行费用仍在原账户，实际 owner 全部退休后退款。该缺口不阻塞已授权的同账户 P3 探针和文件 Watch 施工，不以源码检查替代双账户运行。

**W04-02 承接（MellowPond，2026-10-10）。** 执行付款的通用合同由[执行任务与来源的付款归属](todo-2026-10-10-execution-funding.md)拥有（TaskSubmission 显式付款、SourceFunding Task/Bound、Runtime 构造参数仅为基础设施 sponsor H）。本篇拥有：Root::prepare 接收可信入口付款上下文（同一 Account 的 domain_view 与 execution_view，在发行边界核对 account_id 相同）；srv_fs boot 在同一 Budget 下建 H 与独立入口账户；角色、派生 grant、registration/watch 控制、请求与下游调用沿原可信绑定继承；有界拒绝路径按该前置写死，不由 H 执行业务。真实验证：同一 Budget 下 A 局部耗尽时 B 在全局有余量下可用，全局满时 B 拒绝，释放 A 的真实 owner 后恢复；Watch Query/Ack/Cancel、终态壳、同一 Dispatcher 的 A/B 下游调用均回到 baseline。前置在 host 通过不算 W04-02 完成。

**core 路线时限（用户裁决，2026-10-10）。** 路线超时上限不是硬上限，撞限即翻倍；core 由 90s 调为 180s，Derive 场景全量保留，RPC 合并清单（工作记录 1119）仅作记录不实施。W05-06 按此关闭条件为一次 core 正常 reset、只剩 G05-20。排批顺序：执行付款前置 → Bind/Unbind → 流状态机 → Copy。

**P3 开发断点与误判纠正。** registry_checks 已迁入真实 B→C 后迟到 B 回复及旧控制 Query 验证，使用现有 CallOperation/Caller 和正式服务入口，不建运行体。未知 Replace/队列持有/退款探针的首次接线错误地放在独立消费者出生且 SECONDARY_DISCOVERED 后；主动等待原 5 秒注册期限，与消费者已经启动的 5 秒命令等待窗口竞争。artifacts/service-p5/p3-probes-target-lint1.log 的 RV64 workspace stress clippy 退出0；just virt 的 p3-probes-virt1.log 与 p3-probes-virt2.log 均退出124，完整 guest 分别 artifacts/failed-acceptance-20261010-103022-41721.log、failed-acceptance-20261010-103613-43206.log。两轮实际出现 SRV2 delayed replacement reply passed: B snapshot and withdrawn control survive C，之后消费者 panic。早期曾把 124 误判为整条路线超时及 Sleep 有限等待丢唤醒；本轮调查撤销这一判断。

**纠正证据。** tools/qemu-acceptance.sh:158—164 将任意用户 panic 列为 terminal，192—195 立即 kill runner 并人为置 status=124；故 124 本身不代表 90 秒期限。timer-mi17-host-times.log 记录 continue 后18.26秒 runner 已退出124；timer-hardware14-gdb.log 的消费者期限 hint 现场 time=105930740，而 init epoch183 的期限=117740550，尚差约1.18秒。init 节点当时实际挂在 owner2 队列，timer_prepared=true，sie=0x22，stimecmp=117740550；另一轮 timer-queues7-gdb.log 同样记录 source=None、owner3、期限119797160。timer-cancel16-gdb.log 显示前182轮来源 hint/resume/detach 正常，183登记后没有 hint或detach；这些观察都被主动收割截断，不能据此判内核丢唤醒。失败/超时 GDB 尝试1—17全部保留，只有上述具体节点和时钟记录用作证据，不把失败打印当有效现场。wait.rs/sched.rs 与4523bbf的 SHA256 逐字一致，纯 Sleep source=None 不经过 Notification/Wake/WaitSet；隔离基线 worktree 的 just virt（baseline-virt2.log）退出0，正式脚本必需 public time deadline checks passed 锚点。第一次基线仅缺输出目录失败，baseline-virt.log保留。没有修改内核、脚本或增加轮询。

**已确认的接线修复。** MellowPond 接受上述更正，内核疑点撤销。未知/队列/退款探针移到 confirm_primary_fs_ready 后、launch_fal_consumer 前；只使用已就绪 primary 的实际发布根、目录根与可复制业务入口，A/B RegistrationId 不同，共用业务入口不冒充 endpoint CLOSED 证据。既有 consumer先Watch→secondary出生的 DISCOVERY_ARMED 顺序保持；迟到B/C探针留在原处，没有主动sleep。启动前探针使用500ms有限注册deadline，真实sleep_until并复查expired；到期后仍以GetEntry确认B保持当前注册，填满实际注册准入且持有全部控制，再只关闭B私有回复队列，验证空名不恢复A及准入退款。500ms是降低探针启动时长的夹具参数，消除计时耦合的原因是探针位于消费者出生前。后续运行结果与探针自有注册/控制收束证据继续写本节，不提前报未知/退款门通过。

**第五批前两项验证与覆盖。** W04-01 源码已修并回填 service 实现篇，真实独立付款门只登记 W04-02。启动前探针的新增控制/名字全部按各自寿命收束：恢复准入的控制 Withdraw 返回 Withdrawn(Explicit)，14个仍持有控制逐项 Query 返回 Withdrawn(Replaced)，原A仍 Withdrawn(Replaced)，Watch Cancel 返回 Terminal(Cancelled)，两个自有名字均 GetEntry=NotFound，之后释放全部本地控制；没有残留名字影响消费者。B 的控制只存在于真实回复队列，期限到期后仍保持当前注册；持有A/B和其它控制时16项真实准入用尽，单独关闭B回复队列后一个注册可恢复，证明B的注册准入在控制消散后退款。注册 AccountView 的各项内部字节/来源费用不能从init直接读取；这类逐项金额及独立账户门不由准入恢复替代，仍由W04-02/既有责任门补证。Boot最终registry.is_empty的实际检查及两个provider正常退出确认没有遗留注册控制记录。

| 三类覆盖 | 本轮事实 |
|---|---|
| 已切新协议并实际跑到 | 新队列deadline到期、未知Replace仍Active、不恢复A、实际准入Quota→只清B回复队列→恢复准入；B→C后迟到B取得原Active快照且独立Query=Withdrawn(Replaced)，旧B不能撤C；两个正式provider及独立消费者继续运行直到reset |
| 仅源码/编译或尚无该项运行 | W04-01可信payer独立冻结与终态目标释放后account查表；真实不同于宿主的账户金额/执行收费链待W04-02。队列探针A/B共用primary业务入口，未关闭该正式入口，不报它覆盖endpoint CLOSED |
| 原路线与剩余缺口 | endpoint CLOSED实际退出由同一guest的SRV2 independent endpoint withdrawal passed、SRV2 init endpoint withdrawal passed及FS1 provider supervision reclaimed both provider processes覆盖；G05-01—22本轮均未核销，P3不冒充G05。严格路线仍缺G05-20旧流锚点 |

最终命令及日志（host 与 RV64 分开）：

```sh
just --command bash -c 'cd user && cargo test -p libservice --target aarch64-apple-darwin'
just --command bash -c 'cd user && cargo clippy --workspace --bins --exclude test_fp --features srv_init/acceptance-stress --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings'
just virt
```

artifacts/service-p5/watch-payer-host-tests.log 退出0，11项既有host用例；没有为AccountView getter复制实现或制造Kernel fake。p3-probes-target-lint2.log 退出0，RV64 workspace stress完整编译/lint。p3-probes-virt3.log与virt4.log均退出1，完整guest分别artifacts/failed-acceptance-20261010-110307-64674.log、failed-acceptance-20261010-110507-66177.log；后一轮含上述完整终态/无名字断言，guest跑到system reset accepted，仅严格G05-20锚点缺失。脚本未改，内核未改，基线隔离worktree已清理（baseline-cleanup.log退出0，baseline-kernel.sha256保存其ELF身份），所有本次QEMU/GDB进程已退出。git diff --check通过。

**删除清单与容器。** 本轮没有新增旧模块保留、adapter、重导出或新测试服务，也没有删除G05场景/验收锚点；删除旧Watch“目标引用恢复付款”的事实叙述，原第五批前探针调用点移到出生前，原唯一AccountView仍由各记录拥有。Watch增加字段不增加容器，实际PreparedEntry分配字节跟随真实类型计算、准备阶段收费，提交无分配；探针Vec对每个控制先try_reserve，失败明确返回并由真实owner清理。第一至四批的文件删除统计保持原表，不重复计为本轮删除量。下一步继续文件Watch provider、异步在途消费者及其G05接通；未归档、未commit/push。

### 第五批文件 Watch 施工接缝（进行中）

**弱身份与责任。** Watch 选择由已鉴权路径 resolve/check 完成，提交后的记录仅保存单调不复用的 NodeId，不保存 NodeRef 或 Rust Weak<Rc<Lease>>；维护按 backing 的实际记录/links/queued 查询状态。NodeId 仅是已选目标的弱身份，不作为新请求授权。这样不增加业务 pin，也不因 Rust Weak 保留已退休 Lease 的分配而提前退款。保存原 GrantLease 的活动 token 及独立 AccountView，不克隆带 root pin 的完整 AccessSnapshot；控制请求以实际 context 查此记录，终态释放目标/token仍保留原payer。Unbound在解除名字提交时写位，queued开始退休/记录消失给TargetRetired终态；Create/Delete/Rename只改直接父目录，Modify只改实际文件目标，不向祖先传播子文件内容。

**预留与顺序。** 实际 MemoryBackend 拥有 WatchTable、事件序号与目标事实；宿主以现有根部署容量配置 Watch 准入，不复制报文/旧魔数。每个准备请求持 Counter Permit 及预建 OrderedTable entry、Watch/Bytes/一个控制 Lifetime 来源 Charge；Owner[R|WAIT|MANAGE]、Signaler、Observer[WAIT|TRANSIT及出口允许D]、真实控制 sender/Lifetime 和回复容量均在提交前准备、来源实际准入。提交只插预建 entry 和写序号，未引入提交分配；当前每个 Watch 一个自身控制 Lifetime 来源。控制/通知与记录保留到实际 unregistered、Lifetime 关闭及最后控制退款，回包失败不丢原 owner。Counter 和共享 wake 在装配时建立；每记录动态分配只有收费的预建表项，角色费用按真实内核创建/输出/来源计费。

**已确认来源合同。** MellowPond 已审定 A 方案只登记控制 Lifetime；Observer 关闭不结束订阅，不另设通知 peer/CLOSED 来源。通知 ObjectClosed 的业务故障由原维护推进闭合。447 的旧措辞已按此修正。异步消费者位于 libfs/client/watch.rs 与 libservice/client/watch.rs，只管理自身 Observer；宿主 world 唯一拥有 Dispatcher，两个任务置于既有 InitTask/ReadDuty 运行体，不新建 Runtime 或 Assembly。库报告本地 Complete 候选；init 在 Watcher quiescent 后封 Dispatcher，仅 rpc_complete=false 时 wake 运输，运输回复来源实际注销后 Complete 并 wake Watcher，Watcher 再 Complete。已逐处核对 fs/service 两组；Runtime 会拒绝向已 Complete 任务 wake，不能把双向无条件 wake 当作收束握手。

**本轮审查与处置。** sub-11 只读施工结构审查确认三个 P2：W05-01 Lifetime 来源 Remove 等回执仍 Runnable；W05-02 Rename 对被改名 target 自身发 Rename；W05-03 seq 耗尽未释放活动 token。源码分别改为实际移除等待 Parked、只更新直接源/目标父目录、逻辑 State 统一终态释放弱目标/token并保留 payer。W05-03 的独立 host 回归使用真实 Budget/Counter/GrantLease 收费，在耗尽后验证弱目标/token清空、Grant/WaitSource/Bytes为0且permit恢复；不构造伪 Kernel 对象。W05-01 正常目标路径停驻/回执可执行，实际 Remove 失败暂没有正式故障注入接缝，不能由正常 QEMU 冒充覆盖。独立缺口 owner RubyRaven、协调 MellowPond；触发条件为既有正式来源设施可控地拒绝/延迟实际 Remove；补证标准为未收到真实回执时原 Request/角色/费用完整、任务不连续 runnable自旋，按集合失败期限重试，回执后退款。不得新增模拟能力或测试专用服务/运行体。

**消费者与编排。** fs async 重查/旧 Ack/退休组合只在已就绪 primary 上运行一次，位置为 confirm_primary_fs_ready 之后、launch_fal_consumer 之前，不依赖 consumer/secondary。两个实例为同一 MemoryBackend/ConsumerTask，实现的来源撤出、在途责任、重登记与 captured sequence 不依赖实例；secondary 保留普通同步路径、test_fal 两 provider 的独立 Watch 合同、SRV2 independent endpoint withdrawal，以及原发现/撤名锚点。服务 async 在同期用私有名 init.async-watch 完整跑初始 NotFound→Ack、primary真实 endpoint Register→Query/GetEntry→Ack、Withdraw→NotFound→Ack与两任务stop；scoped authority 仅 ExactName/Create|Replace，不加 Delegate/GRANT。fs.secondary 的同步发现/撤名仍由 registry_watch 与 test_fal 实际执行。

**registry 场景 owner。** A→B→C 的两个真实 endpoint 和旧绑定不转向 C 依赖 secondary，init 无不靠时长的命令等待外空档；实际 exercise 已迁到 test_fal::registry，在 CONTINUE 返回后、COMPLETE report 前运行。唯一 registry 场景实现文件 user/tests/test_fal/src/registry.rs 同时提供出生前 queued_control 给 init 调用，旧 user/services/srv_init/src/registry_checks.rs 及 exercise 调用删除。init 从现有母 authority DelegateName ExactName init.registry-probe，仅 Create|Replace；出生后复用 AUTHORITY_GRANT_KIND/空正文/单槽 Mailbox 交付。init root 创建并支付 inbox，receiver以R|WAIT|MANAGE出生转移给test_fal，不带G；root ledger 持sender到送达并显式关闭，spawn失败沿既有Consumed/Retained路径收束。test_fal有限接收并核parent_pid/kind/空正文/单槽和实际senderW|WAIT|T无G，接收后关闭inbox，探针后关闭scoped authority与所有控制。不新建交付协议、服务或运行体。

**开发验证，按证据分开。** ①实际成功：fs-watch-virt1 guest两provider均输出 FS1 independent Watch contract passed（权限/mask、旧Ack/直接目录非递归、Unbound/TargetRetired、重复Cancel、保留Observer仅关最后控制→CLOSED→Quota恢复），最终双provider回收/reset；严格退出1仍G05-20旧流锚点。②本轮失败：virt2退出124、init stream read failed，归因收束第二次向已Complete RPC wake；virt3/virt4已跑到primary async recheck/old Ack，但test_fal CONTINUE命令到期panic，均退出124，不能归G05-20。virt4实际clock start27.571954s→initialAck28.502076s→changesAck30.087762s→retired31.025256s，每步推进段长3.453302s，secondary31.530113s开始、32.186601s初始Ack，命令窗口中途到期，确认编排耦合。按上述授权移动真实工作，不扩大deadline/改脚本/加sleep。③仅源码/编译：27项libfs host单测通过（fs-watch-host-tests2.log）；host clippy -Dwarnings通过（host-lint3）；全user RV bins/stress feature target clippy -Dwarnings通过（target-lint10）。新顺序virt5运行中；W04-02不同账户费用、W05-01故障注入、真实Rename及其余G05仍未补证。所有日志位于 artifacts/service-p5，失败guest日志由路线保存在artifacts/failed-acceptance-*.log。

**删除与 G05 核销（编号更正）。** 本轮新增源码机制为 WatchTable/State、两个领域 ConsumerTask 与正式watch场景；清除旧registry_checks文件和init长段调用、旧通知peer来源措辞、无条件回wake及target自身Rename。没有adapter/reexport、新测试服务或验收锚点删除。G05-02接通Watch主体，真实Rename由后续Derive补；旧Ack、冻结绑定、注册晚领/退款与撤名属于P3责任门，原记录曾误标为G05-05/06/07，现按894—919的唯一表更正：G05-05为纯值真实类型，G05-06/07为未Start/丢弃Offer，不能据注册锚点核销。G05-20流控制聚合锚点仍未关闭。该轮的后续实际证据、间歇失败与Derive接线由下节承接；未归档、未commit/stash/reset/push。

**新顺序与间歇 Watch 准入（W05-04，未关闭）。** 三项调整后 virt5 已跑完两种 async、queued/退款、test_fal A/B/C冻结旧绑定、primary独立Watch，secondary饱和 Watch 循环在 quota 前 panic，初始日志未打印具体Err，严格退出124：failed-acceptance-20261010-142502-89420.log。只新增Err诊断的 virt6 未复现，两个provider独立Watch与withdrawal/reset均到达，严格退出1唯一G05-20：failed-acceptance-20261010-142845-91972.log；不把它解释为已修。virt7逐次阻塞诊断显示每实例24项正常达到Quota并回收/reset，也退出1 G05-20（failed-acceptance-20261010-143148-95095.log），该诊断会改变时序，不能据此排除间歇失败。

**W05-04 收敛门。** owner RubyRaven、协调 MellowPond。删除来源注册/移出、应用段与命令开始的临时打印以及逐次准入 debug；固定原 deadline/脚本与准入顺序，用原 held.len 和 last_admitted_ns 标量只读计数，失败时一次正式panic报告真实Err/count/last_ns/deadline。time::snapshot 增加一次非阻塞syscall，可能影响时序，未增加容器/收费或等待。计划同一源码 normal virt 3轮、virt-stress 3轮串行；日志 fs-watch-repro-<route>-<n> 位于原artifact目录。若复现，核对其它资源先失败的错误映射、旧控制Lifetime实际注销/退款时刻、两实例账户/限额是否共享及caller原deadline；不可放宽断言。若均不复现，只登记未复现间歇失败、残留假设和触发补证条件，由MellowPond决定是否放行Derive，不能写成已修/全验证完成。已有主计划W04-02、W05-01故障注入、真实Rename与G05-20保持原owner和门。

**W05-04 六轮结果：未复现，不是已修。** 矩阵源码固定；每轮退出1均因现有严格脚本首先缺 FAL2 Open 流锚点。对脚本 required 完整只读核对，非FAL锚点无缺失，24项尚未恢复的旧FAL锚点统一由既有G05-20承接；新增fs/service async、两个provider独立Watch和withdrawal全部到达，stress三轮的control-plane/race16/16也到达，无panic/Watch准入错误且正常reset。结构化摘要为 artifacts/service-p5/fs-watch-repro-summary.json。

| 命令/轮 | 退出 | W05-04复现 | guest完整日志 |
|---|---:|---|---|
| just virt / 1 | 1 | 否 | artifacts/failed-acceptance-20261010-143821-98307.log |
| just virt / 2 | 1 | 否 | artifacts/failed-acceptance-20261010-143945-1547.log |
| just virt / 3 | 1 | 否 | artifacts/failed-acceptance-20261010-144105-4383.log |
| just virt-stress / 1 | 1 | 否 | artifacts/failed-acceptance-20261010-144228-7146.log |
| just virt-stress / 2 | 1 | 否 | artifacts/failed-acceptance-20261010-144641-15420.log |
| just virt-stress / 3 | 1 | 否 | artifacts/failed-acceptance-20261010-145057-23602.log |

runner日志逐项位于 artifacts/service-p5/fs-watch-repro-<route>-<n>.log，退出码同名.exit。已排除“新顺序仍落在CONTINUE窗口”和本矩阵中的稳定两任务收束失败；不能排除virt5的具体Transport/status失败、前轮实际控制退款滞后、其它资源先耗尽或本测试24项饱和整体deadline的时序边界。原virt5未采具体Err，不能事后推断原因。触发补证条件为任何相同panic或Watch准入非Quota出现、源码/平台/部署限额变化使该路径再次可达；须保存原Err、准入count/clock与真实控制Lifetime注销/退款序列，按三种候选根因定位并补独立回归后才关闭。当前报告送MellowPond决定是否放行Derive；任务整体、W05-04与G05门均未宣告完成。

**放行与期限结构（2026-10-10）。** MellowPond 认可六轮矩阵，允许进入 DeriveDirectory，但 W05-04 不关闭。只读核对 user/tests/test_fal/src/watch.rs 的饱和循环原来确有一个外部 quota_until=deadline() 供24项复用；已按授权移进循环，每次独立 Watch 准入获得自己的同值有限期限，达到真正 Quota 才结束，任何其它错误仍panic，未扩大单次期限或放宽断言。消除“饱和整体5s导致较晚请求先过期”的候选结构，不推断它就是virt5原始未知Err根因。改动随Derive批下一次virt复验。just check（fs-watch-check-final.log）已成功退出0；上批六轮之前源码不因这项小改动重写证据。Derive批按已确认范围闭合真实目录签发、各运输上限/ceiling/母root与同backing Rename消费者，补真实Rename/G05-02；不commit/push。

**饱和结构上界与部署唯一真值。** MellowPond 核定现有 srv_fs package 增加同名 lib.rs 的 deployment 模块，仅拥有 NODES/GRANTS/ROLES/REGISTRATIONS/WATCHES/REQUESTS/TASKS/SOURCES 部署容量；boot 私有常量删除并引用唯一定义，test_fal 单向依赖 srv_fs(lib)，不导出 World/Core/Runtime/协议 facade。饱和最多 WATCHES.checked_add(1) 个独立准入尝试，每项原5s，达到Quota才停、非Quota/越界仍panic报告count，溢出作为不可能表示该部署探针的结构错误。此元数据不是运行时授权或额度查询，日后改为运行时装配时该常量消费方须同步迁移。目录/package/lib/bin identifier保持srv_fs，无Cargo alias；读过user/README命名规则。随Derive下一轮正式消费者复验。

### 第五批 DeriveDirectory 与真实 Rename（施工中）

**范围与责任闭合。** 路径必须在母 root 可达目录，准备时与Lifetime准入后的最终提交都复核目标存活、目录类别、当前弱父链母root范围/逐祖先Traverse、母GrantLease活动位、Derive位和目标ceiling。Issuance各自校验FsRights、完整u64 sender_transport与output_transport不超母上限；使用已准入AccessSnapshot，不因普通母sender关闭后的索引移除否定其使用引用。真实sender/Lifetime、GrantTable槽/fee/预建entry、回复容量和Lifetime来源在授权可见前准备，提交只安装并交回复，失败返还原准备owner。创建Request自身持DerivedGrant寿命，不新建临时Root task/测试运行体；普通最后sender关闭只移除入口并退款，provider停止才revoke共享活动位，来源Remove等待Parked且失败保owner。

**自然顺序。** 在实际MemoryBackend整理可复用目录范围复核，接Request的目录准备/提交/寿命分支（同批清除Derive Unsupported路径）；再用现有Client.derive_directory/Rename路径预算接正式消费者。目标父grant通过D产生临时运输副本，实际收到cap以related mailbox/真实context查表，CrossFilesystem、Permission、Revoked分开；同backing原子Rename按原版本/成员/父链约束提交。复用test_fal两真实provider，覆盖派生上限、受限ceiling、目录watch源/目标父Rename位与非递归、自身目录改名不误发目标Rename、跨backing拒绝及正常收束；不把尚无正式revoke入口的真实撤销观察伪造成通过。暂无新增外部设计选择，所有运行证据待接线后登记。

**签发实现与结构复核。** GrantTable.prepare_derive 使用准入时真实 AccessSnapshot；母索引普通删除不改 GrantLease 活动位，最终 backend.check_directory_issue 仍复核当前目录范围、逐祖先 Traverse、母/目标 Derive 与各完整运输上限。PreparedGrant.into_parts 在发布前把 GrantPublication、sender、Lifetime 交给对应 owner；GrantInstall/GrantObserver 旧交付结构已删除，user 源码 rg 零引用。Request 的 PreparedControl 明确区分 Watch/Directory，共用真实 Lifetime 来源登记/撤销，原 Request 提交后不创建额外任务，回复责任结束释放请求费用与母快照，保留派生寿命直到注销后 remove；stop 才 revoke。GrantTable 的 Counter permit、Grant/Bytes/一个 Lifetime 来源 Charge、Rc 活动租约和 OrderedTable entry 均逐请求预建；commit 只 insert_prepared，没有提交分配。根发行同步使用同一 publication API，根来源 Remove 未回执时改为 Parked，不自旋。

**源码 finding W05-05（错误映射，已修）。** 原 accept 对 Rename 槽1先要求 MailboxSender[WRITE]，错型合法运输输入被丢成 RPC ServiceClosed；已把槽数留在准入检查、角色与真实 grant 身份交给 validate_received，错角色返回领域 Permission。正式消费者用 NotificationSignaler 和同 mailbox WatchControl 两种真实能力，验证零输出槽 Permission；不同 provider 的目标真实目录 grant 验证 CrossFilesystem 且源值保持不变。真实管理 revoke 暂无对外接缝，不能用错角色/关闭入口冒充 Revoked 运行；host 的当前活动位检查及未提交 Rename 回归和源码映射提供对应分层证据。

**回归边界。** 新 directory_issue_rechecks_selected_scope_ceiling_and_complete_transport_limits 捕获准备期间所选目录移出母root仍提交、目标ceiling无Derive、完整sender/output分别放大及显式撤销仍提交；使用原 Budget/NodeStore/授权fixture，无Kernel fake。admitted_directory_use_survives_ordinary_owner_drop_but_revoke_blocks_rename 捕获普通原owner Drop误撤销已准入AccessSnapshot，以及撤销目标后Rename仍准备；这只覆盖共享活动位与backing责任，未注入实际Kernel sender消散恰落在已准入Derive/来源等待期间的窗口。真实消费者已验证独立child签发后关闭最后母sender，child仍读取而失权Set拒绝；窗口缺口 owner RubyRaven、协调MellowPond，触发既有正式RPC可控暂停/撤销接缝，补证须在实际准入后关闭母入口或显式revoke并分别得到完成/Revoked、所有来源退款，不造模拟caps或测试服务。

**W05-06：core 外部上限前未到 reset（未闭合，暂停 Bind）。** derive-virt1 的 just virt 退出124、完整日志 artifacts/failed-acceptance-20261010-152702-36338.log；无panic，两provider在152/153行已真实输出 FS1 independent DeriveDirectory and Rename passed，后续撤名与双provider监督到Dead、boot退出前实际 memory/grants空断言成功；最后已到公共时间检查，90s既有外部上限收割。该失败不归G05-20，也不能报core通过。MellowPond要求区分公共时间后的 committed_kill 停驻与新增工作推动起点的总时长增长；只读检查fs-watch三个virt日志确认没有逐行时间戳，不能从文件mtime推断旧分段耗时。当前添加临时用户态阶段mtime边界及provider stop任务/来源/退休状态与drained空表记录；不改Kernel、脚本、90s、业务deadline或加sleep。打印会改变时序，按诊断证据处理，定位后删除。正式virt-stress既有420s路线仅取同mailbox角色等运行证据，不用它关闭core上限问题。

**最终开发验证与三类覆盖。** 以下源码已清除所有临时 phase_clock、provider DIAG 与消费者 Trace，未改90s/业务deadline、未删正式场景或严格脚本。host命令全文 `just --command bash -c 'cd user && cargo test -p libfs --target aarch64-apple-darwin'`，derive-host-tests2.log退出0、29项；`just --command bash -c 'cd user && cargo clippy -p libfs --all-targets --target aarch64-apple-darwin -- -D warnings'` 的 derive-host-lint-final.log退出0。RV64命令全文 `just --command bash -c 'cd user && cargo clippy --workspace --bins --exclude test_fp --features srv_init/acceptance-stress --target platforms/riscv64imac-unknown-erhino-elf.json -Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem -- -D warnings'`，derive-target-lint4.log退出0；最终 just check 与 just clippy 各退出0，derive-check-final.log、derive-clippy-final.log。以上日志均在 artifacts/service-p5，.exit保存实际退出码；lint2因unused import退出101的失败保留，修复后lint3/4成功，不掩盖失败。

| 覆盖类别 | 当前事实 |
|---|---|
| 新协议且实际执行 | derive-virt1 两实例真实签发、完整上限/目标ceiling拒绝、独立child、真实同backing Rename与直接父事件/祖先和目标不递归、跨backing拒绝；derive-virt-stress1另含同mailbox WatchControl及异型Notification输入→Permission、WATCHES+1/逐次5s准入、控制退款恢复、双provider回收、16/16及reset。完整guest artifacts/failed-acceptance-20261010-153020-40070.log；命令just virt-stress退出1唯一首缺G05-20旧Open锚点，不报整路线通过 |
| 仅host/源码/编译 | 目录在准备与提交间移出母root、母显式revoke后的Derive/Rename拒绝、普通原owner释放后已准入快照可用；对应两项独立host回归与真实context映射。实际Kernel准入后故意暂停并关闭母sender/revoke的时点尚未注入，不由已成立child运行替代 |
| 剩余门 | W04-02双账户、W05-01真实Remove拒绝、W05-04未复现间歇Watch失败、N6注入窗口仍按唯一记录保留；W05-06为新增正常场景总时长与core路线余量，处理方式待协调者/用户确认。Bind/流/Copy未接，不前进冒称完成 |

**删除清单。** 删除GrantInstall/GrantObserver及prepare_derive重新查母索引的旧路径；目录弱父链范围复核由cursor/派生共用，不复制。root原8项私有部署常量移到srv_fs唯一deployment，test_fal单向依赖元数据，没有facade/alias或启动新字段。`rg -n '\b(GrantInstall|GrantObserver)\b' user`退出1零匹配，证据derive-deletion-proof.json；临时诊断五个文件rg零匹配。Grant改动累计git diff --numstat为91加/103删，该数含此前Grant迁移，不当作本批独立删除量。新provider/boot/derive文件此前尚未跟踪，不用git diff忽略的新文件数冒充无改动；全任务旧FAL删除侧由既有批次唯一表拥有。

| G05核销面 | 本批状态与尚缺部分 |
|---|---|
| G05-02 | 正式同步Watch合同、弱目标Unbound/Terminal、Query/旧Ack/重复Cancel、权限/mask、最后控制退款及两任务异步组合已有实际证据；本批补真实Rename父/自身/祖先事件。对应正常路径补证齐，W05-04间歇准入问题仍独立保留，不宣告整个Watch无缺口 |
| G05-03 | Watch准入饱和/最后控制恢复已实跑；Node/Bytes/Grant/会话其它额度和全部费用仍未完成，不核销整行 |
| G05-04 | 同backing真实双目录改名、目录改名、跨backing拒绝与基本分页已实跑；目录防环、准备期父链改变及替换前空目录复核已有host证据。Replace冲突/成功原子替换的完整实际消费者尚未接，不核销整行 |
| G05-19 | FsRights、完整运输上限、目标ceiling与独立child已实跑；Open会话和跨Boundary受限出口仍待Bind/流，不核销整行 |
| G05-20及余项 | 原表所列流/Copy/Bind/未领取回复与剩余纯值门保持各自标准；严格脚本未改 |

**W05-06 A/B与清理。** MellowPond认可B（正常推进、起点变晚）。当前diag1完整日志 artifacts/failed-acceptance-20261010-153625-49707.log，退出124；隔离control2命令为副本cwd下just virt，90s/core/50%与主树一致，只删除两处derive::check调用，783个实际路径逐文件比较无其它差异，manifest为derive-control-manifest.json。control1首次因新artifact输出目录不存在未进入QEMU，退出1并保留日志；创建目录后control2到reset、退出1 G05-20，仅诊断不计验收。副本原guest已复制为derive-core-control2-guest.log；git worktree remove --force已清理（derive-control-cleanup.log退出0），git worktree list仅主树，所有QEMU/throttle/acceptance进程已结束，主树正式场景保留。

| 时钟边界（s） | 当前diag1 | 对照control2 |
|---|---:|---:|
| primary FS1开始 | 15.689467 | 17.138945 |
| Derive开始→primary完成→secondary完成 | 61.431833→67.580462→75.823751 | 两调用被省略，仅诊断标记62.414365→62.416830→62.418049 |
| provider全部回收 | 81.048673 | 64.506198 |
| public time完成 | 87.818475 | 69.564844 |
| committed_kill开始→第一个1024项Set | 87.820148→89.467788 | 69.566236→70.685646 |
| 第二Set→spawn→ready→Dead→refund | 90s前被收割，未覆盖 | 71.800827→72.751228→73.191841→73.293151→73.298283 |
| committed_kill完成 | 未覆盖 | 73.300632 |
| reset前最后时钟idle-verified | 未覆盖 | 76.925041，其后实际reset；reset本身没有独立mtime |

diag1新增两段分别6.148629/8.243289s、合计14.391918s；两providerstop→drained为0.426917/0.671447s，source/task在真实drain后消散，memory/grants为空。初始stop只记录tasks5/9、sources各5与RetirementState::Waiting，没有伪造精确退休队列长度；drain后的空表证明其为0。control2的public→committed结束3.735788s、随后到reset前最后时钟3.624409s，约需7.4s；diag1public起点87.8s只余2.2s。两边FS1起点本身有1.449478s波动，public起点差18.253631s不能全部归Derive。旧fs-watch三个virt日志无逐行mtime，不能从文件mtime推断旧分段耗时。

**子段独立诊断。** derive-core-profile1（just virt，退出124、90s不变）的guest为artifacts/failed-acceptance-20261010-154609-57395.log；本轮primary5.748861s、secondary6.686519s，与diag1不同轮，不拼接。时间只存消费者侧定长栈数组，由起点+12固定边界推导13槽，满则停、失败记缺测0、没有分配；场景末尾一次打印，未在provider准入/提交路径增加逐请求日志。Trace原数据及分解保留derive-profile-summary.json，源码已删除临时诊断。

| 子段 | primary(s) | secondary(s) | 实际调用依据 |
|---|---:|---:|---|
| fixture建立 | 0.474386 | 0.467902 | 5次Create |
| 根拒绝/目标类别 | 0.083816 | 0.079702 | 2次即时Permission/NotDirectory |
| issuer签发 | 0.052750 | 0.050161 | 1RPC，包含真实Lifetime来源准入 |
| 完整运输/领域上限拒绝 | 0.369841 | 0.412483 | 3RPC即时Permission |
| child签发 | 0.049926 | 0.090662 | 1RPC，包含真实Lifetime来源准入 |
| 关母后child读/失权/范围 | 0.120486 | 0.404452 | 3RPC与真实owner关闭 |
| Watch setup | 1.467062 | 1.737546 | 14RPC：4Watch+4Query+4Ack+2基准快照 |
| 正常Rename及事件断言 | 1.059205 | 1.391333 | 10RPC：Derive+Rename、2Get、父Query/Ack、祖先/自身Query |
| 跨backing | 0.139887 | 0.138374 | 3RPC |
| 两种错角色 | 0.419418 | 0.409002 | 1Watch+2真实输入RPC |
| 目录Rename自身/父事件 | 0.532791 | 0.532232 | 5RPC |
| 清理 | 0.979293 | 0.972670 | 4Cancel+5Remove |

最大两段为Watch setup与Rename事件断言，占primary约25.5%/18.4%、secondary约26.0%/20.8%；来源登记本身不能从含运输/调度的客户端时长单独分离，单次issuer/child约0.05–0.09s，并非占主导的期限等待。每实例59次真实RPC；无sleep/expired/wait_until负向期限证明，所有预期失败是即时领域错误，Timeout会失败断言。跨进程RPC、实际来源/角色收束与本轮宿主时序共同构成总时长，不以单轮比值冒充性能基准。方案由MellowPond向用户确认，在此之前不改90s/场景、不进Bind。

**只读合并清单（未实施）。** 按MellowPond新指令，路线方案待用户确认期间不改正式场景，不进Bind。原FS1 independent Watch contract在同一MemoryBackend已经覆盖INITIAL→Query/Ack、旧Ack/重复Ack、READABLE清位、一般Cancel与最后控制退款，以及Create/Modify/Delete/Unbound/Terminal；它没有Rename位的直接父/祖先/目标身份反例，不能删除Derive的Rename特有断言。

| 可合并点 | 保留的独立契约及条件 | 可省RPC/每provider |
|---|---|---:|
| Watch成功体info代替额外初始Query | 四个实际Watch与首次Ack仍保留；INITIAL/Query/Ack通用合同已有独立Watch消费者覆盖。使用已验证创建回复的sequence发Ack | 4 |
| 首次Ack返回快照代替祖先/目标两次基准Query | 原Ack回复同样是已验证Snapshot，后续Rename前没有fixture变更；保留Rename后的两个真实Query比较 | 2 |
| 正常Rename后两个父Ack不再重复验证一般Ack | 两父Query/RENAME位必须保留，后续目标自身改名用真实前后快照比较，最后仍Cancel收束；旧Ack和幂等清位由独立Watch合同覆盖 | 2 |
| 进一步把Rename嵌入既有Watch fixture，复用源父与属性的两个订阅 | 需把Rename放在原属性Unbound/Terminal之前、属性mask明确含RENAME；目标父与祖先订阅及四种Rename身份断言仍建立。仅列方案、不实施 | 在上述本地复用后，额外4（2Watch+2首次Ack） |

前三项本地复用共8RPC/实例、两实例16；进一步跨场景复用最多额外4RPC/实例，仅计明确替代的订阅/首次Ack，不估算其余fixture/Cancel/数据读写节省。不能把剩余4个实际Watcher或Rename特有Query全部当重复，也不把观察段耗时按RPC数线性折算。跨场景合并会影响fixture寿命与mask，须由协调者确认后成组重验；当前代码保持profile所测59RPC/实例的正式场景，临时Trace已删。

**本批交付前结构检查。** 主实施者按REVIEW核对独立knowledge owner、真实role/context与完整Rights、不分配提交、prepare permit/收费/来源准入、母活动位与范围最终复核、普通sender关闭与显式revoke分离、原Request持续持有清理、Remove回执前Parked、Outbox所有输出能力清理、源/目标父Rename、Namespace固定借用与同一预算、旧交付结构零引用和impls。未新增unsafe、锁、Kernel工作或运行体；纯逻辑host与实际cap运行分开。正常/失败/取消/期限/退出/释放路径有owner，真实注入窗口与W04/W05/N6仍按原记录补证，不报系统整体交付。git diff --check通过；不commit/stash/reset/push。下一步仅等待core路线/场景组织裁决后按授权补证，Bind保持暂停。

**协调验收与断点（2026-10-10）。** MellowPond已收取本批报告并认可G05-02正式核销，G05-03/04/19的部分补证及剩余缺口、1030编号更正、旧交付结构零引用和诊断/隔离清理均已认可。core路线方案及只读合并清单由协调者交用户裁决；裁决前不改场景/90s、不进Bind。允许期间仅预读Bind/Unbind实际源码和298—310路由装配合同，不启动下一批编码；当前无需提交，全部改动留工作树。

**用户路线裁决与W05-06关闭。** 用户确定超时预算撞限翻倍、正式场景全部保留，不实施1119的合并清单。唯一执行默认值位于Justfile的VIRT_TIMEOUT，本轮仅将该值翻倍；hetero/nofd独立期限未撞限所以不动，脚本未改。BUILD-AND-TEST只登记政策并指向Justfile，不复制core默认数值。just virt复验日志derive-core-expanded1.log退出1，完整guest artifacts/failed-acceptance-20261010-161353-64174.log；两provider的Derive/Rename与独立Watch、全部非FAL必需锚点、provider收束和reset到达，无panic。首失败仍G05-20旧Open锚点，其余缺失为原有未恢复的FAL/G05系列，没有新非FAL失败，不报整路线通过。正式guest日志出生→末次写入跨度100.622691s，作为约100.62s QEMU实际运行观测（含少量wrapper/flush成本），不是guest精确mtime或编译时长；结构化结果derive-core-expanded-summary.json。按MellowPond允许的复验门关闭W05-06，临时副本/诊断仍已清理。付款前置→Bind/Unbind→流→Copy已正式排定；先通用显式付款，主计划W04-02实际多账户随装配闭合，避免Bind返工。

### W04-02 实际多账户装配接续（2026-10-10，RubyRaven）

通用前置已由MellowPond接受；[执行付款记录](todo-2026-10-10-execution-funding.md#施工记录2026-10-10rubyraven)拥有E1—E7与删除证据，E8在本节独立记录。前置formal core正常reset、非FAL缺失0，旧G05-20仍阻严格路线；不重复当作实际多账户证明。当前无W04多账户源码或运行证据，boot仍single Account。

**实际接缝。** sub-15只读地图已结合亲读核实：boot.publish单sender单slot/空body，primary三种根+Ready合计4报文；filesystem.accept_fs_provider按kind保管到AcceptedFsProvider，test_fal已用正式Startup5slots+ScopedAuthorityInbox。域/执行金额仅AccountView::usage/budget_usage可查，fs/provider/consumer没有金额读点，停机当前只检查is_empty。现有boot Dispatcher是get_task_mut→begin方式，cap1；init同一ReadDuty只有一个ConsumerTask/cap1。因此单账户金额及真正A/B同Dispatcher仍是待实施事实，不把结构支持多inflight写成实际消费者。

**施工范围和已确定布局。** 同一Budget建H/A/B，H固定设施/Ingress/Retire/Release/Maintenance/Dispatcher共享reply；A/B文件根、DirectoryAccess/NameAuthority、Root task/观察与各请求沿可信配对收费。三种根publication各以有序A/B两个真实sender移交，正文仍空、不带付款id，报文数仍3根+Ready=4；boot/init/test_fal同步改变角色槽契约，secondary当前正式A出口继续原接缝。init保管两组owner，consumer得到B文件/目录/私有authority，停止、转出与未知/失败消息均沿现有RootSupervisor/Packet值返还关闭，不遗留GRANT复制权到子进程。Root lifecycle的world.root_context单值须审定实际读点后删除或归其owner，不能让第二根覆盖首根身份。

**额度与验证。** A Watch局部上界取当前部署WATCHES的一半，B局部取部署全数，全局仍WATCHES；界值来自现有部署能力，而非任意时长。正式同provider消费者使A局部真实饱和（global仍有余），B继续准入，global满时B局部仍有余却Quota；释放A真实Control/Lifetime后B恢复。Query/Ack/Cancel、通知目标释放后的终态壳、注册控制收费要在provider持有的真实view上读金额，与真实owner/来源退休边界核baseline，不开用户态账户金额查询业务API。最终停止在真实Runtime/Assembly/world释放后逐域/执行核零。实现收入和清理顺序细节仍以代码/本节记录补齐，未形成证据前不关闭W04。

同一既有boot Dispatcher按当前消费扩到2 in-flight，沿get_task_mut→begin既有服务入口冻结A/B下游view，驱动仍原单Runtime/Assembly，真正结果及来源收束后核各自baseline；不造测试服务、新运行体或测试专用actor。Root任务及来源容量按真实新增角色和同时调用上界推导到srv_fs部署唯一元数据，消费者只读该部署事实。下一步先写完上述发行/owner/金额观察具体接线，再按依赖迁boot、init、正式consumer和停机核对；新工作证据与未闭合责任只写本节，完成后进入Bind。

**具体接线与确认条件（同日）。** MellowPond已确认上述布局及四条件，实施已开始。各file Root直接持自己的context、stopping/revoking、Lifecycle来源和Charge；World单值root_context已删除，boot停机遍历实际Root任务调用stop并wake，revoke发生在该owner的推进内。发行者srv_fs创建A/B sender并按对应账户支付domain与Root Task/来源；Packet独占转出母本，init AcceptedFsProvider保管、复制后Startup转给消费者，不含GRANT；init保留的母本及B组在释放provider前显式drop。Scoped A/B NameAuthority由实际母根DelegateName发行，经现有inbox一报文双槽交付；Packet失败持能力Drop，RootSupervisor按既有Consumed/Retained与ledger收束出生grant。

provider内部Billing只持三个真实AccountView配对和观察记录，不持Charge、不暴露接口/消息。fs/service Ingress在accept推进边界捕捉整组三账户实际金额，核Request/Outbox/Bytes/WaitSource增量只在原账户，执行配对相同，其他账户不变；fs控制表私有，原Watch记录与实际Request增量在libfs provider::Request::accept内部独立核对，不把backing/付款表导出。停止顺序Runtime/Assembly实际drain/close→backing/角色存储drop→逐domain/执行金额零；所有不匹配有明确assert/panic英文错误。

同Dispatcher证据在primary发行前：六Root均ready且全部sender仍由boot暂持，existing Dispatcher以两项入口Directory真实GetEntry并发调用（原Registry mailbox/Ingress处理，不产生测试角色/actor）。A/B caller view分别冻结到两调用的进度/CLOSED来源，baseline在两调用前捕捉；必须观察各自InputBytes实际增加、两reply均NotFound、实际call/source和请求责任收束后所有A/B领域/执行费用返回baseline，才发行三报文并Ready。Dispatcher共享reply来源继续在H，不要求其常驻费用在单次调用后消失。

额度由deployment::ENTRY_WATCHES唯一拥有（WATCHES/ENTRIES与WATCHES），test_fal只引用。正式check_accounts持有A额度个控制后A Quota，持有B至global满后B Quota且B local尚余；仅释放A一个真实sender并保留Observer，等CLOSED后继续用真实B准入成功确认permit/记录退休，随后所有Query/Ack/Cancel控制drop并等CLOSED。旧watch.check和registry.exercise分别从A/B真实入口执行，包含目标退休后终态控制与Withdrawn注册壳。当前新增代码已通过RV64定向check（target-check2.log退出0）；首次target-check1.log私有watches/缺依赖/usize mask错误已修且保留日志。正式运行、完整lint、交付前检查尚未形成，W04不关闭。

**W04-02 关闭报告（2026-10-10）。** 本门正常真实多账户合同已闭合；不归档整个专题，也不把它等同G05/P4全面交付。

| 三类覆盖 | 当前证据与边界 |
|---|---|
| 已切协议并实际运行 | 同Budget H/A/B、A/B三种根/Scoped authority双槽与consumer7槽、原账户control Request/Outbox/Bytes/WaitSource实际增量、同Dispatcher两个A/B请求的实际InputBytes增加与完整baseline、局部A满时B继续/global满时B拒绝/只释放A真实Control后B恢复；A/B分别执行fs弱目标终态、service Withdrawn壳及Cancel→Terminal Query/Ack；最终两provider领域与Task/InputBytes全0并正常Stopped/Dead |
| 仅源码/无本轮注入 | 出生交付失败、来源拒绝/延迟实际注销、提前release缺业务覆盖的正常退款分支逐owner审阅；未新造注入设施。N6/W05-01原专项窗口仍按原条目保留，不用正常运行冒称 |
| 未接与剩余门 | Bind/Unbind管理、异步Boundary按需收窄、流/Open/Copy和其G05标准仍待后批；严格路线因G05-20旧Open锚点退出1，没有删场景或放宽脚本 |

**验证文件。** RV64定向check2退出0；just check退出0（artifacts/service-w04/check.log）；just clippy2七面全部退出0，最后P2观测修复后的定向stress-feature RV64 lint退出0（final-target-lint.log）。首次编译及needlessborrow lint错误已修、原失败日志保留。just virt1/virt2和just virt-stress1均退出1，完整guest分别为artifacts/failed-acceptance-20261010-191544-89957.log、failed-acceptance-20261010-192934-5030.log、failed-acceptance-20261010-192228-94821.log；全部正常reset，非FAL缺失0，首缺仍G05-20。acceptance-summary.json保存按实际required列表的逐锚点分类；stress有16/16矩阵，最新版core有内部覆盖矩阵primary fs A/B三项全true、service A/B五项全true、withdrawn A/B均true。secondary只发行A，无registry，B/服务全false正确，仍要求未使用账户全0。时长不作付款性能结论，未撞路线期限，无翻倍动作。没有新增host镜像测试；本门观察真实Kernel角色与目标消费者，前置host事实仍由执行付款记录拥有。

**独立审查与作者复核。** sub-16只读审查支持双槽/能力收束、Root私有context、冻结配对、同Dispatcher实际收束及真实恢复。唯一P2为把验收操作历史当正常停机必需条件，可达Ready后提前release/来源错误stop；已删除覆盖完整性硬门，只内部打印真实矩阵，金额增量和最终全0仍无条件assert。不再因未消费入口阻PROVIDER_STOPPED。MellowPond已认可修法；作者亲读最新206—212 stop零账/观测分离及release/来源停止路径，不重复首审。最终core2在新代码下实际通过上述矩阵与全0；异常/提前停止未注入，不谎报。按REVIEW复核付款/authority各自owner、准备收费/能力失败完整返还、无新运行体/公共账户API、Root同预算来源、Typed Startup消耗、实际退款后恢复及文档。

**删除清单与容器。** 删除World.root_context字段、Root advance赋值及boot单值revoke（src零引用）；单Account boot及单槽bootstrap发行/采用、5槽consumer与单一私有authority接线被有序A/B实际路径替代；没有旧分支、adapter、facade或配置开关。旧FAL大批删除不重复算本门。部署meta单向提供局部/global容量与2inflight来源上界，init新增srv_fs metadata依赖无反向；Billing只存真实view和固定快照/观测数组，没有payer缓存真值或Charge。控制测试Vec在发请求前try_reserve_exact，root sender中间容器为固定数组；提交没有新增容器分配。新增SAFETY仅duplicate新角色的独占Capability接管；未改Kernel/shared ABI/验收脚本，未commit/stash/reset/push。notes/impls/fs、service、rpc回填当前装配/来源消费者，KNOWN_ISSUES和本篇当前入口同步。三个runner pid 89609/94402/4700已无进程，路线脚本完成原QEMU收束，无临时服务/隔离目录遗留。

| G05核销面 | 本门增量与保留 |
|---|---|
| G05-02 | 已核销正常Watch合同不变；新增A/B原付款、终态Query/Ack/Cancel与真实退款，不扩大为故障注入全覆盖 |
| G05-03 | 补同Budget局部/全局Watch耗尽、跨账户隔离与实际金额退款；Node/Bytes/Grant/会话其它饱和门仍未齐，不核销整行 |
| G05-04 | 原Derive/Rename实际证据不变，本门未补完整原子Replace门 |
| G05-19 | 增付款沿可信入口继承，跨Boundary受限出口/Open仍待Bind/流，不核销整行 |
| G05-20/21/其余 | 旧流首失败不变；G05-21接续Bind。其余原表标准保留，不把W04关闭冒充旧能力恢复 |

**关闭依据。** 原W04-01的付款源码修复现由实际不同于H的A/B入口补证；实际资源收费而非仅比较account id，控制终态目标释放后仍收费原账户，实际全部owner/来源退款后零账。同Dispatcher真正两在途调用并观察两原账户InputBytes增加，completion后各自全domain/execution回baseline。所有必需消费者已在原init/test_fal/两个provider运行体接通，旧单值根/单账户发行路径删除，验证与交付前检查完成，本W04-02标记关闭。下一步Bind/Unbind接线方案交MellowPond后按确认合同施工。

### Bind/Unbind 接线方案（已审定并施工，2026-10-10）

本节是下一批施工设计；W04已关闭，MellowPond明确要求审定前不改代码。本批兑现298—310，不恢复旧DelegateTask/管理裸sender、不增加Runtime/Assembly/测试服务。当前实际断点：boot把route receiver保存在局部；无管理角色/Runtime ingress；prepare_binding存在但无Unbind Intent；boundary_export需收窄时Unsupported；BindingState的Rc同时持快照和issuer/shared Arc，替换/目录退休后靠Drop关闭；Dispatcher独占在FileTask slot，业务Request无法借另一Slot启动调用。以下统一迁移实际owner。

**1. 管理 Role、Root 与可信付款。** libfs新增独立ManagementTable/ManagementSnapshot，普通GrantTable不收纳管理context，普通FsRights不因管理kind升权。管理记录拥有实际本地root NodeRef、允许的绑定领域/完整出口上限、不可变Funding、活动token及Grant/Bytes/WaitSource费用；可复用现有GrantLease收费/撤销原语，不通过callback隐藏普通表。srv_fs用已有route MailboxReceiver铸造A/B两个MintedSender，context取真实sender object_id并核related mailbox；Funding借对应tenant.domain/execution，在可信发行边界拒绝跨Account配对。管理Root任务各自保存Prepared角色、Lifetime、来源、context和关闭责任，先实际登记来源，再安装管理表项，再移交sender；Task/InputBytes及角色费用由对应A/B支付。管理Ingress/固定接收设施由H付费；已知context的管理Request/Outbox与下游来源由原A/B，不退回H。未知/普通grant context只走有界Permission/拒收，无管理副作用。

Primary第四种bootstrap报文为管理根，空正文、有序A/B双槽，定义kind于libfs::route；同轮更新boot的最多5报文结构上界、init Adopt/保管/关闭及实际consumer接线。母本GRANT仅用于出生，正式消费者去掉GRANT；沿已有inbox/Startup设施交付，不用正文account id。init保管的原route peer为无管理context临时运输端，角色交付完成后关闭，不能将它当管理授权。secondary同样铸造本地管理角色，由原bootstrap交给init；只要求实际使用的组发行，未使用view仍0。管理根关闭仅撤新请求索引，已准入Snapshot依自身寿命有效；显式revoke/provider stop通过token使最终提交失效。Root真实Remove回执后才关闭Lifetime/移表/退款，失败原owner保留。

**2. Bind 准备与提交。** 管理请求先完成Header/RPC/真实slot检查，冻结ManagementSnapshot与原Funding，Outbox/Task/回复来源准入后才准备业务。只解析本地管理root范围内的父目录：跨Binding或链接为明确错误，不返回管理Boundary；父NodeRef及管理token随请求保活。共同admit_member(name,0)先执行；完整max_rights/output分别不得超管理政策，也不得超目标发行者通过Derive验证的上限。输入槽1为目标issuer的独立临时副本，核MailboxSender实际角色与必需位，不接收caller提供账户或身份。

Bind在发送下游前预留/安装不可见、未链接的Binding记录，接管输入issuer唯一owner；shared槽先为空，避免回复到来后才申请退休承载。请求通过同一Dispatcher使用issuer独立副本调用DeriveDirectory空path，请求max_rights、stored sender至少W|WAIT|T|D、完整output；RPC使用原管理请求execution view、原绝对期限/剩余接续预算，完成唤醒原request，不借backing跨等待。目标回复完整核协议、deadline、DeriveDirectory成功变体/槽和实际sender运输；领域/output来自此次已验证签发请求的契约，不通过ObjectInfo当权限证明。成功把已衰减shared唯一owner填入原记录，再准备唯一成员entry/permit与版本。replace=false空位；replace=true只允许空位或旧Binding，普通本地节点TypeMismatch。成功体与Outbox容量在提交前备妥。提交短借用内复核backing未sealed、管理token/root范围/父ceiling与原父版本/名字条件及deadline，安装预建entry并link新记录，旧Binding失活/unlink，更新父版本及Watch；无分配、无Close、无等待。回复丢失保持已提交事实，未知结果不重放。

**3. Unbind。** 新增prepare_unbind及Intent::Unbind，捕捉管理Snapshot、父NodeRef、父版本、名字和所选旧Binding稳定身份；缺失NotFound，本地文件节点TypeMismatch，跨Boundary/链接拒绝。共同名字/完整请求校验在准备边界。预编码空成功体后，commit再次复核管理活动/root范围、父版本及名字仍指原Binding；只摘原成员、失活/unlink原Binding并写父Watch，后续退休由backing持有。失败返还完整准备owner，不通过普通Remove的Busy分支拼装Unbind。替代的同名Binding不能被迟到准备删除。

**4. 跨 Binding 出口。** 普通fs/OAL请求保留AccessSnapshot和所选Binding pin，先按本地遍历与该操作需要鉴权；effective=source.rights交binding.max_rights，output按完整Rights求交。已有shared入口限制足够窄则只复制实际可交付的W|WAIT|T及允许D；需更窄时用该记录issuer的临时副本发DeriveDirectory空path，分别指定更窄FsRights、新sender运输和新output。下游费用来自原普通请求Funding，而不是Binding创建者或H；目标自己的准入按其issuer绑定正常收费。路径保留实际consumed_bytes/remaining，不重置Follow预算、不会消费suffix后重新提交原业务。

异步返回后必须复核原Access活动/范围、原Binding稳定身份仍linked、provider状态、交集/操作必需位、期限和取消；不能按原名字重新挑替代Binding。失败清理已收到新入口，释放原pin；成功只把验证后的独立capability交给原Outbox返回Boundary。已经交付的出口不被Unbind召回；后续变更由目标provider管理。删除现boundary_export的需要收窄Unsupported分支，仅真实下游Unsupported保留。

**5. Dispatcher 与在途 owner。** 将唯一Dispatcher实际值移至World，由原FileTask运输任务持marker/id和全部来源/期限回调推进它；沿init ReadDuty已有组合模式，删除业务需Runtime.get_task_mut另一slot的假定。boot的Register/Withdraw/W04双账户调用和普通/管理Request都借这一值，不新增Dispatcher或运行体。原request的PendingOutbound保存txid、用途（Bind共享/Boundary收窄）、原期限/预算和Snapshot/Binding pin；begin_for将结果唤醒交回原task，每个request最多一项下游操作。未准入完整返还Request/临时issuer，已发送取消保留txid直到completion，Received输出与Completion.service各有独占owner。停止先封Ingress/新下游，原请求取消或收束后才封Dispatcher；Dispatcher来源实际退出、completion owner取回/清理后才完成请求及释放pin，不在stop中丢PendingCall。容量由REQUESTS+固定boot并发请求的真实上界推导，H共享reply仍唯一。

**6. Binding 退休决策。** 一次advisor咨询支持统一backing稳定记录，候选采用：内部payload区分File与Binding，保留四种公开NodeKind/wire，不把Binding变成可创建文件类型。目录成员保存Binding稳定记录身份；Resolved::Provider只持业务pin和只读限制快照，issuer/shared的唯一typed owner始终在record内，不再外逸Arc。准备Binding先以未链接不可见的收费记录取得承载，再接管issuer/发送下游，由准备request持pin；取消/失败/seal后不能提交时释放pin并进入同一退休队列，无临时清理分配或准备对象Drop关闭。未取得承载的构造失败完整返还输入owner。

成员permit与Binding记录permit分开，替换可转名字槽，旧record pin/关闭责任未清前不能复用record容量/费用。FsResource新增Binding占用槽，deployment::BINDINGS拥有其全局部署数；Node保持文件节点分类，不用Node收费冒充Binding。store的物理record容量按NODES+BINDINGS，各类型实际收费由prepare入口显式选择，Bytes按真实record/lease/entry容量计算；同步Budget布局、Billing枚举、host调用及impls，不保留旧BindingState重复Charge。此变更属于Bind正常取消/替换/退出责任闭包。

unlink立即使旧快照失效，仍有pin为Waiting；最后pin Drop复用Lease enqueue+wake，无pin进入Ready。退休每次最多消费式close一个issuer/shared，成功清槽，Err(owner,error)返原record且明确诊断；两槽及相关真实来源责任全清才删除record/退款。显式保留满足关闭失败合同，不宣称已证明合法sender经重试恢复。现NodeStore current固定失败项与Retire立即Runnable必须同时替换：预建lease记录拥有FIFO链接/重试状态，失败项退出current轮转，按有限单调期限重试，不阻其它retire对象、不靠新工作LIFO压住旧项。Retire task声明最近重试deadline并消费timeout，重试政策/单位按既有执行退避约定闭合实现，不在提交中分配。backing Ready/Waiting/Empty包含未链接准备、失活pin与关闭失败Binding；停止只在真正Empty后撤wake来源并等Remove回执。删除Rc<BindingState>快照/能力共存、可外逸Arc和deactivate后隐式Drop路径。

**验证门与自然顺序。** 按管理Role/Funding/Lifetime→内部Binding记录与公平退休→Unbind prepare/commit→唯一Dispatcher宿主接缝→Bind准备/提交→普通Boundary按需衰减→init/test_fal正式消费者→删除旧路径/impls→分层检查推进。host复用实际backing测试管理范围/父变化/同名替代/共同名字/预留失败及费用，不构造Kernel sender fake。core/stress复用两个正式provider，验证普通sender换kind不升权、A/B可信费用/真实管理关闭；共享出口复用/必须Derive/完全Rights上限/输出必需位与非消费suffix、Binding循环有限预算、Replace/Unbind、目标关闭/原父revoke、取消与迟到cap清理、结果未知不重放及最后pin/实际Close退款。保留最小真实边界优先，不为故障窗口新建测试服务。

| G05 | 本批承接标准 |
|---|---|
| G05-21 | 管理接收/Lifetime、Bind/Unbind、两方真实来源、共享/受限出口、替换/撤下/父revoke、取消/迟到/未消费suffix/循环预算全部接通后核销 |
| G05-19 | 补跨Binding领域/完整运输衰减与原付款；Open部分仍由流拥有，不核销整行 |
| G05-22 | 复用实际RPC未领取/迟到回复设施核临时issuer/新grant及Delivery，不复制测试服务；无正式注入时保留明确窗口 |
| G05-03 | 补Binding实际容量及原owner/最后pin/关闭完毕退款；其它资源与会话饱和仍各自保留 |

施工前提交本节给MellowPond审定；目前仅设计文档和只读取证，未改Bind代码。未获审定的关键选择是统一内部记录/Binding独立计费、公平退休轮转及World唯一Dispatcher接线；它们是一份闭合候选，不能先写临时adapter或恢复旧DelegateTask绕过审定。

**审定与施工启动（同日）。** MellowPond已通过本节全部候选，允许施工；统一内部record、FsResource::Binding/BINDINGS及physical NODES+BINDINGS、唯一Dispatcher移World均生效。新增五项交付门：a迁Dispatcher后原样复跑W04/两类async Watch/P3；b实际backing host回归持续Close失败项不挡其它退休且无立即Runnable自旋，Parked等待deadline；c新管理根创建/付款/关闭owner及匿名route peer零引用，普通sender换kind负向；d跨Binding/链接拒绝、迟到准备不删替代者、稳定ID最终复核有host/运行证据，未覆盖窗口原处登记；e停Ingress/新下游→业务责任收束→运输封口/drain，回复丢失未知不重放。按既有超时规则有界撞限翻倍。当前基线保持W04证据，Bind代码施工自此开始；不重跑已经通过且未受新改动影响的检查，不commit/push。

**Bind 基础施工断点（2026-10-10，继续同一批）。** 已实施：FsResource::Binding（COUNT=10）、deployment::BINDINGS 与 physical NODES+BINDINGS；NodeStore 预建 FIFO 就绪/重试链、单调期限、不固定失败 current，失败记录/费用保留并按 libexecution::runtime::CLEANUP_RETRY_NS（原来源注销同一退避政策）轮转；内部 BackingRecord::File/Binding，Binding record独占issuer/shared Option typed owner，MemberKind存稳定ID，Resolved快照仅pin+冻结限制，目录退休/替换unlink，seal最终活动校验由backing完成。reserve_binding在取得能力前安装不可见记录，adopt可收容封口后迟到能力；准备pair/未linked状态在prepare与commit复核，普通公开NodeKind未变。原BindingState Rc/能力Arc/deactivate隐式Drop路径删尽。libfs/src/management.rs已提供独立ManagementTable/Snapshot、真实MintedSender context/related mailbox核验与同Account Funding/Lease收费、准备Publication/撤销/移表；尚未接真实消费者。

唯一Dispatcher已经移到World.rpc（Option<Box<Dispatcher>>），原FileTask::Dispatcher只保留Transport marker的回调/期限/stop状态。boot六处get_task_mut(dispatcher)全部删除；RPC_CALLS=REQUESTS+DOWNSTREAM_CALLS，原W04断言仍用启动并发数2，不把扩大真实容器当扩大探针。Transport停止在实际fs/service Request计数与pending call皆空后才封口/drain，Step::Complete后释放World中唯一值及共享执行存储；boot对最后业务退款后停驻运输发实际wake。核心事务/来源回调保留原路径。

开发证据：libfs32项host（aarch64-apple-darwin）及libfs/libexecution host lint、srv_fs RV64 lint通过，日志 artifacts/bind/host-fs.log、host-lint.log、target-lint.log。新host test persistent_close_failure_is_fair_and_waits_for_deadline用纯Payload错误结果验证同一FIFO队列持续失败不挡其他记录退休、未来deadline前无立即工作且不重复Close尝试，非Kernel fake；invisible_binding_preparation_has_independent_capacity_and_last_pin_refund核独立计数/实际Bytes、容量、稳定ID、最后pin退款。Retire实际task声明deadline并从timeout重新推进，等待阶段Parked。

迁Dispatcher后的core正式开发路线：完整正常reset；FS1 same Dispatcher tenant A/B source charges/refund、FS1 async Watch、SRV2 async WatchName、P3 unknown Replace/queued registration control refund，以及两个provider最终actual owner refund锚点均存在。严格exit1仍首缺G05-20旧Open锚点，没有超时/panic；完整原日志 artifacts/failed-acceptance-20261010-202829-12833.log，启动包装日志 artifacts/bind/virt-dispatch.log，真实exit文件virt-dispatch.exit=1；PID12466及其路线已结束。a条件core证据已补，stress随完整Bind闭包回归。

仍未闭合/下一步：ManagementTable的实际Root/Lifetime/来源、World route receiver与管理Ingress/Outbox/付款、管理A/B双槽bootstrap/init+test_fal交付与删除匿名route peer；backend prepare_binding仍临时使用AccessSnapshot，必须换独立ManagementSnapshot并补本地root范围/父ceiling/名字与稳定ID复核、prepare_unbind/Intent::Unbind；provider Request PendingOutbound/txid/原Funding期限预算、共享准备和Boundary收窄Derive、取消/迟到cap清理/停止顺序；正式消费者及负向/替换/跨链接与Boundary/稳定ID证据。binding_sender仅借record母本，不外逸Arc，但更窄签发分支仍Unsupported，须本批接通。上述均为已审定当前闭包，不延期、不核销G05-21/19/22/03、不进流状态机、不commit/push；后续从这里连续实施，不重建已删除的DelegateTask。

**管理 Root 接线断点（同日，仍继续本批）。** ManagementTable已进入World，与真实route receiver同行；复用原lifecycle::Root明确区分File/Management准备、安装、revoke与Remove，不共用授权索引。boot按primary A/B、secondary实际A铸MintedSender，原tenant Fs/execution准备与Root task/来源付款，来源实际登记后安装并取sender；管理根kind由libfs::route拥有，primary空正文A/B双槽、secondary空正文单A。bootstrap上界更新五报文，Ready等管理根发布；W04 baseline捕捉前也等管理来源就绪。Runtime Task/Source部署上界增加真实管理Root。init接收/核完整transport/保管两组母本并在既有收束处关闭，普通消费尚未移交管理根。init用既有create_mailbox_owner工厂（工厂关闭初始peer）取得管理receiver，删除AcceptedFsProvider.route、LaunchedServices.fs_route_second匿名peer字段、accept函数route参数、STOPPED后旧peer关闭及两个caller参数；保留实际receiver owner交付。

删除rg证明：在srv_init main/filesystem、srv_fs boot、libfs backend搜索 fs_route.peer、fs_route_second.peer、provider.route、fs_route_second: Handle、let _route、get_task_mut(dispatcher)、BindingState、Arc<MailboxSender> 共零引用；实际receiver route字段/管理协议不删除。srv_fs/srv_init/test_fal实际依赖的RV64 target lint通过，artifacts/bind/target-lint.log；git diff --check通过。此次管理根/bootstrap更改尚未运行，不能沿用迁Dispatcher前后已有运行结果宣告它通过。没有后台路线或未领取子任务。

下一编辑点：libfs/provider.rs 的 Domain/Request::accept/advance 与srv_fs Ingress需要独立管理Snapshot与接收来源、同一Request/Outbox计费；backend::prepare_binding须改ManagementSnapshot，补管理本地父范围/完整上限复核、prepare_unbind/Intent::Unbind和稳定ID；Request PendingOutbound经World唯一rpc及实际transport_task id start/wake/take/cancel，Bind共享准备与普通Boundary收窄最终复核；test_fal获得primary A/B及secondaryA管理根并复用两个真实provider场景。完成后仍按全部G05与交付前REVIEW/just check/clippy/必要core-stress门；没有完成固定格式报告，不进流状态机。

**管理接收/本地事务施工断点（同日）。** 原Ingress增加实际receiver选择，文件与管理各一任务，MAX_SOURCES各2；管理设施由H付费，共用同一Request/Outbox与Runtime，TASKS固定项+1、SOURCES固定项+2。Request Domain新增Management，kind仅选解码；管理真实context只从ManagementTable取Snapshot，普通FsRights/普通grant不会通过换kind取得管理授权。原Grant/Management/Watch资金索引按真实context冻结原Funding，已知管理Request/Outbox/Task沿原A/B，Quota拒收沿已有H设施。Header与Prefix、slot0回复权、Bind业务slot1/Unbind零业务槽校验沿真实接收边界，未知op/version仍按协议回应。

backend Bind Intent改ManagementSnapshot，准备保存实际父NodeRef、原版本/ceiling、原名字Binding ID/空位及预建entry/独占名字permit；prepare不再要求共享cap已齐，适于RPC前预留，commit才要求原未linked记录issuer/shared都已齐。管理本地父遍历从真实管理root走唯一成员表，拒绝中途Binding（CrossFilesystem）、链接（TypeMismatch）、空path（Invalid）；最终沿当前弱父链核root范围，普通FsRights不当管理权限。Unbind有独立prepare/Intent，缺失NotFound、本地节点TypeMismatch，最终核原父版本/ceiling与原Binding稳定ID，不按名字重挑实例；摘原成员/unlink/父版本和Watch一次提交完成。当前 Request 已同步接通管理Unbind；Bind与普通更窄Boundary仍Unsupported，尚无实际异步调用，当前闭包未完成。

33项host通过（新增management_parent_rejects_links_and_rechecks_live_root_scope，真实纯backing/独立收费lease fixture，不伪造Kernel角色/能力）：链接与空路径拒绝，所选父目录移出root后拒绝，同名新目录不恢复旧引用作用域、revoke拒绝。libfs全部host targets lint、srv_fs/srv_init/test_fal RV64 lint通过（artifacts/bind/{host-fs,host-lint,target-lint}.log），diff check通过。正式derive消费者新增同一真实普通sender改route kind发送Unbind的Permission断言/FS1日志，编译通过、尚未运行，不标c证据。没有跑新core，遵守协调“闭合后再跑core”。跨Binding、旧Unbind准备不删同名替代Binding及Binding稳定ID出口最终复核仍缺正式运行证据；目录弱父引用的host证明不替代这些门。

下一编辑点仅剩：Request PendingOutbound（txid/用途/原Snapshot或Binding pin/原期限预算）经World唯一rpc与实际transport task id start/wake/take/cancel，先完成管理Bind的真实DeriveDirectory共享准备，再将Boundary需要收窄Unsupported换真实Derive；取消/停止等completion/真实来源、迟到cap清理后才释放pin/请求。随后test_fal获取管理primary A/B/secondaryA并复用两个provider做Bind/Unbind/共享与受限/跨链接与Binding/同名替代/稳定ID/未知回复矩阵。仍未核销G05/进流/整体审查/正式验收；保留本批全部授权，不需重新确认，不建新的运行体或adapter。

### Bind 异步业务与施工审查收束（2026-10-10）

管理 Bind/普通 Boundary 的真实异步责任已接通 [provider/outbound.rs](../../user/libraries/libfs/src/provider/outbound.rs)。每 Request 一项 PendingOutbound，由 World 唯一 Dispatcher 的实际 driver/waiter 推进；原 Funding/绝对期限不变。Bind 在发送前同时固定原父、名字条件/版本/ceiling和成员 permit（较1184节候选的“回复后准备entry”提前，目的为固定原条件，仍不可见/未提交），不可见稳定记录接管 issuer，回复验证后 shared 进入同一 record。普通 Boundary 保存实际父 pin/原 Binding稳定ID/未消费suffix/预编码响应，更窄出口由原 issuer 真实 Derive。返回后只复核原对象与授权，不按名字重挑替代物；原 reply未知结果不重放。临时 service、无效 reply cap和 CallError 持 owner，每步关闭一项能力，失败等待有限 retry deadline；不可提交的新 Binding 随最后 pin 进入原 FIFO 退休。

正式消费者沿原 init/test_fal 运行体：primary 的发布/管理 A/B在原 inbox空正文按有序角色交付，secondary就绪后给同一inbox单A；保留原创建者/付款者/关闭owner，不带账户id，不加服务或 Runtime。场景包括管理真实A/B调用、共享/受限出口/完整Rights/原suffix、目标不具Derive和管理输出越权拒绝、Bind Exists及普通本地TypeMismatch、管理跨Binding/link拒绝、Replace/Unbind、已导出grant不被Unbind召回及循环Follow预算；普通sender换管理kind的Permission负向沿两provider原derive消费者执行。

host35项通过：新增纯管理codec未知op，以及实际backing稳定ID回归；迟到Unbind不删除同名替代、旧Boundary pin原ID失活（不重选）、旧pin费用直到真正释放/退休才退款。纯Binding成员fixture只构造名字/记录关系和真实Charge，没有Kernel sender/能力fake；不证明目标签发/Close行为。证据 artifacts/bind/host-findings.log/.exit=0。修复后 RV64 bins lint artifacts/bind/target-findings.log/.exit=0；just check/clippy分别 check-final、clippy-final .exit=0。开发中的两个lint发现（host非test配置未使用target helper/test module位置）已调整cfg与测试章节；失败日志host-lint-final.log保留，最终七面just clippy通过，不把失败日志当通过证据。

首次闭合core virt-closed1正常reset、rejected/panic=0、非FAL缺失0；Bind最终锚点与两次普通sender换kind负向均实际到达，原W04同Dispatcher A/B、FS/SRV async Watch、P3 queuedcontrol与双provider owner退款齐全。严格exit1首缺仍G05-20，全部旧FAL缺失而非只缺单锚点列于artifacts/bind/virt-closed1-summary.json，原guest artifacts/failed-acceptance-20261010-215840-22990.log。日志文件跨度68.09s仅文件创建至最后写入观测，不是精确guest计时、不作性能或付款归因。

**sub-19原始findings与处置（基线4523bbf，仅源码审查）：**

| finding | 原始可达反例/影响 | 修复与亲读复核 | 证据边界 |
|---|---|---|---|
| F1 P2 | outbound等待直接返回而跳过Outbox.drive；caller回复owner关闭后CLOSED电平来源仍登记，等待下游注销或retry时持续唤醒 | reply已Abandoned且未complete时，先取消/wake下游并驱动原Outbox真实Remove，pending/pin/费用保持；仅原回复注销完成后才回outbound。活reply未编码时仍不发送。每次推进有预算上界 | 正常/queued reply收束复验；精确下游注销故障+cleanup backoff时的窗口仍源码，不报注入通过 |
| F2 P2 | 未知v1管理op=99、空正文合法Header/回复权，被text32先解码成Invalid，违反Unsupported合同 | 先检查version/op再读取已知参数；新host对空/坏正文均Unsupported，正式管理A/B未知op场景接入 | host已通过；新增实际场景随最终core/stress确认 |

其余owner/付款/稳定身份和停止顺序未发现额外可达finding；reviewer未运行QEMU/完整构建。亲读修复而非再开第二首审。追加正式CallOperation queued owner场景：Bind成功回复未Receive后abort/关闭真实队列，提交事实保持且不重放；普通收窄出口cap留在未领取reply队列，实际队列owner关闭，两个provider最终账本归零用来核真实退出。最终 core/stress已在just check/clippy通过后启动，尚待结果；原精确故障窗口不由这些正常 queued场景替代。

删除证明 artifacts/bind/deletion-proof.json：backend/store旧BindingState/Rc+Arc sender owner、init/boot匿名routepeer、srv_fs跨slot get_task_mut(dispatcher)、旧DelegateTask/stalled helper均rg exit1零引用；boundary_export函数Unsupported分支零处。允许Namespace消费者自身Arc sender，未称全仓Arc删除。G05-21/19/22/03须按最终分层证据核销；当前没有整行关闭，不进流直到本批收口，不commit/push。

**最终验证与五项门（Bind当前能力收口，2026-10-10）。** 修复后的virt-final与stress-final均正常reset，rejected/panic=0、非FAL缺失0；strict exit1仍首缺G05-20，全部旧FAL缺失保留，不修改脚本/阈值/删场景。stress完成原16/16竞态矩阵。新增管理A/B未知op Unsupported、Bind queued成功回复未领取abort→事实保留/不重放、受限Boundary cap未领取队列关闭均在final Bind锚点前实际完成；原W04同DispatcherA/B及局部/全局Watch quota、FS/SRV async Watch、P3 queuedcontrol及双provider原owner账本全0齐全。日志文件观测跨度core70.15s/stress138.44s，不是精确guest计时、不作性能归因；没有撞限，无需改默认超时。

原guest：artifacts/failed-acceptance-20261010-221037-27594.log（core）、artifacts/failed-acceptance-20261010-221152-30396.log（stress）；包装/exit文件 artifacts/bind/{virt-final,stress-final}.{log,exit}，结构化全部锚点/缺失/边界在final-summary.json。最终runner PID27110已消散（ps exit1）；路线自身已完成QEMU收割/清理，无本批临时服务。just check/clippy通过后未再改执行语义；只有本节/impls记录回填，最后diff check。

| 审定门 | 最终证据类别与结论 | 保留边界 |
|---|---|---|
| a：迁Dispatcher回归 | **运行core/stress**：原W04同DispatcherA/B退款、FS/SRV async Watch、P3与双provider全0齐全 | 严格旧流/Copy锚点仍缺，不能称整路线通过 |
| b：失败退休公平与停驻 | **host** persistent_close_failure_is_fair_and_waits_for_deadline：真实队列/Charge与纯Payload错误，持续失败项不阻其它项、到期前无Close尝试、恢复后退款；**源码**真实Retire task接最近deadline/timeout | 未伪造Kernel cap；合法typed Close发生故障并恢复的实际窗口未注入 |
| c：管理A/B可信出生/旧peer/负向 | **运行core/stress**管理primaryA/B和secondaryA实际交付/消费/关闭、普通sender换kind两providerPermission、最终全0；**rg**旧匿名peer零引用；**源码**真实issuer/context配对、Lifetime先登记、init失败owner收束 | 交付失败/来源拒绝精确窗口仍源码，不宣称fault注入 |
| d：拒绝与稳定身份 | **运行core/stress**管理跨Binding/link拒绝；**host** late_unbind_cannot_remove_replacement_and_boundary_rechecks_original_id及原管理范围回归，迟到Unbind不删替代、旧Boundary pin不按同名重选、真实费用保留 | 本地事务/IDhost不等于真实IPC晚回复时点注入；令同名replacement到达的精确RPC窗口仍源码 |
| e：未知结果/停止顺序 | **运行core/stress**真实Bind未领取回复关闭后事实保留不重放、收窄cap queued owner关闭、双provider退款；**源码亲读**停Ingress/新下游→业务pending/pin/fees收束→transport seal/drain，F1修复前置原reply真实Remove | 下游注销故障与cleanup deadline轮转精确窗口仍源码，不伪装验证完毕 |

**交付前结构检查：** 主实施者亲读核心收束、源/目标授权、实际context与Funding、提交前后owner、稳定record与名字permit、FIFO退避、Dispatcher来源回执和scope/drop路径；新增unsafe仅独占duplicate handle采用，公开ABI两侧/实际消费者同步。单owner/短borrow，无跨IPC借用与新锁序；提交无分配/Close，已提交责任不依赖caller继续调用；停止、取消和退款保持同一owner。成形结构已清掉旧Rc+Arc并存、跨slot运输假定与更窄出口Unsupported，未增加运行体/测试服务或匿名授权adapter。sub-19两项P2已亲读修复复核，不再以未来独立Review替代本次检查；未提交，因此固定提交独立Review登记随将来实施交付统一处理。

| G05 | 本批核销 | 未核销部分、唯一owner与触发/完成门 |
|---|---|---|
| G05-21 | 管理接收/Lifetime、实际共享与受限出口、双provider与原Funding、Replace/Unbind、完整Rights/suffix/循环预算、queued未知结果/不重放已**运行**；原父管理活动/稳定ID复核有**host+源码**。当前Bind能力实现收口，矩阵行**部分核销** | 精确“下游已发送/原父显式revoke/IPC迟到替代”及来源注销/Close退避故障窗口仅源码；本行RubyRaven持验证owner，在正式注入入口可用或P4组合收口时复用原init/test_fal/公共IPC，核回执前后原owner/pin/Charge与迟到cap释放，不另建服务 |
| G05-19 | 跨Binding FsRights/完整输出交集、真实衰减及普通角色不升权已运行；先前Derive/Watch证据保持，**部分核销** | Open/会话受限出口及其原payer继续由随后流批闭合，行owner仍RubyRaven，正式消费者接通并核控制/映射/来源退休后才整行核销 |
| G05-22 | 正式CallOperation下Bind/受限Boundary成功reply未领取/Close与结果未知、实际cap退出，**部分核销** | 真正迟到下游回复/坏帧隔离与Delivery故障窗口不得由queued成功推定；仍归本行RubyRaven，沿已有librpc/public_ipc正式设施，在P4或稳定注入可用时补实际owner/错误cap/Delivery证据 |
| G05-03 | Binding独立槽、物理NODES+BINDINGS、未链接准备/last pin退款/稳定ID的**host**、core/stress Binding实际owner最终退款；现有Watch双账户quota证据保持，**部分核销** | 实际Binding饱和后恢复及Node/Bytes/Grant/会话组合饱和仍归本行；后续流/Copy正式资源消费者接通后、P4前沿真实付款/owner跑容量边界及重新准入，不以部署常数/最后全0替代单项饱和 |

运行/host/源码三类已区分；本批必需消费者和付款/清理链齐全、应替换旧路径删除、相应检查通过、impls同步，精确注入与整行G05缺口不移到新todo。按MellowPond已授权顺序直接进入流状态机/SetLength，再Copy；不等待重复批准、不提交。本节是Bind当前能力交付及验证边界，不是P1—P4整体完成宣告。

### 流状态机与 SetLength 当前施工设计（2026-10-10）

Bind当前能力已获MellowPond验收，直接按186—200、381、466、470—486施工，不恢复旧DeferredStub/CreatedTarget/Position、不开新运行体。本批承诺Open/CreateOpen/Start/Query/Finish/Cancel、Read冻结末端/实际内容可变、Write accepted/transported、独立业务终态/物理释放，以及SetLength稀疏增长/分批Trimming；Copy保持下批，正式Copy消费者届时与协调者确定。

**所有权/接线。** 新文件StreamTable与GrantTable/ManagementTable同由World持有，表项拥有唯一控制状态、冻结Funding、所选NodeRef/准入Access、Runnel producer或consumer、收费及准备/退休存储。原建立Request在回复后继续作为该会话唯一Runtime推进者，沿既有Watch/Directory的Lifetime任务结构；Request仅留context/来源登记和最近deadline缓存，不复制业务计数或Runnel owner。来源回执保持原Task执行付款；控制Request从真实context取StreamTable原Funding，不接受正文账户/session_id授权。World保管原启动MemoryPool cap，借其handle创建真实Tunnel，不复制全局账户表。Host可测试纯Data/状态，不伪造Kernel sender/映射。

Open在Outbox/会话域费用及退休承载就绪后选择目标；Read固定start/read_end，Write固定start，冻结原token和NodeRef。CreateOpen用现有PreparedNode的预建NodeRef保持准备实例（增加受限访问器），同一准备事务预留名字和空Data；在公开前完成控制Lifetime/数据endpoint来源、回复及Runnel offer准备，再一次提交原名字与会话。不按名字重新选目标，失败返完整准备owner；已提交名字在回复丢失时保留，不自动删除。控制/offer均按完整输出mask检必需位：控制W|WAIT|T及允许D；Runnel invitation依当前Attach→Endpoint映射合同需R|W|WAIT|SIGNAL|MANAGE|T，不默补GRANT/D。

**状态与等待者。** Offered的有限建立期限取原请求期限、宿主offer窗口和有限session期限最早者；Start只有在真实Runnel握手就绪时原子Active，Active重复Start返回现状，Stopping/Terminal Conflict。数据源Offered只观察终态，Start后按真实Remove回执换DATA/终态登记，端点一直保留到来源真实退出。Active每次至多一个消息大小块；producer的Tail确认导出读accepted，consumer实际读出的量记write transported，只有后端commit才记accepted。EOF/取消/撤销/停止进入Stopping，禁止新块；业务不可再提交时冻结Terminal；Completed读流保留producer与终态观察至数据对端关闭或控制/provider显式停止，最终Tail不证明末次read/notify已结束。映射/来源关闭失败仍持owner+有限deadline重试，相关费用实际释放才退。Terminal释放目标/原token，保留原Funding的最小收费控制壳直到最后控制关闭。

Finish在Offered Conflict、Active/Stopping最多一个原Request等待者（额外Busy）、Terminal立即回冻结Info；等待者context存请求taskID/期限，真实replyowner始终在该Request Outbox；等待槽接管后显式完成该请求Delivery入口use，失败仍原owner按有限期限重试，不用已准入use循环保活控制。超时/关闭只清自己的等待槽，不取消会话或延session期限。Cancel先标停止且不争Finish槽，立即回当前Info；取消/停止中外观仍Active或Offered，不能提前报最终计数。Start/Cancel/Finish与后端变化的wake由表项保留pending意图，经已有Retire/原Runtime有界投递，缓冲满不丢。控制最后副本/Lifetime CLOSED触发停止，原建立Request收束真实来源后才移表/释放Task。

**SetLength/Data维护。** Data增PreparedLength和Trimming{first_discarded_block,cursor}，缩短commit一次改可见长度/version并零尾块（最多BLOCK_BYTES）；后续每维护单元删一个超界块，删除后才退其Charge，扩长不分配洞块。Trimming期间新Write/SetLength Busy，过期PreparedWrite因版本失配不能复活数据。NodeStore只提供内部稳定ID有界扫描/借用；MemoryBackend用持续游标扫描实际记录的Data维护，不建数据副本或靠caller后续调用。退休、Trimming、Watch等轮转有工作上界，unlink后实际Data仍只由原record持有，整体退休接管同一blocks。完成Trimming触发既有维护wake，StreamTable扫描/唤醒等待该目标的writer；已经取出的块由原会话保留，不在Busy时丢掉transported前缀。

部署SESSIONS拥有实际控制记录上界，Offer/Stream/Bytes/两项观察费用独立记账，TASKS/SOURCES按真实会话和瞬时请求推导并同步fixed_input/source_input预算；默认Tunnel几何复用已有Runnel多页布局，有限offer窗口是领域部署政策，不改路线时间限制或其它业务deadline。准备容器/slot/cleanup预建，提交不分配。

自然顺序：Data/SetLength准备提交/持续Trimming owner与纯host→StreamTable/Runnel退休+状态/来源→原Request建立与控制/Finish等待→World pool/部署/停止装配→client StreamControl及正式init/test_fal消费者→删除过渡Unsupported/impls→结构检查+host/target/check/clippy/必要core-stress。G05-20须有实际控制/数据输出满足原Open锚点，不改脚本；相关G05逐行记录，不以只有局部检查当能力完成。精确故障窗口沿原唯一矩阵，不复制todo。

**流批当前源码断点（2026-10-10，未交付）。** 已实施 Data::PreparedLength/Trimming、prepare/commit版本门、尾部清零与Busy、MemoryBackend::Intent::Length/三类维护轮转、稳定记录有界扫描及数据变化wake，provider SetLength已接本地实际RPC。新增host length_commit_hides_tail_busy_trimming_refunds_and_sparse_growth_never_revives_bytes，验证旧写准备Conflict、真正删块前不退款、常驻维护独立推进、稀疏增长不复活字节；36host与RV64 lint开发检查通过（artifacts/stream/{host-length1,target-length1}.log/.exit=0）。此后开始流草稿，旧Bind final路线不能当这些新改动的运行证明。

新stream/state.rs由lib.rs的私有stream_state唯一包含（host test/RV64），纯FSM三项独立合同回归：Start门/重复/取消/Terminal冻结、Finish单等待者与旧等待者清理不删替代、offer/session期限独立。39host已通过，artifacts/stream/host-state1.log/.exit=0。Stopping允许既有确定完成量/读端确认在freeze之前更新，禁止新transport；Terminal禁止全部计数变化。

stream.rs已形成Runnel owner/费用/控制与按块推进草稿并加入RV64模块：预建PaidEntry先于Tunnel，Plane分别Producer/Consumer/失败完整MappedEndpoint/Endpoint/Invitation；创建失败保留同一PreparedStream，ManuallyDrop防异常丢弃映射时伪装退款，数据面释放前不删row；State/原Funding/目标pin/临时块/Finish等待者集中唯一entry，原Request将持来源登记/控制Lifetime。当前Target cargo check通过但有11项未接线/未使用警告（target-owner-draft1.log/.exit=0），不称lint通过/会话完成。Table控制Start/Cancel/Finish、有限wake扫描、读确认/写accepted/transported和Terminal后source→映射Close重试代码已写；PreparedStream发布/清理及table移除亦为草稿，尚未接Request/World，尚未结构检查。

下一编辑点：完成PreparedStream Drop/预建entry移交不提前退款的结构复核；原Request PreparedControl::Stream、真实控制Lifetime和data SourceRegistration（MAX_SOURCES=3）接prepare/commit/abort/registered/unregistered/refused/deadline，控制Request真实context资金与Finish等待/退出清槽。World增加StreamTable/原pool cap/部署Config与SESSIONS及真实TASKS/SOURCES推导，Retire同时推进表wake、Trimming完成唤醒等待writer；blocked writer必须临时撤DATA电平/换终态mask防空转，恢复沿实际回执重新登记。Open/CreateOpen最后权限/期限/真实对象及预建名字提交、不重新选择路径；随后正式client/测试Runnel会话/G05-20锚点。当前Open/CreateOpen/控制仍未接到业务，没有新core/stress，不核销G05。sub-20只读取证已领取（是施工前基线，Data/SetLength的“不存在”及serve_fs兜底Unsupported描述已被新代码取代，不当当前事实）；主实施者亲读了Runnel core/poll/close和Kernel Invitation→Endpoint完整Rights继承，没有新内核ABI/外部契约选择。

**流接线与首轮运行（2026-10-11）。** StreamTable已接原Request/World/启动Pool/Retire，Request三来源上界按真实reply/control/data配置，World任务/来源及Offer/Stream/WaitSource额度增加SESSIONS实际占用；控制按真实context取原Funding。provider/streams.rs接Open/CreateOpen预建实例、两来源回执后名字/会话发布，以及Start/Query/Finish/Cancel；最后控制关闭保留终态至Finish等待者取走或退出，原Task保持付款与退款owner。test_fal/stream.rs复用原消费者与两正式provider真实Runnel 16384字节写EOF/read、重复/未握手/终态控制、冻结末端且内容可变、取消等待Finish、未附着到期后在首次Query前只以Kernel端点观察物理退休。client已有open/create_open/stream_control直接接入，没有新运行体。
39host、RV64 lint与just check通过；完整just clippy七分面通过（artifacts/stream/{host-wire,target-wire5,check-wire,clippy-wire}.log/.exit=0）。首core正常reset、两provider账本全0，新Open/冻结read/无人推进offer到期锚点均各两次；strict exit1首缺已变为旧FAL consumer shutdown，旧Copy等全部缺口仍需按G05记录，不改验收器。原guest artifacts/failed-acceptance-20261011-004817-46325.log，包装virt-wire1.log/.exit=1；runner45886已结束。此运行不能覆盖随后findings修复。

**sub-21首次施工审查（源码，无注入）**：F1 P1：无限Finish Request的Delivery持控制LifetimeUse，最后控制关闭→等Terminal→Delivery不释放形成寿命循环；F2 P1：producer满环分支忽略poll清DATA后复查的Writable，可能丢唯一wake；F3 P1：EofConsumed只证明Tail到Head，读者仍可能在末次notify之前，立即Close使正常read报Closed；F4 P2：控制CLOSED来源在数据Close退避期间仍登记，且反复stop重新标wake，产生电平/Retire自旋。原报告保留于sub-21 run记录；不能以已跑正常core推翻交错。

当前修复已写、尚待重新运行：libkernel Delivery.finish与librpc RequestContext.finish_delivery显式结束已移交操作的入口use并保留回复权；Finish等待槽建立后结束Delivery，失败原owner仍保留按有限deadline重试。满环poll返回Some则Runnable。read Completed先冻结业务/释放目标，但保留Producer与终态来源直到peer CLOSED或显式provider/控制停止；不把Tail当末次调用完成。控制CLOSED先真实移出来源、仍保管Lifetime/表/费用；table.stop首次转换或首次物理退休意图才wake，不重复唤醒退避。正式消费者新增无限session/无限Finish，以原公共Prefix/Packet/SendOnce发送后关闭最后控制，保留数据端且不EOF；只等数据端真实PEER_CLOSED，再取真实回复，捕获F1循环。Runnel新增wait_closed_until仅观察真实端点终态，不发DATA、不另造Runtime。修复后RV64 lint通过（target-findings1.log/.exit=0）。
修复后完整clippy七分面exit0。findings首core夹具缺DUPLICATE，send_once RightsDenied panic，原guest failed-acceptance-20261011-011629-50696.log；该次exit124（panic后未reset）不算寿命回归，未放宽路线预算。补齐原reply sender派生权后virt-findings2正常reset、无panic、两provider实际账本全0（原guest failed-acceptance-20261011-012012-52037.log），无限Finish最后控制回归通过。其strict exit1仍为旧FAL consumer shutdown锚点缺失。
再补正式SetLength消费者：多块文件缩到8，立即ReadAt只见8字节；重试合法Busy直到增长到32，断言新范围全零不复活截断数据。RV64 lint exit0；core与stress均正常reset、无panic，四个流/SetLength锚点各两次、两provider原收费退款检查各一次、stress race16/16。strict exit均1，完整缺失的21条旧FAL锚点保存于artifacts/stream/length2-summary.json，不替换旧文字或缩验收器。原guest分别failed-acceptance-20261011-013126-55155.log、failed-acceptance-20261011-013246-57905.log；包装target-length2/virt-length2/stress-length2.log/.exit。runner54785与之前50219/51714已结束，无残留QEMU。
流/SetLength当前代码与fs impls已回填；新Delivery知识同步rpc/ipc impls、EOF保留/Delivery结束契约回写1299/1301。F1—F4处置已做：真实RPC与数据退休捕获F1；F2满环返回可写后Runnable；F3保留Producer而不以Tail提前关；F4控制源先真实退出且首次stop才标wake。资源Close故障未注入，交错窗口的结构证据与现有正常/压力运行须分别看待。
当前接力仍属第四批施工，未归档/未commit/push；下一步补流在原A/B Funding/原绑定下的权限与QuotaExceeded组合、CreateOpen提交后回复弃置的领域证据，并按已有REVIEW核对最终结构与F1—F4。新source/data/目标责任已接通，Copy仍未接，本专题和严格core/stress整体没有通过。后续先继续本记录，不重跑已通过检查，除非新增实现/具体风险；旧缺口按G05迁移原消费者逐一闭合。

### 流补证收尾与 Copy 固定运行方向（2026-10-11）

MellowPond接受前轮流/SetLength与F1—F4修复，并授权直接补完流的A/B付款、权限、Quota及CreateOpen回复弃置；21条旧锚点逐条映射G05，仍有效的流合同由真实消费者输出，删除的能力属性/affine/条件建立不伪造通过、不恢复API、不改脚本。每项报告三类覆盖、删除清单和G05核销表，Copy项留到下一项。

Copy的正式消费者固定在init现有InitTask族，复用其Dispatcher/Runtime/Assembly。libfs客户端层拥有可复用Copy任务、业务结果与清理；init只组合任务和实例化场景，test_fal不建立Runtime。同一原准入任务从准备、搬运到实际来源/映射/回复释放始终持owner，最后才Complete；失败或取消先停止搬运，结果可先可见，cleanup_pending只诊断，不把清理唯一责任交回调用者。

实现将以原OAL walk状态共用算法提供持Arc<Namespace>的拥有型接续，跨advance保存既有调用剩预算与期限；不复制路径算法，不建另一套树。原Dispatcher每次至多一项在途RPC并唤醒原Copy任务；source/destination Runnel各一项实际观察，实际注销回执前不关映射。任务预建一消息大小搬运块/回复清理存储，按预算至多搬一个块；运输/本地读出/目标确认及两端终态分别计数。

Stream顺序为源Open/附着/Start→目标CreateOpen/附着/Start→按块泵→发布EOF→两端Finish→实际退休；目标CreateOpen之后只沿所得具体会话，不重新按名选择或回删。Property为Get完整纯值→原子Create Property，不制造流计数或会话。CreateOpen/Create发送后的未知结果不重放；可信领域错误、未发送失败和已发送未知分别记录。取消/失联保留空或部分目标，最后确认accepted仅为下界；两端Completed且目标接受量等于本地搬运量才报告Stream Complete。目标替换、取消后前缀、Finish/CreateOpen回复未知、provider停止及旧midflight问题沿G05-10/14—18和原故障记录核对。

### 流补证收口报告（2026-10-11）

**三类覆盖。** 实际运行：core/stress均正常reset，无panic；两provider各执行只读grant/ceiling/真实控制角色与完整Invitation运输上限、超过A局部会话容量的CreateOpen入队回复弃置（每端9次）、提交名字仍空且重新Open可用、offset=3/length=6定界读与EOF/两端计数、pre-Start发布EOF仍Offered再Start/Finish；同primary A/B实际Bytes Quota、A局部Stream/Offer quota、B共享quota、释放A后B真实再准入、拒绝CreateOpen名字不可见。原primary/secondary最终收费账本核对、W04/Watch/Bind/N3/P3回归及stress race16/16保留。纯host：39项原Data/FSM的独立合同证据复用；这次没有伪Kernel、追加镜像FSM或测试Runtime。仅源码/未注入：Close/来源注册拒绝/最后读notify交错精确窗口，既有结构证明不升格为注入通过。

**验证。** clippy-evidence3七分面0，target-evidence-final0；core/stress包装virt-evidence3/stress-evidence3 exit1，原guest failed-acceptance-20261011-115424-88264.log / failed-acceptance-20261011-115601-91702.log。严格首缺Create uncertainty，14条旧锚点仍缺；21→14的逐条映射在下表及artifacts/stream/evidence3-summary.json。evidence1夹具只给普通sender输出位，受限Open正确Permission后virt实际触及180s；已按撞限政策将Justfile唯一VIRT_TIMEOUT翻倍到360。evidence2修夹具后reset，evidence3补弃置重复/定界/EOF后core+stress均reset。全部runner82670/84229/87837结束，QEMU/cpulimit为空，不重跑。

**删除清单。** provider重复的第二次life.closed→table.stop已删除，首处锁存/移出真实来源继续拥有停止；没有兼容API/假输出/新Runtime。旧Copy/CreatedTarget/条件Open等此前删除保持；本轮不删除验收脚本锚点。

**交付前结构检查（REVIEW）。** F1：等待槽接管后finish_delivery只关闭入口use，reply-once/付款/有限重试仍归原Outbox，请求退出清自己的槽；无限Finish关闭最后控制的正式回归保留。F2：满环poll复查Some必Runnable，不会清掉唯一DATA后停驻。F3：Tail只能冻结Completed计数，Producer与终态来源保留到真实peer CLOSED/强制退休，目标pin释放不产生后端新提交。F4：控制CLOSED真实注销先于数据清理、stop只有首次转换/首退休意图才标wake，cleanup_at期限与真实回执恢复推进。Table/预建Entry各有唯一owner；终态不写计数、来源实际退出前不Close映射、失败返原owner并保费；Trimming只沿原Data版本与删块退款；内核状态机/新锁/ABI本轮不变。检查未发现新可达findings；精确Close故障注入仍未覆盖，保留原条目。

**G05核销表。** G05-06/07/08/09/11/13/20的当前流合同运行门已满足；G05-10仅CreateOpen提交后弃置/保留名字部分，Unknown与回复隔离留Copy；G05-03新增Bytes/会话/Offer额度与重新准入，Node/Grant完整组合仍未整行核销；G05-19流角色/权限/完整运输上限门满足，原Derive/Watch证据各归原记录。G05-14—18仍进入已定Copy任务；G05-01/12被废止机制不恢复、不发成功锚点，协调者P4核销。

| # | 原缺失锚点 | G05 | 目标能力 | 本轮状态 |
|---|---|---|---|---|
| 1 | independent consumer shutdown passed | G05-18 | 停止观察 | 本轮真实消费者已满足 |
| 2 | independent Create uncertainty and reply isolation passed | G05-10 | CreateOpen未知/回复隔离 | Copy本批接续 |
| 3 | independent property copy passed | G05-14 | Property Copy | Copy本批接续 |
| 4 | independent cross-provider Move rejection passed | G05-04 | 跨backing Rename拒绝 | 已有领域行为，旧锚点尚缺 |
| 5 | independent Delegate and restricted Open passed | G05-19 | 受限入口与Open运输上限 | 本轮真实消费者已满足 |
| 6 | independent Watch authorization passed | G05-02/G05-19 | Watch真实控制授权 | 已有领域行为，旧锚点尚缺 |
| 7 | independent repeatable handle read passed | G05-01 | 能力Property已删除 | 废止，P4裁决；未输出假成功 |
| 8 | independent affine Take recovery passed | G05-01 | affine Take已删除 | 废止，P4裁决；未输出假成功 |
| 9 | independent Create Modify and Delete Watch passed | G05-02 | Create/Modify/Delete Watch | 已有领域行为，旧锚点尚缺 |
| 10 | resource quota saturation and recovery passed | G05-03 | Bytes/会话与Watch饱和退款 | 本轮真实消费者已满足 |
| 11 | in-flight Copy provider shutdown passed | G05-18 | Copy在途provider停止 | Copy本批接续 |
| 12 | independent enumeration and same-provider Move passed | G05-04 | 分页与同域Rename | 已有领域行为，旧锚点尚缺 |
| 13 | independent property Copy and stream ReadAt passed | G05-05/G05-14 | 纯值Copy与ReadAt | Copy本批接续 |
| 14 | independent Open offer discarded | G05-07 | Offer双owner弃置/退款 | 本轮真实消费者已满足 |
| 15 | independent pre-Start EOF gate passed | G05-08 | Start前EOF门 | 本轮真实消费者已满足 |
| 16 | independent primary and secondary bounded Open read passed | G05-09 | 双provider冻结范围读 | 本轮真实消费者已满足 |
| 17 | independent conditional Open identity and pin passed | G05-12 | 条件Open未承诺 | 废止，P4裁决；未输出假成功 |
| 18 | independent Open reply authority retired after abandonment | G05-13 | 已入队回复业务槽弃置/退休 | 本轮真实消费者已满足 |
| 19 | independent cross-provider stream Copy passed | G05-14 | 跨provider Stream Copy | Copy本批接续 |
| 20 | independent Copy conflict, empty and cancellation variants passed | G05-16/G05-17 | Copy冲突/空/前缀取消 | Copy本批接续 |
| 21 | independent exclusive target Create and pinned cleanup passed | G05-15 | 独占CreateOpen/具体实例不回删 | Copy本批接续 |

下一步已授权直接进入上述Copy固定运行方向（1332—1336）；流本批当前能力/结构检查收口不代表P1—P4总体完成。严格剩余14=Copy7、已实现但旧文本未满足的Watch/Rename4、废止3。继续原计划、不commit/push。

### Copy 前置责任链接入（2026-10-11，施工中）

Copy 主体尚未实施；流/G05 映射收口后，先闭合固定 Namespace 与原 Dispatcher 的 owner 接缝。`Walk::owned(Arc<Namespace>)` 与借用入口共用同一个私有 State/路径算法，持固定 Namespace、调用剩预算和期限，不引入第二棵树或运行体。

亲读发现合法 Namespace sender 可无 DUPLICATE，而原 Dispatcher 消费 Capability；不得靠复制提升 Copy 的入口要求，也不得从借用 handle 伪造 owner。已将 RPC 调用入口、PendingCall、StartFailure/Completion 统一为 `ServiceOwner::{Owned(MailboxSender), Shared(Arc<MailboxSender>)}`；准入核 WRITE|WAIT，失败/实际来源退休后的完成原样返还变体。Watch 删除重新 Query/adopt 及其 orphan 槽，派生/启动调用同步采用 typed owner。Shared 尚无实际消费者，完成 Copy 后须以真实无 DUPLICATE sender 验证。

原 `retire_abandoned` 在 Dispatcher sealed 后无条件 `take`，会抢走活 waiter 的终结结果，使业务任务永久等待。已限定停止退休扫描只处理无 waiter 项，活任务继续持领取责任；失败 wake 回执撤销 waiter。正常停机仍在业务清理及 pending 清零后停止 Dispatcher；内部故障/活 waiter 精确交错尚未注入，不宣告通过。

开发检查：host-owned-walk exit0；target-owned-walk/target-service-owner1 exit0；check-service-owner exit0，clippy-service-owner 七分面 exit0。日志在 artifacts/copy。core/stress 均正常 reset、无 panic；严格包装 virt-service-owner/stress-service-owner 均 exit1，首缺仍为 Create uncertainty。原 guest 日志 artifacts/failed-acceptance-20261011-121820-99067.log / failed-acceptance-20261011-121959-2884.log；汇总 artifacts/copy/service-owner-summary.json。runner98639、QEMU、cpulimit 均结束。本轮没有恢复旧 Copy、CreatedTarget/conditional API，没有新运行体或假锚点；G05 仍剩原 14 条，未核销 Copy 能力。

当前下一步仍为 CopyTask 主体：沿 1332—1336 接通 Stream/Property、两端 Runnel、未知结果与同一任务清理，再在 init 原族实例化；验证无 DUPLICATE 共享调用、取消隔离及 Dispatcher 内部故障结果领取。不得把本节铺路接口视为 Copy 系统能力交付。

Shared 铺路的 owner 为 librpc；用途是在不派生 handle 的情况下持有已签发入口，目标消费者为 CopyTask 的无 DUPLICATE Namespace sender。接通条件是本批 CopyTask 通过原 Dispatcher 提交 Shared 并由正式 init 场景核实际调用、取消隔离和来源退款；保留期限到 Copy 接通收口，届时仍没有真实消费者就删除 Shared 入口/变体，不永久留下未使用路径。完成后以真实消费者接通证据关闭本条。

`retire_abandoned` 回归捕获的独立合同是“sealed 不能撤销仍活 waiter 对自身 txid 的领取责任”。Dispatcher/内核 IPC 均仅 RV64 编译，host 无法直接覆盖真实 FIFO/来源退休；不另造镜像退休算法或假内核。该回归由 init 原 Copy 任务提交真实 Shared 调用后停止同一 Dispatcher、继续推进原 waiter、核失败结果及清理至 Complete；验证后在本记录登记实际证据。实施者 mesh 实名已用 mesh_get_profile 核实为 RubyRaven，旧报告 GrandOwl 署名错误已向协调者纠正；设计 owner 的 GrandOwl 身份不变。

### Copy 主体施工与第一轮走查（2026-10-11）

`libfs::client::copy::CopyTask` 与 init 原 InitTask/ReadDuty 已接通；ReadDuty 准入多个独立客户任务与一个共享 Dispatcher，预算按任务数/来源数准备，取消和清理仍逐任务持 owner。不在 test_fal 建运行体。Stream/Property、Created/Unknown/明确失败、部分进度、两端控制、附着失败 owner、回复槽和实际注销/关闭重试已实施；业务已确认完成后不因取消/关闭重试改写结果，成功只在实际清理结束后对外可见。

sub-25 首次施工结构审查发现三项，原发现与处置由本节统一承载：C1（P1）Copy 等待辅助忽略 duty.error 可在失败后空转；已沿原 await_read 规则返还错误/提前结束，由原根监督收束。C2（P2）Runnel poll 在 ready 时不清 DATA，“同时 poll”不能消除无关电平；已改为只有当前搬运方向登记 DATA，另一端保留终态观察，切换经真实注销/安装回执，再重查共享条件，EOF 后两端只观察终态。C3（P2）Unknown 后用另一 Caller 不能证明共享回复隔离；已改为原 Duty/Dispatcher 的两个 Copy 任务，旧创建 Sent 后由真实 provider 查询确认提交，再取消旧调用、推进原 Dispatcher 的后继 Copy，核实际忽略迟到回复与后继完成。各项目标检查/运行证据待下一轮回填，未以修改等同复核通过。

第一轮开发 core（virt-task1）已输出真实 Property、跨 provider Stream、ReadAt 内容通过，随后停在并发取消，原 guest artifacts/failed-acceptance-20261011-125911-10861.log，包装实际 exit124/360s。独立消费者在根 Copy 编排期间还套用单次 RPC 的 5s，触发 command wait expired；该等待改由根命令/进程退出持责，不借用文件 RPC 期限。按实际撞限政策 Justfile 唯一 VIRT_TIMEOUT 360→720；C1/C2 修复用于排除停滞，翻倍不冒充修复或通过。init-target3/4 为开发目标 lint exit0；本轮日志 artifacts/copy，旧中间输出不构成 Copy 收口。

协调者复核上述期限理由后更正：virt-task1 没有 Copy 内部状态现场，不能只由“撞限”推导有界进展，也不能把 C2 源码路径直接认定为唯一根因。720 暂保留为停驻调查的观察窗口，不计为任务完成门或已确认有界推进的预算；后续停驻不再翻倍。第二轮 virt-task2 的实际状态日志显示取消项已 Complete、另一项 accepted/local/transported=23040 且两端 Completed，原并发停驻点不再存在；这是 C1/C2 及等待修改后的组合证据，不倒推单变量根因。该轮还跑过空/冲突/改名 pin、sealed 活 waiter，随后因夹具把“CreateOpen 已 Sent”误当“名字已插入”，首个同步查询 NotFound 导致 init 失败；原 guest artifacts/failed-acceptance-20261011-131320-14785.log，最终 exit124。创建观察已改用同一 FollowLimits 的调用额度/绝对期限有界核对，禁止重放创建。

第二轮失败接管还暴露编排暂停不能留到 Stopping：原 hold 依赖业务期限重唤醒，而 Stopping 不再武装该期限。InitTask::stop 现先撤销 Copy hold，再交原任务停止，避免保留活 waiter 却阻断其领取。修复及失败收束证据待补，不能把持久根监督的存在写成已经 reset。

命令等待的无限期修改已撤回。Notification 的最后 Signaler 关闭不会关闭仍由消费者 Owner 持有的 core，不能承诺 init 关闭 signaler 会令消费者必收 Closed。共同编排真值现归 `test_fal::command::PHASE_TIMEOUT_MILLIS`，数值承接原 init `await_report`/`finish` 的120s阶段政策，两个根等待及消费者根命令等待同步引用；单次文件 RPC 的5s不参与该推导。消费者在通道 Closed 或共同阶段期限到达时退出，根 ProcessControl DONE/原失败监督拥有收集责任；不只依赖 QEMU 路线超时。

### Copy 本批收口报告（2026-10-11，RubyRaven 实施）

**当前事实。** CopyTask 已由 init 原 InitTask/ReadDuty/Runtime/Assembly 实例化，Stream 与 Property 均使用真实 provider；Shared 已有无 DUPLICATE Namespace 的实际消费者，1388 的铺路接通条目关闭。每个 Copy 保管原回复、控制、两端映射和来源至实际清理结束，Failed 的 cleanup_pending 不转移责任，最后才 Complete。没有 test_fal Runtime、路径回删、重放创建或新增测试服务。本批能力收口不代表 P1—P4 或严格路线完成；后续入口为协调者的 P4 锚点处置、改名与聚合验收。

**三类覆盖。**

- 实际运行：最终 core/stress/release 均正常 reset，panic/rejected 为 0，非 FAL 必需锚点缺失 0；Property 纯值、跨 provider 23040 字节 Stream/ReadAt、空目标、冲突保留原内容、未提交取消、非零 transported 后取消与有界目标前缀、另一个同 Dispatcher Copy 完成、目标改名后沿具体会话写入且同名替代者不变、在途 provider 停止都实际执行。两端 Completed/Ok 的成功 Copy 核 local_read=transported=destination_confirmed；取消/停止的未确认 accepted 保留零下界，不伪造最终数。Unknown Property/Stream 各由同 Duty 的两个 Copy 验证：旧创建 Sent 后取消、实际迟到回复计数增加、原 Dispatcher 的后继完成。无 DUPLICATE sender、sealed 后活 waiter 原完成仍可领取、hold 期间原业务期限唤醒并清理至 Complete 均有独立运行断言。原 W04 A/B 来源及局部/全局额度、fs/service async Watch、P3 Replace/迟到控制、Bind/Unbind、流/SetLength及两 provider 最终退款保持；stress race 16/16。
- 纯 host：前置 `Walk::owned` 复用 OAL 状态的 27 项测试通过（host-owned-walk.log）。未为 RV64 Dispatcher/Copy IPC 造 host 镜像状态机；此前独立流 FSM/数据和资金测试的证据归原批，不重计为 Copy 运行。
- 仅源码/未注入：来源准入拒绝、实际注销/Close 持续失败、now_ns 故障及根全局失败接管时的精确 hold→Stopping 窗口未单独注入。代码保留原 owner/Charge、注销回执先于映射关闭、失败 owner 原槽重试，Task::stop 先撤 hold；正常业务期限的 hold 回归不能冒充上述全局失败交错证据。取消门已证明非零运输和前缀内容/上界，尚未单独要求取消时目标非零 accepted 的精确窗口。owner 为实施者，P4 在既有故障设施可控注入时复核；以实际回执、无提前退款和最终清理为完成标准，不为这些窗口建旁路消费者。

**最终验证与期限。**

| 路线 | 包装退出码 | 含构建耗时 | reset / rejected | 原 guest |
|---|---|---|---|---|
| just virt | 1 | 123.19s | 正常 / 0 | artifacts/failed-acceptance-20261011-133231-23591.log |
| just virt-stress | 1 | 187.41s | 正常 / 0，race 16/16 | artifacts/failed-acceptance-20261011-133434-27794.log |
| just virt-release | 1 | 31.01s | 正常 / 0 | artifacts/failed-acceptance-20261011-133745-34051.log |

耗时来自包装 monotonic 计时，包含构建，不能用作纯 guest 性能数据。check-final、clippy-final 七分面均 exit0；init-target6 为最后源码版本 RV64 lint exit0。最终 diff --check 通过。各 .log/.exit/.elapsed 和严格 required/rejected/原回归核对保存于 artifacts/copy；汇总 final-evidence.json，删除证明 deletion-final.json。runner23114 已结束，QEMU/cpulimit 无残留。

720 只用于失败现场的调查窗口，不是已经证明有界推进的路线默认值。最终 core 在原360以内 reset，Justfile 的唯一 VIRT_TIMEOUT 已恢复360，没有删场景或改验收器。不能由本次组合修复后的正常运行倒推 virt-task1 停驻的唯一根因，也不将性能差异归因于某个修复。

命令协议的唯一元数据 owner 为 test_fal::command；init 单向依赖该元数据。PHASE_TIMEOUT_MILLIS 只定义一次，根报告/退出等待和消费者根命令等待共用，未复制120000；到期消费者退出，DONE/持久根监督收集。没有无限命令等待。

CreateOpen Sent 夹具的最终版本只同步观察一次，允许可见或 NotFound；core/release 的 Property 可见、Stream 不可见均合法，不能把 Sent 当作已提交。任务尚未领取可信结果时保持 Unknown；未知目标留给原 provider backing/停止 owner，不按名字删除或重放。1400 的中间“有界等名字可见”夹具已被本实现取代。

**findings 复核与交付前结构检查（REVIEW）。** C1：所有 Copy await 辅助显式检查 duty.error、world.failed、提前结束并返回原监督；没有吞失败空转。C2：DATA 只在当前搬运方向登记，换向真实注销/重装，另一端及 EOF 后只留终态；并发取消后的另一个 23040 字节 Copy 实际 Complete。C3：Unknown 场景明确共用同一 Duty/Dispatcher，取消旧 txid 后实际 ignored_reply_count 增加，再恢复原后继任务并核内容；另一个独立 Caller 不充当隔离证据。三项处置按原反例亲自复核，最终三路线运行证据齐全。

sealed 活 waiter 修复由真实 Shared 调用提交后停止同 Dispatcher、确认完成仍在、恢复 waiter 领取 Shutdown 并清理捕获；hold 的正常有限业务期限回归明确通过。停止先撤 hold、业务清理完再封运输；故障取消只终结自身 txid。私有 phase 确定 expect/assert 的角色/会话前提，外部错协议与系统错误走 Failed；本批没有新 unsafe、锁或内核 ABI。单个 advance 只推进一项 RPC、一个有界块或一项 owner 清理，源码中不嵌套泵 Runtime。Shared/Owned 和两项 SourceRegistration 各有实际独立责任，不以取消标志替代退休回执；无需另造 dispatcher 或重复路径状态。未发现本批新的可达结构 finding，故障窗口仍按上文分级。

**删除清单。** rg 对 user 的 CreatedTarget/CopySpec/CopyRuntime/delete_if_current、init 旧 client_task 单值、两 Watch 客户端 orphan 槽、test_fal Runtime 均零引用；旧 client/copy/pump.rs 不存在。CopyTask 没有 Request::Remove/Unbind 或按目标路径 remove（来源的 SourceRegistration::remove 是实际注销，不算路径回删）。Watch 的重复 Query/adopt owner 路径已删除，Shared 现为实际 Copy 消费；tools/qemu-acceptance.sh 对 HEAD 无 diff。证明带命令/退出码/范围，不把普通领域 Expected 标识误计为旧 Copy 残留。

**G05 核销表。**

| ID | 本批实际证据 | 本批结论 / 保留边界 |
|---|---|---|
| G05-10 | Property/CreateOpen Sent 取消、Unknown、同 Dispatcher 迟到回复与后继内容；原已提交回复弃置证据保留 | 当前 Unknown/隔离门满足；Sent 自身不证明提交，精确源拒绝/Close 故障未注入 |
| G05-14 | Property u64 原子创建、跨 provider Stream 23040 字节与完整内容/两端终态、ReadAt | 本批两个分支运行门满足；所有 Property 类型组合归 G05-05，不整行扩大核销 |
| G05-15 | CreateOpen 独占建立、改名具体目标及同名替代者保留 | pin/不按名回删门满足；Unknown 不提供名称补偿 |
| G05-16 | Exists 原目标内容、空目标、提交前取消、部分取消 | 当前变体门满足 |
| G05-17 | 非零 transported 后取消、部分进度/目标前缀和上界、原任务清理 Complete、并发 B 完成 | 当前运输取消门满足；目标非零 accepted 精确窗口仍未注入，不报未确认最终计数 |
| G05-18 | 在途 secondary 停止、Created/部分运输/清理 Complete、双 provider 退款与收集 | 当前停止门满足；旧间歇故障原计划的连续40 release/聚合验收门保留，不整行关闭 |
| G05-22 | 旧回复实际弃置、同 Dispatcher 后继正常；sealed 活 waiter 保持领取 | 新增 Copy 隔离门满足；原 Bind/P3/故障时点边界保持 |

严格旧锚点由14减少到7，以下7条仍在每条最终路线中缺失；Copy对应的7条全部有真实断言后输出。首缺已变为 cross-provider Move，严格 exit1 不报通过。

| 剩余锚点（FAL2 前缀省略） | G05 | P4 处置 |
|---|---|---|
| independent cross-provider Move rejection passed | 04 | 对齐现有跨 backing Rename 的正式消费者证据与文本 |
| independent Watch authorization passed | 02/19 | 对齐真实独立控制/运输权限场景 |
| independent repeatable handle read passed | 01 | 已废止 handle Property，协调者按最终合同核销，不恢复/伪造 |
| independent affine Take recovery passed | 01 | 已废止 affine，同上 |
| independent Create Modify and Delete Watch passed | 02 | 对齐既有 Watch 事件真实断言 |
| independent enumeration and same-provider Move passed | 04 | 对齐分页/同域 Rename；Replace/防环未覆盖项仍保留 |
| independent conditional Open identity and pin passed | 12 | 未承诺条件Open，协调者按最终合同核销 |

旧 midflight 的处置写回其唯一[原记录](todo-2026-09-29-fal-midflight-copy-flake.md#2026-10-11-新-copy-消费者承接)。本次 release 一次正常不证明历史根因已修；40次重复与完整 acceptance 是 P4 单独的验证门。没有 commit/push，工作树保留共享任务改动。

### P4 开工与身份迁移（2026-10-11，RubyRaven）

协调者接受 Copy 收口后授权 P4；验收器剩7条的替换/删除须先逐条只读报告，由协调者报用户确认。其余身份迁移、删除审计、当前 impls 回填可以并行，不 commit/push。

**锚点只读建议已交协调者，待用户裁决。** cross-provider Move 由真实双目录 Rename/CrossFilesystem/保留源值承接，建议改为现有 `FS1 independent DeriveDirectory and Rename passed:`；Watch authorization 与 Create/Modify/Delete 合并为现有 `FS1 independent Watch contract passed:`，真实场景含权限/Observer角色/完整运输位、Create/Modify/Unbound/TargetRetired及旧Ack，不沿用旧Delete事件假设。enumeration/same-provider Move 建议由现有 `FS1 independent direct paths passed:` 的两页cursor，加前项Rename标记共同承担；required 不重复加入相同标记。repeatable handle、affine Take、conditional Open 三项建议按废止/未承诺合同删除，不恢复机制或造新成功输出。原7条的精确文本与G05映射保留在上一收口表；本节没有修改 tools/qemu-acceptance.sh。

**身份迁移。** 目录 `user/tests/test_fs`、package、默认 library/binary target、crate import、workspace member、init 直接 path 依赖、启动映像字段/拓扑身份/`bin/test_fs` 路径和 Justfile initfs均同批改名。FsConsumerCoordinator/launch_fs_consumer 与 REQUIRED_FS_CONSUMER 使用领域名，没有名称 alias。命令协议的唯一元数据 owner 随 package 改名为 test_fs::command，init仍单向使用。历史日志、archived及工作记录当时消费者名字不批量改写。

Cargo metadata 的实际 lib/bin均名test_fs，无test_fal/libfal package；libservice不依赖libfs，选定领域/消费者依赖无rename alias。当前 initfs包含bin/test_fs，不含bin/test_fal。Rust/Cargo/Justfile/tools 中旧包、旧组件、旧固定NodeInfo/NodeSummary、DirectoryGrant、ServiceRecord/ExportPolicy、expected_identity、CreatedTarget/CopySpec/CopyRuntime、PublishReady/QueryName 零引用；普通文档里“旧机制已删除”的说明不伪装成引用残留。完整命令、退出码、实际图及文件存在性由 artifacts/p4/deletion-and-graph.json 拥有，metadata完整原件同目录。当前notes/impls与libraries/README、notes索引本地文件链接全部可定位（current-doc-links.json）；失效的主计划设计正文旧源码链接交协调者修订，历史证据不改。

**当前说明回填。** startup 删除两阶段PublishReady/ServiceRecord/匿名route peer及旧Delegate访问B的当前叙述，按实际bootstrap、独立管理入口、publisher handoff和WatchName→GetEntry描述。RPC删除libfal ClientOperation的当前包装层叙述，直接对应CallOperation/Dispatcher及Copy/Watch；原fal导读只链接真实领域owner。libraries说明Copy由libfs客户端层持会话/清理、使用侧init组合运行，不将Runtime搬入领域库；notes索引涵盖流与Copy。

**开发证据与待收束。** rename-clippy七分面exit0；rename-virt正常reset、无panic/rejected，strict exit1仍首缺原cross-provider Move，完整guest artifacts/failed-acceptance-20261011-135212-36487.log，汇总rename-summary.json。这是身份迁移后的运行，不是严格路线通过；guest未打印进程路径，启动名证明来自实际Cargo/ELF/initfs与init唯一映像选择，不把“日志未出现test_fs字样”当作进程未启动。

为补G05-05/04，test_fs原消费者增加六种标量/类型Range边界，以及Rename Exists保持两边/成功Replace/后代防环的正式断言；不新建运行体。evidence-clippy七面0；evidence-virt实际panic于新增夹具derive.rs:319：复用了前段RPC编码长度used=76切20字节属性buf。纯值两provider已跑到，但该轮不提供Rename新门或reset证据；完整guest artifacts/failed-acceptance-20261011-135523-40926.log，exit124。这是实施者引入的夹具错误，不是有界推进撞限，不调360默认期限。incoming Property现独立buffer/encoded长度，修后证据由evidence2日志接续；不得把旧失败抹成通过。

历史记录：当时 P4 尚未完成，等待剩余锚点裁决、领域补证、删除审计、连续 release、结构检查和完整 acceptance；后续工作已在本篇末节完成并由 `a1ca124` 交付。未闭合的 N6、映射撤销/Bind 精确窗口及 W05-04 仍按当前接力保留，不把它们倒填为历史中间态已通过。

**修后领域补证。** evidence2-clippy七分面exit0，evidence2-virt正常reset、无panic/rejected、非FAL缺失0；含构建124.30s，默认360不变。原guest artifacts/failed-acceptance-20261011-135858-44102.log，evidence2-summary.json核对两provider纯值标量/Range与Rename Exists保持两边/成功Replace/后代防环，各标记2次，原provider实际退款2次。G05-04原先“Replace/环路尚无正式消费者”缺口现补齐；G05-05增加Bool/i64/u64/f64/Blob及有符号转换Range，既有String/数组/短ReadAt和host损坏编码证据保留。源码中的NodeKind仍为已审定的四种文件种类，不是跨领域固定摘要；没有恢复旧NodeSummary。

**旧路径清单逐行复核。**

| 原清单行 | 当前事实 / 证明 |
|---|---|
| libfal包、目录及构建引用 | Cargo graph无旧包、Rust/Cargo/Justfile/tools零引用；磁盘空目录已rmdir；halcyon.code-workspace词表已改libfs/liboal |
| 混杂发现/文件与抽象层走路 | prefix/resolve/旧pump已删；通用接续唯一在liboal，真实路径遍历仍由文件backend及service registry各自执行 |
| 公共NodeInfo/NodeSummary与统一领域种类 | 旧固定表示Rust零引用；OAL使用ObjectInfo/tag；保留明确文件协议需要的四种NodeKind，符合Bind审定 |
| DirectoryGrant/Grant包装/必需Position | 旧类型/模块无实际引用，客户端直接Namespace/路径；真实GrantTable和稳定NodeRef为授权/生命周期owner，清单明确排除 |
| ServiceRecord/Record/只读NodeStore投影 | record.rs已删，service无NodeStore/Projection/文件库依赖；GetEntry/OAL摘要由registry生成，真实文件NodeStore<BackingRecord>保留 |
| ExportPolicy/Handle Property/affine Take | Rust无旧policy/操作，纯值独立编码，能力只由领域RPC交付；旧机制锚点等用户裁决 |
| expected_identity/CreatedTarget自动回删 | 旧符号零引用；CreateOpen/Copy只持具体会话，Unknown不重放、不按名删替代者 |
| 裸订阅ID管理授权 | Subscribe旧操作无引用；独立fs/service控制入口处理Query/Ack/Cancel，序列描述不授管理权 |
| Register/PublishReady/旧状态/租约/QueryName/按名Withdraw | service旧opcode/Starting/Draining无引用，单步Register及控制Withdraw/QueryRegistration；Watch Terminal、内部GrantLease、内核进程Starting是不同领域保留项 |
| registry文件Backend/Projection/FAL hook | 旧srv_fs registration/authority/stream模块已删；独立service provider/registry及真实角色表，宿主只装配现有任务族 |
| test_fal身份和失效锚点 | 当前目录/图/启动/initfs/notes均test_fs，无alias；历史原名保留。唯一未清零项是验收器7旧锚点，已逐项只读交协调者报用户裁决，未擅改 |

辅助盘点sub-26为只读事实取证；其并行快照中的空目录、startup/rpc失效说明在本节已关闭，不再作当前残留。设计正文失效Markdown源码链接5处（81三处、457、472），以及457旧符号叙述/源码迁移表旧路径，均已列给MellowPond按主计划owner修改；不改写历史取证。当前impls链接检查与上述代码/图证据均保存于artifacts/p4。

**交付前检查的当前边界。** 本轮增加的是命名同步和原正式消费者断言，没有新ABI、权限/付款接口、状态机、执行宿主或清理owner；实际公开NodeKind/GrantTable/NodeStore不误删。纯值预期来自类型与Range合同，Rename预期来自不可覆盖/原子Replace/不可移动入后代的合同，不用当前输出反推正确答案。夹具切片失败及修复均保留记录，修后运行与lint通过；P4最终结构检查、连续40release、just check及acceptance仍待后续收束，不把本小节等同P4完成。

### P4 验证缺口处置表（2026-10-11，待锚点裁决与聚合证据收口）

本表承接G05唯一编号及既有N6/W05条目，不新建平行todo。“已验”只指所列当前合同门，host与运行不互相替代；协调者最终收口以本表与原条目核对。连续40门与完整acceptance均已有后述证据，历史根因与精确注入窗口仍保持原分级。

| ID | 当前证据与处置 | 剩余边界 / 唯一接续 |
|---|---|---|
| G05-01 | 旧handle Property/affine已删；纯值host严格编码、实际GetEntry/控制typed交付已验 | 用户授权协调者判断后，废止锚点已按裁决删除；能力槽与纯值的分离沿当前协议验证，不恢复旧机制 |
| G05-02 | 两provider Watch权限/mask、Create/Modify/Unbound/Terminal、旧Ack/Query/Cancel、弱目标/最后控制退款及异步组合已验 | 旧两锚点已按裁决合并为当前完整场景输出；W05-04未知历史Err和精确注入不由正常路径关闭 |
| G05-03 | Watch/Bytes/会话/Offer与A/B隔离已运行；P4新增Node/Grant/Binding各自provider共享Quota、未释放时另一入口拒绝、释放实际owner后再准入、恢复项重新占额度及最终全0均已运行 | 当前资源饱和/恢复门已补齐；三类local=global不误写为独立tenant-local耗尽，未来独立local部署时触发本类隔离补测，引用W04及Q项 |
| G05-04 | 实际两页cursor、双目录Rename/目录改名/跨backing拒绝、Exists保持两边、成功Replace、防后代环已验；准备后父链/目录内容改变与稳定IDhost已验 | 精确准入后父链交错仍属host/源码，沿下方F项；新运行锚点裁决不改变范围 |
| G05-05 | 实际六标量、String/数组、类型Mismatch与有符号Range、稀疏/短ReadAt和完整内容；host损坏/过长/截断/计数边界已验 | 未机械枚举每种同质数组/浮点位型的运行组合，协议host与当前共享编码负责；类型/编码变更时补相应实际消费者 |
| G05-06 | 未Start建立期限、真实端点退休与最后控制释放已验 | 精确Close/来源失败仅源码，F项 |
| G05-07 | Offer双owner弃置、原会话退款/再准入实际运行 | 同F项；无DeferredStub |
| G05-08 | Start门/重复Start/建立前EOF、空流正常完成实际运行；FSM独立host | 当前门已验，无新剩余实现 |
| G05-09 | primary/secondary冻结范围、offset/上界/EOF、实际数量与完整收束运行 | 当前门已验，不承诺源快照 |
| G05-10 | CreateOpen提交后队列回复弃置保留事实；Sent Unknown不重放、同Dispatcher实际迟到与后继完成运行 | 已核销当前门；Sent本身不证明提交，故障时点F项 |
| G05-11 | 真实流内容/计数/冻结末端、SetLength缩短/Busy退休/稀疏扩长零填充运行和Data/FSM host | 当前门已验；资源释放失败F项 |
| G05-12 | 未承诺条件Open，公共Expected/conditional已删；具体会话保活由流/Copy现合同负责 | 不适用的旧能力锚点已按裁决删除，不将pin场景冒充条件建立 |
| G05-13 | 已入队offer能力的弃置、重复退役/额度恢复与provider最终清理运行 | Delivery/来源/Close精确故障F项 |
| G05-14 | Property原子Create与跨provider Stream/ReadAt完整内容、23040计数和两端Completed运行 | 当前两分支门核销；纯值其他组合归05，未恢复跨域Move |
| G05-15 | 独占CreateOpen、改名后具体实例继续写与同名替代者不变运行 | 当前pin/不按名回删门核销；Unknown目标仍归provider原backing停止owner |
| G05-16 | Exists保留原值、空目标/空进度、提交前取消/部分取消运行 | 当前变体门核销 |
| G05-17 | 非零transported后取消、前缀内容/上界、原任务物理清理Complete、并发后继不受影响运行 | 未强迫非零目标accepted后取消，结果仍保留已确认下界；精确窗口F项，不误报最终数量 |
| G05-18 | 在途provider停止、Created/部分进度/清理Complete和双provider收集运行 | 固定P4 fingerprint连续40release与完整acceptance门均完成，最终C11基线另有组合证据；[旧midflight档案](todo-2026-09-29-fal-midflight-copy-flake.md#2026-10-11-历史证据复核与最终处置)已按整体机制替换关闭，具体失败子条件仍未知，不将新门通过改写为已定位旧根因 |
| G05-19 | Derive目录/领域及完整运输位、普通母关闭后child、错型Permission、受限Boundary和流权限运行；显式revoke host | 真实准入后母显式revoke交错F项；不把普通关闭等同revoke |
| G05-20 | Open Start/Query/Finish/Cancel、满环/EOF/冻结数量及原聚合锚点运行 | 当前控制门核销，未恢复旧Lookup前置 |
| G05-21 | 管理Root/context/Lifetime与独立付款、Bind/Unbind、共享/受限出口、suffix/循环预算、replacement及queued未知不重放运行；迟到prepare不删替代者/稳定ID host | 发送后父显式撤销/真正迟到cap/来源及Close故障F项，当前正常能力已接通 |
| G05-22 | queued回复未领取、真实cap退出、Copy同Dispatcher旧reply忽略/后继内容和sealed活waiter领取运行 | malformed迟到下游/Delivery故障精确窗口F项，不由普通queued成功推定 |

**Q：本次P4真实饱和组合已运行。** 原延期建议被MellowPond否决后，已只在test_fs补Node→Grant→Binding的实际Quota、未释放时另一入口也拒绝、释放夹具真实owner后真正准入恢复及最终provider全0；quota1-clippy七面0，quota1-virt正常reset，原guest artifacts/failed-acceptance-20261011-144139-94589.log（135.90s含构建，rejected/非FAL缺失0），quota1-summary.json核三标记各1与双provider退款各1。界限由deployment唯一元数据推导，无查询协议/额度政策改动。boot三类tenant local继承global同值，H root占NODES内一个Node、既有A/B grant共享名额，证明的是provider共享上限；隔离证据引用W04 Watch/Session，未来给Node/Grant独立local时触发本类隔离补测。Node尝试上界NODES包含root/已有占用，不猜可创建数量；Grant最后真实sender关闭后成功发行才核实际退款；Binding目标shared grant分摊两provider，成功项等于BINDINGS以排除先触Grant上限，Unbind后成功Bind才核记录/issuer/shared物理退役。恢复项占用后A又Quota，不把撤名回复/CLOSED当恢复。连续40基于先前fingerprint已完成，新增consumer未改Copy，依协调裁决由最终acceptance覆盖而不重跑40。当前运行门已闭合；锚点按用户授权协调裁决后，完整acceptance3严格通过。

**F：精确故障窗口的验证债务。** 当前source/pin/Outbox/控制/映射各自保有原owner，失败返回原槽，注销重试不重新扣账；各批review及host已核结构。owner RubyRaven，协调MellowPond；实际位置分别为fs provider/outbound/stream/Copy、librpc Dispatcher/Outbox和原Runtime/source。触发为既有正式注入入口可安全拒绝/延迟来源操作、制造隔离子进程的真实输出/Delivery错误、相关生产故障出现，或修改这些责任链；无现成入口时不造测试服务或假cap来冒充。补证按准入前→已发送/未提交→已提交/未领取→实际注销/Close回执顺序，在原正式消费者核原付款/owner不提前释放、未回执不自旋、迟到结果不影响替代者及最终退款；各原G05行只在其窗口实际覆盖后关闭，不以单个统一成功标记覆盖全部。

**N6与W05逐项处置。**

| ID | 处置 | 触发与完成门 |
|---|---|---|
| N6 | 三角色负向签发/pending保持及邻接退款已有运行；mint-specific quota/分配/KOID/输出fault/Frozen/接受后Close、逐壳费用仍仅源码；延期验证 | 唯一owner/位置/触发与标准只保留在[Notification前置N6](../todo-2026-10-10-notification-observer-roles.md#实施工作记录角色迁移与开发证据2026-10-10rubyraven)，本篇只引用，不复制；P4普通聚合不能宣称这些时点已注入 |
| W05-01 | Lifetime Remove未回执改Parked已修，正常注销运行；真实拒绝/延迟未注入，延期验证 | 原1026条目拥有：正式来源设施可控拒绝/延迟时核原Request/角色/Charge保留、无连续Runnable、自同一真实回执完成退款；与F项接通 |
| W05-02 | 被改名target自身误发Rename已修；仅直接两父事件、非祖先/非目标实际运行 | 已验关闭，目录与文件Rename原场景保留 |
| W05-03 | sequence耗尽未释放active token已修；真实Budget/NodeStore/GrantLease host核weak/token清空及费用/permit恢复 | host边界已验关闭，不声称真实内核seq耗尽注入 |
| W05-04 | 历史Watch准入未知Err未复现，整体deadline改每次有限准入已落实；原六轮及后续普通/压力回归正常 | 仅观察，不复发就不继续调查；统一触发条件见 [KNOWN_ISSUES](../../KNOWN_ISSUES.md#历史偶发异常仅观察)，原证据及复发后处置见[历史问题重新核定](#历史问题重新核定2026-10-11) |
| W05-05 | Rename错角色映射到RPC Closed已修；NotificationSignaler/同Mailbox WatchControl实际Permission，跨backing保持源值运行 | 已验关闭；真正revoke时点归19/F项，不扩张核销 |
| W05-06 | 已确认正常进展、路线起点变晚；扩大core后实际reset，后续完整Copy又在原360内完成并恢复默认 | 已验关闭；不将panic/持久故障接管后的124作为正常撞限 |

以上延期项均为验证缺口，不保留旧机制或改变当前外部合同；每项有原owner/触发/完成门。当前周期不宣称“所有交错都已测试”。所有新增已知可达代码finding已按原记录修复；锚点裁决、连续40门和完整聚合均已有证据，整体归档及历史替代完成依据由协调者核定这些分级边界后记录。

**P4 当前交付前结构检查（REVIEW，RubyRaven）。** 对照此前各批findings及最终源码复核：通用发现/路径接续、文件backing、service registry、RPC、执行/记账各有唯一owner，Cargo无旧libfal或test_fal/alias，真实NodeKind/NodeStore/GrantTable未误删；共享ABI及Notification五typed角色两侧同步。正常/Quota/失败/取消/退出/期限/退休均沿原任务/来源/Outbox/长期对象责任收束，来源实际注销前保原Charge和映射owner，重试不换账/重新扣账；Shared无DUP输入由真实消费者接通，不存在假cap/重复Query采用或独立Copy运行体。准备与提交分离、稳定ID复核及Unknown不重放/按名回删符合当前合同，Copy业务完成不被物理关闭重试改写。

原C1—C3、流F1—F4、Bind两P2、W05已修项按原反例和对应host/真实运行复核；未发现需要本批再改的可达代码finding。N6、真实来源/Close/父撤销精确窗口与W05-04未知历史Err保持分级，不由普通成功推出全窗口通过。P4新增quota只在既有test_fs：每次调用有限期限、饱和尝试有deployment结构上界，恢复只重试明确Quota（未提交），未知/Transport或其它Status立即失败；夹具仅解除自己持有的名字、sender与Binding owner。Binding分摊目标grant避免错误资源先耗尽，成功计数和恢复项重新占额度捕获饱和/退款错误；没有额度调整、查询协议、新unsafe/锁/生产状态机。标量/Range、Rename Replace/防环预期有独立合同依据，夹具切片失败已修且原失败证据保留。

当前impls、libraries README、notes索引与源职责一致；主设计基线旧路径明确为历史迁移起点，没有死源码链接冒充当前提供者。源/build fingerprint连续40已在原记录分清kernel/容器hash，之后只增加quota消费者。just check exit0（artifacts/p4/check.log），quota1-clippy七分面exit0，quota1-core正常reset/非FAL缺失0；该轮strict1只是当时旧锚点，不改写历史证据。**当前交付断点：锚点裁决已实施，完整just acceptance3 exit0，跨平台与故障路线证据已回填；协调者核定分级缺口和历史最终处置后整理提交范围/归档。** 不重跑已关闭40门，未commit/push。

### P4 锚点裁决实施与聚合验收（2026-10-11）

用户授权协调者判断，MellowPond审定按P4只读建议调整7条旧锚点：跨provider Move换为真实DeriveDirectory/Rename，Watch权限与Create/Modify/Delete合并为真实Watch合同，枚举/同域Move换为真实direct paths并由同一Rename标记共同覆盖；handle Property、affine Take、conditional Open对应已废止或未承诺的能力，删除其旧锚点，不恢复API或伪造输出。3个新required各只出现一次，其余required、profile附加项、rejected及终态/收割/日志规则一律不动。

判定依据是现有标记位于同一完整场景所有断言之后；Watch覆盖Create/Modify与新增Unbound/TargetRetired/staleAck，Rename覆盖同域/跨域、Replace与防环，direct paths包含真实两页cursor。合同覆盖不少于原保留场景。3项废止能力保留成功文本只会诱导伪造输出。锚点调整不关闭任何G05/N6/W05残余窗口；现存领域与失败责任仍按本篇分级表判断。

精确脚本变更证明 artifacts/p4/acceptance-policy-diff-proof.json：只修改获准7项，其余required完全保持、无重复标记，rejected及其后源码逐字保持；原脚本快照 acceptance-policy-before.sh，bash -n通过。直接启动完整 `just acceptance`，runner99637，完整包装日志 acceptance1.log、退出及含构建耗时同名前缀记录；QEMU_ACCEPTANCE_KEEP_LOG=1使用既有出口保留成功guest/meta。当前未报告聚合通过，实际结果按后续证据回填。

**首次聚合与sifive_u运行预算。** acceptance1的clippy七面、debug stress、release core均严格通过；完整成功guest分别 artifacts/qemu-acceptance-20261011-145340-191.log、qemu-acceptance-20261011-145703-7398.log，新增三类Quota/实际恢复、Copy及双provider退款齐全，stress race16/16。sifive_u在外层120s被timeout终止，guest failed-acceptance-20261011-145732-8609.log（日志出生到末次写入120.12s）无panic/FAILED；Quota、Watch/Rename/Bind、Property/Stream/并发取消/空/冲突/提交前取消均到达，最后Copy为Complete且来源Empty，后续pin/未知场景、公共时间与reset尚未完成。allow-timeout仅免于把124直接当终态，继续严格核required后首报public-time缺失；不是reset明确失败后的合法主动收割，不能报平台路线通过。

亲读下一场景及Root等待：每个新Copy共用有限FollowBudget/绝对业务期限，辅助遇duty.error/提前结束返回，后续RPC及业务阶段有界；结合截止前实际完成状态，协调者认可正常有界工作碰到路线预算。按AGENTS规则，只把Justfile唯一SIFIVE_U_TIMEOUT翻倍到240，完整场景不删，其他路线默认值不改。BUILD-AND-TEST第58行删除stress/sifive具体默认秒数，仅保留各路线需更长预算的理由，数值单一真值为Justfile。此调整不倒推唯一根因或性能归因；若240复验仍停同点，则按停驻定位状态交错，不继续盲翻倍。

acceptance1实际exit1，完整日志/退出/含构建总耗时保留（358.24s含前三路线与构建）。完整acceptance2已启动runner12789，包装日志同目录acceptance2.log/.exit/.elapsed；首次失败不被覆盖，已通过路线的重跑是本次失败及预算变更后的完整聚合复验，不重跑已关闭40门。

**第二次聚合与nofd运行预算。** acceptance2的clippy七面、debug stress、release和sifive_u均严格通过，原成功guest依次 qemu-acceptance-20261011-150232-13248.log、150558-20490.log、150625-21692.log；sifive文件跨度139.24s，完整Quota/Copy/退款与公共收束均到达，reset accepted后平台明确返回reset failed，由原allow-timeout规则合规主动收割，不称QEMU正常shutdown退出。240复验没有停在前轮同点，确认120窗口不足以完成这轮负载。

随后nofd在原90s被外层timeout终止，acceptance2实际exit124；guest failed-acceptance-20261011-150847-26660.log（文件跨度90.22s）无panic/FAILED，两个provider的流/Watch/Rename及Node/Grant Quota实际完成，后续Binding/Copy尚未完成。Binding尝试有BINDINGS结构上界、每次RPC及恢复同一有限期限；后续Copy/Root等待守卫不变。这是有界工作撞该路线期限，按既定规则只将Justfile VIRT_NOFD_TIMEOUT翻倍到180，其他当前默认不改，不删场景。若180仍同点停驻则转状态交错取证，不继续盲翻倍。

完整acceptance3已启动runner29785（artifacts/p4/acceptance3.log/.exit/.elapsed）；前两轮失败和已过路线的guest/meta各自保留，不把局部通过拼接成完整聚合通过。默认秒数仍只在Justfile，手册保留路线理由；这两次时间预算调整不改变任何业务协议deadline、quota政策或rejected规则。

**第三次完整聚合通过（RubyRaven实施）。** `just acceptance`实际exit0，总784.97s（含构建、所有路线及诊断），完整包装/退出/耗时为 artifacts/p4/acceptance3.log/.exit/.elapsed，逐路线核验为 acceptance3-summary.json。5个完整guest的required缺失0、rejected0；每条三类Q标记各1、双provider付款/真实退款标记各2，Copy七旧锚点与共享无DUP并发取消、sealed活waiter、hold撤除后业务期限唤醒均齐全。W04同Dispatcher A/B回baseline、FS1/SRV2异步Watch及P3继续运行。本轮未修改生产源码、消费者场景或业务期限。

| 路线 | 实际结果与证据 | 边界 |
|---|---|---|
| clippy | shared-host/os-host/kernel-target/user-host/user-target/user-stress/user-fp七面通过 | 聚合规定检查，不重复单跑 |
| virt-stress | qemu-acceptance-20261011-151246-30223.log，正常reset，16/16矩阵、256登记及完整业务/退款 | 文件出生至末写202.00s，非精确guest计时 |
| virt-release | qemu-acceptance-20261011-151611-37150.log，正常reset、完整core | 文件跨度24.41s，不重跑固定fingerprint40门 |
| sifive_u | qemu-acceptance-20261011-151638-38333.log，完整core，reset accepted后明确reset failed，原allow-timeout终态收割，严格通过 | 文件跨度138.44s，未停原120窗口位置，不称QEMU正常shutdown |
| virt-nofd | qemu-acceptance-20261011-151859-43236.log，完整core、预期FP降级、正常reset | 文件跨度129.52s，180复验未停原90窗口位置 |
| Boot failure | boot-failure/20261011T072112.703260Z-47785，panic/alloc/fatal每项4hart均PARKED、gate=2，包装器各项通过 | 预期故障门，不是正常reset或Notification mint费用注入 |
| IPI failure | ipi-failure/20261011T072117.134687Z-48139/evidence.json，跳过一次真实SBI IPI并回SBI_ERR_FAILED，稳定stop且4hart停驻 | 不证明DMA/device静默或时间上界 |
| VM fault | vm-fault/20261011T072121.184311Z-48488/evidence.json，正式hammer发布窗口真实cause13→RetryFault，detach后完整stress/reset；guest qemu-acceptance-20261011-152121-48483.log | 文件跨度246.50s，GDB暂停非计时/WCET证据 |
| VM repair | vm-repair/20261011T072530.637012Z-56740/evidence.json，read/write实际SFENCE及GPR/pc保持，execute另有FENCE.I，repeat第二次故障界，uaccess读/写/cookie的SUM前栅栏 | trap cause/stval由外部诊断模拟，不冒充真实旧TLB缓存竞争；诊断结束收割，不是完整业务reset |

表中guest和诊断路径均位于artifacts/；完整meta含workload/platform/profile/节流/命令及ELF/BootPackage SHA。SIFIVE和NOFD仅扩大各自真实撞限预算，首两轮失败保留；展开后的两条路线均跨过原截断点并完整收束，没有继续翻倍。BUILD-AND-TEST第58行已无route默认秒数，Justfile拥有唯一默认数值；业务deadline、部署policy、rejected、其他required不动。

**三类覆盖。** 实际运行：P4真实Node/Grant/Binding共享饱和与退休恢复、纯值/Range和Rename Replace/防环、W04/Watch/管理/流/Copy回归、上述平台/外部故障及固定40门。host/源码：准备后稳定ID/迟到Unbind、授权复核、流FSM/稀疏清尾及资金返还等既有独立回归，按原批次证据保留，不升级为现场交错。未覆盖：N6 mint-specific精确失败/逐项费用，F及W05-01真实来源拒绝/延迟与Close回执，准入后母revoke、malformed迟到/Delivery时点、非零目标accepted取消窗口，以及W05-04未知历史Err根因；各自已有唯一owner/触发/完成门，聚合成功不核销它们。无调度域契约变化，本案不另加hetero。

**diff与删除清单。** 整案跨层改动为OAL与fs/service领域拆分、Notification角色及ABI同步、显式执行付款/H-A-B装配、Watch/Bind/流/Copy和正式init消费者、test_fs同名身份/启动/initfs及配套impls/index/计划；本次最终聚合只改获准7锚点、两个受限路线默认值和事实回填。P4旧清单逐行及Cargo图证明在 deletion-and-graph.json，旧libfal/test_fal、NodeInfo/NodeSummary/DirectoryGrant/ServiceRecord/ExportPolicy/CreatedTarget/CopySpec/CopyRuntime/expected_identity/PublishReady/QueryName等实际Rust/Cargo/Justfile引用为0，无alias，service无fs依赖；真实四种NodeKind及文件GrantTable/NodeStore保留，历史叙述不误判为代码残留。脚本精确差异证明 acceptance-policy-diff-proof.json、bash -n和git diff --check均通过；当前源/文档事实与历史日志分开。协调者已认可聚合验收，归档由协调者执行，未commit/push。

P4改名的旧 `artifacts/build/test_fal` 为改名前残留ELF，并未装入当前initfs；它会被build_user的既有test_*审计glob再次读取，现已原样移到 `artifacts/p4/retired-build/test_fal` 保存历史字节/hash，退出当前build输出集。记录 retired-build-artifact.json；不删除历史日志，不改构建行为，不重跑已经通过的运行门。当前initfs/启动仍仅test_fs。提交文件清单与完整diff摘要保存在 artifacts/p4/submission-inventory.md/.json，产物全部排除，不改变共享index。

## 提交前机制收敛

用户在实施完成后授权整理机制及过渡代码，提交留待收敛完成。目标保持已确认外部行为、授权、付款、失败/取消/退休 owner 和长期扩展边界，消除成形实现中没有独立职责的重复表示及政策错层；仅少代码、单一消费者或暂时无消费者不构成删除理由。

分工：GrandOwl 负责主要信息获取，亲读关键代码、决定收敛方案，并独自完成所有代码/文档修改与验证；MellowPond、RubyRaven 仅提供既有实施事实、历史原因及遗漏线索，不修改任何文件。独立只读 reviewer 辅助检查 execution/RPC/Notification 支撑，结论仍由 GrandOwl 据源码核实。不新建平行工作记录。

| 项 | 现状与目标 | 责任与验证 |
|---|---|---|
| C01 根出口政策 | libfs::authority::root_output_transport 的唯一实际政策来自 srv_fs 部署，供 boot 和 test_fs 使用；移到 srv_fs::deployment，不更改位集合、不留旧 facade | GrandOwl；同步装配/消费者，target lint 与现有运输权限运行场景 |
| C02 目录授权表示 | GrantState 与 AccessSnapshot 同字段、同 lease/pin；表直接持 AccessSnapshot，删除重组 snapshot 与重复查表，保留未知角色/撤销错误区别 | GrandOwl；准备容量、原 Charge、使用引用/撤销/最后关闭边界不变，现有 host 授权回归及真实 Derive/Rename 验收 |
| C03 Copy 数据存储 | Kind 与 Progress 重复表示复制种类，且两种分支都预留流块及 Property 缓冲；以 Progress 变体判种类，按分支只准备一份数据缓冲 | GrandOwl；不变更阶段、Unknown/成功冻结、来源掩码及清理 owner，复用 Property/Stream/取消/未知结果运行门 |
| C04 init 组合收尾 | Copy 五组等长 Vec 中 creation 无读取，其余按同一 slot 管理；合成单个观察记录 Vec。FsWatch/ServiceWatch/Copy 的共用运输退出等待由 ReadWorld 一个方法实现 | GrandOwl；不增加运行体，不提前交付 Complete，保留所有客户退出后封口、活 waiter 领取、来源退休及暂停期限行为，完整正式组合回归 |
| C05 文件回复收束 | Request.advance 开头已处理终结 Outbox，后面的 outbound 分支再次检测同一状态不可到达；统一在前段取消在途下游并推进回复来源退休。流维护与普通尾部共用原回复/费用/授权引用释放函数 | GrandOwl；保持取消不重放、来源回执先于退款、原 pending/pin 到 completion 收束，Bind/流/Copy 与取消运行门 |
| C06 Watch 客户会话 | fs/service 两份 ConsumerTask 仅两处文案不同；librpc::subscription 唯一拥有通知/Query/重查确认/Ack/来源及RPC退休，领域 Watch 实现静态 Protocol 提供编解码与快照解释 | GrandOwl；域 Status/WatchInfo/事件位及提供端 WatchTable 仍各自拥有，不新 Runtime、不引入跨域依赖或兼容 alias；真实 fs/service 异步 Watch 与退出回归 |
| C07 装配付款自检 | 删除 Billing/DownstreamCheck 与逐接收账本断言；真实停机直接读 Budget，收费准备归领域 Funding，实际 A/B Dispatcher 验证归 init 原 ReadDuty | GrandOwl；具体责任与证据见下节，提供端不为验收延迟 Ready |
| C08 订阅关闭责任 | 按合法非映射 owner 的必成 Close 合同，删除 ConsumerTask 的 CallError 暂存和角色关闭重试；来源仍须真实注销后才释放角色 | GrandOwl；核 Dispatcher 完成出口与 typed Drop，复用两域异步 Watch/退出 |
| C09 非零接受后取消 | 原 Copy 取消场景在停止客户端搬运后以正式 ReadAt 确认目标非零前缀，再取消并核另一任务正常完成 | GrandOwl；目标前缀保留、运输上界及原 Task 清理完成，不把外部观察写回 Copy 未确认计数 |
| C10 派生/Copy 叶能力清理 | 删除 PendingOutbound 普通关闭槽/已检查标记/退避，Copy 的 Reply/orphan/CallError 暂存及 PartialPlane 多余变体，Binding 母本关闭轮转位 | GrandOwl；完成后按 IPC 固定上界释放叶 owner，真实来源/映射清理与原 pin/费用保留 |
| C11 并发创建误冲突 | 聚合 nofd 发现不同名字的 CreateOpen 被父目录整体 version 拒绝；改为名字独占、当前授权范围/权限及版本可递增的最终条件 | GrandOwl；确定反例修前失败、修后3项新增回归与原 nofd/完整聚合通过，同名竞争及撤销仍拒绝 |

C06 的知识边界：共同客户端只知道 Query/Ack(sequence)、NotificationObserver、控制 sender、快照的 sequence/pending/terminal 投影及 RPC owner；领域 Protocol 保留线上形状与完整响应校验，不把领域业务操作交给 OAL 或 RPC 决策。ConsumerTask 直接以具体领域 Watch 类型实例化，删除原两份任务和 Failure 定义。Watch 的 scope、事件 mask、对象弱引用及 provider 退休不是共同客户端责任，不据字段相似强行抽象。

C01—C06 已由 GrandOwl 亲自实施，当前增量没有更改 kernel/shared ABI、权限数值、提供端业务事件或部署容量。`artifacts/convergence/before/` 保存本轮修改前的实施源码，`incremental.patch` 与 `inventory.json` 区分本轮收敛和原 P1—P4 大改动；Rust 增量为新增 654/删除 973 行（净减319），收益是少一份订阅状态机、一个授权表示、四个平行容器及一份 Copy 缓冲，不以行数充当正确性证据。

**亲读后保留的责任边界。**

- TaskSubmission、Requests 作用域、Task/Bound 来源及 SourceEntry 原 Charge：分别拥有任务准入、声明期借用、独立付款身份及实际占用；不能用一个 Runtime 默认账或当前 Task payer 合并。Notification 三角色共享 Core/WaitQueue，静态权限和业务寿命已经统一；不引入通用 Observer wrapper 或动态 signals mask。
- fs 的 PreparedControl、SubscriptionLife 及 Stream State：未公开准备、控制寿命和数据业务终态不同；已编码回复不等于被领取，Completed 读流仍须保留 Producer 至对端真正退出。Copy 的 before_creation、succeeded、实际来源 mask 同样分别承载 Unknown 回退、成功冻结和真实撤装，均保留。
- OAL 路径/Boundary、fs/service 协议及提供端 WatchTable：领域共享客户端会话不合并事件编号、Status、权限、对象存储或目标退休。两份 Funding 也保留各自领域资源类型；字面形状相同不是跨领域反向依赖的理由。
- FileTask/InitTask 的 enum 转发把实际 owner 纳入宿主同一 Runtime，是组合接缝；不因纯转发改为跨库回调注册或另建运行体。

**装配自检的第一轮处置（已由 C07 接续修订）。** C01—C06 收口时曾将 Billing/DownstreamCheck 与 provider 内接收断言保留到正式装配分案。用户要求继续清理后，确认这项职责分离可在现有库与 init 验收装配内完成，不再以正式服务装配为前置；原决定及第一轮证据保留为历史，当前方案见下节 C07。

**开发与结构检查。** 直接核对 C01—C06 的原/新 owner、失败返还、真实注销与退款点；C05 将原不可达分支的取消意图放到有效的前段，真实 completion/pin 仍在原 Request，未在来源退休前释放映射。C06 按原两份逐分支对照，编解码仍在领域，回调和客户等待共享运输退出保持原次序。辅助 reviewer 因上游连接错误中断，没有形成可用审查结论；本节结论来自主代理亲读，不记为独立审查通过。

host：libfs 39、liboal 27、libservice 12，共78项通过（显式 aarch64-apple-darwin，`artifacts/convergence/host.log/.exit`）。第一次 user-check 通过；提取 Watch 后 user-check2 发现两处泛型容量查询未标具体 Watch 类型，已修正，原失败日志保留。修后完整 `just acceptance` 的 target/stress clippy 及正式消费者通过。未新增测试框架、删减场景或改动验收锚点。费用/Close 精确失败窗口沿原 F/N6/W05 分级保留，本轮普通运行不冒充注入补证。

**最终验证与收口。** `just check` exit0；完整 `just acceptance` exit0，812.59秒（含构建），七面 clippy、virt-stress、virt-release、sifive_u、virt-nofd、Boot panic/alloc/fatal、IPI failure、VM fault/repair 均通过。5份完整 guest 的 required 缺失与 rejected 均0，每份包含 fs/service 异步 Watch、同 Dispatcher A/B 付款退款、Copy 并发取消/活waiter/暂停期限、Node/Grant/Binding Quota及双provider退款。sifive_u 是完整业务后 reset accepted→平台明确 reset failed 的既有合规收割；VM repair 的模拟 cause/stval 不冒充真实陈旧 TLB。

证据统一在 `artifacts/convergence/acceptance-summary.json`、完整 `acceptance.log/.exit/.elapsed`、`source-fingerprint.json` 与 `toolchain.json`；源码指纹 `b12682785b5eedab98e9947620ac0a19a3f03a1f6f8087f8afc676cb49d2fe72` 覆盖363份源码/配置/工具输入，验收期间无变化。Justfile与验收脚本逐字保持本轮起点。进程核对无本轮QEMU/GDB/验证runner遗留；本轮源码未再修改，不重复已通过的检查。

交付前结构检查按 REVIEW 完成：C01—C06 的旧表示/副本/兼容出口已删除，公开消费者同步，准入、业务提交、在途RPC、来源退休及退款各有原 owner；没有新增锁、unsafe、内核ABI、调度器、账户表或独立运行体。impls 的当前描述、库职责和主计划接力已同步；专用半迁移矫正登记解除，原验证窗口及历史未知保留在唯一计划，不宣告根因已定位。上述六项完成，整案仍未 commit/push。

### C07：分离付款验收与正常服务路径

用户要求继续解决第一轮保留项，只有具体前置确实缺失时才延期。当前代码、文档与验证仍由 GrandOwl 一人完成，不向其他代理分派编辑。C01—C06 的源指纹及验收是本轮起点，不能作为 C07 完成证据。

已亲读的责任链与本轮设计：

1. 删除 srv_fs::Billing、boot::DownstreamCheck、启动 GetEntry 和 downstream_finished 的 publish/Ready 前置；删除两种 Ingress 的账本快照/操作覆盖及 libfs provider 的 Watch 控制专用 usage/assert。真实 context→原 Funding→Charge 的授权/付款路径保留。
2. Billing::requests_empty 实际承担正常运输停机条件，迁为 World::requests_empty，直接读原 Budget 的 fs/service Request 全局占用（AccountView::budget_usage 的 used），不保存第二份 H/A/B 账户目录。正常停止先实际退休所有 owner，再确认 Budget 全局资源为零；这是资源结束不变量，不再伪称已运行逐控制付款验收。Budget 的只读 is_empty 不封口、不表示账户关闭，零额度 view 可继续存活。
3. fs/service 的固定请求费用准备由各自 Funding::prepare_request<T> 拥有：请求/Outbox/回复缓冲/来源费用与 Task 存储 Charge 分开返回，保留当前两种释放时机。纯 host 测试直接使用真实 Budget A/B，捕获最后一项准入失败的完整回滚、B 成功不向 A 扣账、回复费用释放后 Task 字节仍持有、最终退款。预期来自已有收费/释放合同，不复制 resolver 或制造假 capability。
4. 同一 Dispatcher 的真实 A/B 来源费用与退款验证移至 srv_init 已有 RootSupervisor/ReadDuty 客户段，在 primary Ready 后使用两份实际服务目录入口。使用同一个 Budget 的 H/A/B、原 Dispatcher、原 Assembly/Runtime 驱动；两条调用仍有有限期限，分别观察真实私有来源费用并从实际 completion 领取后核退款，封口/退出后全0。不建立新服务、运行体种类、账户查询协议或测试 feature；它是原 boot 验收场景的客户端迁移。
5. srv_fs 的 Dispatcher 容量删除两项自检预留，按每个 Request 至多一项下游、另一个互斥的发布/撤出调用推导 REQUESTS+1；全局来源预算随唯一公式同步。部署自检 Ready 门彻底删除，不仅移动文件或加开关。

验证依赖：先受影响 host 与目标检查，再正式 core 验证无探针的真实 Ready、A/B 调用、业务饱和/恢复与正常退出；最后按范围执行 just check 和完整 acceptance。新增消费者输出明确说明客户端付款/真实来源责任；提供端输出只报告实际预算清空，不能沿用旧逐控制核对的成功文案。精确故障窗口逐项复查已有注入设施与可观察契约后记录，历史失败不改写为已定位。

### C08—C09 与剩余窗口复核（2026-10-11）

**C08 的依据与实现。** `os/capability/src/table.rs::Table::close` 对 Occupied 摘出、Claimed 撤销返还资格，不做分配或返回 Busy；`os/kernel/src/task/handle.rs::close` 只解析本进程 owner 后调用它并交接 release；`libkernel::ipc::object::close_object_owner` 已把合法非映射 typed owner 关闭失败定义为不变量破坏。Dispatcher::take 只在真实来源退出后返 Completion，异步错误只产生 Unsent(Request) 或 Sent/None，不创建同步 Caller 的 ReplyCleanup。因此订阅任务可以立即释放返回错误的 Packet owner，并在自身来源实际 unregistered 后 Drop 控制 sender/Observer；删除 cleanup 字段、retry_cleanup 和两份角色关闭重试，不新增清理时钟/任务。正在登记/移除的 SourceRegistration 及原角色仍保持，不能以表项关闭替代来源回执。同步 Caller 的错误 owner 接口不在本切面改动，MappedEndpoint::close 的普通 View unmap 也不按叶对象处理。

**C09 的依据与实现。** 原 G05-17 只确保客户端 transported>0。现在 init 的原并发 Copy 场景在 await_copy_progress 后暂不推进客户任务，用同一个有限期限向真实目标 provider 发 ReadAt(0,1)，直到读到正确首字节；目标 provider 的 stream::pump 同一次 advance 先 backend.commit 再 accept_write，后继 ReadAt 可见字节时已越过非零接受点。随后取消 A，核保留非零正确前缀、目标不越 transported 上界、B 完成及原 Runtime 清理。没有改变 Copy 协议或其 destination_confirmed 下界，不加 sleep、业务 hook、服务或运行体；G05-17 精确门由这组真实观察补证，运行结果以下节为准。

**其余条目按当前源码重新判定。** 原批次记录保持历史，以下替代笼统的“任何 Close/Remove 失败待注入”说法。当前复核与记录由 GrandOwl 负责，后续责任归各机制的所属计划，不要求信息协作代理继续施工。

| 项 | 本轮事实、结论与保留条件 |
|---|---|
| 普通 Handle/Delivery Close 资源性失败 | 对合法存活进程的独占非映射 owner，不分配，Claimed 也可 Close；该假设不适用，不为它制造 Busy 注入。重复 raw Close、伪 owner 或进程已退出是另一契约，不能称普通资源故障。Delivery::finish 同样调用这一出口；提前结束 use 的真实业务门仍保留。此结论不等于已执行错误注入。 |
| W05-01 与 F 的来源退休 | WaitSet::remove 实际是有锁摘 token、锁外 Drop、必要时 kick；没有 Quota/分配或 Busy 分支，未知 token 返回 ObjectNotFound，而 Runtime 按已退休处理。当前合法 owner 的真实 Remove 拒绝不是可达资源故障，原真实 Busy 注入假设不适用。Runtime 是通用 SourceOps，仍保留失败回原槽和退休 timer；既有 completed_task_waits_for_busy_source_retirement、four_sources_retire_before_their_owner_closes、active_source_clock_failure_after_kernel_remove_retains_charge_until_receipt_without_reacquiring 等 host 用例验证拒绝/回执/原费用，不能冒称真实内核注入。Watch 本身等待真实回执 Parked 的结构及正常两域运行保留；未来 SourceOps/内核 Remove 合同增加可恢复失败或现场出现反例时，才重新打开 provider 精确交错门。 |
| F 的映射关闭 | librunnel::mapping::MappedEndpoint::close 先调用 MappedRegion::unmap，失败返完整原 owner；与叶 capability 的 Close 不同。流/Copy 的原映射、来源和 Charge、有限清理期限保留。既有 Runnel host 失败 owner 用例和 Runtime 来源回执用例分层验证；真实 VM 撤销故障与业务控制注销的组合还没有稳定注入接点，留在 F：触发为正式映射故障入口或实际错误，标准是先撤真实来源、保映射/费用、不连续 Runnable、成功后退款。 |
| G05-21 管理父撤销/迟到替代 | route 公开协议只有 Bind/Unbind，ManagementTable::revoke 由 provider Root 生命周期调用；母 sender 普通关闭不等于对已接受请求立即撤销，Delivery 保持其 use。现有 queued-unknown、替换与 backing host 稳定 ID 证据保留。要命中特定下游已发送/父 lease 撤销/迟到 Derive 返回的组合，须有 provider 生命周期可控停驻点或对应实际故障；不为测试新增公开 revoke 操作。保留原 G05-21 owner/来源责任和完成门，延期的是这一组合的现场证据。 |
| G05-22 坏帧/迟到能力 | 现有 public_ipc 真实 protocol-mismatch 携能力拒绝与后继回复隔离、Copy 旧 txid 忽略、Bind queued reply 退出已覆盖各自边界；正式 fs provider 只发合法 Derive 响应，现有工具不拦截其已发送帧。Bind 特定迟到坏帧与 capability/Delivery 组合需要正式运输注入点；不新建模拟 provider 来冒充真实路径。触发后核旧 txid 不影响替代者、能力/Delivery 实际释放与原 Charge 退出。 |
| N6 | 保留精确注入/逐项费用缺口；本轮对现有 Pool/测试目标/注入工具的核实与具体前置，只由 [Notification 前置的 N6 再核](../todo-2026-10-10-notification-observer-roles.md#n6-注入前置再核2026-10-11)拥有。当前不改变 Notification 内核合同或补建故障服务。 |
| W05-04 与旧 midflight 根因 | 原日志缺失精确 Err/交错，现有改造与回归不能恢复当时证据；保留原唯一历史记录及复现触发。已删除的机制不恢复为诊断工具，不把连续成功写成已证明根因。 |

**开发证据。** C07 的 host 与目标 lint 通过，独立完整 core exit0（136.47秒，artifacts/convergence2/core.log/.exit/.elapsed），实际 A/B 付款与两个 provider 全局预算清空均通过；该次 core 先于 C08/C09 最终增量。最终 host libbudget6、libexecution39、libfs40、librpc16、libservice13 共114项及2项 doc 测试通过，just check exit0。此外，为核实 C08 的 Close 合同，执行 os/capability 既有22项 host 单测通过，含 claimed Close 不取消已接管操作、mint 限额不限制 release。最终聚合结果见下节，第一轮 fingerprint 或 C07 单独 core 不作为最终源码证据。

### 第二轮验证与结构收口

C07—C09 最终源码的 `just check`、七面 clippy 与完整 `just acceptance` 均 exit0，聚合780.01秒含构建；virt-stress、virt-release、sifive_u、virt-nofd、Boot panic/alloc/fatal、IPI failure、VM fault/repair 全部通过。验收 required 只新增真实 A/B Dispatcher 付款/退款、provider owner 退休后预算清空、Copy 非零目标前缀后取消三项，其余脚本字节与第一轮结束时完全相同（acceptance-policy-proof.json）；未删除旧锚点、放宽拒绝项或调整超时。G05-17 的非零 accepted 窗口现由原正式消费者经 ReadAt 稳定触发、后续取消/并发完成/前缀及上界/清理均实际验证，当前此门关闭。

本轮完整命令输出、退出码、耗时、host/各面 lint、诊断证据路径及最终源指纹保存在 `artifacts/convergence2/acceptance-summary.json`。源指纹 `af3ca4345dd81c097ccb47c8e3e506058a1ba4453f411f99c79482171226a0f2` 覆盖363份源码/配置/工具输入，验收起点与结束逐文件一致。VM fault 的完整 stress guest 另存 guest-20261011-163939-20247.log：全部 required 在场、rejected 为0，新增付款/非零取消标记各1、provider 预算清空2。首四条普通路线及 C07 core 的 guest 原日志按脚本默认成功策略删除，保留的是聚合/路线命令输出及严格检查结果；不伪造逐 guest 计数，也不为留日志重跑已经通过的检查。Boot/IPI/VM 诊断原目录均保留；sifive_u 仍是明确 reset failed 后按平台规则收割，VM repair 的 cause/stval 模拟不充当真实陈旧 TLB 复现。

交付前结构检查由 GrandOwl 完成：实际 context→Funding 的原授权/付款解析未替换，RequestFees 与长期 Task 存储的两种释放时机保留；停机使用同一 Budget 全局 Request 计数，Runtime/Assembly 与 World 真实退出后才检查资源零。删掉 Billing 账户目录、两种 Ingress 账本/操作副本、Watch 控制专用付款断言、DownstreamCheck 及所有 publish/Ready 自检门、DOWNSTREAM_CALLS 与多余槽；新增预算只读检查不创建账户或关闭账户。C08 在真实来源退出后释放角色，无兼容 alias、新计时机制或异步清理 owner。C09 只改变原验收场景的观察顺序，不修改 Copy 业务计数/期限/取消语义。没有内核/shared ABI、锁序或公开协议变化；新增 unsafe 仅把真实 duplicate 的独占结果转 typed owner，原 boot 同类探针已删除。

Rust 增量新增414/删除538行（净减124），其中新增包含两项费用准入/退款 host 回归与搬迁后的真实客户端付款验收；比较基线、逐文件增量与完整 patch 在 convergence2/before、inventory.json、incremental.patch。收益按责任分离与删除旧路径判断，行数只作范围摘要。impls、当前接力与前置记录已同步；剩余验证依赖按上节分类保留，不把缺少隔离注入设施或历史日志改写为全部验证完成。未 commit/push，提交仍须用户授权。

### C10：派生与 Copy 的叶能力清理（2026-10-11）

用户要求继续收敛，GrandOwl 亲自追查 C08 相同合同的剩余消费者。基线是 convergence2 的已验源码，保存于 artifacts/convergence3/before；其他代理仍只提供信息，不编辑或测试。

设计：PendingOutbound 的完成来源已经由 Dispatcher 退出，返回临时 owner 都是有界 IPC 叶能力，回复槽数由 MESSAGE_HANDLE_MAX=8 限制。收到 completion 后解码一次、采用合法输出并直接释放其余完整 completion，不需要 closing、retry_at、inspected 或普通 Close 的独立推进阶段；业务 pin、状态错误、未提交准备及待输出 capability 保留到最终授权/期限复核与提交。已提交但未完成的 txid 仍由原 Request 取消/领取，不提前释放业务责任。

Copy 同样不再把已解码 Reply、adopt 失败的普通能力或 CallError 存进三种关闭槽；这些返回值在原处理作用域按 typed owner 合同释放。附着失败只有已建立映射的 MappedEndpoint 需要保留完整 owner；Invitation/Endpoint 只拥有普通非映射表项。Retiring 仍先关闭控制以结束业务，后实际注销数据来源，再撤映射/Producer/Consumer；mapping 失败继续原 cleanup_at 期限保管、无新清理任务。Creation Unknown、before_creation、succeeded、实际来源 mask 和费用付款不变。一次处理至多释放一个 RPC 的8槽及常数个临时角色，工作仍有结构上界。

不改公共协议、ABI或授权上限，不为这一纯所有权收敛新增模拟 harness。目标检查与现有 Bind/Boundary、Copy 取消/未知/非零前缀/退出及完整验收验证；旧源指纹不代表本轮通过。

同链的 BindingRecord 也只持 issuer/shared 两个普通 sender。删除为可恢复 Close 假设引入的 issuer_turn 轮转位，退休每项预算先关 issuer、下一项关 shared；NodeStore 的记录 pin/实际费用与通用 Payload 失败合同保持。target lint 与 libfs/librpc host 已通过，整案新验收接续。

**交付前责任链检查。** 下游 begin 拒绝仍返回原 Request/service owner；Sent 取消仍等待原 txid Completion 与两项调用来源实际退出，未改变重放策略。合法回复先采用需要的 sender/Invitation；类型、权限、协议或采用失败留下的其余能力在原局部返回值中释放，最大8槽，未从错误正文猜所有权。Bind 的有效 shared 已进入稳定记录，提交前仍复核原父/lease/版本；取消后 pin 最后释放触发 NodeStore 退休，不按名字寻找或删除替代者。Boundary push 失败仍把返回 capability 留在 pending，随该原值结束释放。

Copy 的 Property 字节在 decode 中复制，Boundary 接续由 Walk 保存自身路径，StreamInfo 是值；没有数据借用逃出 Reply 作用域。有效控制与 Invitation 在字段中保管，协议附着失败的完整映射仍由 partial 保管；已有数据来源实际退出前不 unmap。Retiring 关闭业务控制与释放映射的顺序不变，部分失败继续保留 mapping owner、原费用和 cleanup_at；失败可先交付 Outcome，任务仍推进到 Complete，成功冻结不被清理改变。Binding 两个母本仍分两个预算项退出，原费用只在记录整体真正退休后退还。

**聚合发现 C11（后续已修复，证据见本节末尾）。** C10 首轮 acceptance 的 stress/release/sifive_u 通过，nofd 出现真实失败，完整日志 artifacts/failed-acceptance-20261011-170145-45295.log，聚合退出124。Copy B 在 Destination 已返回 Conflict/NotCreated 后，A 才取消，根报 Copy cancellation affected other task；不是路线撞限，不能扩大超时或让测试重试掩盖。亲读发现 CreateOpen 的 prepare_create 冻结了整个父目录 version，另一不同名字的创建也 bump 同一版本，最终误判冲突。原独立 Copy 合同要求不同目标名字可独立创建，本轮减少清理推进轮次使现有粗粒度冲突暴露，不把此前正常运行当作不存在的证明。

C11 设计：Create 的提交条件由实际合同直接表达：原授权 active、所选父目录仍处于授权根的可遍历范围、仍有 CREATE、所选名字仍为空、目录版本仍能递增。删除 Create intent 的父版本快照，不冻结整目录成员状态；同名竞争仍 Conflict，撤销仍 Revoked，原 pin/预付 Node/成员 permit/回复和退休责任不变。版本仍用于记录变更并须在提交前检查耗尽，数据写/截断及其他已有条件事务的版本合同不在本次改动。先用现有真实 backing host fixture 固定两个不同名字同时 prepare 的反例，再覆盖名字冲突/撤销与范围移出、版本耗尽，最后重跑失败路线及最终聚合。

C11 的独立 host 反例在修复前 exit101（create-red.log），错误稳定为 independent create rejected: Conflict；修复后 libfs43/librpc16 共59项 host 通过，含新三项：同目录不同名字并发准备、准备父目录移出授权根、两次已准备创建耗尽最后版本；原同名竞争及 revoke 继续通过。范围检查同时堵住了旧版本比较无法发现的“所选父目录自身被移出 grant 根而自身 version 未变”情形。目标 lint 通过。初轮失败日志/退出与 C10 指纹单独保留，不把本轮 Conflict 归因回没有原始 Err 的旧 midflight/W05-04。

修后 nofd 原路线 exit0（nofd-fixed.log/.exit；guest qemu-acceptance-20261011-171131-50494.log），保留180秒默认值，完整 Copy 并发取消、非零目标前缀及两个 provider 预算清空通过。check-fixed exit0；runner50122 继续 acceptance-fixed，代码指纹 ca8ba7025a3d44a832672b4a4310b4da222969fb060430a9474f0bef3143066a，验收期间不改源码。C10 首轮聚合和 e3b80146… 指纹作为失败基线保留，不覆盖。

### 第三轮最终收口

C10/C11 最终 `just acceptance` exit0，784.55秒含构建；七面 clippy、virt-stress、virt-release、sifive_u、virt-nofd、Boot panic/alloc/fatal、IPI failure、VM fault/repair 全部通过。失败路线独立复验 nofd-fixed exit0、138.02秒；先前确定 host 反例 exit101 与修后59项通过分别保存，第一轮失败不改写。5份完整 guest/meta 均保存，逐份核 required 缺失/rejected 为0、新付款和非零前缀取消各1、provider预算清空各2。固定指纹 ca8ba7025a3d44a832672b4a4310b4da222969fb060430a9474f0bef3143066a 覆盖363份输入，结束时逐文件复核一致；没有更改 Justfile、验收脚本、超时、部署容量或 kernel/shared ABI。

证据由 artifacts/convergence3/acceptance-summary.json 汇总，含首轮 C10 failure、create-red、host-fixed、nofd-fixed、check-fixed、acceptance-fixed 的日志/退出/耗时，以及全部 guest 与 Boot/IPI/VM 原目录。sifive_u 的明确 reset failed 收割、VM repair 模拟 cause/stval 的证据边界与前轮相同，不宣称 N6 或映射/迟到 Bind 精确窗口已注入。源码差异、起点与清单为同目录 before、incremental.patch、inventory.json；本轮5份 Rust 文件新增118/删除208行，净减90，新增包含3项独立合同回归。

最终按 REVIEW 复核 C10 的局部 owner 释放与映射/来源责任分离，以及 C11 的名字独占、当前授权范围、版本耗尽和各准备项真实 permit。提交只消费预建 Node/AVL 节点，新增最终检查不分配；不同名字不因共享父版本冲突，同名/撤销/范围失效不放行。C11 的范围回溯复用已有有界目录授权校验，不引入名字锁、预约表、并发重试、第二份树或新运行体。impls、详细目标及当前接力均同步。C10/C11 本轮闭合；C11 不是旧 midflight/W05-04 根因证明。未 commit/push。

### 用户确认本轮交付（2026-10-11）

用户确认已完成代码问题与收敛处理，接受 N6、映射撤销/Bind 特定交错的验证前置及历史未知根因继续由上述唯一记录承载，宣布本轮结束并授权提交。主计划保留专项验证条目，Notification 前置保留 N6；已完成的执行付款前置归档。当前没有继续施工指令，不新建测试设施、不重复验收；提交后按 REVIEW 登记固定代码提交的独立审查，登记不等于已执行审查。

交付代码与随附文档已提交为 `a1ca124fdb70c1f20034f3e7d24d587f4323c98e`，父提交为 `4523bbfe65287f6b84ab73d9f034dae1a526d38d`。[独立 Review 统一核定](../review-2026-10-12-consolidated.md)已固定并完成；报告没有新的可达代码 finding，原五项P2已经按当前可达路径拆分、去重与更正，仍有效部分由本计划及所属专题按统一报告接续。本轮没有推送远端。

### 历史问题重新核定（2026-10-11）

用户授权本轮先扫除历史问题，或证明其存在基础已在迭代/重构中消失。调查基线 `a89b899`、交付代码 `a1ca124`，起点工作树干净；范围为旧 midflight Copy 与 W05-04，不启动独立 Review，不修改源码、时间/容量政策或验收器。主代理亲读旧断言/同步Copy与当前CopyTask、init编排和Watch准入责任链，辅助explorer仅提供旧源码/日志地图。证据汇总为 `artifacts/historical-closure/evidence.json`；当前363份源码/工具输入与最终C11验收指纹逐项一致，5份完整guest SHA也一致，没有新执行风险需要重复QEMU。

| 历史条目 | 本轮事实与处置 | 未覆盖边界与接续 |
|---|---|---|
| 旧 midflight Copy | 19份现存失败日志均停在进度后取消场景，可以区分原 `:1919` 与更后的provider停止 `:2084`；具体原因/进度/owner子条件未知。旧同步Copy、私有pump运行体、有限轮退休和返回即检查owner的结构已整体删除，新任务持续持有txid/来源/映射至Complete；真实取消/非零目标前缀/并发隔离/停止接通，P4的40次与最终C11聚合证据分开复核。按用户允许的机制替换依据关闭并归档，详见[唯一档案](todo-2026-09-29-fal-midflight-copy-flake.md#2026-10-11-历史证据复核与最终处置) | 精确原始根因未恢复，不把C11或连续成功倒推为其原因；当前映射/Bind等精确注入缺口继续沿原条目维护 |
| W05-04 Watch准入未知Err | 原guest `failed-acceptance-20261010-142502-89420.log:169—172` 在primary Watch通过、secondary direct paths通过后，于饱和循环panic，确为准入非Quota分支；日志没有Err、count或clock。主计划1061已记录删除最多24次RPC共用一个5s期限的结构；当前 `user/tests/test_fs/src/watch.rs:242—272` 每次独立准入生成同值有限deadline，以部署WATCHES+1限制总尝试，Quota才结束，其它错误仍立即panic并报告实际Err/count/last_ns/deadline | 只能关闭“整体饱和期限被较晚请求耗尽”的候选结构，不能证明它就是原Err，也不能证明其它候选已消失。W05-04继续活跃，由本计划维护；不将未复现改写为修复 |

**W05-04保留理由与当前可达范围。** Watch的实际准入基础仍存在：[Client::watch](../../user/libraries/libfs/src/client.rs) 仍经真实RPC和typed输出采用，[Request的Watch准备](../../user/libraries/libfs/src/provider.rs) 仍创建Notification/Observer、mint控制、准备表项并登记真实Lifetime来源，[WatchTable::prepare](../../user/libraries/libfs/src/watch.rs) 仍取得Counter/Watch/Bytes/WaitSource并分配预建表项。合法额度失败应映射Quota，其他分配/运输/协议错误仍可能返回非Quota；付款分离、准入/退款与退休结构已有后续改造和正常证据，但没有原Err可选择其中哪条作为必要原因。因此不存在足以宣称“原失败全部存在基础消失”的删除证明，也没有证据将W05-04归因为内核、已修的C11或某项当前代码bug。

本轮逐一重读P4固定源码的40份release guest：每份均完成三次独立Watch合同（合计120次成功标记）、无Watch panic，并完成对应Copy与双provider退款。最终C11的5份guest另各有三次Watch合同（合计15次），正常/压力/release及平台变化均继续执行；原六轮矩阵仍按其当时源码保留。此为现有运行证据复核，不是新复现矩阵，也不能用有限次成功排除低概率错误。C09非零目标accepted后取消门是后加场景，只由最终基线承担，未追加到P4的40次覆盖中。

**W05-04 当前处置（2026-10-27）。** 用户决定与平台时钟异常一并登记为 [KNOWN_ISSUES 历史偶发异常](../../KNOWN_ISSUES.md#历史偶发异常仅观察)，统一拥有观察状态与重开条件；不复发就不继续调查，不阻塞当前工作，不因普通源码/平台/限额变化重启历史追查。此前无复发时继续跑矩阵或专门补证的安排不再作为当前任务，本篇保留原始证据和复发后的处置责任。

**W05-04 复发后的处置与关闭依据。** 当前主代理/后续本计划维护者拥有唯一处置责任；相同panic或Watch准入非Quota反例再次出现时，保留现有失败分支输出的具体Err、成功count、last_ns、请求deadline，并保存实际来源/控制Lifetime退休与费用序列，首先区分单次RPC过期、其他资源/分配失败、协议/运输错误及退款顺序。对可定位的当前机制修复并以对应反例验证后关闭；若以后其他任务整体替换该准入责任链，可凭旧必要条件删除与保留行为消费者的证明重新核定，不为此单独启动调查。单纯达到更多无失败次数不构成关闭依据，不恢复旧公共接口、不放宽断言或增加假provider/新测试运行体。

**本轮收口。** 旧Copy条目从COMPASS待办移除，原记录归档、链接同步；W05-04原始未知及精确注入缺口继续保留。本次无源码变化，impls不改；结构检查核对关闭依据、原日志时点/阶段、旧路径搜索、分版本覆盖与导航，不将普通运行升格为故障注入。文档检查覆盖6份逻辑文档的107个当前/新增本地链接及标题锚点、当前全文代码围栏，全部通过；3份旧档案只检查本次迁移的缺陷链接，历史删除源码引用不改写。`git diff --check`通过，未commit/push。
