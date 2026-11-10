# 服务目录实现

服务目录由 [libservice](../../user/libraries/libservice/src/lib.rs) 拥有，与文件领域没有依赖关系。registry 的名字、注册、发布授权和订阅直接生成 OAL 发现视图，不建立文件 NodeStore、ServiceRecord 或持久 ObjectInfo。

## 协议和客户端

[protocol](../../user/libraries/libservice/src/protocol.rs) 定义 service v2、32 字节 Header、op 1—10、ProtocolDescriptor 集合、RegistrationId/Info 和名字 Watch 线形。描述集完整检查数量、重复 ID、版本范围、addressing 和尾部；运输上限为完整 u64。Register 只有 Active/Withdrawn 两个公开状态，没有 Starting、PublishReady 或公开 Draining。RegistrationId 由 registry mailbox 的真实对象 ID 与单调 serial 组成，仅作描述，管理权限来自实际 sender context。

[client](../../user/libraries/libservice/src/client.rs) 的 Publisher 直接沿已有 authority sender 调用 DelegateName/Register；RegistrationControl 和 WatchControl 直接沿各自 sender 调用。GetEntry 与 WatchName/WatchDirectory 从 Namespace 路径发领域请求，只有已验证 Boundary 才接续同一 Walk/FollowBudget，不要求前置 Lookup。GetEntry 配对返回 RegistrationId、协议描述和真实 MailboxSender；客户端本地绑定持有取得时的入口，注册替换不更新这个绑定。普通错误和传输失联均不自动重试变更。

## 授权与独立使用引用

[authority](../../user/libraries/libservice/src/authority.rs) 的 AccessTable/AuthorityTable 分别索引 DirectoryAccess/NameAuthority。每项用 Rc 状态冻结 Root/ExactName、领域位、完整输出上限和 AccountView，持有槽 Permit、角色/来源/实际存储 Charge。索引、已准入请求及独立控制共用使用引用；最后 sender 关闭只移除新请求索引，保留的使用引用仍有效并计费。显式 revoke 改共享活动位；子授权另建状态，不跟随父授权的单项 revoke。封口拒绝新准入。

角色付款由 [Funding](../../user/libraries/libservice/src/funding.rs) 冻结领域/execution 两个同账户视图，构造仅 crate 内。Root::prepare_minted 在采用边界借用显式资金，账户不一致返回 Invalid，角色与 Root 存储从该领域账户收费；派生沿用母配对。Registration 与 Watch 独立保存配对到终态壳释放，不依赖原 authority/access 存活。Request 以真实 Kernel context 解析角色、注册或 Watch 控制的配对，用其 execution view 显式提交；普通 Outbox 明确 Task 模式，未登记且未准入的纯拒绝由既有 Ingress H 推进并保留原领域 owner/Charge。

DelegateName 和 Register 新控制 sender 的默认运输位为 WRITE|WAIT|TRANSIT，加上出口上限允许的 DUPLICATE；缺少必需位时提交前 Permission，GRANT 不自动附加。[root](../../user/libraries/libservice/src/root.rs) 的 prepare_minted 采用已有 sender/Lifetime，复用 libkernel pair 校验核真实 receiver、sender 与 Lifetime 两级关联及权利。失败返还完整 pair；已存在的 Directory/Authority context 跨表拒绝，尚未安装的同号角色在安装前复核，返回普通错误而不进入底层表断言。Root 先实际登记 Lifetime 来源，再安装显式 Policy；装配者取出并释放仅作身份凭据的 sender，Root 只保留 Lifetime。最后 sender CLOSED 和运行体显式撤销分别记录，前者不撤销已经准入的使用引用；来源实际注销、Lifetime 关闭后删除索引并退款。

## 注册、名字和退休

[Registry](../../user/libraries/libservice/src/registry.rs) 持有注册表及名字索引。注册拥有实际 endpoint、描述、控制 context、单调 ID、来源状态、独立可信 AccountView 和退休责任；付款绑定保留到 Withdrawn 控制壳释放，不依赖原发布 authority 存活。名字索引只指向当前 Active 注册。名字字符串由同一个收费 Rc<Name> 共享，替换转移既有名字槽，不建立第二份记录投影。

PreparedRegistration 在公开前预留注册、名字索引、描述、入口、终态承载及费用。Commit 在短独占借用内复核授权、CreateOnly/Replace 条件后安装新记录、更新名字与 Watch；替换使旧记录 Withdrawn，旧控制撤出时只在 ID 仍匹配时删名字。成功回复保存原注册 Active 快照，后来的替换不取消该回复。旧入口来源实际注销之后才关闭 endpoint；控制记录及队列引用继续计费，最后控制副本消散且来源清理完毕后才移除记录。

OAL Lookup/Enumerate 直接借用这些实际表。根是 Directory，注册叶是 ServiceEntry；叶不会冒充业务目录。游标含 registry incarnation 与 after-name，续页重新检查 Root/ExactName 范围；名字准入同时保证单项发现、分页和续请求能够表达。

## 请求与维护

[provider::Request](../../user/libraries/libservice/src/provider.rs) 接收 OAL/service kind，先按真实 context 解析 Access/Authority、注册控制或 Watch 控制的可信付款绑定，持有 Request/Outbox/Bytes/来源费用和授权引用。撤销入口保留绑定，操作时通过 Snapshot::check 返回 Revoked；陌生 context 的角色错误为 Permission，路径不存在为 NotFound，不用 .ok() 抹掉撤销。Outbox 先接管回复槽 0，领域槽位验证仍计入该已验证原槽。建立控制先登记 Lifetime；Register 另登记业务入口 CLOSED。两个来源均成功后最终 probe CLOSED、期限与授权，再提交并编码原快照。回复失败不回滚注册，剩余控制/入口 owner 继续由原请求收束。Watch 记录独立冻结准备时的领域/execution 配对，Query/Ack/Cancel 与终态壳付款不依赖观察目标引用；通知关闭释放目标后仍保持原付款绑定。

[Maintenance](../../user/libraries/libservice/src/maintenance.rs) 按预算轮转通知订阅与唤醒退休 owner，不持借用跨等待。[watch](../../user/libraries/libservice/src/watch.rs) 拥有名字/目录范围、sequence/pending、通知与控制记录；旧 Ack 不清后来变化，确认后 NotificationTake 已空只忽略 ObjectNotAvailable。永久 signal ObjectClosed 标记通知已关闭、转 Cancelled 终态并释放观察目标引用，不再重试信号；终态控制与通知 handles 仍由原 owner 保有到最后控制 Lifetime 关闭，保留部分持续计费。provider 持 typed NotificationOwner/Signaler，Owner[MANAGE] 铸造真实 NotificationObserver，以 WAIT|TRANSIT 及出口允许的 DUPLICATE 在回复槽 1 交付；出口缺所需位时 Permission。控制 sender 使用 WRITE|WAIT|TRANSIT 与出口允许的 DUPLICATE，不附 GRANT。客户端在唯一能力采用边界验证 NotificationObserver 与 WAIT/TRANSIT，持有 typed 观察角色，不消费 pending。运行证据由主计划拥有。

[名字 Watch 协议绑定](../../user/libraries/libservice/src/client/watch.rs) 为 libservice::client::Watch 实现 librpc::subscription::Protocol，拥有本领域 Query/Ack 编解码、槽位校验和 WatchInfo 投影。init 组合 ConsumerTask<libservice::client::Watch>，其通知、来源与在途调用寿命由[通用订阅客户端](rpc.md#带序号确认的订阅客户端)统一持有。应用完成 GetEntry 等实际重查后 acknowledge_read；ReadWorld 的 await_transport 在客户静止后封 Dispatcher，真实运输来源退出并唤醒客户后才完成任务。

## 进程装配与行为限制

[srv_fs boot](../../user/services/srv_fs/src/server/boot.rs) 按 [Config](../../user/services/srv_fs/src/startup.rs) 在同一 Runtime/Assembly 装配文件与可选 registry。进程先由 Namespace::adopt_birth 采用完整初始前缀视图，再采用 memory.pool、file.receiver、file.management.receiver 及 registry 宿主的 service.receiver。三个 receiver 均须 READ|WAIT|MANAGE，以保留收件与运行期派生发行能力；不在宿主重新创建这些 Mailbox。Namespace 与 Pool 在整个服务生命期保留。出生采用结束前拒绝未领取输入，不接受旧 instance.report/instance.stop。

每个入口 N 采用 file.root.N / file.root.lifetime.N 与 file.management.N / file.management.lifetime.N；registry 另采用 service.directory.N / service.directory.lifetime.N、service.authority.N / service.authority.lifetime.N。完整 pair 由 libkernel 的 MintedSender::from_capabilities 与 validate 核实，sender 至少 WRITE|WAIT|TRANSIT，Lifetime 至少 WAIT。main 在任何准备/装表前一次核所有入口 context 唯一性，包括 Directory/Authority 跨表冲突；重复或错 pair 导致基础服务出生失败，不依赖 OrderedTable 的结构断言。

entries 每项必需 watches、sessions、file 和 management；registry 另必需 directory、authority。每个角色的 rights 明确领域业务位，output 明确派生能力运输上限，后两种角色另以 scope = "/" 表 Root、有效 registry 名表 ExactName。表/键/权限缺失、未知项及重复枚举均拒绝；显式空列表表示不授权，不补 ALL。settings 不决定输入 sender 的运输权利，也不选择内核付款 Pool。原入口份额继续建立领域/execution 同账户 Funding；真实 sender 和 Lifetime 内核对象的预建付款不被本地软账户替代。根 NodeRef 仍来自现有 MemoryBackend，不把角色采用变成 backing 共享。

Root owner 各自持 context/Lifetime 与来源责任。boot 先只驱动所有 Root 的真实来源登记和表项安装；这一阶段完全不创建 Ingress，早到消息留在内核 Mailbox 队列，沿既有期限、费用和背压。全部本地角色安装后释放身份凭据 sender，再创建文件、管理及可选目录 Ingress。无启动缓存、权限失败重试、动态 Export/Ready/Stopped、停止 Notification 或 Release task。运行期注册、Watch、派生授权、来源退休与退款仍沿原 owner。Capacity 依实际入口及 registry 形态推导；静态 entry 上限取 64 项出生授权扣四项固定输入后，每普通入口四项、registry 入口八项，分别最多 15/7；Namespace 的实际输入另外计入出生总限制。

不承载 registry 的文件实例由 publish 指定外部服务名，出生输入非编号 service.authority 只是主动 Register 客户端，不能与本地编号 authority 角色混用。首个已安装 file sender 移交 Register，其他身份凭据释放；它从此是注册记录真实持有的业务入口，不是本地隐藏副本。Register 与本地 Ingress 在同一 Runtime 并发推进，不等注册结果才接收入站。发布运输取首入口实际 WRITE|WAIT|TRANSIT 与其允许的 DUPLICATE，不附加 GRANT。内部故障收束仍用 RegistrationControl Withdraw；失联或未知 Register 结果不重放，不按名字删除替代者，原控制寿命负责目录撤出。

[Funding](../../user/libraries/libservice/src/funding.rs) 从冻结视图一次准备请求/Outbox/回复缓冲/来源费用，长期 Request 自身存储单独持费，回复收束不提前退还控制任务存储。Ingress、退休、维护及 Dispatcher 共享 reply 由基础设施账户支付，各入口根角色与业务请求由可信配对支付。运输停机检查 Budget 的 fs/service Request 实际占用；内部封口的 Runtime/Assembly 与 backing/角色退出后检查 Budget::is_empty。libservice::instance 仅保留通用 revoke 线格式，不再拥有实例报告或停止输入；Registry 的 Active/Withdrawn 也不承担宿主健康阶段。
