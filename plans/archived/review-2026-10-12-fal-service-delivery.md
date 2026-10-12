# FAL/OAL/fs/服务目录交付独立 Review

> 审查状态：**未发现新的可达代码 finding；发现 5 项证据缺口，交由既有唯一 owner 承接。** 本报告只读产生，未修改生产代码、`plans/COMPASS.md` 或其他 Review 计划。

## 固定范围与基线

- 主交付：`a1ca124fdb70c1f20034f3e7d24d587f4323c98e`，父 `4523bbfe65287f6b84ab73d9f034dae1a526d38d`，tree `6a3856af8690a9e0deb17da08410c8166a0ed63a`。
- 验收整理：`a06e455d064429d2e6e9984b03474a62104fb9e2`，父 `985fb179a7455d0b251c306007f92d4e8d64b5ce`，tree `19f4d681869b9ed65d4da3ff4697e4517d6ccc28`。该提交是主交付的后代；验收树中的 FS1 前缀同步覆盖了生产消费者和 `tools/qemu-acceptance.sh` required 集合。审查没有用当前 HEAD 替代上述固定树。
- 入口规范：`plans/REVIEW.md`；设计/历史输入：`plans/todo-2026-10-07-fal-service-redesign.md`、`plans/archived/todo-2026-10-11-oal-fs-service-review.md`、`plans/archived/todo-2026-10-11-acceptance-cleanup-review.md`。
- 实现说明核对：`notes/impls/fs.md`、`service.md`、`rpc.md`、`framework.md`、`ipc.md`。

## 覆盖范围

审查了以下责任链及其真实消费者：

- `liboal` 的 `ObjectInfo`、Namespace/Walk、Boundary、路径预算和发现出口；
- `libfs` 的 MemoryBackend、Access/Management/Grant、目录范围及 ceiling、版本/permit、Watch、Bind/Unbind、Stream/Finish、Copy、PendingOutbound；
- `libservice` 的 Registry、Authority/Access、Root、Registration/Watch、真实 sender context、Rights 衰减及控制/业务入口；
- `librpc` Caller/Dispatcher/PendingCall、Outbox、Reply/Delivery owner 与 deadline/cancel/late-result 路径；
- `srv_fs` 双 mailbox、provider/boot、Root/Lifetime/退休、Register/Withdraw、Runtime/Assembly/预算付款；
- `srv_init`、`test_fs` 的真实装配、消费者退出、provider 监督、日志及 `tools/qemu-acceptance.sh` 的 profile/退出码/rejected/required 判定；
- C01--C11 清理结果及旧 `libfal`、`DirectoryGrant`、两阶段发布、路径回删兼容残留；上述旧符号在固定交付树的源码/Cargo/Justfile 引用核对为零。

已核对并未复制既有审查已修复的代码反例：同目录并发 Create、准备后移出授权根、版本耗尽、Rename 替换/防环、Watch 序号耗尽、Retire runnable 自旋、Rename target 自身事件、付款入口与控制壳、queued/unknown Copy 事实保留等均有当前代码或对应 host/guest 证据。`a1ca` 中旧 `FAL2` 日志前缀由后代 `a06` 同步改为 `FS1`，不是把两个提交混成未固定的 HEAD。

## Findings

### F-01 / P2：真实来源 Remove/Close 拒绝或延迟窗口仍只有结构证据（证据缺口）

- **固定位置**：`user/libraries/libfs/src/provider/outbound.rs` 的来源收束、`user/libraries/libfs/src/watch.rs` 的终态/退休、`user/libraries/librpc/src/dispatcher.rs` 与 `user/libraries/libexecution/src/source_registration.rs` 的 Remove/注销重试；对应工作记录的 F/W05-01 条目。
- **可达路径**：已准入 Watch/Stream/Grant/Copy 责任正在停止或取消时，正式来源的 Remove/注销返回可恢复拒绝、延迟或未及时回执；然后 provider 仍须保持原 Request、角色、Charge、映射/Binding pin，Parked 等待同一真实回执，不能连续 Runnable、自行重新扣账或提前退款。
- **契约**：来源未收到注销回执前不关闭物理 owner；失败保留原 owner/费用并按期限重试；最终回执后才退休、退款和唤醒业务。该契约在 `notes/impls/fs.md`、`rpc.md` 及主计划 F/W05-01 表中一致。
- **影响**：当前正常 guest 只证明成功注销和最终零账，不能证明拒绝/延迟下不会自旋、丢角色、重复收费或过早关闭映射。不能把普通 reset/全零账声明为完整失败责任链覆盖。
- **建议归属**：RubyRaven 实施，MellowPond 协调；仅在既有正式 SourceOps/内核接缝可控地产生真实拒绝/延迟时补证，不新造 fake source、测试服务或测试运行体。
- **性质**：证据缺口，非当前已复现代码 bug。

### F-02 / P2：准入后母授权撤销与异步迟到能力交错未有真实运行证据（证据缺口）

- **固定位置**：`user/libraries/libfs/src/grant.rs`、`backend.rs` 的派生目录范围/ceiling/活动位复核，`provider/outbound.rs` 的 `Boundary`/Bind PendingOutbound，`user/libraries/libservice/src/root.rs` 与 authority 表的 revoke 生命周期。
- **可达路径**：Directory/Boundary/Bind 请求已经完成准入并在 Dispatcher 中等待下游时，母 Grant/Management root 被显式 revoke，或迟到的 sender/shared/binding 能力到达；提交必须返回 `Revoked`/失败并释放临时能力，不能删除替代 Binding、按名字重新选择目标或泄漏原付款责任。
- **契约**：准备与提交均复核当前 root 范围、逐祖先链、活动 lease、ceiling、稳定 Binding ID、期限和取消状态；普通 sender 关闭不等于 revoke，已准入快照的寿命与入口索引分离。该边界在主计划 G05-19/G05-21、`notes/impls/fs.md` §授权与 `service.md` §Root 中明确。
- **影响**：host 回归已覆盖稳定范围、ceiling、显式活动位及迟到 Unbind 的局部事实，但没有在真实 Kernel sender/Dispatcher 接续窗口关闭母入口或 revoke；因此尚不能证明迟到 capability 不会穿透替代者、不正确退款或把未知结果交给新请求。
- **建议归属**：RubyRaven/MellowPond；复用正式 Bind/Boundary/Dispatcher 消费者，在真实暂停点取得母撤销/关闭和迟到回复证据。
- **性质**：证据缺口，非当前已复现代码 bug。

### F-03 / P2：Notification N6 的 mint-specific 失败与逐壳付款未完成端到端验证（证据缺口；对象链边界）

- **固定位置**：用户态 `libkernel` Notification typed Owner/Signaler/Observer 采用边界、`libservice`/`libfs` Watch 的通知角色交付与终态壳；底层对象 owner/WaitQueue/ABI 基础归 HighHolly 的 kernel/object-chain 报告，本 finding 不复制该基础责任链。
- **可达路径**：Notification observer/signaler 铸造或安装遇到 quota、分配失败、KOID/输出槽错误、Frozen、接受后 Close，或最后控制/观察壳在不同 payer 下退出；服务必须逐项返还未交付 owner，保留已接受 CLOSED 观察责任，且每个壳的 Charge/Lifetime 只在真实 owner 结束后退款。
- **契约**：五种 typed 角色及其 rights 出口、信号/观察/付款/业务关闭/物理保活必须分别成立；正常成功路径不能替代失败安装和接受后 Close。`notes/impls/ipc.md`、`service.md` 及 Notification 前置 N6 明确列出这些窗口。
- **影响**：固定验收覆盖了三角色负向签发、pending 保持及邻接退款，但 N6 记录仍明确把 mint-specific quota/分配/KOID/output fault/Frozen/accepted-close/逐壳费用列为源码或 host-only；当前交付不能宣称 Notification 全责任链已由真实服务验收闭合。若 HighHolly 报告发现底层 owner 不成立，本报告只承接其对服务端交付的影响，不重复底层 finding。
- **建议归属**：Notification 前置 owner（RubyRaven）与 MellowPond；在已有真实验收入口可注入时补证，保持现有 ABI 和 owner，不增加测试专用机制。
- **性质**：证据缺口，非当前已复现代码 bug；基础对象链属于明确排除项。

### F-04 / P2：W05-04 历史 Watch 准入未知错误未能复现或定位（证据缺口）

- **固定位置**：`user/tests/test_fs/src/watch.rs` 的 quota/Watch 饱和消费者，`user/libraries/libfs/src/watch.rs` 的准入与控制 Lifetime，`user/libraries/libexecution` 的 source/付款注销路径；验收入口为 `tools/qemu-acceptance.sh` 的 FS1 Watch required 锚点。
- **可达路径**：在相同部署额度、deadline、平台节流或源注销时序下，Watch 饱和循环收到非 `Quota`/非预期传输错误，或控制 Lifetime 注销/退款滞后；消费者必须报告真实 status/count/clock，不能把错误吞成 Quota、延长业务期限或以诊断打印改变时序。
- **契约**：每次准入使用独立有限期限；只有明确 Quota 才继续饱和，其他错误立即失败并保留 owner/退款责任；未复现不能写成已修。主计划 W05-04 及 `notes/impls/fs.md` Watch/退休段落均如此限定。
- **影响**：六轮后续普通/压力回归未再复现，且 FS1 Watch 正常合同到达 reset；但原始轮次缺少具体 Err，诊断路线改变时序，根因、资源先耗尽候选和控制退款窗口仍未排除。未来平台/限额/源码变化可能重新出现非 Quota 错误，当前验收无法证明其不会误映射或遗留费用。
- **建议归属**：RubyRaven/MellowPond；触发条件是相同 panic/非 Quota 或相关部署/平台变更，补获真实 Err、准入 count、clock、Lifetime 注销/退款序列和最小 host 回归。
- **性质**：证据缺口，非当前已复现代码 bug。

### F-05 / P2：Copy/Dispatcher 的精确故障窗口仍未覆盖（证据缺口）

- **固定位置**：`user/libraries/libfs/src/client/copy.rs`、`user/libraries/libfs/src/provider/outbound.rs`、`user/libraries/librpc/src/dispatcher.rs`/`outbox.rs`；真实消费者为 `user/services/srv_init/src/filesystem/copy.rs`，验收入口为 FS1 Copy required 集合。
- **可达路径**：Copy 已发送下游后收到 malformed/错型迟到结果、Delivery 写回失败、服务端在非零目标 accepted 后取消，或 Dispatcher 封口时仍有 live waiter。业务必须保持 Unknown/已确认前缀，不重放、不按名字回删替代者；原 PendingCall、Reply/Delivery、源/目标 session、付款及物理清理 owner 要继续收束。
- **契约**：未知结果不可当成功或失败重放；取消只冻结停止意图；Dispatcher/Outbox 的完成与来源注销不能丢失原任务领取责任。`notes/impls/rpc.md`、`fs.md` Copy 段以及主计划 G05-17/G05-22/F 表均将这些作为独立窗口。
- **影响**：固定验收已覆盖正常 Copy、目标接受前取消、sealed live waiter、业务 deadline、provider shutdown、queued/unknown 的既有场景；但 malformed 迟到/Delivery 故障和“非零目标 accepted 后取消”仍是 host/源码或未注入窗口，正常成功锚点不证明这些失败分支无 owner 泄漏、替代者破坏或错误退款。
- **建议归属**：RubyRaven/MellowPond；使用正式 Dispatcher/Delivery/Copy 消费者的可控故障入口补证，不能新增 Copy 专用运行体或把当前 required 文本放宽为可选。
- **性质**：证据缺口，非当前已复现代码 bug。

## 排除项与交界

- kernel Object/Capability/Lifetime/WaitSet 的基础 owner、锁序、物理 ABI 与 VM/funding/Work/Job 底层契约转交 HighHolly；本报告只记录其对 liboal/libfs/libservice/RPC/装配交付的依赖影响。
- 设备、磁盘持久化、高层跨域 Move、正式服务监督不在范围。
- 当前未发现 C01--C11 删除残留、旧 `libfal`/`DirectoryGrant`/两阶段发布/路径回删兼容出口，或新的 sender-context/rights 出口代码反例。

## 结论与残余风险

固定代码树的正常、失败（已有注入）、取消、期限、退出、退款和退休结构与五份 `notes/impls` 一致；`srv_fs`/provider/init/`test_fs` 已真实装配，验收脚本对 profile、退出码、日志拒绝和 required 集合的处理与 FS1 生产日志同步。主交付可以作为**当前正常能力已接通、但精确故障窗口仍分级延期**的固定 Review 输入，不能把“无新的可达代码 finding”写成“所有交错和失败窗口均已验收”。

残余风险唯一由 F-01--F-05 及其既有 owner/触发条件承接；若这些窗口没有正式可控接缝，按项目规则保留证据缺口，不以 fake capability、测试服务、扩大 deadline 或删除 required 锚点伪造闭合。
