"""嵌入原function只读GDB会话，核真实PLIC在途关闭及跨代Ack，不读claim寄存器。"""
CODE = '''
irq_ack=None
irq_close=None
irq_close_write=False
irq_close_disabled=False
irq_last_call=None
def irq_name(frame):return (frame.name() or '').replace('<','').replace('>','')
def irq_caller(name):
    frame=gdb.newest_frame()
    while frame:
        if irq_name(frame)==name:return frame
        frame=frame.older()
    return None
def irq_arc(value):
    if value.type.strip_typedefs().code==gdb.TYPE_CODE_PTR:value=value.dereference()
    while value.type.strip_typedefs().code!=gdb.TYPE_CODE_PTR:
        names=[f.name for f in value.type.fields() if f.name]
        value=value[next(n for n in ('ptr','pointer','__0','0') if n in names)]
    return value.dereference()['data']
def irq_target():
    if IRQ_TARGET is None or not IRQ_TARGET.exists():return None
    return json.loads(IRQ_TARGET.read_text())
def irq_service(line):
    state=line['state']['value']
    return {key:(bool(state[key]) if key in ('closed','in_service','taken') else int(state[key])) for key in ('generation','sequence','closed','in_service','taken')}
def irq_verified():
    return irq_close_disabled and all(key in facts for key in ('irq_old_closed','irq_new_mismatch','irq_new_ack'))
class IrqClose(gdb.Breakpoint):
    def stop(self):
        global irq_close,failure
        try:
            if irq_close is not None:return False
            target=irq_target()
            if target is None or target['round']!=1:return False
            frame=gdb.newest_frame()
            line=irq_arc(frame.read_var('self'))
            if (int(line['source']),int(line['target']))!=(target['source'],target['target']):return False
            state=irq_service(line)
            if state['generation']!=target['generation']:raise gdb.GdbError('closing another IRQ generation')
            if not bool(frame.read_var('owned')) or not bool(frame.read_var('outstanding')):raise gdb.GdbError('real IRQ close did not retain an owned in-service claim')
            irq_close={**state,'source':int(line['source']),'target':int(line['target']),'outstanding':True,'complete_calls':0}
            facts['irq_close']=irq_close
            print('IRQ_TAKEN_CLOSE_OBSERVED '+json.dumps(irq_close,sort_keys=True))
            return False
        except Exception as error:
            failure=str(error)
            return True
class IrqAck(gdb.Breakpoint):
    def stop(self):
        global irq_ack,failure
        try:
            if irq_close is None:return False
            frame=gdb.newest_frame()
            line=frame.read_var('self').dereference()
            if (int(line['source']),int(line['target']))!=(irq_close['source'],irq_close['target']):return False
            token=frame.read_var('token')
            generation,sequence=int(token['generation']),int(token['sequence'])
            state=irq_service(line)
            if generation==irq_close['generation']:
                if state['generation']==generation:
                    if not state['closed']:raise gdb.GdbError('old IRQ alias was not actually closed')
                    kind='irq_old_closed'
                    expected=0x38
                else:
                    if state['closed'] or not state['in_service'] or not state['taken']:raise gdb.GdbError('old token was not tested against a real taken new claim')
                    kind='irq_new_mismatch'
                    expected=3
            elif 'irq_new_mismatch' in facts and state['generation']==generation and sequence==facts['irq_new_mismatch']['sequence'] and 'irq_new_ack' not in facts:
                if state['closed'] or not state['in_service'] or not state['taken']:raise gdb.GdbError('positive new Ack lost the original claim')
                kind='irq_new_ack'
                expected=0
            else:return False
            native=irq_caller('kernel::resource::irq::object::ack')
            if native is None:raise gdb.GdbError('Ack is not the real authorized syscall')
            thread=int(native.read_var('thread'))
            shell=word(thread+process_offset+ptr_offset+pointer_offset)
            if word(shell+data_offset+pid_offset)!=1:raise gdb.GdbError('IRQ Ack is not the actual init consumer')
            kernel_frame=thread+thread_offsets['frame']
            irq_ack={**state,'kind':kind,'token_generation':generation,'token_sequence':sequence,'frame':kernel_frame,'pc':word(kernel_frame+context_offsets['sepc']),'line':int(line.address),'expected':expected,'complete_calls':0}
            irq_ok.enabled=True
            irq_error.enabled=True
            return False
        except Exception as error:
            failure=str(error)
            return True
class IrqComplete(gdb.Breakpoint):
    def stop(self):
        global irq_last_call,failure
        try:
            if irq_close is None:return False
            frame=gdb.newest_frame()
            source,target=int(frame.read_var('source')),int(frame.read_var('target'))
            if (source,target)!=(irq_close['source'],irq_close['target']):return False
            controller=frame.read_var('self').dereference()
            context=int(controller['contexts'][target])
            irq_last_call={'offset':0x200004+context*0x1000,'source':source,'context':context}
            if irq_caller('kernel::resource::irq::controller::Line::close') is not None:
                if irq_close_disabled: return False
                irq_close['complete_calls']+=1
                if irq_close['complete_calls']!=1:raise gdb.GdbError('close completed original claim more than once')
                irq_close['context']=context
            elif irq_ack is not None:
                if irq_ack['kind']!='irq_new_ack':raise gdb.GdbError('rejected old Ack performed hardware complete')
                irq_ack['complete_calls']+=1
            return False
        except Exception as error:
            failure=str(error)
            return True
class IrqWrite(gdb.Breakpoint):
    def stop(self):
        global irq_close_write,failure
        try:
            if irq_close is None or irq_close_disabled or irq_last_call is None:return False
            frame=gdb.newest_frame()
            if int(frame.read_var('offset'))==irq_last_call['offset'] and int(frame.read_var('value'))==irq_close['source']:
                if irq_caller('kernel::resource::irq::controller::Line::close') is not None:
                    irq_close_write=True
            return False
        except Exception as error:
            failure=str(error)
            return True
class IrqEnable(gdb.Breakpoint):
    def stop(self):
        global irq_close_disabled,failure
        try:
            if irq_close is None or irq_close_disabled:return False
            frame=gdb.newest_frame()
            if (int(frame.read_var('source')),int(frame.read_var('target')))!=(irq_close['source'],irq_close['target']) or bool(frame.read_var('enabled')):return False
            if irq_caller('kernel::resource::irq::controller::Line::close') is None:return False
            if not irq_close_write or irq_close['complete_calls']!=1:raise gdb.GdbError('disable preceded real close complete write/return')
            # complete->write已经返回，此处才提交disable，不读有副作用的PLIC claim/complete寄存器。
            irq_close_disabled=True
            facts['irq_complete_before_disable']=True
            print('IRQ_COMPLETE_BEFORE_DISABLE '+json.dumps(irq_close,sort_keys=True))
            return False
        except Exception as error:
            failure=str(error)
            return True
class IrqResult(gdb.Breakpoint):
    def __init__(self,name,success):
        self.success=success
        super().__init__(name,type=gdb.BP_HARDWARE_BREAKPOINT)
    def stop(self):
        global irq_ack,failure
        try:
            if irq_ack is None:return False
            frame=gdb.newest_frame()
            if int(frame.read_var('frame'))!=irq_ack['frame'] or word(irq_ack['frame']+context_offsets['sepc'])!=irq_ack['pc']:return False
            error=0 if self.success else int(frame.read_var('err'))
            if error!=irq_ack['expected']:raise gdb.GdbError('real IRQ Ack returned unexpected status')
            if irq_ack['complete_calls']!=(1 if self.success else 0):raise gdb.GdbError('Ack hardware completion count mismatch')
            if irq_ack['kind']=='irq_new_mismatch':
                line=gdb.Value(irq_ack['line']).cast(gdb.lookup_type('kernel::resource::irq::controller::Line').pointer()).dereference()
                state=irq_service(line)
                if state['generation']!=irq_ack['generation'] or state['sequence']!=irq_ack['sequence'] or not state['in_service'] or not state['taken']:raise gdb.GdbError('rejected old token changed original new claim')
            facts[irq_ack['kind']]=irq_ack.copy()
            print('IRQ_ACK_VERIFIED '+json.dumps(irq_ack,sort_keys=True))
            irq_ack=None
            irq_ok.enabled=False
            irq_error.enabled=False
            return False
        except Exception as error:
            failure=str(error)
            return True
IrqClose('kernel/src/resource/irq/controller.rs:238',type=gdb.BP_HARDWARE_BREAKPOINT)
IrqAck("'<kernel::resource::irq::controller::Line>::ack'",type=gdb.BP_HARDWARE_BREAKPOINT)
IrqComplete("'<kernel::resource::irq::controller::Controller>::complete'",type=gdb.BP_HARDWARE_BREAKPOINT)
IrqWrite("'<kernel::resource::irq::controller::Controller>::write'",type=gdb.BP_HARDWARE_BREAKPOINT)
IrqEnable("'<kernel::resource::irq::controller::Controller>::enable'",type=gdb.BP_HARDWARE_BREAKPOINT)
irq_ok=IrqResult("'kernel::syscall::respond_ok'",True)
irq_error=IrqResult("'kernel::syscall::respond_error'",False)
irq_ok.enabled=False
irq_error.enabled=False
'''
