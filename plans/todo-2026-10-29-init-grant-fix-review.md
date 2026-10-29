# init 出生转授权修复独立 Review

状态：可选、未执行。不阻碍后续验收，不因登记自动运行测试或审查。

固定提交：`fe0ed70`（父提交 `2314f40`）。工作记录由 [init 计划](todo-2026-10-28-init-formation.md#首次运行阻塞2026-10-29) 与[逐条验收记录](archived/todo-2026-10-29-acceptance-groups.md#逐条运行与修复2026-10-29)拥有。

## 范围与目标

重点检查 `srv_init::executor::start` 的 report sender/stop owner：源能力必须持 GRANT，ProcessGrant 交给目标的权利仍为 REPORT_RIGHTS/STOP_RIGHTS；成功移交与失败关闭保持唯一 owner，不能扩大服务接收权限或弱化内核检查。

同提交的 Bash 3.2 参数组装与 FP 配额作为实际验收前置核对，不扩展成全系统重审。FP metadata 采用既有 1 MiB 单实例工作集，原 256 KiB 小于当轮账本最低存储 394624 字节；不声称此数是所有未来布局的精确容量保证。

## 现有证据与边界

- srv_init check/clippy 通过。
- 十个普通 release 组逐条通过，含 task 出生、三宿主 Ready/Stopped、实际正常收集与 Requested Shutdown。
- 结果与每轮 ELF/boot package 身份在 `artifacts/acceptance-sequential/20261029T095802.960248Z-95550/summary.json`；完整日志见工作记录。
- 未验证 init 全部运行期负路径、压力、额外平台或 GDB 精确窗口。不得由正路径通过宣称这些场景已覆盖。

如选择执行，按 [REVIEW](REVIEW.md) 固定上述提交与责任链报告 findings；修复在同份报告接续，不复制工作记录或自动触发下一轮 Review。
