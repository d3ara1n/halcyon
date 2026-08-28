# 竞态矩阵覆盖增强

## 背景

step 9 生命周期多核竞态矩阵已收口（10/10 全绿），但分布观察暴露单向窗口：`kill-vs-exit` 与 `kill-vs-fault` 恒为 kill 胜（0/4 靶胜）——锤等枪后立即 syscall，靶等枪唤醒后还要走用户态自杀/fault 路径，天然慢一步。终因冻结的幂等仲裁（先到者冻结、后到者幂等返回成功）只观察到「kill 先到」一侧；「Exit/fault 先冻结、kill 后到幂等」一侧被断言允许但从未出现，即未真正锤到。`kill-vs-abandon` 反向同理（恒 abandoned 胜）。

## 待办

- 给 `kill-vs-exit`/`kill-vs-fault` 增加靶先行的时序变体，使 Exited/Fault 终因至少有胜出轮。候选方案（实施时定）：
  - 锤侧变体：奇数轮锤等枪后先 `sys_sleep(1)` 再 kill，让靶的自杀/fault 先线性化；
  - 或靶侧变体：`spawn 后不等枪直接自杀/fault`，锤 kill 打向「退出路径已在途」的靶（窗口更宽，且覆盖 REAPABLE 电平建立后 kill 幂等的另一侧）。
- `kill-vs-abandon` 同法补 Killed 胜出轮（锤 close 前加延迟变体）。
- 断言不变：终因仍限合法集合，分布只观察。

## 实施收口注记（2026-09）

- 选型：**锤侧延迟变体**。`Cmd.aux`（原预留字）转正为「执行前延迟（毫秒）」——锤等枪后先 `sys_sleep(aux)` 再执行指令，零线协议结构改动。三个场景的奇数轮延迟 10ms：kill-vs-exit/fault 的 kill 延迟（靶 Exit/fault 先冻结，kill 后到幂等）；kill-vs-abandon 的 close 延迟（kill 先冻结，close 后到只协助收束），abandon 轮次 2→4。
- 延迟量确定过程：1ms 不够——靶从枪响到退出冻结的路径（wait ecall → take ecall → debug UART 写 → exit ecall）实测 1–2ms 虚拟时间，kill 恒落在靶打完 debug 行之前；排除 THROTTLE 干扰（`THROTTLE=100` 同样 0/4）与 sleep 提前醒（`expires_after_ms` deadline 直装本地时钟，非 tick 量化）。10ms 有 5–10 倍余量。
- 分布观察：virt 上 exit/fault 稳定 2/2（偶 1/3，延迟轮偶被 kill 抢回），abandon 2/2（release 3/1）；hetero 弱域 hart 少、锤靶串行化，单轮分布可单向但跨轮两侧均出现（本质是队列次序决定胜负）。断言不变：终因限合法集合 + Dead 收束 + 无泄漏，分布只观察。
- 改动面仅用户态验证负载三处：`libprocess/src/race.rs`（aux 语义）、`srv_hammer/src/main.rs`（枪后延迟）、`init/src/race.rs`（延迟轮编排）。内核零改动。
- 全验证线：virt 多轮 / virt-release / virt-hetero / virt-nofd / sifive_u 全部 10/10，host 单测全绿。

## 完成标准

virt 多轮跑中 exit/fault/abandon 场景的双侧终因均有胜出记录（分布行非 0/N 或 N/0 单向），全验证线保持 10/10。
