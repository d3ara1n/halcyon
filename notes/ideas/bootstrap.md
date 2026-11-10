# Bootstrap 与用户态 launcher

Bootstrap 建立唯一初始进程及其初始授权图，后续系统组成由用户态展开。本篇描述启动包、初始组态、根监督与通用出生/装载的关系；进程管理、服务与应用分别见 [process-management](process-management.md)、[service](service.md) 和 [application](application.md)。

## 两层格式

启动产物分为内核可见的 BootPackage 与初始用户态解释的 payload。本项目使用 initfs 作为 payload：

```text
BootPackage
├── envelope
├── initial ELF
└── opaque payload（本项目为 initfs）
```

BootPackage 是 eRhino 的启动输入，envelope 描述 initial ELF 与 payload 的边界。内核只装载 initial ELF，不把 payload 当文件系统，不解释其中的路径、组态或其他映像。initfs 的组织属于本项目初始用户态，不是内核的服务协议。

具体格式与当前装载实现见 [startup 实现](../impls/startup.md#bootpackage-v1)。

## 唯一 initial process

process-zero 指内核启动的唯一初始进程，不表示 PID 为 0；PID 0 不是合法进程 ID。内核认识初始进程及其存活契约，不认识本项目 init 的二进制名或系统服务名单。进程编号不是权限等级，初始权限由显式交付的 capabilities 表达。

本项目以 init 承担初始用户态。使用者也可以只复用 eRhino 并提供自己的初始用户态，或复用本项目 init、替换组态与基础服务；两者都须满足内核的启动与初始进程存活契约。

初始进程退出属于内核级致命终局，不是可恢复的普通退出；本项目不把“init 退出而其余系统继续”作为部署模式。具体终局实现见 [startup 实现](../impls/startup.md#唯一-init-bootstrap)。

## initfs 作为 StartupBlock payload

initfs 随初始进程的 StartupBlock 交付。StartupBlock 是启动者与接收进程之间的出生约定，承载初始能力与 opaque payload；内核交付 payload 不等于解释它。

本项目 init 从 initfs 取得 Formation 和初始映像。Formation 描述一份初始系统组成；initfs 是这些启动输入的载体，不是运行期服务目录或应用安装登记。

出生块与 payload 的布局、映射和寿命由 [startup 实现](../impls/startup.md#出生块birth-block线格式)说明。

## init 的职责与终止

init 解释 Formation，完成初始基础服务装配，并在装配后持续承担该基础集合的根监督。Formation 是组成数据，表达实例、映像、资源、初始 OAL 命名空间、能力路由及生命周期种类，不是脚本。init 装配环境并发布进程，不以服务业务就绪作为发布下一实例的门槛，也不解释服务间的观察/等待关系；这些业务并发由服务自身组织。init 不解释领域协议，也不硬编码 device、fs、driver 等服务身份。

基础服务之间的业务供应关系、进程管理关系与能力路由不是同一张图。组态实例引用名、映像组件名、服务发布名和展示名分别属于各自语境，不从名字推导权限。

### 根监督与领域管理

init 的根监督集合不同于它的全部后代进程。未来服务管理服务本身属于 init 的基础集合，它管理的普通服务则属于自己的受托集合；两者不竞争同一实例的生命周期管理，也不在初始装配后整体移交基础集合。服务管理者与服务实例的关系由 [service](service.md)拥有。

pm 接受 init 的授权，承担受托进程管理，不替代 init 的组态解释与根监督。受托进程创建、领域管理者的委托以及普通进程视图由 [process-management](process-management.md)拥有；初始装配直接使用 initfs，不形成另一条 OAL 映像装载入口。

### 系统结束

系统组态表达持续运行的服务形态，没有按 task 集合完成而结束的结果。关机与重启是受授权的电源管理操作，不是 init 的 finish 策略；一次性工作负载也通过电源服务请求这些操作。空闲或任务结束不隐含停机，init 不提供正常退出模式。

根监督集合的异常属于系统级失败政策，不把普通用户进程 fault 变成内核 panic；受授权关机中的预期退出与意外故障不同。正常电源操作及跨服务收束不由 init 编排，受托普通服务的恢复政策由其服务管理者表达。系统复位 authority 见 [system-reset](system-reset.md)，职责与具体关机合同见[关机编排](../../plans/todo-2026-09-system-shutdown-orchestration.md#服务系统定位与职责修订)。

组态、根监督与终局的详细关系依据见[服务装配职责](../../plans/todo-2026-10-07-service-assembly.md#process-zero-与-pm-的职责澄清)和[组态计划](../../plans/todo-2026-10-28-init-formation.md)；当前执行器见 [startup 实现](../impls/startup.md#init-的组态装配与根监督)。

## 用户态 ELF 装载

内核只为 bootstrap 初始进程解析 ELF。后续装载由用户态 launcher 解释映像，通过通用的进程构造能力建立目标资源环境；内核不提供按文件名启动程序的入口，也不解释应用包、服务单元或驱动包。

ELF 验证是共享纯逻辑；用户态公共 loader 将映像、出生输入、显式授权与入口线程组合为进程出生。库的复用不产生创建权限，持权的创建入口与调用它的领域管理者仍是不同角色。Halcyon 的统一入口关系见 [process-management](process-management.md)，应用包解释与运行依赖见 [application](application.md)。

动态链接器（`ld-erhino`）是程序环境的扩展构想，连接新进程内的装载与运行依赖；它与 init、pm 的进程创建职责分开，不在本篇展开具体协议。

进程构造、控制与回收的内核概念由 [task](task.md)拥有，映射关系由 [mm](mm.md)拥有。公共 loader 的装配、推进及输入寿命见 [startup 实现](../impls/startup.md#用户态公共-loader)，不在本篇重复构造 ABI 或回收流程。

## 用户态接收与一次性交接

启动者交付出生映像，程序环境接收并向应用交接。该边界与具体语言运行时分开，程序环境分层见 [framework](framework.md)，用户侧内核接口见 [kernel-interface](kernel-interface.md)。

### 启动映像的寿命

出生映像是接收进程的稳定只读输入。payload 视图与能力的领取分别建模：借用或共享输入视图不等于取得映射回收权，领取初始能力也不改变出生映像本身的寿命。

入口线程资源与进程环境资源分开；入口线程与运行期线程使用同一线程模型，见 [task](task.md#用户态线程资源收束)。

### 启动凭据与采用

一次性启动凭据连接平台入口与出生上下文的采用，区分“读到出生数据”和“接管初始能力”。格式解析、真实能力核实与一次性交接是不同关系，不因取得一个句柄数值就产生第二份能力拥有权。

### 应用可见的接口责任

应用获得的 StartupContext 连接启动身份视图、只读 payload 视图和可消费的 InitialGrants；入口线程描述交给线程环境，不作为普通应用输入。启动身份只表达创建关系，不等于进程控制能力或服务身份。命名出生输入在通用出生约定之上组织，具体领域含义仍由对应服务解释。

采用与交接的当前接口、布局和清理行为见 [startup 实现](../impls/startup.md#出生块birth-block线格式)及 [runtime 实现](../impls/runtime.md)。
