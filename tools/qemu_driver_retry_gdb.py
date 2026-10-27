"""两个真实Control Retry的begin/迟到commit，只读用户态原记录与实际执行身份。"""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug


def start(kernel, port, driver_pid, device):
    driver = Path("artifacts/build/srv_driver").resolve()
    init = Path("artifacts/build/srv_init").resolve()
    source = Path("user/services/srv_init/src/driver.rs").read_text().splitlines()
    caller_line = next(i for i,line in enumerate(source,1) if "let caller_id = worker.id();" in line)
    retired_line = next(i for i,line in enumerate(source,1) if "let main_result = main_result?;" in line)
    logs = debug.fresh_logs(Path("artifacts/device-retry"))
    evidence = logs / "evidence.json"
    facts = {"kernel_sha256":hashlib.sha256(kernel.read_bytes()).hexdigest(),
             "driver_sha256":hashlib.sha256(driver.read_bytes()).hexdigest(),
             "init_sha256":hashlib.sha256(init.read_bytes()).hexdigest(),
             "boot_package_sha256":hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
             "pid":driver_pid, "device":device, "scope":"two real Control calls; read-only original actor records; no scheduling or time claim",
             "begins":[], "commits":[], "responses":[], "verified":False}
    evidence.write_text(json.dumps(facts,indent=2)+"\n")
    code = subprocess.check_output([
        "riscv64-elf-gdb","-q","-nx","--batch",str(driver),"-ex","set language c",
        "-ex","info line src/server/control.rs:176",
        "-ex","info line src/server/control.rs:195",
        "-ex","info line src/server/control.rs:109",
        "-ex","add-symbol-file "+str(init),
        "-ex",f"info line services/srv_init/src/driver.rs:{caller_line}",
        "-ex",f"info line services/srv_init/src/driver.rs:{retired_line}",
    ],text=True,stderr=subprocess.STDOUT)
    (logs/"code-windows.log").write_text(code)
    points = {line:int(re.search(rf"Line {line} .*?(?:starts at address|is at address) (0x[0-9a-f]+)",code)[1],16) for line in (176,195,109)}
    caller_pc = int(re.search(rf"Line {caller_line} of .*srv_init/src/driver.rs.*?(?:starts at address|is at address) (0x[0-9a-f]+)",code)[1],16)
    retired_pc = int(re.search(rf"Line {retired_line} of .*srv_init/src/driver.rs.*?(?:starts at address|is at address) (0x[0-9a-f]+)",code)[1],16)
    symbols=debug.symbols(kernel)
    script=f'''set pagination off
set confirm off
set language rust
set breakpoint always-inserted on
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb,json
from pathlib import Path
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
{debug.user_trap_context(kernel,symbols['handle_user_trap'])}
USER_ELFS={{1:{json.dumps(str(init))},facts['pid']:{json.dumps(str(driver))}}}
def require(test,message):
    if not test:raise gdb.GdbError(message)
def variant(value):return next((f.name for f in value.type.fields() if f.name),None)
def optional(value):
    tag=variant(value)
    if tag=='None':return None
    require(tag=='Some','unexpected Retry Option layout: '+str(value.type))
    inner=value['Some'];return inner[next(f.name for f in inner.type.fields() if f.name)]
def pointer(value):
    while value.type.strip_typedefs().code!=gdb.TYPE_CODE_PTR:
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        require(key is not None,'unexpected Retry Rc wrapper: '+str(value.type));value=value[key]
    return value
def integer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_INT,gdb.TYPE_CODE_BOOL):
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('value','v','inner','__0','0') if n in names),None)
        require(key is not None,'unexpected Retry integer layout: '+str(value.type));value=value[key]
    return int(value)
def rc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['value']['value']['value']
def reference(value):
    return {{'incarnation':integer(value['device']['incarnation']),'device':integer(value['device']['serial']),'epoch':integer(value['epoch'])}}
def record(value):
    run=rc(value)
    return {{'rc':int(pointer(value.dereference() if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR else value)),
             'reference':reference(run['reference']),'implementation':int(run['implementation']),
             'phase':str(run['phase']),'task':int(run['task']),
             'process':optional(run['process']) is not None,
             'binding_watch':int(run['binding_watch']),'activation_watch':int(run['activation_watch'])}}
def values(table):
    todo=[table['root']];count=0
    while todo:
        item=optional(todo.pop())
        if item is None:continue
        node=pointer(item['data']).dereference();yield node['value']
        todo.extend([node['left'],node['right']]);count+=1
        require(count<=int(table['len']),'Run traversal exceeded real table len')
    require(count==int(table['len']),'Run traversal omitted an entry')
def owner(expected):
    # 用户tp是TLS；正式用户返回把真正HartLocal放在sscratch。
    anchor=int(gdb.parse_and_eval('$sscratch'))&((1<<64)-1)
    if anchor<{symbols['handle_user_trap']-debug.physical_address(kernel,symbols['handle_user_trap'])}:return None
    thread=word(anchor+hart_offsets['current_thread'])
    if not thread:return None
    shell=word(thread+process_offset+ptr_offset+pointer_offset)
    if word(shell+data_offset+pid_offset)!=expected:return None
    satp=int(gdb.parse_and_eval('$satp'))&((1<<64)-1)
    require(satp==word(thread+thread_offsets['satp']),'Retry user PC differs from original Thread SATP')
    return {{'thread':thread,'frame':thread+thread_offsets['frame'],'satp':hex(satp),'hart':gdb.selected_thread().ptid[1]-1}}
def save():EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
def select_user(pid,pc):
    # 用户映像共享VA范围；PID/SATP已由原纯地址门核实后才选择符号。
    target=USER_ELFS[pid]
    loaded=[str(Path(objfile.filename).resolve()) for objfile in gdb.objfiles()
            if str(Path(objfile.filename).resolve()) in USER_ELFS.values()]
    for filename in loaded:
        if filename!=target:gdb.execute('remove-symbol-file '+json.dumps(filename),to_string=True)
    if target not in loaded:gdb.execute('add-symbol-file '+json.dumps(target),to_string=True)
    gdb.invalidate_cached_frames()
    selected=[str(Path(objfile.filename).resolve()) for objfile in gdb.objfiles()
              if str(Path(objfile.filename).resolve()) in USER_ELFS.values()]
    require(selected==[target],'Retry user symbols do not match the authenticated process')
    if facts.get('active_user_elf')!=target:
        facts.setdefault('symbol_selections',[]).append({{'pid':pid,'pc':hex(pc),'removed':[
            filename for filename in loaded if filename!=target],'active':target}})
        facts['active_user_elf']=target;save()
def caller_variable(name):
    # PC对应的最内层帧可能是core内联体；只从实际Caller外层的变量scope读取。
    frame=gdb.newest_frame();trace=[]
    while frame is not None:
        trace.append({{'name':frame.name(),'pc':hex(int(frame.pc()))}})
        if frame.name()=='srv_init::driver::Manager::retry_concurrent':
            block=frame.block()
            while block is not None:
                for symbol in block:
                    if symbol.name==name and symbol.is_variable:
                        facts['caller_scope']={{'name':name,'start':hex(block.start),'end':hex(block.end),
                            'declared_type':str(symbol.type),'objfile':symbol.type.objfile.filename,'frames':trace}};save()
                        return symbol.value(frame)
                if block.function is not None:break
                block=block.superblock
        frame=frame.older()
    facts['caller_scope_failure']={{'name':name,'frames':trace}};save()
    raise gdb.GdbError('Caller variable lacks its actual outer frame/scope: '+name)
def registry_slots(registry):
    vector=registry['inner']['data']['value']['slots']
    kind=gdb.lookup_type('libthread::SlotState<libthread::NoEnvironment>')
    data=pointer(vector['buf']['inner']['ptr']).cast(kind.pointer())
    return data,int(vector['len'])
registry_name='libthread::ThreadRegistry<libthread::NoEnvironment>'
def observe_user(pc,execution,sequence):
    # 此调用返回后不保留任何用户objfile的Frame/Type/Symbol/Value，再允许切换映像。
    global caller_registry
    frame=gdb.newest_frame()
    facts['last_stage']={{'pc':hex(pc),'sequence':sequence,'function':frame.name(),**execution}};save()
    if pc=={caller_pc}:
        worker=caller_variable('worker')
        registry_kind=gdb.lookup_type(registry_name)
        require(str(Path(registry_kind.objfile.filename).resolve())==USER_ELFS[1],'Caller registry declaration is not from init')
        facts['caller_registry_declaration']={{'type':registry_name,'objfile':registry_kind.objfile.filename,
            'size':registry_kind.sizeof,'fields':[{{'name':field.name,'type':str(field.type),
                'offset':field.bitpos//8,'size':field.type.sizeof}} for field in registry_kind.fields() if field.name]}};save()
        registry=gdb.Value(int(worker['registry'])).cast(gdb.lookup_type(registry_name).pointer()).dereference()
        data,length=registry_slots(registry)
        control=int(pointer(worker['control']))
        selected=[]
        for index in range(length):
            slot=(data+index).dereference()
            if variant(slot)!='Live':continue
            inner=slot['Live'];record_value=inner[next(f.name for f in inner.type.fields() if f.name)]
            recorded=optional(record_value['control'])
            if recorded is not None and int(pointer(recorded))==control:
                require(optional(record_value['stack']) is not None,'Caller record lost its actual stack owner')
                selected.append(index)
        require(len(selected)==1,'new Caller control does not identify one actual registry record')
        caller_registry=(int(registry.address),selected[0])
        facts['caller']={{'tid':integer(worker['id']),'control_arc':control,'registry':int(registry.address),'slot':selected[0],
                          'before':'Live','sequence':sequence,**execution}}
        print('DRIVER_RETRY_CALLER_RECORD '+json.dumps(facts['caller'],sort_keys=True));save();return
    if pc=={retired_pc}:
        require(caller_registry is not None,'Caller retirement lacks its actual admitted record')
        address,index=caller_registry
        registry_kind=gdb.lookup_type(registry_name)
        require(str(Path(registry_kind.objfile.filename).resolve())==USER_ELFS[1],'Retired Caller registry declaration is not from init')
        facts['caller_retired_registry_declaration']={{'type':registry_name,'objfile':registry_kind.objfile.filename,
            'size':registry_kind.sizeof}};save()
        data,length=registry_slots(gdb.Value(address).cast(registry_kind.pointer()).dereference())
        require(index<length and variant((data+index).dereference())=='Free','joined Caller record still owns retirement resources')
        require(integer(caller_variable('caller_id'))==facts['caller']['tid'],'joined Caller identity changed')
        facts['caller_record_retired']=True
        facts['caller']['after']='Free';facts['caller']['retired_sequence']=sequence
        print('DRIVER_RETRY_CALLER_RETIRED '+json.dumps(facts['caller'],sort_keys=True));save();return
    if pc=={points[176]}:
        request=frame.read_var('self').dereference()
        require(bool(request['control']) and bool(request['authorized']),'Retry begin was not authenticated Control')
        retry=optional(request['retry'])
        require(retry is not None,'begin checkpoint precedes the original Retry submission')
        original=record(retry['run'])
        if original['reference']['device']!=facts['device']:return
        require(original['phase'].endswith('Terminal') and original['task']==0 and not original['process'] and original['binding_watch']==0 and original['activation_watch']==0,'Retry did not begin on the original settled record')
        facts['begins'].append({{'request':int(request.address),'txid':int(retry['txid']),**original,'sequence':sequence,**execution}})
        print('DRIVER_RETRY_BEGIN '+json.dumps(facts['begins'][-1],sort_keys=True));save();return
    if pc=={points[195]}:
        retry=frame.read_var('retry').dereference()
        original=record(retry['run'])
        if original['reference']['device']!=facts['device']:return
        info=frame.read_var('info')
        require(optional(info['binding']) is None and bool(info['present']),'late commit did not pass the actual present/unbound sampling gates')
        world=frame.read_var('world').dereference()
        current=[record(value) for value in values(world['runs']) if reference(rc(value)['reference'])['device']==facts['device']]
        require(len(current)<=1,'World contains more than one original device selection')
        context={{'txid':int(retry['txid']),'original':original,'current':current[0] if current else None,
                  'stopping':bool(world['stopping']),'sequence':sequence,**execution}}
        facts['commits'].append(context)
        print('DRIVER_RETRY_COMMIT '+json.dumps(context,sort_keys=True));save();return
    request=frame.read_var('self').dereference()
    if int(request['header']['op'])!=3 or not bool(request['control']):return
    original=next((b for b in facts['begins'] if b['request']==int(request.address)),None)
    if original is None:
        # 原复用路线还核活PCI的Busy；没有本目标begin的回复不能凑进两次迟到响应。
        facts['unrelated_responses']=facts.get('unrelated_responses',0)+1
        save()
        return
    error=optional(frame.read_var('error'))
    response={{'request':int(request.address),'txid':original['txid'] if original else None,
              'error':str(error) if error is not None else None,'sequence':sequence,**execution}}
    facts['responses'].append(response)
    print('DRIVER_RETRY_RESPONSE '+json.dumps(response,sort_keys=True));save()
for address in {list(points.values())+[caller_pc,retired_pc]!r}:
    breakpoint=gdb.Breakpoint('*%#x'%address,type=gdb.BP_HARDWARE_BREAKPOINT);breakpoint.silent=True
sequence=0
caller_registry=None
while len(facts['responses'])<2 or not facts.get('caller_record_retired'):
    gdb.execute('continue',to_string=True)
    pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
    pid=1 if pc in ({caller_pc},{retired_pc}) else facts['pid']
    execution=owner(pid)
    if execution is None:continue
    select_user(pid,pc)
    sequence+=1
    observe_user(pc,execution,sequence)
begins=facts['begins'];responses=facts['responses']
success=[r for r in responses if r['error'] is None]
if len(begins)==2 and len(success)==1:
    a,b=begins
    same=all(a[key]==b[key] for key in ('rc','reference','implementation'))
    simultaneous=max(a['sequence'],b['sequence'])<success[0]['sequence']
    late=[c for c in facts['commits'] if c['sequence']>success[0]['sequence'] and c['txid']!=success[0]['txid']]
    if same and simultaneous and len(late)==1:
        current=late[0]['current'];original=late[0]['original']
        if current is None:facts['coverage']='removed-record'
        elif current['rc']!=original['rc']:facts['coverage']='replacement-Rc'
        elif current['reference']!=original['reference'] or current['implementation']!=original['implementation']:facts['coverage']='same-Rc-new-reference-or-implementation'
        else:facts['coverage']='late-without-change'
        facts['verified']=facts['coverage']!='late-without-change' and any(r['error'] is not None and r['txid']==late[0]['txid'] for r in responses)
facts['finished']=True;save()
print('DRIVER_RETRY_RACE_RESULT '+json.dumps({{'verified':facts['verified'],'coverage':facts.get('coverage'),'begins':len(begins)}},sort_keys=True))
end
detach
quit 0
'''
    script_path=logs/"retry.gdb"
    script_path.write_text(script)
    ast.parse(script.split("\npython\n",1)[1].split("\nend\n",1)[0])
    output=(logs/"retry.log").open("w")
    child=subprocess.Popen(["riscv64-elf-gdb","-q","-nx","--batch","-x",str(script_path)],stdout=output,stderr=subprocess.STDOUT)
    return child,output,logs


def verify(logs,status):
    facts=json.loads((logs/"evidence.json").read_text())
    if status or not facts.get("finished") or not facts.get("verified") or not facts.get("caller_record_retired"):
        raise RuntimeError(f"Retry late-query window not established: {logs}")
    return facts
