# os/shared/user workspace 包归属整理

> 状态：未来独立任务，本次公共 ABI/FAL 施工不搬迁、不改 Cargo workspace 成员和 path 依赖。此文件唯一承接共用包的归属问题。

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

本轮用户明确延期此整理，不阻塞当前公共 ABI 收口。公共前置/服务基座的实际共用范围稳定后再开工；若新的构建或 target 约束使延期无法继续，则先重新审视此计划，不就地添加 path alias 补偿。

## 自然实施顺序

1. 盘点 os/shared/user 的 package、实际依赖者、target/feature、host 检查和 Just 构建入口，区分 ABI、可移植共用逻辑和领域专用实现。
2. 明确三 workspace 的目标成员图、目录/package/crate 唯一身份和统一构建矩阵；列出全部 path、Cargo.lock、工具脚本、文档及编辑器发现路径的共同迁移范围。
3. 一次迁移每个共用包的全部内核/用户态消费者和工具入口，删除旧目录与补偿别名，不保留双轨 crate。
4. 执行三 workspace 的 host/RISC-V 检查、单测、clippy 和必要 QEMU 组合验证，确认整理不改变 ABI/算法语义；同步 AGENTS、实现导航与构建文档。

## 完成与删除门

共用包不再由用户态通过 os 目录取得；erhino_shared 的 ABI 身份与语义保留，三 workspace 的成员/依赖/构建入口一致，所有旧路径与别名删除，验证证据可定位。完成后将持久的包归属原则转入 AGENTS/notes，归档本计划并从 COMPASS 删除活跃入口。
