# 验收分组与失效设施清理

## 范围与基线

用户于 2026-10-29 确认实施：按影响领域选择测试，失败后依据现场扩大范围，删除失效与无独立价值的历史设施。基线 `ec121a5`，分支 `refactor/system-redesign`，接手时工作树干净。

范围：Justfile、guest 验收编排与组态、宿主判据/观察器、构建选择、验证规则和当前实现说明。保持正式内核/服务语义，不恢复已删 FS/设备消费者，不建设新测试框架。QEMU 运行由用户另行决定；本轮提供静态、编译、host 与脚本检查，运行期证据明确未覆盖，不声称已缩短到某一实测耗时。

## 已核实问题

- test_kernel 除 memory 模式外串行跑全域；资源专项随后仍跑 IPC/PM/Tunnel/RPC/Job。
- acceptance 聚合含全仓 clippy、debug stress、release core，平台路线重复 core。stress 跳过部分普通退款断言，不能把它视作普通检查的严格超集。
- wrapper 要求已删 init/FS/device 日志；部分手册 recipe 已删除。当前初始化组态迁移的运行证据仍为空。
- build_user 构建全部程序；组态映像清单在构建后才产生。

## 目标结构与执行合同

1. guest 以 settings 的 suite 选择单个领域：memory、ipc、process、transport、mmio、irq、dma、time；mode 只表达压力或具体故障场景。每次只调用本组函数及必需夹具。startup 只执行既有出生输入采用与 Job 收集，供有限服务组态和平台启动观察使用。未知选择在执行前拒绝。
2. 领域组态不叠加 system。仅资源组签发平台根；仅需要子进程的组装入 target/hammer/pm/self 映像；保留 DMA fixture 仍由独立 retention Pool 付款。services 以 System、startup task 和三宿主 after 依赖验证正式装配；有限组态要求至少一个 task，故复用 test_kernel 的出生/Job 收集分支，不新增运行体；platform 只带所需能力消费者，不携带全域验收。
3. 各组成功必须包含选择身份、guest 完成、init 收集全部实例和请求正常 Shutdown；失败终局/panic/意外复位/缺锚点/非零退出均不能通过。sifive_u 只允许在完整成功事实之后收割明确 reset 不支持。
4. test_kernel 原 Harness 持有 capability/子进程；各组正常收束，失败由 test_kernel panic 与 init 的现有 Job/终局接管。宿主 runner 负责自身进程组的退出、超时、中断清理及日志，不能收其他会话进程。
5. 宿主入口 `just acceptance [group] [case]` 无参数只显示目录；all 为显式普通领域组合，不暗含压力/GDB。普通、stress、fault 分开选择；失败停止并保留原日志，不自动全量重跑。所选组的构建仅消费组态引用映像与 srv_init。
6. 静态检查与 host 单测可选择 package/分面。本轮不实现猜测式 git diff 自动选组；依赖更新按真实消费者选择。跨领域变更直接覆盖相应组合，不以先失败为前提。
7. 删除已失效观察器及其专用模块/入口/当前文档；历史档案保持原事实。仍可运行的专项只按需保留。任何失去的覆盖在本记录列明。

## 验证与交付门

- 解析全部命令和组态，核普通分组无系统服务、无无关用例；无参数与无效参数不启动构建/QEMU。
- guest 编译/clippy；组态 host 编译、相关 host 测试；脚本语法与 runner 的失败/缺锚点/中断判定在隔离子进程上检查。新增检查只验证选组与判定合同，不复刻产品逻辑。
- 按 REVIEW 核最终 owner、清理、旧路径、重复真值与当前文档；记录实际执行命令及日志。QEMU 未运行，运行期独立性、正常停止与速度均待用户选择运行后取证。

## 当前状态与交付边界

实施与离线验证已完成，用户于 2026-10-29 授权整理与提交。没有剩余代码拆分步骤；QEMU 未运行，以下运行边界保持开放，不能归档为全套验收通过。当前入口为 `just acceptance`，选择与命令真值由 `tools/acceptance.py` 拥有；33 个可选路线，包括普通、压力、资源观察、平台与历史时钟诊断。

- test_kernel 按 suite 分派；通用 Job 检查不再调用资源退休。MMIO/DMA 专项也不再重复普通平台自检或不相关保留 fixture。普通 suite 只装配自身及必需映像。
- `just check/clippy` 无参数只列选择；按 package/分面执行，`all` 显式全仓。host tests 自动从 rustc 获取并显式传 host target。
- 组态编译器先 `--list`，再按同一引用集合编码/打包，build_user 只构建所选映像及 srv_init。实际构建发现旧 output 包装会丢含空格参数边界，已改为 positional arguments 与 `"$@"`。
- finite services 组态必须有 task，不能用纯 system+shutdown；现用三宿主 Ready 之后的 startup task 触发正式反序停止。startup 仅运行已有 Birth 采用、Job 收束和完成报告，不夹带其他领域检查。
- 宿主 runner 同时核 suite/mode、guest 完成、init 收束及正常 reset，保留旧夹具仍使用的 ` FAILED` 拒绝词；transport 额外核 pm mailbox/source/账户收尾成功锚点。成功和失败日志均保存。
- GDB 资源分支迁至 `check-resources.py`，用实际 formation kernel PID 接入；五个保留模块均独立建组，与组清理合同一致。runner/资源观察器在 finally 内保护重复终止，组清理失败不能通过。

## 删除与覆盖变化

| 清理项 | 当前覆盖边界 |
|---|---|
| 旧 `check-device.py` 及 function/retry/RPC/IRQ热插/idle 专用观察模块、`check-fs-stop-order.py` | 原消费者在组态切面 B 已删，本轮删除失效工具；不再提供键盘复用、候选/Retry、管理者/主控 fault、服务回复丢失、真实function/IRQ热插、fs停止顺序等旧入口。MMIO/IRQ-exit/DMA 当前消费者与有效GDB观察仍保留 |
| `check-system.py` 的旧根监督状态观察 | services 组覆盖正式就绪与正常停止；没有无限空闲、同刻四hart idle 或根监督内部所有权快照证据 |
| 旧 shell wrapper、core/platforms/faults 聚合、过期 recipe 文档 | 普通组独立完成；all 是普通组顺序集合，额外平台/压力/故障显式选择。它不证明旧全域负载共享同一 Pool/Job 时的混合交错 |
| main 内仅打印收发结果的同步消息演示 | 该段错误只日志后继续，没有独立通过判据；现有严格 IPC、Notification、Once 检查保留，不为旧演示再建场景 |
| nofd/平台完整业务 | nofd 验证 Base64 域与正常出生/收集，不再宣称 FP 的 NotSupported 拒绝；sifive_u 不携带全部资源/服务业务，异构路线保留域声明与真实FP task |

这些是测试覆盖的明确取舍，不恢复旧设施，不为过渡测试侧单独登记债务。需要某一已删除交错的证据时，由实际产品修改/故障需求选择当前最小观察边界；不能引用本轮离线结果补足它。

## 验证证据

| 检查 | 结果与日志 |
|---|---|
| guest/组态开发 check | test_kernel、formation-host 通过；`artifacts/checks/20261029T090923.195988Z-87085/test_kernel.log`、`20261029T090923.195987Z-87084/formation-host.log` |
| 内核选择入口 | `just check kernel` 通过；`artifacts/checks/20261029T091851.749509Z-88028/kernel.log` |
| 最终 clippy | `just clippy test_kernel`、`just clippy formation-host` 通过；`artifacts/checks/20261029T092720.643187Z-89265/test_kernel.log`、`20261029T091833.812780Z-87851/formation-host.log` |
| formation host | 6 项既有测试通过；`artifacts/checks/20261029T091833.812742Z-87856/formation.log` |
| runner 合同检查 | 15 项通过；`artifacts/run/20261029-172720-89266-acceptance-runner-tests.log`。普通短命子进程验证缺锚点、错suite、FAILED、pm收尾缺失、panic、reboot、非零退出、reset边界、超时、中断清理、无参数只列目录及包装参数保真；不模拟内核行为 |
| 实际选择性构建/ELF审计/boot package | transport stress release、fp release、services release、memory stress debug 通过；`artifacts/run/20261029-171949-88142-acceptance-selected-build.log`、`20261029-172109-88389-acceptance-build-fp.log`、`20261029-172352-88756-acceptance-build-services.log`、`20261029-172355-88883-acceptance-build-memory-stress.log` |
| 全目录组态校验 | 33 路线经实际 host 编译器 --list 与 encode 校验，映像清单一致；独立核无关服务/FP/pm/hammer 未混入各组，结果 `artifacts/acceptance-formation-validation.json` |
| 工具/文档 | 修改的 Python 语法、Justfile 解析/dry-run、Rust格式、diff 空白及当前引用检查；无 QEMU 执行 |

修复前失败日志保留：`20261029-171851-88024-acceptance-selected-build.log` 为 output 丢参数边界（exit 127），`20261029-172111-88498-acceptance-build-services.log` 为纯 resident 有限组态被现有校验拒绝（exit 1）。初次 checks.py 使用宿主 Python 不支持的 tomllib，已改用 Cargo metadata 获取真实 package/target，不安装依赖。这些均为工具问题，没有据此修改正式系统语义。

## 交付前结构检查

已按 REVIEW 核对本轮范围。没有共享 ABI、内核并发或正式服务生命周期修改，因此硬件协议与产品锁序不新增审计面；出生与退出使用已有正式责任链。各独立组保留自身成功/失败/收束，资源根缺失明确失败，不把未执行组记为通过。

一次辅助只读检查发现三项具体问题，由主代理按实际代码核实后修复：旧 Job 检查的 FAILED 日志未拒绝、pm Abort 后缺少成功收尾证明、保留 GDB 模块未独立建组。前两项加入上述 runner 合同回归；第三项统一五个启动点并保护清理段。没有再启动第二轮独立审查。

新测试只保护本次选组与判定合同，未新增 guest 服务或模拟运行时。旧纯打印示例与失效观察器已删除，当前 README/BUILD/AGENTS、startup impl 及接力入口已同步。历史档案保持原证据。该交付属于验收/构建维护，按 REVIEW 的登记范围不另建可选系统设计 Review。

提交整理只移除资源观察器中已由公共入口处理的重复失败分支，并将其日志前缀改为实际 MMIO/DMA 领域；Python 语法与暂存差异检查通过，已通过的编译和测试不重复运行。验证日志与构建产物不纳入提交；本记录保留为后续运行取证入口。

## 后续运行边界

用户选择 QEMU 后，先运行所关心领域，检查实际启动/收束与耗时；资源 GDB 路线还需核真实断点与窗口是否命中。当前没有运行速度、硬件行为、平台变体或组态迁移运行期正确性的通过证据。服务组的 Ready/Stopped/撤销、startup task 的实际完成及新PID触发下的资源观察窗口均需运行取证；如失败先查工具/组态/guest首因，不自动运行 all。

组态切面 B 的系统运行期责任继续由[原计划](todo-2026-10-28-init-formation.md#当前接力)拥有，本记录只拥有分组工具的运行取证边界。
