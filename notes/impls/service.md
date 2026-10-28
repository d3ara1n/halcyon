# 服务目录实现

服务目录由 [libservice](../../user/libraries/libservice/src/lib.rs) 拥有，与文件领域没有依赖关系。registry 的名字、注册、发布授权和订阅直接生成 OAL 发现视图，不建立文件 NodeStore、ServiceRecord 或持久 ObjectInfo。

## 协议和客户端

[protocol](../../user/libraries/libservice/src/protocol.rs) 定义 service v2、32 字节 Header、op 1—10、ProtocolDescriptor 集合、RegistrationId/Info 和名字 Watch 线形。描述集完整检查数量、重复 ID、版本范围、addressing 和尾部；运输上限为完整 u64。Register 只有 Active/Withdrawn 两个公开状态，没有 Starting、PublishReady 或公开 Draining。RegistrationId 由 registry mailbox 的真实对象 ID 与单调 serial 组成，仅作描述，管理权限来自实际 sender context。

[client](../../user/libraries/libservice/src/client.rs) 的 Publisher 直接沿已有 authority sender 调用 DelegateName/Register；RegistrationControl 和 WatchControl 直接沿各自 sender 调用。GetEntry 与 WatchName/WatchDirectory 从 Namespace 路径发领域请求，只有已验证 Boundary 才接续同一 Walk/FollowBudget，不要求前置 Lookup。GetEntry 配对返回 RegistrationId、协议描述和真实 MailboxSender；客户端本地绑定持有取得时的入口，注册替换不更新这个绑定。普通错误和传输失联均不自动重试变更。

## 授权与独立使用引用

[authority](../../user/libraries/libservice/src/authority.rs) 的 AccessTable/AuthorityTable 分别索引 DirectoryAccess/NameAuthority。每项用 Rc 状态冻结 Root/ExactName、领域位、完整输出上限和 AccountView，持有槽 Permit、角色/来源/实际存储 Charge。索引、已准入请求及独立控制共用使用引用；最后 sender 关闭只移除新请求索引，保留的使用引用仍有效并计费。显式 revoke 改共享活动位；子授权另建状态，不跟随父授权的单项 revoke。封口拒绝新准入。

角色付款由 [Funding](../../user/libraries/libservice/src/funding.rs) 冻结领域/execution 两个同账户视图，构造仅 crate 内。Root::prepare 在发行边界借用显式资金，账户不一致返回 Invalid，角色与 Root 存储从该领域账户收费；派生沿用母配对。Registration 与 Watch 独立保存配对到终态壳释放，不依赖原 authority/access 存活。Request 以真实 Kernel context 解析角色、注册或 Watch 控制的配对，用其 execution view 显式提交；普通 Outbox 明确 Task 模式，未登记且未准入的纯拒绝由既有 Ingress H 推进并保留原领域 owner/Charge。

DelegateName 和 Register 新控制 sender 的默认运输位为 WRITE|WAIT|TRANSIT，加上出口上限允许的 DUPLICATE；缺少必需位时提交前 Permission，GRANT 不自动附加。[root](../../user/libraries/libservice/src/root.rs) 先准备真实 sender/Lifetime、登记来源、安装角色，再把 sender 移给启动投递者，自身只保留 Lifetime。最后 sender CLOSED 和显式停止分别记录，前者不撤销已经准入的使用引用；来源实际注销、Lifetime 关闭后删除索引并退款。

## 注册、名字和退休

[Registry](../../user/libraries/libservice/src/registry.rs) 持有注册表及名字索引。注册拥有实际 endpoint、描述、控制 context、单调 ID、来源状态、独立可信 AccountView 和退休责任；付款绑定保留到 Withdrawn 控制壳释放，不依赖原发布 authority 存活。名字索引只指向当前 Active 注册。名字字符串由同一个收费 Rc<Name> 共享，替换转移既有名字槽，不建立第二份记录投影。

PreparedRegistration 在公开前预留注册、名字索引、描述、入口、终态承载及费用。Commit 在短独占借用内复核授权、CreateOnly/Replace 条件后安装新记录、更新名字与 Watch；替换使旧记录 Withdrawn，旧控制撤出时只在 ID 仍匹配时删名字。成功回复保存原注册 Active 快照，后来的替换不取消该回复。旧入口来源实际注销之后才关闭 endpoint；控制记录及队列引用继续计费，最后控制副本消散且来源清理完毕后才移除记录。

OAL Lookup/Enumerate 直接借用这些实际表。根是 Directory，注册叶是 ServiceEntry；叶不会冒充业务目录。游标含 registry incarnation 与 after-name，续页重新检查 Root/ExactName 范围；名字准入同时保证单项发现、分页和续请求能够表达。

## 请求与维护

[provider::Request](../../user/libraries/libservice/src/provider.rs) 接收 OAL/service kind，先按真实 context 解析 Access/Authority、注册控制或 Watch 控制的可信付款绑定，持有 Request/Outbox/Bytes/来源费用和授权引用。撤销入口保留绑定，操作时通过 Snapshot::check 返回 Revoked；陌生 context 的角色错误为 Permission，路径不存在为 NotFound，不用 .ok() 抹掉撤销。Outbox 先接管回复槽 0，领域槽位验证仍计入该已验证原槽。建立控制先登记 Lifetime；Register 另登记业务入口 CLOSED。两个来源均成功后最终 probe CLOSED、期限与授权，再提交并编码原快照。回复失败不回滚注册，剩余控制/入口 owner 继续由原请求收束。Watch 记录独立冻结准备时的领域/execution 配对，Query/Ack/Cancel 与终态壳付款不依赖观察目标引用；通知关闭释放目标后仍保持原付款绑定。

[Maintenance](../../user/libraries/libservice/src/maintenance.rs) 按预算轮转通知订阅与唤醒退休 owner，不持借用跨等待。[watch](../../user/libraries/libservice/src/watch.rs) 拥有名字/目录范围、sequence/pending、通知与控制记录；旧 Ack 不清后来变化，确认后 NotificationTake 已空只忽略 ObjectNotAvailable。永久 signal ObjectClosed 标记通知已关闭、转 Cancelled 终态并释放观察目标引用，不再重试信号；终态控制与通知 handles 仍由原 owner 保有到最后控制 Lifetime 关闭，保留部分持续计费。provider 持 typed NotificationOwner/Signaler，Owner[MANAGE] 铸造真实 NotificationObserver，以 WAIT|TRANSIT 及出口允许的 DUPLICATE 在回复槽 1 交付；出口缺所需位时 Permission。控制 sender 使用 WRITE|WAIT|TRANSIT 与出口允许的 DUPLICATE，不附 GRANT。客户端在唯一能力采用边界验证 NotificationObserver 与 WAIT/TRANSIT，持有 typed 观察角色，不消费 pending。运行证据由主计划拥有。

[名字 Watch 协议绑定](../../user/libraries/libservice/src/client/watch.rs) 为 libservice::client::Watch 实现 librpc::subscription::Protocol，拥有本领域 Query/Ack 编解码、槽位校验和 WatchInfo 投影。init 组合 ConsumerTask<libservice::client::Watch>，其通知、来源与在途调用寿命由[通用订阅客户端](rpc.md#带序号确认的订阅客户端)统一持有。应用完成 GetEntry 等实际重查后 acknowledge_read；ReadWorld 的 await_transport 在客户静止后封 Dispatcher，真实运输来源退出并唤醒客户后才完成任务。

## 进程装配与行为限制

[srv_fs boot](../../user/services/srv_fs/src/server/boot.rs) 把文件与独立 registry 放在同一 Runtime/Assembly。私有 [Profile](../../user/services/srv_fs/src/startup.rs) 选择 System 单入口、AcceptancePrimary A/B 入口或 AcceptanceSecondary 文件宿主；同一 [Capacity](../../user/services/srv_fs/src/deployment.rs) 推导 task/source 与各入口额度。Ingress、退休、释放、维护及 Dispatcher 共享 reply 由基础设施 H 支付，各入口根角色、Root task/观察与业务请求由可信配对支付。每种根报文都为空正文，按 profile 交一或两个真实 sender；Ready/Stopped 分别为零能力报文。

发行者创建并支付角色，Root owner 保存 context/Lifetime；根、必要 ingress、release、retire 来源实际安装后才能对外交付或首次注册，Ready 不依赖验收请求。停止时已领取但未移交的 sender 释放，各 owner revoke/注销。Packet 独占移交母本，init 采用前核预期 PID/context、数量及真实 kind/rights；文件、目录、发布 authority、管理根分别保存。文件业务使用 Derive 缩权；目录访问沿已有整个根 Scope::Root 角色，不声称存在远程逐路径目录缩权接口。

secondary 第四出生槽为 publication inbox receiver；init 单次 AUTHORITY_GRANT_KIND 移交 exact-name publisher sender，无 GRANT。secondary 在有限期限内核对内核提供的 sender_pid 与出生 parent_pid、kind、槽和 sender 角色后采用，关闭 inbox。必要来源安装后，用原 Dispatcher 单步 Register 发布文件入口；init/test_fs 经 WatchName→Query→GetEntry 绑定真实 endpoint。停止封文件准入，显式 Withdraw 后再封 Runtime，失联则关闭原控制让目录 owner 自动撤出，不按名字删除替代者或重放未知 Register。

[Funding](../../user/libraries/libservice/src/funding.rs) 从冻结视图一次准备请求/Outbox/回复缓冲/来源费用，长期 Request 自身存储单独持费，回复收束不提前退还控制任务存储。运输停机检查 Budget 的 fs/service Request 实际占用；Runtime/Assembly 与 backing/角色退出后检查 Budget::is_empty。host 的认证 Ready/Stopped、DONE 监督与系统故障政策由[启动实现](startup.md#init-的静态部署与根监督)唯一描述，Registry 的 Active/Withdrawn 不承担这些健康阶段。

init 将验收 B 文件/目录入口交现有 test_fs；A/B 私有 ExactName authority 分别从母根派生，经同 inbox 双槽交付。A Watch/Session 局部额度为全局一半，B 可达全局；System 单入口使用全局额度。消费者读同一 Capacity 元数据，关闭真实控制后以准入恢复核退款。流、Copy、Bind/Unbind、文件和服务 Watch 继续使用原正式消费者与领域合同；证据边界由[工作记录](../../plans/todo-2026-10-07-fal-service-redesign.md)及[装配记录](../../plans/todo-2026-10-07-service-assembly.md)拥有。
