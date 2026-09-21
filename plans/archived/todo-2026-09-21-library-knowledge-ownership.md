# 用户态库知识归属、依赖与目录重排

> 状态：L0–L5 已完成并归档，实现提交为 `96ee03b0d1641c86ed8ab05951bad6954ea84db4`（`refactor(user): 重排用户态库归属与命名`），父提交为 `2124413`。代码、文档、命名、结构 Review 与组合验证均已收口；固定提交审查见 [`library-knowledge-ownership-review`](../todo-2026-09-21-library-knowledge-ownership-review.md)。用户态共同命名规则见 [`user/README.md`](../../user/README.md)，库领域与依赖规则见 [`user/libraries/README.md`](../../user/libraries/README.md)。

## 接手基线与任务边界

- 基线分支 `task/fal-service-capabilities`，固定代码提交 `dfcf7a349fe6d9e2836bb7c8179ff7e96c8ce20a`（父提交 `84eeed6`）；F1–F3c 的代码、文档、装配及本专题原则/计划已整体保存。固定该基线的未来审查见[基线 Review](../todo-2026-09-21-fal-library-baseline-review.md)。接手期间保留基线之后的全部工作树变化，未通过回退或重建 checkout 丢弃迁移成果。
- 接手入口曾为本计划、[COMPASS](../COMPASS.md) 与 [FAL 交接](../todo-2026-09-fal-service-capabilities.md#跨会话接力断点)；本计划归档后不再拥有新施工。
- 本计划拥有现有库的知识归属、类型与依赖图、全部真实消费者迁移、目录迁移、用户态 crate/binary 命名规则和相应文档收口。FAL 总计划继续拥有 F3d–F4 的业务实现与整体验收，执行前置计划保留既有机制的交付事实，不重复安排本任务。
- 不在本任务中实现服务注册/发现、Open、流 Copy 或新服务拓扑；不因库分层新增内核服务类型或权限等级。尚未实施的服务框架能力保留其领域归属与接缝，具体消费者随 F3d 设计，不用空 facade 或伪消费者宣称交付。
- 组件命名按 `user/README.md` 的统一规则：单个普通领域词使用完整名称，多词领域使用公认缩写或正式专名；`srv_`、`drv_`、`test_` 只作为二进制角色前缀。讨论中的 `libproc`、`libsrv`、`libdrv` 不构成保留或新建这些缩写库名的理由。

## 目标契约

1. 库按领域集中知识。服务、进程、FAL、RPC、路径访问、流、驱动以及公共记账/执行各有明确拥有者；管理与消费接口不因调用者角色不同而复制领域类型。
2. 知识由库提供，操作权来自 capability，进程角色由组合形成。普通进程使用进程能力不引入服务角色；pm 组合服务与进程领域，并凭授权执行管理。
3. 依赖从使用者指向知识提供者。未来 `libservice` 可以消费 FAL Record 和 RPC；`libfal`、`librpc`、`libfs`、`libprocess`、`librunnel` 不反向依赖服务框架或预定义服务规则。通用回调、重导出和类型参数不能隐藏上层语义。
4. 通用记账拥有专门的知识库，执行与领域库共同消费。账户、额度、预留、退款保持单一实现；领域资源分类和授权政策由领域或装配提供。通用执行单独拥有任务、观察、期限与唤醒，不能把记账再次附属于执行库。
5. `rinlib` 只承担基础运行环境与内核契约的安全封装；内核/ABI 的既有公共类型保持唯一真值，用户态服务含义不进入内核、共享 ABI 或 rinlib。
6. 原 `user/frameworks/` 下的全部库迁入 `user/libraries/` 后仅保留一套路径和构建身份，无旧目录兼容层、别名或复制实现；库 README 随目录迁移，所有有效入口同步更新。`rinlib` 保持独立的基础运行环境位置。
7. 用户态 Rust 组件身份与领域名称一致：普通单词写全，多词使用公认缩写，正式专名保持正式拼写；不因库名前已有 `lib` 就再把单词 `service`、`driver` 缩为 `srv`、`drv`。空 `libdrv` 不改名为另一空 facade，而是在 L5 删除；未来真实驱动公共库使用 `libdriver`。

## 已确认的现状与责任落点

以下是立项调查基线，实施前需追踪完整的类型、owner 与调用关系，不以包名替代语义审计。

| 现状 | 位置与真实消费者 | 目标归属及必须保留的责任 |
|---|---|---|
| 通用账户/额度归在服务包 | `libsrv/budget.rs`；libfal 后端、授权、节点、值、流数据，Runtime，srv_init/srv_pm/srv_fs | 独立公共记账库；账户来源、双层限额、预付、Charge 持有、shrink/refund、失败保留与实际释放后退款不变 |
| 底层已有记账原语 | `shared/metadata_admission`；内核及用户态消费者 | 审计其与公共账户模型的职责，明确复用、合并或分层的独立依据；不复制 Counter/Permit，也不把用户态服务政策带入 shared |
| 通用执行被命名为服务框架 | `libsrv/runtime.rs`、`work_queue.rs`、`wake.rs`；RPC Dispatcher/Outbox、libprocess 观察/收束、Runnel SourcePlan、三个服务运行体 | 通用执行独立归属；任务/来源代次、期限、Gate、停止、注销回执、任务退休和失败 owner 共同迁移 |
| FAL 认识服务预算类别 | `libfal/resource.rs::FalResource::ServiceRecord`，尚无消费者 | 服务领域定义服务记录成本；FAL 保留自身成本，组合账户不要求下层枚举上层资源 |
| shared 暴露空服务类型 | `erhino_shared/src/service.rs::Endpoint` 与模块导出，尚无消费者 | 服务含义归用户态服务领域；确认引用后清理错误 ABI 归属，不在 shared 保留同名占位 |
| 服务领域尚无正式实现 | libsrv 当前只有上述公共机制；ServiceRecord/RegistrationControl 等仅在方向与 F3d 草稿中 | 保留服务领域的唯一设计归属；实际能力随真实消费者交付，不将通用机制搬走后的空包误标为完整服务框架 |
| provider 组合仍在具体进程 | `srv_fs/server.rs`、`watch.rs`，MemoryBackend、GrantTable、Outbox 与 Runtime 装配 | 审计领域机制和进程政策的接缝，为服务领域消费通用 provider 能力确定接口；不能用 srv_fs 专用服务分支补偿反向依赖 |
| 启动业务解释在装配侧 | rinlib env 只读 outer/Handle/payload；srv_init/srv_fs 等解释启动能力用途 | 继续保持；审计服务声明、namespace 和进程构造的拥有者，避免不同领域重复定义启动知识 |
| 集合目录混称框架 | `user/frameworks/`、user workspace、各服务/驱动/测试的 path 依赖、文档和工具 | 统一迁到 `user/libraries/`；保留正确的相对依赖、host/target 条件、实际构建和验证入口 |

## L0 审计完成门（已完成）

- 全部现有用户库、shared 原语、直接依赖与公共类型已盘点；当前错位及删除门集中在本计划。
- 公共记账定名 `libbudget`，位于用户态库集合；`metadata_admission` 继续唯一拥有跨层 Counter/Permit 原语。
- 公共执行定名 `libexecution`；RPC、进程、流和服务进程直接消费它，同步消费者不被迫承担 Runtime。
- 服务领域拥有注册与发现，FAL 只提供通用 Record/provider 机制；具体 provider interface 随 F3d 两个真实形态定形。
- 迁移保持 SourcePlan、WaitSet、Outbox、授权快照、账户及退休 owner 的生存期；未调整锁和跨线程模型。
- 包名、目标类型图和迁移表已先写入方向文档与库 README，随后实施 L1–L3。

## L0 裁决：目标模块与组合机制

### 目标依赖图

```text
metadata_admission ──> libbudget <── libexecution
                              ↑          ↑
                     libfal / librpc / libprocess / librunnel
                       ↑       ↑
                     libfs   libservice
                               ↑
                    服务进程与装配政策
```

- 新建 `libbudget`：拥有非领域化的 `Budget`、唯一付款身份 `Account`、不可变领域视图 `AccountView<K>`、资源绑定及非泛型 `Charge`。`metadata_admission` 继续只拥有 `Counter`、`Permit`、`SponsoredPermit` 等跨层固定容量原语；用户态账户组合不下沉到 shared。
- 新建 `libexecution`：拥有 Runtime、任务/来源/期限/唤醒、公平工作队列、停止与退休协议，并独立定义 `ExecutionResource`。它消费 `AccountView<ExecutionResource>`，不接收领域枚举或裸槽位。
- `libfal` 保留 `FalResource`，但只枚举 FAL 自身节点、字节、grant、请求、outbox、watch、offer、stream 与等待来源成本；删除执行分类和 `ServiceRecord`。FAL 后端与授权快照接收 `AccountView<FalResource>`，长期 owner 只保存非泛型 `Charge`。
- `libservice` 在公共机制迁出后只拥有服务声明、发布、发现、注册控制和实例生命周期知识；F3d 才随首个真实消费者加入 `ServiceResource`、`ServiceRecord` schema 与 RegistrationControl，不在本专题制造空实现。
- `librpc`、`libprocess` 与 `librunnel` 直接消费 `libexecution`；`libfal` 可直接消费 `libbudget`、`libexecution` 与 RPC，不再通过旧 `libsrv` 取得公共机制。最终 `libservice` 可以单向依赖 FAL/RPC/进程和公共机制，下层领域库不反向依赖它。

### 同源领域视图

一个 `Account` 是唯一付款身份；`AccountView<K>` 只是该身份对一个领域的不可变计费绑定，不创建账户、不复制计数器、不重置额度。领域视图分别命名成本，但可按装配布局引用同一个实际额度；相同限额数值创建的两个计数器不构成共同上限。节点数、任务数和记录数等不同单位不混加；共享字节上限必须由绑定显式共同扣账。

`Charge` 固定实际退款对象并保活账户及结构性账户名额。预留任一步失败通过 Permit RAII 释放已取得额度；多资源准备对象共同持有各项 Charge，全部准备完成后才能发布，但不承诺多个原子计数器形成可观察的事务快照。退款跟随真实 owner 的释放或已提交的占用缩减，取消、撤销、摘名和进入 Draining 本身不是退款点。

grant 派生、视图克隆、sender 别名和重复发现必须继承同一付款身份与既定绑定，不能重新开户扩额。认证、领域授权与付款关系分别验证：内核发送上下文定位 grant，grant 决定 FAL root/rights，可信装配确定付款账户；客户端 payload、PID 或自报 badge 不能选择付款来源。

### ServiceRecord 与 FAL 的接缝

历史审计确认 `FalResource::ServiceRecord` 只是零消费者、零装配限额的枚举占位：它最初位于 `libsrv` 的混合资源枚举，资源分类泛型化时随整批 FAL 分类迁入 `libfal`，没有必须属于 FAL 的机制证据。删除该占位不迁移既有运行行为。

F3d 中 `libservice` 的注册状态机唯一拥有 instance、状态、代次、RegistrationControl 与 endpoint owner；FAL 提供通用 Record 编码、目录访问、能力出口和 Watch。服务目录是对注册状态的受控 FAL 投影，普通 Create/Write/Remove/Move 不能绕过注册状态机。Ready 发布完整快照作为可发现性的线性化点，撤销携带 instance/generation，已交付快照按自身责任收束；允许保存与代次绑定的编码缓存，不形成第二份可写服务真值。

当前 `srv_fs::serve_v2` 与 `MemoryBackend` 直接耦合，`libfal` 尚无通用 provider seam。本专题只登记该事实、消除错误依赖并保持现有 provider 行为；F3d 由现有内存 provider 与首个服务目录投影两个真实形态共同确定 provider interface，同时迁移公共分发机制。不在 L1/L2 机械复制 `MemoryBackend` 方法为 trait，也不新增服务专用 FAL 分支。

## 依赖与实施顺序

```text
既有 F1–F3c 工作树及已交付公共机制
  → L0 全库知识/依赖审计、目标类型图和相关计划重审
  → L1 公共记账与执行归属重排、全部真实消费者迁移
  → L2 领域越层清理与组合接口收口
  → L3 frameworks → libraries 目录和全部有效引用迁移
  → L4 结构 Review、组合验证、文档与计划收口
  → L5 用户态组件命名规则、空库删除与未来名称统一
  → FAL F3d 重新规模审计 → F3e → F3f → F4
```

各步骤是本专题的施工顺序，不构成未闭合机制的独立交付。设计/计划更新贯穿 L0–L5；命名在提交前作为同一重排闭包收口，不能让目录、Cargo 身份、方向文档与未来计划使用不同词汇。

| 项目 | 前置与真实调用者 | 失败边界、删除条件与验证门 |
|---|---|---|
| L0 审计与设计/计划重排 | 当前工作树；上表全部领域与消费者 | 不更改运行行为；所有已发现错位进入本计划且明确目标 owner；FAL 第 5/8 节、前置计划及 ideas 的冲突点形成统一修订方案 |
| L1 公共记账与执行 | L0；FAL/RPC/进程/流及 init/pm/fs 同迁 | 保留现有状态与清理责任，迁移正常/失败/停止/退款消费者；删除 libsrv 中公共机制旧定义与兼容出口；相关 host/target 开发检查通过 |
| L2 领域知识与组合 | L1；服务领域规划、通用 provider 与进程能力消费者 | 清除 FAL 服务分类、shared 服务空壳等越层知识，确认公共类型唯一；任何新接口必须有现有或计划内明确消费者，不能以无消费者占位冒充交付 |
| L3 目录迁移 | L2；user workspace、服务/驱动/测试、工具/构建引用 | 整组迁移目录及 README、Cargo 路径、有效文档链接，更新根 AGENTS 的结构图与入口；不保留旧目录/路径 adapter；元数据与构建入口均指向 libraries |
| L4 组合收口 | L1–L3 全部责任闭合 | 按下节运行验证并结构审查；更新 impls、COMPASS、本计划与 FAL 恢复断点，不把本专题通过写成 F3d 或整体 FAL 已交付 |
| L5 命名收口 | L4；现有用户态组件和未来领域库名称 | `user/README.md` 固化总体命名入口；删除无实现、无符号消费者的 `libdrv` 及空依赖；未来服务/驱动库统一为 `libservice` / `libdriver`，历史固定提交材料保持原名；Cargo metadata、静态门及必要运行门通过 |

## 唯一迁移债务与删除门

本表条目的触发条件均为本专题 L0；保留期限截止对应步骤完成，owner 为本计划实施者。不得在此期间扩展错误归属，也不得在其他 todo 重复立案。

| 债务 | 替代物与删除条件 | 清理验证 |
|---|---|---|
| `libsrv` 内公共记账/执行与各领域反向依赖 | L1 的独立公共库及全部调用点；最后一个消费者迁移后删除旧定义/出口 | Cargo 图及符号消费检查：通用领域库不依赖 libsrv，公共机制仅一份实现；失败/退款证据保留 |
| FAL ServiceRecord 分类、shared service 空壳 | L2 的领域归属；服务业务由 F3d 接通，错误底层位置在本专题删除 | 检索类型、枚举、模块导出与依赖，通用 FAL/ABI 不再定义服务含义 |
| 当前 provider 装配与目标组合接口未审计 | L0/L2 明确可复用契约及装配 owner；属于尚未实施 F3d 的具体业务只由 FAL 总计划承接 | 已有消费者无功能/清理回退，未来服务投影不依赖向下层注入服务分支 |
| `user/frameworks/` 及有效路径引用 | L3 的 `user/libraries/` | 无旧目录和有效旧构建/导航路径；历史固定提交材料只读，若保留旧路径须在本计划登记其历史性质 |
| 旧计划把 libsrv 等同公共执行、资源分类错层 | L0 起同步目标；L4 根据最终实现完成所有当前文档对齐 | ideas 描述目标分工，impls 描述真实代码，活跃计划依赖图一致；旧交付证据不改写成新验证 |
| 单词缩写与角色前缀混用（`libsrv`、`libdrv`） | L5 的 `user/README.md` 规则、未来 `libservice` / `libdriver`；空 `libdrv` 删除 | 当前代码/方向/活跃计划无缩写库身份；历史基线和固定提交 Review 保留旧名并明确其历史性质 |

## 验证与交付标准

- 结构：目标 Cargo DAG 与知识归属图一致；公共记账、执行与服务域独立；领域库的客户端和管理端按 capability 行权，无隐式角色/权限变化；检查 public 类型、资源枚举与回调契约。
- 静态：`git diff --check`、`just check`、`just clippy`；受影响 workspace 的 Cargo 元数据和 path 依赖检查；RISC-V 用户程序构建统一走 just，host 测试显式使用 `aarch64-apple-darwin`。
- 机制：复用并按实际变化补充记账限额/预留失败/退款、Runtime 来源/期限/停止/退休、RPC/Outbox、进程收束、FAL 授权/值/Watch 的既有测试，不写只证明换路径的测试。
- 组合：闭合后跑 `just virt`、`just virt-release`；本次跨公共记账与执行的迁移收口需完整 `just acceptance`，覆盖 stress、平台及退出组合。若审计最终缩小迁移责任，须先在本计划记录新的验证依据。旧通过记录不能替代迁移后的证据，完整日志按仓库约定留在 artifacts。
- 文档：更新实际受影响的 `notes/impls/{runtime,rpc,runnel,fal,startup}.md`、方向文档、`user/README.md`、库 README、AGENTS、FAL 总计划及前置计划的当前导航；历史 Review/ref/archived 材料不批量改写。
- 收口：结构 Review 关闭所有本专题问题或留下唯一且语义闭合的延期；FAL 恢复入口确认新的 owner 与依赖，随后归档本计划。提交须另行授权，提交后登记固定提交 Review。

## 当前断点

L0–L5 已完成。`user/README.md` 现作为用户态总体入口，本轮只写已经确定的组件命名段，不提前虚构尚未裁决的总体布局；`user/libraries/README.md` 承接库领域与依赖细则。普通单词领域写全，多词使用公认缩写，角色缩写只用于 `srv_` / `drv_` / `test_` 二进制前缀；未来服务与驱动公共库统一为 `libservice` / `libdriver`。原空 `libdrv` 没有实现和符号消费者，已删除 crate、workspace member、`drv_spi_sifive` 空依赖与 Justfile lint 项；历史交付基线中的 `libsrv` / `libdrv` 保持原名。

L5 验证通过：Cargo metadata 列表无 `libdrv`，`user/Cargo.lock` 无残留 package，代码/构建入口无 `libraries/libdrv`、`name = "libdrv"` 或 `libdrv =`；九份受影响 Markdown 的本地链接检查通过，`git diff --check`、`just check`、七面 `just clippy` 和默认 50% `just virt` 通过，退出后无残留 QEMU。L5 未修改任何运行机制源码，只删除空 crate/空依赖并更新文档与构建枚举，因此 L4 的同工作树完整 `THROTTLE=100 just acceptance` 证据继续有效，不重复运行平台与 boot-failure 聚合。

本专题已提交并归档。后续由 [`FAL 整体计划`](../todo-2026-09-fal-service-capabilities.md) 从第 8 节的 F3d 服务注册/发现任务规模审计继续；未来 `libservice` 只随真实发布者与发现消费者建立，不提前创建空 crate 或预设 provider trait。
