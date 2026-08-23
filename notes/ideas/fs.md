# 文件系统

文件系统分为内核文件系统和具体文件系统，统一使用 FAL 与用户进程交互。

## 内核文件系统

挂载文件系统并不是由 FAL 要求的，事实是只有 Rootfs 支持文件系统挂载，如果要在其他文件系统上创建挂载点，请先将其挂载在 Rootfs 上并在该文件系统上 Link 到 Rootfs 中的挂载点。
挂载文件系统需要用特殊的 `Mount` 和 `Unmount` 系统调用，而非 `Create`。挂载操作仅限 Rootfs。

目前内核支持以下文件系统，和其对应在 Rootfs 中的挂载点：

|Filesystem|Mountpoint(at rootfs)|Attributes|Note|
|-|-|-|-|
|Rootfs|`/`|`rWx`|根目录文件系统，能创建内存文件和目录，提供挂载其他文件系统(并转发)的能力|
|Procfs|`/proc`|`r-x`|进程列表，包含实时进程信息|
|Sysfs|`/sys`|`r-x`|系统信息|

除此以外 Rootfs 还提供特权写的内存文件如 `/initfs` 和 `/devicetree`
