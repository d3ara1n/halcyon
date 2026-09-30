# 内核资源外部暴露机制系统参照

> 【只读参考资料】2026-09-30 为 Halcyon capability/Handle 暴露模型的外部对照收集的事实快照。参照只用于拓宽选项与核对现状，不替 Halcyon 作设计推导；取舍归 [系统重构总纲](todo-2026-09-29-system-redesign.md) 及其 capability 分案。
> 取证方式：官方手册/规范/源码/原论文核实（版本见各节与来源清单）；原站离线处经 Wayback 快照核实。标注「未核实」处为穷尽官方来源后的缺口。

## 范围与口径

对照维度：capability 的寻址与表所有权、rights 模型与检查点、转移语义、撤销、badge/身份、introspection。Halcyon 当前模型（作为对照基线）：每进程内核 HandleTable，表项 = {对象引用, HandleRole, Rights}；Handle 为 u64（32 位 generation + 32 位槽位）；转移仅 TRANSIT（邮箱消息原子搬运）与 GRANT（跨表直接安装）；DUPLICATE 只能收窄；HandleQuery 输出 per-handle 描述；koid 仅诊断。

## 1. seL4

版本：Reference Manual v16.0.0（2026-07-22）、MCS Manual 10.1.1、白皮书 rev 1.4.1（2026-07-17）、master 源码。

- **无 per-process 内核句柄表**。每 TCB 持 CSpace 根 CNode；CNode 是纯能力槽数组，CSpace 为 CNode 有向图；「requires the programmer to manage all in-kernel data structures, including CSpaces, from userspace」——槽位分配与 CSpace 构造完全在用户态（manual §3.1、§3.3）。
- 寻址：CPtr 经 guard 匹配 + radix 索引逐级下探，用户须给 depth；失败四类（Invalid Root / Missing Capability / Depth Mismatch / Guard Mismatch）。
- 原语：Copy（可收窄 rights）、Mint（改 rights/guard/badge）、Move、Mutate（Move+收窄）、Rotate（三槽原子轮转）、Revoke（对派生子逐个 Delete，目标保留）。
- badge 仅适用 Endpoint/Notification；badged cap 不可去 badge/改 badge/派生不同 badge 子；32 位平台低 28 位可用。
- 派生树（CDT）：Retype/Mint/Copy/Mutate 派生被跟踪；untyped 副本永远生成子节点；普通 original cap 只允许一层派生子；单次 Retype fanout ≤ 256；Revoke 递归 Delete 全部子。
- IPC 转移是**复制**（发送方保留），前提 endpoint cap 有 **Grant**；extraCaps ≤ 3（`seL4_MsgExtraCapBits=2`）；接收端只有一个接收槽；唯一接收侧权利修改：接收 endpoint 缺 Write 时剥离收到的 Write；消息中指向所经 endpoint 的 cap 被 unwrap（badge 进 badges[]）。
- rights 仅 Read/Write/Grant/GrantReply（GrantReply 为弱化 Grant，服务 reply cap）；多数对象类型无 rights（结构由 cap 类型承担）；违规多静默降级而非报错。
- introspection：常规 API 无自查；仅 DEBUG_BUILD 有 `seL4_DebugCapIdentify`/`seL4_DebugSnapshot`。
- 官方承认的代价：白皮书「API is very low-level… building systems on seL4 is particularly difficult」；「revoking a capability can be a long-running operation」（内核以 incremental consistency 拆分）；Microkit/CAmkES 的存在动机即遮蔽此层。

## 2. Fiasco.OC / L4Re

- 每 task 内核维护 object space（+ memory/IO space）；能力地址为 capability index/slot；flexpage = 空间中 2 的幂对齐区间 + rights。
- 转移：IPC 双方各给 send/receive flexpage，内核执行 map；`L4_MAP_ITEM_GRANT` = 搬移（源映射移除，父子关系保留）。
- **对象能力不参与层级**：「Object capabilities are not hierarchical – they have no children. The result of the map operation on an object capability is a copy」；内存映射则层级化：unmap 撤父级联子。
- namespace（`L4Re::Namespace`）为运行期按名字交换能力的存储对象；应用由创建者授予初始能力集，初始集完全定义执行环境。槽位分配是用户态库设施（`L4Re::Cap_alloc`）。
- 「MAP DB」内部术语/结构：官方文档未出现，未核实。

## 3. Genode

版本：Foundations 26.05。

- 能力 = 指向 RPC object 的不可伪造 token；RPC object 创建时在 PD 的 kernel-maintained capability space 占一槽；内核调用时完成 client-local → server-local 翻译。
- **只有 share 型委托**（「does not diminish its authority by delegating… It merely shares」），无 grant/move；**无 per-cap 权限位**（「no superficial notion of access rights associated with a capability」）。
- 生命周期：仅 owner 能销毁 RPC object；identity 销毁时内核从**所有** capability space 删除引用，之后 owner 才析构。
- **cap 配额**：PD = 地址空间 + capability space + 内存与 capability 的 budget；quota-bounded allocator 初始为 0，经 reference account 转账；转账只允许 PD↔其 reference PD（防资源偷窃）；session 关闭时服务端须先释放，配额转回客户端，贪墨可被检出并由 parent 处置。
- parent 机制：PD 初始能力空间为空，唯一可安装的是 parent capability；parent 从不向子发 RPC。

## 4. NOVA

版本：spec（2026-08-27）+ EuroSys'10 论文。

- 每 PD 唯一平坦 Object Space；capability = opaque selector（直接索引，无 guard/层级/派生树）。
- 权限为**每类型 6 位掩码**（PT: CTRL/CALL/EVENT；PD: PD/EC/SC/PT/SM/DC；内存: R/W/XU/XS 等）。
- 委托：ctrl_pd 把源空间 selector 区间 [ssb, ssb+2^ord) 批量委托到目标区间，目标权限 = 源权限 AND pmm；目标区间既有 cap 被撤销；源需空间 cap TAKE、目标需 GRANT；存储记账到目标 PD，不足则部分失败。
- 论文：「By revoking the respective capabilities, an execution context can recursively withdraw any resources that have previously been delegated」。
- portal 调用显式给出目标 PT selector 且需 CALL 权；穿透时调用者捐出 SC，内核创建一次性 reply cap。

## 5. Zircon / Fuchsia

来源：fuchsia.dev（2025-03 至 2026-08 各页）+ 内核源码 commit 7dedc3f。

- handle 表进程私有；`zx_handle_t` 32 位，有效 handle 低两位恒为 1；值构成（源码）：arena 索引（低 18 位）+ 世代号（12 位）+ **每进程 PRNG 随机 XOR mixer**；槽位复用时世代 +1。系统级 arena 容量 `kMaxHandleCount = 256×1024`；单进程上限官方无常量（未核实）。
- rights 为 uint32（26 个值，含 DUPLICATE/TRANSFER/INSPECT/…）；每类型有 `ZX_DEFAULT_*_RIGHTS`。**检查点：每次 syscall 执行时检查** handle 存在且具备所需 rights；rights 铸后不可变。
- `handle_duplicate`：源须有 ZX_RIGHT_DUPLICATE，rights 只能子集/SAME_RIGHTS/NONE；`handle_replace`：原 handle 总被失效、无前置权利要求。
- channel 携带 handle：`ZX_CHANNEL_MAX_MSG_HANDLES = 64`；消息整体原子（无短读写）；`channel_write` 成功失败源 handle 均被丢弃；`channel_write_etc` 支持 per-handle disposition（MOVE ≈ replace+write、DUPLICATE ≈ duplicate+write），rights 为「期望最终 rights」，可剥离 ZX_RIGHT_TRANSFER 使接收方不能再转发；接收方以 `channel_read_etc` 取 `zx_handle_info_t{handle,type,rights}`。
- **无 badge 概念**（全域检索无 Zircon/channel badge 词条）；身份靠 koid（64 位运行期唯一不复用）与 related_koid（不可变）；按 koid 取对象的唯一例外是 `object_get_child`（需父 handle 且有 ENUMERATE，rights 不得超出父）。
- **无撤销**：不存在撤销已派生 handle 或事后收窄的 syscall；`task_kill`（需 ZX_RIGHT_DESTROY）内核遍历整个 handle 表逐个关闭。
- introspection：`ZX_INFO_HANDLE_BASIC/HANDLE_COUNT/HANDLE_TABLE` 等 topic；HANDLE_TABLE 逐条给出 type/handle_value/rights/koid/related_koid/peer_owner_koid；HANDLE_COUNT 官方注明仅调试用途。

## 6. Mach / XNU

来源：Apple 归档文档 + xnu/launchd 官方源码。

- right 类型：SEND/RECEIVE/SEND_ONCE/PORT_SET/DEAD_NAME；「port right 所有权是基本安全机制」。receive right 每 port 恒 1；send right ≥1 uref；send-once 恒 1、一次性；port set 不能随消息迁移；dead name 为墓碑名。
- 消息迁移 disposition：MOVE_RECEIVE / MOVE_SEND / MOVE_SEND_ONCE / COPY_SEND（仅增内核引用）/ MAKE_SEND（持 receive 造 send，mscount+1）/ MAKE_SEND_ONCE；COPY_RECEIVE 标记 NOT VALID。
- 撤销链：销毁 receive right → 端口失活，存量 send/send-once 惰性转 dead name；未消费 send-once 被销毁时内核以它发出 MACH_NOTIFY_SEND_ONCE 后消费；最后 send right 消失可发 MACH_NOTIFY_NO_SENDERS（携带 mscount 同步值）。
- mscount：port 新建 0、带 send right 分配时 1、MAKE_SEND 递增、receive 迁移/销毁时清零；**当前 XNU 的 dead 判定只看端口 active 位，mscount 不参与**。
- 命名：task 本地 32 位名 = index(24b) + generation(8b)，内核挑选；无系统级名、无 badge；不可伪造的接收侧凭证 = 消息 trailer 的 `audit_token_t`。
- 上限：每 task IPC 表 131072（3MB 配置）/ 305834（7MB）entries，超限 grow 返回 KERN_NO_SPACE；send right urefs 上限 65535、溢出后 pegged；qlimit 默认 5/大 1024。
- bootstrap：TASK_BOOTSTRAP_PORT 由内核为 launchd 建立，launchd 管理 name-port 绑定表。
- 批评类文献（right 耗尽 DoS、implicit 语义）：非 Apple 来源，未采信。

## 7. Windows NT

来源：learn.microsoft.com（WDK/内核/Win32 页）。

- 句柄值即句柄表索引；表进程私有；属性位含 OBJ_INHERIT、KERNEL_HANDLE 等；Win32 层 HANDLE_FLAG_PROTECT_FROM_CLOSE 置位后 CloseHandle 不生效。
- **ACL 检查在打开时**：open 时 DesiredAccess 与安全描述符核对，返回句柄携带授予的访问权（表项 GrantedAccess，`!handle` 可见）；使用时按句柄记录执行。
- `DuplicateHandle`：DUPLICATE_SAME_ACCESS 或按 DesiredAccess 造**不同访问权**句柄（收窄可行；扩权仅部分类型可行）；源/目标进程句柄均须 PROCESS_DUP_HANDLE；DUPLICATE_CLOSE_SOURCE 无关关源；IOCP/socket 不可复制。
- 跨进程传递：持对方进程句柄 + DuplicateHandle 装入对方表 + **自行用 IPC 传句柄值**（官方 Remarks）。ALPC 句柄传递无公开官方文档（未核实）。
- Job object 会计：CPU/页错误/进程计数/IO——**无任何句柄层面会计**（结构清单的结构性缺失）。
- `ObRegisterCallbacks`（仅 Process/Thread/Desktop 类型）：pre 回调**只能削减不能增加** DesiredAccess；须签名内核二进制。
- 生命周期：引用计数 + 句柄计数配对；句柄计数归零 → 对象自命名空间移除（引用 >0 时仍可按指针访问）；OBJ_PERMANENT 使对象自持引用。

## 8. Linux

来源：man-pages 6.19 + docs.kernel.org。

- fd 表进程本地；dup 分配最低未用编号，指向同一 open file description（共享偏移与 status flags，不共享 fd flags）；SCM_RIGHTS 传递「语义上等价于把 fd dup 进另一进程的 fd 表」，单条上限 SCM_MAX_FD=253，在途超 RLIMIT_NOFILE 且无 CAP_SYS_RESOURCE → ETOOMANYREFS；unix_gc 细节未核实。
- pidfd：对任务的稳定引用（PID 复用竞态的解法）；`pidfd_getfd`（5.6）在调用者表中为目标进程 fd 造 duplicate——权限要求 PTRACE_MODE_ATTACH_REALCREDS。
- io_uring：`IORING_REGISTER_FILES` 注册 fd 数组供 ring 长期引用；SQE 置 IOSQE_FIXED_FILE 后 fd 字段 = 数组索引；direct descriptor（5.15+）由独立注册表管理，`IORING_FILE_INDEX_ALLOC` 由内核分配——**授权检查在注册时，稳态走索引**。
- 粗粒度 ambient authority：namespace/cgroup 委派（目录写权限或 nsdelegate）；Landlock（5.13+）无特权自我收权、策略只可加严；seccomp BPF 按 syscall 过滤（须 no_new_privs）；pledge 属 OpenBSD。
- **无撤销已共享 fd 的机制**（unix.7/close.2 语义组合结论）；memfd seals 是对象侧单调约束（seal 永不可移除只可添加、加封后内核立即强制），不是 handle 侧机制。

## 9. 研究谱系与替代机制

### KeyKOS → EROS → Coyotos

- key 只存于 slot；domain 既是 key 集合又是执行体。**start key** 只向 available domain 投递消息、不含检查权；**resume key** 由 CALL 隐式产生、任一副本被调用全部变 null（at-most-once）。
- **brand**：root node 中的唯一 key，「never invoked, but only compared for equality」；gate key + brand 匹配换取内部 key = 「primitive rights amplification mechanism」。
- 撤销模式：node range/bank 整体失效；EROS space bank（销毁子系统的存储源即整体死亡）；**选择性撤销 = 用户态转发域**（KeyKOS red segment → EROS wrapper node → Coyotos wrapper 一等内核对象，控制接口可替换所包 capability）；sense/weak key 弱化；Coyotos allocation count（资源级：revoke 时指涉该资源的全部 cap 变 Null）。
- factory/constructor：KeyKOS factory 凭 components 登记即可递归认证 discretion；EROS constructor 仅凭初始能力集即可判定 confinement 且已形式化。
- 持久化：KeyKOS 全系统 checkpoint（能力只存于 node，journal 不允许）；EROS 磁盘 cap = OID + version + allocation count 匹配才有效。

### Twizzler

- 128 位全局对象 ID（「a program can only access an object whose ID it knows」——知情性即访问控制）；权限记录在持久 sctx 对象（按对象 ID 的 rights 列表）；capability 是纳入对象 ID 签名的 token。
- 内核不在数据路径：跨对象指针翻译由用户态 libtwz 完成，映射不进内核；检查为 late-binding（非法操作发生时才 fault）。已映射对象的异步收回机制未核实。

### M3

- DTU 端点是「硬件表示的 capability（similar to CHERI）」；配置寄存器仅内核 PE 可写，建立后数据路径无内核；label 由接收方选定、发送方不可伪造（源自 KeyKOS）。
- Delegate/Obtain/Revoke（递归撤销全部 grant），依赖内核委托树；当前实现为内核向远程 TCU 写 InvEP 命令逐个失效。内核侧仍保留每 VPE 一张 capability 表（类比 fd 表）。

### managarm / Hel

- 实际模型为 **Universe + descriptor**（非 portal）：`Universe` "Maps handles to descriptors"，handle = 低位 20 bit 槽索引 + 高位 per-slot generation，RCU 无锁查找；descriptor = {type（24 种）, extra, rights:u32, object*}，解析时类型+权限双检。
- 官方博客（2026-08-22）：「rights are per-capability and not per-object… Rights are also monotonically decreasing by design」；14 个 rights。
- Universe 本身是内核对象：`helCreateUniverse`/`helTransferDescriptor`（newRights = originalRights & exposedRights）；POSIX 层每进程建 Universe，fd 表为其用户态只读映射窗口；capability 随 lane 消息传递。

### Redox

- scheme/URL 模型：内核把 (pid, fd) 路由到 (scheme 侧 pid, handle)；**授权两段式**——内核只管 namespace 可见性（null namespace 进程「can only use file descriptors that are already open」），权限判定由 scheme 处理者做（`CallerCtx{pid,uid,gid}`）。
- 官方表态（NLnet 博文 2026-03-17）：「an open file descriptor is a capability」；namespace 与 CWD 改为 fd capability，内核不再需要 scheme 名。

### Plan 9

- bind/mount 修改当前进程（组）的 name space，bind 成功后 old 是对象的 alias——**bind 即授权**；9P attach 经 afid 认证。
- Bell Labs 官方资料全文检索 "capab" 零命中——官方从未将其表述为 capability；plan9front《nssec》指出该视角的代价：整个 capability list 经 `/proc/$pid/ns` 对外可见。

### Exokernel 与 CHERI

- Exokernel secure binding：「decouples authorization from the actual use」；授权只在 bind 时一次；visible revocation 为对话式。
- CHERI capability 寄存器 = tag + object type + permission mask + base/length/offset（128+1 bit）；撤销三路径：页粒度虚地址撤销、sweeping revocation（tag 精确清位，配 dirty bit）、sealed 能力的软件 interposition；CheriBSD `cheri_revoke(2)` = shadow bitmap + revocation epoch；官方定位：「complement one another well」——VM 承载粗粒度进程模型，CHERI 做进程内部分舱。

### Capability Myths Demolished

- 论文实名 **Demolished**（无 "Considered" 版本）；Miller/Yee/Shapiro，SRL2003-02，JHU，2003。
- 七性质：A No Designation Without Authority；B Dynamic Subject Creation；C Subject-Aggregated Authority Management；D No Ambient Authority（ambient = "exercised, but not selected"——Unix open() 即典型）；E Composability of Authorities（无 E 则无法构造可撤销 forwarder）；F Access-Controlled Delegation Channels（confinement 的前提）；G Dynamic Resource Creation。
- 三 myth 全驳：ACL≡capability（假，A–C 区分）、capability 无法 confinement（假，Shapiro–Weber 形式化）、capability 不可撤销（表面属实但 forwarder pair 补足）。
- **comparability 与 monotonicity 不在该论文中**（全文 0 次出现）——引用这两性质须另找出处。

## 10. 跨维度对照

| 维度 | 收敛主流 | 离群点 |
|---|---|---|
| 寻址/表所有权 | 内核管理进程本地表 + generation：Zircon、Mach、NT、Linux、managarn、M³ | seL4 用户态 CSpace（可用性代价被官方承认）；Genode 内核表但零权限位；NOVA 平坦区间委托 |
| rights | Zircon 26 位逐调用检查；seL4 4 位；NOVA 每类型 6 位；managarm 单调收窄 | Genode 零权限位（ownership 即权威） |
| 转移 | 消息原子携 handle + per-handle 最终 rights（Zircon disposition）；TRANSFER 权利门控 | seL4 转移是复制且单接收槽；NT 无消息内传递；Mach disposition 词表最全（含 MAKE_SEND 从所有权铸造） |
| 撤销 | 无撤销（Zircon/Linux/NT）+ 对象死亡信号 | seL4 CDT 修剪（成本被官方承认）；Genode owner 销毁全局移除；Coyotos 资源代次；用户态 forwarder（KeyKOS 系）；CHERI sweeping |
| badge | Zircon/NT/Linux/Mach 无；seL4 mint badge、M³ label、KeyKOS brand | — |
| introspection | Zircon 全表枚举、Mach port names、Linux /proc | seL4 仅 debug 构建 |

## 11. 与 Halcyon 现状的对照要点（事实性）

- Halcyon 的 slot+generation、check-at-use、rights 单调收窄、TRANSFER 门控消息运输、对象能力非层级，与 Zircon/Mach/managarm 逐项同构；managarm（20b 槽 + per-slot generation + RCU）为独立收敛的直接证据。
- Halcyon 满足 Capability Myths 性质 A/D/F：内核面权威全部经 handle、无按名打开；mailbox TRANSIT 为受控委托信道；Job seal 支撑 confinement。
- 差异点（供分案权衡，非结论）：① Halcyon 只有 per-handle HandleQuery，无 Zircon `ZX_INFO_HANDLE_TABLE`/Mach `mach_port_names` 式的有界自表枚举；② Genode 把 cap 槽位作为显式记账资源（quota 转账），Halcyon handle 槽消耗内核堆、归属在 [内核内存预算](todo-2026-09-14-kernel-memory-budget.md) 中的登记情况未核对；③ Zircon 对用户可见 handle 值做每进程 PRNG XOR（Halcyon handle 仅本进程表内有意义、转移不经裸数值，暴露面更小）；④ 撤销立场与 Zircon 相同（无派生撤销，靠对象死亡/退休），若未来需要选择性撤销，ocap 文献的标准答案是用户态 forwarder 而非内核派生树。

## 参考来源清单

- seL4 Manual v16.0.0（sel4.systems/Info/Docs/seL4-manual-latest.pdf）；MCS Manual 10.1.1；白皮书（sel4.systems/About/seL4-whitepaper.pdf）；seL4/seL4 master（libsel4/include/sel4/{constants,types}.h、include/api/types.h）
- L4Re：l4re.org/doc/{l4re_concepts_mapping,l4re_concepts_naming}.html、classL4Re_1_1Cap__alloc.html
- Genode Foundations 26.05：architecture/{Capability-based_security,Resource_trading,Core_-_the_root_of_the_component_tree}.html、under_the_hood/Parent-child_interaction_in_detail.html
- NOVA：raw.githubusercontent.com/udosteinberg/NOVA/release/doc/specification.pdf（§2/§4/§5/§6.3）；hypervisor.org/eurosys2010.pdf（§5–6）
- Zircon：fuchsia.dev/fuchsia-src/concepts/kernel/{concepts,handles,rights}、reference/syscalls/*、docs/reference/kernel_objects/objects.md；源码 zircon/kernel/object/handle{,_table}.cc
- Mach：developer.apple.com KPG（Mach 概览/boundaries）；github.com/apple-oss-distributions/xnu（osfmk/mach/{port.h,message.h,mach_port.defs,notify.h}、osfmk/ipc/{ipc_right,ipc_port,ipc_entry}.c、osfmk/kern/}; launchd 仓 bootstrap.h
- Windows NT：learn.microsoft.com kernel/{object-handles,access-rights,life-cycle-of-an-object}、wdm/nf-wdm-obregistercallbacks、api/{handleapi,winnt,ntdef}、secauthz/*、procthread/job-objects
- Linux：man7.org man-pages 6.19（dup.2、fcntl.2、unix.7、pidfd_*.2、io_uring_*.2、ptrace.2、namespaces.7、landlock.7、memfd_create.2、F_ADD_SEALS.2const）；docs.kernel.org admin-guide/cgroup-v2、userspace-api/{landlock,seccomp_filter}
- KeyKOS/EROS/Coyotos：cap-lore.com/CapTheory/KK/（OSR'85、Factory、DT、Rescind）；SOSP'99（doi:10.1145/319344.319163）；Landau OOOW'92（checkpoint）；coyotos.org/docs/ukernel/spec.html、docs/misc/eros-comparison.html
- Twizzler：usenix.org/system/files/atc20-bittman.pdf；twizzler-operating-system.github.io/nightly/book/
- M³：os.inf.tu-dresden.de/papers_ps/asmussen-m3-asplos16.pdf；github.com/Barkhausen-Institut/m3
- managarm：github.com/managarm/managarm（kernel/thor/generic/{thor-internal/universe.hpp,hel.cpp}）；managarm.org/2026/08/22/0-capability-rights.html
- Redox：doc.redox-os.org/book/{schemes,scheme-operation}.html；redox-os.org/news/nlnet-cap-nsmgr-cwd/
- Plan 9：9p.io/magic/man2html/{2/bind,5/intro}；plan9.io/sys/doc/names.html；git.9front.org plan9front sys/doc/nssec.ms
- Exokernel：pdos.csail.mit.edu/papers/exokernel-sosp95.ps；pdos.csail.mit.edu/papers/exo-sosp97/
- CHERI：cl.cam.ac.uk/techreports/UCAM-CL-TR-951.pdf、TR-941；man.cheribsd.org cheri_revoke.2
- Capability Myths Demolished：SRL2003-02（web.archive.org 存档 srl.cs.jhu.edu/pubs/SRL2003-02.pdf）

## Gaps（未核实/存疑）

1. seL4：MCS 是否移除 CDT（v16 手册仍载 CDT）；extraCaps 数值上限出自 libsel4 常量而非 manual。
2. Fiasco.OC：MAP DB 内部术语与数据结构；unmap 最坏成本。
3. NOVA：撤销内部实现；2008 早期 API 与现行规范的完整对照。
4. Genode：cap slot 分配的具体实现（文档明言取决于底层内核）。
5. Zircon：单进程 handle 上限；部分 syscall 页无修订日期。
6. NT：句柄表增长机制与上限数值（仅 Windows Internals 等非官方来源）；ALPC 句柄传递。
7. Linux：unix_gc 在途 fd 回收细节。
8. Twizzler：metadata security 完整机制；已映射对象的异步收回。
9. plan9front nssec 作者/日期。
