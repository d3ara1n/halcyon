# 启动资源交付实现

> 进程身份与启动授权的概念层见 [`../ideas/object.md`](../ideas/object.md)「身份、寻址与启动授权」与 [`../ideas/service.md`](../ideas/service.md)；本篇记录 StartupBlock 机制的落地现状。

## 契约

进程启动 = 一个只读快照 + 一组已安装 Handle。launch 事务（`os/kernel/src/task/proc.rs::launch`）在进程 runnable 前完成：

1. 只读映射 StartupBlock（`map_startup_block`）：块基取 ELF 尾页对齐处（当前 brk），字节经直映射别名写入，映射权限 `USER_RODATA`（进程对块自身也不可写），brk 越过块尾——堆从块后扩展，`Extend` 的 sbrk 语义对块无感；
2. 按数组序安装 Handle：reserve→commit 事务保证一次可见、无半安装状态；空表顺序安装必落槽位 `index+1`、generation 1（`shared::startup::startup_handle` 槽位约定），内核在预留后显式断言；
3. 创建主线程：`a0` = 块基、`a1` = 块字节数（pid/parent 在块头内）、`sp` = 半区顶——内核提供可信长度，接收方与块头 `block_len` 等值校验，截断在入口即确定性拒绝；
4. 入进程表并 enqueue。

失败全量回滚：已装 Handle 随 `Process::drop` 关闭，未消费 Handle 由 launch `close_transit`，进程表不出现半初始化条目。runnable 即事务终结——块帧记入 `AddressSpace.frames` 随地址空间生灭，内核对启动零尾随状态（无清单副本、无快照冻结、无查询 syscall）。

## 块格式（`shared/src/startup.rs`）

```
[StartupBlockHeader (40B)][StartupDescriptor ×N (24B each)][payload 字节区]
```

- header：magic（"STARTUPB"）、`block_len`、`version`、pid/parent_pid、`descriptor_count`、`handle_count`、reserved（读侧必须为零）；
- descriptor：`tag`（语义属授权方 ↔ 接收方私有协议，内核不解释）、`handle_index`（`NO_HANDLE = u32::MAX` 哨兵，否则指向第 i 个安装的 Handle）、`data_off`/`data_len`（payload 区相对块基）、reserved；
- payload 区：args 字符串、fs 路由表、initfs 归档等任意字节，均以 descriptor 引用，内核零解析；
- 组装器 `StartupManifest`（同文件）：`add(tag, handle_index, data)` 追加、`finish(pid, parent, handle_count)` 产出完整块；payload 偏移在 finish 统一计算（descriptor 表长度随后续 add 增长，提前计算会与表区重叠——host 测试钉住）；
- 标准 tag 常量：`TAG_MAILBOX_OWNER`（服务出生自带的邮箱 owner，授权方惯例）、`TAG_PM_MAILBOX`（loader ↔ init）、`TAG_INITFS_ARCHIVE`（loader ↔ init，服务化阶段启用）。

## 授权方

当前授权方是内核 boot loader（`os/kernel/src/initfs.rs`）：装载四服务并为 pm 组装邮箱对——owner 进 pm 的块（`TAG_MAILBOX_OWNER`）、sender 进 init 的块（`TAG_PM_MAILBOX`），与 tar 内条目顺序无关；未认领侧（如 init 缺席）transit 关闭，不泄漏。fs/drv 以空清单（仅 header）启动：无邮箱、无 Handle、无消息路径——「服务出生自带邮箱」是授权方组装惯例而非内核机制。

服务化阶段授权策略整体迁往 init/pm：init 从块内 `TAG_INITFS_ARCHIVE` payload 取归档字节，读配置、现写 manifest、经公开 `ProcessCreate` 交付；launch 机制与块格式原样复用。终态下内核只 spawn init 一个进程，其块携带归档，handles 可为零。

## 接收方（rinlib）

`user/rinlib/src/rt.rs` 的 `lang_start` 在任何用户代码前以 a0/a1（argc/argv 槽位）调用 `env::init`：校验可信长度与块头 `block_len` 等值、magic/版本/reserved/长度自洽/payload 段界/handle_index 界内/tag 唯一，失败即拒绝启动（panic → 干净退出）。`user/rinlib/src/env.rs` 保存块基指针，此后：

- `env::pid()` / `env::parent_pid()`：块头身份；
- `env::startup_handle(tag)`：descriptor 查找 → 按槽位约定复原 Handle 数值；
- `env::startup_payload(tag)`：返回块内 payload 的 `&'static [u8]` 切片（块不可变、随地址空间存活）。

未知 tag 天然跳过（按需查找，无消费语义）；必需资源缺失由服务显式声明并处理（如 pm 对 owner grant 的 expect）。
