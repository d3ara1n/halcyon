# Runnel 实现

Runnel 在 `user/libraries/librunnel/src/lib.rs` 实现 RNL2 单工 SPSC 字节流；布局方向见 [`../ideas/runnel.md`](../ideas/runnel.md)。内核只解释 Tunnel 几何、映射及生命周期。

## 布局与角色

控制块固定 128 B：RNL2 magic、版本/header 长度、规范化总长度、动态 capacity、u64 head/tail、u32 EOF/flags；数据区是映射的剩余全部字节。create 在交出 Invitation 前初始化并 release 发布 magic，attach acquire 后验证必要字段并冻结本地容量，忽略保留字节。RNL1 与固定 CAP 已删除。

私有 `Transport` 是协议算法与承载之间的正式边界；guest 的 `Guest` 持 `Option<rinlib::ipc::tunnel::Endpoint>`，host 测试用固定宽原子控制字段及 AtomicU8 数据存储。`Channel` 持冻结容量、不可逆终态与清理错误；`ProducerCore`/`ConsumerCore` 持角色累计进度、最近接受的对端进度和独立物理 cursor。

角色从零建立且不能重建。物理 cursor 只按实际复制长度模 capacity 推进；累计 u64 只用于 wrapping 差值和合法前进量检查，不用于物理寻址。每次复制至多两段，先完成数据访问再 release 发布本方进度，对端 acquire 后取得可用范围。首次 acquire EOF 后取得 head 并冻结最终值；EOF 回落、最终 head 改变或非法进度均 Broken。

## owner、门铃与错误

公开 guest 接口在 `blocking` 模块：安全 `Producer/Consumer::create` 接受字节长度和 `rinlib::mm::Placement` 并返回 typed Invitation；安全 `attach` 消费 typed Invitation。内核未消费的失败保留 Invitation，已消费后的协议验证失败以 `InitFailure<Endpoint>` 返还本地映射 owner；异常几何清理由 rinlib 的 EndpointCleanup 保留。私有 Channel/ProducerCore/ConsumerCore 构造也以 InitFailure 返还 Transport，不在初始化失败时进入运行期 Broken/关闭路径。原始 ABI 工厂及 unsafe attach 函数已删除，typed 安全入口是唯一构造路径。Producer/Consumer 独占 Endpoint，不导出 Handle 或共享 slice。共享访问通过 [`Tunnel owner`](tunnel.md) 借出的 `SharedMemory`，仅临时借用映射，不创建自引用结构。

所有公开数据操作在正进展后通知，finish 在 EOF 发布后通知；Invited 期 ObjectNotAvailable 不代表 Closed，attach 首次检查取得已经发布的数据。无进展时 acknowledge → 重查 → WaitMany(DATA|PEER_CLOSED|CLOSED)，不做边沿省略。

`IoError` 同时报告协议/系统错误、已完成字节数和清理错误；批量操作累计整次调用进度，通知失败不能撤回已发布的数据。首次协议/等待错误进入不可逆终态并尝试关闭 Endpoint，失败 owner 保留在 Guest 中仅供清理，后续数据访问先拒绝。`close(self)` 失败返回完整角色；Consumer 的 `wait_peer_closed(&mut self, timeout)` 通过内部事件能力观察终态并停止后续数据访问，随后由调用者显式 close。Drop 的最终兜底由 rinlib owner 完成，不无限重试。

## 执行接入

`wait_plan` 返回不透明的 `libexecution::SourcePlan` 值，描述当前角色的等待信号，不分配 Box，也不导出 Endpoint Handle。Runtime 完成登记、generation 过滤、重 arm 与注销，角色通过 `poll` 执行 acknowledge 和状态重查。

Producer 区分 Writable 与 EOF 后的 EofConsumed；Consumer 区分 Readable、PeerAttached 与 EofDrained。对端首次建立可以和数据同批出现，Readable 携带建立变化标志，调用者撤销旧来源并用新的计划登记；建立后不再订阅持久 PEER_ATTACHED 电平。所有入口先检查终态，不访问已关闭映射。

pm 使用安全 Producer::attach；init 使用 Consumer::create 并以 typed Packet 转移 Invitation。init 的 RootSupervisor 持有数据面 Runtime、Consumer 和缓冲，建立失败的 Endpoint/Invitation 也进入预备清理槽。数据停止在注销回执后关闭角色；异常 pm 退出由 init 收束进程。实现责任详见 [runtime.md](runtime.md)。

srv_init 自检与 test_hammer 保留 rinlib 原始 Tunnel ABI 的刻意内核契约验证。真实 FAL provider/Open 尚未接通，本节不表示正式文件 Open 已完成。

## 验证

host 测试覆盖一页/多页/最大容量、空满和分段、u64::MAX 邻域的实际字节传输、畸形布局、几何 shadow、非法游标/EOF、不可逆 Broken、通知失败的读写进度、关闭失败保留以及双线程流传输。

真实 init↔pm 使用三页映射和 65536 B 数据，容量 12160 B，验证完整模式、EOF、背压与对端关闭。Tunnel geometry/多 extent/close/Attach/drain 与独立 guest 非合作字节改写由 [`tunnel.md`](tunnel.md) 的验证入口负责。FAL Open 尚未接线，不将 init↔pm 机制验证解释为正式文件流完成。

本专题组合验证与交付证据见 [`数据面档案`](../../plans/archived/todo-2026-09-memory-object-data-plane.md)。
