# 启动与平台 Admission Fail-Closed 收口计划

> 当前 Review findings 的输入契约归属计划，导航见 [`Review 统筹`](todo-2026-09-review-program.md)。平台 admission 与 ELF admission 是独立纵向子单元，共享 fail-closed 原则但不共用无意义的总状态机。具体接受/忽略/拒绝集合必须先依 `references/CONTRACTS.md` 核验；下文候选规则与历史 finding 描述不替代规范裁定。

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

### 运行期身份的交接

reservation token、Remote Call token 与 AddressSpace epoch 的身份域/耗尽策略由 [`identity-generation-boundaries`](todo-2026-09-identity-generation-boundaries.md) 唯一拥有。本计划只发布 canonical 平台/镜像事实，不另排一轮 token 重构；内存与启动事务直接消费其所需的凭据前置。

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

## 纵向子单元与依赖

1. **规范与契约冻结**：从固定规范确认 status、CPU、ELF header/flags 的接受、合法忽略与明确拒绝集合，形成 canonical 数据与错误接口。不得把已知合法但不可用的节点一概视为 malformed，也不机械拒绝所有非 PT_LOAD header。
2. **ELF admission**：纯逻辑 validated image、audit、libprocess 与 Bootstrap 入口一起迁移并补负向测试。它是构造与启动纵向单元的直接前置，可独立于无关 DT 改动先完成；旧 parser 解释分支与重复工具规则同单元删除。
3. **平台 admission**：DT/区间 checked normalize、FramePool 接入、raw-id/slot、RuntimeGate/HSM/IPI 的生产者到发布者整体收口，补 malformed、duplicate/unsorted、边界算术与平台失败广播测试。

每个子单元自带 `just check`、host debug/release 与相关 QEMU 负向证据；组合阶段运行 virt/release/hetero/nofd/sifive_u/acceptance。身份耗尽的实现不在本计划重复安排，按 identity 计划与事务的直接依赖完成。

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
