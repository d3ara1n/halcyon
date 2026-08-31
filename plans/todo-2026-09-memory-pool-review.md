# MemoryPool 状态机与能力对象未来审查

> 【未来审查计划】审查对象固定为提交 `4715f3a3c08bad358c23b0188d3b3514e4ffcc79`（`feat(mm): 建立 MemoryPool 额度与能力对象`）。只审该提交形成的 root 额度、Pool 状态机、metadata admission、capability/ABI、Handle 发布事务与用户态 affine owner；后续 funded frame broker、ProcessBindMemory、bootstrap root Handle 交付与 MemoryObject 不混入本结论。

## 对象概要

该提交新增 `os/memory_pool` 四项守恒状态机和 `os/metadata_admission` 固定容量 permit 基座；平台账本以一次性 `RootPoolSeed` 冻结 `user-free + boot-held` 额度，内核铸造并锚定唯一 root Pool。ProcessResources 前移最小 MetadataSponsor，普通 Pool core 同时占用全局与 creator sponsor slot。内核 MemoryPool 以 Prepared → Committing → Active 完成 Derive 的单 Handle 发布，child state 内嵌不可拆分的 parent credit 并强持 parent；最后引用消散且 child 全额 available 后逐级退款。shared/rinlib 同步加入固定宽 Query/Derive ABI、rights 与带 Drop close 的 affine owner；root Handle 仍未交给 init，公共创建闭包保持 dormant。

## 审查重点

1. 从平台分类账本重算 `RootPoolSeed = user-free + boot-held`，确认 seed 只能在完整 FramePool 发布后冻结并消费一次；后续 DTB、bootstrap 与 BootPackage 回投只改变物理库存，不改变 root total，permanent/system 页永不进入额度。
2. 逐项证明 `total = available + reserved + allocated + delegated` 在 reserve、commit、rollback、return、charge split/merge 和深度边界下闭合；检查所有加减、identity 与页数均先验证后修改，任一错误保留原 token 和原状态。
3. 审查线性 token 的实例绑定：crate 内 OwnerKey 必须独立于可重复的 ABI PoolId，reservation/credit 不能跨状态提交；forget 最多泄漏额度，不能复制、退款或扩大供给。
4. 构造 delegation 重放反例复核 child 生命周期：Prepared 必须消费唯一 reservation，Commit 后 delegated credit 与 child state 不可安全拆分，`into_parent_credit` 必须同时要求 fully available 并消费 child 身份；不存在 parent 已退款而 child 仍可派生、charge 或再次退款的路径。
5. 追踪 kernel `PreparedChildOwner`、`PoolInner::{Prepared, Committing, Active}` 与 `Funding::Child` 的所有失败和析构路径：metadata/header/Arc/Handle reservation/uaccess 失败均在 Commit 前退款，Commit 后不分配且无可恢复失败；多引用只在最后一个 core 引用消散时退款，父链级联有界且不形成引用环。
6. 复核 metadata admission 的两层取得和唯一退款：global/local 任一耗尽不得留下另一层占用，permit 必须强持 sponsor 至 core 真实析构；creator Process Dead、Handle 跨进程移动或 typed owner 转 raw 均不能提前退款。重新评估 4096 global sponsors、4096 global Pool cores 与 1024 per-sponsor 的政策边界是否被误当作完整 KernelMemoryBudget。
7. 审查锁序与发布原子性：HandleTable → AddressSpace 检查 → MemoryPool Commit 的实际路径必须符合 Lock Ladder；child/parent Pool 锁只能逐把取得，Drop 退款、Prepared rollback 与级联析构均不得嵌套双 Pool 锁或在锁内分配。
8. 对照 shared syscall/布局复核错误码、固定宽字段、reserved、深度唯一常量与 `u64` 页数；Query 必须要求 READ，Derive 必须要求 CREATE，child rights 只能收窄，GRANT/TRANSIT/DUPLICATE 不改变 core 额度。HandleTable 同 generation pin 冲突必须保持 `ObjectBusy`，generation 不匹配才是 `StaleHandle`。
9. 复核 rinlib affine 边界：`MemoryPool` 普通 Drop 必须关闭 Handle，`close(self)` 失败返回可重试 owner，`into_handle(self)` 只能显式移交且不触发双关；unsafe `from_handle` 的唯一所有权契约必须禁止任何 raw alias 后续使用。
10. 检查 boot/syscall self-test 不向 StartupBlock 泄漏 root Handle，不永久占用 sponsor、Handle 槽、KOID 或 parent 额度；其 kind/role/rights、metadata exhaustion、输出 reservation、Prepared rollback、Commit 与最后引用退款断言必须实际穿过正式 helper，而非平行测试实现。

## 基线证据

- `cd os && cargo test -p memory_pool -p metadata_admission -p handle_table --target aarch64-apple-darwin`
- `cd os && cargo test --release -p memory_pool -p metadata_admission -p handle_table --target aarch64-apple-darwin`
- `cd shared && cargo test --target aarch64-apple-darwin`
- `cd shared && cargo test --release --target aarch64-apple-darwin`
- `just check`
- `just build_user`
- `just acceptance`

提交收口时上述内容均通过：MemoryPool 14 项、metadata admission 6 项、HandleTable 17 项 host debug/release 测试及 shared 13 项 ABI 测试全绿；acceptance 完成 virt debug stress、virt release core 与 sifive_u core。三条启动线分别证明 root 闭包 `256401 = 246867 + 9534`、`256593 = 256392 + 201`、`27569 = 19593 + 7976`，且均出现 `Pool syscall self-test passed: policy, admission, and publication rollback ok`；sifive_u 按明确 `NotSupported` reset 后端结果收割。

## 完成标准

所有发现按严重度给出文件/符号证据、可达输入或并发条件，并明确影响的是额度守恒、metadata admission、capability policy、发布原子还是 affine owner。任何 token 重放、提前退款、Commit 后可恢复失败、permit 生命周期断裂或锁序反转均为阻断项；修复后重跑对应 host debug/release 与完整 acceptance。非阻断承接只进入既有唯一计划，不把 funded broker、ProcessBindMemory 或 KernelMemoryBudget 设计扩张进本审查。
