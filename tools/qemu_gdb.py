"""QEMU外部GDB验收共享设施：符号、独立进程组、ELF地址与不覆盖的证据目录。"""

from datetime import datetime, timezone
import os
from pathlib import Path
import re
import signal
import struct
import subprocess
import time


def symbols(elf: Path):
    text=subprocess.check_output(['riscv64-elf-nm','-C','--defined-only',str(elf)],text=True)
    result={}
    for line in text.splitlines():
        match=re.match(r'([0-9a-f]+) [A-Za-z] (.+)$',line)
        if match:result[match[2]]=int(match[1],16)
    return result


def _group_members(pgid):
    # 不读取其他应用参数；killpg(0)的EPERM本身不能区分组仍在与macOS已退组竞态。
    rows = subprocess.check_output(["ps", "-axo", "pid=,pgid="], text=True)
    return [int(pid) for pid, group in (line.split() for line in rows.splitlines())
            if int(group) == pgid]


def stop_group(process):
    """只收调用者以start_new_session创建的专有组；leader DONE不代表后代完成。"""
    pgid = process.pid
    def send(signum):
        try: os.killpg(pgid, signum)
        except ProcessLookupError: pass
        except PermissionError:
            if _group_members(pgid): raise
    def wait_empty():
        end = time.monotonic() + 5
        while True:
            process.poll()  # 回收直接子进程，避免它自己的zombie保留组身份。
            members = _group_members(pgid)
            if not members:
                process.wait(timeout=5)
                return True
            if time.monotonic() >= end: return False
            time.sleep(.05)
    send(signal.SIGCONT)
    send(signal.SIGTERM)
    if wait_empty(): return
    send(signal.SIGKILL)
    if not wait_empty():
        raise RuntimeError(f"owned process group did not disappear: pgid={pgid}, pids={_group_members(pgid)}")


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


def user_trap_context(kernel, entry):
    """正式trap处的DWARF布局；fault与回复运输观察共用同一执行owner核验。"""
    direct_delta = entry - physical_address(kernel, entry)
    return f'''hart_type=gdb.lookup_type('kernel::hart::HartLocal')
thread_type=gdb.lookup_type('kernel::task::process::thread::Thread')
hart_offsets={{field.name:field.bitpos//8 for field in hart_type.fields()}}
thread_offsets={{field.name:field.bitpos//8 for field in thread_type.fields()}}
context_offsets={{field.name:field.bitpos//8 for field in gdb.lookup_type('kernel::context::UserContext').fields()}}
{PHYSICAL_READ}
def field(kind,name):
    member=next(member for member in kind.fields() if member.name==name)
    return member.bitpos//8,member.type
process_offset,arc=field(thread_type,'process')
ptr_offset,nonnull=field(arc,'ptr')
pointer_offset,raw=field(nonnull,'pointer')
data_offset,shell_type=field(raw.target(),'data')
pid_offset,_=field(shell_type,'pid')
def word(address):
    if address<{direct_delta}:raise gdb.GdbError('expected direct-mapped kernel pointer')
    return physical_word(address-{direct_delta})
'''


# 外部调试器解析真实Thread保存的用户SATP；不进入产品uaccess。
USER_MEMORY = '''
def translate(satp,address,write=False):
    # trap已切内核页表；调试器只解析目标Thread保存的用户SATP，不改guest页表/CSR。
    mode=satp>>60
    if mode not in (8,9,10):raise gdb.GdbError('unexpected target user SATP mode')
    levels=mode-5
    table=(satp&((1<<44)-1))<<12
    for level in reversed(range(levels)):
        pte=physical_word(table+((address>>(12+9*level))&511)*8)
        if pte&1==0:raise gdb.GdbError('target user address has no PTE')
        if pte&14:
            if pte&16==0 or (write and pte&4==0):raise gdb.GdbError('target user access lacks U/W permission')
            span=1<<(12+9*level)
            base=((pte>>10)&((1<<44)-1))<<12
            if base&(span-1):raise gdb.GdbError('misaligned target user superpage')
            return base+(address&(span-1)),span-(address&(span-1))
        table=((pte>>10)&((1<<44)-1))<<12
    raise gdb.GdbError('target user address has no leaf')
def user_memory(satp,address,size,write=None):
    result=bytearray()
    offset=0
    while offset<size:
        physical,count=translate(satp,address+offset,write is not None)
        count=min(count,size-offset)
        response=gdb.execute('maintenance packet Qqemu.PhyMemMode:1',to_string=True)
        if 'OK' not in response:raise gdb.GdbError('QEMU physical memory mode unavailable')
        try:
            if write is None:result.extend(gdb.selected_inferior().read_memory(physical,count).tobytes())
            else:gdb.selected_inferior().write_memory(physical,write[offset:offset+count])
        finally:gdb.execute('maintenance packet Qqemu.PhyMemMode:0',to_string=True)
        offset+=count
    return bytes(result)
'''
