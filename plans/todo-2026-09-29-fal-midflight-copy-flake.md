# FAL midflight Copy 间歇失败

## 状态

待定位。该缺陷早于运行时分层收敛阶段，阻塞 `just acceptance`，排在设备实现之前处理。

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

1. 区分触发的是 `:1919` 还是 `:2084`，记录观察到的进度值与 owner。
2. 固定第一坏事件：在失败区间补充只在该用例生效的诊断，或按 [DEBUG-PLAYBOOK](DEBUG-PLAYBOOK.md) 取现场；判断是测试期望、FAL 协议还是 shootdown 可见性错误。
3. 依据归属修复对应机制，不以放宽断言或重试掩盖。

## 完成标准

- 根因与修复写入本记录，受影响 impls 同步。
- `just virt-release` 连续 40 次无该失败；`just acceptance` 全路线通过。
