# 竞态矩阵覆盖增强

## 背景

step 9 生命周期多核竞态矩阵已收口（10/10 全绿），但分布观察暴露单向窗口：`kill-vs-exit` 与 `kill-vs-fault` 恒为 kill 胜（0/4 靶胜）——锤等枪后立即 syscall，靶等枪唤醒后还要走用户态自杀/fault 路径，天然慢一步。终因冻结的幂等仲裁（先到者冻结、后到者幂等返回成功）只观察到「kill 先到」一侧；「Exit/fault 先冻结、kill 后到幂等」一侧被断言允许但从未出现，即未真正锤到。`kill-vs-abandon` 反向同理（恒 abandoned 胜）。

## 待办

- 给 `kill-vs-exit`/`kill-vs-fault` 增加靶先行的时序变体，使 Exited/Fault 终因至少有胜出轮。候选方案（实施时定）：
  - 锤侧变体：奇数轮锤等枪后先 `sys_sleep(1)` 再 kill，让靶的自杀/fault 先线性化；
  - 或靶侧变体：`spawn 后不等枪直接自杀/fault`，锤 kill 打向「退出路径已在途」的靶（窗口更宽，且覆盖 REAPABLE 电平建立后 kill 幂等的另一侧）。
- `kill-vs-abandon` 同法补 Killed 胜出轮（锤 close 前加延迟变体）。
- 断言不变：终因仍限合法集合，分布只观察。

## 完成标准

virt 多轮跑中 exit/fault/abandon 场景的双侧终因均有胜出记录（分布行非 0/N 或 N/0 单向），全验证线保持 10/10。
