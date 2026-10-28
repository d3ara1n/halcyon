# 文件领域实现

文件领域由 [libfs](../../user/libraries/libfs/src/lib.rs) 拥有，发现表示及客户端路径接续由 liboal 拥有，消息、回复权和来源收束使用 librpc/libexecution。文件 backing 不持有 registry，也不建立发现投影树。

## 协议与纯值

[protocol](../../user/libraries/libfs/src/protocol.rs) 定义 fs v1 kind、32 字节 Header、op 1—21 的请求/响应、status、IoResult、StreamOffer、StreamInfo 和 WatchInfo。路径操作成功体包含 result，Boundary 使用 liboal 的两种表示；控制操作直接使用自己的固定响应体。完整长度由共享 PAYLOAD_MAX、RPC Prefix 和 Header 推导。运输字段读取完整 u64 Rights 并拒绝未知位；fs 领域位仍为 u32。实际输入槽和结果变体逐项校验，Get 的请求没有客户端返回类型。

[value](../../user/libraries/libfs/src/value.rs) 只容纳 Bool/Signed64/Unsigned64/Float64/String/Blob 及同构数组。标量 count=1，空数组保留 scalar。String/Blob 每项带长度；Float64 保留位模式。完整检查计数、元素、UTF-8、保留值和尾部后生成借用 Property；本地 FromProperty 决定客户端类型及范围转换。StoredProperty 保存一次完整编码和 Bytes charge，没有 capability、Record、Handle 或 affine 取出责任。准备和 Get 容量共同限制完整值大小。

[route](../../user/libraries/libfs/src/route.rs) 定义独立文件管理 kind 和 Bind/Unbind 线形。Bind 包含 path、FsRights、完整 output_transport 与 replace，业务输入槽1是发行者。该编码不把普通目录 context 变成管理授权。

## 单一 backing 与准备/提交

[MemoryBackend](../../user/libraries/libfs/src/backend.rs) 的 MemoryNode 只含实际 body、固定 ceiling、内部版本和目录弱父关系。body 为目录成员索引、StoredProperty、稀疏 Data 或链接 target。目录成员只有一个 OrderedTable，条目指向本地 NodeId 或 Binding；没有持久 ObjectInfo。

[NodeStore](../../user/libraries/libfs/src/store.rs) 拥有不复用的内部 serial、实际 NodeRef 使用引用、命名 link、物理记录槽和退休队列。内部记录区分 File 与 Binding，公开四种 NodeKind 不变；NodeRef 不表示 OAL 对象身份。最后命名/使用引用释放后进入预建 FIFO，Wake 通知持有者按预算调用 retire_step。目录退休逐项解除成员，Data 逐块退休；关闭失败原 payload/owner 留在记录，移到按单调期限排序的重试 FIFO，不占住其它项。本轮到期迁移和 Close 尝试均计入预算，退款在 payload 完成和记录删除后发生。close 要求 backing 封口且物理记录为空，不能把仍有记录的 Drop 当正常关闭。

Create、Binding 安装、Remove、Rename、Property Set、WriteAt 与 SetLength 各自返回 PreparedMutation。准备对象持有真实授权使用引用、实际 NodeRef、操作需要的版本或名字条件、必要 permit、预建节点/索引及收费。提交先复核授权和操作条件；封口后拒绝提交。Create 只要求原父仍在授权根可遍历范围、具有 CREATE、所选名字仍空闲及父版本可递增，不保存整个父目录的版本快照；不同名字的预付准备可依次提交，同名竞争返回 Conflict，移出授权范围返回 Permission，版本耗尽返回 Resource。数据更新和其他条件事务仍复核各自原版本。失败返还完整准备对象，成功只消费已准备的记录和索引。Create 的准备可提供本次预建 NodeRef，实例在 commit 前不在名字中可见，供 CreateOpen 在公开前保管同一目标；这不产生授权。Rename 的目标固定为收到的实际目录 grant 根，按同一 backing、兼容种类、空目录、目标父权限和弱父链防环检查；不会依靠调用者提供的数字原子域。改名转移原成员槽，替换 Binding 复用原槽；目录成员删除/替换解除弱父关系。最终提交再次按 NodeLimit 验证目的父链并检查删除/替换目录仍为空，不能以源/目的父目录的成员版本替代这些条件。链接正文按实际 String capacity 持有独立 Bytes charge。

Binding 目录成员只存稳定记录 ID，Resolved 快照只持 pin 与冻结限制。记录独占 issuer/shared typed owner，名字 permit 与记录 permit 分账；不可见准备先取得 Binding/Bytes 和物理槽，再接管能力，取消释放最后 pin 后进入同一退休链。解除链接使旧实例立即失活，封口也由 backing 活动复核阻止出口。两个普通 sender 按预算逐次释放，每项先选择尚存 issuer、否则 shared；合法叶 owner 必成关闭，全部释放才删除记录/退款，外部独立副本不计入母本 pin。部署物理容量为 NODES+BINDINGS，Binding 独立占用 FsResource::Binding。

共同名字准入函数为 `44 + 2*name_bytes + max_info_len ≤ BODY_MAX`，Create、Binding 安装和 Rename 直接调用它。Property 按最大数组面貌计入，不按当前值缩小约束。CreateOpen 使用同一 Create 准备固定名字条件与空 Stream 实例，在控制及数据来源登记后提交。

[Data](../../user/libraries/libfs/src/data.rs) 的稀疏块各有独立 slot/Charge。SetLength 增长仅改变逻辑长度；缩短一次提交改变长度/版本、清零最后保留块尾部，并留下 Trimming{first_discarded_block,cursor}。期间读只见新长度，新 Write/SetLength 返回 Busy，旧写准备因版本失配不能提交。MemoryBackend 沿 NodeStore 的实际稳定记录逐项扫描；每个 Trimming 工作单元最多选一记录并删除一块，实际删除后退款。退休、Trimming 与 Watch 三类工作轮转，调用者消散不丢清理；整体退休独占同一 Data 后，live 扫描不再处理它。清理完成发布现有维护 wake 和合并的数据变化标记；Retire 据此有界扫描流表、唤醒等待该目标 Trimming 的 writer。

## 发现与分页

Lookup/Enumerate 使用同一 MemoryBackend 遍历和授权，不要求先走外部 Lookup。中间目录逐项检查 Traverse；末项目标或父目录按操作需要与 node ceiling 求交。Lookup 不跟随末项链接；枚举及相应领域操作可产生中途或末项转发。Boundary 只在业务尚未提交时产生。

ObjectInfo 在借用期间按实际 body 编码：Directory 为 OAL Directory；Property 为 RegularFile/Property/标量，数组增加 Array；Stream 为 RegularFile/Stream 及逻辑 ByteLength；链接只有 SymbolicLink。四类最大长度为 28/88/80/28。Binding 枚举 info 为空，摘要需从实际入口查询。摘要不输出 node serial、权限证明、属性内容或链接正文。

分页按 UTF-8 字节序，用 `{mailbox_object_id, directory_serial, after_name}` 续读。初次用 path，续页只用空 path 和 opaque cursor；每次从当前 backing 重选目录并验证活动授权、存活、逐祖先 Traverse、当前 grant 根范围及 Enumerate。目录在范围内改名不失效；移出范围、被解绑且不再属于该范围、未知 serial 或实例不符返回 CursorInvalid。入口 revoke/失权仍返回 Revoked/Permission。页内只输出完整项，容量和 max_entries 限制不会生成无进展非终页；不承诺并发目录快照。

## 授权、RPC 与实际接收任务

[authority](../../user/libraries/libfs/src/authority.rs) 的 AccessSnapshot 是不可外部构造的 root/FsRights/完整出口/账户/GrantLease 使用引用。GrantLease 持共享撤销位及 grant、来源、存储收费和槽；最后 sender 关闭与显式 revoke 不混为一个事实。Issuance 分别检查 sender 必需位、新 sender 的完整运输位以及新出口上限，后两者都不得超过母 output_transport；派生领域权限不得超过母 grant 与所选目录 ceiling。根出口政策由 [srv_fs::deployment](../../user/services/srv_fs/src/lib.rs) 拥有，按 sender 与 Runnel invitation 的实际角色位构造，不使用 Rights::ALL。

[GrantTable](../../user/libraries/libfs/src/grant.rs) 按内核 context 直接保存 AccessSnapshot，访问与输入能力校验从同一表项克隆使用引用；prepare_minted 校验真实 related mailbox 和 sender identity，prepare 阶段取得每项独立 permit/charge 和预建索引。PreparedGrant 分拆成尚未发布的 GrantPublication、sender 与 Lifetime；install 只插预建表项，是授权可见点。采用者须在此之前完成 Lifetime 来源登记、回复编码与输出能力准备。prepare_derive 接受已准入 AccessSnapshot，准备与最终提交分别复核当前目录弱父链范围、Traverse、母活动位、Derive 与目标 ceiling；不因普通母 sender 消散而重新查询已移除的入口索引。输入 Rename grant 按实际 kind/完整运输位、related mailbox 与 object_id 找真实目录授权，角色错误、跨 backing 和 revoke 分别返回 Permission、CrossFilesystem、Revoked。根和派生请求各自持真实 Lifetime，普通最后 sender 关闭只清入口索引与退款；provider 停止才 revoke，已准入使用和独立子授权按各自寿命结束。

可信发行配对由 [Funding](../../user/libraries/libfs/src/funding.rs) 拥有，构造仅 crate 内；GrantTable prepare_issue/prepare_minted 借用领域与 execution 视图，账户不一致返完整 root/已铸角色及 IllegalArgument。表内 AccessSnapshot、ManagementSnapshot 和独立 Watch/Stream 壳保存同一不可变配对；派生继承母配对，显式 revoke 保留付款绑定。Request 从真实 context 的 grant/管理/Watch/Stream 绑定冻结 execution view，Ingress 用它创建 TaskSubmission；普通 Outbox 明确 Task 模式。尚未登记且未准入的纯 Quota 拒绝由原 Ingress 的 H 来源推进，原 Request 的领域 payload Charge 留到实际 owner 释放，已经 Bound/登记的责任不迁账。

[provider::Request](../../user/libraries/libfs/src/provider.rs) 按真实 message kind 选择 OAL、fs 或文件管理；kind 只选择解码，授权始终取真实 context 对应的独立表。管理请求冻结 ManagementSnapshot，普通文件请求取 AccessSnapshot；普通 sender 换管理 kind 得到 Permission。未知 kind 返还完整 ReceivedMessage。RequestContext 验证 Prefix 和真实 send-once 回复权，已知操作在采用业务输入前检查槽；Rename 与 Bind 要求输入槽 1，Header 不安全时拒收，可信头的未知操作/版本用对应协议返回 Unsupported。请求任务持 Request/Outbox/Bytes/WaitSource charge 和原账户授权快照；管理与普通接收各沿原 Ingress 机制，设施付 H，已知入口业务沿原 Funding。

Request.advance 先消费 CLOSED/timeout，再完成 Outbox 来源准入；随后一次短 backing 借用解码并处理本地操作。变更先准备和编码成功体，最终检查期限和授权，再执行无分配 commit。DeriveDirectory、Watch 和 Stream 的真实控制 Lifetime 来源先实际登记，最终复核后把能力交给 Outbox，再安装预建表项；原 Request 的 finish_reply 在回复来源实际退出后统一释放 Outbox、请求费用及准入快照，继续持新入口的寿命责任，直到真实来源注销、Lifetime 关闭与索引移除。提交后的回复失败由 Outbox 保有消息/能力/Delivery 收束，不回滚文件。支持 Get/Set/ReadAt/WriteAt/SetLength/Create/Remove/Rename/ReadLink、DeriveDirectory、Watch、Open/CreateOpen/Start/QueryStream/Finish/Cancel、Bind/Unbind 与 OAL 发现，Sync 明确 Unsupported。跨 Binding 先求领域与完整出口交集；共享入口足够窄时复制，否则沿原 Request Funding/绝对期限真实派生，不泄露过宽母本。

[srv_fs 文件任务族](../../user/services/srv_fs/src/server.rs) 的 World 直接持 MemoryBackend/GrantTable/Mailbox/账户、StreamTable/原启动 Pool、独立管理 receiver/table 及唯一 Dispatcher。Ingress 从真实文件 Mailbox 接收并调用 Request。接收前准备 MessageStorage，已收到而未成功入 Runtime 的请求由 pending 独占，拒收完整 owner 沿 RPC 释放；FileTask 把来源回执、推进、停止和期限交给原请求。pending 暴露请求期限，到期释放尚未登记的请求；Task 满载不循环重试业务，Ingress 在原任务内沿已准备 Outbox 返回 Quota，等待时暂停 mailbox READABLE，回复来源关闭后再恢复接收。Ingress 需要两项来源容量，分别用于 mailbox 和拒绝回复；停止必须收束拒绝回复的真实来源。根 Lifetime、释放 Notification 和 backing 分批退休由 [lifecycle](../../user/services/srv_fs/src/server/lifecycle.rs) 持有；boot 为文件与独立 registry 装配不同 mailbox、同一个 Runtime/Assembly，没有旧 ServiceBackend 的分流。运输 task 只持 marker、回调及最近期限，在实际领域请求和 pending call 均收束后封口/drain，真实来源清空后释放 World 内的共享运输存储。服务装配见[服务目录实现](service.md)。

每个文件 Root 在 [lifecycle](../../user/services/srv_fs/src/server/lifecycle.rs) 自己持 context、Lifetime、来源和停止/revoke 责任；同一 backing 可发行不同付款入口，不以 World 的单值根 context 选择退休对象。多账户发行与运输停机预算核对由[进程装配](service.md#进程装配与行为限制)拥有。

MemoryBackend 的 RetirementState 从实际退休队列与记录计算：Ready 表示有立即工作，Waiting 表示仍有 pin 或尚未到期的失败记录，Empty 表示所有记录与 Watch 清空。退休任务仅 Ready 保持 runnable；Waiting 等最后引用释放通知或最近重试期限，timeout 后重新推进。停止得到 Empty 后继续撤来源，实际注销回执后才 Complete。

[ManagementTable](../../user/libraries/libfs/src/management.rs) 独立登记真实管理 sender context、root、完整出口上限、可信 Funding 与活动 lease。普通 GrantTable 不含该角色。prepare_minted 核 actual related mailbox、sender 必需位及同账户视图，预备独立索引/permit/Grant、WaitSource、Bytes；Root task 先实际登记 Lifetime，再安装并交付 sender。primary 管理根报文空正文、A/B 双槽，secondary 只交付实际使用的 A；srv_fs 创建，原租户付款，init 保管、移交及关闭。init 的管理 mailbox 使用 owner-only 工厂，不再保管匿名 route peer。init 将 primary A/B 复制到原消费者交付邮箱，secondary 就绪后再交付其 A；消费者所有入口去掉 GRANT，各保管者沿既有退出路径关闭。管理 Ingress 与 Request/Outbox 接入原 Runtime；Unbind 解码后以固定原父、版本/ceiling、原 Binding ID 准备和提交。管理父目录遍历只允许本地目录，Binding 与链接明确拒绝；最终沿当前弱父链复核管理根范围。Bind 在 RPC 前取得不可见 Binding 记录、原父/名字条件和成员 permit；记录先接管 issuer，真实 DeriveDirectory 返回完整核协议/槽/运输后接管 shared，成功体预编码后才复核并提交。

[PendingOutbound](../../user/libraries/libfs/src/provider/outbound.rs) 留在原 Request，持用途、txid、冻结 Issuance、原 Binding pin 或原 Boundary（含父目录 pin 与未消费 suffix）、准备事务及临时 owner。宿主借 World 唯一 Dispatcher 与实际运输 task ID，begin_for 指定原业务 task 为 waiter；每请求最多一项下游，失败或未知结果均不重放。原 Outbox 已 Abandoned 时先取消/唤醒下游并推进原回复来源的真实注销，保留原 pending/pin/费用，避免关闭电平来源在等待期间持续唤醒；活回复未编码时不发送。取消仍等 Dispatcher.take 交付真实来源已注销的 Completion；解码采用一次后，其 service、未采用输出和错误 Packet 在同次推进由 typed owner 释放。回复最多 MESSAGE_HANDLE_MAX 个非映射能力，不保存逐 capability 关闭槽、已检查标记或叶 Close 退避期限。Bind 的迟到有效 shared 进入原不可见记录，不能提交时随 pin 释放进入统一退休；Boundary 新出口在原授权范围/原父与原稳定 Binding ID、期限和取消复核后才交原 Outbox。同名替代不会成为迟到请求的新目标。

## 流控制与数据退休

[StreamTable](../../user/libraries/libfs/src/stream.rs) 由 World 唯一持有。记录拥有业务 State、不可变 Funding、已选择的 NodeRef/AccessSnapshot、Runnel Producer/Consumer、固定起点/读末端、一个消息大小的待接受块，以及 Stream/Offer/Bytes/观察占用。控制身份是实际铸造 sender 的 object_id，不从正文 session_id 授权或选择账户。准备索引、块缓冲和费用先于 Tunnel 出生；创建失败的完整映射、Endpoint、Invitation 留在同一预建记录，由原 Request 在来源注销后逐步关闭。

srv_fs 的 A/B 根由同一全局预算签发；部署 `Capacity::entry_sessions` 按实际 profile 限制 Stream/Offer 局部占用，B 可以用尚未被 A 占用的共享容量。Bytes 先于 Tunnel 出生预留映射几何与记录/块存储；局部、共享或 Bytes 拒绝均返回 Quota，CreateOpen 准备沿原 owner 退出，名字尚不可见。Invitation 的完整输出上限还包括 R/WRITE/WAIT/SIGNAL/MANAGE/TRANSIT；只允许普通 sender 必需位的派生 grant 无法输出合法 Invitation，fs 只读权限本身不补这些运输位。

[建立与控制接缝](../../user/libraries/libfs/src/provider/streams.rs) 位于原 provider Request。创建任务持控制 Lifetime 和数据来源登记、回执及最近期限；业务与 Runnel 不在任务壳复制。Open 固定已选择实例，Read 固定末端但逐块读取实际当前内容。CreateOpen 的名字仍不可见，先备妥两种来源与回复，最终消费原准备事务并插入预建流记录；失败清理原准备，提交后丢失回复不删除名字。

[业务状态](../../user/libraries/libfs/src/stream/state.rs) 区分 Offered、Active、内部 Stopping 和 Terminal。Offered 只观察端点终态且有有限建立期限；Start acquire 观察实际 Runnel 握手后变 Active，重复 Active Start 返回现状。来源换掩码等待真实 Remove 回执，端点始终保留。Active 每次至多搬一个消息大小块；读 transported 是实际发布量，accepted 来自已确认 Tail；写 transported 是实际消费量，只有 backing commit 后增加 accepted。Trimming Busy 时保留已取出的块、临时只观察端点终态，维护完成由有界表扫描唤醒原任务后重新登记 DATA。

进入 Stopping 后不开始新块，内存后端没有在途异步写。业务不能再提交时冻结全部计数、释放目标和授权 token；Terminal 不再修改计数或理由。Completed 读流仍保留 Producer 和终态来源至数据对端关闭：最终 Tail 只证明排空，不能证明消费者末次 notify 已结束。取消、控制关闭和 provider 停止可以要求物理退休。数据来源实际注销后才 Close 映射/端点，失败 owner 和对应费用留在原记录，按 CLEANUP_RETRY_NS 期限停驻；控制关闭的 CLOSED 来源先移出观察，避免清理退避被永久电平重新唤醒。

Finish 在 Offered 返回 Conflict，Active/已开始 Stopping 只有一个等待槽，Terminal 直接复制冻结 Info。等待槽仅保存原 task/期限，真实回复权、Outbox 和付款仍在等待 Request；槽接管后显式完成原 Delivery 的入口 use，避免入口寿命与等待 Terminal 循环。超时/回复关闭仅清自己的槽，不取消会话；旧等待者不能清掉替代者。Cancel 不争等待槽。表项保留 wake 意图，原 Retire 有界投递；最后控制关闭也要等等待者取走终态或退出，才移除最小控制记录与退款。

## 路径客户端与消费者

文件订阅由 [WatchTable](../../user/libraries/libfs/src/watch.rs) 拥有。记录的逻辑 State 保存已鉴权目标的 NodeId、GrantLease 活动 token、事件序号及 mask，不持 NodeRef 或 Rust Weak；进入任何终态均释放目标和 token。控制壳独立保留原领域/execution Funding 配对、Watch/Bytes/WaitSource 收费、Counter permit 和通知角色。Create/Delete/Rename 在同一 backing 提交点只更新直接父目录，Modify 只更新实际文件；解除命名写 Unbound，实际节点进入退休给 TargetRetired，目录订阅不递归传播子节点内容。

Watch 准备阶段出生 Owner[R|WAIT|MANAGE]、Signaler 和 Observer[WAIT|TRANSIT及母出口允许的D]，准备真实控制 sender/Lifetime、索引、收费及来源；提交前完成 Lifetime 来源准入和回复容量预留，提交只插入预建记录。创建请求提交后成为控制寿命任务，回复失败仍保持这个 owner。最后控制关闭或停止后，来源撤销入队即 Parked，直到真实 unregistered 回执才关闭 Lifetime、删除记录及退款；移除失败保留原角色和记录。维护轮转检查实际 backing 状态并推送提示。Query/Ack/Cancel 以真实控制 context 查原账户；Ack 只忽略 ObjectNotAvailable，旧序号不清新提示，重复 Cancel 不覆盖原终态理由。保留 Observer 而关闭最后控制仍结束订阅。

[文件 Watch 协议绑定](../../user/libraries/libfs/src/client/watch.rs) 为 libfs::client::Watch 实现 librpc::subscription::Protocol，负责 Query/Ack 编码、完整响应校验及 WatchInfo 的 sequence/pending/terminal 投影。通知、控制、捕获序号和来源/RPC 收束由[通用订阅客户端](rpc.md#带序号确认的订阅客户端)持有；init 直接组合 ConsumerTask<libfs::client::Watch>，不存在另一份领域客户端状态机。文件目标的重查由应用完成，随后调用 acknowledge_read。

[部署元数据](../../user/services/srv_fs/src/lib.rs) 由 srv_fs package 的同名 library target 拥有，boot 的 Budget、Counter 及任务/来源容量使用同一 Profile/Capacity。Capacity::entry_watches 表达验收入口 A/B 的局部政策，System 单入口取得全额，WATCHES 拥有同 provider 全局容量；test_fs 由此驱动局部饱和、全局饱和和释放真实 A 控制后 B 恢复，每次请求使用自己的有限期限。CLOSED 仅是观察条件，真正准入恢复才核退款。该元数据不是运行时授权或额度查询，容量改为运行时装配时消费方须同步迁移。

test_fs 的 [quota](../../user/tests/test_fs/src/quota.rs) 和 [Binding 容量消费者](../../user/tests/test_fs/src/binding.rs) 使用 NODES/GRANTS/BINDINGS 界限驱动共享额度拒绝与实际再准入；Node 的总上界包括宿主所有的 root。名字撤销或最后 sender 关闭后，原 provider 继续持物理退休责任，消费者只由后继真实操作成功核可用容量。当前这三类 tenant local 与 provider global 同值；Watch/Session 的独立 local 政策另由 Capacity::entry_watches/entry_sessions 按 profile 表达，消费者不新增账户查询接口。
[Client](../../user/libraries/libfs/src/client.rs) 直接组合Namespace、RPC Exchange与FollowLimits。Get/Set/Create/Remove、小 I/O 和 ReadLink 直接发送带剩余路径的文件请求，只有已验证 Boundary 才由同一 Walk 接续；没有 DirectoryGrant、Position 或强制前置 Lookup。本地 FromProperty 检查类型和范围；纯数组通过已验证 Property 视图逐项使用。Rename 先在原 FollowBudget 中规范化完整目标路径、计入末名字，再取父目录/名字并派生目标父真实授权，把同一预算交给源提交；派生回复先校验 sender 的实际角色和完整 requested Rights，再 take 转 typed owner。请求字节缓冲先 try_reserve_exact，失败在 RPC 投递前返回 OutOfMemory，不进入 allocator abort 或重放变更。Watch 路径创建和控制 Query/Ack/Cancel 已接通，控制与 Observer 两角色按各自身份采用；Open/CreateOpen 返回 StreamOffer、控制 sender 和实际 invitation，stream_control 解码固定 StreamInfo。正式流消费由 test_fs 接入，Copy 消费由 init 原任务族接入。

## Copy 组合与清理 owner

[CopyTask](../../user/libraries/libfs/src/client/copy.rs) 持固定 `Arc<Namespace>`，通过 `Walk::owned` 复用 OAL 私有路径状态；源完成后把同一剩余预算交给目标 walk，控制请求也消费原调用额度，所有业务步骤共享同一绝对期限。路径 RPC 使用 Shared sender，控制 RPC 使用独占 sender；任务至多一项在途调用，Dispatcher 在来源实际退出后才返还 service/result。每次 advance 至多推进一项 RPC、一块搬运或一项清理，不同步泵运行体。

复制种类由 Progress 变体唯一表达；任务只持一个数据缓冲，Property 按纯值协议上界预留，Stream 按消息块大小预留。Property 分支取得一份完整纯值编码后原子 Create；进度只有编码长度和创建确认。Stream 分支依次源 Open/附着/Start、目标 CreateOpen/附着/Start、按消息大小块读出/发布、EOF、两端 Finish。目标建立后只使用实际会话，不重新按名字选对象，也没有回删。Start/Finish 核 offer 的 session 身份；Complete 要求两端 Completed/Ok 且目标 accepted、本地读出和运输量相等。未确认 accepted 的下界为零，终态与本地计数分别保存，不承诺源快照、元数据或持久化。init 的取消场景暂停客户端搬运后，用正式 ReadAt 观察目标已提交的非零前缀，再取消该 Copy 并核另一任务继续完成；这个外部观察不写入 Copy 的未确认计数。

源 Consumer 和目标 Producer 各有一项实际来源。只有当前搬运方向观察 DATA，另一端保留 PEER_CLOSED/CLOSED；切换经真实 Remove/安装回执，随后再 poll 共享 head/tail。EOF 后两端只留终态观察，避免已满足但不需要的 DATA 电平持续唤醒。注册拒绝、超时、取消或数据错误停止新搬运，保持创建 NotAttempted/NotCreated/Created/Unknown 与已确认进度；Sent 后无可信结果的创建保持 Unknown，不重放。

同一任务保留已采用的控制、待附着 Invitation、附着失败的完整映射以及两端数据 owner。取消在本地终结等待后仍领取原 txid 的完成；回复中未采用的能力、adopt 失败的普通能力及 CallError 的 Packet 都在处理作用域释放，槽数受 IPC 上界限制，不存 Reply/orphan/call_error 三种暂存值。附着尚未建立映射时返回的 Invitation/Endpoint 直接释放，协议初始化失败返回的 MappedEndpoint 继续由 partial 持有。清理先关闭两端控制，实际注销数据来源后才关闭映射/数据 owner，映射失败放回原槽并以 CLEANUP_RETRY_NS 停驻重试。Failed 的 cleanup_pending 是诊断，所有物理责任继续由原任务推进；业务已确认完成不被后续取消/关闭重试改写，成功只在清理结束后可见，最后返回 Step::Complete。

[init 的 Copy 消费](../../user/services/srv_init/src/filesystem/copy.rs) 由既有 InitTask/ReadDuty/Assembly 组合，客户段按实际数量准入独立 Copy 与唯一 Dispatcher，各任务仍独立持来源/取消/清理。ReadWorld 的 copies 按客户保存 outcome/progress/done/hold，不复制未消费的 creation。全部客户清理后由共用 await_transport 封运输，回复来源退出后唤醒原客户完成退休；两类 Watch 使用同一收尾方法。短期编排暂停保留业务期限；Task::stop 先撤暂停再交任务收束。根等待辅助遇 duty.error/提前结束立即返回原监督者，不吞掉失败。init→test_fs 的命令/报告元数据依赖单向，共同 PHASE_TIMEOUT_MILLIS 拥有阶段期限；消费者的命令等待保留有限退出界，不借用单次文件 RPC 的期限。

[init 验收消费者](../../user/services/srv_init/src/filesystem/acceptance.rs) 分别持文件根、service 目录和发布根，服务发现通过 WatchName→Query→GetEntry，取得入口后本地 Namespace.bind 再调用文件协议。[独立文件消费者](../../user/tests/test_fs/src/main.rs) 从普通共享 sender 建 Namespace，使用属性、数组、符号链接、小 I/O、OAL 分页及独立 Watch。目录签发与真实双目录改名场景由 [derive](../../user/tests/test_fs/src/derive.rs) 使用两个正式 provider 执行，核对领域/运输上限、母普通关闭后的独立 child、直接父 Rename 与非递归、跨 backing 拒绝及真实错角色输入；没有旧属性能力导出或 affine Take。运行证据和未覆盖的真实显式 revoke 时点由工作记录拥有。
