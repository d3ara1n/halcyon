# OAL、fs 与服务目录改造计划

状态：**D0 详细方案已成稿，下一步是设计 Review；尚未确认施工就绪，Rust 改造未开始。** 本篇是原 FAL 拆分的唯一活跃记录。当前方案见[目标设计](#目标设计)，设计产物见[D0 设计收口](#d0-设计收口)，审查接力见[当前接力](#当前接力)。成稿、自查、设计审查通过和实现验收分别记录。

第一至第七轮讨论、被替代方案及当时证据已存入[只读设计档案](archived/ref-2026-10-07-oal-fs-service-design-rounds.md)。档案中的“当前”“下一步”不再作为施工入口；本篇独立说明现行方案、未闭合事项、实施与验收。归档的是讨论快照，不是尚未完成的改造任务。

## 交付目标与范围

把现有 libfal/libfs/libservice 中混合的发现、文件和服务协议职责拆开，交付 liboal 发现协议、libfs 文件协议及 libservice 注册取得协议，接通现有 provider、装配与验收消费者。所有领域数据、状态、实际路径遍历与操作由具体 provider/backing 实例持有和执行；协议库不在 backing 之上另建状态层或执行层。

| 范围 | 本案承诺 |
|---|---|
| OAL | 发现的请求方式、路径关系、Lookup/Enumerate、ObjectInfo/Boundary 与开放 tag 表达；不承载领域操作 |
| fs | 内存文件 provider 的目录/Property/Stream、授权、小 I/O、流完成、名字变更、订阅和 Copy |
| 服务目录 | 名字发布授权、注册/发布/替换/撤出、GetEntry、名字/目录变化订阅及入口退休 |
| 实际接通 | srv_fs、srv_init、现有 test_fal 及 libfs/libservice 内部消费者；验收进程随迁移改名为 test_fs |
| 支撑面 | 必要的 RPC 编码工具和 provider ingress 分派、Cargo/构建/initfs/验收入口、对应实现说明与导航 |

不在本案实现磁盘/网络文件系统、音乐播放器、完整设备/驱动资源协议、正式长期服务监督或新 RPC/Runnel 运行体。内存后端不承诺持久化；设备/驱动仅检验 OAL 的扩展接点，其领域设计由[设备分案](todo-2026-09-device-system-design.md)拥有，启动和健康监督由[装配分案](todo-2026-10-07-service-assembly.md)拥有。

## 已确认的架构边界

1. OAL 是 provider 暴露内部结构和对象的协议，规定初次发现的请求方式和外貌表达。Lookup 直接到 provider，由其 backing 产生 ObjectInfo 或 Boundary。树和摘要是单向只读投影，没有 OAL 对象存储，也没有从 tag 反向修改 backing 的操作语义。
2. Trait/tag 是开放命名空间中的平铺声明和带值摘要。领域库解释自己的标签；OAL 不引入递归类型语言、逐标签绑定或全域形状证明系统。
3. 实际路径解析作用于 provider 的 backing；libfs 可以提供遍历等基础算法与操作封装供进程采用。“只有一套”指没有 OAL/fs 各自独立的目录真值或执行层，不是要求遍历源码必须写在 provider 模块中。OAL 表达发现的路径/边界关系，不接管领域操作。
4. 普通 Get/Read/Write 按本次实际目标执行，不要求先 Lookup，也不携带上一查询的 expected_identity；观察不产生锁定、占用或每进程隔离视图。
5. liboal/libfs/libservice 提供领域知识、接口/数据形状和可复用工具，不替所有 provider 统一决定具体行为、策略或实现。libfs 不只限于协议声明，也应帮助引入它的进程实现文件能力；实例状态仍由实际使用者/backing 持有，不因使用库再出现一份真值。
6. provider 可以组合多种领域协议，也可以消费其它 provider 来实现自己的协议；播放器的多协议摘要/内部 Open 转发仅是设计检验例。
7. 真实能力、付款和已提交责任保持各自 owner。可见的标签不产生权限，撤名不证明已有业务资源已经回收，Close 不冒充 Finish/Sync。

## 基线、前置与方案状态

本次整理核对分支 `refactor/system-redesign`，HEAD `70bb29b9d5cba6a9a43cc353b8f9e7c67f357fc5`。初始设计基线 `183d25c` 等历史版本保留在档案。本轮开始时已有本会话的五份 plans 修改；本次只整理文档，不改运行代码、ideas 或 impls。

代码与[当前 FAL 实现说明](../notes/impls/fal.md)描述旧实现；[KNOWN_ISSUES](../KNOWN_ISSUES.md#矫正区)已登记矫正范围，不把旧结构当新方案的正确性依据。现存 RPC、Runnel、Process/Job 和 SourceRegistration 已有实现基础，本案不重新启动已完成的对象链或运行时分层任务。

本案从既有 FAL/fs 的协议和 backing 实现做职责分离。库定义契约与可复用代码，实际实例由 provider 持有；不能把“代码位于某库”解释成“运行时多一个拥有对象真值的层”。共同 PathHeader、OAL 通用 Continue 业务封装、框架先定位再调用领域 handler、强制 Locate 前置均撤出当前方案。CreateOpen、共享流、Copy 保留部分结果等仍只是 fs/backing 的领域设计，不是 OAL 的职责。

当前共享运输常量为 `PAYLOAD_MAX = 4096`、`MESSAGE_HANDLE_MAX = 8`，来源是 [message.rs](../shared/erhino_shared/src/message.rs)，它们是现状依据而非在新库复制的常量。旧 `PATH_MAX = 512`、链接/步数限额不能未经论证照搬。完整报文必须计入 RpcPrefix、路径/领域头、参数、响应和实际能力槽。

## D0 设计收口

架构方向已经明确，当前没有等待用户再次选择对象模型、Provider 写法或运行时分层的问题。D0 是实施者需要自主完成的工程准备，不是五项待用户确认的设计议题。按旧 FAL/fs 的实际代码和已有机制补齐映射、表示和适配；只有新需求、范围扩大或与已确认原则无法闭合的选择才提交用户裁决。产物直接补入本篇，代码形成后由相应 owner 拥有类型/常量，计划不维护第二份数值真值。

| 编号 | 设计内容 | 已形成的待审查产物 | 实施依赖 |
|---|---|---|---|
| D0.1 既有知识与工具的拆分 | 类型、遍历算法、操作封装及提供端支持的归属 | [源码迁移映射](#源码迁移映射)；保留库中的有用实现，实例归真实 provider | P1 及其后全部 |
| D0.2 线形与错误 | 独立协议、路径参数、摘要、能力槽、长度和结果 | [路径接续细则](#路径接续细则)、[线形与操作表](#线形与操作表)；没有 OAL 业务信封或强制 Lookup 前置 | P1—P3 的两端迁移 |
| D0.3 backing 授权、容量与清理承载 | 权限、付款、准入、工作界与退休 | [实际授权与资源责任](#实际授权与资源责任)；复用真实账户、来源登记和退款机制 | P1—P3 的资源路径 |
| D0.4 fs 完成协议 | 部分结果、流、创建、取消及订阅 | [文件提交与完成状态](#文件提交与完成状态)；文件解绑、业务终态与资源释放分开 | P2 |
| D0.5 服务完成协议 | 发布替换、期限、回复丢失、入口关闭和名字观察 | [注册提交与完成状态](#注册提交与完成状态)；名字索引无额外文件投影，控制者不能撤出替代者 | P3 |

D0 当前产物：上述五组方案给出支持矩阵的表示、两端调用、提交及资源收束，作者自查见[设计走查](#设计走查)。下一步先对完整方案进行设计 Review，检查结构、必要性、扩展性和责任链，再修订本篇并复核发现；审查通过后才关闭 D0、进入 P1。Rust trait 的组织、私有模块拆分和依据实际布局计算的容量值属于实施选择，不能借这些选择改动协议语义或遗漏清理。

## 目标设计

以下承载当前目标契约；后面的线形、状态和迁移表是这些边界的实施细化。方案状态由上一节统一记录。

### 职责与依赖

| 旧机制 | 目标归属与处置 |
|---|---|
| 路径表达、Lookup/Boundary 与实际遍历 | OAL 定义发现请求与结果的表达；实际解析、链接/授权根处理留在 provider/backing，按其实现的领域协议执行，不抽成 OAL 走路引擎 |
| 固定 NodeInfo/NodeSummary | 由 OAL 的动态 ObjectInfo 替代；文件标签由 libfs 定义，不保留统一 NodeKind 或强制对象身份/版本字段 |
| Property/Stream、目录变更、文件权限、DirectoryGrant、流控制 | libfs 定义契约和类型；实际存储、授权表、流状态与操作属于文件 provider 的 backing；旧实现按这一归属迁移，不另建 fs 状态层 |
| ServiceRecord 伪装为 Property/Record，Read 时导出 endpoint | 删除该表达；服务摘要为 OAL 投影，取得入口是 libservice 的显式操作 |
| 服务名字授权、发布控制、就绪/撤出、endpoint 失效观察 | libservice 定义契约，registry backing 持有并推进实际状态，直接产生 OAL 视图，不增加协议层状态或 FAL 投影 NodeStore |
| Directory 特制 ExportPolicy、属性中的能力值及 affine Take | 不进入 OAL/tag 或 fs Property；真实能力交付由领域操作和 RPC 承担，旧探针随迁移移除 |
| 文件 Watch、Copy | 保留有独立用途的文件能力，按下文归 libfs；服务变化订阅归 libservice，不让服务目录冒充文件 |
| 通用 LE Reader/Writer | 移到 librpc 的协议编码工具模块，由 OAL/fs/service 直接使用；不为共享字节游标保留 libfal 或建立 fs→service 依赖 |
| RPC、Runnel、来源登记、预算 | 复用各自 owner；不新建通用 OAL 运行体或第二种流运输 |

目标依赖为 `libfs → liboal + librunnel`、`libservice → liboal`，二者均可直接消费 librpc/libexecution/libbudget/libkernel；libservice 不依赖 libfs。music_player 可以同时消费 liboal/libfs 并实现自己的协议。设备/驱动库按相同方式定义标签和领域操作，不使 OAL 增加领域分支。

核实依据：现有 [文件后端](../user/libraries/libfal/src/backend.rs) 的 lookup/resolve 已共用 lookup_child，它是 provider 实际走路的参考。现有 [客户端代码](../user/libraries/libfs/src/resolve.rs) 中 Found/Delegate/Link 和 Position 同时混合了发现继续与文件假设，须按当前责任重新核定，不能整块搬入 OAL 成为第二个执行器。[RPC 前缀](../user/libraries/librpc/src/lib.rs) 和 [Move 准入](../user/services/srv_fs/src/server.rs) 的传输/真实授权校验可复用；消息分派不等于框架提前遍历 backing。

### 库与 provider 的使用关系

liboal/libfs/libservice 提供各自领域的知识和可互通的形状，具体 provider 按自身需求使用。库可以提供类型、编解码、客户端封装、遍历等基础算法及实现协议所需的工具；它们不是必须逐层经过的运行时，也不替 provider 统一选择数据组织、共享/独占或其它业务策略。

用户给出的 `impl Provider for MyProvider` 只是表达接口关系的伪代码，不是已选定的 Rust trait、async 签名、分派或模块布局要求。具体写法由实施者结合现有代码选择，无须再把这些常规工程选择交用户确认。

例如文件 provider 可以直接使用 libfs 提供的遍历工具操作自己的 backing，并将结果按 liboal 的 ObjectInfo/Boundary 形状暴露；也可以在适合自身后端时提供专用实现。复用算法和操作封装不等于多出一个拥有领域真值的 fs 层。

RPC 接收设施只负责协议识别、解码、调用方法和发送回复；调用关系分别为 `OAL client → MyProvider::lookup → backing` 和 `fs client → MyProvider::read → backing`，不存在 `OAL 执行层 → fs 执行层 → backing` 的串联关系。查找本身当然是一次协议调用，但 OAL 不接收、解释或执行文件/服务业务操作。

协议无领域状态，不等于运行程序没有调用上下文或缓冲。回复缓冲、临时请求、客户端持有的入口和流进度由实际运行实例持有；它们不构成另一份可独立更新的对象真值。若库提供可复用 backing 实现，其实例仍是 provider 选用的 backing，不能再为它配一份所谓 OAL/fs 层状态。

### OAL 入口与路径

入口是能接受相应协议请求的真实通信能力；transport 提供已认证的请求来源，provider/backing 解释自己的授权范围。文件实现按 libfs 契约处理 DirectoryGrant、路径和读写权限；播放器按自己的播放列表与访问规则实现发现。OAL 规定路径参数与 ObjectInfo/Boundary 的表达及错误，不要求所有 provider 采用同一目录表、FsRights 或授权算法。

OAL 描述名字、子项和跨 provider 的路径关系。路径如何在内存树、ext4、注册表或播放列表中解析，由该 provider 的 backing 实现；文件路径中的父目录、链接及访问根规则按 fs 契约兑现。共同语法/编码见路径接续细则；OAL 不提前规范化路径、维护目录栈或替 provider 选择实际对象。

Boundary 中可携带继续发现的目标入口及剩余路径关系；接收者据此向相应 provider 发起后续查询。这是协议结果的使用，不是 OAL 遍历远端内部结构。只有 provider 知道应该返回摘要、边界或错误；客户端不根据 tag 自行猜测底层位置。

若边界涉及目录权限衰减，仍由具体 provider 与真实发行者兑现后再交付入口；不能只发送过宽能力和一份要求客户端自律的 rights。相关请求和临时能力由实际 provider/调用者持有并沿已有运输契约清理，OAL 不维护 grant 表。

### 路径请求与领域接续

Lookup、Read、Open、GetEntry 分别是各协议的请求，直接交给实现它的 provider 方法。已经知道领域的调用者可以直接发领域请求，不要求先握手或先取得摘要；provider 在本次操作中验证实际目标。OAL 发现帮助未知调用者认识对象，不成为所有领域操作的必经执行层。

在同一进程/入口复用接收设施时，可以根据协议标识调用不同的 Provider 实现；也可以按实际授权角色设置不同入口。这属于具体 provider 的装配和 RPC 分派，不引入统一 PathHeader 或 OAL 的 `Done/Continue(domain_response)` 业务信封。

文件操作遇到跨 provider 情况时如何继续、是否内部转发、哪些结果可以重试，由 fs 协议及其具体实现负责；服务和播放器同理。领域可以复用 OAL 的 Boundary 表达，但 OAL 不接管其请求、能力输入或副作用。本案 fs 的接续结果只发生在领域操作提交前，具体合同见路径接续细则。

### 发现操作与目录枚举

liboal 的提供端接口以 Lookup/Enumerate 为基础。Lookup 的结果为 `Found(ObjectInfo)` 或 `Boundary`，失败返回协议错误；具体实现可内部查询/遍历 backing 后生成结果，也可直接计算这个视图。没有 Locate、通用前置遍历或定位后统一执行业务的要求。

Boundary 的字段从旧 FAL 的委派/链接表达取证，明确目标入口和路径继续关系；它不替接收者取得额外领域权限。跟随后由应答 provider 生成最终摘要，OAL 不自动合并沿途 tag。文件符号链接的读取、末项跟随等由实现 fs 的 provider 按 fs 契约处理，不变成 OAL 执行器的控制状态。

枚举由 provider 对自己的 backing 执行，返回子项名字和 ObjectInfo，必要时表达边界项。分页游标由 backing 解释；OAL 只定义请求/响应及完整、结束、失效和过大摘要等结果。目录变化不默认生成每个消费者的快照或镜像树。客户端与提供端的响应/查询预算沿各自请求生命周期落实，不以通用走路状态机代替。

### 摘要与标签

ObjectInfo 是条目序列；每项为 `namespace[16] + local_id:u32 + value_len:u32 + value[value_len]`。命名空间为协议包固定的 128 位标识，第三方可自行分配并在包中固定；OAL 不维护运行时中央标签注册表。标准包标识和 local_id 在各自协议模块保持唯一常量，变更含义须使用新标识。无值声明的 payload 为空；空字符串等带值 tag 仍按其键的已知类型解释。

同一摘要中键唯一；顺序无语义，不要求 provider 排序。解码器验证条目边界和重复键，未知键按 value_len 跳过，不推理不同领域标签的组合合法性。tag 中不承载活 capability；业务入口经领域调用和 RPC capability 槽交付，描述性 ID 不成为授权。

首批标准 tag 为 Directory、Boundary、Size、CreationTime、ModifiedTime、SummaryRevision。Size 为逻辑字节长度 u64；时间为 UTC epoch 秒 i64 加纳秒 u32（纳秒小于十亿），缺少可靠时间就省略；SummaryRevision 为可选的不透明字节串，仅描述该 provider 视图的摘要变化，不代表内容版本、对象身份或 Get 前置条件。无可靠修订来源时省略，不为填 tag 创建 OAL 状态。

fs、service、music_player、mime 等自行定义命名空间与 tag 值类型。fs 的 Property/Array/Integer 等仍是平铺标记；一个 tag 的值类型不自动变成整个对象的新 Trait。完整 ObjectInfo 必须在一次有界响应内编码；过大返回 SummaryTooLarge，完整领域记录或正文通过领域协议获取。标准 tag 是摘要语义，不是可反向映射的属性存储。

### fs 基础接口

libfs 提供文件领域的类型、接口形状、编解码、遍历算法和操作封装，帮助进程实现或消费文件能力，不限定所有 provider 使用同一种内部组织或行为策略。以下接口与行为表用于核对本项目当前文件 provider 的迁移方案；其中共享、原子性、会话或清理政策不自动成为所有引入 libfs 的 provider 的强制行为。目录、RegularFile/Property/Stream、SymbolicLink 等摘要仍来自实际 backing。

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

Watch 由文件协议拥有，服务目录另有服务变化订阅。两者复用底层通知/来源登记和单调计数工具，不再共同依赖文件 NodeStore。每次变化递增 sequence 并合并事件位；Query 不清空，Ack(sequence) 仅在未出现更晚变化时清除，否则保持通知，避免 Query/清除之间丢唤醒。事件是重查提示，不是完整日志。fs Watch 绑定所选后端文件/目录，其退休、明确撤销订阅授权或 provider 停止给出终态；服务按名字/目录订阅的终止条件见下文，不能因某次 publication 退休就终止它。独立控制入口关闭后取消订阅并最终释放资源。

消费者顺序固定为：先建立订阅，Query 订阅得到 sequence，再读取/枚举实际状态，最后 Ack 这个旧 sequence 并等待通知；不能在状态读取之后取一个更晚的 sequence 来确认，否则会吞掉中间的变化。序列耗尽时明确终止订阅，不回绕复用。

Copy 是 libfs 客户端组合：Open 源、CreateOpen 目标、按预算搬运、分别完成两端。源内容不是默认快照，目标是本次创建并打开的文件；名字随后改变不把会话转向替代者。结果分开记录本地读出量、提交到运输的量、最后确认的目标 accepted，以及两端可缺失的终态；只有收到目标可信终态才能报告精确最终 accepted。失联时最后确认值只是下界，最终写入/完成可以未知，关闭映射不补足这个知识。初版保留已经创建的空文件或部分结果，不自动按原路径删除。源、目标会话及映射由 Copy owner 全部收束；文件本身仍归后端，保留结果不是泄漏。

发送 CreateOpen 后丢失结果时报告创建结果未知，不自动重试、按名字清理或宣称回滚。这个选择替代旧 CreatedTarget 的默认条件删除路径；它不需要跨 Lookup/Get 身份匹配。是否另提供显式临时文件/原子发布由独立文件需求触发，不是当前 Copy 成功或失败责任的缺口。

### 服务目录

libservice 提供服务目录领域的知识、形状和可复用支持；具体 registry backing 持有名字、Publication、授权和生命周期状态。以下描述本项目现有注册目录的迁移方案，不要求所有使用 libservice 的进程都建立相同注册表或采用同一运行体。该 provider 的 OAL 接口从实际 backing 生成 ServiceEntry 摘要，GetEntry 等操作也作用于该 backing，不建立额外文件投影。

| 接口 | 语义及授权 |
|---|---|
| DelegateName | 持管理 authority 者签发限定名字的发布权；它与发现根入口分开，不因目录可见而获得 |
| Register | 凭发布 authority 提交目标名字、协议描述、可共享入口和有限建立期限，返回 PublicationControl；只进入 Starting，不公布名字 |
| Publish(CreateOnly/Replace) | publication 的控制者声明入口已准备好；CreateOnly 要求名字空闲，Replace 要求发布授权允许替换；一次提交名字指向 Ready publication |
| GetEntry(path, protocol_requirement) | 在本次调用中选取 Ready publication，返回匹配的协议描述、publication 标识和同一条目的入口副本；不依赖旧 Lookup，不创建业务会话 |
| Withdraw | 经 PublicationControl 停止该 publication 的新取得，并从名字索引移除它自己的绑定；旧控制者不能移除替代者 |
| QueryPublication | 经控制入口查询自己 publication 的状态及终止原因，不通过重查名字寻找旧实例 |
| WatchDirectory/WatchName | 服务领域的变化提示，使用独立订阅控制入口；订阅建立先于首次查询，并用上述 sequence/Ack 规则消除丢唤醒窗口 |

WatchName 绑定已授权目录中的名字，允许名字暂不存在，并跨 publication 替换持续有效；调用目标是目录，名字作为服务参数，因此不依赖先 Lookup 到某个服务条目。WatchDirectory 绑定目录范围。发布、替换和撤出仅产生变化提示；订阅的终态由取消、其授权范围明确失效、目录本身退休或 provider 停止决定。单次 publication 的状态通过自己的 PublicationControl 查询，不拿它的寿命代替名字订阅寿命。

Register 接受的是目录内单个名字；管理 authority 规定允许的名字范围及 Create/Replace 权限，控制入口持有准入时授予该 publication 的范围和权利。关闭发布 authority 的某个别名不等于撤销已经交付的注册控制权。GetEntry 返回的协议描述须描述该入口实际接受的协议及路径调用约定，不把某个工厂/准入入口伪装成已经建立的业务会话。

状态为 Starting → Ready → Draining → Terminal。Register 可准备多个有相同目标名字的 publication，实际名字竞争在 Publish 提交时决定；CreateOnly 失败保留 Starting，调用者可继续管理或关闭。Replace 原子更换可见条目，并使旧 publication 进入 Draining；旧 publication 的控制、已发出的入口和费用不因此冒充已经回收。对同一已 Ready publication 重复 Publish 返回当前结果，不恢复已被撤出的注册。

Starting 到期、控制入口消散或被注册入口关闭时，由 registry 停止发布并收束；Ready 失效同样撤出自己的绑定。名称删除只在索引仍指向本 publication 时生效，这是注册资源自身的清理责任，不是用户 Lookup 后锁定对象的机制。Terminal 后只保留必要控制终态，真实入口、观察来源及临时交付引用逐一退休后才退款；控制入口最后关闭后释放终态记录。

GetEntry 的选择与获得副本在 registry 的一次受保护步骤完成；它返回的描述和入口对应同一个 publication。此后撤发布只阻止新选择，已选定/已交付入口按其发行者的业务协议存活；registry 不承诺召回它们。若请求已投递但结果丢失，调用者按既有 RPC 未知结果处理，不据此创建虚假的新实例。

注册的入口必须是发行者允许重复共享、实际权限已经限定的调用入口。registry 只复制其被允许的运输权，不理解或衰减 FsRights，不向文件 provider 隐式请求 Derive。需要独立文件授权/业务会话时，发布实际的领域准入入口，客户端 GetEntry 后调用该领域协议；固定共享入口不能被声称为逐客户端隔离会话。付款由可信发布/发行上下文绑定，不接受请求中的裸账户 ID 作为付款授权。

Publish 是发布者的就绪声明，registry 不探测任意业务协议，也不根据 endpoint 存在推断业务健康。谁启动、监督、重启以及怎样确认真实就绪由正式装配分案拥有；registry 只兑现发布与取得关系。首轮由现有 srv_fs 组合文件 provider 与 registry，不新增 srv_oal/srv_service。

### 提交与资源交接

| 责任 | 准入/提交点 | 失败、退出与最终释放 |
|---|---|---|
| Lookup/枚举/Boundary 回复 | provider 方法读取/遍历自己的 backing，必要时准备继续入口 | 请求和回复资源归实际运行实例；未交付/未采用能力沿 RPC 清理，不产生 OAL 走路状态或叶对象会话 |
| Property/目录变更 | 内存/配额准备完后，由后端原子提交值或名字变更 | 提交前可回滚准备；提交后回复失败不回滚，不自动重放 |
| 文件 Open/CreateOpen | 预留文件/会话/运输资源后建立可交付结果；CreateOpen 的名字插入是创建提交点 | 丢失回复时控制/offer 经寿命观察及期限退休；已提交文件保留，结果未知须报告 |
| Publication | Register 持真实入口及控制资源；Publish 才提交名字可见 | 未发布回复失败可关闭预备资源；发布后失联由已登记观察/期限接管，不依赖发布者继续调用 |
| GetEntry | 选定 publication 并准备对应入口副本 | 交付失败清理副本；成功后由消费者持有，发行者拥有业务资源；撤名不提前退款仍存引用 |
| 订阅/流会话 | 各领域显式建立资源并注册既有来源观察 | 端点关闭/超时触发当前 owner 退休；清理待办继续被 provider 持有，真实释放后退款 |

协议库不重建 RPC、执行或来源登记。实际宿主负责把收到的消息调用到相应 provider 方法，backing 持有并推进 root、会话、publication、订阅等领域状态。复用现有 RpcPrefix、回复授权、能力槽和 PreparedResponse/Outbox 的运输规则，不给 ObjectInfo/tag 或抽象协议层另建账户/对象状态。

### 路径接续细则

单次线上路径为 UTF-8 相对路径，空串表示入口根；`/` 分隔，不含 NUL、空段、`.`、`..`，不以 `/` 起止。名字为一个非空分量。`*`、`?`、反斜杠没有通配或转义作用，不沿用旧 FAL 对这些普通字符的额外禁令。长度取决于本次完整报文的可用字节，不再额外规定 512 字节上限。库不在解码时访问对象或推断领域类型。

Boundary 只表达 provider 已经发现的接续关系，有两种线形：

| 形式 | 字段与能力 | 使用规则 |
|---|---|---|
| Delegate | `consumed_bytes:u32, remaining:Text`，回复 slot 0 为新入口 | consumed 必须截止在原路径分量末尾且非零，原路径由 consumed 前缀和 remaining 连续覆盖；目标从自己的授权根处理 remaining。空 remaining 合法；空路径不能继续 Delegate |
| Redirect | `parent_bytes:u32, target:Text, remaining:Text`，无能力 | parent 是原路径中被重定向分量的父路径前缀；remaining 是其后缀。target 是相对该父位置或以 `/` 开头的视图绝对路径；它只是 provider 明示的路径关系。允许其中的 `.`/`..`，其解释属于采用相应路径语义的调用库，不进入单次 provider 请求 |

`liboal::lookup`/`enumerate` 的基础封装返回一次真实请求的结果，不偷偷跟随、不输出叶对象 handle。只消费 liboal 的程序可以显示 Boundary 或按自身视图继续发现。libfs 保留已有 prefix、边界覆盖检查、相对/绝对链接展开等客户端工具，但将其驱动输入改成当前操作的结果；这些是调用期间的路径上下文，不是 provider 内部树或另一份对象真值。只有 fs 消费者需要解释文件链接、末项策略及跨挂载 `..`；这些代码不移入 liboal。OAL Lookup 在末项返回该对象本身的摘要，末项链接可表现为 SymbolicLink；中途遇到链接时由文件 provider 返回 Redirect。已知类型的文件调用按各自 FollowAll/NoFollowFinal 规则处理，ReadLink、Remove、Rename 的末项不跟随。

fs 中所有单目标路径操作的响应明确包含 `Boundary` 分支，且只可在尚未创建、修改、建立流/订阅或交付操作结果时产生。ReadAt 的 Boundary 也不能夹带已读取的数据。provider 的 handler 使用同一 backing 遍历接口，在本次请求里得到本地目标或边界；本地目标直接执行，不再重做一次外部 Lookup。fs 客户端收到明确 Boundary 后，以新 txid 向目标发送同一领域操作及改写后的路径；已投递后的超时、损坏回复和失联不等价于 Boundary，不自动重放变更。

带输入能力的 Rename 采用现有“两份真实目录授权”结构：客户端先用 **fs DeriveDirectory** 取得目标父目录 grant，再向源入口发送源路径、目标单个名字及该 grant 的临时副本。源 provider 在一次提交中校验两个目录及本次源项；跨 provider 返回 CrossProvider。这不是 OAL 定位前置，也不宣称两次调用之间目标父目录名字不变：派生 grant 明确选择了目录，之后按该目录操作。源路径遇边界可接续；每次运输使用调用者保留 grant 的临时副本，旧请求负责清理收到的副本，不能将唯一输入移走后盲目重发。

边界安装仍是具体宿主的管理行为。现有 route Bind 迁入文件 provider 的管理支持，持独立管理入口和真实目标 grant；不能根据 ObjectInfo 的 Directory tag 安装授权。当前 `/System/Services` 由同一宿主的装配配置暴露已经收窄的 registry 发现入口，`/System` 的目录摘要和枚举只来自该宿主的绑定配置，服务内容仍由 registry 生成。该宿主分别实现 OAL、fs 和 service 的路径接口；service 的 GetEntry/Watch 路径 handler 可在自身绑定处返回 service 响应中的 Boundary，不把请求送进 OAL。动态 fs Bind 只配置 fs 目录，不用 FsRights 解释 registry 入口。未来设备/驱动是其它绑定及 provider，不要求 OAL 增加 CreateObject、挂载表或领域分支，也不意味着任何 provider 自动实现所有未知领域操作。绑定的替换/撤出只改变后续路径选择，已交出的边界入口仍按发行者契约存活。

客户端的接续有显式 `FollowBudget { calls, expanded_bytes, deadline }`，在每次请求和链接展开前扣除；默认值来自调用方配置的请求/内存预算，不复制旧 40/256 魔数为协议限额。耗尽返回本地 BudgetExceeded 并清理临时入口。provider 每次只处理一个报文允许的路径长度，远端 Derive 使用原始调用的剩余期限；不得在每跳重新获得无限时间。内部转发的 provider 同样持有自己的有限任务预算及下游请求 owner。没有跨进程全局路径锁。

### 线形与操作表

以下是目标线形，不是旧源码已经实现的 ABI。每个协议拥有自己的 Header/Request/Response；共享的是 librpc 的 LE 字节工具、RpcPrefix，以及 liboal 的摘要/Boundary 数据类型。相同头布局不引入统一业务 dispatcher 或通用 PathHeader。

| 协议 | 消息 kind / 版本 | 接收入口 |
|---|---|---|
| OAL | `0x4f41_4c31` / 1 | 实际 provider 暴露发现的入口 |
| fs | `0x4653_3031` / 1 | 文件目录 grant、流/订阅控制入口 |
| service | `0x5352_5632_5245_4701` / 2 | 发现入口、发布 authority、publication/订阅控制入口 |
| 文件宿主管理 | `0x4653_3031_5254_4501` / 1 | 独立管理 sender；沿用 Bind 的领域化配置作用 |

同一文件 grant 的 mailbox 接受 OAL/fs 两个 kind，二者使用同一个已认证 sender context 查真实 grant；registry 的发现入口接受 OAL/service。错误角色不会因换一个 kind 获得管理权。现有分开的 ingress 按此合并相应 handler，管理/控制角色仍单独签发 sender。这里不新增消息运输协议。

宿主管理 op 1 Bind 的体为 `name:Text, rights:u32, replace:u32`，请求槽 1 为下游文件母 grant；op 2 Unbind 为 `name:Text`，二者成功体为空且没有 Boundary。Bind 限于管理入口所属根的一个直接子项，replace=0/1 分别要求空位/允许替换；配置只定义路由，实际输出仍须由文件发行者完成授权收窄。Unbind 只关闭配置持有的引用，旧交付 grant 不因此 revoke。`/System/Services` 的跨领域绑定由宿主可信装配建立，显式记录准许到达的目标发现入口及协议；它不复用这个 fs-only Bind，也不通过 FsRights 推导服务的 GetEntry/发布权。

RpcPrefix 保持 16 字节；随后各协议头均为 `version:u16, op:u16, status:u32, body_len:u32, reserved:u32=0, Deadline:16`，共 32 字节。请求 status=0；回复回显 op、txid 和请求期限。op 先按原始 u16 读取，以便对未知操作返回 Unsupported。请求 slot 0 固定为 librpc 回复权；下表请求业务槽从 1 开始，回复槽从 0 开始。不使用的槽必须为空，错误回复不带能力。

本文使用 `Text/Bytes = len:u32 + len 个字节`，无隐含对齐；Text 必须合法 UTF-8。`OptionalU64 = present:u32 + value:u64`，present 只能是 0/1，缺省时 value=0。Deadline 直接使用 shared 的既有 16 字节编码及合法性校验。所有加法、乘法、偏移和 u64→usize 转换做溢出/范围检查。

ObjectInfo 线形为 `entry_count:u32` 后接本篇定义的 tag 条目。首批命名空间取固定 16 字节：ASCII `halcyon.oal`、`halcyon.fs`、`halcyon.service` 分别以零补齐到 16 字节；它们是包分配的标识，不在运行时按字符串注册。OAL local_id 1—6 依次为 Directory、Boundary、Size、CreationTime、ModifiedTime、SummaryRevision。fs local_id 1—11 依次为 RegularFile、Property、Stream、SymbolicLink、Array、Bool、Integer、Float、String、Blob、Unsigned；Integer 可与 Unsigned 并列，默认有符号。service local_id 1—3 为 ServiceEntry、Publication、Protocols，后两项采用下述 PublicationId/协议描述编码。不要求所有对象含这些 tag，也不把 fs 值编码器放入 OAL。

首批 backing 的投影对应为：目录→OAL Directory；Property→fs RegularFile/Property 加当前 scalar 标记及可选 Array/Unsigned；流文件→fs RegularFile/Stream 加 OAL Size；链接→fs SymbolicLink；服务条目→service ServiceEntry/Publication/Protocols。链接目标只由 ReadLink 返回，服务入口只由 GetEntry 返回。普通文件与服务记录的区别来自声明而非路径前缀；一个自有 provider 同时支持文件/播放等协议时可以组合这些声明。摘要与实际操作间允许变化，操作端仍校验本次 backing 类型，库不把曾见过的 tag 当作成功保证。

`ProtocolDescriptor = id:u64, min_version:u16, max_version:u16, addressing:u32`，addressing=0 表示不带路径的领域入口、1 表示按入口根相对寻址；它描述已公布协议的调用约定，不授予能力。列表为 count:u32 加固定 16 字节条目，同一个 id 不重复，版本区间非空。`Requirement` 使用同样字段表示所需区间；GetEntry 返回所请求 id 的最高相交版本以及该入口完整描述列表，不在 registry 内执行业务协议握手。

| OAL op | 请求体（按字段顺序） | 成功结果 / 业务槽 |
|---|---|---|
| 1 Lookup | path:Text | result:u32，0=ObjectInfo、1=Delegate、2=Redirect；仅 Delegate 回复槽 0 |
| 2 Enumerate | path:Text, cursor:Bytes, max_entries:u32 | result:u32；0=Page，1/2=Boundary。Page 为 next_cursor:Bytes, done:u32, count:u32，随后重复 name:Text + info:Bytes；无业务能力 |

枚举初始 cursor 为空。内存 FS/registry 的 cursor 为 `{directory_id:u64, epoch:u64, next_index:u64}`，绑定入口授权范围和被枚举目录；epoch 变化返回 CursorInvalid，调用者重新枚举，OAL 不保存扫描会话。单次静态枚举不重复/漏项，变化期间不承诺快照。Boundary 子项通过标记摘要表达，不在枚举时批量导出入口；进入该子项才 Lookup。装不下下一项就返回已有整项及继续游标；空页仍装不下单项则 EntryTooLarge，不静默截断。摘要变化也使该 provider 的枚举 epoch 前进；溢出封闭新枚举/变更准入，不回绕。必要的退出/退休仍可进行，此后所有旧游标一律失效，不以维持游标为由阻止资源释放；服务可见名字的版本预约另见注册状态节。

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
| 1 DelegateName | name:Text, publish_rights:u32 | 空；回复槽 0=名字 authority |
| 2 Register | name:Text, descriptors:Bytes, export_transport:u32, establish_deadline:Deadline | PublicationInfo；请求槽 1=可共享 endpoint，回复槽 0=PublicationControl |
| 3 Publish | mode:u32，0=CreateOnly/1=Replace；发到控制入口 | PublicationInfo |
| 4 GetEntry | path:Text, requirement:ProtocolDescriptor | result:u32；0=`PublicationId + descriptors:Bytes + selected_version:u16`、1/2=Boundary；成功取得时回复槽 0=入口 |
| 5 Withdraw、6 QueryPublication | 空，发到控制入口 | PublicationInfo |
| 7 WatchDirectory、8 WatchName | directory_path:Text；WatchName 追加 name:Text | result:u32 + WatchInfo 或 Boundary；回复槽 0=控制、1=waiter |
| 9 QueryWatch、11 CancelWatch；10 AckWatch | 与 fs 相应控制请求一致 | WatchInfo；类型由 service 拥有，不依赖 libfs |

PublicationId 是 `{registry_incarnation:u64, serial:u64}`；incarnation 取实际 registry 接收 mailbox 的内核对象 ID，serial 由该 backing 单调分配、永不回绕。它是描述身份而非授权，生命周期内不复用；进程重启获得新 mailbox。PublicationInfo 为 `PublicationId, state:u32, reason:u32`，实际控制权由 sender context 确认，不接受调用者填入 ID 代行控制。描述列表在 Register 时冻结，要换入口或协议就准备新 publication。首次支持每个 publication 一个可共享 MailboxSender；一个 sender 可以接受多个已声明协议。文件目录也是这种真实入口，不需要 service 认识 fs 类型。

三个协议各自固定 status：0 Ok，1 NotFound，2 NotDirectory，3 Permission，4 Invalid，5 Unsupported，6 Exists，7 NotEmpty，8 Revoked，9 Conflict，10 Resource，11 Quota，12 Busy，13 Cancelled，14 Internal，15 CrossProvider，16 TypeMismatch，17 Range，18 Expired，19 CursorInvalid，20 SummaryTooLarge，21 EntryTooLarge，22 ValueTooLarge，23 Unavailable。各协议只产生适用项，枚举定义归各自协议，librpc 不拥有业务错误。UnknownProtocol/未知版本由接收端明确拒绝，不执行操作；合法 RPC 前缀且具备可用回复权时发送该协议的 Unsupported，缺少可靠回复头或回复权时丢弃并关闭收到的能力。客户端将未知版本/status 或畸形回复报告为协议错误，不据此重放变更。IoResult 的 cause 使用 fs status，end=0 可继续、1 EOF、2 Error；短 I/O 与后续错误可以同时表达，不能用空错误体抹掉已提交数量。

每个报文的领域体容量为 `B = PAYLOAD_MAX - librpc::PREFIX_LEN - protocol::HEADER_LEN`，当前为 4048。ObjectInfo 总长 `S=4+Σ(24+value_len)`，Lookup Found 要求 `4+S≤B`；枚举页按真实 name/info/cursor 总长装整项。Set 要求 `4+path_len+4+value_len≤B`；ReadAt 成功最多返回 `B-4-20` 个字节（result 与 IoResult 头），WriteAt 同时受请求体及 offset 加法约束。Register 还须预检未来 GetEntry 的 `26+descriptors_len≤B`；Set/Create 的值必须能够经 Get 返回，即 `8+value_len≤B`，不能成功存下一个无法按本协议读取的值。路径过大、值过大必须在提交前拒绝；边界改写后放不下也明确失败，不能截断参数。所有回复最多两个业务槽，请求最多 slot 0+1，共享的 8 槽上限足够；不要把旧“每页 7 项”能力槽限额带入纯摘要枚举。

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
| 服务 `Publication` / `names` | publication 拥有冻结的名字/描述、endpoint、授权快照、控制/入口观察、退休责任；names 仅索引 Ready publication | 发布 authority 的账户支付 Publication、Bytes 和来源；名字索引存 publication 引用，不复制 ServiceRecord/NodeStore。准备时预留索引与退休节点 |
| 请求、Outbox、下游请求 | 收到的输入、回复权、临时能力、编码缓冲、尚未交付结果 | 对应入口的可信账户承担领域 Request/Outbox/Bytes；执行任务另用同一付款身份绑定的 libexecution 视图。Outbox/ReplyCleanup 原 owner 负责失败重试 |

`FsRights` 从原 FalRights 拆出，保留 Traverse/Enumerate、ReadProperty/WriteProperty、ReadStream/WriteStream、Create/Remove/Watch，增加明确 Describe 与 Derive；原 AcquireCapability 删除。Lookup 要求路径 Traverse 与 Describe，Enumerate 要求 Traverse/Enumerate，不借 ReadProperty 授予摘要权限。SetLength 使用 WriteStream，Sync 按相应文件访问权；Rename 检查源 Remove、目的 Create，替换还须目的 Remove。目录 Derive 要求 Derive、范围位于母 root 内且 FsRights/输出运输都单调收缩。数字位号在线形中依此顺序从 bit 0 分配，未知位拒绝；底层运输 Rights 直接使用 shared 真值。

服务 DirectoryAccess 的四项权利从 bit 0 开始；NameAuthority 的 Create/Replace/Delegate 同样从 bit 0 开始，枚举各自独立。Root 发布权可 DelegateName，ExactName 不再向其它名字委派；控制入口冻结该 publication 可否 Replace，不重新依赖已经关闭的父 authority。Register 校验入口种类和实际 transport，registry 持有的母本必须包含 WRITE/WAIT/TRANSIT/DUPLICATE；export_transport 至少包含 WRITE/WAIT/TRANSIT，不能超出收到的真实能力或 authority 的输出上限，可以不向消费者交付 DUPLICATE。GetEntry 还检查发现入口允许输出这些运输权；不改变入口中的领域权限。共享副本沿用发行者的 context/账户，不伪称逐客户端身份或逐客户端撤销。Rename helper 使用的目标父 grant 则显式请求 DUPLICATE，以保留原件并运输临时副本。

首批计费不引入“请求传裸 account id”或新的付款 capability 协议。共享发现/目录入口下的请求、流和订阅继承该入口的可信账户，别名共用额度；装配需要隔离时签发不同账户绑定的入口。服务 publication 的 endpoint 预先由发行者收窄；registry 的 GetEntry 只产生一次运输副本，不在 provider 新建 grant。以后确需取得者支付时，由对应领域新增真实付款授权契约；当前不假称已经取得者付费。

费用统一按实际占用计算：`固定记录分配字节 + 实际预留的可变容量字节 + 表索引/退休节点分配字节`，固定容器使用 `PreparedEntry::allocation_bytes()` 等实际 owner 的布局函数，Vec/String 计 capacity 而非只计 len。独立资源类别分别按对象数/来源数扣账，不能把字节和个数混成一个额度。`FsResource` 承接旧 FalResource 九类；ServiceResource 保留 Authority/Bytes/WaitSource，Registration 改名 Publication，增加 Access/Request/Outbox/Watch。槽位上限、账户额度和 libexecution 准入共同限制占用，数值由实际装配配置给出；同一逻辑对象即使仍在退休也占原槽，不以提前归还 Permit 制造超额。

订阅和控制的保留可能由持有者延长，但始终计费且有全局/账户上界。grant、stream、watch、publication 的 cleanup 节点、必要的终态记录与唤醒承载均在公开前准备；提交后不得因再次申请容器失败而丢掉责任。准备失败关闭已经铸造的入口/通知/映射；close 或来源撤销失败时，完整 owner 留在原任务的退休状态继续推进，不在错误值被丢弃时泄漏。

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
| Stopping | 停止新块，等待已提交后端操作确定，关闭数据面、撤销来源并重试失败 owner | wire 仍报告 Active（若未启动则 Offered）；Query 不冒充最终计数。Cancel/Finish 可以等终态，等待者退出不取消清理 |
| Terminal | 已不会再发生后端写入，冻结 outcome/reason/计数；重清理资源可仍在退休 | Query/Finish/Cancel 幂等返回终态；Start 返回 Conflict；保存收费的最小控制记录，控制最后关闭后最终回收 |

业务计数冻结与映射物理释放是两道条件：可以先有 Terminal，再有映射关闭重试，但不能有 Terminal 后仍能提交数据。写方向 `accepted≤transported`；读方向 transported 是已放入隧道的字节、accepted 是对端已确认消费的部分。Copy 的本地读出量独立统计，不拿读流 producer 的计数冒充自身已经读到。

Finish 请求超时只结束该次等待；控制仍有效时可以 Query/再次 Finish。Cancel 显式要求会话停止，可重复；控制的最后副本关闭也触发停止。session_deadline 可以是领域允许的无限期限，但 Offered 的建立期限必须有限，且端点/控制关闭始终受观察；调用期限、建立期限和会话期限不能互相替代。内存 backend 没有异步写入，Stopping 可快速完成；未来后端不能沿用“发出取消即完成”的假设。

文件 Watch 持目标的弱身份及通知登记，不用强 NodeRef 把目标永久保活。名字解除发 Unbound 提示（mask bit 4），实际 backing 在最后命名/使用引用消失并开始退休时发 Terminal；目录名字事件为 Create/Delete/Rename，内容为 Modify（沿用 bit 0—3），Terminal 固定最高位。明确 revoke 引起该授权建立的活动流/订阅停止，已经提交的数据不回滚。仅关闭父 grant 的一个副本不触发 revoke。

两种领域的订阅都由独立控制 context 管理，不能使用共享根 context 加裸 ID 操作他人订阅。sequence 从 1 开始，建立时置 Initial 重查提示（bit 62）；Initial/Terminal 自动包含，不允许请求方借 mask 排除。Query 不清，Ack 只确认给定旧序列，通知复位和 pending 判断由同一 owner 完成。CancelWatch 是幂等终止，不关闭仍由调用者持有的控制记录；Terminal 记录仍可 Query，终态通知保持到确认/关闭。发生新变化时先更新计数再置通知；`u64::MAX` 保留给终态，Active 不能达到该值，从而耗尽转终态仍有一个未被确认的新序列。reason 的数字为 Active=0、TargetRetired=1、ProviderStopping=2、Cancelled=3、Revoked=4、SequenceExhausted=5，service 的 TargetRetired 指范围目录退休。provider 停止后的控制不可用由传输 Closed 表达，不保证停止进程还能回复最后一次查询。

Copy 的返回类型明确分为 `Complete` 与 `Failed { creation: NotAttempted|NotCreated|Created|Unknown, local_read, transported, destination_confirmed, source_terminal:Option<StreamInfo>, destination_terminal:Option<StreamInfo>, cause, cleanup_owners }`。没有确认过目标计数时 lower bound 为零；仅收到目标 Terminal 才知道最终 accepted。Complete 要求两端完成、目标完成原因正确且接受量等于本地搬运量，不代表内容快照或持久化。cleanup_owners 承接现有可重试的映射/回复清理，不因业务返回失败就丢弃；不再提供默认按路径回删的 CreatedTarget。

### 注册提交与完成状态

Registry 保留一个 publication 表、一个只索引可见名字的表，以及按实际资源持有的授权/订阅/退休记录。没有对应的文件节点。Starting 不占可见名字，多个准备者可以竞争；每份准备者都支付自身费用和将来插入名字所需承载。第一次可见发布才建立名字绑定。

| 当前状态 / 事件 | 原子变更与结果 | 后续 owner |
|---|---|---|
| Register 准备 | 验证 authority/endpoint/描述/有限期限；预留 publication、名字候选、退休节点、控制与 endpoint 来源；安装 Starting 后才发送控制 | 准备失败由原请求清理全部资源；安装后回复失败由 observation/建立期限触发退休，不自动重复注册 |
| Starting + Publish(CreateOnly) | 到期先判 Expired；名字空则 Ready 并插入，存在则 Exists 且保持 Starting | 存在失败不延长期限、不顶替别人 |
| Starting + Publish(Replace) | 校验冻结的 Replace 权；准备完成后同一步切换名字→新 Ready，旧 Ready→Draining(Replaced) | 新旧 publication 各持自己的控制/费用，旧者进入已预留退休链 |
| Ready + Publish | 同一 publication 仍是可见绑定则返回当前 Ready | 回复丢失后可对同一控制重复 Publish；已被替换/撤出则 Conflict，不能复活 |
| Starting/Ready + Withdraw | 进入 Draining(Withdrawn)，仅在 names 仍指向自己时摘名 | 从公共接口删除语义重复的 BeginDrain；业务服务如何 drain 旧会话由其自身协议决定 |
| 控制 Lifetime CLOSED、endpoint CLOSED、Starting 到期、provider 停止 | 在单 owner 步骤中从 Starting/Ready 进入 Draining，并按自身绑定条件摘名 | 分别记录 ControlClosed、EndpointClosed、EstablishExpired、ProviderStopping；竞争时首个终止原因固定 |
| Draining | 拒绝新 GetEntry；撤销 endpoint 来源后关闭 registry 母本，失败保留 owner 重试 | 不等待所有消费者关闭副本，也不调用发行者 revoke；registry 不拥有那些业务资源 |
| 清理母本/来源完成 | Terminal，保留 PublicationId/state/reason 的收费控制终态 | 控制仍存活可 Query；最后控制引用关闭及其来源撤销后释放剩余表项/费用 |

公开 State 使用 Starting=1、Ready=2、Draining=3、Terminal=4；Reason 使用 None=0、Withdrawn=1、ControlClosed=2、EndpointClosed=3、EstablishExpired=4、ProviderStopping=5、Replaced=6。Draining/Terminal 的 Withdraw 幂等返回现状，Query 在全部公开状态可用。Publish 的 RPC 期限和 establish_deadline 分开；到期与 Publish 并发时以 owner 检查单调时钟并提交的先后为准，不能依赖超时通知任务是否已经运行。

GetEntry 在同一次 backing 借用中校验范围/权利、选定 Ready、匹配协议并取得入口副本，随后脱离借用编码/发送。Withdraw 在线性化点之后发生时该副本仍可交付；之前发生则本次找不到旧条目。发行者恰在复制后关闭 endpoint，消费者可收到已经不可用的入口，这属于真实能力寿命，不是描述配错了对象。发现也不授予 lease。复制失败不产生会话，收到 CLOSED 时安排正常撤名；回复丢失只退休那份待交付副本。

名字表的内部 epoch 为枚举/变化提示服务，不进入 GetEntry 的 expected 条件。Publish/Replace/Withdraw 在名字提交点更新 epoch 并标记匹配的 name/directory 订阅，完成后才交回执行器。WatchName 的目标是授权范围内的文本名字，可以不存在；旧 publication 进入 Terminal 不会终止它。service 事件位为 Published/Replaced/Withdrawn（bit 0—2）和 Terminal（最高位）；事件合并意味着重查当前状态，不保证消费者看到每次中间发布。

容量与溢出也不能阻止已承诺撤名：准备 publication 时为可能的首次发布和最终撤出保留名字 epoch 推进额度，Replace 同时消费新发布及旧撤出预约、形成一次可见 epoch 前进；剩余未使用额度在退休时归还。若计数空间不足，封闭新 Register/Publish，已 Ready 的撤出仍能完成。该预约只用于内部有限计数正确性，不是 Lookup 后占有对象。

### 源码迁移映射

| 现有源码 / 实际调用 | 迁移或删除 |
|---|---|
| `libfal/src/node.rs` 的路径/NodeInfo 与 `libfs/src/resolve.rs` 的结果类型 | 通用路径字节/Boundary/ObjectInfo 归 liboal；内部文件种类仍归 libfs backing。删除公共固定 NodeSummary/Position.info 与普通请求的 Expected |
| `libfs/src/{prefix,resolve}.rs`、`client.rs` | 保留 fs 客户端 prefix/链接/边界覆盖知识，将“先 Lookup 得到 Position 再操作”改成领域请求自身接续；目录 grant wrapper 仍归 fs，不变成 OAL 叶对象 |
| `libfal/src/{authority,grant,backend,store,data,resource,provider,watch,route}.rs` | 按实际文件职责迁入 libfs 支持模块；解除服务 backing 接口共用，Watch 改独立控制，稀疏 Data 补 SetLength 有界清理；存储、授权和运行实例继续由 srv_fs 持有 |
| `libfal/src/{protocol,client}.rs`、`protocol/stream.rs` | 文件请求与流/值编码归 libfs；发现请求归 liboal；有 owner 的失败/清理类型跟随实际调用保留 |
| `libfal/src/bytes.rs` | Reader/Writer/DecodeError 移入 librpc 编码工具；fs/service/oal 同次改依赖，不留旧重导出 |
| `libfal/src/value.rs`、`libservice/src/record.rs` | fs 承接纯值编码；删除 Record/Handle/ExportPolicy/Take 与服务记录文件化，不给 tag 添加旁路 handle |
| `libservice/src/{protocol,authority,registry,resource,client}.rs` | 承接自己的 authority、控制/endpoint 观察及退休；替换 Registry 的 NodeStore/projection 为直接摘要，增加独立 DirectoryAccess 与名字观察 |
| `srv_fs/src/server.rs`、`server/{stream,registration,authority}.rs` | 接收端直接按协议和真实 context 调用实际 backing；复用任务/来源/Outbox，删除 ReadTask 目录值导出；Open 等直接遍历并按领域返回边界 |
| `srv_fs` 的 secondary 注册、`srv_init` 名字授权与入口发布 | 发布前用 fs DeriveDirectory 得到确实允许共享的目录入口，再 Register/Publish；以后 GetEntry 无远端隐式 Derive |
| `srv_init` 与 `test_fal` 的 Subscribe→Read/ServiceRecord→Lookup | 改 service Watch→GetEntry→fs/OAL 实际调用；订阅先建立，按 sequence/Ack 读状态；删除专为 Handle/Affine/Record 构造的探针 |
| `libfs/src/client/copy.rs` 及真实 Copy 验收 | CreateOpen 与显式部分/未知结果；保留 Runnel/退出/清理 owner，删除默认 CreatedTarget 条件删除和条件 Open |

OAL 的 Object Abstraction Layer 名称及组件身份在 P1 同步写入 user/README；libraries/README、notes 索引与 impls 在实际迁移后反映代码，不在设计交付时把未来状态写成事实。Provider 接口可按同步 backing 方法与现有任务式长操作分别组织；不强迫所有方法实现一个异步 trait，也不把遍历/收束算法复制到每个进程。

### 设计走查

| 反例 / 既有审阅问题 | 本方案的闭合点 |
|---|---|
| 看过的名字被另一进程替换 | 普通操作直接处理本次目标；类型/权利不合返回错误，兼容替代者可以成功，没有 expected_identity 或 OAL 锁 |
| 完全未知领域对象 / 音乐对象同时像文件 | OAL 显示开放 tag；libfs 按已知 fs 声明请求当前对象，其它标签可忽略；具体 provider 可内部转发，不要求 OAL 懂播放、设备或驱动 |
| 普通 Read 必须先 Lookup、走两遍远端 | fs 请求自身遇边界才接续，本地最终 provider 在同次调用里选定目标并执行；未知对象初次 discovery 仍是显式可选行为 |
| Rename 携带目标授权、接续时丢失唯一输入 | 临时 grant 副本运输，原 owner 保留；只对明确未提交的 Boundary 重试；跨 provider 不伪装原子搬移 |
| 读服务记录使 provider grant 随读者增长 | GetEntry 只复制已经签发的共享入口；需要逐客户端资源时直接调用领域准入协议。共享账户、撤销及 watch context 的限制不隐藏 |
| Query 与通知复位之间发生新变化 | sequence 在状态提交时更新，Ack(old) 不吞掉新变化，通知投递可以分批但状态不能后补 |
| 服务 A→B→C 后旧控制者撤名、名字订阅消失 | 名字删除带实际 publication 相等条件；watch 绑定名字而非 publication；控制只管理自己 |
| Copy/写流回复丢失却报告精确接受量 | 已确认下界与可缺失的终态分开，CreateOpen 可 Unknown；Cancel 不在后端仍可提交时冻结计数 |
| 巨大 SetLength 无界清零、截断后再扩长露出旧数据 | 稀疏扩长；缩短同步遮蔽范围并清零边界块，尾块分批释放，完成前写入/扩长背压 |
| 撤名/Terminal 提前退款，关闭失败 owner 消失 | 业务可见性、终态、实际释放分别记录；预留退休承载，原任务继续持有并推进失败资源 |

走查结论限于设计与源码映射：没有保留旧通用执行候选的必要性，也没有发现需要新增内核原语才能实现本案的前置。旧审阅关于 Copy 未知计数、名字 watch 寿命和真实目标目录授权的发现均由上表对应契约承接；原 midflight Copy 故障仍需在代码阶段按验证门处置，不能因设计走查关闭。

## 目标模块与实际消费者

## 目标模块与实际消费者

以下是知识归属和编辑范围，不要求每个表格行成为独立交付阶段。新目录只有在实施时才创建；本文不将计划中的类型/模块写成已经存在。

| owner / 位置 | 目标职责与迁移面 |
|---|---|
| `user/libraries/liboal/` | 发现知识、路径/Boundary 表达、ObjectInfo/tag、提供端支持及客户端封装；不包含 backing 内部树遍历、业务路由器或对象状态 |
| `user/libraries/libfs/` | 文件领域知识、接口/值类型、编解码、遍历基础算法、操作封装及可复用实现支持；不固定 Provider 写法，不替使用者增加独立状态层 |
| `user/libraries/libservice/` | 服务目录知识、接口/值类型、调用封装及可复用注册支持；实际采用哪些工具、行为和状态组织由具体 provider 决定 |
| `user/libraries/librpc/` | 通用 LE 编码工具及已有请求/回复能力清理；不增加领域权限、OAL 路径或文件类型知识 |
| `user/services/srv_fs/` | 具体文件/registry provider 及 backing 实例拥有真实存储、授权、会话、订阅与操作；同一实例按需要实现多个协议接口，接收设施直接调用方法 |
| `user/services/srv_init/` | 根入口/名字发布权交付、服务发现、流与退出验收驱动；消费新协议和真实能力，不解码旧 ServiceRecord |
| `user/tests/test_fal/ → user/tests/test_fs/` | 复用现有真实进程验收，覆盖文件主线及所消费的 OAL/服务接点；改名同步 workspace、package、target、initfs 和启动路径 |
| 文档与构建 | `user/Cargo.toml`、相关 Cargo 依赖、Justfile、启动/验收引用；实现后同步 libraries/README、notes 索引及对应 impls |

旧消费者已核实集中在上述 provider、init、验收进程及两库内部；`srv_fs` 当前声明的 libfs 依赖未被其源码使用，不能把这视作已接通目标层。音乐播放器、设备/驱动仅作接口组合推演，不新增测试专用服务或假业务运行体。

## 实施顺序

一次交付覆盖完整责任链。下列 P1—P3 是相互依赖的迁移工作包，中间状态不要求独立通过整体验收；不为临时通过检查创建旧协议 adapter、双编码或假消费者。局部不编译的迁移继续按目标接完；暂停时准确记录断点和已实施部分。

| 工作包 | 前置 | 成组迁移的内容 | 完成证据与删除门 | 当前状态 |
|---|---|---|---|---|
| P0 设计收口 | 本计划及当前源码 | 完成 D0.1—D0.5 方案，经设计 Review 修订并复核 | 结构与责任链审查通过，当前承诺内的 findings 已关闭，再确认施工就绪 | 方案已成稿；设计 Review 未开始 |
| P1 协议接口与 provider 对接 | P0 | 拆出 liboal 的发现表达/接口，分开各领域协议及编解码，接收端直接调用真实 provider 方法 | Lookup 和文件调用都落在同一实际 backing；没有抽象层状态、框架前置走路或 OAL 业务封装 | 未开始 |
| P2 文件领域 | P1 的接口与入口 | 将文件协议独立到 libfs，承接现有实际文件 backing、客户端及 init/test 调用；由 provider/backing 实现小 I/O、流、目录、Watch、Copy 及收束 | 目标支持矩阵接通；普通操作无旧 Lookup 条件；不在协议层复制 backing，旧能力属性探针退场 | 未开始 |
| P3 服务目录领域 | P1 的接口与入口；联合 P2 检验文件入口发布 | 由 libservice 定义新接口，registry backing 持有发布/控制/订阅状态并执行 GetEntry；迁移 init/provider 发现流程 | Watch → GetEntry → 实际领域调用闭合；删除文件化 ServiceRecord/FAL Record 和额外投影 NodeStore | 未开始 |
| P4 组合收口 | P2、P3 接通 | 改名 test_fs，统一 Cargo/启动/initfs/验证锚点，删除 libfal 和剩余旧入口；回填实际 impls，完成结构检查与聚合验收 | 删除清单清零、验证矩阵有证据、历史缺陷处置明确；符合本篇交付条件后才归档 | 未开始 |

实际编辑可按公共类型 → 核心实现 → 消费者 → 清理顺序交错推进 P1—P3。不得将仅新增源码、局部编译或阶段结束登记为 OAL/fs/服务系统能力完成。根入口、会话、publication 和订阅的已提交清理始终由真实 owner 持有，不等待调用者回来补最后一步。

## 旧路径删除清单

| 待替换项 | 替代物与删除条件 |
|---|---|
| libfal package/目录、Cargo/import/构建引用 | 文件协议归 libfs、发现协议归 liboal；真实实现和状态由具体 provider/backing 承接，共享工具归实际 owner 后删旧包，不留旧名 facade |
| 混杂的发现/文件表示与抽象层走路假设 | 协议表达分别归各库；真实遍历保留在 provider/backing；只删除重复/错误的层，不为拆分搬出第二个 resolver |
| 固定公共 NodeInfo/NodeSummary、统一文件 NodeKind | ObjectInfo/tag 接通后删除旧公共表示；具体文件 backing 内部需要的种类和状态不在删除范围 |
| ServiceRecord/FAL Record/只读文件投影 NodeStore | registry 直接生成 OAL 摘要，GetEntry 返回入口；所有服务发现调用迁移后删除 |
| ExportPolicy 中 Directory 特判、Handle Property、affine Take | 正式能力交付走领域协议；移除原专用探针及 handler，不以保留测试为由保留机制 |
| 常规路径调用的 expected_identity 与旧 CreatedTarget 自动条件清理 | 当前路径请求、CreateOpen 会话和明确 Copy 结果接通后删除；不误删后端内部身份、publication 标识或订阅序列 |
| 旧共享 grant 下裸订阅 ID 的管理授权 | 独立订阅控制入口接通后删除；通知/单调计数等真正公共机制仍复用 |
| 语义重复的 BeginDrain/Withdraw、服务 QueryName/固定 Record 读取 | 统一控制入口 Withdraw/QueryPublication、发现 Lookup/GetEntry；不留旧 opcode 或别名 |
| test_fal 的当前 package/产物/启动名和失效锚点 | test_fs 和对应消费者/验证器同次切换，不留名称补偿 alias；历史日志与档案保留原名 |

最终删除检查同时看代码、Cargo 图、initfs/启动和有效文档。真实文件 backing 需要的 NodeRef/store、grant 表、流会话等继续按实际职责保留或重构；它们的实例属于 provider，不是要被清空的数据或要重新在 libfs 抽象层建立的副本。

## 验证矩阵与执行入口

开发证据和完整验收分开记录。优先迁移现有 host 单测、srv_init 和 test_fs 场景；下表给出需要捕获的独立错误，不为每个阶段另造 harness。

| 契约 / 风险 | 最小可信验证边界与预期 |
|---|---|
| 编码、未知 tag 与帧边界 | host 协议测试：按定义构造完整/截断/超长/重复键/未知标记/非法能力槽；未知 tag 可跳过，损坏帧返回错误，不触发业务 |
| 接口直接调用与单一 backing | 同一具体 provider 实现 OAL/fs 接口；分别请求 Lookup/Read，验证 backing 数据变化自然反映到摘要，接收胶水不预先遍历或保存对象镜像 |
| Boundary 和受限授权 | 复用双文件 provider：多跳、空剩余路径、链接与预算耗尽；只读 grant 不能取得更强下游权利；取消派生/回复丢失不遗留请求能力 |
| 领域路径与能力请求 | Rename 等按 fs 契约在 backing 检查当前目标、两个真实目录授权和循环条件；结果丢失不经 OAL 自动重放，输入资源沿所属协议收束 |
| 枚举与只读摘要 | host 后端及真实枚举：静态目录有限无重复，变化目录遵守游标规则，过大单项不伪装完整；摘要不能成为可写状态或能力来源 |
| Property、小 I/O 与文件会话 | 类型/范围/短结果、同一文件共享流、SetLength 交错、建立失败和退出；截断→清理→扩长读零、清理期间写流可恢复且无旧数据复活；实际计数及支持/权限错误符合接口 |
| Copy 与结果未知 | 迁移既有 Copy 场景：跨 provider、已提交前缀后取消、CreateOpen/Finish 回复丢失、provider 停止；已确认值不能冒充最终值，不按名字删替代者 |
| 服务发布与替换 | registry 单测加真实发现：Starting 不可取得；Publish/Replace 原子可见；GetEntry 的描述和能力匹配；旧控制者撤出不影响替代者 |
| 名字订阅与丢唤醒 | 从未发布到 A→B→C 的连续变化；Query → 读状态 → Ack 间插入变化仍可观察；publication 退休不终止名字订阅 |
| 资源与退出 | 正式 provider/消费者退出和响应丢失路径；入口、来源、映射、终态记录在真实释放后退款，无无人持有的退休责任 |
| 依赖和旧路径 | Cargo/import、实际调用、构建/启动及有效文档检查；libservice 不依赖 libfs/libfal，liboal 不解释领域状态 |

host 检查按受影响包选择并显式指定 host target；例如目标包建成后在 user workspace 使用 `cargo test -p liboal -p libfs -p libservice -p librpc --target aarch64-apple-darwin`。实施前按实际 cfg/包测试能力核对，不把该示例当已执行结果。

责任链接通后按[构建与验证手册](BUILD-AND-TEST.md)执行 `just check` 和跨机制聚合 `just acceptance`；后者已包含 clippy 及规定平台/退出路线，无新风险时不重复单跑同一套检查。`just check` 只覆盖 os 及其依赖，不能代替用户态验证。本案未改调度域契约时不额外要求 hetero。长日志、进程清理和证据边界遵守手册。

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

全部条件满足才归档本活跃计划；提交、push 和发布仍按各自授权边界执行。代码交付后的独立 Review 按[REVIEW](REVIEW.md)登记固定提交或序列，不替代本次交付前检查。纯文档整理不登记代码独立 Review。

## 工作记录与当前证据

当前阶段：D0 详细方案已成稿，设计 Review 尚未开始，P1—P4 均未开始。先前“D0 已闭合、下一步 P1”的表述已校正：本轮作者自查与文档检查不替代完整设计审查。历轮源码盘点、外部事实和旧候选方案的只读独立审阅保存在[设计档案](archived/ref-2026-10-07-oal-fs-service-design-rounds.md)，不能当作当前完整方案已经通过审查的证据。

本轮进一步明确协议/实现关系：Lookup 直接调用 provider，走路只在其 backing；OAL 和 fs 都不持有领域状态。此前为了 OAL 通用执行层设计的 PathHeader、Continue 业务信封、强制 Locate/NoContinue 前置撤出当前方案。旧审阅有关 Copy 未知终态、名字订阅和真实权限的发现仍有领域依据，保留在实际 backing 的合同中；不能把对旧候选方案的修补当成必须保留该候选的理由。

本次整理将 897 行讨论版完整归档，保留当前详细目标并补上 D0、模块/消费者、工作包、删除、验证和延期门。档案记录原文件 SHA-256；它的相对链接已调整，历史事实不改写为当前状态。未运行 Rust 编译、host 测试或 QEMU，不新增代码验收结论。

本轮 D0 收口按现有协议、实际消费者和资源实现补齐：直接领域调用的边界接续、独立线形/能力槽、单 owner 的状态与付款、文件完成和服务替换/退休，以及旧模块的具体迁移表。设计检查使用 codebase-design 的知识归属与接口边界视角；没有因拆库增加新的对象真值、运行体或假消费者。另核实旧退休错误路径的实际处理，不把未具备的重试写成现状。

文档验证：七份 Markdown 的 183 个本地链接/标题锚点及代码围栏检查通过，`git diff --check` 通过；历史档案逆向还原相对链接后仍与原 897 行正文的 SHA-256 一致。按 REVIEW 完成当前设计的所有权、提交、取消、工作界、重复真值和迁移删除检查，具体反例见设计走查。没有运行 Rust 编译、host 测试或 QEMU；P1—P4 的实现与验收证据仍为空。

## 当前接力

下一步先做完整方案的设计 Review。以已确认的单向视图、provider/backing 唯一真值和库提供知识与工具为约束，审查新增细节是否仍符合这些原则；重点检查 OAL/fs/service 边界、路径接续与权限、文件/服务/设备/驱动需求的可组合性，以及是否把内存 provider 的具体策略不必要地固化成公共协议。

审查既找正确性缺口，也检查能否消除重复表示、特殊路径和没有独立需求依据的机制；保留必要的长期扩展能力。用真实调用与失败/退出场景核实，findings、处置及复核留在本篇，不另开平行设计。通过并修订计划后再进入 P1 的源码迁移；当前不开始 Rust 施工。
