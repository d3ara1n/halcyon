# 内存管理

分四层：帧池（物理帧）、Sv39 纯逻辑（可 host 测试）、内核地址空间与启动协议、用户地址空间。地址空间布局见 `internals.md`；旧实现的教训见 `plans/review-2026-08-mm-map-bug.md`（六条注意事项全部为本文设计的反面输入）。

## 帧池

物理帧的唯一来源，自研 **in-band 有序空闲链**（os/frame_pool，纯逻辑 crate），包装进内核 `Spinlock`，经 `PoolMemory` trait 抽象内存访问——与 page_table 同一「host 可测」流水线。

选型（2026-08，弃 buddy_system_allocator）：其 free list 元数据是堆上 BTreeSet，每次分配碰 talc+锁，帧池与堆形成运行时互依；且 alloc 按 2 的幂取整，与精确 count 记账不合。frame-alloc（TUM 2026-07）设计对路但 RISC-V/SMP 未测试。自研理由：算法需求收敛（任意 count 连续、精确记账、零堆依赖），in-band 结构比 buddy 更简单。

### 结构

空闲区间按地址排序成单链，节点 `{len, next}` 内嵌于区间首帧的前两个 usize——**空闲内存自身承载元数据**，零堆依赖、零外部存储。节点内容是帧号而非地址（`next` 为 usize，`usize::MAX` 哨兵表链尾，不依赖 Rust 对 Option 的内部表示），与地址转换解耦：内核从 bare 切到高半区直映射时，`FramePool::into_mem` 取回后端、换转换函数重建实例即可，链结构零迁移。

```rust
pub struct RegionNode { pub len: usize, pub next: usize }

pub trait PoolMemory {
    fn read_meta(&mut self, frame: FrameNumber) -> RegionNode;
    fn write_meta(&mut self, frame: FrameNumber, node: RegionNode);
    fn clear_frames(&mut self, base: FrameNumber, count: usize);
}

pub struct FramePool<M: PoolMemory> { /* mem, head, free_frames */ }
```

- 内核实现：`phys_to_virt` 转换后裸访问（bare 期恒等），unsafe 边界收敛在此；host 实现：`BTreeMap` 模拟帧槽。

### 操作与契约

- `add_region(start, end)`：注册启动期空闲区间（DTB memory 剔除内核镜像/栈/initfs 后），地址序入链。
- `alloc_contiguous(count)`：first-fit 取首个足够区间，**从尾端切**——低地址区间先消耗，大区间主体保持完整；整取时重链，否则节点原地减 len；返回前整块清零（安全 + 上层拿来即用）。
- `alloc_at(base, count)`：取指定区间（启动协议三件套、页表解映射回投）；区间内三向切割，左右残段各自成节点；不可用（未注册/已分配）返回 `Err`。
- `dealloc(base, count)`：按地址序插入 + 相邻三向合并。debug 断言拒绝与现有空闲区间重叠（双重释放检测）。
- 复杂度：分配/释放 O(空闲区间数)，教学规模无压力；分桶/命中提示等优化留待需要时在范式内逐点做。

- 区域来源：DTB `memory` 节点，剔除已占用区间——`[0x80000000, _kernel_end_pa)`（SBI + 内核镜像 + 栈）、initfs 所在段（loader 加载的 tar 区）。**（已知简化）**占用剔除目前是启动期固定两洞 + bootstrap `free_range` 回投两条路径；接入新占用方（保留设备区/新平台保留段）时，应收敛为排序合并、重叠校验的 Boot Reservations 集合，让非法重叠在注册期即被拒绝。
- `FrameTracker { base, count }` RAII 归还，Drop 时整批 dealloc；进程持有的页帧以此为单位记账（M3）。
- 内核堆由帧池供血：talc 内存源（FrameSource）耗尽时取 1MiB 连续帧块 claim 建新区，帧块所有权终身归堆（acquire 内不可碰堆与锁，无归还记账）；启动路径（DTB 解析/帧池注册）零堆依赖，引导序线性：帧池 → 堆首分配 → 一切堆消费者。

### 测试集（host）

整取整还、尾端切地址递减、三向合并、双重释放、alloc_at 中切/边切/失败、多区域跨链、碎片化后总帧数守恒、空池与大请求拒绝。

## 页表模式选择

单模式（全系统同一 satp 模式）是共享内核映射的结构性上限而非妥协：内存物理上同一份，异模式 hart 各持平行映射树，反而割裂调度域（进程不可跨模式迁移）——多模式无收益。模式是启动期从硬件自动识别，不依赖手工配置：dtb 各 cpu 节点的 `mmu-type` 给出每个 hart 的支持上限，取全体 Application hart 的**最小上限**（硬件允许集），与内核支持集取交集选最高模式；不支持交集（如仅 sv32）则拒绝启动。运行时选出的模式作为常量贯穿后续初始化（satp 组装、地址宽度断言）。

内核支持集由编译期决定（当前 {sv39}），扩充到 sv48/sv57 只是打开配置，不是重写。

## 页表纯逻辑（os/page_table crate）

no_std + alloc 的独立 crate，`cargo test` 在 host 直接跑，内核 target 复用同一份代码。**页表树 const 泛型于 `LEVELS`**（3=sv39、4=sv48、5=sv57——三者的 PTE 编码相同，仅级数与 VA 宽度不同，均可由 LEVELS 推导）——不写死 sv39 是硬约束。**页表逻辑不得直接解引用物理地址**——所有表访问经 trait 抽象：

```rust
pub trait FrameMemory {
    fn table(&mut self, frame: FrameNumber) -> &mut [Pte; 512];
}
```

内核实现 = `phys_to_virt` + 转换；host 实现 = `Vec` 模拟。旧实现的 `&'static mut [E]` 别名与 `PageTableIter` 自引用问题在此结构下不存在。

### 类型

- `FrameNumber(usize)`、`Vpn(usize)`（4KiB 页号）、`Ppn(usize)`——newtype，禁止裸 usize 传递。
- `Pte(u64)`：编码/解码、标志位（V R W X U G A D）、`leaf()`/`branch()` 判别；组合常量（用户页、内核直映射页等）集中定义。
- `TableTree<M: FrameMemory, const LEVELS: usize>`：root 帧（经 `M` 分配/释放）、`map / unmap / translate`；`type Sv39<M> = TableTree<M, 3>`。

**Root 借用模型（已知简化）**：TableTree 名义拥有全部中间表，但用户 root 拷入内核共享子树（直映射/栈窗口槽，见「栈窗口」），靠 `AddressSpace::drop` 手工 `clear_slots` 配对剥离——所有权事实与类型声明不一致，配对纪律靠调用点自觉。扩展用户表共享分区（procfs/调试映射）或新增 teardown 路径时，应收敛为 root 槽所有权显式登记（Drop 只递归 owned 槽），消灭逐点配对并移除通用 `clear_slots` 逃生口。

### 区域切段算法

`map(vpn 区间, 连续 ppn, flags)` 把区间按**当前级别的对齐边界**切为若干段（匿名整备＝先取帧再映射，由内核 mm 层组合，本 crate 只管表结构）；每段取最大可行 mega 级（对齐且整段覆盖 512^l 页：2MiB/1GiB/…），走单一路径，无递归分叉：

1. 段首与段长均按 2MiB 对齐（且 ppn 对齐）→ L1 大页；
2. 段首与段长按整表（512 页）对齐 → 挂新中间表，下放继续；
3. 其余 → 叶表内逐 PTE 建立。

每步只做一次决策，段与段之间无共享状态——这是对旧三路递归（未对齐分支传参错位的温床）的结构性替代。

- 冲突策略：目标位置已有有效映射时，**同 flags 幂等成功，异 flags 返回 `MapConflict`**——禁止静默改权限（旧 `ensure_managed_leaf_created` 的教训）。
- 大页分裂：在已映射 2MiB 区间内需要 4KiB 粒度时，`split` 分配叶表、512 项继承原 flags 展开。level 0 禁止 non-leaf，debug_assert。
- `unmap` 走同一套切段逻辑，对称解除；空中间表归还帧。

### 测试集（host）

切段算法数值用例：未对齐跨表大区间（mm-map-bug 原案 `vpn=65, count=8192`）、整表对齐、大页对齐、未对齐首尾混合、同 flags 幂等、异 flags 冲突、unmap 后重映射、大页分裂后部分 unmap、clear_slots 剥离不递归。

## 内核地址空间与启动协议

### 链接与线性偏移

内核镜像 VMA = PA + `KERNEL_VA_BASE`（`0xFFFFFFC0_00000000`），LMA = PA（链接脚本 `AT()`）。单一偏移覆盖镜像与全物理直映射：

```rust
pub fn phys_to_virt(pa: usize) -> usize { pa + KERNEL_VA_BASE }
pub fn virt_to_phys(va: usize) -> usize { va - KERNEL_VA_BASE }
```

直映射范围 `[0, max(DRAM 末, MMIO 窗口末))`，按 1GiB mega 项映射（virt 平台 MMIO 0x10000000 起，被首项覆盖）。比 Linux 的分区直映射（image/vmalloc/modules 分区）简化为两个分区：直映射单一区 + 栈窗口（见下）；不再增加分区的门槛是真实需求。

### 栈窗口

正式内核栈的专用虚拟分区：高半区顶 vpn2 槽（链接脚本 `STACK_WINDOW_VA_BASE = 0xFFFFFFFFC0000000`，与直映射解耦——直映射槽数上限 255，满配也只到 510，与顶槽结构性互斥）。目的：栈向下溢出立即 store page fault，溢出即时可见（对照 `plans/DEBUG-PLAYBOOK.md` 的静默踩踏事故；构建期兑底见 os/tools/audit_elf.py）。

- **布局真值链**：`os/stack_layout` 纯逻辑 crate 是几何唯一真值（构造期整体校验，host 可测）；数字只写在链接脚本（`STACK_SIZE`/`STACK_GUARD`/`EMERGENCY_SIZE`/`HART_NUM_LIMIT`/窗口基址）→ 汇编 `_ENTRY_CONSTS` 物化 → 内核 `mm::stack_layout()` 构造消费；audit_elf.py 从 ELF 符号表读 `STACK_GUARD` 构建期强制「单函数最大帧 ≤ guard 洞跨度」——否则一次 sp 下调整体越过洞落入邻槽，即时可见失效。
- **布局**：每槽 `[槽底 guard | formal (stack_size − emergency) | emergency guard | emergency]`，步长 `stack_size + 2×guard`。formal sp 从 emergency guard 洞下方起；emergency 占槽顶、fatal 路径专用，独立 guard 使其溢出不再踩入 formal。物理侧按槽连续打包 `stack_size` 字节（formal+emergency 相邻），guard 纯虚拟不占帧——这是 Linux `CONFIG_VMAP_STACK` 同构：物理页同时存在于直映射别名中，但内核只经 sp/窗口 VA 引用栈，**禁止经 phys_to_virt 触碰栈内存**（绕过即无防护）。
- **建表**：mm init 内、satp 发布前，静态子表（1 中间 + 若干叶表，不入帧池）按 `layout.mappings` 逐页映射（RW、不可执行——`flags::KERNEL_STACK` 无 X）、guard 洞置 invalid；所有 hart 与全部用户表共享同一子树。
- **地址转换**：`virt_to_phys` 是全函数（直映射线性算术 + `layout.translate` 互逆）；同一物理页有两个内核 VA，PA→VA 无唯一逆——`phys_to_virt` 恒给直映射别名。SBI ecall 传 PA 前必须经它（console 缓冲在栈上即依赖此）。
- **用户表拷贝**：栈窗口槽随直映射槽一起拷入用户 root（trap 在用户 satp 下即取调度栈指针）；进程 teardown 前 `AddressSpace::drop` 必须先剥离这些共享顶层项（`TableTree::clear_slots`），否则树回收会把内核子表当用户页表拆掉回投（双重释放 + 栈内存被复用）。
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

## 用户地址空间（M3 消费）

- 低半区 `[0, 2^38)` 完全归用户，进程页表 root 创建时拷贝内核高半区顶层项（含栈窗口槽）；
- 用户区布局（程序/堆/栈/隧道区）沿用旧设计，M3 随任务模型定稿。

## 施工顺序

1. `os/page_table` crate：类型/Pte/FrameMemory/TableTree 切段算法 + host 测试集全绿（含 mm-map-bug 数值用例）。
2. 帧池：os/frame_pool 纯逻辑 crate（host 测试集）+ 内核 Spinlock 包装，DTB 段注册（剔除内核镜像/栈/initfs 占用）。
3. 启动协议：链接脚本高半区化（VMA/LMA 分离）+ TRAMPOLINE_PG_DIR 双 mega 项 + _start PA 段 + secondary 同路径。
4. 收口：堆 arena 切帧池供给（消灭 HEAP_ARENA）、just virt 高半区 MMU 下回验收线（banner + 4 核 online）。

步骤 3 动启动路径，独立成段实施。

## 异构 hart 与 rv32（纪律）

- HartKind 扩展点与实时核 AMP 方向见 `internals.md`「hart 种类」；跨 hart 共享数据不假设全体核有 MMU。
- rv32 无设备目标，park。纪律：地址运算一律 usize（不用 u64 写死）；64 位魔数仅限本 crate（rv64 家族专属）；shared ABI 的地址类字段用 usize——rv32 目标下内核与用户两侧同宽，天然成立；仅面向网络/外部交换的数据交换才需要定宽编码。
