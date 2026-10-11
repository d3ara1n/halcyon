# FAL midflight Copy 间歇失败

## 状态

**已按整体机制替换关闭并归档。** 2026-10-11 用户授权重新调查历史问题，并允许证明其存在基础已随机制迭代消失。本轮核对旧源码、19份保留失败日志、当前责任链及分版本验证，确认旧同步 Copy/pump、有限轮退休和消费者断言已整体删除，同语义消费者由新 CopyTask 接通并验证；具体关闭依据见[最终处置](#2026-10-11-历史证据复核与最终处置)。旧断言的具体失败子条件及原始根因仍未知，关闭不表示已定位或修复旧根因。本篇独占原始现象、复现和处置证据；新路径出现新失败时由[OAL/fs/service 主计划](../todo-2026-10-07-fal-service-redesign.md)按当前事实承接，不恢复已删除机制。

## 原始现象与当时判断

`virt-release`（core workload，release，4 hart）间歇失败：`test_fal` 在 `tests/test_fal/src/main.rs:2616` panic，错误为 “FAL consumer midflight Copy progress or owner mismatch”（断言源自 `:1919` 或 `:2084` 两处之一，尚未区分），init 随后报告 “FAL consumer exited before phase report”。失败前一条内核事件均为 `same-address-space multi-hart shootdown observed: 2 active harts`。

## 复现证据

各 20 次 `just virt-release`：

| 版本 | 失败 |
|---|---|
| `1d869cf`（运行时分层收敛前） | 4/20 |
| 收敛工作树（第 1、2 组） | 6/20、7/20 |

失败全部为同一报错。样本不足以区分两者失败率；用例只经 `spawn/join`，不经收敛中删除的取消路径。debug core/stress 路线较少复现。

## 2026-09-30 复测

`9c4abd7` 在两处断言失败分支加只读诊断后，`just virt-release` 0/20 失败（日志 `artifacts/flake/`，未随 Git）。代码未变而失败率由约 25% 降为 0，判定为对代码布局或宿主负载敏感的时序竞态；两处嫌疑（测试取消窗口不完整、Copy 失败进度语义）均属 Copy 语义，由 FAL 重新设计覆盖，诊断已撤回。仅当重写后仍复现且计数与 owner 正确时，再单独查多 hart shootdown。

## 可疑区间

- FAL midflight Copy 的进度/owner 观察与 provider 响应的时序（`srv_fs` 在失败后报告 `provider response abandoned after commit`）。
- 同地址空间多 hart TLB shootdown 与该 Copy 所用映射的发布/可见性。

## 最终处置前的接力（历史）

1. P4已按下节执行固定source/build fingerprint连续40 release与完整acceptance，两个运行门均已完成；原始失败根因仍未证实，当前只待协调者明确历史替代完成依据和归档处置，不重跑已关闭40门。
2. 若新消费者再次失败，按 [DEBUG-PLAYBOOK](../DEBUG-PLAYBOOK.md) 固定第一坏状态转换，区分 transported/已确认 accepted、实际注销与映射 owner。只有领域计数与 owner 正确时才回到 shootdown 调查。
3. 依据归属修复对应机制，不以放宽断言或重试掩盖；历史触发分支未区分和根因未证实的事实继续保留。

## 原完成标准

- 根因与修复写入本记录，受影响 impls 同步。
- `just virt-release` 连续 40 次无该失败；`just acceptance` 全路线通过。

## 2026-10-11 新 Copy 消费者承接

旧 Copy/CreatedTarget 和旧 test_fal midflight 运行体已整体替换。可复用 CopyTask 位于 libfs 客户端层，由 init 原 InitTask/ReadDuty 和同一 Dispatcher 实例化；源/目标会话、回复、来源与映射由原任务持续持有至清理 Complete。创建 Unknown 不重放、不按名字补偿删除，local_read/transported/destination_confirmed 分开建模；未领取目标终态时 accepted 只是零下界，不能把运输量冒充最终确认量。

新正式场景覆盖：非零 transported 后取消、实际目标前缀/上界、同 Dispatcher 另一 Copy 不受影响、目标改名/替换后仍沿具体会话、在途 secondary provider 停止及原 task 清理完成。正常成功核两端 Completed/Ok 和实际内容/计数相等；原 provider 停止责任持 uncertain 目标至 backing 退休，没有把清理责任交回短命调用者。源码与实现说明见[主计划 Copy 收口报告](../todo-2026-10-07-fal-service-redesign.md#copy-本批收口报告2026-10-11rubyraven-实施)及 [fs impl](../../notes/impls/fs.md#copy-组合与清理-owner)。

最终 core/stress/release 各一次正常 reset，无 panic/rejected，Copy 七条旧锚点均满足，两 provider 退款回归保留；完整 evidence 为 artifacts/copy/final-evidence.json，release 原 guest 为 artifacts/failed-acceptance-20261011-133745-34051.log。严格路线仍因7条其他旧 FAL 锚点缺失退出1，因此本次不是完整 acceptance 通过，也不是连续40 release。

历史根因依然未证实，不能由新进度语义或一次 release 正常倒推原断言失败的原因。本条继续活跃；owner 为主计划协调者/实施者，P4 锚点处置和最终消费者稳定后执行原连续40 release 与完整 acceptance 门。只有满足原完成标准才关闭；若新代码无法再证明旧根因，须在本条明确裁决替代完成依据，不默默降低门槛。

## 2026-10-11 连续40 release 门完成

固定源码及构建配置下串行执行40次 `just virt-release`，全部正常reset，panic/rejected为0、非FAL缺失0、Copy七项及同Dispatcher并发取消/迟到隔离/sealed活waiter/held业务期限场景均到达，两provider实际退款各次完整。40次严格退出均1，仅同一组待用户裁决的7个其他旧FAL锚点缺失；本条判定为连续40次未出现Copy/owner失败，不判定40条完整严格路线通过。聚合acceptance仍须在主计划锚点处置后执行。

source/build fingerprint：
`874a1942f104fe01def8a49ff871712f50e278a9e52ef0b6584186f3f7142ad1`。
范围及逐文件集合由 artifacts/p4/midflight40/source-manifest.json 固定；每轮启动前和完成后重新核对，循环期间未改运行源码或构建配置。kernel ELF SHA全40相同为 `29d725ac4fcca74513036e98c18db3b985f8d7688349cb1e327cb9d17ddaf590`。BootPackage容器SHA有40个不同值：每轮Justfile重新strip并打包含mtime的tar，容器hash不能当作编译源码发生变化的证明。未单独保留每轮payload成员字节作逐项比较，不宣称BootPackage逐字节固定；每轮真实metadata/SHA已保留。

逐轮包装、退出码、完整guest、脚本政策hash、reset/required/rejected/Copy门结果为 artifacts/p4/midflight40/run-01—40*、progress.json及final-summary.json；原guest从 artifacts/failed-acceptance-20261011-140738-48854.log 到 failed-acceptance-20261011-142338-91253.log。各次含构建24.01—27.05s，总986.91s；这是包装时长，不是纯guest性能数据。QEMU_SUMMARY不替代完整guest。runner48533已结束。

MellowPond认可连续门关闭；历史根因仍未被新证据证明，不倒推旧失败由哪个交错造成。随后只在test_fs增加Node/Grant/Binding饱和消费者，不修改Copy路径；主计划记录新场景与本fingerprint的边界，由最终acceptance覆盖回归，按协调裁决不重跑这40次。原记录保持活跃至完整聚合门和历史缺陷最终处置，不将“40无此失败”写成“历史根因已修”。

## 2026-10-11 完整聚合门完成

用户授权协调者判定7条旧锚点后，验证器只替换/合并现存场景输出并删除3项废止能力锚点，rejected和其他required不变；判定依据与精确差异证明由主计划P4记录拥有。`just acceptance`第三轮实际exit0，完整包装 artifacts/p4/acceptance3.log/.exit/.elapsed，784.97s含构建/全部路线，结构化证据 acceptance3-summary.json。debug stress、release core、sifive_u、nofd、Boot三故障、IPI、VM fault/repair全通过；5个完整guest缺失/rejected为0，Copy七锚点及并发取消、sealed活waiter、业务期限与实际物理退款回归齐全。sifive_u完整负载后明确reset failed由原allow-timeout规则收割，其余完整guest正常reset；VM repair是外部模拟异常诊断，不冒充完整负载或真实TLB陈旧竞争。

第一轮sifive120预算、第二轮nofd90预算实际截断证据保留；确认有界工作后各自默认翻倍，第三轮均跨原截断点并完成，未删场景。连续40门基于上节固定fingerprint，之后仅test_fs新增Q场景、不改Copy路径；最终聚合已覆盖这些新增消费者，按协调裁决不重跑40。两个运行门现均已完成。

历史两处断言的触发分支与根因依然未证实。本记录没有把“新实现运行门通过”改写为“旧根因已定位/修复”；原完成标准的根因项尚不能由现有证据满足。协调者拥有最终处置：在本条明确新消费者/整体机制替换能否作为替代完成依据后归档，保留原概率性证据及事实边界，不默默降低门槛。未commit/push。

## 2026-10-11 历史证据复核与最终处置

本轮基线为 `refactor/system-redesign` / `a89b899`，代码交付为 `a1ca124`，工作树起点干净。范围只包括旧 midflight Copy 和主计划 W05-04 的历史处置，不执行固定交付的独立 Review，不修改源码、验收器或时间/容量政策。旧源码以 `git show 1d869cf:<path>`、`git show 9c4abd7:<path>` 核实；本机快照和逐日志/hash结果为 `artifacts/historical-closure/`，长期结论和可重复核实位置由本节拥有。

### 原始现场可以进一步区分

1. 本机保留19份包含同一 mismatch 的旧失败 guest，全部已输出 `FAL2 independent Copy conflict and empty variants passed`，均未输出紧接取消场景之后的 `FAL2 independent Copy conflict, empty and cancellation variants passed`。`1d869cf` 的 `user/tests/test_fal/src/main.rs:2432—2447`、`325c0ee` 的对应调用顺序明确：前一标记→`exercise_copy_after_progress_cancel`→后一标记，provider 停止场景在更后面。因此这些现场命中的是 `1d869cf:1919` 的取消断言，不是 `:2084` 的停止断言。该判断不补造原20次批次的分母或失败率；本机没有完整批次清单，`artifacts/flake/` 的0/20诊断复测原件也已不存在。
2. 取消断言将原因类型、`target_bytes` 的4096—65536范围及 `has_unretired_owners()` 合并成同一句错误。具体哪个布尔子条件失败仍不能从日志恢复。旧 watcher 通过独立线程反复 Lookup 目标 `size>=4096` 后通知取消，不能冻结 Copy 所处相位；`copy_stream` 的 `check_cancel!` 和可取消 SourceFinish/TargetFinish 仍可能在不同相位响应。不能将日志位置确定误写成已找到精确竞态。
3. 原“失败前均有多 hart shootdown”不适用于全部保留现场：19份中3份没有该标记；`failed-acceptance-20260929-153755-73034.log:166—168` 则在标记后通过取消场景。旧 `os/kernel/src/task/proc.rs::report_multi_hart_shootdown` 只是 AtomicBool 门控的一次性观测。`provider response abandoned after commit` 在19份现场均出现在 panic 之后，成功取消运行的既有弃答场景也会输出。两类日志均不足以证明内核或 provider 是第一失败原因。原“加诊断后消失即判竞态”保留为历史判断，本轮只认可布局/时序敏感这一观察事实。

### 存在基础与当前责任链

| 旧结构与断言的依赖 | 当前实现与核实位置 |
|---|---|
| 同步 `Transport::copy_stream` 调用内创建私有 pump Runtime/WaitSet；`pump.rs` 以 `RETIRE_TURNS=28` 有限轮推进；`copy.rs::fail!` 只尝试一次 `retry_cleanup` 并丢弃其结果，然后返回失败 owner | `a1ca124` 删除旧 `client/copy/pump.rs`，整体重写 `client/copy.rs`。新 [CopyTask](../../user/libraries/libfs/src/client/copy.rs) 嵌入原 init 任务族/ReadDuty 和共享 Dispatcher，没有私有 pump Runtime |
| 失败返回即断言所有 owner 已退休，原因类型与进度范围也在同一断言内 | `CopyTask::outcome` 明确报告 `cleanup_pending`；`fail/cancel/advance` 沿原 txid 等完成，`retire` 先撤实际来源、等待回执，再释放映射，失败保回原 owner。只在责任已清空后到 Complete；init 等任务和运输收束后检查 `cleanup_pending=false` |
| 旧 `target_bytes` 来自 pump 的本地写入量；旧 CreatedTarget/条件Open/按名字补偿删除参与结果与清理 | 新 Progress 分别记录 `local_read`、`transported`、`destination_confirmed`，未取得目标终态只声明确认下界。CreatedTarget/CopyFailure/PumpRunFailure/PumpSnapshot/run_pump 及旧目标名在当前 os/shared/user Rust 中零引用，旧 test_fal 整体删除 |
| watcher Lookup 4096阈值、通知取消与同步调用返回之间依赖线程交错 | [正式 Copy 消费者](../../user/services/srv_init/src/filesystem/copy.rs) 先观察真实任务 transported>0，再用 ReadAt 确认目标正确非零前缀，随后直接取消原任务；核前缀内容/上界、并发另一 Copy 成功和最终清理。provider 停止场景单独保留，不以删除旧断言取消原行为承诺 |

结构证据消除了旧同步返回/有限轮退休/合并断言这一责任链；不能从中推导共享 VM、RPC 或新 Copy 的所有未来故障都不可能发生。共享机制没有已证实的旧 finding 需要靠恢复旧消费者继续追查；新路径若出现内容、计数或 owner 反例，按其第一坏状态另行定位。

### 分版本验证与关闭依据

- 原固定P4源码/build fingerprint的40次 release：本轮逐一重读完整 guest，并用各轮保存的验收政策核对应 Copy 标记、正常 reset、无 panic和两个 provider 退款，全部符合原门。严格退出1的7条旧非Copy锚点事实保留。C09新增的“目标实际 accepted 非零后取消”标记当时不存在，不能追加到这40次的覆盖声明。
- 最终C11基线：当前363份输入逐项SHA与 `artifacts/convergence3/source-fingerprint.json` 一致；5份完整guest的SHA与保存摘要一致，均有目标非零前缀取消、并发隔离、provider停止、Watch及最终预算清空。`just acceptance` 已有 exit0，相关host59项、check/七面clippy结果继续引用原证据，不以P4的40次冒充最终基线重复40次。
- 本次仅静态责任链核对、删除搜索、历史源码/日志与hash复核；没有新代码执行或故障注入，未重复通过的QEMU路线。impls已准确描述新Copy责任链，本轮无需修改。

依据用户允许“证明存在基础已消失”的授权，**将原完成标准的“定位旧根因并修复”替换为：旧失败责任链整体删除、保留的行为由新真实消费者接通、原重复运行门及最终组合验收有分版本证据。** 这三项均成立，因此关闭旧实现缺陷条目并归档，保留全部概率性证据和具体失败子条件未知的边界。主计划继续拥有当前机制的精确注入缺口及 W05-04，不因本条关闭而核销。
