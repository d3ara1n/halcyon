# 验收 fixture 与启动期自检逐步清理

> 状态：**本轮文档校准完成，未删除fixture或自检**。2026-10-07以`refactor/system-redesign` / `f174d8e`核对现存装配、调用入口和脚本锚点；原首批清理与验证保留历史。`test_target`/`test_hammer`仍有独立进程与竞态消费者，本轮未建立等价替代覆盖，也未执行代码Review或新验收。

## 目标与边界

逐步整理`test_target`、`test_hammer`、内核启动期自检和服务内验收残留，减少不必要的资源消耗与维护面。删除前必须核定原覆盖契约、正式替代观察点及受影响装配；不能用删除锚点代替验证完成。

本轮只校正文档中的现存路径、机制名称、装配和覆盖描述，保留未知项。未调整FAL、进程生命周期或验收实现，未清理`artifacts/`、修改脚本、添加替代测试或运行QEMU。局部代码Review另按其固定计划执行。

## 当前消费者与装配

以下是源码与脚本所声明的观察点，不表示本轮已运行或重新审定这些测试。

| 对象 / 装配owner | 当前行为与观察点 | 后续删除或替代的前提 |
|---|---|---|
| `test_target` / `srv_init::launch_test_services`及按需spawn | 首实例进入acceptance Job；同一映像在pm_domain中启动另一个成员，供枚举、派生管理能力、kill与监督接管使用。`public_ipc::committed_kill`通过独立进程制造非空Close提交后的调用者消散和自动回收；stress的`test_automatic_retirement`以`retirement` payload创建256项WaitSet，Kill后观察DONE与Dead/Killed快照 | 保留独立Job归属、能力与进程消散窗口、IPC并发和非空WaitSet退休观察；复用服务的普通退出不能直接充当替代证据 |
| `test_hammer` / `srv_init::race` | 仅stress打包。HAMMER模式提供两个独立指令/回执Mailbox与Notification发令枪；TARGET模式提供独立地址空间的退出、fault、线程/映射和Tunnel等剧本。init负责竞态编排及终态、收束断言，脚本要求16/16锚点 | 先建立相同独立执行点、地址空间、故障/竞争窗口和逐场景收束证据；不能以init内串行调用替换 |

`Justfile`始终打包`test_target`，仅在stress打包`test_hammer`。`srv_init`以`RequiredLaunchSet`检查必需映像及首批启动；hammer只保留映像，由竞态剧本按需spawn。`target_image`/`hammer_image`通过`StartupImage::from_slice`借用init只读启动payload，不复制到init堆。payload由当前backing/View与init地址空间持有并随退休回收；不再使用旧root PoolBinding描述。

`tools/qemu-acceptance.sh`在公共部分要求committed IPC caller kill、监督和相关退款锚点；stress另要求256注册自动退休、control/Tunnel压力与16/16竞态矩阵。脚本拒绝`acceptance failed`及` FAILED`日志。Job子项并非每项都有单独的required锚点，不把聚合通过表述为每个内部子项均被独立断言。

相关事实入口：`Justfile::make_initfs`、`srv_init/src/{main,public_ipc,race}.rs`、`test_target/src/main.rs`、`test_hammer/src/main.rs`、`tools/qemu-acceptance.sh`。启动与映像持有的当前实现视图见[startup](../notes/impls/startup.md)。

## 当前启动期自检

`os/kernel/src/boot.rs`在bootstrap发布前依次调用以下三项。这里登记调用与断言内容，不新增保留/删除的质量结论。

| 自检 / owner | 源码中的观察点 | 当前文档处置 |
|---|---|---|
| `clock::selftest::run` / `os/kernel/src/clock/selftest.rs` | 隔离ClockState的origin偏斜、单调水印、延迟样本、不可逆ClockRange、epoch末端及嵌套失败；失败实例不污染平台时钟 | 按隔离样本描述，不再把它写成直接注入平台时钟并验证全hart停驻 |
| `MemoryPool::self_test` / `os/kernel/src/task/memory_pool.rs` | prepared子Pool回滚、delegation提交、funded owner保活来源、最后释放后的credit/库存归还，以及额度不足的错误与退款 | 补入原记录遗漏的现存自检；本轮未核定与host/用户态测试的等价覆盖 |
| `frame::selftest::run` / `os/kernel/src/frame/selftest.rs` | 全范围清零、funding失败、extent分割与两种释放顺序、回滚及来源退款 | 补入现存funded frame自检；源码来源与历史Review入口见[库存来源计划](todo-2026-09-frame-source-selftest-review.md) |

原记录中的`task/wait_set/selftest.rs`、`task/tunnel/selftest.rs`及其旧continuation、HeapPressure细目已不对应当前源码，不能继续列为待保留的现存自检。原“时钟回退、WaitSet交错、Tunnel堆耗尽”的保留判断属于旧实现基线；本轮只撤下失效的当前描述，没有追查其删除过程或判定新机制是否等价覆盖。当前机制与已有验证边界由[对象链档案](archived/review-2026-10-07-object-chain.md)拥有；若后续审查产生具体覆盖缺口，由该次唯一报告承接，不凭路径消失认定覆盖完成或缺失。

## 已实施清理与历史证据

本节保留原批次事实，不作为2026-10-07重新执行的证据。

- `shared/tar::walk`保留输入slice生命周期，init的target/hammer ELF改为借用启动payload；原sifive_u复制映像导致的`Heap allocation error, layout = 5001400`在该批解除。
- 首批删除内核堆启动自检及仅供其使用的`frame::remaining_heap_chunks`、`srv_pm`两次共40 ms的启动Sleep；竞态指令由5个word缩为3个，移除无效entry/sp字段与冗余START helper。旧system ticket等成本模型只用于追溯该批，不描述当前堆付款。
- `test_derive_kill`枚举/派生失败仍保留control进行收束，但返回失败；脚本将Job子项的` FAILED`判为失败。
- 原批次`just check`、七面`just clippy`、`git diff --check`通过；virt core/stress/release、sifive_u、virt-nofd通过，stress 16/16。sifive_u在明确reset失败后按既有方式收割。
- 原boot-failure的panic/alloc/fatal注入确认四hart停驻。验收判定收敛后`bash -n tools/qemu-acceptance.sh`、七面clippy与完整acceptance通过；没有单独注入Job失败日志的自动化用例。这些均为历史运行结果。

## 后续代码清理的完成门

后续代码任务先固定一个fixture或自检及独立覆盖契约，再核定替代观察点。需要删除时，同步Cargo、Justfile、BootPackage、RequiredLaunchSet、init映像持有、脚本锚点和文档；验证按实际受影响路线选择，不为文档修改运行实现验收。独立失败窗口无替代时保留，保留原因和重审条件写在本记录；不为删测试另造专用服务。

本轮完成门为：现状表能对应当前源码入口和脚本声明，旧机制/旧结果明确归于历史，未核定的等价覆盖保留未知，且没有代码变更、测试删除或新运行结论。上述文档范围已完成。

## 当前接力

本轮到此结束，不自动开始代码清理或Review。后续获准处理fixture时，从上表选定单一对象并先核定覆盖；获准执行库存/时间Review时从对应固定计划接手。`test_target`、`test_hammer`及三类现存自检的代码保持当前状态。
