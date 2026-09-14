# FAL 集成开发基线未来代码复核

> 状态：待执行的提交后代码 Review；完成后归档，结论归所属修复专题或实现记录。本文件只安排固定提交的复核，不重复安排公共时间、执行基座或 FAL 业务施工。

## 固定对象

- 提交：`d22b9d71ef810145bf4d5bfb3673ffec8640f361`，`chore(fal): 保存公共对象收口后的集成开发基线`。
- 父提交：`5d406a481e7c4b7c414a742bfa8b155bc4a98f73`；开发分支 `task/fal-service-capabilities`。
- 范围：131 个文件，15858 行新增、1702 行删除。公共对象前置已交付；公共时间前置、运输/执行前置与 FAL 后端/协议仍是草稿，业务暂停。
- 入口：`git show --stat d22b9d7`，`git diff 5d406a4 d22b9d7 -- shared os user notes plans`。复核固定代码快照，不能用后续 HEAD 的修复冒充该提交原有行为；修复关闭须另列实际提交证据。

## 复核闭包

1. 公共 ABI 与消费端：MailboxSender 独立身份/badge、Lifetime 观察不保活、affine Delivery/ReceiveResult、HandleQuery 不授新权；shared/kernel/rinlib 与真实调用者同步，旧 WaitSet Seal/Drain 无兼容残留。
2. 消息事务：full/到期/权限失败保 moves/once；receiving 占位和精确电平；partial header 后复制失败完整回滚，旧编号跨槽复用仍 stale；closed-owner 失败拒收并锁外退款，不能以末尾清理掩盖中途损失。
3. 来源和持久轮次：最小相关历史 serial、Seen/CLOSED fallback、Complete/Deferred/Lost 终态摘完整订阅；操作/finish 的 epoch 快照不跨重置，所有跨对象工作及最后引用锁外。
4. 退休与拥有根：Create/Register 准入预付，operations/Done/source_id 联合门；普通 Close 原子摘 owner/mandatory、actor 独占推进、回复取消不撤退休；私有 progress/completion 分离，不等待用户观察者。
5. Native ProcessDrain：固定目标/输出/预算、跨停驻累计，More 正工作；captured epoch 取消当前来源登记、旧取消不影响新 parked 依赖；槽/Pending/Done/交付顺序、Fault/StoreAccess 与普通观察错误边界；Finalization 在 Job 摘除前接独立根。
6. 资源与短路径：真实 finish 分区、slot/token/Pending 同锁、早到 wake/跨 owner 重排；控制面 16 与 deferred 面独立 16；来源、队列和 Actor 锁阶，无栈轮询/全表析构/不可保证的清理分配。
7. 用户拥有者：Capability 的非映射构造/关闭契约，WaitSet 显式 GRANT/into_capability/非空 Drop；affine role 不因包装可复制或 TRANSIT，错误责任不被 silent Drop 丢弃。
8. 测试可证性：旧 continuation 三项、Mailbox 三项、最终六项 P2 的关闭断言仍成立；区分启动顺序夹具与真实用户并发/GDB 单次窗口，确定性 FIFO/Full 与自由竞速分开，退款不代替成功交付。
9. 混合集成边界：审计时间/执行/FAL 草稿是否破坏公共对象前置或当前正式服务路径，识别未连接重复类型/owner/adapter。尚未承诺的能力缺失归既有专题，不据此要求在本 Review 内完成 FAL 或恢复旧兼容机制。

## 验证与收束

当前本机基线：七面 clippy、140+23 host、virt core/release、128MiB sifive_u、virt-nofd、三类 boot-failure 通过。定位见 `notes/impls/ipc.md` 与公共前置档案；artifacts 日志和 GDB ELF/SHA256 被 Git 忽略，异机重跑才能形成新的运行证据，不能声称 clone 自带验证产物。

完整 stress 的旧静默截断与概率覆盖误失败未宣称通过或修复，唯一后续安排仍在 [验收可靠性计划](todo-2026-09-13-acceptance-reliability.md)。本 Review 不做重跑直到绿，不用延期豁免新 correctness finding。

报告按严重度给出固定代码引用、真实机制与可证性边界。公共对象前置的新正确性问题须立独立修复闭包并同步实现记录/导航；属于公共时间前置、运输/RPC/服务执行前置或 FAL 的剩余责任回写其既有唯一计划。所有 findings 有固定提交的关闭证据后归档本文件；本 Review 不替代最终装配/旧路径删除/整体交付门。
