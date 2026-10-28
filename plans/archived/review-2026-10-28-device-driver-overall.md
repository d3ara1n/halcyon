# 设备与驱动整体 Review：最新实现与机制收敛

状态：**G1–G5、C1 与 D1 已完成修复及交付前复核，所选静态、host 和真实组合验证通过，本报告归档。** 本轮交付是 `d19fdfe` 上的未提交工作树；原始发现的位置和反例保留审查时事实，最终处置见文末。精确动态覆盖缺口及一次未归因的候选出生异常由[设备主计划](../todo-2026-09-device-system-design.md#整体审查修复后的接续2026-10-28)承接，不宣称逐窗注入或异常已修复。

## 授权、固定对象与历史范围合并

用户要求把设备设计、首轮遗留 Review 和第二轮代码 Review 合并，基于最新代码整体审查设备与驱动，重点收敛机制。本轮固定提交为 `d19fdfe4d9f655d20cc8f46d8d0cb570b879f05f`，分支 `refactor/system-redesign`，接手工作树干净。范围是当前完整责任链，不限于第二轮 diff。

| 原记录 | 原范围与实际状态 | 本轮承接方式 |
|---|---|---|
| [首轮设计基线报告](review-2026-10-23-device-system-design.md) | `9ef1858..e6d11a1` 的四项设计发现已修订复核 | 保留历史；核当前发布许可、跳轮、选择 owner 和供应持权，不恢复旧设计缺陷 |
| [首批施工设计增量登记](todo-2026-10-23-device-implementation-design-review.md) | 固定 `a0f01e2`、原来尚未执行 | 用户本轮明确改变审查对象，以最新实现承接八个设计切面；不冒称对旧提交做过独立通过审查 |
| [第一轮实现收敛报告](review-2026-10-27-device-implementation.md) | 固定 `f3337eb` 的 C1–C11 已由第二轮处置 | 核替代结构及剩余接缝，不照搬旧行号或把已删除机制列成现存问题 |
| [第二轮完整交付登记](todo-2026-10-27-device-convergence-review.md) | 固定 `c5d005d`、原来尚未执行 | 合并到本轮最新代码整体审查；保留原提交和验证边界 |
| [第二轮生命周期报告](review-2026-10-27-device-lifecycle.md) | L1–L7 已修复并完成交付前复核 | 原结论保留，本轮追查稳定终态、来源退出和正常停止的剩余接缝 |

`c5d005d..d19fdfe` 的产品侧差异仅有两处注释，另有工具、配置和文档整理；本轮仍按 `d19fdfe` 实际文件取证。历史 DMA＋IPI、全 hart idle 与真实 IRQ 门保持已关闭；平台时钟与 Watch 偶发异常按 KNOWN_ISSUES 仅观察。IOMMU、非一致 DMA、外置 hub、复合 USB 多接口、通用 HID、持久化关机和驱动安装仓库不属于本轮承诺。

## G1 · P1 · 来源封口先于维护扫描时，迟到资源就绪被升级为管理者故障

状态：原发现确认；现已修复及复核，见文末处置。owner：`libdevice::binding_work` 与 Registry 的就绪提交接点。

证据位置：

- [Registry::resources_ready](../../user/libraries/libdevice/src/registry.rs) 1041–1057 行，在修改 Seat 前检查 device/source 的实际开放性；来源封口后明确返回 `Closed`。
- [Work::drive](../../user/libraries/libdevice/src/binding_work.rs) 588–592、659–669 行，只依据尚未被维护扫描改写的 Seat phase 决定准备仍可提交；先取走 Collecting、保存 Frozen，再以 `?` 传播 `resources_ready` 的拒绝。
- 同文件 518–523 行把该拒绝保存为 fault；[srv_device Request](../../user/services/srv_device/src/server/request.rs) 998–1007 行因 `has_fault` 终止管理者，root 将其升级为系统故障终局。
- [现有 Registry 测试](../../user/libraries/libdevice/tests/registry.rs) `ancestor_seal_retains_old_binding_owner_and_prevents_late_ready`（254–305 行）明确验证未 maintenance 时合法的 `resources_ready → Closed`。本轮该用例通过。

可达顺序：原 Work 已收齐供应表，准备进入 `index == count`；另一个请求先 Seal 申请来源或撤出设备祖先，准入立即关闭；维护逐项扫描尚未推进到此 Seat，原 Work 在 ready FIFO 中先获执行，仍看到 Preparing；就绪提交被 Registry 正确拒绝，却被 Work 当成不可恢复故障。维护是分步任务，服务外层的 dirty/wake 不提供它必须先于所有已就绪 Work 执行的屏障。

影响：正常取消、停止或祖先撤出可导致 `srv_device` abort，继而整机故障复位/保留；这不是供应丢失或硬件清理失败。

收敛方向：Registry 继续唯一拥有祖先准入与 Seat 提交；Binding Work 将“就绪提交已被正常停止取代”归入原轮停止转换，保有部分/完整资源 owner，继续 Stop→明确 Closed→Released。不能依赖维护调度顺序，也不能给所有 `Closed` 错误一律套成功。提交 Frozen 与 ResourcesReady 的次序应作为同一准备完成动作审阅，拒绝后不留下可再次发表的准备分支。

验证门：保留既有 Registry 前置用例；在原真实准备链的最后 Get 与就绪提交之间封来源/撤祖先，核没有 ResourcesReady、没有管理者故障，原轮安全释放且原账退款。普通成功路线不替代此窗口。

## G2 · P1 · 停止期间的在途枚举拒绝仍终止 srv_driver

状态：原发现确认；现已修复及复核，见文末处置。owner：`srv_driver::Manager` 的发现停止与 RPC 收束。

证据位置：

- [init 的正常驱动停止](../../user/services/srv_init/src/driver.rs) 544–581 行：先发送 Manager Stop，随后封 ClaimSource；Stop 接纳只提交停止意图，不保证之前发出的发现 RPC 已完成。
- [Manager::advance](../../user/services/srv_driver/src/server/manager.rs) 184–224 行：先领取原 call，Enumerate 回复只要 `header.error.is_some()` 就 `fatal(Invalid)`，没有按当前停止意图处理该操作的合法拒绝；正常停止分支在 333 行以后。
- [Registry::enumerate](../../user/libraries/libdevice/src/registry/discovery.rs) 70–78 行：来源封口后明确返回 `Closed`。
- [Manager Stop 接纳](../../user/services/srv_driver/src/server/control.rs) 311–314 行仅设置 `world.stopping` 并唤醒 Manager。

可达顺序：Manager 已发出 Enumerate；Control 任务先接纳正常 Stop 并回包；init 通过另一个运输入口 SealSource；原 Enumerate 随后在 srv_device 执行并得到合法 Closed。Manager 即使已经知道停止，仍先把这个回复当格式错误终止自身。不同 sender 的入队/Runtime 请求执行没有跨进程全序保证。

影响：正常停止被升级为管理者异常退出和系统故障终局。把停止放在下一段代码并不足以解决在途调用的收束。

收敛方向：Manager 对发现的执行与不可逆退场分别推进；Stop 生效即禁止新扫描/候选接纳，原 txid 由 Dispatcher 真实取消或领取后退休，合法的封口拒绝不启动新业务也不触发故障。Scope Watch 的撤销同样进入该转换。运输、帧格式和身份错误仍按各自合同处理，不将所有失败转换为停止成功。

验证门：原 Manager 的 Enumerate 在途时接纳 Stop，先完成来源封口再放行原请求，核返回正常停止、旧 Binding 均 Released、Worker 与两管理者正常收集；同时核非停止期的畸形回复仍拒绝。

## G3 · P1 · Released 先于首次 Watch 时，稳定控制壳失去终态观察出口

状态：原发现确认；现已修复及复核，见文末处置。owner：Binding 回执、Binding Watch 准入及 Managed 初始观察。

证据位置：

- [Binding::snapshot](../../user/libraries/libdevice/src/binding_work.rs) 103–109 行已能从稳定 Shared 读取保存的 Released；同文件 598–619 行在释放 Seat 后保存该回执。
- [WatchBinding](../../user/services/srv_device/src/server/request.rs) 679–687、856–887 行取得这个稳定 Binding，却仍把创建交给 Registry。
- [Registry::create_watch](../../user/libraries/libdevice/src/registry/discovery.rs) 128–134 行再次要求 `binding_snapshot(reference)` 存在；[Seat::release](../../user/libraries/libdevice/src/binding.rs) 237–246 行已经删除 active。Registry 因而返回 `StaleEpoch`，即使调用者持有正确的旧 Control。
- [Managed](../../user/services/srv_driver/src/server/managed.rs) 151–164 行在 Claim 回包后才建 Watch；485–504、366–380 行将 Watch 拒绝转为停止并等待 Released。唯一设置 `run.released` 的是 313–323 行的 Binding Watch 快照；没有该观察便最终进入 342–350 行的停止超时故障。

可达顺序：Claim 已提交并保有 Control；provider 很快明确拒绝 Prepare，或另一个合法停止先完成，Work 安全释放原轮；管理者随后第一次 WatchBinding。原轮没有任何遗留硬件责任，却因为订阅只接受活动 Seat 而不能取得终态。现有用例覆盖“先建 Watch 再 Released”，未覆盖这个相反顺序。

影响：合法的快速准备拒绝/取消可演变为 30 秒停止期限后的 `srv_driver` abort；候选选择也无法安全进入后继尝试。

收敛方向：原 Binding 稳定回执应成为该轮观察的依据。活动占用仍由 Seat 唯一拥有，Released 回执仍由原 Binding 持有；新建观察必须同时覆盖这两种寿命，终态订阅不能依赖已经退休的活动 Seat 或可回收的来源/设备表项。保留真实认证、原轮身份、最后别名和已接请求的费用，不按裸 BindingRef 自造终态，不恢复定时 Query 作为补丁。实现时应同时检查 Control、Use、Watch 的身份保留与付款边界，避免只放宽一条 `create_watch` 检查。

验证门：原 Control 持续存活，先取得真实 Released，再创建 Watch，初始快照立即给原轮 Released，Ack/退出完成；新 epoch 不影响旧回执。真实快速 Prepare 拒绝后 Managed 正常 Detach/Retire，不等停止期限或触发系统终局。

## G4 · P1 · 枚举中断时依据描述符选择尚未配置的 endpoint

状态：原发现确认；现已修复及复核，见文末处置。owner：`libusb::xhci::Device` 的命令/TD 与停止状态。

证据位置：

- [Device 枚举](../../user/libraries/libusb/src/xhci/device.rs) 443–503 行：WaitConfig 解析并保存 keyboard，清原 transfer 后进入 SetConfig；SET_CONFIGURATION 完成后进入 Configure，后者才提交实际 Configure Endpoint 命令。
- 同文件 117–139 行的 stop 对已有 slot、非 Address 且无在途命令的阶段统一选择 StopEndpoint；505–515 行在 `transfer=None` 时直接用 `keyboard.dci()` 选端点。
- 同文件 574–575 行将非 Success 命令完成返回 Unavailable；[xHCI Engine](../../user/drivers/drv_usb_xhci/src/server/engine.rs) 246–260 行把错误升级为激活失败，若已在停止则直接 fatal。失败的清理命令已被取走而阶段仍为 WaitStopEndpoint，不能再推进到 Disable，后续停止期限亦会触发故障。

可达顺序：WaitConfig→SetConfig，或 WaitSetConfig→Configure 两个间隙，`keyboard=Some`、`transfer=None`，interrupt endpoint 还未建立；真实拔出或父 Binding 停止令 update_intent 调用 stop，向描述符给出的未启用 endpoint 提交 Stop Endpoint。该清理命令被拒绝，原本可正常收束的局部连接升级成主控失败，影响其他连接和整个系统。

外部事实：独立子审查直接读取本地 QEMU 仓库的固定 `v11.1.2` 对象，[hcd-xhci.c 的 xhci_stop_ep](https://github.com/qemu/qemu/blob/v11.1.2/hw/usb/hcd-xhci.c#L1339) 对不存在的 endpoint 返回 `CC_EP_NOT_ENABLED_ERROR`；非 EP0 endpoint 由 Configure Slot 路径实际建立。停止和 Disable 的独立前提按[固定 xHCI 1.2b §4.6.4/§4.6.9/§4.6.10](../ref-2026-10-23-device-platform-contracts.md#取消停止及内存释放)核定。主代理亲核实际状态和错误传播，未把一般 USB 描述符知识当成配置完成证据。

收敛方向：停止从原命令/TD/endpoint 的实际硬件责任决定下一动作；在途定向命令先收完成，实际活动 TD 才进入对应的 Stop/SetDequeue。没有未完成 TD、相关事件已消费且满足 Idle 前提时可以走 Disable Slot。保留真实端点状态的唯一 owner，不新增从 `keyboard.is_some()` 推导的镜像标志，也不吞掉任意非 Success 命令完成。

验证门：复用真实 QEMU/GDB 热插路线，在上述两个窗口 QMP 删除；核不向未启用端点发命令、slot/DMA 正常退休、主控存活；双连接时另一键盘继续输入。此次未执行这两个窗口。

## G5 · P2 · Page 析构吞掉 CPU Unmap 失败，局部清理丢失 owner

状态：原发现确认；现已修复及复核，见文末处置。owner：`libusb::xhci::Page` 及 Device/Controller 退休。

证据位置：

- [Page::close_cpu / Drop](../../user/libraries/libusb/src/xhci/memory.rs) 129–142 行：Unmap 失败先把 MappedRegion 放回 `self.cpu`，但 Drop 随即忽略返回值并销毁 Page。这是 close_cpu 的唯一调用者。
- [Device::Retire](../../user/libraries/libusb/src/xhci/device.rs) 537–555 行只等 DMA mapping DONE 后即进入 Closed；886–893 行的断开删除和 926–929 行的重建会销毁旧 Page。Controller 的 [Retire](../../user/libraries/libusb/src/xhci.rs) 493–498 行同样只等 DMA。
- [MappedRegion::unmap](../../user/libraries/libkernel/src/mm.rs) 188–195 行在错误时返回原 token，类型没有替代清理 Drop。[VM modify](../../os/kernel/src/vm/operation.rs) 498–508 行仍须为操作和边界承载分配，Quota/OOM 可发生，不能把普通 Unmap 当作必成。

可达顺序：设备已真实 Disable Slot、逐页 DMA DONE；删除已 Closed 的连接时，某页 CPU Unmap 提交前分配失败。Page 析构吞错，CPU View 继续持 backing 与费用，而用户态原映射几何 owner 已丢失。xHCI 进程跨热插继续存活，因此局部泄漏可一直保留到该进程退出；上层却已经得到局部 Closed。

影响是释放责任和费用丢失，不是已经证明提前 unpin 或 DMA 越界。普通成功退款路线没有覆盖这个失败点。

收敛方向：把 CPU Unmap 纳入原 Page/Device/Controller 的显式退休，按 DMA DONE→CPU Unmap→释放页 owner 推进；失败保留 Page 并交原 Engine 的失败政策，不在析构中静默消失。构造失败和未暴露页同样必须有显式收束或明确的进程失败兜底。复用现有 owner 和 `close_cpu`，不新增清理运行体。

验证门：真实连接退休期间在一笔 CPU Unmap 的提交前分配注入失败，核 Page/映射 owner 保留，后续成功清理或明确故障退出承担退款；在错误未收束前不发布局部 Closed。

## C1 · 分步装载尚未接入运行期驱动进程 owner

状态：原结构候选已获授权实施并完成复核，见文末处置；初次审查未观察到一次由此触发的运行故障，不将其称为内核无界执行。

[ProcessTask::advance](../../user/services/srv_driver/src/server/worker.rs) 369–399 行在一次 Runtime 推进内执行 `Worker::spawn`；79–169 行同步映射映像并调用 `libprocess::spawn`。后者由 [management.rs](../../user/libraries/libprocess/src/management.rs) 247–268 行循环驱动整个 Preparation，错误时同步 cancel/collection。实际新 worker 的 DONE 来源到 spawn 已 Start 后才在 198–203 行安装。

同一 srv_driver actor 同时拥有其他进程 DONE、Managed、订阅、Control Stop、运输与 deadline。运行期插入需要启动一个新 worker 时，这些责任在完整 ELF 装载与失败回收结束前均不能推进。root 自身的监督仍在工作，但不能代替 srv_driver 处理其内部活跃 Binding。已存在的 [root loader 接线](../../user/services/srv_init/src/supervisor/loading.rs) 和公共 `Preparation` 提供了复用基础；第二轮仅把 root 的阻塞装载迁移，不能由此宣称所有驱动管理运行期推进也已分步。

建议把映像借用、Preparation、未启动 PreparedProcess、运行期 Control、失败 Collector 纳入原 ProcessTask 的出生/运行/回收寿命；每次只推进已有 loader 的一步，取消交同一 Collector，预先附着 DONE 后再 Start。不创建新的 loader、线程或驱动运行体，不复制 root 的业务脚本。映像映射的 owner 必须覆盖整个准备期，不能用自引用借用或临时裸指针逃过 Rust 生命周期。

验证门：既有活跃 Binding 运行时装载另一 worker，在装载未结束前可处理原进程 DONE 和 Stop；取消/失败仍归原 Job/Pool/Collector，覆盖未 Start、Grant 已消费和开始运行后的三个边界。不得只把超时数字调大作为修复。

## D1 · 现行设计与实现说明仍残留相反合同

状态：确认文档偏差，本轮已校正设备主计划与 device impls；不修改产品行为。

- 设备主计划第二轮规定固定、可恢复的原轮 Registrar，但“启动与枚举”仍写 `IssueRegistrar 不幂等`；实际 Source::admit 已恢复同一对子。
- 主计划“激活与功能关联”仍写准备失败报告 Terminal，第二轮及 Activation::fail 已规定先发布 Stopping/首错，防止父子清理相互等待。
- device impls 把 Installing/Removing 统一解释为“Runtime 登记回执不自动唤醒”。当前 `Runtime` 的成功 Register 回调确实不自动排队（runtime.rs 1209–1215），但实际 Remove 成功调用 unregistered 后明确 schedule（817–837）。文档应分别说明这两种事实；不据此把所有待登记状态改为 Parked。

这些是现行依据中的错误叙述，不能由“历史过程”解释；历史归档报告保持原貌。校正不等于产品行为已修复。

## 建议保留的领域边界

- Seat、Provision、Activation 和 Process 分别拥有占用、供应安全、驱动功能与执行寿命，完成条件不能合并成一个 Done。
- 固定原轮 Use/Registrar、受限直接静止报告和不透明功能关联已经减少跨层转述；继续沿这个方向闭合终态观察与停止，不恢复 StopActivation、ConfirmStopped 或重复 Activate 查询。
- DMA 的显式退休/暴露后稳定保留、MMIO 强制退休与 VM Frozen 交接仍有独立安全理由；不能为精简类型改成句柄 Drop 推导硬件静止。
- Registry 的来源/祖先关系承担准入与停止集合的唯一事实，不另建镜像供应 DAG。平台自 RPC 暂无必须分裂为 Local/Remote 双实现的证据。

## 覆盖与保留判断

| 责任切面 | 本轮检查与结论 |
|---|---|
| 来源授权、Seat、Provision、绑定准备/停止 | 主代理沿实际入口和提交点核查；G1 是准入事实与异步维护之间的错误分流 |
| 固定 Use/Registrar、激活、功能与观察 | 核认证、实际别名、结果与观察、终态尾部；G3 是稳定回执与 Watch 出生的缺口，旧 C/L 替代机制继续保留 |
| 驱动选择、复用、进程、root 监督 | 核 Manager/Managed/ProcessTask、root 的 Start 前观察和正常/故障交接；G2、C1 承接剩余接缝 |
| 资源树、DMA、MMIO/VM、IRQ | 独立子审查深入读取准入、稳定保留、退休、发布许可/强制退休、PLIC Take/Ack/撤销及对应封装，未新增有充分证据的可达缺陷；不等于全量内存/锁/硬件安全证明 |
| PCI/xHCI/USB/HID 消费者 | 核实际配置/命令/TD/slot/Page 责任与上层退出；G4/G5 经主代理亲读复核；HID Watch 全部尾部交错未逐项审计 |
| ABI、平台与启动 | 依据 shared/libkernel、固定平台合同及现存根交付接点核边界；未逐 syscall 编解码穷举、未全面重审 DT/PLIC parser、boot 全部失败退款和通用 VM planner/shootdown |
| 验收与工具 | 复核路线所声称的合同、版本和已关闭门；没有逐行审计全部 GDB/Python 工具，也没有把旧运行成功外推为本轮窗口已覆盖 |

这是一轮跨领域责任链和机制审查，不是逐指令形式证明或全部平台认证。前两份待办登记合并为本报告的修复入口，不把它们分别标成原固定版本“全部通过”。

## 初次只读审查的验证与证据边界

主代理亲读 Registry/Seat/Binding Work、来源/角色/Watch 宿主、Activation/BindingTask/Managed/Manager/ProcessTask、root 监督/等待/loader、公共 Runtime 回执接点，并核历史 C/L 处置及第一轮设计责任。内核资源与硬件消费者使用一次独立只读子审查；其两项发现经主代理亲读 Device、Page、Engine、MappedRegion 和内核 Unmap 准入复核后纳入 G4/G5，未再启动重复 reviewer。

本轮运行：`cd user && cargo test -p libdevice -p libdriver -p libexecution --target aarch64-apple-darwin`，退出 0，103 项测试与 2 项 doctest 通过。完整日志：[host.log](../../artifacts/device-overall-review/host.log)。未新增测试、未修改产品代码、未运行 QEMU、check 或全量 Clippy。G1–G5 的反例依据当前源码、调用合同及固定 QEMU 行为，尚未动态注入；host 通过不消除这些发现。

文档交付检查覆盖统一报告、两份搬移登记及其余改动新增行的 78 个本地链接、18 个标题锚点和新文档代码围栏，全部通过；`git diff --check` 通过。设备 impls 同步描述当前停止/观察、同步出生及 Page 清理的真实行为限制。原归档报告只调整搬移登记的链接，不改历史发现或运行结论。

## 初次审查的修复安排（历史）

后续收敛按共同责任安排，不逐 finding 叠补丁：

1. **原轮停止与终态观察**：G1/G2/G3 一起闭合“停止取代原操作、原结果仍有稳定出口”。Registry 保留唯一准入/占用，Binding 保留原轮回执，Manager/Managed 统一退出在途操作；不恢复旧轮询、第二种停止命令或另造恢复账本。
2. **实际硬件与页退休**：G4/G5 一起闭合 Device 从任意枚举阶段退出的命令/TD/slot/Page 责任，显式退休贯通 DMA 与 CPU 映射。正常拔出/取消可局部结束；无法证明硬件安全或清理失败才升级。
3. **运行期出生与监督**：C1 在原 ProcessTask 中接同一个分步 loader 和 Collector，避免创建其他 worker 时阻塞既有职责；与上述真实双连接及停止路线组合验证。

实施前仍须在设备主计划和服务装配计划中写清目标状态、提交点、输入/来源 owner、取消交接及删除门；本报告的候选方向不冒充已完成的详细设计。findings 的处置、验证与复核只更新本报告。不得把 G1–G5 的当前正确性欠账延期为未来扩展；C1 如另排，须登记具体运行期推进边界、owner、触发和验证门，不能只留一句“性能优化”。

当前没有半重构产品代码；本次交付为审查报告、登记归并和 D1 文档校正。后续产品修复回填 impls，执行 `just check`、`just clippy` 及上述必要真实消费者组合，未覆盖窗口按其真实证据分类保留。提交与 push 均未授权。

## 实施记录（2026-10-28）

用户授权 G1–G5 与 C1 后，按三条责任链统一实施。设备目标见主计划本轮修复设计；C1 输入所有权与出生状态见服务装配计划运行期分步接线。接手保留上一轮审查文档改动，产品基线仍为 d19fdfe；此处追加实施事实，原始 findings 与原验证保持历史。

### 已实施结构与开发检查

- G1：Frozen 候选仅在 Registry 就绪提交成功后发表；该提交点的 Closed 进入原轮停止，未改变其他错误的故障政策。
- G2：Manager 在消费发现调用前进入不可逆退场，cancel/take 后才放开原运输；合法的枚举 Closed 也进入同一停止，来源注销后释放 Scope Watch。
- G3：终态 Watch 使用认证 Shared Snapshot 与原角色 Funding，直接产生 Released 初始回执；不保活已退出的来源/设备/供应记录。新增 host 回归在设备记录退休后创建终态 Watch；既有 function-session 消费者增加真实 Released 后 Watch/Ack 检查。
- G4/G5：Stop/SetDequeue 只取原 TD 的 dci，没有 TD 则在定向命令结束后 Disable；设备页与主控页均在 DMA DONE 后显式解除 CPU 映射。Disable 后的待领结果消费为独立字节回执，页退出不再使结果读取悬空；Unmap 失败保持 Page 并上交 Engine，构造期析构失败进入进程终局。
- C1：ELF/ISA 跨步状态改为范围/游标，Validation 持输入 owner；ImageView 独立保活映射。借用与拥有输入沿同一 Preparation；OwnedGrant 在 ProcessGrant 成功的同一步解除本地关闭责任。ProcessTask 持 Preparing/Prepared/Supervised/Empty owner，输入清理与 DONE 附着成功后才 Start，取消仍由原 Collector 收束。
- 用户态目标开发检查通过。首次检查发现 Collector 借用入口可见性及旧 Worker.control 访问，已同步修正。Clippy 首次拒绝新增测试的临时 Vec 指针表达，改为先保留实际返回 owner；后续拒绝 ProcessOwner 的大小差异，按 Task 出生预付最大状态、Start/取消移交不得再分配的理由记录局部 expect，未加入新的分配失败面。两次原始失败日志保留。
- `just check`、十个分面的 `just clippy` 通过；用户态相关 182 项 host 测试及 2 项 doctest 通过，ELF 的 21 项 host 测试通过。测试不以当前输出生成预期：新增用例分别依据终态控制回执和拥有输入可移动/失败返还的独立契约。
- QEMU core、reuse、candidates、function-session 与 root loading 故障组合分轮执行，最终结果见文末；validation-status.json 保存最终退出码，validation-versions.json 区分实际验证源码版本，较早 source-version.json 只代表初始开发版本。

### 交付前检查与精确证据边界

主代理复核了取消前后 GrantOutcome、Collector/PreparedProcess 唯一关闭权、Start 前来源回执、停止期间迟到调用、终态 Watch 付款与去引用，以及 Page 退休前结果移交。终态观察曾比较过延长 Registry 身份引用的候选，因其间接延长供应 payload 寿命而舍弃；现行独立终态回执是完整替代，不保留过渡路径。新增代码没有内核锁或硬件 ABI 修改，ISA 解析保持既有推进粒度。一次限定新输入/进程 owner 与终态/页退休接点的只读交付前检查已完成，没有新增可达 finding；主代理亲核其所有权和顺序结论，运行证据仍以下节结果为准，不由此宣告未执行的动态窗口已通过。

G1/G2 的精确多任务时序、G4 两个指定枚举阶段的拔出、G5 的 Unmap 分配失败和 C1 的每个取消窗口，现有工具没有完整的直接触发/判据。当前以提交点/owner 的源码论证、纯逻辑测试及真实组合验证分层取证，不声称逐窗注入；不为凑窗口制造测试专用服务或修改客体状态。剩余精确动态覆盖由[设备主计划](../todo-2026-09-device-system-design.md#整体审查修复后的接续2026-10-28)登记 owner、触发和验证标准，不重复普通成功路线代替缺失窗口。

### 组合验证发现的就绪通知接点

首轮多绑定路线退出 124，原入口 `artifacts/run/20261028-093508-41934-acceptance-device-reuse.log`，客体现场 `artifacts/failed-acceptance-20261028-093520-42321.log`。原 PCI worker 在约 58.3s 已认证 bootstrap，Managed 仍停在 WaitBoot，约 65.2s 准备期限到达后停止；设备客户端随后失败并进入 root 故障路径。该失败不是有界业务仍在推进时撞验收上限，故没有扩大路线或业务期限。

源码定位：Worker.boot 的布尔结果原来只在 bootstrap 来源已经清空时为 true；来源回执清空后，ProcessTask 的调用条件又跳过已经有 entry 且来源为空的 boot。就绪事实已存在，却没有给已停驻 Managed 的可靠唤醒。修复让返回值只表达本次认证提交了 entry；ProcessTask 在同一步将关联唤醒游标置位，来源继续独立注销，不重复发布就绪。该修复保留实际 Process Running 核验、PID/context/邮箱认证和单调错误状态。

先前 core 通过只作为该中间版本开发证据，未把它当成完整硬件激活成功。就绪修复后的源码版本逐路线记录于 validation-versions.json，首轮失败汇总保留于 validation-first.json；末版继续执行 reuse/candidates/function-session/loading，每条路线不借用旧版本通过结果。

就绪修复后的第二轮走到 PCI 枚举完成和 Activation Ready，但 InstallFunction 在原 Claim→功能安装的 20s 总期限处到期。客体原现场保存为 `artifacts/device-overall-review/reuse-second-live-failure.log`，对应入口 `artifacts/run/20261028-094601-50567-acceptance-device-reuse.log`；确认业务失败后主动结束专有运行，退出 143，并核 QEMU/操作者清理，未把它改写为超时成功。

本轮分步装载允许其他职责持续推进，正常 PCI 完整准备已实测接近原总窗口。Managed 的一次绝对部署准备窗口据此从 20s 调为 40s，留同量调度余量；单次 RPC 5s、worker 出生/boot 窗口、停止期限和硬件规范期限均未扩大，Stopping 仍不可逆，期限不随回复刷新。第三轮 reuse 重新构建并完整通过，具体版本及日志见下表。

### 最终组合验证与版本

| 检查 | 结果及证据 |
|---|---|
| 静态与 host | `just check`、最终十分面 `just clippy` 退出 0；用户态相关 182 项 host 测试及 2 项 doctest、ELF 21 项 host 测试通过。日志位于 `artifacts/device-overall-review/` 的 check-fix.log、clippy-final.log、host-fix.log、host-elf.log |
| 多 Binding 复用与故障退出 | `THROTTLE=100 QEMU_ACCEPTANCE_KEEP_LOG=1 just acceptance-device-reuse` 退出 0；[入口](../../artifacts/run/20261028-095157-57895-acceptance-device-reuse.log)。A/B/C 复用、旧别名、HID 实际故障、Released、进程收集与整体收束通过 |
| 候选拒绝、取消与重插 | 同环境 `just acceptance-device-candidates` 最终退出 0；[入口](../../artifacts/run/20261028-100809-76781-acceptance-device-candidates.log)、[客体](../../artifacts/qemu-acceptance-20261028-100815-77195.log)。错误实现拒绝、准备中取消、同端口重插及后续 HID 输入通过；此前一次失败保留于下节 |
| 功能会话与迟建 Watch | 同环境 `just acceptance-device-function-session` 退出 0；[入口](../../artifacts/run/20261028-101507-83215-acceptance-device-function-session.log)、[客体](../../artifacts/qemu-acceptance-20261028-101512-83616.log)。客体第 184、202 行分别记录两轮真实 Released 后首次 Watch/Ack 成功，整轮 core/shutdown 通过 |
| root 装载期间监督 | 同环境 `just acceptance-device-manager-fault Driver loading` 退出 0；[入口及故障证据](../../artifacts/run/20261028-101706-86975-acceptance-device-manager-fault-Driver-loading.log)。实际 Driver 管理者故障后，root 封来源/平台根、收集执行 Job 并确认 SystemFailure cold reboot。这验证公共 loader 改动后的 root 监督，不替代 C1 拥有输入的每个取消窗口 |

最终 candidates/function/loading/Clippy 的本次改动产品源码与交付仅差 Preparation 的一行说明注释（补明借用与拥有两种输入），逐路线指纹见 [validation-versions.json](../../artifacts/device-overall-review/validation-versions.json)，交付比对见 artifacts/device-overall-review/delivery-version.json。reuse 另差 Preparation Debug 和 worker 出生/Start 失败诊断，行为修复一致，没有重复该已通过路线。较早 core 成功只算中间版本开发证据；最终 function-session 的正式 core 聚合通过另列。检查完成后核对本轮 QEMU、操作者、GDB 和验证 runner 均已结束。编译保留平台 DTS/linker 既有警告及 function-session 特性组合的未用 wait import 警告，退出码和上述运行判据均通过。

### 候选路线的一次未归因出生异常

第一次 candidates 退出 124，保存[入口](../../artifacts/run/20261028-095827-68209-acceptance-device-candidates.log)和[客体](../../artifacts/failed-acceptance-20261028-095839-68665.log)。在此前拒绝/取消/重插之后，HID process 5 尚未记录 Start，就使原 Run 以 Unavailable 停止；PID 16 随后被收集，原 Pool 的 pages/metadata 均为 0，供应完成 Closed。旧现场没有具体失败分支、准备阶段或出生 deadline，不能确定是超时、Start 拒绝还是其他失败。

末版只增加原出生 owner 的阶段/GrantOutcome、deadline 和 Start 错误诊断，未改变 10s worker 出生窗口。同一 candidates 完整重跑通过，七个 worker 从准入至 bootstrap 约 1.2–2.3s，没有触发新诊断。一次通过不证明原异常消失，也不将其归因于日志修复；按[主计划观察项](../todo-2026-09-device-system-design.md#候选出生异常仅观察)保存事实和复发取证条件，不增加预防性分支或反复压力测试。

### Findings 复核与交付结论

| 条目 | 最终责任链与关闭依据 |
|---|---|
| G1 | Registry 仍唯一裁决就绪；Work 只在成功后发表 Frozen，提交点 Closed 直接进入原 Stop。源码覆盖 maintenance 先后两种顺序，既有来源封口回归通过；精确外部交错未注入 |
| G2 | Stop 先于发现结果消费；旧 txid 真实 cancel/take 后退休，合法 Enumerate Closed 单调停止；真实组合的两管理者收束通过，指定 RPC 交错未注入 |
| G3 | 认证原 Control/Use 的 Shared 终态配原 Funding 创建 Watch，不依赖退休 Registry 身份。设备记录退休后的 host 回归和两轮真实 Released→Watch→Ack 均通过 |
| G4 | 无 transfer 的两个枚举间隙不再推导 descriptor dci；定向命令结束后 Disable，有 transfer 时才 Stop/SetDequeue。候选取消/热插与复用通过，两个精确阶段未定向拔出 |
| G5 | Page 显式完成 DMA DONE→CPU Unmap 后才放行退休；失败保存映射并交原 Engine。未领取结果先转独立回执，构造期析构失败进入进程回收；正常热插/退出通过，Unmap 分配失败未注入 |
| C1 | 同一个 Preparation 持拥有输入与可移动游标，Grant 成功原子移交本地责任；原 ProcessTask 分步推进、DONE 实际登记后 Start、取消交原 Collector。host、实际新 worker/候选取消及公共 root 故障路线通过；逐阶段取消覆盖由主计划保留 |
| D1 | 现行设计与 impls 的 Registrar、Stopping、Register/Remove 叙述已与代码核对 |

最终结构检查覆盖授权/付款、单一 owner、正常/失败/取消/超时/释放和旧路径清除；就绪唤醒修正与一次绝对总窗口由主代理追加复核。共享内核 ABI、锁序及内核执行模型未改变，无须为本轮增加内核锁审查或重跑既有平台专属门。同步 worker spawn 路径、描述符推导停止端点、吞掉 CPU Unmap 错误、终态 Watch 必须活动 Seat 等旧行为已替换；没有暂留 adapter、第二个 loader 或重复供应图。impls 已回填实际结构，ideas 未修改。

当前承诺的修复已完成；覆盖限制和未归因观察不被改写成运行通过。交付在本地工作树，未 commit/push。提交后按 REVIEW 登记固定交付的独立审查，范围为本轮 G1–G5/C1、组合发现的就绪接点、准备窗口和消费者；不能用本次交付前检查替代该审查。继续维护的入口由设备主计划拥有，本报告保存当时证据。

文档收口检查覆盖本轮新增/改动内容的 95 个本地链接和 28 个标题锚点，全部通过；结果保存于 artifacts/device-overall-review/document-final-check.json。`git diff --check` 通过。唯一后置源码编辑是一行 Preparation 输入说明注释，交付指纹已单独记录，不因此重复已通过的编译或 QEMU。
