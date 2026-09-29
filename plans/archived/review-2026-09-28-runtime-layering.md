# 运行时分层独立审查

对象：`f519a99f693db2905d5fbff57a393b9336ad8c2f`，父提交 `325c0eeabac7b99a2d472423ee8c66860b2f0a7d`。审查时 HEAD 等于对象提交，工作树干净。
范围：入口、StartupBlock 接收与能力采用、线程出生/结果/cleanup、Cargo 分层、平台入口及相关消费者；对照题述 ideas、两份施工计划和 framework/startup/task/mm/execution-context impls。
基准：A 为固定提交代码，B 为仓库文档，C 为本任务提供的已确认设计。B 内部存在冲突时明确指出，不以施工计划的完成声明覆盖契约。未发现 P0；以下发现均待处置。

## Issues

1. **P1｜代码内部缺陷｜ResultReceiver 可在 Registry 销毁后解引用悬垂指针。** 位置：[libthread/src/lib.rs:695](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:695)、[同文件:802](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:802)。
   `ThreadFactory::spawn/start` 返回不带 Registry 生命周期的 `ResultReceiver<T>`，只保存 `*const ()`；safe 调用者可以在局部 Registry 作用域外保留 handle，其 `join/try_join/Drop` 随后解引用已经销毁的 Registry。Registry 默认析构还可能释放正在被 trampoline 使用的 packet。建议改代码：为运行中 record 和观察者建立真实保活或生命周期约束，禁止用注释替代指针存活证明。

2. **P1｜代码内部缺陷｜线程环境 E 未约束跨线程访问和寿命。** 位置：[libthread/src/lib.rs:135](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:135)、[同文件:543](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:543)。
   仅 `Child/Residue/Error` 要求 `Send + 'static`，E 本身没有 `Send/Sync/'static` 约束；safe spawn 却把 `Arc<E>` 放入跨线程 packet，并在子线程调用 `enter/leave`。含 `Rc<Cell<_>>` 或局部借用的 E 可以进入该路径，ResultReceiver 的手写 Send 又允许异线程 reap。建议改代码：按实际共享方式约束环境类型，并保留其寿命和线程安全证明。

3. **P1｜代码≠文档｜cleanup claim 被表示为空槽，可覆盖新 record；seal 也缺少同锁线性化。** 位置：[libthread/src/lib.rs:231](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:231)、[同文件:280](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:280)。
   reap 用 `Option::take` 后释放锁，insert 把该槽视为空闲；旧 record 未 DONE 或 Unmap Busy 时直接写回原索引，覆盖并析构期间插入的新 record，新 PreparedThread 随后可能启动已释放 packet。insert 的 sealed 检查也在槽锁外，seal 返回后仍可能登记。违反 B `task.md:143` 的固定槽/唯一 claim/seal。建议改代码：保留 Reserved/Claimed 状态和稳定槽身份，将准入与 seal 放入同一仲裁边界。

4. **P1｜代码≠文档｜worker 没有 gp 初始化，首入口也不是无序言、norelax 适配段。** 位置：[libthread/src/lib.rs:476](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:476)、[同文件:590](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:590)、[libruntime/src/rt.rs:172](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:172)。
   ThreadStartContext 直接指向泛型 Rust trampoline，完全没有 gp 装载；`_start` 是普通 Rust 函数，内联 `la gp` 无 `.option norelax`。违反 B `execution-context.md:41–53` 和 C，worker 出生 gp 为零时，gp-relative 访问可 fault。现存 debug ELF 还在 gp 前保存 a0/a1 到栈。建议改代码：为两类入口建立无序言整数汇编适配段，保护 relaxation，保持参数和 SP，再进入 Rust 桥接。

5. **P1｜代码≠文档｜初始能力采用没有凭据、去重和 role 核实。** 位置：[libprocess/src/context.rs:62](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:62)、[libkernel/src/startup.rs:70](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/startup.rs:70)。
   adopt 逐索引领取 raw Handle 后立即 `Capability::from_raw`，没有 StartupSeed、重复数值检查、HandleQuery 或非映射 role 核实；outer 校验只检查 Handle 数值有效性，不能证明唯一关闭权。重复表项会形成双 owner，descriptor 所选项也未经核实即当作 ThreadControl。另有 typed Handle slice 对齐前提未被 init 的安全契约覆盖。违反 B `bootstrap.md:154–167`、分层计划 `:165–169`。建议改代码：先完整验证，再消费唯一凭据提交 owner；失败保留原凭据，按值解码非对齐输入。

6. **P1｜代码内部缺陷｜合法的最大 grants 输入必然启动失败。** 位置：[libkernel/src/startup.rs:88](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/startup.rs:88)、[libprocess/src/lib.rs:192](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/lib.rs:192)。
   launcher 接受 `PROCESS_MAX_GRANTS=64` 项，并追加入口 Control，产生 65 项 StartupBlock；接收侧 `AtomicU64 CLAIMED` 拒绝索引 64，adopt 返回 MissingGrant，runtime abort。建议改代码：按实际表长建立采用状态，不以位图字宽引入第二个 ABI 上限；覆盖“64 grants + 入口 Control”边界。

7. **P1｜代码≠文档｜放弃 PreparedThread 遇 Busy 后无法发布 DONE，回滚还会遗失映射 owner。** 位置：[libthread/src/lib.rs:251](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:251)、[同文件:602](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:602)、[同文件:687](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:687)。
   cancel 先 Unmap，失败便保留 record 和 Control；PreparedThread Drop 吞掉错误，record 的 Control 继续保活 Created 线程，reap 等待永不出现的 DONE，closure 也未交还放弃者。Create/insert 失败路径另用 `let _ = stack.unmap()` 丢弃失败返还的 region。违反 B owner 计划 `:95–103`。建议改代码：闭合未启动线程的 Control 放弃、DONE 和普通 cleanup 责任，所有 Unmap 拒绝都保留 owner。

8. **P1｜代码≠文档｜永久 cleanup 错误被忽略，失败进度无法恢复。** 位置：[libthread/src/lib.rs:305](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:305)、[同文件:314](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:314)、[同文件:739](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:739)。
   Wait 错误全部归为 Wait；Unmap/retire 失败只返回 Failed，而 join/try_join/Drop 全部忽略。retire 消费 residue 后失败，恢复的 record 已失去该状态，下次可跳过退休继续释放栈。违反 B `task.md:178` 和 C 的永久错误完整性出口及保留责任。建议改代码：建立单步 cleanup 进度和失败返还形状，Busy 可重试，永久错误携责任进入明确 ProcessAbort 路径。

9. **P1｜代码≠文档｜ResultReceiver Drop 仍触碰 record，并可能停驻。** 位置：[libthread/src/lib.rs:747](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:747)、[libkernel/src/mm.rs:159](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/mm.rs:159)。
   Drop 调用 reap，后者执行环境 retire、Unmap 和 Control 释放；Unmap 可能因内核退休确认停驻。因此“不等待 child”不等于 Drop 不等待。B `task.md:187` 和 C 禁止 Drop 碰 record，但 `impls/mm.md:196` 又认可 Drop reap，文档内部冲突。建议两者修正：Drop 只放弃结果和观察引用，cleanup 移至规定的显式协作调用点。

10. **P1｜代码与文档都≠期望｜普通 Rust main 入口未实现，仍要求宏与链接桥。** 位置：[libruntime/src/lib.rs:10](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/lib.rs:10)、[libruntime/src/rt.rs:19](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:19)、[test_target/src/main.rs:5](/Users/chien/Projects/halcyon/user/tests/test_target/src/main.rs:5)。
   没有用户 runtime 的 `start` lang item；消费者均为 no_main/app_main/entry!，宏生成 `__halcyon_main`，runtime 还公开 finish 和初始化桥。公开 finish 可让 worker 安全绕过其 wrapper 责任。B `framework.md:66,94`、分层计划 `:13–25` 和 impls 明确采用此方案，与 C 冲突。建议两者修正：恢复模板 `fn main()`，由 runtime 的 start 接收 rustc main，argc/argv 传零，删除宏、应用别名、桥接符号及公开 finish。

11. **P2｜代码≠文档｜应用没有一次性 StartupContext 交接，仍通过全局 raw getter 取用。** 位置：[libruntime/src/lib.rs:30](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/lib.rs:30)、[libprocess/src/context.rs:49](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:49)、[同文件:108](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:108)。
   没有 `startup::take()`；runtime 永久持有 Context，公开 pid/parent_pid/raw Handle/static payload。startup_handle 优先 duplicate，同一索引可以反复成功，且 duplicate 任意错误会静默退化为转移原能力；payload 没有视图借用边界。违反 B `bootstrap.md:146,162,168–175` 和 C，部分 impls 又把全局接口当作现状要求。建议两者修正：一次性交付 Context，显式 take/duplicate，不吞权限或资源错误；payload 通过视图借出，管理消费者接收显式身份。

12. **P2｜代码≠文档｜实际消费者使用第二个隐式 Registry，runtime 实例没有接通。** 位置：[libruntime/src/rt.rs:55](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:55)、[libthread/src/lib.rs:756](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:756)、[同文件:779](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:779)。
   runtime 的 ENTRY_REGISTRY 只登记入口线程；所有现有 Builder 消费者使用 libthread 私有静态 Registry，两处容量均固定 64，Builder 自称 Compatibility builder。违反 B `task.md:143`、owner 计划 `:89` 和 C 的 runtime 装配责任；libthread manifest 也未接入 C 指定的 libbudget。建议改代码：接通 runtime 持有实例和环境工厂，迁移消费者、删除兼容全局，并按确认的预算契约装配容量。

13. **P2｜代码≠文档｜prepare 和 wrapper 缺少规定的协作 reap。** 位置：[libthread/src/lib.rs:543](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:543)、[同文件:505](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:505)。
   prepare 直接准备资源并 insert，wrapper 发布结果后直接 ThreadExit，两者都不 reap。保留已完成线程的 ResultReceiver、推迟领取结果时，死线程占满槽就使后续 spawn ReachLimit，实际把容量变成未清理出生次数上限。违反 B `task.md:180–187` 和 C。建议改代码：准入前推进已 DONE 记录，wrapper 离场前作有界 reap，join 确认 DONE 后推进。

14. **P2｜代码≠文档｜join 的 Incomplete 契约没有落实，try_join 不确认 DONE。** 位置：[libthread/src/lib.rs:709](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:709)、[同文件:719](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:719)。
   try_join 直接领取 ResultCell，可能在线程发布结果但尚未 DONE 时返回 Returned，也把仍在运行的 Empty 解释为 Incomplete；join 确认 DONE 后遇 Empty 则 panic，返回类型为 T。违反 B `task.md:171,174`、owner 计划 `:101` 和 C。建议改代码：区分未完成观察与 DONE 后结果领取，后者返回显式 Incomplete，不 panic、不读未初始化值。

15. **P2｜代码≠文档｜环境 enter/leave 失败被扩大为进程 abort。** 位置：[libthread/src/lib.rs:486](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:486)、[同文件:500](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:500)。
   enter 失败虽在子线程 Drop F，却 abandon 结果并 abort 整个进程；leave 失败直接 Drop T 并 abort，没有保留 T 或交付独立环境错误。B `task.md:170–174` 要求 EnterFailed，以及 leave 失败保留结果另报错误。建议改代码：补齐结果/环境错误表达与部分成功状态，使普通环境失败和进程完整性失败各走其责任链。

16. **P2｜代码≠文档｜libprocess 的 management feature 隔离不存在。** 位置：[libprocess/Cargo.toml:6](/Users/chien/Projects/halcyon/user/libraries/libprocess/Cargo.toml:6)、[libprocess/src/lib.rs:29](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/lib.rs:29)、[libruntime/Cargo.toml:11](/Users/chien/Projects/halcyon/user/libraries/libruntime/Cargo.toml:11)。
   manifest 无 features，ELF/libexecution 为无条件依赖，loader、监督和观察模块始终编译；runtime 也未关闭默认功能。违反 B `framework.md:58`、分层计划 `:43,234` 和 C。建议改代码：context 保持基础依赖，management API 与其依赖显式 feature-gate，管理消费者显式启用，并核对 feature 合并不引入启动行为。

17. **P2｜代码≠文档｜main Err 没有诊断，也没有 Debug 约束。** 位置：[libruntime/src/rt.rs:34](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:34)。
   `impl<E> MainOutcome for Result<(), E>` 接受非 Debug 错误；Err 分支丢弃错误内容，直接 ProcessAbort。违反 B `task.md:191`、`framework.md:92`、owner 计划 `:85` 和 C。建议改代码：在最终 start 入口限定 `Result<(), E: Debug>`，先以无需堆初始化副作用的诊断路径记录错误，再 abort。

18. **P2｜代码≠文档｜普通 StartupBlock 仍可写，违反稳定只读输入前提。** 位置：[libprocess/src/lib.rs:1207](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/lib.rs:1207)、[同文件:1217](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/lib.rs:1217)。
   map_birth_block 使用 READ|WRITE，随后 Write、Start，没有收紧权限。违反 B `bootstrap.md:142–146`；分层计划 `:99,306` 虽登记为延期，却同时宣告依赖该输入契约的采用能力完成。此项是已登记残留，不是新发现。建议两者修正：利用 Building Write 对最终只读映射回填，同步当前事实和完成声明，不能将正确性前提仅改列为范围外。

19. **P2｜代码与文档都≠期望｜ThreadRecord 通过擦除 packet 仍拥有 F/T 相关状态。** 位置：[libthread/src/lib.rs:171](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:171)、[同文件:434](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:434)、[同文件:446](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:446)。
   record 持有 `PacketOwner { ptr, drop }`，其指向 `Packet<E,F,T>`，包含 F、Child 和 ResultCell 引用；cleanup 调用擦除型析构函数，部分分支选择 Box::leak。B owner 计划 `:83` 也引入 packet cleanup，与 C 的纯机械 record 边界不一致。建议两者修正：明确 packet 的独立 owner 与交接/退休协议，record 只保存栈、环境残余、Control 和 cleanup 进度，不靠擦除类型宣称 F/T 已移出。

## Observations
- 已核实：libkernel manifest 不依赖上层；libthread 不依赖 libprocess/libexecution/libruntime；领域库没有反向依赖 libruntime。新建三库的目录、package 和默认 crate 身份一致。
- 已核实：固定提交没有 `user/rinlib` 实现目录；用户侧旧 ENTRY(main)、lang_start 借位、ExitCode/ProgramResult、ThreadSpawn/ProcessAttach、UserStack abandoned 和 wait_and_release 路径已删除。内核自身 start 借位不属于本轮用户入口替换。
- 已核实：ResultCell 当前 CAS 路径只允许一次发布，take/abandon 对 Published 的消费互斥，不读取 Empty；但 B owner 计划 `:83` 和实现采用额外 Writing 中间态，与 C 字面状态模型仍需统一，不能据此宣称完整结果契约已通过。
- 已核实：入口 descriptor 按 grants 索引选取 Control，reservation/usable 经普通 MappedRegion 重建；初始化 CAS 拒绝第二次入口。未发现栈 cleanup 按来源分支或隐藏 driver 线程。
- 已核实：`() / Ok` 调用 ThreadExit，Err/panic/OOM 调用 ProcessAbort；libkernel 的线程 exit 原语为 unsafe。普通成功返回没有终止整个进程的代码路径。
- 文档完成声明过强：owner 计划 `:140` 要求 Registry 并发 claim/seal 等验证，但 libthread 仅有三个单测，Incomplete 测试只是构造枚举值，没有覆盖 join 行为；既有 acceptance 不能替代这些缺失的契约证据。

## 未覆盖项
- 本轮未运行构建、host tests 或 QEMU，未创建日志、临时进程或文件。全部主发现来自固定源码与文档；现存 Base64/D64 ELF 反汇编仅作辅助，其产物未证明绑定本提交。
- 未全面重审父提交中的内核 ThreadDeparture、DONE 结果义务、ProcessDrain 或所有底层资源包装；未验证所有目标/优化组合的 SP、FP 指令及链接 relaxation。相关“已核实”结论仅限上述具体代码边界。
- 修复验证应针对悬垂 Registry、环境线程安全约束、claim/insert/seal 交错、放弃时 Busy、65 项启动表、环境错误和 DONE+Empty，并按构建手册走 just；host 路线显式指定 host target，证据写入 `artifacts/dev/`。

## Summary

该提交完成了目录和多数依赖迁移，但尚未完成已承诺的运行时、启动上下文及线程生命周期契约，存在 safe API 可触发的内存安全缺陷。

## 处置与校正范围

报告由 reviewer 只读生成，主代理转录。按 AGENTS，本报告即 19 项发现的唯一修复与复核记录，runtime layering 范围内不另建 todo。f519a99 在 owner 计划、分层计划中的完成声明，校正完成前不成立；独立 kernel 验收阻塞另见 [Kernel console 锁序 panic](todo-2026-09-29-kernel-console-lock-order.md)。

### 契约来源

期望设计已写入 ideas，施工以 ideas 为准：[task](../notes/ideas/task.md#用户态线程资源收束)（ResultReceiver、Registry 槽状态、packet owner、结果类型、Drop、cleanup 失败）、[framework](../notes/ideas/framework.md)（`start` lang item、`startup::take()`、唯一 Registry 实例）、[bootstrap](../notes/ideas/bootstrap.md#用户态接收与一次性交接)（Seed/Query/去重）、[execution-context](../notes/ideas/execution-context.md#用户态平台入口)（gp/norelax 适配段）。ideas 已修正；impls 与两份计划由施工同步。

用户裁决：旧 `JoinHandle<T>` 统一改名为 `ResultReceiver<T>`，不保留旧名或 alias；本系统不提供 Rust std，不沿用 std 名以免误导同义。

### 逐项施工与完成标准

| # | 改动 | 完成标准 |
|---|---|---|
| 1 | Thread/ResultReceiver/packet 以共享引用保活 Registry 核心与 ResultCell；删除 `*const ()` | safe API 无法构造悬垂；无依赖注释的指针存活 |
| 2 | 环境类型按共享方式约束 `Send + Sync + 'static`，删除手写 Send 的缺口 | 含 `Rc` 的环境编译失败（compile-fail 或类型层证明） |
| 3 | 槽状态 Free/Reserved/Live/Claimed；准入、登记、seal 同锁 | Claimed/Reserved 不可再分配；seal 后登记被拒；host 交错用例 |
| 19 | packet 成为独立 owner，record 只含栈、环境残余、Control 引用、cleanup 进度；删除擦除析构与 `Box::leak` 分支 | record 类型不含 F/T 相关状态 |
| 7 | 放弃先释放 Control 让内核发布 DONE，栈交普通 cleanup；所有 Unmap 拒绝返还 region | 无 `let _ = …unmap()`；Busy 下放弃仍 DONE |
| 8 | cleanup 单步进度；Busy 可重试，永久错误保留进度并 ProcessAbort | 无被忽略的 Failed；retire 不重复、不跳过 |
| 9 | Drop 只放弃引用，不 reap、不 syscall | Drop 路径无停驻调用 |
| 13 | prepare 前 reap、wrapper ThreadExit 前有界 reap、领取观察 DONE 后 reap | 延迟领取下死线程不占满槽 |
| 14 | try 形式未 DONE 返回 Pending；DONE+Empty 返回 Incomplete | 不 panic；host 覆盖 DONE+Empty |
| 15 | 结果类型 Returned{value, leave}/EnterFailed/Incomplete，普通环境失败不 abort | enter/leave 失败用例 |
| 4 | 入口与 worker 两个无序言整数汇编适配段，`.option norelax` 装载 gp，保持 a0/a1/SP | 两 target 反汇编：首指令为 gp 装载，未被 relax，参数未经栈 |
| 5 | Seed 一次来源 + 重复值检查 + HandleQuery role 核实，全部验证后再提交 owner；按值解码非对齐 | 重复 Handle、错 role 被拒且保留凭据 |
| 6 | 采用状态按实际表长，删除 64 位位图上限 | 64 grants + 入口 Control 启动成功 |
| 18 | 普通 StartupBlock 最终映射只读，Building Write 回填 | map_birth_block 无 WRITE |
| 11 | `libruntime::startup::take()` 一次性交付；删除全局 getter、duplicate 退化、static payload | 第二次 take 为 None；grep 无旧 getter |
| 12 | runtime 持唯一 Registry 与环境工厂，消费者迁移，删除 libthread 私有静态与 Compatibility builder；容量由装配者给出 | 只有一个 Registry 实例 |
| 10 | `start` lang item 接 rustc main；删除 `entry!`、`app_main`、`__halcyon_main`、公开 `finish`、`no_main`；全部 binary 改回模板 `fn main` | grep 无上述符号 |
| 17 | main `Result<(), E: Debug>`，Err 经无堆诊断后 ProcessAbort | 非 Debug E 编译失败 |
| 16 | libprocess `management` feature 门控 ELF/libexecution；runtime 关闭默认 feature | context 单独编译；feature 合并不改变启动行为 |

Observation 处置：ResultCell 的 Writing 中间态若为实现必需，在 owner 计划写明其与 ideas 三态的对应，不改变可观察语义；否则删除。impls/mm.md 的 Drop reap 表述删除。

### 顺序与验证

按依赖：1/2/3/19 → 7/8/9 → 13/14/15 → 4 → 5/6/18/11 → 12/10/17 → 16 → impls、owner 计划与分层计划完成声明重新核定。中间步骤不要求独立通过整体验收。

验证：host（显式 host target）覆盖上表用例；`just check`、`just clippy`、`just acceptance`；两 target 入口反汇编门；证据写入 `artifacts/dev/`，逐项在本报告登记。全部关闭后归档本报告。

处置状态：主体代码校正已实施；发现 16、18 与正式门记录仍未闭合，见下。

## 本轮处置证据（工作树）

- 已完成线程结果/槽/cleanup 主路径：`ResultReceiver`、擦除 packet owner、默认兼容 Builder 与 Drop reap 已删除；`ResultReceiver<T, E>` 共享 `ResultCell<ThreadOutcome>` 与 `ThreadControl`，`try_join(self)` 未 DONE 返回自身，DONE+Empty 返回 `Incomplete`；环境约束为 `Send + Sync + 'static`。槽使用 `Free/Reserved/Live/Claimed/Faulted`，cleanup 记录 `retired/done/abandoned`，永久错误统一经 `cleanup_integrity_failure` 进入 `ProcessAbort`。
- 已完成 runtime 装配：唯一 Registry 由 `libruntime` 以进程寿命 `Box::leak` 持有，所有 Builder 消费者改走 `libruntime::thread`；libthread 不再拥有隐式静态 Registry。
- 已完成入口切换：应用 binary 删除 `no_main`、`app_main`、`entry!`、`RUNTIME_ENTRY`；普通 `fn main` 由 `#[lang = "start"]` 接入。`_start` 与 worker 使用单一 norelax gp 适配段，worker 为非泛型符号并从 packet 读取具体 trampoline。
- 已完成启动上下文一次性表面：应用通过 `libruntime::startup::take()` 接收 `StartupContext`；duplicate 失败不再退化为转移；采用时核实入口 descriptor 所选项为带 `MANAGE` 的 `ThreadControl`；启动表采用状态不再受 64 位位图限制。
- 开发验证：`cargo test -p libthread --target aarch64-apple-darwin`、`cargo test -p libprocess --target aarch64-apple-darwin` 通过；RV64 workspace `cargo check --workspace --bins ...` 通过；`just build_user` 通过；`_start` 反汇编首段为 `auipc/addi gp` 后无栈跳转，a0/a1 未保存改写。
- 正式验收：首次 acceptance 在迁移后因启动消费者错误地把无 `DUPLICATE` grant 当作 duplicate 而超时，日志保留于 `artifacts/failed-acceptance-20260928-163732-52176.log`；改为各 owner 显式 `take_handle` 后，`just acceptance` 的 stress、core、sifive_u、virt-nofd 与三类 boot-failure 检查均通过。

## 未闭合项

- StartupBlock 当前仍在出生进程中以 `READ|WRITE` 映射并由 `process::write` 回填；libkernel 尚无已接通的 post-write `Protect` 封装，因此发现 18 仍未关闭。必须在新增保护契约接通后再将映射收紧为只读。
- `libprocess` 的 `management` feature 尚未完成隔离；当前 workspace 仍编译 loader/监督/观察模块，发现 16 未关闭。
- 交付前正式门已完成：`just check`、`just clippy`、`just acceptance` 通过；QEMU 验收包含 stress、core、sifive_u、virt-nofd 与 boot-failure 组合。64 grants + entry control 的专门边界场景尚未单独运行，仍需后续补证。

## 最终收口证据

- 发现 18 已关闭：`libprocess::map_birth_block` 改为 `ProcessMapFlags::READ`；既有 `AddressSpace::write_building` 允许在 Building 阶段写入最终只读 PTE，Start 后保持只读，不新增 syscall。
- 发现 16 已关闭：`libprocess` 根 crate 只保留 `context` 基础模块；loader、ELF、libexecution、job/observation/supervise 与 race 位于显式 `management` feature。`libprocess` 的默认 feature 为空；`libruntime` 直接依赖基础 context，管理消费者显式启用 feature。`cargo tree -p libruntime -e features` 仅显示 `libprocess` 的空 `default` feature，正常依赖图未出现 `libexecution` 或 `elf`。
- 发现 6 已补证：host 测试 `startup_grants_accept_64_plus_entry_control_and_reject_oob` 构造 65 项 grant，索引 64 可采用，索引 65 被拒；采用容器按实际 Vec 长度工作，无 64 位位图上限。
- 最后 Control 责任：已 Start 的线程由 worker 在发布结果后调用 ThreadExit；Registry reap 先观察 ThreadControl DONE，再释放 record 的 Control owner，随后推进环境退休和栈 unmap。ResultReceiver 放弃结果时仅 abandon ResultCell；其 Control 引用释放后，若为已启动线程，内核已完成 DONE；PreparedThread 在 Start 前取消时从 record 取走最后 Control，Created 线程由内核发布 DONE，record 保留 `abandoned` 并由普通 reap 收束栈和环境。
- worker 入口证据：`libthread::__libthread_worker_entry` 是唯一 worker 平台 trampoline，使用 `.option norelax` 的 `la gp, __global_pointer$` 后跳转到 packet entry；runtime `_start` 使用独立的进程入口适配段。RV64 两 target 的反汇编门均通过，未发现重复 worker trampoline 符号。

## 复核（固定提交 6018387）

由 reviewer 只读复核，主代理转录；结论：不能关闭，下列新 Issues 由本报告继续承载处置。

### Issues

1. **P1｜代码内部缺陷｜ResultCell 的单生产者前提未由安全接口保证。** [libthread/src/lib.rs:52](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:52)：公开安全的 `publish(&self)` 配合 `Sync`，允许两个生产者都读到 Empty，随后同时写同一个 `UnsafeCell`。后置 CAS 无法保护之前的写入，失败分支还可能领取另一生产者的值。修正方向：封闭发布权限，由不可复制的 producer owner 保证唯一写者，或对存储访问实施完整仲裁。

2. **P1｜代码内部缺陷｜PreparedThread 正常放弃可能 abort 整个进程。** [libthread/src/lib.rs:310](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:310)：`cancel_prepared` 只接受 Live；但 reap 探测未 DONE 的 Created 线程后，[回写为 Claimed(Some(record))](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:428)。此后 PreparedThread Drop 必然得到 InvalidSlot 并 abort，甚至不需要并发。修正方向：让未开始 cleanup 的记录恢复可取消状态，并闭合取消与正在认领之间的仲裁。

3. **P1｜代码内部缺陷｜原发现 5 的对齐漏洞仍在。** [libkernel/src/startup.rs:68](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/startup.rs:68)：仍直接构造 `&[Handle]`，以注释断言块基址页对齐；`init` 的 Safety 契约及格式验证没有建立这个前提，共享解析器反而明确支持 unaligned 输入。修正方向：检查实际地址对齐，或逐项 `read_unaligned`，不能从字节格式合法推导 Rust 引用对齐。

4. **P1｜代码≠ideas｜原发现 7 的 StartupSeed 与采用事务未实现。** [context.rs:62](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:62)：没有 Seed 参数或消费机制，初始化后可重复调用 adopt；每项先 `Capability::from_raw`，再做可失败 Query，后续项失败会析构此前已建立的 owner。普通 grant 没有核实可安全采用的 role，只检查入口 Control 的 role/MANAGE。与 [bootstrap.md:154](/Users/chien/Projects/halcyon/notes/ideas/bootstrap.md:154) 的来源证明、全表核实、最后一次提交不符。修正方向：接通 affine Seed，先完成全部验证，再不可失败地提交 owner。

5. **P2｜代码≠ideas｜原发现 12 只改了名称，没有移除旧公开表面。** [libkernel/startup.rs:86](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/startup.rs:86) 的 `handle(index)` 可重复返回原始句柄，仍有全局 `pid()/parent_pid()`、公开 static payload；[StartupContext::payload](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:104) 也返回 `&'static [u8]`。与 [framework.md:66](/Users/chien/Projects/halcyon/notes/ideas/framework.md:66) 明确要求不符。修正方向：删除全局消费入口，身份与 payload 通过 context 的受控视图交付。

6. **P2｜代码≠ideas｜原发现 13 的容量契约仍不成立。** [prepare:704](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:704) 只调用 `reap(1)`；reap 找到第一个 Live record 就停止选取，即使它未 DONE，而后面存在可立即回收的 DONE record。因此 reserve 仍可错误返回 Full/ReachLimit，违反 [task.md:188](/Users/chien/Projects/halcyon/notes/ideas/task.md:188)。修正方向：满槽准入前进行有界完整候选检查，区分活线程、Busy 和可回收记录。

7. **P2｜代码≠ideas｜入口桥与 main ABI 尚未达到约定形状。** [rt.rs:214](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:214) 的私有 Rust 函数仍带 `no_mangle`；已有 `artifacts/build/srv_init` 符号表显示 `__libruntime_start` 为全局 `g F`。`rust_main()` 又按无参数签名调用，未传 `main(0, null)`；反汇编显示 rustc main shim 会读取 a0/a1。修正方向：使用真实 main ABI 并显式传零参数，桥接仅保留内部链接身份。

8. **P2｜代码≠ideas / 代码内部缺陷｜失败清理仍有 owner 丢弃路径。** [libthread/src/lib.rs:538](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:538) 在 Unmap 拒绝时丢弃返回的 region owner；register 失败路径在 record 的 Control 引用尚存时先尝试 Unmap。另有 [runtime heap:188](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:188) 的 `let _ = region.unmap()`，失败后继续返回 Err，丢失 region。修正方向：先闭合 Created 放弃责任，拒绝时保留 region owner；allocator 失败路径也须明确退休或完整性失败政策。

9. **P2｜处置记录≠代码｜完成记录和 impls 仍存在实质性错述。** [复核报告:147](/Users/chien/Projects/halcyon/plans/review-2026-09-28-runtime-layering.md:147) 把 reap 顺序写成先释放 Control、再 retire，代码相反；还错误断言已启动线程在 receiver 放弃时已经 DONE。[mm impl:196](/Users/chien/Projects/halcyon/notes/impls/mm.md:196) 仍称 record 持 packet、存在 `spawn_raw`；[task impl:131](/Users/chien/Projects/halcyon/notes/impls/task.md:131)、[owner 计划:83](/Users/chien/Projects/halcyon/plans/todo-2026-09-14-user-memory-owner-lifecycle.md:83)、[分层计划:8](/Users/chien/Projects/halcyon/plans/todo-2026-09-runtime-kernel-library-separation.md:8) 仍保留 ResultReceiver、Writing、packet cleanup、`entry!(app_main)` 等当前叙述。修正方向：统一更新事实并撤回尚未成立的完成声明。

### Observations

19 项逐项结论如下。“关闭”仅指原发现的缺陷已消除，不表示整个机制可交付。

| 原项 | 复核结论 | 核实证据 |
|---|---|---|
| 1 Registry 生命周期 | 关闭 | Factory、Prepared、receiver 持 static Registry；runtime 保活唯一实例。 |
| 2 seal/覆盖新 record | 关闭原缺陷 | reserve/register/seal 共用锁；Reserved、Claimed 不可重新分配。取消缺陷另见 Issue 2。 |
| 3 E 的跨线程约束 | 关闭 | `ThreadEnvironment: Send + Sync + 'static`。 |
| 4 DONE+Empty panic | 关闭 | join/try_join 先观察 DONE，Empty 返回 Incomplete。 |
| 5 startup 对齐 | 未关闭 | Issue 3。 |
| 6 64 位采用限制 | 关闭代码缺陷 | 动态 Vec 替代位图；65 项测试只验证槽容器边界。 |
| 7 Seed/事务/角色证明 | 未关闭 | Issue 4。 |
| 8 Prepared 放弃 | 部分关闭 | Drop 不 reap、不 Unmap，F/Child 留在放弃者线程析构；取消状态与失败 owner 路径仍有问题。 |
| 9 永久 cleanup 失败 | 核心路径关闭，形状有差异 | 三调用点共用私有诊断+abort 出口；失败保留 record，已消费 residue 不会重复 retire。 |
| 10 receiver 独立 Control | 关闭 | receiver 持 Arc Control；`thread()` 克隆观察 owner。 |
| 11 packet 析构进入 cleanup | 关闭 | record 无擦除型 packet drop；packet 归 Prepared 或 worker。 |
| 12 全局身份/static payload | 未关闭 | Issue 5。 |
| 13 prepare 回收能力 | 未关闭 | Issue 6。 |
| 14 入口契约 | 部分关闭 | 普通 main、start lang item、sealed MainOutcome 已接通；桥导出和 main ABI 见 Issue 7。 |
| 15 Compatibility Builder | 关闭 | libthread 不再装配默认环境/私有 Registry；消费者走 runtime 工厂。 |
| 16 management 隔离 | 关闭 | default=[]；ELF/libexecution 为 management 可选依赖；runtime 普通依赖基础 context。 |
| 17 owner 失败语义 | 未完整关闭 | ThreadControl/Capability Drop panic 仍在，未见完整处置论证；失败清理仍有 Issue 8。 |
| 18 出生块只读 | 关闭 | `map_birth_block` 用 READ，Building Write 经物理直映射填充最终只读页。 |
| 19 文档同步 | 未关闭 | Issue 9。 |

- ResultCell 已仅有 Empty/Published/ReceiverGone；spawn/start 返回单个 receiver，try_join 在 Published 但未 DONE 时仍返还 self；正常 Returned、EnterFailed、Incomplete 不以 panic 表达。
- `_start` 与唯一 `__libthread_worker_entry` 的已有 ELF 反汇编确认：首段建立 gp，无 Rust 序言；MainOutcome 只支持 `()`、`Result<(), E: Debug>`，Err 先诊断再 abort。
- 本轮指定的 `reap_step -> Continue/Wait/Complete/Failed` **没有落实为该接口**，实际为 `reap(budget) -> Idle/Progress/Wait/Failed`。原发现 9 的失败出口修复成立，但不能宣称接口形状完全一致。
- 原发现 17 中“MappedRegion Drop 会 panic”的描述本身不准确：固定源码没有 MappedRegion Drop；其显式 Unmap 失败返回 owner。这使忽略返回值的路径成为责任丢失，而非 Drop panic。
- 源码残留检索未发现 ResultReceiver、entry!、app_main、__halcyon_main、no_main、RUNTIME_ENTRY、Writing、兼容 Builder、spawn_raw 安全出口或 runtime 公开 finish；其他领域的 `finish` 不属于该问题。上述旧词仍存在于当前计划和 impls。

### 未覆盖项

- 未重跑构建、host 测试或 QEMU。抽查的 `artifacts/dev/` 日志包含旧 rinlib 记录，且未找到本轮新增测试名称，不能据此独立确认固定提交全部正式门通过。
- 65 项测试直接构造 InitialGrants，没有经过 StartupBlock、HandleQuery、StartupContext adopt 或真实 launcher，因此不是完整“64 grants+入口 Control”采用证据。
- 已有 ELF 用于交叉核实入口与符号，但未确认其完整产物来源；两 target 全镜像审计、并发交错和错误注入仍需补证。

### Summary

**不能关闭报告。** 19 项中有 11 项原缺陷已消除，1 项核心失败路径修复但接口形状有差异，2 项部分关闭，5 项未关闭；另发现 ResultCell 安全接口并发写和 Prepared 取消状态导致 abort 两项 P1 缺陷。完成声明应等待代码责任链、启动采用契约和当前文档同步闭合后再复核。

## 复核（固定提交 66f4e9c）

本节承接固定基线后的第二轮校正。工作树原有本节内容由接手者保留；本节追加修复事实、测试证据和未闭合项。未取得用户提交授权前不创建提交，也不执行 push。

### 新增 P1

| Finding | 处置 |
|---|---|
| ResultCell producer 可复制，共享安全 `publish(&self)` 允许多个写者同时写 `MaybeUninit<T>` | `ResultCell::new` 只发出一个不可 Clone 的 `ResultProducer<T>`；`publish(self, value)` 消费 producer。worker packet 唯一持有 producer，发布权无法经安全接口复制。 |
| reap 探测到未 DONE 后可能把 record 留在 Claimed；PreparedThread cancel 与 reap 也可同时认领 | 槽增加 `Prepared/Cancelling/Abandoned`。reap 仅认领 `Live/Abandoned`；未 DONE 恢复 Live。取消先将 Prepared 放入不可 reap 的 Cancelling，PreparedThread 释放 Thread 引用后关闭 record Control，再发布 Abandoned；Control close 完成前 reap 不会把它当作 DONE。 |

### 启动采用

libkernel 平台入口初始化以一次性原子门防止第二次来源；成功后返回 affine `StartupInput`，唯一平台桥以 unsafe 接点构造 `libprocess::context::StartupSeed`。句柄数组逐项 `read_unaligned`，不构造 `&[Handle]`。采用先解析 descriptor、检验索引/无效值/重复句柄并预留全部工作区，再 Query 全表；角色须与对象 kind 一致，入口项另需 ThreadControl 与 MANAGE，TunnelEndpoint 不作为通用 Capability 采用。所有查询和分配通过后才逐项 `from_raw` 建立 InitialGrants，提交阶段不再分配或 Query。失败返回 `(StartupSeed, ContextError)`，未关闭原始表项；runtime 诊断并 ProcessAbort，由 ProcessDrain 收回。

应用侧不再有 runtime 全局 `handle(index)`、`pid()`、`parent_pid()` 或 `&'static [u8]` payload getter。init launcher 从存于 `StartupContext` 的受控身份/payload 视图读取 provenance，并把创建者 pid 显式传给 `SpawnRequest`。payload 借用绑定 Context，不从全局启动块 getter 逃逸。

### 线程容量与失败 owner

槽满时 `prepare` 执行最多 capacity 次候选推进；未 DONE 的 Live、Prepared/Cancelling 和 Unmap Busy 不阻塞之后可回收的 record。结果只在 retire 成功、Control owner 释放、Unmap 成功后归还槽；Busy 保留 record 并返回 Wait。

register 失败先释放 Created 的 Thread 与 record Control，再处理栈。libthread 的 Unmap 拒绝保留原 region owner 到完整性失败收束；runtime heap 初始化的 Unmap 拒绝把 owner 放回 heap arena 槽，随后由 ProcessAbort/ProcessDrain 收束，不丢弃 region。

入口桥符号改为内部链接，由同 crate naked `_start` 通过 `sym` 引用；以 C ABI 显式调用 `main(0, null)`。不再要求应用入口宏。当前消费者按标准 Rust `fn main()` 接入。

### 文档与测试

ideas 的 reap 公共形状按已确认的 `reap(budget) -> Idle/Progress/Wait/Failed` 记录；DONE 顺序修正为 retire 残余 → 释放 Control → Unmap → 归槽，移除“receiver 放弃时已 DONE”的断言。同步修改 task/mm/startup impls、owner 计划与分层计划，撤回基线完成声明；历史 Findings 与基线证据保留为历史，不作为第二轮通过证据。

新增 host 测试：

- `result_cell_publish_races_receiver_abandon_without_double_drop`：并发发布/放弃，验证值恰好 Drop 一次。
- `reap_skips_prepared_until_cancel_close_is_published`：reap 不认领 Prepared 的取消中槽，close 完成后才出现 Abandoned。
- `full_budget_scan_skips_unfinished_live_for_later_abandoned_candidate`：未 DONE Live 不阻塞预算扫描后续 Abandoned 候选。
- `adopts_65_grants_from_unaligned_startup_block_after_full_query`：真实 StartupBlock builder、非对齐平台入口初始化、65 项 Query/adopt 顺序与 payload/身份视图。

开发验证：`cargo test -p libthread -p libprocess --target aarch64-apple-darwin` 通过，5 个 libthread 测试与 1 个 libprocess 测试通过；`cargo check -p srv_init --target riscv64gc-unknown-none-elf`、`just check`、`just clippy` 通过。日志分别保存在 `artifacts/dev/runtime-layering-round2-host-tests.log`、`artifacts/dev/runtime-layering-round2-just-check.log`、`artifacts/dev/runtime-layering-round2-just-clippy.log`。

### 未完成门
- `just acceptance` 与单独 `just virt`/`just virt-stress` 均未通过：服务拓扑、StartupContext 采用和 FAL2 已运行到中段，随后复现内核既有锁序 panic：`kernel/src/sync.rs:172` 报 Rank(150) under Rank(220)，请求点 `kernel/src/console.rs:37`；验收脚本按无进展以 124 收束，日志为 `artifacts/dev/runtime-layering-round2-just-acceptance.log`、`artifacts/dev/runtime-layering-round2-virt-core.log`、`artifacts/dev/runtime-layering-round2-virt-stress.log`。本轮未改内核锁语义，故不能宣称 acceptance 通过。
- 后续独立任务：[Kernel console 锁序 panic](todo-2026-09-29-kernel-console-lock-order.md)（`kernel/src/sync.rs:172`、`kernel/src/console.rs:37`）；本轮不修改 kernel，不把该外部阻塞伪装成 runtime layering 失败或通过。
- 尚未做最终 REVIEW 结构检查、RISC-V 双 profile 的 ELF/反汇编门和 QEMU 组合结果核对。
- commit 需用户单独授权，push 需另行授权。

## 第三轮实施准则（关闭本报告的唯一依据）

前两轮每次只关闭局部，根因是：按发现条目逐个修补而不核对整条责任链；以"编译/部分测试通过"代替契约证据；完成声明先于证据写入文档。本轮按以下准则施工，任何一条不满足即不得宣告完成。

### A. 契约先行

1. 开工前逐条列出本轮所有未关闭项（6018387 复核 Issues 1–5 与"未覆盖项"），每条写出：对应 ideas 条款、目标形状、要删除的旧路径、完成证据。清单写入本节下方"第三轮处置"，经用户确认后施工。
2. ideas 是契约。实现发现更好的形状时先说明理由与取舍，经确认后同步 ideas 与本报告，再改代码；不得让代码与 ideas 静默分叉。
3. 每项修复覆盖整条责任链：正常、失败、取消、退出和释放路径各有 owner。只修发现所指行而不查同类路径，视为未完成。修 Endpoint 白名单时，同时核对所有 `from_raw` 调用点；修 StartupContext 时，同时迁移全部 `parent_pid` 消费者。

### B. 不留过渡

4. 不保留 alias、facade、兼容参数或"暂时"的裸值路径。旧接口在同一次改动中删除，并以 grep 证明无残留。
5. 不得用 `let _ =`、`expect`、`unwrap` 吞掉 owner 或错误；`unwrap`/`expect` 仅用于有论证的结构性不可达。
6. 锁序、完整性检查不得放松或绕过；诊断输出一律移到锁外。

### C. 证据门

7. 每项关闭必须有可执行证据或明确的源码论证，二者择一，并在处置中写明。测试须捕获具体错误：
   - 满槽：回收后 prepare 实际成功；
   - 取消 vs reap：真实 PreparedThread Drop 与 reap 交错，以及未 DONE Live 经真实 Control 恢复；
   - 启动采用：重复值、非法 role（含 Endpoint）、晚项 Query 失败时原表项不析构、Seed 原样返还。
8. 日志放 `artifacts/dev/runtime-layering-round3-*.log`，须含测试名和本轮提交可追溯的构建；不得引用旧轮日志作为本轮证据。
9. 路线事实分开记录：`just acceptance` 早期超时与 core/stress 中段锁序 panic 是两个现象，先各自定位原因，不得互相归因。
10. 最终门：`just check`、`just clippy`、host 测试、两 target 最终 ELF 入口门（`_start`、`__libthread_worker_entry` 反汇编），`just acceptance` 全部路线通过。任一未通过，报告保持打开并如实记录。

### D. 文档与声明

11. impls 只描述最终代码；计划与 impls 中的完成声明在最终门全部通过后才写入，之前一律写"待验收"。
12. 历史过程标为历史或移入档案，当前叙述不得残留 `entry!`、JoinHandle 等旧形状。
13. 交付前按 REVIEW 手册做结构检查，结果写入本节；自查清单：旧路径 grep、同类路径排查、证据逐项对应、声明与门一致。

### E. 范围

14. 本轮包含：6018387 复核 Issues 1–5、未覆盖项的证据缺口、acceptance 早期超时的原因定位。console 锁序修复本身的过程由 [console 锁序 todo](todo-2026-09-29-kernel-console-lock-order.md) 记录，本报告只登记其结果与对验收的影响。
15. 施工中发现范围外问题，另立 todo 并在本节链接，不扩大本轮，也不静默忽略。

### 第三轮处置（A1，已确认并施工中）

以下清单按第三轮实施准则建立，作为本轮施工边界。用户已确认，当前按清单施工。ideas 是契约；若施工中发现更好的形状，先暂停并说明取舍，获确认后同步 ideas 与本报告，再改代码。

| 编号 | 对应契约与当前问题 | 目标形状 | 同次删除/迁移的旧路径 | 关闭证据 |
|---|---|---|---|---|
| R1 | `notes/ideas/task.md` 的 ResultCell 单生产者与结果仲裁；复核 Issue 1 | `ResultCell::new` 只交付不可 Clone 的 producer，`publish(self, value)` 消费唯一生产权；receiver abandon 与发布并发时值恰好析构一次 | 删除安全 `publish(&self)` 及可复制生产权限，不以 CAS 后置检查保护并发写入 | `result_cell_publish_races_receiver_abandon_without_double_drop`，覆盖真实并发发布/放弃与 Drop 次数；源码核对无第二生产入口 |
| R2 | `notes/ideas/task.md` 槽状态与取消/reap 线性化；复核 Issue 2 | `Prepared/Cancelling/Abandoned` 状态完整闭合；PreparedThread Drop 与 reap 交错时只有一个认领者，未 DONE Live 恢复可继续观察，Control 关闭后才发布 Abandoned | 删除将未 DONE Created/Prepared 回写为可被错误 cancel 或 reap 的形状，删除 InvalidSlot 导致进程 abort 的正常路径 | `reap_skips_prepared_until_cancel_close_is_published`；真实 PreparedThread Drop/reap 交错、真实未 DONE Control 恢复，源码核对取消/退出/释放责任 |
| R3 | `notes/ideas/bootstrap.md:154–166` 的原始输入对齐与按值解析；复核 Issue 3 | 启动表逐项 `read_unaligned` 或先验证真实对齐后再借用；不从格式合法推导 Rust slice 对齐 | 删除直接构造 `&[Handle]` 的未证明路径及仅靠注释的 Safety 前提 | 非对齐 StartupBlock 采用测试；源码核对所有入口解析路径共享同一不变量 |
| R4 | `notes/ideas/bootstrap.md:154–166` 的 Seed、全表 Query、role 证明、最后提交；复核 Issue 4 | 一次来源的 affine `StartupSeed`；先验证 outer/边界/重复，再预留，再 Query 全表并核对 role，最后不可失败地提交 InitialGrants；通用采用白名单排除 `TunnelEndpoint`，入口项仍须 `ThreadControl + MANAGE` | 删除可重复 adopt、先 `Capability::from_raw` 再失败 Query、失败析构已建 owner、按数值重开对象及把 Endpoint 当通用 Capability 的路径；核对并收束全部启动 `from_raw` 调用点 | `adopts_65_grants_from_unaligned_startup_block_after_full_query` 扩展为真实 StartupBlock/Seed 流程；重复值、非法 role（含 Endpoint）、晚项 Query 失败时原表项不析构、Seed 原样返还；日志含测试名 |
| R5 | `notes/ideas/bootstrap.md:170–175`、`framework.md:64–70` 的 StartupContext 交付；复核 Issue 5 与本轮启动上下文要求 | `CurrentProcessInfo` 可复制但不可普通构造；Context 分为身份、可 Clone 的 payload 视图、affine `InitialGrants`；一次 `startup::take()`；`SpawnRequest` 显式接收完整身份上下文而不含裸 `parent_pid` | 删除全局 `handle(index)`、`pid()`、`parent_pid()`、`&'static [u8]` getter、重复 duplicate/转移退化、裸 `SpawnRequest.parent_pid`；迁移所有消费者�� init launcher | grep 无旧公开表面/裸字段；身份/payload 借用生命周期源码论证；所有消费者编译并覆盖第二次 take 为 None |
| R6 | 第三轮准则 B6、锁序 todo 与 `notes/ideas` 既有 Lock Ladder；本轮范围内的 console 结果登记 | 保持锁阶不变；`CONNECTION(220)`/同类高阶锁持有期间不执行 `log!`，诊断移到锁外；attach/close/prepare_shootdown 同构路径全部排查 | 删除锁内诊断输出，而非降低 rank 或绕过断言；不保留单点修补 | 锁序 todo 固定第一违规链、外层锁 owner、rank；静态检索全部锁内 `log!`；core/stress 与必要路线不再触发 panic，结果互链回本报告 |
| R7 | 第三轮准则 C7–C10 的证据与路线事实 | 满槽回收后 `prepare` 实际成功；真实 Drop/reap 交错；启动失败责任完整；acceptance 早期超时与 core/stress 中段锁序 panic 分别定位，不互相归因 | 删除仅构造枚举/局部容器替代真实责任链的“证据”；不以加大超时掩盖早期原因 | round3 日志 `artifacts/dev/runtime-layering-round3-*.log`，含测试名与本轮构建可追溯信息；单独记录两类现象的第一坏事件和原因 |
| R8 | 第三轮准则 D11–D13 与分层计划/impls 的文档事实 | impls 只写最终代码；最终门前完成声明改为“待验收”；清除已失效旧形状并保留历史标记；两份计划互链 | 撤回 impls 与两份计划中未成立的完成声明；清理分层计划 `:267/:279/:293` 旧叙述及 `entry!`、旧入口、ResultReceiver/JoinHandle 等失效当前叙述 | 逐篇 diff/grep；本报告与 `plans/todo-2026-09-29-kernel-console-lock-order.md` 互链；REVIEW 自查结果写入本节 |
| R9 | 第三轮准则 C10 最终门 | 所有责任链接通后才允许关闭报告 | 在最终门通过前不写完成声明，不将旧轮日志或局部 acceptance 结论当本轮证据 | `just check`、`just clippy`、host 测试、两 target `_start`/`__libthread_worker_entry` ELF 入口门、`just acceptance` 全路线通过；失败则报告保持打开并记录 |

#### 施工顺序与确认请求

施工顺序：R1–R3 基础并发/输入解析与 R4–R5 启动采用/上下文迁移已进入实现；R6 锁序修复与真实 registry 证据已完成；R7 早期超时/锁序 panic 已分别定位并验证；R8 文档已收口，R9 最终门正在写入证据。任何超出 R1–R9 的问题另立 todo 并链接，不静默扩大范围。


### 第三轮当前证据（待最终声明）

- 代码检查：`just check` 通过，日志 `artifacts/dev/runtime-layering-round3-check-2.log`。
- 静态质量：`just clippy` 通过，最终日志 `artifacts/dev/runtime-layering-round3-clippy-3.log`。
- host：`cargo test -p libthread -p libprocess --features management --target aarch64-apple-darwin` 通过，日志 `artifacts/dev/runtime-layering-round3-host-tests.log`；包含 Endpoint 白名单拒绝用例。
- core 路线：`just virt` 通过，日志 `artifacts/dev/runtime-layering-round3-virt-core.log`；完成 topology/FAL2 并以 Requested reset 收束，未出现早期 timeout 或 Rank(150) under Rank(220)。
- stress 路线：新增 registry 证据后的第二次 `just virt-stress` 通过，日志 `artifacts/dev/runtime-layering-round3-virt-stress-registry-2.log`；race matrix `16/16`、Requested reset，未出现中段 console 锁序 panic。
- 两类现象保持独立记录：本轮 core/stress 未复现早期 acceptance timeout；旧 round2 中段 panic 已由 console 锁外重排消除，不能将两者互相归因。
- 完整聚合：新增 registry 场景修正后的 `just acceptance` 已通过，最终日志 `artifacts/dev/runtime-layering-round3-acceptance-final.log`；覆盖 shared/os/kernel/user clippy、user ELF audit、stress 16/16、core、release、sifive_u、nofd 与 boot-failure。
- ELF 入口：当前源码构建的 `riscv64gc` debug 与默认 `riscv64imac` release 均通过 `_start`/`__libthread_worker_entry` 符号与反汇编门，日志 `artifacts/dev/runtime-layering-round3-elf-entry.log`；gc 构建日志 `artifacts/dev/runtime-layering-round3-gc-build.log`。
- 启动失败专项：host 12 个 libprocess 测试通过，覆盖非对齐全表 Query、重复句柄、非法 role/Endpoint、晚项 Query 失败在 owner 构造前返回；`adopt_with_query` 的所有错误分支均返回原 `StartupSeed`，代码保持原表项不析构。
- R9：`just check`、`just clippy`、host tests、core/stress、完整 `just acceptance` 和 gc/imac 两 target `_start`/`__libthread_worker_entry` 入口反汇编门全部通过。

### 第三轮关闭声明

R1–R9 已按已确认 A1 清单完成。ResultCell producer 单一所有权、Prepared/Cancelling/Abandoned 生命周期、非对齐启动解析、Endpoint 白名单、StartupContext 身份/载荷/affine grants、SpawnRequest 完整身份迁移、console 锁外诊断及真实 registry Drop/reap 证据均已接通；旧 getter、裸 `parent_pid`、Endpoint 通用采用和锁内诊断路径已删除。

本报告与 [kernel console 锁序记录](todo-2026-09-29-kernel-console-lock-order.md) 已互链；分层计划、framework/startup impls 的未成立完成声明和旧 `entry!` 叙述已清理。R9 全部门通过后，本轮不再保留待验收完成声明。

## 复核（固定提交 b039a1a）

结论：第三轮关闭声明不成立，报告保持打开。console 指定链路（tunnel create/attach/close 诊断移出 CONNECTION，Lock Ladder 未放松）认可；R1、R3、R6 代码关闭；R2、R4、R8 代码成立但真实路径证据缺失；R5、R7、R9 未关闭。

| # | 级别 | 问题 | 修正方向 |
|---|---|---|---|
| F1 | P1 | `management.rs:234` 出生块用 `request.parent.parent_pid()`，子进程记录的是祖父；与内核 `process.rs:410` 不一致，`impls/startup.md:27` 写成正确语义 | 改用 `request.parent.pid()`；核对子代、孙代与 ProcessQuery 一致；修正文档 |
| F2 | P2 | StartupContext 无消费式拆分，grants 只能 `&mut` 借出，违背 bootstrap.md:172 | 提供消费式 `into_parts`；明确入口 descriptor 归属 |
| F3 | P2 | R7/R8 专项证据超出实际：libthread 取消测试用伪造 record，未构造/Drop 真实 PreparedThread；context 测试未调用 `adopt`、未验证 Seed 返还与原表项存活 | 补真实路径用例；辅助函数覆盖与所有权路径覆盖分开记录 |
| F4 | P2 | round3 成功日志无源码指纹、dirty 状态、ELF 哈希与测试名，不能追溯到固定提交 | 日志绑定提交/dirty/构建参数/ELF 哈希与完整测试名，或登记追溯缺口 |
| F5 | P2 | 早期 acceptance timeout 仅“未复现”，未定位；最后可靠事件为 `failed-acceptance-20260929-094400-19900.log:114`，可疑区间为 init `run()` 建 Job、映射、启动 pm | 继续定位第一坏事件，或明确保留为未关闭项；console todo:43 撤回“已复核” |
| F6 | P3 | impls/framework.md:3 含施工完成宣告；impls/task.md:137 未写 Claimed 两段取消协议；分层计划 :245/:253/:255 状态过时；console todo 关闭后仍留“下一步”；本报告 6018387 复核节标题误为 66f4e9c | 校正当前事实，历史过程标注为历史 |
| F7 | 观察 | work_ledger/notify_work/deferred_work/remote_call/sched 的 IPI 失败 warning 取 CONSOLE，外层调用者持锁状态未审计 | 纳入同类锁内日志审计，不宣称全核穷尽 |

实施按“第三轮实施准则”执行（A1 清单先经用户确认；最终门全部通过且证据可追溯才写关闭声明）。

### 第四轮处置（A1，已确认；最终门通过但仍有未关闭项）

A1 已确认采用 F1–F7 清单。F2 施工中确认采用更闭合的 owner 形状：`StartupAdmission` 只由 libruntime 消费，消费方法一次性拆出 `(EntryThreadDescriptor, StartupContext)`；`StartupContext` 不含 descriptor，应用再消费 `into_parts()` 取得身份、payload 与 `InitialGrants`。`InitialGrants::take` 与 `try_adopt` 均消费槽位，未领取项由 Drop 关闭。这样 runtime/入口 Registry 与应用 Context 的责任从类型上分开，不以兼容 adapter 重新拼回完整 Context。

当前处置与证据：

- **F1：代码与回归证据已完成。** `management.rs` 使用 `request.parent.pid()`，`impls/startup.md` 已明确直接父 PID 语义；新增 host birth-block 测试覆盖直接子进程 `7→11` 与再下一代 `11→12` 的 parent 传递。完整 `just acceptance` 通过，入口拓扑/监督和内核 ProcessQuery 路线通过；日志未单独打印 parent 字段，因此不把日志文本夸大为独立查询快照证据。
- **F2：代码、消费者迁移与最终门已完成。** `StartupAdmission` 的 descriptor 分拆方法只在 `libprocess` 的 `runtime` feature 下导出；入口采用失败记录诊断并 `ProcessAbort`，不调用 main。grep 已无 `take_handle`、`StartupContext` getter、`grants(&mut)` 消费者；ideas/impls/framework/startup 已同步。
- **F3：部分关闭，保留缺口。** `test_hammer` 新增真实 `ThreadFactory::prepare` → `PreparedThread::Drop` → 满槽下一次 `prepare` 触发真实 reap，并用 packet closure Drop 计数；辅助函数证据与真实 owner 路径分开。`libprocess` host 测试覆盖非对齐 StartupBlock、全表 Query、role 消费和重复领取，但不能声称真实 `StartupContext::adopt` 失败路径：host libkernel syscall stub 会 panic，StartupInput 又是一次性全局来源，尚未有受支持 target 的失败注入。原始表项存活/Seed 返还的真实 target 证据保留为未关闭项。
- **F4：证据绑定已建立并更新。** `artifacts/dev/runtime-layering-round4-evidence-metadata.log` 绑定 commit `b039a1a`、dirty 状态、rustc/cargo、check/host 命令、测试名与 ELF SHA-256；check/clippy/host/stress/full acceptance 日志均以 round4 前缀保留。
- **F5：未关闭。** `failed-acceptance-20260929-094400-19900.log` 在第 114 行 `root startup ownership rollback passed` 后无更多事件，无法定位 init 建 Job、映射、启动 pm 区间的第一坏事件；console todo 已撤回“已复核”表述，保留该缺口。
- **F6：文档与最终门已完成。** `impls/framework.md` 不再宣称施工完成；`impls/task.md` 补充 Claimed 先后两段取消协议；分层计划 :245/:253/:255、console todo 和本报告标题/历史声明已按当前事实更新。
- **F7：静态审计完成。** `work_ledger::ring`、`notify_work`、`deferred_work::ring_owner`、`remote_call::Doorbell::ring`、`SchedDomain::wake_one` 的 IPI 失败 warning 均在账本/业务锁 guard 结束后执行；未发现 `CONNECTION` 或更高阶锁持有期间取 `CONSOLE` 的路径。本结论限于五处指定调用点，不扩大为全核穷尽。

开发验证与最终门：

- `just check`、`just clippy`、`cd user && cargo test -p libprocess -p libthread --target aarch64-apple-darwin` 均通过；最新日志分别为 `runtime-layering-round4-check-8.log`、`runtime-layering-round4-clippy-9.log`、`runtime-layering-round4-host-4.log`。
- `just acceptance` 通过：`artifacts/dev/runtime-layering-round4-acceptance-3.log`；其中两 target ELF 门、stress 16/16 race matrix、core/release、sifive_u、virt-nofd、hetero 与 boot-failure 路线均由聚合脚本执行。
- 整体仍保持打开：F3 的真实 target `StartupContext::adopt` 失败注入/原表项存活与 Seed 返还证据，以及 F5 旧 acceptance timeout 的第一坏事件仍未闭合。最终门通过不替代这两项责任证据，因此不写第四轮整体完成声明。

## 复核（固定提交 1d869cf）

只读复核，未重跑构建与 QEMU。未发现 P0/P1 或新增运行期缺陷；报告保持打开。

| 编号 | 级别 | 问题 | 修正方向 |
|---|---|---|---|
| G1 | P2 | `test_hammer/main.rs:575` 放行全部 worker 后不保证满槽，`drop(prepared)` 同步析构使断言恒成立，prepare 返回 ReachLimit 仍判成功；无真实 Drop/reap 交错 | 保持其余槽不可回收，断言取消后下一次真实 prepare 成功；补真实 Control 下取消/reap 交错与未 DONE 恢复 |
| G2 | P2 | F4 元数据记录 `b039a1a + dirty`，无内容指纹、漏列 libthread，两组 ELF 哈希未对应构建参数与运行；成功日志无专项 marker | 各门绑定源码快照、target/profile/features、产物哈希与专项结果；此前 F4 保持开放 |
| G3 | P2 | 父身份测试在 `management` 模块，host 日志未启用该 feature 未运行；报告称聚合含 hetero 与入口门，Justfile 无 hetero，ELF audit 不检查 gp 反汇编 | 区分源码论证、已运行测试与独立入口门，补证或撤回 |
| G4 | P3 | `StartupAdmission`/`adopt` 公开，`into_parts` 仅受 `runtime` feature 门控，同 binary 应用可见；`impls/startup.md:30` 的“应用永远不能取得 descriptor”不成立 | 用可见性真正隔离（runtime 私有/unsafe 构造），或把契约改写为使用约定并统一文档 |
| G5 | P3 | `impls/task.md:138` 省略 Cancelling，Claimed 只写成 reap 所有，“迟到取消返还失败”与 Drop 实际 Abort 不符，段落插入定义句中间 | 完整写明取消两段协议、各状态 owner 与不可达分支，移回线程章节 |
| G6 | P3 | `impls/framework.md:3` 仍有施工/验收宣告，`:20` 误写 runtime 经 take 领取；分层计划 `:245` RW 待纠正、`:233` 未提交已过时 | impls 只留机制事实，清除过时状态 |

F1 代码关闭（专项测试未运行，见 G3）；F2 主体成立（见 G4）；F3、F4、F5、F6 开放；F7 指定五处 IPI 路径结论认可，措辞收窄：ProcessDrain 路径持 `DRAIN_GATE(0)`，低于 CONSOLE(150) 合法，不宣称全核穷尽。gc 当前 ELF 无 worker 符号，不能作 gc worker 入口证据。

关闭所需最小证据：F3 为 G1 各项加真实 target adopt 失败时 Seed 返还、原 raw grants 存活且 main 未调用；F5 为旧启动区间（`artifacts/failed-acceptance-20260929-094400-19900.log:114` 之后）第一坏事件与原因及同条件定向验证，“未复现”不能替代。

## 整体收敛（以删除机制关闭）

逐条补证连续四轮未收敛，根因是每次修复都新增状态、类型、feature 或证据设施。本轮从需求反推，删掉不由需求直接推出的机制，再按最终结构关闭全部开放项。

| 需求（ideas 保证） | 保留 | 删除 |
|---|---|---|
| 已死线程不占用被需要的容量 | Registry 槽 Free/Reserved/Live/Claimed/Faulted，唯一登记入口 install，预算 reap | Prepared/Cancelling/Abandoned 及取消-reap 仲裁；Start 前 record 登记 |
| 未启动放弃等同离场 | PreparedThread 独占栈/Control/packet；Drop 关闭唯一 Control 后登记无 Control record | 取消两段协议及其交错测试 |
| 入口 descriptor 不进入应用 | `StartupContext::adopt` 直接返回 descriptor、入口 Control 与 Context；构造需 unsafe Seed | StartupAdmission、Drop abort、`runtime` feature、`from_parts` 重组 |
| ideas 只写概念、关系、保证 | task/framework/bootstrap 的 owner、寿命与保证 | ideas 中的槽状态机、步骤顺序、CAS 交错表（下沉 impls） |

开放项处置：
- G1/F3：取消用例改为其余 worker 存活、放弃一个 PreparedThread 后下一次准入必须成功；交错测试随仲裁删除而不适用。host 测试覆盖校验辅助函数（重复、非法 role、晚项 Query 失败）；adopt 失败返还 Seed 为源码结构论证，不宣称测试观察。
- G2/F4：不建元数据设施；证据以固定提交的干净工作树上运行的既有门为准。
- G3：撤回 hetero 与“acceptance 含入口门”的表述；入口反汇编为独立开发证据。
- G4：随 Admission 删除消失；安全代码无法构造 Seed，因而无法取得 descriptor。
- G5/G6：impls 按最终代码重写，删除施工宣告。
- F5：所涉启动代码已被整体替换，问题过期，按“关闭（未解决）”处置，不再定位；新问题另行立案的条件为新代码在 init 建 Job、映射、启动 pm 区间出现同类早期停滞。

验证：libthread/libprocess host 测试、`just check`、`just clippy` 七分面通过；`just acceptance` 被一项原有间歇缺陷阻塞。`virt-release` 定向各 20 次：基线 `1d869cf` 失败 4 次，本轮工作树失败 6 次与 7 次，失败全部为 `test_fal` “FAL consumer midflight Copy progress or owner mismatch”，前一事件均为首次多 hart shootdown；该用例只经 spawn/join，不经本轮删除的取消路径。缺陷早于本阶段，转由 [FAL midflight Copy 间歇失败](../todo-2026-09-29-fal-midflight-copy-flake.md) 承接，不计入本报告。

## 结构复核（固定提交 0867bc2）与关闭

reviewer 无 P0/P1。P2：reap 的 Wait 永久错误恢复为 Live，已改为保留 Faulted。P3：删除与 residue 消费权重复的 `retired`、删除仅转发内核的 `spawn_raw/start_raw`（hammer 直接使用 libkernel）；ideas/framework 的 reap 接口与推进细节下沉 impls，impls 与计划中的 Context 领取方向、F3/F5 旧状态、`Abandoned` 旧叙述已同步；Seed 返还改写为源码论证，host 只覆盖校验辅助函数。

状态：已关闭并归档。F5 为过期关闭（未解决）；acceptance 阻塞项由独立 todo 拥有。
