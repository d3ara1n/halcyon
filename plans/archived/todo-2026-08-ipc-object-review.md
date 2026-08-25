# todo：IPC 对象 / Handle 统一 review

状态：**待统一 review 节点执行**。本文件只保存本次实施的提交边界、任务上下文和核证入口，不代表已经完成 review，也不要求在 IPC 重建任务结束时立即审查。

## 提交范围

| 性质 | 提交 |
|---|---|
| 审查基线（不含） | `4c06a99a3c6979b34f199bad4d0ce653d39e7720` `docs(plans): 立 IPC 三面实现的 review 计划` |
| IPC 对象重建实现 | `836e5017b05139e2a79903bce6da3595a9e198b5` `feat(ipc): 重建对象与 Handle 基础` |

后续统一 review 以 `4c06a99a3c69..836e5017b051` 为本任务代码范围。旧实现的审查档案是 `plans/review-2026-08-ipc.md`，只用于理解被替换问题，不可当作新实现已经通过审查的证据。

## 原任务上下文

目标是整体替换旧 message/signal/tunnel/Runnel，而不是逐项修补：

- 普通 IPC 从 PID/全局 id 寻址改为进程本地 `u64 Handle`；
- generation 防陈旧引用，rights 与 lifecycle role 分离；
- WaitMany 成为对象等待的唯一入口，以单 WaitContext 解决安装、arm、完成和取消竞态；
- Mailbox Send/Receive 与 Handle move 是全提交或全回滚事务；
- Notification 独立承载显式消费的 OR 累积事件；
- Tunnel 改为 Connection/Endpoint/Invitation，无全局 registry 或 bearer id；
- Runnel 使用对齐原子 Acquire/Release、角色视图、对端游标验证和永久 Broken；
- Sleep deadline 复用 WaitContext；
- shared/kernel/rinlib/服务一次纵向切换，不保留旧 ABI。

方向契约见 `notes/ideas/{object,wait,ipc,message,signal,shared-memory,tunnel,runnel,service}.md`；实际实现入口见 `notes/impls/ipc.md`；实施档案见 `plans/archived/2026-08-ipc-object-foundation.md`。

## 未来 review 范围

统一 review 时至少覆盖：

1. **ABI**：固定宽度、对齐、reserved、调用号、`a0–a5`、用户输出的成功可见性；
2. **Handle**：generation 回绕退休、rights 裁剪、role、duplicate/move/drain、失败守恒；
3. **等待**：Installing/Armed 竞态、重复 Handle 仲裁、关闭/期限/取消/退出、订阅清理和唯一入队；
4. **消息**：锁序、满箱不 move、Receive reservation/token、失败不出队、owner close 与 transit close；
5. **Tunnel**：Invitation 单次消费、attach/close 线性化、映射 reservation、进程退出和帧守恒；
6. **Runnel**：RVWMO 配对、EOF 读取顺序、shadow cursor、Broken、门铃确认闭环；
7. **启动授权**：只审查当前过渡实现的安全性；最终模型由 `todo-2026-08-process-startup-resources.md` 另行设计，不能把 `StartupMailbox` 当作冻结方向。

## 已有验证证据

实现提交形成时已通过：

- HandleTable 10 项 host 单测；
- WaitContext 4 项并发 host 单测；
- Runnel 8 项 host 单测，含双线程多轮回绕；
- `just check`、shared check、全部用户程序 check；
- os 纯逻辑 crate 全量 host 测试；
- `virt` 四核最终压力构建 3/3；
- `sifive_u` 四个可运行 hart 完成同一负载并进入 quiescent；
- 集成负载含 128 轮控制面事务、满箱不 move、64 轮 Tunnel 生命周期和 8192 字节 Runnel。

这些是回归基线，不替代未来 review。

## 完成条件

统一 review 节点实际执行后，把结论写入只读 `review-<日期>-<主题>.md`；修复项另立 todo。确认本提交范围的结论已被新档案承接后，再归档本文件。
