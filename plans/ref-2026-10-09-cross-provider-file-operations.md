# 跨文件服务边界的 Move/Copy 取证

取证日期：2026-10-09。用途：为 [OAL/fs/服务目录重构](todo-2026-10-07-fal-service-redesign.md) 后续 fs 设计提供事实；不表示 Halcyon 已选择或实现相同语义。本文是只读参考快照，设计决策由所属计划拥有。未运行这些系统或验证它们的崩溃行为。

## 已核实的区别

- rename/move 原语修改名字关系；跨文件系统的用户态 `mv` 可以由复制和删除组合。组合不因此具有整个迁移的原子性。
- 进程边界、协议入口、文件系统实例、挂载视图不是同一个边界。同一服务进程并不充分保证能共同提交 rename；不同进程也不从理论上排除专门协议，但以下样本的公开机制没有给任意 provider 提供通用原子迁移。
- 复制协调可以在调用者，数据搬运可以由后端加速。没有跨 provider 原子 rename，不能推导为所有复制都必须经用户缓冲逐字节转发。

## 样本与证据

| 系统 / 固定依据 | rename 边界与 IPC 表达 | 跨边界复制、搬移的证据范围 |
|---|---|---|
| QNX SDP 8.0 官方文档 | `rename()` 对不同文件系统返回 EXDEV；资源管理器 `io_rename` 文档描述调用它时两名字位于同一 filesystem/device | 文档区分 libc rename 返回错误与 shell mv 的 cp/rm 组合；不把旧版工具的部分失败细节套到 8.0 toybox |
| GNU Hurd `1:0.9.git20260919-2` 的 libdiskfs 源码（Debian 固定包），配合官方参考手册 | dir_rename 携两个目录 port；目标 port 不归同一 translator 时 EXDEV，同一 translator 的不同 shadow_root 也 EXDEV | 官方接口措辞是不同 filesystem 时“might fail with EXDEV”；固定 diskfs 实现核实了拒绝路径，不能泛化为所有 translator 必须采用相同政策 |
| Genode 25.05 官方源码 | VFS rename 显式检查同一 filesystem instance；不匹配可返回 RENAME_ERR_CROSS_FS。File_system session 的 move 接收同一 session 下两个目录 handle 和名字 | 该 session 接口没有第二个 session capability 或联合迁移/copy RPC；不能据此声称所有实现都没有内部复制优化。未核实某个具体 mv 工具的失败清理 |
| MINIX v3.3.0 官方固定 tag | VFS do_rename 比较两个父目录的 v_fs_e，不同则 EXDEV；通过检查后，一次 req_rename 向该 fs_e 发送两个 inode 与名字 | 已核实 VFS 门禁和单服务请求，未逐行审计 MFS 内部多实例限制或特定 mv 工具 |
| Linux v6.12 固定源码 | do_renameat2 对 old_path.mnt != new_path.mnt 返回 EXDEV；同一底层文件系统的不同 mount 也可能被此检查拒绝 | POSIX.1-2024 和 GNU Coreutils 在线手册明确区分 rename 与跨文件系统的复制后删除。Linux copy_file_range 另提供数据复制加速，见下文 |

## 关键原文与源码

### QNX

- [SDP 8.0 rename()](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/r/rename.html)：EXDEV — “The files or directories named by old and new are on different filesystems.” 本轮主会话复核取得该原文。
- [SDP 8.0 io_rename](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.getting_started/topic/s1_resmgr_io_rename.html)：研究取证取得 “This function will be called only with two filenames that are on the same filesystem (same device)” 及 “the shell utility mv will perform a cp followed by an rm (the C library function rename() will do no such thing—it will return only an errno of EXDEV).” 主会话再次直接抓取此页失败，故不增加更底层路由判断的推断。
- [QNX 6.5.0SP1 mv 存档](https://www.qnx.com/developers/docs/6.5.0SP1.update/com.qnx.doc.neutrino_utilities/m/mv.html) 描述派生 cp、成功后派生 rm，以及目录复制失败可能留下部分目标。它仅是旧版本工具证据；8.0 的 [mv-target](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.utilities/topic/m/mv-target.html) 属于 toybox，不能据旧文档断言现代实现仍 spawn 同样子进程。

### Hurd

[官方参考手册 §5.4.7](https://www.gnu.org/software/hurd/doc/hurd_6.html) 给出：

```c
kern_return_t dir_rename(file_t olddirectory, char *oldname,
                        file_t newdirectory, char *newname, int excl)
```

原文：“If olddirectory and newdirectory are not on the same filesystem, then dir_rename might fail with EXDEV.” 这是允许的失败，不是禁止所有 server 使用更强协议。

[固定 Debian 包中的 libdiskfs/dir-rename.c](https://sources.debian.org/data/main/h/hurd/1%3A0.9.git20260919-2/libdiskfs/dir-rename.c) 已复核为源码：

```c
/* Verify that tocred really is a port to us. */
if (! tocred)
  return EXDEV;

if (tocred->po->shadow_root != fromcred->po->shadow_root)
  /* Same translator, but in different shadow trees. */
  return EXDEV;
```

上游 Savannah 抓取受阻，使用固定发行包；不声称该包等于某个未核实的上游 commit。

### Genode

- [25.05 dir_file_system.h](https://github.com/genodelabs/genode/blob/25.05/repos/os/include/vfs/dir_file_system.h) 的 rename 在 `_sub_path()` 后检查目标：注释为 “Check if destination path resides within the same file system instance as the source path”；`if (!to_path) return RENAME_ERR_CROSS_FS;`。同一进程中的组合 VFS 因此仍可能跨实例失败。
- [25.05 file_system_session.h](https://github.com/genodelabs/genode/blob/25.05/repos/os/include/file_system_session/file_system_session.h) 声明 `move(Dir_handle, Name const &, Dir_handle, Name const &)` 及对应 Rpc_move。参数没有另一个 session 能力，公开 RPC 列表未提供联合跨 session move/copy。
- [25.05 libc vfs_plugin.cc](https://github.com/genodelabs/genode/blob/25.05/repos/libports/src/lib/libc/vfs_plugin.cc) 将 RENAME_ERR_CROSS_FS 映射为 EXDEV。

### MINIX

[官方仓库 v3.3.0 minix/servers/vfs/link.c](https://github.com/Stichting-MINIX-Research-Foundation/minix/blob/v3.3.0/minix/servers/vfs/link.c) 的 do_rename：

```c
/* Both parent directories must be on the same device. */
if (old_dirp->v_fs_e != new_dirp->v_fs_e) r = EXDEV;
/* 权限等检查之后，仅 r == OK 时： */
r = req_rename(old_dirp->v_fs_e, old_dirp->v_inode_nr, old_name,
               new_dirp->v_inode_nr, fullpath);
```

本轮主会话取得固定 release 源码，替代初步调查只读 master 的版本限制。此证据只证明 VFS 发送边界；不推导同一 FS endpoint 内任意两个对象必然可 rename。

### POSIX、GNU mv 与复制加速

- [Linux v6.12 fs/namei.c](https://github.com/torvalds/linux/blob/v6.12/fs/namei.c) 的 do_renameat2：`error = -EXDEV; if (old_path.mnt != new_path.mnt) goto exit2;`。
- [POSIX.1-2024 mv，DESCRIPTION 3/6/7](https://pubs.opengroup.org/onlinepubs/9799919799/utilities/mv.html)：先执行等价 rename；EXDEV 后复制源文件层级，再删除源。符号链接复制其链接内容；复制时间、uid/gid、mode 有各自规则，硬链接保留未统一规定。错误部分明确允许源或目标留下部分层级，要求任一中断点至少一侧完整。这不表示迁移在两端之间原子切换。
- [GNU Coreutils mv 手册](https://www.gnu.org/software/coreutils/manual/html_node/mv-invocation.html)（本轮页面标题标注 9.12，在线内容快照，非固定源码 tag）：跨 filesystem “falls back on copying as if by cp -a”，成功后删除源；复制失败清理部分目标；多源批量可出现前项已移走、后项仍在源的部分完成。不要把某工具的失败清理当所有文件协议的唯一选择。
- [copy_file_range(2)](https://man7.org/linux/man-pages/man2/copy_file_range.2.html) 描述 reflink 和 NFS server-side-copy 加速；Linux 5.19 起跨 filesystem 复制要求同类型且后端实现支持。允许短复制，输出是已复制字节数；不包含删除源或原子改名语义。这里引用其明确版本边界，不以在线 man 页替代所有版本行为。

## 对本项目后续讨论的适用边界

以上样本支持将原子名字变更与跨后端的复制/删除组合分别建模。它们不能证明“只要不同进程就不可能原子移动”，也不能证明“一切 Copy 必须由客户端搬运字节”。Halcyon 是否提供跨 provider 高层 Move、是否支持目录递归/属性保存/部分目标清理，以及是否增加后端复制加速，仍由 fs 设计明确。OAL 不承担这些文件语义。
