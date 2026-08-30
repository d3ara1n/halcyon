# 内存管理

方向见 [`../ideas/mm.md`](../ideas/mm.md)。当前实现分五层：帧库存、用户地址空间纯逻辑规划器、可 host 测试的 Sv39 页表、内核地址空间与启动协议、现有用户地址空间接入；本篇是内存实现事实的唯一拥有者。

## 帧库存

物理帧的唯一来源是 `os/frame_pool` 的**外置元数据分级 order 树**。纯逻辑 crate 不使用堆、不含 unsafe；内核只在 `os/kernel/src/frame.rs` 提供启动 reservation、真实帧清零、全局 POOL 锁与 `FrameTracker` RAII 适配。

### 结构

每个 DT memory region 先按全局帧号作 canonical power-of-two 分解，形成物理对齐 arena。单个任意区间至多产生两倍地址位宽个 arena；板级最多 8 个 memory region，`MAX_ARENAS = 1024` 是结构上限。arena 不跨 DT 物理缺口，因此任何 order 块天然物理连续并按自身大小对齐。

每个 arena 使用完全二叉树，节点一个 `u8`：`0..` 表示该子树当前可提供的最大 order，`u8::MAX` 表示没有空闲块。整块空闲由父节点直接代表；向下分配时才物化两个子节点，归还时沿祖先即时合并。分配和归还不维护运行期碎片链，也不读取被管理帧内容。

树节点每个托管帧精确占 2 字节；固定 1024 项 `ArenaMetadata` 与树节点一起从启动期物理 reservation 取得。`FramePool` 自身只持两个外置切片和计数器，不把大数组压入内核栈。实测 virt 1 GiB 使用 134 个元数据帧，sifive_u 128 MiB 使用 22 个。

```rust
pub struct FramePool<'a> {
    /* tree metadata slice, arena metadata slice, counters */
}

pub struct FrameTracker {
    geometry: Option<ExtentGeometry>,
}
```

`FramePool` 不再参数化帧内容后端，claim/return 路径只能读写外置树元数据。内核 adapter 取得 claimed geometry 后先释放 POOL 锁，再经 `phys_to_virt` 清零完整 extent；清零结束才构造 `FrameTracker`。`ExtentGeometry` 可复制但只表达几何，私有字段的 `FrameTracker` 才表达 affine 所有权。

### 操作与契约

- `add_managed_region(start, end)`：注册完整 DT memory，初态全部 unavailable；重叠、arena 超限或元数据不足在修改前失败。
- `release_range(start, end)`：把启动 reservation 的补集或已结束 reservation 发布为空闲；bootstrap 与 BootPackage prefix 回投走此入口。
- `alloc_order(order)`：库存分配 `2^order` 个连续且同阶对齐帧，沿固定 arena 集合选树、沿树下降；内核 adapter 在锁外清零后发布 tracker。
- `alloc_largest(max_count)`：库存单次扫描固定 arena 集合，取得不超过上限的最大 power-of-two extent；普通匿名 backing 用多个 extent 表达任意页数，不要求整段 PA 连续；内核同样在锁外清零后发布。
- `alloc_at(base, count)`：预验证完整指定区间空闲后，按 canonical blocks 精确取走；失败不改变库存。
- `dealloc(base, count)`：任意区间 canonical 分解后沿祖先合并；重复归还或与现有空闲库存重叠立即触发断言。普通 `FrameTracker` 持 power-of-two extent，BootPackage payload 可持任意长度保留区间。

`alloc_order`/`alloc_largest` 的库存步骤上界只取决于 `MAX_ARENAS` 与地址位宽；单 extent 归还只沿一棵树上行，任意区间的 canonical block 数同样由地址位宽和 DT region 数限制。清零与返回帧数线性，但在 POOL 锁外执行，也不存在随全局碎片数增长的扫描。

启动流程先排序并验证 DT memory，再按总托管帧数计算元数据 reservation；SBI+内核、实际 BootPackage 和元数据页保持 unavailable，只发布其补集。内核堆明确申请 order 8（1 MiB）并终身持有；页表与 Tunnel 申请 order 0。Building 匿名区间由 `alloc_largest` 组合多个 extent。

`FrameTracker` 不可复制或由安全代码任意构造；只暴露只读几何，`split_at` 消费原 tracker 并产生两个精确相邻 tracker。页表帧通过 `into_table_frame`/`adopt_table_frame` 显式移交，BootPackage payload 通过独立 reservation adopt 收编，内核堆通过 permanent transfer 终身持有。`FrameTracker::Drop` 直接走结构性有界归还。ProcessDrain 不再保存帧池扫描游标：tracker 从拥有结构摘下与下一 work unit 的实际归还分开计费，手工摘除的表帧经 table adopt 进入同一路径。

### 测试集（host）

15 项 host 用例覆盖：整阶取还、逐层 split/coalesce、全局 order 对齐、碎片不伪造大阶、extent 几何边界与精确切割、最大 extent fallback、`alloc_at` 精确/失败原子/跨 arena、reservation 延后发布、多 DT region 不跨洞、元数据/arena/重叠准入失败、canonical arena 数上界、双重释放、零长度拒绝，以及 2000 轮随机分配归还的帧数守恒与最终整阶合并。纯逻辑库存类型没有帧内容后端，因此 host claim 路径结构上无法访问帧内容。内核启动自检覆盖 claim、锁外清零、affine split、分片归还计数与再次清零；virt debug/release/hetero/nofd 和 sifive_u 均通过。

## 页表模式选择

单模式（全系统同一 satp 模式）是共享内核映射的结构性上限而非妥协：内存物理上同一份，异模式 hart 各持平行映射树，反而割裂调度域（进程不可跨模式迁移）——多模式无收益。模式是启动期从硬件自动识别，不依赖手工配置：dtb 各 cpu 节点的 `mmu-type` 给出每个 hart 的支持上限，取全体 Application hart 的**最小上限**（硬件允许集），与内核支持集取交集选最高模式；不支持交集（如仅 sv32）则拒绝启动。运行时选出的模式作为常量贯穿后续初始化（satp 组装、地址宽度断言）。

内核支持集由编译期决定，当前只有 Sv39。

## 页表纯逻辑（os/page_table crate）

`os/page_table` 是 `no_std + alloc`、禁止 unsafe 的独立 crate，host 与内核 target 复用同一份代码。页表树 const 泛型于 `LEVELS`（3=Sv39、4=Sv48、5=Sv57），PTE 编码、VA 宽度与各层覆盖页数都由级数推导。页表逻辑不直接解引用物理地址；表帧 reservation、已发布帧回收与表内容访问全部经 trait 适配：

```rust
pub trait ReservedTableFrame {
    fn number(&self) -> FrameNumber;
    fn commit(self) -> FrameNumber;
}

pub trait FrameMemory {
    type ReservedFrame: ReservedTableFrame;
    fn reserve_frame(&mut self) -> Result<Self::ReservedFrame, FrameExhausted>;
    fn free_frame(&mut self, frame: FrameNumber);
    fn table_mut(&mut self, frame: FrameNumber) -> &mut [Pte; 512];
}
```

内核以 `TableFrameToken(FrameTracker)` 实现 affine reservation：token 被丢弃即归还帧池，Commit 才通过 `into_table_frame` 把帧交给树；已发布分支帧回收时经 `adopt_table_frame` 重新收编。host backend 用同一 token 生命周期核对 reserved/committed/free 计数。root 和全部 reservation 都在发布前清零。

### 类型与事务边界

- `FrameNumber`、`Vpn`、`Ppn` 是页号 newtype；`Pte` 集中编码 V/R/W/X/U/G/A/D 与 leaf/branch 判别。Map/Protect 在 preflight 拒绝超出 PTE 编码、V=0、无 RWX 以及 W 且非 R 的非法叶标志。
- `TableTree<M, LEVELS>` 拥有 root 和全部 owned 中间表帧，不拥有叶数据 backing。root 另有固定宽 `owned_root`/`shared_root` 位图；创建或摘除用户项与位图同步，`attach_shared_root` 原子安装外部 PTE 并登记 shared 所有权。普通 Drop 只递归 owned 槽；ProcessDrain 经槽状态 API 逐项摘除 owned 分支，`finish_drain` 只在 owned 位图归零后交出 root。
- `PreparedTranslation<M::ReservedFrame>` 是 Commit 前 affine token。`prepare_map/unmap/protect` 完成参数与冲突验证、精确表帧需求计算、申请和清零；`publish` 只消费 reservation 并写完整 PTE，不分配且不返回可恢复错误。预留不足在任何 PTE 修改前失败并自动归还已取得 token；其它非重叠变更先建立共享路径时，多余 token 在 Publish 结束自动归还。

### Preflight 与发布

Map 仍把连续 VPN/PPN 区间切成最大可行 mega 段。preflight 只读遍历现树：缺失路径以“表层级 + 覆盖区编号”去重；兼容 mega 的细化同样登记将要 split 的路径；已有异 PPN、异 flags 或更细子树冲突在 reservation 前返回。Publish 按同一切段重放，只能写叶、链接 reservation 帧或展开已验证的兼容 mega。

Unmap 与 Protect 递归携带当前表的真实覆盖基址。preflight 只为目标区间部分覆盖的现有 mega 计数：完整覆盖直接改叶，部分覆盖逐级精确预留；普通 4 KiB Unmap 因而预留零帧。Unmap 对未映射洞保持宽松，Protect 则要求区间完整映射且当前 flags 全部匹配。split 后 512 个子项保持原物理连续性和 flags；空中间表仍保留到整树 Drop 或 ProcessDrain。

当前 `AddressSpace` 的 Building/bootstrap 与 Running Extend 均经 `MemorySpace` planner 产生 translation intent，再由 `OwnedBacking` 和 reservation-aware `TableTree` 物化；旧 `TableTree::map/unmap` 分配路径、`FrameMemory::alloc_frame`、`mem_mut`、`clear_slots`、`leak_root` 和内核 root 区间配对均已删除。Tunnel 仍是唯一尚未迁入该组合的外部映射调用点。

### 测试集（host）

25 项 tree 用例与独立 Drop ledger 用例覆盖：未对齐 8192 页跨表映射的精确 18 帧需求、mega 选择与细化、跨子表 Unmap、Protect、幂等与冲突、非法叶 flags、资源不足零修改、部分 reservation 自动归还、未发布 reservation Drop、非重叠并行准备后的多余帧归还、owned/shared root 槽转换、共享子树不回收、整树 Drop 数量守恒。debug/release host 测试、clippy `-D warnings`、`just check`、virt debug 与 virt-release 全部通过。

## 用户地址空间纯逻辑规划器（os/memory_space crate）

`os/memory_space` 是 `no_std + alloc`、禁止 unsafe 且不依赖其它 crate 的内部规划模块。它只拥有页对齐半开区间、区域账本、backing view、权限、owner、事务阶段和 MemoryObject 写许可状态，不访问页表、物理帧、用户指针、hart 或内核对象。内核 `AddressSpaceState` 已把该 planner 与匿名 `OwnedBacking`、reservation-aware `TableTree` 组合为同一事务；Building/bootstrap 与 Running Extend 已共用该 seam，Tunnel 外部 lease 尚待迁移。

`MemorySpace` 在构造时一次性预留区域与在途事务的硬容量。有序 ledger 中每个 fragment 持唯一 `RegionKey`，同一次 Map 的 guard 与 mapping 共享 `AllocationKey`；fragment 另持 `AddressSpace`/lease owner、匿名 backing identity 或 `ObjectId + offset` view，以及当前/最大权限。`Anywhere` 在 ledger 与在途事务之间选 first-fit 完整空洞；`FixedEmpty` 不覆盖旧区域。Unmap 严格要求请求区间连续覆盖且 owner 一致，完整 reservation、usable-only、guard-only 与 mapping 中段都按精确交集切割；Protect 同样消费旧 key，只有 owner、种类、AllocationKey、连续 backing 与权限全部兼容的相邻 fragment 才合并。fault lookup 只返回 free、guard 或 eager mapping。

变更由不可复制的类型状态表达：`ValidatedChange → PreparedChange → CommittedChange → PublishedChange → SynchronizedChange → RetiredChange`。Validate 可以分配规划元数据但不改 ledger；Reserve 复检 region snapshot、真实范围冲突、UserWriteLease pin 与 WritePermit multiset，并预留 Commit 所需的 fragment、retire permit 和事务容量。rollback 只存在于 Commit 前并归还全部 permit。Commit 是不可失败的 ledger 线性化点，不再分配；其后的 Publish、Synchronize、Retire 与 Complete也不返回可恢复错误，错配 token 视为内核所有权不变量破坏。内核 adapter 按 translation intent 在 Commit 前准备真实表帧和 leaf 投影，Commit 后发布 PTE 与 ledger；Running Extend 的 `PublishedChange` 由 Remote ack completion 推进到 Complete。

`UserWriteLease` 把非页对齐结果区间投影为固定上限的 writable backing segments，并以 RegionKey pin 到 Commit 或 rollback；与结果范围或变更 footprint 相交的其它在途事务返回 Busy。MemoryObject 状态独立实现 `Mutable → Sealing → Executable`：writable replacement 必须携带不可复制 `WritePermit`，permit 从 Reserve 覆盖到 retiring fragment 完成 Synchronize 后交给 Retire；最后一个 permit 退出计数时完成 seal，即使原 waiter 已消散也不回退状态。

host debug/release 共 18 项测试，覆盖区间溢出与对齐、容量和 backing 边界、Anywhere/FixedEmpty、双 guard、四类精确 Unmap、AllocationKey 保持与 RegionKey 消费、同 allocation 合并与跨 allocation 拒绝、owner/权限上限、object offset、UserWriteLease projection/Busy/rollback、非重叠并行事务、stale 与 permit mismatch 失败原子、类型化阶段、seal/permit retire、逐区域 drain，以及 2000 步确定性 shadow model 的逐页覆盖与 fragment 不重叠。`cargo clippy -p memory_space --all-targets -- -D warnings` 与 `just check` 通过。

## 内核地址空间与启动协议

### 链接与线性偏移

内核镜像 VMA = PA + `KERNEL_VA_BASE`（`0xFFFFFFC0_00000000`），LMA = PA（链接脚本 `AT()`）。单一偏移覆盖镜像与全物理直映射：

```rust
pub fn phys_to_virt(pa: usize) -> usize { pa + KERNEL_VA_BASE }
pub fn virt_to_phys(va: usize) -> usize { va - KERNEL_VA_BASE }
```

直映射范围 `[0, max(DRAM 末, MMIO 窗口末))`，按 1GiB mega 项映射（virt 平台 MMIO 0x10000000 起，被首项覆盖）。当前内核虚拟空间只有直映射区与栈窗口两个分区。

### 栈窗口

正式内核栈的专用虚拟分区：高半区顶 vpn2 槽（链接脚本 `STACK_WINDOW_VA_BASE = 0xFFFFFFFFC0000000`，与直映射解耦——直映射槽数上限 255，满配也只到 510，与顶槽结构性互斥）。目的：栈向下溢出立即 store page fault，溢出即时可见（对照 `plans/DEBUG-PLAYBOOK.md` 的静默踩踏事故；构建期兑底见 os/tools/audit_elf.py）。

- **布局真值链**：`os/stack_layout` 纯逻辑 crate 是几何唯一真值（构造期整体校验，host 可测）；数字只写在链接脚本（`STACK_SIZE`/`STACK_GUARD`/`EMERGENCY_SIZE`/`HART_NUM_LIMIT`/窗口基址）→ 汇编 `_ENTRY_CONSTS` 物化 → 内核 `mm::stack_layout()` 构造消费；audit_elf.py 从 ELF 符号表读 `STACK_GUARD` 构建期强制「单函数最大帧 ≤ guard 洞跨度」——否则一次 sp 下调整体越过洞落入邻槽，即时可见失效。
- **布局**：每槽 `[槽底 guard | formal (stack_size − emergency) | emergency guard | emergency]`，步长 `stack_size + 2×guard`。formal sp 从 emergency guard 洞下方起；emergency 占槽顶、fatal 路径专用，独立 guard 使其溢出不再踩入 formal。物理侧按槽连续打包 `stack_size` 字节（formal+emergency 相邻），guard 纯虚拟不占帧——这是 Linux `CONFIG_VMAP_STACK` 同构：物理页同时存在于直映射别名中，但内核只经 sp/窗口 VA 引用栈，**禁止经 phys_to_virt 触碰栈内存**；该禁律由 debug 断言兕底（`phys_to_virt` 拒绝栈物理打包区，release 构建无检查）。
- **建表**：mm init 内、satp 发布前，静态子表（1 中间 + 若干叶表，不入帧池）按 `layout.mappings` 逐页映射（RW、不可执行——`flags::KERNEL_STACK` 无 X）、guard 洞置 invalid；所有 hart 与全部用户表共享同一子树。
- **地址转换**：`virt_to_phys` 是全函数（直映射线性算术 + `layout.translate` 互逆）；同一物理页有两个内核 VA，PA→VA 无唯一逆——`phys_to_virt` 恒给直映射别名。SBI ecall 传 PA 前必须经它（console 缓冲在栈上即依赖此）。
- **用户表拷贝**：栈窗口槽随直映射槽一起拷入用户 root（trap 在用户 satp 下即取调度栈指针）；正常 ProcessDrain 在有界 Root 阶段逐槽剥离共享顶层项后才归还 root，`AddressSpace::drop` 只作未完成构造/回滚的防御兜底。
- **边界**：bootstrap 过渡环境的栈在过渡表的 mega 映射里，不受此防护管辖——bootstrap 期栈溢出不可观测，接受为已知边界（窗口短，audit_elf 兑底大帧）。

### 启动：PA 执行 → 高半区

QEMU 以 raw binary 引导（`-kernel` 加载 ELF 会按 VMA 估算内核末端，高半区 VMA 直接溢出 DRAM；raw bin 由 `riscv64-elf-objcopy -O binary` 按 LMA 生成，ELF 仅供 gdb/符号），`_start` 在 **bare satp、PC=PA** 下执行：

1. `_start` 及开 MMU 前的全部代码与位置无关纪律：`la` 取到的是 VMA，访问 PA 需减链接期常量 `_va_pa_delta`（镜像 VMA 基 - LMA 基）；
2. cold-bootstrap 临时 root 表覆盖当前 PA 与同一物理段的高半区别名；
3. 写入临时 satp、`sfence.vma` 后跳到高半区；
4. 高半区继续段构建正式内核页表（直映射区全量），写正式 satp并再次 `sfence.vma`，此后内核恒在高半区执行。

**地址纪律**：raw 引导无 ELF 加载器清 bss，各空间 bss 由对应入口汇编清零；SBI ecall 的地址参数（DBCN base_addr、HSM start_addr）一律传 PA，内核指针必须先 `virt_to_phys`；正式内核表无低半区 identity 映射，切表后一切 PA 访问必须经 `phys_to_virt`。裸 PA 直访只存在于 bootstrap 与永久 secondary PA 前导。

### secondary hart

HSM 唤醒入口是永久无栈 PA 前导：从 record PA 取得过渡表，按“过渡 satp→`sfence.vma`→高半区→正式 satp→`sfence.vma`”进入 formal entry。它不复用可回收的 cold-bootstrap 环境。完整生命周期见 [`execution-context.md`](execution-context.md)。

### trap

任意用户页表共享内核高半区与栈窗口，用户 trap 不切 satp；`stvec` 恒指共同内核入口。用户上下文存于内核对象，`sscratch` 存本 hart 陷阱锚。内核稳态 SUM=0，只有 user-copy guard 可以临时直访用户 VA。

## 用户地址空间

`Process.space` 是稳定 `AddressSpace` 外壳：单调不复用的 identity、translation/instruction epoch 和内部 `AddressSpaceState` 锁分离。state 内的 `MemorySpace` ledger 是全部用户区域的 VA 真值；anonymous 区域由 `OwnedBacking(BackingId + logical page offset + affine FrameTracker extents)` 持有，Tunnel 区域由 `ObjectView(ObjectId + LeaseKey + RegionKey + PageRange)` 引用 Connection 的固定 backing，PTE 只是投影。Remote Call 只引用外壳身份与 epoch。Running 变更在 Commit 前快照 lifecycle execution gate，准备 planner/backing 或 WritePermit/PTE/WaitContext 和全部目标槽；Commit 在 `ADDRESS_SPACE → LIFECYCLE` 下复检 active sequence、发布 ledger/PTE/epoch 并登记 mandatory operation，锁外敲门铃；最后 ack 后才 Synchronize→Retire→Complete。

低半区 `[0, 2^38)` 完全归用户；进程 root 创建时通过 `attach_shared_root` 挂入内核高半区顶层项（含栈窗口槽），PTE 安装与 shared 位登记同一调用完成。当前区间：

```text
[0, brk')             ELF LOAD 段
[brk', block_end)     只读 StartupBlock
[block_end, stack)    Extend 向上扩展的堆
[2^38 - 8MiB, 2^38)  首线程栈（libprocess 放置约定；内核仅映射 init bootstrap 栈）
```

brk 在 launch 时越过 init bootstrap 出生块；Extend 仍提供既有 sbrk ABI，但非零增长已经是异步 `MemoryChange`：Commit 前失败零副作用，Commit 后线程转 Waiting，Remote ack 后返回新 brk。普通进程的出生块由组装者（libprocess）写入映像顶之上页对齐的约定区；首线程 sp 由组装者经 ProcessAttach 供给（libprocess 置于 `2^38`，16 字节对齐）。ASID 恒 0，地址空间切换与 Remote Call 第一版均执行保守全量 `sfence.vma`。

- 进程页表 root 的内核高半区由 `shared_root` 位图标记，用户树 Drop/Drain 不进入这些外部子树；
- owned anonymous、ELF、bootstrap stack、StartupBlock 与 Running Extend 页均由 `OwnedBacking.extents` 持有；旧 `AddressSpace.frames`、`alloc_map` 与 `DrainStage::Frames` 已删除；PTE 调用先 prepare 精确表帧 reservation，再不可失败 publish；
- bootstrap StartupBlock prefix 是 owned backing；opaque payload 页映入 init 时收编进同一个 backing（启动保留洞的帧首次入账），地址空间销毁时随 extent 归还帧池；initial ELF 复制完成后 package prefix 页对齐前缀回投帧池；
- ProcessMap 只服务 Building process，创建 anonymous zero pages 并使用最终权限，拒绝 write-only/W+X；ProcessWrite 经已发布 PTE 的物理直映射回填 backing，Running 发布后不再存在该写入口；
- ProcessDrain 先逐区域清空 ledger，再逐 extent 归还 backings，最后收束页表。lifecycle 的 mandatory operation 屏障保证 REAPABLE 前无在途 `PublishedChange`；
- TunnelCreate/Attach 使用内部 MemoryObject view 建立 lease-owned RW mapping；每个 reserved/published/retiring writable view 持一个 affine WritePermit。MemoryObject state 与 Connection side state 分锁，permit 在进入 AddressSpace 前移出对象锁，Retire 也在 AddressSpace 锁外归还。Create/Attach 与显式 Endpoint HandleClose 都使用预构造 WaitContext 和 mandatory Remote completion；Close 先提交 ledger/PTE Unmap，远端确认后才 Retire permit、发布 CLOSED/PEER_CLOSED 并完成 syscall。Terminating 进程 active 已归零，ProcessDrain 仍走同一 planner→PTE→Retire 顺序；若 detached close 与在途 transaction 冲突，entry 原样留在 `pending_close` 供下一批重试。旧 `external_mappings`、按 VA 搜索、本地 `sfence.vma` 与 Drop 隐式解除已删除。

## 架构边界

admitted hart、无 MMU hart 与 AMP 边界见 [`execution-context.md`](execution-context.md)。当前内核与用户目标均为 RV64；地址运算使用 usize，外部线协议才使用固定宽编码。
