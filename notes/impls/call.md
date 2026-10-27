# 内核调用实现

方向见 [call idea](../ideas/call.md)。系统调用 ABI 为 a7 调用号、a0–a5 参数，返回 a0 错误码与 a1 值。真实分发使用 Shell/Body、持久续体、普通 View/许可根与单次 capability 运输；任务进度与验收证据由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。

## 同步调用与输出错误

同步 handler 在 trap 中操作保存的 UserContext，sepc 前进 4，返回 Completed/Resume。异步完成续体先 detach 并取走唯一 Entry，再在 set_result 推进 sepc 4 并写返回值；未就绪重停驻和退出 detach 都不推进 PC。输入和输出经当前 AnySpace 的 translate 前置校验与受保护 SUM 直访；已发生副作用的最终输出使用 uaccess::deliver_output/deliver_bytes。复检失败只在当前执行 owner 的 Thread 中登记 output_fault/失败地址；业务锁全释放后，trap 调 publish_output_fault 冻结 UserAccess 终因并执行退出，不能把已成功业务报成可重试成功。

提交前还能回滚的表槽、队头和 VM 准备由各自 owner 归还。DebugPrint 的有界 SUM 读取先由执行者 Pool 付款 MetadataBuffer，复制/输出结束后归还；metadata拒绝与allocator OOM分别返回，临时存储不隐式占用未付款堆。SystemReset 是终局同步调用，固定宽参数与 capability 校验后进入单次平台调用；后端拒绝或异常返回才写错误。权限与 SBI 映射由 [internals.md](internals.md) 拥有。

## 等待与已提交操作

WaitPlan 位于 sched::HART_WAIT_PLANS 的 per-hart 内联槽，交接不分配。执行点离开 VM/hart 现场后才安装 Thread 内嵌节点与续体；Waiting 的唯一执行 owner 在成员槽中。来源只按 epoch 提示 pending，调度续体每次复检一次，正常返回与 Frozen 退出共用 detach。机制分别由 [task.md](task.md) 与 [ipc.md](ipc.md) 拥有。

WaitOne 返回当前电平/关闭/到期；Sleep 使用绝对期限。内部 VM 完成计划仅持私有 DONE 来源并标记返回单位值 a1=0，不携带异步用户输出指针。早到完成由安装后复检观察；线程冻结可以放弃回复，不取消独立 Work 已接管的发布/回滚/退款。

## 分步请求

[`task/request.rs`](../../os/kernel/src/task/request.rs) 拥有可让出请求的执行合同。请求按具体载荷 Layout 付款，Box 和 charge 存在 Thread 的独占请求槽；不保存 Thread/ProcessBody 强借用。syscall 先复制固定输入并移交请求，再返回 Requeue，尚不推进 sepc。调度器取得原线程执行资格，经过 Shell/VM enter 后在目标 satp 下调用一次 advance；Pending 保留请求并重新排队，Complete 释放请求后返回寄存器值，Wait 释放准备后转交独立 Work 的完成来源。

目标页表与内核页表共用内核高半区及当前栈，请求期间 IRQ 关闭且不嵌套调度。每次进入和恢复内核 root 都执行全量本地 SFENCE.VMA；恢复后才 RunPermit.leave、排队或安装等待。用户输出继续在当前线程执行 owner 下使用 translate+SUM，output_fault 在业务锁释放且回到内核 root 后发布。请求未完成时不返回用户态，不通过重复执行用户 ecall 推进内部状态。

Frozen reaping 在 Departure 前清空请求槽。请求的 Drop 归还未提交准备，已登记的 VM Operation 则把准备交给出生预付的回滚 Work；已提交工作不需要线程继续调用。当前消费者为普通/MMIO MemoryMap、MMIO/IRQ/DMA 子 control/use 派生及 DMA Pin/ReadSegments/Retire；资源祖先检查每轮一项，Busy 不在内核中自动重试。请求执行计入已授予的 U 类执行机会，不绕过轮转或另建内核线程。

## VM 真实消费者

公开 MemoryMap/MemoryUnmap/MemoryProtect 分发到 [vm/operation.rs](../../os/kernel/src/vm/operation.rs)。MemoryMap 的 MapCall 在 Thread 请求槽中持准备阶段，MMIO 的祖先检查可以跨轮次；Operation 出生并付款维护宿主/数组/完成来源，取得 VM access 后才受理实物资源。失败移交同记录 Rollback，成功进入 Publishing/fence/直接退休/空表清理。Outcome::Done只结束推进，最后live宿主销毁/退款和访问强根交回后才通知独立来源；持久等待续体复检观察，不为逐 use 创建子任务。

Map 按共享 ABI 解释 guard 字节数，结果槽由 UserWriteLease 预约。调用线程在自己的用户页表下、持 VM 锁写 payload 前缀，再 atomic release store committed cookie，随后 Views/ledger 的责任 Commit 必成，不再 uaccess。PTE/ledger 发布在 Work 中按预算推进；数据与空表退款都完成后才通知成功。准备错误可以先返回，已经接管的回滚责任仍自主执行，并用 VM access 防止空间提前退出。

ProcessCreate使用a0=Job、a1=Pool、a2=control rights、a3=结果地址；Control权限按显式请求校验类型上限，Builder固定MANAGE|GRANT|TRANSIT且不可复制；ProcessMap与ProcessStart返回内部完成计划。Builder/bootstrap复用统一操作链，Start的完成回执不携Body指针并阻止Frozen领先私有宿主退款。Tunnel的Endpoint来源通过普通Map，Create/Attach不持映射。具体责任与验证边界由[对象链记录](../../plans/archived/todo-2026-10-01-object-chain-kernel-integration.md)拥有。

## Fence 运输

[vm/remote.rs](../../os/kernel/src/vm/remote.rs) 的 Transport 随 ChangeWork/RetireWork 出生，每目标 hart 一个可复用槽。PTE/epoch 发布后将槽加入目标 FIFO 收件链，不分配、不竞争全局容量；FIFO 避免持续新批次插队使早期事务饥饿。目标在安全点按预算取槽，执行本地翻译/指令同步、归还槽，再 release ack；最后 ack 方锁外唤醒 owner，之后不再访问旧批次。最终确认包含“fence 完成且槽已归还”，零目标使用同一完成谓词立即唤醒。

发布者释放全部业务锁后发门铃；门铃失败不丢FIFO pending，idle双检禁止带待收槽WFI。active/epoch属于VmShell，enter/leave与当前epoch复检协作；它与Shell冻结gate分别拥有事实。Work实际host与transport按布局一起付款，运输是宿主字段，不保留平行固定全局槽表、ReservedBatch或旧请求消费者。
