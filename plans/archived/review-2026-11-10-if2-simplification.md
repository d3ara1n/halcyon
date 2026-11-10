# IF2 提交前简化审查与修复

状态：**S1–S3 已修复，定向验证和主代理同组复核通过，报告关闭归档。** 本报告唯一承载本组 findings、处置与验证，不另建修复 todo。用户已授权核对设计落实/延期归属、审查过度设计/防御并清理；满足条件后由主代理提交，不授权 push 或 QEMU。

## 范围与证据

基线 `master / 68843ce`，对象为 IF2 未提交工作树。依据为[IF2.1 具体设计与交付记录](../todo-2026-10-28-init-formation.md#if21-接手审计与具体设计2026-11-10)及 REVIEW 的成形结构简化要求。本次不重做此前三组正确性检查，不以单个消费者、代码行数或抽象数量否定独立契约；输入/授权、配对身份和真实失败 owner 校验均保留。

sub-21/sub-22 因执行中断未保存结果，不计审查通过。重新执行的 sub-23 审查 schema/birth/OAL/init，sub-24 审查 Mailbox 付款、领域采用及三个宿主；均只读。主代理逐项核对实际代码，确认下列三项是局部简化机会，不是已证明的授权漏洞或资源泄漏。

## S1：消费 BirthPlan 时填回伪空计划（低）

原位置：`user/services/srv_init/src/executor.rs:504–516,612–626`。`start` 通过 replace 取走实际 BirthPlan，再填回空 inputs/payload/零预算；这些槽位后续无消费者，混淆合法计划与已消费状态，保留无用途计划数组。

目标修复：全部实例 Pool 准备完后一次 take plans，按声明顺序消费 `into_iter().enumerate()`；start 直接接收 BirthPlan 值。不新增 Option 或阶段类型。计划只持纯描述/字节，未启动尾部丢弃不涉及 capability 责任；实际资源仍由 executor 账本持有。保持实例索引、DONE cookie、发布顺序及中途失败政策。

状态：已修复，主代理对照实际实现复核通过，关闭。

## S2：终局入口列表二次分配及 owner 注释失真（低）

原位置：`srv_init/src/executor.rs:324–341`、`main.rs:55–57,79–92`、`terminal.rs:12–18,32–38`。admins 已返回 Vec，fail 又 to_vec；terminal 仅遍历，不修改或持久保存。裸 Handle 列表复制不是能力 owner 移交，实际 sender owner 仍在 executor。

目标修复：Terminal 借用 `&[Handle]`，fail 直接交付已有切片；注释准确表达借用与真实 owner 保活。保留内部为 RPC 准备 sender 的必要 duplicate，不扩大为终局框架重写。空列表和 executor 存活至不返回终局两条路径均核对。

状态：已修复，主代理对照实际实现复核通过，关闭。

## S3：driver 入口身份重复 Query（低）

原位置：`srv_driver/src/main.rs:32–46`、`server.rs:344–360`。main 在 adopt_endpoint 配对校验后再次 Query 两个 sender，只为独立性断言；server::run 在 World 构造时又对同一 owners 查询认证 context/incarnation。

目标修复：将两个入口独立性断言移到 server::run World/Ingress 建立前，复用已有两份完整 description；删除 main 的两次额外 Query。不新增缓存或验证包装类型，不移除 adopt_endpoint 的 pair/rights 校验，不改变双 Mailbox 隔离要求。

状态：已修复，主代理对照实际实现复核通过，关闭。

## 明确保留的结构

- shared/oal 由 host formation 和运行期 liboal 共同使用，birth named sections 保持进程层与领域解耦；没有证据要求删除这些独立知识边界。
- 当前 Pool 的 create/mint 便利入口与显式付款路径进入同一实现，不是失败兼容双轨。
- MintedSender 可由公开成员构造，领域边界不能假设先前一定经过安全采用；保留相关验证。libfs mailbox_id 表达授权表所属接收端，不由单次 pair 检查替代。
- service prepare/install 的复核覆盖多个尚未安装的准备对象；Lifetime、领域 BOOT_READY、Binding 安全与退休状态不能当旧 init Ready 残留删除。

## 验证与关闭条件

修复者记录具体 diff 与日志，只检查受影响 srv_init/srv_driver 及其最终构建产物；不重跑未受影响内核/纯机制的 239 项 host 测试，不新增测试设施。按 S1–S3 做同组定向复核，成功后关闭并归档；不自动生成下一轮全量审查。正式运行证据缺口仍由 IF2 原记录拥有。

## S1–S3 修复与验证（2026-11-10）

修复者仅修改分配的五个源码文件，没有扩大机制或更改 ABI/领域契约；未提交、未运行 QEMU。原 findings 与目标修复保留在上方，以下记录实际落地：

- S1：`Executor::start_all` 完成全部 Pool 准备及静态资源构造后一次 `mem::take(&mut self.plans)`，以 `into_iter().enumerate()` 按声明顺序消费；`start(index, plan: BirthPlan)` 直接接收值，不再 replace 一个输入/字节/预算全空的伪计划。没有新 Option/阶段；计划尾部只有纯描述，capability 仍在原资源/实例账本，原 GrantOutcome 与 DONE 中断失败路径未改。
- S2：`Terminal.admins` 改为借用 `&[Handle]`，`fail` 不再 `to_vec`，循环直接遍历切片；注释改为借用语义。services/reset/platform 与 executor 的实际 capability owners 均活至不返回的终局调用；构造失败时的空切片和后续实例失败时的局部 admins 列表均能直接借用。RPC 内必要 sender duplicate 保留。
- S3：删去 driver main 在采用配对后为独立性断言额外发出的两次 sender Query；server::run 保存已有 observer 完整 description，与已有 control description 在 World/Ingress 构造前核独立 receiver/context，随后直接使用两份描述生成 World 身份。main 的 pair/kind/rights 校验、双 Mailbox 队列隔离均保留，不引入缓存类型。

现有 startup/device impls 描述的是预建采用、真实 context、发布与终局借用责任，不依赖被删除的伪空计划、第二份 Handle 向量或重复 Query；无需为这些内部简化改写长期机制说明。

| 定向验证 | 结果 / 完整日志 |
|---|---|
| `just check srv_init` | 通过：`artifacts/checks/20261110T081408.352939Z-81745/srv_init.log` |
| `just clippy srv_init` | 通过：`artifacts/checks/20261110T081408.828320Z-81776/srv_init.log` |
| `just check srv_driver` | 通过：`artifacts/checks/20261110T081409.267006Z-81810/srv_driver.log` |
| `just clippy srv_driver` | 通过：`artifacts/checks/20261110T081409.678752Z-81852/srv_driver.log` |
| `just make_boot_package` | 正式 System 用户态编译/ELF 审计、initfs 与 BootPackage 打包通过：`artifacts/checks/if2-simplification-build-20261110T081417Z.log` |

单文件 rustfmt 与 `git diff --check` 通过。内核/纯机制及其消费者协议未改，不重复既有 239 项 host 测试或内核检查；此次构建不产生实际启动、跨进程或硬件运行证据。主代理已对照实际实现复核：所有 Pool/资源准备早于 plans 消费；未启动尾部无 capability owner；索引及 GrantOutcome/终局路径不变；Terminal 借用在不返回调用期间由原调用栈保持；driver 独立性断言仍早于 World/Ingress。三项均关闭，无新增 finding，不新开下一轮全量 Review。
