# IF2 静态资源与 OAL 组态装配独立 Review

状态：**可选登记，未执行，不占当前串行排期。** 本次代码/设计交付及提交前简化已完成；登记不表示独立审查通过，不阻碍后续工作。正式运行证据仍按原计划边界接续。

## 固定对象

- 分支：`master`。
- 基线：`68843cea0e39b1e996df5a6aee855469b430e1da`。
- 目标：`c8c54da39cce64f2c32c4753baea5d785ffaf320`（`refactor(init): 实现静态资源与 OAL 组态装配`）。
- 范围：基线到目标的 82 个路径，含本轮体系关系/组态设计、Mailbox 付款 ABI、纯 OAL 描述、formation/birth v2、init 与三个正式宿主迁移、impls/接力及同组简化。
- 本登记之后的提交状态/导航回填是文档操作，不改变此固定代码范围。

## 依据与审查重点

目标合同由[组态 IF2.1](todo-2026-10-28-init-formation.md#if21-接手审计与具体设计2026-11-10)拥有，用户态职责取[服务装配 U2](todo-2026-10-07-service-assembly.md#启动依赖澄清-u2讨论中)的最终用户决定，不从前一轮 Ready/finish 历史恢复旧目标。

1. 显式 Pool 的授权与收费覆盖 MailboxCore/Receiver、Sender/Lifetime 及持久维护；当前 Pool 便利入口与显式选择进入同一实现，转授不迁费。
2. formation v2 的资源/付款/权限引用、唯一 receiver/Lifetime move、实例循环接线、旧字段拒绝；终局通用撤销仅由显式 terminal_revoke 声明，不从名字推断。
3. shared/oal、birth named sections、运行期 Namespace 的知识归属及一次采用；本地前缀路由不冒充根目录聚合枚举。
4. init preflight、出生块实际几何、准备/Grant 消费与发布/DONE 监督；失败原 owner 不丢失，不重复关闭已移交能力，不等待业务 Ready。
5. fs/device/driver 的真实配对身份、领域权限/Funding/Lifetime 安装后才 Receive；提前排队消息不先收后误拒绝，静态母本不意外保活客户端授权。
6. 旧 Export/Ready/Stopped、finish/task 和 init 正常停机链删除；运行期注册/撤出、设备 Binding/硬件静止及 worker BOOT_READY 不误删。
7. 本次 S1–S3 简化的责任保持与长期文档一致性；候选和延期能力不被误当已交付。

## 证据与未覆盖范围

[IF2 验证/交付](todo-2026-10-28-init-formation.md#if2-交付与接力)拥有完整日志与边界；[提交前简化报告](archived/review-2026-11-10-if2-simplification.md)拥有 S1–S3 原发现、修复及主代理复核，均已关闭。

- kernel/user target 检查、clippy、239 项 host 测试（含 2 doctest），内核构建及 ELF 审计、System v2 与七个用户 ELF 审计、BootPackage 打包通过。
- 简化后仅受影响的 srv_init/srv_driver 再做 check/clippy，并重新完成正式用户构建与打包；未重复无关内核/host 检查。
- **未运行 QEMU。** 未证明该版本真实显式付款/退款、跨进程早到消息交错及设备/DMA运行；历史 Ready/Stopped 路线不能替代本版运行证据。
- 不提供正式 pm、应用启动、包格式、正常电源操作、完整故障救援或聚合根枚举；正常关机当前未接通，不由 init 退出代替。具体后续、工作集旧债务及取证触发条件见 IF2 当前承接表。

前三组代码交叉核查、主代理交付核对及 S1–S3 审阅属于交付前协作检查，本登记尚未执行独立 Review。

## 执行与接续

用户选择执行时，用新上下文审查上述固定提交及相关责任链。实际 findings 需给出路径、可达场景和违反的契约；形成 review 报告后由该报告统一拥有修复和复核，本登记链接报告并归档。同组修复不自动开启下一轮全量 Review。未选择执行时维持可选入口，不自动运行 QEMU 或扩大任务。
