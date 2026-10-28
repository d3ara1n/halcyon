# init 组态与统一服务契约

状态：**目标设计已定稿，待实施；尚未修改产品代码**。本篇是本任务唯一工作记录，拥有目标设计、实施顺序、验证门、债务与接力。上游职责边界（init 根监督、`srv_service` 分工）由[服务装配计划](todo-2026-10-07-service-assembly.md#已确认的-init-与服务管理职责)拥有；系统停止政策由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有；设备安全事实由[设备计划](todo-2026-09-device-system-design.md)拥有。

## 目标与非目标

目标：init 编译为一个固定二进制，由 initfs 中的**组态文件**（formation）、映像与服务设定选择系统初始组态；System 与各验收部署只通过更换 initfs 区分。

前提：统一服务出生、就绪、停止与撤销契约，使 init 不链接、不解码任何领域协议。

已确认的决定：

- 组态文件是数据（实例、映像、资源、能力路由、生命周期种类），不是脚本。init 不执行消息编解码、条件或循环，不解释领域协议。
- 系统服务与用户服务共用同一契约，不按 `srv_` 前缀或启动者拆库，不各自实现握手。差别只在监督方政策（init 下失败进系统终局，`srv_service` 下按其恢复政策）和描述丰富度。
- 命名采用本项目自己的领域词，不沿用其他系统的惯例目录或术语（如 `etc`、`manifest`、`rc`）。

非目标：`srv_service` 的实现、用户服务描述的格式、服务恢复与重启、持久化。本任务只保证契约与组态 schema 核心可被 `srv_service` 复用。

## 命名

| 对象 | 名称 | 说明 |
|---|---|---|
| initfs 根的组态文件 | `formation` | 二进制；init 读取的唯一入口 |
| initfs 映像目录 | `image/<组件名>` | 由现有 `bin/` 改名，与 `libprocess::image`、`StartupImage` 的用词一致 |
| 组态 schema crate | `shared/formation` | no_std 编解码与静态校验；`host` feature 下的同名 bin 为编译器 |
| 服务设定 crate | `shared/settings` | 类型化值树的编码与只读访问 |
| 组态源文件 | `user/formations/<部署名>.toml` | 如 `system.toml`、`core.toml`；按部署而非平台命名 |
| 出生输入 | `libprocess::birth` 模块，类型 `Birth` | |
| 实例契约 | `libservice::instance` 模块 | 不复用 libservice 现有目录“服务描述”类型 |
| 生命周期保留输入 | `report`、`stop` | 由 `libservice::instance` 定义的常量 |
| 实例生命周期种类 | `resident`、`task` | |
| 全部 task 完成后的动作 | `finish = stay \| shutdown \| reboot` | |

源格式用 TOML 的理由：编译器只在 host 运行，需要注释、表与数组，Rust 生态有成熟解析器；init 内不出现文本解析器。依赖版本按全局规则固定。组件命名按 [user/README](../user/README.md) 命名章节落地，新 crate 名在实施前于该篇登记。

## 现状（代码盘点，2026-10-28）

srv_init 约 17.3k 行，直接依赖 `srv_fs`、`test_fs`、`libdevice`、`libdriver`、`libpci`、`libusb`，可选 `test_device`；验收约占 55–60%（粗估）。三个常驻宿主做同一件事，写法各自独立：

| 环节 | srv_device | srv_driver | srv_fs |
|---|---|---|---|
| 出生槽位 | 0 报告、1 Pool、2 平台描述、3+ 平台根 | 0 报告、1 Pool、2 worker Job、3 ClaimSource、4+ 映像 | 0 报告、1 停止通知、2 路由 receiver、3 登记 receiver、4 Pool |
| payload | `DEVPLATF` + 根数 | init 用 Rust 常量编码的声明表 | `Profile` 枚举 |
| 就绪 | 一条：registry mailbox KOID + 2 句柄 | 一条：两 mailbox KOID + 2 句柄 | 四种根授权消息（按 kind 区分角色，非句柄上限所迫）+ 空 Ready，位掩码 15 或 2 |
| 就绪后 | init 发 6 次 RPC，含权限拒绝、Watch 拒绝、枚举、OAL 等验收断言 | 无 | 无 |
| 等待 | 阻塞 `receive_until` 5s | 阻塞 `receive_until` 5s | 并入监督轮询 10s |
| 停止 | init 封 observer、等 Watch Revoked、封根、轮询 `QuerySourceStop`；服务无 Stopped | init 发 Stop RPC op1、封 ClaimSource、轮询 `QuerySourceStop`；服务无 Stopped | init signal 停止通知，服务发 `PROVIDER_STOPPED` |
| Pool | 固定 8 MiB + 主栈 | `WORKER_POOL` ×（有 ECAM 5 份，否则 1 份） | ELF load runs + `HEAP_PAGES` |

部署选择散在服务内：`srv_fs::Profile` 决定入口数、是否承载 registry、报告消息数；驱动匹配、优先级、信任位写在 init。故障终局在 `supervision::Terminal::run` 中硬编码 libdevice `SealSource`。部署由 `system` 与十余个 `acceptance-*` feature 选择，Justfile 按 workload 决定 initfs 条目。

### 已核实事实

| 项 | 结论 | 出处 |
|---|---|---|
| 平台上限 | `MESSAGE_HANDLE_MAX = 8`、`MAILBOX_CAPACITY = 16`、`PAYLOAD_MAX = 4096`、`PROCESS_MAX_GRANTS = 64`；出生 payload 无固定字节上限，仅受启动块 `u32` 几何约束 | `shared/erhino_shared/src/message.rs`、`proc.rs`；`libprocess/src/preparation.rs` |
| seat 释放 | 只有 `release_binding` 移除活跃 seat，要求 binding 处于 Stopping、依赖 binding 已释放、提供方进度为 Closed/Rejected。资源已暴露时，清理等待消费者 `ReportQuiescent`（`REPORT_STOPPED` 权限）。封源或关闭最后 Lifetime 只发起停止，不释放 seat | `libdevice/src/binding_work.rs:124,588,645`、`registry.rs:1059`、`srv_device/src/server/platform.rs:222` |
| `QuerySourceStop == Closed` | 该来源子树影响的活跃 seat 为空。正常停止路径下，它蕴含所有已暴露资源的 binding 均已由执行者报告静止；执行者异常死亡时不保证最终 Closed，可能一直 Stopping 或 srv_device 因期限故障 | `libdevice/src/registry.rs:537,575` |
| fs 预建 receiver | 登记 receiver（registry profile）与 secondary 的 publication inbox 共用槽 3；secondary 依赖 init 在 Start 前把 publication authority 投入 inbox。路由 receiver 成为 fs 管理 receiver。没有找到必须由 init 建立的付款或身份理由 | `srv_init/src/filesystem/launch.rs:176`、`srv_fs/src/server/boot.rs:156,202` |
| 就绪 KOID 消费者 | device incarnation 与 driver `ManagerReady` 两个 KOID 只有 init 解码 | `srv_device/src/server/source.rs:257`、`srv_driver/src/server/manager.rs:398` |
| driver Stop op1 | 除 init 外，`test_device` 只把它发给 observer 端作为权限负探针 | `test_device/src/drivers.rs:62` |
| 平台描述 | 格式属于 `erhino_shared::platform`：头含 `resource_count`，资源记录含 kind、DT 节点偏移与初始槽号；不含 ECAM 标志。ECAM 识别需要 libpci 解析 DTB | `erhino_shared/src/platform.rs`、`libpci/src/platform.rs:250` |
| worker Job | manager Job 与 worker Job 是 `services` 下的兄弟；manager 死亡不封、不杀 worker。系统终局封 `services` 并由 JobCollector 有界收集全部后代 | `srv_init/src/driver.rs:285`、`supervision.rs:125` |

## 分层与知识归属

依据 [libraries/README](../user/libraries/README.md)“不按调用者是不是系统服务拆出重复知识库”。

| 层 | owner | 内容 |
|---|---|---|
| 出生输入 | `libprocess::birth` | payload 帧中的命名输入表与设定段；进程领域，task 与测试进程也可用 |
| 实例契约 | `libservice::instance` | 保留输入、Export/Ready/Stopped 消息、撤销协议；子进程侧 helper；监督方 launcher（libexecution 任务，init、`srv_service`、验收控制 task 共用） |
| 组态 schema | `shared/formation` | 二进制编解码、静态校验；`host` feature 下为 TOML 编译器 |
| 服务设定 | `shared/settings` | 整数、布尔、字节串、列表、表的编码与只读访问；不认识任何服务键 |
| 领域语义 | libfs、libdevice、libdriver 等 | 导出名常量、设定键及含义；导出能力的业务协议不变 |
| 执行器 | srv_init | 组态执行、根监督、系统终局；只依赖 libkernel、libprocess、libservice、libexecution、libbudget、librpc、erhino_shared、tar、elf、formation |

`libservice::instance` 只依赖 libprocess、libexecution、librpc、libkernel，不依赖 OAL 与 registry；不注册目录的服务只用该模块。libdevice 依赖 `libservice::instance` 的撤销协议并在 SourceAdmin 端实现；libservice 不认识设备。

## 契约一：出生

- payload 帧：magic、version、输入表（名字 → 槽号）、设定段（`settings` 字节）。名字为 ASCII，上限 32 字节，表内唯一。对象类型与权限以 `adopt` 时内核 query 的实际结果为准，表中不记录。
- `Birth::adopt(StartupContext)` 提供：`take(name, kind, min_rights)`；`take_all(prefix)` 取多值输入（如 `platform.root[0..n]`）；`settings()`；`finish()`。`finish()` 拒绝未取用的输入，使组态与实现不一致在出生时失败。
- `report`：MailboxSender，WRITE|WAIT|TRANSIT。`stop`：NotificationOwner，READ|WAIT。其余输入（Pool CREATE 别名、Job、映像、平台根）均为普通命名输入。
- 不需要生命周期的 task（如 `test_fp`）可空 payload、不使用 `Birth`。
- 不采用 ELF note 携带契约：输入/导出不一致在出生与就绪时被捕获，note 需要额外构建步骤并依赖 strip 保留，收益仅是提前到构建期发现。

## 契约二：就绪与导出

- 子进程在 `report` 上依次发若干 `Export { name }`（每条恰一个句柄），最后发 `Ready { export_count, summary }`。一导出一能力，例如 device 的根入口与管理端是 `root` 与 `root.admin`。`summary` 为可选领域字节，init 不解释、可转交消费者。
- init 通用校验：sender pid 与 `report` 通道 context；导出名属于组态为该实例声明的集合且不重复；实际 kind 与 rights 不低于声明；Ready 时计数相符、声明导出全部到齐、进程仍 Running 且未 DONE。
- 删除 KOID/incarnation 交叉核对与位掩码（仅 init 消费，见已核实事实）。
- 就绪等待并入监督轮询，每轮有界领取；init 持续排空 `report`（容量 16）。子进程按启动期限 `try_send`，超期即启动失败。

## 契约三：停止

- init 对 `stop` 发 signal；`stop` 关闭（init 消失）同样视为停止请求。服务自行收尾，发 `Stopped`，以 Completed 退出。init 要求 Stopped 与正常 DONE 两项事实，期限取组态 `stop_timeout`，超期进系统终局。
- srv_fs：现有停止通知与 `PROVIDER_STOPPED` 改用统一消息。
- srv_driver：删除 Stop RPC op1，改为响应 `stop`；在 Binding Released、worker 收集之后发 Stopped。`test_device` 的 op1 权限负探针随之删除。
- srv_device：收到 `stop` 后封自身根来源、等根来源退休、发 Stopped。原由 init 执行的封源、Watch 终止确认与 `QuerySourceStop` 轮询移入服务内部。

## 契约四：可撤销导出

- 组态可在一条路由上声明 `revoke = "<提供方导出名>"`。该导出必须是实现撤销协议的管理端，只有两个操作：`Revoke`（幂等）与 `QueryRevoked → Open | Revoking | Closed`。
- 语义：`Closed` 表示提供方账本中经该来源派生的活跃使用为空。“活跃使用”及其与硬件静止的关系由领域拥有。libdevice 的实现即现有 SealSource 与 QuerySourceStop；依已核实事实，正常停止路径下 Closed 蕴含已暴露资源均已由执行者报告静止，可作为放行门。
- init 的两处使用：
  - 正常停止：消费者 Stopped + DONE 之后撤销其收到的可撤销路由，轮询至 Closed，再停止提供方。Revoking 超过 `stop_timeout` 进系统终局。
  - 系统终局：对已收到的全部撤销管理端发 `Revoke`，替代 `Terminal::run` 中的 libdevice `SealSource`。终局不等待 Closed，仍按现行封 `services`、有界收集、冷重启。
- 签发方式：需要给其他实例的受限来源，由提供方按自身设定在 Ready 前签发为预限定命名导出。driver 的 ClaimSource 由 srv_device 按设定签发为 `claim.driver` 与 `claim.driver.admin`（scope 与 access 写在 device 设定）。init 只路由，不再运行期调用 `IssueSource`；原“签发 Unknown 时封原根”分支随之删除，签发失败即 device 未就绪，走启动失败终局。

## 组态文件

### 实例字段

| 字段 | 含义 |
|---|---|
| `name` | 实例名，唯一 |
| `image` | initfs 路径 |
| `kind` | `resident` 或 `task` |
| `job` | 新建（可指定父 Job）或使用具名 Job |
| `pool` | 映像页由 ELF load runs 推导；组态声明工作集页与 metadata 字节，可附额外委托额度 |
| `inputs` | 输入名 ← 来源；可声明 rights 衰减与 `revoke` |
| `exports` | 名字、kind、最小 rights |
| `settings` | 内联表，编译时编码进组态文件 |
| `start_timeout`、`stop_timeout` | 期限 |
| `after` | 无数据依赖时的顺序约束 |

### 来源种类

`init:platform.description`；`init:platform.root[*]`；`initfs:<path>`（封存为只读 MemoryObject）；`job:<name>`、`pool:<name>`（init 按组态建立的具名对象及 rights）；`self.pool`（本实例 Pool 的 CREATE 别名）；`<instance>.<export>`。`SystemReset` 不可路由。

### 全局与 initfs 布局

- 全局 `finish`：全部 task 正常完成后的动作。task 失败与 resident 意外退出一律进系统终局。
- initfs 只含 `formation` 与 `image/*`。服务设定内联进 `formation`，不另设设定文件目录。

### 静态校验（任何副作用之前）

路径存在且唯一；实例名、输入名唯一；引用存在；引用的导出已声明；rights 只衰减不提升；依赖无环；预算加和不溢出且不超过根 Pool；实例与路由数不超过实现上限。编译器与 init 调用同一校验函数。

### 执行

- 按依赖拓扑序启动，组态顺序作为确定性 tie-break。resident 的导出在其 Ready 之后才可路由给后继。
- 停止按反拓扑序，每步为“消费者 stop → Stopped + DONE → 撤销其可撤销路由至 Closed → 下一个”，替代 `deployment.rs` 固定的 Consumers/Files/Devices/Root 阶段。停止总预算为各实例 `stop_timeout` 之和加根预算。
- Supervisor 运行体、记录与来源数由组态在执行前计算并一次准入，替代 `Instance` 枚举、`deployment::instances()` 与 `file_hosts()`。
- 组态解码结果与 initfs 映像在 init 生命周期内持有。

## 领域迁移

- **srv_fs**：删除 `Profile`。设定给出入口列表（每入口 watches/sessions 份额）与是否承载 registry，`Capacity` 由设定计算。管理与登记 receiver 改由 fs 自建（未找到须由 init 预建的理由）。四种根改为命名导出：`file.root[i]`、`directory[i]`、`management[i]`、`registry.authority[i]`。secondary 的 publication authority 改为命名出生输入，删除 Start 前投递与 parent pid 核对。
- **srv_driver**：声明表进入设定（key、identity、priority、capacity、reusable、child_registrar、trusted_physical），映像为命名输入 `image.<key>`，`image_bytes` 由 sealed 对象有效长度得出。worker Job 为输入 `job.workers`。worker Pool 额度写在组态，由部署决定；srv_driver 启动时校验额度覆盖声明，不足即启动失败。init 不再识别 ECAM。
- **srv_device**：平台描述与根为命名输入；导出 `root.admin` 与设定声明的预限定来源。init 不再持有 root 入口、observer 与 Watch。
- **init 平台输入**：平台描述头经 `erhino_shared::platform` 解析，按 `resource_count` 与 `handle_index` 取根。终局按实际 object kind 封 MMIO/IRQ/DMA 根，只需 libkernel。删除 libpci 依赖。

## 验收迁移

- 内核级检查（IPC、内存映射、时间、Job/Pool、tunnel、Once、race、MMIO/IRQ/DMA 根、Building/Job 管理）移入新 task `test_kernel`，接收派生 Pool/Job、所需映像与平台根别名，由它启动 `test_target`、`test_hammer` 与 `srv_pm`。断言 sender PID 等于 `init_pid` 的检查改为对照自身 pid。平台根别名只出现在验收组态，用 `after` 保证与 srv_device 使用串行。
- 服务级检查由已有 `test_device`、`test_fs` 承接：device/driver 自检（权限拒绝、Watch、枚举、OAL）移入 `test_device`；fs/registry/copy/secondary 生命周期移入 `test_fs`，secondary 由 `test_fs` 用 `libservice::instance` launcher 启动。正式装配路径不再包含探针。
- 留在 init 的是自身不变量：startup capture、idle ownership、dispatcher funding、终局与 reset。reset 负路径检查删除：attenuated rights 拒绝属于内核契约，由 `test_kernel` 持衰减别名验证。
- 删除全部 init feature；Justfile 的 workload 只选择 `user/formations/<部署名>.toml` 与映像集合。

## 实施顺序

### 切面 A：内核级验收移出（独立先行）

1. 新建 `test_kernel`，搬入内核级检查及其辅助（`once_checks`、`time_checks`、`public_ipc`、`mmio/irq/dma_checks`、`race`、`building` 与 `main.rs` 中对应函数）。
2. 现有 init 以现有 spawn 启动 `test_kernel`，按 feature 传模式字节。
3. 验证：`just check`、`just clippy`，现有 core/stress/memory/device 路线结果与切面前一致。

该传模式路径是登记的债务，见[债务](#债务)。

### 切面 B：统一契约与组态（一次耦合迁移）

按依赖自底向上，中间步骤不要求 QEMU 通过：

1. `shared/settings`、`shared/formation`（含编译器），host 单测覆盖编解码与全部静态校验负路径。
2. `libprocess::birth`，host 单测覆盖帧解码、重名、`finish()` 拒绝未取用输入。
3. `libservice::instance`：消息、撤销协议、子进程 helper、launcher；host 单测覆盖导出校验与撤销状态机。
4. srv_fs、srv_device（含 SourceAdmin 撤销协议与预限定导出）、srv_driver 迁移。
5. srv_init 执行器替换 `deployment.rs`、`device.rs`、`driver.rs`、`filesystem/`；平台输入去 libpci；Terminal 改用撤销协议。
6. `test_device`、`test_fs` 承接服务级验收；`test_kernel` 改由组态启动并从设定取模式。
7. `user/formations/` 写出 system 与各验收部署；Justfile 改为编译组态、按部署组装 initfs，`bin/` 改为 `image/`。

开发证据：步骤 1–3 的 host 单测；步骤 5 后 System 路线。

### 收口

`just check`、`just clippy`、受影响 crate 的 host 测试与全部验收路线（路线选择见 [BUILD-AND-TEST](BUILD-AND-TEST.md)）。按 [REVIEW](REVIEW.md) 做交付前结构检查并写回本篇。回填 `notes/impls`（服务、init、device 相关篇），更新 KNOWN_ISSUES 矫正区与 COMPASS。

## 删除清单（切面 B 完成时）

- srv_init：`deployment.rs` 的 `instances`/`file_hosts`/`system` 与固定停止阶段；`supervision::Instance` 及 `Record` 的 `filesystem`、`device_closed`、`admin` 特化字段；`device.rs`、`driver.rs`、`filesystem/hosting.rs`、`filesystem/launch.rs`；服务级验收代码；全部 feature；对 `srv_fs`、`test_fs`、`test_device`、`libdevice`、`libdriver`、`libpci`、`libusb` 的依赖。
- srv_fs：`startup::Profile`、`PROVIDER_*`/四种根 kind 的旧报告。
- libdevice：`startup` 槽位常量与 `Delegation`。
- libdriver：`boot` 槽位常量、`ManagerReady`、`Declaration` 的 payload 编码（声明类型可保留为设定解析结果）。
- srv_driver：Stop RPC op1；`test_device` 中对应负探针。
- Justfile：按 feature 与 workload 选择 initfs 条目的分支。

## 债务

| 项 | 现状 | 目标 | 触发与删除条件 |
|---|---|---|---|
| `test_kernel` 按 feature 传模式 | 切面 A 后 init 仍按 feature 编码模式字节 | 模式写在验收组态的 `settings` | 切面 B 步骤 6 接通时删除 init 侧编码与对应 feature |

## 完成门

1. 同一 init ELF（比对哈希）分别在 System 与各验收 initfs 下运行。
2. System：三宿主、按需驱动、常驻监督成立；正常 shutdown 按反拓扑序停止，经撤销门放行 device 停止。
3. 两个 srv_fs 实例由组态定义，取代 `Secondary` 枚举。
4. 组态负路径（host 单测）：缺失路径、重名、未声明导出、依赖环、超预算、rights 提升，均在副作用前拒绝。
5. 运行期负路径：导出缺失或超时、Ready 前退出、Stopped 缺失、Revoke 超时，均进系统终局且不遗忘已启动实例。
6. 删除清单全部完成；srv_init 的 Cargo 依赖只剩执行器所列。
7. 交付前结构检查写回本篇，impls 回填。

## 当前接力

设计已定稿，尚未开始施工。实施者从切面 A 开始；进入切面 B 前通读本篇与[服务装配计划](todo-2026-10-07-service-assembly.md)的常驻基座设计，理解现有 Start 前监督、Running 门、DONE 判定与终局责任链，这些机制保留，只把领域特化换成通用契约。施工中发现本篇前提不成立时，先在本篇记录并修订设计，再继续。
