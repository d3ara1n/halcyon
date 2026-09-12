# RV64 共享内存访问的语言与平台证据

> 【只读参考资料】为多页 Tunnel/RNL2 实施前设计取证。事实与证明边界在此记录；推荐方案与交付义务由 [`数据面计划`](todo-2026-09-memory-object-data-plane.md) 和 `notes/ideas/shared-memory.md` 拥有。

## 版本

本机调查工具链：`rustc 1.100.0-nightly (c54751567 2026-08-22)`，完整提交 `c54751567b19c4ceb08b0412d83529c2568cba8b`，LLVM 23.1.0。仓库 `rust-toolchain` 实际写 `nightly`；这是本轮观测基线，不把浮动 channel 称为固定版本。实施验证时记录实际 rustc/LLVM，若变化则重新核对相关代码生成。

RISC-V 固定规范为 `references/normative/riscv-isa-v20250508/src/`，入口见 `references/CONTRACTS.md`。既有广泛系统取样见 `references/systems/INDEX.md` 和 `plans/ref-2026-09-ipc-data-plane-systems.md`；其中语言内共享（Theseus/RedLeaf）与不可信跨进程共享的假设不能互换。

## Rust 原子契约

固定源码：[core atomic.rs](https://github.com/rust-lang/rust/blob/c54751567b19c4ceb08b0412d83529c2568cba8b/library/core/src/sync/atomic.rs)，在线入口：[Memory model for atomic accesses](https://doc.rust-lang.org/core/sync/atomic/index.html#memory-model-for-atomic-accesses)。

- Rust 原子采用 C++20 `intro.races` 规则，不含 consume；数据竞争是未同步的冲突访问且至少一个非原子，属于 UB。
- 未同步且冲突的原子访问不能部分重叠：必须访问完全相同的字节范围及尺寸、彼此不相交，或都只读。因此不能并发用 AtomicU8 覆盖 AtomicU64 控制字段来构造“恶意测试”。
- `AtomicU8/U32/U64::from_ptr` 要求对齐、整个借用生命周期内可读写，以及遵守原子内存模型；没有“外来进程可以违反上述前提”的豁免。
- 整数可以承载任意已初始化位型，bool 不可以；不可信 EOF 必须先采样整数再检查 0/1，不能直接读 AtomicBool。
- 提供的原子类型保证 lock-free，但具体指令不保证；实现可能用较大原子指令完成小尺寸操作。不能由类型名推断 RV64 的每条实际访问指令。

这些材料没有给出任意跨进程、不合作写者的完整语言模型。把数据改成 AtomicU8 Relaxed 可以构成合规 host 原子模型，但不能据此声称任意外部非原子/混合宽写已经获得 Rust 标准证明。

## 普通指针、volatile 与 mmap

固定源码：[core ptr/mod.rs](https://github.com/rust-lang/rust/blob/c54751567b19c4ceb08b0412d83529c2568cba8b/library/core/src/ptr/mod.rs)；在线 [ptr safety](https://doc.rust-lang.org/core/ptr/index.html#safety)、[read_volatile](https://doc.rust-lang.org/core/ptr/fn.read_volatile.html)、[write_volatile](https://doc.rust-lang.org/core/ptr/fn.write_volatile.html)。

- ptr 模块普通访问是非原子的；`copy_nonoverlapping` 的范围、对齐与不重叠要求不替代并发论证。
- volatile 对 Rust allocation 内存与普通读写具有相同的并发约束。官方明确：是否 volatile 对多线程并发问题没有帮助，不能用来作线程同步。
- volatile 对所有 Rust allocation 之外的内存有另一组条件：不 trap、其副作用不影响 Rust 分配内存等。这不等于 mmap 都自动属于“分配之外”：ptr 文档本身把经页表/mmap 操作产生的内存纳入 allocation 讨论。
- 仅写“共享物理页不是 Rust 堆”不足以调用 volatile 例外；必须明确映射、别名、访问及平台责任。

## asm 与 ISA

Rust [inline assembly rules](https://doc.rust-lang.org/reference/inline-assembly.html#rules-for-inline-assembly) 规定 asm 可访问的内存位置与 FFI 相同；pure/nomem/readonly 会给优化器更强承诺，nomem 还排除通过内存进行线程同步。asm 本身不是一个自动消除 UB 的语言特例，仍须证明访问范围、存活期和编译器边界。

RISC-V 固定规范：

- `rv32.adoc`「Load and Store Instructions」：自然对齐的 load/store 保证原子执行；非对齐访问不具备同样保证。RV64 的宽度扩展见 `rv64.adoc`。
- `rvwmo.adoc`「Memory Model Primitives」与「Preserved Program Order」：单次内存操作、load-value 与显式同步约束。
- `mm-eplan.adoc`「Fences」「Explicit Synchronization」：`fence r,rw` 对前序读与后序读写建立 acquire 所需顺序，`fence rw,w` 对前序读写与后序写建立 release 所需顺序；这不意味着一对 acquire/release 自动建立所有 store→load 全序。
- `supervisor.adoc`「Supervisor Memory-Management Fence Instruction」：地址翻译失效与数据内存排序职责不同，不能拿门铃或数据 fence 替代 lease shootdown。

数据面计划据此选择一个显式的 RV64 平台访问后端：不向 Rust 业务代码暴露共享引用，所有共享地址读写集中在有内存副作用的 asm 边界，控制字段按规范宽度访问并用 ISA fence 排序。**这是待实现并验证的 Halcyon 平台契约，不是 Rust 官方对跨进程并发已有完整形式定义的结论。**

## 验证能证明什么

- host 的同宽原子控制字段、AtomicU8 数据模型可验证 SPSC 算法、进度界、EOF、回绕和故障状态机；恶意输入也必须通过符合 host 语言模型的写法注入。
- RV64 后端须检查 debug/release 代码生成：共享访问宽度、fence、无普通共享 memcpy、没有意外锁/原子运行库，且仅访问被 owner 保活的范围。
- guest 独立进程可作非合作共享写与关闭测试，验证实际平台上的越界防护、终态与存活性。该运行证据不能升级为 Rust 语言模型的形式证明。
- 任意不可信对端都可破坏共享字节的真实性或拒绝服务；协议只能拒绝已发现的不可能状态，不能承诺检测一切“看似合法”的伪造。

相关官方讨论可用于理解未决边界，但不是规范承诺：[UCG #152](https://github.com/rust-lang/unsafe-code-guidelines/issues/152)、[UCG #476](https://github.com/rust-lang/unsafe-code-guidelines/issues/476)、[Rust memory model](https://doc.rust-lang.org/reference/memory-model.html)。
