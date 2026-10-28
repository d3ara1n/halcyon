# 静态常驻服务基座代码交付的独立 Review

状态：**可选登记，未执行，不占当前串行排期。** 实施与结构清理已经交付；登记不表示审查通过，也不阻碍后续按条件接入新能力。

## 固定对象

- 分支：`refactor/system-redesign`。
- 基线：`caa1f5ef63bf7fe852c71e5c3d310b31fdfab034`。
- 目标：`bfb4fddf2e68ee4706bcd0f2f98cab63694ce6e6`（`feat(init): 实现静态常驻服务基座与统一生命周期监督`）。
- 范围：基线到目标的40个路径，包含静态部署、长期监督、文件宿主启动与授权、正常停止/故障终局、验收工具及文档，以及同轮结构清理。

[服务装配计划](todo-2026-10-07-service-assembly.md#静态常驻服务基座设计2026-10-28)和[关机计划](todo-2026-09-system-shutdown-orchestration.md#静态常驻基座停止2026-10-28)拥有目标合同；`caa1f5e` 已确认 init 根必需服务与未来普通服务管理的故障边界。原[设计 Review 登记](todo-2026-10-28-service-foundation-design-review.md)仅固定 `99f5e9b`，不替代本次代码审查。后续登记与提交状态更新不扩大本次目标。

## 审查范围

1. System 的硬编码部署、映像校验、真实就绪与空闲停驻。init 持续监督必需宿主；未提交停止的正常退出与异常退出均进入系统故障终局，不把任意后代都列为必需服务。
2. `srv_fs::startup/deployment` 的三种 profile、实际 grant 角色与权限、容量来源、必要来源安装后才交根/Register/Ready；验收和 System 共用业务与退休实现。
3. `supervision` 的唯一 PreparedProcess/Running Control、Start 前 DONE/报告来源、身份与真实 Running 交付门。`start_instance` 仅返回 PID；Bootstrap 持生命周期关联，Host 持业务根；准备/发布失败和 Stop-before-Start 显式取消交原 Collector，不落回同步析构等待。
4. Stopped 与真实 Completed 的独立事实、到期复查、迟到能力释放、来源实退后 Control 转交。正式 Collector 与验收 Batch 接点明确，批次接管容量在 detach 前预留；Pool 零账仅作为无外域 owner 的局部验收条件。
5. root 的停止意图、冻结总期限、消费者到供应者的收束、services Job 收集及 Completed；系统故障终局独立于观察运行体退休，设备 Closed/Binding/Provision 安全证明不由进程 DONE 替代。
6. 原正式消费者、验收工具判据及 impls 是否与实现相符；DMA 自检只调整完成观察时点，reset 负测与真实最终动作分离，旧路径和重复状态是否清除。

审查按 [REVIEW](REVIEW.md)执行。服务管理器、initfs 描述脚本、应用工作流、持久化和自动恢复不在本轮能力承诺中；不以这些尚未接入为由追加框架或撤销当前有限保证。

## 已有证据与限制

完整证据由[基座实施记录](todo-2026-10-07-service-assembly.md#最终验证与覆盖边界)及[交付后结构清理](todo-2026-10-07-service-assembly.md#交付后结构清理)拥有：

- `just check`、十一分面 Clippy、srv_fs 两项 host 用例；release core、debug stress、sifive_u/nofd、设备正常停止及 Driver 装载期和 Filesystem start/wait/stopping 故障；reset 拒绝后真实 DMA 保留、init DONE 和该路线四 hart 停驻。
- System 有限来源观察；secondary 正常 Stopped 先与 primary 原到期复查中 DONE 先，核真实 tick、冻结总期限和随后 Requested reset。不得外推为无限活性、System 全 hart idle 或两种普通通知交付顺序均受控。
- 清理后重新通过 check/Clippy、严格 release core 与 System 有限观察；未改判据的专项沿用此前证据，不声明全部组合在清理后重跑。
- 错误身份、配额和 Stop-before-Ready 交错未穷举动态注入，部分边界仅经源码路径核验。真实硬件未覆盖；reset 拒绝的 DMA 保留不是硬件静止证明。

进行中的主代理/peer 协作检查及辅助只读检查均属于交付前结构检查。本登记尚未执行独立 Review，也不另造验证结果。

## 执行与接续

选择执行时，使用新上下文审查固定提交及相关完整责任链，先核源码再判断记录。发现须给出具体路径、可达场景及违反的契约；形成报告后由同一报告承载修复和复核，本登记链接该报告并归档。没有选择执行时保留可选入口，不恢复已结束的施工任务。
