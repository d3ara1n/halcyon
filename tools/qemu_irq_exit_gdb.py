"""原IrqUse消费者退出：只读核WaitOne停驻、WaitSet成员和最后loan归还。"""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug

MODES = {"irq-wait-exit", "irq-set-exit"}


def start(kernel, port, target_file, controller_pid):
    logs = debug.fresh_logs(Path("artifacts/irq-exit"))
    consumer = Path("artifacts/build/test_kernel").resolve()
    symbols = debug.symbols(kernel)
    entry = debug.symbols(consumer)["test_kernel::irq_checks::process_retirement"]
    command = ["riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel)]
    for path, line in (("process.rs", 364), ("resource/irq/object.rs", 220)):
        command += ["-ex", f"info line kernel/src/task/{path}:{line}"] if path == "process.rs" else ["-ex", f"info line kernel/src/{path}:{line}"]
    code = subprocess.check_output(command, text=True, stderr=subprocess.STDOUT)
    (logs / "code-windows.log").write_text(code)
    points = {line: int(re.search(rf"Line {line} .*starts at address (0x[0-9a-f]+)", code)[1], 16)
              for line in (364, 220)}
    for line, name in ((364, "kernel::task::process::kill"), (220, "kernel::resource::irq::object::query")):
        begin = symbols[name]
        end = min(value for value in symbols.values() if value > begin)
        if not begin <= points[line] < end:
            raise RuntimeError("IRQ exit checkpoint is outside its actual native function")
    evidence = logs / "evidence.json"
    facts = {"kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
             "consumer_sha256": hashlib.sha256(consumer.read_bytes()).hexdigest(),
             "controller_pid": controller_pid,
             "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
             "scope": "original ProcessKill, WaitOne Waiting/source or child-held WaitSet/Line member, retained parent IrqUse and caller Pool; read-only, no timing claim",
             "points": points, "sources": {}, "targets": {}}
    evidence.write_text(json.dumps(facts, indent=2) + "\n")
    script = SCRIPT
    substitutions = {"KERNEL": json.dumps(str(kernel.resolve())), "INIT": json.dumps(str(consumer)), "CONTROLLER": str(controller_pid),
                     "PORT": str(port), "TARGET": json.dumps(str(target_file.resolve())),
                     "EVIDENCE": json.dumps(str(evidence.resolve())), "ENTRY": hex(entry),
                     "KILL": hex(points[364]), "QUERY": hex(points[220]),
                     "SET_NEW": hex(symbols["<kernel::task::wait_set::WaitSet>::new"]),
                     "CONTEXT": debug.user_trap_context(kernel, symbols["handle_user_trap"])}
    for key, value in substitutions.items():
        script = script.replace("__" + key + "__", value)
    script_path = logs / "exit.gdb"
    script_path.write_text(script)
    for body in re.findall(r"\npython\n(.*?)\nend\n", script, re.S):
        ast.parse(body, filename=str(script_path))
    output = (logs / "exit.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)],
                             stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
    return child, output, logs


def verify(logs, status):
    facts = json.loads((logs / "evidence.json").read_text())
    if status or not facts.get("verified") or {v["mode"] for v in facts["targets"].values()} != MODES:
        raise RuntimeError(f"IRQ exit evidence incomplete: {logs}")
    return facts


SCRIPT = r'''set pagination off
set confirm off
set language rust
set tcp auto-retry on
set tcp connect-timeout 10
file __KERNEL__
target remote 127.0.0.1:__PORT__
python
import gdb,json,time
from pathlib import Path
TARGET=Path(__TARGET__)
EVIDENCE=Path(__EVIDENCE__)
facts=json.loads(EVIDENCE.read_text())
failure=None
complete=False
__CONTEXT__
gdb.execute('add-symbol-file '+json.dumps(__INIT__),to_string=True)
def require(ok,message):
    if not ok:raise gdb.GdbError(message)
def variant(value):return next((f.name for f in value.type.fields() if f.name),None)
def optional(value):
    name=variant(value)
    if name=='None':return None
    require(name=='Some','unexpected IRQ exit Option layout')
    inner=value['Some']
    return inner[next(f.name for f in inner.type.fields() if f.name)]
def pointer(value):
    while value.type.strip_typedefs().code!=gdb.TYPE_CODE_PTR:
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('ptr','pointer','data_ptr','__pointer','__0','0','data') if n in names),None)
        if key is None and len(names)==1:key=names[0]
        require(key is not None,'unexpected IRQ exit pointer layout')
        value=value[key]
    return value
def arc_pointer(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value)
def arc(value):return arc_pointer(value).dereference()['data']
def integer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_INT,gdb.TYPE_CODE_BOOL):
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('v','value','__0','0','inner') if n in names),None)
        require(key is not None,'unexpected IRQ exit integer layout')
        value=value[key]
    return int(value)
def save():EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
def failed(stage,error):
    global failure
    failure=str(error)
    frame=gdb.newest_frame()
    facts['failure']={'stage':stage,'error':failure,'function':frame.name(),'pc':hex(int(frame.pc())),
                      'backtrace':gdb.execute('bt',to_string=True)}
    save();print('IRQ_EXIT_OBSERVATION_FAILED '+json.dumps(facts['failure'],sort_keys=True))
    return True
def pid(thread):return int(arc(thread['process'])['pid'])
def source_raw(source):
    # 两类trait Arc均从本轮DWARF核宽度，只读取仍有实际owner的对象。
    require(source.type.sizeof==16,'unexpected trait Arc width')
    address=int(source.address)
    return word(address),word(address+8)
def chain_empty(line):
    chain=line['queue']['chain']['value']
    boundary=int(chain['boundary'].address)
    return int(chain['head']) in (0,boundary) and int(chain['tail']) in (0,boundary)
def line_state(line_arc):
    inner=arc_pointer(line_arc).dereference()
    line=inner['data']
    dispatch=line['queue']['record']['inner']['value']['state']['state']
    return {'inner':int(inner.address),'address':int(line.address),'source':int(line['source']),
            'generation':int(line['state']['value']['generation']),
            'strong':integer(inner['strong']),'work':variant(dispatch),'empty':chain_empty(line)}
def members(table):
    stack=[table['root']]
    seen=0
    while stack:
        owned=optional(stack.pop())
        if owned is None:continue
        node=pointer(owned['data']).dereference()
        yield node['value']
        stack.extend([node['left'],node['right']])
        seen+=1
        require(seen<=int(table['len']),'WaitSet traversal exceeded actual members')
    require(seen==int(table['len']),'WaitSet traversal omitted members')
def field_type(kind,name):
    return next(field.type for field in kind.fields() if field.name==name)
# 具体对象类型由当前ELF的KernelObject vtable确定，不执行guest方法。
set_block=gdb.block_for_pc(__SET_NEW__)
while set_block is not None and not set_block.is_static:set_block=set_block.superblock
require(set_block is not None,'WaitSet native static scope missing')
set_vtables=[symbol for symbol in set_block if symbol.is_variable and
             symbol.name=='<kernel::task::wait_set::WaitSet as kernel::task::object::KernelObject>::{vtable}']
require(len(set_vtables)==1,'WaitSet KernelObject vtable is not unique in this ELF')
set_vtable=int(set_vtables[0].value().address)
set_inner_type=gdb.lookup_type('kernel::task::wait_set::WaitSet')
for name in ('this','data','value','ptr','pointer'):set_inner_type=field_type(set_inner_type,name)
set_inner_type=set_inner_type.target()
cap_slot_type=gdb.lookup_type('capability::table::Slot<kernel::task::cap::ObjectAuthority>')
facts['wait_set_layout']={'vtable':set_vtable,'arc_inner_type':str(set_inner_type),
                          'cap_slot_type':str(cap_slot_type),'runtime_objects_read':False}
save()
def child_wait_set(body,before):
    table=body['handles']['data']['value']
    storage=table['slots']
    length=int(storage['len'])
    require(not bool(table['closed']) and 0<length<=int(table['limit'])+1,
            'IRQ child capability table is closed or has invalid initialized length')
    data=optional(storage['data'])
    require(data is not None,'IRQ child capability storage missing')
    slots=pointer(data).cast(cap_slot_type.pointer())
    candidates={}
    scan={'initialized_slots':length,'occupied':int(table['occupied']),'sets':[]}
    facts['kill_read']['cap_scan']=scan;save()
    for index in range(length):
        slot=slots[index]
        state=slot['state']
        if variant(state)!='Occupied':continue
        occupied=state['Occupied']
        cap=occupied[next(field.name for field in occupied.type.fields() if field.name)]
        authority=optional(cap['object'])
        require(authority is not None,'occupied child cap has no authority')
        raw,vtable=source_raw(authority['__0'])
        if vtable!=set_vtable:continue
        alias={'slot':index,'generation':int(slot['generation']),'rights':integer(cap['rights'])}
        if raw in candidates:
            candidates[raw]['aliases'].append(alias)
            continue
        set_=gdb.Value(raw).cast(set_inner_type.pointer()).dereference()['data']
        registrations=list(members(set_['state']['data']['value']['members']))
        record={'inner':raw,'address':int(set_.address),'koid':integer(set_['header']['koid']),
                'aliases':[alias],'member_count':len(registrations),'members':[]}
        candidates[raw]=record;scan['sets'].append(record);save()
        for registration in registrations:
            member=pointer(registration['member']).dereference()
            source,vtable=source_raw(member['source'])
            record['members'].append({'address':int(member.address),'node':int(member['node'].address),
                                      'attached':bool(member['attached']),'source':source,'source_vtable':vtable,
                                      'chain':int(member['node']['links']['value']['owner'])})
        save()
    matches=[record for record in candidates.values() if record['member_count']==1 and
             record['members'][0]['attached'] and record['members'][0]['source']==before['inner']]
    require(len(matches)==1,'child-held WaitSet with the original Line member is not unique')
    return matches[0]
def waiting_thread(member,target_pid):
    waiting=member['state']['Waiting']
    admitted=waiting[next(field.name for field in waiting.type.fields() if field.name)]
    thread=arc(admitted['value'])
    require(pid(thread)==target_pid,'Waiting owner belongs to another process')
    facts['kill_read'].update(stage='Thread.wait_entry/source',thread=int(thread.address),
                             frame=int(thread['frame'].address),satp=int(thread['satp']));save()
    entry=optional(thread['wait_entry']['value'])
    require(entry is not None and bool(entry['source_attached']) and
            not bool(entry['timer_prepared']) and optional(entry['expires_at']) is None,
            'IRQ waiting source/timer state changed')
    require(int(entry['epoch'])==int(member['epoch']) and bool(member['active_wait']),
            'IRQ waiting epoch is not active')
    source=optional(entry['source'])
    require(source is not None,'IRQ Waiting owner has no source loan')
    raw,vtable=source_raw(source)
    return thread,{'thread':int(thread.address),'frame':int(thread['frame'].address),
                   'satp':int(thread['satp']),'epoch':int(member['epoch']),
                   'waiting_source':raw,'waiting_vtable':vtable}
class Entry(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            # 用户态sscratch由正式返回路径持有HartLocal，不使用用户tp。
            anchor=int(gdb.parse_and_eval('$sscratch'))&((1<<64)-1)
            current=word(anchor+hart_offsets['current_thread'])
            if not current:return False
            shell=word(current+process_offset+ptr_offset+pointer_offset)
            if word(shell+data_offset+pid_offset)!=__CONTROLLER__:return False
            self.enabled=False
            query.enabled=True
            killing.enabled=True
            print('IRQ_EXIT_NATIVE_CONSUMER_ENTERED')
            return False
        except Exception as error:
            return failed('native entry identity',error)
class Query(gdb.Breakpoint):
    def stop(self):
        global failure,complete
        try:
            frame=gdb.newest_frame()
            if pid(frame.read_var('thread').dereference())!=__CONTROLLER__:return False
            node=arc(frame.read_var('node'))
            line_arc=optional(node['line'])
            if line_arc is None:return False
            current=line_state(line_arc)
            facts['query_read']=current;save()
            key=str(current['generation'])
            sources=facts['sources']
            if key not in sources:
                require(current['empty'] and current['work']=='Idle','initial Line has pending observation work')
                # 尚未交子观察：实际强根只有Node字段和Controller源槽，cap别名不复制Line。
                require(current['strong']==2,'initial Line has an unexpected strong owner')
                sources[key]={'before':current}
            else:
                before=sources[key]['before']
                # take封装也会Query generation；只有原Kill已取证后才解释为退出后取样。
                if not any(v['generation']==current['generation'] for v in facts['targets'].values()):
                    return False
                require('after' not in sources[key],'retained Line was sampled more than twice')
                require(current['inner']==before['inner'] and current['source']==before['source'],'retained Line identity changed')
                require(current['strong']==before['strong'] and current['work']=='Idle' and current['empty'],'Line member or source loan remains after child refund')
                sources[key]['after']=current
                complete=len([v for v in sources.values() if 'after' in v])==2
            save()
            print('IRQ_EXIT_LINE_SOURCE '+json.dumps(sources[key],sort_keys=True))
            return complete
        except Exception as error:
            return failed('retained Line query',error)
class Killing(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            frame=gdb.newest_frame()
            if pid(frame.read_var('caller').dereference())!=__CONTROLLER__:return False
            shell=arc(frame.read_var('shell'))
            target_pid=int(shell['pid'])
            facts['kill_read']={'pid':target_pid,'shell':int(shell.address),'stage':'admission metadata/body'};save()
            end=time.monotonic()+2
            metadata=None
            while time.monotonic()<end:
                if TARGET.exists():metadata=json.loads(TARGET.read_text()).get(str(target_pid))
                if metadata is not None:break
                time.sleep(.01)
            require(metadata is not None,'native target admission metadata missing at Kill')
            body=optional(shell['state']['data']['value']['body'])
            require(body is not None,'IRQ target has no live Body before Kill')
            body=pointer(body['data']).dereference()
            slots=body['members']['inner']['data']['value']['slots']
            require(int(slots['len'])==1,'IRQ exit target has unexpected thread slots')
            storage=optional(slots['data'])
            require(storage is not None,'IRQ target lacks member storage')
            slot_type=gdb.lookup_type('kernel::task::process::members::Slot')
            slot=pointer(storage).cast(slot_type.pointer()).dereference()
            member=optional(slot['entry'])
            state=None if member is None else variant(member['state'])
            facts['kill_read'].update(stage='actual member/child capabilities',member_state=state);save()
            require(member is not None,'IRQ target has no actual member before Kill')
            generation=str(metadata['generation'])
            before=facts['sources'][generation]['before']
            require(before['source']==metadata['source'],'IRQ grant source mismatch')
            observed={**metadata,'tid':int(member['tid']),'member_state':state,
                      'epoch':int(member['epoch']),'waiting':state=='Waiting'}
            if metadata['mode']=='irq-wait-exit':
                require(state=='Waiting','WaitOne target was not actually parked before Kill')
                thread,waiting=waiting_thread(member,target_pid)
                observed.update(waiting)
                require(waiting['waiting_source']==before['inner'],'WaitOne source is not the original Line')
                node=thread['wait_node']
            else:
                require(metadata['mode']=='irq-set-exit','unexpected IRQ exit target mode')
                record=child_wait_set(body,before)
                observed['wait_set']=record
                set_=gdb.Value(record['inner']).cast(set_inner_type.pointer()).dereference()['data']
                attached=next(members(set_['state']['data']['value']['members']))
                node=pointer(attached['member']).dereference()['node']
                if state=='Waiting':
                    thread,waiting=waiting_thread(member,target_pid)
                    observed.update(waiting)
                    require(waiting['waiting_source']==record['inner'],'host Waiting source is not its child-held WaitSet')
                    require(int(thread['wait_node']['links']['value']['owner'])==
                            int(set_['source']['chain']['value'].address),
                            'host thread is not linked to its actual WaitSet')
            node_address=int(node.address)
            line_type=gdb.lookup_type('kernel::resource::irq::controller::Line')
            line=gdb.Value(before['address']).cast(line_type.pointer()).dereference()
            chain=int(line['queue']['chain']['value'].address)
            facts['kill_read'].update(stage='original Line chain',node=node_address,line_chain=chain);save()
            require(int(node['links']['value']['owner'])==chain,'original Line node is not actually linked')
            observed.update(line_node=node_address,line_chain=chain)
            facts['targets'][str(target_pid)]=observed
            save()
            print('IRQ_EXIT_MEMBER_BEFORE_KILL '+json.dumps(observed,sort_keys=True))
            return False
        except Exception as error:
            return failed('Kill member/capability/source/Line chain',error)
query=Query('*__QUERY__',type=gdb.BP_HARDWARE_BREAKPOINT)
query.enabled=False
killing=Killing('*__KILL__',type=gdb.BP_HARDWARE_BREAKPOINT)
killing.enabled=False
Entry('*__ENTRY__',type=gdb.BP_HARDWARE_BREAKPOINT)
end
continue
python
require(failure is None,failure or 'IRQ exit probe failed')
require(complete and len(facts['targets'])==2,'incomplete IRQ exit snapshots')
facts['verified']=True
save()
print('IRQ_EXIT_VERIFIED WaitOne Waiting, child-held WaitSet membership and original Line loans returned')
end
detach
quit 0
'''
