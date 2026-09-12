# 库存来源与启动自检代码复核

## 对象与结论

本报告复核切片 10 的完整未提交实现，基线为 `e973763`；不属于固定提交的未来 Review，也不审查方案。独立 reviewer 使用角色默认模型 `pttt-openai-responses/gpt-5.6-sol`，只读检查代码和已有验证日志。

未发现可确认的 bug、回归、unsafe 违规或旧 raw 分配路径残留，无开放 finding。本报告归档；提交后如需固定提交复核，按实际哈希另登记。

## 核对范围

- `os/kernel/src/frame.rs`：删除普通 raw allocation adapter/tracker；`BootHeldExtent` 直接持有几何，adopt 保持启动来源前置，split 先验证再转移，Drop 仅归还仍持有的范围。`BootFundedExtent` 字段顺序继续保证物理先归还、charge 后退款。
- `os/kernel/src/frame/selftest.rs`：私有写脏来源委托真实 claim，不伪造 clear 状态、不自行清零；同一 owner 完整范围写脏和读验后，由真实 broker clear 并全范围验证零态。正式入口、表页、extent-limit rollback 与两种切分释放顺序均核对精确静止点账本。
- `os/kernel/src/task/memory_pool.rs`：child 支付真实表页，外部强引用消散后 funded owner 保活来源池，最后 owner 消散才触发父级退款；临时 Weak upgrade 不跨语句保活。
- `os/frame_pool/tests/pool.rs`：部分重叠归还的前半/后半两种布局均在修改前拒绝，精确再取证明拒绝不修改库存。
- `os/funded_frame/tests/broker.rs`：逐 owner 释放后的物理与 quota 中间账本期望正确。
- `os/kernel/src/{main,boot,rt}.rs` 与 `tools/qemu-acceptance.sh`：统一入口在 root 建立后、Ready 发布前执行，secondary 无并发 funding；成功锚点在全部 frame 自检完成后输出，并由正常验收路线强制检查。

## 证据边界

- 启动 Pool/frame 快照证明静止点资源守恒；精确析构先后、失败前不清零由 broker host 事件模型证明。
- 切分后存活侧直映射内容读写验证实际访问范围，但直映射不会随库存归还失效，因此读写成功不能单独证明独占所有权。切分所有权正确性还由实际 affine split/Drop 几何路径和纯库存测试支持。
- 连续四页是当前两平台启动 fixture 的前置，不扩展普通 funding ABI 的连续性承诺。

## 验证

- `just check` 与相关四个逻辑 crate 的 host debug/release 通过；完整日志 `artifacts/frame-source/{check,host-debug,host-release}.log`。
- 默认 50% 节流 `just acceptance` 通过：七面 clippy、debug stress 16/16、release core、sifive_u core、nofd、panic/alloc/fatal 三类启动 Failed；日志 `artifacts/frame-source/acceptance.log`。
- debug/release 最大栈帧 `0x26f0` / `0x1250`，布局派生的 12KiB 限额和每 hart 256KiB 栈不变；结束后无 QEMU/GDB 残留。
- reviewer 读取已有日志，未重新运行长验收、未修改代码。
