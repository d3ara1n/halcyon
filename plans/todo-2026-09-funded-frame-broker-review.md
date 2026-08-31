# 资金化帧取得事务未来审查

> 【未来审查计划】审查对象固定为提交 `48227c87e9e3488fca259073ddafec75dbf79d60`（`feat(mm): 建立资金化帧取得事务`）。只审该提交形成的 generic broker、MemoryPool charge 接线、user inventory claim/清零/退款、固定 extent storage 与栈 guard 契约；ProcessBindMemory、boot-held primordial adopt、页表/匿名 backing 全面资金化及公共 MemoryObject 不混入本结论。

## 对象概要

该提交新增 `os/funded_frame`：一个 `no_std`、禁止 unsafe、零堆分配的事务编排 crate，通过 `QuotaSource/QuotaReservation` 与 `PhysicalSource/PhysicalClaim` 两组仿射端口执行 `reserve quota → bounded claim → clear → commit`。内核 `task/memory_pool.rs` 以 `PreparedMemoryCharge/MemoryCharge` 把 reservation 与 allocated credit 接回具体 Pool core，`frame.rs` 以 `ClaimedUserExtent` 接回唯一 user FramePool，并用私有 `FundedFrames` 把 extents 与 charge 保持到同一析构边界。普通 user-funded claim 已建立；保留内容的 boot-held adopt 延至切片 5 随不可伪造启动 owner 接入，backing split/retire 延至切片 6 随真实 AddressSpace owner 接入，system tickets 继续由 `SystemSupply` 分型。

固定结构最多容纳 64 个 extent。debug 内核最大单帧随之成为 `0x2390`；链接期纯虚拟 `STACK_GUARD` 扩至 `0x3000`，ELF audit 上限扩至保留 `0x800` 余量的 `0x2800`，对应 stack_layout 几何和 host 测试同步更新。现有页表、匿名 backing、Tunnel 与库存自检仍走登记过的 transitional raw adapter，后续按所属切片迁移。

## 审查重点

1. 逐分支验证 broker 的顺序与失败原子性：请求边界必须在 quota reservation 前拒绝；quota 成功后，物理中途失败、extent 上限或非法 claim 必须先析构全部 physical owners，再回滚 reservation；清零只在全部 extent 取得后发生，commit 只在全部清零后发生。
2. 复核 `Funded` 的封装与自然析构：claims 字段必须先于 credit 析构，外部只能取得共享只读观察，不能任意拆包、提前退款、改写页数或绕过清零；`#[must_use]` 与私有字段不能被安全 API 规避。
3. 审查 trait 信任边界：`QuotaSource::reserve(pages)` 必须返回等额且可回滚的 reservation，`QuotaReservation::commit` 必须不可失败，`PhysicalSource::claim_largest` 返回值必须满足 `1..=remaining`，`PhysicalClaim::clear` 必须锁外、完整且不可失败。确认 generic 层对零页和超额 claim fail closed，而内核 adapters 不依赖未验证外部输入。
4. 追踪 `PreparedMemoryCharge` 的 reserve/commit/Drop 与 `MemoryCharge::Drop`：token 只能提交或退款一次，Pool core 由强引用保活，`reserved → allocated → available` 全程保持四项方程；任何 owner-key、页数或状态错配必须作为内核不变量失败，不能静默泄漏或扩容。
5. 追踪 `ClaimedUserExtent` 的 claim/clear/Drop：POOL 锁只保护 `alloc_largest/dealloc`，清零必须位于锁外；extent geometry 不能复制成第二份 owner，错误返回、正常 `FundedFrames` 析构与 panic unwind 假设下均不得双重归还。
6. 复核跨账本事务窗口：静止点满足 Pool allocated 与 funded extents 对应，claim/return 窗口由唯一 broker owner 覆盖；先归物理、后退额度的逐锁结算不能形成可被其它安全路径解释成双重供给的状态，也不得嵌套 `MEMORY_POOL`、`POOL`、`ADDRESS_SPACE` 或 heap 锁。
7. 审查两类独立工作边界：调用方提供的 `max_pages/max_extents` 必须在进入页数线性清零前验证，const extent capacity 必须阻止数组越界；不能把 64-extents 结构容量误写成所有对象共享的延迟政策，未来每类消费者仍需给出自己的页数和 extent 上限。
8. 重做栈契约证明：`Funded<..., 64>` 在 debug 代码生成中的最大帧、`DEFAULT_MAX_FRAME=0x2800`、`STACK_GUARD=0x3000` 与 stack window stride 必须使用同一真值链；确认 virt `0x230000` 总窗口跨度、sifive_u `0x80000` 跨度、页表静态容量与形式栈容量仍闭合，单帧不能越过 guard 落入相邻映射。
9. 检查启动自检确实穿过正式 MemoryPool/FramePool adapters，成功路径同时改变并恢复 `available/allocated/free_frames`，extent-limit 路径在清零前恢复双账本；自检不得依赖特定 boot hart、物理地址或超过平台 admission 的连续块。
10. 搜索全部运行期帧取得路径，确认本提交没有新增 raw user path；页表、匿名 backing、Tunnel 与 inventory selftest 的 transitional adapters 与后续切片归属保持唯一登记。通用 broker 不得出现可把普通 claim 当 boot-held adopt、或把 system ticket 当 user-funded extent 的安全入口。
11. 复核切片边界：ProcessBindMemory 负责第一次实际消费 funded root/table backing；切片 5 负责带不可伪造 boot-held token 的 primordial adopt；切片 6 在真实 backing owner 内同步实现 extent/charge split、merge 与锁外 retire。审查不得用当前缺少这些下游能力否定 acquisition broker 的闭包，也不得提前制造脱离 owner 生命周期的通用转换 API。

## 基线证据

- `cd os && cargo test -p funded_frame -p memory_pool -p frame_pool -p stack_layout --target aarch64-apple-darwin`
- `cd os && cargo test -p funded_frame -p memory_pool -p frame_pool -p stack_layout --release --target aarch64-apple-darwin`
- `cd os && cargo clippy -p funded_frame --all-targets --target aarch64-apple-darwin -- -D warnings`
- `just check`
- `just acceptance`

提交收口时上述相关验证均通过：funded_frame 6 项、MemoryPool 14 项、FramePool 16 项、stack_layout 7 项 host debug/release 测试全绿，funded_frame clippy 无警告。acceptance 完成 virt debug stress、virt release core 与 sifive_u debug core；三条启动线均出现 `funded frame self-test passed: commit, rollback, and dual-ledger refund ok`。最终 ELF audit 的最大帧为 virt/sifive_u debug `0x2390`、virt release `0x1260`；virt 正常 reset，sifive_u 按明确 `NotSupported` reset 后端结果收割。

## 完成标准

所有发现按严重度给出文件/符号证据、可达失败点或锁上下文，并分别说明对 Pool 方程、物理供给方程、清零发布边界、栈 guard 或类型隔离的影响。任何提前退款、未清零发布、双重归还、跨账本 owner 丢失、降秩取锁、extent 数组越界或单帧越过 guard 均为阻断项；修复后重跑相关 host debug/release、ELF audit 与完整 acceptance。非阻断承接只回到主计划中已有的切片 4–6，不复制新的 TODO 真值点。
