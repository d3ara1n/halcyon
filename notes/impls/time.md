# 公共时间实现

时间前置 #14 已完成，分支为 `task/fal-service-capabilities`；提交仍待本轮用户授权。运行期协作停止作为其必要前置已完成，固定记录见 [runtime-stop 前置](../../plans/archived/todo-2026-09-13-runtime-stop-prerequisites.md)。本篇只记录实际时间机制，不宣称执行/FAL 交付；后续服务期限由 [执行前置](../../plans/todo-2026-09-13-service-runtime-prerequisites.md) 消费。

`shared/src/time.rs` 定义显式 Infinite/At Deadline、ClockSnapshot 和 ClockGeometry。平台 frequency/origin 是换算唯一真值，elapsed 向下取整、有限期限向上取整，中间用 u128。最大期限从最后可读且可编程的 tick 推导：可读 offset 上界为 `((2^64 × frequency) - 1) / 1e9`，再与 `u64::MAX-1-origin` 取较小值；由该 tick 的 elapsed 反推上限，避免低频下准入期限的触发 tick 超过纳秒表示范围。`resolution_ns` 是名义 timebase tick 的 ceil 纳秒单位，不承诺可观察更新频率或唤醒精度。

`os/kernel/src/clock.rs` 的 ClockState 拥有 raw/ns 高水位和不可逆 failed。读取先取得 prior，再采样 raw；相对已知 prior 回退至少两 tick 时失败。origin 是已发布的样本，一 tick 合法差异归启动时间零。raw 转换失败或硬件 epoch 末端同样锁存失败，返回前再次检查并发故障；之后所有读均返回 ClockRange，没有恢复路径。

`os/kernel/src/sbi.rs::read_time` 使用 `fence rw,i; rdtime; fence i,rw`，asm 不使用 nomem/readonly/pure：固定 Zicsr「CSR Access Ordering」把 CSR read 归 I，内存 acquire 不替代 R→I 排序，CSR 后发布也有显式 I→RW 排序。GeometryCell 唯一 boot 写入，release/acquire 发布 geometry 与 origin，之后只读。

`clock/selftest.rs` 使用独立 ClockState 和编排的 raw 样本，验证 origin/skew、已采旧值迟到发布、真正回退、不可逆失败、epoch 末端与并发失败复检，不污染正式 STATE。Ready 前启动调用已接入，正常验收要求 Clock state anchor；这些不是实际多 hart 弱序的全面验证。

本轮证据：shared host 的 Deadline 编码、低频/极频/接近硬件末端和取整边界通过；virt core 的隔离状态检查与既有消费者通过，七面 lint 通过。日志 `artifacts/check/time-{shared-host,clock-core,clock-clippy}.log`。rustc 为 `c54751567b19c4ceb08b0412d83529c2568cba8b`，LLVM 23.1.0；debug 实际反汇编 `time-read-time-disassembly.log` 确认 `fence rw,i / rdtime / fence i,rw`，不是仅凭 asm 文本推断。artifact 被 Git 忽略，异机须重跑。

时间前置已完成：初始就绪/At0 优先、park/安装延期、Send 末端到期及 moves/once/Delivery 退款、真实满箱阻塞超时/已投递期限、用户线程因果读取、ClockState 失效与在途 timer 的停止边界、release/板型验证均已通过。剩余唯一时间范围是跨硬件 epoch 连续时间，本篇第 8 节已有独立触发条件。RPC 阶段/迟到回复/offer/Outbox 属 #15/业务，不混入公共时钟交付。#17 继续独立延期，完整 stress 未宣称通过。
