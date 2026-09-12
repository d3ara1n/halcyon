# 库存来源与启动自检未来代码 Review

> 【未来审查计划】固定对象为 `606b59db22f071943a4bfc2c174633454e449401`（`refactor(mm): 收口库存来源与正式启动自检`），父提交 `e973763`。提交后生成，只安排已完成代码的独立只读复核，不审查方案，不阻塞 FAL 后续设计或实现。

## 提交范围

- `os/kernel/src/frame.rs` 删除 `alloc_user_order`、`publish_claimed` 与通用 `FrameTracker`；`BootHeldExtent` 直接持 affine 几何，保留唯一 unsafe adopt、消费式切分和最终回投。
- `os/kernel/src/frame/selftest.rs` 与 main/boot 接线统一正式自检：root 建立后、Ready 发布前执行，普通页与表页入口、同 claim 全范围写脏再真实清零、一 extent 三页失败退款、四页单 extent 切成 1+3 页的两种释放顺序。
- MemoryPool 自检验证 child 支付真实表页，funded owner 独自保活来源 Pool，最后 owner 释放后才退款父级。
- 库存 host 测试验证部分重叠归还修改前拒绝；broker host 测试补逐 owner 释放后的中间账本；正常 QEMU 路线要求统一成功锚点。
- 同步实现文档、交付导航与观察登记；数据面计划归档，无 shared ABI 或用户接口变动。

## 复核清单

1. BootHeldExtent adopt 仅接管已验证、尚未发布为空闲的 boot-held 范围；split 不重复或遗漏物理 owner，失败和析构路径仍保持来源语义及启动内容。
2. BootFundedExtent 的 charge 与物理几何同步切割，字段析构与锁外归还继续先物理、后额度；退役与 BootPackage prefix/payload 生命周期无回归。
3. 普通生产 funding 路径没有 raw adapter/tracker 旁路；纯库存原语只由合法 adapter 与 host 测试使用，启动来源不能伪造普通 funding owner。
4. DirtyInventory 仅作为私有测试来源委托真实 claim，不自行清零、不伪造 cleared 状态、不引入重复 raw claim/return；写脏、读回与正式 clear 的同 owner 全范围证据成立，访问不越界且不持库存锁。
5. 快照比较位于真实启动静止点；child-funded owner 是外部 child 强引用消散后的唯一来源保活者，Weak upgrade 不改变最后引用退款结果。
6. 1+3 页切分两种释放顺序的中间双账本与存活侧访问正确；读写成功不冒充未回投证明，独占不变量由 affine 几何与库存模型共同支持。连续四页仅是已验证的启动 fixture 前置。
7. 部分重叠归还拒绝发生在任何库存修改前；host 覆盖前半/后半两种布局。broker 的分解、部分释放、全量释放账本与析构事件次序一致。
8. 成功锚点必须由全部 frame 自检通过后输出，正常 debug/release 与 sifive_u/nofd 路线不跳过；Boot failure 仍按独立预期失败语义判定。
9. 不存在旧 raw API、通用 tracker、早期自检、过时现状文档或未登记迁移 adapter；真实代码与报告证据边界一致。

## 已有验证证据

- `just check`；`frame_pool` 18 项、`funded_frame` 12 项、`memory_pool` 14 项、`memory_supply` 7 项 host debug/release 均通过。
- 默认 50% 节流 `just acceptance` 通过：七面 clippy、debug stress 16/16、release core、sifive_u core、nofd、panic/alloc/fatal 三类启动 Failed 广播。
- debug/release 最大栈帧 `0x26f0` / `0x1250`，均低于布局派生的 12KiB 限额；每 hart 256KiB 栈不变，结束后无 QEMU/GDB 残留。
- 完整日志 `artifacts/frame-source/`；交付记录见 [`数据面档案`](archived/todo-2026-09-memory-object-data-plane.md)。提交前默认 reviewer 对完整 diff 的只读复核无开放 finding，报告见 [`代码复核档案`](archived/review-2026-09-frame-source-selftest.md)；该记录不替代本次固定提交 Review。

## 边界与完成门

本计划仅拥有切片 10 的固定提交复核；`d00604a` 的多页 Tunnel/RNL2 仍由统一架构 Review 拥有，`4b27ce6`/`8aa7bc2` 由既有后续内存 Review 拥有，不重新安排旧 program。

未来 reviewer 使用新上下文对目标提交只读审查代码，核对验证与真实调用/失败/退役路径，记录有证据的 finding 或确认无 finding。无 finding 后本计划归档；如有 finding，由一份 review 报告承载修复与复核，本计划转为该报告的导航而不重复安排同一问题。不得把未完成的正式 FAL Open、RPC deadline 或全局架构收口视为本提交已完成。
