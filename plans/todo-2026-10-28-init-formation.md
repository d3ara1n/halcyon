# init 组态与统一服务契约

状态：**切面 A（内核级验收移出 init）已完成，提交前整体整理后在冻结点 A-F4 全部路线重跑通过；retry-race 为既有观察器缺陷，已移交设备计划，不计通过；切面 B 未开始**。证据与最终表见[当前接力](#当前接力)。本篇是本任务唯一工作记录，拥有目标设计、实施顺序、验证门、债务与接力。上游职责边界（init 根监督、`srv_service` 分工）由[服务装配计划](todo-2026-10-07-service-assembly.md#已确认的-init-与服务管理职责)拥有；系统停止政策由[关机编排](todo-2026-09-system-shutdown-orchestration.md)拥有；设备安全事实由[设备计划](todo-2026-09-device-system-design.md)拥有。

## 目标与非目标

目标：init 编译为一个固定二进制，由 initfs 中的**组态文件**（formation）、映像与服务设定选择系统初始组态；System 与各验收部署只通过更换 initfs 区分。

前提：统一服务出生、就绪、停止与撤销契约，使 init 不链接、不解码任何领域协议。

已确认的决定：

- 组态文件是数据（实例、映像、资源、能力路由、生命周期种类），不是脚本。init 不执行消息编解码、条件或循环，不解释领域协议。
- 系统服务与用户服务共用同一契约，不按 `srv_` 前缀或启动者拆库，不各自实现握手。差别只在监督方政策（init 下失败进系统终局，`srv_service` 下按其恢复政策）和描述丰富度。
- 命名采用本项目自己的领域词，不沿用其他系统的惯例目录或术语（如 `etc`、`manifest`、`rc`）。

非目标：`srv_service` 的实现、用户服务描述的格式、服务恢复与重启、持久化。本任务只保证契约与组态 schema 核心可被 `srv_service` 复用。

## 命名

本节为定稿，实施中不再另议；新增名字按本节语法派生。

### 术语

| 中文 | 代码 | 含义 |
|---|---|---|
| 组态 | `formation` | 一份完整的初始系统组成：实例、资源、路由与结束动作 |
| 部署 | 组态源文件名 | System 或某一验收形态；一个部署对应一份组态 |
| 实例 | `instance` | 组态中的一个进程；同一映像可有多个实例 |
| 种类 | `resident` / `task` | 常驻（须 Ready、须正常停止）/ 任务（以 Completed 结束即成功） |
| 出生 | `birth` | 进程启动时收到的命名输入与设定 |
| 输入 / 导出 | `input` / `export` | 实例收到的命名能力 / 实例交出的命名能力 |
| 路由 | `route` | 把来源接到某实例的某输入 |
| 撤销 | `revoke` | 停止或终局时收回某条路由派生的全部使用 |
| 设定 | `settings` | 实例私有的类型化配置值，由服务自己解释 |

### 名字语法

- 实例名、输入名、导出名、job 名统一语法：小写 ASCII 段 `[a-z][a-z0-9_]*`，段间用 `.`，总长不超过 32 字节。
- 首段是名字语义的所属领域：`instance.*` 属 `libservice::instance`；`platform.*` 属 `erhino_shared::platform`；`memory.*` 属内核内存对象；`device.*` 属 libdevice；`driver.*` 属 libdriver；`file.*` 属 libfs；`service.*` 属 libservice 目录。新领域按其库名取首段。
- 列表：末段为十进制序号，从 0 连续无空洞，如 `platform.root.0`。`Birth::take_all("platform.root")` 按序取全部；组态中把列表前缀接到列表前缀即整表路由，序号保持。
- 撤销管理端的名字为被管导出名加 `.admin`，如 `device.source.driver.admin`。
- 组态内引用来源写作 `<实例名>/<导出名>`。保留实例名 `init`（init 内建来源）与 `self`（引用方实例自身），不得作为实例名。
- 组态源中的种类、对象类型与权限取值用 snake_case：`mailbox_sender`、`notification_owner`、`memory_object`、`job`；权限 `read`、`write`、`wait`、`signal`、`manage`、`create`、`duplicate`、`transit`、`grant`。

### 文件、组件与构建

| 对象 | 名称 |
|---|---|
| initfs 组态文件 | 根目录 `formation`（二进制，魔数 `ERFORMTN` + u32 版本 1） |
| initfs 映像 | `image/<组件名>`，由现有 `bin/` 改名，与 `libprocess::image`、`StartupImage` 用词一致 |
| 出生帧 | payload 魔数 `ERBIRTH\0` + u32 版本 1 |
| 组态 crate | `shared/formation`：no_std 库；`host` feature 下同名 bin `formation` 为编译器 |
| 设定 crate | `shared/settings` |
| 出生模块 | `libprocess::birth`，类型 `Birth` |
| 实例契约模块 | `libservice::instance`（不复用 libservice 现有目录“服务描述”类型） |
| 内核级验收 task | `test_kernel` |
| 组态源 | `user/formations/<部署名>.toml` |
| 部署名 | 沿用现有 workload：`system`、`core`、`stress`、`memory`、`device`、`device-reuse`；原 feature 覆写各自成为部署：`device-retry-race`、`device-candidates`、`device-function-session`、`irq-retirement`、`irq-exit`、`mmio-publication`、`mmio-forced`、`dma-races`、`dma-shared`、`dma-quota`、`dma-install-failure`。部署名不是 Rust 组件，保留连字符 |
| Justfile | 变量 `FORMATION`（环境 `ERHINO_FORMATION`，默认 `core`）取代 `ERHINO_ACCEPTANCE_WORKLOAD` 与 `INIT_FEATURES`；`make_formation` 编译组态，`make_initfs` 按组态引用的映像组装 |

新增名字均为完整英文领域词，符合 [user/README](../user/README.md) 命名章节，无需登记缩写。源格式用 TOML：编译器只在 host 运行，需要注释、表与数组，Rust 生态有成熟解析器；init 内没有文本解析器。依赖版本按全局规则固定。

### 保留名与内建来源

| 类别 | 名字 |
|---|---|
| 生命周期输入 | `instance.report`（mailbox_sender，write\|wait\|transit）、`instance.stop`（notification_owner，read\|wait） |
| 报告消息 | `Export`、`Ready`、`Stopped` |
| 撤销协议 | `Revoke`、`QueryRevoked` → `Open` \| `Revoking` \| `Closed` |
| init 内建来源 | `init/platform.description`、`init/platform.root`（列表）、`init/image.<组件名>`（只读 MemoryObject）、`init/job.services`、`init/job.<名>`（组态 `[job.<名>]` 声明）、`init/pool.<名>`（组态 `[pool.<名>]` 声明，CREATE 别名） |
| 自身来源 | `self/memory.pool`（本实例 Pool 的 CREATE 别名） |
| 全局结束动作 | `finish = "stay" \| "shutdown" \| "reboot"` |

`SystemReset` 不是来源，不可路由。

### 领域输入与导出名

| 宿主 | 输入 | 导出 |
|---|---|---|
| srv_device | `instance.*`、`memory.pool`、`platform.description`、`platform.root` | `device.root`、`device.root.admin`；设定 `sources` 表每项 `<名>` 产生 `device.source.<名>` 与 `device.source.<名>.admin` |
| srv_driver | `instance.*`、`memory.pool`、`device.source`、`driver.workers`（worker Job）、`driver.image.<组件名>` | `driver.control`、`driver.observer` |
| srv_fs | `instance.*`、`memory.pool`；不承载 registry 时加 `service.authority` | 每入口 N：`file.root.N`、`file.management.N`；承载 registry 时加 `service.directory.N`、`service.authority.N` |

驱动实现键即映像组件名：删除声明中独立的 `key` 字段，设定条目以 `implementation = "drv_usb_hid"` 指定实现，srv_driver 据此取输入 `driver.image.drv_usb_hid`。实现键、映像路径与输入名由同一个组件名派生，不再有三份名字。

### System 组态源示例

数值按现有常量换算，示例只定结构与名字：

```toml
finish = "stay"

[job.drivers]
parent = "services"
pool = "driver"            # 由 driver 实例的 Pool 付款

[instance.device]
image = "srv_device"
kind = "resident"
pool = { work_pages = 0, metadata_bytes = 0 }   # 按现值换算
start_timeout_ms = 5000
stop_timeout_ms = 40000
inputs."memory.pool" = "self/memory.pool"
inputs."platform.description" = "init/platform.description"
inputs."platform.root" = "init/platform.root"
exports."device.root" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."device.root.admin" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."device.source.driver" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."device.source.driver.admin" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
settings.sources = [{ name = "driver", scope = "root", access = ["observe", "claim", "child_register", "report_stopped"] }]

[instance.driver]
image = "srv_driver"
kind = "resident"
pool = { work_pages = 0, metadata_bytes = 0 }   # 含 worker 工作集
stop_timeout_ms = 40000
inputs."memory.pool" = "self/memory.pool"
inputs."device.source" = { from = "device/device.source.driver", revoke = "device/device.source.driver.admin" }
inputs."driver.workers" = "init/job.drivers"
inputs."driver.image.drv_pci_generic" = "init/image.drv_pci_generic"
inputs."driver.image.drv_usb_xhci" = "init/image.drv_usb_xhci"
inputs."driver.image.drv_usb_hid" = "init/image.drv_usb_hid"
exports."driver.control" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
exports."driver.observer" = { kind = "mailbox_sender", rights = ["write", "wait", "duplicate", "transit", "grant"] }
settings.declarations = [
  { implementation = "drv_pci_generic", identity = "<PCI host identity 字节>", priority = 1, capacity = 1, reusable = false, child_registrar = true, trusted_physical = true },
  { implementation = "drv_usb_xhci", identity = "<xHCI identity 字节>", priority = 1, capacity = 1, reusable = false, child_registrar = true, trusted_physical = true },
  { implementation = "drv_usb_hid", identity = "<boot keyboard identity 字节>", priority = 1, capacity = 2, reusable = true, child_registrar = false, trusted_physical = false },
]

[instance.fs]
image = "srv_fs"
kind = "resident"
pool = { work_pages = 4096, metadata_bytes = 1048576 }
start_timeout_ms = 10000
stop_timeout_ms = 10000
inputs."memory.pool" = "self/memory.pool"
exports."file.root.0" = { kind = "mailbox_sender", rights = ["write", "wait", "transit", "duplicate", "grant"] }
# file.management.0、service.directory.0、service.authority.0 同上
settings.registry = true
settings.entries = [{ watches = 24, sessions = 16 }]
```

`instance.report`、`instance.stop` 由 init 为每个 resident 自动供给，不写进组态。identity 字节的书写形式（十六进制字符串）由 `formation` 编译器的设定编码规则定义；领域常量（如 `libpci::host::HOST_IDENTITY`）在 host 单测中与 System 组态逐字节比对，不在两处各写一份真值后不核对。

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
| Pool 委托寿命 | 派生 Pool 的整份委托只在其全部计费退休后才回到父 Pool。`MemoryObject::create`、`DmaDomainControl::create_control/create_use` 隐式向调用进程绑定的 Pool 计费，无显式付款参数。故意跨 reset 保留的 DMA 映射会使创建者所在 Pool 的整份委托一直保活 | `libkernel/src/memory_object.rs:37`、`os/kernel/src/resource/dma/object.rs:195,304`；切面 A 开发 core 日志 `artifacts/failed-acceptance-20261029-082357-44659.log` |

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

- payload 帧：魔数 `ERBIRTH\0`、version、输入表（名字 → 槽号）、设定段（`settings` 字节）。名字按[名字语法](#名字语法)，表内唯一。对象类型与权限以 `adopt` 时内核 query 的实际结果为准，表中不记录。
- `Birth::adopt(StartupContext)` 提供：`take(name, kind, min_rights)`；`take_all(prefix)` 取列表输入（如 `platform.root`）；`settings()`；`finish()`。`finish()` 拒绝未取用的输入，使组态与实现不一致在出生时失败。
- `instance.report` 与 `instance.stop` 的类型与权限见[保留名](#保留名与内建来源)。其余输入（`memory.pool`、Job、映像、平台根）均为普通命名输入。
- 不需要生命周期的 task（如 `test_fp`）可空 payload、不使用 `Birth`。
- 不采用 ELF note 携带契约：输入/导出不一致在出生与就绪时被捕获，note 需要额外构建步骤并依赖 strip 保留，收益仅是提前到构建期发现。

## 契约二：就绪与导出

- 子进程在 `instance.report` 上依次发若干 `Export { name }`（每条恰一个句柄），最后发 `Ready { export_count, summary }`。一导出一能力，例如 device 的根入口与管理端是 `device.root` 与 `device.root.admin`。`summary` 为可选领域字节，init 不解释、可转交消费者。
- init 通用校验：sender pid 与 `instance.report` 通道 context；导出名属于组态为该实例声明的集合且不重复；实际 kind 与 rights 不低于声明；Ready 时计数相符、声明导出全部到齐、进程仍 Running 且未 DONE。
- 删除 KOID/incarnation 交叉核对与位掩码（仅 init 消费，见已核实事实）。
- 就绪等待并入监督轮询，每轮有界领取；init 持续排空 `instance.report`（容量 16）。子进程按启动期限 `try_send`，超期即启动失败。

## 契约三：停止

- init 对 `instance.stop` 发 signal；`instance.stop` 关闭（init 消失）同样视为停止请求。服务自行收尾，发 `Stopped`，以 Completed 退出。init 要求 Stopped 与正常 DONE 两项事实，期限取组态 `stop_timeout_ms`，超期进系统终局。
- srv_fs：现有停止通知与 `PROVIDER_STOPPED` 改用统一消息。
- srv_driver：删除 Stop RPC op1，改为响应 `instance.stop`；在 Binding Released、worker 收集之后发 Stopped。`test_device` 的 op1 权限负探针随之删除。
- srv_device：收到 `instance.stop` 后封自身根来源、等根来源退休、发 Stopped。原由 init 执行的封源、Watch 终止确认与 `QuerySourceStop` 轮询移入服务内部。

## 契约四：可撤销导出

- 组态可在一条路由上声明 `revoke = "<实例名>/<导出名>.admin"`。该导出必须是实现撤销协议的管理端，只有两个操作：`Revoke`（幂等）与 `QueryRevoked → Open | Revoking | Closed`。
- 语义：`Closed` 表示提供方账本中经该来源派生的活跃使用为空。“活跃使用”及其与硬件静止的关系由领域拥有。libdevice 的实现即现有 SealSource 与 QuerySourceStop；依已核实事实，正常停止路径下 Closed 蕴含已暴露资源均已由执行者报告静止，可作为放行门。
- init 的两处使用：
  - 正常停止：消费者 Stopped + DONE 之后撤销其收到的可撤销路由，轮询至 Closed，再停止提供方。Revoking 超过提供方 `stop_timeout_ms` 进系统终局。
  - 系统终局：对已收到的全部撤销管理端发 `Revoke`，替代 `Terminal::run` 中的 libdevice `SealSource`。终局不等待 Closed，仍按现行封 `services`、有界收集、冷重启。
- 签发方式：需要给其他实例的受限来源，由提供方按自身设定在 Ready 前签发为预限定命名导出。srv_device 按设定 `sources` 表签发 `device.source.<名>` 与 `device.source.<名>.admin`（scope 与 access 写在该表项）；System 中 driver 使用 `device.source.driver`。init 只路由，不再运行期调用 `IssueSource`；原“签发 Unknown 时封原根”分支随之删除，签发失败即 device 未就绪，走启动失败终局。

## 组态文件

### 字段

| 位置 | 字段 | 含义 |
|---|---|---|
| 顶层 | `finish` | 全部 task 正常完成后的动作 |
| `[job.<名>]` | `parent`、`pool` | 父 Job（`services` 或另一具名 job）；付款 Pool 所属实例 |
| `[pool.<名>]` | `work_pages`、`metadata_bytes` | init 从根 Pool 派生并持有至系统结束的具名 Pool，不随任何实例收集而关闭；用于寿命超出单个实例的计费（如故意跨 reset 保留的计费）。额度取其承载进程的部署工作集：能从公共契约推导的按推导，否则沿用项目已有的单进程部署工作集常量并在债务登记，不另拟数值 |
| `[instance.<名>]` | `image` | initfs 内 `image/<组件名>` 的组件名 |
| | `kind` | `resident` 或 `task` |
| | `job` | 可选，具名 job；缺省时 init 在 `services` 下为该实例新建 Job |
| | `pool` | `{ work_pages, metadata_bytes }`；映像页由 ELF load runs 推导后另加 |
| | `inputs."<输入名>"` | 来源字符串，或 `{ from, rights, revoke }`（`rights` 只衰减） |
| | `exports."<导出名>"` | `{ kind, rights }`，为最小要求 |
| | `settings.*` | 内联设定，编译时编码进 `formation` |
| | `start_timeout_ms`、`stop_timeout_ms` | 期限，单位毫秒 |
| | `after` | 无数据依赖时的顺序约束，列出实例名 |

来源只有[保留名与内建来源](#保留名与内建来源)所列的 `init/*`、`self/*` 与 `<实例名>/<导出名>` 三种。

### 结束与 initfs 布局

- 全部 task 正常完成后执行 `finish`。task 失败与 resident 意外退出一律进系统终局。
- initfs 只含 `formation` 与 `image/*`。设定内联进 `formation`，不另设设定文件目录。

### 静态校验（任何副作用之前）

路径存在且唯一；实例名、输入名唯一；引用存在；引用的导出已声明；rights 只衰减不提升；依赖无环；预算加和不溢出且不超过根 Pool；实例与路由数不超过实现上限。编译器与 init 调用同一校验函数。

### 执行

- 按依赖拓扑序启动，组态顺序作为确定性 tie-break。resident 的导出在其 Ready 之后才可路由给后继。
- 停止按反拓扑序，每步为“消费者 stop → Stopped + DONE → 撤销其可撤销路由至 Closed → 下一个”，替代 `deployment.rs` 固定的 Consumers/Files/Devices/Root 阶段。停止总预算为各实例 `stop_timeout_ms` 之和加根预算。
- Supervisor 运行体、记录与来源数由组态在执行前计算并一次准入，替代 `Instance` 枚举、`deployment::instances()` 与 `file_hosts()`。
- 组态解码结果与 initfs 映像在 init 生命周期内持有。

## 领域迁移

- **srv_fs**：删除 `Profile`。设定 `registry`（布尔）与 `entries`（每入口 watches/sessions 份额）决定形态，`Capacity` 由设定计算。管理与登记 receiver 改由 fs 自建（未找到须由 init 预建的理由）。四种根改为[领域导出名](#领域输入与导出名)所列命名导出。secondary（`registry = false`）的 publication authority 改为输入 `service.authority`，路由自 primary 的 `service.authority.N`，删除 Start 前投递与 parent pid 核对。
- **srv_driver**：声明表进入设定 `declarations`（implementation、identity、priority、capacity、reusable、child_registrar、trusted_physical），映像为输入 `driver.image.<组件名>`，`image_bytes` 由 sealed 对象有效长度得出。worker Job 为输入 `driver.workers`。worker 工作集计入 driver 实例 `pool.work_pages`，由部署决定；srv_driver 启动时校验额度覆盖声明，不足即启动失败。init 不再识别 ECAM。
- **srv_device**：输入 `platform.description` 与列表 `platform.root`；导出 `device.root`、`device.root.admin` 与设定 `sources` 声明的预限定来源。init 不再持有 root 入口、observer 与 Watch。
- **init 平台输入**：平台描述头经 `erhino_shared::platform` 解析，按 `resource_count` 与 `handle_index` 取根。终局按实际 object kind 封 MMIO/IRQ/DMA 根，只需 libkernel。删除 libpci 依赖。

## 验收迁移

- 内核级检查（IPC、内存映射、时间、Job/Pool、tunnel、Once、race、MMIO/IRQ/DMA 根、Building/Job 管理）移入新 task `test_kernel`，接收派生 Pool/Job、所需映像与平台根别名，由它启动 `test_target`、`test_hammer` 与 `srv_pm`。断言 sender PID 等于 `init_pid` 的检查改为对照自身 pid。平台根别名只出现在验收组态，用 `after` 保证与 srv_device 使用串行。
- 寿命到 reset 的计费不得进入可回收 Pool。`test_kernel` 另收输入 `memory.pool.retention`（路由自 `init/pool.retention`）。DMA `process_retirement` 中故意跨 reset 保留的模式（exposed、output-fault）由 `test_kernel` 以保留模式自启动的子进程执行：子进程出生时绑定 retention Pool，原 fixture 的 scope/use/MemoryObject 创建、目标启动与核验整体在子进程内运行，判据不变。验收断言：该子进程收集后，`test_kernel` 主 Pool 在其自身收集后双维归零；retention Pool 的残留恰为被保留映射的计费。其余 fixture 若存在跨收集保留的计费，按同一规则归入保留域。
- 服务级检查由已有 `test_device`、`test_fs` 承接：device/driver 自检（权限拒绝、Watch、枚举、OAL）移入 `test_device`；fs/registry/copy/secondary 生命周期移入 `test_fs`，secondary 由 `test_fs` 用 `libservice::instance` launcher 启动。正式装配路径不再包含探针。
- 留在 init 的是自身不变量：startup capture、idle ownership、dispatcher funding、终局与 reset。reset 负路径检查删除：attenuated rights 拒绝属于内核契约，由 `test_kernel` 持衰减别名验证。
- 删除全部 init feature；Justfile 由 `FORMATION` 选择 `user/formations/<部署名>.toml`，initfs 映像集合由该组态引用的 `image` 与 `init/image.*` 决定，不再手列。

## 实施顺序

### 切面 A：内核级验收移出（独立先行）

1. 新建 `test_kernel`，搬入内核级检查：按领域分为 `ipc_checks`、`ipc_thread_checks`、`memory_checks`、`job_checks`、`tunnel_checks`、`rpc_checks`、`time_checks`、`once_checks`、`race_checks` 与 MMIO/IRQ/DMA 资源检查，测试本地 owner 为 `harness`，出生采用与保留模式子进程在 `startup`。
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
| `test_kernel` 按 feature 传模式 | 切面 A 使用 `test_kernel::birth` 私有出生帧：56 字节，依次为小端 u32 部署模式位、u8 进程角色（主验收进程或两种保留模式子进程，与部署模式不共用位空间）、12 个输入槽号（主 Pool/Job/reset/pm/target/hammer/MMIO/IRQ/DMA/retention Pool/self 映像/私有报告 sender，255 表示缺省）、保留零字节与 pm/target/hammer/self 四个小端 u64 映像长度；`test_kernel::report` 拥有 104 字节 held Pool 快照报告。能力实际种类/权限在采用时核验。init 按 feature 编码模式；`srv_init` 因共享 birth/report 格式而在 Cargo 中反向依赖测试 crate `test_kernel`。retention Pool 是 RootSupervisor 通用 held Pool 集合的一员，以 `memory.pool.retention` 签发给 test_kernel | 输入改用 `libprocess::birth`，模式写在验收组态的 `settings`，系统服务不再依赖测试 crate；held Pool 集合由组态 `[pool.<名>]` 声明 | 切面 B 步骤 6 接通时删除私有帧、init 侧编码、对应 feature 及 `srv_init → test_kernel` Cargo 依赖 |
| 单进程工作集无公共推导 | retention Pool 与 `test_kernel` 主 Pool 的额度沿用 `libdriver::boot::WORKER_POOL`（每进程一份）这一已有部署工作集，未从页表预取峰值、VM planner 缓冲、堆碎片等内部布局闭合精确上界；`test_kernel` 因此依赖 libdriver 的该常量。WORKER_POOL 描述普通进程，不含按线程数扩展的成本：memory 部署的 raw thread storm（`PROCESS_MAX_THREADS` 个线程成员并存，要求 ReachLimit 先于 Quota）改用 stress 的 5 份组合，依据是 memory 在 storm 期间的同时存活集合是 stress 的子集，而 stress 在 5 份下已通过同一 storm；这是组合证据，不是逐线程成本推导 | 内存领域提供单进程工作集的公共推导，含每线程成员的 metadata 与栈成本；fs/device/driver/test_kernel 的同类常量收敛为一个真值 | 触发：出现须精确核定单进程工作集的需求（不可信创建域、紧凑部署或额度不足的实证）。由[内存重构](todo-2026-10-01-memory-redesign.md)或[内核内存预算](todo-2026-09-14-kernel-memory-budget.md)承接；接通后删除对 WORKER_POOL 的借用与 memory 借用 stress 组合 |

## 完成门

1. 同一 init ELF（比对哈希）分别在 System 与各验收 initfs 下运行。
2. System：三宿主、按需驱动、常驻监督成立；正常 shutdown 按反拓扑序停止，经撤销门放行 device 停止。
3. 两个 srv_fs 实例由组态定义，取代 `Secondary` 枚举。
4. 组态负路径（host 单测）：缺失路径、重名、未声明导出、依赖环、超预算、rights 提升，均在副作用前拒绝。
5. 运行期负路径：导出缺失或超时、Ready 前退出、Stopped 缺失、Revoke 超时，均进系统终局且不遗忘已启动实例。
6. 删除清单全部完成；srv_init 的 Cargo 依赖只剩执行器所列。
7. 交付前结构检查写回本篇，impls 回填。

## 当前接力

切面 A（内核级验收移出 init）已完成并经提交前整体整理，在冻结点 A-F4 全部路线重跑，结果见[最终验证](#a-f4-最终验证)；切面 B 未开始，从[切面 B](#切面-b统一契约与组态一次耦合迁移) 接手。retry-race 为既有观察器缺陷，已[移交设备计划](todo-2026-09-device-system-design.md#retry-观察器缺陷2026-10-29)，不计通过。

### 接线与责任

- **init 侧**：`AcceptanceImages` 在任何子进程出生前一次核完验收归档（缺失或重复即拒绝）。[`kernel_acceptance`](../user/services/srv_init/src/kernel_acceptance.rs) 的 `KernelAcceptance::run` 用原 Preparation/spawn 与 root 账本启动 `test_kernel`，并由 init 原短任务路径启动 `test_fp`；两者均须 Dead/Completed/NONE。`finish` 以二者为输入执行 RootSupervisor 失败隔离自检（原 QuotaExceeded 首因、恰好一个健康成员、InvalidPolicy 后同一 Collector 恢复及隔离 Job 收集判据不变），再核主 Pool 页与 metadata 的 allocated/reserved/delegated 全零并关闭，核 held Pool 自报告后逐字不变；此后才准入 srv_device，平台根别名与设备使用由此串行。
- **nofd 降级**：判据集中在 `run_companion`，严格为 test_fp spawn `System(NotSupported)`、grants `Retained` 且 `cleanup_error == None`；此时以单进程 Collector 收集 test_kernel 并输出自检不适用原因。FP 映像仍必需，其余出生错误或非 Completed 均失败。memory 不运行 test_fp 与该自检。
- **test_kernel 侧**：按领域分模块（见[切面 A 第 1 步](#切面-a内核级验收移出独立先行)），`main` 只负责启动子进程与编排顺序，`startup` 负责出生采用与保留模式子进程，[`harness`](../user/tests/test_kernel/src/harness.rs) 持能力与子进程 owner。它只持 services 下独立验收 Job 的子树，不持根 Job；根拓扑遍历、startup capture、idle ownership、Dispatcher funding 与 RootSupervisor 自检留 init。pm 收到 `libprocess::DERIVED_CONTROL_RIGHTS` 的委托域 JobControl。辅助均在测试包内，不新增公共库 API。
- **held Pool**：RootSupervisor 持有通用 held Pool 集合（`hold_pool`），从根 Pool 派生并持有至系统结束，不随实例收集关闭；对应组态的 `[pool.<名>]`。retention Pool 是其唯一当前使用者。
- **保留 fixture**：DMA exposed/output-fault 的父 fixture 与目标全部在 test_kernel 以保留角色自启动的子进程中执行，出生绑定 retention Pool。test_kernel 在最后一个保留子进程收集后核残留页数恰为 `RETAINED_PAGES_PER_FIXTURE` 之和，再经私有 sender 报告 104 字节全快照；init 只持通用判据，不认识 fixture 细节。metadata 残留记实测，不设任意上界。
- **memory 停止**：`deployment::instances()` 对 memory 为空集合，`stop_providers` 只停止声明集合中的 Driver/Device，声明了却缺记录报原错误；实例监督运行体建立过才退休。该声明集合在切面 B 由组态替换，不另登记债务。此修正针对 `bfb4fdd` 起即存在的缺陷。

### 工作集来源与每部署份数

每份为原 [`libdriver::boot::WORKER_POOL`](../user/libraries/libdriver/src/boot.rs) 的 4096 页与 1 MiB metadata；份数按场景同时存活峰值取 max，不全部累加，由 `kernel_acceptance::kernel_pool_budget` / `held_pool_budget` 唯一计算。无公共单进程工作集推导的限制由[债务](#债务)承接。主域并存依据：[`main::launch_children`/`run`](../user/tests/test_kernel/src/main.rs)、[`once_checks::run`](../user/tests/test_kernel/src/once_checks.rs)、[`race_checks::race_matrix`](../user/tests/test_kernel/src/race_checks.rs) 及资源检查的 `process_retirement`；MMIO/IRQ/DMA 目标子域付款在 `mmio_checks.rs:198`、`irq_checks.rs:197`、`dma_checks.rs:292`。

| 部署/recipe 形态 | 主 Pool 份数 | 同时存活场景/依据 | retention |
|---|---:|---|---|
| core / acceptance-virt、virt-nofd | 4 | 普通阶段主+pm+目标为 3；Once 阶段主+pm+独立 2 MiB metadata 域为 4，取 max | 2 份，两项各保留 2 页 |
| device / device-reuse | 4 | 内核检查同 core；设备消费者在主域收集后才准入，不与主测试域共存 | 同 core |
| stress / virt-stress | 5 | 主+pm+两 hammer+1 并发目标；Once 仍 4，取 max | 同 core |
| memory / virt-vm-fault | 5 | 借用 stress 组合：主+1 hammer 是 stress 并存集合的子集，执行同一 raw storm（1023 个 raw 线程并存、第 1025 次准入 ReachLimit，见 test_hammer:737–764、proc.rs:71）；按进程计两份漏掉线程成本 | 无 |
| mmio-publication / mmio-forced | 4 | 同 core；退出专项逐个派生一份 WORKER_POOL 给目标，不与 Once 域并存 | 同 core |
| dma-races / dma-shared / dma-quota | 4 | 同 core；目标逐个派生一份 WORKER_POOL，只有当前目标子域并存 | 同 core |
| dma-install-failure | 4 | 同 core；单一安装失败目标一份 WORKER_POOL，无保留模式 | 无 |
| irq-exit | 4 | 同 core；两个变体顺序执行，各派生一份 WORKER_POOL | 同 core |
| irq-retirement | 4 | 内核检查同 core；真实 function/IRQ 场景留 init 服务阶段 | 同 core |

主 Pool 除 READ/CREATE/DUPLICATE/GRANT 外需 TRANSIT：race 的 ACTION_CREATE 经消息把付款授权交给 hammer。held Pool 别名不含 TRANSIT。工作集不足一律报告为模型错误，不自动扩额。

### Pool 断言迁移口径

| 检查 | 迁移后的基线与判据 |
|---|---|
| test_process_funding | 原本派生独立 16 页/1 MiB Pool，现以测试 Pool 为父；WrongObjectType、缺 CREATE 拒绝、Map 后 allocated>baseline、abandon 后等于 baseline 均保留 |
| test_pool_budget | 双维回滚、无隐式 quota、父关闭、完整退款与活 View 付款链保留；起点 depth 多 1，链短 1，但仍到同一 MAX_DEPTH 并核 ReachLimit |
| memory mapping | 非 stress 对照测试 Pool 的 allocated 前后相等；stress 不查询的原条件保留 |
| ipc_thread_checks committed_kill | 对照测试 Pool 前后页数相等，原条件不变 |
| tunnel close | 非 stress 核关闭后 allocated 下降且退款 >0；stress 不查询 |

没有必须限定 depth 1 根 Pool 才成立的原断言，没有删除退款/拒绝检查。两处 sender PID 自检对照 test_kernel 自身 pid，子进程 SpawnRequest.parent 为其真实直接启动者。

### fixture 收集后寿命核查

| fixture | 主域外仍活计费/依据 |
|---|---|
| DMA exposed / output-fault | 明确保留，位于 retention；Mapping、Backing/MemoryObjectCore、WritePermit、域节点与保留表，合计 4 页 |
| DMA unexposed | 最后 Close 取消、scope DONE、writer 0/Executable，主 Pool 最终双维零 |
| dma-races / shared / quota / install-failure | 每个目标 Pool 在 CPU/Control/父对象关闭后全零；shared 另核父 RAM 精确退款 |
| mmio-publication / forced | 逐目标核范围失效、scope DONE、Control 关闭及独立 Pool 零；MMIO 映射不占 RAM backing |
| irq-exit | 目标 Control 关闭、独立 Pool 零、use/Line 贷款与来源退休后 DONE |
| irq-retirement | function_checks 仍在 init 服务域，不持主测试 Pool 计费 |
| race / memory suite | 目标/hammer 与 Job 退出后收集；主 Pool 双维零作为组合门 |

### A-F4 最终验证

冻结点 A-F4：541 个产品/工具输入（`user/`、`os/`、`shared/`、`tools/`、Justfile、rust-toolchain），SHA-256 `3815d17e48bcf484a2ba2df13ff487a9e546e271e950c012f7aeea1e649bc07a`；每条 recipe 前后核对，全部相等。`just check`、十一分面 `just clippy` 与 rustfmt 检查均返回 0。结果与运行时刻见 `artifacts/init-formation-a/final-F4/results.json`，下列链接为各 recipe 的完整构建/验收日志。

| just recipe | workload / 构建 | srv_init feature | rc | 日志 | 说明 |
|---|---|---|---:|---|---|
| acceptance-virt | core / release | — | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-virt.log) |  |
| virt-stress | stress / debug | acceptance-stress | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-stress.log) |  |
| virt-vm-fault | memory / debug | acceptance-memory → acceptance-stress | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-vm-fault.log) |  |
| acceptance-device | device / release | acceptance-device | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device.log) |  |
| acceptance-device-reuse | device-reuse / debug | acceptance-device-reuse | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-reuse.log) |  |
| acceptance-device-candidates | device-reuse / debug | acceptance-device-candidates | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-candidates.log) |  |
| acceptance-device-function-session | core / debug | acceptance-device-function-session | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-function-session.log) |  |
| acceptance-fs-stop-order | core / debug | — | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-fs-stop-order.log) |  |
| acceptance-mmio-publication | core / debug | acceptance-mmio-publication | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-mmio-publication.log) |  |
| acceptance-mmio-forced | core / debug | acceptance-mmio-forced | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-mmio-forced.log) |  |
| acceptance-dma-races | core / debug | acceptance-dma-races | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-races.log) |  |
| acceptance-dma-shared | core / debug | acceptance-dma-shared | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-shared.log) |  |
| acceptance-dma-quota | core / debug | acceptance-dma-quota | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-quota.log) |  |
| acceptance-dma-install-failure | core / debug | acceptance-dma-install-failure | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-dma-install-failure.log) |  |
| acceptance-irq-exit | core / debug | acceptance-irq-exit | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-irq-exit.log) |  |
| acceptance-irq-retirement | core / debug | acceptance-irq-retirement → acceptance-device-function-session | 0 | [日志](../artifacts/init-formation-a/final-F4/acceptance-irq-retirement.log) |  |
| virt-nofd | core / release，无 D64 域 | — | 0 | [日志](../artifacts/init-formation-a/final-F4/virt-nofd.log) |  |
| system-observe | system / debug | system | 0 | [日志](../artifacts/init-formation-a/final-F4/system-observe.log) |  |
| acceptance-device-retry-race | device-reuse / debug | acceptance-device-retry-race | 124 | [日志](../artifacts/init-formation-a/final-F4/acceptance-device-retry-race.log) | 既有观察器缺陷，已[移交设备计划](todo-2026-09-device-system-design.md#retry-观察器缺陷2026-10-29)，不计通过；同样失败于 retry.log:12 读取 `self`，外层 617 行 `Retry late-query window not established` |

通过路线的判据由 Guest 内硬失败保证：主 Pool 未归零、held Pool 报告后变化或保留页数不符，均使 test_kernel panic 或 init 进系统终局，wrapper 据此判失败；wrapper 在成功时删除原始 Guest 输出，因此 A-F4 不保留逐路线采样值，A-F3 的采样（保留域 4 页/884 槽）保存在 `final-F3/`。stress 16/16 矩阵、memory 1024 成员 raw storm 与真实 RetryFault、nofd 不适用锚点、System 三宿主常驻等待均由各自 wrapper 锚点核验。

### 交付前结构检查

按 [REVIEW](REVIEW.md) 核对最终结构，没有待修 finding：

- **责任/授权**：真实 sender/parent 身份、独立验收 Job、root 账本与终局对失败/取消的接管保持；test_kernel 收集与主 Pool 关闭先于 srv_device 准入；held Pool 由 root 持有至系统结束，未以遗漏退款保活主域。
- **知识归属**：init 不认识 DMA fixture 细节，只持通用判据；fixture 期望页数由 test_kernel 自核。部署模式与保留子进程角色分为两个字段，不共用位空间。pm 委托域权利引用 `libprocess::DERIVED_CONTROL_RIGHTS`，不复制。`SUPERVISOR_RIGHTS`/`JOB_FULL_RIGHTS` 在公共库中没有同用途常量，两侧各自定义；pm 与 test_kernel 的 `WRITABLE_WAKE_*` 是切面前即有的双端约定，srv_pm 不应为此依赖测试 crate，保持现状。
- **收口/简化**：srv_init 的八个旧模块、`kernel_checks`、`LaunchedServices`、`RunFailure`、验收专用 `retention_pool` 字段已删除；test_kernel 去掉 init 时代命名（`Checks`、`root`、`root_pool_allocated`、`kill_and_supervise`、`RunFailure`）与纯转发方法。init 的 ReadWorld/ReadDuty 经核没有只服务已删路径的字段。
- **安全/并发**：新增 unsafe 的采用/移交及只读映像存活依据成立；保留子进程 spawn 前释放 Context 锁；不新增跨进程锁或内核临界区；未改共享 ABI 或硬件协议。
- **工具**：GDB 观察器按 test_kernel 真实 PID 与 `test_kernel::irq_checks::process_retirement` 符号附着；`check-vm-fault` 在 Guest 明确 panic/reboot 后停止；nofd 脚本期望不适用锚点。业务判据未改。

未做硬件或性能/WCET 验收；sifive_u、hetero、Boot/IPI/VM-repair/独立 DMA-retention 等未列路线未运行。私有出生帧与 WORKER_POOL 借用由[债务](#债务)两行承接。

### 施工过程索引

过程证据保留在 `artifacts/init-formation-a/`，均不计交验：

- **付款寿命**：首轮 core 暴露保留 DMA 把主池整份委托保活，test_fs QuotaExceeded（`artifacts/failed-acceptance-20261029-082357-44659.log`）；据此建立 held/retention Pool 与保留子进程，设计修订见[已核实事实](#已核实事实)「Pool 委托寿命」。
- **工作集口径**：撤回逐项证明内存内部布局与单 64 KiB arena 的要求，额度改为 WORKER_POOL 组合；memory 两份在 raw storm 前 QuotaExceeded（`artifacts/vm-fault/20261029T005457.159966Z-78248/fault-qemu.log:352`），改借 stress 五份。
- **memory 停止缺陷**：A-F2 暴露 `driver assembly record missing`（`artifacts/failed-acceptance-20261029-090925-95909.log`），按声明集合修复。
- **工具定稿**：运行中修改 wrapper 导致一次 stress 外层 rc2，此后规定冻结期间不改 tools/Justfile/产品代码；同期平台时钟异常复发按 [KNOWN_ISSUES](../KNOWN_ISSUES.md#历史偶发异常仅观察) 记入原设备记录。
- **A-F3**：整理前的功能冻结点，19 条路线结论与 A-F4 一致，证据在 `final-F3/`；随提交前整理作废，由 A-F4 取代。
- **Retry 归因**：00274bf 干净基线同样在观察器读取无 `self` 的 `commit_retry` 帧时失败，ELF 差异来自 ignored lockfile 重新解析与路径等环境差异；证据在 `retry-attribution/`，已[移交设备计划](todo-2026-09-device-system-design.md#retry-观察器缺陷2026-10-29)。
