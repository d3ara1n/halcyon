# 静态 ELF admission 规范取证

> 生命周期：本文件是只读参考资料。实现真值见 `shared/elf`，实施档案见 [`admission-fail-closed`](archived/todo-2026-09-admission-fail-closed.md)，本轮复核记录见 [`Review program 档案`](archived/todo-2026-09-review-program.md)。

## 规范入口

- RISC-V psABI：`references/normative/riscv-psabi-v1.0/riscv-elf.adoc`「File Header」「Program Header Table」「Attributes」。
- ELF gABI：System V ABI Chapter 4「ELF Header」与 Chapter 5「Program Header」「Program Loading」。RISC-V psABI 明确以 gABI 为通用 ELF 格式来源。
- GNU 扩展：glibc `elf/elf.h` 的 `PT_GNU_*` 定义与 binutils ld `-z execstack/noexecstack/relro` 文档；这些值属于 gABI 的 OS-specific 区间，不是 RISC-V psABI 自有段。

## 规范事实

- 可装载程序至少有一个 `PT_LOAD`；`p_filesz` 不得大于 `p_memsz`，差额由 loader 清零；LOAD 条目按 `p_vaddr` 升序。
- `PT_LOAD` 的 `p_vaddr` 与 `p_offset` 必须按系统页大小同余。`p_align` 为 0/1 时无额外要求，否则规范建议为二次幂且二者按该值同余。
- `PT_NULL` 除类型外的成员未定义，必须直接忽略，不能验证其 flags/offset/alignment。
- `PT_NOTE` 是可选辅助信息；不影响执行行为的 note 不改变 ABI conformance。
- `PT_PHDR` 必须位于 LOAD 之前、至多一个，且只在 program-header table 本身属于内存映像时出现。
- `PT_INTERP` 要求系统装载解释器；`PT_DYNAMIC` 要求处理动态链接信息；`PT_TLS` 定义 TLS template。未实现这些机制时必须明确拒绝，不能静默忽略。
- `PT_SHLIB` 的语义未定义，包含它的程序不符合 gABI。
- psABI 定义 `PT_RISCV_ATTRIBUTES = 0x70000003`；attributes 用于 linker/runtime compatibility 检查。
- `PF_R/PF_W/PF_X` 是通用权限位。gABI 允许系统授予请求权限的上界，例如 X-only 可映射为 R+X，但绝不能在未请求 W 时授予 W。
- `e_entry` 规范只定义为初始控制转移地址，并不要求它位于 file-backed X bytes；该约束是 Halcyon 为避免从 BSS/零填充区启动而采用的 fail-closed 策略。

## Halcyon 决策

当前只支持静态 RISC-V ELF64 `ET_EXEC`：

- 接受并完整解释 `PT_LOAD`。
- 忽略 `PT_NULL` 和边界合法的 `PT_NOTE`；接受并核验 `PT_PHDR`；接受非 executable、零尺寸的 `PT_GNU_STACK`；接受只读且文件范围合法的 `PT_RISCV_ATTRIBUTES`。
- 拒绝 `PT_INTERP`、`PT_DYNAMIC`、`PT_TLS`、`PT_SHLIB`、`PT_GNU_RELRO` 及其它未知 OS/processor/future 类型。未来支持动态链接、TLS 或 RELRO 时以能力增量扩充显式解释器，不把旧拒绝改成静默忽略。
- 拒绝 `PF_RWX` 之外的 program-header flags、W-only、零权限、段范围溢出/越界、LOAD 字节重叠或失序、非页同余、非法 `p_align`、页级 W+X、entry 不在 executable file bytes。
- 接受 X-only LOAD，但输出的最终页权限明确提升为 R+X，与当前页表 Protection 模型一致。
- program headers 上限 128、LOAD 上限 64；容量超限在分配和地址空间发布前拒绝。
- `Elf` 字段私有，只有 `elf::validate` 能构造；Bootstrap、libprocess 与 host audit 只消费其只读 segments/runs/entry/requirement/image_end。

## 来源

- https://www.sco.com/developers/gabi/latest/ch4.eheader.html
- https://www.sco.com/developers/gabi/latest/ch5.pheader.html
- https://www.sco.com/developers/gabi/latest/ch5.dynamic.html
- https://refspecs.linuxbase.org/elf/gabi41.pdf
- https://sourceware.org/git/?p=glibc.git;a=blob_plain;f=elf/elf.h;hb=HEAD
- https://sourceware.org/binutils/docs/ld/Options.html
