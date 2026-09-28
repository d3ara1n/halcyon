# 对象级最后 capability 引用机制

状态：**已登记，未开始**。只读盘点前不定实施范围。

## 目标

把“某对象最后一个存活 capability 引用消散”做成 capability/对象层的通用机制，和各类型的收尾语义分开：

- **机制（通用）**：按对象统计存活 capability 引用，包括任意 HandleTable 表项、运输中尚未安装的引用、已提交未交付的 Grant。计数只在引用铸造与消散时变化，全部消散路径（Close、运输丢弃或退回、HandleTable Drain）汇入唯一 release 入口。计数与内核内部 `Arc` 寿命无关，不用 `strong_count` 推断。
- **语义（按类型）**：计数归零时调用该类型的“最后引用消散”钩子，由类型决定动作，例如 Created 线程 abandon 并走统一离场入口；其它类型可能是通知对端或无动作。

对象只是内核抽象概念，不要求统一形状；机制只要求对象可以被 capability 指向。

## 来源与前置

- 起点是 [生命周期收敛](archived/todo-2026-09-28-lifecycle-convergence.md) 中 ThreadControl 的对象级引用计数：只有最后 capability 消散且线程仍是 Created 才 abandon，WAIT-only 引用照常计数。
- 前置：生命周期收敛切面提交。本记录以其提交为基线，不在该切面内扩大范围。

## 第一步：只读盘点

1. 列出内核中现有的“最后引用”类语义，例如发送端全部关闭、最后 Control 关闭、Builder 放弃、Endpoint 对端关闭，以及它们各自的计数或判定方式。
2. 判断是否存在重复真值：两处及以上各自手写计数，或借用 `Arc::strong_count` 与 capability 存活混用。
3. 核对各消散路径（Close、Transit 失败/退回、Drain、Grant 回滚）是否都能落到同一 release 入口，以及锁序约束。
4. 外部参照（如 Zircon handle count / `on_zero_handles`）按官方资料核实后才写入 ideas。

## 决策门

- 存在多处重复计数：立案统一机制，所有消费者一次迁移，删除逐类型计数。
- 只有 ThreadControl 一处：机制仍放在 capability/对象层，记录接入契约，下一个消费者出现时直接复用，不为单一消费者另建类型私有计数。

结论、施工范围、删除清单和验证门在盘点后回填本记录，并在实施前取得用户确认。
