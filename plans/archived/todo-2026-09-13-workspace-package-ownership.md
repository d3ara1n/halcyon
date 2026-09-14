# os/shared/user workspace 包归属整理

> 状态：已完成，已归档。此项是独立的 workspace/package 重新组织，不改算法语义或公共契约。已完成接手、规模审计、拆分/合并、设计闭包、迁移和分层验证。此文件唯一承接共用包的归属与公共算法契约；不另建共享库整理 todo。

## 目标与边界

os、shared、user 三个 workspace 作为各领域包的容器：内核专用机制属于 os，用户态框架/服务属于 user，内核与用户态实际共用的可移植算法和契约包属于 shared。ABI 包和共用算法包保持独立 Rust 身份，不能因为共用便把通用算法塞进 erhino_shared 的 ABI 模块，也不复制同一算法两份。

当前 `shared/` 已组织为独立 workspace：`erhino_shared` ABI package 与 `elf`、`tar`、`monotonic_id`、`ordered_table`、`timer_queue`、`metadata_admission` 共用纯逻辑 package 分置于独立目录。保持内核和用户态不同 target/运行时，不把三者合成一次 target 构建。

## 现状与候选

跨层使用的可移植纯逻辑 package 统一归入 `shared/`：

- `elf`、`tar`：跨组件使用的格式解析器；
- `monotonic_id`：跨组件使用的不回绕身份分配器；
- `ordered_table`、`timer_queue`、`metadata_admission`：跨组件使用的索引、期限和准入原语。

这些包不依赖 kernel 运行时；归属依据是跨层可移植性与共享契约，而不是当前消费者数量。`user/frameworks/` 只承载用户态公共框架与正式库，`rinlib` 只承载用户态运行库。内核专用且没有用户态共用需求的纯逻辑 crate 继续留在 `os/`。

## 目标布局与迁移决策

```text
shared/
  Cargo.toml                 # workspace
  erhino_shared/             # package erhino_shared，ABI
  elf/ tar/ monotonic_id/    # 跨层纯逻辑 package
  ordered_table/ timer_queue/ metadata_admission/
```

`tar` 与 `elf` 同属跨层共享纯逻辑库；当前 `srv_init` 是 tar 的消费者，但消费者位置不改变其 shared 归属。迁移只调整目录、workspace、path 依赖和构建入口，不将算法并入 `erhino_shared`，不修改 `TimerToken`、`OrderedTable` 或 admission 契约。

## 迁移前置与设计结论

本项位于运输/通用执行/RPC 闭包之后、内核执行结构收束与正式 FAL 后端扩展之前；实际迁移保持算法语义不变，并与主线机制改造分离验证。TimerQueue 的稳定 token、预付容量和 park/reschedule 契约保持原样；`owner_slot` 仍由调用侧解释（内核使用 hart slot，用户态使用固定 slot），不在本次目录整理中重构。OrderedTable 的预付节点继续区分已分配存储与已预留表容量，准备后定 key 的接口保持不变。

## 自然实施顺序

本计划执行前遵循 `AGENTS.md`「标准施工流程」；以下只记录包归属专题的审计对象、迁移顺序和完成门。

1. 已完成 package/消费者/target/feature/构建入口盘点，并确认 `tar` 归属 `shared/`。
2. 已确定三 workspace 目标成员图：`shared/` 为跨层 workspace，`erhino_shared` 下沉为独立 package 目录；`os/` 保留内核专用 package，`user/` 保留用户态运行库、框架、服务、驱动和测试。
3. 已迁移全部共用 package 及所有内核/用户态消费者、工具入口，删除旧目录与补偿别名，不保留双轨 crate。
4. 已完成三 workspace 的 host/RISC-V 检查、单测、clippy 和 QEMU core 组合验证，确认目录迁移保持语义；本次未调整算法契约。`shared` workspace 73 项 host 测试、`os` 剩余 workspace host 测试、`just check`、七面 `just clippy`、`just build_user` 和 `just virt` 均通过。同步 AGENTS、实现导航与构建文档。

## 完成与删除门

共用包不再由用户态通过 `os/` 目录取得；`erhino_shared` 的 ABI 身份与语义保留，三 workspace 的成员/依赖/构建入口一致，所有旧路径与别名删除，验证证据可定位。持久包归属原则已同步至 `AGENTS.md` 与现行实现文档；本计划完成后归档并从 `COMPASS` 删除活跃入口。
