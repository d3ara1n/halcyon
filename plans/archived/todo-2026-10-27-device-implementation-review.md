# 设备首批实现检查点独立 Review

状态：**已完成首轮机制收敛审查并转交活跃报告，本登记归档。** 本轮按用户要求从所有权、生命周期、接口、执行、成本和验证等视角比较替代机制，不作为全量安全审计通过。发现、候选、后续设计与复核由[机制收敛报告](review-2026-10-27-device-implementation.md)唯一承接；原运行证据和独立异常仍由[设备主计划的唯一收口表](../todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)维护。产品实现未修改、未重跑验收；首批设备闭包未交付。审查纪律以 [REVIEW](../REVIEW.md) 为准。

## 固定对象

- 对比基线：`549a9e78400f9e59f3bdf516ef4d54f101846178`。
- 实现检查点：`f3337ebd789a79c8cad23c419b690b702e8f96bb`（`feat(device): 接通首批设备体系并保存审查检查点`）。
- 审查差异：上述基线到实现检查点的完整提交差异，以及使其成立的实际调用链；不使用未来浮动 HEAD 替代固定对象。
- 本登记及其导航提交不包含新的产品实现。后续修复产生新提交时，在审查报告中登记，不移动本次原始审查对象。
- 原设计对象：`a0f01e2359749ab2bc2e882016919a28ea3bc9a1`，原[设计增量 Review 登记](../todo-2026-10-23-device-implementation-design-review.md)保留其固定历史范围。本次以成形实现及同步演进后的契约审查，不把旧设计登记视为新实现已经通过审查。

## 当前能力与边界

本次实现包含真实平台根与启动交付、MMIO/IRQ/DMA capability、设备登记与绑定供应、驱动选择与激活、PCI→xHCI→USB boot HID 键盘、Devices/FunctionAccess/OAL 消费者、运行期管理者监督及正常/故障收束。直接端口多键盘与同进程多 Binding、旧入口尾部、候选交接、回复 Unknown、费用与资源退休均有实现和分层验证。

当前范围是已选平台的首批 TrustedPhysical/Coherent 设备链。复合 USB 多接口、外置 hub、通用 HID、IOMMU、非一致 DMA、透明管理者重启、通用驱动仓库和完整持久化关机不在本次交付承诺内；不能仅因未实现这些扩展就判为当前缺陷，也不能以非目标掩盖已承诺责任链的错误。

这是可审查的施工检查点。两项待证和一次时钟异常仍开放；独立 Review 可以先于这些缺口的最终运行收口，不把“未取得证据”一概当成产品缺陷或直接删除验收要求。

## 阅读顺序与事实归属

1. 先读 [AGENTS](../../AGENTS.md)、[KNOWN_ISSUES](../../KNOWN_ISSUES.md)、本计划及设备主计划的[审查交接](../todo-2026-09-device-system-design.md#审查交接2026-10-27)和唯一收口表，明确本次范围与未定项。
2. 以[设备当前实现](../../notes/impls/device.md)为主线，按需读 [mm](../../notes/impls/mm.md)、[call](../../notes/impls/call.md)、[ipc](../../notes/impls/ipc.md)、[startup](../../notes/impls/startup.md)、[execution-context](../../notes/impls/execution-context.md)。实现说明和源码冲突时以固定源码为准，报告文档差异。
3. 沿实际入口读类型、授权、状态转换和最后 owner；设备主计划提供已确认契约、设计取舍和运行证据，不要求重演历史调试轮次。
4. 硬件/ABI 依据从[外部契约索引](../../references/CONTRACTS.md)及[固定平台取证](../ref-2026-10-23-device-platform-contracts.md)进入；装配和终局分别参考[服务装配](../todo-2026-10-07-service-assembly.md)、[关机与设备故障终局](../todo-2026-09-system-shutdown-orchestration.md)。

本次主代理的结构检查和验收审定是可复核的输入，不能替代本次独立审查，也不限制 reviewer 用具体证据推翻其结论。ideas 处于矫正态，目标依据按设备主计划及系统重构登记判断。

## 审查切面

| 切面 | 主要源码入口 | 要回答的问题 |
|---|---|---|
| 内核设备资源与有界工作 | `os/resource_tree/`、`os/dma/`、`os/plic/`、`os/kernel/src/resource/`、`os/kernel/src/task/request.rs` | 祖先授权、准入/封口、每步工作界、已提交责任及最后引用是否形成唯一稳定 owner；失败、取消、退出和退款能否完整收束 |
| VM/MMIO/DMA 接点 | `os/memory_space/`、`os/page_table/`、`os/kernel/src/vm/{mapping,forced,operation,change,backing,views}.rs`、`os/kernel/src/uaccess.rs` | 真实来源、基础页、发布许可、强制预付槽、Frozen 交接、空洞重用、写许可和 exposed DMA 保留是否正确；是否存在重复退休或提前退款 |
| IRQ 与执行活性 | `os/kernel/src/resource/irq/controller.rs`、`trap.rs`、`sched.rs`、`rt.rs`及既有维护/等待机制 | 每次 claim/通知有界、逐 hart 接管、Take/Ack 与 generation、等待退出、公平仲裁及全 idle 唤醒是否满足承诺 |
| 共享 ABI 与封装 | `shared/erhino_shared/`、`shared/dtb/`、`user/libraries/libkernel/` | 两侧调用号/布局/rights/结果采用一致；安全封装是否如实表达借用、映射存活、撤销和显式硬件停止前提；原 DTB 迁移及旧路径删除完整 |
| 登记、绑定、供应与公开视图 | `user/libraries/libdevice/`、`user/services/srv_device/` | Seat、来源、Binding、Frozen 资源表与任务各自的唯一真值；不可逆暴露、父子依赖、Closed/Released/退款的区别；公开流量与管理准入是否有实质资源保证 |
| 选择、进程复用与激活 | `user/libraries/libdriver/`、`user/services/srv_driver/` | Managed 与 ProcessTask 的职责、成员推导、有限候选/Retry、同轮 Unknown 恢复、激活认证及终态尾部；是否有重复状态、循环清理或无理由的抽象层 |
| PCI/USB/HID 真实消费者 | `user/libraries/{libpci,libusb,libhid}/`、`user/drivers/{drv_pci_generic,drv_usb_xhci,drv_usb_hid}/` | BME 与逐轮 function 入口、DMA/ring/TD/slot owner、预算与命令活性、物理断开/描述符未齐/取消/死亡、报告停止的可信边界 |
| 启动、监督与付款 | `os/kernel/src/boot/{bootstrap,watch}.rs`、`user/services/srv_init/`、`user/libraries/libprocess/src/{management,supervise,image}.rs`、`libthread`、`os/kernel/src/task/mailbox.rs` | prepare/start/cancel、出生前 DONE 观察、正常撤防与唯一故障终局、Job/Pool/根权限；Once 发行者付款与普通 Sender 付款的真实失败和尾部退款 |
| 验收消费者和工具 | `user/tests/{test_device,test_target}/`、`srv_init` 各 checks、`Justfile`、`tools/check-*.py`、`tools/qemu_*_gdb.py` | 测试是否来自独立契约；是否误把提示当事件、地址当身份、CPU DONE 当硬件静止、Queued 当退款；调试恢复/缓存/多 ELF/清理是否正确，工具复杂度能否在保留必要证据下收敛 |

跨切面重点看正常、失败、取消、超时、进程退出、来源撤出和释放/退款；不能只按新增文件逐个验收。长工作归属、跨 hart 锁序、用户可达 panic/unsafe 前提、容量来源以及旧路径/重复真值均属于审查范围。

## 已有验证与适用限制

| 证据 | 当前结论与限制 |
|---|---|
| 通用静态与 host | `just check`，os/shared/user host 分别 268/119/254 passed，十面 Clippy 已有通过记录；后续改动按各自定向检查记录，不宣称最终树又重跑了全部检查 |
| 通用聚合 | 首次 `just acceptance` 的 Clippy/stress 通过，release 在原外层 35 秒到期；将测试防挂死默认翻倍到 70 秒后，release、platforms、faults 续跑全部通过。保留首次 124 和续跑 0，不改写为首次整套成功 |
| 真实设备链与故障 | 正常热插拔/输入、A/B/C 同 PID 多绑定、HID 死亡、两管理者 Start/wait/Stopping、主控死亡和 reset 拒绝保留已有对应证据；直接端口多设备不等于复合 USB 多接口 |
| 资源/费用 | MMIO 许可两侧、来源/uaccess、强制槽容量、IRQ 在途/跨代/退出、DMA 四竞争窗/双 Pin/Quota/晚期安装失败、root 资金失败、Once 发行者付款等已分层取证 |
| 最新精确门 | Root retained-entry 第八轮通过；Retry 第四轮 removed-record 真实交错/线程退休通过，其余替换保护依同 actor/强 Rc/唯一 mutator 的源码论证；MMIO 第十二轮两精确 Frozen 窗口及最终收束通过 |
| 工具最终增量 | IPI 资格纯诊断异常处理在收尾阶段仅做离线验证，未因此关闭 DMA+IPI；具体最终差分、命令和指纹见设备主计划的审查交接 |

原始入口和版本边界统一见设备主计划的[唯一收口表](../todo-2026-09-device-system-design.md#当前唯一收口表2026-10-26)及各证据段。`artifacts/` 是本机保留的未提交运行材料；克隆仓库后若缺失，必须明确“原始材料不可得”，不能把链接存在、正文结论或调试器退出 0 当作重新验证。

无需为了开始 Review 重跑全套验收。选择能降低具体风险的最小检查；涉及新发现时保留原失败和版本，区分工具错误、测试消费者错误、真实产品错误及未定项。

## 必须保留的开放项

| 项目 | 已知事实 | 审查/后续责任 |
|---|---|---|
| DMA 保留与 IPI 失败组合 | 单独保留和 IPI 失败路线已有证据；组合曾有注入动作/后续发送的局部事实，原选中返回/消费尚未完整闭合。最新一次在纯诊断 RuntimeError 前停止，最后工具修复仅离线 | 评估必要门与工具边界，保留同一次调用失败/消费、后续原广播及同一映射集合的完整要求；不以 CPU 终态推 DMA 静止，不立即恢复多轮试探 |
| 全 hart idle／PCI 观察 | 未取得同一时点四 hart 的合格 idle 快照；PCI 原轮询起点与完成后的推进关系尚待有限取证，未确认轮询产品缺陷 | 检查实际 idle/工作归属/观察责任及工具扰动；不得删除安全轮询或制造状态来凑快照 |
| 独立平台时钟异常 | 一次 IRQ 验收前置出现 `platform clock failed`；一次改为全速/冷启动/仅错误点的调查未复现，无首因样本 | 未定位异常继续登记；不得据未复现宣布修复或归因某个环境变量。若审查发现具体可达错误，按证据立 finding |

这些条目的施工事实由设备主计划唯一维护；本表固定本次审查输入，不建立第二份可独立更新的施工清单。审查可以指出证据不足或合同过强，但须说明独立契约依据与替代证据，不因工具难用就降低正确性门。

## 执行与报告接力

由新上下文对固定对象进行独立只读审查，先形成发现，不边审边改。每条 finding 给出源码位置、可达触发、违反的合同、影响及建议验证方式；纯风格偏好或没有触发路径的猜测不登记为缺陷。

结构收敛以消除重复责任、重复状态、循环依赖和不必要特殊分支为目标。必要的长期领域边界不能因为当前实现只有一个消费者就删除；错误边界也不能因为已有验收而保留。候选简化方案须说明替代后的 owner、授权、失败收束和演进能力。

审查开始并形成报告后，创建活跃 `review-<日期>-device-implementation.md`，固定本次提交，统一承载本轮 findings、修复与复核；本登记链接报告后按 REVIEW 手册归档。设备主计划仍承接未完成能力与原验收门，报告通过链接引用，避免重复登记相同 finding。审查本身不授权提交、push 或扩大实现范围。
