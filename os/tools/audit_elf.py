#!/usr/bin/env python3
"""内核 ELF 链接后 ISA/ABI 审计（notes/execution-context.md「内核与用户 ABI」）。

检查项：
1. ELF64 / RISC-V / soft-float ABI（e_flags 浮点位为 0）；
2. FP 指令与浮点 CSR（fcsr/fflags/frm）访问只允许出现在 .text.ctx_fp
   （capability-guarded 的用户 FP helper）；其余任何 section 不得出现。

用法：audit_elf.py <kernel.elf>（依赖 riscv64-elf-readelf / objdump）。
"""

import re
import subprocess
import sys

READELF = "riscv64-elf-readelf"
OBJDUMP = "riscv64-elf-objdump"

# objdump 反汇编行：地址: 字节 助记符 操作数。f 开头的助记符除 fence* 外
# 全部视为浮点指令族（fsd/fld/fmadd/fcvt/fsgnj/...）。
LINE = re.compile(r"^\s*[0-9a-f]+:\s+(?:[0-9a-f]{4}(?:\s[0-9a-f]{4})*)\s+(\S+)")
FP_MNEMONIC = re.compile(r"^f(?!ence)")
# csrr/csrw 等访问浮点 CSR：操作数含 fcsr/fflags/frm。
FP_CSR_OPERAND = re.compile(r"\bfcsr\b|\bfflags\b|\bfrm\b")
# 反汇编块头部 `Disassembly of section ...`。
SECTION_MARK = re.compile(r"^Disassembly of section ([^:]+):")


def fail(msg: str) -> None:
    print(f"审计失败: {msg}")
    sys.exit(1)


def main() -> None:
    if len(sys.argv) != 2:
        print(__doc__)
        sys.exit(2)
    elf = sys.argv[1]

    # ---- 头部契约面 ----
    header = subprocess.run(
        [READELF, "-h", elf], capture_output=True, text=True, check=True
    ).stdout
    if "ELF64" not in header:
        fail("非 ELF64")
    if "RISC-V" not in header:
        fail("非 RISC-V")
    flags_line = next(
        line for line in header.splitlines() if line.strip().startswith("Flags:")
    )
    if "soft-float" not in flags_line:
        # LP64 整数 ABI 是内核基线；double/single ABI 直接违约。
        fail(f"内核必须为 soft-float (LP64) ABI：{flags_line.strip()}")

    # ---- FP 指令分布 ----
    disasm = subprocess.run(
        [OBJDUMP, "-d", elf], capture_output=True, text=True, check=True
    ).stdout

    current = "<none>"
    violations: list[str] = []
    ctx_fp_seen = False
    for line in disasm.splitlines():
        if m := SECTION_MARK.match(line):
            current = m.group(1)
            if current == ".text.ctx_fp":
                ctx_fp_seen = True
            continue
        if current == ".text.ctx_fp":
            continue
        m = LINE.match(line)
        if not m:
            continue
        mnemonic, rest = m.group(1), line[m.end():]
        if FP_MNEMONIC.match(mnemonic) or (
            mnemonic.startswith("csr") and FP_CSR_OPERAND.search(rest)
        ):
            violations.append(f"[{current}] {line.strip()}")

    if not ctx_fp_seen:
        fail("缺少 .text.ctx_fp 节（用户 FP helper 未链接进来？）")
    if violations:
        shown = "\n".join(violations[:20])
        fail(f".text.ctx_fp 之外出现 FP 指令/CSR 访问（共 {len(violations)} 处）：\n{shown}")

    print("内核 ELF 审计通过：LP64 soft-float ABI，FP 面收敛于 .text.ctx_fp")


if __name__ == "__main__":
    main()
