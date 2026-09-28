# Halcyon std 平台层（长期方向）

> 状态：仅登记方向，未设计。

## 方向

为 `target_os = "halcyon"` 实现 Rust std 的平台层（PAL），经 build-std 构建 sysroot，使依赖 std 的第三方 crate 可用。系统程序模型仍由分散组合的 libkernel/libthread/libprocess/libruntime 等库拥有，std 只是生态兼容的薄适配。

## 已知冲突

std 预设的形状与已定契约不符：`std::process` 的退出码与 argv/env、`std::thread` 的 detach/TLS、`std::fs` 的全局路径命名空间（FAL 为能力式访问）、`std::io` 的阻塞读写。这些接口只能返回 Unsupported 或另行设计映射，不得为兼容 std 反向修改系统契约。

## 触发与起点

触发：出现必须依赖 std 的真实第三方需求。届时先评估最小 PAL（alloc/thread/time），fs/process 保持 Unsupported；现有 `fn main` 形状与 `startup::take()` 迁为 std 平台扩展，应用代码尽量不变。
