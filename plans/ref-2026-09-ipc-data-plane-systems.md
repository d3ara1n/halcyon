# IPC 数据面系统参照

> 【只读参考资料】2026-09 为 Halcyon IPC 数据面设计收集的外部系统事实摘要。参照只用于拓宽选项，不替 Halcyon 作设计推导；最终决策见 [`archived/todo-2026-09-ipc-data-plane-design.md`](archived/todo-2026-09-ipc-data-plane-design.md)。

## 共同收敛

- Zircon、QNX、seL4、L4Re、Genode 都区分小控制消息/能力运输与多页共享数据；Twizzler 进一步把数据对象作为主要访问单元。
- 跨地址空间稳定引用使用对象内 offset、region id 或预注册缓冲区，不把进程本地 Handle、VA 或裸指针写进共享协议。
- 门铃通常是可合并的电平/位集合，只提示重查共享状态，不携带数据量。
- “零拷贝”只来自共享数据区；控制消息仍复制或走寄存器 fast path。
- 原始共享内存同时带来 TOCTOU、同步、生命周期和故障归属问题，不能仅凭映射存在就声称安全 ownership。

## 数据结构差异

| 系统 | 数据面 | 队列/所有权事实 | 对本轮选项的边界事实 |
|---|---|---|---|
| Zircon/Fuchsia | VMO、IOBuffer region | FIFO 只适合作共享数据控制面；IOBuffer 把多 region 与端点生命周期组合 | 内核多区域对象可减少对象拼装，但扩大特权状态与受管操作 |
| QNX Neutrino | shared memory handle + mmap | 消息携 shm 引用、offset、length；MsgSend 本身复制 | 共享内存不是默认替代消息，复杂度必须由大载荷收益支付 |
| seL4/Microkit | frame mapping | Notification 是粘滞位；bulk 不走 endpoint IPC | 控制与 bulk 分离，不要求通用 descriptor ring |
| Fiasco.OC/L4Re | Dataspace + region map | attach 接对象 offset；l4shmc 提供共享 ring/signal | VA 可不同，裸指针不成立 |
| Genode | RAM dataspace | packet stream = bulk buffer + submit/ack descriptor queue | submit/ack 很适合显式 buffer 交接；signal 不携负载 |
| Twizzler | 持久对象 | `FOT index:offset` 跨 view 稳定 | 对象相对引用成立，但持久单地址空间假设不直接搬用 |
| managarm | lane + 共享通知区 | 因乱序完成使用 chunk 区 + index queue，而非单一数据环 | descriptor 形态由完成顺序决定，“一个环”不是普遍答案 |
| Hubris | 同步 rendezvous + lease | sender 阻塞期间可借出内存，reply 原子撤销；异步只发 notification | lease 的硬撤销依赖同步调用前提，不能套到异步 Tunnel |
| RedLeaf | shared heap + RRef | move/不可变 borrow 由 safe Rust/IDL 保证，禁止可变 borrow | 语言 ownership 不能约束不可信进程仍可写的共享映射 |
| Theseus | 单地址空间语言共享 | MappedPages move；禁止语言层以下 alias | 范式不同，可说明别名风险，不能替多进程页表契约 |
| Asterinas | Frame/Segment、untyped memory | 用户/DMA 可改内存不能建立普通 Rust 引用 | DMA buffer 必须另有 untyped/硬件契约，不能冒充普通对象 |
| KeyKOS/EROS/CapROS | space bank/meter | authority、存储预算、执行预算分离 | 资源预算应是显式对象，不应隐含进 Job/PID 层级 |

## Capability 与资源预算

- seL4 untyped、L4Re factory quota、Genode RAM/cap quota、KeyKOS/EROS space bank 都把“能访问什么”与“能分配多少”分开。
- capability move 改变 authority 可达性，不必改变原资源账户；若随接收方重记费用，会引入跨域转移的双重事务和撤销难题。
- 资源池/账户可以派生更小预算，普通对象保留来源 charge，最终释放时退款；这与对象 Handle 是否仍在创建 Job 内正交。

## 主要来源

- Fuchsia VMO/FIFO/IOBuffer RFC: https://fuchsia.dev/fuchsia-src/concepts/memory/address_spaces ; https://fuchsia.dev/reference/syscalls/fifo_create ; https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0218_io_buffer
- QNX shared memory/messages: https://qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Shared_memory_and_messages.html
- seL4 IPC/Notification/MCS: https://docs.sel4.systems/Tutorials/ipc ; https://docs.sel4.systems/Tutorials/notifications.html ; https://docs.sel4.systems/Tutorials/mcs.html
- L4Re Dataspace/IPC: https://l4re.org/doc/l4re_concepts_ds_rm.html ; https://l4re.org/doc/l4re_concepts_ipc.html
- Genode inter-component communication: https://genode.org/documentation/genode-foundations/24.05/architecture/Inter-component_communication.html
- Twizzler ATC 2020: https://www.usenix.org/system/files/atc20-bittman.pdf
- managarm FGBS 2024: https://dl.gi.de/server/api/core/bitstreams/dabcf748-5ee0-4cbe-a2b5-cb2576fa8113/content
- Hubris Reference: https://hubris.oxide.computer/reference/
- RedLeaf OSDI 2020: https://www.usenix.org/system/files/osdi20-narayanan_vikram.pdf
- Theseus OSDI 2020: https://www.usenix.org/system/files/osdi20-boos.pdf
- Asterinas ATC 2025: https://www.usenix.org/system/files/atc25-peng-yuke.pdf
