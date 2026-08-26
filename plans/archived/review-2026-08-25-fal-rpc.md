# Review：FAL/RPC 集成批次

日期：2026-08-25。对象：`54d3e02`（设计入档 + WaitMany deadline + librpc/libfal 线协议）、`bf32c1c`（libfs + memfs/provider + fs 真实化 + 旧 ABI 清除）、`277a87c`（syscall 清表 + kind 动词化）。reviewer：CosmicGale（GPT 5.6 Sol，只读审查），归档：ClearCreek。计划：[2026-08-25-fal-rpc-review.md](2026-08-25-fal-rpc-review.md)。

**裁决：修复后收口。** 首轮不可收口（1 blocker + 8 major + 5 minor，集中于同进程泵未覆盖的真路径）；修复批落地 B1、M1–M3、M5–M8、m1–m5 全部项，M4（slot 1 锚授权）与 M7 的 Delegate 映射随跨进程批次（见下）；基线复验全绿（host 59 项、三 workspace check、just virt 四服务 + fs 验收线）。

## 发现清单

### Blocker

| # | 位置 | 问题 |
|---|------|------|
| B1 | `librpc/caller.rs` | ReplyPort peer 创建权利 `WRITE\|DUPLICATE` 缺 `TRANSFER`，而派生 send-once 请求 `WRITE\|TRANSFER` 超出子集——`Caller::call` 必然 `RightsDenied`，librpc 同步调用面从未真正可用。fs 泵自路径已修，librpc 漏改。 |

### Major

| # | 位置 | 问题 | 性质 |
|---|------|------|------|
| M1 | `libfs/resolve.rs` | Link 的 consumed/remaining 校验错位：wire 语义是 `consumed + [链接分量] + remaining`，代码按 `consumed + remaining` 连续验证；`split_off` 在恶意边界下越界 panic；绝对 target `/` + 剩余拼接出 `//` | 走路引擎正确性 |
| M2 | `libfal/memfs.rs` | `NoFollowFinal` 对**中途**链接也返回链接节点，未检查是否终段（`node_at` 检查了，`lookup` 没有） | 提供者语义 |
| M3 | `srv_fs serve_one` | 每请求泄漏 slot 1 anchor Handle（从不关闭）；reply 发送 `expect` 可被恶意 slot 0 触发 panic | Handle 生命周期 |
| M4 | `provider::serve` | slot 1 锚目录未参与解析/授权——所有请求从 MemFs 根解析，目录 grant 无沙箱边界 | 契约缺口（跨进程前置） |
| M5 | `libfal/bytes.rs` + fs 缓冲 | `Writer::u16/u32/u64` 无界写 panic；fs 固定缓冲（128/256/512）与合法上限（payload 4096、路径 512、值 4096）脱节——合法长输入静默截断或 panic | 边界 |
| M6 | provider + fs | `FalHeader.total_len` 未与消息长度交叉校验；Write 属性解码不 `finish()`（接受残尾）；短应答切片 panic | wire 校验 |
| M7 | `libfs NodeSummary` + Fs transport | Found 的 value 尾（target）被丢弃——`NoFollowFinal` 找到链接却拿不到 target，与「无 ReadSymbolicLink、target 随 NodeInfo 返回」契约冲突；transport 无 Delegate 映射 | 契约缺口 |
| M8 | `memfs` 属性写/偏移写 | 属性值不经 `DecodedValue`/`VALUE_MAX` 校验直接存原始字节（违反提供者不信任输入）；`write_at` 加法未 checked，大 offset panic | 不可信输入 |

### Minor

| # | 位置 | 问题 |
|---|------|------|
| m1 | `node.rs validate_path` | 未拒绝通配符（`*`/`?`），与注释/ideas 契约不符 |
| m2 | `memfs enumerate` | index 越过末尾按空页处理而非 CursorInvalid；项成本 `+12` 实为 `+10`；首项超预算仍返回 |
| m3 | `librpc RpcPrefix` | 解码接受 txid 0（wire 非零约束未执行）；分配器回绕可产 0 |
| m4 | `op.rs` | `ReadSymbolicLinkRequest` 尸体（kind 已删）；`PropertyReadRequest` 等旧名未同步 |
| m5 | `libfs prefix.rs` | 重挂载覆盖旧 Handle 不返还，泄漏授权 |

## 已知疑点裁决（review 计划七项）

| # | 疑点 | 裁决 |
|---|------|------|
| 1 | 寻址偏移三种写法 | **需修复**：`OpAddress::decode` 应返回消费长度/剩余 Reader，消除重复算术 |
| 2 | 512B 缓冲 vs 4096 常量 | **需修复**：已造成静默截断/panic 风险，非纯未来问题 |
| 3 | Caller 需要 WAIT 权 | **需修复（合并 B1）**：API 未表达权利需求；ReplyPort 缺 TRANSFER |
| 4 | 属性原始字节存储 | **需修复**：违反不信任输入契约（M8） |
| 5 | Lookup 独有 variant 字段 | **接受为协议形状**：tagged union 合理；统一 status/variant codec 并冻结成功/失败 body 形状 |
| 6 | 500ms → Internal 归并 | **立后续计划**：正式客户端须区分 transport deadline / FAL Status / framing 错误；同进程泵可暂留 |
| 7 | Enumerate 预算边界 | **需修复**（合并 m2）：规范最小预算语义 |

## 已知事故验证

| 事故 | 结论 |
|------|------|
| send-once 缺 TRANSFER | **未完整修复**：fs 侧已修，librpc Caller 漏改（B1） |
| FalHeader 切片越界 | **修法正确**；但 `total_len` 交叉校验仍缺（M6） |
| Link 映射丢 consumed | **字段映射已修，端到端仍错**：resolve 校验算法错误（M1） |

## WaitMany deadline（观察项）

静态审查通过：x13 传参、0=无限映射、Deadline 写回（cookie=0/observed=NONE/item_index=u32::MAX/reason=Deadline）、与对象完成共用仲裁、不消费观察项。缺专项集成测试（到期、竞态、写回）；当前 virt 泵不触 Deadline 分支。

## 尸体清除

旧内核直连 ABI 清除通过（shared/fal.rs、path.rs、rinlib::fs 无残留，call.rs 无 0x70–0x79）。`ReadSymbolicLinkRequest` 已删（m4）。

## 修复批落地对照

| 项 | 修法 |
|----|------|
| B1 | Caller ReplyPort 创建权利补 TRANSFER；发送失败关 reply_once；等待/接收失败废弃端口 |
| M1 | `verify_cover` 取代 `consume_prefix`：Delegate 校验连续覆盖，Link 校验「consumed + 链接分量 + remaining」；绝对 target 组件式拼接；伪造边界/中途链接/根 target 三组新测试 |
| M2 | memfs `lookup` 与 `node_at` 统一终段判定（中途链接 NoFollowFinal 仍返边界）+ 测试 |
| M3 | `serve_one` 槽位契约校验（恰好 2 Handle）、不消费 Handle 显式关闭、reply 失败关回复权不 panic |
| M5 | Writer 全部定宽写 fallible；fs 缓冲由 `PAYLOAD_MAX`/header 常量推导（`REPLY_BODY_MAX`） |
| M6 | provider 校验 `total_len == request.len()`；Write 属性解码 `finish()` 拒残尾 |
| M7(value) | `NodeSummary` 携带 owned value 尾；NoFollowFinal 演示经 Found 取 target；Delegate 映射随跨进程批次 |
| M8 | 属性写入经 `DecodedValue` 解码校验 + `VALUE_MAX`；`write_at` checked 算术 + 测试 |
| m1 | `validate_path` 拒通配符（`*?\`）+ 测试 |
| m2 | cursor 越界返 CursorInvalid；项成本按实际 10 字节开销 |
| m3 | `RpcPrefix::decode` 拒 txid 0；分配器跳过回绕 0 |
| m4 | `ReadSymbolicLinkRequest` 删除 |
| m5 | `mount` 替换时返还旧 Handle（调用方关） |

**遗留（随跨进程/服务化批次）**：M4 锚授权（provider 按 slot 1 Handle 解析子树根——需真实跨进程客户端才有意义）；M7 Delegate 传输映射；疑点 6 超时语义分层。reviewer 另建议（未落实，后续批次评估）：Deadline 专项集成测试、Caller 的 RISC-V 真路径集成测试。
