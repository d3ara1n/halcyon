# todo：进程身份与启动资源交付

状态：**已完成（2026-10）**。方案 A 机制 + C 门面已实施完毕，方向入 `notes/ideas/object.md`/`service.md`，实现记录入 `notes/impls/startup.md`，本计划归档。

## 前置结论（方向已定）

- startup 的内核形状只有两个物理动作：进程 runnable 前批量原子安装 Handle（根图的唯一例外），与投递版本化 STARTUP 消息。清单的语义——tag、类型、标签、名字前缀——是授权方与接收方库之间的版本化用户态协议，内核零解释；Mailbox 无特殊形状，`StartupGrant` 即 descriptor 的雏形。
- 因此下述 A/B/C 的分歧只在清单的存放与读取时机，上述底座不变。
- namespace 授权的形状是成对交付：`StartupGrant`（新 tag）+ payload 中的（名字前缀, handle_index）；前缀是消息字节，内核无感。成对交付才是完整授权语义——“给你一个目录，并声明它在你的视图中的名字”。

## 已确认方案（2026-10，A 机制 + C 门面）

经外部调研（`ref-2026-08-startup-research.md`）与多轮讨论拍板：

- **机制选 A（用户只读映射启动块）**，不选 B（查询 ABI）/C-proper（Process-self 对象）：A 是唯一让内核在 launch 事务结束后完全退出启动事务的形状——零常驻清单、零启动 syscall、块随地址空间生灭；B 会扩大 uaccess 写回 panic 面，C 的唯一刚需（入口查身份）已被「身份进块头」消解。
- **入口契约**：`a0` = 块指针（动态 VA，映像后、堆前），`sp` = `USER_TOP`；pid/parent_pid 进块头。不引入固定块地址，不自定义 `_start`（Rust `#[lang = "start"]` 从 argc/argv 槽位取 a0/a1 已够用）。
- **launch transaction**：装载 ELF → 分配帧写入 manifest 字节 → 只读映射（R|U 无 W）→ handles[] 按数组顺序装进槽位 0..N（连续槽位约定，descriptor 的 handle_index 语义即槽位号）→ 设入口寄存器 → enqueue。失败全量回滚；runnable 即事务终结，内核对启动零尾随状态。块帧记入 `AddressSpace.frames`，生命周期归进程。
- **manifest 对内核是不透明字节串**：内核不 parse、不校验头；块长度与 handle 数来自 launch 参数。版本化、未知 tag 忽略规则、payload 透传（如 fs 路由表）全在 rinlib 侧。
- **资源三分**：信息（args/配置/归档字节，随块过境）、能力（create 类 syscall，对所有进程无差别开放，不存在下放）、权利（对他人对象的 Handle，只能沿进程树向下流动且单调收窄；权利之源是对象创建者，不是内核）。「服务出生自带 mailbox」是 pm 的组装惯例（声明驱动），不是内核机制。
- **init 终态**：普通 rinlib 进程，manifest = 身份 + 整个 initfs 归档字节（payload 区透传，tag 语义属 boot loader ↔ init 私有协议），handles 可以为零（需邮箱自己 create）。本次不实施归档 handover（两步走）：本次内核 loader 仍起四服务但按新契约组 manifest；归档 handover 与「内核只 spawn init」收缩留到服务化阶段（ProcessCreate 就绪后组装代码平移到 init）。
- **manifest 上限是 launcher 策略参数**（机制只受帧池约束），不写死在 ABI。

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

- [x] shared/kernel/rinlib/loader/服务一次纵向切换：StartupBlock ABI（`shared/src/startup.rs`）、launch 事务（`task::launch` + `map_startup_block`）、rinlib 块解析（`env::init`）与 loader 组装（`initfs.rs`）；
- [x] 验证：`virt` 与 `sifive_u` 四服务全绿（含 fs 验收线、init/pm 全套 IPC 测试）；帧守恒与基线一致（quiescent 差 251 帧）；无 Handle/空清单进程（fs/drv）正常启动；结果只依赖 a0；shared host 测试钉住块布局与槽位约定；
- [x] `StartupMailbox`、`Process.bootstrap_mailbox`、`enqueue_startup` 与强制 STARTUP 接收路径全部删除，代码零残留；
- [x] 方向入 notes（ideas/object.md、ideas/service.md）、实现入 notes/impls/startup.md，本计划归档。
