# 内存管理

分四层：帧池（物理帧）、Sv39 纯逻辑（可 host 测试）、内核地址空间与启动协议、用户地址空间。地址空间布局见 `internals.md`；旧实现的教训见 `plans/2026-08-mm-map-bug.md`（六条注意事项全部为本文设计的反面输入）。

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

- 区域来源：DTB `memory` 节点，剔除已占用区间——`[0x80000000, _kernel_end_pa)`（SBI + 内核镜像 + 栈）、initfs 所在段（loader 加载的 tar 区）。
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

切段算法数值用例：未对齐跨表大区间（mm-map-bug 原案 `vpn=65, count=8192`）、整表对齐、大页对齐、未对齐首尾混合、同 flags 幂等、异 flags 冲突、unmap 后重映射、大页分裂后部分 unmap。

## 内核地址空间与启动协议

### 链接与线性偏移

内核镜像 VMA = PA + `KERNEL_VA_BASE`（`0xFFFFFFC0_00000000`），LMA = PA（链接脚本 `AT()`）。单一偏移覆盖镜像与全物理直映射：

```rust
pub fn phys_to_virt(pa: usize) -> usize { pa + KERNEL_VA_BASE }
pub fn virt_to_phys(va: usize) -> usize { va - KERNEL_VA_BASE }
```

直映射范围 `[0, max(DRAM 末, MMIO 窗口末))`，按 1GiB mega 项映射（virt 平台 MMIO 0x10000000 起，被首项覆盖）。比 Linux 的分区直映射（image/vmalloc/modules 分区）简化为单一区——我们的规模不需要分区，偏移纪律更简单。

### 启动：PA 执行 → 高半区

QEMU 以 raw binary 引导（`-kernel` 加载 ELF 会按 VMA 估算内核末端，高半区 VMA 直接溢出 DRAM；raw bin 由 `riscv64-elf-objcopy -O binary` 按 LMA 生成，ELF 仅供 gdb/符号），`_start` 在 **bare satp、PC=PA** 下执行：

1. `_start` 及开 MMU 前的全部代码与位置无关纪律：`la` 取到的是 VMA，访问 PA 需减链接期常量 `_va_pa_delta`（镜像 VMA 基 - LMA 基）；
2. `TRAMPOLINE_PG_DIR`：静态 root 表，两条 mega 项——首 1GiB PA identity 映射 + 同一物理段的高半区映射（内核镜像/栈/bss 都在首 1GiB 内）；
3. 装 `satp = TRAMPOLINE_PG_DIR`，`sfence.vma`，`jalr` 跳 `继续标签 + _va_pa_delta`... 即跳到高半区地址的同一代码；
4. 高半区继续段：构建正式内核页表（直映射区全量），切换 satp，此后内核恒在高半区执行。

对应 Linux `head.S` 的 `trampoline_pg_dir → relocate_enable_mmu` 一次性机构。

**地址纪律**（高半区迁移的指针契约，两处实战教训）：raw 引导无 ELF 加载器清 bss，各空间 bss 由各自入口汇编清零；SBI ecall 的地址参数（DBCN base_addr、HSM start_addr）一律传 PA，内核指针必须先 `virt_to_phys`；正式内核表无低半区 identity 映射，切表后一切 PA 访问必须经 `phys_to_virt`，裸 PA 直访仅限 `.text.init` 裸机段。

### secondary hart

HSM 唤醒入口指向 PA 侧 `_start` 等价段：各自装 `TRAMPOLINE_PG_DIR` 跳高半区后，再走 `_awaken` 装配（tp/栈/stvec）。不再需要裸 PA 长驻路径。

### trap（M3 实现，mm 侧契约）

无 trampoline 页：`stvec` 恒指内核 .text（任意用户页表共享内核顶层项，trap 不切 satp）。用户内存访问开 SUM 位直访 VA。trap 帧存内核侧（M3 定存储位置），`sscratch` 存本 hart 陷阱锚。

## 用户地址空间（M3 消费）

- 低半区 `[0, 2^38)` 完全归用户，进程页表 root 创建时拷贝内核高半区顶层项；
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
