# Runtime 清理与 Job 有界分页固定提交 Review

> 【未来审查计划】固定对象 `5de2780fe4802f2bb31ddeeafccf87737ba80cc4`（`refactor(runtime): 复用预付请求并有界分页收束 Job`），父提交 `d2d83def973461c992fcb41c62916fc4dc9de437`。提交后登记，供未来独立只读复核；本批实现、完整验收与集中复核已完成，不阻塞后续任务。

## 范围与复核重点

1. libsrv：存活任务槽直接持 T；Add/Arm 共用 Register/SourceOps；Requests 与固定输入额度在 Runtime 创建时预付并复用。Gate 未结算时不得覆盖请求，失败/Complete 仍保留 owner、输入、期限及最终退款。
2. libprocess：每 frame 一份 ABI 页，成员与 children 按阶段复用；当前页完成后才继续枚举。严格验证回执与零进展，子 frame 栈位及页在派生 authority 前预付。跨页、嵌套、条目消失、枚举/关闭失败及 replenish 不回退已提交进度或丢失控制能力。
3. srv_pm 与验收：测试邮箱保持 Active 到 stop，实际登记与注销回执、Runtime 排空/关闭和账户归零共同构成必检锚点。服务进程都是验收夹具，不以生产服务架构要求评判固定装配和测试阶段关系。
4. 文档：COMPASS 明确核心及框架较成熟后逐步替换正式服务；本批没有扩展 RPC/Outbox、服务架构或内核 ABI。

## 已有证据

- 59 项相关 host：libsrv 26、libprocess 8 单测及 8 集成、librunnel 17；包含拒绝新分配时正式 Gate 仍返还 owner，以及栈/子页分别 OOM、两类多页和后页错误、零进展续作、嵌套关闭失败。
- just check、七面 clippy、core 与完整 acceptance 通过：`artifacts/acceptance-cleanup-paging-20260915-131912.log` exit 0，包含 stress 16/16、release、sifive_u、nofd 和 panic/alloc/fatal 启动失败三线。
- `.sources.json` 的 7 个改动源码哈希与该提交一致；完成时无 QEMU/GDB 残留。日志是本机 artifacts，不随 clone 交付。
- 本批两份集中独立复核均无 finding；设计审视、用户收窄边界与完整过程见 [原固定提交审视记录](todo-2026-09-15-runtime-admission-review.md) 的已授权清理批次。

## 完成门

未来 reviewer 使用新上下文只读核对该固定差异，finding 必须有位置、可达顺序、影响与契约证据。无 finding 后归档；有 finding 时在唯一报告中承接修复和复核，不重开已关闭问题或为测试服务另建正式框架。
