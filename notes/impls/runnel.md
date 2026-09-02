# Runnel 实现

Runnel 是运行在 Tunnel 映射区上的用户态单工 SPSC 字节流协议。内核只提供映射、端点、门铃和生命周期；页内控制块的格式、角色访问、游标校验和内存序全部由 `user/frameworks/librunnel` 封装。

## 当前版本与布局

当前实现位于 `user/frameworks/librunnel/src/lib.rs`，仍是 RNL1，使用一个 4 KiB Tunnel 页：前 128 B 为控制区，后续空间为数据环，容量为 `4096 - 128` B。控制字段为 little-endian `MAGIC`、`VERSION`、`HEAD`、`TAIL` 和 `EOF`；协议版本值为 1。

Producer 只写 `HEAD`/`EOF`，只读 `TAIL`；Consumer 只写 `TAIL`，只读 `HEAD`/`EOF`。双方以本地 shadow 游标检查对端是否只向前推进、`used <= capacity` 且 EOF 只取 0/1；违反约束即把本端置为 Broken。数据写入先完成，再以 Release 发布游标；读取方以 Acquire 读取对端游标后访问数据。

## 用户态封装

`Producer` 与 `Consumer` 持有 Tunnel Endpoint Handle 和映射基址，构造入口区分创建方与接入方。`create_producer`、`create_consumer` 通过 TunnelCreate 建立连接；`attach_producer`、`attach_consumer` 通过 TunnelAttach 消费 Invitation。Attach 后的页内 magic/version 校验失败会关闭已取得的 Endpoint Handle，并向调用方报告 `BadMagic`。

Producer 提供 `writable`、`write`、`set_eof`、`write_all`、`finish`；Consumer 提供 `readable`、`read`、`eof_reached`、`read_exact_or_eof`。阻塞封装遵循“检查 → 无进展时 acknowledge → 重查 → WaitMany”的闭环；取得进展后通过 TunnelNotify 提示对端。等待观察 `DATA | PEER_CLOSED | CLOSED`，真实可读/可写量始终来自控制块。

`RunnelError` 将页内校验失败区分为 `BadMagic`/`Broken`，将 Tunnel 终态映射为 `Closed`，其它系统调用错误保留为 `Syscall`。协议层不携带 Handle，不登记 MemoryObject，不定义记录边界，也不承担 BufferQueue 的 descriptor 或 buffer ownership。

## 边界与后续

Runnel 只负责字节流；双工通信由两条方向相反的单工 Tunnel 组合。记录边界、预注册 MemoryObject region、descriptor 和缓冲交接属于并列的 BufferQueue，不进入本协议。

多页 Tunnel 与 RNL2 尚未实现；当前实现仍以单页 RNL1 为准。方向契约见 [`../ideas/tunnel.md`](../ideas/tunnel.md) 与 [`../ideas/runnel.md`](../ideas/runnel.md)，实施进度由 [`../../plans/todo-2026-09-memory-object-data-plane.md`](../../plans/todo-2026-09-memory-object-data-plane.md) 记录。

## 验证入口

`librunnel` 的 host 测试与 init 验收覆盖创建/接入、角色访问、环形游标、EOF、Broken、门铃和关闭；跨 hart、Endpoint lease、Invitation 生命周期与资源守恒由 [`tunnel.md`](tunnel.md) 及内核验收负责。
