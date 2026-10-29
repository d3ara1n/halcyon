# 验收分组与失效设施清理

状态：已完成。用户已授权整理提交；按最终内核公开契约范围归档，后续依据构建手册按实际改动选择验证。

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

已提交基线为 `78d7da8`（分组 `2314f40`、出生修复 `fe0ed70`）。用户进一步明确自动验收限于内核公开契约：GDB自动路线与跨服务业务消费者已移除，当前目录20条，普通集合9条。新结构下9条普通及IRQ退出/DMA共享/额度3条专项逐条通过，工具与清理合同14项通过；其余压力和额外平台不在本轮运行声明内。清理、证据与边界由[公开行为验收替代](#公开行为验收替代2026-10-29)拥有；此前两轮结果按当时范围保留为历史。本轮实现、验证与文档已收口，按用户授权整理提交并归档。

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

自动验收当前只覆盖内核公开契约，按本篇最后一轮的目录与证据接手。此前服务启停、GDB与故障注入的运行结果保留当时事实，不恢复到自动验收目录。压力与额外平台按具体风险显式选择，同版本同输入通过的检查不重跑。

组态切面 B 的系统运行期责任继续由[原计划](../todo-2026-10-28-init-formation.md#当前接力)拥有，本记录只拥有分组工具的运行取证边界。

## 首次运行取证（2026-10-29）

基线 `2314f40`，接手时工作树干净。用户选择先实跑新组织的验收流程，执行 `just acceptance all`（10 条普通 release 路线，默认 THROTTLE=50；首错停止）。未选择 stress、GDB、故障注入或额外平台。

| 轮次 | 结果 | 证据 |
|---|---|---|
| 首轮 | 选择性构建、组态编码、boot package 与用户/内核 ELF 审计通过；尚未启动 QEMU，`run_acceptance` 的空 `options` 数组在 Bash 3.2.57、`set -u` 下展开报 `unbound variable`，exit 1，聚合约 3.5s | `artifacts/acceptance/20261029T094252Z-ordinary-run/summary.json`；完整构建日志 `artifacts/run/20261029-174253-90809-acceptance-memory-basic.log` |
| 修复后 | 入口成功启动 QEMU；memory/basic 在 test_kernel Start 前收到 `instance preparation failed: kernel, System(RightsDenied)`，init 随即进入系统失败终局；runner 正确判失败并清理进程组，exit 1，运行段 0.694s、聚合约 3.1s | `artifacts/acceptance/20261029T094343.227369Z-91322/{guest.log,result.json}`；聚合 `artifacts/acceptance/20261029T094340Z-ordinary-run/{console.log,summary.json}` |

入口修复将普通与 retention 分支的可选参数数组改为从完整 Python 命令开始的非空 `runner` 数组，不改变参数、超时或判据。普通、sifive_u reset、两条 retention 分支的 just dry-run 生成脚本经 `/bin/bash -n` 通过；普通分支另有实际 QEMU 启动证据，其他分支仅做语法检查。没有为这处低影响脚本修复新增测试或重复运行已通过的 Rust 检查。

正式阻塞由[init 计划的首次运行阻塞](../todo-2026-10-28-init-formation.md#首次运行阻塞2026-10-29)唯一拥有。本轮只修入口，不改产品出生权限；静态核实生命周期输入缺少 ProcessGrant 必需的 GRANT，运行日志尚未标出 Preparation 具体失败阶段，不把静态定位冒充断点取证。后续从该责任链接续修复，再验证 memory/basic 与其余普通路线。

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

## 后续五条取证与 MMIO 观察修复（2026-10-29）

RubyCreek 转交用户的继续测试指示后，在 `78d7da8`、干净工作树接手并独占共享构建产物。先选择尚缺运行证据的三种平台变体，再用 process stress 覆盖 debug/mode 与竞态矩阵入口、mmio-publication 覆盖新 PID 下的 GDB 精确窗口。每条结束后核 result、guest/观察事实与进程清理，再启动下一条；没有调用 all、用循环批量运行或重跑前十条普通组。

| 单条命令 | 已核事实 | QEMU/观察段 / 含构建 | 原始证据目录 |
|---|---|---|---|
| `just acceptance platform nofd` | 四 hart Base64 域，startup 出生、完成、正常收集与 Shutdown，runner_exit=0 | 0.290s / 3.966s | `artifacts/acceptance/20261029T102048.214653Z-96959/` |
| `just acceptance platform hetero` | hart0 Base64、hart1–3 Base64+D64；真实 FP 验证、正常收集与 Shutdown，runner_exit=0 | 0.294s / 2.993s | `artifacts/acceptance/20261029T102122.578123Z-97242/` |
| `just acceptance platform sifive_u` | startup 完成、全部收集、Requested Shutdown 后才出现 NotSupported；按平台合同主动收割，harvested_after_reset_failure=true、runner_exit=null | 0.700s / 3.100s | `artifacts/acceptance/20261029T102144.098106Z-97499/` |
| `just acceptance process stress` | debug/mode=1，注册表回收、线程压力和16/16竞态矩阵，正常收集与 Shutdown，runner_exit=0 | 78.963s / 83.633s | `artifacts/acceptance/20261029T102220.318238Z-97788/` |
| `just acceptance resources mmio-publication`（修复后） | 原 controller PID=2、目标PID=3/4；Preparing/PublishGranted 两窗口、Revoke返回、目标存活时scope DONE/members0、32页IO PTE均移除、两个Pool双维零；正式完成及Shutdown，runner_exit=0 | 3.972s / 7.240s | `artifacts/acceptance/20261029T102541.761364Z-98842/`；窗口证据 `artifacts/mmio-publication/20261029T102541.818048Z-98845/evidence.json` |

全部 result 的 exit_code=0 且无 cleanup_error；virt 四条与观察器成功退出，sifive_u 只按明确成功后 reset 不支持的例外通过。每条之后均核无 QEMU 残留；MMIO 结束另核无 GDB 残留。五次成功调用含增量构建合计100.932s，运行/观察段84.219s，三平台与process使用THROTTLE=50，精确MMIO观察为100；不含人工核对、修复和首次失败，不作性能承诺。汇总、源码hash与工具修复patch在 `artifacts/acceptance-sequential/20261029T102627Z-followup-summary/`。

### MMIO 首败与观察器处置

首轮 `mmio-publication` 的真实 Map 返回 ObjectClosed，目标随后因预期 ObjectBusy 触发断言；原始现场 `artifacts/acceptance/20261029T102354.463811Z-98422/`，运行段2.977s、含构建7.239s。对应 `artifacts/mmio-publication/20261029T102356.678918Z-98424/` 中 publication.log 为空、windows=[]，说明没有建立目标许可交错，不能据此认定内核 MMIO 发布契约错误。

工具此前在读取 `formation instance started` 后才做离线GDB/机器码核验并连接客体，期间客体继续运行，父Revoke与子Map可先完成。修复仅涉及 `tools/check-resources.py`、`tools/qemu_mmio_gdb.py`：该路线以 -S 冷启动，先核代码窗口、连接GDB并安装原硬件断点，再由GDB恢复；controller PID 随实际出生记录与目标准入记录传入，断点处核其真实owner和原scope。原目标rights、Map结果、PTE/DONE/退款判据及产品代码保持。单条复验的两个完整窗口证明该启动顺序能够建立原交错。

### 本轮结构检查与边界

主代理核对冷启动、离线准备失败、GDB失败和guest失败均沿原 finally 收专属进程组；没有新增控制线程、测试框架或产品同步协议。修改Python经AST语法检查、实际GDB脚本执行和diff检查；其余已通过的Rust/host检查不因工具改动重跑。按REVIEW的验收工具维护口径，不新增可选系统Review。

这五条是当时33条目录中的通过事实，剩余18条当时未运行。用户随后取消GDB自动验收并收缩为内核契约测试，目录、未提交工具修复及后续范围由下一节取代；不继续补跑旧目录。clock没有复发，不重开调查。

## 公开行为验收替代（2026-10-29）

用户明确决定移除依赖 GDB 内部状态/断点的自动验收，改用其他方法。此前 mmio-forced 首轮中 guest 两场完成退款与正常 Shutdown，但调试器晚连接超时、windows=[]，原现场 `artifacts/acceptance/20261029T103742.322669Z-99780/`，不能记为专项通过。为该观察器进行的未验证冷启动修改随工具删除，不作为交付能力。

本轮范围：用户随后进一步明确自动测试只覆盖内核，不接受跨模块组合与两小时级验收。删除13条GDB自动路线及专用脚本/组态/guest阶段假设，移出services与pm/RPC跨服务组合；transport收缩为tunnel，仅验证内核能力、映射几何、失败owner和生命周期。保留现有普通领域及显式压力，DMA共享/额度与IRQ退出改走普通runner。取消尚未实施的MMIO新竞态方案，复用普通MMIO检查。手动clock调查与离线栈分析不作为自动验收入口。产品代码、公共ABI与权限不修改。

替代合同：host按独立纯机制选择单测；QEMU按内核公开系统调用契约检查正常、拒绝、撤销、退出与资源收束。MMIO保留授权冲突、Seal/Revoke、RAM洞保值/旧VA复用、最后use关闭与进程退休；IRQ保留WaitOne/WaitSet使用者退出、原generation/来源DONE及退款，不宣称命中内部Waiting；DMA共享和额度的guest断言保持。旧精确阶段、内部借用/快照、强制分配/IPI/VM修复/reset拒绝注入及跨服务RPC/业务流不再由当前自动验收提供，历史结果保持当时事实。普通目录包含9条有限路线，压力与平台变体显式选择，不设置全目录必跑门，也不为未覆盖内部窗口新增产品hook或测试框架。

验证顺序：脚本/目录与guest check/clippy，分组/清理合同沿用既有runner测试；随后逐条运行受本次guest收缩与runner变更影响的普通内核路线，以及IRQ退出、DMA共享和额度，核结果后再下一条。通过后不重复，失败只修当前路线；不自动追加长压力/故障调查，历史clock不运行。本节拥有本轮清理与替代的进度、证据及剩余边界。

### 最终实施与验证

当前20条目录均配置为普通runner：9条普通内核、4条显式压力、4条额外平台与3条公开资源专项。删除13条GDB自动路线及14个专用宿主脚本/模块，移除其组态和指定阶段guest分支；services退出自动目录，transport改为tunnel，删除pm/RPC业务流、异构流读取任务及其测试侧owner/依赖。手动clock与离线栈分析保留，自动runner不导入GDB模块。新的 `qemu_support.py` 只拥有目录与进程组清理。

原mmio-forced失败还暴露macOS清理误报：直接子进程已成为zombie时，对只含它的组发信号返回EPERM。以独立短命进程复现后，在公共清理入口先poll回收直接子进程，再按原合同收组/核后代。复现前后证据 `artifacts/acceptance-sequential/cleanup-reap-check/{before.log,after.log}`；既有runner超时、中断、错误拒绝等14项测试通过。未新增测试框架或产品注入hook。

guest最终clippy：test_kernel通过（`artifacts/checks/20261029T104912.938299Z-2005/test_kernel.log`），test_target通过（`artifacts/checks/20261029T104952.235295Z-2189/test_target.log`）。期间check/clippy发现的旧流读取常量、已删owner/导入及残留注释均随清理消除，原失败日志保留。运行后仅统一测试诊断措辞和文档，没有改变判定或控制流。

| 单条选择 | 运行段 / 含增量构建 | result.json 所在目录 |
|---|---|---|
| tunnel | 0.292s / 4.794s | `artifacts/acceptance/20261029T105127.751946Z-2712/` |
| resources mmio | 0.855s / 4.158s | `artifacts/acceptance/20261029T105201.939009Z-2995/` |
| memory | 0.866s / 3.561s | `artifacts/acceptance/20261029T105310.958328Z-4007/` |
| ipc | 0.849s / 3.474s | `artifacts/acceptance/20261029T105326.582666Z-4275/` |
| process | 0.845s / 3.724s | `artifacts/acceptance/20261029T105347.713682Z-4563/` |
| time | 1.372s / 3.997s | `artifacts/acceptance/20261029T105359.935504Z-4805/` |
| resources irq | 0.295s / 2.940s | `artifacts/acceptance/20261029T105412.819178Z-5051/` |
| resources dma | 0.838s / 3.479s | `artifacts/acceptance/20261029T105424.421039Z-5288/` |
| platform fp | 0.297s / 3.207s | `artifacts/acceptance/20261029T105438.179448Z-5557/` |
| resources irq-exit | 0.295s / 2.932s | `artifacts/acceptance/20261029T105222.311801Z-3257/` |
| resources dma-shared | 0.846s / 3.509s | `artifacts/acceptance/20261029T105235.953570Z-3502/` |
| resources dma-quota | 0.862s / 3.648s | `artifacts/acceptance/20261029T105249.691810Z-3740/` |

以上均为release、默认THROTTLE=50，各条核领域完成、全部收集、Requested Shutdown、exit_code/runner_exit=0且无cleanup_error，之后核无QEMU/GDB残留。先核当前条再启动下一条，没有调用all或批量运行。9条普通调用含增量构建合计33.334s，QEMU段6.509s；3条资源专项另用10.089s/2.003s。统计不含人工核对、静态检查和编辑，缓存条件及覆盖均与旧两小时混合集合不同，不作等价性能比较。完整命令、逐条结果、汇总及差异在 `artifacts/kernel-acceptance-migration/`。

### 收口与覆盖限制

主代理按REVIEW检查最终边界、owner、失败/取消清理、过时入口及文档：自动链只调用普通runner，旧GDB目录、单一时序假设和RPC/服务消费路径已删；手动调查与自动清理分属各自模块；未改内核/正式服务、共享ABI或产品锁序。实现说明和构建手册已回填，文档引用、Python语法、Rust格式及diff核对通过。该交付是验收设施维护，不新增系统机制或设计Review登记。

当前承诺为有限的内核公开契约验收，上表12条通过；其余4条压力和4条额外平台未在本轮运行，也不要求为日常门补齐。已取消的精确内部窗口、故障注入和跨服务行为覆盖不以其他成功用例代替；业务能力按需人工验证。没有待接通的GDB替代设施或产品正确性修复；后续按实际内核改动选领域，本轮不继续追加运行。本轮已获用户授权整理提交，记录归档。
