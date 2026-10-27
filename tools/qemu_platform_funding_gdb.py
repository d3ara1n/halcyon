"""Ready前真实平台根构造/整批安装的指定分配失败；复用boot失败运行体。"""
import ast
import hashlib
import json
from pathlib import Path
import re
import subprocess


def program(mode, kernel, addresses, port, logs, boot_package):
    construction = mode == "platform-node"
    retained = mode == "platform-dma-retained"
    source, line = (("kernel/src/resource/mmio/node.rs", 120) if construction else
                    (("kernel/src/resource/dma/mod.rs", 197) if retained else ("kernel/src/boot/bootstrap.rs", 271)))
    error_line = 214 if construction else (266 if retained else 278)
    allocator = addresses["__rustc::__rust_alloc"]
    code = subprocess.check_output([
        "riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel), "-ex", "set language c",
        "-ex", f"info line {source}:{line}",
        "-ex", f"info line kernel/src/boot/bootstrap.rs:{error_line}",
        "-ex", f"disassemble /r {allocator:#x},{allocator+52:#x}",
    ], text=True, stderr=subprocess.STDOUT)
    (logs / f"{mode}-code.log").write_text(code)
    point = int(re.search(rf"Line {line} .*starts at address (0x[0-9a-f]+)", code)[1], 16)
    error_point = int(re.search(rf"Line {error_line} .*starts at address (0x[0-9a-f]+)", code)[1], 16)
    if not re.search(rf"{allocator:#x}[^\n]*addi\s+sp,sp,-32", code):
        raise RuntimeError("platform allocator does not match the verified unexecuted prologue")
    evidence = logs / f"{mode}-evidence.json"
    facts = {"mode":mode, "kernel_sha256":hashlib.sha256(kernel.read_bytes()).hexdigest(),
             "boot_package_sha256":hashlib.sha256(boot_package.read_bytes()).hexdigest(),
             "checkpoint":hex(point), "error_checkpoint":hex(error_point), "verified":False}
    evidence.write_text(json.dumps(facts,indent=2)+"\n")
    script = f'''set pagination off
set confirm off
set language rust
set tcp auto-retry on
set tcp connect-timeout 10
file "{kernel}"
target remote 127.0.0.1:{port}
python
import gdb,json
from pathlib import Path
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
construction={construction!r}
retained={retained!r}
parked=set();nodes={{}};baseline=None;injected=False;observed=False
pool=None
retained_root=None

def require(test,message):
    if not test:
        facts['failure']={{'message':message,'stage':facts.get('observation_stage')}}
        save()
        raise gdb.GdbError(message)
def variant(value):return next((f.name for f in value.type.fields() if f.name),None)
def optional(value):
    tag=variant(value)
    if tag=='None':return None
    require(tag=='Some','unexpected Option debug layout: '+str(value.type))
    inner=value['Some'];return inner[next(f.name for f in inner.type.fields() if f.name)]
def pointer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_PTR,gdb.TYPE_CODE_INT):
        names=[f.name for f in value.type.fields() if f.name]
        key=next((n for n in ('ptr','pointer','__0','0') if n in names),None)
        require(key is not None,'unexpected pointer wrapper: '+str(value.type))
        value=value[key]
    return value
def arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['data']
def stack():
    result=[];frame=gdb.newest_frame()
    while frame:result.append(frame);frame=frame.older()
    return result
def in_source(frame,path,function):
    # 当前SAL可指向内联core函数；用实际命名帧的定义scope核源码归属。
    if function not in (frame.name() or ''):return False
    block=frame.block()
    while block is not None:
        symbol=block.function
        if symbol is not None and function in symbol.name and symbol.symtab is not None and path in symbol.symtab.fullname():return True
        block=block.superblock
    return False
def snapshot():
    inner=pool['inner']['data']['value']
    require(variant(inner)=='Active','platform funding Pool is not Active')
    ledgers=optional(inner['Active']['state'])
    require(ledgers is not None,'Pool ledger is transiently outside its owner')
    return {{kind:{{field:int(ledgers[kind][field]) for field in ('available','allocated','reserved','delegated')}} for kind in ('pages','metadata')}}
def body_table(frame):
    return pointer(frame.read_var('body')['body']).dereference()['handles']['data']['value']
def save():EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
DMA_ROOT='kernel::resource::dma::Root'
NODE_TYPES=(('resource/mmio/node.rs','kernel::resource::mmio::node::MmioNode'),
            ('resource/irq/mod.rs','kernel::resource::irq::Node'),
            ('resource/dma/mod.rs','kernel::resource::dma::Node'))
def declared_field(kind,path):
    offset=0
    for name in path.split('.'):
        field=next(field for field in kind.fields() if field.name==name)
        require(field.bitpos%8==0,'observed field is not byte aligned: '+path)
        offset+=field.bitpos//8;kind=field.type
    return offset,kind
def fresh(address,name):
    # 只复用ELF声明，不跨停点保存Value.type或带旧enum状态的Value。
    return gdb.Value(address).cast(gdb.lookup_type(name).pointer()).dereference()
def field_bytes(address,kind,path):
    offset,field=declared_field(kind,path)
    raw=bytes(gdb.selected_inferior().read_memory(address+offset,field.sizeof))
    return {{'address':hex(address+offset),'type':str(field),'offset':offset,
             'size':field.sizeof,'raw':raw.hex()}}
def fresh_field(address,kind,path):
    offset,field=declared_field(kind,path)
    return gdb.Value(address+offset).cast(field.pointer()).dereference()
def recorded_field(record,address,kind,path,decode):
    # 原字节先落盘，解释失败也不丢该次实际值。
    facts['observation_stage']=record['stage']+': '+path
    item=field_bytes(address,kind,path);record['fields'][path]=item;save()
    item['canonical']=decode(fresh_field(address,kind,path));save()
    return item['canonical']
def optional_id(value):
    value=optional(value)
    return None if value is None else int(pointer(value))
def lock_snapshot(address,kind):
    return {{name:int.from_bytes(bytes(gdb.selected_inferior().read_memory(
                address+declared_field(kind,'raw.'+name)[0],4)),'little')
             for name in ('next','serving')}}
queue_symbol=gdb.lookup_static_symbol('kernel::maintenance::QUEUE')
require(queue_symbol is not None,'original maintenance queue declaration is missing')
queue_type=queue_symbol.type
queue_address=int(queue_symbol.value().address)
layouts={{}}
for name in (DMA_ROOT,)+tuple(name for _,name in NODE_TYPES):
    kind=gdb.lookup_type(name)
    paths=['record.inner.value.state.state','id'] if name!=DMA_ROOT else [
        'state.data.value.tree.nodes.len','state.data.value.mappings.len',
        'state.data.value.reclaim_head','state.data.value.reclaim_tail',
        'state.raw.next','state.raw.serving','record.inner.value.state.state']
    source=('resource/dma/mod.rs' if name==DMA_ROOT else next(path for path,node in NODE_TYPES if node==name))
    layouts[name]={{'source':source,'objfile':kind.objfile.filename,'size':kind.sizeof,'fields':{{path:{{'offset':declared_field(kind,path)[0],
        'type':str(declared_field(kind,path)[1]),'size':declared_field(kind,path)[1].sizeof}}
        for path in paths}}}}
facts['observation_declarations']={{'types':layouts,'maintenance_queue':{{'address':hex(queue_address),
    'source':'maintenance.rs','objfile':queue_type.objfile.filename,'type':str(queue_type),'size':queue_type.sizeof,'lock_offsets':{{
        name:declared_field(queue_type,'raw.'+name)[0] for name in ('next','serving')}}}}}}
save()
def work_snapshot(address,name,stage):
    record={{'stage':stage,'address':hex(address),'type':name,'fields':{{}}}}
    facts.setdefault('work_observations',[]).append(record);save()
    kind=gdb.lookup_type(name)
    try:
        record['maintenance_lock']=lock_snapshot(queue_address,queue_type);save()
        if name!=DMA_ROOT:record['node_id']=recorded_field(record,address,kind,'id',lambda value:int(pointer(value)))
        record['state']=recorded_field(record,address,kind,'record.inner.value.state.state',variant)
    except Exception as error:
        record['read_error']={{'stage':facts.get('observation_stage'),'message':str(error)}}
        facts['failure']=record['read_error'];save()
        raise
    return record
def root_snapshot(address,stage):
    record={{'stage':stage,'address':hex(address),'type':DMA_ROOT,'node_id':facts['node_id'],
             'thread':gdb.selected_thread().num,'pc':hex(int(gdb.parse_and_eval('$pc'))),
             'fields':{{}}}}
    facts.setdefault('root_observations',[]).append(record);save()
    kind=gdb.lookup_type(DMA_ROOT)
    try:
        fields=(
                ('state.data.value.tree.nodes.len',int),('state.data.value.mappings.len',int),
                ('state.data.value.reclaim_head',optional_id),('state.data.value.reclaim_tail',optional_id),
                ('record.inner.value.state.state',variant))
        for path,_ in fields:
            facts['observation_stage']=stage+': raw '+path
            record['fields'][path]=field_bytes(address,kind,path);save()
        # 票号只表示门是否被持有/等待，不据此猜测hart身份。
        state_offset,state_type=declared_field(kind,'state')
        record['root_lock']=lock_snapshot(address+state_offset,state_type)
        record['maintenance_lock']=lock_snapshot(queue_address,queue_type);save()
        for path,decode in fields:
            facts['observation_stage']=stage+': canonical '+path
            record['fields'][path]['canonical']=decode(fresh_field(address,kind,path));save()
        root=fresh(address,DMA_ROOT)
        entry=optional(root['state']['data']['value']['tree']['nodes']['root'])
        record['member']=None
        if entry is not None:
            member=pointer(entry['data']).dereference()['value']
            record['member']={{'sealed':bool(member['sealed']),'finished':bool(member['finished']),
                              'dropped':bool(member['value']['dropped'])}}
        record['backtrace']=gdb.execute('bt',to_string=True);save()
    except Exception as error:
        record['read_error']={{'stage':facts.get('observation_stage'),'message':str(error)}}
        facts['failure']=record['read_error'];save()
        raise
    return record
def scope_description(block):
    result=[]
    while block is not None:
        symbol=block.function
        if symbol is not None:
            result.append({{'start':hex(block.start),'end':hex(block.end),'function':symbol.name,
                            'source':None if symbol.symtab is None else symbol.symtab.fullname()}})
        block=block.superblock
    return result
retained_layout=None
if retained:
    # 从实际领域类型的retained字段推导布局，祖先筛选失败也保留明确的期待值。
    kind=gdb.lookup_type('kernel::resource::dma::Prepared')
    for name in ('retained','Some','__0','__0','data'):
        kind=next(field.type for field in kind.fields() if field.name==name)
    while kind.strip_typedefs().code!=gdb.TYPE_CODE_PTR:
        fields={{field.name:field.type for field in kind.fields() if field.name}}
        key=next((name for name in ('ptr','pointer','__0','0') if name in fields),None)
        require(key is not None,'retained entry Box pointer type changed')
        kind=fields[key]
    kind=kind.target()
    retained_layout={{'type':str(kind),'size':kind.sizeof,'align':kind.alignof}}
    facts['retained_target_layout']=retained_layout;save()
def value_declaration(block):
    while block is not None:
        function=block.function
        if (function is not None and '::try_new' in function.name and function.symtab is not None
                and 'funded_storage/src/lib.rs' in function.symtab.fullname()):
            declarations=[symbol for symbol in block if symbol.is_argument and symbol.name=='value']
            require(len(declarations)==1,'OwnedBox value declaration is not unique in its actual scope')
            return declarations[0],{{'start':hex(block.start),'end':hex(block.end),'function':function.name,
                                     'source':function.symtab.fullname()}}
        block=block.superblock
    raise gdb.GdbError('actual OwnedBox value declaration scope is missing')
def retained_error_declaration(block):
    while block is not None:
        declarations=[symbol for symbol in block if symbol.is_variable and symbol.name=='error']
        if declarations:
            require(len(declarations)==1,'bootstrap error declaration is not unique')
            declaration=declarations[0]
            require(str(declaration.type)=='erhino_shared::call::SystemCallError','unexpected bootstrap error type')
            owner=block
            while owner is not None and owner.function is None:owner=owner.superblock
            require(owner is not None and owner.function.symtab is not None and
                    'publish' in owner.function.name and 'boot/bootstrap.rs' in owner.function.symtab.fullname(),
                    'error declaration is outside the original bootstrap owner')
            return declaration,{{'start':hex(block.start),'end':hex(block.end),'symbol':declaration.name,
                                  'type':str(declaration.type),'line':declaration.line,
                                  'function':owner.function.name,'source':owner.function.symtab.fullname()}}
        block=block.superblock
    raise gdb.GdbError('bootstrap error declaration is missing at the actual checkpoint')
def retained_candidate(frames,gate):
    # 仅诊断本次Root的至多四个候选；筛选仍用原真实frame.block，不以pc-1猜owner。
    checks=facts.setdefault('candidate_checks',[])
    require(len(checks)<4,'DMA root candidate diagnostic limit reached without injection')
    check={{'stage':'raw/definition scopes','gate':gate,
            'raw':{{'pc':hex(int(gdb.parse_and_eval('$pc'))&((1<<64)-1)),
                    'size':int(gdb.parse_and_eval('$a0')),'align':int(gdb.parse_and_eval('$a1'))}},
            'expected':retained_layout,'value_layout':None,'declared_layout':None,
            'payer':{{'stage':'not-reached','expected_pool_address':hex(int(pool.address)),
                      'expected_pool_koid':facts['pool'],'matched':None}},
            'rejection':None,'stack':[]}}
    checks.append(check);save()
    try:
        for frame in frames:
            pc=int(frame.pc())
            item={{'name':frame.name(),'pc':hex(pc)}};check['stack'].append(item)
            check['stage']='original frame predicates';check['scope_frame_pc']=hex(pc)
            definitions=(('root','resource/dma/mod.rs','::root'),
                         ('owned_box','funded_storage/src/lib.rs','::try_new'),
                         ('bootstrap','boot/bootstrap.rs','publish'))
            # 必要筛选先严格完成；同一结果供诊断和后续选择复用。
            item['matches']={{role:in_source(frame,path,function) for role,path,function in definitions}}
            item['scope_required']=any(function in (item['name'] or '') for _,_,function in definitions)
            check['stage']='frame.block definition scope'
            sal=frame.find_sal()
            item['source']=None if sal.symtab is None else sal.symtab.fullname()
            try:
                item['frame_scope']=scope_description(frame.block())
            except RuntimeError as error:
                if item['scope_required'] or str(error)!='Cannot locate block for frame.':raise
                item['frame_scope']=None;item['scope_unavailable']=str(error)
            check['stage']='pc definition scope'
            block=gdb.block_for_pc(pc)
            item['pc_scope']=None if block is None else scope_description(block)
            check['stage']='pc-1 definition scope'
            block=gdb.block_for_pc(pc-1)
            item['pc_minus_one_scope']=None if block is None else scope_description(block)
        save()
        check['stage']='original Root ancestor'
        if gate!=0 or not any(item['matches']['root'] for item in check['stack']):
            check['rejection']='not-before-Ready' if gate!=0 else 'original-Root-definition-scope-missing'
            save();raise gdb.GdbError(check['rejection'])
        check['stage']='OwnedBox definition scope'
        candidate=next((frame for frame,item in zip(frames,check['stack']) if item['matches']['owned_box']),None)
        if candidate is None:
            check['rejection']='OwnedBox-definition-scope-missing';save();return None
        check['candidate_pc']=hex(int(candidate.pc()))
        check['stage']='runtime value Layout diagnostic'
        runtime_type=candidate.read_var('value').type
        check['value_layout']={{'type':str(runtime_type),'size':runtime_type.sizeof,'align':runtime_type.alignof}}
        check['stage']='value declaration Layout'
        declaration,definition=value_declaration(candidate.block())
        expected=declaration.type
        check['declared_layout']={{'type':str(expected),'size':expected.sizeof,'align':expected.alignof}}
        check['declared_scope']=definition
        # 分配布局来自实际词法声明；Value.type的动态形态只用于诊断。
        if check['declared_layout']!=retained_layout:
            check['rejection']='value-declaration-domain-Layout-mismatch';save();return None
        save()
        check['stage']='MetadataFunding payer'
        argument=candidate.read_var('payer')
        check['payer'].update(stage='argument read',argument_type=str(argument.type))
        save()
        payer=argument.dereference()
        actual=arc(payer['__0'])
        check['payer'].update(argument_address=hex(int(argument)),actual_pool_address=hex(int(actual.address)),
                              actual_pool_koid=int(actual['header']['koid']),
                              matched=int(actual.address)==int(pool.address),stage='compared')
        if not check['payer']['matched']:
            check['rejection']='MetadataFunding-payer-mismatch';save();raise gdb.GdbError(check['rejection'])
        check['stage']='raw allocator Layout'
        if check['raw']['size']!=expected.sizeof or check['raw']['align']!=expected.alignof:
            check['rejection']='allocator-Layout-mismatch';save();return None
        check['stage']='bootstrap ancestor'
        if not any(item['matches']['bootstrap'] for item in check['stack']):
            check['rejection']='bootstrap-definition-scope-missing';save();raise gdb.GdbError(check['rejection'])
        check['stage']='selected';check['selected']=True;save()
        return candidate,expected
    except Exception as error:
        if check['rejection'] is None:check['rejection']='observation-error'
        check['error']={{'stage':check['stage'],'message':str(error)}}
        save();raise
def reclaim(address,stage):
    record=root_snapshot(address,stage)
    fields=record['fields']
    require(fields['state.data.value.tree.nodes.len']['canonical']==1 and
            fields['state.data.value.mappings.len']['canonical']==0,'retained-entry root has unexpected responsibility')
    require(fields['state.data.value.reclaim_head']['canonical']==facts['node_id'],'original root reclaim does not contain the dropped Node')
    member=record['member']
    require(member is not None,'original root tree lost its unfinished storage')
    require(member['sealed'] and member['finished'] and member['dropped'],'last Node Drop did not finish the original tree member')
    require(fields['record.inner.value.state.state']['canonical']=='Queued','original Root Work is not retained in the maintenance queue')
checkpoint=gdb.Breakpoint('*{point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
allocation=gdb.Breakpoint('*{allocator:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
error=gdb.Breakpoint('*{error_point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
kick=gdb.Breakpoint('*{addresses['kernel::maintenance::kick']:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
parking=gdb.Breakpoint('*{addresses['kernel::hart::park']:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
root_return=None;root_return_pc=None
allocation.enabled=False;error.enabled=False;kick.enabled=False;parking.enabled=False
for breakpoint in (checkpoint,allocation,error,kick,parking):breakpoint.silent=True
while len(parked)<4:
    gdb.execute('continue',to_string=True)
    frame=gdb.newest_frame();frames=stack();pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
    gate=int(gdb.Value({addresses['kernel::registry::GATE']}).cast(gdb.lookup_type('u8').pointer()).dereference())
    if root_return is not None and pc==root_return_pc:
        facts['root_returned']=True;save();root_return.enabled=False
        require(injected,'DMA root returned without the selected allocation; candidate checks retained')
        continue
    if pc=={point}:
        require(gate==0,'platform funding checkpoint is not before Ready')
        if construction or retained:
            pool=arc(frame.read_var('pool'))
            if construction:
                facts['range']={{'physical':int(frame.read_var('physical')),'bytes':int(frame.read_var('bytes'))}}
            else:
                retained_root=int(arc(frame.read_var('root')).address)
                facts['root']=hex(retained_root);facts['node_id']=int(pointer(frame.read_var('id')))
                root_snapshot(retained_root,'before retained allocation')
                boot=next(f for f in frames if in_source(f,'boot/bootstrap.rs','publish'))
                facts['cap_count']=int(boot.read_var('caps')['len'])
                require(in_source(frame,'resource/dma/mod.rs','::root'),'checkpoint is not the actual DMA root frame')
                caller=frame.older()
                require(caller is not None,'DMA root has no real return owner')
                root_return_pc=int(caller.pc())
                root_return=gdb.Breakpoint('*%#x'%root_return_pc,type=gdb.BP_HARDWARE_BREAKPOINT)
                root_return.silent=True
                facts['root_return_pc']=hex(root_return_pc)
        else:
            shell=arc(frame.read_var('shell'))
            require(int(shell['pid'])==1,'Building table is not the real initial process')
            pool=arc(shell['_metadata']['pool'])
            table=body_table(frame)
            require(int(table['occupied'])==0,'initial table already published capabilities')
            facts['cap_count']=int(frame.read_var('caps')['len'])
            require(facts['cap_count']>6,'batch lacks actual platform root capabilities')
            facts['table_occupied']=0
        facts['pool']=int(pool['header']['koid']);baseline=snapshot();facts['before']=baseline
        print('PLATFORM_FUNDING_BEGIN '+json.dumps(facts,sort_keys=True));save()
        checkpoint.enabled=False;allocation.enabled=True
        continue
    if pc=={allocator}:
        if retained:
            selected=retained_candidate(frames,gate)
            if selected is None:continue
            candidate,expected=selected
        else:
            candidate=(next((f for f in frames if in_source(f,'resource/mmio/node.rs','::new')),None) if construction else
                       next((f for f in frames if 'funded_storage::Buffer<capability::table::LastCapability<' in (f.name() or '')),None))
        if candidate is None:continue
        if construction:
            expected=gdb.lookup_type('alloc::rcs::arc::ArcInner<kernel::resource::mmio::node::MmioNode>')
        elif not retained:
            require(any(in_source(f,'capability/src/table.rs','::reserve') for f in frames),'batch allocator not in real table reserve')
            expected=gdb.lookup_type('capability::table::LastCapability<kernel::task::cap::ObjectAuthority>')
        size=int(gdb.parse_and_eval('$a0'));align=int(gdb.parse_and_eval('$a1'))
        count=1 if construction or retained else facts['cap_count']
        if size!=expected.sizeof*count or align!=expected.alignof:continue
        require(any(in_source(f,'boot/bootstrap.rs','publish') for f in frames),'allocation not under original bootstrap')
        if not construction and not retained:
            payer=candidate.read_var('payer').dereference()
            require(int(arc(payer['__0']).address)==int(pool.address),'batch buffer charged a different Pool')
        facts['allocation']={{'pc':hex(pc),'size':size,'align':align,'stack':[f.name() for f in frames],
                              'return_pc':hex(int(gdb.parse_and_eval('$ra'))&((1<<64)-1)), 'ledger':snapshot()}}
        print('PLATFORM_FUNDING_ALLOCATION '+json.dumps(facts['allocation'],sort_keys=True));save()
        allocation.enabled=False;error.enabled=True
        kick.enabled=retained
        gdb.execute('set $a0 = 0');gdb.execute('set $pc = $ra')
        injected=True;facts['injected']=True
        continue
    if pc=={error_point}:
        require(injected,'platform failed before the specified allocation')
        if retained:
            facts['error_observation']={{'stage':'exact declaration lookup','pc':hex(pc),'function':frame.name(),
                                        'backtrace':gdb.execute('bt',to_string=True)}};save()
            declaration,definition=retained_error_declaration(frame.block())
            facts['error_observation'].update(stage='symbol value read',definition=definition);save()
            actual=str(declaration.value(frame))
            facts['error_observation'].update(stage='post-error ledger',value=actual);save()
        else:actual=str(frame.read_var('error'))
        require(actual.endswith('OutOfMemory') if construction or retained else actual.endswith('AllocationFailed'),'unexpected actual platform error: '+actual)
        current=snapshot()
        if not retained:require(current==baseline,'local construction/reservation charge did not return to its original owner')
        if not construction and not retained:require(int(body_table(frame)['occupied'])==0,'batch failure installed a capability')
        facts['actual_error']=actual;facts['after']=current
        if retained:facts['error_observation']['stage']='recorded'
        print('PLATFORM_FUNDING_RETURN '+json.dumps({{'error':actual,'ledger':current}},sort_keys=True));save()
        error.enabled=False;kick.enabled=not construction;parking.enabled=True;observed=True
        continue
    if kick.enabled and pc=={addresses['kernel::maintenance::kick']}:
        for caller in frames:
            if retained and in_source(caller,'resource/dma/mod.rs','::drop'):
                node=fresh(int(caller.read_var('self')),'kernel::resource::dma::Node')
                address=int(arc(node['root']).address)
                facts['last_drop']={{'node_address':hex(int(node.address)),'node_id':int(pointer(node['id'])),
                    'root_address':hex(address),'expected_root':hex(retained_root),'expected_node_id':facts['node_id']}}
                facts['observation_stage']='last Node Drop identity';save()
                require(address==retained_root,'last Node Drop published a different Root')
                require(facts['last_drop']['node_id']==facts['node_id'],'last Node Drop has a different NodeId')
                reclaim(address,'last Node Drop')
                nodes[address]=DMA_ROOT
                facts['root_drop_observed']=True
                print('PLATFORM_DMA_LAST_NODE_DROP original Root Work queued with reclaim member')
                break
            kind=next((name for path,name in NODE_TYPES if in_source(caller,path,'::notify')),None)
            if kind is not None:
                address=int(arc(caller.read_var('self')).address)
                record=work_snapshot(address,kind,'root notify')
                require(record['state']=='Queued','root notification did not retain its original queued Work')
                nodes[address]=kind
                break
        continue
    if pc=={addresses['kernel::hart::park']}:
        number=gdb.selected_thread().num
        facts.setdefault('park_observations',[]).append({{'thread':number,'gate':gate,
            'error_observed':observed,'pc':hex(pc),'previously_parked':number in parked}})
        facts['observation_stage']='park admission';save()
        require(observed and gate==2,'park did not follow actual funding error and Gate Failed')
        require(number not in parked,'one parked hart reentered the terminal path')
        parked.add(number);print('PARKED thread=%d gate=2'%number)
        facts['parked']=sorted(parked);save()
        if not construction:
            require(len(nodes)==facts['cap_count']-6+(1 if retained else 0),'not all real platform root cleanup owners were observed')
            if retained:
                require(facts.get('root_drop_observed'),'specific retained-entry last Node Drop was not observed')
                reclaim(retained_root,'park thread '+str(number))
            facts['queued_roots']=[]
            for address,kind in nodes.items():
                record=work_snapshot(address,kind,'park thread '+str(number))
                facts['queued_roots'].append({{'address':hex(address),'type':kind,'state':record['state']}});save()
                require(record['state']=='Queued','terminal lost original queued root responsibility')
        save()
facts['verified']=True;save()
print('PLATFORM_FUNDING_VERIFIED '+json.dumps(facts,sort_keys=True))
end
detach
quit 0
'''
    ast.parse(script.split("\npython\n",1)[1].split("\nend\n",1)[0])
    return script


def verify(logs, mode):
    facts=json.loads((logs/f"{mode}-evidence.json").read_text())
    if not facts.get("verified") or not facts.get("injected") or len(facts.get("parked",[]))!=4:
        raise RuntimeError(f"platform funding evidence incomplete: {logs}/{mode}")
