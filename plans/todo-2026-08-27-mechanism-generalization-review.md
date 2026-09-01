# 机制层泛化改造 Review 计划

> 【未来审查计划】对象是本轮三笔提交；Review 纪律见 [`REVIEW.md`](REVIEW.md)。发起背景与发现清单见 [`archived/review-2026-08-27-mechanism-generalization.md`](archived/review-2026-08-27-mechanism-generalization.md)。

## 提交对照

| 提交 | 内容 |
|---|---|
| `15c7811` | docs：review 入档、impls 失同步修复、KernelRequest 正名、README 归属纪律、COMPASS/carryover |
| `9c03251` | feat：Lock Ladder（sync 三件套 + 16 处 rank 标注）、TIMERS per-hart 化、MappingLease、phys_to_virt 栈区断言、role 叶子公理注释、drain_gate 显式释放 |
| `95deea6` | fix：release 构建 ladder 空模块补 mark_tp_ready 桩 |

## Review 轴（设计 + 代码）

### Lock Ladder

- **rank 表完整性**：`sync::ranks` 常量与 `impls/task.md` 契约表逐行一致；新增锁是否漏声明（编译期强制，但 `RawSpinlock` 直接使用者绕过 ladder——核实无裸用）。
- **bootstrap 帧切换时序**：`TP_READY` 在 `hart_formal_entry` 首行发布；secondary 被唤醒时 boot hart 已发布（bring_up_runtime 先发布全部 record 再 HSM start）。Release/Acquire 序是否必要（帧间无共享数据，序语义是否过度）。
- **ladder 断言的 panic 路径**：持锁态 panic → RawWriter 直写无堆无锁 → `hart::park()` 停驻；断言消息格式化不触发 HEAP 分配（fmt::Arguments 惰性）。
- **同秩链段 key 的语义边界**：HandleTable 嵌套 key = pid 的前提是「child 表必为新建进程」（pid 单调）。未来若出现老进程间的 GRANT 类嵌套，此断言会正确报警——确认该前提成文于 task.md。
- **debug/release 行为差**：release 下 ladder 为空桩；断言只在 debug 负载暴露。集成验证（virt/sifive_u）均以 MODE=debug 执行才算覆盖断言面。

### per-hart Timeout queue

- **稳定 token**：owner slot、arena slot 与 generation 是否闭合跨 hart cancel、到期弹出和槽位复用；错误 owner queue 必须结构性拒绝。
- **完成即注销**：对象命中、Abandoned、Timeout 与注册发布竞态是否都使 queue live entry 消散，不滞留 WaitContext。
- **终局解耦**：`is_quiescent` 已由显式系统复位删除；确认 TimerQueue 只承担有限等待的唤醒所有权，不再参与任何整机生命周期谓词。
- **不迁移前提**：注册 owner 是发起 hart，不随 Waiting 线程迁移；显式迁移接入时重审。

### MappingLease

- **Weak upgrade 失败路径**：owner 消散（地址空间已亡）时 release 无操作——核实该路径下 external_mappings 已随空间销毁，无泄漏。
- **锁序**：release 内 `owner.space.lock()` 发生在 connection 锁外、无表锁上下文；drain 的 close 回调链中调用点核实。
- **失败回滚守恒**：create/attach 在 map_external 之后的失败路径，映射随 Endpoint Drop 自动解除——压力验证线（carryover 既有条目）覆盖此面。

### 公理层（已吸收）

- 已由 [`todo-2026-09-midterm-design-review.md`](todo-2026-09-midterm-design-review.md) 的「公理层一致性」轴吸收，真值点转移；本计划不再重复审查。

### 文档自洽（已吸收）

- 已由 [`todo-2026-09-midterm-design-review.md`](todo-2026-09-midterm-design-review.md) 的「文档纪律与自洽」轴吸收，真值点转移；本计划不再重复审查。
