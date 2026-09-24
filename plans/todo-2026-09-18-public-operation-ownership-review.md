# 公共操作所有权固定提交 Review

> 【未来审查计划】固定审查提交 `8e0467a8231e61ad6646d0d095bdac0493c1bfb6`（`refactor(kernel): 闭合公共操作所有权与公平推进`），父提交为 `59a6d8e`。本计划在实现提交之后登记，不阻塞 FAL F0 主线；审查对象是该固定提交的完整 diff，不以当前分支后续代码替代。

## 改动概要

该提交完成 [`公共操作所有权专题`](archived/todo-2026-09-14-public-operation-ownership.md) P0–P6：

- 以 `DebtLedger` 统一各领域工作债务的票据、登记、停驻、唤醒、完成和退款机械协议；领域 payload、容量、取消边界与执行器保持独立。
- 以捕获的 `WaitKey` 收束 Waiting 发布、请求 start/cancel 和 activation 复用，允许取消先赢、迟到 start 空操作，并隔离旧 epoch 回调。
- 以 `RetirementTicket`、类型化 Process driver 接缝和 `drain_gate` 内部仲裁收束对象退休、MemoryChange、Unpublished、Termination、Finalization 与 ProcessDrain 的 owner 交棒。
- 新增 `FairBudget<4>`，分别驱动 deferred 四类和 control 四类债务；删除动态 dependency、重复 waiter 根、空完成阶段、重复预算真值和 WaitSet 自检旁路。
- 增加确定性内核夹具、host 公平预算测试和用户态在途 Drain/调用者退出/管理者接管组合验证；保持公开 ProcessDrain、REAPABLE/CLOSED 和管理者 capability 语义不变。
- 同步 ideas/impls、归档专题计划，并将主线切换为 FAL F0 重新基线审计；FAL 实现不属于本提交的机制交付。

## Review 范围

1. `os/work_debt` 与 `os/kernel/src/work_ledger.rs` 的 Reservation 生命周期、容量计数、Pending 电平、门铃失败、park/wake 和完成退款是否各有唯一线性化点。
2. `task/{wait,request}.rs` 的 Waiting 发布、start/cancel 顺序、旧 epoch 回调、activation 静止与复用是否可能重复启动、误取消新轮或遗漏回复责任。
3. `task/{retirement,object,notify_work}.rs` 的来源锁内单步推进、锁外交付、ticket 强根和 mandatory 生命周期是否保持锁序且不提前释放 owner。
4. `task/{proc,process,lifecycle,handle}.rs` 的 ProcessDrain、Unpublished rollback、Termination 和 Finalization 是否串行共享批次许可；PublishDead 前后强根交棒、管理者接管、失败与退出路径是否完整。
5. 两组 `FairBudget<4>` 在预算 1、持续 backlog、类别缺席和新类别后进入时是否保证有界公平；`work_done` 是否只报告真实业务进度而不把阻塞登记伪装成公开进度。
6. 所有真实消费者是否已迁移，旧 dependency/预算/driver 旁路是否确实删除；测试是否调用正式账本和 payload，而非复制状态机或以测试专用运行体代替生产路径。
7. shared/rinlib ABI、ProcessDrain 公开结果、REAPABLE/CLOSED、调用者取消与管理者 capability 是否保持原契约；文档是否准确区分最终结构、实现现状和后续 FAL 工作。

## 已有验证证据

- `cd os && cargo test --workspace --exclude erhino_kernel --target aarch64-apple-darwin`：通过，日志 `artifacts/check/public-operation-p6-os-host.log`。
- `cd shared && cargo test --workspace --target aarch64-apple-darwin`：通过，日志 `artifacts/check/public-operation-p6-shared-host.log`。
- `just check` 与七面 `just clippy` 通过；lint 日志位于 `artifacts/lint/`。
- `just acceptance` 通过：debug stress 16/16、release core、`sifive_u` core、`virt-nofd` core，以及 panic/alloc/fatal 三类 Ready 前 boot-failure 全部达到预期终态。
- P6 施工期三个失败日志只用于记录已修复的夹具/观测问题，不是最终验收证据；具体归因见专题归档的 P6 收口记录。

## 完成门

未来 reviewer 在新上下文中只读审查固定提交 `8e0467a8231e61ad6646d0d095bdac0493c1bfb6`，并复核上述责任链、公开契约及已有证据。finding 必须给出代码位置、可达路径、影响和违反的契约；不得因后续 FAL 改动扩大本次范围，也不得以测试通过替代 owner、锁序和退款证明。

无 finding 时记录复核结论并归档本计划；存在实际 finding 时在本文件登记唯一修复入口、责任边界和复核证据，不另建平行 Review。任何修复仍须按项目授权边界另行实施和提交。
