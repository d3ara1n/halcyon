# 文件系统抽象层

FAL（Filesystem Abstract Layer）是用户态客户端与目录提供者之间的固定宽协议。内核不感知路径、节点、挂载、属性和文件权限，只提供对象、Mailbox、Delivery、Tunnel 与等待。

协议规定授权、身份、操作结果与资源责任，领域边界同时面向当前能力与长期系统演进。后端共享共性，服务拥有组合政策；可提前为新后端、外层运行时和多绑定规划接缝或建设基础能力。实现形状由职责与变化边界决定，简化须减少相互牵制、重复状态和特殊路径，同时保留演进空间及完整责任链。当前消费者数量或抽象层数不单独决定一个机制的去留。

## DirectoryGrant

DirectoryGrant 是对 provider 内一个稳定目录及操作上限的 capability，由独立的 badged Mailbox sender 承载。provider 按内核提供的发送授权身份找到授权状态；badge 是不可变标签，不假定同 badge 必然是同一寿命实例。

请求路径只命名对象，不携带 authority。目录根是稳定对象，不是一条会随 rename 指向别处的路径。复制、转交不依赖原持有进程存活；provider 通过发送授权的 Lifetime 观察全部引用及交付责任的最终消散。具体寿命契约由 [message](message.md) 拥有。

派生只能选择当前根内可到达的目录，并收窄权限；派生结果有独立寿命。关闭父 grant 不撤销子 grant。策略撤销阻止该 grant 的新操作准入，已准入操作、已建立的流和独立派生能力按自身契约完成或取消；不隐含跨 provider 的递归撤销。

## 权限与能力导出

授权分两层：内核 Handle rights 控制 Send、Wait、Duplicate、TRANSIT、GRANT；FAL rights 控制 Traverse、Enumerate、ReadProperty、WriteProperty、ReadStream、WriteStream、Create、Remove、Watch 与 AcquireCapability。

有效权限是 grant 上限、节点支持和 provider 政策的交集。元数据描述当前 grant 下允许的操作，不是全系统通用 inode mode。uid/gid 或 ACL 不是必需前提；若引入，只参与 grant 铸造与政策校验。

读取普通属性与取得其中 capability 分开授权。Handle 类型标签只是协议提示，接收者通过实际对象描述及对应协议验证其角色。导出遵循两类政策：

- DirectoryGrant 必须由目标 provider 实际派生满足上限的发送授权；只裁剪内核 WRITE 等位无法收窄 FAL 权限。
- 其他协议的 endpoint 使用发布者明确给出的导出授权。FAL 不能猜测该协议的 badge 含义；需要按调用者进一步衰减时，调用目标协议显式提供的派生操作。

DirectoryGrant 属性出口与路径 Delegate 共享目标派生机制，但不共享权限计算：路径委派保持来访路径权限与 route ceiling 的交集；属性出口先在存储域验证 ReadProperty/AcquireCapability，再按该字段的出口 ceiling 向目标域派生。存储域的只读权限不等于目标域只能读。派生返回的协议、目录类型、槽位及实际运输权利仍须验证，不能靠 Handle 标签证明。

存储域授权还可限定导出 capability 的 TRANSIT/GRANT 转授方式，字段出口政策不得突破该运输上限；不满足时整个出口失败，不部分交付或伪改原属性。这一上限与目标能力的业务使用权分别校验，不能拿运输位掩码裁掉目标协议的操作权。Read 与 affine Take 遵守同一出口门。

## 走路与稳定位置

客户端负责 namespace、符号链接和跨 provider 组合。provider 必须从发送授权指定的稳定根出发，在该根和权限范围内解析路径；只有走到该命名空间中实际存在的委托绑定，后端才返回 Found、DelegationBoundary 或 SymbolicLinkBoundary。共享消息入口、路径文本前缀或 provider 的全局 route 表本身都不授予到达绑定的权限。

委托边界在解析时固定来源授权根、命中的绑定位置、目标母授权副本和衰减政策；已完成解析的请求不因稍后替换绑定而转向新目标，不需要另造挂载节点或对调用者暴露绑定代次。provider 随后仅据该结果异步派生 DirectoryGrant，返回消费前缀与剩余后缀。路由不能以另一个根或子根的同名路径旁路授权走路，也不能把母本直接交给调用者要求其自行限制；它不包含同步等待下游的控制循环。

符号链接只是持久路径文本，不携带 Handle、不铸造 authority，可以悬空。相对 target 在已知逻辑父位置展开，绝对 target 从调用者自己的 namespace 重启。`..` 按实际走路顺序处理，不能在遇到链接前词法抵消 `link/..`；只能回到已持有的逻辑父位置，不能要求 provider 越过 grant 根取得父目录。

每个边界报告必须恰好覆盖本次请求，满足组件边界与推进要求。整次解析限制链接次数、组件数、总字节和 provider 跳数；失败、重启和重试共同消费同一预算与 Deadline。

路径结果不是稳定授权。Create、Delete、Move 针对稳定父目录 grant 和最终名字；需要操作先前所见节点时增加身份或版本前置条件，失配且无副作用地返回冲突。Open 在同一次本地操作中确认目标、鉴权并取得节点引用。单个 provider 的最终操作有线性化点，跨 provider 解析不承诺全局快照。

携带待交付能力的修改先解析到目标 provider 和稳定父目录，再运输能力；不能把捐赠的 endpoint 沿途交给路由 provider。最终条件冲突是明确的未提交结果，普通超时不提供这种保证。

## 节点与属性

节点分目录、属性、流与符号链接，挂载点不是节点类型。目录项的名字与节点身份分离：移动改变目录项，删除名字不销毁已由 grant 或流引用的对象。删除非空目录不隐含递归操作。

属性包含固定宽整数、浮点、字符串、字节集、Array、异构具名 Record 与 Handle 引用。整个属性值，包括其所有 capability 字段，是一个一致快照；写入是整值替换。嵌套值共享一个总字节、深度、元素和 Handle 预算，槽引用必须完整、唯一且符合本次消息结构。

节点引用只保住身份，不自动冻结属性内容。需要异步导出 capability 的读取，必须先在后端的一次状态访问中取得完整内容与出口 owner，再释放后端借用并执行下游调用；不能等待期间按名字补读字段。成功派生的出口与完整回复由请求 owner 持有，部分失败、取消和回复放弃均关闭未交付能力，保留仍需退休的责任，不重新执行具有副作用的业务请求。

异步派生期间上游请求可能关闭或超时：尚未投递下游时直接归还预备授权与请求 owner，已投递后取消本地等待并处理迟到能力/回复，不把取消当作撤销下游已提交的行为。请求的额定容量与清理责任在发生业务提交前取得；正常容量耗尽不能升级为整个 provider 的不可恢复故障。

重复读取的 Handle 属性持有具 DUPLICATE 与 TRANSIT 的母本，按出口政策派生后交付。affine 值只能通过显式 Take 消费：回复成功入箱是取走的提交点，投递前失败恢复原值，不能先清空再尝试发送。属性预留期间的并发操作返回忙或等待该预留完成。

写入带 Handle 的属性先完整验证与预留，再原子替换旧值。请求尚未投递时能力仍归调用者；已投递之后由接收方承担接受、返还或关闭责任，回复丢失不意味着能力还在调用者本地。

硬链接不进入通用协议；去重与 COW 可以由 provider 内部实现。Move 只承诺同 provider、同存储事务域内的原子移动，分别核验源 Remove 和目标 Create，跨域返回 CrossDevice。Copy 的基本承诺是普通流和不携带能力的数据属性复制；跨 provider 流复制由客户端编排，失败允许部分目标，不隐含 copy+delete 或原子替换。

流 Copy 的成功需要源与目标的业务最终结果均成功，不能以字节搬运结束替代。操作分别保留读取、传输和目标已确认接受的进度；一端失败时仍负责另一端的停止与退休，尚未确认的结果明确报告未知。部分目标的处理必须区分本次创建对象与同名替代者；没有身份和权限依据时不能以失败清理为由删除目标。

已解析位置或独占 Create 的回执可以作为 Open 的预期对象身份，provider 必须在本次解析并 pin 的同一操作中条件核验；不在 Create 与 Open 间仅凭名字相信对象未变。跨 provider 的本地 NodeId 不能直接比较。Copy 默认保留部分目标和已投递 Create 的未知结果；清理由调用者显式提出，并以当前名字的身份和版本条件提交，不能以检查后无条件 Delete 制造同名替代者竞态。源与目标的 EOF/Finish 共用一份绝对期限，用户取消和任一数据端关闭须使双端退休责任继续推进。

## 服务发现

服务记录使用原子 Record，一次读取同时得到 instance、protocol/version、endpoint 和记录代次。服务发布、Ready/Draining、注册控制权和实例替换由 [service](service.md) 拥有。boot-critical 依赖仍直接 grant，不形成发现引导环。

服务目录消费通用投影接口，不要求 FAL 理解服务状态。投影由服务注册权威控制，不能通过普通文件修改绕过；发现视图失效的事件与代次由其发布契约定义，不通过扩大通用 Watch 为递归事件系统来补偿。

## Watch

每个订阅使用独立 Notification。客户端交付具 SIGNAL、WAIT 和 TRANSIT 的 signaler；provider 在一次状态修改中完成鉴权、安装订阅并取得目录代次，然后回复订阅身份、代次和有效 mask。

安装后、回复接收前的变化仍须留下 pending 位。客户端先订阅再读取快照；枚举期间发生修改可以使 cursor 失效，重读不重建一个有丢事件窗口的订阅。

事件位表示 create/delete/modify/rename，允许 OR 合并，不表达次数、顺序、名字或重放。订阅结束有独立终态位，原因可查询。Unsubscribe 验证 grant 与订阅身份的归属，确认后不再提交新信号；已 pending 的位不因此虚构为尚未发生。Notification owner 关闭结束订阅，provider 观察 CLOSED 后清理；客户端同时观察 provider 的关闭，不能只等自己的 Notification。

提交产生的失效状态与后续任务唤醒是不同责任。通知待发送并不表示消费者已被唤醒；发布者须持有预付的剩余唤醒责任，按执行预算推进，不能因为一次批量唤醒装不下就丢失尾部。取消只影响相应订阅，provider 停止则由统一退出路径接管尚未兑现的发布责任。

订阅只覆盖本 provider 中已授权的节点或目录直接成员，不隐含跨 provider 或递归 Watch。可重放、高频、带负载或递归事件系统属于独立能力。

## Open 与流完成

Open 为现有流节点建立单工连接，声明 Read 或 Write、offset、范围约束、协议和几何请求；不隐含 append、truncate、创建或原子文件替换。文件位置、范围终点和业务进度的算术溢出必须明确拒绝，不能因回绕而解释成另一个有效范围。provider 预留配额、取得节点引用、建立 Tunnel，回复 Invitation、StreamControl、协商几何和 offer 期限。

客户端 Attach 并验证 Runnel 后，通过 StreamControl 提交 Start。provider 必须确认对端已实际 Attach，且 offer 尚未到期，才允许数据任务开始；同一控制权重试已提交的 Start 须返回既有 Active 状态，不再次提交业务或要求重新打开。客户端可以通过 Query 确认丢失回复后的 Active，不把它误报为未建立。Open、Attach、验证和 Start 消费同一客户端连接期限；offer 还受 provider 的独立有限期限约束。

Read 时 provider 是 Producer，Write 时 provider 是 Consumer。非阻塞数据任务受公平工作预算驱动，背压只挂起该流。普通读取不默认承诺快照，写入允许部分完成；Finish 成功表示后端接受了相应字节，不默认表示已经持久落盘。

应用提交、共享环发布或消费、后端实际接受是不同进度。后端尚未接受的已取出字节必须仍有明确 owner，失败报告不能把传输进度冒充业务进度。已经打开的流保留稳定对象身份，名字移动或删除不把它重定向到替代者；非快照读取仍须由后端明确并发变化和范围终点的行为。

控制状态为 Preparing → Offered → Active → Terminal → Retiring。StreamControl 提供 Query、Finish 与 Cancel：Query 立即给出状态，Finish 等待并返回稳定的业务最终结果，Cancel 返回取消结果和已确定的部分进度；等待中的 Finish 不能挡住后续 Cancel。已冻结的成功不能被迟到 Cancel 改写。流引用在 terminal 和清理期间仍有唯一 owner；终态结果在控制权消散、有限会话期限和服务停止之间有明确保留边界，持有人不关闭 sender 不能使 provider 永远无法退出。

Runnel EOF 只表示数据阶段结束，PEER_CLOSED 不表示成功；业务最终状态与后端错误通过流控制协议返回，不写入 Runnel 共享头。读流在确认 EOF 和全部字节消费后取得最终结果；写流在发布 EOF 后等待 provider 消费并完成后端工作。普通客户端读到正常 EOF 必须已经确认业务成功；写入方法返回的共享环进度不能冒充 Finish。

未 Attach、未 Start、回复投递失败、控制权消散、数据端关闭和服务退出都走同一条取消或退役路径。已有部分字节不能回滚为未发生；最终结果未确认前服务退出，调用者不能推断成功。Drop 只负责放弃和清理，不等于 Finish。

## 协议边界

FAL 使用 [RpcPrefix](rpc.md) 后的固定宽版本化头、little-endian 字段和明确长度。请求 slot 0 的 send-once 由 RPC 拥有，回复没有这个保留槽；每个业务回复独立声明能力槽。Delivery 是运输责任，不进入业务槽编号。

provider 对编码、长度、路径、cursor、身份、权限和槽布局重新验证。版本升级同时替换生产者和消费者，不保留临时 anchor 或无鉴权路径。FAL wire 留在用户态框架，shared 只拥有内核与用户态 ABI。

provider 内部存储、缓存、持久性、ACL 和配额算法可以替换，但本篇承诺的授权、所有权、失败原子性和已声明能力范围必须独立成立。

## Provider 与服务编排边界

`libfal` provider 拥有协议、授权快照、后端事务、Grant、Watch、退休和这些机制需要的有界生命周期算法。它可以通过窄宿主事实接口接收唯一 State、初始能力出口和停止/失败政策，但不拥有具体服务的 route、注册状态、Dispatcher 单槽交接或监督决策。

服务进程拥有 Runtime Task 编排：Ingress 准入、Read/Delegate/Request 的下游提交与取消、route/注册控制以及进程停止。公共编排接缝可依据长期 provider 模型、独立契约或实际复用需求提前设计，不以第二个实现出现作为唯一门槛。提取应使协议机制与宿主政策各自内聚，避免双向知识依赖、重复任务队列或 Dispatcher 状态；留在宿主或进入公共库都须按这一结构收益判断。
