"""QEMU外部GDB验收共享设施：符号、独立进程组、ELF地址与不覆盖的证据目录。"""

from datetime import datetime, timezone
import os
from pathlib import Path
import re
import signal
import struct
import subprocess


def symbols(elf: Path):
    text=subprocess.check_output(['riscv64-elf-nm','-C','--defined-only',str(elf)],text=True)
    result={}
    for line in text.splitlines():
        match=re.match(r'([0-9a-f]+) [A-Za-z] (.+)$',line)
        if match:result[match[2]]=int(match[1],16)
    return result


def stop_group(process):
    try:os.killpg(process.pid,signal.SIGTERM)
    except ProcessLookupError:pass
    try:process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:os.killpg(process.pid,signal.SIGKILL)
        except ProcessLookupError:pass
        process.wait()


def fresh_logs(root: Path):
    stamp=datetime.now(timezone.utc).strftime('%Y%m%dT%H%M%S.%fZ')
    path=root/(stamp+'-'+str(os.getpid()))
    path.mkdir(parents=True,exist_ok=False)
    return path


def physical_address(elf: Path, address: int):
    data=elf.read_bytes()
    if data[:6]!=b'\x7fELF\x02\x01':raise ValueError('ELF64 little-endian image is required')
    header=struct.unpack_from('<16sHHIQQQIHHHHHH',data)
    for index in range(header[10]):
        segment=struct.unpack_from('<IIQQQQQQ',data,header[5]+index*header[9])
        # PT_LOAD的memsz包含BSS；读取停止标志不能依赖当前选中hart的satp。
        if segment[0]==1 and segment[3]<=address<segment[3]+segment[6]:
            return segment[4]+address-segment[3]
    raise ValueError(f'address {address:#x} is outside ELF load segments')


# QEMU v11.1.2 docs/system/gdb.rst「Examining physical memory」：
# https://github.com/qemu/qemu/blob/v11.1.2/docs/system/gdb.rst
# 切换只影响调试器内存包，读后必恢复虚拟模式；不改变guest页表/寄存器。
PHYSICAL_READ = '''
def physical_word(address, size=8):
    response = gdb.execute("maintenance packet Qqemu.PhyMemMode:1", to_string=True)
    if 'OK' not in response:
        raise gdb.GdbError("QEMU physical memory mode is unavailable")
    try:
        return int.from_bytes(gdb.selected_inferior().read_memory(address, size).tobytes(), "little")
    finally:
        gdb.execute("maintenance packet Qqemu.PhyMemMode:0", to_string=True)
'''
