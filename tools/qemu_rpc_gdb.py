"""真实回复Send的一次运输期限失败；业务/ref/cap不改，原Once与返回值只读核验。"""
import hashlib
import json
import os
from pathlib import Path
import subprocess

import qemu_gdb as debug

OPERATIONS = ("Claim", "Prepare", "ProvisionGet", "GetResource", "Activate", "Master")


def start(kernel, port, operation):
    if operation not in OPERATIONS:
        raise ValueError("unknown device operation")
    logs = debug.fresh_logs(Path("artifacts/device-unknown"))
    facts = {
        "operation": operation,
        "kernel": str(kernel),
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "boot_package": os.environ["ERHINO_BOOT_PACKAGE"],
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "one successful RPC response receives transport At(0); request/business committed, reply not enqueued; no timing claim",
    }
    evidence = logs / "evidence.json"
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    symbols = debug.symbols(kernel)
    delivery = next(name for name in symbols if name == "<kernel::task::delivery::Delivery>::create")
    script = f'''set pagination off
set confirm off
set language c
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb
import hashlib
import json
import struct
from pathlib import Path
OPERATION={json.dumps(operation)}
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
selected=None
complete=False
failure=None
{debug.user_trap_context(kernel, symbols['handle_user_trap'])}
def offsets(name):
    return {{f.name:f.bitpos//8 for f in gdb.lookup_type(name).fields()}}
header_offsets=offsets('erhino_shared::message::SendHeader')
header_size=gdb.lookup_type('erhino_shared::message::SendHeader').sizeof
move_size=gdb.lookup_type('erhino_shared::message::HandleMove').sizeof
{debug.USER_MEMORY}
def memory(address,size):
    return user_memory(selected['satp'],address,size)
def u32(data, offset=0):return struct.unpack_from('<I',data,offset)[0]
def u64(data, offset=0):return struct.unpack_from('<Q',data,offset)[0]
def pointer(value):
    for _ in range(8):
        if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:return value
        names=[f.name for f in value.type.fields()]
        name=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        if name is None:raise gdb.GdbError('unexpected funded Arc layout')
        value=value[name]
    raise gdb.GdbError('funded Arc nesting exceeded')
def resource(body):
    if len(body)<36:return None
    length=u32(body,28)
    if 32+length+4>len(body):return None
    name=body[32:32+length]
    metadata=u32(body,32+length)
    if 36+length+metadata!=len(body):return None
    return name.decode('ascii')
def eligible(kind, payload, caps):
    if len(payload)<48:return None
    version,shape,reserved,txid=struct.unpack_from('<HHIQ',payload)
    if (version,shape,reserved)!=(1,2,0) or txid==0:return None
    version,op,error,length,zero=struct.unpack_from('<HHIII',payload,16)
    if version!=1 or error!=0 or zero!=0 or length!=len(payload)-48:return None
    body=payload[48:]
    device=0x4445564943450001
    provision=0x50524f5649530001
    activate=int.from_bytes(b'DRVACT01','little')
    master=int.from_bytes(b'PCIFUNC1','little')
    detail={{'txid':txid,'kind':kind,'opcode':op,'caps':caps}}
    if OPERATION=='Claim' and kind==device and op==6 and caps==1 and len(body)==36 and u32(body,32)==1:
        detail.update(incarnation=u64(body),device=u64(body,8),epoch=u64(body,16),claimant_context=u64(body,24))
    elif OPERATION=='Prepare' and kind==provision and op==1 and caps==0 and len(body)==16 and u32(body,8)==2 and u32(body,12)==4:
        detail.update(epoch=u64(body),resource_count=u32(body,12))
    elif OPERATION=='ProvisionGet' and kind==provision and op==2 and caps==1 and len(body)>=44 and u32(body,8)==0 and resource(body[8:])=='ecam':
        detail.update(epoch=u64(body),resource='ecam',index=0)
    elif OPERATION=='GetResource' and kind==device and op==10 and caps==1 and resource(body)=='ecam':
        detail.update(resource='ecam',index=0)
    elif OPERATION=='Activate' and kind==activate and op==1 and caps==1 and len(body)>=56 and u32(body,32)==1 and u32(body,36)==0:
        detail.update(incarnation=u64(body),device=u64(body,8),epoch=u64(body,16),activation_sequence=u64(body,24))
    elif OPERATION=='Master' and kind==master and op==1 and caps==0 and not body:
        pass
    else:return None
    if detail.get('device',1)!=1 or detail.get('epoch',1)!=1:return None
    return detail
class Select(gdb.Breakpoint):
    def stop(self):
        global selected,failure
        if selected is not None:return False
        try:
            current=gdb.newest_frame()
            thread=int(current.read_var('thread'))
            anchor=int(gdb.parse_and_eval('$tp')) & ((1<<64)-1)
            if word(anchor+hart_offsets['current_thread'])!=thread:raise gdb.GdbError('Send is not the current execution owner')
            frame=thread+thread_offsets['frame']
            registers=frame+context_offsets['x']
            if word(registers+17*8)!=0x41:raise gdb.GdbError('expected real Send syscall')
            shell=word(thread+process_offset+ptr_offset+pointer_offset)
            pid=word(shell+data_offset+pid_offset)
            satp=word(thread+thread_offsets['satp'])
            header=int(current.read_var('header_ptr'))
            payload_ptr=int(current.read_var('payload_ptr'))
            size=int(current.read_var('payload_len'))
            if not 48<=size<=4096:return False
            raw_header=user_memory(satp,header,header_size)
            expected_kind={{
                'Claim':0x4445564943450001,'GetResource':0x4445564943450001,
                'Prepare':0x50524f5649530001,'ProvisionGet':0x50524f5649530001,
                'Activate':int.from_bytes(b'DRVACT01','little'),
                'Master':int.from_bytes(b'PCIFUNC1','little')
            }}[OPERATION]
            if u64(raw_header,header_offsets['kind'])!=expected_kind:return False
            payload=user_memory(satp,payload_ptr,size)
            caps=u32(raw_header,header_offsets['handle_count'])
            detail=eligible(u64(raw_header,header_offsets['kind']),payload,caps)
            if detail is None:return False
            moves=int(current.read_var('moves_ptr'))
            selected={{'thread':thread,'frame':frame,'satp':satp,'pc':word(frame+context_offsets['sepc']),'header':header,'payload':payload_ptr,'size':size,'raw_header':raw_header,'payload_hash':hashlib.sha256(payload).hexdigest(),'moves':moves,'raw_moves':user_memory(satp,moves,caps*move_size) if caps else b''}}
            facts.update(detail,pid=pid,frame=hex(frame),user_satp=hex(satp),sepc=hex(selected['pc']),sender_handle=word(registers+10*8),payload_sha256=selected['payload_hash'])
            at=header+header_offsets['deadline']
            user_memory(satp,at,16,struct.pack('<IIQ',1,0,0))
            self.enabled=False
            Delivery({json.dumps("'"+delivery+"'")},type=gdb.BP_HARDWARE_BREAKPOINT)
            Result("'kernel::syscall::respond_error'",False)
            Result("'kernel::syscall::respond_ok'",True)
            print('DEVICE_REPLY_AT0_SELECTED operation=%s pid=%d txid=%d frame=%#x caps=%d' % (OPERATION,pid,detail['txid'],frame,caps))
            return False
        except Exception as error:
            failure=str(error)
            return True
class Delivery(gdb.Breakpoint):
    def stop(self):
        global failure
        if selected is None or complete:return False
        try:
            current=gdb.newest_frame()
            caller=current.older()
            while caller and caller.name()!='kernel::task::mailbox::send':caller=caller.older()
            if caller is None or int(caller.read_var('thread'))!=selected['thread']:return False
            if not bool(caller.read_var('send_once')):raise gdb.GdbError('selected Response is not a claimed Once')
            context=int(current.read_var('sender_id'))
            if context==0:raise gdb.GdbError('reply Once has no authentic identity')
            pool=pointer(current.read_var('pool')).dereference()['data']
            facts.update(once_context=context,payer_pool=int(pool['header']['koid']))
            print('DEVICE_REPLY_ONCE_AUTHENTICATED operation=%s pid=%d context=%d payer_pool=%d' % (OPERATION,facts['pid'],context,facts['payer_pool']))
            return False
        except Exception as error:
            failure=str(error)
            return True
class Result(gdb.Breakpoint):
    def __init__(self,name,success):
        self.success=success
        super().__init__(name,type=gdb.BP_HARDWARE_BREAKPOINT)
    def stop(self):
        global complete,failure
        if selected is None:return False
        try:
            current=gdb.newest_frame()
            if int(current.read_var('frame'))!=selected['frame']:return False
            if word(selected['frame']+context_offsets['sepc'])!=selected['pc']:return False
            error=0 if self.success else int(current.read_var('err'))
            if error!=0x3d:raise gdb.GdbError('selected Send returned %d instead of DeadlineExpired' % error)
            if not facts.get('once_context'):raise gdb.GdbError('selected Send did not authenticate the original Once/context')
            now=memory(selected['header'],header_size)
            at=header_offsets['deadline']
            original=selected['raw_header']
            if now[:at]!=original[:at] or now[at+16:]!=original[at+16:]:raise gdb.GdbError('non-deadline SendHeader bytes changed')
            if hashlib.sha256(memory(selected['payload'],selected['size'])).hexdigest()!=selected['payload_hash']:raise gdb.GdbError('RPC/domain body changed')
            if facts['caps'] and memory(selected['moves'],len(selected['raw_moves']))!=selected['raw_moves']:raise gdb.GdbError('capability move input changed')
            complete=True
            facts.update(send_error='DeadlineExpired',reply_enqueued=False,once_authenticated=True)
            EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
            print('DEVICE_REPLY_LOSS_VERIFIED operation=%s pid=%d txid=%d error=DeadlineExpired; body/ref/caps unchanged; reply not enqueued' % (OPERATION,facts['pid'],facts['txid']))
            return True
        except Exception as error:
            failure=str(error)
            return True
Select("'kernel::task::mailbox::send'",type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
if failure is not None:raise gdb.GdbError(failure)
if not complete:raise gdb.GdbError('real successful Response/Once/DeadlineExpired were not all observed')
end
detach
quit 0
'''
    script_path = logs / "reply-loss.gdb"
    script_path.write_text(script)
    output = (logs / "reply-loss.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)], stdout=output, stderr=subprocess.STDOUT)
    return child, output, logs


def verify(logs, status, operation):
    facts = json.loads((logs / "evidence.json").read_text())
    raw = (logs / "reply-loss.log").read_text()
    if status or facts.get("operation") != operation or facts.get("send_error") != "DeadlineExpired" or not facts.get("once_authenticated") or facts.get("reply_enqueued") is not False or "DEVICE_REPLY_LOSS_VERIFIED" not in raw:
        raise RuntimeError(f"real {operation} reply-loss evidence incomplete: {logs}")
    return facts
