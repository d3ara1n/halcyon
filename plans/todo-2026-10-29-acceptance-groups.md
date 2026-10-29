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

分组实施已提交为 `2314f40`。首次实跑暴露的 Bash 空数组与 init 出生授权问题已修复；用户随后要求逐条运行，本轮十个普通组分别执行并全部通过，FP 配额失败及单条复验也已保留。当前入口为 `just acceptance`，33 条目录路线中其余压力、GDB、故障注入和额外平台未在本轮选择，不能一并标为通过。结果见[逐条运行与修复](#逐条运行与修复2026-10-29)。

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

十个普通组已分别通过，正常出生、领域完成、服务就绪与正常停止取得运行证据。同版本同输入通过的组不重跑；压力、GDB、额外平台和故障注入仍按具体影响与用户选择执行，不能据普通组通过推断这些专项窗口已覆盖。

组态切面 B 的系统运行期责任继续由[原计划](todo-2026-10-28-init-formation.md#当前接力)拥有，本记录只拥有分组工具的运行取证边界。

## 首次运行取证（2026-10-29）

基线 `2314f40`，接手时工作树干净。用户选择先实跑新组织的验收流程，执行 `just acceptance all`（10 条普通 release 路线，默认 THROTTLE=50；首错停止）。未选择 stress、GDB、故障注入或额外平台。

| 轮次 | 结果 | 证据 |
|---|---|---|
| 首轮 | 选择性构建、组态编码、boot package 与用户/内核 ELF 审计通过；尚未启动 QEMU，`run_acceptance` 的空 `options` 数组在 Bash 3.2.57、`set -u` 下展开报 `unbound variable`，exit 1，聚合约 3.5s | `artifacts/acceptance/20261029T094252Z-ordinary-run/summary.json`；完整构建日志 `artifacts/run/20261029-174253-90809-acceptance-memory-basic.log` |
| 修复后 | 入口成功启动 QEMU；memory/basic 在 test_kernel Start 前收到 `instance preparation failed: kernel, System(RightsDenied)`，init 随即进入系统失败终局；runner 正确判失败并清理进程组，exit 1，运行段 0.694s、聚合约 3.1s | `artifacts/acceptance/20261029T094343.227369Z-91322/{guest.log,result.json}`；聚合 `artifacts/acceptance/20261029T094340Z-ordinary-run/{console.log,summary.json}` |

入口修复将普通与 retention 分支的可选参数数组改为从完整 Python 命令开始的非空 `runner` 数组，不改变参数、超时或判据。普通、sifive_u reset、两条 retention 分支的 just dry-run 生成脚本经 `/bin/bash -n` 通过；普通分支另有实际 QEMU 启动证据，其他分支仅做语法检查。没有为这处低影响脚本修复新增测试或重复运行已通过的 Rust 检查。

正式阻塞由[init 计划的首次运行阻塞](todo-2026-10-28-init-formation.md#首次运行阻塞2026-10-29)唯一拥有。本轮只修入口，不改产品出生权限；静态核实生命周期输入缺少 ProcessGrant 必需的 GRANT，运行日志尚未标出 Preparation 具体失败阶段，不把静态定位冒充断点取证。后续从该责任链接续修复，再验证 memory/basic 与其余普通路线。

交付前检查：修复只涉及命令组装，runner 日志、失败拒绝、独立进程组与清理合同保持；退出后核无本轮 launcher 或 QEMU 残留。普通领域通过数为 0，失败 1，未运行 9；不能由本次失败判断这些领域自身已执行或存在缺陷。构建与验收完整日志保留，未提交改动。

## 逐条运行与修复（2026-10-29）

用户明确授权完善流程并逐条运行、逐条确认，禁止一次性运行整个集合。本轮保留前一轮四份未提交修改；正式出生权限修复由 init 原阻塞条目拥有。每次只执行单组，先检查 result/guest 完成事实与清理结果，再选择下一组。普通组之外的压力、GDB、故障注入和额外平台不自动扩大。

| 单条命令 | 已核事实（均含正常收集/Shutdown、无清理错误） | QEMU 段 / 含构建 | 原始证据目录 |
|---|---|---|---|
| `just acceptance memory` | Start、内存完成、正常 DONE | 0.862s / 4.8s | `artifacts/acceptance/20261029T095220.171407Z-92827/` |
| `just acceptance services` | 三宿主 Ready，fs→driver→device Stopped/DONE | 1.330s / 4.0s | `artifacts/acceptance/20261029T095246.622626Z-93096/` |
| `just acceptance ipc` | Sender、Once 跨 Pool 运输与退款 | 0.847s / 3.3s | `artifacts/acceptance/20261029T095329.862153Z-93382/` |
| `just acceptance process` | Kill、Job、封口与枚举检查，无 FAILED | 0.844s / 3.5s | `artifacts/acceptance/20261029T095349.923023Z-93656/` |
| `just acceptance transport` | 真实流、pm 收尾退款、RPC 取消 | 0.841s / 3.4s | `artifacts/acceptance/20261029T095406.851193Z-93908/` |
| `just acceptance time` | 公共期限、到期与因果读 | 1.378s / 3.9s | `artifacts/acceptance/20261029T095421.180035Z-94146/` |
| `just acceptance resources mmio` | 映射、撤销及进程退休 | 0.285s / 2.9s | `artifacts/acceptance/20261029T095440.864270Z-94390/` |
| `just acceptance resources irq` | 独占来源、Seal/Revoke、WaitSet 与关闭 | 0.293s / 2.8s | `artifacts/acceptance/20261029T095456.091923Z-94626/` |
| `just acceptance resources dma` | Pin/SG/退休；两保留 fixture 共 4 页 | 0.846s / 3.6s | `artifacts/acceptance/20261029T095510.981868Z-94869/` |
| `just acceptance platform fp` | D64 运算、FPR/fcsr 跨 trap 校验 | 0.295s / 2.2s | `artifacts/acceptance/20261029T095641.471031Z-95464/` |

FP 首次运行在 Start 前报 `System(QuotaExceeded)`，现场为 `artifacts/acceptance/20261029T095530.772212Z-95123/`。组态仅给 262144 字节 metadata，而当前 ELF 的启动诊断已给出地址空间账本最低存储 394624 字节；`AnySpace::empty` 以目标 Pool 的 MetadataFunding 创建该账本，尚不计其他进程/线程元数据，静态可确认原预算不足。FP 改用既有单实例 1 MiB metadata 工作集，work_pages 仍为 256，映像/主栈/出生页仍由 init 另计；不改内核限额、不自动扩额、不放宽浮点判据。只重跑 FP 后通过，其余九组结果继续有效。

十条成功调用含增量构建合计约 **34.4 秒**，QEMU 运行段合计 **7.821 秒**；不含人工逐条核对、静态检查和 FP 首次失败的 3.3 秒。此为当前缓存、release、THROTTLE=50 条件下的记录，覆盖范围与旧全量不同，不能声称是相同测试集合的等价加速。统计、每轮 ELF/boot package 身份与修复差异快照在 `artifacts/acceptance-sequential/20261029T095802.960248Z-95550/`。没有调用 all 或用循环批量启动测试。

### 本轮收口

- init 的源 GRANT 修复与 owner/失败责任核查由原 init 记录拥有；check/clippy 通过，task 与 resident 的实际正路径均通过。公共接收权限与内核授权规则保持。
- FP 配额只改验收组态，依据实际地址空间账本下界及已有单实例工作集；没有为跑通旧测试增加产品旁路或删除判据。
- 原 Bash 3.2 非空 runner 修复保留，普通分支已有十次成功运行；未运行的 retention/sifive 等分支仍仅有原静态检查证据。
- 最终核无 QEMU 进程残留；每条 result 都为 runner_exit=0、exit_code=0 且无 cleanup_error。源码格式、差异与当前文档链接检查通过。已有通过测试不因文档回填重跑。
- 当前交付为十个普通组的逐条运行及相关修复，已提交为 `fe0ed70`。init 全部负路径、压力和精确故障窗口不在本轮通过声明内；后续按具体需求选择，不把33条目录自动变成每次提交的门。

### 后续测试接力

用户于本轮提交后要求交给另一 agent 继续测试；沿用逐条执行、逐条核对、首错定位的方式，不批量启动整个目录。十个普通组的已有结果继续有效。后续优先选择尚未验证的平台变体，再按风险选择压力或资源专项；历史 clock 调查仍只按复发条件启动。新结果追加在本记录，产品问题回到所属计划，不另建重复验收计划。
