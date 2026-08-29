# 内存管理

方向见 [`../ideas/mm.md`](../ideas/mm.md)。当前实现分四层：帧池、可 host 测试的 Sv39 页表、内核地址空间与启动协议、用户地址空间；本篇是内存实现事实的唯一拥有者。

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

no_std + alloc 的独立 crate，`cargo test` 在 host 直接跑，内核 target 复用同一份代码。**页表树 const 泛型于 `LEVELS`**（3=sv39、4=sv48、5=sv57——三者的 PTE 编码相同，仅级数与 VA 宽度不同，均可由 LEVELS 推导）——不写死 sv39 是硬约束。**页表逻辑不得直接解引用物理地址**——所有表访问经 trait 抽象：

```rust
pub trait FrameMemory {
    fn table(&mut self, frame: FrameNumber) -> &mut [Pte; 512];
}
```

内核实现以 `phys_to_virt` 转换，host 实现以 `Vec` 模拟。

### 类型

- `FrameNumber(usize)`、`Vpn(usize)`（4KiB 页号）、`Ppn(usize)`——newtype，禁止裸 usize 传递。
- `Pte(u64)`：编码/解码、标志位（V R W X U G A D）、`leaf()`/`branch()` 判别；组合常量（用户页、内核直映射页等）集中定义。
- `TableTree<M: FrameMemory, const LEVELS: usize>`：root 帧（经 `M` 分配/释放）、`map / unmap / translate`；`type Sv39<M> = TableTree<M, 3>`。

**Root 借用模型（当前简化）**：TableTree 名义拥有全部中间表，但用户 root 拷入内核共享子树（直映射/栈窗口槽），靠 `AddressSpace` 收束阶段手工清除共享 root slots 后再释放 owned 子树。所有权区分尚未进入 TableTree 类型系统，正确性依赖该唯一收束入口。

### 区域切段算法

`map(vpn 区间, 连续 ppn, flags)` 把区间按**当前级别的对齐边界**切为若干段（匿名整备＝先取帧再映射，由内核 mm 层组合，本 crate 只管表结构）；每段取最大可行 mega 级（对齐且整段覆盖 512^l 页：2MiB/1GiB/…），走单一路径，无递归分叉：

1. 段首与段长均按 2MiB 对齐（且 ppn 对齐）→ L1 大页；
2. 段首与段长按整表（512 页）对齐 → 挂新中间表，下放继续；
3. 其余 → 叶表内逐 PTE 建立。

每步只做一次决策，段与段之间无共享状态。

- 冲突策略：目标位置已有有效映射时，**同 flags 幂等成功，异 flags 返回 `MapConflict`**，禁止静默改权限。
- 大页分裂：在已映射 2MiB 区间内需要 4KiB 粒度时，`split` 分配叶表、512 项继承原 flags 展开。level 0 禁止 non-leaf，debug_assert。
- `unmap` 走同一套切段逻辑，对称解除；递归显式携带当前子表的实际覆盖基址，跨 512 页边界不会复用初始请求基址。mega 部分解除必须先成功分裂，表帧耗尽会返回 `FrameExhausted` 并保持原映射；空中间表当前保留到整棵 AddressSpace Drop。

### 测试集（host）

切段算法数值用例：未对齐跨表大区间（`vpn=65, count=8192`）、跨子表批量 unmap、整表/大页对齐、未对齐首尾混合、同 flags 幂等、异 flags 冲突、unmap 后重映射、大页分裂后部分 unmap、split OOM 保持原映射、clear_slots 剥离不递归。

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

低半区 `[0, 2^38)` 完全归用户，进程 root 创建时拷贝内核高半区顶层项（含栈窗口槽）。当前区间：

```text
[0, brk')             ELF LOAD 段
[brk', block_end)     只读 StartupBlock
[block_end, stack)    Extend 向上扩展的堆
[2^38 - 8MiB, 2^38)  首线程栈（libprocess 放置约定；内核仅映射 init bootstrap 栈）
```

brk 在 launch 时越过 init bootstrap 出生块；Extend 从 brk 逐页映射并返回新 brk。普通进程的出生块由组装者（libprocess）写入映像顶之上页对齐的约定区；首线程 sp 由组装者经 ProcessAttach 供给（libprocess 置于 `2^38`，16 字节对齐）。ASID 恒 0，地址空间切换执行全量 `sfence.vma`。

- 进程页表 root 共享内核高半区顶层项；
- owned anonymous/ELF/stack/普通 StartupBlock 页由 `AddressSpace.frames` 的一个或多个 FrameTracker extent 持有；任何 PTE 安装前先按最坏 extent 数 `try_reserve` 记账容量，批量安装失败按逆序 unmap 后才释放 backing；
- bootstrap StartupBlock prefix 是 owned 页；opaque payload 页在映入 init 时即收编为该地址空间的 owned FrameTracker（启动保留洞的帧首次入账），地址空间销毁时随 owned 帧归还帧池；initial ELF 复制完成后 package prefix 页对齐前缀回投帧池；
- ProcessMap 只服务 Building process，创建 anonymous zero pages并使用最终权限，拒绝 W+X；ProcessWrite 经物理直映射写 backing，Running 发布后不再存在该写入口；
- Tunnel 映射由 Endpoint lease 记入 `AddressSpace.external_mappings`，关闭时解除。

## 架构边界

admitted hart、无 MMU hart 与 AMP 边界见 [`execution-context.md`](execution-context.md)。当前内核与用户目标均为 RV64；地址运算使用 usize，外部线协议才使用固定宽编码。
