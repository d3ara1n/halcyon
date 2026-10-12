# 验收 fixture 与启动期自检逐步清理

> 状态：**验收运行整理已完成，未提交**。用户授权保留合理检查、清理重复并优化运行；本轮从 `985fb17` 干净工作树接手。2026-10-07 文档校准及更早证据保留为历史，本轮设计、保留边界与验证由末节拥有。

## 目标与边界

逐步整理`test_target`、`test_hammer`、内核启动期自检和服务内验收残留，减少不必要的资源消耗与维护面。删除前必须核定原覆盖契约、正式替代观察点及受影响装配；不能用删除锚点代替验证完成。

2026-10-07 仅校正文档中的路径、装配和覆盖描述，没有清理 fixture、自检或运行新验收。当前实施范围见末节，不改内核机制、业务 deadline、部署容量或节流政策。

## 当前消费者与装配

以下表格描述当前源码与脚本观察点；本轮运行结果及保留判断由末节记录，不把历史证据算作新验收。

| 对象 / 装配owner | 当前行为与观察点 | 后续删除或替代的前提 |
|---|---|---|
| `test_target` / `srv_init::launch_test_services`及按需spawn | 首实例进入acceptance Job；同一映像在pm_domain中启动另一个成员，供枚举、派生管理能力、kill与监督接管使用。`public_ipc::committed_kill`通过独立进程制造非空Close提交后的调用者消散和自动回收；stress的`test_automatic_retirement`以`retirement` payload创建256项WaitSet，Kill后观察DONE与Dead/Killed快照 | 保留独立Job归属、能力与进程消散窗口、IPC并发和非空WaitSet退休观察；复用服务的普通退出不能直接充当替代证据 |
| `test_hammer` / `srv_init::race` | stress/memory 打包。HAMMER模式提供两个独立指令/回执Mailbox与Notification发令枪；TARGET模式提供独立地址空间的退出、fault、线程/映射和Tunnel剧本。stress要求16/16；memory只复用线程/映射目标及完整收束 | 保留相同独立执行点、地址空间、故障/竞争窗口和逐场景收束证据；不能以init内串行调用替换 |

`Justfile`始终打包`test_target`，stress/memory另打包`test_hammer`。core/stress以`RequiredLaunchSet`检查必需映像及首批启动；memory只采用hammer映像，不启动FS/PM。映像均通过`StartupImage::from_slice`借用init只读启动payload，不复制到init堆；payload由当前backing/View与init地址空间持有并随退休回收。

`tools/qemu-acceptance.sh`对core/stress及平台变体要求committed IPC caller kill、监督和业务退款锚点；stress另要求256注册自动退休、control/Tunnel压力与16/16矩阵。memory要求真实发布窗口、线程完成、Job收束和辅助能力清理；各正常负载均核启动自检和reset。脚本拒绝`acceptance failed`及` FAILED`等日志。Job子项并非每项都有单独的required锚点，不把聚合通过表述为每个内部子项均被独立断言。

相关事实入口：`Justfile::make_initfs`、`srv_init/src/{main,public_ipc,race}.rs`、`test_target/src/main.rs`、`test_hammer/src/main.rs`、`tools/qemu-acceptance.sh`。启动与映像持有的实现视图见[startup](../../notes/impls/startup.md)。

## 当前启动期自检

`os/kernel/src/boot.rs`在bootstrap发布前依次调用以下三项。本轮保留理由见末节，不将纯逻辑host覆盖冒充目标侧库存/付款集成证据。

| 自检 / owner | 源码中的观察点 | 当前文档处置 |
|---|---|---|
| `clock::selftest::run` / `os/kernel/src/clock/selftest.rs` | 隔离ClockState的origin偏斜、单调水印、延迟样本、不可逆ClockRange、epoch末端及嵌套失败；失败实例不污染平台时钟 | 按隔离样本描述，不再把它写成直接注入平台时钟并验证全hart停驻 |
| `MemoryPool::self_test` / `os/kernel/src/task/memory_pool.rs` | prepared子Pool回滚、delegation提交、funded owner保活来源、最后释放后的credit/库存归还，以及额度不足的错误与退款 | 补入原记录遗漏的现存自检；本轮未核定与host/用户态测试的等价覆盖 |
| `frame::selftest::run` / `os/kernel/src/frame/selftest.rs` | 全范围清零、funding失败、extent分割与两种释放顺序、回滚及来源退款 | 补入现存funded frame自检；源码来源与历史Review入口见[库存来源计划](../archived/todo-2026-09-frame-source-selftest-review.md) |

原记录中的`task/wait_set/selftest.rs`、`task/tunnel/selftest.rs`及其旧continuation、HeapPressure细目已不对应当前源码，不能继续列为待保留的现存自检。原“时钟回退、WaitSet交错、Tunnel堆耗尽”的保留判断属于旧实现基线；文档校准只撤下失效的描述，没有追查其删除过程或判定新机制是否等价覆盖。机制与已有验证边界由[对象链档案](review-2026-10-07-object-chain.md)拥有；若后续审查产生具体覆盖缺口，由该次唯一报告承接，不凭路径消失认定覆盖完成或缺失。

## 已实施清理与历史证据

本节保留原批次事实，不作为2026-10-07重新执行的证据。

- `shared/tar::walk`保留输入slice生命周期，init的target/hammer ELF改为借用启动payload；原sifive_u复制映像导致的`Heap allocation error, layout = 5001400`在该批解除。
- 首批删除内核堆启动自检及仅供其使用的`frame::remaining_heap_chunks`、`srv_pm`两次共40 ms的启动Sleep；竞态指令由5个word缩为3个，移除无效entry/sp字段与冗余START helper。旧system ticket等成本模型只用于追溯该批，不描述当前堆付款。
- `test_derive_kill`枚举/派生失败仍保留control进行收束，但返回失败；脚本将Job子项的` FAILED`判为失败。
- 原批次`just check`、七面`just clippy`、`git diff --check`通过；virt core/stress/release、sifive_u、virt-nofd通过，stress 16/16。sifive_u在明确reset失败后按既有方式收割。
- 原boot-failure的panic/alloc/fatal注入确认四hart停驻。验收判定收敛后`bash -n tools/qemu-acceptance.sh`、七面clippy与完整acceptance通过；没有单独注入Job失败日志的自动化用例。这些均为历史运行结果。

## 后续代码清理的完成门

后续代码任务先固定一个fixture或自检及独立覆盖契约，再核定替代观察点。需要删除时，同步Cargo、Justfile、BootPackage、RequiredLaunchSet、init映像持有、脚本锚点和文档；验证按实际受影响路线选择，不为文档修改运行实现验收。独立失败窗口无替代时保留，保留原因和重审条件写在本记录；不为删测试另造专用服务。

2026-10-07 文档校准完成门为：现状表对应源码入口和脚本声明，旧机制/旧结果归于历史，未核定的等价覆盖保留未知。该文档范围已完成。

## 当前接力

本轮实施与验证已完成，无待续施工。`test_target`、`test_hammer`及三类启动自检按独立覆盖保留，不列为无条件待删债务；仅在相关 owner 调整、消费者替代或测得显著成本时重审，并由当时任务引用本篇。代码尚未提交，获准提交后按 REVIEW 登记固定交付的独立审查，本轮只读结构检查不替代该审查。

## 验收运行整理（2026-10-11）

### 基线与范围

`artifacts/convergence3/acceptance-summary.json` 记录旧完整验收 784.55 秒含构建。分路线日志跨度约为 stress 201 秒、VM fault 241 秒、sifive_u 141 秒、nofd 129 秒、release 25 秒；这些跨度不是精确 guest CPU 时间。VM fault 只需观察真实发布窗口及其消费者收束，却复跑完整 stress，是本轮主要重复。平台路线保留完整业务仍有收益：近期 nofd 确实暴露并发 CreateOpen 的独立错误。

### 已定设计与责任链

- `acceptance-core` 拥有 clippy、无调试器的 debug stress、release core；`acceptance-platforms` 拥有 sifive_u/nofd 完整 core；`acceptance-faults` 拥有 Boot/IPI/VM fault/VM repair。`acceptance` 依次组合三组，保持原聚合覆盖；hetero 仍在调度域变更时额外运行。日常局部开发按影响选单条路线，不把所有组变成每次编辑的门。
- 平台路线默认 release，可通过 recipe 参数选 debug；独立观察仍包括完整 core 收束、FP 拒绝/域拓扑和平台 reset 边界。无调试器 debug stress 保留。此举改变构建档位而不删平台业务场景。
- VM fault 使用同一 srv_init 的 `memory` workload，只复用已有 `race::thread_memory_suite` 和原 test_hammer 映像。完整 stress 仍运行该场景及原 16/16 矩阵；不复制目标逻辑、不增加进程映像或替代 Runtime。
- memory 沿原 RootSupervisor 建立子 Job，消费者等待 Dead/Completed，再收束子 Job、关闭辅助 authority、检查根账本并走同一 SystemReset 权限检查。失败返回原 init 错误路径，RootSupervisor 保有 services 域的兜底责任；host wrapper 检查退出、失败文本和独立 memory 锚点。GDB 仍只旁观真实 RetryFault，首次观察后 detach；直到消费者完整收束与 QEMU 正常 reset 才算通过，不提前杀 guest 宣称退款完成。
- 验收公共锚点拆为所有正常 workload 共同的启动自检/收束/reset，以及完整 core/stress 的业务要求。memory 不伪造 FS/PM 成功输出；VM fault 仍独立核真实 cookie-before-return 和 GDB 证据。
- 删除 Copy 已有独立成功断言后的合并成功文本，并将当前源码/验收器的 FAL2 前缀统一为 FS1；历史日志、归档证据不重写。不通过删 required 规避仍有效的独立合同。
- 保留三项启动自检：frame/Pool 与真实库存和付款集成有关，host 等价覆盖未证明；Clock 隔离状态虽可下移，但当前纯状态与内核时间 owner 尚未分离，为省极少启动工作扩张库边界不属于本轮。后续在时钟 owner 调整时再核定迁移，不恢复已删除 WaitSet/Tunnel 自检。

### 实施顺序与验证门

先接 feature/打包/现有消费者和完整收束，再同步脚本与路线组，最后回填手册、impls 与导航。没有新增 ABI、并发机制或生产授权政策；新 workload 仅组合现存责任链，锁序和 hart 合同不变。

验证风险：memory 必须实际命中发布窗口、完整退出而非提前收割；普通 workload 不得因拆锚点漏验业务；release 平台必须继续正确降级和收束。执行脚本语法/Justfile 解析、`just check`、包含 memory feature 的 clippy、定向 memory/GDB 验收和最终三组聚合（成功路线不因汇总再次运行），另验 hetero 默认 release。日志、退出与含构建耗时保存在 `artifacts/acceptance-cleanup/`，不以旧超时上限当性能证据。通过后依 REVIEW 逐项核对覆盖映射、失败/清理和文档，不提交。

### 实施、验证与交付检查

feature/打包/消费者/脚本/路线组已接通，构建手册与startup impls已回填。`just check`及八面clippy通过；验收策略集合对照确认core/stress/hetero/nofd仅删除一条重复Copy汇总、FAL2→FS1同义改名，其余required不变，rejected/终态/日志策略逐字不变。自审发现用户态Pool锚点不适用于memory，已改要求真实启动Pool自检，完整业务仍保留原锚点。辅助只读结构检查复核该修正、memory真实消费者及现有监督收束，未发现新增可达问题。

`just acceptance` 一次完整通过，含构建/八面lint/三组路线 **392.79 秒**；相较原留存 784.55 秒减少约49.9%。该比较包含构建，采用默认50%节流，不是严格控制环境的性能基准。另行 `just virt-hetero` 默认release通过，含构建34.84秒。完整命令退出、日志、工具链、诊断证据、日志SHA及比较边界保存在 `artifacts/acceptance-cleanup/summary.json`，原始 `results.json` 和各 `.log` 保留。

| 路线 | 本轮结果 | guest日志跨度（非CPU时间） |
|---|---|---|
| debug stress | 完整业务、256注册退休、16/16矩阵、退款/reset通过 | 214.10秒 |
| release core | 完整业务/收束通过 | 25.50秒 |
| release sifive_u | 完整core通过，明确reset失败后按既有规则收割 | 32.00秒 |
| release nofd | 完整core与预期FP降级通过 | 24.85秒 |
| debug memory / VM fault | 真实RetryFault、cookie-before-return、线程Completed/Dead、Job/辅助能力关闭、reset通过 | 36.14秒 |
| Boot/IPI/VM repair | 三种启动故障、IPI四hart停驻、read/write/execute/repeat/uaccess全部通过 | 各诊断保留原证据，不以GDB停顿推导计时 |
| release hetero | 两域拓扑、FP实际执行及完整core通过 | 32.59秒 |

验收器另回放本轮真实memory日志：完整日志通过，缺Job收束标记、错误core profile、成功后追加panic分别被拒绝；这是host脚本判定检查，不是新增QEMU证据。对照脚本及回放记录为 `policy.json`、`wrapper-checks.json`。Python/Bash语法、Justfile解析、默认/显式debug命令展开和本次文档本地链接核对通过；没有改host逻辑库，不重复运行无关host全套，clippy仍覆盖host测试目标。平台debug保留原入口但本轮实际执行的是release，未把命令展开算作debug新运行。

交付前按REVIEW核对：新增memory无内核测试开关、共享ABI或第二Runtime；由原Pool/Job/RootSupervisor拥有资源与失败兜底，GDB不提前收割成功；两个close结果进入线程成功条件；完整业务独立断言及无调试器stress保留；共享产物串行构建，所有本轮QEMU/GDB及runner已退出。Clock/Pool/frame保留理由明确，已删重复文本和旧FAL2前缀，无当前承诺范围内的过渡实现。历史Watch未知错误、精确来源/映射/迟到Bind等注入缺口仍由原所属计划维护，不由本轮普通验收关闭。当前能力边界、实际消费者、收束与验证已闭合，记录归档，不提交。
