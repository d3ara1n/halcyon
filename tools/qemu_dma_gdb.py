"""原DMA保留验证的GDB观察体，供core reset拒绝与真实设备管理故障共用。"""
import subprocess
import re
import qemu_gdb as support

SETUP = r'''import gdb
gdb.execute("set language rust")
injected = False
watch_seen = False
parked = set()
before = None
failure = None
__PHYSICAL_READ__

def require(test, message):
    if not test:
        raise gdb.GdbError(message)

def variant(value):
    return next((f.name for f in value.type.fields() if f.name), None)

def optional(value):
    name = variant(value)
    if name == 'None':
        return None
    require(name == 'Some', 'unexpected Option debug layout: '+str(value.type))
    inner = value['Some']
    field = next(f.name for f in inner.type.fields() if f.name)
    return inner[field]

def pointer(value):
    for _ in range(8):
        if value.type.strip_typedefs().code == gdb.TYPE_CODE_PTR:
            return value
        names = [f.name for f in value.type.fields()]
        name = next((name for name in ('ptr', 'pointer', '__0', '0') if name in names), None)
        require(name is not None, 'unexpected pointer debug layout: '+str(value.type))
        value = value[name]
    raise gdb.GdbError('pointer wrapper nesting exceeded')

def arc(value):
    return pointer(value).dereference()['data']

def table_values(table):
    stack = [table['root']]
    count = 0
    while stack:
        owned = optional(stack.pop())
        if owned is None:
            continue
        node = pointer(owned['data']).dereference()
        yield node['value']
        stack.extend([node['left'], node['right']])
        count += 1
        require(count <= int(table['len']), 'table traversal exceeded its node count')
    require(count == int(table['len']), 'table traversal omitted nodes')

def retained(stage):
    # 此时所有 hart 已在正式环境；全部用户/内核页表共享同一高半区直映射。
    table = gdb.parse_and_eval('kernel::resource::dma::mapping::RETAINED')['data']['value']
    require(int(table['len']) > 0 if DEVICE_FAULT else int(table['len']) == 2, 'expected real exposed DMA mappings')
    result = []
    for handle in table_values(table):
        mapping = arc(handle)
        owned = optional(mapping['owned']['data']['value'])
        require(owned is not None, 'retained mapping has a concurrent release owner')
        usage = optional(owned['usage'])
        require(usage is not None and optional(owned['write']) is not None, 'retention lost backing use or device write permit')
        require(optional(owned['acquisition']) is None and optional(owned['release']) is None, 'exposed mapping is being acquired or retired')
        span = optional(usage['range'])
        require(span is not None, 'retained backing use lost its range')
        pages = int(span['hi']) - int(span['lo'])
        require(pages > 0 and int(mapping['bytes']) > 0 if DEVICE_FAULT else pages == 2 and int(mapping['bytes']) == 8192, 'retained mapping geometry changed')
        state = mapping['record']['inner']['value']['state']['state']
        require(variant(state) == 'Idle', 'static DMA retention became runnable maintenance debt')
        backing = arc(usage['backing'])
        require(DEVICE_FAULT or optional(backing['state']['data']['value']['root']) is None, 'MemoryObject root release is unfinished')
        require(int(backing['state']['data']['value']['acquisitions']) == 0, 'retained mapping has a preparation lease')
        result.append((int(mapping['id']), int(pointer(usage['backing'])), int(span['lo']), int(span['hi'])))
    result.sort()
    print('DMA_RETENTION stage=%s mappings=%d work=Idle ids=%s' % (stage, len(result), result))
    return result

class ResetArm(gdb.Breakpoint):
    def stop(self):
        self.enabled = False
        injection.enabled = True
        return False

class Injection(gdb.Breakpoint):
    def stop(self):
        global injected, before, failure
        if IPI_FAILURE and injected and int(gdb.parse_and_eval('$a7')) == 0x735049:
            return ipi_send()
        if int(gdb.parse_and_eval('$a7')) != 0x53525354:
            return False
        try:
            require(not DEVICE_FAULT or (int(gdb.parse_and_eval('$a0')) == 1 and int(gdb.parse_and_eval('$a1')) == 1), 'expected ColdReboot/SystemFailure')
            before = retained('before-reset-failure')
            injected = True
            gdb.execute('set $pc = $pc + 4')
            gdb.execute('set $a0 = -2')
            gdb.execute('set $a1 = 0')
            self.enabled = IPI_FAILURE
            print('DMA_RESET_INJECT: SBI_ERR_NOT_SUPPORTED')
            return False
        except Exception as error:
            failure = str(error)
            return True

class StopRequest(gdb.Breakpoint):
    def stop(self):
        global watch_seen, failure
        if not injected:
            return False
        frame = gdb.newest_frame()
        names = []
        while frame:
            name = frame.name() or ''
            names.append(name)
            if 'InitialWatch' in name or name.startswith('kernel::boot::watch::'):
                break
            frame = frame.older()
        if frame is None:
            print('DMA_STOP_REQUEST_OTHER:', names)
            return False
        try:
            require(retained('initial-DONE') == before, 'initial process exit changed retained pins')
            watch_seen = True
            print('DMA_INITIAL_DONE_OBSERVED')
            if IPI_FAILURE:
                ipi_request_started()
            return False
        except Exception as error:
            failure = str(error)
            return True

class Parking(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            require(injected and watch_seen, 'runtime parked before initial DONE observation')
            require(physical_word(__REQ__, 1) == 1 and physical_word(__STOP__, 1) == 1, 'park preceded runtime-stop publication')
            parked.add(gdb.selected_thread().num)
            if len(parked) == 4:
                require(retained('all-parked') == before, 'park propagation changed retained pins')
                return True
            return False
        except Exception as error:
            failure = str(error)
            return True

injection = Injection('*__ECALL__', type=gdb.BP_HARDWARE_BREAKPOINT)
injection.enabled = False
ResetArm('*__RESET__', type=gdb.BP_HARDWARE_BREAKPOINT)
StopRequest('*__REQUEST__', type=gdb.BP_HARDWARE_BREAKPOINT)
Parking('*__PARK__', type=gdb.BP_HARDWARE_BREAKPOINT)'''
FINISH = r'''require(failure is None, failure or 'retention check failed')
require(injected and watch_seen and len(parked) == 4, 'incomplete DMA terminal evidence')
print('DMA_RETENTION_PASSED: real exposed pins survive reset failure, initial DONE and all 4 terminal parks')
'''


def program(kernel, symbols, device_fault=False, ipi_failure=False):
    start=symbols['kernel::sbi::sbi_call']
    end=min(value for value in symbols.values() if value > start)
    asm=subprocess.check_output(['riscv64-elf-objdump','-d',f'--start-address={start}',f'--stop-address={end}',str(kernel)],text=True)
    ecalls=re.findall(r'^\s*([0-9a-f]+):\s+00000073\s+ecall',asm,re.M)
    if len(ecalls)!=1:raise RuntimeError('fixed SBI wrapper must contain one ecall')
    replacements={
        'ECALL':'0x'+ecalls[0],
        'RESET':hex(symbols['kernel::sbi::system_reset']),
        'REQUEST':hex(symbols['kernel::runtime_stop::request']),
        'PARK':hex(symbols['kernel::hart::park']),
        'REQ':hex(support.physical_address(kernel,symbols['kernel::runtime_stop::REQUESTED'])),
        'STOP':hex(support.physical_address(kernel,symbols['kernel::runtime_stop::STOPPED'])),
        'PHYSICAL_READ':support.PHYSICAL_READ,
    }
    setup='DEVICE_FAULT='+repr(device_fault)+'\nIPI_FAILURE='+repr(ipi_failure)+'\n'+SETUP
    for key,value in replacements.items():setup=setup.replace('__'+key+'__',value)
    finish = FINISH
    if ipi_failure:
        import qemu_dma_ipi_gdb
        extra, after = qemu_dma_ipi_gdb.program(kernel, symbols, int(ecalls[0], 16))
        setup += '\n' + extra
        finish += '\n' + after
    return setup, finish
