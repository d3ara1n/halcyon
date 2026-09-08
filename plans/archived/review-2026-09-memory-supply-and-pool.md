# 批次 B-1：平台供给、系统储备、MemoryPool 与 funded frame Review

## 2026-09-08 提交后复核（B-1，通过）

固定对象 `9ee2791d3e18fdb7857fe41c74bacc7bb0c7c774`；WiseHare 独立只读复核，路径/行号对应此提交。

| Finding | 结论与证据 |
|---|---|
| F-1 / DT status | 闭合。`os/dtb/src/memory.rs:234` 共用 node_status，缺省/okay、合法不可用、unknown/malformed 分开；CPU 与 reservation 同边界。`tests/memory.rs:148` 及 cpu tests 覆盖拒绝集合。 |
| F-2 / FramePool arithmetic | 闭合。`os/frame_pool/src/lib.rs:187` 对 arena/metadata 终态 checked preflight，写入前拒绝；`tests/pool.rs:237` 极端几何保持库存不变。 |
| F-3 / MemoryPool Drop | 闭合，采用合法 typed owner leaf close 不可失败方案。`user/rinlib/src/memory_pool.rs:18` 冻结 unsafe 构造契约，`:70` 显式 close/Drop 共用 `ipc/object.rs:14` 的 leaf-close；内核 `task/handle.rs:182` 仅非法表项失败。违约 panic 保留，不当作普通用户错误策略。 |
| F-4 / SystemSupply query | 闭合。`os/memory_supply/src/lib.rs:149` 分开 range snapshot 与 Option ticket owner，`tests/planner.rs:61` 覆盖消费后查询和再次 take。 |

相关 dtb/frame_pool/memory_pool/memory_supply host 测试通过。统筹者补跑全速 stress、release、sifive_u、hetero 通过；nofd 旧锚点失败归 E2-7-02。非法 unsafe 构造和各启动错误未逐项 guest 注入，保留验证限制。四项 finding 闭合，本报告归档。

---

以下保留首审历史记录，旧“当前”及不通过判定不表示修复提交状态。


> 首审已完成；本报告保留目标提交证据与逐条复核条件，不重复首审。当前实施归属以 [`Review 统筹导航`](../todo-2026-09-review-program.md) 为准；下文第 7 节为首审归属记录，当前 F-1/F-2 归 admission、F-3/F-4 归 capability/owner 计划，不按报告逐项另排实施。

## 1. 审查范围与基线

审查对象固定为：

- `198e665`：平台物理供给账本
- `0a944c7`：系统物理储备
- `4715f3a`：MemoryPool 状态机与能力对象
- `48227c8`：funded frame broker

基线按统筹计划为工作树 `2ed7e1e`。目标提交按各自提交内容逐项读取；当前工作树已有 plans 文档改动，但未作为目标提交证据。审查未修改文件、未提交代码。

审查遵循 [`REVIEW.md`](../REVIEW.md)，并参考四份历史审查清单、`notes/ideas/mm.md`、`notes/ideas/object.md` 与目标提交时的实现文档。

## 2. 执行命令与结果

```text
git show --format=fuller --stat 198e665 0a944c7 4715f3a 48227c8
git show --check 198e665 0a944c7 4715f3a 48227c8
cd os && cargo test -p memory_supply -p memory_pool -p metadata_admission -p funded_frame -p frame_pool -p dtb -p stack_layout --target aarch64-apple-darwin
cd os && cargo test -p memory_supply -p memory_pool -p metadata_admission -p funded_frame -p frame_pool -p dtb -p stack_layout --release --target aarch64-apple-darwin
cd shared && cargo test --target aarch64-apple-darwin
cd shared && cargo test --release --target aarch64-apple-darwin
cd os && cargo clippy -p funded_frame --all-targets --target aarch64-apple-darwin -- -D warnings
just check
```

host debug/release 相关 crate 共 81 项测试通过，shared debug/release 各 18 项通过，funded_frame clippy 通过，`just check` 退出码 0。

这些 host cargo 与 `just check` 命令是在当前工作树执行；当前工作树 HEAD 为 `2ed7e1e` 且存在用户预先改动，不能替代目标提交隔离复放。未运行 `just acceptance`、`THROTTLE=100 just acceptance`，也未在目标提交隔离 checkout 下重放 QEMU virt/release/sifive_u；不能据本次结果声称平台启动与完整验收通过。

## 3. 四个提交逐项结论

### `198e665`

DTB parser、FDT reservation block、静态 `/reserved-memory/reg`、no-map 子集、页对齐/溢出/重叠检查、EagerMapper 最大叶、transition 4KiB 叶撤销和 direct-map 静态表预算整体方向正确。host 测试覆盖多 tuple、disabled、no-map、动态/reusable fail-closed、重叠与容量错误。

发现 F-1：`status = "ok"` 被当作可用，和 CPU 严格要求 `"okay"` 的规则不一致，存在平台 admission 误收风险。

### `0a944c7`

`memory_supply::Planner` 先规范化 managed，再裁剪 permanent/boot-held、按 metadata → heap → recovery 放置、最后求 user-free；system/user 类型隔离、heap ticket 单向消费、FramePool 不接收 system 页、system 与总分类守恒断言均成立。Talc 的 `HEAP → SYSTEM_SUPPLY` O(1) 供血路径未进入 FramePool、未清零/扫描；`recovery = 0` 与当前没有 Commit 后物理页消费者的实现闭包一致。

F-2 的 FramePool checked arithmetic 问题适用于该提交的后续接线，但不否定 planner 的区间分类机制。

### `4715f3a`

纯逻辑 PoolState 的 `total = available + reserved + allocated + delegated`、checked 算术、独立 OwnerKey、charge/delegation 分型、quiescent child 才能取 parent credit、深度上限与 wrong-owner 错误均成立。内核 Prepared → Committing → Active 发布、parent 强引用、逐锁退款不形成父子锁嵌套；Query READ、Derive CREATE、rights 子集与固定 64-byte ABI 均正确。

发现 F-3：rinlib typed owner 的 Drop 对 close 直接 `expect`，缺少面向用户态析构的可证明不可失败边界。

### `48227c8`

funded_frame generic broker 按 `validate → quota.reserve → bounded claim → clear 全部 extent → quota.commit` 执行；中途失败时 claim 先析构、reservation 后回滚；字段声明顺序保证先归还物理 owner、再退额度。内核 `PreparedMemoryCharge/MemoryCharge` 与 `ClaimedUserExtent` 接线满足 Pool/FramePool 双账本；claim 后锁外清零，成功才发布 Funded；`MAX_FUNDED_EXTENTS = 64` 受 request limits 约束。

host debug/release broker 失败原子、跨 extent、wrong-owner 和析构顺序测试均通过。另发现 F-2（FramePool checked arithmetic）及 F-4（公共 SystemSupply range 查询在消费后 panic）。raw `alloc_user_*` 与 `adopt_*` 是计划已登记的过渡/内部 unsafe seam，本批不判作 broker 机制本身失败，但列入验证缺口与后续清理。

## 4. Findings

### F-1 / P1：未知 DT status 值被静默当作可用，平台 admission 非 fail-closed

位置：目标提交 `198e665`，`os/dtb/src/memory.rs:204-205`；同一函数由 memory 与 reserved-memory child 共用。

可达前提：固件/DTB 在 `/memory` 或 `/reserved-memory` 节点给出 `status = "ok"` 或任意非标准值，`is_available` 返回 true。

直接证据：实现为 `node.prop("status").is_none() || matches!(node.prop_str("status"), Some("ok") | Some("okay"))`；同提交 `os/kernel/src/board.rs:256` 对 CPU 明确 `status != "okay"` 即拒绝该 CPU。相同 DTB 中 status 语义不一致。

违反契约/不变量：`notes/ideas/mm.md` 要求平台描述矛盾和未知语义不能降级为普通供给并应 fail closed；Devicetree operational value 是 `okay`，未知 status 不应成为 managed RAM 或 reservation 输入。

影响：平台可能把未确认可用的 memory node 加入 managed RAM，或把未知状态的 reserved child 加入永久排除。后续守恒式只能证明错误输入被一致计算，不能证明输入合法。

建议修复方向：统一可用判定为缺省或严格 `"okay"`；未知值、错误 UTF-8、非标准状态按平台 admission error 拒绝；为 memory/reserved child 增加 unknown status 测试，并与 CPU 规则共用同一 helper。

### F-2 / P2：FramePool metadata 索引算术未 checked

位置：目标提交 `48227c8`（源自 `198e665`，后续提交未改动），`os/frame_pool/src/lib.rs:178-190`。

可达前提：平台给出接近 `usize::MAX` 的 managed 帧区间，或提供极大 metadata slice；`metadata_bytes(frames)` 自身未溢出，但 `self.metadata_used + metadata_need` 或 `self.metadata_used + arena.metadata_len()` 溢出。

直接证据：相关加法未使用 `checked_add`，失败后可能在 release 中得到环绕索引/切片边界 panic，而不是 `AddRegionError::MetadataExhausted` 的 preflight 失败。

违反契约/不变量：固定容量耗尽应在修改前 fail closed，所有地址/字节算术必须 checked；`add_managed_region` 的 metadata failure-before-mutation 不能依赖非溢出输入。

影响：恶意或损坏 DTB 可触发不可诊断的 panic；当前真实平台地址不足以复现，因此属于硬化项而非已观测运行时故障。

建议修复方向：将 metadata_used + need、metadata_end、arena_count + arena_need 等全部改为 checked 算术，在任何写入前返回 `MetadataExhausted`/`ArenaLimit`；补 `usize` 边界 host case 和 release 测试。

### F-3 / P2：rinlib MemoryPool Drop 将用户态 close 失败升级为 panic

位置：目标提交 `4715f3a`，`user/rinlib/src/memory_pool.rs:80-83`。

可达前提：用户创建 `MemoryPool` typed owner 后自然离开作用域，`close` 返回错误，例如有效性前提被破坏或未来 close 路径出现阶段/进程退出错误。

直接证据：显式 `close(self)` 在前面保留 `(Self, SystemCallError)` 供重试；Drop 对同一 syscall 使用 `.expect("MemoryPool owner failed...")`，没有可证明的 infallible kernel primitive 或 SAFETY 约束说明。

违反契约/不变量：用户可达错误应返回而不是 panic；允许 close 返回错误的 API 与生命周期必经 Drop 路径的错误边界不闭合。

影响：错误 Handle、终止竞态或未来 close 扩展可能使用户态 Drop abort/panic，掩盖资源错误并破坏正常收束语义。

建议修复方向：优先提供对合法 MemoryPool role 的 infallible close primitive，并在 typed owner 构造处建立唯一有效 Handle invariant；否则 Drop 不应调用可能失败 syscall。若保留 expect，必须把有效性与 close 不失败证明写成 API 契约，并只以内部断言守护。

### F-4 / P2：SystemSupply 几何查询在 ticket 消费后 panic

位置：目标提交 `0a944c7`，后续 `48227c8` 未改动，`os/memory_supply/src/lib.rs:129-144`；recovery 对应 `152-166`。

可达前提：调用者先执行 `take_heap_chunk()`，随后再次调用 `heap_ranges()`；recovery ticket 具有相同问题。

直接证据：`take_heap_chunk` 消费 slot；`heap_ranges` 随后对整个 `heap[..heap_count]` 执行 `ticket.as_ref().expect("heap ticket missing")`。合法单向消费动作使同一只读 accessor 进入 panic 状态。

违反契约/不变量：SystemSupply ticket 是 affine、用途隔离、单向消费对象；消费后应能观察剩余/已消费状态，不能让公共只读 accessor 对合法消费序列 panic。

影响：未来新增系统供给消费者、启动诊断或 recovery allocator 若在消费后复用 range accessor，会触发内核 panic；这是 API 生命周期残留，不是账本数字错误。

建议修复方向：几何查询只返回仍未消费 ticket 的 ranges，或单独保存不可变几何快照；`remaining_*` 与 query 对任意合法消费序列都不得 panic；补 take 后 query 测试。

## 5. 已证实不变量

- DTB/FDT：`Fdt::new` 做 totalsize、块边界、reservation block 终止与重叠检查；memory parser 对 reg tuple、页边界、算术溢出、内存重叠和 dynamic/reusable 未实现语义有显式错误；静态 no-map 同时进入 reservation 与 no-map 子集。
- 平台/系统分类：Planner 的 managed、permanent、boot-held、system、user-free 区间运算和失败后可重规划通过 host debug/release；heap ticket 与 recovery ticket 类型不可互转；FramePool 只发布 user-free，system metadata/heap 不进入用户库存；Talc Source 只做 O(1) ticket claim。
- FramePool：canonical arena 分解、alloc/alloc_at、split/coalesce、reservation 发布、重复归还拒绝、free frame 数量守恒和失败原子性通过 16 项 debug/release host tests。
- MemoryPool：纯逻辑 `total = available + reserved + allocated + delegated` 在 reserve/rollback/commit/return、charge split/merge、child delegation、depth、wrong-owner、并发 reserve 下通过 14 项 debug/release；ParentCredit 只有 child fully-available 且 child state 被消费时可取回；PoolId 与 crate-local OwnerKey 分离。
- Capability/ABI：MemoryPoolSnapshot 固定 64 bytes/alignment 8，Query 要求 READ、Derive 要求 CREATE、child rights 仅可收窄；HandleTable generation/pin/reservation 逻辑不在本批专门重跑。
- funded broker：request limit 在 quota 前检查；quota reservation 先于物理 claim；claim 全部取得后才 clear；clear 完成后才 commit；失败时 claims 先 Drop 回库存、reservation 后 Drop 回 Pool；Funded 字段顺序保证物理 owner 早于额度 owner 析构。
- 锁序：目标提交主路径是 HandleTable → AddressSpace → MemoryPool（递增 rank），HEAP → SYSTEM_SUPPLY → POOL；MemoryPool child/parent 退款逐把锁取得，未发现同时持有两把 Pool 锁的证据。Commit 后 broker generic path 无普通可恢复分配。

## 6. 验证缺口与过渡项

- 未运行目标提交 QEMU acceptance（virt debug stress、virt release core、sifive_u core），未在目标提交隔离 checkout 重放；不能据本次结果声称平台启动、ELF audit 或竞态矩阵通过。
- 目标提交仍保留计划登记的 transitional raw adapter：`os/kernel/src/frame.rs:611-620` 的 `alloc_user_order/alloc_user_largest`，以及 crate-private unsafe `adopt_table_frame/adopt_reserved`。这些不是本批 generic broker 的新绕过入口，但仍未完成最终 funded owner 收口。
- `FundedFrames` 尚未向公共 backing split/merge、retire 接线；这符合 `48227c8` 的计划边界。
- `RECOVERY_TICKET_LIMIT = 0` 仅由当前实现没有 Commit 后物理页消费者推出；新增 completion/drain/remote 物理页消费者前必须扩充独立 ticket 预算并补 exhaustion-before-Commit 验证。

## 7. 后续归属

- F-1：平台 admission 规则进入 `notes/ideas/mm.md`，实现事实和测试要求进入 `notes/impls/mm.md`；修复与复核由本报告承接。
- F-2：进入 `notes/impls/mm.md`/FramePool 平台 ledger 审查收口；修复与复核由本报告承接，不新增重复计划。
- F-3：方向错误边界进入 `notes/ideas/object.md`，用户态实现事实进入相关 MemoryPool impls；修复与复核由本报告承接。
- F-4：实现事实进入 `notes/impls/mm.md` 的 SystemSupply ticket 生命周期；修复与复核由本报告承接。
- transitional raw adapters：更新既有 funded-frame/page-table lifecycle 计划状态，不新增重复 TODO。

## 8. 最终判定

B-1 **不通过**：F-1 是平台 admission 的 P1 阻断项，需修复并补 unknown-status/相关边界测试；F-2、F-3、F-4 为 P2 非阻断但应在结构收口前处理。MemoryPool 四项账本、capability 发布与 funded broker 顺序/双账本在已运行的 host debug/release 证据下通过；但 host tests 全绿不替代平台启动与错误边界 Review。
