# 进程内存绑定与 root bootstrap 未来审查

> 【未来审查计划】审查对象固定为提交 `7c76097aa957abe14246191fb9bc097169f17fb2`（`feat(mm): 闭合进程内存绑定与根池启动链`）。只审该提交形成的 Unbound Process shell、一次性 ProcessBindMemory、Building 截止、AddressSpace root 资金化、root MemoryPool capability 交付与 BootPackage payload owner 闭包；切片 6 的中间页表/普通匿名 backing 全面资金化、公共 MemoryObject、多页 Tunnel 与 Runnel v2 不混入本结论。

## 对象概要

该提交把 ProcessCreate 收窄为 page-resource-light 的 Building shell：稳定 Process identity、空 HandleTable、`AddressSpaceState::Unbound`、ProcessBuilder、ProcessControl 与内部 MetadataSponsor，不创建 ledger、页表或 Pool charge。Process core、Builder 与 Control 分别持有按真实寿命退款的 metadata permit；Builder 最后 authority 消散触发 abandonment，Control 可在 core Dead 后继续保存终态快照。

`ProcessBindMemory(builder, pool)` 是唯一 `Unbound → Bound` 操作。调用在精确 Building 下登记 operation lease，串行化同一 shell 的竞争 Bind，以同一 HandleTable pin 保护 Builder 并暂存待消费 Pool entry；锁外准备 AddressSpace permit、单页 funded root、shared root 与 ledger，再在 `HANDLE_TABLE → ADDRESS_SPACE` 双锁提交段同时发布 Bound 状态和逻辑消费 Pool entry。提交前失败恢复 pin、保留 Pool Handle 并自然回滚 permit/frame/charge；成功后 PoolBinding 不可转移，随 AddressSpace 有界 drain 在锁外退休。

Building admission 统一覆盖 Bind/Map/Write/Grant/Attach/Start。Start 只有在自身是唯一 `building_ops`、AddressSpace 已 Bound 且活体门成立时才能发布；后到终止关闭新登记并等待既有 lease。已登记 Attach 若在提交前遭遇终止截止，仍消费 tid 并构造线程，但不再插入 Staging，而由终止路径在 lifecycle 锁外直接析构。

bootstrap 先创建 init Unbound shell 并复用普通 Bind helper。StartupBlock 固定交付 root JobControl、SystemReset、init ProcessControl 与 root MemoryPool，后者和内部 PoolBinding 指向同一 root core。BootPackage payload 由 `BootHeldExtent` 与 root charge 先合成 `BootFundedExtent`；prefix 在任何 ledger/PTE 发布前直接回填，可失败映射期只安装 `BootBorrowed` 几何投影，成功后无分配地移交 owner 本体。初始 ELF 与 package prefix 回投库存，payload 随 init AddressSpace 在锁外同步归还物理 extent 与 charge。

## 审查重点

1. 逐分支复核 ProcessCreate 发布事务：sponsor、Process Arc、Builder/Control permits、Job member reservation、两个输出 Handle 槽与 uaccess 任一点失败都不得留下 Job 成员、Handle、permit 或 page charge；成功前不得让 shell 对其它 hart 可见。
2. 复核 Process core/Builder/Control 的 metadata admission：global/local 部分取得失败必须回滚；permit owner 必须与真实对象寿命一致；Dead core、Builder consume/close、Control shell 延寿与 JobDerive 重新铸造 Control 均不得提前退款、重复退款或绕过全局上限。
3. 追踪 Bind 的 authority 与 alias 检查：Builder 必须是 ProcessBuilder role 且具 MANAGE，Pool 必须是 MemoryPool role 且具 GRANT；同槽 alias、重复 transfer、同 generation pin 冲突与 stale generation 必须在任何资源消费前 fail closed。
4. 重建 Bind 的线性化证明：Building lease、`bind_in_progress`、Builder/Pool pin、完整 BoundAddressSpace preparation、双锁提交和锁外 close 尾段之间不得存在双 Bind、Pool entry 已消费但 AddressSpace 未发布，或 Bound 已发布却可恢复 Pool Handle 的状态。
5. 枚举 Bind 全部失败路径：AddressSpace metadata、Pool quota、root frame、页表构造、Handle pin/moved buffer与提交前终止；核对 HandleTable、Pool 四项方程、FramePool、AddressSpace state、`building_ops` 和 bind reservation 均恢复，且 owner 析构不在 AddressSpace 锁内取得 MemoryPool/POOL 锁。
6. 复核统一 Building 截止：Bind/Map/Write/Grant/Attach 只能在精确 Building 登记，Start 必须要求 `building_ops == 1`；登记先于终止者保留提交资格，终止先于登记者返回 ObjectClosed，Start 不得越过任何既有 lease。
7. 专门审查 Attach 的截止后接管：`attach_registered_member` 在 Terminating 下不得留下 Staging 条目或未请求离场的容器成员，tid 只在成功构造后消费，Thread/ThreadDeparture 析构位于 lifecycle 锁外；lease Drop 必须使空进程最终达到 REAPABLE。
8. 审查 `AddressSpaceState::{Unbound, Bound}` 的稳定外壳：Unbound query/kill/abandon/drain 合法，Map/Write/Attach/Start 明确拒绝；Bound 的 ledger、TableTree、PoolBinding 与 epoch identity 不得形成双重真值或允许第二次绑定。
9. 逐阶段审查 AddressSpace drain：region/backing、页表与 PoolBinding 必须按有界游标推进；`BootFundedExtent` 和 PoolBinding 只能在 AddressSpace 锁内摘除，实际物理/charge owner 必须由 `RetiredSpaceResource` 在锁外释放，遵守 `MEMORY_POOL → ADDRESS_SPACE` Lock Ladder。
10. 重算 funded root 生命周期：`FundedRootFrame` 必须恰持一页物理 owner 与一页 MemoryCharge，TableMem 只能借用该 root；Bind 失败、Unbound shell 终止、正常 Bound drain 和 Process core 提前 Dead 均不得泄漏或双重归还。
11. 复核 bootstrap payload 的 owner 时序：`BootHeldExtent → BootFundedExtent` 必须先于任何借用投影；prefix 物理回填必须先于 ledger/PTE 发布；映射失败期间外层 funded owner 必须持续覆盖全部 `BootBorrowed` 几何；成功后的安装不得分配、失败或改变 base/pages。
12. 审查 payload split/retire：`BootFundedExtent::split_at` 必须同步切割物理 owner 与 charge，RegionOwner::Lease 必须阻止普通 Unmap 提前释放 primordial payload；init drain 后 FramePool 与 root Pool allocated 应同时回到对应基线。
13. 核对 root capability 同源性：root Pool seed 只铸造一个 core，init PoolBinding 与 StartupBlock Handle[3] 必须 `Arc::ptr_eq`；Handle rights 足以派生、查询、复制和供 child Bind，但不能复制额度或产生第二 root。
14. 对照 shared/kernel/rinlib/libprocess/srv_init 全调用链：syscall 号、参数、错误码、StartupBlock 固定索引与 `SpawnRequest.memory_pool` 必须两侧一致；loader 只能复制 GRANT-only Pool authority，Bind 成功消费复制件，失败清理不得误关调用者原 authority。
15. 复核 bootstrap 与普通路径的同构边界：init 可以有 boot-held adopt 来源，但不能绕过 PoolBinding、AddressSpace readiness、Attach/Start 生命周期与 Handle 安装契约；bootstrap 专用 assert 不得依赖 debug 构建才执行。
16. 检查过渡边界：中间页表、普通 anonymous backing、Tunnel 与库存自检仍走主计划登记的 transitional raw adapter。本审查不得以切片 6 尚未完成否定当前 root/payload 闭包，也不得把 boot-held adopt 泛化成可绕过清零的公共接口。
17. 复核验证是否实际覆盖 Unbound 零收费、错误 kind/rights/alias、成功消费、重复 Bind、drain 退款、metadata shell permit 耗尽/退款和 Attach 截止后接管；并评估 Bind 与 Bind/Start/Kill/Builder close 的并发及故障注入是否仍有值得补入主计划既有验证面的盲区。

## 基线证据

- `cd os && cargo test -p tar -p elf -p page_table -p frame_pool -p dtb -p handle_table -p wait_context -p timer_queue -p stack_layout -p sched_domain -p memory_pool -p funded_frame -p metadata_admission --target aarch64-apple-darwin`
- `cd shared && cargo test --target aarch64-apple-darwin`
- `just check`
- `just build_user`
- `just build_kernel`
- `just acceptance`

提交收口时上述验证均通过：HandleTable 17 项、MemoryPool 14 项、funded_frame 6 项、metadata admission 6 项及其余相关 host 测试全绿，shared 13 项 ABI 测试全绿。启动期自检实际穿过 Builder/Control 本地 permit 耗尽与退款、已登记 Attach 在终止截止后的接管，以及 Pool/funded frame 正式 adapters。acceptance 完成 virt debug stress 16/16、virt release core 与 sifive_u debug core；core 线出现 `process shell and memory binding acceptance passed`，三线均完成业务收束与 reset 锚点。

## 完成标准

所有发现按严重度给出文件/符号证据、可达失败点、并发顺序或锁上下文，并明确影响的是 Handle/Pool/FramePool/AddressSpace/Job 守恒、Building 截止、metadata admission、bootstrap owner 还是 ABI authority。任何 Pool Handle 双消费或恢复、Bound 半发布、permit 提前退款、post-cutoff 资源遗留、无 owner payload 投影、AddressSpace 锁内逆序析构、root core 分叉或 debug-only 正确性均为阻断项；修复后重跑相关 host 测试、`just check` 与完整 acceptance。非阻断承接只回到主计划既有切片 6/10，不复制新的 TODO 真值点。
