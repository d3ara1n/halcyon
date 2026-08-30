# 外部参考索引

本文件只负责导航。资料的证据等级、使用边界和更新纪律见 [`README.md`](README.md)。

| 入口 | 内容 | 用途 |
|---|---|---|
| [`CONTRACTS.md`](CONTRACTS.md) | SBI、RISC-V ISA、psABI、Devicetree 与固定实现用法索引 | 硬件、ABI、协议设计和 review 的固定版本取证 |
| [`systems/INDEX.md`](systems/INDEX.md) | 微内核、分离内核、单体/混合内核及邻近架构全景 | 发现系统、比较设计、选择后续专题 |
| [`MANIFEST.md`](MANIFEST.md) | 入库上游副本的版本、commit、来源、许可与裁剪范围 | 复现和更新固定语料 |

`implementations/` 中的 Linux、OpenSBI 和 QEMU 快照服务于具体指令/API/平台行为取证，不因此成为系统设计结论；同一系统若具有架构参考价值，会另行出现在 `systems/`。