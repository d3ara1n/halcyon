# 第八至第九轮 OAL、fs 与服务目录设计快照

2026-10-09 完整设计整理前的只读快照。以下正文保留当时的讨论、未决项及证据，不作为施工目标。唯一活跃入口为[重构计划](../todo-2026-10-07-fal-service-redesign.md)。相对链接仅按归档位置调整。

原正文 SHA-256：`01408ffbcfb536e19f801fced17d1d98d1849661e21e503d85f8f862a752c0d0`。

---

# OAL、fs 与服务目录改造计划

状态：**第八轮 OAL 与入口分层已确认；第九轮服务目录/provider/libservice 重构方案已成稿待用户审阅，跨文件服务 Move/Copy 取证已完成、fs 语义边界待讨论。完整设计 Review 尚未完成，Rust 改造未开始。** 本篇是原 FAL 拆分的唯一活跃记录。OAL 决定见[第八轮设计修订](#第八轮设计修订)，本轮服务方案见[服务目录](#服务目录)，责任与完成见[注册提交与完成状态](#注册提交与完成状态)，接力见[当前接力](#当前接力)。用户确认、设计结论、独立审查和实现验收分别记录。

第一至第七轮讨论、被替代方案及当时证据已存入[只读设计档案](ref-2026-10-07-oal-fs-service-design-rounds.md)。档案中的“当前”“下一步”不再作为施工入口；本篇独立说明现行方案、未闭合事项、实施与验收。归档的是讨论快照，不是尚未完成的改造任务。

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

本轮于 2026-10-09 核对分支 `refactor/system-redesign`，HEAD `6187ce194b26881076f0eba209f997b808fd0c3a`；工作树原有本篇的第八轮设计修改，全部保留并在其上继续。初始 `183d25c`、先前整理 `70bb29b` 等基线见历史记录。本轮只更新设计与参考文档，不改运行代码、ideas 或 impls。

代码与[当前 FAL 实现说明](../../notes/impls/fal.md)描述旧实现；[KNOWN_ISSUES](../../KNOWN_ISSUES.md#矫正区)已登记矫正范围，不把旧结构当新方案的正确性依据。现存 RPC、Runnel、Process/Job 和 SourceRegistration 已有实现基础，本案不重新启动已完成的对象链或运行时分层任务。

本案从既有 FAL/fs 的协议和 backing 实现做职责分离。库定义契约与可复用代码，实际实例由 provider 持有；不能把“代码位于某库”解释成“运行时多一个拥有对象真值的层”。共同 PathHeader、OAL 通用 Continue 业务封装、框架先定位再调用领域 handler、强制 Locate 前置均撤出当前方案。CreateOpen、共享流、Copy 保留部分结果等仍只是 fs/backing 的领域设计，不是 OAL 的职责。

当前共享运输常量为 `PAYLOAD_MAX = 4096`、`MESSAGE_HANDLE_MAX = 8`，来源是 [message.rs](../../shared/erhino_shared/src/message.rs)，它们是现状依据而非在新库复制的常量。旧 `PATH_MAX = 512`、链接/步数限额不能未经论证照搬。完整报文必须计入 RpcPrefix、路径/领域头、参数、响应和实际能力槽。

## D0 设计收口

D0 由本案承接设计与工程准备。当前 OAL 原则与 DirectoryGrant 分层已经用户确认，没有新的 OAL 架构选择等待用户再次批准；Trait/tag 编码、标识布局、方法签名、模块迁移和预算计算由实施者按已定语义收口。只在出现无法据现有决定闭合的外部行为或范围变化时再讨论。fs/service 初稿仍须按后续领域讨论和设计 Review 复核，不以 OAL 方向确认代替完整施工就绪。

| 编号 | 设计内容 | 已形成的待审查产物 | 实施依赖 |
|---|---|---|---|
| D0.1 既有知识与工具的拆分 | 类型、遍历算法、操作封装及提供端支持的归属 | [源码迁移映射](#源码迁移映射)；保留库中的有用实现，实例归真实 provider | P1 及其后全部 |
| D0.2 线形与错误 | 独立协议、路径参数、摘要、能力槽、长度和结果 | [路径接续细则](#路径接续细则)、[线形与操作表](#线形与操作表)；没有 OAL 业务信封或强制 Lookup 前置 | P1—P3 的两端迁移 |
| D0.3 backing 授权、容量与清理承载 | 权限、付款、准入、工作界与退休 | [实际授权与资源责任](#实际授权与资源责任)；复用真实账户、来源登记和退款机制 | P1—P3 的资源路径 |
| D0.4 fs 完成协议 | 部分结果、流、创建、取消及订阅 | [文件提交与完成状态](#文件提交与完成状态)；文件解绑、业务终态与资源释放分开 | P2 |
| D0.5 服务完成协议 | 发布替换、期限、回复丢失、入口关闭和名字观察 | [注册提交与完成状态](#注册提交与完成状态)；名字索引无额外文件投影，控制者不能撤出替代者 | P3 |

D0 初稿的五组产物按当前讨论持续修订。第八轮用户确认了 OAL 面貌模型、通用目录与入口分层；第九轮服务目录方案已结合真实消费者重写，采用单步 Register、Active/Withdrawn、显式 GetEntry 和独立 registry owner，外部语义仍待用户审阅。fs Move/Copy 本轮先作外部取证，不因其它系统行为自动扩充本案承诺。修订和完整设计 Review 通过后才关闭 D0、进入 P1。

## 第八轮设计修订

本轮用户明确了以下模型；先前把 Trait 写成“形状与接口契约”、要求证明组合相容，或要求决定 ObjectInfo 固定/动态形状的解释均撤销。

- Trait 只是数字标记，具体整数、枚举或命名空间表示由后续实现确定。它的含义只存在于理解它的 provider 和消费者领域代码中，OAL 不理解其含义，也不据此生成操作或检查 provider 是否兑现声明。
- 消费者可根据 `Property+Integer+Array` 选择 `Get<Vec<usize>>`；`T` 只指导客户端解码和范围检查，不进入 Get 请求。provider 按 Get 操作返回自己提供的值，不按消费者未发送的期望类型选择不同返回形状。同一对象仍可同时被 fs、person、music_control 等不同协议/操作解释；撤销“同一 Get 按不同 T 分别产生整数/字符串数组”的扩展承诺。
- 未知 Trait 直接忽略；认识但当前用法不关心的 Trait 同样可忽略。`Property+Boolean+mime::Mp3` 可以按布尔属性使用；OAL 和 fs 不验证 MIME 声明是否合理。`RegularFile+Property+Integer+person::Age+person::Person` 可由 fs 和 person 消费者按各自知识调用，解释不经过 OAL。
- music_player 可把歌单暴露为目录，歌曲返回文件、MIME 和 music_control 的混合标记。fs 消费者可 Open 字节流，认识 MIME 的消费者可解码 MP3，music_control 消费者可直接调用播放协议；不要求程序根据全部 Trait 动态发现并调整业务策略。Song/Category 等标记间是否矛盾由 provider 与相应消费者负责，不属于 OAL 的组合校验。
- Trait、tag 和 ObjectInfo 是 backing 的统一面貌视图。ObjectInfo 在观测时生成，像照片；后端变化自然产生不同照片，无须为此设计 ObjectInfo 的变更状态机。每次枚举/Lookup 都产生随机但编码有效的视图也不违反 OAL 的面貌表达模型，OAL 不保证跨次稳定或声明真实。领域请求、真实能力和运输仍各按自身规则执行。
- OAL 保留原 FAL 的树状发现、用户态声明、provider、目录入口传递与 Boundary 使用。Directory 归 OAL，节点的文件/人员/歌曲等领域解释归双方领域代码。库不维护 OAL 树，不等于删除客户端使用树状视图所需的公共工具。

用户已确认采用通用接续，保留 FAL 的两种边界行为：ProviderBoundary（现有 Delegate）交付真实 Endpoint 能力及剩余路径，LinkBoundary 改写路径而不交付新入口。后者推广为通用路径转发，不要求背后存在 fs 符号链接；music_player 的 `Latest-Releases/Song` 转向 `Lady-Gaga-2026/Song` 同样适用。二者可连续组合，换路径后可能遇到换入口，换入口后也可能再次转发；当前目录发现模型不需要第三种领域边界。

当前代码事实：`libfal::protocol::Response` 为已知变体枚举，Lookup 在线形中用 0/1/2 区分 Node/Delegate/LinkBoundary，未知值返回 DecodeError，`Client::validate_reply` 映射为 ClientError::Protocol。Delegate 要求一个回复 capability，LinkBoundary 不带 capability；RPC 不解释领域变体。新 OAL 保留这种公共 tagged union 结构，领域 provider 将自己的内部原因表达成两种已知行为，无须让 OAL 枚举引入第三方类型。

两种边界的解释及 walk 归客户端。Boundary 是接续控制结果，不是 object，也不增加面向应用的“观察 Boundary 对象”需求。Lookup 中途遇到链接并仍有后缀时返回 LinkBoundary；末项符号链接直接返回含 fs::SymbolicLink 的 ObjectInfo，需要查看链接内容时调用 fs 操作。`Latest-Releases` 是专辑目录的别名，末项 Lookup 返回该 provider 暴露的专辑/目录面貌，查询其子项才可能产生转发。ProviderBoundary 交付 Endpoint 后由客户端继续到应答 provider，保留可在入口根继续的空剩余路径。相对目标以转发位置父目录为基点，视图绝对目标从客户端视图根重启。

旧代码与目标须区分：当前 MemoryBackend::lookup 在末项也产生 LinkBoundary，内含节点引用；libfs Transport::lookup 在 NoFollowFinal 且剩余路径为空时把其节点摘要转换为 Found。用户描述的对外使用方向保留，新 OAL 直接表达末项 ObjectInfo，不要求 OAL 理解 fs::SymbolicLink 来完成这次转换。

此前开放领域边界封装的候选不采用，不增加未知边界的 opaque payload、领域解码器注册或插件式跟随分派。Trait 仍是可忽略的开放标记；Boundary 是 OAL 定义的公共控制结果，两者的未知值处理不要求相同。该决定只涉及发现接续，不恢复 OAL 通用业务执行信封。

“名字”是已有目录项/路径分量，不是本轮新增对象模型。现存 `libfal::protocol::DirectoryEntry` 已含 `name` 和 `info`；旧 Move 请求也使用源/目标名字。其通用发现表达归 OAL，文件改名/移动的具体行为以后在 fs 设计中处理。用户要求先理清上述 OAL 模型，Copy/Move、文件引用/身份、快照及持久化不在本轮继续展开。

本轮撤销混合 tag 的旧 ObjectInfo 表示，确认两种通用 Boundary、客户端 walk、末项对象视图和下述 DirectoryGrant 分层；同一 Get 不传期望返回形状，保留跨协议/操作的多种用法。剩余编码和迁移细化由实施者按这些原则完成。当前只修订设计记录，不声称完整设计 Review 或实现验收已完成。

### 入口、路径与授权分层（用户已确认）

用户要求重新设计 DirectoryGrant 的使用与知识分层。本轮源码事实：`libfs::prefix::DirectoryGrant<G>` 只有 endpoint 字段，正式 Grant 是 `DirectoryGrant<Arc<MailboxSender>>`；复杂度来自周边 prefix/Position/NodeSummary、先 Lookup 再操作、派生与文件权限知识的耦合。真实授权在 provider 的 GrantTable，以内核 sender context 对应 root、rights 和账户；客户端包装本身不产生隔离。旧 Move 等也消费真实目标目录能力，不能只因简化 walk 就删除这种业务授权需求。

用户已确认以下分层；源码身份和具体方法签名由实施者据此确定：

| owner | 拥有的知识与状态 | 普通消费者接触面 |
|---|---|---|
| libkernel / librpc | 沿既有机制持有、复制、运输和调用真实 Endpoint；RPC 不解释目录/文件 | 采用现有 typed sender owner，不另建同义授权包装 |
| liboal | Namespace 的路径前缀→入口映射、两种 Boundary、当前请求的 walk 帧/相对路径/预算、Lookup/Enumerate | 绑定入口并传入路径；不要求手工构造 DirectoryGrant、Position 或先读 ObjectInfo |
| libfs / libservice / 其它领域库 | 自己的请求编码、返回解码及领域操作；收到公共 Boundary 时复用 OAL 路径推进工具 | `fs::get<T>(namespace, path)`、`music_control::play(namespace, path)` 等领域 API（仅伪代码） |
| 实际 provider | 将接收入口/context 解释成真实访问范围、领域权限、backing 及付款来源；决定回复的下游入口 | 需要额外授权时显式取得真实入口，普通 walk 不暴露授权表管理 |

普通路径调用以 Namespace 和路径开始，领域客户端直接发送领域请求；收到 Boundary 才让 OAL 的 walk 状态更新入口/路径，然后由领域客户端继续调用。OAL 的工具只处理地址和两种控制结果，不接收通用业务信封、不识别领域 payload、不强制提前 Lookup。显式观察对象时才走 OAL Lookup。

目录进入只改变当前请求的路径位置；跨 provider 时采用边界已经交付的真实入口，不要求应用额外调用 Derive 或重新包装 DirectoryGrant。provider 可以交付已满足当前授权的入口副本，确需不同访问范围时由真实发行者签发；不承诺取消所有服务端派生或状态。客户端本地路径前缀不构成对子进程的隔离。

将目录范围收窄后交给另一进程，以及 Move 等需要稳定目录 authority 的业务仍由相应授权操作提供真实能力；领域客户端可在自己的操作封装内取得、运输和释放这些能力，普通路径调用者不管理 grant。具体领域操作签名随 fs/service 收口，不能把任意入口加本地路径宣称为受限目录，也不通过简单重命名 DirectoryGrant 保留原有调用负担。

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

核实依据：现有 [文件后端](../../user/libraries/libfal/src/backend.rs) 的 lookup/resolve 共用 lookup_child；[目录枚举项](../../user/libraries/libfal/src/protocol.rs) 已有 DirectoryEntry.name/info；[客户端代码](../../user/libraries/libfs/src/resolve.rs) 的 Found/Delegate/Link、prefix 和 Position 承载现有路径接续。通用换入口、相对/绝对转发及路径上下文由 OAL 承接，解除对文件 NodeKind/NodeSummary 的依赖；fs 保留符号链接内容读取、创建等领域操作。现有 RPC 和 Move 准入仅作既有运输/授权事实。

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

### 路径请求与领域接续

Lookup、Read、Open、GetEntry 分别是各协议的请求，直接交给实现它的 provider 方法。已经知道领域的调用者可以直接发领域请求，不要求先握手或先取得摘要；provider 在本次操作中验证实际目标。OAL 发现帮助未知调用者认识对象，不成为所有领域操作的必经执行层。

在同一进程/入口复用接收设施时，可以根据协议标识调用不同的 Provider 实现；也可以按实际授权角色设置不同入口。这属于具体 provider 的装配和 RPC 分派，不引入统一 PathHeader 或 OAL 的 `Done/Continue(domain_response)` 业务信封。

文件操作遇到跨 provider 情况时如何继续、是否内部转发、哪些结果可以重试，由 fs 协议及其具体实现负责；服务和播放器同理。领域可以复用 OAL 的 Boundary 表达，但 OAL 不接管其请求、能力输入或副作用。本案 fs 的接续结果只发生在领域操作提交前，具体合同见路径接续细则。

### 发现操作与目录枚举

liboal 的提供端接口以 Lookup/Enumerate 为基础。Lookup 的结果为 `Found(ObjectInfo)` 或 `Boundary`，失败返回协议错误；具体实现可内部查询/遍历 backing 后生成结果，也可直接计算这个视图。没有 Locate、通用前置遍历或定位后统一执行业务的要求。

Boundary 保留换入口和路径转发两种通用表达，客户端负责跟随，不把 Boundary 当作 object。Lookup 中途 LinkBoundary 用于继续未完成路径；末项符号链接返回其 ObjectInfo，fs::ReadLink 等操作按领域协议读取实际链接。专辑别名的末项视图由播放器直接生成，OAL 不把别名强制解释成符号链接。ProviderBoundary 仍可换入口后查询目标入口根；最终应答 provider 生成 ObjectInfo，不合并沿途 Trait/tag。

枚举由 provider 对自己的 backing 执行，返回子项名字和 ObjectInfo，必要时表达边界项。分页游标由 backing 解释；OAL 只定义请求/响应及完整、结束、失效和过大摘要等结果。目录变化不默认生成每个消费者的快照或镜像树。客户端与提供端的响应/查询预算沿各自请求生命周期落实，不以通用走路状态机代替。

### ObjectInfo、Trait 与 tag

ObjectInfo 是 provider 在 Lookup/Enumerate 时生成的面貌描述，包含 Trait 标记和 tag 属性；它不是持久对象、统一对象存储或接口契约注册表。创建和解码可以检查报文边界、长度等表示完整性，不检查领域标记的组合是否合法，也不验证面貌是否真实。没有可靠时间等属性的 provider 可省略相应 tag；OAL 不为补全视图创造 backing 状态。

Trait 是可比较的数字标记，不含 OAL 解释的操作清单、互斥类型规则或组合约束。标记定义由各领域库拥有；客户端是否理解某标记取决于引入的领域知识。消费者按当前用法匹配所需子集，未知或无关标记不影响匹配，不要求 Trait 集合恰好等于某个封闭对象类型。数字标识的布局尚待确定，不能把 Rust 闭合枚举变成接收端拒绝未知领域标记的理由。

OAL 允许承载 `Property+Integer+Array+Boolean+String` 等任意标记组合，但这不使无期望形状参数的 fs Get 自动获得多种返回值分派能力。Get 返回 provider 实际提供的值，`Get<T>` 在消费者处解码或转换，不相容时返回类型/范围错误。provider 须避免宣称其 Get 实际无法提供的相矛盾形状；OAL 不阻止、证明或修复虚假声明。`Property+Boolean+mime::Mp3` 同样不触发 OAL/fs 的 MIME 合法性检查。不同协议/操作各自拥有的返回值仍可并存。

tag 表达创建时间、长度等带值属性，和 Trait 一起组成照片。解释 tag 的值同样需要相应知识；ObjectInfo 自身不提供通过修改 tag 改写 backing 的通用操作。Directory 属于 OAL 的树状发现表达；文件、服务、MIME、播放器等领域分别拥有自己的标记与属性。

观察不会固定对象的形状、名字、接口可用性或实际内容。provider 可以动态计算甚至随机产生视图，已知消费者也可以不经 Lookup 直接发领域请求；provider 收到后执行或返回错误。provider 对业务数据与行为负责，OAL 不以摘要构造器、全域形状验证器或发现握手为其背书。

本轮只确认该语义模型；旧的统一 tag 条目格式与固定编号撤销，Trait/tag 的具体线上表示待闭合。完整 ObjectInfo 仍须适合有界响应，过大的领域正文通过领域协议取得；这一运输边界不产生领域形状约束。

### fs 基础接口

libfs 提供文件领域的类型、接口形状、编解码、文件操作封装与可复用实现，通用 Directory 与树状发现使用归 OAL。以下接口与行为表是当前内存文件 provider 的待复核迁移初稿；共享、原子性、会话、值存储或清理政策不自动成为所有引入 libfs 的 provider 的统一选择。Get 请求保留路径参数，不传客户端 T 或期望形状，T 仅用于返回值解码；撤销因同一 Get 返回形状多态而扩大请求的要求。用户要求先闭合 OAL 模型，其余 fs 操作设计暂缓。

| 操作 | 明确语义 |
|---|---|
| `Get<T>` / `Set<T>` | 完整读取/替换有类型 Property；检查本次实际目标。Set 可改变 Property 值形状，随后的摘要自然改变；不是修改 tag |
| ReadAt/WriteAt | 对 Stream 在明确偏移执行有界字节访问；返回实际字节数，支持短读/短写；普通 Write 不隐式创建或截断 |
| Open | 按当前路径建立指定方向/范围的文件流会话，返回控制入口和 Runnel offer；不带先前 Lookup 的 expected_identity |
| Create | 在已定位父目录中独占创建 Directory、Property、Stream 或 SymbolicLink；名字已存在报 Exists |
| CreateOpen | 独占创建 Stream 并在同次成功结果取得其流会话；用于避免 Copy 在创建与按名打开之间操作到别人替换的文件 |
| Remove/Rename | 对实际目录名字操作；非空目录不递归删除。Rename 校验两个真实目录 grant，在同一 provider 原子提交；跨 provider 返回 CrossProvider，不暗中 Copy/Delete |
| ReadLink | 在 NoFollowFinal 下读取链接目标字节，属于 fs 的链接操作；不要求调用者通过摘要读取权取得目标 |
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

发送 CreateOpen 后丢失结果时报告创建结果未知，不自动重试、按名字清理或宣称回滚。这个选择替代旧 CreatedTarget 的默认条件删除路径；它不需要跨 Lookup/Get 身份匹配。是否另提供显式临时文件/原子发布由独立文件需求触发，不是当前 Copy 成功或失败责任的缺口。

### 跨 provider 文件操作取证

2026-10-09 已完成[微内核及 POSIX/Linux 对照取证](../ref-2026-10-09-cross-provider-file-operations.md)：QNX、Hurd、Genode、MINIX 的公开 rename/move 机制分别受 filesystem/device、translator 视图、filesystem instance 或 FS endpoint 限制；POSIX mv 与 GNU 工具在跨文件系统时组合复制和删除。Linux 同一底层文件系统的不同 mount 也可能返回 EXDEV。取证未运行这些系统，不把个别后端或工具行为推广为全部实现。

fs 后续设计据此区分可共同提交名字变更的后端范围、跨范围的复制/删除组合，以及独立的数据复制加速。当前初稿的“同 provider 原子 Rename / 跨 provider 返回 CrossProvider”仍需按实际 backing 边界复核，不能将同一进程自动当作原子域；也不因跨进程就增设通用分布式事务。是否提供高层跨 provider Move、目录/属性复制和部分结果政策仍待 fs 讨论，本轮不修改代码或宣告能力完成。


### 服务目录

本节为第九轮服务目录重构设计结论，依据已确认的 OAL 模型和当前源码重新规划；单步注册、服务条目为叶对象及客户端选择协议属于本轮提出的外部语义，待用户审阅，不冒充此前已确认。详细完成与失败合同见[注册提交与完成状态](#注册提交与完成状态)。本轮只改计划，不开始 Rust 施工。

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
| Register(CreateOnly/Replace) | 发布者在业务就绪后提供名字、协议描述与可重复共享的真实入口；一次提交名字可见并返回 RegistrationControl。CreateOnly 要求空位；Replace 需替换权，可在空位创建或原子替换现有注册 |
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

### 路径接续细则

单次线上路径为 UTF-8 相对路径，空串表示入口根；`/` 分隔，不含 NUL、空段、`.`、`..`，不以 `/` 起止。名字为一个非空分量。`*`、`?`、反斜杠没有通配或转义作用，不沿用旧 FAL 对这些普通字符的额外禁令。长度取决于本次完整报文的可用字节，不再额外规定 512 字节上限。库不在解码时访问对象或推断领域类型。

Boundary 只表达下一步如何继续发现，保留两种公共形式。ProviderBoundary 对应旧 Delegate；LinkBoundary 对应旧 Link，推广为通用路径转发。两者均由 OAL 解释，具体 provider 决定何时返回；不根据 fs、播放器或其它领域标记分派第三方边界。以下字段沿用原初稿的路径覆盖表达，具体编解码随整体协议收口复核。

| 形式 | 字段与能力 | 使用规则 |
|---|---|---|
| ProviderBoundary | `consumed_bytes:u32, remaining:Text`，回复 slot 0 为新入口 | consumed 截止在原路径分量末尾且非零，前缀和 remaining 连续覆盖原路径；目标从已交付入口的授权根处理 remaining。空 remaining 合法；空路径不能继续换入口 |
| LinkBoundary | `parent_bytes:u32, target:Text, remaining:Text`，无能力 | parent 为转发分量父前缀，remaining 为未完成后缀；相对 target 从该父位置展开，视图绝对 target 从客户端视图根重启，再拼接 remaining。Lookup 中的 LinkBoundary 用于中途接续，末项返回 ObjectInfo；具体领域操作的转发按其协议决定。`.`/`..` 由客户端路径上下文处理 |

OAL 的客户端 walk 持有 prefix、入口、相对/绝对转发、`.`/`..` 和跨入口返回上下文。消费者无需引入 fs 才能跟随 LinkBoundary。查询 `Latest-Releases/Song` 时，播放器可返回 parent 为空、target=`Lady-Gaga-2026`、remaining=`Song`，客户端在播放器入口根下继续；直接 Lookup `Latest-Releases` 则返回专辑/目录 ObjectInfo。末项符号链接同样返回 ObjectInfo，不引入观察 Boundary 本体的对象接口。转发与换入口可交替，循环由已有请求/展开/期限预算终止。

fs 中所有单目标路径操作的响应明确包含 `Boundary` 分支，且只可在尚未创建、修改、建立流/订阅或交付操作结果时产生。ReadAt 的 Boundary 也不能夹带已读取的数据。provider 的 handler 使用同一 backing 遍历接口，在本次请求里得到本地目标或边界；本地目标直接执行，不再重做一次外部 Lookup。fs 客户端收到明确 Boundary 后，以新 txid 向目标发送同一领域操作及改写后的路径；已投递后的超时、损坏回复和失联不等价于 Boundary，不自动重放变更。

带输入能力的 Rename 采用现有“两份真实目录授权”结构：客户端先用 **fs DeriveDirectory** 取得目标父目录 grant，再向源入口发送源路径、目标单个名字及该 grant 的临时副本。源 provider 在一次提交中校验两个目录及本次源项；跨 provider 返回 CrossProvider。这不是 OAL 定位前置，也不宣称两次调用之间目标父目录名字不变：派生 grant 明确选择了目录，之后按该目录操作。源路径遇边界可接续；每次运输使用调用者保留 grant 的临时副本，旧请求负责清理收到的副本，不能将唯一输入移走后盲目重发。

边界安装由实际宿主配置并持有真实目标入口；ObjectInfo 的 Directory 标记不产生权限。通用树状装配与目录入口传递归 OAL 使用范畴，真实授权由入口发行者兑现；原来把全部 route Bind 迁入 fs-only 管理支持的映射待复核。`/System/Services` 的发现内容由实际 registry 生成，宿主只持装配关系；绑定替换/撤出改变后续选择，已交付入口仍按发行者规则存活。具体 Bind、fs 权限衰减与跨领域装配表示，按已确认的两种公共 Boundary 和目录入口授权归属同步下方操作表。

客户端的接续有显式 `FollowBudget { calls, expanded_bytes, deadline }`，在每次请求和链接展开前扣除；默认值来自调用方配置的请求/内存预算，不复制旧 40/256 魔数为协议限额。耗尽返回本地 BudgetExceeded 并清理临时入口。provider 每次只处理一个报文允许的路径长度，远端 Derive 使用原始调用的剩余期限；不得在每跳重新获得无限时间。内部转发的 provider 同样持有自己的有限任务预算及下游请求 owner。没有跨进程全局路径锁。

### 线形与操作表

以下是目标线形，不是旧源码已经实现的 ABI。每个协议拥有自己的 Header/Request/Response；共享的是 librpc 的 LE 字节工具、RpcPrefix，以及 liboal 的摘要/Boundary 数据类型。相同头布局不引入统一业务 dispatcher 或通用 PathHeader。

| 协议 | 消息 kind / 版本 | 接收入口 |
|---|---|---|
| OAL | `0x4f41_4c31` / 1 | 实际 provider 暴露发现的入口 |
| fs | `0x4653_3031` / 1 | 文件目录 grant、流/订阅控制入口 |
| service | `0x5352_5632_5245_4701` / 2 | 独立 registry 的目录访问、发布 authority、注册/订阅控制入口 |
| 文件宿主管理 | `0x4653_3031_5254_4501` / 1 | 独立管理 sender；沿用 Bind 的领域化配置作用 |

文件 provider 的 mailbox 接受 OAL/fs，registry 的独立 mailbox 接受 OAL/service；各自从内核 sender context 查询真实角色与访问范围。换消息 kind 不产生额外权限。二者可由同一 Runtime 调度，但 registry 的目录访问、发布和控制记录不进入文件 GrantTable，不按文件 NodeRef 分派。这里只复用现有消息运输。

宿主管理 Bind/Unbind 的现有 fs 线形暂作为文件装配初稿，后续随 fs 审阅收口。服务目录的首轮接入明确使用客户端 Namespace.bind：srv_init 分别交付真实文件入口和 registry 目录入口，消费者在本地绑定 `/System/Services`；不要求文件宿主为该路径接收或代理 service/未知领域请求。绑定只保存入口引用，不产生权限，解除绑定不撤销已经交付给其它 owner 的能力。

RpcPrefix 保持 16 字节；随后各协议头均为 `version:u16, op:u16, status:u32, body_len:u32, reserved:u32=0, Deadline:16`，共 32 字节。请求 status=0；回复回显 op、txid 和请求期限。op 先按原始 u16 读取，以便对未知操作返回 Unsupported。请求 slot 0 固定为 librpc 回复权；下表请求业务槽从 1 开始，回复槽从 0 开始。不使用的槽必须为空，错误回复不带能力。

本文使用 `Text/Bytes = len:u32 + len 个字节`，无隐含对齐；Text 必须合法 UTF-8。`OptionalU64 = present:u32 + value:u64`，present 只能是 0/1，缺省时 value=0。Deadline 直接使用 shared 的既有 16 字节编码及合法性校验。所有加法、乘法、偏移和 u64→usize 转换做溢出/范围检查。

ObjectInfo 的旧统一 tag 线形和首批 local_id 表已撤销。新表示须区分纯数字 Trait 与带值 tag，支持未知标记且不验证领域组合；具体布局、命名空间标识与编号仍待协议收口，本轮不冻结新的编码表。Boundary 两种公共形式已确认，不再把第三方领域边界选择作为该表示的前置。

面貌示例：目录可带 OAL Directory；内存属性可带 fs RegularFile/Property 及其值形状标记；歌曲可同时带 fs Stream、mime::Mp3 和 music_control 标记。它们只是 provider 输出视图的示例，不是 OAL 的 backing 类型映射表；同一对象可支持多个领域协议/操作，但同一个 Get 不按未发送的客户端 T 选择返回形状。服务入口等真实能力通过对应领域操作取得，面貌本身不产生能力。

`ProtocolDescriptor = id:u64, min_version:u16, max_version:u16, addressing:u32`，addressing=0 表示无路径领域入口、1 表示入口根相对寻址；列表编码为 count:u32 加固定 16 字节条目，同 id 不重复，版本区间非空。它是 service 领域的发布描述，不是 OAL Trait 的接口证明。Register 冻结描述，GetEntry 原样返回同一注册的完整描述；消费方在本地按领域知识选择相交版本，registry 不接受返回值期望或执行业务协商。

| OAL op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 Lookup | path:Text | result:u32，0=ObjectInfo、1=ProviderBoundary、2=LinkBoundary；仅 ProviderBoundary 回复槽 0 |
| 2 Enumerate | path:Text, cursor:Bytes, max_entries:u32 | result:u32；0=Page，1/2=Boundary。Page 为 next_cursor:Bytes, done:u32, count:u32，随后重复 name:Text + info:Bytes；Page 无业务能力，ProviderBoundary 带回复槽 0 |

枚举初始 cursor 为空，游标由实际 provider 解释，OAL 不保存扫描会话。内存文件 provider 的 epoch/index 游标仍属于 fs 待复核初稿。registry 则按服务名字字节序分页，cursor 为 `{registry_incarnation:u64, after_name:Text}`；每次仍按真实 DirectoryAccess 范围筛选，游标数字不产生权限。Root 从 after_name 之后取当前 Active 条目，ExactName 只查该名字；跨实例游标返回 CursorInvalid。静态目录完整且不重复；并发插入在游标之前的名字可能不在此次遍历出现，删除/替换按该次请求观察，不承诺跨页快照。无需为每次摘要变化维护全局 epoch 或预留未来撤名序号。装不下下一项则返回已有整项及游标；空页仍装不下单项返回 EntryTooLarge。registry 叶条目无能力槽；Endpoint 只经 GetEntry 取得。

| fs op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 Get、2 Set | path:Text；Set 追加 value:Bytes | Get 返回 value:Bytes；Set 为空 |
| 3 ReadAt、4 WriteAt | path:Text, offset:u64；ReadAt 追加 count:u32，WriteAt 追加 data:Bytes | `IoResult { count:u64, end:u32, cause:u32, data:Bytes }`；只有 ReadAt 含 data |
| 5 Open、6 CreateOpen | path:Text, direction:u32, offset:u64, length:OptionalU64, session_deadline:Deadline, tunnel_bytes:u32；CreateOpen 再加初始 FsRights:u32 | StreamOffer；回复槽 0=控制 sender、1=Runnel invitation |
| 7 Create | path:Text, kind:u32, rights:u32, initial:Bytes | 空；kind=1 目录、2 Property、3 Stream、4 SymbolicLink。目录/Stream 的 initial 为空，Property 是值编码，链接是 UTF-8 target |
| 8 Remove、10 ReadLink、12 Sync | path:Text | Remove/Sync 空；ReadLink 为 target:Text |
| 9 Rename | source:Text, destination_name:Text, replace:u32 | 空；请求槽 1=目标父目录 grant；replace 只取 0/1 |
| 11 SetLength | path:Text, length:u64 | 空 |
| 13 DeriveDirectory | path:Text, rights:u32, sender_transport:u32, output_transport:u32 | 空；回复槽 0=实际收窄后的目录 grant |
| 14 Watch | path:Text, mask:u64 | WatchInfo；回复槽 0=独立控制 sender、1=通知 waiter |
| 15 Start、16 QueryStream、17 Finish、18 Cancel | 空，直接发到流控制入口 | StreamInfo，不含能力 |
| 19 QueryWatch、21 CancelWatch | 空，直接发到订阅控制入口 | WatchInfo |
| 20 AckWatch | sequence:u64 | WatchInfo |

fs 路径操作成功体前均有 result:u32，0=表中结果，1/2=Boundary；控制操作没有该字段。Create/Remove/Rename/ReadLink 不跟随末项链接，其余路径操作跟随；创建操作只解析父路径。CreateOpen 固定创建空 Stream 并以 Write 方向打开，不允许借这个入口创建只读流；结果丢失语义见下文。Rename replace=1 可替换兼容种类，目录必须为空，目录不能移入自身子树；不兼容类型返回 TypeMismatch。同一目录同一名字是成功的无操作。

Open 的 direction=1 Read/2 Write，offset 是绝对文件偏移。Read 的末端为 `max(offset, min(Open 时文件长度, 可选的 offset+length 上界))`，offset 超过 EOF 时是空读流；Write 初版要求 length 缺省，按 EOF 结束，CreateOpen 固定 offset=0。tunnel_bytes=0 采用宿主当前 Runnel 配置，非零值须满足既有 Runnel 布局及额度；这不是文件数据的最大长度。有限 offer_deadline 取请求剩余期限、宿主 offer 配置期限及有限 session_deadline 的最早者。每个 provider 可以不支持某种 Open 形态并明确返回 Unsupported，不能成功回复后悄悄改变所承诺的方向/范围。

Property 编码为 `scalar:u32, array:u32, count:u32, payload`；scalar=1 Bool、2 Signed64、3 Unsigned64、4 Float64、5 String、6 Blob，array 只取 0/1，标量 count 必须为 1。数值逐项定宽 LE，Bool 每项一字节且仅 0/1；String/Blob 每项是 Text/Bytes。空数组保留 scalar 标识。Float64 保留 IEEE 位模式；`usize` 的转换属于 libfs，负数和越界报 Range，不把平台 usize 写上线形。Get 不要求请求方提供先前摘要。

StreamOffer 复用现有 48 字节布局（session_id、direction、tunnel_bytes、start、read_end、offer_deadline），session_id 仅关联实际会话，不是 OAL 对象身份；StreamInfo 复用现有 40 字节布局（session_id、state、outcome、reason、reserved、accepted、transported）。状态/原因按现有枚举迁移，增加内部 Stopping 不增 wire 状态。Runnel 仅支持当前 v2，类型本身已表明运输协议；不再携带无实际选择的 stream_protocol 字段。WatchInfo 为 `sequence:u64, pending:u64, state:u32, reason:u32`，state=1 Active/2 Terminal；Ack 未来 sequence 返回 Invalid，旧 sequence 无副作用地返回当前状态。

| service op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 DelegateName | name:Text, publish_rights:u32 | 空；回复槽 0=限定名字的发布 authority |
| 2 Register | name:Text, mode:u32, descriptors:Bytes, export_transport:u32；Header Deadline 必须有限 | RegistrationInfo（提交快照）；请求槽 1=已允许共享的 endpoint，回复槽 0=RegistrationControl |
| 3 GetEntry | path:Text | result:u32；0=`RegistrationId + descriptors:Bytes`、1/2=Boundary；成功取得或 ProviderBoundary 的回复槽 0=实际入口 |
| 4 Withdraw、5 QueryRegistration | 空，发到注册控制入口 | 当前 RegistrationInfo，不带能力 |
| 6 WatchDirectory、7 WatchName | directory_path:Text；WatchName 追加 name:Text | result:u32 + WatchInfo 或 Boundary；建立成功槽 0=控制、1=通知 waiter，ProviderBoundary 仅槽 0 |
| 8 QueryWatch、10 CancelWatch；9 AckWatch | 与既定 Query/Ack/Cancel 体相同，直接发到订阅控制入口 | WatchInfo；编码及语义由 service 拥有，不依赖 libfs |

`RegistrationId = {registry_incarnation:u64, serial:u64}`，前者取该 registry mailbox 的内核对象 ID，后者由 provider 单调分配、不回绕；耗尽只拒绝新注册，不妨碍撤出旧注册。它用于描述和区分注册，不是授权。`RegistrationInfo = RegistrationId + state:u32 + reason:u32`，实际控制身份来自不可伪造 sender context。Register mode=0 CreateOnly、1 Replace；每次提交创建新注册，控制不支持再 Publish 或重新激活 Withdrawn。一个注册持一个可复制 MailboxSender，该入口可接受多个协议；控制只属于目录注册，与业务会话独立。Register 回复表示提交时的 Active 快照，回复到达前可能已经被替换，须用该控制 Query 观察后续状态。

三个协议各自固定 status：0 Ok，1 NotFound，2 NotDirectory，3 Permission，4 Invalid，5 Unsupported，6 Exists，7 NotEmpty，8 Revoked，9 Conflict，10 Resource，11 Quota，12 Busy，13 Cancelled，14 Internal，15 CrossProvider，16 TypeMismatch，17 Range，18 Expired，19 CursorInvalid，20 SummaryTooLarge，21 EntryTooLarge，22 ValueTooLarge，23 Unavailable。各协议只产生适用项，枚举定义归各自协议，librpc 不拥有业务错误。UnknownProtocol/未知版本由接收端明确拒绝，不执行操作；合法 RPC 前缀且具备可用回复权时发送该协议的 Unsupported，缺少可靠回复头或回复权时丢弃并关闭收到的能力。客户端将未知版本/status 或畸形回复报告为协议错误，不据此重放变更。IoResult 的 cause 使用 fs status，end=0 可继续、1 EOF、2 Error；短 I/O 与后续错误可以同时表达，不能用空错误体抹掉已提交数量。

每个报文的领域体容量为 `B = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN`，当前为 4048。ObjectInfo 总长按最终 Trait/tag 编码计算，Lookup Found 的 result 与完整摘要均计入 B；枚举页按真实 name/info/cursor 总长装整项。Set、ReadAt/WriteAt 沿各自头和参数计算容量，不能截断路径或值。Register 必须预检其完整请求及未来 GetEntry 的 `4(result)+16(RegistrationId)+4(Bytes长度)+descriptors_len≤B`，同时保证该记录的 OAL 摘要和单项枚举可表达；名称也受完整 Register/WatchName 报文约束。Set/Create 的值仍须能够经 Get 返回。服务最多两个业务回复槽，Register 业务输入仅 slot 1；能力数按实际成功/Boundary/错误分支分别校验，不沿用旧每页能力槽限额。

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

`FsRights` 从原 FalRights 拆出，保留 Traverse/Enumerate、ReadProperty/WriteProperty、ReadStream/WriteStream、Create/Remove/Watch，增加明确 Describe 与 Derive；原 AcquireCapability 删除。Lookup 要求路径 Traverse 与 Describe，Enumerate 要求 Traverse/Enumerate，不借 ReadProperty 授予摘要权限。SetLength 使用 WriteStream，Sync 按相应文件访问权；Rename 检查源 Remove、目的 Create，替换还须目的 Remove。目录 Derive 要求 Derive、范围位于母 root 内且 FsRights/输出运输都单调收缩。数字位号在线形中依此顺序从 bit 0 分配，未知位拒绝；底层运输 Rights 直接使用 shared 真值。

服务 DirectoryAccess 的 Describe/Enumerate/GetEntry/Watch 与发布 authority 的 Create/Replace/Delegate 分开。根发布权可 DelegateName，ExactName 不再扩大范围；CreateOnly 需 Create，Replace 需 Create|Replace（含空位创建），Register 在最终提交时重新检查 scope、权利与名字条件。已交付控制不依赖父 authority 的某个别名持续存活。Register 校验母本是可重复共享的 MailboxSender，实际运输权含 WRITE/WAIT/TRANSIT/DUPLICATE；export_transport 至少含 WRITE/WAIT/TRANSIT，且不超出母本与发布 authority 上限。GetEntry 还受目录访问入口的输出上限限制，不自动附加 GRANT，不改变领域权限；描述是否真实由发行者负责。共享副本沿用原 context/账户，不伪称逐客户端隔离或撤销。控制和订阅操作只按自己的 sender context 认领资源，正文 RegistrationId 不授予控制。

首批计费不引入“请求传裸 account id”或新的付款 capability 协议。共享发现/目录入口下的请求、流和订阅继承该入口的可信账户，别名共用额度；装配需要隔离时签发不同账户绑定的入口。服务 registration 的 endpoint 预先由发行者收窄；registry 的 GetEntry 只产生一次运输副本，不在 provider 新建 grant。以后确需取得者支付时，由对应领域新增真实付款授权契约；当前不假称已经取得者付费。

费用按真实占用计算：固定记录分配字节、预留可变容量及索引/退休节点；可共享名字存储由一个实际 owner 计费，索引和 OAL 摘要不各存一份领域真值。ServiceResource 保留 Authority/Registration/Bytes/WaitSource，增加 Access/Watch 及必要请求/Outbox 准入；具体布局统一按实际 owner 计算，不能对同一存储重复收费。Runtime 的 Task/InputBytes/来源额度与 service 账户来自同一可信付款上下文。每注册在提交前至少准备控制 Lifetime、业务入口 CLOSED 两个来源及最终关闭承载；每 Watch 准备控制 Lifetime、通知对端关闭来源及唤醒承载。请求/回复来源依现有 RPC 生命周期另计。容量是宿主的真实预算，控制壳继续收费直到关闭，释放的 endpoint/来源分别退款。

订阅和控制的保留可能由持有者延长，但始终计费且有全局/账户上界。grant、stream、watch、registration 的 cleanup 节点、必要的终态记录与唤醒承载均在公开前准备；提交后不得因再次申请容器失败而丢掉责任。准备失败关闭已经铸造的入口/通知/映射；close 或来源撤销失败时，完整 owner 留在原任务的退休状态继续推进，不在错误值被丢弃时泄漏。

并发沿现有 provider 执行方式：真实 backing 由本地单 owner/短可变借用串行访问；borrow 内完成选择、验证和提交，不持有它跨 IPC、映射或等待。RPC/流任务保存自己的准备对象和实际 NodeRef；异步准备回来后重新验证仍需成立的授权及当前名字条件，再提交。OAL 摘要只是这次借用生成的数据，不登记观察者。不同 provider 没有跨进程锁序；Rename 只在一个 backing 提交，远端派生先交出本地借用。

每次 poll 有显式工作预算：路径/值扫描受单个报文长度限制；流最多搬运缓冲可容纳的一个块；退休最多处理传入 budget 项。watch 状态标记最多扫描准入的 `W_limit` 条，内存容量及这一次扫描成本由宿主配置约束；提交前预留足够的 WakeBatch，标记在提交步骤完成，唤醒投递满时保留游标分批兑现。这样 Query 可立即看到变化，延迟的是通知投递。停止时先封准入，再让已提交 entry 和退休任务推进；临时任务消散或外部不再调用均不能删除清理责任。

源码复核的复用边界：当前 [Registry 退休](../../user/libraries/libservice/src/registry.rs) 的 retire_registrations 在 endpoint.close 错误时保留母本但向上传播 Err，最终可到达 [runtime.turn 的 expect](../../user/services/srv_fs/src/server.rs)；[Watch close_signaler](../../user/libraries/libfal/src/provider.rs) 则丢弃返回的 owner，转入 typed Drop 的必成关闭边界。迁移不能把这两种写法当作“可恢复退休错误已有重试”的证据。目标任务对可恢复来源/映射/退休错误保留 owner、记录下一次唤醒并重试，轮转游标防止一项失败阻挡其它退休项；合法 typed 非映射 Handle 关闭的结构性必成约束仍以 libkernel 为准，不通过制造非法 handle 来伪造可恢复故障。

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

SetLength 复用 [Data 的稀疏块表示](../../user/libraries/libfal/src/data.rs)：扩长只改逻辑长度，未分配洞读为零，不按目标长度同步分配/清零。缩短在一次有界提交中更新长度/内部版本并清零最后保留块的尾部（最多一个块），将 `Trimming { first_discarded_block, cursor }` 责任留在实际 Data；之后每次退休推进删除预算内的尾块，释放后才退款。Trimming 期间 Read 只见新长度，新的 WriteAt/SetLength 返回 Busy，活动写流停驻于该 backing 的变化唤醒，不继续提交或复活尾块；清理完成后清除标记并唤醒写流。缩短前准备的写因内部版本不符必须重新准备。清理承载在首次可能需要它的节点准入时预留，截断后调用者消失仍由 provider 推进；节点整体退休接管同一 Data，不并行清理两次。这是内存 backing 的短期写入背压，不是观察对象产生的锁或 OAL 状态。

创建返回空结果是有意的：成功表示已创建，不隐含必须再制造一份 ObjectInfo；需要摘要的消费者单独 Lookup。CreateOpen 只返回实际会话，省掉“创建→查询身份→条件 Open”的旧组合。

| 流内部状态 | 触发 / 转移 | 对控制请求的行为 |
|---|---|---|
| Preparing | 路径与权限校验、资源准备；失败进入原请求退休，成功安装 Offered | 不对外可见，不允许业务数据提交 |
| Offered | offer 有限期限；成功 Start→Active；超时/Cancel/控制消散/停止→Stopping | Start 校验 Runnel 握手后原子启动；重复 Start 在 Active 返回现状，不重置进度 |
| Active | 每块确认后端接受量及运输量；EOF 且排空→Stopping(Completed)，取消/失效/后端错→Stopping(对应原因) | Query 返回单次一致的计数；Finish 登记一个有界等待者，额外等待者返回 Busy；请求等待期限不延长 session 期限 |
| Stopping | 停止新块，等待已提交后端操作确定，关闭数据面、撤销来源并重试失败 owner | wire 仍报告 Active（若未启动则 Offered）；Query 不冒充最终计数。Cancel/Finish 可以等终态，等待者退出不取消清理 |
| Terminal | 已不会再发生后端写入，冻结 outcome/reason/计数；重清理资源可仍在退休 | Query/Finish/Cancel 幂等返回终态；Start 返回 Conflict；保存收费的最小控制记录，控制最后关闭后最终回收 |

业务计数冻结与映射物理释放是两道条件：可以先有 Terminal，再有映射关闭重试，但不能有 Terminal 后仍能提交数据。写方向 `accepted≤transported`；读方向 transported 是已放入隧道的字节、accepted 是对端已确认消费的部分。Copy 的本地读出量独立统计，不拿读流 producer 的计数冒充自身已经读到。

Finish 请求超时只结束该次等待；控制仍有效时可以 Query/再次 Finish。Cancel 显式要求会话停止，可重复；控制的最后副本关闭也触发停止。session_deadline 可以是领域允许的无限期限，但 Offered 的建立期限必须有限，且端点/控制关闭始终受观察；调用期限、建立期限和会话期限不能互相替代。内存 backend 没有异步写入，Stopping 可快速完成；未来后端不能沿用“发出取消即完成”的假设。

文件 Watch 持目标的弱身份及通知登记，不用强 NodeRef 把目标永久保活。名字解除发 Unbound 提示（mask bit 4），实际 backing 在最后命名/使用引用消失并开始退休时发 Terminal；目录名字事件为 Create/Delete/Rename，内容为 Modify（沿用 bit 0—3），Terminal 固定最高位。明确 revoke 引起该授权建立的活动流/订阅停止，已经提交的数据不回滚。仅关闭父 grant 的一个副本不触发 revoke。

两种领域的订阅都由独立控制 context 管理，不能使用共享根 context 加裸 ID 操作他人订阅。sequence 从 1 开始，建立时置 Initial 重查提示（bit 62）；Initial/Terminal 自动包含，不允许请求方借 mask 排除。Query 不清，Ack 只确认给定旧序列，通知复位和 pending 判断由同一 owner 完成。CancelWatch 是幂等终止，不关闭仍由调用者持有的控制记录；Terminal 记录仍可 Query，终态通知保持到确认/关闭。发生新变化时先更新计数再置通知；`u64::MAX` 保留给终态，Active 不能达到该值，从而耗尽转终态仍有一个未被确认的新序列。reason 的数字为 Active=0、TargetRetired=1、ProviderStopping=2、Cancelled=3、Revoked=4、SequenceExhausted=5，service 的 TargetRetired 指范围目录退休。provider 停止后的控制不可用由传输 Closed 表达，不保证停止进程还能回复最后一次查询。

Copy 的返回类型明确分为 `Complete` 与 `Failed { creation: NotAttempted|NotCreated|Created|Unknown, local_read, transported, destination_confirmed, source_terminal:Option<StreamInfo>, destination_terminal:Option<StreamInfo>, cause, cleanup_owners }`。没有确认过目标计数时 lower bound 为零；仅收到目标 Terminal 才知道最终 accepted。Complete 要求两端完成、目标完成原因正确且接受量等于本地搬运量，不代表内容快照或持久化。cleanup_owners 承接现有可重试的映射/回复清理，不因业务返回失败就丢弃；不再提供默认按路径回删的 CreatedTarget。

### 注册提交与完成状态

本节定义第九轮 registry 的完成合同。公开状态为 Active=1、Withdrawn=2；reason 为 None=0、Explicit=1、Replaced=2、ControlClosed=3、EndpointClosed=4、ProviderStopping=5。Withdrawn 是名字不可再选择的业务终态，不能据此推断观察来源和存储已全部释放；第一次撤出原因固定，不重新激活。

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
| `libfal/src/{authority,grant,backend,store,data,resource,provider,watch,route}.rs` | 按知识归属拆分：文件存储/操作支持归 fs；通用目录声明和路径装配支持归 OAL；真实 grant/context/付款及退休状态归实际 provider。边界使用现成受限入口或由真实发行者签发，不保留普通 walk 必须新派生的统一政策，不整包搬进 fs |
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

## 实施顺序

一次交付覆盖完整责任链。下列 P1—P3 是相互依赖的迁移工作包，中间状态不要求独立通过整体验收；不为临时通过检查创建旧协议 adapter、双编码或假消费者。局部不编译的迁移继续按目标接完；暂停时准确记录断点和已实施部分。

| 工作包 | 前置 | 成组迁移的内容 | 完成证据与删除门 | 当前状态 |
|---|---|---|---|---|
| P0 设计收口 | 本计划及当前源码 | 按已确认 OAL/入口分层完成表示与迁移细化，继续 fs/service 审阅并完成设计 Review | 结构与责任链审查通过，当前承诺内的 findings 已关闭，再确认施工就绪 | OAL 原则与入口分层已确认；工程细化及其余领域审阅未完成 |
| P1 协议接口与 provider 对接 | P0 | 拆出 liboal 的发现表达/接口，分开各领域协议及编解码，接收端直接调用真实 provider 方法 | Lookup 和文件调用都落在同一实际 backing；没有抽象层状态、框架前置走路或 OAL 业务封装 | 未开始 |
| P2 文件领域 | P1 的接口与入口 | 将文件协议独立到 libfs，承接现有实际文件 backing、客户端及 init/test 调用；由 provider/backing 实现小 I/O、流、目录、Watch、Copy 及收束 | 目标支持矩阵接通；普通操作无旧 Lookup 条件；不在协议层复制 backing，旧能力属性探针退场 | 未开始 |
| P3 服务目录领域 | P1 的 OAL/RPC 表达；与 P2 联合验证文件入口发布 | 先实现 service 协议和 registry 完成合同，再迁入 provider 生命周期支持，最后改宿主/消费者的 Register、WatchName、GetEntry 与 Namespace 绑定；同责任链删除旧投影和两阶段注册 | 第九轮语义审阅通过，替换/未知回复/控制队列引用/Watch/退款用例通过，libservice 不依赖 libfs/libfal | 设计成稿待审阅；代码未开始 |
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
| 接口直接调用与单一 backing | 同一具体 provider 实现 OAL/fs 接口；分别请求 Lookup/Read，验证 backing 数据变化自然反映到摘要，接收胶水不预先遍历或保存对象镜像 |
| 普通路径 API 与入口授权分离 | 复用现有消费者，Get/Open 从 namespace/路径直接发领域请求，跟随两种 Boundary 后由当前 provider 执行；不要求 DirectoryGrant/Position 或前置 Lookup。独立受限入口仍由 provider 校验，修改客户端路径不能扩大能力范围 |
| Boundary 和受限授权 | 复用现有 provider/host 接续设施：只依赖 OAL 的客户端跟随路径转发与换入口交替，不需要领域 Boundary 解码；覆盖相对/视图绝对目标、未消费后缀、播放器式目录别名、空剩余路径和循环预算；真实能力不放大，取消/回复丢失无遗留入口 |
| 领域路径与能力请求 | Rename 等按 fs 契约在 backing 检查当前目标、两个真实目录授权和循环条件；结果丢失不经 OAL 自动重放，输入资源沿所属协议收束 |
| 枚举与只读摘要 | host 后端及真实枚举：静态目录有限无重复，变化目录遵守游标规则，过大单项不伪装完整；摘要不能成为可写状态或能力来源 |
| Property、小 I/O 与文件会话 | 类型/范围/短结果、同一文件共享流、SetLength 交错、建立失败和退出；截断→清理→扩长读零、清理期间写流可恢复且无旧数据复活；实际计数及支持/权限错误符合接口 |
| Copy 与结果未知 | 迁移既有 Copy 场景：跨 provider、已提交前缀后取消、CreateOpen/Finish 回复丢失、provider 停止；已确认值不能冒充最终值，不按名字删替代者 |
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
| 完整方案的设计 Review | 本计划 D0；已有作者走查，独立设计审查尚未开展 | 检查机制必要性、分层与组合、生命周期/授权/失败路径及迁移闭包；发现、修订与复核记录在本篇，通过后才进入 P1 |
| 新协议与实际资源链尚未实现 | 本计划 P1—P4；仅有待审查设计 | 设计 Review 通过后按源码迁移表接通真实两端；以完整验证矩阵和旧路径删除为完成标准 |
| 迁移期间仍存旧路径 | 本计划 P1—P4；代码尚未改，未产生新的半重构区域 | 同一切面接通后删除对应旧实现；P4 前清零。暂停时记具体文件/责任/验证断点并同步矫正区，不创建兼容桥接 |
| 旧 Copy 间歇失败 | [原问题记录](todo-2026-09-29-fal-midflight-copy-flake.md)独占现象与根因证据；当前根因仍未知 | 新路径若复现或涉及复用机制，属于本次 Copy 正确性闭包；交付前提供修复或旧责任链被替换且等价场景已验证的证据，在原记录说明处置，不用一次通过宣称原问题根因已解决 |
| 设备/驱动与正式装配 | 各所属分案；不在本次实现 | 它们开始接入时使用本案协议；本案只保证扩展接点，不能宣称硬件资源或长期服务能力已交付 |
| 磁盘持久化、排他/所有权转移、临时文件原子发布 | 本计划范围外边界；当前接口不承诺这些能力 | 出现独立需求再闭合相应领域契约；内存 Sync=Unsupported、共享 Open 和保留 Copy 部分结果构成当前完整行为，不伪装成已具备未来能力 |

其它临时表示、清理缺口或范围外残留如在实施中出现，按项目债务规则加入本节，写明 owner、位置、触发、删除条件和验证。未经登记不得作为跨阶段默认前提；当前承诺内的正确性欠账不能延期。

## 完成条件与记录维护

- D0 已关闭，当前目标、接口与资源责任完整，实际支持矩阵与非目标相符。
- OAL、fs、service 的请求/响应两侧及 srv_fs/srv_init/test_fs 接通；基础库存在或局部检查通过不能替代真实消费者接入。
- 正常、失败、取消、超时、退出和退款责任逐项兑现，已提交工作不依赖消散的调用者。
- 删除清单清零，Cargo 身份一致，未留下旧库 facade、重复对象真值或未登记过渡路径。
- 相应验证矩阵、`just check`/聚合验收和交付前结构检查有明确结果；旧 Copy 问题按原记录处置，未覆盖项如实记录。
- 实现说明按当前源码回填，COMPASS 和关联计划指向有效入口；ideas 的线稿清理/审定与代码完成分开登记。

全部条件满足才归档本活跃计划；提交、push 和发布仍按各自授权边界执行。代码交付后的独立 Review 按[REVIEW](../REVIEW.md)登记固定提交或序列，不替代本次交付前检查。纯文档整理不登记代码独立 Review。

## 工作记录与当前证据

当前阶段：第八轮 OAL 原则已获用户确认；2026-10-09 的第九轮服务目录重构方案已成稿，单步 Register、叶条目/GetEntry、客户端协议选择与弱一致分页属于本轮设计结论，待用户审阅。已核对 libservice/registry、srv_fs 任务及 init/test 消费者；一次针对注册简化的设计咨询所提示的替换未知结果、队列控制寿命及迟到成功回复均已纳入合同。跨文件服务取证已完成并记录版本/限制，未据此默认采用新的 fs 语义。以上不等于完整独立设计 Review 通过，P1—P4 均未开始。较早文档证据不覆盖本轮修订，历史过程见[设计档案](ref-2026-10-07-oal-fs-service-design-rounds.md)。

此前明确 Lookup 直接调用 provider，内部走路只在其 backing；OAL 和 fs 不另持领域真值。这里不排除 OAL 客户端的路径/目录入口/Boundary 接续状态，本轮归属见第八轮修订。旧 PathHeader、Continue 业务信封、强制 Locate/NoContinue 前置仍不恢复；Copy 和服务生命周期细则留待相应领域设计复核，不能据此改变 OAL 面貌模型。

本次整理将 897 行讨论版完整归档，保留当前详细目标并补上 D0、模块/消费者、工作包、删除、验证和延期门。档案记录原文件 SHA-256；它的相对链接已调整，历史事实不改写为当前状态。未运行 Rust 编译、host 测试或 QEMU，不新增代码验收结论。

本轮 D0 收口按现有协议、实际消费者和资源实现补齐：直接领域调用的边界接续、独立线形/能力槽、单 owner 的状态与付款、文件完成和服务替换/退休，以及旧模块的具体迁移表。设计检查使用 codebase-design 的知识归属与接口边界视角；没有因拆库增加新的对象真值、运行体或假消费者。另核实旧退休错误路径的实际处理，不把未具备的重试写成现状。

文档验证：七份 Markdown 的 183 个本地链接/标题锚点及代码围栏检查通过，`git diff --check` 通过；历史档案逆向还原相对链接后仍与原 897 行正文的 SHA-256 一致。按 REVIEW 完成当前设计的所有权、提交、取消、工作界、重复真值和迁移删除检查，具体反例见设计走查。没有运行 Rust 编译、host 测试或 QEMU；P1—P4 的实现与验收证据仍为空。

本轮第九轮文档验证：5 份 Markdown、91 个本地链接/标题锚点及代码围栏检查通过，`git diff --check` 通过。结构自查覆盖实际 owner、文件投影删除、namespace 绑定与业务调用边界、注册提交/回复分离、旧控制替换保护、队列控制引用、Watch 跨替换与退休费用；已将会改变外部行为的设计结论显式标为待用户审阅。服务目录新方案未执行完整独立设计 Review，未运行 Rust 编译/host/QEMU，未把文档取证计作实现验收。


## 当前接力

下一步审阅第九轮服务目录完整方案，重点是单步 Register 替换后的未知结果、ServiceEntry 叶语义、GetEntry 后显式绑定、控制/Watch 寿命与独立 provider 装配。OAL 的已确认原则不重开，剩余编码/迁移细化由实施者收口。跨 provider Move/Copy 先按外部取证讨论 fs 的承诺边界，不将普通跨进程 IPC 误当作原子搬移障碍或保证。两端设计和完整 Review 闭合前不开始 Rust 施工。

审查既找正确性缺口，也检查能否消除重复表示、特殊路径和没有独立需求依据的机制；保留必要的长期扩展能力。用真实调用与失败/退出场景核实，findings、处置及复核留在本篇，不另开平行设计。通过并修订计划后再进入 P1 的源码迁移；当前不开始 Rust 施工。
