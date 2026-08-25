# todo：FAL/RPC 集成批次 review

状态：**待 review**。对象是 FAL/RPC 集成两个提交：`54d3e02`（设计入档 +
WaitMany deadline + librpc/libfal 线协议首批）与 `bf32c1c`（libfs 走路引擎 +
memfs/provider + fs 真实化 + 旧 ABI 清除）。本文件为 reviewer 提供入口；
review 完成后结论归档 `review-`，本计划归档，持久问题进 KNOWN_ISSUES。

## 动机

本批为快速纵向贯通（真路径验收线优先），自查与 host 测试覆盖了协议编解码
与走路核心，但未经过独立 review——按仓库惯例（IPC 对象层先例
`review-2026-08-ipc-object.md`），大体量落地需统一审查后才算收口。已知实现
中有一批「能跑但值得质疑」的形状，集中在下文「已知疑点」。

## Review 面（按层）

### 内核：WaitMany deadline（`os/kernel/src/syscall.rs`、`task/wait.rs`）

- syscall 面第四参数 x13（相对毫秒，0=无限）与 `WAIT_DEADLINE_INFINITE` 语义；
- `deliver_wait_result` 公共写回：Deadline 交付 `reason=Deadline`、
  `item_index=u32::MAX`、无观察项消费；仅 Cancelled 占位是否仍准确；
- rinlib `wait_many` 新签名与全部调用点（init/pm/librunnel）改动的机械正确性。

### librpc（`user/frameworks/librpc/`）

- RpcPrefix 编解码：16B 布局、保留区零校验、txid 单调非零；
- Caller：单 outstanding 约束、ReplyPort 超时废弃重建路径、迟到回复隔离
  （owner 关闭后 send-once 投递失败）、应答 txid/flags/kind 三重验证；
- send-once 权利集 `WRITE|TRANSFER`（extract_moves 的 TRANSFER 内核前提）。

### libfal（`user/frameworks/libfal/`）

- 线协议常量冻结面（PROTOCOL_ID、13 kind、12 Status、路径契约）与
  ideas/fal.md 的一致性；
- memfs walk 核心语义：中间分量 X 检查、终段策略、链接只返边界、
  generation cursor 失效路径；
- provider 分发：各 kind 应答形状（状态字 + 变体/负载）一致性；
  解码违约 → Internal 应答保持协议闭环。

### libfs（`user/frameworks/libfs/`）

- 前缀表段边界匹配、重复挂载/卸载；
- resolve：帧栈 base 分区、consumed/remaining 校验（consume_prefix）、
  `..` 跨帧与根钳制、绝对 target 重启、相对 target 替换、
  40/4096/65536/256 限额、resolve_parent 终段规则。

### fs 服务与尸体清除（`user/systems/fs/`、`shared/`、`rinlib`）

- 泵架构：单 outstanding 下 reply 邮箱「永不触满」假设；
- WalkTransport 映射（Lookup 三值 → 客户端视图）字段保真；
- 旧 ABI 删除无残留引用（shared/fal.rs、path.rs、rinlib::fs、0x70-0x79）。

## 已知疑点（实现者自列，review 重点）

1. **寻址前奏偏移算术三处重复**：`op.rs`（`8 + 2 + rel.len()`）、`io.rs` 与
   provider 的 `reader_after_address`（`10 + rel.len()`）同一布局三种写法，
   无共享常量——layout 漂移风险。
2. **fs 栈缓冲 512B vs PAYLOAD_MAX 4096**：泵与请求构造用 `[0u8; 512]`/
   `[0u8; 544]`，协议常量是 4096；当前 body 远小于此，但无编译期关联，
   跨进程大属性/大枚举页会静默 IllegalArgument。
3. **librpc Caller 等待项对 service handle 要求 WAIT 权**：跨进程首次使用时
   若 grant 未含 WAIT，`wait_many` 即 RightsDenied——权利需求未在 API 层表达。
4. **memfs 属性按原始字节存储**：类型校验（编码侧 DecodedValue 与声明类型
   一致）只在客户端，提供者不校验——与「提供者把输入当不可信」的契约张力。
5. **provider Lookup 应答 = 状态 + 变体 + body，其余 kind = 状态 + body**：
   expect_ok 只跳状态字，变体语义分散在调用侧，无统一应答形状。
6. **fs 泵 500ms deadline 归并为 Internal**：诊断可区分性差（正式超时语义
   应留待 Deadline 状态或协议错误）。
7. **Enumerate 预算边界**：首项超预算仍返回（`!entries.is_empty()` 守卫），
   max_bytes=0 时每页恰一项——终止性成立但语义未文档化。

## 已知事故（实现期在真路径暴露并修复，review 验证修法）

- send-once 派生缺 TRANSFER → extract_moves RightsDenied；
- FalHeader::decode 对「header+body」整段解码越界（finish 拒绝尾差）；
- WalkTransport Link 映射丢 consumed 前缀 → symlink 展开后路径错位。

## Review 方法

- host 测试全量（`cargo test -p libfal -p libfs -p librpc -p librunnel
  --target aarch64-apple-darwin`）与 `just virt` 作为回归基线；
- 对照 notes/ideas/{fal,rpc}.md 契约逐条核对编码与语义；
- 疑点 1-7 逐条给出裁决（接受为已知简化 / 本批修 / 立后续计划）。

## 完成条件

review 结论入档 `plans/reviews/` 或 `review-` 前缀文件；疑点清单全部裁决；
需修项转新 todo 或随批修复；本计划归档，COMPASS 同步。
