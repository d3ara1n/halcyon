# mm 映射算法 bug 档案（重构参考）

> 来源：2026-08 迁移开发环境到 macOS + rust nightly 升级适配后，`just virt` 内核 panic 的完整诊断。
> 行号以诊断时（65e0b14）为准，重构时以符号名 `grep` 定位。
> 结论先行：**不是升级引入的 regression，是 `mm/unit.rs` 手写多级页表递归里潜伏的 copy-paste 错误，升级改变了用户态堆分配模式后踩中**。重写 mm 时照抄本文件标注的设计缺陷会原样复现。

---

## 症状与复现

- **4 核（`just virt`）**：四个系统服务起来后，`fs`/`init` 各请求 0x2000000 字节堆扩展，随即 pm 进程在 `0x105e4` 取指 fault：
  ```
  Kernel panicking #3
  kernel/src/hart/app.rs:674: unexpected Execute memory page fault at: 0x105e4, sepc=0x105e4
  ```
  注意：panic 的是 pm（Pid=3，只 extend 过 0x1000），改页表的是 fs（Pid=2）/init（Pid=4）——跨进程破坏的表象实际是同因（详见下文「进程隔离假象」）。
- **单核（`-smp cores=1`）**：不 panic 但挂起，输出停在 `app.extend Pid=2 request 0x2000000 bytes` 之后；gdb attach 见 fs 仍在用户态执行（等待循环），pm/init 从未被调度。单核挂起是**独立的调度问题**，见「附带发现」。

复现命令（agent 环境需自包含清理 QEMU）：

```sh
just virt                       # 4 核，数十秒内 panic
just PLATFORM=qemu MODEL=virt MODE=debug run_qemu -smp cores=1   # 单核，挂起
```

## 根因：`map_internal` 未对齐分支的递归参数错误

`os/kernel/src/mm/unit.rs` map_internal 未对齐分支（诊断时 365-372 行），第二段递归把父表 `container` 误传成子表 `table`：

```rust
Self::map_internal(table, vpn, ppn, remaining, flags, level - 1)  // 第一段：进子表，降级 ✓
if count > max_remaining {
    Self::map_internal(
        table,                   // ✗ 应为 container——对照 aligned 分支尾段的正确写法
        vpn + max_remaining,
        next,
        count - max_remaining,
        flags,
        level,                   // ✗ 与 table 不匹配：L0 表配 L1 索引逻辑
    )
}
```

`free_internal` 的对应分支（~167-183 行）存在**完全相同的错误**（第二段同样传 `table`+`level`）——两处同源 copy-paste 错位，补丁需一起打。

**gdb 实测证据**（`-S -s` 冻结启动，断点 unit.rs:365）：

```
#0 map_internal (vpn=65, count=8192, level=1) at unit.rs:365
#1 map_internal (vpn=65, count=8192, level=2) at unit.rs:399
#2 fill (vpn=65, count=8192) at unit.rs:81
#3 Process::extend (size=33554432) at task/proc.rs:206
```

fs 的 32MB 堆扩展（brk=0x21000 未 1GiB 对齐 → 未对齐分支；count=0x2000 > max_remaining=0x1bf → 必走第二段）确定命中。

### 毁灭链条（每步都有代码对应）

以 fs 为例（brk=0x21000，即 L0 表位于 L2[0]/L1[0]）：

1. **第一段**（进子表，逻辑本身正确）：`ensure_managed_leaf_created` 遇到**已存在的 leaf** 时不报错，而是 `set_flags(R|W)`（page.rs ~218 行）——静默清掉 text/rodata 页的 X 位，且把权限直接改写为堆权限。
2. **第二段**（bug 所在）：在 **L0 叶表**上按 level 1 索引把 `L0[2..15]` 改写成 1 级"大页"——但 L0 表没有下一级，这些 entry 实际成了指向物理页 2..15（内核低地址物理页）的可写映射，虚拟地址 0x2000..0x10000（rodata + text 头部）全毁。
3. **尾段**（vpn=0x2000 起，同样带 bug 的表/级别错配）：`ensure_table_created(L0[0x10])` 遇到 fs text 页（vaddr 0x10000）返回 `LeafExists`，走 `split_page_into_table`（unit.rs:477）——在 L0 表上做 level-1 分裂，`free_entry` 回收原 text 物理帧，然后把 `L0[0x10]` 写成 Valid non-leaf。**Sv57 规范里 level 0 的 non-leaf PTE 是 reserved 非法编码，硬件 walk 直接 page fault**。
4. fs 从 ecall 返回用户态，取指 0x105e4（readelf 确认在 fs text `[0xeea0, 0x3e1c0)` 内，页 0x10）→ Execute fault → `app.rs:674` 的 `todo!()` panic。

### 进程隔离假象

panic 的是 pm 而不是 fs：4 核下 fs 破坏完自己页表后先被调度走，pm 随后在自己的 text 上 fault（pm text `[0x69dc, 0x1714c)` 同样含 0x105e4）。每个进程的 L0 表都是独立帧，bug 只毁本进程映射——**不是跨进程写坏，是各进程先后自杀，谁先跑到毁坏页谁 panic**。这与 KNOWN_ISSUES 的调度器 SMP unsoundness 无关（单核同因，只是还没执行到毁坏页）。

## 为什么迁移 macOS + rust 升级后才炸

32MB 请求不是硬编码：用户态 talc 分配器按 2 的幂增长动态 `sys_extend`（`user/rinlib/src/rt.rs` 的 `acquire`）。旧代码里用户程序堆用量小、只到过 0x1000 量级；升级依赖（talc 3→5 等）+ 新 nightly 改变了用户态二进制布局与分配模式，`String::with_capacity`/format 等触发了一次 32MB 的堆增长，`brk` 落点 + 请求大小的组合恰好满足未对齐分支的第二段条件。bug 自 `1dcb7ab`（unit 职责迁移到 process 时重写）起就在，只是从未有进程发起过这么大的未对齐扩展。

## 附带发现：单核调度挂起（独立问题）

单核下 fs 阻塞在用户态等待（PC 0x1d1d8），此后 pm/init 永远没有被调度——调度/唤醒路径存在挂起（方向与 KNOWN_ISSUES「调度器 SMP soundness」条目一致，但单核也复现，说明不纯是别名 UB）。重构调度器时需一并解决；本文件不展开。

## 重写注意事项

按破坏链条倒序，设计层面规避：

1. **不要手写「同一张表上混用多级索引」的递归**。本次 bug 的结构根源是 map/free 的递归把「当前表、当前级别、剩余数量」三个量耦合在两处循环 + 尾递归里，aligned/unaligned/尾段三路写法高度相似又各有微差。替代设计：
   - **区域切段**：先把 `[vpn, vpn+count)` 按当前级别的对齐边界切成若干段，每段只走「对齐 → 大页 or 整表下放」的单一路径，段与段之间无共享状态；
   - 或**约定堆从大页边界对齐分配**（brk 起点对齐到 2MiB/1GiB），让用户态 extend 永远走 aligned 路径——简单但把不变量藏在分配约定里，需在 `Process::extend` 入口 assert；
   - 无论哪种，level 0 的表**禁止任何 non-leaf 写入**，这是硬件规范红线，值得一个 debug_assert + 类型层面区分（`LeafTable` / `MidTable` 两个类型）。
2. **映射冲突必须显式报错**。`ensure_managed_leaf_created` 遇已有 leaf 时 `set_flags` 静默改权限（清 X 位）是本次链条的第一环。映射已存在区域时应当：完全同权限 → 幂等成功；否则 → `EntryOverwrite` 错误。绝不静默改写。
3. **`split_page_into_table` 的级别合法性**。分裂只对 level ≥ 1 合法；对 level 0 调用即产生非法 PTE。重构时把「分裂」收进 MidTable 类型的方法里，叶表无此操作。
4. **页表纯逻辑必须上 host 单测**。映射算法是纯索引计算，天然可在 host 上 `cargo test`：构造 → fill 未对齐区域 → 逐页断言 translate 结果与权限 → free → 再映射。本次 gdb 三小时定位的问题，一组快照测试几毫秒就能拦住。测试用例直接抄本文件的数值：`vpn=65, count=8192, brk=0x21000`（32MB 未对齐扩展）。
5. **`Process::extend` 的返回值语义**。当前返回新堆区末尾，用户态 talc 靠 `offset - size` 反推 base——隐含「新区域必须连续接在旧堆后」的假设。重写时要么内核保证连续性并在文档写明，要么显式返回 (base, end)。
6. **panic 路径不要用 `todo!()` 兜底用户可触发的 fault**（app.rs:674）。用户进程页故障的正确终态是杀进程（回收资源、通知 pm），不是内核 panic。
