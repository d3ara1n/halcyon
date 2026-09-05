# 启动与平台 Admission Fail-Closed 收口计划

> 当前代码审查后形成的统一修复批次。先完成方案内的整体收口，再实施；不把同一 admission 原则拆成互不协调的逐点补丁。

## 目标

平台、启动与镜像边界对未知、矛盾、重复、溢出和未实现语义统一 fail closed：

```text
raw input
  → normalize / checked validate
  → canonical admission record
  → publish immutable runtime identity
```

任何 admission 失败都必须发生在对应资源发布、hart 启动、PTE 安装或用户进程可运行之前；不能通过跳过字段、降级到默认值或延迟到运行期 fault 来“接受”未实现输入。

## 当前问题簇

### 平台 DT admission

- `os/dtb/src/memory.rs::is_available` 同时接受 `"ok"` 与 `"okay"`，未知/错误 status 可能被静默视为可用；CPU parser 仅接受 `"okay"`，规则不一致。
- `os/kernel/src/board.rs::parse_cpu` 只拒绝 `d && !f`，未拒绝 `q && !d`；病态 capability 仍可进入域构造。
- memory/reserved-memory 与 CPU 的 status、能力约束没有共用 fail-closed helper。

### FramePool / 物理算术

- `os/frame_pool/src/lib.rs` 的 `frames() * 2`、`arena_count + arena_need`、`metadata_used + metadata_need`、`metadata_used + arena.metadata_len()` 仍使用未 checked 加法。
- 这些算术必须在任何 metadata/arena 写入前失败，不得依赖现实平台数值“不会太大”。

### Hart identity / RuntimeGate

- `HartRegistry::admit` 不拒绝重复 raw hartid，且 slot 升序契约依赖 FDT child 顺序但未排序。
- HSM `hart_start` 错误仍直接 `sbi::require`，未先发布 `RuntimeGate::Failed`。
- `ipi_slots` 仍把业务唤醒/终止路径的 SBI 失败升级为 panic；应与 Remote Call 的 pending/failed-mask 语义统一。
- raw HartId、内部 HartSlot、active mask、SBI `(mask, base)` 的身份转换必须在 admission 后形成不可变 canonical record。

### ELF admission

- `os/elf/src/lib.rs` 对非 `PT_LOAD` 直接跳过，未拒绝 `PT_INTERP` 或未知 program-header 类型/flags。
- runtime parser 与 `tools/audit-user-elf.py` 的规则不一致。
- `PF_RWX` 之外的未知 flags、zero-permission `PT_LOAD`、`PT_INTERP` 尚未形成统一拒绝策略。
- entry 仍按 `vaddr .. vaddr + memsz` 判断，允许落入 BSS；应按 executable segment 的实际 file-byte interval 判断。
- bootstrap、libprocess planner、静态 audit 必须共用同一 ELF admission 语义，而非各自复制判断。

### reservation / generation 边界

- reservation token、Remote Call token、AddressSpace epoch 等 generation 只在局部检查零值或溢出，缺少统一身份域与耗尽政策。
- 该项与事务状态机计划存在交叉，但 admission 批次只负责输入/启动身份发布；MemoryChange 内部阶段 token 仍由 `todo-2026-09-memory-transaction-state-machine.md` 负责。

## 最终形态

### Canonical platform admission

新增或重构零分配 admission helper，统一：

1. status：缺省或严格 `"okay"` 才可用，其他值显式错误；
2. CPU capability：`q ⇒ d ⇒ f`、基线扩展、MMU 与 hart identity 在同一入口校验；
3. raw hart：排序、去重、容量与 SBI 可表达范围一次确定；
4. memory/reservation：checked normalize、排序、合并/重叠拒绝、容量和 direct-map 区间闭合；
5. admission 输出冻结为 immutable `PlatformAdmission`/`HartAdmission` 记录，后续 runtime 不再重新解释原始 DT child 顺序。

### Canonical ELF admission

`libelf` 应成为运行时与工具规则的唯一语义源，至少返回：

- validated PT_LOAD 列表；
- executable file-byte intervals；
- page-level protection union；
- 明确的 unsupported header/flags 错误；
- entry 是否落在实际文件字节的 executable interval。

用户态 audit 只做格式适配和错误呈现，不重新实现规则。bootstrap 与 libprocess 直接消费相同的 validated result。

### RuntimeGate / IPI failure

- HSM 启动前先冻结 expected 集合；任一 `hart_start` 失败先 Release 发布 Failed，再进入平台 fatal/park；
- 普通 IPI 发送只返回/记录失败，不在已发布业务 Pending 后伪造完成；终止/唤醒路径使用固定失败政策；
- Gate 的 Preparing/Ready/Failed 必须对所有 admitted hart 可观察，失败状态不允许永久停留 Preparing。

## 自然实施顺序

1. 先冻结平台 admission 错误分类与 raw-id/slot canonical 记录；
2. 收口 DT status、CPU capability、duplicate/sort；
3. 统一 FramePool/平台 reservation 的 checked arithmetic；
4. 收口 RuntimeGate/HSM/IPI 失败传播；
5. 抽出 libelf validated image/entry/flags 语义，迁移 audit、libprocess、bootstrap；
6. 统一 token/epoch 的身份域与耗尽策略（与事务计划交叉处只保留一个 owner）；
7. 补 malformed DTB、duplicate/unsorted hart、HSM failure、frame arithmetic、PT_INTERP/unknown flags/entry-in-BSS 的 host 与启动负向测试；
8. 运行 `just check`、host debug/release、virt/release/hetero/nofd/sifive_u/acceptance，确认失败态广播和无半发布资源。

## 完成标准

- 未知 DT status、q-only/d-only capability、重复/乱序 raw hart、溢出算术均在资源发布前返回明确错误；
- slot 顺序和 raw HartId→HartSlot 映射由 canonical admission 记录决定，不依赖 FDT 遍历顺序；
- HSM/IPI 失败不会留下 Preparing 永久态，也不会把已发布 Pending 请求伪造成完成；
- runtime ELF parser、audit、libprocess、bootstrap 对 PT_INTERP、未知 flags、zero-permission segment、entry-in-BSS 给出一致结果；
- entry、PTE 权限、W^X 与 page-level union 的验证在启动边界完成；
- token/epoch 耗尽策略统一为不可回绕/明确拒绝或永久退休，跨实例误用不可静默成功；
- 相关负向测试和完整平台路线通过；
- 完成后删除重复 parser/helper、旧注释和不再拥有真值的 fallback。

## 依赖与边界

- 事务阶段 token、Commit 后 owner 与 Bound rollback 由 `todo-2026-09-memory-transaction-state-machine.md` 负责；本计划不得为事务重新引入第二套状态机。
- 监督 authority 与无限等待由后续监督政策计划负责；Admission 只保证启动失败可观察，不决定服务重试/重启。
- 当前不因验证缺口单独添加兼容层；若某项需要结构性新类型，必须在本批一次迁移调用点。
