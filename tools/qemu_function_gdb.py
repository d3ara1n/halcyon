"""只读核真实PCI function context跨轮及ECAM BME；不改客体状态或运输。"""
import ast
import hashlib
import json
import os
from pathlib import Path
import subprocess

import qemu_gdb as debug
import qemu_irq_gdb as irq_probe
import qemu_irq_idle_gdb as idle_probe


def start(kernel, port, address_file, irq_target=None, irq_idle=False):
    logs = debug.fresh_logs(Path("artifacts/device-function"))
    facts = {
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "read-only authentic function sender contexts and PCI command BME; no timing claim",
        "calls": [],
        "irq_requested": irq_target is not None,
        "irq_idle_requested": irq_idle,
    }
    evidence = logs / "evidence.json"
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    symbols = debug.symbols(kernel)
    delivery = "<kernel::task::delivery::Delivery>::create"
    script = f'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb
import json
import struct
import time
import re
from pathlib import Path
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
ADDRESS=Path({json.dumps(str(address_file.resolve()))})
IRQ_TARGET={("Path("+json.dumps(str(irq_target.resolve()))+")") if irq_target is not None else "None"}
IRQ_IDLE_READY={("Path("+json.dumps(str(irq_target.with_name("irq-idle-ready.json").resolve()))+")") if irq_idle else "None"}
IRQ_IDLE_INSERTED={("Path("+json.dumps(str(irq_target.with_name("irq-idle-inserted").resolve()))+")") if irq_idle else "None"}
DIRECT_DELTA={symbols['handle_user_trap']-debug.physical_address(kernel,symbols['handle_user_trap'])}
IDLE_START={symbols['kernel::sched::idle']}
facts=json.loads(EVIDENCE.read_text())
active={{}}
pending=None
provider=None
old_context=None
new_context=None
first_closed=False
prepared=False
complete=False
failure=None
{debug.user_trap_context(kernel, symbols['handle_user_trap'])}
{debug.USER_MEMORY}
def u32(data,at=0):return struct.unpack_from('<I',data,at)[0]
def u64(data,at=0):return struct.unpack_from('<Q',data,at)[0]
header_type=gdb.lookup_type('erhino_shared::message::SendHeader')
header_offsets={{f.name:f.bitpos//8 for f in header_type.fields()}}
FUNCTION=int.from_bytes(b'PCIFUNC1','little')
PROVISION=0x50524f5649530001
def command():
    if 'command_address' not in facts:
        end=time.monotonic()+1
        while not ADDRESS.exists() and time.monotonic()<end:time.sleep(.01)
        if not ADDRESS.exists():raise gdb.GdbError('actual adopted PCI command address missing')
        observed=json.loads(ADDRESS.read_text())
        facts.update(command_address=observed['command'],device=observed['device'])
    return physical_word(facts['command_address'],2)
class Select(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            current=gdb.newest_frame()
            thread=int(current.read_var('thread'))
            satp=word(thread+thread_offsets['satp'])
            header=user_memory(satp,int(current.read_var('header_ptr')),header_type.sizeof)
            kind=u64(header,header_offsets['kind'])
            if kind not in (FUNCTION,PROVISION):return False
            size=int(current.read_var('payload_len'))
            if not 48<=size<=4096:return False
            payload=user_memory(satp,int(current.read_var('payload_ptr')),size)
            version,shape,zero,txid=struct.unpack_from('<HHIQ',payload)
            version2,op,error,length,zero2=struct.unpack_from('<HHIII',payload,16)
            if version!=1 or zero!=0 or version2!=1 or zero2!=0 or length!=size-48 or txid==0:return False
            anchor=int(gdb.parse_and_eval('$tp'))&((1<<64)-1)
            if word(anchor+hart_offsets['current_thread'])!=thread:raise gdb.GdbError('Send execution owner mismatch')
            frame=thread+thread_offsets['frame']
            if word(frame+context_offsets['x']+17*8)!=0x41:raise gdb.GdbError('not an actual Send syscall')
            shell=word(thread+process_offset+ptr_offset+pointer_offset)
            pid=word(shell+data_offset+pid_offset)
            caps=u32(header,header_offsets['handle_count'])
            body=payload[48:]
            if kind==PROVISION:
                if provider==pid and shape==2 and op in (1,4) and error==0 and caps==0 and len(body)==16 and u64(body)==2 and u32(body,8)==2 and u32(body,12)==4:
                    active[thread]={{'prepared':True,'pid':pid,'txid':txid}}
                    delivery_break.enabled=True
                return False
            if op!=1 or body:return False
            if shape not in (1,2):return False
            if caps!=(1 if shape==1 else 0):raise gdb.GdbError('function RPC capability slots mismatch')
            if shape==1 and pid!=1:raise gdb.GdbError('function check request is not init')
            active[thread]={{'pid':pid,'shape':shape,'txid':txid,'error':error,'satp':hex(satp),'frame':hex(frame)}}
            delivery_break.enabled=True
            return False
        except Exception as error:
            failure=str(error)
            return True
class Delivery(gdb.Breakpoint):
    def stop(self):
        global pending,provider,old_context,new_context,first_closed,prepared,complete,failure
        try:
            current=gdb.newest_frame()
            caller=current.older()
            while caller and caller.name()!='kernel::task::mailbox::send':caller=caller.older()
            if caller is None:return False
            event=active.pop(int(caller.read_var('thread')),None)
            if event is None:return False
            if not active:self.enabled=False
            context=int(current.read_var('sender_id'))
            once=bool(caller.read_var('send_once'))
            if context==0:raise gdb.GdbError('missing authenticated sender context')
            if event.get('prepared'):
                if not once:raise gdb.GdbError('Prepared response lacks original Once')
                prepared=True
                facts['new_prepared']={{'epoch':2,'pid':event['pid'],'once_context':context,'txid':event['txid']}}
                print('FUNCTION_NEW_PREPARED epoch=2 pid=%d once=%d' % (event['pid'],context))
                return False
            if event['shape']==1:
                if once or pending is not None:raise gdb.GdbError('function request is not one serial ordinary sender')
                event.update(context=context,command_before=command())
                if old_context is None:old_context=context
                if first_closed and new_context is None:
                    if not prepared or context!=old_context:raise gdb.GdbError('old function call did not follow real new Prepared')
                    if event['command_before']&4:raise gdb.GdbError('new Prepared BME was enabled before old alias call')
                    event['old_new_prepared']=True
                elif new_context not in (None,-1) and context!=new_context:
                    raise gdb.GdbError('new round request changed function context')
                pending=event
                return False
            if not once or pending is None or event['txid']!=pending['txid']:raise gdb.GdbError('function reply lacks matching real Once/txid')
            if provider is not None and event['pid']!=provider:raise gdb.GdbError('function provider PID changed')
            provider=event['pid']
            pending.update(provider=provider,reply_once=context,error=event['error'],command_after=command())
            if pending.get('old_new_prepared'):
                if event['error']!=1 or pending['command_after']&4:raise gdb.GdbError('old context was not Permission with BME remaining clear')
                facts['old_rejected']=pending.copy()
                # 下一份请求必须由新context发起，不能再次使用旧alias。
                new_context=-1
            elif new_context==-1:
                if pending['context']==old_context or event['error']!=0 or pending['command_before']&4 or not pending['command_after']&4:
                    raise gdb.GdbError('new context did not independently enable BME')
                new_context=pending['context']
                facts['new_enabled']=pending.copy()
            elif event['error']==12:
                if not first_closed:
                    if pending['context']!=old_context:raise gdb.GdbError('first Stop context changed')
                    first_closed=True
                    facts['first_stop_closed']=pending.copy()
                else:
                    if pending['context']!=new_context:raise gdb.GdbError('second Stop context changed')
                    facts['second_stop_closed']=pending.copy()
                    if IRQ_TARGET is not None and not irq_verified():raise gdb.GdbError('real IRQ revoke/generation evidence incomplete')
                    if IRQ_IDLE_READY is not None and not facts.get('irq_idle',{{}}).get('claim'):raise gdb.GdbError('all-idle insertion lacks actual same-source claim')
                    complete=True
            elif event['error']!=0:raise gdb.GdbError('unexpected function result')
            facts['calls'].append(pending.copy())
            if IRQ_IDLE_READY is not None and event['error']==0 and 'irq_idle' not in facts:
                for breakpoint in irq_idle_bps:breakpoint.enabled=True
            print('FUNCTION_CONTEXT_OBSERVED '+json.dumps(pending,sort_keys=True))
            pending=None
            if complete:
                facts['verified']=True
                EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
                print('FUNCTION_SESSION_VERIFIED authentic old/new contexts, Permission/Closed, BME 0/0/1')
                return True
            return False
        except Exception as error:
            failure=str(error)
            return True
Select("'kernel::task::mailbox::send'",type=gdb.BP_HARDWARE_BREAKPOINT)
delivery_break=Delivery({json.dumps("'"+delivery+"'")},type=gdb.BP_HARDWARE_BREAKPOINT)
delivery_break.enabled=False
{irq_probe.CODE if irq_target is not None else ""}
{idle_probe.CODE if irq_idle else ""}
try:
    while not complete:
        resumed=irq_idle_continue() if IRQ_IDLE_READY is not None else gdb.execute('continue',to_string=True)
        # GDB在多hart自动step-over时可反复输出切换行；原事实/错误输出仍全部保留。
        for line in resumed.splitlines():
            if not line.startswith('[Switching to thread '):print(line)
        if failure is not None:raise gdb.GdbError(failure)
        if IRQ_IDLE_READY is not None and irq_idle_pending:
            # GDB正常stop事件已完成，才切换调试器线程读取共同快照；stop回调不切上下文。
            irq_idle_pending=False
            irq_idle_snapshot()
            if failure is not None:raise gdb.GdbError(failure)
        elif not complete:raise gdb.GdbError('function session evidence incomplete')
finally:
    if failure is not None:facts['probe_failure']=failure
    EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
end
detach
quit 0
'''
    script_path = logs / "function.gdb"
    script_path.write_text(script)
    ast.parse(script.split("\npython\n",1)[1].split("\nend\n",1)[0], filename=str(script_path))
    output = (logs / "function.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)], stdout=output, stderr=subprocess.STDOUT)
    return child, output, logs


def verify(logs, status, provider):
    facts = json.loads((logs / "evidence.json").read_text())
    if status or not facts.get("verified") or facts.get("new_enabled", {}).get("provider") != provider:
        raise RuntimeError(f"function session read-only evidence incomplete: {logs}")
    return facts
