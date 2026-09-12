# Tunnel 实现

Tunnel 是内核提供的共享内存连接对象：`Connection` 持有共享 backing 与两端关系，每个 `Endpoint` 持有本地地址空间中的 lease，`Invitation` 是一次性对端接入 capability。内核不解析页内协议；协议实现见 [`runnel.md`](runnel.md) 及对应的用户态库。

## Connection、Endpoint 与 Invitation

当前实现位于 `os/kernel/src/task/tunnel.rs`。`ConnectionState` 保存两侧 lease 与 `Alive`、`Invited`、`Closed` 状态；`Connection` 持 `Arc<MemoryObjectCore>`（对象身份、有界多 extent `ObjectBacking`、发布状态机与 backing metadata owner 的统一 core，见 [`memory-object.md`](memory-object.md)），复用 `memory_space` 的对象授权和 `WritePermit` 基元，但不向用户公开独立 MemoryObject Handle。

Tunnel 接受非零字节长度，按页取整至最多 512 页；物理 backing 最多 64 个 extent，一页仅是同一接口的退化几何。backing 由 `MemoryObjectCore` 持有——创建进程绑定的 MemoryPool 经 funded broker 支付，随对象持物理 extent 与 Pool charge。`Endpoint` 与 `Invitation` 各持 `EndpointPermit` / `InvitationPermit`，attach 端由附着进程支付，与创建端分账；backing 持 `ObjectBackingPermit`，view 所有权（对象强引用 + `ObjectViewPermit`）与 `WritePermit` 的生命周期归 AddressSpace 统一管理（见 [`mm.md`](mm.md)）。Tunnel view 是 object-owned lease：`ObjectMappingLease` 记录位置、对象内偏移与权限，撤销与退役 fragment 复核都以它为凭据；完整范围与释放事实由本篇记录。显式 close 通过内存事务在 Commit 前预留 bounded work debt；REAPABLE 后的 detached close 只提交逻辑关闭，映射资源由 ProcessDrain 收束。

`Endpoint` 是可等待对象，允许 `WAIT | SIGNAL | MANAGE`，可观察 `DATA | PEER_CLOSED | CLOSED`，不可进入 TRANSIT/GRANT。`Invitation` 允许 `MAP | TRANSIT | GRANT`，不可等待；它不可复制，成功 attach 后消费，失败不消费。Endpoint 与本进程地址空间 lease 绑定，不能通过 Handle 运输。

## Create 与 Attach

`shared/src/tunnel.rs` 定义固定宽 Create/Attach 请求与结果；两个调用都从 a0 接收请求指针，结果包含 Endpoint、实际基址与长度，Create 额外交付 Invitation。选址复用 `MapIntent::parse_placement` 的 Anywhere/FixedEmpty；PreparedMemoryChange 的 lease.range 是输出与 shootdown 的共同几何来源。

`TunnelCreate` 先为 Connection（`ConnectionPermit`）、Endpoint、Invitation、共享 backing 与两侧 Handle 预留 metadata，再在地址空间中建立创建端映射。映射、页表、输出槽、Handle 或 metadata 任一提交前步骤失败，事务回滚且不发布对象或消费资源。

`TunnelAttach` 从 Invitation 取得 Connection 的实际映射几何，在接入进程预留完整 object-backed view 和页表资源；只有映射准备、Handle 输出和 AddressSpace Commit 全部成功后，才在线性化点消费 Invitation、安装对端 Endpoint 并发布同步请求。Connection 已关闭或 Invitation 已放弃时，Attach 返回终态错误。

创建与接入的对象授权、WritePermit、MemoryChange、Remote 确认和资源退款由 [`mm.md`](mm.md) 的统一事务核拥有；本篇只记录 Tunnel 如何把这些机制组合成两端连接。Create/Attach 的映射准备收敛为 `plan_side_mapping`：锁外取得投影与 view 所有权，重入 AddressSpace 组装事务，失败路径统一走 `abandon_mapping`。

## 关闭与终止接管

显式 Endpoint close 在摘除 Handle 前预留完整 lease 撤销事务。lease 凭据不含唯一 RegionKey，AddressSpace 验证完整范围的无空洞覆盖、同一 lease/对象/权限与连续对象 offset。LeaseRetire 用固定 512 bit 覆盖集拒绝重复页并验证完整退役，不依赖单 fragment 或单 permit；强 holder 只在 Commit 内安装，预提交失败不形成 Endpoint 引用环。本端 side state 提交为 Closed，幸存端收到 `PEER_CLOSED`；本端和等待者收到 `CLOSED`，已经发布的映射不会在 stale translation 确认前提前拆除。

进程 REAPABLE 后，`close_detached` 在 Connection 锁内取走 lease 并提交 side Closed，锁外发布关闭通知；不创建 `RetiringSpaceChange`、lease retire sink 或另一笔 Unmap。Handle 阶段之后由 ProcessDrain 逐区域归还 view/WritePermit，再收束 backing/PTE。`pending_close` 只承接 Handle 摘除与 close 之间的预算切分。具体资源游标由 [`mm.md`](mm.md) 唯一拥有。

Invitation 在未 attach 前被关闭或进入 transit 清理时，连接一侧转为 Closed，并向创建端发布 `PEER_CLOSED`。Attach 与 Invitation 放弃在同一 Connection 状态锁下竞争，旧 generation 不能重放。

## 门铃与等待

`TunnelNotify` 要求 Endpoint 的 SIGNAL right，向对端置 `DATA` 电平；它只提示重新检查共享区，不携带数据计数。`TunnelAcknowledgeData` 要求 MANAGE right，仅清除本端 DATA 电平，不确认终态。调用者以 WaitMany 等待 `DATA | PEER_CLOSED | CLOSED`，醒来后重新检查页内协议。

Endpoint 的等待订阅复用通用 ObjectWaitState/WaitContext；WaitContext、TimerQueue 和 WaitMany 实现见 [`ipc.md`](ipc.md)。Tunnel 不拥有页内数据格式、游标、记录边界或 buffer ownership。

## rinlib owner 与共享访问

`user/rinlib/src/ipc/tunnel.rs::Endpoint` 独占 Handle 与只读 MappingGeometry；create/attach 工厂返回正式 owner，事件操作借用 owner 并内部组装 WaitItem，安全接口不导出原始 Handle。`raw_handle` 是明确的 unsafe 诊断边界，仅用于关闭后 generation 检查。

`close(self)` 失败返回完整 owner；Drop 单次尝试，普通错误记入饱和 abandoned 计数与 last_error，由 `cleanup_snapshot` 查询，残留 entry/映射留给 ProcessDrain。`ipc::object::close` 与 `process::abandon_to_completion` 均为 unsafe raw cleanup，不能由可构造 ABI 数值安全撤销其它 owner 的映射；compile-fail 示例覆盖这些入口及 memory 借用期间消费 owner。

`user/rinlib/src/shared_memory.rs` 提供受 owner 生命周期约束的有界视图；控制字段用 RV64 lwu/ld/sw/sd，自然对齐，acquire 在 load 后 fence r,rw、release 在 store 前 fence rw,w；数据只作 lbu/sb 字节访问，不建立普通共享 slice。asm 保留默认内存副作用，guest 只承诺 Halcyon 的正常一致性 RAM 平台边界，不声称 Rust 标准已形式化覆盖任意外部写者。证据见 [`共享访问取证`](../../plans/ref-2026-09-shared-memory-access.md)。

## 验证入口

`test_hammer::concurrent_tunnel_close` 覆盖 Endpoint close 与同地址空间普通 Unmap 的 8 轮并发；`tunnel_exit_target` 留存 Endpoint，由 stress 的 16 轮进程退出验证 ProcessDrain 接管。core 的 Running geometry 检查六种长度（含取整与最大容量）、双端不同 VA、每页共享内容和完整范围复用。init↔pm 的三页 RNL2 大流验证 64 KiB 字节传输与 EOF。

`os/kernel/src/task/tunnel/selftest.rs` 在调度开始前建立隔离 Building fixture，验证 1/2/3/512 页正式对象 view 的每页投影、发布/完整撤销及 Pool/frame/metadata 精确退款；三页请求必需多个 power-of-two extent，并检查真实 backing extent 数。Create/Attach 失败 fixture 使用三页 backing，覆盖非法长度/placement/reserved、VA Conflict、无效输出和完整 Prepare 后无 Running 提交资格的回滚。创建端持真实已提交 view，Attach 失败后核对 Invitation 保留、无目标 PTE 和 write_views 不变。表页 funding 以暂存全部可用额度触发 QuotaExceeded；测试 owner 真实占满堆，投影准备返回 NoFrame，Attach 返回 OutOfMemory，结束后释放全部压力分配，无生产故障开关。

输出竞态用确定性顺序固定：输出初检成功→真实 MemoryUnmap 撤销输出页→Tunnel Prepare→正式 deliver_output 复检失败→abandon_mapping。fixture 经 lifecycle 的 Running/active 与离场接口模拟调用者，检查 Fault 终因、Invitation 未消费、permit/PTE 回滚；最终以每批 `max_work=1` 的重复 ProcessDrain 归还全部 owner，比较 Pool/frame 和 16 类 metadata admission 库存。该用例验证真实内核事务与写回失败 seam，不依赖概率窗口。

`test_hammer::tunnel_close_attach` 在用户态执行三组各 8 轮：Attach 先完成、creator close 先完成、并发竞争。成功必须消费 Invitation；失败必须保留可关闭的 Invitation；两端关闭后重复 close 必须拒绝，每轮以三页普通映射重用两端完整 VA 范围验证 lease/PTE 已撤销。stress 的退出靶同时按字节改写独立 guest 共享区，对端作有界整数/字节采样，不在 host Rust 测试中引入混合尺寸 UB。所有新用例由 acceptance 强制锚点检查，原 [`D-1 P2-D1-03`](../../plans/archived/review-2026-09-mechanism-generalization.md) 保留提交后复核记录。用户态页内协议见 [`runnel.md`](runnel.md)。
