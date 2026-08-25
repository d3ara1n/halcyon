# todo：进程身份与启动资源交付

状态：**待设计**。本计划替换 IPC 重建期间的 `StartupMailbox` 过渡模式；核心设计需先比较方案并由用户确认，再修改 ABI 和代码。

## 问题

当前实现位于：

- `os/kernel/src/task/proc.rs`：每个进程固定安装 bootstrap Mailbox，并在 `Process` 保存其 Handle；
- `shared/src/call.rs`：临时 `StartupMailbox = 0x18`；
- `os/kernel/src/syscall.rs`：查询当前进程的固定 bootstrap owner；
- `os/kernel/src/initfs.rs`：装载器无条件投递 STARTUP 消息；
- `user/rinlib/src/rt.rs`、`env.rs`：进入 `main` 前查询 Mailbox 并强制接收消息。

它完成了 IPC 纵向迁移，但把 Mailbox 提升成所有进程启动时必有的特殊资源。Mailbox 的性质更接近 args、环境、设备或服务 grant：可选、由启动策略提供、运行时主动探索；它不应占固定入口寄存器、固定 Handle slot、`Process` 专用字段或专用 syscall。

现有 Rust 生成入口还带来一个实际约束：ELF `main` wrapper 只保留内核入口的 `a0/a1`，会重排并覆盖后续参数。不能继续假设 `a2` 能直接到达 rinlib，也不能为绕过该事实而截断或打包 Mailbox Handle。

## 目标

- 把**进程身份**与**附属启动资源**分层；
- 明确 PID、parent 关系属于何种内核身份契约，以及用户态是否需要在入口立即取得；
- 以通用、版本化、可扩展的方式枚举 args/environment/Handle grants；
- Mailbox 只是 typed/tagged startup resource，可有可无；
- Handle 仍由内核在进程 runnable 前原子安装，清单只描述已授权的本地 Handle；
- 未来 init/pm 接管 ProcessCreate/ProcessStart 时复用同一契约；
- 完成后删除 `StartupMailbox` syscall、`Process.bootstrap_mailbox` 和强制 STARTUP 接收路径，不留双 ABI。

## 必须先决定的方案

### A. 用户映射的只读启动块

内核/父进程在启动事务中构造版本化 startup block，包含身份元数据、args 和 typed/tagged Handle descriptors；入口寄存器只传身份或 block 指针。rinlib 从内存主动解析。

- 优点：启动快照自包含，枚举无需多次 syscall，接近 args/auxv 心智模型；
- 代价：需要正式定义启动块映射、大小上限、用户栈/独立页布局和回收时机；若要摆脱编译器 `main` wrapper 的两参数限制，需要受控 `_start` trampoline。

### B. 通用 StartupResource 查询 ABI

入口保持最小身份参数；rinlib 用版本化 `StartupInfo/StartupResourceEnumerate` 查询数量和 descriptors。接口覆盖 args、环境和 Handle grants，不对 Mailbox设专门调用。

- 优点：不依赖编译器入口传参细节，容易先建立正确对象边界；
- 代价：启动需要 syscall，必须定义快照一致性、一次性读取/重复读取和进程何时可释放内核清单。

### C. 进程自身对象 + 属性/资源查询

启动时交付或隐式提供 Process-self 能力，通过它查询身份与启动资源。

- 优点：身份、管理与查询都进入对象模型；
- 代价：若 Process-self 仍是固定隐式 Handle，只是把特殊值换了名字；还会提前扩大进程管理 ABI。本方案只有在 Process 对象设计本身成立时才可选。

不得在未确认前自行选择。调研应以成熟系统的进程初始栈/auxv、启动 Handle 表或 bootstrap namespace 的官方 ABI/源码为证据，独立推导本项目契约。

## 设计问题

1. PID 是否继续由 `a0` 直接提供；parent 是否属于进程可查询属性而非入口必需数据？
2. 是否引入自定义 `_start`，以及它与 Rust `#[lang = "start"]`、用户栈和未来 argc/argv 的边界？
3. startup resource descriptor 的版本、类型、标签、rights、Handle index、长度与 reserved 布局；
4. 清单是不可变快照还是可消费队列；重复查询是否返回同一 Handle 数值；
5. 未识别资源的忽略规则、必需资源缺失、重复标签和上限；
6. ProcessCreate/ProcessStart 如何在「装载映像、安装 Handles、发布清单、创建线程、变为 runnable」之间保证原子性；
7. 启动失败、进程未运行即死亡、用户解析失败时，谁关闭未接收资源；
8. 内核 loader 过渡到 init/pm 策略后，哪些机制保留、哪些策略整体删除。

## 实施顺序

1. 从固定规范和成熟实现取得进程入口、auxv/启动清单及 Handle bootstrap 的证据；
2. 在 `notes/ideas/object.md`、`service.md` 固化经确认的身份/资源分层契约；
3. 在 shared 定义最终启动 ABI、上限和 reserved 规则；
4. 内核实现统一 launch transaction，先供 initfs loader 使用；
5. rinlib 改为通用枚举和按类型/标签探索，不强制每个进程拥有 Mailbox；
6. 服务显式声明必需资源并处理缺失；
7. 删除 `StartupMailbox` 及所有过渡字段、封装和文档；
8. 为未来公开 ProcessCreate/ProcessStart 保留同一机制入口，不复制 loader 私有协议。

## 验证

- 无 Mailbox、一个 Mailbox、多个不同标签 Mailbox 均能启动；
- 只有 args 或只有非消息 Handle 的进程不触发消息机制；
- 未知可选 descriptor 可忽略，未知必需 descriptor 明确拒绝；
- 清单截断、版本错误、重复标签、越权 rights、无效 Handle index 均失败且资源守恒；
- runnable 发布前失败不暴露半安装 Handle 或半初始化线程；
- `virt` 与 `sifive_u` 启动四服务，结果不依赖 ELF 编译器碰巧保留 `a2+`；
- 代码和文档中不再出现临时 `StartupMailbox`。

## 完成条件

经用户确认最终方案后，shared/kernel/rinlib/loader/服务一次纵向切换；所有验证通过，方向进入 notes、实现进入 `notes/impls/`，本计划归档。
