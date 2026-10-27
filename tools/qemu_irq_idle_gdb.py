"""原WFI点同一快照的四hart只读事实，暂停期间QMP真插入后核同源领取。"""
CODE = r'''
def irq_integer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_INT,gdb.TYPE_CODE_BOOL):
        names=[f.name for f in value.type.fields() if f.name]
        name=next((n for n in ('v','value','__0','0','inner') if n in names),None)
        if name is None and len(names)==1:name=names[0]
        if name is None:raise gdb.GdbError('unexpected atomic wrapper: '+str(value.type))
        value=value[name]
    return int(value)
def irq_global(name):return gdb.parse_and_eval("'"+name+"'")
wfi_pc=min(s.pc for s in gdb.decode_line('kernel/src/sched.rs:694')[1])
if physical_word(wfi_pc-DIRECT_DELTA,4)!=0x10500073:raise gdb.GdbError('idle checkpoint is not actual WFI instruction')
# 不把断点放在WFI本身：GDB的断点step-over可能在只恢复一hart时执行阻塞WFI。
# 只选机器码里原检查/装填后直接跳WFI的非阻塞j，不改PC、CSR或step标志。
idle_start=IDLE_START
response=gdb.execute('maintenance packet Qqemu.PhyMemMode:1',to_string=True)
if 'OK' not in response:raise gdb.GdbError('physical instruction inspection unavailable')
try:
    instructions=gdb.selected_inferior().architecture().disassemble(idle_start-DIRECT_DELTA,wfi_pc-DIRECT_DELTA)
finally:gdb.execute('maintenance packet Qqemu.PhyMemMode:0',to_string=True)
idle_jumps=[]
for instruction in instructions:
    branch=re.match(r'^(?:c\.)?j\s+(0x[0-9a-f]+)',instruction['asm'])
    if branch and int(branch[1],16)==wfi_pc-DIRECT_DELTA:
        idle_jumps.append(instruction['addr']+DIRECT_DELTA)
if not idle_jumps:raise gdb.GdbError('no actual nonblocking post-check jump to WFI')
probe_jumps=idle_jumps
facts['irq_idle_code']={'wfi':hex(wfi_pc),'post_check_jumps':[hex(pc) for pc in idle_jumps],'probe_jumps':[hex(pc) for pc in probe_jumps],'step_mask':gdb.execute('maintenance packet qqemu.sstep',to_string=True)}
irq_idle_pending=False
irq_idle_quiet=None
irq_idle_layout=None
def irq_idle_continue():
    # 只由宿主请求普通all-stop；期间没有j断点，不截断每次idle/RPC推进。
    # 后台线程不调用GDB API，也不触客体；信号仅送当前调试器。
    if irq_idle_quiet is None or 'irq_idle' in facts:
        return gdb.execute('continue',to_string=True)
    import os,signal,threading
    running=threading.Event()
    def interrupt():
        global irq_idle_pending
        if not running.is_set():return
        irq_idle_pending=True
        os.kill(os.getpid(),signal.SIGINT)
    timer=threading.Timer(.2,interrupt)
    running.set()
    with gdb.blocked_signals():timer.start()
    try:
        try:return gdb.execute('continue',to_string=True)
        except KeyboardInterrupt:
            if not irq_idle_pending:raise
            return ''
    finally:
        running.clear()
        timer.cancel()
        timer.join()
def irq_idle_miss(reason,detail=None):
    counts=facts.setdefault('irq_idle_rejections',{})
    counts[reason]=counts.get(reason,0)+1
    if counts[reason]<=3:
        print('IRQ_IDLE_NOT_YET '+json.dumps({'reason':reason,'detail':detail},sort_keys=True))
        EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
    return False
def irq_hart_state(hartid,domain_address):
    # 仅在原kernel-idle hart选中时调用；返回值完全materialize，不携lazy gdb.Value跨switch。
    locals_=irq_global('kernel::hart::HART_LOCALS')
    lo,hi=locals_.type.range()
    native=next(locals_[index] for index in range(lo,hi+1) if irq_integer(locals_[index]['emergency_sp']) and irq_integer(locals_[index]['hartid'])==hartid)
    slot=irq_integer(native['slot'])
    current=irq_integer(native['current_thread'])
    pid=None
    if current:
        shell=word(current+process_offset+ptr_offset+pointer_offset)
        pid=word(shell+data_offset+pid_offset)
    result={'slot':slot,'native':int(native.address),'current_thread':current,'pid':pid}
    if domain_address is not None:
        pending=irq_global('kernel::sched::HART_TIMER_PENDING')
        inboxes=irq_global('kernel::vm::remote::INBOXES')
        domain=gdb.Value(domain_address).cast(gdb.lookup_type('kernel::sched::SchedDomain').pointer()).dereference()
        storage=gdb.Value(int(domain['classes'][0]['pointer'])).cast(gdb.lookup_type('kernel::sched::FairClass').pointer()).dereference()['ready']['data']['value']['storage']
        result.update(idle_mask=irq_integer(domain['idle_mask']),ready_len=int(storage['len']),timer_pending=bool(irq_integer(pending[slot])),remote_head=int(inboxes[slot]['data']['value']['head']))
        queue=irq_global('kernel::sched::HART_TIMERS')[slot]['data']['value']
        first=queue['first']
        result['timer_first']=None if int(first)==0 else int(first.dereference()['links']['value']['deadline'])
    return result
def irq_idle_current():
    # 同一全停下先读原HartLocal数组，一次物理包排除仍在执行用户工作的候选。
    # 这只是廉价拒绝门；通过后仍完整核每CPU的真实PC/CSR/domain/debt，不累加旧样本。
    global irq_idle_layout
    if irq_idle_layout is None:
        value=irq_global('kernel::hart::HART_LOCALS')
        maintenance=irq_global('kernel::maintenance::QUEUE')['data']['value']['ready']
        irq_idle_layout=(int(value.address)-DIRECT_DELTA,value.type.sizeof,hart_type.sizeof,int(maintenance.address)-DIRECT_DELTA)
    address,size,stride,maintenance=irq_idle_layout
    ready=physical_word(maintenance)
    if ready:return irq_idle_miss('maintenance',ready)
    raw=physical_word(address,size).to_bytes(size,'little')
    for offset in range(0,size,stride):
        emergency=struct.unpack_from('<Q',raw,offset+hart_offsets['emergency_sp'])[0]
        current=struct.unpack_from('<Q',raw,offset+hart_offsets['current_thread'])[0]
        if emergency and current:
            hartid=struct.unpack_from('<Q',raw,offset+hart_offsets['hartid'])[0]
            shell=word(current+process_offset+ptr_offset+pointer_offset)
            return irq_idle_miss('current-precheck',{'hartid':hartid,'pid':word(shell+data_offset+pid_offset)})
    return True
def irq_idle_snapshot():
    global failure
    original=gdb.selected_thread()
    try:
        target=irq_target()
        if target is None or target['round']!=1 or 'irq_idle' in facts:return False
        facts['irq_idle_snapshots']=facts.get('irq_idle_snapshots',0)+1
        if facts['irq_idle_snapshots']%32==0:EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
        # time与队列以这一次全停的首个采样时点比较；不拿后续调试耗时评价旧队列。
        sampled_ticks=int(gdb.parse_and_eval('$time'))&((1<<64)-1)
        threads=list(gdb.selected_inferior().threads())
        if len(threads)!=4:raise gdb.GdbError('all-idle fixture requires four actual vCPUs')
        samples=[]
        for cpu in threads:
            if not cpu.is_stopped():raise gdb.GdbError('idle sampling did not stop every actual CPU')
            cpu.switch()
            gdb.execute('maintenance flush register-cache')
            samples.append({'hartid':int(gdb.parse_and_eval('$mhartid')),'pc':int(gdb.parse_and_eval('$pc'))&((1<<64)-1)})
        captured=facts.setdefault('irq_idle_progress',[])
        if len(captured)<12:
            # HartLocal地址/布局由首次真实kernel-idle帧已固定；全物理读取，不用用户tp。
            address,size,stride,_=irq_idle_layout
            raw=physical_word(address,size).to_bytes(size,'little')
            current={}
            for offset in range(0,size,stride):
                if not struct.unpack_from('<Q',raw,offset+hart_offsets['emergency_sp'])[0]:continue
                hartid=struct.unpack_from('<Q',raw,offset+hart_offsets['hartid'])[0]
                thread=struct.unpack_from('<Q',raw,offset+hart_offsets['current_thread'])[0]
                value={'thread':thread}
                if thread:
                    shell=word(thread+process_offset+ptr_offset+pointer_offset)
                    saved=thread+thread_offsets['frame']
                    value.update(pid=word(shell+data_offset+pid_offset),saved_sepc=word(saved+context_offsets['sepc']),saved_a7=word(saved+context_offsets['x']+17*8),saved_a0=word(saved+context_offsets['x']+10*8))
                current[hartid]=value
            progress={'sampled_ticks':sampled_ticks,'cpus':[{**sample,**current[sample['hartid']]} for sample in samples]}
            captured.append(progress)
            EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
            print('IRQ_IDLE_PROGRESS '+json.dumps(progress,sort_keys=True))
        eligible=(*idle_jumps,wfi_pc,wfi_pc+4)
        if any(sample['pc'] not in eligible for sample in samples):
            return irq_idle_miss('pc',samples)
        original.switch()
        gdb.execute('maintenance flush register-cache')
        original_satp=int(gdb.parse_and_eval('$satp'))&((1<<64)-1)
        maintenance=int(irq_global('kernel::maintenance::QUEUE')['data']['value']['ready'])
        if maintenance!=0:return irq_idle_miss('maintenance',maintenance)
        if not irq_idle_current():return False
        harts=[]
        for cpu in threads:
            cpu.switch()
            gdb.execute('maintenance flush register-cache')
            tp=int(gdb.parse_and_eval('$tp'))&((1<<64)-1)
            pc=int(gdb.parse_and_eval('$pc'))&((1<<64)-1)
            hartid=int(gdb.parse_and_eval('$mhartid'))
            sip=int(gdb.parse_and_eval('$sip'))&((1<<64)-1)
            sie=int(gdb.parse_and_eval('$sie'))&((1<<64)-1)
            # 只有已在kernel idle的帧才取本地domain指针；不在用户页表下懒读内核global。
            domain_address=int(gdb.newest_frame().read_var('domain')) if pc in (*idle_jumps,wfi_pc,wfi_pc+4) else None
            original.switch()
            gdb.execute('maintenance flush register-cache')
            # GDB默认同inferior共用地址空间；强制寄存器读取令RSP general thread也回原kernel CPU。
            if int(gdb.parse_and_eval('$satp'))&((1<<64)-1)!=original_satp:raise gdb.GdbError('kernel inspection context changed')
            state=irq_hart_state(hartid,domain_address)
            if pc not in (*idle_jumps,wfi_pc,wfi_pc+4):
                return irq_idle_miss('pc',{'hartid':hartid,'pc':hex(pc),'current_thread':hex(state['current_thread']),'pid':state['pid']})
            if tp!=state['native']:return irq_idle_miss('tp',{'slot':state['slot'],'tp':hex(tp),'native':hex(state['native'])})
            if state['current_thread']!=0:return irq_idle_miss('current',hex(tp))
            slot=state['slot']
            if sip&0x222 or sie&0x222!=0x222:return irq_idle_miss('interrupts',{'slot':slot,'sip':sip,'sie':sie})
            if state['timer_pending'] or state['remote_head']!=0:return irq_idle_miss('debt',{'slot':slot,'timer_pending':state['timer_pending'],'remote_head':state['remote_head']})
            if state['timer_first'] is not None and state['timer_first']<=sampled_ticks:return irq_idle_miss('timer-due',{'slot':slot,'first':state['timer_first'],'sampled_ticks':sampled_ticks})
            idle=state['idle_mask']
            if not idle&(1<<slot):return irq_idle_miss('idle-mask',{'slot':slot,'idle_mask':idle})
            if state['ready_len']!=0:return irq_idle_miss('ready',state['ready_len'])
            harts.append({'slot':slot,'hartid':hartid,'pc':hex(pc),'current_thread':0,'sip':sip,'sie':sie,'idle_mask':idle,'ready_len':0,'timer_pending':False,'timer_first':state['timer_first'],'remote_head':0,'gdb_thread':cpu.global_num})
        if {h['slot'] for h in harts}!={0,1,2,3}:raise gdb.GdbError('idle snapshot did not cover all admitted slots')
        original.switch()
        gdb.execute('maintenance flush register-cache')
        if int(gdb.parse_and_eval('$satp'))&((1<<64)-1)!=original_satp:raise gdb.GdbError('kernel source inspection context changed')
        controller=irq_global('kernel::resource::irq::controller::CONTROLLER')['data']['value']['Some']['__0']
        source,slot=target['source'],target['target']
        context=int(controller['contexts'][slot])
        base=int(controller['base'])-DIRECT_DELTA
        pending_at=base+0x1000+(source//32)*4
        enabled_at=base+0x2000+context*0x80+(source//32)*4
        threshold_at=base+0x200000+context*0x1000
        before={'pending':bool(physical_word(pending_at,4)&(1<<(source%32))),'enabled':bool(physical_word(enabled_at,4)&(1<<(source%32))),'priority':physical_word(base+source*4,4),'threshold':physical_word(threshold_at,4)}
        if before['pending']:return irq_idle_miss('source-pending',before)
        if not before['enabled'] or before['priority']<=before['threshold']:raise gdb.GdbError('actual idle IRQ route is not enabled')
        observation={'harts':harts,'sampled_ticks':sampled_ticks,'maintenance_ready':0,'kernel_inspection_satp':hex(original_satp),'wfi_pc':hex(wfi_pc),'post_check_jumps':[hex(pc) for pc in idle_jumps],'source':source,'target_slot':slot,'context':context,'generation':target['generation'],'before':before,'scope':'same snapshot at original post-check jump/WFI after uninterrupted execution, not inferred from debugger stop; future timers allowed'}
        IRQ_IDLE_READY.write_text(json.dumps(observation,indent=2)+'\n')
        # 仅外部操作者握手，QEMU CPU仍暂停；不改产品时钟、调度或ready事实。
        end=time.monotonic()+10
        while not IRQ_IDLE_INSERTED.exists() and time.monotonic()<end:time.sleep(.01)
        if not IRQ_IDLE_INSERTED.exists():raise gdb.GdbError('QMP insertion did not complete during idle snapshot')
        after=[]
        for cpu in threads:
            cpu.switch()
            gdb.execute('maintenance flush register-cache')
            cpu_slot=next(h['slot'] for h in harts if h['gdb_thread']==cpu.global_num)
            after.append({'slot':cpu_slot,'sip':int(gdb.parse_and_eval('$sip'))&((1<<64)-1)})
        original.switch()
        gdb.execute('maintenance flush register-cache')
        if int(gdb.parse_and_eval('$satp'))&((1<<64)-1)!=original_satp:raise gdb.GdbError('kernel post-insertion context changed')
        pending_after=bool(physical_word(pending_at,4)&(1<<(source%32)))
        target_sip=next(h['sip'] for h in after if h['slot']==slot)
        if not pending_after or not target_sip&0x200:raise gdb.GdbError('real insertion did not assert target SEIP/pending')
        observation.update(after_qmp={'pending':pending_after,'harts':after},timer_coincident_after_qmp=any(h['sip']&0x20 for h in after))
        facts['irq_idle']=observation
        EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
        print('IRQ_ALL_IDLE_INSERTION '+json.dumps(observation,sort_keys=True))
        for breakpoint in irq_idle_bps:breakpoint.enabled=False
        for breakpoint,enabled in irq_idle_quiet:breakpoint.enabled=enabled
        irq_claim.enabled=True
        return False
    except Exception as error:
        failure=str(error)
        return True
    finally:
        original.switch()
class IrqIdle(gdb.Breakpoint):
    def stop(self):
        global irq_idle_pending,irq_idle_quiet,failure
        try:
            target=irq_target()
            if target is None or target['round']!=1 or 'irq_idle' in facts:return False
            if irq_idle_quiet is None:
                if active or pending is not None:raise gdb.GdbError('idle sampling overlaps an unfinished selected function send')
                irq_idle_quiet=[(breakpoint,breakpoint.enabled) for breakpoint in gdb.breakpoints() if breakpoint not in (*irq_idle_bps,irq_claim)]
                for breakpoint,_ in irq_idle_quiet:breakpoint.enabled=False
                irq_idle_current() # 只固定当轮kernel上下文中的DWARF地址，当前拒绝不授予任何事实。
                for breakpoint in irq_idle_bps:breakpoint.enabled=False
                facts['irq_idle_sampling']={'target':target,'host_run_interval_seconds':.2,'disabled_breakpoints':[breakpoint.number for breakpoint,enabled in irq_idle_quiet if enabled]}
                print('IRQ_IDLE_SAMPLING '+json.dumps(facts['irq_idle_sampling'],sort_keys=True))
            irq_idle_pending=True
            return True
        except Exception as error:
            failure=str(error)
            return True
class IrqClaim(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            idle=facts.get('irq_idle')
            if idle is None or 'claim' in idle:return False
            frame=gdb.newest_frame()
            source,target=int(frame.read_var('source')),int(frame.read_var('target'))
            if (source,target)!=(idle['source'],idle['target_slot']):return False
            # source是原service真正读claim得到的局部值；探针不读取claim寄存器。
            line=irq_arc(frame.read_var('line'))
            state=irq_service(line)
            if state['generation']!=idle['generation']:raise gdb.GdbError('idle insertion was claimed by another IRQ generation')
            sip=int(frame.read_var('sip'))
            idle['claim']={'source':source,'slot':target,'sip_at_service_entry':sip,'timer_coincident':bool(sip&0x20),'generation':state['generation'],'sequence_before':state['sequence']}
            print('IRQ_ALL_IDLE_REAL_CLAIM '+json.dumps(idle['claim'],sort_keys=True))
            EVIDENCE.write_text(json.dumps(facts,indent=2)+'\n')
            self.enabled=False
            return False
        except Exception as error:
            failure=str(error)
            return True
irq_idle_bps=[IrqIdle('*%#x'%pc,type=gdb.BP_HARDWARE_BREAKPOINT) for pc in probe_jumps]
for breakpoint in irq_idle_bps:
    breakpoint.enabled=False
    breakpoint.silent=True
irq_claim=IrqClaim('kernel/src/resource/irq/controller.rs:154',type=gdb.BP_HARDWARE_BREAKPOINT)
irq_claim.enabled=False
'''
