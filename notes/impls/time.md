# 公共时间实现

时间前置已完成，实现提交为 `c6e0a84`。运行期协作停止作为其必要前置已完成，固定记录见 [runtime-stop 前置](../../plans/archived/todo-2026-09-13-runtime-stop-prerequisites.md)。本篇只记录实际时间机制；执行与服务期限的交付证据见 [执行前置档案](../../plans/archived/todo-2026-09-13-service-runtime-prerequisites.md)，FAL 的交付证据由其专题档案拥有。

`shared/erhino_shared/src/time.rs` 定义显式 Infinite/At Deadline、ClockSnapshot 和 ClockGeometry。平台 frequency/origin 是换算唯一真值，elapsed 向下取整、有限期限向上取整，中间用 u128。最大期限从最后可读且可编程的 tick 推导：可读 offset 上界为 `((2^64 × frequency) - 1) / 1e9`，再与 `u64::MAX-1-origin` 取较小值；由该 tick 的 elapsed 反推上限，避免低频下准入期限的触发 tick 超过纳秒表示范围。`resolution_ns` 是名义 timebase tick 的 ceil 纳秒单位，不承诺可观察更新频率或唤醒精度。

`os/kernel/src/clock.rs` 的 ClockState 拥有 raw/ns 高水位和不可逆 failed。读取先取得 prior，再采样 raw；相对已知 prior 回退至少两 tick 时失败。origin 是已发布的样本，一 tick 合法差异归启动时间零。raw 转换失败或硬件 epoch 末端同样锁存失败，返回前再次检查并发故障；之后所有读均返回 ClockRange，没有恢复路径。

`os/kernel/src/sbi.rs::read_time` 使用 `fence rw,i; rdtime; fence i,rw`，asm 不使用 nomem/readonly/pure：固定 Zicsr「CSR Access Ordering」把 CSR read 归 I，内存 acquire 不替代 R→I 排序，CSR 后发布也有显式 I→RW 排序。GeometryCell 唯一 boot 写入，release/acquire 发布 geometry 与 origin，之后只读。

[`shared/timer_queue`](../../shared/timer_queue/src/lib.rs) 使用侵入式 AVL 树，节点地址稳定，期限相等时以节点地址区分。登记/取消/弹出不分配，沿树高修复 parent/height；first 是同一队列同步域维护的最小值缓存，peek 为 O(1)。owner_slot 只定位队列，cancel_if 锁内复检载荷 epoch；到期先彻底摘链/清 owner 再交载荷，没有 arena token/generation。

Thread 出生内嵌 TimerNode，per-hart 队列载荷为强 Thread/epoch；service_expired 锁外只提示，不提交 Timeout。执行 owner detach 注销 timer，旧消息不能取消新 epoch。Thread HintTarget 已接成员 pending/epoch，正常续体/Frozen reap 共用注销入口；旧 wait_context/request 的 token 消费未删除，内核不能构建。用户态 WorkQueue（deadline/retry）、Runtime 来源退休与 RPC Dispatcher 各自出生预付 pin Box，取消/到期后才 drop，容器析构先排空 timer 再释放节点。Runtime 预算与实际 charge 共用包含独立 timer 存储的来源字节数。

`clock/selftest.rs` 使用独立 ClockState 和编排的 raw 样本，验证 origin/skew、已采旧值迟到发布、真正回退、不可逆失败、epoch 末端与并发失败复检，不污染正式 STATE。Ready 前启动调用已接入，正常验收要求 Clock state anchor；这些不是实际多 hart 弱序的全面验证。

本轮证据：shared host 的 Deadline 编码、低频/极频/接近硬件末端和取整边界通过；virt core 的隔离状态检查与既有消费者通过，七面 lint 通过。日志 `artifacts/check/time-{shared-host,clock-core,clock-clippy}.log`。rustc 为 `c54751567b19c4ceb08b0412d83529c2568cba8b`，LLVM 23.1.0；debug 实际反汇编 `time-read-time-disassembly.log` 确认 `fence rw,i / rdtime / fence i,rw`，不是仅凭 asm 文本推断。artifact 被 Git 忽略，异机须重跑。

时间前置已完成：初始就绪/At0 优先、park/安装延期、Send 末端到期及 moves/once/Delivery 退款、真实满箱阻塞超时/已投递期限、用户线程因果读取、ClockState 失效与在途 timer 的停止边界、release/板型验证均已通过。剩余唯一时间范围是跨硬件 epoch 连续时间，本篇第 8 节已有独立触发条件。RPC 阶段/迟到回复/offer/Outbox 属执行前置/业务，不混入公共时钟交付。验收可靠性改进继续独立延期，完整 stress 未宣称通过。
