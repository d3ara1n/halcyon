#!/usr/bin/env python3
"""M原子源码语义模型；未知动态site/CFI/递归界一律拒签。输入必须同一ELF。"""

import argparse
from collections import defaultdict
from functools import lru_cache
import hashlib
import importlib.util
import json
from pathlib import Path
import re
import subprocess
import sys


def source_bindings():
    root=Path(__file__).resolve().parents[1]
    files=[]
    for directory in ('os','shared'):
        files.extend(path for path in (root/directory).rglob('*') if path.is_file()
            and (path.suffix in ('.rs','.toml','.ld','.S','.s','.asm','.json') or path.name=='Cargo.lock')
            and 'target' not in path.parts)
    files.extend((root/'tools').glob('maintenance-stack-*.py'))
    result={str(path.relative_to(root)):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(files)}
    sysroot=Path(subprocess.check_output(['rustc','--print','sysroot'],text=True).strip())
    library=sysroot/'lib/rustlib/src/rust/library'
    for crate in ('core','alloc'):
        for path in sorted((library/crate/'src').rglob('*.rs')):
            result['rust-src/'+str(path.relative_to(library))]=hashlib.sha256(path.read_bytes()).hexdigest()
    if not any(name.startswith('rust-src/core/') for name in result):
        raise ValueError('audited rust-src is missing')
    return result


def validate_certificate(path,args,result):
    certificate=json.loads(path.read_text())
    if certificate['sources']!=source_bindings():
        raise ValueError('certificate source/model binding differs; semantic re-audit required')
    if certificate['elf_sha256']!=result['sha256']:
        raise ValueError('certificate ELF identity differs')
    facts=json.loads(args.facts.read_text())
    if hashlib.sha256(Path(facts['elf']).read_bytes()).hexdigest()!=result['sha256']:
        raise ValueError('actual ELF bytes differ from the certified facts')
    for name in ('facts','sites','receivers'):
        file=getattr(args,name)
        if file is None or certificate['inputs'][name]!=hashlib.sha256(file.read_bytes()).hexdigest():
            raise ValueError('certificate '+name+' identity differs')
    result['certificate']=dict(path=str(path),sha256=hashlib.sha256(path.read_bytes()).hexdigest())

spec=importlib.util.spec_from_file_location('stack_sites',Path(__file__).with_name('maintenance-stack-sites.py'))
site_module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(site_module)
normalize=site_module.normalize


def components(graph):
    counter=0; stack=[]; on=set(); index={}; low={}; result=[]
    def visit(v):
        nonlocal counter
        index[v]=low[v]=counter; counter+=1;stack.append(v);on.add(v)
        for w in graph.get(v,()):
            if w not in index: visit(w);low[v]=min(low[v],low[w])
            elif w in on:low[v]=min(low[v],index[w])
        if low[v]==index[v]:
            group=[]
            while True:
                w=stack.pop();on.remove(w);group.append(w)
                if w==v:break
            result.append(group)
    for v in graph:
        if v not in index:visit(v)
    return result


def cumulative(facts,s,graph,reachable,roots,format_targets):
    source=Path(__file__).resolve().parents[1]
    frame=(source/'os/kernel/src/frame.rs').read_text()
    chunk=1<<int(re.search(r'const HEAP_CHUNK_SIZE: usize = 1 << (\d+);',frame)[1])
    chunks=int(re.search(r'const HEAP_CHUNK_LIMIT: usize = (\d+);',frame)[1])
    depth_source=(source/'shared/erhino_shared/src/memory_pool.rs').read_text()
    pool_depth=int(re.search(r'MEMORY_POOL_MAX_DEPTH: u32 = (\d+);',depth_source)[1])+1
    # 可达AVL节点和RegionTemplate在RV64布局均至少32B；一个节点至少两个
    # 子边、key、value及高度。对整个system heap作上界，忽略其他所有占用。
    items=chunk*chunks//32
    previous,current=0,1;height=0
    while current<=items:
        height+=1;previous,current=current,1+current+previous
    avl_depth=height+1
    quick_depth=2*items.bit_length()
    median_depth=1;sample=items
    while sample>1:sample=(sample+7)//8;median_depth+=1
    # 正常格式数据仅scalar、无递归的错误枚举和console两层Arguments。
    # 按全部已解析formatter callback数加两层Arguments保守覆盖每成员重入，
    # 同层PadAdapter/数字辅助函数均包含在SCC帧和内。
    format_depth=len(format_targets)+2
    live={a:graph[a]&reachable for a in reachable}
    groups=components(live);owner={a:i for i,group in enumerate(groups) for a in group}
    weights={};domains=[];unknown=[]
    for i,group in enumerate(groups):
        names=[s[a]['name'] for a in group]
        recursive=len(group)>1 or group[0] in live[group[0]]
        depth=1;kind='acyclic'
        if recursive:
            if 'core::ptr::drop_glue<kernel::maintenance::Account>' in names:
                depth=pool_depth;kind='pool ancestry'
            elif all('ordered_table::' in n or 'memory_space::ordmap::' in n for n in names):
                depth=avl_depth;kind='AVL height'
            elif any('core::slice::sort::unstable::quicksort::quicksort' in n for n in names):
                depth=quick_depth;kind='introsort recursion limit'
            elif any('core::slice::sort::shared::pivot::median3_rec' in n for n in names):
                depth=median_depth;kind='pivot sample /8'
            elif 'core::fmt::rt::Argument::fmt' in names and all(n.startswith(('core::','<core::')) or ' as core::fmt::' in n for n in names):
                depth=format_depth;kind='fixed format inputs'
            else:unknown.append(names)
            domains.append(dict(kind=kind,depth=depth,frames=sum(s[a].get('frame',0) for a in group),names=names))
        weights[i]=sum(s[a].get('frame',0) for a in group)*depth
    dag={i:{owner[b] for a in group for b in live[a] if owner[b]!=i} for i,group in enumerate(groups)}
    @lru_cache(None)
    def longest(i):
        child=max(dag[i],key=lambda c:longest(c)[0],default=None)
        if child is None:return weights[i],[i]
        cost,path=longest(child);return weights[i]+cost,[i]+path
    peak,path=max((longest(owner[a]) for a in roots),key=lambda pair:pair[0])
    by_name={row['name']:row for row in s.values() if ' [' not in row['name']}
    prefixes={}
    for label,names in [('boot',['hart_formal_entry','kernel::rt::bring_up_runtime','kernel::sched::run']),('secondary',['hart_formal_entry','kernel::sched::run'])]:
        if any(name not in by_name or 'frame' not in by_name[name] or by_name[name].get('unsupported_cfi') for name in names):
            unknown.append(['missing entry prefix: '+label]);continue
        prefixes[label]=dict(frames={name:by_name[name]['frame'] for name in names},bytes=sum(by_name[name]['frame'] for name in names))
    constants=facts.get('absolute_symbols',{})
    size,emergency=constants.get('STACK_SIZE'),constants.get('EMERGENCY_SIZE')
    stack=size-emergency if size is not None and emergency is not None else None
    if stack is None:unknown.append(['missing linked STACK_SIZE/EMERGENCY_SIZE'])
    return dict(peak=peak,path=[[s[a]['name'] for a in groups[i]] for i in path],
        heap_bytes=chunk*chunks,minimum_node_bytes=32,item_upper_bound=items,domains=domains,unknown=unknown,
        prefixes=prefixes,stack_slot_bytes=size,emergency_bytes=emergency,formal_stack_bytes=stack,
        bounds={label:peak+row['bytes'] for label,row in prefixes.items()})


def build(facts, registry, receivers=None):
    if facts['sha256']!=registry['sha256']:
        raise ValueError('facts/site ELF identity differs')
    if receivers and receivers['sha256']!=facts['sha256']:
        raise ValueError('receiver/facts ELF identity differs')
    s={a:dict(row,name=normalize(row['name']),actual_name=row['name']) for a,row in facts['symbols'].items()}
    by_name=defaultdict(list)
    for a,row in s.items():by_name[row['name']].append(a)
    def one(name):
        result=by_name[name]
        if len(result)!=1:raise ValueError(f'expected unique symbol: {name}: {result}')
        return result[0]
    graph={a:set() for a in s};cuts=[]
    ignored=('core::panicking::','core::option::unwrap_failed','core::result::unwrap_failed','core::str::slice_error_fail','kernel::panic::','kernel::rt::fatal_msg','kernel::rt::handle_alloc_error','kernel::trap::fatal','kernel::runtime_stop::report','kernel::hart::park','<kernel::rt::KernelRuntime as core::rt::Runtime>::','core::rt::')
    def permitted(target):
        return not s[target]['name'].startswith(ignored)
    for a,row in s.items():
        for call in row['calls']:
            if row['name'] in ('kernel::sync::ladder::push','kernel::sync::ladder::pop') and 'core::fmt::rt::Argument' in s[call['target']]['name']:
                cuts.append(dict(caller=row['name'],target=s[call['target']]['name'],reason='仅锁阶断言失败构造的诊断参数，正常加锁/解锁保持自身真实CFI帧'))
                continue
            if permitted(call['target']):graph[a].add(call['target'])
        if row.get('self_calls'):graph[a].add(a)
    unresolved=[];fmt_sites=[]
    for row in registry['sites']:
        caller=row['caller_address']
        if row['category']=='fmt.fixed_binary_vtables':
            fmt_sites.append(row);continue
        if row['category'] in ('unresolved','local_jump_requires_witness') or (not row['targets'] and row['category']!='compiler.local_match'):
            unresolved.append(row)
        for target_name in row['targets']:
            targets=[a for a,entry in s.items() if entry['actual_name']==target_name]
            if not targets:raise ValueError('missing target '+target_name)
            graph[caller].update(a for a in targets if permitted(a))
    def cut(a,b,reason):
        if b in graph[a]:
            graph[a].remove(b);cuts.append(dict(caller=s[a]['name'],target=s[b]['name'],reason=reason))
    def null_fields(a,predicate,reason):
        for b in list(graph[a]):
            if predicate(s[b]['name']):cut(a,b,reason)
    def clone(a,suffix):
        key=a+'#'+suffix
        if key not in s:
            s[key]=dict(s[a],name=s[a]['name']+' ['+suffix+']');graph[key]=set(graph[a])
        return key
    def context(a,b,suffix,reason):
        replacement=clone(b,suffix)
        cut(a,b,reason);graph[a].add(replacement)
        return replacement
    def empty_option(a,b,reason):
        # None仍执行包装函数的prologue/epilogue；只去掉不可能的Some载荷边。
        empty=context(a,b,'empty-option',reason)
        graph[empty]=set()
    def empty_payload(a,b,needle,end_predicate,suffix,reason):
        # 沿精确字段类型的wrapper/Buffer.clear链克隆，保留每层CFI与费用路径。
        visited={}
        def project(node):
            if node in visited:return visited[node]
            replacement=clone(node,suffix)
            visited[node]=replacement
            if end_predicate(s[node]['name']):
                graph[replacement]=set()
            else:
                graph[replacement]={project(child) if needle in s[child]['name'] else child for child in graph[node]}
            return replacement
        replacement=project(b)
        cut(a,b,reason);graph[a].add(replacement)
    def nonlast_arc(a,b,needle,suffix,reason):
        visited={}
        def project(node):
            if node in visited:return visited[node]
            replacement=clone(node,suffix);visited[node]=replacement
            graph[replacement]=set()
            for child in graph[node]:
                if needle in s[child]['name'] and s[child]['name'].endswith('::drop_slow'):
                    cuts.append(dict(caller=s[replacement]['name'],target=s[child]['name'],reason=reason))
                    continue
                graph[replacement].add(project(child) if needle in s[child]['name'] else child)
            return replacement
        replacement=project(b)
        cut(a,b,reason);graph[a].add(replacement)
    def typed_work(a,b,types,suffix,reason):
        visited={}
        allowed={'core::ptr::drop_glue<'+kind+'>' for kind in types}
        def project(node):
            if node in visited:return visited[node]
            replacement=clone(node,suffix);visited[node]=replacement
            if s[node]['name']=='core::ptr::drop_glue<dyn kernel::maintenance::Work>':
                graph[replacement]={child for child in graph[node] if s[child]['name'] in allowed}
                if not graph[replacement]:raise ValueError('typed token target set vanished')
            else:
                graph[replacement]={project(child) if 'kernel::maintenance::Work' in s[child]['name'] else child for child in graph[node]}
            return replacement
        replacement=project(b);cut(a,b,reason);graph[a].add(replacement)
    if receivers:
        if receivers['sha256']!=facts['sha256']:raise ValueError('receiver ELF identity differs')
        grouped=defaultdict(list)
        for proof in receivers['witnesses']:grouped[(proof['caller'],proof['target'])].append(proof)
        for (a,b),proofs in grouped.items():
            accepted=[proof for proof in proofs if proof['receiver']]
            if not accepted:continue
            receiver=clone(b,'nonlast-receiver')
            for child in list(graph[receiver]):
                if 'alloc::sync::Arc<' in s[child]['name'] and s[child]['name'].endswith(' as core::ops::drop::Drop>::drop'):
                    arc=context(receiver,child,'nonlast-receiver','Work原执行根保留至finish后；此调用准确别名self槽')
                    for slow in list(graph[arc]):
                        if s[slow]['name'].endswith('::drop_slow'):cut(arc,slow,'原执行器强根尚存，fetch_sub不能为1')
            if len(accepted)==len(proofs):
                cut(a,b,'全部同callee site均由DWARF/寄存器证据定位为receiver Drop')
            graph[a].add(receiver)
    # 这些字段为结构性终态；只剪字段析构，不剪付款/宿主退款。
    for a,row in list(s.items()):
        name=row['name']
        if name in ('kernel::maintenance::publish','kernel::maintenance::wake'):
            for b in list(graph[a]):
                if s[b]['name']=='core::mem::drop<alloc::sync::Arc<dyn kernel::maintenance::Work>>':
                    nonlast_arc(a,b,'alloc::sync::Arc<dyn kernel::maintenance::Work>','queue-root-held','实际drop只在QUEUE门内活跃状态；Idle/Done由返回值移交，不在wake析构')
        if name in ('kernel::maintenance::link','kernel::maintenance::pick'):
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>':
                    empty_option(a,b,'Ready链head/tail.next仅覆盖已take的空槽；摘出执行根由局部work保留，不在此最后析构')
        if name in ('kernel::task::wait_source::SignalSource::publish','kernel::task::wait_source::SignalSource::complete_internal'):
            for b in list(graph[a]):
                if s[b]['name']=='core::mem::drop<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>':
                    empty_payload(a,b,'alloc::sync::Arc<dyn kernel::maintenance::Work>',lambda n:n=='core::ptr::drop_glue<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>','completion-wake-active','内部唯一DONE waiter不提前放弃；来源锁内提交完成与wake，消费者不能先观察新事实并finish')
        if name.startswith('kernel::task::process::shell::ProcessShell::') or name in ('kernel::task::job::Job::wake_completion','<kernel::boot::bootstrap::Bootstrap as kernel::task::wait_source::HintTarget>::hint'):
            for b in list(graph[a]):
                if s[b]['name']=='core::mem::drop<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>':
                    nonlast_arc(a,b,'alloc::sync::Arc<dyn kernel::maintenance::Work>','token-self-held','Shell/Job token仅指同一宿主；调用者Lease、parent或Delivery的原Arc仍保留')
        if name=='<kernel::vm::change::VmAccessLease as core::ops::drop::Drop>::drop' or name in ('kernel::vm::change::VmShell::leave','kernel::vm::change::VmShell::close_entry'):
            for b in list(graph[a]):
                if s[b]['name']=='core::mem::drop<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>':
                    typed_work(a,b,['kernel::task::process::shell::ProcessShell','kernel::task::process::builder::ProcessBuilder'],'vm-waiter-targets','VM两个槽仅存WaitVm的Shell与Starting的Builder；保留其可能最后析构，不倒用VM Arc保活')
                if name.startswith('<kernel::vm::change::VmAccessLease') and s[b]['name']=='core::mem::drop<alloc::sync::Arc<kernel::vm::change::VmShell>>':
                    nonlast_arc(a,b,'alloc::sync::Arc<kernel::vm::change::VmShell>','access-count-still-held','VM门内先还Arc、后减access计数；Body/退休责任不能越过非零access')
        if name=='kernel::vm::remote::drain_current':
            for b in list(graph[a]):
                if s[b]['name']=='core::mem::drop<core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>>':
                    typed_work(a,b,['kernel::vm::change::ChangeWork','kernel::vm::retire::RetireWork'],'transport-targets','固定运输token仅由对应Change/Retire发布；保留最后ack后可能的完整宿主析构')
        if 'core::option::IntoIter<kernel::maintenance::WakeToken>' in name and name.startswith('core::ptr::drop_glue<'):
            for b in list(graph[a]):
                if 'kernel::maintenance::WakeToken' in s[b]['name']:
                    empty_payload(a,b,'kernel::maintenance::WakeToken',lambda n:n in ('core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>','core::ptr::drop_glue<core::option::Item<kernel::maintenance::WakeToken>>'),'token-inner-exhausted','唯一VmAccessLease双token for循环无早退，flatten只销毁耗尽的单项迭代器；Item仅有已take的Option字段')
        if 'core::iter::adapters::flatten::FlattenCompat<' in name and 'kernel::maintenance::WakeToken' in name and name.endswith('::next'):
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>':
                    empty_option(a,b,'flatten next的Some项移交单项迭代器；此临时Option析构仅None分支，真实token由循环mt::wake处理')
        if name.startswith('<core::array::iter::IntoIter<core::option::Option<kernel::maintenance::WakeToken>, 2> as core::ops::drop::Drop>::drop'):
            for b in list(graph[a]):
                if 'kernel::maintenance::WakeToken' in s[b]['name']:
                    empty_payload(a,b,'kernel::maintenance::WakeToken',lambda n:n=='core::ptr::drop_glue<[core::option::Option<kernel::maintenance::WakeToken>]>','token-array-exhausted','同一完整for循环已交付两个token，剩余slice长度为0；保留空slice函数帧')
        if name=='core::ptr::drop_glue<kernel::task::mailbox::MailboxCore>':
            for b in list(graph[a]):
                if 'kernel::task::mailbox::CoreState' in s[b]['name']:
                    empty_payload(a,b,'kernel::task::mailbox::',lambda n:n=='core::ptr::drop_glue<core::option::Option<kernel::task::mailbox::Message>>','mailbox-queue-drained','已发布Core由持久Work逐消息清退后Done；未发布出生队列全None，最终每slot只析构None，真实Message/cap门外释放不剪')
        if name=='core::ptr::drop_glue<kernel::maintenance::RecordInner>':
            for b in list(graph[a]):
                if 'core::option::Option<alloc::sync::Arc<dyn kernel::maintenance::Work>>' in s[b]['name']:
                    empty_option(a,b,'Record最终根/后继均空；Running原根在finish后才销毁')
        if name=='core::ptr::drop_glue<kernel::vm::remote::Inner>':
            null_fields(a,lambda n:'Transit' in n or 'WakeToken' in n,'Transport final outstanding=0；arm/root/inbox均已take')
        if name=='core::ptr::drop_glue<kernel::task::job::JobPhase>':
            null_fields(a,lambda n:n=='core::ptr::drop_glue<kernel::maintenance::WakeToken>','Job最终只能Dead或未发布且空的Open；Waiting自持Work根')
        if name=='core::ptr::drop_glue<kernel::task::process::thread::Thread>':
            for b in list(graph[a]):
                if 'kernel::task::wait::Entry' in s[b]['name']:
                    empty_payload(a,b,'kernel::task::wait::Entry',lambda n:n=='core::ptr::drop_glue<core::option::Option<kernel::task::wait::Entry>>','thread-wait-none','Thread最后引用前统一detach；未发布Thread未装等待')
                if 'TimerNode<kernel::sched::TimerDelivery>' in s[b]['name']:
                    empty_payload(a,b,'kernel::sched::TimerDelivery',lambda n:n=='core::ptr::drop_glue<core::option::Option<kernel::sched::TimerDelivery>>','thread-timer-none','TimerNode最终未链接且payload=None，注销/到期已锁外交回强Thread')
        if name=='core::ptr::drop_glue<kernel::task::process::members::Slot>':
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::task::process::members::Entry>>':
                    empty_option(a,b,'私有Slot只在已take entry后或空候选存储内析构；真实摘出Entry仍完整析构')
        if name=='core::ptr::drop_glue<kernel::task::process::builder::State>':
            for b in list(graph[a]):
                if 'Option<alloc::sync::Arc<kernel::task::process::shell::ProcessShell>>' in s[b]['name']:
                    empty_option(a,b,'M可达Builder最后析构仅Started/Aborted；shell已take；构造出生在M原子根外')
        if name in ('core::ptr::drop_glue<kernel::boot::bootstrap::Bootstrap>','core::ptr::drop_glue<kernel::task::process::builder::ProcessBuilder>'):
            for b in list(graph[a]):
                if 'Option<(u64, kernel::maintenance::WakeToken)>' in s[b]['name']:
                    empty_payload(a,b,'kernel::maintenance::WakeToken',lambda n:n=='core::ptr::drop_glue<core::option::Option<(u64, kernel::maintenance::WakeToken)>>','self-hint-none','内嵌hint token指同一宿主，非空时自身强根阻止最后析构')
        if name=='core::ptr::drop_glue<kernel::task::process::reclaim::ReclaimState>':
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):
                    empty_option(a,b,'Shell最终回收Done；retirement在发布完成前已take/锁外drop')
        if name=='core::ptr::drop_glue<kernel::task::process::shell::ShellState>':
            for b in list(graph[a]):
                if 'OwnedBox<kernel::task::process::body::ProcessBody' in s[b]['name']:
                    empty_option(a,b,'Shell最后态body=None；构造未装Body或已take/drop后才mark_dead')
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>':
                    empty_option(a,b,'最后Shell无ingress/quiescence等待根；有token时其Work强根仍保持Shell')
        if name=='core::ptr::drop_glue<kernel::task::wait::Entry>':
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):
                    empty_option(a,b,'唯一detach已take source，Entry flags断言注销；真实局部source在门外另行drop')
        if name=='core::ptr::drop_glue<kernel::task::wait_source::SignalState>':
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>':
                    empty_option(a,b,'内部完成waiter由DONE/complete_internal取走；等待方/被等待Work均保留该Source至交付')
        if name=='core::ptr::drop_glue<kernel::vm::remote::Transport>':
            for b in list(graph[a]):
                if 'UnsafeCell<core::option::Option<kernel::maintenance::WakeToken>>' in s[b]['name']:
                    empty_payload(a,b,'kernel::maintenance::WakeToken',lambda n:n=='core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>','transport-wake-none','最后Transport outstanding=0，最后ack已take token并交回真实根')
        if name=='core::ptr::drop_glue<kernel::vm::change::VmState>':
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::vm::space::AnySpace>>':
                    empty_option(a,b,'M可达VmShell最终空间已drain/take；冷出生失败不在M原子根')
                if s[b]['name']=='core::ptr::drop_glue<core::option::Option<kernel::maintenance::WakeToken>>':
                    empty_option(a,b,'最后VmShell已交回ingress/quiescence token；有等待Work根时不能最后析构')
                if 'Buffer<(memory_space::space::ChangeKey, alloc::sync::Arc<dyn kernel::task::wait_source::ObservationSource>)' in s[b]['name']:
                    needle='(memory_space::space::ChangeKey, alloc::sync::Arc<dyn kernel::task::wait_source::ObservationSource>)'
                    empty_payload(a,b,needle,lambda n:n=='core::ptr::drop_glue<['+needle+']>','vm-deps-empty','access=0断言dependencies空；保留Buffer/clear/空slice帧和真实charge退款')
        if name in ('core::ptr::drop_glue<kernel::vm::change::State>','core::ptr::drop_glue<kernel::vm::retire::State>'):
            namespace='change' if 'change::State' in name else 'retire'
            for b in list(graph[a]):
                if s[b]['name']=='core::ptr::drop_glue<kernel::vm::'+namespace+'::Phase>':
                    empty_option(a,b,'私有State最终仅未发布Idle或已完成Done；真实step取出的Phase继续完整析构')
        if name=='core::ptr::drop_glue<kernel::task::process::body::ProcessBody>':
            for b in list(graph[a]):
                if 'Option<alloc::sync::Arc<kernel::vm::retire::RetireWork>>' in s[b]['name']:
                    empty_option(a,b,'Body退休入口已take预付RetireWork；M中最终Body仅来自回收drop_body')
        if name in ('core::ptr::drop_glue<kernel::vm::completion::Completion>','core::ptr::drop_glue<kernel::vm::change::VmAccessLease>'):
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):
                    empty_option(a,b,'Drop实现已take该字段，真实局部metadata/access/vm的释放路径保留')
        if name=='core::ptr::drop_glue<kernel::task::process::builder::BuilderCompletion>':
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):
                    empty_option(a,b,'BuilderCompletion先take metadata，再发布DONE')
                if 'WorkReceipt' in s[b]['name']:
                    empty_payload(a,b,'kernel::task::process::shell::WorkReceipt',lambda n:n=='core::ptr::drop_glue<core::option::Option<kernel::task::process::shell::WorkReceipt>>','receipt-taken','Drop实现已take receipt，真实局部释放Body计数不剪')
        if name=='core::ptr::drop_glue<kernel::task::process::members::Members>':
            # 同函数最后态，不影响逐项摘出的真实Thread owner。
            for b in list(graph[a]):
                if 'Slot' in s[b]['name'] and 'Buffer' in s[b]['name']:
                    empty=context(a,b,'members-empty-buffer','Members Drop前全部entry=None且len/occupied归零；批量释放不含Thread')
                    null_fields(empty,lambda n:'drop_glue<' in n and ('Member' in n or 'Thread' in n),'空成员buffer保留真实费用析构')
        if name.startswith('core::ptr::drop_glue<ordered_table::OrderedTable<'):
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):empty_option(a,b,'OrderedTable最终root=None；摘出的真实NodeOwner仍保留')
        if name=='core::ptr::drop_glue<kernel::maintenance::Account>':
            for b in list(graph[a]):
                if s[b]['name'].startswith('core::ptr::drop_glue<core::option::Option<'):
                    empty_option(a,b,'Account Drop实现已take字段，局部真实parent/funding析构未剪')
        if name=='core::ptr::drop_glue<kernel::maintenance::AccountInner>':
            for b in list(graph[a]):
                if 'drop_glue<core::option::Option<alloc::sync::Arc<' in s[b]['name']:empty_option(a,b,'最终Account quiescent/head/ring空')
    # Shell Body必须先drain VM/Views/Members；只为该最后字段创建空对象上下文。
    body='core::ptr::drop_glue<kernel::task::process::body::ProcessBody>'
    for a in by_name.get(body,[]):
        for b in list(graph[a]):
            if 'Arc<kernel::vm::change::VmShell>' in s[b]['name'] or 'Arc<kernel::vm::views::Views>' in s[b]['name']:
                empty=context(a,b,'body-drained','Body access=0及VM退休完成后才销毁这些字段，费用仍必须退款')
                # 字段的真实退款、存储释放与最后析构完整保留。
    roots=[one('kernel::maintenance::service')]
    def closure(entries=None):
        result=set();todo=list(roots if entries is None else entries)
        while todo:
            a=todo.pop()
            if a in result:continue
            result.add(a);todo.extend(graph[a]-result)
        return result
    def formatter(name):
        return name.startswith('core::fmt::') or ' as core::fmt::' in name
    fmt_targets=set();fmt_arguments={};fmt_missing=[];fmt_terminal_refs=set()
    while True:
        reachable=closure();fmt_context=closure(fmt_targets);found=set()
        for a in reachable:
            row=s[a]
            constructor='core::fmt::rt::Argument' in row['name'] and '::new_' in row['name']
            references=set(row.get('constant_functions',())) if constructor or a in fmt_context else set()
            for values in row.get('constant_vtables',{}).values():
                for b in values:
                    if constructor or a in fmt_context or ('core::fmt::Write' in s[b]['name']):
                        references.add(b)
                    elif formatter(s[b]['name']):
                        fmt_terminal_refs.add((row['name'],s[b]['name']))
            callbacks={b for b in references if b in s and formatter(s[b]['name'])}
            found.update(callbacks)
            if constructor:
                fmt_arguments[row['name']]=sorted(s[b]['name'] for b in callbacks)
                if not callbacks and row['name'] not in fmt_missing:fmt_missing.append(row['name'])
        new=found-fmt_targets
        if not new:break
        fmt_targets.update(new)
        for row in fmt_sites:
            graph[row['caller_address']].update(fmt_targets)
    reachable=closure()
    missing=[dict(address=a,name=s[a]['name']) for a in reachable if 'frame' not in s[a] or s[a].get('unsupported_cfi')]
    open_sites=[r for r in unresolved if r['caller_address'] in reachable]
    open_direct=[r for r in facts['unresolved_direct'] if r['caller'] in reachable]
    live_graph={a:graph[a]&reachable for a in reachable}
    cycles=[]
    for group in components(live_graph):
        if len(group)>1 or group[0] in live_graph[group[0]]:
            cycles.append(dict(nodes=group,names=[s[a]['name'] for a in group],frames=sum(s[a].get('frame',0) for a in group)))
    bound=cumulative(facts,s,graph,reachable,roots,fmt_targets)
    blocked=bool(open_sites or open_direct or missing or fmt_missing or bound['unknown'])
    if not blocked and any(value>bound['formal_stack_bytes'] for value in bound['bounds'].values()):
        blocked=True;bound['unknown'].append(['cumulative bound exceeds linked formal stack'])
    if blocked:bound['bounds']={}
    return dict(elf=facts['elf'],sha256=facts['sha256'],roots=roots,reachable=len(reachable),cuts=cuts,
        open_sites=open_sites,open_direct=open_direct,missing_cfi=missing,cycles=cycles,
        format_arguments=fmt_arguments,format_targets=sorted(s[a]['name'] for a in fmt_targets),format_missing=fmt_missing,
        format_terminal_references=sorted(fmt_terminal_refs),
        cumulative=bound,status='UNRESOLVED' if blocked else 'VERIFIED',
        scope='virt/debug正常M service及全部生产Work；包含真实有界递归和boot/secondary入口；panic/fatal/平台终态报告与WCET不在结论内'),s,graph,reachable


def main():
    sys.setrecursionlimit(50000)
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('facts',type=Path);parser.add_argument('sites',type=Path);parser.add_argument('output',type=Path)
    parser.add_argument('--receivers',type=Path)
    parser.add_argument('--certificate',type=Path)
    args=parser.parse_args()
    receiver=json.loads(args.receivers.read_text()) if args.receivers else None
    result,s,graph,reachable=build(json.loads(args.facts.read_text()),json.loads(args.sites.read_text()),receiver)
    if args.certificate:
        try:
            validate_certificate(args.certificate,args,result)
        except (ValueError,KeyError,OSError) as error:
            result['status']='UNRESOLVED'
            result['cumulative']['bounds']={}
            result['cumulative']['unknown'].append(['certificate rejected: '+str(error)])
    elif result['status']=='VERIFIED':
        result['status']='UNSIGNED'
        result['cumulative']['bounds']={}
    args.output.write_text(json.dumps(result,indent=2,ensure_ascii=False)+'\n')
    print(result['status'],'reachable',result['reachable'],'open_sites',len(result['open_sites']),'open_direct',len(result['open_direct']),'missing_CFI',len(result['missing_cfi']),'SCC',len(result['cycles']))
    for row in result['open_sites']:print('OPEN',row['site'],row['caller'],row['instruction'])
    for row in result['cycles']:print('CYCLE',len(row['nodes']),row['frames'],'; '.join(row['names'][:5]))
    print('Bounds:',result['cumulative']['bounds'],'linked formal stack:',result['cumulative']['formal_stack_bytes'])
    for reason in result['cumulative']['unknown']:print('UNRESOLVED',reason)
    if result['status']!='VERIFIED':sys.exit(1)


if __name__=='__main__':main()
