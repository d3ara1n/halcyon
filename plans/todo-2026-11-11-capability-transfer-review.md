# 统一 TRANSFER 交付的可选独立 Review

状态：**可选、未执行**。不占当前串行排期，不阻碍归档或后续 pm 设计；未执行不表示审查通过。

## 固定交付与范围

- 交付提交：`04816569304e275a3590e8d5c7837d2570be822f`（`refactor(capability): 统一转交权限与门控托管生命周期`）。
- 前置基线：`d0a6fd8046cd9080edcd3d3aefcdf67105108e90`；审阅该基线至交付提交的完整差异及实际责任链，不以登记提交或后续 HEAD 替换目标。
- 唯一实施记录：[统一转交与托管森林](archived/todo-2026-11-11-capability-transfer.md#实施交付与验证2026-11-11)。
- 本提交同时包含本会话确认的 [pm 有限范围设计](todo-2026-10-07-service-assembly.md#pm-范围澄清常规创建入口与元信息列表)，只审其目标边界与前置衔接，不将尚未实施的 pm 当作产品交付。

范围包括：

1. `os/capability` 的确定性 ETT/AVL 森林、固定批量回滚、unsafe 稳定地址/存储合同及借用槽安装核心。
2. kernel custody、Mailbox、Building/Running/冻结/退休与真实表安装：来源 pin、最后 authority 同步切边、业务锁→FOREST、锁外析构/退款、原能力和 rights 的失败恢复。
3. published_members、ProcessStart 初始承诺、运行期 ThreadStart/ThreadExit 仲裁及 reap/Departure 的资源职责；未发布线程不保活进程。
4. TRANSFER bit 4、旧 bit 8 拒绝、CyclicTransfer 编解码；formation、实际库/服务/驱动及正式组态的消费者同步。
5. 当前 impls、窄范围 ideas 同步与 pm 设计记录的一致性；不恢复已撤回的通知、退出结果服务、多方付款或强制 driver 迁移。

不扩展为全仓最终架构审查、一般对象图 GC、业务等待死锁检测或未来 pm/应用包实现。邻接问题只按实际责任链取证。

## 已有证据与限制

- kernel/实际消费者 check、clippy，capability 34 项 host 测试及其他受影响 host 路线通过；逐项命令、失败与最终日志由[产品验证记录](archived/todo-2026-11-11-capability-transfer.md#产品验证与日志)拥有。
- debug/virt 内核构建及 ELF 单帧审计通过；保留 LOAD RWX 链接警告。布局、源码工作界和机器帧证据不证明累计调用栈、真实运行时长或跨 hart 交错。
- 提交前整理通过变更链接/锚点、旧权利符号与 diff 检查；仅文档增量，未重复已通过的产品检查。
- 同组两角度结构检查及最终责任链复核已经执行，属于交付前检查，不是本登记的独立 Review。
- 未运行 QEMU；无真实请求方→pm→新进程两跳使用证据，pm 尚未实施。未 push。

## 接续

用户选择执行后，以新上下文按 [REVIEW](REVIEW.md) 只读审阅固定交付；形成报告后将 findings、修复与复核的唯一入口转到报告，本登记链接报告并归档。不为登记本身自动开启第二轮审查。未选择执行时，从[服务装配当前接力](todo-2026-10-07-service-assembly.md#当前接力)讨论 pm 的请求、返回权利、组态与真实消费者；驱动/用户服务创建归属未决，只阻塞依赖它的迁移。
