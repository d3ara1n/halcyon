# 启动与平台 Admission Fail-Closed 收口计划

> 状态：平台与 ELF admission 两个纵向子单元均已完成，本实施计划现已归档；提交后复核由 [`Review program`](todo-2026-09-review-program.md) 统筹。二者共享 fail-closed 原则但不共用无意义的总状态机，接受/忽略/拒绝集合以 `references/CONTRACTS.md` 和本计划引用的固定规范为准。

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

### 平台 DT admission（已完成）

`dtb::node_status` 统一严格 DTSpec 规则：缺省/`okay` 可用，`disabled`/`reserved`/`fail[-sss]` 合法但不准入，`ok`、未知值和非单一 NUL 字符串拒绝。`dtb::cpu::parse` 一次冻结 timebase、现代 ISA 扩展、`q => d => f`、MMU、频率和 raw hartid，在固定容量内排序去重；`dtb::memory::parse` 对 memory/reserved-memory 使用同一 status 边界。内核 `BoardInfo` 只复制 canonical 结果，不再按 DT child 顺序解释 CPU。

### FramePool / 物理算术（已完成）

managed range 的 frame/arena/metadata 目标终态均以 checked arithmetic 在写循环前预计算；arena range/metadata 几何、free-frame 加减和 canonical cursor 也不回绕。极端 `0..usize::MAX` 几何测试确认 `MetadataExhausted` 返回后 arena、metadata 与 free counter 均未发布。

### Hart identity / RuntimeGate（已完成）

raw hartid 由 CPU admission 排序去重，`HartRegistry::admit` 再断言严格升序后铸造稠密 slot。`os/runtime_gate` 固定 `Preparing -> Ready | Failed` 单向状态机；HSM 错误、formal CSR 拒绝和 Online 超时先广播 Failed，boot/secondary 均可观察。普通 IPI 统一返回失败 slot mask，Ready/termination/Remote/deferred-work 真值保持已发布而不伪造完成或 panic；SBI 边界逐项把 slot 转回 raw `(mask=1, base=hartid)`。

### ELF admission（已完成）

固定规范与项目策略见 [`静态 ELF admission 规范取证`](../ref-2026-09-elf-admission-research.md)。`os/elf::validate` 现为唯一构造入口，一次冻结 ELF64/ET_EXEC/RISC-V 头部、program-header 分类、文件与内存几何、页级权限并集、entry、映像顶和 ISA requirement；结果字段私有，调用者不能伪造 validated image。

Bootstrap、libprocess 与 `tools/audit-user-elf.py` 已全部消费该结果；Python 工具只启动同 crate 的 host audit binary，不再复制 parser。`PT_NULL`/`PT_NOTE` 等合法可忽略项与必须解释的 `PT_INTERP`/`PT_DYNAMIC`/`PT_TLS` 已区分；未知类型/flags、zero-permission、W-only、页级 W+X、LOAD 失序/重叠、entry-in-BSS 和容量超限均在创建进程前拒绝。X-only 按 gABI 允许的权限上界明确映为 R+X。

### 运行期身份的交接

reservation token、Remote Call token 与 AddressSpace epoch 的身份域/耗尽策略由 [`identity-generation-boundaries`](todo-2026-09-identity-generation-boundaries.md) 唯一拥有。本计划只发布 canonical 平台/镜像事实，不另排一轮 token 重构；内存与启动事务直接消费其所需的凭据前置。

## 最终形态

### Canonical platform admission（已实现）

零分配 admission helper 已统一：

1. status：缺省或严格 `"okay"` 才可用，其他值显式错误；
2. CPU capability：`q ⇒ d ⇒ f`、基线扩展、MMU 与 hart identity 在同一入口校验；
3. raw hart：排序、去重、容量与 SBI 可表达范围一次确定；
4. memory/reservation：checked normalize、排序、合并/重叠拒绝、容量和 direct-map 区间闭合；
5. admission 输出冻结为 immutable `PlatformAdmission`/`HartAdmission` 记录，后续 runtime 不再重新解释原始 DT child 顺序。

### Canonical ELF admission（已实现）

`os/elf` 返回私有构造的 `Elf`：validated PT_LOAD、页级 `LoadRun`、entry、page-aligned image_end 与 ISA requirement 是同一结果。用户态 audit 只做进程启动和错误呈现，bootstrap 与 libprocess 直接消费相同结果；旧 `parse + isa_requirement + page_plan` 平行解释路径已删除。

### RuntimeGate / IPI failure（已实现）

- HSM 启动前先冻结 expected 集合；任一 `hart_start` 失败先 Release 发布 Failed，再进入平台 fatal/park；
- 普通 IPI 发送返回失败 slot mask，不在已发布业务 Pending 后伪造完成；终止/唤醒保留业务真值并记录失败；
- Gate 的 Preparing/Ready/Failed 由独立原子逻辑 core 实施，Ready 与 Failed 均为终态，并发 Failed 发布幂等。

## 纵向子单元与依赖

1. **规范与契约冻结**：从固定规范确认 status、CPU、ELF header/flags 的接受、合法忽略与明确拒绝集合，形成 canonical 数据与错误接口。不得把已知合法但不可用的节点一概视为 malformed，也不机械拒绝所有非 PT_LOAD header。
2. **ELF admission（已完成）**：纯逻辑 validated image、audit、libprocess 与 Bootstrap 入口已一起迁移；旧 parser 解释分支与重复工具规则已删除。host 覆盖 program-header 分类、entry-in-BSS、段/页权限、容量与 ISA，变异 PT_INTERP audit 被拒绝，virt 完成真实 Bootstrap/launcher 验证。
3. **平台 admission（已完成）**：DT/区间 checked normalize、FramePool 接入、raw-id/slot、RuntimeGate/HSM/IPI 已从生产者到发布者整体收口。host debug/release 覆盖 malformed status、duplicate/unsorted hart、能力依赖、边界算术与 Gate 终态；virt、hetero、nofd、sifive_u 覆盖真实平台发布。

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
