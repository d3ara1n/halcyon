# 页表资金化事务重构未来审查

> 【未来审查计划】审查对象固定为提交 `cfad6cf4a8b86b250ea515a34e90e9492deff173`（`feat(mm): 完成页表资金化事务重构`）。只审该提交形成的 root owner 归属、Running/Tunnel/Building 映射跨锁 funded table transaction、`image_end` 提交语义、transaction gate 与失败回滚；匿名 backing 全面资金化、公共 MemoryObject、多页 Tunnel 协议和 Runnel v2 不混入本结论。

## 对象概要

该提交把绑定 Pool 提供的 funded table owner 接入 root、Running anonymous mapping、Tunnel object mapping/unmapping 以及 Building `ProcessMap` 的主体事务路径。`PoolBinding` 不再与 `TableTree` 重复持有 root owner；Bind 在 AddressSpace 发布前锁外取得 funded root，树持有唯一 root owner。映射操作统一拆为锁内 validate/preflight 与 ledger reservation、锁外 Pool funding、重新加锁后的代次复检和 PTE prepare/commit。

Building mapping plan 额外携带 `building_image_end`。只有映射成功完成 commit 且不进入固定主栈窗口时，才推进地址空间的 `image_end`；固定栈映射不改变该游标。用户态 loader 的 `image_top` 仍是独立的 ELF 规划结果，不能与内核 `image_end` 或 Running heap top 混淆。

`table_transaction_active` 跨越 funding 边界保持，第二个事务必须返回 Busy 而不能触发重复安装断言。funding、preflight、complete、prepared allocation 和 commit 前的容器容量失败都必须恢复 ledger、backing、table owner 与 transaction gate；object unmap 的 prepared allocation 失败也纳入显式 rollback。

## 审查重点

1. **Root owner 唯一归属**：确认 `FundedRootFrame` 只由 `TableTree` 持有，`PoolBinding` 只保留 Pool authority、资源来源和 metadata sponsor；Bind 失败、重复绑定、AddressSpace drain、Process drop 与异常构造不得双重归还或泄漏 root frame/charge。
2. **Lock Ladder**：逐入口核对 `MEMORY_POOL < ADDRESS_SPACE`。任何 Pool quota reserve、FramePool claim、清零或 funded owner 构造都不得发生在 AddressSpace 锁内；重新加锁后的 complete 只能复检并消费既有 owner。
3. **Plan/complete 代次闭包**：`TranslationPreflight`、ledger reservation、funded owner 数量和树 generation 必须保持同一事务意图。stale preflight、结构变化、owner 数量不符和 prepare 失败必须在 PTE 发布前完整回滚。
4. **Transaction gate 并发性**：Running、Building、Tunnel Map/Unmap/Protect 的 plan 入口都必须拒绝已有事务；跨锁 funding 期间第二个操作只能得到 Busy，不能 panic、覆盖 gate 或遗失原事务的 backing/permit/ledger reservation。
5. **Commit 后零失败**：Prepared mapping 的 commit、publish、`image_end` 更新和空 retire 处理不得再执行可恢复分配；unused/retired table owner 必须由调用者在 AddressSpace 锁外析构。
6. **Building `image_end` 语义**：确认 `image_end` 只由已成功提交且位于 image 区域的 Building mapping 推进，固定主栈 `[USER_TOP - STACK_SIZE, USER_TOP)` 不得推进它；映像为空、映射失败、重复映射和 Attach/Start 失败时边界值应保持正确。
7. **`image_top` 与 `image_end` 分工**：核对 `libprocess` 的 ELF page plan、StartupBlock 放置和内核 `ProcessAttach` 校验，确保用户态规划结果不被误当成内核已发布状态，且 `image_end == 0` 的非法 Building 状态仍 fail closed。
8. **Building/ELF/bootstrap 过渡边界**：盘点 `supply_raw_table_frames`、`TableFrameToken::Raw` 与旧 `prepare_install` 的剩余调用点；确认 transitional adapter 只有主计划登记的调用者，不能重新进入新的 Running/Tunnel funded seam，也不能被写成最终 6D 状态。
9. **Object unmap 失败原子性**：`PreparedObjectUnmap::allocate`、页表 prepare、Retire preparation 和 detached close 的每个失败点都必须清除 gate、rollback ledger，并使 pending close 可重试；不可把可恢复内存失败升级为内核 panic。
10. **错误分类保持**：funded root/table funding 的 quota 不足、物理库存耗尽、metadata/结构上限和对象忙状态必须分别映射为 `QuotaExceeded`、`OutOfMemory`、`ReachLimit`、`ObjectBusy` 等既有错误；不能用统一 `NoFrame`/`OutOfMemory` 掩盖 Pool 状态。
11. **资源基线**：成功重复 Map/Unmap、mega split、表剪枝、Remote ack、AddressSpace drain 和 process teardown 后，除 live root 外 Pool allocated/available、FramePool free inventory 与 metadata sponsor 必须恢复到对应基线。
12. **验证闭包**：补充 Building ProcessMap → ProcessAttach 回归、funding 中途失败、prepared allocation 失败、并发 gate、固定栈不推进 `image_end`、root/table owner 基线和 detached close 重试场景；覆盖 host debug/release、`just check`、`just virt`、`just virt-stress`、`just virt-release` 与适用平台验收。
13. **实现文档一致性**：对照 `notes/impls/mm.md`、主计划 6D、`plans/COMPASS.md` 与代码，确认 root owner、Building image_end、transitional raw 边界和错误分类没有互相矛盾。

## 当前基线证据

- `cargo check -p erhino_kernel --target riscv64gc-unknown-none-elf`
- `just check`
- `cd os && cargo test -p page_table -p funded_frame --target aarch64-apple-darwin`
- `just virt`
- `git diff --check`

当前基线中 kernel check、`just check`、page_table/funded_frame host 测试（30 项）和 `just virt` 均通过。目标架构仍有 transitional helper 的 dead-code 警告；`cargo clippy -D warnings` 受既有 `stack_layout` lint 影响，不能作为本提交的完整通过证据。

## 完成标准

所有发现按严重度给出文件/符号证据、可达交错和 owner/ledger 方程影响。Root 双重归还、Pool/FramePool 账本不守恒、AddressSpace 锁内取得 Pool、stale Prepared 发布、跨锁 gate 遗失、Commit 后可恢复失败、`image_end` 越过固定栈边界、错误分类丢失或 detached close 不可重试均为阻断项。修复后重跑相关 host debug/release、`just check` 与至少 `just virt`；6D 未实现能力回到主计划唯一真值点，不在本审查文档复制新的实现计划。
