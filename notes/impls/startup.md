# 启动资源交付实现

方向见 [`../ideas/object.md`](../ideas/object.md)「StartupBlock」与 [`../ideas/task.md`](../ideas/task.md)。当前内核已落地通用 StartupBlock v2；公开用户态 ProcessCreate/ProcessStart 尚未接入，boot loader 暂代 launcher。

## Outer ABI

`shared/src/startup.rs` 定义：

```text
[StartupBlockHeader (40 B)]
[Handle × handle_count]
[opaque payload]
```

Header 包含 magic、版本、块长、pid、parent_pid、Handle 数、payload 偏移/长度与 reserved。`validate_startup_block` 只校验 outer 几何、reserved 和实际 Handle 值，不解释 payload。

旧 descriptor/tag 与“index → slot+1、generation 1”推导已删除。Handle 区直接保存 child HandleTable reservation 产生的真实值，允许非连续 slot 和任意有效 generation。

## launch 事务

`os/kernel/src/task/proc.rs::launch` 的当前顺序：

1. 为全部输入 entries 在尚未发布的 child HandleTable 中 reserve；
2. 以 reservation 的实际 Handle、`Process.pid/parent` 与 launcher payload 调用 `build_startup_block`；
3. `AddressSpace::map_startup_block` 在 ELF 后、堆前只读映射完整 outer；
4. commit entries；
5. 创建主线程，`a0 = block base`、`a1 = block length`；
6. 插入进程表，调用方随后 enqueue 发布 runnable。

reserve/build/map 失败都 rollback 临时 Handle，并按目标进程退出语义关闭未安装 entries；进程不进入表。commit 后不再存在可恢复失败步骤。块帧归 `AddressSpace.frames`，随进程回收。

## 当前 boot launcher

`os/kernel/src/initfs.rs` 装载四个服务，暂时创建 pm Mailbox：

- pm 的 StartupBlock `Handles[0]` 是 owner，目标 rights 含 READ/WAIT/MANAGE/GRANT；
- init 的 `Handles[0]` 是 badge-0 sender，含 WRITE/WAIT/TRANSIT/GRANT/DUPLICATE；
- fs 与驱动当前无启动 Handle；
- payload 当前为空。

Handle[0] 的业务语义只是 boot launcher 与对应二进制的临时约定，不属于 outer ABI。服务化后由用户态 LauncherParcel 在 opaque payload 内按索引描述资源。

## 接收方

`user/rinlib/src/rt.rs` 在任何用户代码前把入口 a0/a1 交给 `env::init`。`user/rinlib/src/env.rs` 使用 shared validator 校验 outer，然后提供：

- `pid()`、`parent_pid()`；
- `startup_handles()`、`startup_handle(index)`；
- `startup_payload()`。

rinlib 不解释 payload，不按 tag 查资源，也不猜 Handle 数值。解析失败触发用户态 panic 并干净退出，不影响内核。

## 验证

- shared host 测试覆盖实际非默认 Handle 值、空资源、opaque payload 和 outer 几何损坏；
- handle_table host 测试覆盖 reservation、TRANSIT/GRANT 与 badge 保持；
- init/pm 集成负载从实际 StartupBlock Handle[0] 建立跨进程 Tunnel 与流控；
- `just virt` 四服务完成后全员回收并静默停机。
