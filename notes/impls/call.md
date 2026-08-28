# 内核调用实现

方向见 [`../ideas/call.md`](../ideas/call.md)。系统调用 ABI 为 a7 调用号、a0–a5 参数，返回 a0 错误码与 a1 值。

## 同步调用

同步 handler 在一次 trap 内完成：结果写入 UserContext、sepc 前进 4，dispatcher 返回 Resume。输出地址先校验，副作用后的最终写回统一使用 `uaccess::deliver_output`；若同进程另一线程在两次 AddressSpace 临界区之间拆除输出映射，复检失败冻结调用进程 `(Fault, StoreAccess)`，不把已发生副作用与错误返回混合。

handler 返回 Completed 后，dispatcher 再检查 lifecycle。若 syscall 期间另一 hart 已冻结终止，或 deliver_output 触发 Fault，出口改为 Killed，不再返回用户态。

## 异步调用

dispatcher 只登记 `WaitPlan` 到 HartLocal park 槽并返回 Wait。调度循环在当前线程离开执行点、清 active 后调用 `park_publish` 安装 `WaitContext`。

WaitPlan、WaitContext、TimeoutRegistration、订阅清理与 rejected-park 竞态由 [`ipc.md`](ipc.md) 唯一记录。调用层只区分两种交付：WaitMany 把结果或 MemoryNotAccessible 写回保存现场，Sleep 在相对超时到达时写回成功。终止 Abandoned 不交付结果。

dispatcher 的 Wait 出口不提前改为 Killed，终止竞态由等待安装路径吸收。

## Remote Call

当前没有参数帧或通用 remote-call queue。已落地的 IPI 只作门铃：调度唤醒和进程终止向目标 hart 置 SSIP，目标检查自身待办。
