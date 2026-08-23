#!/usr/bin/env python3
"""内核 ELF 链接后 ISA/ABI 审计（notes/execution-context.md「内核与用户 ABI」）。

检查项：
1. ELF64 / RISC-V / soft-float ABI（e_flags 浮点位为 0）；
2. FP 指令与浮点 CSR（fcsr/fflags/frm）访问只允许出现在 .text.ctx_fp
   （capability-guarded 的用户 FP helper）；其余任何 section 不得出现。

3. 单函数最大栈帧不超过 --max-frame（默认 0xc00）：扫描 sp 减量
   （含 lui+addi 装载立即数后 sub sp,sp,reg 的模式），防止巨型栈帧
   溢出每 hart 栈预算（plans/todo-2026-09-stack-guard.md 方案 C）。

用法：audit_elf.py <kernel.elf> [--max-frame N]
（依赖 riscv64-elf-readelf / objdump）。
"""

import re
import subprocess
import sys

READELF = "riscv64-elf-readelf"
OBJDUMP = "riscv64-elf-objdump"

# 默认帧上限：正常栈预算（sifive_u 每 hart 0x4000 减 emergency 页 =
# 0x3000）的一半。当前最大合法函数为 compiler_builtins memmove
# （debug 构建 0x1620）；release 帧更小，同一阈值对两 MODE 都成立。
# 更深的调用链总和超限由 guard page 兑底（todo-2026-09-stack-guard.md）。
DEFAULT_MAX_FRAME = 0x1800

# objdump 反汇编行：地址: 字节 助记符 操作数。字节列为若干组 4 位
# 十六进制（RVC 为一组，32 位指令为两组连写），后随助记符；'#' 后为
# 符号注解需剥离。注意不能只匹配单组：否则 32 位指令整行被跳过。
LINE = re.compile(r"^\s*[0-9a-f]+:\s+(?:[0-9a-f]{4}\s*)+(\S+)")
FP_MNEMONIC = re.compile(r"^f(?!ence)")
# csrr/csrw 等访问浮点 CSR：操作数含 fcsr/fflags/frm。
FP_CSR_OPERAND = re.compile(r"\bfcsr\b|\bfflags\b|\bfrm\b")
# 反汇编块头部 `Disassembly of section ...`。
SECTION_MARK = re.compile(r"^Disassembly of section ([^:]+):")
# 函数标签行：地址 <符号名>:
FUNC_MARK = re.compile(r"^[0-9a-f]+ <(.+)>:$")


def scan_frames(disasm: str) -> list[tuple[str, int, str]]:
    """逐函数扫描 sp 减量，返回 (函数名, 最大下探字节数, 位置地址)。

    跟踪寄存器常量（li/lui/addi/mv）以解析 `lui+addi 装载立即数 →
    sub sp,sp,reg` 的大帧模式；分支导致的误跟踪只会低估不会高估，
    对护栏语义可接受。跨分支的加/减净额按序累计，函数尾 epilogue
    加回后 worst 不受影响。
    """
    funcs: list[tuple[str, int, str]] = []
    name = None
    delta = worst = 0
    worst_addr = ""
    regs: dict[str, int] = {}

    def bump(addr: str) -> None:
        nonlocal worst, worst_addr
        if delta < worst:
            worst, worst_addr = delta, addr

    for line in disasm.splitlines():
        if fm := FUNC_MARK.match(line):
            if name is not None:
                funcs.append((name, -worst, worst_addr))
            name, delta, worst, worst_addr, regs = fm.group(1), 0, 0, "", {}
            continue
        if name is None or not (m := LINE.match(line)):
            continue
        mnem = m.group(1).removeprefix("c.")
        rest = line[m.end():].split("#")[0].replace("\t", " ").strip()
        ops = [o.strip() for o in rest.split(",")]
        addr = line.split(":")[0].strip()
        if len(ops) < 2:
            continue
        rd = ops[0]
        if mnem in ("addi", "addiw") and len(ops) == 3:
            v = int(ops[2], 0)
            if rd == "sp":
                delta += v
                bump(addr)
            elif ops[1] == rd and rd in regs:
                regs[rd] += v
            else:
                regs.pop(rd, None)
        elif mnem == "li" and len(ops) == 2:
            try:
                regs[rd] = int(ops[1], 0)
            except ValueError:
                regs.pop(rd, None)
        elif mnem == "lui" and len(ops) == 2:
            try:
                regs[rd] = int(ops[1], 0) << 12
            except ValueError:
                regs.pop(rd, None)
        elif mnem == "mv" and len(ops) == 2:
            if ops[1] in regs:
                regs[rd] = regs[ops[1]]
            else:
                regs.pop(rd, None)
        elif mnem in ("sub", "add") and rd == "sp" and len(ops) == 3 and ops[1] == "sp":
            v = regs.get(ops[2])
            if v is not None:
                delta += -v if mnem == "sub" else v
                bump(addr)
        else:
            regs.pop(rd, None)
    if name is not None:
        funcs.append((name, -worst, worst_addr))
    return funcs


def fail(msg: str) -> None:
    print(f"审计失败: {msg}")
    sys.exit(1)


def main() -> None:
    args = sys.argv[1:]
    max_frame = DEFAULT_MAX_FRAME
    if "--max-frame" in args:
        i = args.index("--max-frame")
        max_frame = int(args[i + 1], 0)
        del args[i : i + 2]
    if len(args) != 1:
        print(__doc__)
        sys.exit(2)
    elf = args[0]

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
        mnemonic, rest = m.group(1), line[m.end():].split("#")[0]
        if FP_MNEMONIC.match(mnemonic) or (
            mnemonic.startswith("csr") and FP_CSR_OPERAND.search(rest)
        ):
            violations.append(f"[{current}] {line.strip()}")

    if not ctx_fp_seen:
        fail("缺少 .text.ctx_fp 节（用户 FP helper 未链接进来？）")
    if violations:
        shown = "\n".join(violations[:20])
        fail(f".text.ctx_fp 之外出现 FP 指令/CSR 访问（共 {len(violations)} 处）：\n{shown}")

    # ---- 单函数栈帧上限 ----
    frames = [f for f in scan_frames(disasm) if f[1] > 0]
    over = [f for f in frames if f[1] > max_frame]
    if over:
        over.sort(key=lambda f: -f[1])
        shown = "\n".join(
            f"  {f[1]:#x} @ {f[2]}  {f[0][:80]}" for f in over[:10]
        )
        fail(f"{len(over)} 个函数栈帧超过 {max_frame:#x}：\n{shown}")

    peak = max((f for f in frames), key=lambda f: f[1], default=None)
    peak_info = f"，最大栈帧 {peak[1]:#x}（{peak[0][:60]}）" if peak else ""
    print(f"内核 ELF 审计通过：LP64 soft-float ABI，FP 面收敛于 .text.ctx_fp{peak_info}")


if __name__ == "__main__":
    main()
