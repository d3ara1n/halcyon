# 运行时分层独立审查

对象：`f519a99f693db2905d5fbff57a393b9336ad8c2f`，父提交 `325c0eeabac7b99a2d472423ee8c66860b2f0a7d`。审查时 HEAD 等于对象提交，工作树干净。
范围：入口、StartupBlock 接收与能力采用、线程出生/结果/cleanup、Cargo 分层、平台入口及相关消费者；对照题述 ideas、两份施工计划和 framework/startup/task/mm/execution-context impls。
基准：A 为固定提交代码，B 为仓库文档，C 为本任务提供的已确认设计。B 内部存在冲突时明确指出，不以施工计划的完成声明覆盖契约。未发现 P0；以下发现均待处置。

## Issues

1. **P1｜代码内部缺陷｜JoinHandle 可在 Registry 销毁后解引用悬垂指针。** 位置：[libthread/src/lib.rs:695](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:695)、[同文件:802](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:802)。
   `ThreadFactory::spawn/start` 返回不带 Registry 生命周期的 `JoinHandle<T>`，只保存 `*const ()`；safe 调用者可以在局部 Registry 作用域外保留 handle，其 `join/try_join/Drop` 随后解引用已经销毁的 Registry。Registry 默认析构还可能释放正在被 trampoline 使用的 packet。建议改代码：为运行中 record 和观察者建立真实保活或生命周期约束，禁止用注释替代指针存活证明。

2. **P1｜代码内部缺陷｜线程环境 E 未约束跨线程访问和寿命。** 位置：[libthread/src/lib.rs:135](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:135)、[同文件:543](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:543)。
   仅 `Child/Residue/Error` 要求 `Send + 'static`，E 本身没有 `Send/Sync/'static` 约束；safe spawn 却把 `Arc<E>` 放入跨线程 packet，并在子线程调用 `enter/leave`。含 `Rc<Cell<_>>` 或局部借用的 E 可以进入该路径，JoinHandle 的手写 Send 又允许异线程 reap。建议改代码：按实际共享方式约束环境类型，并保留其寿命和线程安全证明。

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

9. **P1｜代码≠文档｜JoinHandle Drop 仍触碰 record，并可能停驻。** 位置：[libthread/src/lib.rs:747](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:747)、[libkernel/src/mm.rs:159](/Users/chien/Projects/halcyon/user/libraries/libkernel/src/mm.rs:159)。
   Drop 调用 reap，后者执行环境 retire、Unmap 和 Control 释放；Unmap 可能因内核退休确认停驻。因此“不等待 child”不等于 Drop 不等待。B `task.md:187` 和 C 禁止 Drop 碰 record，但 `impls/mm.md:196` 又认可 Drop reap，文档内部冲突。建议两者修正：Drop 只放弃结果和观察引用，cleanup 移至规定的显式协作调用点。

10. **P1｜代码与文档都≠期望｜普通 Rust main 入口未实现，仍要求宏与链接桥。** 位置：[libruntime/src/lib.rs:10](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/lib.rs:10)、[libruntime/src/rt.rs:19](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:19)、[test_target/src/main.rs:5](/Users/chien/Projects/halcyon/user/tests/test_target/src/main.rs:5)。
   没有用户 runtime 的 `start` lang item；消费者均为 no_main/app_main/entry!，宏生成 `__halcyon_main`，runtime 还公开 finish 和初始化桥。公开 finish 可让 worker 安全绕过其 wrapper 责任。B `framework.md:66,94`、分层计划 `:13–25` 和 impls 明确采用此方案，与 C 冲突。建议两者修正：恢复模板 `fn main()`，由 runtime 的 start 接收 rustc main，argc/argv 传零，删除宏、应用别名、桥接符号及公开 finish。

11. **P2｜代码≠文档｜应用没有一次性 StartupContext 交接，仍通过全局 raw getter 取用。** 位置：[libruntime/src/lib.rs:30](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/lib.rs:30)、[libprocess/src/context.rs:49](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:49)、[同文件:108](/Users/chien/Projects/halcyon/user/libraries/libprocess/src/context.rs:108)。
   没有 `startup::take()`；runtime 永久持有 Context，公开 pid/parent_pid/raw Handle/static payload。startup_handle 优先 duplicate，同一索引可以反复成功，且 duplicate 任意错误会静默退化为转移原能力；payload 没有视图借用边界。违反 B `bootstrap.md:146,162,168–175` 和 C，部分 impls 又把全局接口当作现状要求。建议两者修正：一次性交付 Context，显式 take/duplicate，不吞权限或资源错误；payload 通过视图借出，管理消费者接收显式身份。

12. **P2｜代码≠文档｜实际消费者使用第二个隐式 Registry，runtime 实例没有接通。** 位置：[libruntime/src/rt.rs:55](/Users/chien/Projects/halcyon/user/libraries/libruntime/src/rt.rs:55)、[libthread/src/lib.rs:756](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:756)、[同文件:779](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:779)。
   runtime 的 ENTRY_REGISTRY 只登记入口线程；所有现有 Builder 消费者使用 libthread 私有静态 Registry，两处容量均固定 64，Builder 自称 Compatibility builder。违反 B `task.md:143`、owner 计划 `:89` 和 C 的 runtime 装配责任；libthread manifest 也未接入 C 指定的 libbudget。建议改代码：接通 runtime 持有实例和环境工厂，迁移消费者、删除兼容全局，并按确认的预算契约装配容量。

13. **P2｜代码≠文档｜prepare 和 wrapper 缺少规定的协作 reap。** 位置：[libthread/src/lib.rs:543](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:543)、[同文件:505](/Users/chien/Projects/halcyon/user/libraries/libthread/src/lib.rs:505)。
   prepare 直接准备资源并 insert，wrapper 发布结果后直接 ThreadExit，两者都不 reap。保留已完成线程的 JoinHandle、推迟领取结果时，死线程占满槽就使后续 spawn ReachLimit，实际把容量变成未清理出生次数上限。违反 B `task.md:180–187` 和 C。建议改代码：准入前推进已 DONE 记录，wrapper 离场前作有界 reap，join 确认 DONE 后推进。

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

报告由 reviewer 只读生成，主代理转录。按 AGENTS，本报告即 19 项发现的唯一修复与复核记录，不另建 todo。f519a99 在 owner 计划、分层计划中的完成声明，校正完成前不成立。

### 契约来源

期望设计已写入 ideas，施工以 ideas 为准：[task](../notes/ideas/task.md#用户态线程资源收束)（ResultReceiver、Registry 槽状态、packet owner、结果类型、Drop、cleanup 失败）、[framework](../notes/ideas/framework.md)（`start` lang item、`startup::take()`、唯一 Registry 实例）、[bootstrap](../notes/ideas/bootstrap.md#用户态接收与一次性交接)（Seed/Query/去重）、[execution-context](../notes/ideas/execution-context.md#用户态平台入口)（gp/norelax 适配段）。ideas 已修正；impls 与两份计划由施工同步。

用户裁决：`JoinHandle<T>` 改名 `ResultReceiver<T>`，不保留旧名或 alias；本系统不提供 Rust std，不沿用 std 名以免误导同义。

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

- 已完成线程结果/槽/cleanup 主路径：`JoinHandle`、擦除 packet owner、默认兼容 Builder 与 Drop reap 已删除；`ResultReceiver<T, E>` 共享 `ResultCell<ThreadOutcome>` 与 `ThreadControl`，`try_join(self)` 未 DONE 返回自身，DONE+Empty 返回 `Incomplete`；环境约束为 `Send + Sync + 'static`。槽使用 `Free/Reserved/Live/Claimed/Faulted`，cleanup 记录 `retired/done/abandoned`，永久错误统一经 `cleanup_integrity_failure` 进入 `ProcessAbort`。
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
