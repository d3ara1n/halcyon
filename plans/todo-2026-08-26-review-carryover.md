# review 未闭合承接项收拢

> 各 review 文件的调查本身均已收口并归档至 `archived/`，但其中登记的承接项、验证要求与文档整改不随归档消失。本文件是这些项的唯一跟踪点：逐条列出内容与触发条件，完成后勾销或转入对应正式计划。

## 来源 [`review-2026-08-ipc.md`](archived/review-2026-08-ipc.md)

- [ ] **A5 设备接入重审**：静默论证须随设备/中断接入重审；新等待不得让对象订阅保有线程。（触发：设备/中断接入设计前）
- [ ] **压力验证线**：消息风暴覆盖 MailboxFull/ObjectBusy/回滚且资源守恒；隧道 create/attach/write/close 与退出风暴帧守恒；host 双线程 + RISC-V 双 hart Acquire/Release 压测；`sifive_u` 五核既有集成负载连续至少十轮。（触发：用户态多线程落地、正式服务编排接入前）

## 来源 [`review-2026-08-26-notes-design.md`](archived/review-2026-08-26-notes-design.md)

文档结构问题（2026-08-26 核查均未处理；2026-08-28 拍板与生命周期 step 10 文档终态收口合并执行，见 [todo-2026-08-26-process-lifecycle.md](todo-2026-08-26-process-lifecycle.md) 实施顺序 10）：

- [ ] `ideas/device.md` 与 capability/FAL/Notification 方向冲突，整体重写；
- [ ] `ideas/ecs.md` 仍是应用构想，降级到 robotics/application 子域或 plans；
- [ ] 协作式内核、执行环境、调度域、内存模式等方向性结论主要藏在 impls，补 idea 层唯一归属；
- [ ] ideas 中「当前实现」「尚未接管」等施工状态表述移至 impls/plans；
- [ ] impls 中历史选型、施工顺序和未实施方向移出至 review/todo/ideas；
- [ ] wait/signal、fs/fal 重复定义同一不变量，Deadline 术语漂移需统一；
- [ ] `notes/README.md` RPC 索引重复且主题映射不准确。

方向性后续（原「建议后续顺序」，已完成项已删；fs 真路径验收后的一次性 notes 修订与上述结构整改同属 step 10 批次）：

- [ ] fs 真路径验收达成后，一次性修订 object/message/startup/task 等 ideas 与对应 impls（核查是否已随各批提交同步完成）。

## 来源 [`review-2026-08-27-mechanism-generalization.md`](review-2026-08-27-mechanism-generalization.md)

机制层设计 review（特例补丁与可泛化机制）的未闭合承接项：

- [x] **Lock Ladder**：已落地（2026-08-27）——`sync::ranks` 秩表 + per-hart 秩栈断言（同秩链段 key 递增：Job 链锁 jid、HandleTable pid）；bootstrap 专用帧经 formal entry 汇合点切换；talc 经 `RankedRawSpinlock` 类型级注入。锁序契约已按实测重写（impls/task.md），全负载 debug 构建验证无违规。
- [x] **收束分层公理 + role 叶子公理**：已落地（2026-08-27）——ideas/object.md 新增「收束分层」节，object.rs role 注释同步；close fanout 上界由公理推导而非枚举重审。
- [x] **MappingLease / StackVA**：已落地（2026-08-27）——`MappingLease` RAII 凭证（release 唯一解除路径，close 显式 + Drop 兕底，create/attach 失败回滚自动解除）；`phys_to_virt` debug 断言拒绝栈物理打包区。
- [x] reserve/commit/rollback 协议成文（impls/task.md「reserve/commit/rollback 协议」）；第四处出现时评估共享骨架。
- [x] TIMERS per-hart 化已落地；锁优化判据成文（impls/internals.md「全局状态分层」）。

## 来源 [`review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)

- [x] **F4 Job「预算」**：已处理（2026-08-27 Job 管理面设计拍板）——ideas/task.md 补一句话契约（域内成员资源总量上限记账，接入时点待需求出现），Job ABI 不含预算面。
- [x] **F2 ready-marker 预留域路由**：已落地（2026-08-28，step 8）——reserve/commit/rollback 上收为 `SchedClass` trait 契约（域路由按 eligibility 选定目标类），实施与验证见 [todo-2026-08-28-domain-eligibility.md](todo-2026-08-28-domain-eligibility.md)。
