#!/usr/bin/env python3
"""Audit eRhino user ELF load geometry and W^X at page granularity."""

from __future__ import annotations

from pathlib import Path
import struct
import sys

PAGE_SIZE = 4096
PT_LOAD = 1
PF_X = 1
PF_W = 2
PF_R = 4


def audit(path: Path) -> None:
    data = path.read_bytes()
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise ValueError("not an ELF64 little-endian image")
    e_type, machine = struct.unpack_from("<HH", data, 16)
    if e_type != 2 or machine != 243:
        raise ValueError("expected RISC-V ET_EXEC")
    entry, phoff = struct.unpack_from("<QQ", data, 24)
    phentsize, phnum = struct.unpack_from("<HH", data, 54)
    if phentsize != 56:
        raise ValueError("unexpected program-header size")
    table_end = phoff + phentsize * phnum
    if table_end > len(data):
        raise ValueError("program-header table escapes file")

    pages: dict[int, int] = {}
    entry_executable = False
    loads = 0
    for index in range(phnum):
        offset = phoff + index * phentsize
        p_type, flags = struct.unpack_from("<II", data, offset)
        if p_type != PT_LOAD:
            continue
        loads += 1
        file_offset, vaddr = struct.unpack_from("<QQ", data, offset + 8)
        filesz, memsz, align = struct.unpack_from("<QQQ", data, offset + 32)
        if filesz > memsz or file_offset + filesz > len(data):
            raise ValueError(f"PT_LOAD {index} has invalid file geometry")
        if align < PAGE_SIZE or vaddr % PAGE_SIZE != file_offset % PAGE_SIZE:
            raise ValueError(f"PT_LOAD {index} violates page congruence")
        if flags & ~(PF_R | PF_W | PF_X):
            raise ValueError(f"PT_LOAD {index} has unknown flags")
        if flags & PF_W and not flags & PF_R:
            raise ValueError(f"PT_LOAD {index} is writable but not readable")
        for vpn in range(vaddr // PAGE_SIZE, (vaddr + memsz + PAGE_SIZE - 1) // PAGE_SIZE):
            pages[vpn] = pages.get(vpn, 0) | flags
        if flags & PF_X and vaddr <= entry < vaddr + memsz:
            entry_executable = True

    if loads == 0:
        raise ValueError("image has no PT_LOAD segment")
    for vpn, flags in pages.items():
        if flags & PF_W and flags & PF_X:
            raise ValueError(f"page {vpn:#x} requires both W and X")
    if not entry_executable:
        raise ValueError("entry point is not inside an executable PT_LOAD")


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: audit-user-elf.py ELF...", file=sys.stderr)
        return 2
    try:
        for argument in sys.argv[1:]:
            path = Path(argument)
            audit(path)
            print(f"user ELF audit passed: {path}")
    except (OSError, ValueError) as error:
        print(f"user ELF audit failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
