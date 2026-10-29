"""原MMIO消费者的真实Map/Revoke许可交错；只用vCont安排hart，不改内核状态。"""
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug


def start(kernel, port, target_file, controller_pid):
    logs = debug.fresh_logs(Path("artifacts/mmio-publication"))
    evidence = logs / "evidence.json"
    evidence.write_text(json.dumps({
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "controller_pid": controller_pid,
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "real Map/Revoke, read-only state/PTEs, thread-scoped vCont only; no timing claim",
        "windows": [],
    }, indent=2)+"\n")
    symbols = debug.symbols(kernel)
    grant_pc = symbols["<kernel::resource::mmio::MmioPublication>::grant"]
    commit_pc = symbols["kernel::vm::operation::commit"]
    # 断点尚在固件/bare页表时也能按当轮ELF核机器码；不依赖所选hart的SATP读高半区。
    offline = subprocess.run([
        "riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel),
        "-ex", "set language c",
        "-ex", "info line kernel/src/vm/operation.rs:783",
        "-ex", f"disassemble /r {commit_pc:#x},{commit_pc+256:#x}",
        "-ex", f"disassemble /r {grant_pc:#x},{grant_pc+160:#x}",
    ], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=True).stdout
    (logs / "code-windows.log").write_text(offline)
    post_pc = int(re.search(r"Line 783 .*starts at address (0x[0-9a-f]+)", offline)[1],16)
    def address(text):
        line = next(line for line in offline.splitlines() if text in line)
        return int(re.search(r"0x[0-9a-f]+",line)[0],16)
    grant_call = address("17grant_publication>")
    vm_lock = address("VmStateE4lock")
    root_lock = address("mmio5StateE4lock")
    if not grant_call < post_pc < vm_lock or not grant_pc < root_lock:
        raise RuntimeError("DWARF checkpoint does not meet actual lock/call order")
    facts = json.loads(evidence.read_text())
    facts["code_windows"] = {
        "grant_entry":hex(grant_pc), "root_lock_call":hex(root_lock),
        "grant_call":hex(grant_call), "post_grant_pre_vm":hex(post_pc),
        "vm_lock_call":hex(vm_lock),
    }
    evidence.write_text(json.dumps(facts,indent=2)+"\n")
    script = f'''set pagination off
set confirm off
set language c
set breakpoint always-inserted on
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb
import json
import re
import struct
import time
from pathlib import Path
TARGET=Path({json.dumps(str(target_file.resolve()))})
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
{debug.user_trap_context(kernel,symbols['handle_user_trap'])}
{debug.USER_MEMORY}
def pointer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_PTR,gdb.TYPE_CODE_INT):
        names=[f.name for f in value.type.fields() if f.name]
        name=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        if name is None:raise gdb.GdbError('unexpected owner pointer layout: '+str(value.type))
        value=value[name]
    return value
def arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['data']
def some(value):
    branch=value['Some']
    return branch[next(f.name for f in branch.type.fields() if f.name)]
def scalar(value):return int(pointer(value))
def owner(thread):
    shell=word(thread+process_offset+ptr_offset+pointer_offset)
    return word(shell+data_offset+pid_offset)
def caller(name):
    name=name.replace('<','').replace('>','')
    frame=gdb.newest_frame()
    while frame:
        if (frame.name() or '').replace('<','').replace('>','')==name:return frame
        frame=frame.older()
    return None
def normal():
    gdb.execute('continue')
def partial(threads):
    actions=['c:p%x.%x' % (t.ptid[0],t.ptid[1]) for t in threads]
    if not actions:raise gdb.GdbError('no admitted hart available for partial resume')
    packet='vCont;'+';'.join(actions)
    response=gdb.execute('maintenance packet '+packet,to_string=True)
    print('MMIO_VCONT '+packet+' '+response.strip())
    match=re.search(r'thread:p([0-9a-f]+)\.([0-9a-f]+);',response)
    if match is None:raise gdb.GdbError('partial resume did not produce a concrete all-stop hart')
    pid,tid=(int(part,16) for part in match.groups())
    gdb.execute('maintenance flush register-cache')
    stopped=next(t for t in gdb.selected_inferior().threads() if t.ptid[:2]==(pid,tid))
    stopped.switch()
    gdb.execute('maintenance flush register-cache')
def range_of(value):
    if 'start' not in [f.name for f in value.type.fields() if f.name]:value=value['__0']
    return int(value['start']),int(value['end'])
def absent(satp,address):
    mode=satp>>60
    if mode not in (8,9,10):raise gdb.GdbError('unexpected child SATP mode')
    table=(satp&((1<<44)-1))<<12
    for level in reversed(range(mode-5)):
        pte=physical_word(table+((address>>(12+9*level))&511)*8)
        if pte&1==0:return True
        if pte&14:return False
        table=((pte>>10)&((1<<44)-1))<<12
    return True
grant_pc={grant_pc}
post_pc={post_pc}
revoke_bp=gdb.Breakpoint("'<kernel::resource::mmio::node::MmioNode>::revoke'",type=gdb.BP_HARDWARE_BREAKPOINT)
query_bp=gdb.Breakpoint("'kernel::resource::mmio::object::query'",type=gdb.BP_HARDWARE_BREAKPOINT)
query_bp.enabled=False
result_ok=gdb.Breakpoint("'kernel::syscall::respond_ok'",type=gdb.BP_HARDWARE_BREAKPOINT)
result_ok.enabled=False
result_error=gdb.Breakpoint("'kernel::syscall::respond_error'",type=gdb.BP_HARDWARE_BREAKPOINT)
result_error.enabled=False
window_bp=None
record=None
parent=None
child=None
query=None
state='select'
while len(facts['windows'])<2:
    if state in ('select','retire'):normal()
    elif state=='need-child':partial([t for t in gdb.selected_inferior().threads() if t.ptid!=parent.ptid])
    elif state=='revoke':partial([parent])
    else:raise gdb.GdbError('unexpected external probe stage')
    pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
    frame=gdb.newest_frame()
    name=(frame.name() or '').replace('<','').replace('>','')
    if state=='select':
        if name!='kernel::resource::mmio::node::MmioNode::revoke':continue
        original=caller('kernel::resource::mmio::object::revoke')
        if original is None:continue
        thread=int(original.read_var('thread'))
        if owner(thread)!={controller_pid}:continue
        node=arc(frame.read_var('self'))
        if int(node['bytes'])!=32*4096:continue
        end=time.monotonic()+1
        while not TARGET.exists() and time.monotonic()<end:time.sleep(.01)
        if not TARGET.exists():continue
        target=json.loads(TARGET.read_text())
        if target['mode'] in [r['mode'] for r in facts['windows']]:continue
        source=word(thread+thread_offsets['frame']+context_offsets['x']+10*8)
        if source!=target['scope']:raise gdb.GdbError('actual Revoke capability differs from admitted scope')
        parent=gdb.selected_thread()
        parent_frame=thread+thread_offsets['frame']
        record={{**target,'parent_hart':parent.ptid[1]-1,'parent_frame':parent_frame,'parent_sepc':word(parent_frame+context_offsets['sepc']),'node_id':scalar(node['id']),'physical':int(node['physical'])}}
        revoke_bp.enabled=False
        window_bp=gdb.Breakpoint('*%#x' % (grant_pc if target['mode']=='mmio-before' else post_pc),type=gdb.BP_HARDWARE_BREAKPOINT)
        state='need-child'
        continue
    if state=='need-child':
        if pc!=(grant_pc if record['mode']=='mmio-before' else post_pc):raise gdb.GdbError('partial resume stopped outside selected publication window')
        commit=caller('kernel::vm::operation::commit')
        complete=caller('<kernel::vm::operation::MapCall>::complete')
        if commit is None or complete is None:raise gdb.GdbError('window is not actual original MMIO Map continuation')
        thread=int(complete.read_var('thread'))
        if owner(thread)!=record['pid']:raise gdb.GdbError('window belongs to another process')
        child=gdb.selected_thread()
        if child.ptid==parent.ptid:raise gdb.GdbError('fixture did not reach a separate running hart')
        operation=commit.read_var('operation')
        work=arc(operation['work'])
        publication=some(work['completion']['publication'])
        mapping=arc(some(publication['mapping']))
        start,end=range_of(commit.read_var('footprint'))
        if int(mapping['pages'])!=32 or int(mapping['physical'])!=record['physical']:raise gdb.GdbError('window changed actual mapping geometry')
        record.update(child_hart=child.ptid[1]-1,child_thread=thread,child_satp=word(thread+thread_offsets['satp']),mapping_id=scalar(mapping['id']),footprint_start=start,footprint_end=end,window_pc=hex(pc),permission='Preparing' if record['mode']=='mmio-before' else 'PublishGranted')
        print('MMIO_PUBLICATION_WINDOW '+json.dumps(record,sort_keys=True))
        window_bp.delete()
        window_bp=None
        result_ok.enabled=True
        result_error.enabled=True
        state='revoke'
        continue
    if state=='revoke':
        returned=int(frame.read_var('frame'))
        if returned!=record['parent_frame'] or word(returned+context_offsets['sepc'])!=record['parent_sepc']:raise gdb.GdbError('unexpected return while only original Revoke hart resumed')
        if name!='kernel::syscall::respond_ok':raise gdb.GdbError('actual parent Revoke failed')
        record['revoke_returned']=True
        print('MMIO_PARENT_REVOKE_COMMITTED '+json.dumps(record,sort_keys=True))
        result_ok.enabled=False
        result_error.enabled=False
        query_bp.enabled=True
        state='retire'
        continue
    if state=='retire':
        if name=='kernel::resource::mmio::object::query':
            thread=int(frame.read_var('thread'))
            if owner(thread)!={controller_pid}:continue
            native_frame=thread+thread_offsets['frame']
            if word(native_frame+context_offsets['x']+10*8)!=record['scope']:continue
            query={{'frame':native_frame,'pc':word(native_frame+context_offsets['sepc']),'satp':word(thread+thread_offsets['satp']),'output':int(frame.read_var('output'))}}
            result_ok.enabled=True
            result_error.enabled=True
        elif name in ('kernel::syscall::respond_ok','kernel::syscall::respond_error') and query is not None:
            if int(frame.read_var('frame'))!=query['frame'] or word(query['frame']+context_offsets['sepc'])!=query['pc']:continue
            if name!='kernel::syscall::respond_ok':raise gdb.GdbError('scope query failed')
            raw=user_memory(query['satp'],query['output'],48)
            members=struct.unpack_from('<Q',raw,16)[0]
            flags=struct.unpack_from('<I',raw,24)[0]
            if members!=0 or flags&4==0:raise gdb.GdbError('actual scope is not DONE/members0')
            satp=record['child_satp']
            pages=list(range(record['footprint_start'],record['footprint_end'],4096))
            if not pages or not all(absent(satp,address) for address in pages):raise gdb.GdbError('scope DONE retained IO PTEs in still-live target VM')
            record.update(scope_done=True,members=0,absent_pages=len(pages),before_kill=True)
            facts['windows'].append(record)
            EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
            print('MMIO_PUBLICATION_RETIRED '+json.dumps(record,sort_keys=True))
            query_bp.enabled=False
            result_ok.enabled=False
            result_error.enabled=False
            revoke_bp.enabled=True
            state='select'
facts['verified']=True
EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
print('MMIO_PUBLICATION_VERIFIED before/after permission, actual Revoke return, real scope DONE and no IO PTEs before Kill')
end
detach
quit 0
'''
    script_path = logs / "publication.gdb"
    script_path.write_text(script)
    output = (logs / "publication.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)], stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
    return child, output, logs


def verify(logs, status):
    facts = json.loads((logs / "evidence.json").read_text())
    if status or not facts.get("verified") or {r["mode"] for r in facts["windows"]} != {"mmio-before", "mmio-after"}:
        raise RuntimeError(f"MMIO publication evidence incomplete: {logs}")
    return facts
