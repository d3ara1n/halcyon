# 持久 init 监督政策 Review 计划

> 【未来审查计划】对象是生命周期 step 6 的两笔提交；Review 纪律见 [`REVIEW.md`](REVIEW.md)。设计公理已入档 `notes/ideas/bootstrap.md`（委托语义、重启政策维度、init supervisor），实现现状见 `notes/impls/startup.md`「当前 init 集成政策」与 `notes/ideas/system-reset.md`。

## 提交对照

| 提交 | 内容 |
|---|---|
| `fcbd5b6` | feat(systems)：step 6 主体——root → services → pm_domain/acceptance 拓扑、pm 委托域管理段（枚举→派生铸造→kill→drain→seal）、监督闭环、失败整树收束、稳态不自终止、拓扑快照打印、notes 两篇 |
| `b161163` | feat：fs 前缀表自打印、sifive_u 窗口 5s、THROTTLE 油门经环境变量修复 |

## Review 轴（代码为主）

### 显式 reset 后的 supervisor 语义

- 正常终局不再依赖 TimerQueue 或 IPC 等待者枚举：init 只在业务与资源锚点成立后提交 `SystemReset`；review 需确认提交点严格位于监督闭环之后，且 authority 未被 pm 委托域意外持有。
- 平台拒绝 reset 时，稳态 endpoint 的消息或等待错误都只能记录并继续循环，不能让 `main` 返回导致 init 退出、管理根失效。

### GRANT 消费与 authority 留存

- launch 的 duplicate-then-grant 顺序是正确性前提（GRANT 直接跨表安装、源 handle 被消费）：核对 init 对 pm_domain 的复制件确实在 grants 之外独立持有，兜底 job_kill 的 authority 链完整。
- pm 域内成员 control 即弃 → pm 派生走铸造路径。若未来 spawn 时保留 control，铸造路径变为 shell 复用——两条路径的 REAPABLE 电平重放语义均已被 step 5 覆盖，此处只需确认委托语义没有依赖「control 必弃」。

### 委托权利面

- MANAGE|READ|WAIT（无 CREATE）是当前 pm 管理面的精确权利：pm 只收束不扩张。pm 协议接入 spawn 委托时需重开 CREATE，届时重审权利面与域拓扑（受托域是否应改由 pm 自建子域）。
- acceptance 域用 JOB_FULL_RIGHTS 创建后即 seal 收净——确认收束失败（`acceptance collection degraded`）路径下无成员泄漏（job_kill 的 CLOSED 屏障已保证，日志降级只是观测）。

### 观察性打印的边界

- `dump_topology` 的派生即查即关在多核竞态下只是尽力而为的快照，不构成协议依据；step 9 验证矩阵若重组验收自测，保持「终态拓扑干净」（services 空）不变量。
- fs 前缀表打印是开发期自观测，正式服务编排（manifest/另案）落地时评估去留。

### 失败路径资源计平

- failure-path `job_kill(services)` 后 init 直接进稳态：局部 handle（pm_mailbox 剩余端、delegated_domain 未消费副本等）随 init 存续持有，不回收——确认这符合「稳态持权、不追求进程内清零」的定位，不为打印/失败路径引入额外 close 仪式。
