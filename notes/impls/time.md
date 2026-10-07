# 公共时间实现

公共时间由共享ABI的Deadline/Geometry、内核clock高水位与timer消费者组成。实现证据与范围由[时间档案](../../plans/archived/todo-2026-09-13-runtime-stop-prerequisites.md)、[执行前置档案](../../plans/archived/todo-2026-09-13-service-runtime-prerequisites.md)及[对象链修复档案](../../plans/archived/review-2026-10-07-object-chain.md)拥有；本篇描述当前机制。

`shared/erhino_shared/src/time.rs` 定义显式 Infinite/At Deadline、ClockSnapshot 和 ClockGeometry。平台 frequency/origin 是换算唯一真值，elapsed 向下取整、有限期限向上取整，中间用 u128。最大期限从最后可读且可编程的 tick 推导：可读 offset 上界为 `((2^64 × frequency) - 1) / 1e9`，再与 `u64::MAX-1-origin` 取较小值；由该 tick 的 elapsed 反推上限，避免低频下准入期限的触发 tick 超过纳秒表示范围。`resolution_ns` 是名义 timebase tick 的 ceil 纳秒单位，不承诺可观察更新频率或唤醒精度。

`os/kernel/src/clock.rs` 的 ClockState 拥有 raw/ns 高水位和不可逆 failed。读取先取得 prior，再采样 raw；相对已知 prior 回退至少两 tick 时失败。origin 是已发布的样本，一 tick 合法差异归启动时间零。raw 转换失败或硬件 epoch 末端同样锁存失败，返回前再次检查并发故障；之后所有读均返回 ClockRange，没有恢复路径。

`os/kernel/src/sbi.rs::read_time` 使用 `fence rw,i; rdtime; fence i,rw`，asm 不使用 nomem/readonly/pure：固定 Zicsr「CSR Access Ordering」把 CSR read 归 I，内存 acquire 不替代 R→I 排序，CSR 后发布也有显式 I→RW 排序。GeometryCell 唯一 boot 写入，release/acquire 发布 geometry 与 origin，之后只读。

[`shared/timer_queue`](../../shared/timer_queue/src/lib.rs) 使用侵入式 AVL 树，节点地址稳定，期限相等时以节点地址区分。登记/取消/弹出不分配，沿树高修复 parent/height；first 是同一队列同步域维护的最小值缓存，peek 为 O(1)。owner_slot 只定位队列，cancel_if 锁内复检载荷 epoch；到期先彻底摘链/清 owner 再交载荷，没有 arena token/generation。

Thread出生内嵌TimerNode，per-hart队列载荷为强Thread/epoch；service_expired锁外只提示，由执行owner复检并提交Timeout。正常续体/Frozen reap共用detach注销，旧消息不能取消新epoch。WorkQueue的业务deadline与执行退避节点共用一份稳定TaskTimers分配，仍各自登记/取消；Runtime来源退休与RPC Dispatcher各持预付pin节点，先取消/弹出所有登记再释放存储；Runtime来源费用包含实际timer布局。非Gate stop/unregistered回调前兑现已到期责任、回调后同步同一预付业务timer；Finalizing只继续来源退休，不重新武装业务期限。

`clock/selftest.rs` 使用独立 ClockState 和编排的 raw 样本，验证 origin/skew、已采旧值迟到发布、真正回退、不可逆失败、epoch 末端与并发失败复检，不污染正式 STATE。Ready 前启动调用已接入，正常验收要求 Clock state anchor；这些不是实际多 hart 弱序的全面验证。

当前时间只覆盖一个硬件epoch内可读且可编程的有限范围，不提供跨epoch连续时钟。ClockState失败/范围尽头锁存不可恢复事实，runtime_stop令继续运行或随后唤醒的hart在安全点停止；不靠周期timer掩盖失败。公共时钟不拥有RPC offer/阶段/迟到回复政策；这些绝对期限由实际消费者拥有。
