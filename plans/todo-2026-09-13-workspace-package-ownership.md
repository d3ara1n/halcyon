# os/shared/user workspace 包归属整理

> 状态：独立待实施，本次设计审视只更新任务顺序，不搬迁、不改 Cargo workspace 成员和 path 依赖。此文件唯一承接共用包的归属与公共算法契约；不另建共享库整理 todo。推荐位置见 [公共操作所有权收束计划](todo-2026-09-14-public-operation-ownership.md)。

## 目标与边界

os、shared、user 三个 workspace 作为各领域包的容器：内核专用机制属于 os，用户态框架/服务属于 user，内核与用户态实际共用的可移植算法和契约包属于 shared。ABI 包和共用算法包保持独立 Rust 身份，不能因为共用便把通用算法塞进 erhino_shared 的 ABI 模块，也不复制同一算法两份。

当前 shared 是单个 erhino_shared package。实施时先审视将 shared 转为包容器所需的目录、package 身份和 Cargo 成员布局；保持内核和用户态不同 target/运行时，不把三者合成一次 target 构建。具体目录图以调查结果为依据，不由当前 path 写法反推领域。

## 现状与候选

用户态已有或新增的跨层依赖直接指向 os 下的纯逻辑 crate：

- elf、tar：libprocess 与 srv_init 使用的格式解析器；
- monotonic_id：rinlib/librpc/libsrv/libfal 的不回绕身份；
- ordered_table、timer_queue、metadata_admission：RPC、服务执行与 FAL 后端使用的索引、期限和准入。

这些包不依赖 kernel 运行时；当前问题是归属和构建入口不清，而非用户程序链接了内核。内核专用且没有用户态共用需求的纯逻辑 crate 不因“host 可测”便迁入 shared。

## 前置与触发

推荐自然位置为：运输/通用执行/RPC 闭包完成之后，内核执行结构收束与正式 FAL 后端扩展之前。共享包延期不阻塞现有消费者继续使用已成立的纯逻辑算法；实际消费者稳定后统一迁移目录与公共契约，避免与运输所有权改造同时进行。此前的独立延期不视为已经执行授权；本次仅明确接手位置，未搬包。若出现真实构建或契约缺口则先按证据重排，在唯一计划和 COMPASS 同步修改，不就地添加 path alias 补偿。

公共契约与目录同时审视：TimerQueue 的稳定 token、预付容量、park/reschedule 有独立用途，但 token 的可用位与 WaitContext 控制位目前相互约定，必须明确是公共的可标记 token 契约还是调用侧封装，不因单个消费者方便而削减所有使用者的身份空间。OrderedTable 的预付节点应清楚区分已分配存储与已预留表容量，准备后定 key 不得要求调用者理解树的内部状态。保留可证明的公共能力，不把共享目录变成内核调用者约定的集合。

## 自然实施顺序

1. 盘点 os/shared/user 的 package、实际依赖者、target/feature、host 检查和 Just 构建入口，区分 ABI、可移植共用逻辑和领域专用实现；同时审视 token 编码、预付存储/容量与取消协议，不仅按目录搬迁。
2. 明确三 workspace 的目标成员图、目录/package/crate 唯一身份和统一构建矩阵；列出全部 path、Cargo.lock、工具脚本、文档及编辑器发现路径的共同迁移范围。
3. 一次迁移每个共用包的全部内核/用户态消费者和工具入口，删除旧目录与补偿别名，不保留双轨 crate。
4. 执行三 workspace 的 host/RISC-V 检查、单测、clippy 和必要 QEMU 组合验证，确认目录迁移保持语义；任何经过论证的算法契约调整须另有明确的消费者迁移与验证证据，不混称纯搬迁。同步 AGENTS、实现导航与构建文档。

## 完成与删除门

共用包不再由用户态通过 os 目录取得；erhino_shared 的 ABI 身份与语义保留，三 workspace 的成员/依赖/构建入口一致，所有旧路径与别名删除，验证证据可定位。完成后将持久的包归属原则转入 AGENTS/notes，归档本计划并从 COMPASS 删除活跃入口。
