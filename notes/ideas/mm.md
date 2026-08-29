# 内存模型

内存管理同时维护物理资源所有权、地址空间隔离和映射生命周期。地址空间区域账本是映射关系的唯一真值，页表只是它的硬件投影；物理库存、backing、虚拟区域、对象 view 与借出 lease 必须各有唯一所有者。所有调用者通过同一个地址空间深模块提交意图，不各自组合帧、PTE、回滚和地址翻译同步。

## 物理内存

物理帧是库存资源，以 affine extent 所有权表达。空闲库存不能依赖由自身供血的通用堆；启动内存先注册为不可用，再把统一 reservation 集合的补集发布为空闲。库存 claim、split、coalesce、指定区间和归还都必须具有由地址宽度与固定 arena 容量推导的结构上界，不能扫描随碎片数量增长的全局链。

库存只决定哪些帧已经从空闲集合原子取走，不在库存锁内完成与页数线性的内容初始化。claim 成功后，调用者独占 extent，先在库存锁外清零，再把“已初始化 backing”发布给页表或对象；初始化失败或事务放弃仍由该 affine 所有权自动归还。单次可清零页数受调用硬上限约束，因而库存锁持有时间与数据清零成本分别有界。

精确长度不要求物理连续。普通用户 backing 由若干 extent 组成；需要连续物理区间的页表、DMA 或其他消费者必须显式请求受 order 上限约束的连续块。extent token 不允许复制或任意构造；切割消费原 token 并返回互不重叠的剩余 token，合并只接受物理相邻且所有权语义相同的 token。失败回滚、延迟 retire 与正常归还遵守同一数量守恒。

普通对象只在持有固定容量 backing 时适用有界 close；进程地址空间等宽度可增长的容器以有界 drain 释放。任何解除容量上限的 backing 对象都必须先取得可恢复的 drain 生命周期，不能把无界页遍历放进最后一个引用的析构。

## 单一地址翻译模式

统一内核运行环境只选择一个所有 admitted hart 都支持的地址翻译模式。共享物理内存配多套模式会复制内核映射、割裂线程迁移和回收契约，却不增加隔离能力；若不存在共同支持模式，启动应明确失败。

内核映射在所有用户地址空间中保持一致，使用户 trap 可以直接进入共同入口。用户半区属于进程，内核半区属于系统；共享内核页表子树与地址空间拥有的用户子树必须由显式所有权记录区分，建立、借用和收束经过同一地址空间模块，不能依赖创建与 Drop 调用点的手工配对知识。

## AddressSpace 深模块

每个进程拥有一个 AddressSpace。它的外部 seam 只接受映射变更意图、调用 authority 与有界 drain，不向 Building 组装、Running syscall、bootstrap 或对象 lease 暴露区域容器、extent 排列、页表步骤、epoch 或 Remote Call 请求。

模块内部共同拥有：

```text
AddressSpace
├── RegionLedger       虚拟区域、权限、owner、AllocationKey 与 RegionKey
├── Backing            OwnedExtents 或 ObjectView
├── TranslationTree    PTE 与页表子树所有权
├── MemoryChange       validate → reserve → commit → publish → synchronize → retire
├── EpochState         translation/instruction epoch 与待确认义务
└── DrainState         区域、事务与页表的可恢复收束进度
```

Building 与 Running 只在 authority、允许的来源和生命周期阶段上不同，共用同一变更机制。bootstrap initial process 是内核持有 Building authority 的固定调用者；Tunnel 等对象以内部 lease authority 建立和撤销 view。新的映射来源或调用通道必须进入这个 seam，不能旁路建立 PTE 或另建地址占用表。

地址空间具有独立于可变账本锁的稳定身份与 translation epoch。active-hart 成员关系的唯一真值仍属于进程执行模型；AddressSpace 通过固定内部 seam 取得发布时快照，并只拥有由该快照产生的 epoch 义务与完成确认，不维护第二份成员集合。Process 的短 execution gate 同时线性化 enter/leave、Running 事务 Commit 准入与 Running→Terminating 截止：终止先取得 gate 则未 Commit 事务回滚，Commit 先取得 gate 则终止路径接管该必成事务。短锁只保护账本、事务状态和有硬页数上限的 PTE 发布；帧清零、用户态工作和远端确认等待均不得持有地址空间 spinlock。一个地址空间只允许固定数量的变更事务在途；初始实现可以串行化修改，未来扩大固定槽数不改变外部 seam。

Commit 的嵌套顺序固定为 AddressSpace commit lock 在外、Process execution gate 在内。终止路径只在 execution gate 内发布准入截止并生成接管义务，释放 gate 后才进入 AddressSpace，不能反向嵌套。MemoryObject state lock 与 AddressSpace lock 之间不嵌套：WritePermit 在对象锁内预留后以 affine token 转入 PreparedChange，rollback/retire 先从 AddressSpace 摘出 token，解锁后再回到对象推进计数。

## 区域账本

区域按虚拟地址排序。空闲洞不入账；guard 等 reservation 入账但不安装 PTE；mapping 同时记录虚拟区间、backing view、对象内偏移、当前权限、权限上限和解除所有者。任一稳态有效用户 PTE 必须恰由一段 mapping 覆盖，reservation 范围内不得存在有效 PTE。

地址空间拥有普通 mapping 与 reservation。对象可以借出由内部 lease 拥有的 mapping；这类区域仍进入同一账本参与冲突检测，但普通内存操作不能替换、切割或解除，只有对应 lease 可以撤销。地址空间最终收束前，全部 lease 必须先关闭或被进程收束接管。

部分解除在请求边界切分区域。匿名 mapping 的 OwnedExtents 随区域切分；解除的中段转入事务 retire 所有权，只有相关 hart 确认旧翻译失效后才归还库存。对象 mapping 只切分 ObjectView 并调整对象内偏移，不改变对象对 backing 的所有权。账本决定以上关系，PTE 的存在或解除本身不决定帧归谁。

一次成功 Map 建立一个内部 `ReservationGroup`，以不可复用的 `AllocationKey` 标记共同来源；完整 reservation 的 guard 与 mapping 都引用该组。每个当前 ledger fragment 另有唯一 `RegionKey`。切割消费旧 fragment 及其 key，为存活左右片段和 retiring 中段分别铸造新 key；`AllocationKey` 保持不变，直到最后一个 ledger 或 retire fragment 消散。ReservationGroup 不保存第二份占用区间，也不授予按组操作的隐式 authority；区域账本中的当前 fragment 集合始终是范围真值。

普通 `MemoryUnmap` 只作用于请求区间，不因命中某个 AllocationKey 而解除区间外同组 fragment。解除完整 reservation 会同时移除 guard 与 mapping；只解除 usable mapping 会留下两侧 reservation-only fragment；只解除 guard 或 mapping 中段也只切割对应范围。请求跨空洞、越界或不同解除 owner 时整笔失败。对象 lease-owned fragment 仍只能由 lease authority 撤销。

ledger 只在 owner、区域种类、AllocationKey、backing identity/连续 offset、当前与最大权限全部相同且几何相邻时兼容合并；合并同样消费旧 RegionKey 并铸造新 key。不同 AllocationKey 的相邻区域保持独立，不能为了节省节点丢失分组来源。

## Backing

匿名 mapping 是最常见路径，其私有 OwnedExtents 由 mapping 直接拥有，不为每次分配产生用户可见 Handle。区域切割只变换内核 affine 所有权；释放最后一段区域才释放对应 backing。需要共享或独立于某个地址空间存活的字节才使用 MemoryObject。

MemoryObject 是固定长度的共享 backing identity，不是地址空间、虚拟区域或用户态分配器。Handle 的 `MAP` right 授权建立新 view，`READ`、`WRITE` 约束该 view 可请求的权限；mapping 建立后独立持有对象引用，关闭原 Handle 不隐式解除已有 view。同一对象可以按不同地址和更窄权限映入多个进程。

可由普通 Handle close 触发最终析构的 MemoryObject 必须受硬容量上限约束，长度创建后不可改变。更大的逻辑对象由用户态协议组合多个 MemoryObject；未来若需要无界对象、resize、COW、文件缓存或 pager，必须先为对象建立有界 drain 与明确的缺页协议，不能改变既有 eager mapping 的成功语义。

## Map、Unmap 与 Protect

Running 进程对自己的地址空间具有固有管理权；组装者只凭 ProcessBuilder 在 Building 期操作目标地址空间。两条 authority 通道经过校验后消费同一 MemoryChange，不各自维护布局、backing 或回滚规则。

Map 以字节长度请求匿名或 MemoryObject backing，并同时声明权限、placement 与两端 guard。`Anywhere` 由账本选择完整空洞；`FixedEmpty` 只在指定范围完全空闲时成功。不存在隐式覆盖旧区域的 fixed 模式，替换必须显式 Unmap 后再 Map。成功结果包含可访问区间及含 guard 的完整 reservation 区间；AllocationKey 与 RegionKey 都是内核账本身份，不进入以地址区间为真值的用户 ABI。

单次请求的页数、guard 数量、区域切分数、PTE 步数和元数据增长都有共享硬上限。超出上限由用户态运行时在安全 interface 后分段组合；rinlib 内部以 affine region 表达 allocator arena、线程栈等映射所有权。完整解除消费原 region；部分解除消费原 region，并按请求洞的左右两侧返回至多两个 compound fragment，每个 fragment 可以同时描述 usable mapping 与相邻 reservation-only guard。该所有权类型不是应用扩堆 interface；内核 ABI 仍以地址区间为真值。

Unmap 采用严格全覆盖语义：目标范围必须被调用者可解除的 mapping 或 reservation 完整覆盖；空洞、越界或 object-owned lease 使整笔请求在 Commit 前失败。调用者负责先停止其它线程对该区间的访问；内核保证成功返回后不存在 stale translation，不能替应用修复并发 use-after-unmap。

Protect 只能在 mapping 创建时冻结的权限上限内改变当前权限。权限只允许只读、读写与读执行，不提供 write-only 或 W+X；增加权限、收窄权限和解除映射都走同一 MemoryChange 与地址翻译同步纪律。

连续堆顶不是地址空间真值。rinlib allocator 在内部取得并持有若干匿名 arena，向应用只暴露分配器语义，不暴露 Extend、brk 或堆映射操作；线程库同样消费通用 Map/Unmap 建立带 guard 的栈。不存在内核线程栈分配调用或固定预映射栈池。

## Guard 与故障

Guard 是占据虚拟区间但不持有 backing、也不安装 PTE 的 reservation。它阻止自动选址或 fixed mapping 占用该洞；普通 Unmap 对 guard 与 mapping 都遵守同一精确请求区间，不执行组级隐式清理。Guard fault 与普通未映射访问都属于用户程序违约并终止该进程，不升级为内核失败。

不存在按需分页时，成功 mapping 的全部页在返回前已有 backing 和有效 PTE。未来 pager 必须通过显式允许 fault 的新语义接入：账本区分可解析的缺页 mapping 与永远不可解析的 guard/空洞，既有 eager mapping 不因 pager 出现而改变。

用户内存访问经统一 uaccess seam 验证；内核不把用户指针当普通引用长期保存。一般异步调用不得在 publish 后依赖可能失败的用户写回；Map 使用下述提交前结果承诺，把固定宽写回放在不可逆边界。其它调用若确需异步写回，必须各自定义地址空间并发变化时的故障契约，不允许以内核 panic 代替。

## MemoryChange 事务

Map、Unmap、Protect 以及 object-owned lease 的建立与撤销都由 AddressSpace 内部事务完成：

```text
Validate → Reserve → Commit → Publish → Synchronize → Retire → Complete
```

Validate 检查完整范围、authority、生命周期、权限与固定宽输出槽。Reserve 取得 backing、区域节点、页表中间帧、事务槽和全部目标 hart 的 Remote Call 容量；与页数线性的 backing 清零在独占 extent 后、Commit 前完成。页表模块必须支持先准备资源再发布修改，Publish 段不能临时申请表帧或发生普通可恢复失败。

Commit 是不可逆线性化点。此前任何失败都保持账本、PTE、backing 和请求槽零副作用；一旦 Commit 成功，事务归 AddressSpace 且必然完成。区域与 PTE 可以在有硬上限的 Publish 段逐项更新，未同步的其它线程在调用进行期间可能观察到旧状态或正在发布的新状态；内核不承诺任意多页在所有 hart 上瞬时切换。Synchronize 完成并向仍存活的调用者返回后，所有目标 hart 必须已经达到新 epoch，这才是对外完成边界。

Map 的 Reserve 已经唯一确定 usable 与 reservation 范围。`MemoryMapResult` 使用调用者提供的稳定结果槽，包含四个固定宽范围字段和位于末尾、自然对齐的 committed cookie；request 携带非零 cookie，调用者把 committed 初始化为零，Validate 拒绝非零初值或不满足原子对齐的槽。Reserve 从 ledger 取得覆盖该固定宽可写范围的 `UserWriteLease`，阻止其它事务在 Commit 或 rollback 前撤销、降权或替换结果 backing；lease 保存有界、已验证的写入投影，使 Commit 不必重新进入 uaccess 或 AddressSpace lock。全部可失败检查与资源预留完成后，内核先写 payload，再在 execution gate 内通过 lease 以 release store 把同一 cookie 写入 committed 字段；该 store 就是 Map 的 Commit。用户或 rinlib 只有以 acquire 观察到 cookie 匹配后才可相信 payload 完整，cookie 为零时 payload 内容无语义；在 System Call 成功返回前仍不得访问新 mapping。Commit 后释放 UserWriteLease，不再执行可能失败的 uaccess。

结果槽必须位于调用前已经存在且可写的 eager mapping；UserWriteLease 只 pin 固定宽 ledger/backing 与写权限，不把用户指针变成可长期解引用的内核引用。与 lease 冲突的 Unmap/Protect 在各自 Commit 前返回 Busy。rinlib 为可能 park 的 Map 把结果槽放进随线程保留到 join 的运行时记录，而不是可提前回收的临时缓冲。若调用线程在 Commit 前消散，事务释放 lease 并回滚；若在 Commit 后消散，结果承诺已经属于进程，事务继续完成。线程的 departed/join 完成边界不得越过仍挂接于该线程结果记录的 committed transaction，因而接管者在 join 后观察 cookie 时也已经越过 mapping 完成边界。

Unmap/Protect 的旧 backing、旧权限或 writable-view permit 在 Synchronize 期间由事务隔离持有，不能归还库存、映射到其它地址、退出 seal 计数或被另一对象重新解释。Retire 只在全部确认后释放旧 OwnedExtents、旧 ObjectView、写 permit 与过期元数据。一般事务状态属于地址空间而非发起线程；等待远端确认的 System Call 通过 WaitContext park，发起线程终止只消散最终返回权。进程终止冻结后拒绝新事务并接管已 Commit 的事务；只有线程、active hart、事务与对象 lease 全部完成或进入可恢复收束后，地址空间才可进入最终 drain。

## 跨 Hart 地址翻译同步

本协议依据 [RISC-V Privileged Architecture「Supervisor Memory-Management Fence Instruction」](../../references/normative/riscv-isa-v20250508/src/supervisor.adoc)、[RVWMO Memory Consistency Model 的适用范围说明](../../references/normative/riscv-isa-v20250508/src/rvwmo.adoc)、[RVWMO explanatory material「Fences」与「Explicit Synchronization」](../../references/normative/riscv-isa-v20250508/src/mm-eplan.adoc) 以及 [Zifencei「FENCE.I Instruction」](../../references/normative/riscv-isa-v20250508/src/zifencei.adoc)。RVWMO 尚未形式化页表遍历、`SFENCE.VMA` 与 `FENCE.I` 的组合，因此不能仅凭语言原子序推导完成条件；协议同时遵守 privileged architecture 给出的 data fence → IPI → remote `SFENCE.VMA` → ack 顺序。

`SFENCE.VMA` 只同步执行它的 hart。Reserve 在 Process execution gate 下取得 active 集合、成员序列与事务准入状态快照，锁外预留每个目标的 Remote Call 槽；Commit 前重新取得 gate，若成员序列或生命周期准入已变化则在零业务副作用状态下回滚或重试。复检成功后，AddressSpace 在 gate 内完成 Map 结果 cookie、完整 PTE 宽度写入、新 translation epoch 和请求状态 release 发布，再释放 gate。随后执行保守的本地 `FENCE RW,RW` 并触发 IPI，保证 PTE、epoch 与请求槽先全局可见；IPI 只是门铃。目标 hart 以 acquire 取得请求，验证稳定 AddressSpace identity 与 epoch，执行覆盖请求的本地 `SFENCE.VMA`，若请求携带 instruction epoch 再执行 `FENCE.I`，最后以 release 确认。完成者必须以 acquire 观察每一项确认后才能 Retire 旧 backing、权限或 view permit，并在释放槽前以 release 发布空闲；后续槽预留以 acquire/CAS 取得。初始实现可以合法地全局 over-fence，范围与 ASID 优化不得削弱完成条件。

请求槽的 Pending 状态而非 IPI 边沿是真值；门铃允许合并和重复，每个 trap 安全出口都检查本 hart 的固定槽，因此已经可见的请求不会因单次门铃边沿合并而丢失。平台 admission 必须先保证 admitted hart 的 IPI 与周期性 trap 路径可用；Commit 后的门铃异常不能转成 System Call 业务错误，Remote Call 只能保留 Pending 并由后续安全点补消费。平台永久违反 admission 契约属于系统级失败，不能假装事务已完成。

发起 hart 若属于 active 快照，也必须作为普通目标执行同一套 acquire → local fence → release ack，不能因正在修改页表而隐式视为已确认；不在快照中的发起 hart 只负责发布，不产生虚构的本地义务。

| happens-before 边 | 发布侧 | 观察侧 | 所保证事实 |
|---|---|---|---|
| PTE/epoch → 请求 | 完整宽度 PTE stores；请求状态 release；IPI 前 data fence | 目标请求状态 acquire | 目标执行失效前已能观察对应 PTE 与 epoch，门铃不会越过业务发布 |
| 既有用户写入 → 失效确认 | 目标 hart 在 trap 安全点处理请求，`SFENCE.VMA`，必要时 `FENCE.I`，随后 ack release | 事务完成者读取 ack acquire | 旧 writable view 的显式写入及本地翻译失效先于 permit/backing retire |
| 全部确认 → backing 复用 | 完成者对每项确认作 acquire，再推进 Retire | 后续库存/object owner 经事务发布取得资源 | 任何可能持旧翻译的目标 hart 均已失效，旧帧才可被重新解释 |
| 请求完成 → 槽复用 | 完成者清理 payload 后以 release 发布 Empty | 新预留者以 acquire/CAS 取得槽 | 新请求不能观察旧 payload、identity 或完成引用 |
| active 快照 → enter/leave | Commit 在 execution gate 内复检成员序列并发布 epoch | enter/leave 在同一 gate 内线性化并 acquire epoch | 快照前 active hart 必须确认；快照后 enter 必须先本地同步；leave 不能逃逸既有义务 |

active-hart 集合的唯一真值仍属于 Process 执行模型，enter、leave 与 AddressSpace 快照共享上述短 execution gate。enter 在登记 active 前以 acquire 读取 epoch并完成所需本地 fence，随后在 gate 内复检 epoch 未变化才登记并获准返回用户态；若变化则退出 gate、同步并重试。Commit 复检成功后在 gate 内发布新 epoch，因此 gate 释放后的 enter 必然观察它。leave 在 gate 内只有确认自己承担的 epoch 后才清除 active。由此，在线性化上早于 Commit 快照的 enter 必在目标集合中；晚于快照的 enter 必在返回用户态前自行达到新 epoch；早于快照的 leave 已不再执行该地址空间，晚于快照的 leave 必先履行确认。

最后一个 acquire 确认推进事务，原发起线程是否仍存在不影响完成。Process 终止、hart 离场和普通 SSIP 都只能协助消费同一请求，不能以“目标将不再运行”伪造确认。地址空间 root、ASID 或稳定 identity 在全部请求与 active 关系收束前不可复用；未来若复用 ASID，必须遵守规范「ASID Usage」要求另行完成覆盖该 ASID 的 fence。

## 可执行发布

可执行内存遵守 backing 级 W^X，而不只检查单个 PTE。Running 匿名 mapping 只产生数据权限；Building 构造方可以在目标尚不可运行时经受控写入口填充最终只读或可执行 backing，ProcessStart 在首次执行前统一发布代码代次。

运行期动态代码使用 MemoryObject 的单向状态机：

```text
Mutable --SealExecutable--> Sealing --last WritePermit retired--> Executable
```

| 状态 | 允许的新操作 | 拒绝的操作 | 前进条件 |
|---|---|---|---|
| Mutable | 只读 view；取得 WritePermit 后的 writable view 或受控直接写 | executable view | `SealExecutable` 在线性化点改为 Sealing；若 permit 为零可同点进入 Executable |
| Sealing | 只读 view；既有 writable view 的撤销/降权 | 新 WritePermit、直接写、executable view、回到 Mutable | 最后一个 WritePermit 完成 retire |
| Executable | 只读或读执行 view | 任意写入口、WritePermit、回到 Mutable/Sealing | 终态 |

Mutable 允许只读 view，并允许在对象锁内取得 `WritePermit` 后建立或重新启用 writable view；不允许 executable view。`SealExecutable` 要求对象定义的管理 authority，并与 WritePermit 预留在同一对象锁上线性化：先取得 permit 的变更计入 seal 等待，先进入 Sealing 的对象拒绝新 writable view、重新加写权限和直接写入口。Sealing 允许既有 writable view 继续存在直至其 owner 显式撤销，但状态不可回退；Executable 永久拒绝全部写入口与 writable permit，只允许只读或读执行 view。普通 Protect 不能把读写 mapping 转成读执行，也不能重新打开已发布对象。

WritePermit 的计数覆盖 reserved、published 和 retiring 三个阶段。Map/Protect 在 Commit 前放弃 permit 可以直接回滚；Commit 后移除 W 权限或 Unmap 时，permit 随旧 view 进入 retire，只有 active-hart 快照全部完成 `SFENCE.VMA` 并以 acquire 收齐确认后才退出计数。最后一个 permit 的 retire 负责把 Sealing 单向推进到 Executable，并完成仍存活的 seal waiter；seal 发起线程消散不撤销已发布状态转换。RX view 只能在观察 Executable 后建立，并为可能执行该代码代次的 hart 推进 instruction epoch 与 `FENCE.I`。

状态机不遍历 view。对象只维护带溢出检查的 permit 计数和固定容量 seal 完成槽；首个调用发布 Sealing 后可以通过 WaitContext 等待，重复调用在 Executable 上幂等成功。Sealing 期间，空闲完成槽可挂接一个新 waiter，槽已占用则返回 Busy。精确公共 ABI 延后决定，但不得改变“seal 状态属于对象、发起者消散后继续、最后一个 permit retire 自动完成”的契约。受控直接写同样取得临时 WritePermit，数据写入结束并完成必要的 release 后才归还。

只读 view 不阻塞 seal，已有 writable 最大权限但当前不含 W 的 view也不持 permit；它日后尝试重新加 W 时必须重新取得 permit，因此在 Sealing/Executable 下失败。新代码版本通过新对象、新代次和用户态引用切换表达，不原地修改已发布代码。Handle 消散不撤销 view 或 seal；view 与 pending seal 都强持对象，最终 close 仍遵守固定 backing 容量上限。

具体帧库存、页表、内核高半区、用户布局和当前调用面见 [`../impls/mm.md`](../impls/mm.md)。
