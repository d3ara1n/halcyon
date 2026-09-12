# 多页 Tunnel 与 Runnel 数据面

> 切片 8/9 与切片 10 的实现及完整组合验收均已完成，本专题归档。8/9 的固定提交范围由统一架构 Review 入口登记；切片 10 当前尚未提交，提交后另登记真实哈希。调查基线：`726fc9f`；用户已确认方案及容量/审计约束修订。
>
> 方向由 `notes/ideas/{mm,object,tunnel,runnel,shared-memory}.md` 拥有；当前实现以 `notes/impls/{mm,memory-object,tunnel,runnel}.md` 与代码为准。本档案保存交付范围与验证证据，不再安排实施。

## 切片 10 验证证据

- `just check` 通过；相关 `frame_pool`、`funded_frame`、`memory_pool`、`memory_supply` host debug/release 通过，日志 `artifacts/frame-source/{check,host-debug,host-release}.log`。
- 默认 50% 节流 `just acceptance` 通过：七面 clippy、debug stress 16/16、release core、sifive_u core、nofd 和 panic/alloc/fatal 三类启动 Failed 广播；完整聚合日志 `artifacts/frame-source/acceptance.log`。四条正常路线都要求新 frame 自检成功锚点，因此两平台四页 fixture、全范围清零、切分退款与 child 来源保活都已真实执行。
- ELF 审计通过：debug 最大单帧仍为 `0x26f0`，release 最大单帧 `0x1250`，均小于布局派生的 12KiB 限额；每 hart 256KiB 栈布局不变。退出后无残留 QEMU/GDB。
- 源码与现状文档已删除通用 raw allocation adapter/tracker；纯库存原语与来源分型 owner 保留，详细最终实现由 `notes/impls/{mm,internals}.md` 拥有。
- 独立代码复核无开放 finding，报告见 [`库存来源代码复核`](review-2026-09-frame-source-selftest.md)；复核对象为基线 `e973763` 上的本次完整未提交实现，不冒充固定提交 Review。

## 切片 8/9 验证证据

- `just check` 与七面 `just clippy` 全通过。
- os 全部纯逻辑 crate、shared 与八个用户态公共库的 host debug/release 全通过，日志在 `artifacts/data-plane/*host*.log`；RNL2 14 项模型测试、rinlib Endpoint owner 失败返还和 3 项 compile-fail 示例通过。
- 默认 50% 节流的 `just acceptance` 全通过：debug stress 16/16、release core、sifive_u core、nofd、panic/alloc/fatal 三类启动 Failed 广播，最终完整日志 `artifacts/data-plane/acceptance.log`。全速调查轮也通过，日志 `artifacts/data-plane/acceptance-full-speed.log`；不改 recipe 默认超时。
- debug kernel 最大单帧 `0x26f0`、release `0x11f0`；局部汇编标签不再切分帧统计，默认上限直接读取 12KiB guard。两平台每 hart 256KiB 栈真实启动通过。审计工具 6 项单测、stack_layout debug/release 通过。
- 内核隔离 fixture 验证 1/2/3/512 页、物理不连续投影、全部 PTE、完整撤销、零可用堆下 Commit/Retire，以及 Handle、Connection/backing/Endpoint/Invitation/view、三种 operation metadata、work debt、Remote、Quota/OOM/输出故障的失败退款与 Invitation 保留。
- Running 双端调用验证六种长度、不同 VA、每页共享内容与双方完整范围复用；真实 init↔pm 三页 RNL2 流传输 65536 B、容量 12160 B。stress 包含完整 close/Attach 矩阵、退出/drain 与独立 guest 非合作共享字节访问。
- 共享访问 debug/release 反汇编已核对 lwu/ld/sw/sd、lbu/sb、acquire/release fence，后端不调用 memcpy/memmove 或原子运行库；日志 `artifacts/data-plane/shared-memory-{debug,release}-selected.asm`。
- 旧 RNL1、固定 CAP、公开角色 Handle、单页 ABI/shootdown、单 RegionKey/fragment 退役假设与解析拆帧已删除。Create/Attach 共用完整 `prepare_side_mapping`，保留理由是消除重复编排。

后续自然序：FAL 正式服务能力由自己的计划继续设计；8/9 事后 Review 留到统一架构审查，切片 10 的代码复核针对本次完整实现。

## 推进结论与交付范围

A–E 前置已闭合，公共 MemoryObject、多 extent backing、AddressSpace 事务、Remote completion 与 ProcessDrain 均可复用，没有必须先另造内核机制的阻塞项。历史证据见 [`Review program 档案`](todo-2026-09-review-program.md)。`4b27ce6` / `8aa7bc2` 的事后复核仍由独立的 [`后续复核计划`](../todo-2026-09-design-audit-followup-review.md) 拥有，不插入本专题施工流程。

**切片 8/9 合为一次完整交付**：多页 Tunnel ABI、Endpoint owner、安全关闭边界、共享内存访问、RNL2、现有消费者与验证一起完成。编号保留作历史定位，内部可按依赖施工，但不交付“多页内核 + 单页协议”的过渡状态，不在中途做局部闭环或验收。

真实 Runnel 消费者是 `srv_init ↔ srv_pm`；Tunnel 机制消费者还包括 `srv_init` 生命周期/退出压力、`test_hammer` close/Attach/drain 与内核 Tunnel selftest。当前 `libfal::provider` 对 Open 返回 Unsupported，`librpc/libfs/srv_fs` 没有 Runnel 调用链。**本轮不承诺 FAL Open 已接线**；其 DirectoryGrant、provider 路由、服务发现与 Open 生命周期，以及“大于一页的正式 FAL 流”验收，统一由 [`FAL 服务能力计划`](../todo-2026-09-fal-service-capabilities.md) 承接。现有 FAL/RPC 仅随 raw-close 安全边界作必要调用迁移。

不引入 resize、COW、pager、KernelMemoryBudget 公共 ABI、BufferQueue、DMA/IOMMU、动态链接、普通 Pool revoke/reparent、RPC 有限 deadline 或全局 Handle 类型重构。切片 10 保持独立库存 selftest 来源收口，不承担本轮旧接口清理。

## 已冻结决策

| 问题 | 采用方案与理由 |
|---|---|
| 施工/交付单元 | 8/9 一起交付；共用 owner 与几何，直接删除 RNL1，避免临时单页适配层 |
| Tunnel 选址 | 支持 `Anywhere` / `FixedEmpty`，复用 MemoryMap 的选址语义；用户库默认 Anywhere，地址冲突测试显式 FixedEmpty |
| backing 容量 | 非零字节向上取整，最多 512 页（2 MiB），与当前有界 MemoryObject 容量一致；extent 上限使用内核现有 `MAX_FUNDED_EXTENTS = 64`，不复制 allocator 政策到线协议 |
| 对象与资金 | Connection 继续持共同 MemoryObjectCore，不公开 MemoryObject Handle；创建池支付全部数据页，各端绑定池支付各自表页 |
| lease | identity + 完整范围 + 对象连续 offset + 权限；撤销/退役不依赖唯一 RegionKey、单 fragment 或单 permit |
| 用户态所有权 | rinlib Endpoint 独占 Handle 和映射；Runnel 消费 Endpoint，只导出借用的事件能力及容量，不导出可复制 Handle 或共享 slice |
| raw close | `ipc::object::close(Handle)` 明确为 unsafe；任意可构造 ABI 结果不能作为安全清理凭据，`process::abandon_to_completion` 同步收紧为 unsafe raw 边界 |
| close 失败 | 显式 close 失败原样返回 owner；Drop 单次尝试，错误不无限重试、不 panic，记录有界进程内诊断并交给进程 drain |
| RNL2 回绕 | 保留规定的 128 B header 与 u64 累计计数；每角色另持本地物理 cursor，从零起以实际复制长度推进 |
| 共享字节 | rinlib 提供集中、可替换的 RV64 外部共享内存访问后端；控制字段按指定宽度原子访问，数据拷贝不建立普通 Rust 引用、不使用普通 memcpy 或把 volatile 当并发证明 |
| 门铃 | 每次正进展和 EOF 发布都在等待/返回前通知；本轮不加边沿省略或等待意图字段 |
| FAL | 正式 Open 独立立案，RNL2 用真实 init/pm 大流交付；不把机制测试称为 FAL Open 验收 |

以上是推荐方案的设计决策，不是尚待实施者挑选的分叉；本轮无必须再交用户拍板的产品语义选择。若施工发现这些承诺不能由既有事务闭合，先更新本计划与 COMPASS，报告具体前置；不得降级安全性或留下兼容层。

## 调查基线取证入口

- `os/kernel/src/task/memory_object.rs`：`new_tunnel_connection` 固定一页，`MemoryObjectCore::new` 已按 pages 参数化。
- `os/kernel/src/frame.rs`：`ObjectBacking::{pages,project,projection_capacity}` 与 funded owner 已多 extent；非连续物理页不要求新增来源类型。
- `os/kernel/src/task/tunnel.rs`：`reserve_mapping_resources` 已按完整 backing 投影；Create/Attach 的 shootdown 页数仍写死为 1；`LeaseRetireState::fragment_retired` 与完整单 fragment 断言需要替换。
- `os/kernel/src/task/proc.rs`：`MapIntent::object_lease` 目前仅 FixedEmpty；`plan_object_map` 已产出真实 layout，`plan_object_unmap` 仍以唯一 RegionKey 匹配；MemoryChange/RetiringSpaceChange 拥有全部 PTE/view/permit 责任。
- `user/rinlib/src/ipc/tunnel.rs` / `call.rs`：只有裸 Handle 接口；`ipc/object.rs` 有安全 raw close；`process.rs::abandon_to_completion` 接收可伪造的 `ProcessCreateResult` 并无条件 close。
- `user/frameworks/librunnel/src/lib.rs`：RNL1 固定容量、`u32 % CAP` 寻址、普通数据拷贝、公开 `handle()`；`srv_init/src/main.rs` 通过该 Handle 等待 peer close。
- `user/frameworks/libfal/src/provider.rs`：Open 返回 Unsupported；`notes/impls/fal.md` 明确与 init/pm 数据面验证分开。
- 外部契约：[`references/CONTRACTS.md`](../../references/CONTRACTS.md) 中 RVWMO、Load and Store Instructions、Supervisor Memory-Management Fence Instruction；Rust 语言边界证据见 [`共享访问取证`](../ref-2026-09-shared-memory-access.md)。

## 最终类型与所有权

```text
内核：
创建进程 Pool ──charge──> ObjectBacking(extents ≤ 64, pages ≤ 512)
                              ↑ 唯一 owner
                      MemoryObjectCore
                         ↑ Arc       ↑ Arc
                    Connection    AddressSpace per-object view owner
                    /        \          + WritePermit / retiring owner
             Endpoint A   Invitation → Endpoint B
                 │                        │
                 └── 本地 ObjectMappingLease ──┘
                       各自 AddressSpace ledger 为映射真值

用户态：
rinlib::ipc::tunnel::Endpoint { 私有 Handle, MappingGeometry }
   ├─ 借用的 SharedMemory<'_>：有界读取/写入，无普通共享 slice
   ├─ 借用的 EndpointEvents<'_>：notify / acknowledge / wait
   └─ 被 librunnel::Producer 或 Consumer 独占持有
        + GeometryShadow + RoleProgress + 本地 cursor + 协议终态
```

Endpoint 不 Clone/Copy，可在满足现有线程模型的条件下 Send；Runnel 角色不 Sync，读写及可能观察终态的等待要求 `&mut self`。不创建 owner 与映射指针互相引用的自引用结构：Runnel 保存 owner 和数值 offset，每次访问临时借用 owner 的共享访问视图。跨线程并发使用同一个角色必须由调用者串行化。

Invitation 仍是一次性可运输 capability；内核是 consume-on-success 的真值。用户态本轮可继续用裸 Invitation 值接消息/Grant，因其本身不暴露共享映射安全引用；失败保留原值的责任必须在 API 中明确。从 raw 值构造 Endpoint owner 只能为私有或 unsafe，不开放安全的任意 Handle owner 构造器。

## Tunnel ABI 与几何

在 `shared/src/tunnel.rs` 定义固定宽、无隐式 padding 的结构，保留调用号 0x60–0x63，删除旧签名：

```text
TunnelCreateRequest（32 B）
  bytes:u64, address:u64, result_address:u64, placement:u32, reserved:u32
TunnelAttachRequest（32 B）
  invitation:Handle, address:u64, result_address:u64, placement:u32, reserved:u32
TunnelEndpointResult（24 B）
  endpoint:Handle, base:u64, bytes:u64
TunnelCreateResult（32 B）
  local:TunnelEndpointResult, invitation:Handle
```

Create/Attach 均从 a0 接收请求指针；Attach 输出 TunnelEndpointResult，Create 输出 TunnelCreateResult。Notify/Acknowledge 的机制语义不变。用户态 safe wrapper 在成功返回后才解释结果。

- `bytes == 0`、向上取整溢出、未知 placement、非零 reserved、Anywhere 带非零 address、FixedEmpty 非页对齐/越界等为 IllegalArgument；超过公开页数上限或实际 extent 上限为 ReachLimit；池不足为 QuotaExceeded；物理/堆分配失败为 OutOfMemory；冲突保持 AddressConflict。
- `Anywhere` 使用正式地址空间分配器；不在 rinlib 设递增 VA 全局表。复用/提取 MemoryMap 的纯选址解析，不复制另一份数值校验。
- `MapIntent::object_lease` 接受正式 MapPlacement；从已预留的 MemoryChange 取得将发布的 lease.range，作为**结果与 shootdown 的共同几何来源**。不能继续用请求 VA 或自行再算一份几何。
- `TunnelEndpointResult.bytes` 是内核规范化长度；创建方验证它等于请求的页取整长度，Attach 方完全以返回值为准。两端 VA 可以不同。
- 没有 guard、控制页、对象 offset 输入或第二份 reservation 几何；本地 lease 恰覆盖完整 backing，权限始终 RW。
- 继续使用现有 Tunnel 的 Commit 前输出复检/写回与 Commit 后完成语义，不另造 cookie 协议。输出在 syscall 成功前没有发布效力，失败内容不可使用；输出复检 Fault、线程/进程终止仍走正式结果/终止路径。
- ABI size/alignment/offset、reserved 拒绝与 rinlib 解码同步验证。MemoryObject/Tunnel 的公开上限保持一个明确来源关系，不新增可独立漂移的内部 backing 数字。

## 内核事务、lease 与容量

Create：解析完整请求 → 从创建者绑定 Pool 建零态 backing → 预留 Connection/Endpoint/Invitation/Handle → 对象锁内取 view authority/WritePermit → AddressSpace 预留完整几何 → 锁外供给全部表页 → PreparedMemoryChange → completion/Remote/work 预留 → 输出复检 → 同一提交点发布 mapping、lease、双方关系和 Handle → Synchronize/Retire/Complete。

Attach：先校验 Invitation role/rights → 在 Connection 锁内确认仍 Invited 且创建端 Alive → 以上同形 view 事务 → 同一提交点消费 Invitation 并发布 Endpoint。任何 Commit 前业务失败保持 Invitation 可重试；格式检查失败发生在用户态 attach 已成功之后，不能谎称 Invitation 未消费，此时返回协议错误并关闭新 Endpoint。

不增加 Tunnel 私有 plan/complete 类型、第二套 backing owner、回滚矩阵或页表调用面。失败继续走 `abandon_mapping` / 正式 rollback owner；对象来源与 WritePermit 必须在 AddressSpace 锁外归还。

lease 的最终凭据是 `LeaseKey + range + ObjectId + object_offset + protection`，去掉仅为唯一 region 匹配保存的 RegionKey。`plan_object_unmap` 验证 ledger 对完整 lease 的无空洞覆盖、相同 lease owner/对象/权限，以及按 VA 递增的连续 object offset；多 extent 只影响翻译投影，不制造数据 backing 所有权分叉。

`LeaseRetire` 使用有界页覆盖证明替换 `fragment_retired: bool`：在 close 预备阶段清空最多 512 bit 的覆盖集；每个 retiring fragment 检查子范围、对象 offset 与权限，拒绝重复覆盖；finish 要求恰覆盖完整 lease。覆盖集是退役验证状态，不持资源；view/permit 的真正归还仍由 AddressSpace 完成，sink 不按“回调一次”等同于“许可全部归还”。无需预设 fragment 回调顺序或每页一枚 permit。

显式 close 在摘 Handle 前预留完整 lease 撤销、table、work 与 Remote 容量；成功返回意味着本端映射已按正式事务完成撤销。对端映射继续有效，终态通知使协议停止信任内容。REAPABLE 后 detached close 只结束 Connection 关系，ProcessDrain 继续原 ledger/owner 游标，不建第二笔 Unmap。Connection/backing 可在双方 Handle 消散后继续由 retiring view 保活。

容量与退款：

| owner/资源 | 来源与上界 | 退款/释放点 |
|---|---|---|
| backing | 创建者 Pool，最多 512 页/64 extents；现有 ObjectBackingPermit | 最后 backing owner 消散；逐 extent 归还数据页与 charge |
| Connection | 创建者 sponsor；现有全局 512、每 sponsor 16 | 最后 Connection 引用消散 |
| Endpoint / Invitation | 各创建操作的 sponsor；全局 1024/512，每 sponsor 32/16 | 对应对象与 retire 引用终结 |
| view / WritePermit | 各端 AddressSpace；共同对象状态机，正常两端各一写 view | Synchronize 后正式 retire 或 ProcessDrain |
| PTE/table | 各端绑定 Pool；由真实多 span preflight 推导 | rollback 或正式 table retire/drain |
| 投影、结果与覆盖证明 | Commit 前有界预留；投影最多 64 span，覆盖集最多 64 B/Endpoint | rollback 或对象收束 |
| completion/Remote/work | 复用现有每笔事务 admission 与有界槽 | 同一 Complete 点兑销 |

不引入 Tunnel per-sponsor 页预算这一第二资源账本。512 页清零仍在 Commit 前、AddressSpace 锁外；backing 最后析构最多 64 个 extent，各 extent 的归还上界须连同池深度在实现验证中核对。扩大页数不放宽内核路径恒短或 Commit 后零分配规则。

锁阶保持 `HANDLE_TABLE(100) → CONNECTION(220) → MEMORY_OBJECT(250) → ADDRESS_SPACE(300) → LIFECYCLE(600)`；对象锁在 AddressSpace 外取得。completion 内摘出状态后才能进较低秩的 wait/对象锁，不能在诊断或析构中反向取锁。锁外供给和退款复用现有 seam。

## rinlib 安全边界与关闭

- Endpoint 工厂 `create(bytes, Placement)` / `attach(invitation, Placement)` 返回 typed owner 和必要的 Invitation。MappingGeometry 私有字段、只读访问器；拒绝内核成功结果中的非法几何，不以错误几何构造安全访问视图。
- `EndpointEvents<'_>` 只提供 `notify`、`acknowledge_data`、`wait(signals, timeout)` 等操作；内部组装 WaitItem，外部不拿可复制 Handle。Runnel 将该能力封装在内部，向 srv_init 提供 `wait_peer_closed(&mut self, timeout)`；观察终态时同时使角色停止数据访问，不允许上层直接清除协议 DATA。后续若需要混合 WaitMany，另以带生命周期的等待项组合，不在本轮导出 raw WaitItem 绕过借用。
- `close(self) -> Result<(), (Self, SystemCallError)>`；失败返回相同 owner 与几何。Runnel 的消费式 close 同样保留失败时的完整角色状态。调用者决定有界重试/升级，不在库里无限 sleep 重试。
- Drop 至多提交一次 close；Commit 前错误停止重试，进程内饱和计数与 last_error 可查询，资源仍留在 HandleTable/AddressSpace 供 ProcessDrain 接管。参考 `rinlib::thread::stack_cleanup_snapshot` 的诊断职责，不建立后台清理队列或新 registry。
- `object::close(Handle)` 改 unsafe，Safety 要求调用者拥有该 entry 的关闭权且不会使现存安全 owner/引用失效；内部 typed leaf close 继续按自己的不可失败契约工作。
- `process::abandon_to_completion(ProcessCreateResult)` 也是 raw cleanup，必须改为 unsafe 并明确要求两枚 Handle 是本调用者独占、未被 Start/其它路径消费的真实 Create 结果；libprocess 的 safe spawn 在内部凭真实 Create 来源调用它。不能只给内部裸 close 包 unsafe 而保留外部可伪造的安全入口。
- 全仓逐一核对 raw close 来源。librpc 私有 ReplyPort、send-once、真正 receive 结果，libprocess 内部 create/duplicate/derive 和 JoinHandle 私有 ThreadSpawn 结果可在有来源证明的局部 unsafe 中关闭。公开可构造的 ReceivedMessage/Reply 不自动成为凭据，不能新增“接受任意 message 并安全关闭 handles”的公共 helper。
- `collect_process` 接收 raw control，但其成功 close 前已有 WaitMany/Drain/Query 的角色验证，Endpoint 无法通过；记录该证据并保持验证，不为本专题改造所有 ProcessControl/Job API。Generation 不复用保证通过验证的旧 Handle 不会变成新 Endpoint。
- ABI 负面/竞态测试可以显式使用 unsafe raw close，必须标注唯一责任或故意打破协议的测试边界；正常服务和 Runnel 不依赖这条路径解除 Endpoint。

## 共享内存访问边界

正式 RV64 后端放在 rinlib 的 `shared_memory` 模块，由 Endpoint 借出带生命周期和已验证长度的访问视图。不得把不可信映射转换为 `&[u8]`、`&mut [u8]` 或由对端可破坏值域的 Rust 类型；共享字段采样为整数，完成本地检查后才解释。

- 控制字段：固定偏移、自然对齐的 32/64 bit load/store，按 wire 规定的 Acquire/Release/Relaxed 操作；不使用 AtomicBool，不对控制字段作不同宽度的重叠 Rust 原子访问。
- 生产后端把实际共享地址访问集中为 RV64 asm：32/64 bit 控制访问及 byte 数据 load/store；acquire 在 load 后执行 `fence r,rw`，release 在 store 前执行 `fence rw,w`。asm 不声明 pure/nomem/readonly；编译器须保留其内存副作用与同步边界。所有共享访问使用经范围检查的地址，Rust 普通引用仅用于本地输入/输出缓冲。
- 首版 byte 拷贝可从 `lbu/sb` 的明确访问宽度实现，循环/批量组织为一个可替换后端；不承诺普通 memcpy 性能。后续宽拷贝优化必须保持相同范围、并发与排序契约，不改 RNL2。
- 初始化同样走后端的指定宽度字段 store 和保留字节写入，不能通过 `write_bytes` 建立另一条共享访问路径。
- 合规双方：发布顺序证明完整字节流和回收顺序。恶意对端：可能破坏数据完整性、伪造进度或拒绝服务；本端仍只访问 owner 保活的有界映射和本地合法缓冲，不从共享数据构造非法 Rust 值，不把所有作弊都可检测作为承诺。
- 这是 **Halcyon RV64 + rustc/LLVM 的平台契约**，不是 Rust 标准已经形式化证明任意跨进程并发。正式承诺依赖后端代码生成和 ISA 核对；host 模型只证明合规原子模型下的协议，不冒充对恶意外部进程的语言证明。
- host 测试用永久的原子存储后端：控制字段同宽 AtomicU32/U64，数据 AtomicU8 Relaxed；用合规原子写模拟畸形输入。实际混合宽/非原子恶意写由独立 guest 进程做平台测试，不能在 Rust host 测试中故意制造 UB。

语言/平台证据与验证边界见 [`共享访问取证`](../ref-2026-09-shared-memory-access.md)。

## RNL2 状态与算法

布局精确遵守 `notes/ideas/runnel.md`：固定 128 B header、RNL2 magic、version/header_bytes、total_bytes/capacity、u64 head/tail、u32 EOF/flags；capacity = 内核返回 bytes − 128，且小于 2^63。格式直接替换 RNL1。

初始化由 create 工厂完成，在返回 Invitation 给上层前以 release MAGIC 发布。Attach acquire MAGIC，然后每个几何字段只采样一次，验证版本、长度、容量和 flags，冻结 GeometryShadow；保留字节由写方置零、读方忽略（与共享协议公共契约一致），未知 flags 拒绝。后续寻址不再读取共享几何。

角色在一条连接上只建立一次，不提供断线重连或从任意进度重建 wrapper：

- 生产者持 `head:u64, tail_shadow:u64, cursor:usize, eof:bool`；消费者持 `tail:u64, head_shadow:u64, cursor:usize, eof_head:Option<u64>`。cursor 是该角色实际完成的总字节数对 capacity 的余数，与 u64 计数回绕独立。
- 两角色的本地 cursor 均从零开始；attach producer 必须看到尚未使用的本方 head/eof 与合法 tail 初态；attach consumer 的本方 tail 必须为零，允许 creator producer 已发布不超过 capacity 的首批数据/EOF。拒绝把一个任意旧游标解释成新角色。
- `used = head.wrapping_sub(tail)`；生产者只接受 tail 的前进量不大于旧 outstanding，消费者只接受 head 的前进量不大于旧 free，随后再次保证 used ≤ capacity。每次操作也核对本方共享计数没有被外端修改。
- 复制从**本地 cursor** 起至多分两段，count 不超过本地缓冲和已验证的 available；复制后 `cursor = (cursor + count) % capacity`，计数 `wrapping_add(count)`。cursor+count 由容量硬界保证不会溢出；禁止共享计数 `% capacity` 寻址。
- 该规则不需要额外 wire cursor/epoch：双方角色均从零产生，同一逻辑字节的物理位置归纳相同；任意 u64 回绕只改变计数表示，不改变物理 cursor。
- 写完数据再 release head；读完数据再 release tail。消费者先 acquire EOF 再 acquire head，首次 EOF=1 冻结 final_head；已观察 EOF 后 flag 回落、head 再前进均 Broken，只有 tail 追上冻结 head 才正常 EOF。
- 任何已观察的终态/协议错误进入不可逆 Broken/Closed，停止共享访问；尝试消费式关闭 Endpoint，关闭失败保留仅供清理的 owner，不允许恢复传输。协议错误与清理错误分别可观察。

门铃与错误交付：

- 所有正式公开读写入口在产生正进展后执行 notify，避免只有 write_all/read_exact 才通知的隐式模式混用；纯协议算法作为私有实现及 host 模型存在，本轮不开放独立轮询模式。
- 每个正进展都通知；无进展时 ack → 重查 → wait(DATA|PEER_CLOSED|CLOSED)。Runnel 的公开终态等待经过角色状态机，不给使用者清 DATA 的自由入口。
- notify 在对端仍 Invited 时返回 ObjectNotAvailable，可视为没有现存接收者需要唤醒：Attach 的首次条件检查会发现已有数据/EOF。真正 ObjectClosed 则进入终态；不把它等同于 Invited。
- 若本次已发布 count 后 notify 失败，错误必须携带已完成字节数；write_all/read_exact 的错误也报告整次调用已完成量。不得用一个无进度错误让调用者重试整段并重复写入。EOF 发布后通知失败同样不能撤回 EOF。
- 不承诺在尚未观察关闭通知前，每一次数据访问都与对端 close 线性化；本端 mapping 的存活由自己的 Endpoint 保证。观察关闭后即停止数据访问。

## 消费者、失败与组合验证

源码及消费者全部迁移后统一验证；实施途中不为单个 crate 拼过渡适配层或做局部验收。

| 验证面 | 必须提供的证据 |
|---|---|
| ABI/几何 | 一页退化、非页对齐长度取整、2/3/最大页数、0/溢出/超限、Anywhere 与 FixedEmpty、两端不同 VA、完整返回长度 |
| 物理投影 | 确定性制造多 extent，并跨 extent/页表边界读写；不能只凭请求多页推断物理多段 |
| lease/退款 | 完整区间及连续 offset 校验；重复/缺失 retiring 覆盖拒绝；两端关闭后全部范围可重用；不只检查首个 PTE |
| 失败矩阵 | VA 冲突、输出初检/复检故障、QuotaExceeded、OOM、Handle/permit/metadata/Remote/work 不足；失败后 Invitation 保留、全部库存精确恢复 |
| 并发/终止 | Attach 先、close 先与竞争；跨 hart 双 close；Commit 前后 kill/退出；max_work=1 ProcessDrain 接管；最终无孤立 work/owner |
| owner 安全 | 编译失败样例证明角色存活期不能消费 Endpoint、不能得到 raw close 的安全通道；任意 ABI 结果不能调用安全 abandon；测试显式 close 失败返回 owner与 Drop 诊断 |
| RNL2 算法 | 空满、分段、多圈、不同 capacity；把计数置于 u64::MAX 附近并设置与历史一致的独立物理 cursor，跨回绕逐字节核对；仅测差值不算 |
| 对端不可信 | 畸形/变化几何、非法 flags/EOF、倒退/跳跃/篡改本方计数、EOF 后变化；Broken 后不再访问共享区；guest 外端并发改写仍不越界 |
| 门铃 | 发布/ack/重查/入睡各竞争次序，无丢唤醒；Invited 期已有数据/EOF；notify 失败报告已发布进度 |
| 真实消费者 | init↔pm 使用多页映射，数据量超过多倍容量（建议 3 页映射、至少 64 KiB 流），完整模式校验/EOF/背压/关闭；全范围 VA 常量与锚点同步 |
| 工具链 | RV64 debug/release 后端反汇编核对访问宽度、fence、无共享 memcpy/原子库锁；单帧默认上限由 ELF guard 派生为 12KiB，两平台物理栈均为 256KiB，不为无依据的经验阈值拆帧 |

多 extent fixture 使用正式单页 funded owner：保持第二个 claim 并归还第一个，在最早库存位置留下不能与 buddy 合并的洞。三页 backing 先取两页 extent，再取最早单页洞，fixture 检查真实 span 不按逻辑顺序物理连续，并逐页核对全部 PTE 投影；不需要捕获全部库存或引入 raw allocation。所有压力持物在结束前归还，不引入生产故障开关。资源守恒在隔离 fixture 的静止点比较 Pool/frame/PTE/Handle/WritePermit 和 16 类 metadata；共享长期服务中的自然波动不能冒充精确退款证据。

验证命令：相关 host debug/release（显式 `aarch64-apple-darwin`）、shared ABI、`just check`；全部接通后运行 `just acceptance`，覆盖七面 clippy、debug stress、release core、sifive_u、nofd、boot-failure。若改变调度域契约另跑 virt-hetero。后端代码生成核对附完整日志；按已知竞态 flake 的规则判读复跑，不把超时当内核挂死。结束确认没有残留 QEMU/调试器。

## 施工顺序与删除门

1. 对照本计划与方向文档冻结 shared ABI、Endpoint/事件/共享访问类型、lease 覆盖图和错误进度类型。
2. 接通 shared → syscall → MemoryObjectCore/AddressSpace → Tunnel 多页几何与 lease 撤销；删除单页 shootdown、单 RegionKey/fragment 假设。
3. 接通 rinlib Endpoint 与共享访问后端；收紧 raw close / raw abandon 的公开安全边界，同步真实来源调用点和诊断。
4. 替换 librunnel 为 RNL2，接通 owner、独立 cursor、EOF、门铃与部分进度错误；迁移全部 init/pm/hammer/selftest 消费者。
5. 删除 RNL1、固定 CAP、公开 role.handle、旧 Tunnel ABI、共享普通拷贝、未使用 adapter 与过时注释；同步 impls 描述最终实现。
6. 完成以上全部连接、失败和退出路径后，执行组合验证及全仓残留搜索；验证全部通过才标记 8/9 完成。
7. 向用户展示 diff/摘要后取得提交授权；提交后按真实哈希登记未来 Review，不在施工中插入 Review。

不设置临时 adapter 的保留期，因为本方案不引入临时接口；分片代码可以暂未接通，但必须是上述最终类型的一部分。切片 8/9 的验收不勾销独立任务；切片 10 的独立完成证据见本档案开头，FAL、RPC deadline 与最终架构 Review 仍由各自入口拥有。

## 切片 10：库存来源与启动自检收口

用户已确认完整方案并授权实施；整体连接与旧机制删除完成后统一验证，不做局部验收。全部完成门已通过，证据见本档案开头。

- 最终物理来源分型：普通 claim 由 `ClaimedUserExtent` 与 `MemoryCharge` 合成 funded owner；`BootHeldExtent` 直接持 `Option<ExtentGeometry>`，以唯一 unsafe adopt 接管未入库存的启动范围，切分保持相邻且不重叠，Drop 回投，不清零启动内容。删除通用 `FrameTracker`、`alloc_user_order`、`publish_claimed` 与 main 的早期 raw 自检；保留纯库存 order/largest 原语和正式 clear。
- 自检统一位于 `frame/selftest.rs`，boot 在 root 建立后、Ready 发布前调用。正式入口检查三页总几何、全范围零态、单页表 owner、完整 Pool 快照和 frame 守恒；一 extent 三页失败验证真实退款。
- 私有 `DirtyInventory` 只委托正式库存来源，锁外把同一个独占 claim 全范围写脏并读回，再由真实 broker clear；逐字节零态证明不依赖重取相同 PA。它是永久测试来源端口，不是生产 adapter 或故障开关，不另建 raw 分配/归还路径。
- 四页单 extent fixture 经正式 owner 转换后切成一页与三页，两种释放顺序都检查部分退款、存活一侧内容可访问和最终退款。连续四页是当前两平台启动供给前置，不扩大分配 ABI 承诺；不得为测试新增指定 PA 原语。
- MemoryPool 现有 child 自检用 child 支付真实表页，核对 root delegated/child allocated，删除外部 child 强引用后 funded owner 继续保活来源，最后物理 owner 与 charge 析构才触发父级退款，不开放 child 构造 API。
- host 库存测试补部分重叠归还的修改前拒绝；broker 切分测试补逐 owner 释放后的中间账本。精确析构次序和失败前不清零由 host 模型证明，不把启动静止点快照夸大为事件次序证据。
- 新统一成功锚点仅在全部 frame 自检通过后输出，并纳入正常 QEMU required；boot-failure 继续独立判定。同步 `notes/impls/{mm,internals}.md` 与导航/Review 当前观察登记，历史归档和只读参考不改。
- 整体完成门：相关 host debug/release、`just check`、全仓 clippy 与完整 `just acceptance`，核对 debug/release 帧审计和两平台四页 fixture；结束确认无 QEMU/GDB 残留。源码与现状文档无旧 raw adapter/tracker，BootPackage 内容与真实退役路线保持正确；通过后归档本计划并更新 COMPASS。提交仍需展示摘要后取得独立授权，未来代码 Review 只登记真实提交。
