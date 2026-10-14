# 进程与线程生命周期收敛

> 状态：已完成并归档；基线 `a592e38`。本记录拥有本轮收敛范围、删除清单、验证与结构检查；ThreadRegistry/owner 的后续实施由[用户内存 owner 计划](todo-2026-09-14-user-memory-owner-lifecycle.md)拥有。方向归 [task](../../notes/ideas/task.md)，当前实现归 [task 实现](../../notes/impls/task.md)与[启动实现](../../notes/impls/startup.md)。

## 承诺与边界

本轮同时收敛以下八项，不留下过渡字段、旧描述符写法或第二套状态真值：

1. Process 与 Thread 遵循共同的 Create（挂起且资源备齐）→ Start（只发布）→ 离场形状；Process 冻结终局原因并在 Drain 后发布 CLOSED，Thread 离场后发布 DONE。Control 只观察或管理，不充当资源 owner。
2. Builder 放弃与挂起线程最后 Control 关闭分别进入其对象唯一的终止/离场入口；Created 线程不经直接丢弃的旁路。核对重复关闭与终止游标的唯一消费权。
3. ProcessKill 保持 Accepted/AlreadyTerminating 两值线性化事实，自杀不返回；未来 ThreadKill 使用同一返回形状，本轮不新增 ThreadKill。
4. ProcessTerminate、ProcessAbort、ProcessKill、fault 和 Builder 放弃在内核通过同一带原因终止入口冻结首因；syscall authority 保持独立。
5. Building ThreadCreate 安装的 child-local Control 是启动 grants 通道的一项。StartupBlock 的 Handle 数组追加该值，EntryThreadDescriptor 只含其零基索引与栈 reservation/usable 范围。提升启动块及 section 版本，同步 bootstrap、libprocess 和接收端；删除 descriptor control 字段、INVALID 占位回填与旧校验。入口采用尚由 Registry 切面接续。
6. 已成功的内核操作在输出用户 VA 写回失败时，统一冻结 `Faulted(UserAccess)`，记录发起线程 tid 和失败 VA；对各带输出 syscall 审查提交点、预检/回滚边界与异步完成方，不将尚未提交的输入或预检错误混作已提交输出故障。
7. BindMemory、BindExecution、Map、Write、Grant、Building ThreadCreate 全部登记同一 Building lease/`building_ops`；Start 以自身登记且计数为 1 为提交门，终止等待已获准操作，不另设计数。
8. 入口栈与运行期栈采用相同的用户态 region owner/cleanup 模型。当前 libthread Registry 不存在，本轮只交付 descriptor 足以重建普通 region owner 的准确契约；Registry 实际采用和 cleanup 由原计划施工，不伪称本轮完成。

无进程/线程退出码；ThreadStart 使用 ThreadControl MANAGE（WAIT 只观察）；ProcessStart 不分配。这些已定裁决不变。非目标为 ThreadKill、Registry/libruntime 实施、TLS、unwind、detach、专职 reaper 和无关资源机制重构。

## 责任链与删除门

- shared ABI 是唯一线格式 owner；builder/launcher 把 ThreadCreate 目标句柄加入 StartupBlock Handle 数组，descriptor 仅引用索引。bootstrap 的四个既有固定索引不变，额外的入口 Control 由显式索引引用，不从 `last()` 猜测角色。接收端同时验证索引、句柄几何与栈区间；目标 HandleTable 和出生块失效由 ProcessDrain 收束。
- 终止首因在 lifecycle 锁内一次冻结，锁外待办仍由同一 driver 运行；Created 离场通过 ThreadDeparture 解除成员与发布 DONE。关闭控制权、终止游标与 Start 竞争须只由获胜者请求一次离场。
- 输出故障在持业务锁时只登记冻结事实和预付待办，释放锁后由既有 trap/完成方驱动；已提交副作用由目标 owner 收束，不能对用户报告可重试成功。
- 删除旧 descriptor control、INVALID 回填写、针对 ThreadCreate/Grant/Receive 的不同完整性失败语义、BindExecution 裸 Building 快照门、重复终止包装和来源特判。具体代码审查以最终调用链为准。

## 验证门

- shared host 校验 grants 索引合法/越界、伪造 offset、版本与栈几何；libprocess host 跑现有单测与集成测试，真实入口索引交付由 QEMU core/stress 覆盖。
- 内核现有自检/竞速核对 BindExecution 与 Start/Kill 的截止、Created 关闭/终止只发布一次 DONE、Kill 两值与首因稳定；针对已提交输出的失败检查 tid、UserAccess 与用户 VA。
- 责任链接通后运行 `just check`、`just clippy`，按 [构建手册](../BUILD-AND-TEST.md)选择 core/stress QEMU 与跨机制组合验收，保留完整日志并标注开发证据与最终验收的区别。
- 交付前按 [REVIEW](../REVIEW.md)查 bug、锁序、owner、所有输出点、旧字段/过渡路径及 ideas/impls 同步；逐项记录通过与未覆盖项。

## 完成判定与证据

八项中的内核和 shared/launcher 契约已接通：ProcessBindExecution 持 lease 并拒绝重复绑定；StartupBlock v3 / section v2 将入口 Control 作为 grants 一项，bootstrap 显式使用第 4 项；普通 launcher 映射后创建线程，再一次写完整块。ThreadControl capability 引用覆盖表项、复制和 transit，最后释放与 Start/终止竞争唯一 Created 离场；WAIT-only 引用保留观察，长期持有者相应持有挂起资源。`ThreadDeparture::request` 幂等，ProcessKill 保留两值返回。同步输出和异步 WaitMany/ProcessDrain 输出失败统一冻结发起 tid/用户 VA 的 Faulted(UserAccess)，Receive 未提交队头/槽位仍回滚。旧 descriptor control 与 INVALID 回填、逐调用完整性失败、BindExecution 裸快照门、Control `started` 重复状态以及 race harness 的旧 kill code 均已删除。

已定裁决：ThreadControl 的存活计数不区分 rights。即使只剩 WAIT-only 引用，也不因当前没有持 MANAGE 的调用者而提前 abandon；持有者不启动而持续占用挂起线程准入，属于其自身的资源使用，系统不另设回收政策。唯一的自动放弃条件仍是该对象在全系统的最后一份 capability 引用消散，且线程仍处于 Created；进程终止另按统一终止路径收束。

开发定位曾在 `just virt` 触发两个旧 selftest 前提：park 后读取已移走的 Caller 线程，以及 Receive 写回失败后仍试图重试；分别改为 park 前保存 tid、断言退款和 Faulted 后收束。`virt-stress` 的首次 15/16 失败源于锤对已绑定 Builder 重复执行 BindExecution；删去重复绑定后，Start/Kill 才按真正提交门竞争。早期失败日志见 `artifacts/dev/lifecycle-virt-dev.log`、`artifacts/dev/lifecycle-virt-dev-2.log`、`artifacts/dev/lifecycle-virt-dev-3.log`、`artifacts/dev/lifecycle-virt-stress.log`；均已由后续同路线通过覆盖。clippy 旧断点还要求修正 libprocess 集成测试签名、rinlib unsafe 文档和 srv_init race cause-only 断言。

最终证据：`just check`（`artifacts/dev/lifecycle-kernel-check-final.log`）；shared host 28 项（`shared/artifacts/dev/lifecycle-shared-host-final.log`）；libprocess host 单测 8 项和集成 4+4 项（`user/artifacts/dev/lifecycle-libprocess-host-final.log`）；`just acceptance` 全七面 clippy、stress 16/16、release core、sifive_u core、nofd core 和 boot-failure 全部通过（`artifacts/dev/lifecycle-acceptance.log`）。聚合后补充了同一内核 selftest 的“终止先于最后 Control 释放”顺序，重跑 `just check`、`just virt` 和 `just clippy` 通过（`artifacts/dev/lifecycle-virt-final.log`、`artifacts/dev/lifecycle-clippy-post-selftest.log`）；随后将 init 的 `HANDLE_COUNT` 收敛为实际五项，shared host、`just check`、`just virt` 和七面 clippy 均再通过（`shared/artifacts/dev/lifecycle-shared-host-post-count.log`、`artifacts/dev/lifecycle-kernel-check-post-count.log`、`artifacts/dev/lifecycle-virt-post-count.log`、`artifacts/dev/lifecycle-clippy-post-count.log`）。其余平台行为未变，不重复路线。测试由脚本收割，检查无残留 QEMU/GDB 进程。

交付前结构检查：所有内核输出写点已盘点；用户输入/预检错误仍正常返回，成功后输出失败才 fault；业务锁内只缓存待办、锁外执行。CapabilityRef 只计授权表项/在途引用，不以 Arc 强计数代替；Created 领取与 Start/termination 同锁，结果义务未清不提前 DONE。检索无旧入口字段、回填写入、重复 Building 计数或不适用的退出码；ideas、impls 与当前计划已同步。未覆盖的是 Registry 尚未实施，入口栈 owner 一次采用/cleanup 留在上述用户内存 owner 计划，其完成门为从 grants 索引领取 Control、按 reservation 重建普通 region owner、复用运行期线程 cleanup 并验证退出/取消路径。本轮不实现 ThreadKill，也不为未来 syscall 预留运行时机制。提交与 push 均另行取得授权。
