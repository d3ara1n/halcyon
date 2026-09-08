# 单调时间与 RPC 全调用期限

> 当前待设计/实施。方向由 `notes/ideas/{rpc,wait,call}.md` 拥有；当前实现见 `notes/impls/{rpc,ipc,call}.md`。本计划是有限 RPC deadline 的唯一行动真值，不改变 WaitMany 现有相对超时 ABI，也不把取消伪装成超时。

## 现状与缺口

`librpc::Caller::call(timeout_ms)` 先经 `rinlib::ipc::message::send_blocking` 投递，再用同一参数等待回复。满 Mailbox 时 `send_blocking` 使用无限 WaitMany，因此有限 `timeout_ms` 只约束回复阶段；服务仍存活但队列长期满时，调用可以永久停在投递阶段。简单地让投递和回复各等待一次相对 `timeout_ms` 会把总上界扩大到两倍，重试或伪唤醒还会继续重置预算。

当前用户态没有可读取的公共单调时钟，无法在调用入口冻结绝对 deadline 并在每个等待点计算剩余时间。`shared::time::Timestamp` 只有类型别名，不构成时钟来源、单位或回绕契约。

## 目标契约

- 有限 RPC deadline 覆盖从调用开始到回复接受的完整本地操作，包括 MailboxFull 背压、ReplyPort 等待、Receive 与 framing 接受；无限调用仍显式使用无限值。
- 用户态入口把相对 duration 一次转换为绝对单调 deadline；后续每个阻塞点只使用剩余预算，不因重试、竞争或伪唤醒重置总期限。
- deadline 到期只表示调用方停止等待。请求若已成功入箱，业务结果仍是“是否执行未知”；库不得自动重试有副作用调用。
- 投递失败必须准确报告请求和随附 capability 是否仍在本地；投递成功后的 timeout 必须废弃 ReplyPort，迟到回复不能污染下一调用。
- 单调时间的单位、精度、溢出、最大可表达期限和跨 hart 一致性形成公开契约；不能直接暴露未经约束的平台 timebase 或固件编码。
- Cancel 仍是未来用户态协作协议，不下沉为内核强取消，也不与 Timeout 合并。

## 设计前置

先依据 `references/CONTRACTS.md` 固定 RISC-V time/counter 与 SBI TIME 的适用边界，并调查成熟系统的公开单调时钟与 deadline ABI。冻结以下选择后再编码：

1. capability-free `MonotonicNow` 系统调用、受控用户计数器读取或其它可替换时间 seam；
2. ABI 时间单位及 checked duration/deadline 运算；
3. rinlib `Deadline` 类型与“无限”表示；
4. Mailbox 阻塞投递接受 deadline 的接口，以及 Caller 对同一 deadline 的贯穿方式。

公共时间 seam 应服务未来 timer、服务监督和协议 deadline，但本计划只交付其最小读取契约与 RPC 消费者，不扩张为日历时间、时区、定时器对象或通用异步运行时。

## 自然实施顺序

1. 外部契约取证并冻结单调时间 ABI、单位和溢出边界；同步 `notes/ideas/`。
2. shared/kernel/rinlib 纵向接入最小单调时间读取，覆盖跨 hart 单调性与非法参数。
3. 为 Mailbox 阻塞发送增加绝对 deadline 版本；保留明确无限 wrapper，不复制等待循环。
4. `librpc::Caller` 在入口冻结 deadline，投递与回复等待消费同一剩余预算；同步错误类型和 owner 归还。
5. host 模型覆盖阶段耗时组合、临界到期、伪唤醒与 checked overflow；QEMU 覆盖满箱直到 timeout、投递后 timeout、迟到回复隔离和下一调用成功。

## 完成标准

- 任意有限 Caller 调用的本地等待总时长不因阶段数或重试次数重置；满箱服务不能绕过 timeout。
- timeout 前后 request、send-once、extra moves 与 ReplyPort 的 owner 状态有唯一可查询结果，无泄漏或重复关闭。
- 现有无限 RPC/FAL 路径语义不变；无业务自动重试或内核取消旁路。
- shared、kernel、rinlib、librpc、相关消费者与 ideas/impls 同步，host debug/release、七面 clippy、virt/virt-release 通过；涉及并发投递时追加 stress。

## 触发与顺序关系

该项不阻塞多页 Tunnel/RNL2，可与数据面独立排队；在对外承诺有限 RPC timeout、正式跨进程 FAL 或需要统一服务 deadline 前必须完成。它不并入已归档 A–E Review program，实施提交后单独生成未来 Review 计划。
