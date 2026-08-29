# 批一代码 Review 与 sifive_u 挂死调查报告

> 对象：`794a4c0`（线程资源模型批一）+ `275e4f1`（文档入档）。按 [`REVIEW.md`](../REVIEW.md) 代码 Review 三轴执行；sifive_u 挂死按调查任务并行推进。reviewer 子代理不可用，全部审查由主会话执行，explorer 仅提供唤醒链地图。
>
> 状态：**已收口**——A1–A5 全部修复并验证（末尾「修复记录」节）；§B 挂死根因无法定性，按 §B4 挂起处置，装备留档。
>
> **后续结论**：本报告只记录首轮审查。对 `c1b2ac2..51f4184` 的事务复审随后发现 Ready 前缀预留、Building 操作配平、capability 所有权与 execution binding 等缺口；最终结论见 [`review-2026-08-29-thread-model-transaction-convergence.md`](review-2026-08-29-thread-model-transaction-convergence.md)。本报告“内核侧未发现正确性缺陷”的表述不得脱离该后续报告引用。

## 总评

批一的结构质量高：三 op 事务的 reserve/commit/rollback 四要素完整，锁序无新边（LIFECYCLE→HEAP 已成文、链锁→lifecycle 方向合法），Staging 强引用环的存在域与打破路径论证成立，building_ops 配平在全部失败路径核对无遗漏，begin_running 的原子提取确实消除了「gate 后、入队前」kill 游标窗口，bootstrap 内嵌序列与 start_staged 同构。对照 review 计划的事务正确性/Staging 环/ABI 同步/回归面四轴逐项核过，**内核侧未发现正确性缺陷**。

发现集中在三处：用户态组装语义的两个真实 bug、一个**负载设计缺陷（wfi）**——后者解释了目前所有验证线的间歇性失败，以及一项验证基础设施配置错误，它制造了「挂死」假象。

---

## A. Diff Review Findings

### A1【中危·负载设计缺陷】kill-vs-start 靶在 U-mode 执行 wfi 必然 IllegalInstruction

- **位置**：`user/systems/init/src/race.rs:192`——`process::write(0x1000, &[0x73, 0x00, 0x50, 0x10])`（wfi 编码 0x10500073）；`race.rs:506` allowed 集合仅 `[(Killed, Some(kill_code))]`。
- **证据**：多平台多档位复现，终因恒为 `reason 2 code 0x2`（Fault/IllegalInstruction）：
  - virt 5 核 @50%：3 轮中 2 轮失败（round 3 / rounds 0+2）；
  - sifive_u 全速：8 轮中约 2-3 轮失败（round 0/1/2/3 随机）；
  - **virt 4 核 @50%（标准 `just virt` 等价配置）：2 轮中 1 轮失败**；
  - 失败轮 `dist` 恒为 `start-first 4/0`，即 Start 总先赢、靶线程已入册。
- **机理**：场景假设「Start 先行则靶在 wfi 中安静等待被 kill 收束」。但 wfi 是特权指令，RISC-V U-mode 执行触发 illegal-instruction 异常（QEMU 实测如此；特权规范允许实现如此）。靶一旦真正上核，第一条指令即 fault，终因冻结为 Fault——与 kill 的 Killed 竞争，**首达者胜**，fault 路径有完整胜出机会。内核行为完全正确（用户态非法指令 → 杀进程）。
- **为什么批一验证时 virt 4 核全绿**：4 核满载时靶线程入队后排队靠后，kill（短路径）总在靶首次 dispatch 前冻结进程，pick gate 吸收，终因恒为 Killed。任何有空闲核或时序抖动的配置（5 核、全速、节流档漂移）都会撕开这个窗口。**这不是批一引入的回归**——批一前同样的 wfi 页与同样的竞速结构已存在（`c1b2ac2` 上 8 轮未失败属调度运气）；批一改变 attach/Start 时序分布使窗口出现率上升。
- **修复建议**：入口指令改为 `j .`（自旋 0x6f 0x00 0x00 0x00），保持「Running 后被 kill 收束」的场景语义最纯正；或 allowed 集合增加 `(Fault, Some(IllegalInstruction))`。前者优于后者（后者弱化 start-first 分支的 kill 收束覆盖）。注意 kill-vs-abandon 复用 `build_wfi_building` 但从不 Start（靶不上核），不受影响。

### A2【低危·真实 bug】出生块 parent_pid 错位一代

- **位置**：`user/frameworks/libprocess/src/lib.rs:86`——`build_birth_block(created.pid, rinlib::env::parent_pid(), ...)`。
- **违反契约**：出生块描述**目标**进程的创建关系，parent 字段应为创建者（组装者）pid。旧内核机制（`map_startup_block` + `build_startup_block(process.pid, process.parent, ...)`，proc.rs 旧 1292 行）写的是 `process.parent` = 创建者。新代码写的 `env::parent_pid()` 是**组装者自己的父进程**（目标的祖父）——init（parent 0）spawn 服务时，出生块 parent_pid = 0 而非 1。
- **正确值**：`rinlib::env::pid()`。
- **可达性**：所有经 libprocess spawn 的进程；当前 `env::parent_pid()` 无其他消费者（全树 grep），属潜伏语义 bug，不引发现行故障。内核侧 ProcessControl 快照的 parent_pid 仍用内核真值（process.rs:228），两处数据源不一致已是事实上的双重真值苗头。
- **顺带**：计划篇「文档同步清单」未覆盖此字段语义，修复时应一并入档。

### A3【低危·语义回归】libprocess grant 超限静默截断

- **位置**：`user/frameworks/libprocess/src/lib.rs:75`——`request.grants.len().min(MAX_START_GRANTS)`。
- **旧语义**：grants 超 64 时内核 `IllegalArgument` → spawn 失败，调用方立即知晓。新语义：静默装前 64 个、丢弃余量，`Spawned` 不报告截断。
- **修复建议**：超限直接报错（`SpawnError::InvalidImage` 或新增变体），不截断。

### A4【验证基础设施】`just sifive_u` 在当前验收面下必然失败

- **位置**：`Justfile:160-162`——`run_qemu_acceptance_timed` 用 `timeout --foreground 5` 且**不经过 `qemu-throttle.sh` 包装**（其余 acceptance 线都有）。
- **实测**：全速 sifive_u 完整验收需 ~9-10s（正常完成时 init lifespan ~8.3s + boot）；50% 节流需 ~45s。5s timeout 无论档位都砍在中段——实测 `just sifive_u` 7.2s 总时长（缓存构建）后以 missing anchor 失败，**100% 复现**。
- **历史成因**：5s timeout 由 b161163 设定（当时验收面轻）；step 9 竞态矩阵并入 common profile、c1b2ac2 加入锤侧延迟变体后未重新校准。
- **危害**：砍断点恰在矩阵中段（kill-vs-exit round 0 前后），形态与「挂死」无法区分——无 panic、无 quiescent、日志中断在 spawn 附近。**计划档案记录的「确定性挂死」观察窗口与此高度吻合**（见 §B2）。
- **修复建议**：timeout 提到全速档的 2 倍余量（≥20s），或给该线接 throttle 包装后按节流档校准；AGENTS.md 的 sifive_u 超时纪律段落同步更新。

### A5【文档】三处与代码事实不符

1. 计划篇 ABI 表：`ProcessGrant(builder, grants_ptr, count)` 三参——实现是四参（+`out_values`，`syscall.rs` x[13] / `rinlib call.rs`）。ABI 文档应记录完整签名。
2. 批一验证状态表「`just check` 零错误零警告」——当前环境实际有 1 条链接器警告（`riscv64-elf-ld` RWX LOAD segment，`linker_messages` lint）。环境性、非代码问题，但「零警告」陈述不成立。
3. review 计划（`todo-2026-09-thread-model-review.md`）称「init race.rs 探针已移除」——实际探针保留在提交中（计划篇自己说「收口时移除」）。两处矛盾，收口时统一。

### 观察项（不构成 finding）

- **出生块 RW 映射**：旧内核机制是 USER_RODATA 只读；新 `write_birth_block` 映射 READ|WRITE，只读性降级为接收方约定。计划 D6c 已拍板为 v1 约定，风险已知悉——建议 notes/impls/startup.md 保持显式声明（已检查，275e4f1 已写明）。
- **`freeze_requirement` 断言对 Base64 失效**：Base64=0 与「未冻结」不可区分，double-freeze 断言只对 D64 有效。Start 的 Building 一次性门使二次冻结实际不可达，无风险；如追求严谨可存 1+discriminant。
- **`commit_pinned_for_start` 返回 `Option` 在 grant 路径被丢弃**：builder=None 时确定性返回 None，语义成立；建议 `debug_assert!(builder.is_none())` 提升自证性（Option 非 must_use 类型，无警告，非 bug）。

---

## B. sifive_u 挂死调查

### B1 复现实验（严谨记录）

| 配置 | 轮数 | 真挂死 | 失败形态 |
|---|---|---|---|
| HEAD（=794a4c0 代码）全速 | 8 | 0 | A1 wfi fault ×2-3，其余 10/10 |
| HEAD + gdbstub（`-s`） | 12 | 0 | 全部完成 |
| HEAD @50% 节流（hunt 脚本，8s 无增长判据） | 15 | 0 | 全部到终态 |
| `just sifive_u`（recipe 原样） | 3 | — | **100% 失败**（A4：5s timeout 砍断，形态酷似挂死） |
| 794a4c0 全速 | 9 | 0 | A1 ×1 |
| c1b2ac2（批一前）全速 | 8 | 0 | 全绿 |
| virt 5 核 @50% | 3 | 0 | A1 ×2 |
| virt 4 核 @50%（标准线） | 2 | 0 | A1 ×1 |

合计 55+ 轮，**已提交代码上真挂死 0 次**；全部可观测失败均由 A1（负载）或 A4（基础设施）解释。每轮均验证到达终态（quiescent 停机或 init panic 收束），无「日志停增且无终态」样本。

### B2 原始证据重新解读

- `artifacts/.qemu-acceptance-50536.log`（8月29 03:53，无 timeout 的 acceptance 管道产物）：**真挂死现场**——停在 `race kill-vs-kill passed` + `pid 24 reaped` 之后、kill-vs-exit 首个探针之前，无 panic 无 quiescent。时点在批一提交（04:29）前 36 分钟，对应**开发中间态代码**，不对应任何已提交版本。
- 同日 04:25 / 04:27 的 `sifive-manual.log` / `sifive-long.log`：10/10 全绿。同一开发窗口内「既全绿又挂死」——非确定性，与计划档案「确定性挂死」的定性冲突。
- 档案描述互相矛盾：计划篇称「init 卡在 `h.report(0)`（探针显示 spawn 全部完成）」；现场日志显示卡点在 spawn 之前；review 计划称「142% CPU ≈ 2.5 核自旋、卡点漂移（先 spawn 中段后 report）」。三者描述的卡点不同，说明观察来自不同（中间态）代码版本。
- **结论**：「sifive_u 确定性挂死」在已提交代码上不成立。最可能的解释按概率排序：
  1. 开发中间态的内核自旋死锁（review 计划的 CPU 采样证据支持「条件自旋」），提交前的某次改动顺带消解——即挂死从未存在于 `794a4c0`；
  2. 极低频竞态，当前宿主负载/QEMU 11.1.0 时序下无法触发；
  3. A4 基础设施失败在调查期被误读为挂死（形态逐字吻合：推进至 kill-vs-exit round 0 spawn → timeout 收割 → 无 panic 无 quiescent）。档案「约 25s 收割」与 recipe 的 5s 不符，指向手动长 timeout 实验，故此解释需要「25s 观察也来自挂死」才成立——与解释 1/2 不互斥。

### B3 唤醒链审计（explorer 产物，全部带位置证据）

- wait_many → park → 订阅时电平复查 → offer/arm/claim 仲裁 → finish_offered → enqueue → wake_one → ipi_slots → SBI send_ipi：全链核过。
- 三个关键不变量成立：**清 idle 位后必再 pick**；**置 idle 位后 has_ready 双重检查与 enqueue 同锁线性化**；**waker 必醒着**（quiescent 停机安全性）。
- 头号嫌疑窗口（若挂死真实存在）：IPI 投递到入睡 secondary hart——sifive_u 有历史前科（pre-ai ca400da「OpenSBI 唤醒的 secondary 无法接收 IPI」），当前用 SBI send_ipi 是否消解未证实。次级：quiescent 谓词对「队列有线程但目标 hart 醒不来」无感知（挂死呈现为永挂而非误停机）——这是已知限制而非缺陷。
- 内核无 hart 数量/编号连续性假设：slot 稠密抽象完整（raw hartid 只在 SBI/DTB 边界），`1u64 << raw` 对 hart 1-4 正确；timebase 换算无截断（1MHz → 1000 ticks/ms）。

### B4 未闭合声明

按用户指示，无法解决的问题停下报告：**挂死根因无法定性**——未复现即无现场，55+ 轮零样本。唯一确凿的真挂死现场（50536）属于未提交的中间态代码。建议处置：

1. 不作为批一阻塞项（批一的问题清单里它是「未决」而非缺陷主张，且证据链已翻转为「不可复现」）；
2. 保留复现装备：hang-hunt 模式（日志停增 + 无终态判据 + QEMU 存活）+ `qemu -s` gdbstub + `riscu64-elf-gdb thread apply all bt`——本次已验证装备可用（gdbstub 会改变时序，挂死复现概率可能下降，记录在案）；
3. 若再次复现：优先 GDB 多采样（explorer 提示条件自旋的静态 PC 需多样本），次选在 `send_ipi` 与 `idle()` wfi 醒来处加计数探针；
4. review 计划 B 节的调查现场描述需按本报告 §B2 更新（当前内容与代码事实不符，会误导后续会话）。

---

## C. 建议行动（按优先级）

1. **修 A1 + A4**（负载与基础设施）——二者叠加使当前所有验证线间歇性假失败，不修复则批二无法得到可信的验证信号；
2. **修 A2/A3**（libprocess 两处，一行级改动）；
3. 文档修订：A5 三处 + review 计划 B 节更新；
4. 挂死：按 §B4 挂起处置，装备留档。

## 修复记录（收口）

A1–A5 已全部实施并验证，见下。

### 代码
- **A1**：`build_wfi_building` → `build_spin_building`，入口指令 wfi（0x10500073）改 `j .`（0x0000006f）；`SPIN_FOREVER` 常量收在 race.rs，seal_before_start 改走共享 helper（原重复的 wfi 内联序列删除）。
- **A2**：`libprocess::spawn` 出生块 parent_pid 改 `rinlib::env::pid()`（组装者自身）；`build_birth_block` 文档注明「parent_pid 是组装者 pid，不是组装者的父」。
- **A3**：grants 超 `MAX_START_GRANTS` 改为建进程前显式 `SpawnError::TooManyGrants`（新变体），不静默截断。
- **A4**：`run_qemu_acceptance_timed` 接 `qemu-throttle.sh` 包装 + `ACCEPTANCE_TIMEOUT`（默认 60s，经 env 覆盖）；`tools/qemu-acceptance.sh` 增加终态锚点（quiescent / panic 收束）驱动的主循环，出现即主动收割 QEMU——sifive_u 正常轮次由等满 timeout（64s）降为 ~22s，且不再依赖 timeout 数值与验收面耗时的脆弱对齐；真挂死仍由 timeout 兜底。AGENTS.md sifive_u 纪律段落同步（统一走 `just sifive_u`，timeout 只作兜底不作期望）。
- **探针清理**：`srv_hammer`（gun consumed / report ready / killing / kill returned 四处）与 `race.rs`（kill-vs-exit round spawning/spawned 两处）调查探针移除；kill-vs-exit 保留原有的 round/终因 debug 汇总。

### 文档
- 计划篇 ABI 表 `ProcessGrant` 补第四参 `out_values`；「零警告」陈述改为「零错误（1 条环境性链接器警告）」；「sifive_u 确定性挂死」按 §B2 改写为证伪 + 两类根因 + 复现装备留档；review 计划 B 节同步（挂起 + 探针移除声明）。
- 计划篇「文档同步清单」补 parent_pid 字段语义（A2 顺带项）。

### 验证
- `just check` / `just build_user`：全绿，无新增警告；host 单测 106（os 纯逻辑）+ 7（shared）。
- `just sifive_u` ×5 连过（22–23s/轮，终态锚点收割）；`just virt`（debug 50%）、`THROTTLE=100 just virt`、`just virt-release`、`just virt-hetero`、`just virt-nofd` 全绿；静默停机帧数正常终态（virt 248843）。
- 帧守恒：virt 248843 free（基线 248836；差 7 帧系探针移除）；sifive_u 19927 free（不变）。
## 审查覆盖声明

- diff 全量逐文件审读（17 文件 +929/-501）；重点轴（事务、锁序、引用环、配平、原子性）逐项对照 review 计划核过；
- 验证执行：内核 check（0 error，1 环境性 linker warning）、virt 4/5 核、sifive_u 多档位共 55+ 轮、`just sifive_u` 3 轮；
- 初始审查未覆盖的 release 与 host 测试已在修复收口阶段补齐；挂死未复现，因此没有可采集的 GDB 现场。
