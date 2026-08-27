# review 未闭合承接项收拢

> 各 review 文件的调查本身均已收口并归档至 `archived/`，但其中登记的承接项、验证要求与文档整改不随归档消失。本文件是这些项的唯一跟踪点：逐条列出内容与触发条件，完成后勾销或转入对应正式计划。

## 来源 [`review-2026-08-ipc.md`](archived/review-2026-08-ipc.md)

- [ ] **A5 设备接入重审**：静默论证须随设备/中断接入重审；新等待不得让对象订阅保有线程。（触发：设备/中断接入设计前）
- [ ] **压力验证线**：消息风暴覆盖 MailboxFull/ObjectBusy/回滚且资源守恒；隧道 create/attach/write/close 与退出风暴帧守恒；host 双线程 + RISC-V 双 hart Acquire/Release 压测；`sifive_u` 五核既有集成负载连续至少十轮。（触发：用户态多线程落地、正式服务编排接入前）

## 来源 [`review-2026-08-26-notes-design.md`](archived/review-2026-08-26-notes-design.md)

文档结构问题（2026-08-26 核查均未处理）：

- [ ] `ideas/device.md` 与 capability/FAL/Notification 方向冲突，整体重写；
- [ ] `ideas/ecs.md` 仍是应用构想，降级到 robotics/application 子域或 plans；
- [ ] 协作式内核、执行环境、调度域、内存模式等方向性结论主要藏在 impls，补 idea 层唯一归属；
- [ ] ideas 中「当前实现」「尚未接管」等施工状态表述移至 impls/plans；
- [ ] impls 中历史选型、施工顺序和未实施方向移出至 review/todo/ideas；
- [ ] wait/signal、fs/fal 重复定义同一不变量，Deadline 术语漂移需统一；
- [ ] `notes/README.md` RPC 索引重复且主题映射不准确。

方向性后续（原「建议后续顺序」，已完成项已删）：

- [ ] fs 真路径验收达成后，一次性修订 object/message/startup/task 等 ideas 与对应 impls（核查是否已随各批提交同步完成）。

## 来源 [`review-2026-08-26-bootstrap-launcher-mechanism.md`](archived/review-2026-08-26-bootstrap-launcher-mechanism.md)

- [x] **F4 Job「预算」**：已处理（2026-08-27 Job 管理面设计拍板）——ideas/task.md 补一句话契约（域内成员资源总量上限记账，接入时点待需求出现），Job ABI 不含预算面。
- [ ] **F2 ready-marker 预留域路由**：预留语义目前硬连单一公平类的自由函数而非 `SchedClass` trait 契约；接 D64 时必须决定预留语义上收为调度类接口还是由域层提供统一通道。（触发：调度域 eligibility 接入、开放 D64 前）
