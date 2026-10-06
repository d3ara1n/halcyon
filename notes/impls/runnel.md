# Runnel 实现

[`librunnel`](../../user/libraries/librunnel/src/lib.rs) 实现单工 SPSC 字节流。内核只提供 Tunnel 授权/事件与普通 View，机制见 [tunnel](tunnel.md)；页内格式、进展与双侧 Start 由本库拥有。

## 布局与角色

控制块为 128 B：RNL2 magic、版本 3/header 长度、规范化总长度、capacity、u64 head/tail、u32 EOF、0x2c producer Start、0x30 consumer Start，其余 76 字节保留。create 在发送 Invitation 前初始化完整头，再 release 发布 magic；attach acquire 验证版本、几何、游标与 Start 位型并冻结容量。每个角色的映射和协议验证成功后 release 发布本方 Start，随后 Notify；该事实独立于内核 PEER_ATTACHED。

私有 Transport 是协议与承载的边界。Guest 独占 MappedEndpoint（Endpoint capability + 普通 MappedRegion），host 使用固定宽原子控制字段与 AtomicU8 数据。Channel 持冻结容量与不可逆终态；角色保存累计 u64 进度、对端 shadow 和独立物理 cursor。角色不能重建，游标按实际复制长度模 capacity 前进，wrapping 差值只验证容量内的合法进展。复制至多两段，随后 release 发布 HEAD/TAIL；对端 acquire 后取得数据/可写范围。首次 EOF 冻结最终 HEAD；回落、终点改变或非法进展均 Broken。

## owner 与失败

Producer/Consumer::create 接受 Pool、字节数和 Placement，先出生授权，再普通 Map，最后协议初始化。attach 接受 Invitation、约定字节数和 Placement，先接入、再映射、再初始化。Create 的映射失败保留 Endpoint/Invitation；Attach 未消费时失败返还 Invitation，已接入后的 Map 失败保留 Endpoint；协议初始化失败返还完整 MappedEndpoint。执行者据当前剩余责任清理，不能把 Attach 失败一概当作邀请未消费。

MappedEndpoint 的字段私有。memory() 只在持完整 RW View 时返回生命周期绑定的 SharedMemory，不创建自引用或普通共享 slice。close 先 Unmap 再 Close，失败保存尚未结算责任，返回 owner 只供继续清理；Unmap 已成功后不会再允许 memory()。Drop 分别尝试一次，失败的空间责任仍由进程自动回收持有，不无限重试。

IoError 包含错误与已完成字节数。Notify 失败不撤回已发布数据，Invited 时 ObjectNotAvailable 不代表 Closed。协议/等待错误固定终态，随后所有操作先拒绝，Runtime 注销观察后才消费 owner。Producer 的 Started 快速返回也必须先过终态检查。

## 进展与等待

Producer 区分 Writable、Started 与 EofConsumed。可写空间不证明对端 Start；空环可在接入前写入，attach 后 acquire 读取已有数据。Consumer 区分 Readable（含首次 peer_started 标志）、Started 与 EofDrained。它先 acquire HEAD，再 acquire producer Start，防止首数据与 Start 并发时消耗旧快照却丢失首次事件；仅真实 Start 观察提交本地 start_observed。

无进展时 acknowledge DATA，再完整复检协议条件，最后等 DATA/PEER_CLOSED/CLOSED。Start 字段在每次复检中 acquire 读取，不订阅内核 PEER_ATTACHED，也不把数据量视作接入事件。wait_plan 返回 SourcePlan 值，由 libexecution 负责登记与注销；Runtime以持久成员保持来源token/任务身份，Assembly独占WaitSet接收。正式fs/pm/RootSupervisor装配与同步Caller均按该来源/续体契约接入；停止时先注销观察、等待回执收束再消费owner。

批量读写对各次等待传递同一个绝对 Deadline；超时携带累计进度，仍可明确清理或继续非阻塞推进。所有 I/O 在终态后不访问共享内存。host协议核心与目标端普通映射共用协议条件；RV64映射/跨进程观测及平台组合的证据边界由对象链记录拥有。

## 实际消费者与限制

libfs 从 StreamOffer.tunnel_bytes 建立 View；srv_fs 从第五份出生 grant 获得 Pool[CREATE] 付款，协议 Started 才推进其流就绪；init/pm 数据面同样显式创建/接入/映射。真实压力消费者通过Endpoint/Invitation、独立View、持久Runtime/Caller及Job DONE监督推进；失败值携带尚未收束的owner，由当前装配负责注销/清理，进程退出的空间责任由内核自动退休兜底。测试与运行证据、延期与接续见[对象链计划](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)。
