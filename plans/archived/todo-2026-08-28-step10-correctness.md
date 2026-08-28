# 生命周期 Step 10 文档收口前正确性修复

## 目标

生命周期 step 10 只描述可成立的终态契约。文档同步前先闭合四个实现缺口：ProcessStart 前提交事务、WaitContext timeout registration 生命周期、ProcessDrain 最小预算进展、Tunnel Invitation 非等待角色表面；同时把 WaitMany 的相对时间参数统一命名为 Timeout。

本批不决定设备 IRQ 的具体投递模型。`ideas/device.md` 只确立资源 capability 与用户态授权政策的边界；IRQ 等待、确认与投递在设备/中断接入设计时统一决定。

## 设计决策

### 1. ProcessStart 是请求顺序保持的零分配提交事务

提交前必须完成所有可失败工作：

1. 复制并验证 descriptor、payload 与 grants；
2. 为按请求顺序承载 grant entries 的 Vec 预留完整容量；
3. 预留 child Handle slots；
4. 构造并映射 StartupBlock；
5. 构造主线程并预留 Ready 容量；
6. 在调用者 HandleTable 内原子验证并 pin builder 与 grants。

Job 链锁内的 `Building → Running` 是唯一提交线性化点。提交后 HandleTable 按 `grant_pairs` 的请求顺序定点取出 pinned entries，直接提交到对应 child slots；builder 单独取出并消费。该阶段不扫描聚合、不分配、不可失败。

任一提交前失败都撤销 caller pins、Ready reservation、StartupBlock 映射和 child Handle reservation；builder 与 grants 对调用者保持原值，目标仍可重试 Start。

### 2. Timeout registration 由稳定 token 唯一标识

公开 WaitMany 参数是相对毫秒 `timeout_ms`，零表示无限；完成原因是独立的 `Timeout`。内核在安装等待时把相对时长换算为单调时钟绝对到期点，但不把内部时间点术语泄漏到 ABI。

每个 hart 持有独立 timer queue。队列采用索引最小堆：

- 稳定 token 包含 owner hart slot、arena slot 与 generation；
- 注册、注销、到期弹出为 O(log n)，读取最早到期点为 O(1)；
- arena 槽位复用时 generation 前进，旧 token 幂等失效；
- 注册前为 arena 与 heap 的必要增长预留容量，OOM 零副作用；
- 队列是纯逻辑结构，放入独立 host 可测 crate。

WaitContext 以原子状态保存 `Unregistered | Token | Closed`。注册与完成按 CAS 仲裁：

- 注册先取得 queue token，再把 `Unregistered` 发布为 `Token`；若完成已发布 `Closed`，立即注销 token；
- 对象命中、错误、显式取消或终止完成方把状态交换为 `Closed`，取得 token 者负责注销；
- timer queue 弹出到期项后以 token 通知 context，只有成功退休该 token 的路径参与 Timeout outcome 仲裁；
- 注销与到期竞争幂等，任何路径都不会遗留强持 WaitContext 的 queue entry。

跨 hart 完成只锁 owner queue 删除条目，不远程重编程 owner 的 timer。删除最早项至多导致一次提前时钟中断；owner 在下一调度/idle 装填点按堆顶重新编程，不会睡过后续期限。

### 3. ProcessDrain 的每个 work unit 必须产生可恢复进展

Handle 收束仍按“扫描一个槽位 = 1、执行一次 close callback = 1”计费。Process 增加由 `drain_gate` 串行保护的 `pending_close`：

- 批次先关闭已有 pending entry；
- 扫描取得 entry 而剩余预算不足时，把 entry 存入 pending，游标已经前进；
- 后续批次先消费 pending，再继续扫描；
- HandleTable Exhausted 且无 pending 后才进入 AddressSpace drain；
- 防御性 Drop 同样关闭 pending entry。

因此任意 `max_work > 0` 都会推进至少一个可恢复步骤；返回 `More` 时 `work_done > 0`，总工作不超过预算。

### 4. Tunnel Invitation 当前不是可等待对象

Invitation 只承担一次性 `MAP | TRANSIT | GRANT` 授权。它保留关闭/放弃状态以仲裁 attach，并在放弃时向创建端 Endpoint 发布 `PEER_CLOSED`；不公开 `WAIT`、ObjectSignals 或订阅队列。

这只定义当前角色表面，不宣告永久禁止等待。未来若真实协议需要观察 Invitation 终态，须连同 rights、完成语义与唤醒验证单独设计，不保留不可达的预实现表面。

### 5. 设备文档只收束授权边界

平台根由可信平台事实铸造 MMIO、IRQ source、DMA window 等资源 capability；用户态资源管理服务负责设备匹配、最小授权、驱动重启与重新租借政策；内核不向用户 handler 注入执行。

IRQ source 是直接可等待对象、绑定 Notification，还是采用其他投递机制，取决于后续对 mask/ack、共享 IRQ、MSI/MSI-X、合并/溢出、撤权与设备 reset 的完整设计。本批不得提前写死。

## 实施顺序

1. 新增并 host 测试索引最小堆 timer queue；
2. 接入 per-hart timer queue 与 WaitContext token 仲裁，统一 Timeout 命名；
3. 重构 HandleTable Start pin 的请求顺序提交与零分配提交区；
4. 修复 ProcessStart 全失败面回滚；
5. 接入 ProcessDrain pending close；
6. 删除 Invitation 非法等待表面；
7. 完成定向 host 与 QEMU debug/release 验证；
8. 回到 lifecycle step 10，同步并重组 notes。

## 验证

- timer queue：乱序注册、相同期限、取消堆顶/中部/尾部、generation 复用、到期与取消竞争模型、OOM 零副作用；
- WaitContext：对象提前完成、Abandoned、Timeout 三路都立即使 queue live count 归零，长 timeout 不阻止 quiescent；
- ProcessStart：grant 请求顺序独立于 caller slot 顺序；每个可失败点后 builder/grants、child slots、StartupBlock、Ready reservation 均无残留并可重试；
- ProcessDrain：含 Handle 的目标用 `max_work=1` 循环收束，每个 More 均有正进展并最终 Complete；
- Invitation：WaitMany 因缺少 WAIT 拒绝，关闭/放弃/attach 既有语义不变；
- 全线：`just check`、`cd shared && cargo check`、全部 host 纯逻辑测试、`just virt`、`just virt-release`；涉及多域调度与平台收口时补跑 hetero/nofd/sifive_u。

## 实施收口（2026-08-28）

全部实施项已完成。独立审查另外发现并闭合：WaitContext 在 `clear_active → park_waiting` 窗口被终止时的 Installing 离场遗漏；Invitation 初始 Handle 缺 GRANT；AddressSpace Root drain 的另一处最小预算零进展；QEMU 正常退出掩盖 acceptance 失败。

验证终态：

- `just check`、shared check、user build 与全部 host 纯逻辑测试通过；
- `virt`、`virt-release`、`virt-hetero`、`virt-nofd`、`sifive_u` 均命中最小预算 Drain、竞态矩阵 10/10、服务监督、委托域与 quiescent 锚点；
- `tools/qemu-acceptance.sh` 按 common/hetero/nofd profile 校验成功锚点并拒绝 panic/acceptance failure；sifive_u 只有锚点齐全时才接受平台 timeout。
