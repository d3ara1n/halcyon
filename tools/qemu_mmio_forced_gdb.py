"""原双MMIO Map消费者在非空FIFO/已提交Publishing与Frozen之间交接。"""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug

MODES = {"mmio-forced-queued", "mmio-forced-publishing"}

RESPONSE_SNAPSHOT = r'''
def response_query(snapshot, request):
    query={'request':request,'response':None}
    snapshot.setdefault('rsp_queries',[]).append(query);save()
    try:
        output=gdb.execute('maintenance packet '+request,to_string=True)
        query['response']=output;save()
        require('received: "' in output,'response diagnostic query has no reply')
        payload=output.split('received: "',1)[1].rsplit('"',1)[0]
        return re.sub(r'\\x([0-9a-fA-F]{2})',lambda m:chr(int(m[1],16)),payload)
    except Exception as error:
        query['error']={'type':type(error).__name__,'message':str(error)};save()
        raise
def response_register_layout(snapshot):
    if 'response_register_layout' not in facts:
        target=response_query(snapshot,'qXfer:features:read:target.xml:0,fff')
        require(target.startswith('l'),'target XML exceeds the bounded diagnostic read')
        first=re.search(r'<xi:include\s+href="([^"]+)"',target[1:])
        require('<architecture>riscv:rv64</architecture>' in target and first is not None
                and first[1]=='riscv-64bit-cpu.xml','unexpected target architecture or first register feature')
        core=response_query(snapshot,'qXfer:features:read:riscv-64bit-cpu.xml:0,fff')
        require(core.startswith('l'),'core XML exceeds the bounded diagnostic read')
        feature=ET.fromstring(core[1:])
        require(feature.get('name')=='org.gnu.gdb.riscv.cpu','unexpected CPU register feature')
        cursor=0;registers={}
        for reg in feature.findall('reg'):
            cursor=int(reg.get('regnum',cursor))
            if reg.get('name') in ('sp','a0','a1','pc'):
                registers[reg.get('name')]={'number':cursor,'bitsize':int(reg.get('bitsize'))}
            cursor+=1
        require({name:(r['number'],r['bitsize']) for name,r in registers.items()}
                =={'sp':(2,64),'a0':(10,64),'a1':(11,64),'pc':(32,64)},
                'target XML register numbering differs from the audited RV64 core')
        facts['response_register_layout']={'target_xml':target[1:],'core_xml':core[1:],
            'registers':registers,'byte_order':'little'};save()
    return facts['response_register_layout']['registers']
def response_value(destination, key, read):
    # 只收诊断值；不可用字段明确保留，不转换成身份或放行证据。
    try:destination[key]=read()
    except Exception as error:destination[key]={'unavailable':type(error).__name__+': '+str(error)}
    save()
def response_frame(frame):
    return {'pc':hex(int(frame.pc())),'function':frame.name(),'level':frame.level(),
            'type':frame.type(),'valid':frame.is_valid(),'unwind_stop_reason':frame.unwind_stop_reason()}
def response_snapshot(frame, pc):
    phase='revoke' if not record.get('revoke_returned') else 'kill' if record.get('kill_frame') is not None else None
    if phase is None:return
    dispatch=caller('kernel::syscall::dispatch')
    if dispatch is None:return
    expected_thread=record['parent_frame']-thread_offsets['frame']
    if int(dispatch.read_var('thread'))!=expected_thread:return
    require(record['mode'] in ('mmio-forced-queued','mmio-forced-publishing'),
            'response snapshot is outside the two original consumers')
    key=record['mode']+'/'+phase
    snapshots=facts.setdefault('response_snapshots',{})
    require(key not in snapshots,'selected response stage was observed more than once')
    layout=next(v for v in facts['response_locations'].values() if int(v['point'],16)==pc)
    snapshot={'target_pid':record['pid'],'phase':phase,'thread':expected_thread,
        'expected_frame':hex(record['parent_frame'] if phase=='revoke' else record['kill_frame']),
        'cpu':list(gdb.selected_thread().ptid[:2]),'gdb_thread':gdb.selected_thread().num,
        'pc':hex(pc),'layout':layout,'diagnostic_only':True,
        'sources':{'raw_rsp_registers':'explicit Hg then raw remote p, target XML numbering',
                   'raw_rsp_stack':'raw remote m of the audited prologue range in virtual debug mode',
                   'gdb_registers_before_rsp':'GDB expression registers before diagnostic Hg/p',
                   'frame_registers':'newest Frame.read_register, through GDB frame model',
                   'symbol_value_before_rsp':'DWARF Symbol.value(newest frame)',
                   'dispatch_ancestor':'GDB unwind comparison only; never a replacement frame'}}
    snapshots[key]=snapshot;save()
    response_value(snapshot,'newest_frame',lambda:response_frame(frame))
    response_value(snapshot,'selected_frame',lambda:response_frame(gdb.selected_frame()))
    gdb_values={};frame_values={}
    snapshot.update(gdb_registers_before_rsp=gdb_values,frame_registers=frame_values);save()
    for name in ('sp','a0','a1','pc'):
        response_value(gdb_values,name,lambda name=name:hex(int(gdb.parse_and_eval('$'+name))&((1<<64)-1)))
        response_value(frame_values,name,lambda name=name:hex(int(frame.read_register(name))&((1<<64)-1)))
    try:
        block=frame.block()
        symbol=next((s for s in block if s.name=='frame' and s.is_argument),None)
        require(symbol is not None and block.function is not None,'response frame argument declaration is unavailable')
        snapshot['symbol']={'name':symbol.name,'type':str(symbol.type),'function':block.function.print_name,
                            'objfile':symbol.symtab.objfile.filename,
                            'scope':[hex(block.start),hex(block.end)],'file':symbol.symtab.fullname()}
        response_value(snapshot,'symbol_value_before_rsp',lambda:hex(int(symbol.value(frame))))
    except Exception as error:
        snapshot['symbol']={'unavailable':type(error).__name__+': '+str(error)};save()
    response_value(snapshot,'read_var_before_rsp',lambda:hex(int(frame.read_var('frame'))))
    response_value(snapshot,'dispatch_ancestor',lambda:{**response_frame(dispatch),
        'frame':hex(int(dispatch.read_var('frame'))),'thread':hex(int(dispatch.read_var('thread')))})
    require(response_query(snapshot,'Hgp%x.%x'%gdb.selected_thread().ptid[:2])=='OK',
            'raw register CPU selection failed')
    registers=response_register_layout(snapshot);raw={}
    snapshot['raw_rsp_registers']=raw;save()
    for name,reg in registers.items():
        payload=response_query(snapshot,'p%x'%reg['number'])
        require(re.fullmatch(r'[0-9a-fA-F]{16}',payload) is not None,'raw RV64 register reply is invalid')
        raw[name]={'number':reg['number'],'bytes':payload,'value':hex(int.from_bytes(bytes.fromhex(payload),'little'))};save()
    require(int(raw['pc']['value'],16)==pc,'raw remote PC differs from the selected response checkpoint')
    sp=int(raw['sp']['value'],16)
    require(sp>=0xffffffc000000000 and sp%8==0 and sp+layout['stack_size']<(1<<64),
            'raw response SP does not describe an aligned kernel stack range')
    require(response_query(snapshot,'qqemu.PhyMemMode')=='0','response stack read requires virtual debug memory mode')
    data=response_query(snapshot,'m%x,%x'%(sp,layout['stack_size']))
    require(re.fullmatch(r'[0-9a-fA-F]{%d}'%(layout['stack_size']*2),data) is not None,
            'response stack byte reply is invalid')
    stack=bytes.fromhex(data)
    snapshot['raw_rsp_stack']={'address':hex(sp),'bytes':data,
        'frame_slot':hex(sp+layout['frame_sp_offset']),
        'frame_value':hex(int.from_bytes(stack[layout['frame_sp_offset']:layout['frame_sp_offset']+8],'little')),
        'argument_slot':hex(sp+layout['argument_sp_offset']),
        'argument_value':hex(int.from_bytes(stack[layout['argument_sp_offset']:layout['argument_sp_offset']+8],'little'))}
    response_value(snapshot,'read_var_after_rsp',lambda:hex(int(frame.read_var('frame'))))
    save()
'''


def start(kernel, port, target_file):
    header = kernel.read_bytes()[:20]
    if header[4:6] != b"\x02\x01" or int.from_bytes(header[18:20], "little") != 243:
        raise RuntimeError("response diagnostics require the actual little-endian RV64 ELF")
    logs = debug.fresh_logs(Path("artifacts/mmio-forced"))
    symbols = debug.symbols(kernel)
    step = symbols["<kernel::vm::forced::ForcedRetirement as kernel::maintenance::Work>::step"]
    advance = symbols["<kernel::vm::forced::ForcedRetirement>::advance"]
    code = subprocess.check_output([
        "riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel), "-ex", "set language c",
        "-ex", "info line kernel/src/vm/forced.rs:266",
        "-ex", "info line kernel/src/vm/forced.rs:421",
        "-ex", "info line kernel/src/resource/mmio/node.rs:279",
        "-ex", "info line kernel/src/task/process/shell.rs:537",
        "-ex", "info line kernel/src/syscall.rs:840",
        "-ex", "info line kernel/src/syscall.rs:846",
        "-ex", f"disassemble /r {step:#x},{step+950:#x}",
        "-ex", f"disassemble /r {advance:#x},{advance+220:#x}",
        "-ex", f"disassemble /r {symbols['<kernel::resource::mmio::node::MmioNode>::revoke']:#x},{symbols['<kernel::resource::mmio::node::MmioNode>::revoke']+100:#x}",
        "-ex", f"disassemble /r {symbols['<kernel::task::process::shell::ProcessShell>::freeze_and_seal']:#x},{symbols['<kernel::task::process::shell::ProcessShell>::freeze_and_seal']+120:#x}",
        "-ex", f"disassemble /r {symbols['kernel::syscall::respond_ok']:#x},{symbols['kernel::syscall::respond_ok']+100:#x}",
        "-ex", f"disassemble /r {symbols['kernel::syscall::respond_error']:#x},{symbols['kernel::syscall::respond_error']+100:#x}",
    ], text=True, stderr=subprocess.STDOUT)
    (logs / "code-windows.log").write_text(code)
    def line_address(line):
        return int(re.search(rf"Line {line} .*?(?:starts at address|is at address) (0x[0-9a-f]+)", code)[1], 16)
    queued = line_address(266)
    post = line_address(421)
    revoke_point = line_address(279)
    kill_point = line_address(537)
    ok_point = line_address(840)
    error_point = line_address(846)
    response_locations = {}
    for name, point in (("respond_ok", ok_point), ("respond_error", error_point)):
        scope = subprocess.check_output([
            "riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel), "-ex", "set language c",
            "-ex", f"info scope *{point:#x}",
        ], text=True, stderr=subprocess.STDOUT)
        code += "\n" + scope
        offset = re.search(r"Symbol frame is a variable at frame base reg \$sp offset 0\+(\d+), length 8\.", scope)
        if offset is None:
            raise RuntimeError(f"{name} frame does not have the audited stack location")
        entry = symbols["kernel::syscall::" + name]
        before = [(int(address, 16), instruction) for address, instruction in re.findall(
            r"^\s*(0x[0-9a-f]+) <[^>]+>:\s+[0-9a-f]+\s+([^\n]+)", code, re.M)
            if entry <= int(address, 16) < point]
        stores = [address for address, instruction in before
                  if re.fullmatch(r"sd\s+a0," + offset[1] + r"\(sp\)", instruction.strip())]
        if not stores or any(re.match(r"s[bhdw]\s", instruction) and "(sp)" not in instruction
                             for _, instruction in before):
            raise RuntimeError(f"{name} checkpoint must follow its frame store and precede frame writes")
        argument = "ret" if name == "respond_ok" else "err"
        argument_offset = re.search(r"Symbol " + argument + r" is a variable at frame base reg \$sp offset 0\+(\d+), length 8\.", scope)
        stack_sizes = [int(match[1]) for _, instruction in before
                       if (match := re.fullmatch(r"addi\s+sp,sp,-(\d+)", instruction.strip()))]
        a0_writes = [instruction.strip() for _, instruction in before
                     if not re.match(r"s[bhdw]\s", instruction.strip())
                     and re.match(r"\S+\s+a0,", instruction.strip())]
        if argument_offset is None or len(stack_sizes) != 1:
            raise RuntimeError(f"{name} parameter slots or prologue stack size are unavailable")
        if (name == "respond_ok" and a0_writes) or (name == "respond_error" and
                (len(a0_writes) != 1 or not re.fullmatch(r"addi\s+a0,sp," + argument_offset[1], a0_writes[0]))):
            raise RuntimeError(f"{name} a0 provenance changed before the response checkpoint")
        if max(int(offset[1]), int(argument_offset[1])) + 8 > stack_sizes[0]:
            raise RuntimeError("response parameter slots exceed the audited prologue")
        response_locations[name] = {"point": hex(point), "frame_sp_offset": int(offset[1]),
                                    "frame_store": hex(stores[-1]), "stack_size": stack_sizes[0],
                                    "argument": argument, "argument_sp_offset": int(argument_offset[1]),
                                    "frame_base": "sp", "a0_role": "frame" if name == "respond_ok" else "error_slot"}
    (logs / "code-windows.log").write_text(code)
    call_line = next(line for line in code.splitlines() if "16ForcedRetirement7advance>" in line)
    call = int(re.search(r"0x[0-9a-f]+", call_line)[0], 16)
    locks = [int(re.search(r"0x[0-9a-f]+", line)[0], 16) for line in code.splitlines() if "forced5QueueE4lock" in line]
    if not call < post < next(address for address in locks if address > call):
        raise RuntimeError("forced Publishing checkpoint is not after advance and before the actual Queue lock")
    root_lock = next(int(re.search(r"0x[0-9a-f]+", line)[0],16) for line in code.splitlines() if "mmio5StateE4lock" in line)
    if not symbols["<kernel::resource::mmio::node::MmioNode>::revoke"] < revoke_point < root_lock:
        raise RuntimeError("Revoke selection must be after its argument stores and before its root lock")
    # 原六点均只能跨一条已核的非阻塞指令；不单步等待、原子锁操作或控制转移。
    instructions = {int(address, 16): (encoding, instruction.strip())
                    for address, encoding, instruction in re.findall(
                        r"^\s*(0x[0-9a-f]+) <[^>]+>:\s+([0-9a-f]+)\s+([^\n]+)", code, re.M)}
    step_over = {}
    for label, point in (("queued", queued), ("publishing", post), ("revoke", revoke_point),
                         ("kill", kill_point), ("respond_ok", ok_point), ("respond_error", error_point)):
        encoding, instruction = instructions[point]
        # 两条store分别属于已核栈Execution和原响应frame；不接受其他store形态。
        pattern = (r"sb\s+a1,1031\(a2\)" if label == "queued" else
                   r"sd\s+a2,80\(a0\)" if label == "respond_ok" else
                   r"auipc\s+ra,0x[0-9a-f]+")
        if not re.fullmatch(pattern, instruction):
            raise RuntimeError(f"checkpoint instruction is not an audited nonblocking step: {instruction}")
        size = len(encoding) // 2
        if size not in (2, 4):
            raise RuntimeError("unexpected checkpoint instruction width")
        step_over[point] = {"label": label, "next": point + size,
                            "encoding": encoding, "instruction": instruction}
    evidence = logs / "evidence.json"
    evidence.write_text(json.dumps({
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "real two Map/Revoke/Kill; no product state/result/PC/clock writes; no timing claim",
        "code": {"queued":hex(queued), "publishing":hex(post), "advance_call":hex(call),
                 "revoke_selection":hex(revoke_point), "kill_selection":hex(kill_point),
                 "respond_ok":hex(ok_point), "respond_error":hex(error_point)},
        "response_locations": response_locations,
        "step_over_instructions": {hex(point): detail for point, detail in step_over.items()},
        "windows": [], "verified": False,
    }, indent=2)+"\n")
    script = f'''set pagination off
set confirm off
set language rust
set breakpoint always-inserted on
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb,json,re,time
import xml.etree.ElementTree as ET
from pathlib import Path
TARGET=Path({json.dumps(str(target_file.resolve()))})
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
facts['gdb_configuration']={{'version':gdb.VERSION,'remotetimeout':gdb.parameter('remotetimeout'),
    'watchdog':gdb.parameter('watchdog'),'non_stop':gdb.parameter('non-stop'),
    'tcp_connect_timeout':gdb.parameter('tcp connect-timeout')}}
{debug.user_trap_context(kernel,symbols['handle_user_trap'])}
def require(test,message):
    if not test:raise gdb.GdbError(message)
def variant(value):return next((f.name for f in value.type.fields() if f.name),None)
def optional(value):
    tag=variant(value)
    if tag=='None':return None
    require(tag=='Some','unexpected Option layout: '+str(value.type))
    inner=value['Some'];return inner[next(f.name for f in inner.type.fields() if f.name)]
def pointer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_PTR,gdb.TYPE_CODE_INT):
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        require(key is not None,'unexpected pointer wrapper: '+str(value.type));value=value[key]
    return value
def arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['data']
def caller(part):
    frame=gdb.newest_frame()
    while frame:
        if part in (frame.name() or ''):return frame
        frame=frame.older()
    return None
def pid(thread,source,origin):
    shell=observed_word(thread+process_offset+ptr_offset+pointer_offset,source+'.process pointer',
                        {{'thread':thread,'original_frame':origin}})
    return observed_word(shell+data_offset+pid_offset,source+'.process.pid',
                         {{'thread':thread,'shell':shell,'original_frame':origin}})
def values(table):
    stack=[table['root']];count=0
    while stack:
        owned=optional(stack.pop())
        if owned is None:continue
        node=pointer(owned['data']).dereference();yield node['value']
        stack.extend([node['left'],node['right']]);count+=1
        require(count<=int(table['len']),'table traversal exceeded len')
    require(count==int(table['len']),'table traversal incomplete')
{RESPONSE_SNAPSHOT}
STEP_AT={step_over!r}
MAX_OBSERVATIONS=128
def raw_resume(actions):
    packets=facts.setdefault('packets',[])
    require(len(packets)<MAX_OBSERVATIONS,'bounded packet trace ended before another resume')
    started=time.monotonic()
    packet={{'sequence':len(packets)+1,'actions':actions,'origin_stop':facts.get('last_stop'),
             'response':None,'halt_observed':False}}
    packets.append(packet);facts['last_packet']=packet
    facts['last_stop']=None;save()
    try:
        response=gdb.execute('maintenance packet vCont;'+';'.join(actions),to_string=True)
        packet['response']=response;save()
        require(re.search(r'received: "T05',response) is not None,'partial resume did not stop with SIGTRAP')
        match=re.search(r'thread:p([0-9a-f]+)\\.([0-9a-f]+);',response)
        require(match is not None,'partial resume did not identify its actual stopped CPU')
        stopped=tuple(int(part,16) for part in match.groups())
        gdb.execute('maintenance flush register-cache',to_string=True)
        next(t for t in gdb.selected_inferior().threads() if t.ptid[:2]==stopped).switch()
        gdb.execute('maintenance flush dcache',to_string=True)
        gdb.execute('maintenance flush register-cache',to_string=True)
        gdb.invalidate_cached_frames()
        pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
        packet.update(halt_observed=True,stopped=list(stopped),pc=hex(pc))
        facts['last_stop']={{'pc':hex(pc),'hart':gdb.selected_thread().ptid[1]-1,
            'gdb_thread':gdb.selected_thread().num,'stage':'confirmed remote SIGTRAP',
            'mode':None if record is None else record['mode']}}
        return stopped,pc
    except Exception as error:
        packet['error']={{'type':type(error).__name__,'message':str(error)}}
        raise
    finally:
        packet['host_elapsed_seconds']=time.monotonic()-started;save()
def partial(threads):
    require(bool(threads),'no independent admitted CPU can advance the parent')
    admitted={{t.ptid[:2] for t in threads}}
    if parent_held and worker is None:
        require(parent.ptid[:2] not in admitted,'parent must remain held until the Work window')
    if worker is not None:
        require(worker.ptid[:2] not in admitted,'held Work must not resume before Frozen')
    # 只跨刚处理且本次准许运行的CPU停点；未观察/仍持有的其他CPU不提前单步。
    thread=gdb.selected_thread()
    if thread.ptid[:2] in admitted:
        gdb.execute('maintenance flush register-cache',to_string=True)
        gdb.invalidate_cached_frames()
        pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
        if pc in STEP_AT and any(bp.enabled and address==pc for bp,address in points):
            # raw包不经过infrun；本固定QEMU的单步覆盖BP_GDB，其他CPU保持暂停。
            detail={{'thread':list(thread.ptid[:2]),'pc':hex(pc),**STEP_AT[pc]}}
            facts['last_step_over']=detail;save()
            stopped,after=raw_resume(['s:p%x.%x'%thread.ptid[:2]])
            detail.update(stopped=list(stopped),after=hex(after));save()
            require(stopped==thread.ptid[:2] and after==STEP_AT[pc]['next'],
                    'audited single instruction did not finish on its admitted CPU')
            counts=facts.setdefault('step_over_counts',{{}})
            counts[hex(pc)]=counts.get(hex(pc),0)+1;save()
    raw_resume(['c:p%x.%x'%t.ptid[:2] for t in threads])
def save():EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
def describe_frame(frame):
    return {{'function':frame.name(),'pc':hex(int(frame.pc()))}}
def failure(error):
    facts['failure']={{'type':type(error).__name__,'message':str(error),
        'stop':facts.get('last_stop'),'last_word_read':facts.get('word_read')}};save()
    if facts.get('last_stop') is None:
        facts['failure']['backtrace_unavailable']='no confirmed halt after the latest resume'
        facts['failure']['last_word_is_historical']=True
    else:
        try:facts['failure']['backtrace']=gdb.execute('bt',to_string=True)
        except Exception as trace_error:facts['failure']['backtrace_unavailable']=str(trace_error)
    save()
def observed_word(address,source,origin):
    # 在原共享word准入检查之前保留本次地址；错误仍原样抛出。
    facts['word_read']={{'address':hex(address),'source':source,'origin':origin,
        'stop':facts.get('last_stop')}};save()
    try:return word(address)
    except Exception as error:
        facts['word_read']['error']={{'type':type(error).__name__,'message':str(error)}};save()
        raise
EXECUTION='kernel::vm::forced::Execution'
VM='kernel::vm::change::VmShell'
WORK='kernel::vm::forced::ForcedRetirement'
def fresh(address,name):
    return gdb.Value(address).cast(gdb.lookup_type(name).pointer()).dereference()
def live_vm(weak):
    # 全停快照：Weak保留ArcInner，但strong为零时data已退，不能按旧地址继续读VM。
    inner=pointer(weak)
    kind=inner.type.target()
    strong=next(field for field in kind.fields() if field.name=='strong')
    data=next(field for field in kind.fields() if field.name=='data')
    require(strong.type.sizeof==8,'VM Arc strong count is not the audited usize layout')
    count=int.from_bytes(bytes(gdb.selected_inferior().read_memory(
        int(inner)+strong.bitpos//8,strong.type.sizeof)),'little')
    detail={{'inner':int(inner),'strong':count,'data_offset':data.bitpos//8}}
    return (None if count==0 else fresh(int(inner)+data.bitpos//8,VM)),detail
def raw_field(address,name,member):
    field=next(field for field in gdb.lookup_type(name).fields() if field.name==member)
    return bytes(gdb.selected_inferior().read_memory(address+field.bitpos//8,field.type.sizeof)).hex()
def reject_candidate(reason,detail):
    # 每模式/原因只留首份完整现场；计数不是产品推进界。
    counts=facts.setdefault('candidate_rejection_counts',{{}}).setdefault(record['mode'],{{}})
    counts[reason]=counts.get(reason,0)+1
    first=facts.setdefault('candidate_rejections',{{}}).setdefault(record['mode'],{{}})
    if reason not in first:
        first[reason]={{'stop':facts['last_stop'],**detail}}
    save()
facts['declarations']={{}}
for name in (EXECUTION,VM,WORK):
    kind=gdb.lookup_type(name)
    facts['declarations'][name]={{'objfile':kind.objfile.filename,'size':kind.sizeof,
        'fields':[{{'name':field.name,'offset':field.bitpos//8,'type':str(field.type),'size':field.type.sizeof}}
                  for field in kind.fields() if field.name]}}
save()
revoke=gdb.Breakpoint('*{revoke_point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
kill=gdb.Breakpoint('*{kill_point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
ok=gdb.Breakpoint('*{ok_point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
error=gdb.Breakpoint('*{error_point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
queue_bp=gdb.Breakpoint('*{queued:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
post_bp=gdb.Breakpoint('*{post:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
for breakpoint in (revoke,kill,ok,error,queue_bp,post_bp):breakpoint.silent=True
for breakpoint in (kill,ok,error,queue_bp,post_bp):breakpoint.enabled=False
points=((revoke,{revoke_point}),(kill,{kill_point}),(ok,{ok_point}),(error,{error_point}),
        (queue_bp,{queued}),(post_bp,{post}))
record=None;worker=None;parent=None;parent_held=False;execution_address=None
try:
    while len(facts['windows'])<2:
        if worker is not None:
            if parent_held:
                require(parent.ptid!=worker.ptid,'parent Kill and held Work are on the same CPU')
            partial([t for t in gdb.selected_inferior().threads() if t.ptid!=worker.ptid])
        elif parent_held and worker is None:partial([t for t in gdb.selected_inferior().threads() if t.ptid!=parent.ptid])
        else:
            facts['last_stop']=None;save()
            gdb.execute('continue',to_string=True)
        pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
        facts['last_stop']={{'pc':hex(pc),'hart':gdb.selected_thread().ptid[1]-1,
            'gdb_thread':gdb.selected_thread().num,'function':None,'stage':'fresh frame after resume',
            'mode':None if record is None else record['mode']}};save()
        gdb.invalidate_cached_frames()
        frame=gdb.newest_frame()
        facts['last_stop'].update(function=frame.name(),stage='dispatch');save()
        if pc=={revoke_point} and record is None:
            facts['last_stop']['stage']='Revoke source identity';save()
            original=caller('kernel::resource::mmio::object::revoke')
            if original is None or not TARGET.exists():continue
            thread=int(original.read_var('thread'))
            if pid(thread,'Revoke.thread',describe_frame(original))!=1:continue
            target=json.loads(TARGET.read_text())
            if target['mode'] in [r['mode'] for r in facts['windows']]:continue
            native_frame=thread+thread_offsets['frame']
            if observed_word(native_frame+context_offsets['x']+10*8,'Revoke Thread.frame.x[10]',
                             {{'thread':thread,'native_frame':hex(native_frame),'original_frame':describe_frame(original)}})!=target['scope']:continue
            facts['selection']={{**target,'stage':'scope argument after prologue','pc':hex(pc),'thread':thread}}
            save()
            scope=arc(frame.read_var('self'))
            facts['selection']['stage']='original scope mapping table';save()
            entries=list(values(arc(scope['root'])['state']['data']['value']['mappings']))
            mappings=[arc(entry['mapping']) for entry in entries]
            facts['selection']['stage']='original mapping VM identities';save()
            require(len(mappings)==2,'original scope does not have the two real Map responsibilities')
            require(all(int(m['physical'])==int(scope['physical']) and int(m['pages'])==32 for m in mappings),'original Map geometry differs from scope')
            observed_vms=[live_vm(m['vm']) for m in mappings]
            facts['selection']['vm_owners']=[detail for _,detail in observed_vms];save()
            vms=[vm for vm,_ in observed_vms]
            require(all(vm is not None for vm in vms),'original Maps lost their live VM owner')
            require(int(vms[0].address)==int(vms[1].address),'two Maps do not belong to one actual VM')
            vm=fresh(int(vms[0].address),VM)
            require(int(vms[0]['identity'])==int(vms[1]['identity']),'two Maps changed the actual VM identity')
            record={{**target,'parent_frame':native_frame,'parent_sepc':observed_word(native_frame+context_offsets['sepc'],
                     'Revoke Thread.frame.sepc',{{'thread':thread,'native_frame':hex(native_frame),'original_frame':describe_frame(original)}}),
                    'vm':int(vm.address),'vm_identity':int(vm['identity']),'satp':int(vm['satp']),
                     'mapping_ids':sorted(int(pointer(m['id'])) for m in mappings)}}
            facts['pending']=record;save()
            revoke.enabled=False;kill.enabled=True;ok.enabled=True;error.enabled=True
            queue_bp.enabled=target['mode']=='mmio-forced-queued';post_bp.enabled=not queue_bp.enabled
            continue
        if record is None:continue
        if pc=={kill_point}:
            facts['last_stop']['stage']='Kill source identity';save()
            original=caller('kernel::task::process::kill')
            if original is None:continue
            shell=arc(frame.read_var('self'))
            if int(shell['pid'])!=record['pid']:continue
            thread=int(original.read_var('caller'))
            require(pid(thread,'Kill.caller',describe_frame(original))==1,'Kill is not from the original init consumer')
            require(record.get('revoke_returned'),'parent Kill preceded the observed Revoke success')
            parent=gdb.selected_thread();parent_held=True
            record['kill_frame']=thread+thread_offsets['frame']
            record['kill_sepc']=observed_word(record['kill_frame']+context_offsets['sepc'],'Kill caller.frame.sepc',
                {{'thread':thread,'native_frame':hex(record['kill_frame']),'original_frame':describe_frame(original)}})
            kill.enabled=False
            if worker is None:continue
        elif pc in ({ok_point},{error_point}):
            facts['last_stop']['stage']='respond frame parameter/sepc';save()
            response_snapshot(frame,pc)
            native_frame=int(frame.read_var('frame'))
            sepc=observed_word(native_frame+context_offsets['sepc'],'respond frame.sepc',
                {{'native_frame':hex(native_frame),'original_frame':describe_frame(frame),
                  'parent_frame':hex(record['parent_frame']),'kill_frame':record.get('kill_frame')}})
            if not record.get('revoke_returned') and native_frame==record['parent_frame'] and sepc==record['parent_sepc']:
                require(pc=={ok_point},'original parent Revoke failed')
                record['revoke_returned']=True;print('MMIO_FORCED_REVOKE_RETURNED '+json.dumps(record,sort_keys=True));save();continue
            if record.get('kill_frame') is None or native_frame!=record['kill_frame'] or sepc!=record['kill_sepc']:continue
            require(pc=={ok_point},'original parent Kill failed')
            record['read_stage']='fresh Frozen VM/held Execution';save()
            require(worker is not None,'Kill completed before the intended Work window')
            # 原Work hart一直held，唯一Execution仍在栈上；VM只在核到活强owner后读。
            vm,weak=live_vm(fresh(record['work'],WORK)['vm'])
            record['frozen_read']={{'weak':weak,'vm':None if vm is None else int(vm.address),
                'vm_identity':None if vm is None else int(vm['identity']),'execution':execution_address}};save()
            require(vm is not None,'held Work lost its live VM owner')
            require(int(vm.address)==record['vm'] and int(vm['identity'])==record['vm_identity'],
                    'held Work changed the original VM identity')
            record['frozen_read']['access_open']=bool(vm['space']['data']['value']['access_open']);save()
            require(not record['frozen_read']['access_open'],'Kill returned without actual VM Frozen')
            record['frozen_read']['phase_raw']=raw_field(execution_address,EXECUTION,'phase')
            record['frozen_read']['active_raw']=raw_field(execution_address,EXECUTION,'active');save()
            held=fresh(execution_address,EXECUTION)
            active=optional(held['active'])
            record['frozen_read'].update(phase=variant(held['phase']),
                active=None if active is None else int(pointer(arc(active['mapping'])['id'])));save()
            require(record['frozen_read']['phase']==record['phase'],'Frozen replaced the held original execution')
            require(record['frozen_read']['active']==record.get('active_mapping'),
                    'Frozen changed the held original active mapping')
            record['frozen']=True;record['access_open']=False
            facts['windows'].append(record);facts.pop('pending',None);save()
            print('MMIO_FORCED_FROZEN_BEFORE_RESUME '+json.dumps(record,sort_keys=True))
            # 此后不再读旧地址；原消费者的同PID/scope完成/退款由宿主另核。
            for breakpoint in (kill,ok,error,queue_bp,post_bp):breakpoint.enabled=False
            worker=None;parent=None;parent_held=False;record=None;execution_address=None
            revoke.enabled=True;continue
        elif pc in ({queued},{post}):
            facts['last_stop']['stage']='Work execution/queue';save()
            samples=facts.setdefault('candidate_samples',{{}})
            samples[record['mode']]=samples.get(record['mode'],0)+1;save()
            # 显式宿主采样成本：每模式至多128次Work检查，不是内核活性/性能阈值。
            require(samples[record['mode']]<=MAX_OBSERVATIONS,'bounded Work sampling ended without an eligible window')
            work=arc(frame.read_var('self'))
            vm,weak=live_vm(fresh(int(work.address),WORK)['vm'])
            if vm is None:
                reject_candidate('retired-vm',{{'work':hex(int(work.address)),'weak':weak}})
                continue
            if int(vm.address)!=record['vm'] or int(vm['identity'])!=record['vm_identity']:
                reject_candidate('different-vm',{{'work':hex(int(work.address)),'vm':hex(int(vm.address)),
                    'vm_identity':int(vm['identity']),'expected_vm':hex(record['vm']),
                    'expected_vm_identity':record['vm_identity'],'phase':None,'active':None,'fifo':None}})
                continue
            execution=frame.read_var('execution')
            address=int(execution) if execution.type.strip_typedefs().code==gdb.TYPE_CODE_PTR else int(execution.address)
            execution=fresh(address,EXECUTION)
            phase=variant(execution['phase']);active=optional(execution['active'])
            queue=work['queue']['data']['value']
            queued_ids=sorted(int(pointer(arc(optional(entry['mapping']))['id'])) for entry in values(queue['entries']))
            active_id=None if active is None else int(pointer(arc(active['mapping'])['id']))
            candidate={{'work':hex(int(work.address)),'vm':hex(int(vm.address)),'vm_identity':int(vm['identity']),'phase':phase,
                       'active':active_id,'fifo':queued_ids,'expected_mappings':record['mapping_ids']}}
            if worker is not None:continue
            if record['mode']=='mmio-forced-queued':
                if phase!='Idle' or active is not None:
                    reject_candidate('not-unstarted',candidate);continue
                if not queued_ids:
                    reject_candidate('empty-fifo',candidate);continue
                if not set(queued_ids).issubset(record['mapping_ids']):
                    reject_candidate('foreign-mapping',candidate);continue
            else:
                if phase!='Publishing' or active is None:
                    reject_candidate('not-committed-publishing',candidate);continue
                if active_id not in record['mapping_ids']:
                    reject_candidate('foreign-active-mapping',candidate);continue
                record['active_mapping']=active_id
            require(optional(work['execution']['data']['value']) is None,'step did not hold its sole Execution outside the field')
            record.update(phase=phase,queued_ids=queued_ids,work=int(work.address),window_pc=hex(pc))
            execution_address=int(execution.address)
            record['execution']=execution_address
            record['phase_raw']=raw_field(execution_address,EXECUTION,'phase')
            record['active_raw']=raw_field(execution_address,EXECUTION,'active')
            worker=gdb.selected_thread();queue_bp.enabled=False;post_bp.enabled=False
            print('MMIO_FORCED_HELD '+json.dumps(record,sort_keys=True));save();continue
except Exception as error:
    failure(error)
    raise
facts['verified']=True;save()
print('MMIO_FORCED_VERIFIED original nonempty FIFO and committed Publishing captured across actual Frozen; consumer completion remains separate')
end
detach
quit 0
'''
    script_path = logs / "forced.gdb"
    script_path.write_text(script)
    ast.parse(script.split("\npython\n",1)[1].split("\nend\n",1)[0])
    output = (logs / "forced.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb","-q","-nx","--batch","-x",str(script_path)],stdout=output,stderr=subprocess.STDOUT)
    return child,output,logs


def verify(logs,status):
    facts=json.loads((logs/"evidence.json").read_text())
    if status or not facts.get("verified") or {r["mode"] for r in facts["windows"]}!=MODES:
        raise RuntimeError(f"MMIO Forced/Frozen evidence incomplete: {logs}")
    return facts
