"""真实Pin完整取得use/write后的安装分配失败；只跳过尚未执行的一次allocator调用。"""
import ast
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess

import qemu_gdb as debug


def start(kernel, port, target_file):
    logs = debug.fresh_logs(Path("artifacts/dma-install-failure"))
    evidence = logs / "evidence.json"
    symbols = debug.symbols(kernel)
    allocator = symbols["__rustc::__rust_alloc"]
    pin = symbols["<kernel::resource::dma::object::Pin as kernel::task::request::Request>::advance"]
    code = subprocess.check_output([
        "riscv64-elf-gdb", "-q", "-nx", "--batch", str(kernel), "-ex", "set language c",
        "-ex", "info line kernel/src/resource/dma/object.rs:387",
        "-ex", f"disassemble /r {pin:#x},{pin+1150:#x}",
        "-ex", f"disassemble /r {allocator:#x},{allocator+52:#x}",
    ], text=True, stderr=subprocess.STDOUT)
    (logs / "code-windows.log").write_text(code)
    point = int(re.search(r"Line 387 .*starts at address (0x[0-9a-f]+)", code)[1], 16)
    # 当轮首指令须尚未进入allocator栈帧/堆，注入以RV64标量返回ABI直接返回null。
    if not re.search(rf"{allocator:#x}[^\n]*addi\s+sp,sp,-32", code):
        raise RuntimeError("allocator entry instruction differs from the verified unexecuted prologue")
    facts = {
        "kernel_sha256": hashlib.sha256(kernel.read_bytes()).hexdigest(),
        "boot_package_sha256": hashlib.sha256(Path(os.environ["ERHINO_BOOT_PACKAGE"]).read_bytes()).hexdigest(),
        "scope": "one unexecuted releases Buffer allocation returns null; not physical heap exhaustion or quota",
        "checkpoint": hex(point), "allocator": hex(allocator), "verified": False,
    }
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
import gdb,json
from pathlib import Path
TARGET=Path({json.dumps(str(target_file.resolve()))})
EVIDENCE=Path({json.dumps(str(evidence.resolve()))})
facts=json.loads(EVIDENCE.read_text())
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
        require(key is not None,'unexpected pointer layout: '+str(value.type))
        value=value[key]
    return value
def arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    return pointer(value).dereference()['data']
def stack():
    frames=[];frame=gdb.newest_frame()
    while frame:frames.append(frame);frame=frame.older()
    return frames
def find(frames,part):return next((f for f in frames if part in (f.name() or '')),None)
def owner(frames):
    pending=find(frames,'kernel::task::request::Pending::advance')
    space=find(frames,'kernel::task::request::advance_in_space')
    require(pending is not None and space is not None,'installation lacks actual Pending admission')
    thread=int(pending.read_var('thread'))
    require(int(arc(space.read_var('owner').dereference()['value']).address)==thread,'Pending owner differs from admission')
    installed=word((int(gdb.parse_and_eval('$tp'))&((1<<64)-1))+hart_offsets['current_thread'])
    require(installed in (0,thread),'conflicting installed user thread')
    require((int(gdb.parse_and_eval('$satp'))&((1<<64)-1))==word(thread+thread_offsets['satp']),'actual SATP differs from admitted owner')
    shell=word(thread+process_offset+ptr_offset+pointer_offset)
    return thread,word(shell+data_offset+pid_offset),installed
def values(table):
    todo=[table['root']];count=0
    while todo:
        item=optional(todo.pop())
        if item is None:continue
        node=pointer(item['data']).dereference();yield node['value']
        todo.extend([node['left'],node['right']]);count+=1
        require(count<=int(table['len']),'ordered table traversal exceeded len')
    require(count==int(table['len']),'ordered table traversal incomplete')
checkpoint=gdb.Breakpoint('*{point:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
allocation=gdb.Breakpoint('*{allocator:#x}',type=gdb.BP_HARDWARE_BREAKPOINT)
returned=gdb.Breakpoint("'kernel::syscall::respond_error'",type=gdb.BP_HARDWARE_BREAKPOINT)
allocation.enabled=False;returned.enabled=False
record=None
while True:
    gdb.execute('continue',to_string=True)
    frames=stack()
    pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
    if pc=={point}:
        if not TARGET.exists():continue
        target=json.loads(TARGET.read_text())
        if 'pool' not in target:continue
        thread,pid,installed=owner(frames)
        if pid!=target['pid']:continue
        pin=frames[0].read_var('self').dereference()
        mapping=arc(pin['prepared']['mapping'])
        owned=optional(mapping['owned']['data']['value'])
        require(owned is not None,'mapping lost its original physical owner')
        require(optional(owned['acquisition']) is None and optional(owned['release']) is None,'mapping not fully acquired')
        usage=optional(owned['usage']);write=optional(owned['write'])
        require(usage is not None and write is not None,'installation lacks complete use/write')
        core=arc(pin['core'])
        require(int(pointer(core['state']['data']['value']['object']))==target['identity'],'Pin uses a different MemoryObject identity')
        require(int(pointer(usage['backing']))==int(pointer(core['backing'])),'complete use belongs to a different Backing owner')
        span=optional(usage['range'])
        require(span is not None and int(span['hi'])-int(span['lo'])==2,'installation use is not the complete two-page range')
        root=arc(arc(mapping['owner'])['root'])
        members=[entry for entry in values(root['state']['data']['value']['mappings'])
                 if int(arc(entry['mapping']).address)==int(mapping.address)]
        require(len(members)==1,'original mapping is not uniquely registered')
        state=members[0]['state']
        require(str(state['phase']).endswith('ReadyUnexposed') and not bool(state['stop']) and int(state['readers'])==0,'mapping phase is not ReadyUnexposed')
        original_pool=arc(arc(frames[0].read_var('thread').dereference()['process'])['_metadata']['pool'])
        require(int(original_pool['header']['koid'])==target['pool'],'original caller Pool differs from admitted Pool')
        record={{**target,'thread':thread,'frame':thread+thread_offsets['frame'],'hart_current':installed,
                 'satp':hex(word(thread+thread_offsets['satp'])),'hart':gdb.selected_thread().ptid[1]-1,
                 'pin':int(pin.address),'mapping':int(mapping.address),'mapping_id':int(mapping['id']),
                 'phase':str(state['phase']),'readers':0,'write':True,'range':[int(span['lo']),int(span['hi'])],
                 'caller_pool':int(original_pool.address),'checkpoint_pc':hex(pc)}}
        record['sepc']=word(record['frame']+context_offsets['sepc'])
        facts['window']=record;EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
        print('DMA_INSTALL_READY '+json.dumps(record,sort_keys=True))
        checkpoint.enabled=False;allocation.enabled=True
        continue
    if pc=={allocator}:
        if find(frames,'kernel::task::request::Pending::advance') is None:continue
        thread,pid,_=owner(frames)
        if record is None or thread!=record['thread'] or pid!=record['pid']:continue
        install=find(frames,'kernel::task::handle::install')
        # Rust DWARF可能用impl#而非demangled CapTable；源文件与reserve函数共同定位。
        reserve=next((f for f in frames if f.find_sal().symtab is not None
                      and 'capability/src/table.rs' in f.find_sal().symtab.fullname()
                      and '::reserve' in (f.name() or '')),None)
        buffer=find(frames,'funded_storage::Buffer<capability::table::LastCapability<')
        if buffer is None:continue
        facts['allocation_stack']=[f.name() for f in frames]
        EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
        require(install is not None and reserve is not None,'allocator not in original handle reservation')
        require(int(install.read_var('thread'))==thread,'installation thread changed')
        payer=buffer.read_var('payer').dereference()
        require(int(arc(payer['__0']).address)==record['caller_pool'],'Buffer payer is not original caller Pool')
        require(int(buffer.read_var('capacity'))==1,'release Buffer is not the single-cap reservation')
        size=int(gdb.parse_and_eval('$a0'));align=int(gdb.parse_and_eval('$a1'))
        expected=gdb.lookup_type('capability::table::LastCapability<kernel::task::cap::ObjectAuthority>')
        require(size==expected.sizeof and align==expected.alignof,'allocator layout does not match the releases element')
        handles=reserve.read_var('handles')
        require(optional(handles['charge']) is not None and optional(handles['data']) is not None,'first handles Buffer has not acquired its real storage/charge')
        charge=optional(buffer.read_var('charge'))
        require(charge is not None and int(arc(charge['pool']).address)==record['caller_pool'],'release Buffer has not paid its actual charge')
        record['allocation']={{'size':size,'align':align,'entry':hex(pc),'return_pc':hex(int(gdb.parse_and_eval('$ra'))&((1<<64)-1)),
                              'buffer':buffer.name(),'stack':[f.name() for f in frames], 'handles_prepaid':True,'releases_prepaid':True}}
        facts['window']=record;EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
        print('DMA_INSTALL_ALLOCATION '+json.dumps(record['allocation'],sort_keys=True))
        allocation.enabled=False;returned.enabled=True
        gdb.execute('set $a0 = 0');gdb.execute('set $pc = $ra')
        facts['injected']=True
        continue
    if returned.enabled:
        frame=frames[0]
        actual=int(frame.read_var('frame'))
        if actual!=record['frame'] or word(actual+context_offsets['sepc'])!=record['sepc']:continue
        error=str(frame.read_var('err'))
        require(error.endswith('OutOfMemory'),'Pin did not return actual OutOfMemory: '+error)
        record['actual_error']=error;facts['verified']=True
        EVIDENCE.write_text(json.dumps(facts,indent=2)+'\\n')
        print('DMA_INSTALL_FAILURE_VERIFIED '+json.dumps(record,sort_keys=True))
        break
end
detach
quit 0
'''
    script_path = logs / "install.gdb"
    script_path.write_text(script)
    ast.parse(script.split("\npython\n", 1)[1].split("\nend\n", 1)[0], filename=str(script_path))
    output = (logs / "install.log").open("w")
    child = subprocess.Popen(["riscv64-elf-gdb", "-q", "-nx", "--batch", "-x", str(script_path)], stdout=output, stderr=subprocess.STDOUT)
    return child, output, logs


def verify(logs, status):
    facts = json.loads((logs / "evidence.json").read_text())
    if status or not facts.get("verified") or not facts.get("injected"):
        raise RuntimeError(f"DMA specified installation allocation failure incomplete: {logs}")
    return facts
