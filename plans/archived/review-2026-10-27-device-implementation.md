# 设备首批实现的机制收敛审查

状态：**第二轮一致性重构、所选组合验证与交付前结构检查完成；原首批设备闭包仍保留独立验收门**。本报告承接原发现、设计处置、实现及复核，完成后归档。目标合同由[设备第二轮设计](../todo-2026-09-device-system-design.md#第二轮收敛设计2026-10-27)及[持续监督设计](../todo-2026-10-07-service-assembly.md#第二轮持续监督设计2026-10-27)拥有；DMA＋IPI组合与时钟异常仅由设备主计划维护。工作树尚未提交或push。

## 第二轮交付与边界

本轮按共同变化的两条责任链统一替换，没有保留旧协议兼容层或第二套监督实现。最终代码指纹及各路线原始退出码见[版本快照](../../artifacts/refactor/final-source-version.json)；下方失败记录保持原结果，不把开发过程中的失败改成通过。

| 收敛切面 | 当前结构与检查结论 |
|---|---|
| 逐绑定控制链 | 原轮Use直接报告消费静止；固定Use/Registrar出口、完整Binding快照、单调Stop和provider通知。停止仍等待实际供应/后代责任，不从CPU完成合成硬件静止。旧转述、重复Activate、StopActivation、ConfirmStopped和Query回跳已删 |
| 驱动与尾部寿命 | 实例独占首次准备与后续业务，RPC只引用结果；准备拒绝会发布终态、退原调用/观察后释放输入。Released后只保留紧凑回执，不保留供应业务Box；PCI一次Get配对描述与真实能力 |
| 事件推进 | USB完成用原seq和单接收者通知；Binding生命周期与PCI DONE不再常态短周期查询。保留确有硬件/收束前提的有限阶段轮询。真实四hart共同idle及IRQ唤醒通过 |
| 持续监督 | 原Assembly上固定两管理记录独占Control/SourceAdmin，登记DONE后Start；SourceClosed和DONE各守原事实，注销后交原Collector。删除观察线程、Shared锁、双通知、admin ACK、撤防和join；业务等待、注册探针、有限Collector和loader均接root |
| 装配与局部失败 | 同一ELF/loader/Image可分步推进，同步入口复用；Map按既有ABI单次上限，复制按页，未扩大预算。未发布空DMA根失败局部退款，不进入正常Root Work；旧C5专属问题的成立条件已删除 |

验证均使用真实消费者及既有路线；新loading变体只扩展原外部故障注入器，未增加测试服务、运行体或产品接口。

- 静态与host：`just check`、十个分面的`just clippy`通过；三workspace格式检查和`git diff --check`通过。设备相关库63项、ELF20项、loader/RPC/OAL/FS/service104项host测试通过。新增ELF用例核失败终态和vendor/Tag_File长度，预期来自固定格式与拒绝合同。
- 正常及准备/退休：[新版core](../../artifacts/run/20261027-153545-72566-acceptance-virt-debug.log)、[复用/真实HID fault](../../artifacts/run/20261027-154623-89082-acceptance-device-reuse.log)、[负候选/取消/复用](../../artifacts/run/20261027-161931-4090-acceptance-device-candidates.log)均退出0；含原轮Released、唯一进程收集及Pool退款。
- 持续监督：[Device等待期](../../artifacts/run/20261027-160006-95274-acceptance-device-manager-fault-Device-wait.log)、[Driver首ecall](../../artifacts/run/20261027-160034-95777-acceptance-device-manager-fault-Driver-start.log)、[Driver停止中](../../artifacts/run/20261027-160111-96356-acceptance-device-manager-fault-Driver-stopping.log)、[reset拒绝](../../artifacts/run/20261027-160335-97251-acceptance-device-reset-rejection.log)均退出0。等待/停止中可由真实业务错误先进入终局，不再要求已删除观察线程继续打印DONE；最早Start和loading仍核root实际DONE。
- 装载期故障：[入口](../../artifacts/run/20261027-161641-98955-acceptance-device-manager-fault-Driver-loading.log)、[原取证](../../artifacts/device-fault/20261027T081645.276122Z-99351/process-fault.log)退出0。真实test_target首段/首页指纹匹配、尚有293,922字节未复制时向Driver的实际ecall注入fault；root观察其Dead/Fault，未完成子进程一直未Start，随后唯一故障终局与QMP RESET成立，不作墙钟延迟承诺。
- IRQ与空闲：[最终入口](../../artifacts/run/20261027-161721-99758-acceptance-irq-idle.log)及[共同快照](../../artifacts/device-function/20261027T081730.356125Z-245/evidence.json)通过；具体门的唯一证据归属仍在设备主计划。
- 回复缺失：Claim、首次Prepare、ProvisionGet、GetResource、Activate、首次QueryUse、Master七条均退出0，逐条入口和结果见版本快照。GetResource保留不可逆暴露位并由原Use报告后释放；QueryUse在激活已接纳、IO尚未开始时失败，无资源暴露/硬件访问且原轮退休。该额外场景验证共享BindingTask准备拒绝合同，不宣称逐一注入所有HID取消交错。
- 平台：[sifive_u](../../artifacts/run/20261027-165856-43408-acceptance-sifive_u.log)及[无F/D](../../artifacts/run/20261027-165954-45095-virt-nofd.log)完整业务通过。未因平台差异删除消费者；C5指定分配失败的局部退款证据见下文。

交付前结构检查由主代理完成，有限辅助检查的发现均逐项核实处置：设备控制链sub-7、增量装配sub-8、持续监督sub-10。核心owner/权限、真实释放条件、先挂后Start、失败终态、SourceClosed/DONE先后、旧路径删除、稳态唤醒及跨层消费者均已核对。代码/工具中无旧StopActivation/ConfirmStopped/ReportStopped操作、旧PCI Query探针或监督线程握手；QueryBinding保留为显式公共查询。所有本会话启动的QEMU/GDB/验收进程已清理。

本轮不承诺复合USB、IOMMU、透明恢复、任意故障组合或真实硬件全覆盖；不关闭原DMA＋IPI组合证据缺口和根因未定的时钟异常，也不据多次正常运行归因旧失败。它们继续由[唯一收口表](../todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)按原触发和判据处理。没有当前承诺所需的半迁移或待接通消费者。提交需另行授权；取得固定提交后，才按REVIEW登记本次完整代码交付的独立审查。

## 一致性重构施工过程（历史开发证据）

用户已授权由本会话主代理实施，不启用 peer；subagent 只承担带完整目标和上下文的有限盘点/审查。基线仍为 a5a1a78，原设计文档改动保留，没有提交或 push。

设备控制链同步迁移了libdevice/libdriver/libusb、srv_device/srv_driver、PCI/xHCI/HID及function_checks：显式StopRequirement、固定Use/Registrar、Use报告与Get、完整Binding Watch、Provision通知和单调Stop、稳定激活准备/独立BindingTask、管理结果出口、USB完成通知及PCI单次Get。旧StopActivation、ConfirmStopped、Control直接资源取得/报告和管理者Query回跳删除；DMA未发布空根失败改为局部回滚。

早期开发证据：`artifacts/refactor/device-host.log`中libdevice/libdriver/libusb共63个host测试通过；新增用例依据停止事实单调性与原轮完整终态观察合同，删除无消费者的timing轮询门及测试。设备服务/驱动target check、用户态build、check/clippy的过程结果均保留；最终验证以上方交付表为准。

开发 QEMU：首两次 core 在 xHCI Image 创建遇 ReachLimit，原 initfs ELF 为 2,126,448 字节而 PT_LOAD 文件范围仅到 772,800；固定 MemoryObject 512 页上界未改。initfs 改为 strip-all，完整调试 ELF 留在 build；独立比较 11 个程序的 PT_LOAD 几何及字节完全相同。第三次 core 已看到 PCI/xHCI Ready、两 Binding Released、两实际 worker 收集及 Pool 页/metadata 归零，随后 Managed 重复消费缓存 Released 导致退场反复回到 Detach，已修为只转换一次。该次未通过，保留日志 `artifacts/failed-acceptance-20261027-133936-27310.log`；此前两次日志为 `artifacts/failed-acceptance-20261027-133123-24619.log` 与 `artifacts/failed-acceptance-20261027-133345-25682.log`。三次均先有明确装配/正常停止错误，不以外层最终 124 当纯推进预算不足。

限定施工审查由 sub-7 检查中央 Work 和 BindingTask。主代理核代码后接受并修正：HID 完成来源安装后保持 Runnable；ReportQuiescent 的合法 Closed 只结束报告、不合成 Released；Preparing 拒绝通知激活观察者；首次订阅失败与其他 pre-IO 失败走同一拒绝出口；核验/观察运输退场后释放拒绝记录的 Use/Registrar。该检查不覆盖全部硬件或持续监督，不是交付前结构检查的替代。

以下记录保留每次开发检查的实际失败与处置，已经闭合的中间缺口不再作为当前接续指令。

### 设备链后续开发证据

- `THROTTLE=100 just acceptance-device debug` 退出 0，入口 `artifacts/run/20261027-134338-31028-acceptance-device-debug.log`，双轮真实热插/输入/正常停止及 core 收尾通过。此后进一步把 PCI DONE 等待改为真实来源，并给 xHCI 旧供应 Lifetime 接入来源；控制器稳定运行及 HID 待输入不再依赖固定查询节拍。
- 复用首轮 `artifacts/failed-acceptance-20261027-135240-35839.log` 已到 A/B/C 同 PID 与输入，但旧注入器等静默 HID 下一次 ecall 超时。改为先 arm 实际 trap 断点，再用真实按键触发；GDB 短暂停机的明确 QMP `VM not running` 只重试未提交输入。第二轮 `artifacts/failed-acceptance-20261027-135813-39061.log` 确认真实 fault、B/C Released 与唯一收集，发现两个 Closed session 的 Signaler 仍保留 HID Pool 138 metadata；已在实际供应完成后 signal 并释放出口，中央 Provision 的通知出口同样随 Closed 释放。
- `artifacts/failed-acceptance-20261027-140725-43521.log` 的候选/复用客体全部收束：不兼容实际 PCI 拒绝、原设备更新 epoch、描述符尚未就绪时取消、A/B/C 输入与单进程复用、实际 HID trap fault、B/C 安全 Released、唯一进程收集、原 Pool refund 和显式 Shutdown。对应产品源码与 `artifacts/refactor/candidates-development-version.json` 一致。runner 退出 1 仅因新增 `hid-fault-armed` 私有文件不在 cleanup 清单；已补清单，实际三份遗留目录仅含该文件，逐份 unlink/rmdir 成功，无 QEMU/GDB 遗留。本轮保留退出 1，不改写为正式通过。
- HID 在终态且完成来源脱离后释放自己的通知 pair；PCI/xHCI 的正常业务错误先由统一出口发布 Stopping/首错，再推进既有硬件清理，清理中不可继续的错误才升级。该收敛后的编译与最终验证仍须按实际末版核对。
- idle 新一轮在客体装配前即因旧 PCI Query 源码行探针失效退出：`artifacts/failed-acceptance-20261027-141421-48015.log`。已删除 `qemu_pci_poll_gdb.py` 和接线；真实四 hart 同时快照与 IRQ 判据未放宽，Prepared 的取证同步接受新 QueryProvision 回复。修正版 `THROTTLE=100 just acceptance-irq-idle` 退出 0；真实共同四 hart idle、插入后的同源 pending/claim、Take/Ack 与 core 收口成立。该门已移入主计划的可复用表，具体原证据由该表拥有；这不证明旧八轮失败的唯一首因。
- Activate回复缺失实际完成原轮停止/Released/唯一收集及core关机，`artifacts/failed-acceptance-20261027-142832-52807.log`；runner退出1因仍匹配已删除的`next: Released`调试布局，已改为终态回执字段，不改写本轮退出码。
- `THROTTLE=100 just acceptance-device-unknown GetResource`退出0，`artifacts/run/20261027-144412-57482-acceptance-device-unknown-GetResource.log`。真实首项Get回复未交付时，暴露位始终true、PCI未开始ECAM访问，原Use报告后安全Released并完成core正常关机；旧“必走全局故障”的成立条件已由局部实际静止证明消除，未将Unknown改写成无暴露。

### 持续监督前置施工

已新增同一`elf::Validation/IsaReader`、`libprocess::Preparation`和`ImageCopy`分步入口；同步入口组合原owner。root新增统一Assembly的有限等待槽和显式loader/Image推进方法；管理者与存活期出生、OAL/文件/服务RPC、报告/停止/功能验收和并发Retry主线的等待已接入。新的两槽ManagerTask持唯一ProcessControl与SourceAdmin，实际DONE来源先附后Start；旧线程、Shared锁、changed/ack通知、admin交还、撤防和join全部删除。独立内核ABI自检保留全部用例，放在实际消费者完成、两管理者正常收集之后。**新版运行验证与有限责任链检查仍在进行，不是持续监督交付。**

ELF原18项host测试通过，`libprocess/srv_driver`与随后`root-loading-check.log`的target编译通过。第一次逐页Map开发core在设备管理者Start前明确QuotaExceeded，日志`artifacts/failed-acceptance-20261027-145822-63484.log`；外层124来自Reboot停留，不是单纯推进撞限。核实每笔Map都建独立backing/region后，Map改为每step一笔原ABI的256页上限，Write仍逐页，未扩大预算；依据同步回填服务装配设计，修改后尚待运行复核。

sub-8只读检查增量前置，主代理核实后修正：Validation冻结首错；取消后保留已消费GrantOutcome；Create前核MANAGE/WAIT/READ完整取消权限；Image提供显式cancel/close并保留Unmap失败owner，Drop不能吞掉失败。属性vendor长度/命名空间的旧缺口和测试fixture已按固定格式修正；原18项加失败终态、vendor/Tag_File边界两项共20项host测试通过，`elf-incremental-host.log`。此检查不替代最终结构检查。

属性布局取证由sub-9核对[固定aaelf32](https://github.com/ARM-software/abi-aa/blob/daa7a94ca55973736c0e434a67a6e4bbcd35d7fa/aaelf32/aaelf32.rst)「Build Attributes」：vendor长度包括自身4字节、vendor NTBS和属性数据；Tag_File长度包括tag、4字节size和内容。实际产物的78字节节/77字节vendor与此相容；RISC-V固定psABI只拥有tag、odd/even值和架构字符串约束，不把两规范之间写成有规范性引用。当前实现只从riscv命名空间采用架构声明，其余vendor跳过。

`artifacts/failed-acceptance-20261027-151401-65732.log` 已越过loader与服务装配，随后旧Copy验收在另一笔同步观察RPC期间不再冻结原root运输，原创建回复被正常消费，导致“迟到回复”断言失去窗口。已改为确认Sent后立即取消，再观察实际目标，保持Unknown/迟到回复与后继隔离的原验收目的，不停驻监督或删除场景。该轮明确失败，不算单纯撞限。新版单owner target检查（含Retry/IRQ特性）和用户态build通过；`THROTTLE=100 just acceptance-virt debug`退出0，入口`artifacts/run/20261027-153545-72566-acceptance-virt-debug.log`。sub-10已核Start前观察、正常两种完成顺序、真实detach/Collector及唯一终局，无新增owner问题；发现的init注册队列探针和1ms有限Collector等待均已接root。队列探针编排移回init，test_fs只共享原编解码/注册夹具，未增加pump回调或新测试运行体。

### 最终验收器接点修正

- 第一次单root的Device等待期fault已发生真实非法指令、唯一故障终局和QMP RESET，但旧验收器仍等待独立观察线程的DONE日志，继续进入新启动。`artifacts/run/20261027-155042-93872-acceptance-device-manager-fault-Device-wait.log`保留；主代理向本次操作者发送TERM并确认清理，原状态143，不改写通过。现在等待/停止路线要求实际被注入PID的kernel fault、唯一封口和RESET；Start/loading还要求root实际DONE。修正版退出0。
- Prepare首轮`artifacts/failed-acceptance-20261027-162755-13084.log`完整core正常关闭，但探针仍等已删除的“再次Prepare返回Prepared”，未命中注入，退出1。改为首次真实Preparing/Prepared接纳回复，其后Prepared由Query取得；重跑退出0。
- 新QueryUse首轮因工具prefix映射遗漏，在注入前KeyError，`artifacts/failed-acceptance-20261027-165012-34533.log`保留外层124；这不是产品推进撞限。已补齐并核所有操作均有观察prefix；重跑退出0。失败注入均只改变一次实际回复Send期限或真实用户trap原因，没有修改业务正文/ref/cap、安全证明或产品状态。


### C5 替代路径的指定失败证据

`THROTTLE=100 just virt-platform-funding dma-retained` 退出 0，入口 `artifacts/run/20261027-135625-37995-virt-platform-funding.log`；[实际证据](../../artifacts/boot-failure/20261027T055629.589219Z-38357/platform-dma-retained-evidence.json) 的 `verified=true`、`root_birth_refunded=true`。仍只令原 retained entry 的一次未执行分配返回 null，实际返回 OutOfMemory；原 DMA 根出生前与 Node::root 返回后的同 Pool 两维账完全相同：pages allocated=4、metadata allocated=63936，reserved/delegated=0。该未发布根没有排正常 Root Work；四个不同 hart 的 Gate Failed 停驻时，只保留先前实际已发布的三 MMIO/一 IRQ 清理 owner。

C5 的旧“未发布根经 Drop 排 Root Work 并在四 park 保持 Queued”成立条件已删除；旧八轮记录保持历史，新工具直接核原构造前后退款，不再解引用已释放 Root。此证据仅覆盖未发布零成员空根，不关闭已暴露 DMA＋IPI、正常深树或整个 bootstrap 全额退款的独立问题。

## 固定对象与证据边界

- 基线：`549a9e78400f9e59f3bdf516ef4d54f101846178`。
- 实现：`f3337ebd789a79c8cad23c419b690b702e8f96bb`。
- 审查接手 HEAD：`a5a1a78`，只增加审查登记与导航。已用 Git 核对它与实现检查点之间的 `os/`、`shared/`、`user/`、`tools/` 无差异；初始工作树干净。
- 输入：[固定审查登记](todo-2026-10-27-device-implementation-review.md)、[设备审查交接](../todo-2026-09-device-system-design.md#审查交接2026-10-27)、[唯一收口表](../todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)、[当前实现说明](../../notes/impls/device.md)、[固定平台契约](../ref-2026-10-23-device-platform-contracts.md)。
- 主代理亲读中央 Registry/Binding Work/Watch、实际 srv_device 入口与平台供应、PCI 资源取得与轮询、Managed 订阅/停止、init 监督线程、DMA 根出生/稳定保留/Retire、VM ForcedRetirement 与 runtime_stop；辅助审查限于驱动管理和 PCI/xHCI/HID 激活、轮询及停止链。
- 本轮没有运行 QEMU、构建或测试，没有重新核验历史 artifacts；已有运行结果仅按主计划的版本与限制引用。以下源码位置均针对固定检查点。本报告不是全部 unsafe、硬件时序、ABI 与并发路径的穷尽安全审计。

## 总体判断

建议先做第二轮机制收敛，再为改变后的结构补必要验收。当前的主要问题是：同一生命周期事实在多处以订阅、轮询、回复和长期任务反复协调；部分补偿代码的原前提已不再成立；局部未发布失败也进入了正式异步退休路径。

不能据此把所有领域层次合并。Seat 的独占、Provision 的供应效果、Activation 的驱动状态、ProcessTask 的进程寿命分别回答不同问题。DMA 静止证据、资源 DONE、Binding Released 与最后旧入口退款也不是同一个完成条件。收敛目标是减少它们之间不必要的协调与持有关系，同时保留这些真实区别。

本报告的 C 编号是源码证据索引，不是逐条裁剪的施工清单。C6 包含确认的生命周期缺陷，其余主要是机制替代候选。没有证明的性能退化和故障根因不登记为事实。

## 多视角判断：怎样换机制才能更轻、更好理解

用户明确要求同时比较效果和心智负担，不能以减少字段、文件或代码量作为结论。以下各视角检查的是同一条责任链；最终建议须在多项上共同改善，不能把复杂度搬到另一层。

| 视角 | 当前具体负担 | 更直接的表达 | 要得到的实际改善 |
|---|---|---|---|
| 所有权 | 首次 Activate 的准备查询在 Request，Slot/Engine 又提前存在；被拒后没有统一退场状态 | 原轮实例持有唯一准备与取消责任；运输请求只引用它 | 请求失败不再决定业务 owner 是否存在；准备拒绝不必先进入 Active 才能退休 |
| 时间与生命周期 | 业务执行、硬件释放、入口尾部共同塞在长期 Work 中 | 同一 owner 消费式移交准备、活动、收束、回执所需资源 | 读某阶段时只需理解该阶段真正可能存在的资源与责任 |
| 消息接口 | Watch 已给状态，管理者仍重新 Activate/Stop 取得结果；PCI Describe 后再 Get 同一描述 | 提交命令、读取原结果、观察变化分别有明确语义 | 正常观察不再重新运输授权能力；一次资源取得就给完整配对结果 |
| 活性与执行 | 生命周期、USB 完成、硬件就绪混用短周期轮询；init 同步等待另配监督线程 | 完成事实由原 owner 发布；监督 owner 持续处理事件 | 无变化时能自然停驻；慢 RPC 不决定查询频率；监督不依赖另一条协调线程 |
| 成本与容量 | 每次无变化 Query 都有运输/任务费用；旧入口保留完整业务承载 | 少量有界的观察来源和紧凑终态记录 | 用明确常驻成本换掉随时间持续产生的无效工作；尾部成本与业务寿命分离 |
| 可验证性 | 很多证明依赖捕捉两个状态机交接的精确窗口 | 由 owner 的提交/转移点给出不变量 | 能证明一类交错不会出现，减少逐个故障窗口补证；硬件风险仍保留组合验证 |
| 演进 | 增加一个驱动要重写 Binding 查询/停止观察；增加服务要复制监督协调 | 复用有明确领域合同的普通组合类型，各宿主仍用原 Runtime | 新消费者只实现自身协议推进；不引入通用驱动框架或全局事务管理器 |

### 方向 A：原轮实例拥有准备到退场，消息不拥有业务生命周期

这是 C3、C5、C6 的共同机制方向，并非把不同服务的状态合成一个总状态机。

在每个实际 owner 内，准备中的记录持有唯一提交资格、原查询和预付存储；成功后把已准备资源移给活动阶段；停止后由该 owner 收束已经产生的效果；硬件/绑定责任结束后仅保留回执阶段所需的旧身份和 Lifetime。未产生效果的拒绝直接进入本地退休，不要求先成为活动对象。跨进程的 Registry、Provision、驱动、ProcessTask 继续各自拥有独立事实，不建立第二个全局状态副本。

RPC 失败只结束本次请求/回复；已提交业务仍由原轮记录持有。重复请求在认证相同输入后取得同一个记录，不各自再持一份首次准备查询。具体类型可用消耗式转换或按阶段持不同资源；如果只是给现有 bool/Option 外包 enum 而仍保存全部状态，则没有达到目标。

收益是更少的合法状态组合和更自然的失败路径。HID 未提交记录泄漏、DMA 未发布空根的异步退休、Released 后完整 Work 的长期留存，可分别成为这一原则的具体检验点。三者不是一个泛型事务库的理由。

代价是需要明确“第一次接纳”与“运输请求退休”的边界，并预付回复缺失后仍要推进的责任；不能靠把 Request 扔掉来取消已提交操作。

### 方向 B：业务 owner 发布事实，观察者直接消费事实

这是 C1、C4、C7、C8 的共同机制方向。正常路径使用现有序号/状态快照和 Notification；收到变化才读取原结果，只有首次提交、明确取消及 Unknown 恢复进入命令接口。

协调者自身也应按事实依赖推进：区分原轮接纳、资源就绪、激活接纳、静止证明与供应关闭；一次 Query 不成为新的业务阶段，也不需要为它保存“返回旧步骤”的 resume。运输层仍保存真实 txid、取消和注销责任，阶段用明确的局部状态表达，不把 next/resume 改成更多任意组合的 bool。现有 Managed 的步骤、回跳和发送标志见 managed.rs 9–51、571–617 行。

绑定生命周期由逐绑定 owner 组合现有 Watch；激活 Starting/Stopping/Terminal 直接消费已收到的 Snapshot，首次 Ready 才获取原功能 capability；USB session 在实际结果写入 current.result 时通知，HID 查询同一 seq。资源 Get 一次取得描述与能力，不额外 Describe 同一 Frozen。

供应侧也要一起比较。Binding Work 当前重复 Prepare 读取 Preparing/Prepared，重复 Stop 读取 Stopping/Closed（binding_work.rs 426–463 行）；只改驱动的 Watch 并没有收敛整个控制面。若给 Provision 增加进度观察，须用它替换对应常态重发及计时责任，不能在原轮询外再套一层订阅。

收益不仅是少几次 RPC：驱动 Engine 可以专注硬件和领域数据，慢回复不再形成下一次轮询的起点；无输入自然没有重复业务工作；状态查询失败与重新发起命令失败不再混在一起。当前全 idle/PCI 观察问题也获得了改变成立条件的机会。

增加观察来源也有存储和寿命成本。USB 的方案只需考虑原独占 session 的完成提示，不默认复制多观察者 Watch/Ack 框架；序号和真实 result 是真值，提示允许合并。若某处只能靠另一套缓存/订阅表/通用事件总线成立，就应放弃该替代。硬件缺少事件的 CNR/reset/halt 等阶段仍可有有界轮询，原 owner 死亡仍由监督接管。

### 方向 C：装配本身是持续推进的监督工作

这是 C2 的替代目标。运行服务的监督不再作为同步装配脚本旁边的补充观察器；启动、就绪、业务等待、停止与终局都提交给持续运行的单一监督 owner。已有进程/Job/Pool 管理及 Runtime/Assembly 是可复用基础。

收益是正常停止和异常死亡在同一 owner 下排序，终局 authority 不必跨线程争夺；装配等待不再制造监督盲区。代价是受该 owner 管理的同步等待都必须转为可让出的推进，不能通过只改几处调用或增加定时 pump 冒充完成。

本方向不承诺“改成单线程必然更好”。若需要独立 CPU 执行能力来隔离无法让出的工作，线程仍有独立理由；届时应让监督状态由一个线程独占，而不是两侧分别修改记录再握手。第二轮先证明实际执行需求，再选择形态。

外层装配可以继续保持顺序表达：提交一个实际操作后，由 RootSupervisor 的明确等待循环推进全部责任，直到取到该操作结果。没有必要把整个 init 改成一张巨型异步状态机。也不能在通用 Caller 中藏回调来偷偷 pump supervisor；那会引入隐式重入和借用边界。应以现有 RootSupervisor 的 await/turn_and_wait 接点审定完整可达等待。

### 控制面还需正面对照：协调者转述，还是参与者直接提交事实

方向 A/B 可以保留现有信任和服务边界。另一个值得比较的更深入候选是：原 Binding 拥有不可逆停止意图，可信参与者直接观察该意图并提交原轮事实，srv_driver 主要负责选择、进程和失败政策。

| 维度 | 保留协调者，依据事实推进 | 原 Binding 与参与者直接连线 |
|---|---|---|
| 静止报告 | Activation Snapshot→Managed→ReportStopped→Binding→Provision | 可信激活持原轮受限报告入口，直接交 Binding，再由供应 owner 判定安全 |
| 主要收益 | 不扩大授权模型；消除按 RPC 步骤回跳和观察重发 | 正常清理不必逐步经过驱动管理者转述；责任链更短 |
| 实际代价 | 报告转交及管理者持有关系仍在 | 新的受限入口、准备时接线、付款与尾部寿命必须有便宜而完整的合同 |
| 失败 | 原轮 owner 保留责任，协调者故障按已有政策升级 | 直连不等于自动恢复；缺报告仍保留并升级，不能推出管理者可透明重启 |
| 不可接受的变体 | 新增订阅但保留全部常态重发 | 把 BindingControl 整体交给驱动，或让 srv_device 管程序/进程和驱动政策 |

直连必须在资源暴露前具备停止观察；报告能力只限真实原轮、已授权受信角色和静止事实，不能从 payload/tag 自授。HID 不因此取得物理 DMA 报告权；同进程多个激活互不代报。原 Binding 的“唯一出口从未暴露且已封口”内生证明仍有效；进程死亡或 Activate Unknown 不产生该证明。

**当前建议**：先把“保留协调者但依据事实推进”画成完整可实施对照，再比较直连方案。直连只有在正常、Unknown 与父子停止三条路径的总交接确实更少时才胜出；如果需要通用参与者注册、恢复账本或多方事务，就应选前者。本轮没有证据支持现在直接合并服务或定稿新报告权限。

贯穿各方案的责任检查如下，表中阶段允许并行和提前取消，不是全局线性状态机：

| 事实 | 唯一提交者 | 其他参与者可以依赖什么 |
|---|---|---|
| Claim 已接纳 | Registry/原 Binding | 预付责任存在、原轮占用成立；回复缺失不撤销承诺 |
| 供应已准备 | 实际 Provision owner | 本轮资源与准备效果已有 owner；中央仍控制唯一资源出口 |
| 激活已接纳 | 驱动内原轮实例 | 固定输入与准备责任归该实例；重复 RPC 不新开准备 |
| 停止已提交 | Binding 封新准入，各实际 owner 停自身业务 | 意图不可逆；父供应仍服务已有子轮清理 |
| 消费者已静止 | 有权且实际掌握硬件/访问事实的 owner | 不能直接推导供应 Closed、CPU DONE 或退款 |
| Binding Released | Registry/原 Binding | 供应与实际依赖满足释放条件；旧入口仍只指原轮 |
| 入口尾部退休 | 原终态承载 | 已接 Delivery/Lifetime 结束后按实际存储退款 |

第二轮以三条完整轨迹比较方案：正常停止；Activate 已提交但回复丢失；父激活已停止访问但子 Binding 尚未 Released。逐步列出权限、付款、观察和失联接管，才足以判断效果与心智负担，而不是按减少的行数选型。

## C1：生命周期观察与周期查询并行，且一条补偿理由已经失效

**优先级：高；双轨实现与失效理由已确认，统一观察方式需第二轮设计。**

证据：

- [`Managed::consume`](../../user/services/srv_driver/src/server/managed.rs) 194–204 行已经建立 Binding Subscription；232–237 行在每次读到 Stopping 后再设 1ms 重查；626–634 行 `WaitReleased` 继续 QueryBinding，理由写为“来源封口可先终结 Binding Watch”。
- [`Registry::watch_reason`](../../user/libraries/libdevice/src/registry/discovery.rs) 163–174 行的实际实现明确区分 Scope 与 Binding：只有 Scope 检查 `source_open`；已有 Binding Watch 继续观察原轮，直到 `binding_snapshot` 不再存在。`release_binding` 在原提交点调用 changed（[`registry.rs`](../../user/libraries/libdevice/src/registry.rs) 1051–1081 行）。[`request.rs`](../../user/services/srv_device/src/server/request.rs) 700–714 行也没有在 QueryWatch/AckWatch 前重新加来源开放门。
- [`Subscription`](../../user/services/srv_driver/src/server/subscription.rs) 105–112 行已将变化唤醒原 Managed。因此“来源封口必然需要另一路周期查询”不是当前代码事实；接手时 [device impl](../../notes/impls/device.md) 的管理者描述也沿用了这条旧解释，本轮已按实际实现校正。
- [`PCI Engine`](../../user/drivers/drv_pci_generic/src/server/engine.rs) 234–265、275–286、475–486 行独立查询 Binding：计时在发送时起算，完成后在同一 advance 继续判断。若往返耗时已超过 1ms，会立刻再提交；这是可达的持续 RPC 条件，不等于已经证明历史全 idle 失败由它造成。Runtime 对未改变的 deadline 不重复登记（[`work_queue.rs`](../../user/libraries/libexecution/src/work_queue.rs) 564–587 行），所以不能把此事误写成“旧 deadline 自身无限触发”。
- PCI 的 [`Host::advance`](../../user/drivers/drv_pci_generic/src/server/supply.rs) 427–511 行还承担准备/退休推进与 DONE probe。不能只删 Query 定时器而让资源退休失去唤醒。

建议方向：让原轮生命周期观察成为正常路径的唯一变化来源。管理者已有的 Binding Subscription 应覆盖停止到 Released；驱动侧将重复的 Binding 观察组合到既有逐激活 owner，资源退休等待真实内核 DONE，硬件状态轮询只在硬件协议确实没有可用完成事件的阶段保留。沿用现有 Runtime、SourceRegistration、RPC subscription，不新增驱动执行器或通用事件总线。

必须闭合的例外：Watch 尚未成功建立就失败；真实服务/运输失效；停止与订阅安装/注销交错；未知激活已提交；父供应仍要服务子轮直到安全结束。它们需要有限的停止/升级责任，不能从“正常路径不轮询”推出“任何失败都可以只等通知”。准备/停止绝对期限继续作为故障边界，不能把超时当完成。

可删除的是重复 Query 调度、与之配套的周期状态和旧理由；不是删除 Query 接口、认证核验或硬件完成责任。此项最直接对应全 hart idle/PCI 观察的待证门：先减少无变化时的常态工作，再检验真实静默与 IRQ 唤醒，保留共同快照和真实事件要求。

验证应覆盖：来源 Seal 后原 Watch 仍观察 Stopping→Released、旧轮不跟新轮；订阅未建成的失败收尾；无变化时不持续发 Query；真实停止/资源 DONE/热拔出仍推进。现有 Watch 单测及实际设备消费者可复用，不需新增测试服务。

## C2：init 的同步装配前提制造了第二套跨线程监督协调

**优先级：高；复杂度来源已确认，替代执行结构属于设计候选。**

[`device_supervision.rs`](../../user/services/srv_init/src/device_supervision.rs) 38–82 行定义 Gate、Stopping、Admin、Record、Shared 和 Terminal；287–310 行由两个线程争唯一终局 authority；320–348 行创建额外线程、WaitSet、changed/ack 两组 Notification；439–566 行实现正常停止意图、来源关闭、admin 关闭确认、撤防、join 和线程记录 reap。

[现有施工合同](../todo-2026-09-device-system-design.md#init设备管理者的独立观察与终局施工合同) 明确：该线程是为了在主线程同步 Caller/wait 时仍能观察两管理者死亡。这个需求真实且当前方案有必要性，但“主线程必须长期同步阻塞”并不是设备领域的独立契约。现有 [`RootSupervisor`](../../user/services/srv_init/src/supervisor.rs) 已有长期责任、Runtime/Assembly 与收集 owner，值得从整体执行方式重新设计。

建议将装配、RPC 等待、管理者 DONE、正常停止和故障终局纳入单一持续推进的监督 owner。依赖服务的等待本身以任务状态表达，不能阻塞该 owner。这样可以删除共享 Gate 的跨线程仲裁、观察者 admin 副本交还 ACK、changed/ack 互通知及设备专用线程 join/reap；Start 前登记 DONE、正常意图先于停止、真实 cause/detail 核验、唯一终局仍保留。

这不是在当前同步主线下直接删除 Observer，也不是把同一批状态搬进另一条线程或通用 supervisor 框架。详细设计必须盘点全部可能阻塞监督 owner 的调用，含验收 workload 的等待；不能只迁移设备启动几处调用就宣称监督统一。正常与故障路径都要有明确的控制根持有者，故障终局仍不能等待无证明的 DMA 释放。

已有长同步 wait/Start/Stopping 故障验收证明当前约束下的行为，不构成保留第二套协调的理由。重构后保留等价的“其他业务等待时仍及时处理死亡”场景；专用线程的撤防/退款窗口在删除线程后可按代码删除证据关闭，不再逐窗复现旧实现。

## C3：Released 后仍由完整业务 Work 承载旧入口尾部

**优先级：中高；持有关系已确认，缩减终态承载需设计。**

[`binding_work.rs`](../../user/libraries/libdevice/src/binding_work.rs) 169–188 行的 Work 包含 provider、Collecting、RPC 状态、策略与多个时间/停止字段；371–385 行在 Released 时释放资源表，但保留 Work；594–608 行必须等 Binding Rc 只剩自己，才归还 Registry 身份引用并结束任务。旧 Control/Use/Watch 的 Lifetime 分别经 [`request.rs`](../../user/services/srv_device/src/server/request.rs) 921–1001 行移除角色并唤醒原任务。

因此“持有一个旧轮查询入口”继续保有完整准备/停止执行对象、provider 强引用及任务承载。请求正文和 Outbox 已在 1124–1130 行释放，本项不声称存在这些缓冲泄漏；付款和旧身份留存本身也不是错误。

建议在真实 Released、原 RPC 已脱离之后，将原任务转为明确的终态承载，只保存固定 BindingRef、终态快照、旧角色/Lifetime、实际费用和确有必要的身份引用；结束准备/停止状态及 provider 持有。优先在原 task 内进行消费式状态转换，避免为了“轻量”另造运行体、全局墓碑表或第二份轮次索引。

旧入口仍只能看到旧轮，不能转向新 epoch；已接 Delivery 仍有 owner；保留记录继续收费且有容量上界；是否还需保留来源/设备身份要逐项按终态 API 判断，不能一律提前 release。若压缩只是多包一层 enum，而原 Work 和引用都继续存在，则不算收敛。

这一结构可减少旧入口长期持有对后续激活容量、provider 寿命和退款验证的干扰，但不能消除最后引用语义。复用已有 A/B/C 同 PID、A 旧入口尾部与 B/C 继续运行场景；另核 Released 后不再有供应能力或业务 RPC，而旧查询与最终退款仍正确。

## C4：PCI 的 Describe→Get 重复读取同一冻结描述

**优先级：中；可删除的冗余协议阶段已确认。**

[`PCI Engine`](../../user/drivers/drv_pci_generic/src/server/engine.rs) 9–12、122–160、349–379 行为四项已知名称资源先走 OAL Lookup/Describe，保存 Descriptor 后再 GetResource。[`Host::adopt`](../../user/drivers/drv_pci_generic/src/server/supply.rs) 78–154 行比较两份描述，随后独立核实际 capability kind/rights、HostInfo、MMIO 几何、IRQ 范围和 DMA 模式。

服务两端的描述都来自同一 Frozen 表（[`Binding`](../../user/libraries/libdevice/src/binding_work.rs) 97–154 行）；Get 已将名字、Descriptor、metadata 与 capability 放在同一回复里。前一次 Describe 既不是独立信任源，也不提供原 Get 缺少的硬件事实。

建议 PCI 直接对声明要求的名字 GetResource，在同一回复中完成目前的真实能力和几何校验。删除 Call::Describe、跨调用 Descriptor 以及“Describe 成功后 Stop 先到”的专有分支。OAL 资源发现是独立公共能力，继续保留；需要验证它时由现有发现消费者/协议测试承担，不能把每次实际驱动初始化固定变成双往返。

这能消除一组初始化中间态和失败点，不消除 Get 的 Unknown、资源暴露或实际硬件安全责任，也不直接关闭当前 DMA＋IPI 待证门。

## C5：未发布 DMA 根的末次分配失败被送入正常异步回收

**优先级：中；可以从结构上消除一个已验收特殊失败路径。**

[`Node::root`](../../os/kernel/src/resource/dma/mod.rs) 150–199 行先构造单节点 ResourceTree 和 Node，最后 197 行才为 NODES 保留条目分配。此时根未返回调用者、没有 capability 发布、没有子节点或 Mapping；但 `?` 失败使正常 Node::drop 在 366–391 行标 dropped、链接 reclaim 并发布 Root Work。

同一构造器 172–188 行对较早的 `Self::new` 失败已经有未发布单根的同步 seal/finish/remove 回滚。晚一笔软件存储分配失败并没有产生必须异步接管的新物理责任，没必要仅因 Rust 对象已经构造就切换到正常生命周期。主计划已通过的 retained-entry 门也明确只证明五个 owner Queued，未证明停驻前退款。

建议把所有未发布空根失败归于同一构造 owner：保留条目失败时，在丢最后 Node 前显式撤回该单节点；再释放局部存储和付款。现有 Drop 对树中不存在的节点直接返回，可作为具体实现核对点。也可评估预付全部出生存储，但不为此引入通用构造事务框架。适用前提严格是“未发布、零成员、零子项、没有外部观察者”；成功发布后的失败仍走稳定维护 owner。

预期删除的是 retained-entry 失败专属的 Drop→Root Work→启动失败停驻队列取证链。Root Work 对已完成后代的逐步身份回收仍有独立用途；Retire 的祖先 authority 校验、NODES 与 RETAINED 稳定根不能跟着删掉。

验证边界可以缩到精确失败点、无发布事实、局部费用与队列无遗留，以及正常根出生/派生/退休既有回归。原成功通过记录保留历史；替代路径验证后登记“旧路径删除”，不把旧八轮取证改写成失败。此项与暴露后的 DMA＋IPI 保留门不同，不能互相替代。

## C6：首次激活准备与实例寿命分开，HID 被拒记录没有退场路径

**确认缺陷，建议由方向 A 整体解决；置信度高，尚未运行复现。**

[`HID Request::activate`](../../user/drivers/drv_usb_hid/src/server/request.rs) 99–134 行先 associate Instance，再由请求内的 Activate 发起 Binding 查询；[`World::associate`](../../user/drivers/drv_usb_hid/src/server.rs) 89–120 行已把 Instance 放入索引并准入 Engine。等待期间 Binding 可以停止/Released，或者该请求失败/超时。

[`Activation::prepare`](../../user/libraries/libdriver/src/activation.rs) 25–39 行拒绝非 ResourcesReady。Request 的 239–291 行随拒绝调用 [`reject_uncommitted`](../../user/libraries/libdriver/src/worker.rs) 209–215 行，释放 active 额度、关闭入口母本，但留下 activation=None；此后 Adopt 对这个 inactive/None Slot 返回 Closed（431–432 行）。

[`HID Engine`](../../user/drivers/drv_usb_hid/src/server/engine.rs) 118–125 行却只有在 activation.is_some() 且 tail_done 时才移除 Instance；None 直接 Parked。即使所有 Lifetime 已完成，这个未提交实例也没有退休路径。另一个 HID 保持同进程活跃时，该记录及 Engine/来源预算会一直占用，重复发生可耗尽 32 项记录容量。

它暴露的结构问题是：准备 owner 在 Request，存储/执行 owner 在 Instance，拒绝与退休由两者以 Option/额度位间接约定。方向 A 让同一原轮实例持有唯一准备提交资格，拒绝后不可逆转入退休，等待原查询/请求与 Lifetime 结束后移除；RPC 只持本次回复责任。由此可以同时处理成功、Stop 先到、Unknown 和失败退场。

不能把修复范围缩成只去掉 activation.is_some()：还要复核同 ref 重复请求、已在途查询与索引移除的关系，明确迟到提交被何处拒绝。验证复用实际多 Binding 消费者，保持一个活接口，令另一轮首次准备被 Stop/Released 拒绝，核原实例/任务退场、旧请求不复活且活接口继续工作。该 finding 的处置与复核留在本报告，不另建 todo。

## C7：已观察到激活状态，却把它翻译为再次执行激活协议

**方向 B 的核心接口候选；重复往返已确认。**

[`Subscription`](../../user/services/srv_driver/src/server/subscription.rs) 90–112 行取得完整 Activation Snapshot 并写入 run.activation_status；[`Managed`](../../user/services/srv_driver/src/server/managed.rs) 605–617 行把激活变化转成 Activate 或 StopActivation，768–807 行再次运输 BindingUse/Registrar；[`Activate::adopt`](../../user/libraries/libdriver/src/worker.rs) 415–487 行再次复制 identity、采用并核验能力。

这里混合了三个不同动作：第一次命令接纳、观察已提交状态、取得 Ready 的功能能力。Starting/Stopping/Terminal/consumer_stopped 已在快照里，却仍经过用于提交和幂等恢复的整条协议。Ready 功能 capability 确实不能凭纯状态凭空产生。

更直接的接口是：Managed 消费已获得的状态；仅首次 Ready 缺功能结果时，从受原激活权限限制的结果出口取得同一 capability。可比较“同 ref Activate 只用于一次结果恢复”和“显式取得原激活结果”两种形态，以总状态/运输/认证成本选择；不是为了命名整齐机械增加 opcode。停止命令只提交停止意图，之后的停止进展由同一观察读取。

订阅的读取后 Ack 可以在原 Managed 内组合现有 ConsumerTask，减少两个任务之间的 changed/reading 握手，但不放松序号确认，也不让一条未完成业务 RPC 阻塞观察推进。首次认证、Binding 查询、Unknown 同轮恢复及硬件停止证明仍需保留。功能取得/安装去重、收到 Terminal 但没有 consumer_stopped、Stop 与 Ready 同时到达是验证重点。

## C8：USB 完成已经有唯一结果记录，HID 仍按固定周期询问

**方向 B 的领域消费者候选；收益与新增来源成本需比较。**

[`HID Engine`](../../user/drivers/drv_usb_hid/src/server/engine.rs) 233–295 行把 Pending 变为同 seq Query，每次回复后延迟 2ms 再查；[`xHCI Request::usb`](../../user/drivers/drv_usb_xhci/src/server/request.rs) 242–255 行连只读 Query 也唤醒硬件 Engine；[`Host::advance`](../../user/drivers/drv_usb_xhci/src/server/supply.rs) 371–391 行才把真实 Controller 结果写入 current.result 或完成停止。

原 session 已有唯一 current.seq/输入/结果，下一 seq 只在旧结果存在后接纳（同文件 232–306 行），并不缺“由谁保存结果”的机制。可以在这个提交点发布一个预付的 session 完成提示，HID 醒来后读取原 seq，下一 seq 沿原合同确认上一结果。信号必须与序号复核配合，先完成后开始等待也不能丢事件；取消/断开/供应停止须同时覆盖。

这样使无输入的 interrupt IN 不再产生持续跨进程询问，响应也不额外等待 2ms 查询窗口。新增的是一个有界、角色受限的通知寿命，不是第二份 USB 结果账本。保留 TD 与 RPC 寿命分离；不能把一个挂起回复作为硬件责任的唯一 owner，或因客户关闭通知就释放 TD。

本案全 idle 门在插入键盘前采样，因此 HID 轮询不是那个历史门的解释；此项改善插入后无输入的长期行为。验证实际 Pending→完成、完成早于等待、取消/拔出、旧 seq 提示与新 seq 交错，仍按原 xHCI 硬件停止合同收束。

## C9：Binding 观察只返回变化标志，业务状态还要再次查询

**第二轮追加的接口收敛项，已选目标、未实施。**

[`libdevice::client::Watch`](../../user/libraries/libdevice/src/client.rs) 56–60 行的 Snapshot 是 watch::Info，仅有序号、pending 和原因；[`Subscription`](../../user/services/srv_driver/src/server/subscription.rs) 90–112 行只设置 binding_changed，再由 Managed QueryBinding 取得实际状态。服务器已经在同一 actor 中持原 Binding，因此这里不需要两个远端读步骤。

目标是在 Binding Watch Query/Ack 中原子交 `{sequence, pending, terminal_reason, BindingSnapshot}`，已有 typed Protocol 可以容纳 Copy 快照，不需修改 RPC 为设备专用框架。Scope Watch 仍提供范围变化并触发弱一致枚举。订阅把有版本状态存入宿主缓存即可 Ack，Managed 据事实推进，去掉 reading/changed 的请求式交接和 Query→resume 回跳。等待/运输任务本身保留，避免为了少任务引入共享 Input 分派和隐式重入。

这比单纯删除停止轮询更完整：正常生命周期的观察结果本身就足以推进业务。验证重点是序号与快照同点读取、停止不被旧快照覆盖、Released 后仍指原轮及旧 Ack 不清新变化。

## C10：供应关闭被表达为按步骤确认，停止要求又从资源类型推断

**第二轮追加的机制收敛项，已选目标、未实施。**

[`Binding Work`](../../user/libraries/libdevice/src/binding_work.rs) 481–489 行从 DmaUse/DmaDomainControl 判断报告条件，640–685 行还要组合 resources_exposed、reported、stop_acknowledged、confirmed 和 Stop/Confirm 两种请求。真实需求是供应者已经接纳停止，以及消费访问结束这一不可逆事实；两者可以在同一幂等停止请求中单调合并。

目标登记明确的 StopRequirement；原轮 Use 的报告权由真实来源授权决定。`StopProvision(e, consumer_quiescent)` 接受累积意图，false 不能覆盖 true，首次 Stop 就能带已经成立的证明。provider 的 Prepare 效果、硬件与资源 DONE 仍由它负责。中央直接消费原 Use 报告，不再通过驱动管理者中转；普通 USB 会话由 provider 独立收束，不给 HID 物理报告权。

供应进度由原 Work 预付的单接收者通知提示，再 Query 原进度；去掉重复 Prepare/Stop 作为时钟，以及 Stop ACK→ConfirmStopped 的专有阶段。不是用一句 Closed 合并所有完成条件，也不新增参与者登记系统。

## C11：原轮出口每次重新发行，增加了不必要的未知身份

**第二轮追加的能力寿命收敛项，已选目标、未实施。**

[`srv_device Request`](../../user/services/srv_device/src/server/request.rs) 569–597 行每次 IssueUse 都 mint 新 context，另建 Use 请求/Lifetime owner，并立即丢恢复母本；647–658 行每次 IssueRegistrar 也准备新的来源。当前每个 Binding 是一个独占逻辑消费单元，一个子 Registrar 已能发布许多子设备，多次发行本身没有增加当前承诺的独立使用能力，却使回复缺失不能恢复原身份。

目标每 Binding 固定一个 Use、至多一对子 Registrar/Admin，首次提交后由原 Binding 保存母本，重复取得相同身份别名。无需额外事务 ID、恢复表或通用授权事务；通用 GrantSource 的任意派生保持自己的合同。Stopped/Released 的恢复、母本关闭和旧运输付款在目标计划中明确。

同时让硬件 Get 只经原 Use，Control 只管理和发现，使实际消费与静止报告归于同一逻辑 owner。实际 function_checks 当前经 Control Get/Report 的路径须一起迁移。多 Binding 的身份隔离保留，不能把进程级复用误写成共享一个 Use。

## 暂不建议合并或删除的机制

- **Seat / Provision / Activation / ProcessTask**：分别拥有占用、供应安全、驱动功能和执行寿命。父供应可能硬件已静止但仍须提供子轮清理服务；把 Terminal、Closed、Released、Process DONE 合成一个 Done 会丢真实区别。
- **MMIO 强制退休与 VM Frozen 交接**：当前 ForcedRetirement 复用 ChangeContext，并以访问租约处理已提交事务与整 VM 退出的交接（[`forced.rs`](../../os/kernel/src/vm/forced.rs) 265–405 行）。没有证据支持为减状态删除预付执行责任或把它交回退出中的调用者。
- **DMA 稳定保留、不可逆暴露与显式 Retire**：[`mapping.rs`](../../os/kernel/src/resource/dma/mapping.rs) 的 RETAINED 强根和 [`MappingState`](../../os/dma/src/lib.rs) 的 MayBeExposed 使 CPU/句柄消散不能触发无证据释放。首批 TrustedPhysical/无 IOMMU 的安全要求不会通过管理层简化消失。
- **ResourceTree 的分步祖先准入和退休游标**：这是协作式内核的单步工作界以及 Seal 后即时禁新准入的依据。DMA 仍保存必要祖先身份，是因为 Retire 需要核真实上级 authority（[`object.rs`](../../os/kernel/src/resource/dma/object.rs) 526–569 行），不能只为与 MMIO 相似就强行统一析构。
- **平台自 RPC**：srv_device 的 PlatformTask 确实向自身 mailbox 登记，Binding Work 又通过该 mailbox 调平台 Provision（[`server.rs`](../../user/services/srv_device/src/server.rs) 721–752 行；[`platform.rs`](../../user/services/srv_device/src/server/platform.rs) 293–370 行）。存在消掉本地运输失败面的机会，但当前共享了真实认证、付款与注册路径；只加 Local/Remote 特判会增加两套路线。本轮不把它列为立即改造项，除非后续设计能连同整段本地运输责任一起移除，并保持单一授权提交点。

## 对开放项与已通过验收的处置

| 项目 | 第二轮收敛的作用 | 本轮结论 |
|---|---|---|
| 全 hart idle / PCI 观察 | C1 可去掉无变化的常态查询和重复观察；若实际 Query 往返覆盖轮询间隔，完成后立即再发的条件也随之消失 | 有直接结构切入点；未证明历史失败首因，不能现在记通过，也不先继续八轮后的盲目采样 |
| DMA 保留＋IPI 失败组合 | DMA 保留由稳定 Mapping/Backing owner 决定，停止/IPI 由 runtime_stop 决定；分别建立不变量，再核真实连接处的组合证据 | 物理安全要求仍在。可以重新审定证据分解，但不能用“代码看起来独立”直接代替已要求的同 call 失败/消费及同映射集合证据 |
| 独立 platform clock failed | 降低无谓轮询会改变时序负载，不能据此证明时钟正确或旧异常已消失 | 保持根因未定；不因重构后未复现关闭，不将其塞进设备状态机做猜测性补偿 |
| 已通过的 root retained-entry 失败 | C5 可删除构造期异步退休条件 | 替代实现和针对性证据成立后按路径删除关闭旧专属门 |
| 已通过的 PCI Describe/Get 配对 | C4 可删除前置 Describe 及两次读取的配对义务 | 保留 OAL 公共能力验证和单次 Get 的真实 capability/metadata 核验 |
| 已通过的监督 admin ACK / join / reap | C2 在持续监督单 owner 成立后，可删除设备专用跨线程协调 | 保留 Start 前观察、正常/异常区分和唯一终局；旧专用窗口不强行移植到新结构 |
| 旧入口、多 Binding、退款 | C3 可压缩尾部持有的业务状态 | 旧轮隔离、真实引用寿命及费用守恒必须继续验收 |

验收项须按独立契约保留，按实现细节重写。只有被删除的机制及其成立条件确实不存在，才能用源码删除证据关闭其专属交错门。保留的硬件风险不能靠少跑测试消失；没有被改变的机制也不因本轮审查无条件重跑。

## 后续设计顺序与完成门

1. 优先闭合 **方向 A＋B 的逐绑定生命周期与结果交付**，以 C1/C3/C6/C7 为主要证据：一次准备由谁接纳、消息与业务谁活得更久、哪些状态已可直接消费、Released 后剩什么。目标是得到更自然的完整实现，C6 不以孤立条件补丁代替设计。C8 按 USB session 的独立完成合同评估，不为它造通用异步框架。
2. **C2 的装配执行方式**回到现有服务装配计划闭合，报告保留此 finding 和处置链接；不另建同题 todo。重点是使所有依赖等待都不阻塞唯一监督 owner，不扩张到仓库、透明重启或通用 manifest。
3. **C4、C5**可分别按资源取得和未发布出生失败的独立契约实施；它们不依赖全项目监督重构，也不被当作整体已收敛的替代。
4. 再按最终改动重画必要验证：源码/host 验证局部不变量，真实设备验证跨进程和硬件责任；只有仍依赖特定交错的风险才保留精确 GDB 窗口。原两待证和时钟异常由主计划继续维护，不复制为本报告第二份施工清单。

首轮仅交付审查判断与候选；后续修复提交与验证在本报告追加记录，原始固定对象和发现保留。

## 第二轮设计处置（2026-10-27）

用户随后要求继续寻找更轻便的替代机制并推进重构设计。主代理亲读供应状态、角色发行、来源/设备祖先谓词、观察 Protocol，以及 root 调度和同步 loader；辅助调查定位 init 的阻塞调用，外部取证比较 Genode 26.05、Linux 6.12 与明确标注滚动版本的 Fuchsia 文档。成熟系统只提供机制对照，不替代本项目的独立安全论证。

本轮裁决采用稳定逐轮 owner、完整状态观察和现有 Use 的窄报告权限直连 Binding。直连未增加参与者登记：借原认证 context、原 REPORT_STOPPED 委派来源和明确 StopRequirement 表达。Binding 是唯一停止入口，原管理者转述、StopActivation、ConfirmStopped 等路径由完整替代链删除。init 则沿既有 root await/统一推进结构迁移全部可达外部等待与 loader，满足删除门后去掉双线程协调。

| 发现 | 当前处置与目标 owner | 实施/复核 |
|---|---|---|
| C1/C7/C9 | 设备计划：完整快照、事实推进、结果与命令分开 | 已闭合；旧常态轮询/重复Activate/Query回跳已删除，末版组合验证通过 |
| C3/C6 | 设备计划：实例持唯一首次准备，业务到终态消费式移交 | 已闭合；原身份拒绝与首次QueryUse丢回包覆盖准备前/接纳后拒绝，静态检查补齐输入及来源退休责任 |
| C10/C11 | 设备计划：固定原轮出口、受限直接报告、单调供应停止及通知 | 已闭合；固定出口、直接报告、通知、实际拒权及七项回复缺失处置均有相应验证 |
| C8 | 设备计划：session 的预付单接收者完成通知 | 已接通知并保持 TD owner，热插/复用有开发证据；通知出口随真实终态释放 |
| C2 | 服务装配计划：单一 root 监督、完整等待迁移、同一 loader 分步化 | 已闭合；单root、全部已盘点存活期等待和增量loader接通，旧线程删除，最早死亡/等待/装载/停止/reset拒绝及平台组合通过 |
| C4 | 设备计划：实际 Get 一次配对资源，OAL 继续独立可用 | 已迁移实际 PCI 与 init 消费者；单次 Get 的真实能力/metadata 核验保留 |
| C5 | 设备计划：未发布空根局部回滚 | 已实施并通过原精确失败点的局部退款验证；不触已暴露 DMA 和深树回收 |

补充检查没有把“父子关系复杂”直接变成另一张依赖图。Registry 的来源/设备祖先关系同时决定准入、撤出和实际释放范围；当前保留原谓词作为单一真值，不增加供应 DAG 或镜像计数。平台自 RPC 也未加入本轮目标：加入 Local/Remote 特判而保留两套运输路径没有足够收益。

新协议的限定接口审查发现一个实质性顺序问题：自主失败若等到本地清理完成才发布 Terminal，父驱动可能等待子供应、子又等待 Binding Stop。目标已修正为立即封新业务并公布 Stopping/首错，管理者立即 Stop，清理/Quiescent/Terminal 随真实责任完成。主代理核对现有 PCI 先 Withdraw 再等子清理的路径后接受修订。NotificationSignaler 关闭不终结接收 Owner 的电平，故保留原入口 Lifetime/进程监督及有限失败路径，不把通知 CLOSED 当生产者死亡。

本轮只写目标设计和审查处置，产品源码未改。设计检查覆盖指定接口，不冒充全量代码审计或运行验收；原两待证及时钟异常保持开放。当前下一步是按主计划两条责任链实施与验证，实施进度/修复证据由本报告统一承接，不再创建同题 todo。

## 一致性重构的施工准入核对（2026-10-27）

用户进一步询问方案是否明确、能否进行一致性重构。对照设备目标、持续监督目标和本报告重新核对后的结论：**已具备进入约定范围内一致性重构的设计条件，未发现还需在两种互斥架构间选择的阻塞项**。这是设计准入判断，不是代码正确性或验收结论；本次仍未修改产品代码。

已经确定的契约包括：逐轮 owner 与唯一占用真值、固定 Use/Registrar 身份、报告授权与唯一停止入口、准备和拒绝退休、观察/命令/结果的分工、供应单调停止、USB 通知寿命、父子收束顺序、终态尾部付款，以及持续监督的完整等待与 loader 前置。原自主失败传播顺序的缺口已在目标中闭合。

施工按共同契约统一迁移：

- 设备控制链同时覆盖 libdevice/libdriver/libusb、中央管理与实际 provider、PCI/xHCI/HID、srv_driver、init 的硬件消费者及对应验收；不能只改协议一侧或留下旧命令兼容层。
- 持续监督链覆盖 RootSupervisor、管理者装配/停止、监督期间可达等待、同一 loader 与 Image 构造。loader 分步化和等待接入属于本次实现前置，不能把它们延期后仍宣告监督收敛完成。
- DMA 未发布空根回退具有独立局部契约，可以单独落地；不据此扩大修改已暴露保留、VM Frozen 或 IRQ 语义。

依赖顺序仅安排施工，不要求每个编辑步骤独立可编译或交付。两条责任链最终一起纳入既有设备集成与故障验证，C5 另作对应局部检查。新的数字 opcode、精确 wire 布局、Rust 类型拆分、按实际布局计算的额度和每个硬件 Stage 的唤醒接线是受既定契约约束的实施选择；它们不构成另开一轮架构讨论的前置，也不能被用来改变角色、停止证明或错误语义。

已有验收缺口不阻止按目标替换造成复杂度的结构，但必须保留到替代合同验证或原独立风险取证完成。实施若发现新的权限边界、责任循环或无法成立的活性前提，应回到对应设计点纠正；不能以 adapter、旁路或扩大重试维持已被否定的结构。
