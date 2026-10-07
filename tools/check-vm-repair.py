#!/usr/bin/env python3
"""外部诊断：真实UserContext/PTE上模拟fault入口，核fence、原现场和重复故障终止。"""

import argparse
import hashlib
import json
from pathlib import Path
import re
import signal
import socket
import subprocess
import sys

import qemu_gdb as boot


def instruction(addresses,elf,name,opcode):
    start=addresses[name];end=min(a for a in addresses.values() if a>start)
    text=subprocess.check_output(['riscv64-elf-objdump','-d',f'--start-address={start}',f'--stop-address={end}',str(elf)],text=True)
    found=re.findall(r'^\s*([0-9a-f]+):\s+[0-9a-f]+\s+'+re.escape(opcode)+r'\s*$',text,re.M)
    if not found and opcode=='fence.i':
        # LP64 soft-float ELF attrs未声明Zifencei时，binutils可将同一指令显示为.4byte。
        found=re.findall(r'^\s*([0-9a-f]+):\s+0000100f\b',text,re.M)
    if len(found)!=1:raise RuntimeError(f'expected one {opcode} in {name}')
    return int(found[0],16)


def observe(args,command,script,name,port):
    script_path=args.logs/(name+'.gdb');script_path.write_text(script)
    qemu_log,gdb_log=args.logs/(name+'-qemu.log'),args.logs/(name+'-gdb.log')
    with qemu_log.open('w') as qlog,gdb_log.open('w') as glog:
        process=subprocess.Popen(command+['-S','-gdb',f'tcp:127.0.0.1:{port}'],stdout=qlog,stderr=subprocess.STDOUT,start_new_session=True)
        try:
            result=subprocess.run(['riscv64-elf-gdb','-q','-nx','--batch','-x',str(script_path)],stdout=glog,stderr=subprocess.STDOUT,timeout=args.timeout)
        finally:boot.stop_group(process)
    if result.returncode:
        raise RuntimeError(f'{name} GDB failed: {gdb_log}, {qemu_log}')
    return gdb_log.read_text(),qemu_log


def uaccess_script(args,addresses,port):
    fence=instruction(addresses,args.kernel,'kernel::uaccess::prepare_direct_access','sfence.vma')
    script=r'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "__ELF__"
target remote 127.0.0.1:__PORT__
python
import gdb
expected={'kernel::uaccess::copy_from_user','kernel::uaccess::copy_to_user','kernel::uaccess::publish_u64'}
pending={}
seen=set()
failed=None
def caller():
    frame=gdb.newest_frame()
    while frame:
        if frame.name() in expected:
            return frame.name(),int(frame.read_register('sp')) & ((1<<64)-1)
        frame=frame.older()
    return None
class Begin(gdb.Breakpoint):
    def stop(self):
        value=caller()
        if value:pending[(gdb.selected_thread().num,value)]=False
        return False
class Fence(gdb.Breakpoint):
    def stop(self):
        value=caller()
        if value:pending[(gdb.selected_thread().num,value)]=True
        return False
class Open(gdb.Breakpoint):
    def stop(self):
        global failed
        value=caller()
        if value is None:return False
        key=(gdb.selected_thread().num,value)
        if not pending.pop(key,False):
            failed='SUM opened without a preceding real SFENCE in this invocation';return True
        if value[0] not in seen:
            seen.add(value[0]);print('UACCESS_FENCE_BEFORE_SUM '+value[0])
        return seen==expected
Begin('*__PREPARE__',type=gdb.BP_HARDWARE_BREAKPOINT)
Fence('*__FENCE__',type=gdb.BP_HARDWARE_BREAKPOINT)
Open('*__OPEN__',type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
if failed or seen!=expected:raise gdb.GdbError(failed or 'uaccess paths missing')
print('UACCESS_FENCE_PASSED read/write/cookie')
end
detach
quit 0
'''
    for key,value in dict(ELF=str(args.kernel.resolve()),PORT=str(port),
            PREPARE=hex(addresses['kernel::uaccess::prepare_direct_access']),
            FENCE=hex(fence),OPEN=hex(addresses['<kernel::mm::SumGuard>::open'])).items():
        script=script.replace('__'+key+'__',value)
    return script


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',type=Path,required=True)
    parser.add_argument('--logs',type=Path,default=Path('artifacts/vm-repair'))
    parser.add_argument('--timeout',type=int,default=60)
    parser.add_argument('command',nargs=argparse.REMAINDER)
    args=parser.parse_args();command=args.command[1:] if args.command[:1]==['--'] else args.command
    if not command:parser.error('QEMU command is required')
    args.logs=boot.fresh_logs(args.logs)
    addresses=boot.symbols(args.kernel)
    sfence=instruction(addresses,args.kernel,'kernel::vm::fault::handle','sfence.vma')
    fence_names=['kernel::vm::fault::handle']+[n for n in addresses if 'fence_i' in n]
    fencei=None
    for name in fence_names:
        try:fencei=instruction(addresses,args.kernel,name,'fence.i');break
        except RuntimeError:pass
    if fencei is None:raise RuntimeError('instruction fence body was not found')
    entry=addresses['handle_user_trap']
    freeze=addresses['<kernel::task::process::shell::ProcessShell>::freeze_and_seal']
    def interrupted(signum,_frame):raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM,interrupted)
    evidence=[]
    for name,cause,repeat in [('read',13,False),('write',15,False),('execute',12,False),('repeat',13,True)]:
        with socket.socket() as listener:
            listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
        script=r'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "__ELF__"
target remote 127.0.0.1:__PORT__
python
import gdb
CAUSE=__CAUSE__
REPEAT=__REPEAT__
context=gdb.lookup_type('kernel::context::UserContext')
offsets={field.name:field.bitpos//8 for field in context.fields()}
assert offsets['sepc']-offsets['x']==32*8
saved=None
frame=None
fault_address=None
injections=0
sfences=0
ifences=0
finished=False
failed=None

def memory(address,size):return gdb.selected_inferior().read_memory(address,size).tobytes()
def integer(address):return int.from_bytes(memory(address,8),'little')
def related():
    current=gdb.newest_frame()
    while current:
        if (current.name() or '')=='kernel::vm::fault::handle':return True
        current=current.older()
    return False

class Trap(gdb.Breakpoint):
    def stop(self):
        global frame,saved,fault_address,injections,finished,failed
        if int(gdb.parse_and_eval('$a0'))!=8:return False
        incoming=int(gdb.parse_and_eval('$a2')) & ((1<<64)-1)
        if frame is not None and incoming!=frame:return False
        if frame is None:
            frame=incoming
            saved=memory(frame+offsets['x'],offsets['sepc']-offsets['x']+8)
            pc=integer(frame+offsets['sepc'])
            fault_address=integer(frame+offsets['x']+2*8) if CAUSE==15 else pc
        elif memory(frame+offsets['x'],len(saved))!=saved:
            failed='GPR/sepc changed before original ecall retried';return True
        elif not sfences or (CAUSE==12 and not ifences):
            failed='required fence did not execute before return';return True
        elif not REPEAT:
            finished=True
            print('VM_REPAIR_PASSED cause=%d same_gpr_pc=1 sfence=%d fence_i=%d' % (CAUSE,sfences,ifences))
            return True
        injections+=1
        if injections>5:
            failed='same unchanged fault exceeded four admitted harts';return True
        print('SIMULATED_FAULT cause=%d address=%#x frame=%#x attempt=%d thread=%d' % (CAUSE,fault_address,frame,injections,gdb.selected_thread().num))
        gdb.execute('set $a0 = %d' % CAUSE)
        gdb.execute('set $a1 = %d' % fault_address)
        return False

class Fence(gdb.Breakpoint):
    def __init__(self,address,kind):
        super().__init__('*%#x'%address,type=gdb.BP_HARDWARE_BREAKPOINT);self.kind=kind
    def stop(self):
        global sfences,ifences
        if frame is not None and related():
            if self.kind=='i':ifences+=1
            else:sfences+=1
        return False

class Freeze(gdb.Breakpoint):
    def stop(self):
        global finished,failed
        if frame is None:return False
        if not REPEAT:
            failed='single repair terminated the process';return True
        if not sfences or not 2<=injections<=5:
            failed='repeated fault did not perform then reject repair';return True
        finished=True
        print('VM_REPAIR_REPEAT_PASSED attempts=%d sfences=%d' % (injections,sfences))
        return True

Trap('*__ENTRY__',type=gdb.BP_HARDWARE_BREAKPOINT)
Fence(__SFENCE__,'vma')
Fence(__FENCEI__,'i')
Freeze('*__FREEZE__',type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
if failed or not finished:raise gdb.GdbError(failed or 'repair evidence incomplete')
end
detach
quit 0
'''
        replacements={'ELF':str(args.kernel.resolve()),'PORT':str(port),'CAUSE':str(cause),'REPEAT':str(repeat),
            'ENTRY':hex(entry),'SFENCE':hex(sfence),'FENCEI':hex(fencei),'FREEZE':hex(freeze)}
        for key,value in replacements.items():script=script.replace('__'+key+'__',value)
        text,qemu_log=observe(args,command,script,name,port)
        if ('VM_REPAIR_REPEAT_PASSED' if repeat else 'VM_REPAIR_PASSED') not in text:
            raise RuntimeError(f'{name} evidence incomplete: {args.logs}, {qemu_log}')
        evidence.append(dict(case=name,gdb_exit=0,observations=[line for line in text.splitlines() if 'PASSED' in line or 'SIMULATED_FAULT' in line]))
    with socket.socket() as listener:
        listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
    text,_=observe(args,command,uaccess_script(args,addresses,port),'uaccess',port)
    if 'UACCESS_FENCE_PASSED' not in text:raise RuntimeError('uaccess fence evidence incomplete')
    evidence.append(dict(case='uaccess',gdb_exit=0,observations=[line for line in text.splitlines() if 'UACCESS_' in line]))
    output=dict(kernel_sha256=hashlib.sha256(args.kernel.read_bytes()).hexdigest(),cases=evidence,
        scope='simulated trap cause/stval on a real U ecall frame; real software PTE, hardware fence instructions and restored GPR/PC; real read/write/cookie SFENCE before SUM',
        limits='does not create a stale hardware TLB entry or claim a real cache race; stops guest after the diagnostic, not a full profile acceptance')
    (args.logs/'evidence.json').write_text(json.dumps(output,indent=2)+'\n')
    print(f'VM repair diagnostics passed: read/write/execute/repeat/uaccess; logs: {args.logs}')


if __name__=='__main__':
    try:main()
    except (RuntimeError,subprocess.SubprocessError,KeyError) as error:
        print(f'VM repair diagnostics failed: {error}',file=sys.stderr);sys.exit(1)
