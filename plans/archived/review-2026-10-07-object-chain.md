# 对象链交付批次：独立 Review 与机制收敛

状态：**独立审查、修复与机制收敛已完成，未提交**。固定目标的 **3 项 P1、3 项 P2** 已修复并复核；C1/C2真实消费者已接通，D1–D6已回填。10项结构候选采纳8项，另外2项按独立合同关闭本轮候选，不留下当前正确性欠账。用户授权主代理与CalmStoat继续修复/收敛；CalmStoat只提供设计意见，主代理亲核源码并承担取舍。本篇保留原始finding、施工设计、最终验证及边界；提交须另获明确授权。

## 固定目标、范围与证据边界

- 固定目标：`2aee0286dae3c3a8118f8d24f96e79a35f337c92`，`refactor(kernel): 收口对象链自动回收与维护执行`。
- 完整比较基线：`253c04acc25799dd8f930ed883f1e760131924bc`。中间检查点 `4b6e4c14e1af7ae94cc1f5a56f46771f8e8c6f21` 当时不可编译，不作为完整比较起点。
- 五个 reviewer 使用同一 detached 固定源码工作树、独立上下文，只报告，不修改实现/ideas/计划。主代理亲读交接及具体发现位置，核实可达时序与合同，作本篇取舍。临时工作树在报告形成后清理；源码位置均指上述固定提交，可用 `git show <目标>:<路径>` 复核。
- 实际工具链：`rustc 1.100.0-nightly (c54751567 2026-08-22)`；`just 1.58.0`。
- 审查任务与切面：[原任务档案](todo-2026-10-07-object-chain-review.md)；既有契约与历史验证：[交付档案](todo-2026-10-01-object-chain-kernel-integration.md#最终结构检查完成条件与后续触发)；纪律：[REVIEW](../REVIEW.md)。

范围包括角色 capability/授权与消费、Process/Thread/Job 自动回收、实际付款与共享存储、VM/backing/View/普通 Tunnel、持久等待及 Runtime/Assembly/Caller、真实 fs/pm/init 组合，以及维护费用、有限服务机会、单步界与累计栈证据。TRANSFER 环收集、可靠物理执行计量/精确 CPU 份额/WCET、terminal 诊断栈及全分配点注入不在既有承诺内。FAL 全面重设计另案，本次纳入其授权、运输与装配接缝。

历史日志、栈脚本/JSON 和固定 ELF 位于 ignored `artifacts/`。已有验收只证明其已执行行为，不替代本次源码判断。报告确认目标树上的缺陷，不默认断言每项都由目标提交新增；已确认继承项在各 finding 单独注明。

## 审查组织、原始输出与验证

| 切面 | 实际亲读覆盖 | 本次证据及边界 |
|---|---|---|
| R1 | funded_storage、ordered_table、timer_queue、memory_pool、ready_queue、funded_frame；内核 resources/frame/heap/Pool、容器增长及消费者退款接缝 | 源码、固定 rust-src Arc 布局与历史存储失败日志；没有新测试，未验证 kernel Arc OOM/增长碰撞/output fault 注入 |
| R2 | Operation/Change/Backing/Views/Space/Table/Remote/Retire/Completion；memory_space/page_table；MemoryObject/Tunnel、bootstrap Map/Write、用户 mm 与 ABI | 源码、固定 ISA、历史 host/路线日志；没有新测试，没有动态复现 SUM/TLB 窗口，未重审 Runnel 页内协议与 Process 全状态机 |
| R3 | capability mint/claim/restore/commit；Process/Create/Builder/Thread/Departure/Job；Mailbox/Once/Delivery/Lifetime、Notification、SystemReset、bootstrap、libkernel/libthread | 显式 `aarch64-apple-darwin` 的 capability 现有 host 测试 22 项通过；未跑 QEMU/故障注入，原语测试不覆盖 Job 角色策略 |
| R4 | Account/Ledger/Rotor、双环执行与父归账；调度/clock/trap/uaccess/sync/runtime_gate/域/idle；15 种 Work 的执行及最后引用接缝 | 源码、固定 SBI/ISA、历史 host 与 ELF/JSON；没有新测试，没有重建完整 CFI/调用图或重算累计界，不签发新 WCET/栈证明 |
| R5 | Wait/WaitSet/source/epoch/timer；Source/Assembly/Runtime/WorkQueue；Caller/Dispatcher/Exchange/Outbox；监督、Runnel 与 init/fs/pm 实际装配接缝 | 现有 libexecution 精确期限取消用例 1 项通过；未完整逐行覆盖 init main/race、fs handlers、FAL 节点与全部 hammer 剧本 |
| 主代理 | 上述跨切面授权、cookie 提交/失败转交、最后退款/access/ingress/DONE、Tunnel View 分离、用户期限缓存及监督接缝；亲核六项发现与具体收敛依据 | 源码、固定外部规范与基线归因核验；32 份变更 Markdown 本地链接无缺失；没有新整套验收或新回归用例 |

原始 reviewer 输出分别保存在 [R1](../../artifacts/object-chain-review-r1-001.md)、[R2](../../artifacts/object-chain-review-r2-001.md)、[R3](../../artifacts/object-chain-review-r3-001.md)、[R4](../../artifacts/object-chain-review-r4-001.md)、[R5](../../artifacts/object-chain-review-r5-001.md)。这些是审查证据，后续不改写原始判断；链接由临时工作树规范化到主仓库，发现状态与更正只维护本篇。原始会话记录还保存在 pi subagent history，本篇不依赖其存在才能解释发现。

收口文档检查：报告、任务档案、COMPASS 与 KNOWN_ISSUES 四份文件的101个本地链接目标全部存在；`git diff --check` 通过。原任务正文只调整完成状态、执行协议标题及归档后的链接，保留五切面原合同。临时固定工作树及其测试构建产物已清理，五份审查输出作为证据保留在主仓库 artifacts。

本次两个有效 host 检查由 reviewer 在固定工作树执行：

```sh
cargo test --manifest-path os/Cargo.toml -p capability --target aarch64-apple-darwin --offline
cargo test --offline --manifest-path user/Cargo.toml -p libexecution --target aarch64-apple-darwin runtime::tests::successful_gate_and_callbacks_cancel_deadline_before_later_clock_sample -- --exact
```

结果分别为 22 项、1 项通过，主代理未重复执行。R5 先前一次未包含完整模块路径的 `--exact` 过滤不计有效覆盖。以上通过不复现本篇发现，也不作为修复验收。历史 stack ELF 的 SHA256 已由 R4 核对为 `294a36aa292db1d99e0bdee2060a01f5f07499ecd76ffb9780cc1e092f466082`，历史模型累计 boot `120000B`、secondary `119216B`，本次仅核身份及抽查剪边前提，未重新计算。

## 缺陷 findings

以下正文保留固定目标上的原始发现与建议验证门，最终实现、实际验证及未覆盖边界由末尾「最终复核与交付」拥有。

| 编号 | 级别 | 主 owner | 状态 | 影响 |
|---|---|---|---|---|
| R2-B1 | P1 | VM/uaccess；R3/R4 协作 | 已修复并复核 | 软件预检通过后，陈旧 TLB 可使用户输出触发 S 态 fatal |
| R3-B1 | P1 | Job/capability；监督协作 | 已修复并复核 | 子 Job 最后 capability 归零后父域不能重新取得管理权，子树无法收束 |
| R4-B1 | P1 | 调度/平台门铃 | 已修复并复核，采用runtime_stop终态 | IPI 失败后目标 hart 可永远 WFI，Ready 责任没有可靠推进者 |
| R4-B2 | P2 | sync 类型合同 | 已修复并复核；继承缺陷 | LockGuard 自动 Send/Sync 破坏 hart 归属及 `T: Sync` 要求 |
| R5-B1 | P2 | libexecution | 已修复并复核 | 非 Gate 回调取消期限后缓存未同步，后续可交付已取消的 timeout |
| R1-B1 | P2 | funded_frame | 已修复并复核；继承缺陷 | `split_single` 错误消耗原 owner，违背失败保留合同 |

### R2-B1：SUM 直访前没有同步本地硬件翻译

位置：[uaccess.rs:77](../../os/kernel/src/uaccess.rs#L77)、[space.rs:235](../../os/kernel/src/vm/space.rs#L235)、[change.rs:585](../../os/kernel/src/vm/change.rs#L585)、[assembly.asm:304](../../os/kernel/src/assembly.asm#L304)。主代理确认 `check_range` 只逐页软件 translate，没有执行 SFENCE；SUM 字节复制及 `publish_u64` 均直接访问当前用户 VA。

可达时序：同进程 hart A 缓存 RO 翻译并进入带输出 syscall、完成入口 remote drain；hart B 合法推进原 maximum=RW 页的 RO→RW Protect，写 PTE 后放 VM 锁；A 尚未处理该次 shootdown，取得 VM 锁后软件预检看到 RW，硬件 SUM store 却使用旧 RO 翻译并触发 S 态 page fault。S 态 trap 直接进入 `_fatal_entry`，无法被 `defer_output_fault` 的软件错误路径接住。

固定 [ISA supervisor 的 SFENCE.VMA](../../references/normative/riscv-isa-v20250508/src/supervisor.adoc#L1213) 明确允许 fence 前继续使用旧权限并产生 page fault。空间锁只防后续 PTE 修改，不能消除旧 TLB；这是用户输入可触发内核 fatal 的隔离缺陷。本次未动态复现，不声称所有 QEMU/硬件都保留该旧翻译。

目标修复：在空间锁保护下、SUM 直访前完成覆盖访问区间的本地翻译同步，保留 translate 预检和 SUM 通道，不另建拷贝框架。同步审读、写与 cookie 发布入口。不能简单调用现有 `synchronize_local(current_epoch)`：分步 PTE 发布的未完成批次返回 Ready，epoch 尚未前进；若选择代次优化，必须把每个实际 PTE 可见批次纳入协议。新增 fence 的费用与单步界归 R4 同步核实。

验证门：复用双 hart hammer，让一侧合法变更页权限、另一侧将该页作为正式 syscall 输出，定向覆盖 PTE 已写而目标 hart 尚未 ack 的窗口；确认不进入 S 态 fatal。host 页表测试无法替代硬件翻译验证。失败/取消/退出仍由既有 Operation、执行线程与 Shell owner 收束。

### R3-B1：Job 的 Terminal capability 策略撤销了父域管理权

位置：[job.rs:258](../../os/kernel/src/task/job.rs#L258)、[job.rs:789](../../os/kernel/src/task/job.rs#L789)、[holders.rs:89](../../os/capability/src/holders.rs#L89)、[management.rs:911](../../user/libraries/libprocess/src/management.rs#L911)。root 与 child Job 都采用 Terminal，Job 未实现最后 capability 消散动作；父 children 表仍强持未 sealed 的子 Job。

无需并发：创建子 Job并记 JobId → Close 最后子 capability/创建者退出 → 父枚举仍含该子 Job → 合法父 MANAGE 调用 JobDerive 找到对象 → `mint` 被 SEALED Holders 拒绝为 ObjectClosed。空子 Job 也不自行完成，父子付款根继续保留；实际 JobCollector 只把 ObjectNotFound 当已消失，ObjectClosed 使收束失败。

JobDerive 的授权源是父域 MANAGE，Close 不等于 Seal。目标为 root/child Job 统一 Remintable，由父表/root anchor 与 sealed/dead 事实拥有生命周期；最后子 capability 消散不得取消父域管理资格。不能用“最后 Close 自动 Seal”作为局部补丁，它改变已确认 Close 语义。已经完成并移表的目标继续 ObjectNotFound，已经 sealed 的现存目标仍可观察及按合法权限管理。

验证门：真实 Job testcase 覆盖创建子 Job→关闭最后子 cap→从父枚举并重新派生→Seal→观察 CLOSED→确认父枚举移除，再覆盖创建者退出和 JobCollector 收束。capability 22 项原语测试仅支持 Terminal/Remintable 机制事实，未覆盖本角色回归。C1 的后代 Seal 设计冲突与本缺陷独立，不阻止先恢复父派生授权。

### R4-B1：失败门铃没有稳定推进责任

位置：[sched.rs:168](../../os/kernel/src/sched.rs#L168)、[sched.rs:653](../../os/kernel/src/sched.rs#L653)。A 把仅由 B 域可执行的线程放入 Ready；B 已登记 idle 并完成最后检查；IPI 失败只 warning。若 B 没有 deadline，随后卸载 timer 并 WFI；A 也可完成工作后入睡，没有来源保证再敲 B。Ready 电平只有 B 醒来才会被重读。

固定 [SBI IPI Send Errors](../../references/normative/riscv-sbi-v3.0/src/ext-ipi.adoc#L22) 允许 SBI_ERR_FAILED；固定 [ISA WFI](../../references/normative/riscv-isa-v20250508/src/machine.adoc#L2429) 允许停驻，不能依赖无中断自行恢复。成功/合并门铃下双重检查的证明不适用于发送失败，额外一次复检仍有最后检查到 WFI 的窗口。主代理核实真实 ReadyBatch 发布和该失败分支；不是当前运行已观测的死锁。

施工前提：闭合可恢复失败的稳定 owner、由谁在哪个有保证的安全点重试、如何保证该 owner 不一起睡死，以及平台不能恢复时的最终故障收束。一次重试或无约束永远重试都不构成完整合同；也不能只警告后继续声称正常活性。若采用 runtime_stop，须核对 stop 的跨 hart 唤醒同样不依赖已失效 IPI。同步检查 maintenance/remote 门铃，但不未经证据把所有同类调用自动列为相同 bug。

验证门：复用异构域路线，在 B 最后 idle 检查之后注入一次 IPI 失败；唯一 Ready 线程只能在 B 执行，移除无关 timer/后续门铃；验证最终执行或进入已确认的平台故障终态。故障政策与推进上界未闭合前不实施该项。

### R4-B2：守卫的自动 Send/Sync 与安全封装不一致

位置：[sync.rs:386](../../os/kernel/src/sync.rs#L386)、[sync.rs:390](../../os/kernel/src/sync.rs#L390)、[sync.rs:471](../../os/kernel/src/sync.rs#L471)。LockGuard 仅含 `&Lock<R,T>` 和布尔 InterruptState；`Lock: Sync` 只要求 T:Send，因此通常的票锁守卫自动 Send/Sync。

安全代码可跨 hart 移交守卫，Drop 却在新 hart 弹 ladder/恢复 IRQ；也可共享 `LockGuard<Cell<u64>>`，安全 Deref 暴露 `&Cell` 而 Cell 并非 Sync。主代理核实 InterruptState 没有禁止这些 auto trait 的 marker。未找到当前消费者实际移交，不宣称已发生数据竞争。基线的 SpinlockGuard 同样只有锁引用/IRQ 状态，该缺陷为继承问题。

目标：显式禁止守卫 Send，Sync 要求 T:Sync 或一并禁止，按现有 RankedRawSpinlock 的 GuardNoSend 合同对齐。删除错误的自动能力，不引入运行期 hart owner/回调。锁序、门外释放与正常持锁使用保持原契约。

验证门：针对实际守卫的编译期用例证明无法跨线程移动、`Cell` 守卫无法共享，正常本 hart 访问可编译；再执行受影响的 kernel check/clippy。本次没有新增或运行该负向用例。

### R5-B1：非 Gate 回调未刷新任务期限

位置：[runtime.rs:707](../../user/libraries/libexecution/src/runtime.rs#L707)、[work_queue.rs:305](../../user/libraries/libexecution/src/work_queue.rs#L305)、[work_queue.rs:387](../../user/libraries/libexecution/src/work_queue.rs#L387)。来源独立退休重试成功时，`unregistered` 后只 schedule；stop 回调后同样没有同步任务 deadline。Gate 内对应回调则有统一收尾同步。

可达时序：缓存期限 50；时刻20回调将业务 deadline 取消为 INFINITE；下一推进时刻60先按缓存调用 mature_deadline，设置 timed_out 后交业务 advance；此后才同步 deadline，无法撤销已经送出的旧 timeout。框架拥有的既有合同是回调后同步期限，不能要求每个业务自己再验证来修补缓存错误。

目标：非 Gate 回调也遵守统一成熟/同步协议，回调前已到期的责任仍成熟，回调前未到期而被取消的不得随后成熟。正确区分 Active/Finalizing，不能给 Finalizing 重装业务 timer；非法新期限仍保留原 Task 并报告错误，清理责任不丢失。真实 Outbox.stop 会进入 deadline=INFINITE 的终态；未证明现有服务因此失败，部分消费者会重查绝对期限或忽略终态 timeout，内核 Remove 正常路径也没有可恢复 Busy 的运行证据。

验证门：扩展既有期限 fixture，覆盖 Remove 首次 Busy/退休重试成功/取消后跨旧期限，stop 取消或替换后跨旧期限，以及回调前已经到期不能抹除。本次有效用例只覆盖 Gate/注册回调成功取消，未覆盖此缺口。

### R1-B1：切分失败消费原 owner

位置：[funded_frame/lib.rs:279](../../os/funded_frame/src/lib.rs#L279)。`split_single(mut self, …)` 承诺失败保留原 owner，但 InvalidSplit 和 credit split 拒绝都只返回 DecomposeError，随后 self 析构，物理 claim/费用归还，调用者不能重试。该代码与比较基线相同，属于继承的公共 API 缺陷；没有证明当前内核非法切分可达，生产 FundedExtent 已先校验边界并把同源 credit 切分作为必成。

优先目标：删除重复的按值 `split_single`，消费者先用失败保留 owner 的 `split_off(&mut self, …)`，成功再组合左右 owner；若保留按值 API，错误必须携回原 Self。同步 kernel FundedExtent 与 funded_frame 用例，不加适配保留两套失败语义。

验证门：复用 broker 计数 fixture，两个非法边界和一次 credit 拒绝均检查原 claim/allocated credit 仍由调用者持有，最终释放才退款。现有成功切分用例不能替代失败边界。

## 机制收敛取舍

以下保留独立审查时的候选及理由；授权后的实际采纳结果见末尾。没有以类型多、只有一个实现或现有消费者为删除/保留理由；中间编辑步骤不拆成独立交付。

### 优先闭合的结构候选

**R2-S1：删除非生产的第二套页表修改协议。** [page_table/lib.rs:587](../../os/page_table/src/lib.rs#L587) 的 preflight/prepare/PreparedTranslation/publish_batch 与 [lib.rs:1476](../../os/page_table/src/lib.rs#L1476) 的 PublishCursor/publish_step 各实现 Map/Unmap/Protect；旧消费者只在本 crate/测试，内核采用后者。由 TableTree 保留唯一运行期分步算法，迁移 mega 切分、跨表/shared root、真实 owner/drain 等契约测试后，删除旧 PreparedTranslation/PrepareFailure/PublishOutcome、专用递归编辑和仅服务旧路径的 generation。保留启动未发布树的 EagerMapper。收益是减少完整算法、退款/剪枝假设与误导性验证；代价是迁移测试，不改 ABI。验证必须驱动生产 cursor 并证明 detach/fence 前保活及最后退款。本项承接已有旧 API 债务，不重复列 bug。

**R5-S1：把任务侧来源登记寿命收敛为显式状态。** [ObservationSlot](../../user/libraries/libprocess/src/observation.rs#L116)、[Dispatcher SourceState](../../user/libraries/librpc/src/dispatcher.rs#L42)、[Outbox](../../user/libraries/librpc/src/outbox.rs#L26) 以及 fs/pm 任务重复 source Option、安装/注销布尔与回执转换。libexecution 可以拥有 Empty/Installing/Attached(SourceId)/Removing(SourceId) 的登记寿命值及显式请求/回执操作；业务继续拥有结果、掩码、变化政策和实际 owner，Runtime 继续拥有内核 token/真实 Remove 重试/付款。删重复字段和转换，避免非法组合；禁止隐式 Drop 注销、另接收集合或回调反向依赖领域。迁移 Observation/Dispatcher/Outbox/fs/pm/init 后，验证拒绝、安装中停止、迟到回执、Remove 重试及注销后关闭。收益明确且跨真实消费者；代价是一次共同契约迁移，须先确定停止中迟到 registered 的状态转换。

**R1-S1：OrdMap 空槽使用 arena 内链，删除第二份 free 分配。** [ordmap.rs:27](../../os/memory_space/src/ordmap.rs#L27) 的节点数组和 paid free 索引数组共同准备/发布；删除后节点 value=None，空闲节点的闲置 left 可保存下一空槽。arena 保存 free_head/free_count，重新入树覆盖链接；删除 free Buffer、其 charge、配对容量校验和第二次准入/allocator 失败。保持 reserved，它表达未消费的业务承诺；不由空闲数替代。arena 搬移保持索引，旧块仍带出全部 VM/Backing 高锁退款，锁阶不变。收益是每槽4字节独立存储及一次 allocation/失败边界；代价是空节点链接第二种用途。同步 OrdMap/SegmentMap/MemorySpace，验证 AVL 模型、删除复用、并存 reservation、换块保索引、拒绝不改状态和门外退款。空 arena 最后释放不得重新扫描高水位。

**R3-S1：删 Builder 的 `abandoned` 和不可达失败仲裁。** [builder.rs:37](../../os/kernel/src/task/process/builder.rs#L37) 的 abandoned 在失败分支选择 Pending/Aborted；公开 Start 的 Invocation 在 builder.start 返回前一直持唯一 authority，并发 Close 只撤销返还资格，不能造成此期间最后 capability 归零。成功 consume 后 Starting 的 abandoned 写入不再参与先前失败分支；bootstrap 未发行 Builder capability。失败直接回 Pending，再由 Invocation restore/release 决定放弃；已提交责任继续稳定 Work。删字段和分支，不删 Builder/WorkReceipt。收益是少一套无效仲裁；前置是所有 Start 调用者持续遵守 authority/构造责任；验证 seal/Frozen/付款拒绝、并发 Close、成功后 Kill。无 ABI 变化。

**R3-S2：删 Departure 单 bool 锁。** [departure.rs:16](../../os/kernel/src/task/process/departure.rs#L16) 的 requested 只由 ThreadRelease 最后字段 Drop 单次发布。将 request 明确为单次入口，删 DepartureState/锁及重复请求分支；保留 Thread 未提交出生判断、Departure.completed、独立宿主和末字段退款，它们分别决定是否发布、是否已经摘成员/可结算。收益是少一个同步域和两次锁，metadata 按新布局自动变化；前置是唯一调用点和 Work 串行执行。验证未提交出生、Created 放弃、Running/Waiting 退出、迟到 Thread 根及退款→Thread DONE→Shell 回执的顺序。主代理已核调用点，不能把相关 footer 一并合并。

### 局部资源成本候选

**R1-S2：已有写许可稳定来源时省掉必然废弃的 ObjectSource 候选。** RW Map 先 [hold_writes](../../os/kernel/src/vm/operation.rs#L487)，该操作许可使来源 permits 非零；随后 [prepare_map](../../os/kernel/src/vm/views.rs#L142) 再付款/分配同来源节点，commit 只给旧来源 views 加一。准备接口可携带“本操作许可保证来源仍存”的明确事实，省一次节点分配/Arc/退款/峰值拒绝；RO 或只有一次查表快照不能套该优化。R2 主导，R1 核付款：同步 Operation/Views/回滚，验证提交前取消、并发退休、RO 来源消失及边界额度。保留许可与 View 两种独立事实，不用缓存命中猜稳定性。

**R5-S3：两个 task timer 共用一次稳定分配。** [work_queue.rs:117](../../user/libraries/libexecution/src/work_queue.rs#L117) 分别分配业务期限和执行退避的 Pin Box；可改为一次 Pin<Box<TaskTimers>>，包含两个独立节点。删除一次 allocator 调用和一个 Box owner，保留两类 timer 的登记/取消/成熟。代价是字段 pin 投影及别名论证；构造失败返原任务、期限与退避并存、stop/finalize 先解除两链的用例是验证门。收益有限，可随 R5-B1 的同一责任切面评估，不单独制造施工阶段。

### 先保留现状，满足前置才推进

**R2-S2：按实际树需求去重预付表帧。** [worst_case_frames](../../os/page_table/src/lib.rs#L1457) 对 Sv39 Unmap/Protect 固定4帧，普通4KiB Unmap和共享路径因此有可避免峰值。目标是预约成立后由页表层给唯一需求 owner；但仅凭当前树快照会在不相交事务摘表/拆大页后失效。必须先证明需求只减，或闭合门外补齐/最终复检且 cookie 后必成，才能减少储备。收益是页额度/清零/退款减少，代价是新证明与交错。当前最坏界有独立 correctness 理由，**不为减少分配先删除该保证**。触发为准备成本/额度峰值改善有价值且上述证明闭合；验证需 cover 并发摘表、拆分、碎片计划和付款拒绝。

**R5-S2：统一同步/异步 RPC 事务值。** Caller 的 OperationState 与 Dispatcher 的 PendingCall 都持未发送 Request/已发送 observation/期限/终态，Exchange 只共享编码/接受。目标可由 librpc 单一事务值拥有发送与失败 owner 转换；同步等待、Dispatcher 多调用索引/公平队列/来源寿命、空 ReplyPort 复用及 Outbox 回复责任仍独立。当前 Caller 借用式 ReplyPort 复用与 Dispatcher 自持多事务并非可直接互换，**先闭合事务值与 port 池归还协议**，再判断消除的重复是否大于新增间接层。触发为共用 owner 设计成立；同步 init/test_fal/fs，验证发送返 Request、已发超时、迟到回复、Delivery abandonment、注销与 cleanup 失败。不得以一个通用 driver/facade 掩盖不同调度合同。

**R4-S1：减少外层 M 重复采样和量子换算。** [maintenance.rs:693](../../os/kernel/src/maintenance.rs#L693) 使用相邻 M/atomic 起点及两个终点，每次机会换算固定账户量子。可以缓存不可变 Geometry 的量子换算；采样合并须先明确完整原子诊断与类别费用区间。原 Rotor 终点包含 record_max 的工作，直接复用较早终点会改变尾段计费，不能把“少一次取时”当等价变换。前置为该尾段归属和 clock 故障合同闭合；收益有限，后置。保留 Account 与类别两层计量，验证零样本有限轮转、实际费用守恒和完整原子诊断覆盖，不新增 meter facade。

### 应保留的责任边界与未采纳候选

- `OwnedBox/Buffer → MetadataCharge → AllocatedCredit + Pool` 分别拥有分配/初始化、实际退款责任、账本单位/付款根。PreparedEntry 让摘出节点保持原 charge 并锁外释放，不是单纯套壳。
- Ready Capacity/Admission 与 storage charge 分别拥有全寿命资格和持续存储；客户端离场不能撤销系统域共享容量。
- ledger 几何/权限/预约与 Views 的仿射 use/来源根、ViewRetirement VA 游标与 BackingRelease 物理退款游标分别拥有不同事实。失败等价切界不应靠合并两份账掩盖。
- Operation 的失败 owner、ChangeWork 稳定执行根与 Completion 最后退款/access 屏障不能折成提前 DONE；ConstructionLease、ingress、receipt、RunPermit 和成员身份亦不能凭相似计数合并。
- Account/Ledger 拥有跨 hart 在途/父债务，Rotor 拥有本 hart U/M 机会；账户/类别/线程量子及 HartCosts/Execution 不能合并。后者还避免析构重借 Execution 独占引用。
- Assembly 唯一接收/关闭与 Runtime 登记借用支撑多 Runtime 装配；Finalizing 等真实来源退休，不等于业务终态；业务 deadline 与执行退避独立。
- Tunnel Connection root、Endpoint authority 和普通 View use 独立。主代理另查 MappedEndpoint 几何副本及 close 在 unmap 后返部分 owner：真实消费者会重试关闭，未发现当前错误使用；其几何是不可变描述，不足以单独证明两套可变真值。**本次不列第11项收敛或新 bug**，不为字段数量另造 partial-close facade。

## 设计合同与实现说明差异

### C1：Job 后代 Seal 能力未闭合

[自动回收计划的 Job 闭合门](../todo-2026-09-29-process-automatic-reclamation.md#job闭合门-4)（目标行184–187）要求 Seal 时发布有界后代封口 Work、物化 inherited seal、自动收束空子树，并写先 CLOSED 后摘父表。当前代码只封本 Job，Work 在本节点两表清空后执行父表摘除；真实用户 JobCollector 逐层 Seal。后续交付明确改为**先摘父表再发布 CLOSED**，该旧顺序已被取代；但是未找到明确撤销“父 Seal 自动收束空子树”的决定，不能凭现有代码选择用户递归政策。

原始 R3 输出将入口写为行181，主代理核正为184–187。后续只读检索 CalmStoat 设计会话，确认其与 LoneCrane 的轮次3/4已选择自动后代封口：Seal 时发布 Work，按有界游标传播；成员未空则 Blocked，封口时在途 ChildReservation 提交后出生即 sealed 并尝试收束。未找到改成用户逐层 Seal 的后续决定。故按现存计划与既有选择补接该能力，不再将两条路线视作等价未决项；详细游标、稳定根、准入、取消与验证仍须在施工前闭合。若另有后续明确决定，先取证更新本条。C1 不与 B1 混算，父域重新派生缺陷在当前用户遍历语义下也成立。

### C2：用户态伪 fault 分类/重试仍未接入 trap

[内存设计轮次3 fault](../todo-2026-10-01-memory-redesign.md#fault) 要求按 R/W/X 对照事务阶段：不允许才投递正常 fault，允许未发布则锁外停驻/让出等待依赖，已发布而本地翻译陈旧则 sfence 重试。纯层已有 fault_class/published_until，但 [trap.rs:119](../../os/kernel/src/trap.rs#L119) 将全部非 ecall 用户同步异常直接终止进程。

原始审查将此列为待核实的设计/实现冲突。后续只读检索 CalmStoat 会话，确认轮次4已采用上述按访问类型分类、锁外等待发布/本地同步重试方案，并明确反对无条件 sfence 无限重试；未找到撤销记录。故按既有目标补用户 fault 责任链，不能把纯层分类存在当系统交付。它造成用户进程误杀，与 B1 的 S 态 fatal 隔离缺陷不同；停驻稳定 owner、取消/退出、依赖唤醒和防无进展重试须设计闭合。该会话曾有 vm/fault.rs 实施报告，但最终目标树无相应接入，本篇仍以最终源码为事实；本次没有动态证明误杀窗口。

### D1–D6：实现说明修复清单

| 编号 | 位置与现状 | 目标事实/验证 | owner与接通条件 |
|---|---|---|---|
| D1 | mm.md 62/98/157 后续、internals.md 97：直接完整表帧 owner、PreparedTranslation、精确零帧Unmap/发布时摘表混入生产描述 | 实际 adopt 把物理身份交 PTE、费用归空间聚合，reclaim 重建 funded owner；生产 cursor/最坏供帧/后续摘表。核实际FrameMemory与生产调用；旧 API清理后同步测试说明 | R2，与 S1/S2及对应实现同步，不能用旧测试证生产成本 |
| D2 | task.md 134/154/169：锁秩遗漏及 MEMORY_COMPLETION/PublishedChange/TerminationTodo/drain_gate/prepare_commit 等旧名 | sync::ranks 为真值；按当前 Frozen、稳定 Work、reserve/write/publish/commit/Released 与门外退款描述 | R1/R3，按当前源码修正文档，不复建旧路径 |
| D3 | internals.md 40：timer_queue 写作索引最小堆 | 当前侵入 AVL及稳定pin节点/payload 生命周期，定位源码验证 | R1/R5，与 timer布局候选同步或先独立改正文档 |
| D4 | maintenance.rs 54：RecordInner.root 写成 Running/Blocked 强根；sched idle 注释宣称失败门铃复检闭合 | Running 根在执行器局部，Blocked 在字段；idle失败责任按B1闭合 | R4，root说明可按事实修正，活性说明随失败政策同步 |
| D5 | runtime.md 53/framework.md 39：fs两个 provider Runtime 共用Assembly | srv_fs server.rs 5273 在每个provider进程建自己的Assembly与一个Runtime，按进程边界描述 | R5，亲核真实装配修正文档；framework的回调期限合同随B1修复核对，不降低合同掩盖缺陷 |
| D6 | time.md 13：旧wait_context/request未删、内核不能构建；末尾不存在第8节 | 描述当前time几何/clock/timer消费者，清除施工状态及失效导航 | R4/R5，源码事实核对后清理，运行结果仍由工作记录拥有 |

原始审查只登记差异，未修改impls或ideas。随后施工已将D1–D6回填到当前impls，C1/C2按本篇闭合；本轮没有修改ideas。

## 施工前判断与设计记录补核（历史）

本节保留用户授权继续之前的判断，不是当前施工入口。用户当时询问能否修复/收敛及是否需要与CalmStoat讨论；主代理先只读mesh身份和会话记录，尚未发送消息。该记录支持C1/C2既有选择，不能替代最终源码或证明不存在其他会话决定。后续授权、实际设计讨论和实施结论见以下设计轮次与最终复核。

当时判断六项缺陷及优先候选可由主代理负责闭合、实施、消费者迁移与复核，设计者不逐项审批。原报告把未施工写成未授权，现保留此更正：总纲允许已授权重构内部自主取舍，不能反复把逐步确认设为前置。用户随后明确授权继续，不等于要求无条件采纳所有候选。

有价值的新讨论集中在 R4-B1 的平台故障合同、R5-S1 的跨消费者登记寿命及 C1/C2 稳定推进接缝；恢复既有能力无需重新问是否要提供该能力。R2-S2/R5-S2/R4-S1 在证明与收益前置满足后再判断采纳，未采纳也可形成完整一致的最终合同，不为落实建议编号制造新机制。修复和已采纳收敛按共同责任链统一规划，交付包含 ABI两侧/消费者/旧路径删除/impls/验证，不把每个局部编辑视作独立完成。

## 施工前依赖顺序（历史，已执行）

1. **先闭合施工前提**：R4-B1的平台失败责任；C1/C2已有目标，补齐后代封口与fault等待/重试接入。SUM隔离、Job重派生、守卫与deadline/切分失败修复不必等全部候选完成。这是当时未改代码时的自然顺序，已经执行完毕。
2. **按责任切面实施缺陷修复及必要消费者**：R2 uaccess覆盖读/写/cookie及硬件验证；R3 Job统一角色策略与真实监督；R4守卫及门铃政策；R5非Gate期限同步；R1失败保留切分。相关正常/失败/取消/退出/退款/完成路径在同一切面闭合，不给每函数/编辑独立造阶段。
3. **收敛实施次序**：优先 R2-S1 唯一生产页表路径、R5-S1 登记寿命；R1-S1 与R3-S1/S2按消费者共同迁移；R1-S2/R5-S3可随相关责任链评估。R2-S2/R5-S2/R4-S1须先满足各自前置，再决定是否值得实施，暂留现状不冒充未登记债务。
4. **验证门**：每项最小回归用例/真实硬件窗口按本篇finding，代码接通后依BUILD执行just check/clippy及必要平台/退出组合；修复改动涉及固定栈模型前提、动态目标/析构深度时重新取得对应新ELF证据，禁止套旧数值。没有此类影响则不为形式重复全部栈取证。
5. **复核与关闭**：保留原始finding，逐项写实际修复位置、验证结果、相应责任链与impls变化，主代理沿原反例复核；不另派reviewer确认已收结论。全部已承诺项、C/D与已采纳S语义闭合后，未采纳/延期候选按触发与验证门登记或关闭，本篇才可归档。

以上保留施工前的依赖与自然顺序；原审查任务已归档，修复及最终证据由本篇末尾承接，不重复五切面首审。代码提交未获授权。

## 本轮施工设计与证据

### 授权、基线与设计讨论

施工基线为目标2aee028，进入时只有本篇、原审查计划归档、COMPASS与KNOWN_ISSUES的审查文档改动。主代理负责生产代码、判断和验证；CalmStoat明确本次未亲读报告与源码，仅依据参与过的定稿提供设计候选，不作为已核实源码结论。其确认C1/C2定稿后没有在该会话撤销；讨论通过用户授权的mesh消息进行，先只读避免同工作树冲突。

对R4-B1，保留idle_mask成功路径仲裁，候选为发送方本地平台失败终止；SBI_ERR_FAILED是规范允许的错误，不称规范被违反。须核SRST失败/无后端及runtime_stop跨hart依赖，尚未凭讨论把fail-stop定为完整终态。R5-S1的仿射状态值方向可用，实际SourceOps只有add/remove，不能无证据增加mask就地更新；安装中停止与迟到身份回执必须保留真实token的Runtime退休责任。

### 已闭合的首批实现选择

- R2-B1：SUM直接通道的read/write/cookie均走一个预检与本地全量SFENCE helper；在空间锁内先translate检查，随后SFENCE.VMA再打开SUM。不依赖尚未随每PTE步前进的事务epoch，常数一条fence；Building跨空间物理初始化保留原路径。原有映射锁、失败返回与输出fault owner不变，新增fence费用在现有执行原子内采样。必要硬件窗口验证仍按finding，不用host测试替代。
- R4-B2：LockGuard加入零大小hart-local marker，显式禁止自动Send/Sync；当前消费者不需要跨hart共享守卫，保留Lock<T>自身的Send/Sync规则。用生产类型编译期负向断言确认守卫不能移动或共享、Cell载荷的锁仍可共享；不另建锁框架或运行期owner。
- R1-B1：删除按值split_single，全部消费者改用同一split_off(&mut self)；内核SegmentOwner在既有严格内部边界/同源credit必成前提下转换左右owner。现有broker fixture补credit拒绝开关与单extent非法边界/拒绝仍保留原claim和quota用例，预期来自失败保留合同而非当前输出。
- R5-B1：所有非Gate的stop/unregistered回调在变更业务前兑现该任务已经成熟的期限，回调后读取deadline并同步同一预付timer。Active才武装，Finalizing只继续退休；回调失败保留Task与ready资格并报告原task id。退休推进不再吞掉注销成功后产生的非法deadline；内核Remove失败仍由已有独立来源timer重试。复用FakeSet/Task现有设施验证取消后跨旧期限、回调前已经到期、Remove Busy后真实回执及非法期限owner。
- R3-B1：root/child Job统一Remintable，父表/root anchor决定对象寿命；不把Close改成Seal，继续由sealed/dead控制生命周期。C1随后在同一Job责任链补有界后代封口与出生继承，原子发布/完成顺序以交付定稿为准。

以上是同一责任链施工中的编辑顺序，不是独立交付阶段；最终还需其余B、C、采纳S、消费者/旧路径/impls及整体验证收口。

### 维护执行根与Job后代封口闭合

Work接收者改为对象安全的`self: Arc<Self>`：执行器保留原根到finish结算后的门外析构，clone作为本步调用根；领域直接用该根arm。删除ChangeWork、RetireWork、ProcessShell、ProcessBuilder与Bootstrap五份自Weak及配套leaf锁、构造回填和root方法；Bootstrap.advance借用Arc。旧upgrade均为结构性expect，没有死亡跳过分支。物理Arc数可多于两份，唯一性指执行grant/Record状态；WakeToken仍按既有arm/wake保存责任。receiver析构时执行器原根在，不能成为最后引用。同步动态调用/Arc receiver drop边证明与新ELF，不称旧自Weak已造成计量失真。

JobInner用Open/Sealing{cursor}/Waiting{token}/Dead统一原sealed/dead与传播/等待状态。Seal在Job锁内置Sealing并发布原内嵌Work，随后门外kick，不再仅空Job才发布。每步最多处理一个直接child项（含Reserved占位），门外只调用child.seal，不递归step；游标用单调JobId，源表删除不失进度。游标结束但成员/child未空则同锁arm并保存等待凭证；只有最后依赖释放取得凭证，门外wake。Reservation跨过游标后commit，必须在父锁内读sealed并在父锁外让新child出生即seal/publish；父在该检查后才seal则从0扫描该child。成功/失败预约始终保留表owner，不能依赖调用者再泵。

新增`closing_children`仅表达“父表已摘child但该child尚未发布CLOSED”的在途事实：child Work先在父锁内摘表并增计数，门外退款并发布child CLOSED，再回父锁减计数并判定完成。计数最多同时在步内的hart数；不阻塞、不跨步保存此责任。这样父Work即使已在执行/扫描结束，也不能抢在child CLOSED前宣布parent Dead；保留先摘父表/退款、再child CLOSED、最后parent收束的既有顺序，不新增Retiring表项或第二个名单。最后Process成员回执、预约rollback、child最后完成都由JobInner统一完成判定并在门外wake。Dead后无等待凭证，根在finish末尾结清。

### OrdMap单arena与来源准备成本

R1-S1采用节点内空链：空节点value=None、left存free_next，仅保存free_head；free数直接由nodes高水位减live len推导，不另存计数。required保留高水位以保障换块不重映射索引，reserved仍拥有未消费业务承诺。OrdMapStorage只有一个Buffer，换块移动已初始化节点与空链索引，旧charge随旧块门外返回；take_empty_storage先forget_values避免最终原子扫描，再重置head。现有AVL/BTreeMap模型、复用、并存预约与付款失败/换块外层退款用例迁移到单owner合同，删除两块大小必须相等的实现镜像断言。额外验证高水位空链经过增长后仍可复用、不再分配。

### C2用户fault发布依赖与重试

只处理U态instruction/load/store page-fault（12/13/15）；access-fault、非法指令及其他异常仍按真实fault终止。MemorySpace在Committed整个窗口（含ledger游标已完但PTE/运输未完）提供唯一ChangeKey与目标权限；Map目标区间限定usable，不把guard承诺为可访问。目标不允许则正常fault；允许且软件PTE已含U及相应R/W/X，则锁内本地SFENCE（X加FENCE.I）后重试原指令；允许未发布则锁外等待该事务已有Completion DONE，随后重试原指令。等待到整个事务完成是同一依赖的保守终点，不另造每PTE事件、轮询或新维护actor；不依赖调用者再泵。

VmState增加预付Dependency Buffer<ChangeKey, SignalSource>，容量唯一来自ledger.max_transactions，出生时用原Pool付款；不复制几何/权限。Operation取得纯层reservation时同Vm锁登记来源；仿射Prepared/RetiringChange拥有索引寿命，在rollback/complete释放纯层预约的同一临界区摘索引，来源门外释放。不能推迟到Completion最后析构才摘：纯层已释放的预约会让下一事务进入，而旧调用根/远端回执可延迟最后析构，届时fixed buffer的容量证明失效。预约结束时硬件发布/旧fence依赖已满足，随后最后表运输/退款仍由原Work完成；此前捕获的fault等待继续持原source，到宿主metadata与access归还后由既有Completion发布DONE。access退尽时必须索引空，Body析构返空buffer，Process DONE不越过该资源。查询/摘除至多max_transactions项，register/remove无锁内分配；Source不持被等待的ChangeWork/RetireWork，来源自身通知Work与它们无自持环。

等待返回合同改为显式WaitResult/KernelCompletion/RetryFault{cause,address}，取代kernel_completion bool；RetryFault不写返回寄存器、不加sepc，来源异常则用保存的fault因果终止，Frozen仍统一detach/退出。Thread独占执行字段保存修复事实签名（pc/address/cause、真实PTE翻译、translation epoch及可选ChangeKey）与本组事实已经修复的dense-hart位图，来自context的架构续体数据；同组事实每hart最多一次sfence，已尝试hart间来回迁移不能重置历史；真实事务/翻译事实变化才重置位图。进入真实依赖等待或成功ecall清旧签名。Kernel pte_flags对用户R/X预置A、RW预置A|D，按Svade条件核实，不靠重复fence填A/D。字段不改UserContext汇编布局，归原Thread metadata，执行owner独占访问明确SAFETY。

验证：纯层Committed ledger完成后仍Publishing、Map guard在发布阶段仍拒绝、目标按R/W/X区分；正式多hart消费者测试捕获Map/Protect发布窗口与取消/退出，原Wait detach/epoch及返回pc路径用QEMU核实，host不能替代TLB/指令缓存行为。新的buffer/meta布局、Fault模块动态调用及析构路径纳入最终ELF/stack审计与整体验证。

### R4-B1不可恢复门铃终态

选择复用既有runtime_stop，与clock故障相同；不新增SRST、重试actor或兜底轮询timer。runtime_stop拥有notify_slots：真实Pending/Ready责任先发布，raw IPI失败时只request（Release全局事实并best-effort敲其他admitted），不在调用者可能持锁的栈中check/park。Ready/maintenance同一wake_one路径、remote运输及Freeze执行门铃统一用该policy入口；raw registry try_ipi_slots继续只报failed，runtime_stop自身用raw发送避免递归。

源码已核主循环每轮、trap入口/锁外finish、idle进/出WFI及U量子装填边界的check；已运行U有有限硬件quantum，M和syscall工作结构有界，跨hartfence等依赖用Blocked/wake，不同步自旋等ack。Idle SIE=0的WFI醒来先check再清门铃/调度；无timer的idle本身没有运行用户或推进内核责任，若后来任意唤醒仍先park。REQUESTED Release/check Acquire，STOPPED AcqRel只有首个转换者报告；PARKED仅实际进入Rust park的证据，不是所有admitted的完成barrier，也不拿它等待强制回收。全局不可恢复终态不再执行U及推进内核责任，资源留到机器重启；已发设备/DMA不被撤销，不声称设备静止。

针对性验证用正式affinity消费者/真实Ready发布，GDB在选定SBI IPI ecall前跳过一次并设置规范允许的失败返回，避免产品加入专用消费者/测试运行体；核无timer WFI目标、REQUESTED/STOPPED、后续唤醒先park与无后续U执行。此开发证据与正常聚合路线分别记录；现有成功路径idle_mask仲裁保留。

### R5-S1任务侧来源值

SourceRegistration拥有私有Empty/Installing(kind)/CancellingInstall(kind)/Attached(id)/RemovePending(id)/Removing(id)相位；无Clone/Copy，不持handle/token，不Drop注销。install/remove把Requests入队成功与相位转移放同一方法，失败保持原值；安装中取消记录明确意图，registered校role并转RemovePending；下一业务推进请求Remove，Runtime维持真实重试直到unregistered。迟到旧id的unregistered不清新登记，安装refused只返匹配kind的命令责任；真实registered无匹配安装属于结构错误，不静默丢token。业务保留mask、结果、终态/公平性，SourceId仍是不透明借用cookie；没有mask就地更新，保持原remove/reinstall协议。

先迁移Observation/Dispatcher/Outbox和实际fs/pm来源任务，全体同步删除旧requested/installing/removing表示，不保留delegate facade。借已有Runtime/FakeSet用例验证命令拒绝、安装中停止、迟到身份、Remove重试/退休，mask替换由真实RPC服务验收；所有Task.advance错误仍走原refuse_all确保已排队命令回执返还，不能把Installing悬挂成无真实责任。

### 通知与引用归还的最终协议（设计轮次4）

publish合并通知及wake活跃记录归还多余Arc时，在QUEUE锁内借Queued/Running/Blocked稳定根保证非最后drop；不能解锁后凭旧状态推断非last。wake对Idle/Done返回潜在最后根，调用方带出全部领域锁后drop。SignalSource把内部DONE/internal_done提交与wake状态捕获放在同一来源临界区：唯一内部等待者不能先读新完成事实、finish，再把最后析构递归回发布者。来源门外清理返回值并kick，原有其他调用方也在锁外清理点接回返回根。

OBJECT_WAIT(500)→MAINTENANCE_QUEUE(660)只做记录状态转换；Work.record合同为纯字段投影，15个生产实现均核实，不运行step/领域回调。Shell的内部DONE waiter持续到真实退款，不提前取消；VmAccessLease的等待目标仅Shell/Builder，但lease本身不保活它们，因此保留可能最后析构；remote最后ack的Change/Retire根亦保留。主代理核实Shell/Job/Bootstrap其他同宿主token由原lease、parent或Delivery Arc保活。没有新增退休队列、自引用或回调接口。

对这组新增协议作一次聚焦独立审查（sub-12），结论无阻断发现：核QUEUE内真实非last、来源→QUEUE锁序、finish竞争、Idle/Done锁外返回、内部waiter不可提前放弃及原根到finish后的寿命。该审查覆盖新增协议，不是重跑五切面首审；完整原始输出保存于pi subagent history，最终论证不依赖其记录才能理解。

## 最终复核与交付

### 处置与责任链

| 项目 | 最终结构及复核结论 |
|---|---|
| R2-B1 | read/write/cookie统一translate→本地全量SFENCE→SUM直访，均在同一空间锁内且不让出；不会再采用预检之前的旧权限翻译。Building物理初始化保持独立合同。固定ISA允许陈旧权限的反例已被这条同步切断。实际三入口指令顺序诊断通过，未宣称动态复现旧TLB内容。 |
| R3-B1、C1 | root/child Job均Remintable，父MANAGE授权不随最后子cap消散；Open/Sealing游标/Waiting/Dead统一寿命。每步一个child、Reserved跨游标后出生封口，closing_children保证child CLOSED先于parent完成。正式init回归覆盖关闭最后cap后重派生、嵌套空子树只Seal父、CLOSED及父表移除；创建者退出/监督由现有组合覆盖。 |
| R4-B1 | Ready/Pending先发布，统一notify_slots在IPI失败后request runtime_stop；锁外安全点停止U及内核推进，无期限WFI后续任意唤醒先park。失败政策不依赖IPI必成，不承诺设备/DMA撤销或所有hart强制完成屏障。真实失败注入通过。 |
| R4-B2 | LockGuard带hart-local marker，显式!Send/!Sync；实际生产类型编译期负向约束与正式check/clippy通过。锁本身仍按T:Send提供共享能力。 |
| R5-B1 | 非Gate stop/unregistered先成熟已到期责任，回调后同步原预付timer；Finalizing不重装业务期限。错误保原Task/id与清理资格，Remove Busy仍独立重试。现有fixture补取消后跨旧期限、先到期、重试回执、非法新期限，全部通过。 |
| R1-B1 | 删除split_single，唯一split_off借用原owner，两个非法边界及credit拒绝都保claim/credit；最后释放才退款。kernel FundedExtent同步迁移，host回归通过。 |
| C2 | Committed整个事务窗口提供Key/目标R/W/X，guard/Unmap不允许即终止；允许未发布捕获已有Completion来源，释放VM锁、Body短loan后沿attach→probe→park等待；Frozen可detach，不等DONE。已发布则真实PTE/A/D检查、每组事实每hart最多一次SFENCE，X加FENCE.I，原pc/GPR重试；重复无进展正常终止进程。真实发布窗口与模拟修复诊断分别通过。 |
| D1–D6 | mm/task/internals/runtime/framework/time按当前生产路径、付款/退款、锁序、Source及timer布局回填；删除旧API、旧类型和失效施工说明。运行结果由本篇拥有。 |

正常、拒绝/回滚、提交后取消/退出、超时与最终退款各沿原仿射Operation、Source Runtime、Thread/Shell/Job及持久Work owner收束。C2等待不持VmAccessLease/结果租约/表锁或Body loan；DONE在metadata/access归还之后，Frozen等待线程按detach先离场，不形成回收环。平台不可恢复失败由runtime_stop持有全局终态，保留资源直到重启，此时正常释放合同不再推进。所有范围内消费者已接通，没有测试专用内核actor或产品开关。

### 收敛结果与保留边界

采纳R1-S1/S2、R2-S1、R3-S1/S2、R4-S1、R5-S1/S3共8项：OrdMap一个arena/free_head；稳定写许可省ObjectSource候选；TableTree唯一运行期PublishCursor并迁移原真实owner/退款测试；删除Builder abandoned、Departure bool锁和5份Work自Weak；一次M始末采样、构造期账户量子缓存；SourceRegistration迁移实际libprocess/librpc/fs/pm/FAL任务；两个任务timer共享一个Pin宿主。被替换的公开旧接口、冗余状态与准备/回填路径均删除，不留adapter。EagerMapper仍只用于未发布出生树，Work调用根与Blocked根责任仍独立。

R2-S2本轮不采纳并关闭候选：最坏表帧预付仍由page_table拥有，保证cookie后必成；不能用会被不相交事务改变的树快照减供帧。以后若有准备成本/额度峰值证据，由VM/page_table设计另行证明需求只减或门外补齐/复检，再共同替换；验证须覆盖并发摘表、大页拆分、碎片与付款拒绝。R5-S2本轮不采纳并关闭候选：librpc保留借用ReplyPort的Caller和自持多事务的Dispatcher两种owner，SourceRegistration已消除共同的登记重复；未来出现可共享的事务owner/port归还设计时才重开，必须同时覆盖Request返还、已发超时、迟到回复、Delivery abandonment及cleanup失败。两项均非过渡实现或当前正确性欠账，没有承诺待实施的半套通用机制。

### 最终验证

完整日志统一在`artifacts/object-chain-fixes/`，专项日志使用独立时间戳目录。有效结果与边界如下；命令失败日志仍保留，未用后续成功抹去失败。

- host：funded-frame-host-002共13项；memory-space-host-003共48项；page-table-host-001共32项；user-host-004的libexecution/libfal/libprocess/librpc共33/39/4/9项，全部通过。显式host target为aarch64-apple-darwin。页表随后只删冗余plan字段，正式target门覆盖最终接口。
- 正式静态门：`check-003.exit=0`、`clippy-003.exit=0`。clippy覆盖七分面。之后只撤销6份无关格式噪声、修一行过时kick注释及改诊断工具/文档；没有改变Rust执行语义。
- `acceptance-003.exit=0`：clippy、debug完整stress/16竞态矩阵、release core、sifive_u预期NotSupported收束、nofd、三种boot失败、IPI失败、真实VM fault及R/W/X/repeat诊断全部通过。`hetero-001.exit=0`覆盖异构域完整core收束。
- IPI最终聚合证据：`artifacts/ipi-failure/20261007T110716.594242Z-96963/evidence.json`，目标确为no-timer WFI；真实SBI IPI被跳过一次并返回FAILED，后续真实广播唤醒后4 hart停驻。早期ipi-001静态目录曾被覆盖，其旧记录作废，不引用为固定证据；ipi-002及最终聚合均在独立目录保存。
- VM fault最终聚合证据：`artifacts/vm-fault/20261007T110720.448950Z-97305/evidence.json`，真实cause13、地址0x194ff8进入RetryFault，cookie-before-return=true，随后完整stress收束。此前acceptance-002退出1：旁观已取得同一真实fault，但GDB继续陪跑完整stress而沿用240秒，总预算小于原stress的420秒；无内核故障证据。脚本现首次观察后detach，GDB与后续正式负载共用继承stress的一个总期限；vm-fault-003及最终聚合均通过。
- 扩展VM repair：`vm-repair-004.exit=0`，`artifacts/vm-repair/20261007T111221.038654Z-6380/evidence.json`。真实PTE/用户现场上模拟R/W/X原因与地址，实际执行SFENCE，X另执行FENCE.I，GPR/sepc完整保留；同组事实重复故障两次即进入冻结。另直接旁观真实copy_from_user/copy_to_user/publish_u64三入口，每次SFENCE先于SUM打开。专项结束后清理guest，不把它冒充完整profile验收。
- 最终`just build_kernel`通过，ELF为下节b3d821…；与最终聚合a1cfb5…的全部可执行区段相同。撤销共享ABI文件的格式噪声仅使37份24字节panic Location的line/column变化，其file指针/长度及其他映射字节不变；比较见`stack-final-verified/acceptance-image-comparison.json`。最终修复诊断与栈证据直接针对b3d821…，无需为格式/注释重复整套正常验收。

早期开发失败均已定位并修复：Views锁内identity导致250 under325；Stream两处多行raw register漏迁；PM FlowTask族漏转登记回执。其后core-004、virt-stress-003与正式聚合通过，最终选定消费者不残留raw register/add_source旁路。VM repair开发的FENCE.I反汇编显示为.4byte、GDB有符号a2转换均是工具缺口，已按真实编码与64位mask修正；未改产品语义绕过验证。

R2-B1原建议的双hart精确旧TLB窗口没有动态复现。本轮采用固定ISA保证、空间锁排除后续PTE改变、最终指令顺序与实际三入口执行证据闭合修复：SFENCE之后不得再用此前权限翻译，是否碰巧重现旧缓存不影响这条证明。C2的真实发布等待与修复分支采用不同证据，模拟cause/stval不宣称真实缓存竞争。未覆盖真实芯片微架构、终态诊断栈、WCET/精确CPU份额、全分配点失败注入与DMA静止；均不扩张为本轮交付承诺。

### 新ELF累计栈与拒签证据

最终virt/debug ELF SHA256为`b3d821af407b0b1b36daf78326c1b665938aec43fb95633266958fcff3a5c2f5`，取证目录`artifacts/object-chain-fixes/stack-final-verified/`；`facts.json`、`sites.json`、`receivers.json`、`certificate.json`与`bound.json`共同拥有证据。旧294a…的120000/119216和旧语义脚本未复用。原prefix辅助调查sub-10超时，只作为未完成调查；主代理亲读入口、真实调用点及最后生命周期，所有数字重新来自最终ELF。

四份`tools/maintenance-stack-{facts,sites,receivers,bound}.py`按参数处理不可变ELF：按地址保留符号身份/别名，提取直接调用/尾跳/真实CFI；未知CFI形式拒签，只有精确ret叶函数可给零帧。176个间接site登记Work/Observation/Hint/Object等固定binary候选；55处局部jr核只读section中的实际跳表、slli/auipc/addi/add/ld寄存器链和固定源码tag/range，不能把jr自动当局部分支。M不可达的49个未决site保留，不拿名字过滤后冒充已证明。service可达集合内未知site/直接转移/CFI均为0。

15种生产Work及Placeholder保守候选都通过同一真实service根覆盖；精确DWARF self槽和调用前a0别名证明17/21 receiver drop非last，其余同类型child/parent Arc保留完整路径。终态字段剪边逐类绑定源码：Record next/root、Job等待根、Thread等待/定时器、Shell Body与各waiter、Builder shell/hint、Reclaim retirement、Members slot、Wait Entry source、Transport wake、VmState space/依赖buffer和Change/Retire终态。保留每层空Option/Buffer/slice的实际CFI、费用与存储退款，不剪完整包装函数。真实Completion.source、访问归还、remote最后ack、finish退休根及财务父链继续计入。

递归界重新推导：system heap唯一来自16×1MiB供给；本根可达ordered_table/OrdMap实例的节点及排序RegionTemplate在RV64至少32B（节点key/child边/载荷，Template仅PageRange已有两个usize，另有allocation/owner/kind），故N≤524288。AVL最少节点递推给最大高度27，再含空基例深度28。固定rust-src ipnsort入口limit=2*floor(log2(len))，quicksort每轮递减且0转heapsort，模型取2*bit_length(N)=40含基例；median3_rec每层n/8，取ceil递推深度8。各SCC分别累加自身真实帧后乘深度，DAG最长路径继续计完整后继。

Account/Pool财务环核每次重入沿不可回头的父祖先下降：Account先take父根并在本层退款/移债后释放，Pool child返还父credit；shared ABI最大深度32，模型保守取33。最终SCC为18成员、帧和832B，不沿用旧912B。正常格式化仅九种实际Argument构造：usize/u64十进制与十六进制、u32大写十六进制、&str、两种无递归错误枚举，以及Console包装的一层Arguments。沿实际构造函数常量函数指针/Write vtable闭合21个callback；Console本身不递归格式化载荷，链上只有原消息和外层标签Arguments。模型取全部callback数+2共23层覆盖每成员可能重入，SCC38成员帧和4208B；仅锁阶断言失败构造的诊断参数从正常路径剪出，锁函数自身CFI保留。没有使用“所有fmt天然深度3”或旧2720B常数。所有29个剩余SCC归入上述有独立依据的递归域，没有未知业务环。

正式汇编重置sp，bootstrap临时栈不叠加。service唯一物理调用者sched::run的五处调用共用实际前缀：hart_formal_entry288B、bring_up_runtime784B、sched::run1216B，boot2288B、secondary1504B。M内部保守累计171440B，加前缀后boot173728B、secondary172944B。链接符号STACK_SIZE262144、EMERGENCY_SIZE4096，正常可用栈258048B；boot余量84320B。结论仅正常M及其入口，不签terminal/panic/fatal诊断栈或WCET。

持久证书`tools/maintenance-stack-certificate.json`绑定最终ELF、三份输入、模型工具及os/shared（含assembly.asm）和实际rust-src core/alloc共563份源码hash，并复读实际ELF字节核hash；没有证书只输出UNSIGNED并清空可交付bounds，任何源码/输入变化要求重审语义，不能只换ELF hash签旧数。五项负向检查分别删service CFI、加未知动态site、引入无界业务自环、改源码绑定、替换实际ELF字节，均退出1、UNRESOLVED且bounds为空；结果见`stack-final-verified/negative/results.json`。

复算命令（证书须来自本轮已核语义，生成新facts本身不授权新的证明）：

```sh
python3 tools/maintenance-stack-facts.py artifacts/object-chain-fixes/stack-final-verified/kernel.elf artifacts/object-chain-fixes/stack-final-verified/facts.json
python3 tools/maintenance-stack-sites.py artifacts/object-chain-fixes/stack-final-verified/facts.json artifacts/object-chain-fixes/stack-final-verified/sites.json
python3 tools/maintenance-stack-receivers.py artifacts/object-chain-fixes/stack-final-verified/facts.json artifacts/object-chain-fixes/stack-final-verified/receivers.json
python3 tools/maintenance-stack-bound.py artifacts/object-chain-fixes/stack-final-verified/facts.json artifacts/object-chain-fixes/stack-final-verified/sites.json artifacts/object-chain-fixes/stack-final-verified/bound.json --receivers artifacts/object-chain-fixes/stack-final-verified/receivers.json --certificate tools/maintenance-stack-certificate.json
```

### 交付前结构检查与完成门

主代理按REVIEW核对原六项反例、C1/C2设计、ABI两侧实际消费者、正常/失败/取消/退出/超时/退款owner、跨hart锁序和最后析构。重点复核TaskTimers私有pin投影及先摘两链、FaultRepair执行owner独占UnsafeCell、权限A/D预置、来源容量/Key同锁寿命与DONE握手；没有static mut、新回调反向依赖或并行真值。Work.record纯投影、Source→QUEUE及所有返回潜在最后根的锁外释放位置核实；非last证明与最后退款分别进入栈模型。运行期旧页表API、split_single、五份自Weak、旧来源布尔、Departure锁和Builder abandoned均删除。D1–D6当前事实与代码一致，本轮未修改ideas。

本次能力/非目标明确，必要消费者与前置接通，原六项缺陷及C/D全部处置，已采纳收敛没有过渡残留；各层验证与结构检查通过。剩余优化候选已关闭本轮提议，不留下依赖它们才能成立的承诺。进程/测试均由各自诊断器回收，最后核本会话进程组无遗留；本报告归档，COMPASS删除失效下一步，KNOWN_ISSUES删除本范围矫正条目。尚未提交；提交后独立Review须按实际获授权的新固定提交/序列登记，不以本轮旧目标首审代替未来审查。
