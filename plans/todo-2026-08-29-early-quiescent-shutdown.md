# 提前 quiescent 停机调查计划

> 【待实施计划】发现于批一 review 收口验证（2026-08-29）。首份现场基于
> `275e4f1`，该提交已包含线程模型改造，故现有证据不能判定引入批次。当前
> 状态：现场已抓、根因未定性。

## 现象

竞态矩阵 kill-vs-exit round 1 期间，系统在负载仍存活时判定 quiescent 并停机：

```
race kill-vs-kill passed (code wins 4/0)
pid 24 thread 1 reaped: 1 switches, lifespan 8 ms
[pid 25] hammer target: exiting with 769        ← round 1 靶自杀
pid 25 thread 1 reaped: 2 switches, lifespan 15 ms
[Sched] system quiescent (no waker), powering off; 234158 frame(s) free
QEMU acceptance failed: missing anchor: race matrix acceptance passed: 10/10
```

此刻 init（pid 1）应阻塞在 `h.report(0)` 等锤回执，双锤（pid 18/19）仍活着，
但调度器判定「无 waker」直接 SRST 停机。

## 判据（区分本问题与真挂死）

**帧数是最快的判据**：正常终态 virt 4 核 `248843 frame(s) free`；本问题
`234158`（差 14685 帧 = 双锤 + 靶未回收）。低于 240000 即负载存活时停机。

平台差异决定表象——**同一根因两种脸**：

| 平台 | SRST | 表象 |
|---|---|---|
| virt | 可用 | QEMU 退出，锚点缺失 → acceptance 失败 |
| sifive_u | 不可用 → `hart::park()` | QEMU 永不退出 = **看起来完全就是挂死** |

因此本问题**可能就是批一 review §B 那个「无法定性的 sifive_u 挂死」的真身**。
review 报告 §B3 已把「IPI 投递到入睡 secondary hart」标为头号嫌疑窗口，但当时
判断方向反了（认为挂死应呈现为永挂而非误停机，见该报告 §B3 末句）。

## 复现

- 配方：`just virt`（4 核，debug，默认节流 50%）反复跑，读 `powering off; N frame(s)`；
- 频率：极低且不稳定。首次发现时 8 轮撞 1 次、基线 6 轮撞 1 次；收口验证阶段
  连续 26 轮（12+8+6）未再复现——**属于典型的窗口极窄竞态，不是稳定复现路径**；
- 失败现场已由 `tools/qemu-acceptance.sh` 自动保留（失败即 `mv` 到
  `artifacts/failed-acceptance-<时间戳>-<pid>.log`，成功才删）；
- 首次现场副本：`artifacts/evidence-2026-08-29-early-quiescent.log`
  （artifacts/ 不入 git，关键片段见上方「现象」节）。

## 调查方向（按优先级）

1. **quiescent 谓词与 waker 所有权的竞态**：判定「无 runnable、无 timeout owner」
   与「锤线程刚被唤醒但尚未入册」之间是否存在窗口。重点看 `sched.rs` 静默判定
   遍历全部域的时刻，与 `wake_one` 清/置 idle 位、`enqueue` 发布 Ready 的线性化
   关系。review §B3 审计过三个不变量成立（清 idle 位后必再 pick、置 idle 位后
   has_ready 双重检查与 enqueue 同锁线性化、waker 必醒着）——但那次审计的假设是
   「挂死」，对「误判静默」这一侧未做同等强度的论证；
2. **消息投递唤醒丢失**：靶 exit 后 init 的 `h.report(0)` 等待与锤发 report 的
   Mailbox 唤醒是否有窗口让 waker 记账消失；
3. **quiescent 谓词的保守性**：谓词对「队列有线程但目标 hart 醒不来」无感知
   （review §B3 已记为已知限制）。若本问题是该限制的实例，则修复方向是让谓词
   把「存在非 Dead 进程且其线程处于 Waiting」纳入判定，而不是只看 runnable。

## 装备

- 帧数快速判据（见上）；
- `just virt` 循环 + 失败日志自动保留（已落地）；
- `THROTTLE=100 just virt` 全速缩短单轮，提高单位时间轮次；
- GDB：`qemu -s` + `thread apply all bt`（gdbstub 改变时序，复现率可能下降）；
- 若需探针：`sched.rs` 静默判定点、`wake_one`、`enqueue` 三处计数。

## 完成标准

- 根因定性并有机制级修复（不接受「加重试/放宽谓词」式补丁）；
- virt 与 sifive_u 各 20+ 轮无复现，帧数恒为正常终态值；
- 结论入档：方向进 `notes/ideas/task.md`（若涉及静默停机公理）或
  `notes/impls/task.md`「调度」（若为实现层竞态）；
- 若确认与 review §B 的 sifive_u 挂死同源，更新该报告 §B 的定性并归档。
