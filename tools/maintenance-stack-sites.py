#!/usr/bin/env python3
"""固定ELF的动态方法site登记。未知间接跳转保留未决，累计证明必须拒绝。"""

import argparse
import json
from pathlib import Path
import re
import struct


def jump_table_candidate(elf, caller, site):
    """只提取局部绝对表事实；索引域必须另由range/源码证明，不自动放行。"""
    data=Path(elf).read_bytes()
    header=struct.unpack_from('<16sHHIQQQIHHHHHH',data)
    segments=[]
    for i in range(header[10]):
        segment=struct.unpack_from('<IIQQQQQQ',data,header[5]+i*header[9])
        if segment[0]==1:segments.append(segment)
    previous=site['preceding_instructions']
    if len(previous)<4 or [r['opcode'] for r in previous[-3:]]!=['addi','add','ld']:
        return None
    match=re.search(r'# ([0-9a-f]+)',previous[-3]['operands'])
    if not match:return None
    base=int(match[1],16)
    found=next((s for s in segments if s[3]<=base<s[3]+s[5]),None)
    if found is None:return None
    # 当前链接的PT_LOAD可同时含text/data；只读性由表所属section独立核实。
    readonly=[]
    for i in range(header[12]):
        section=struct.unpack_from('<IIQQQQIIQQ',data,header[6]+i*header[11])
        if section[2]&2 and not section[2]&5:
            readonly.append((section[3],section[3]+section[5]))
    table_section=next((span for span in readonly if span[0]<=base<span[1]),None)
    if table_section is None:return None
    # 固定指令链必须确实把index*8加到该只读表，再从此地址加载jr目标。
    if [r['opcode'] for r in previous[-5:]] != ['slli','auipc','addi','add','ld']:
        return None
    operands=[r['operands'].split('#')[0].strip() for r in previous[-5:]]
    shift=re.fullmatch(r'(\w+),(\w+),(?:0x3|3)',operands[0])
    high=re.fullmatch(r'(\w+),[^,]+',operands[1])
    low=re.fullmatch(r'(\w+),(\w+),-?\d+',operands[2])
    add=re.fullmatch(r'(\w+),(\w+),(\w+)',operands[3])
    load=re.fullmatch(r'(\w+),0\((\w+)\)',operands[4])
    target_register=site['instruction'].split()[-1]
    if not all((shift,high,low,add,load)):
        return None
    if not (high[1]==low[1]==low[2]==add[1]==add[2]==load[2]
            and shift[1]==add[3] and load[1]==target_register):
        return None
    offset=found[2]+base-found[3]
    start=int(site['caller_address'],16);end=start+caller['size']
    targets=[]
    for i in range(512):
        if base+8*(i+1)>min(found[3]+found[5],table_section[1]):break
        target=struct.unpack_from('<Q',data,offset+8*i)[0]
        if not start<=target<end:break
        targets.append(hex(target))
    return dict(base=hex(base),entries=len(targets),targets=targets) if targets else None


def normalize(name):
    name = name.replace('::<', '<')
    if name.startswith('<'):
        match = re.fullmatch(r'<(.+)>::([^<>]+)', name)
        if match and ' as ' not in match[1]:
            return match[1] + '::' + match[2]
    return name


def jump_premise(name, candidate):
    if candidate is None:return None
    name=normalize(name)
    if name.startswith('core::sync::atomic::'):
        return (5,'Rust core Ordering五个合法值；weak-CAS第二层Ordering同域') if candidate['entries']==5 else None
    rules={
        'core::ptr::drop_glue<kernel::vm::change::Phase>':(8,'os/kernel/src/vm/change.rs：有载荷变体的drop tag先减2，显式bltu以7为上界'),
        'kernel::sbi::SbiExtension::number':(6,'os/kernel/src/sbi.rs：封闭SbiExtension枚举match'),
        'kernel::sbi::sbi_call':(15,'os/kernel/src/sbi.rs：SBI error整数match，默认分支在表前范围外分流'),
        'kernel::boot::bootstrap::Bootstrap::advance':(9,'os/kernel/src/boot/bootstrap.rs：Loader Phase封闭枚举match'),
        'kernel::task::process::members::Members::termination_step':(6,'os/kernel/src/task/process/members.rs：成员状态封闭枚举match'),
        '<kernel::vm::retire::RetireWork as kernel::maintenance::Work>::step':(6,'os/kernel/src/vm/retire.rs：退休Phase封闭枚举match'),
        '<kernel::boot::bootstrap::Bootstrap as kernel::maintenance::Work>::cost_name':(9,'os/kernel/src/boot/bootstrap.rs：与Loader Phase同域'),
        '<kernel::task::process::shell::ProcessShell as kernel::maintenance::Work>::step':(13,'os/kernel/src/task/process/shell.rs：ReclaimState封闭枚举match'),
        '<kernel::vm::change::ChangeWork as kernel::maintenance::Work>::step':(11,'os/kernel/src/vm/change.rs：发布Phase封闭枚举match'),
        'kernel::vm::backing::map_error':(7,'os/kernel/src/vm/backing.rs：BackingError封闭枚举match'),
        'kernel::vm::operation::error':(17,'os/kernel/src/vm/operation.rs：ChangeError封闭枚举match'),
        'kernel::task::memory_pool::map_pool_error':(9,'os/kernel/src/task/memory_pool.rs：PoolError封闭枚举match'),
    }
    errors={
        'dtb::memory::MemoryMapError':13,'erhino_shared::call::SystemCallError':62,
        'stack_layout::LayoutError':5,'dtb::cpu::CpuError':20,
        'memory_space::space::ChangeError':20,'memory_space::backing::BackingError':7,
        'kernel::sbi::SbiError':15,
        'funded_frame::FundError<memory_pool::PoolError, kernel::frame::UserClaimError>':6,
        'elf::ElfError':24,'core::num::error::IntErrorKind':6,'dtb::FdtError':5,
        'memory_pool::PoolError':9,'memory_supply::PlanError':9,
    }
    for error,count in errors.items():
        if name in (f'<{error} as core::fmt::Debug>::fmt',f'<{error} as core::fmt::Display>::fmt'):
            rules[name]=(count,'对应固定源码错误枚举的derive/穷尽match，合法tag域；不是函数指针调用')
    result=rules.get(name)
    return result if result and result[0]==candidate['entries'] else None


def method_offset(site):
    text = site['instruction'].split(' ', 1)[1]
    register = re.search(r'\((\w+)\)', text)
    register = register[1] if register else text.split(',')[-1].strip()
    for instruction in reversed(site['preceding_instructions']):
        operands = instruction['operands'].split('#')[0].strip()
        if instruction['opcode'] == 'ld':
            match = re.fullmatch(r'(\w+),(-?\d+)\((\w+)\)', operands)
            if match and match[1] == register:
                return int(match[2])
        if instruction['opcode'] == 'mv':
            match = re.fullmatch(r'(\w+),(\w+)', operands)
            if match and match[1] == register:
                register = match[2]
                continue
        if operands.split(',')[0] == register and instruction['opcode'] not in ('sd', 'sw', 'sb'):
            return None
    return None


def classify(facts):
    symbols = facts['symbols']
    names = [row['name'] for row in symbols.values()]
    work_types = sorted(set(match[1] for name in names if (match := re.fullmatch(r'<(.+) as kernel::maintenance::Work>::step', name))))
    kernel_types = sorted(set(match[1] for name in names if (match := re.fullmatch(r'<(.+) as kernel::task::object::KernelObject>::holders', name))))
    hint_types = sorted(set(match[1] for name in names if (match := re.fullmatch(r'<(.+) as kernel::task::wait_source::HintTarget>::hint', name))))
    observation_types = sorted(set(match[1] for name in names if (match := re.fullmatch(r'<(.+) as kernel::task::wait_source::ObservationSource>::snapshot', name))))
    def methods(trait, method):
        return sorted(set(name for name in names if re.fullmatch(r'<.+ as '+re.escape(trait)+r'>::'+re.escape(method), name) or normalize(name) == trait+'::'+method))
    def drops(types):
        wanted = set('core::ptr::drop_glue<'+name+'>' for name in types)
        return sorted(set(name for name in names if normalize(name) in wanted))
    sites = []
    for caller_address, caller in symbols.items():
        for source in caller['dynamic']:
            row = dict(caller=caller['name'], caller_address=caller_address, **source)
            name = normalize(caller['name'])
            targets, category = [], 'unresolved'
            offset = method_offset(source)
            if source['opcode'] == 'jr':
                category = 'local_jump_requires_witness'
                row['jump_table_candidate']=jump_table_candidate(facts['elf'],caller,row)
                premise=jump_premise(name,row['jump_table_candidate'])
                if premise:
                    category='compiler.local_match'
                    row['jump_witness']=dict(**row['jump_table_candidate'],index_bound=premise[0],premise=premise[1])
            elif name == 'core::ptr::drop_glue<dyn kernel::maintenance::Work>':
                category, targets = 'dyn.Work.drop', drops(work_types)
            elif name == 'core::ptr::drop_glue<dyn kernel::task::wait_source::HintTarget>':
                category, targets = 'dyn.HintTarget.drop', drops(hint_types)
            elif name == 'core::ptr::drop_glue<dyn kernel::task::wait_source::ObservationSource>':
                category, targets = 'dyn.ObservationSource.drop', drops(observation_types)
            elif name == 'core::ptr::drop_glue<dyn kernel::task::object::KernelObject>':
                category, targets = 'dyn.KernelObject.drop', drops(kernel_types)
            elif name == 'core::ptr::drop_glue<dyn core::any::Any + core::marker::Sync + core::marker::Send>':
                category, targets = 'dyn.Any.drop', drops(kernel_types)
            elif name.startswith('<dyn core::any::Any>::is'):
                category, targets = 'dyn.Any.type_id', methods('core::any::Any','type_id')
            elif name == 'kernel::maintenance::advance_one':
                method = {24:'cost_name',32:'record',40:'step'}.get(offset)
                if method:
                    category, targets = 'Work.'+method, methods('kernel::maintenance::Work', method)
            elif name.startswith('kernel::maintenance::') and offset == 32:
                category, targets = 'Work.record', methods('kernel::maintenance::Work','record')
            elif name == 'kernel::boot::bootstrap::Bootstrap::advance' and offset in (24,32,40):
                method={24:'snapshot',32:'attach',40:'detach'}[offset]
                category,targets='ObservationSource.'+method,methods('kernel::task::wait_source::ObservationSource',method)
            elif name == 'kernel::task::wait::detach' or name == '<kernel::task::wait_set::Member as core::ops::drop::Drop>::drop':
                category,targets='ObservationSource.detach',methods('kernel::task::wait_source::ObservationSource','detach')
            elif name == 'kernel::task::cap::deliver':
                category,targets='KernelObject.on_last_capability',methods('kernel::task::object::KernelObject','on_last_capability')
            elif name == '<kernel::task::cap::ObjectAuthority as capability::Authority>::holders':
                category,targets='KernelObject.holders',methods('kernel::task::object::KernelObject','holders')
            elif name.startswith('kernel::sched::SchedDomain::') or name == 'kernel::sched::ReadyBatch::publish_one':
                method={'enqueue_fair':'enqueue','reserve_ready':'reserve_batch','cancel_ready_step':'scan_ready_for','ready_len':'ready_len','publish_one':'enqueue'}.get(name.rsplit('::',1)[1])
                if method:
                    category,targets='SchedClass.'+method,methods('kernel::sched::SchedClass',method)
            elif name == 'kernel::task::object::release':
                category, targets = 'KernelObject.on_last_capability', methods('kernel::task::object::KernelObject','on_last_capability')
            elif name == 'kernel::task::object::ObjectHeader::finish_offered':
                category, targets = 'HintTarget.finish', methods('kernel::task::wait_source::HintTarget','hint')
            elif name in ('kernel::task::wait_source::SignalSource::step', 'kernel::task::wait_source::Delivery::send'):
                category, targets = 'HintTarget.source', methods('kernel::task::wait_source::HintTarget','hint')
            elif name == 'kernel::registry::try_ipi_slots':
                category = 'PlatformBackend.send_ipi'
                targets = [name for name in names if name.endswith(' as kernel::platform::PlatformBackend>::send_ipi')]
            elif name == 'kernel::registry::timer::set_timer':
                category = 'PlatformBackend.set_timer'
                targets = [name for name in names if name.endswith(' as kernel::platform::PlatformBackend>::set_timer')]
            elif name == 'alloc::sync::Weak<T,A>::drop_slow':
                category = 'Weak.heap_dealloc'
                targets = [name for name in names if 'kernel::heap::LockedKernelAllocator' in name and name.endswith('::dealloc')]
            elif name == 'alloc::sync::Arc<T,A>::drop_slow':
                category = 'Arc.dynamic_drop'
                targets = drops(work_types+kernel_types+hint_types+['kernel::task::wait_source::SignalSource','kernel::task::wait_set::WaitSet','kernel::task::process::thread::Thread'])
            elif name.startswith('core::fmt::') or ' as core::fmt::' in name:
                category = 'fmt.fixed_binary_vtables'
                targets = [entry for entry in names if (' as core::fmt::' in entry and entry.endswith('::fmt') and '<dyn ' not in entry)
                    or ' as core::fmt::Write>::' in entry or normalize(entry).startswith('core::fmt::Write::')]
            row.update(category=category, targets=sorted(set(targets)), vtable_offset=offset)
            sites.append(row)
    missing = [dict(site=row['site'], target=name) for row in sites for name in row['targets'] if name not in names]
    return dict(elf=facts['elf'], sha256=facts['sha256'], sites=sites, missing_targets=missing,
        types=dict(work=work_types,kernel_object=kernel_types,hint=hint_types,observation=observation_types),
        limits='targets是固定binary内保守候选；调用者/阶段/最后引用剪边由累计模型单独证明，未决不能放行')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('facts',type=Path)
    parser.add_argument('output',type=Path)
    args=parser.parse_args()
    result=classify(json.loads(args.facts.read_text()))
    args.output.write_text(json.dumps(result,indent=2,ensure_ascii=False)+'\n')
    print('Dynamic facts:',len(result['sites']),'sites;',len(result['types']['work']),'Work types;',len(result['missing_targets']),'missing targets')
    from collections import Counter
    print(dict(Counter(row['category'] for row in result['sites'])))


if __name__=='__main__':
    main()
