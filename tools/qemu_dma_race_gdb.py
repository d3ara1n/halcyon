"""原DMA消费者的Pin/Seal、reader/Retire及准备退出窗口，只安排实际CPU次序。

Pending推进在sched::set_context之前，HartLocal.current可为0；实际执行资格
从原Pending/advance_in_space的thread与Admitted owner核定，不要求内核补current。
"""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug

MODES = {"dma-pin-before", "dma-pin-after", "dma-pin-exit", "dma-reader-retire"}


def start(kernel, port, target_file):
    logs = debug.fresh_logs(Path("artifacts/dma-races"))
    evidence = logs / "evidence.json"
    facts = {
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "real Pin/Seal/ReadSegments/Retire/Kill; vCont only, no state or result mutation; no timing claim",
        "windows": [],
    }
    symbols = debug.symbols(kernel)
    before = symbols["<kernel::task::memory_object::MemoryObjectCore>::reserve_dma_write::<kernel::task::resources::MetadataCharge>"]
    pin = symbols["<kernel::resource::dma::object::Pin as kernel::task::request::Request>::advance"]
    read = symbols["<kernel::resource::dma::object::Read as kernel::task::request::Request>::advance"]
    commands = ["riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel), "-ex", "set language c"]
    for line in (357, 381, 496):
        commands += ["-ex", f"info line kernel/src/resource/dma/object.rs:{line}"]
    commands += ["-ex", f"disassemble /r {pin:#x},{pin+1370:#x}",
                 "-ex", f"disassemble /r {read:#x},{read+700:#x}",
                 "-ex", f"disassemble /r {before:#x},{before+180:#x}"]
    code = subprocess.check_output(commands, text=True, stderr=subprocess.STDOUT)
    (logs / "code-windows.log").write_text(code)
    points = {line: int(re.search(rf"Line {line} .*starts at address (0x[0-9a-f]+)", code)[1], 16)
              for line in (357, 381, 496)}
    def call_address(text):
        return int(re.search(r"0x[0-9a-f]+", next(line for line in code.splitlines() if text in line))[0], 16)
    expose = call_address("7Mapping6expose>")
    segments = call_address("7Mapping8segments>")
    if not expose < points[496] < segments:
        raise RuntimeError("DMA reader checkpoint is not after expose and before segments")
    acquire_call = call_address("3Pin7acquire>")
    if not pin < acquire_call < points[357] < points[381]:
        raise RuntimeError("DMA Pin checkpoints do not follow the actual acquisition call")
    facts["code_windows"] = {"before_write": hex(before), "after_acquire": hex(points[357]),
                             "after_partial_step": hex(points[381]), "after_expose": hex(points[496]),
                             "expose_call": hex(expose), "segments_call": hex(segments)}
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    script = f'''set pagination off
set confirm off
set language rust
set breakpoint always-inserted on
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb,json,re,struct,time
from pathlib import Path
TARGET=Path({json.dumps(str(target_file.resolve()))})
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
{debug.user_trap_context(kernel,symbols['handle_user_trap'])}
def variant(value):return next((f.name for f in value.type.fields() if f.name),None)
def optional(value):
    name=variant(value)
    if name=='None':return None
    if name!='Some':raise gdb.GdbError('unexpected Option debug layout: '+str(value.type))
    inner=value['Some']
    return inner[next(f.name for f in inner.type.fields() if f.name)]
def pointer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_PTR,gdb.TYPE_CODE_INT):
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        if key is None:raise gdb.GdbError('unexpected pointer wrapper: '+str(value.type))
        value=value[key]
    return value
def arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['data']
def name(frame):return (frame.name() or '').replace('<','').replace('>','')
def caller(wanted):
    frame=gdb.newest_frame()
    while frame:
        if name(frame)==wanted:return frame
        frame=frame.older()
    return None
def execution_owner():
    anchor=int(gdb.parse_and_eval('$tp'))&((1<<64)-1)
    installed=word(anchor+hart_offsets['current_thread'])
    pending=caller('kernel::task::request::Pending::advance')
    space=caller('kernel::task::request::advance_in_space')
    if pending is not None:
        if space is None:raise gdb.GdbError('DMA Pending lacks original admitted execution scope')
        thread=int(pending.read_var('thread'))
        admitted=space.read_var('owner').dereference()['value']
        if int(arc(admitted).address)!=thread:raise gdb.GdbError('DMA Pending thread differs from original admission owner')
        if installed not in (0,thread):raise gdb.GdbError('DMA Pending conflicts with installed user owner')
        if int(gdb.parse_and_eval('$satp'))&((1<<64)-1)!=word(thread+thread_offsets['satp']):raise gdb.GdbError('DMA Pending is not in original owner address space')
    else:
        thread=installed
        if not thread:raise gdb.GdbError('DMA syscall has no installed execution owner')
    shell=word(thread+process_offset+ptr_offset+pointer_offset)
    return thread,word(shell+data_offset+pid_offset)
def partial(threads):
    actions=['c:p%x.%x'%t.ptid[:2] for t in threads]
    if not actions:raise gdb.GdbError('DMA fixture has no runnable selected CPU')
    packet='vCont;'+';'.join(actions)
    response=gdb.execute('maintenance packet '+packet,to_string=True)
    print('DMA_VCONT '+packet+' '+response.strip())
    match=re.search(r'thread:p([0-9a-f]+)\\.([0-9a-f]+);',response)
    if match is None:raise gdb.GdbError('DMA partial resume lacks a concrete all-stop CPU')
    stopped=tuple(int(part,16) for part in match.groups())
    gdb.execute('maintenance flush register-cache')
    next(t for t in gdb.selected_inferior().threads() if t.ptid[:2]==stopped).switch()
    gdb.execute('maintenance flush register-cache')
    # 切换后先取真实PC，使调试器的寄存器/帧上下文与返回的stopped hart一致。
    print('DMA_STOPPED_PC %#x'%(int(gdb.parse_and_eval('$pc'))&((1<<64)-1)))
select_bps=[gdb.Breakpoint(quoted,type=gdb.BP_HARDWARE_BREAKPOINT) for quoted in (
    "'<kernel::task::memory_object::MemoryObjectCore>::seal'",
    "'<kernel::task::process::shell::ProcessShell>::freeze_and_seal'",
    "'<kernel::resource::dma::mapping::Mapping>::retire'")]
result_ok=gdb.Breakpoint("'kernel::syscall::respond_ok'",type=gdb.BP_HARDWARE_BREAKPOINT)
result_error=gdb.Breakpoint("'kernel::syscall::respond_error'",type=gdb.BP_HARDWARE_BREAKPOINT)
release=gdb.Breakpoint("'kernel::task::notification::signal'",type=gdb.BP_HARDWARE_BREAKPOINT)
for breakpoint in (result_ok,result_error,release):breakpoint.enabled=False
window=None
parent=None
child=None
record=None
state='select'
while len(facts['windows'])<4:
    if state=='select':gdb.execute('continue')
    elif state=='need-child':partial([t for t in gdb.selected_inferior().threads() if t.ptid!=parent.ptid])
    elif state=='commit-parent':partial([parent])
    elif state=='reader-proof':partial([t for t in gdb.selected_inferior().threads() if t.ptid!=child.ptid])
    else:raise gdb.GdbError('unexpected DMA external orchestration stage')
    frame=gdb.newest_frame()
    here=name(frame)
    if state=='select':
        if not TARGET.exists():continue
        target=json.loads(TARGET.read_text())
        if target['mode'] in [r['mode'] for r in facts['windows']]:continue
        thread,pid=execution_owner()
        core=None
        if target['mode'] in ('dma-pin-before','dma-pin-after'):
            original=caller('kernel::task::memory_object::seal')
            if original is None or pid!=1:continue
            native_frame=thread+thread_offsets['frame']
            if word(native_frame+context_offsets['x']+10*8)!=target['memory']:continue
            core=int(frame.read_var('self'))
            point={before} if target['mode']=='dma-pin-before' else {points[357]}
        elif target['mode']=='dma-pin-exit':
            if caller('kernel::task::process::kill') is None or pid!=1:continue
            if int(arc(frame.read_var('self'))['pid'])!=target['pid']:continue
            point={points[381]}
        else:
            if 'kernel::resource::dma::mapping::Mapping::retire'!=here or pid!=target['pid']:continue
            mapping=arc(frame.read_var('self'))
            core=None
            point={points[496]}
        parent=gdb.selected_thread()
        native_frame=thread+thread_offsets['frame']
        record={{**target,'parent_pid':pid,'parent_thread':thread,'parent_hart':parent.ptid[1]-1,
                 'parent_frame':native_frame,'parent_sepc':word(native_frame+context_offsets['sepc']),
                 'memory_core':core,'window_pc':hex(point)}}
        if target['mode']=='dma-reader-retire':
            record.update(mapping_id=int(mapping['id']),mapping=int(mapping.address))
        for breakpoint in select_bps:breakpoint.enabled=False
        window=gdb.Breakpoint('*%#x'%point,type=gdb.BP_HARDWARE_BREAKPOINT)
        state='need-child'
        continue
    if state=='need-child':
        if int(gdb.parse_and_eval('$pc'))&((1<<64)-1)!=int(record['window_pc'],16):raise gdb.GdbError('DMA partial resume stopped outside the exact original window')
        thread,pid=execution_owner()
        if pid!=record['pid']:continue
        child=gdb.selected_thread()
        if child.ptid==parent.ptid:raise gdb.GdbError('DMA fixture did not reach another executing CPU')
        if record['mode']=='dma-pin-before':
            original=caller('kernel::resource::dma::object::Pin::acquire')
            if original is None:raise gdb.GdbError('DMA before-write window is outside original Pin')
            pin=original.read_var('self').dereference()
        elif record['mode']!='dma-reader-retire':pin=frame.read_var('self').dereference()
        if record['mode']=='dma-reader-retire':
            read=frame.read_var('self').dereference()
            record['read_request']=int(read.address)
            mapping=arc(read['mapping'])
            if int(mapping.address)!=record['mapping']:raise gdb.GdbError('reader belongs to another mapping')
            record['reader_accepted']=True
        else:
            mapping=arc(pin['prepared']['mapping'])
            record['pin_request']=int(pin.address)
            core=arc(pin['core'])
            if record['memory_core'] is not None and record['memory_core']!=int(core.address):raise gdb.GdbError('Pin and Seal refer to different MemoryObjects')
            owned=optional(mapping['owned']['data']['value'])
            if owned is None:raise gdb.GdbError('Pin checkpoint still holds its Owned outside the mapping')
            write=optional(owned['write'])
            if (write is None)!=(record['mode']=='dma-pin-before'):raise gdb.GdbError('Pin checkpoint has the wrong write-permit ownership')
            record.update(mapping_id=int(mapping['id']),mapping=int(mapping.address),memory_core=int(core.address),write_owned=write is not None)
            acquisition=optional(owned['acquisition'])
            if acquisition is None:raise gdb.GdbError('Pin checkpoint lacks original acquisition')
            cursor=optional(acquisition['cursor'])
            record['acquisition']={{key:int(cursor[key]) for key in ('lo','next','hi')}}
            record['backing']=int(pointer(arc(acquisition['backing'])['id']))
            if record['mode']=='dma-pin-exit':
                if not record['acquisition']['lo']<record['acquisition']['next']<record['acquisition']['hi']:raise gdb.GdbError('Pin exit checkpoint lacks a strict nonempty incomplete prefix')
                result=frame.read_var('result')['Ok']
                if bool(result[next(f.name for f in result.type.fields() if f.name)]):raise gdb.GdbError('Pin exit checkpoint already completed acquisition')
        record.update(child_thread=thread,child_hart=child.ptid[1]-1)
        record.update(actual_pc=hex(int(gdb.parse_and_eval('$pc'))&((1<<64)-1)),child_satp=hex(word(thread+thread_offsets['satp'])),actual_satp=hex(int(gdb.parse_and_eval('$satp'))&((1<<64)-1)),child_frame=thread+thread_offsets['frame'],hart_current=word((int(gdb.parse_and_eval('$tp'))&((1<<64)-1))+hart_offsets['current_thread']))
        print('DMA_EXACT_WINDOW '+json.dumps(record,sort_keys=True))
        window.delete()
        window=None
        result_ok.enabled=True
        result_error.enabled=True
        state='commit-parent'
        continue
    if state=='commit-parent':
        returned=int(frame.read_var('frame'))
        if returned!=record['parent_frame'] or word(returned+context_offsets['sepc'])!=record['parent_sepc']:raise gdb.GdbError('DMA parent returned from another syscall')
        if here!='kernel::syscall::respond_ok':raise gdb.GdbError('actual DMA Seal/Kill/Retire failed')
        record['parent_committed']=True
        print('DMA_PARENT_COMMITTED '+json.dumps(record,sort_keys=True))
        result_ok.enabled=False
        result_error.enabled=False
        if record['mode']=='dma-reader-retire':
            release.enabled=True
            state='reader-proof'
            continue
    elif state=='reader-proof':
        thread,pid=execution_owner()
        if pid!=record['pid'] or int(frame.read_var('bits'))!=2:continue
        if thread!=record['parent_thread']:raise gdb.GdbError('DMA retiring-reader proof belongs to another execution owner')
        record['reader_proof_gate']=True
        release.enabled=False
        print('DMA_READER_RETIRE_PROOF accepted reader held through actual query/writer/DONE assertions')
    facts['windows'].append(record)
    EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
    for breakpoint in select_bps:breakpoint.enabled=True
    state='select'
facts['verified']=True
EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
print('DMA_RACES_VERIFIED original Pin/Seal, reader/Retire and nonempty preparation/Kill windows')
end
detach
quit 0
'''
    script_path = logs / "races.gdb"
    script_path.write_text(script)
    ast.parse(script.split("\npython\n", 1)[1].split("\nend\n", 1)[0], filename=str(script_path))
    output = (logs / "races.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)], stdout=output, stderr=subprocess.STDOUT)
    return child, output, logs


def verify(logs, status):
    facts = json.loads((logs / "evidence.json").read_text())
    if status or not facts.get("verified") or {r["mode"] for r in facts["windows"]} != MODES:
        raise RuntimeError(f"DMA exact-window evidence incomplete: {logs}")
    return facts
