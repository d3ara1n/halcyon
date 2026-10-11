# FAL midflight Copy 间歇失败

## 状态

历史间歇性缺陷，根因尚未证实；2026-09-30复测见下文。2026-10-07对象链修复的完整acceptance/release已通过，但一次通过不能消除本记录的概率性证据。当前不再把它记为已经复现的全局验收阻塞或设备设计前置；由[FAL/服务重设计分案](todo-2026-10-07-fal-service-redesign.md)按Copy语义及现存消费者承接。本记录继续独占原始现象、复现和缺陷处置证据，不另建平行缺陷记录。

## 现象

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

## 下一步

1. P4已按下节执行固定source/build fingerprint连续40 release与完整acceptance，两个运行门均已完成；原始失败根因仍未证实，当前只待协调者明确历史替代完成依据和归档处置，不重跑已关闭40门。
2. 若新消费者再次失败，按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 固定第一坏状态转换，区分 transported/已确认 accepted、实际注销与映射 owner。只有领域计数与 owner 正确时才回到 shootdown 调查。
3. 依据归属修复对应机制，不以放宽断言或重试掩盖；历史触发分支未区分和根因未证实的事实继续保留。

## 完成标准

- 根因与修复写入本记录，受影响 impls 同步。
- `just virt-release` 连续 40 次无该失败；`just acceptance` 全路线通过。

## 2026-10-11 新 Copy 消费者承接

旧 Copy/CreatedTarget 和旧 test_fal midflight 运行体已整体替换。可复用 CopyTask 位于 libfs 客户端层，由 init 原 InitTask/ReadDuty 和同一 Dispatcher 实例化；源/目标会话、回复、来源与映射由原任务持续持有至清理 Complete。创建 Unknown 不重放、不按名字补偿删除，local_read/transported/destination_confirmed 分开建模；未领取目标终态时 accepted 只是零下界，不能把运输量冒充最终确认量。

新正式场景覆盖：非零 transported 后取消、实际目标前缀/上界、同 Dispatcher 另一 Copy 不受影响、目标改名/替换后仍沿具体会话、在途 secondary provider 停止及原 task 清理完成。正常成功核两端 Completed/Ok 和实际内容/计数相等；原 provider 停止责任持 uncertain 目标至 backing 退休，没有把清理责任交回短命调用者。源码与实现说明见[主计划 Copy 收口报告](todo-2026-10-07-fal-service-redesign.md#copy-本批收口报告2026-10-11rubyraven-实施)及 [fs impl](../notes/impls/fs.md#copy-组合与清理-owner)。

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
