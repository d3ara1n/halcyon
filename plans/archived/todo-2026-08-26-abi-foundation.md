# ABI 基座重构计划

## 目标

为后续 DirectoryGrant、用户态 ProcessStart、Job/设备授权建立统一 ABI 基座。本阶段只处理：

- 通用 StartupBlock 外层；
- Handle `TRANSIT` / `GRANT` 运输权利；
- affine owner 的直接启动 grant；
- badged mailbox sender；
- send-once target/transit alias；
- 内核、shared、rinlib 与集成负载的同步验证。

本阶段不实现公开 ProcessCreate/ProcessStart、Job、DirectoryGrant、设备对象或 FAL watch；只把它们依赖的机制定形。

## 已确认契约

### StartupBlock v2

```text
[StartupBlockHeader (40 B)]
[Handle × handle_count]
[opaque payload]
```

Header 由内核构造，包含 magic、version、block_len、pid、parent_pid、handle_count、payload_off、payload_len 与 reserved。Handle 数组保存 reservation 产生的实际 child-local 值；不得依赖连续 slot 或 generation 1。payload 只由 launcher 与 child 解释。

`parent_pid` 仅用于诊断创建关系，不产生任何 authority。

launch 事务顺序：

1. child HandleTable reserve；
2. 用 reservation 实际 Handle 构造 outer block；
3. 只读映射 block；
4. commit Handle entries；
5. 创建主线程，a0/a1 = block base/length；
6. 插入进程表；
7. 调度器 Release 发布 runnable。

任何失败均须 rollback reservation、关闭未安装 entries、回收映射与进程骨架，不发布半初始化进程。

rinlib 只校验 outer 几何，公开：

- `pid()` / `parent_pid()`；
- `startup_handles()` / `startup_handle(index)`；
- `startup_payload()`。

当前 boot loader 与 init/pm 暂以 Handle[0] 约定 mailbox grant；未来 LauncherParcel 在 payload 内按 index 赋予业务 tag。

### Handle 运输 rights

将旧 `TRANSFER` 拆为：

- `TRANSIT`：可进入 mailbox message；
- `GRANT`：可直接跨 HandleTable 安装。

建议 role 最大 rights：

| role | TRANSIT | GRANT | DUPLICATE | 说明 |
|---|---:|---:|---:|---|
| MailboxOwner | 否 | 是 | 否 | affine，launcher 可直授 child |
| NotificationOwner | 否 | 是 | 否 | affine，launcher 可直授 child |
| MailboxSender | 是 | 是 | 是 | badge 随派生保持 |
| MailboxSenderOnce | 是 | 是 | 否 | 成功 Send 后消费 |
| NotificationSignaler | 是 | 是 | 是 | 普通委托 |
| TunnelInvitation | 是 | 是 | 否 | Attach 后消费 |
| TunnelEndpoint | 否 | 否 | 否 | 与进程 VM lease 绑定 |

当前 `HandleMove[]` 与 `HandleTable::extract_moves` 改为要求 TRANSIT。未来 ProcessStart 的直接跨表提取要求 GRANT。两条路径都只能收窄 rights。

### Capability badge

`handle_table::Entry` 增加不可变 u64 badge，默认 0；clone、duplicate、TRANSIT、GRANT 和 rights 裁剪均保持。badge 不放入 lifecycle role 或对象共享状态。

`MailboxCreate` 继续原子返回 owner + 初始 badge-0 sender，避免额外 bootstrap 调用。新增：

```text
MailboxMintSender(owner, badge, rights) -> sender
```

要求 owner 的 MANAGE；新 sender rights 必须是 MailboxSender 最大 rights 的子集。

MessageHeader v2 保持 64 B，字段为：

```text
sender_pid: u64
sender_badge: u64
kind: u64
payload_len: u32
handle_count: u32
reserved: [u64; 4]
```

PID 只作 provenance；badge 是服务端授权上下文，但 badge 数值本身不构成 capability。

`MailboxMakeSendOnce` 必须保留源 sender badge。

### send-once alias

若 target role 是 MailboxSenderOnce 且 moves 中包含同一 Handle，Send 在取得任何对象锁、摘除 Handle 或入队前返回 IllegalArgument。失败不消费 once，随后仍可完成一次正常投递。

## 锁序与短路径

保持：

```text
Send: user copy -> HandleTable -> Mailbox.state
Receive: AddressSpace precheck -> HandleTable -> Mailbox.state -> AddressSpace copy
Mint: HandleTable -> AddressSpace
```

badge 读取、target role 解析、TRANSIT 验证、move 与入队处于同一 HandleTable 临界区。不得引入 Mailbox.state -> HandleTable 反向锁序。

TRANSIT/GRANT 分离用于避免 buffered owner capability graph circularity；Send 不进行对象类型特判、图遍历或环检测。

## 当前实施状态

- [x] StartupBlock v2 shared 布局与 host 测试草案；
- [x] kernel launch 改为 reserve 后构造真实 Handle 数组；
- [x] rinlib outer parser 与 init/pm Handle[0] 迁移；
- [x] Entry badge、MessageHeader badge、MailboxMintSender 草案；
- [x] send-once target/transit alias 拒绝草案；
- [x] 完成旧 TRANSFER → TRANSIT 机械迁移并加入 GRANT role 矩阵；
- [x] 移除 owner 经消息移动的临时测试，改为验证 owner 无 TRANSIT；
- [x] 检查所有 close_transit、rights 裁剪和错误回滚路径；
- [x] `cd shared && cargo test --target aarch64-apple-darwin`；
- [x] `cd os && cargo test -p handle_table --target aarch64-apple-darwin`；
- [x] `just check`、`just build_user`；
- [x] 单独构建后，以规定运行超时执行 `just virt`；
- [x] reviewer 只读审查最终 diff并修正低风险项；
- [x] 更新 notes/ideas、notes/impls、notes/README 与 plans/COMPASS。

## 验证场景

### StartupBlock

- 0、1、多个实际 Handle；
- 非连续 slot、非 1 generation 的值按原样出现；
- 空与任意 opaque payload；
- header/handles/payload 几何和截断校验；
- reserve/build/map 任一步失败不发布 Handle 或进程；
- init/pm 不再依赖固定 Handle 数值。

### Badge

- badge 0 初始 sender；
- owner mint 非零 badge；
- duplicate、message TRANSIT 和 send-once 保持 badge；
- MessageHeader 的 sender_pid 与 sender_badge 均由内核生成；
- mint rights 放大和错误 role 被拒绝。

### 运输权利

- owner 可请求 GRANT、不可请求 TRANSIT；
- owner duplicate 失败；
- owner 放入 HandleMove 因缺少 TRANSIT 原子失败且源项保留；
- sender/signaler/invitation 的 TRANSIT 保持；
- Endpoint 仍不可移动。

### send-once

- 成功一次后 stale；
- 满箱失败不消费；
- 经其他 sender TRANSIT 后由接收方使用一次；
- target/transit alias 整体失败，once 仍能随后成功一次；
- badge 在派生与投递后不变。

## 完成标准

所有检查与 host 测试通过；`just virt` 对照负载完整运行并按既定日志验收；最终代码不存在 fixed startup slot、旧 TRANSFER 名称、owner message transit 或 sender/badge 混义。此后再修订 notes，使 ideas 表达最终方向、impls 记录真实落地，不保留施工历史。
