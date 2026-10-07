# 对象链修复与机制收敛：提交后独立 Review

状态：**已登记，待执行**。固定目标为 `ac9d7d9364770a69c8135e918d512069980c06bb`，父提交为 `2aee0286dae3c3a8118f8d24f96e79a35f337c92`。本次是修复与收敛的代码交付审查；原批次的五切面首审及修复记录已由[审查修复档案](archived/review-2026-10-07-object-chain.md)闭合，不重复原始首审，不将档案中的“未提交”历史描述当作当前状态。

## 目标与边界

用新上下文核实最终代码责任链、简化收益及证明前提，发现新的反例或未被原修复切断的路径。阅读固定目标完整相关代码，以父提交差异定位变动；不能只看diff而忽略真实消费者或最后释放。ideas没有在本轮审定，不作为新实现必须遵循的依据。

本次重点包括：

1. SUM的translate→SFENCE→直访；用户fault目标权限、Committed窗口、现有Completion等待、每组事实每hart修复界、Frozen detach及S态fault分流。
2. Job Remintable、父MANAGE重新派生、Seal有界传播、Reserved跨游标出生封口、父表摘除/child CLOSED/parent完成的顺序。
3. Work的Arc接收者、执行器原根到finish后的寿命、QUEUE内真实非last、SignalSource内部完成的来源→QUEUE短边、所有潜在最后根的锁外归还与IPI失败runtime_stop终态。
4. OrdMap单arena、唯一运行期PublishCursor、切分拒绝保owner、LockGuard类型约束，以及被替换旧路径是否真正删除。
5. SourceRegistration在libprocess/librpc/FAL/fs/pm真实任务中的安装/停止/注销/回执、期限缓存与两timer共用Pin宿主；正常、Busy、超时、迟到、取消和退出的owner不丢失。
6. 最终M累计栈的真实CFI、动态目标、局部跳表、末端状态剪边、递归深度与入口前缀，源码证书是否确实绑定全部语义前提；不能把模型输出或已有QEMU通过当成证明本身。

范围包括上述涉及的`os/kernel`、`os/{capability,funded_frame,memory_space,page_table}`、共享契约实际依赖、`user/libraries/{libexecution,libprocess,librpc,libfal}`及fs/pm/init/hammer消费者、诊断/栈工具和配套文档。资金/来源/回收路径须延伸到真实最终退款；更广FAL重设计、不可达capability环收集、可靠物理CPU计量、设备/DMA静止、terminal诊断栈不因此扩张为本轮承诺。

## 已有事实与证据

六项原finding、C1/C2、八项已采纳收敛、两项未采纳候选及最终结构检查，由[最终复核与交付](archived/review-2026-10-07-object-chain.md#最终复核与交付)唯一承载。当前impls描述固定目标实现，关联设计计划已纠正旧接力和被替代合同。

已有host契约、`just check`、`just clippy`、完整`just acceptance`、异构域及补充VM/SUM专项通过；命令、日志和覆盖限制均在同一档案。栈证据由[源码证书](../tools/maintenance-stack-certificate.json)、四份maintenance-stack工具及档案累计栈章节共同提供。真实VM发布窗口已观察，修复入口的部分cause/stval由GDB注入；未动态复现真实硬件旧TLB缓存，不提供WCET或全故障点穷举保证。

审查先读[REVIEW](REVIEW.md)、[KNOWN_ISSUES](../KNOWN_ISSUES.md)、档案最终合同和相应impls，再亲核关键代码。固定源码未变且无具体风险时不重复已过验收；需要运行验证时按[构建手册](BUILD-AND-TEST.md)选最小足够边界并隔离进程/产物。

## 执行与完成门

本计划只登记，尚未启动reviewer或新测试。执行者只读报告，不修改生产代码；每条finding应有固定位置、可达路径、违反合同及影响，并区分实际反例与证据缺口。重复旧finding须说明最终修复为何仍未切断路径；结构候选须给出独立收益及完整责任链替代。

审查报告形成后，修复与复核的唯一入口转至对应`review-*.md`，本计划登记入口并归档；不复制第二套findings。没有finding也须说明覆盖范围及限制。后续修复或新能力按实际交付闭包组织，不以逐条finding机械启动新一轮首审。
