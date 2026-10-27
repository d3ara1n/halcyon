"""原DMA静态保留观察中的一次request广播IPI失败；不改变产品停止协议。"""
import hashlib
import re
import subprocess

import qemu_gdb as support

SETUP = r'''
import json
ipi_observation = None
ipi_calls = {}
ipi_returns = []
ipi_rejected = []
ipi_eligibility = {'calls': 0, 'gates': {}, 'first': None, 'first_rejections': {}}
ipi_window = None
ipi_code = __CODE__
ipi_stack = __STACK__
print('DMA_IPI_CODE ' + json.dumps(ipi_code, sort_keys=True))

def ipi_stack_address(address, size, slot):
    # 对应mm::stack_layout与StackWindowLayout::formal_range/translate。
    # 只接受原发送者的formal栈；ELF的.stack_window是NOLOAD虚拟预约，不能用其p_paddr。
    require(0 <= slot < ipi_stack['slots'], 'IPI sender stack slot is invalid')
    base = ipi_stack['window'] + slot * (ipi_stack['size'] + 2 * ipi_stack['guard']) + ipi_stack['guard']
    require(base <= address and address + size <= base + ipi_stack['size'] - ipi_stack['emergency'],
            'IPI result is outside the original sender formal stack')
    return ipi_stack['physical'] + slot * ipi_stack['size'] + address - base

def ipi_result_snapshot(address, slot):
    kind = gdb.lookup_type('core::result::Result<usize, kernel::sbi::SbiError>')
    physical = ipi_stack_address(address, kind.sizeof, slot)
    response = gdb.execute('maintenance packet Qqemu.PhyMemMode:1', to_string=True)
    require('OK' in response, 'QEMU physical memory mode is unavailable')
    try:
        result = gdb.Value(physical).cast(kind.pointer()).dereference()
        tag = variant(result)
        error = None
        if tag == 'Err':
            inner = result['Err']
            error = str(inner[next(f.name for f in inner.type.fields() if f.name)])
        # 所有lazy读取在同一物理模式内完成，跨模式只交标量。
        raw = gdb.selected_inferior().read_memory(physical, kind.sizeof).tobytes().hex()
        return {'tag': tag, 'text': str(result), 'error': error, 'physical': physical, 'bytes': raw}
    finally:
        gdb.execute('maintenance packet Qqemu.PhyMemMode:0', to_string=True)

def ipi_request_started():
    global ipi_window
    require(ipi_window is None, 'initial DONE request window was entered twice')
    require(physical_word(__REQ__, 1) == 0 and physical_word(__STOP__, 1) == 0,
            'runtime stop was already published before the selected request')
    require(gdb.parameter('scheduler-locking') == 'off', 'unexpected initial debugger scheduling mode')
    # 只调度原请求者直到一次失败结果被消费，避免其他hart先提交STOPPED。
    # 不改guest停止标志或目标PC；WFI、无quantum及原调用者资格仍在ecall处核验。
    ipi_window = {'sender_thread': gdb.selected_thread().num, 'released': False}
    gdb.execute('set scheduler-locking on')
    ipi_request_end.enabled = True
    print('DMA_IPI_WINDOW_BEGIN ' + json.dumps(ipi_window, sort_keys=True))

def ipi_release_window():
    require(ipi_window is not None and not ipi_window['released'], 'IPI request window is not held')
    require(gdb.selected_thread().num == ipi_window['sender_thread'], 'IPI request window changed sender')
    gdb.execute('set scheduler-locking off')
    ipi_request_end.enabled = False
    ipi_window['released'] = True
    print('DMA_IPI_WINDOW_END ' + json.dumps(ipi_window, sort_keys=True))

class IpiRequestEnd(gdb.Breakpoint):
    def stop(self):
        global failure
        try:
            # 原request返回后即调用check；没有合格目标时结束本次有限窗口。
            ipi_release_window()
            raise gdb.GdbError('selected request ended without an eligible IPI failure')
        except Exception as error:
            failure = str(error)
            print('DMA_IPI_WINDOW_FAILED ' + json.dumps({
                'error': failure, 'eligibility': ipi_eligibility}, sort_keys=True))
            return True

def ipi_snapshot(result):
    # 仅首个候选/每个固定拒绝门采栈；只存标量，不携Frame/Value跨停点。
    stack = result['stack']
    try:
        frame = gdb.newest_frame()
    except (gdb.error, RuntimeError) as error:
        result['newest_frame_unavailable'] = {'type': type(error).__name__, 'message': str(error)}
        result['stack_truncated'] = True
        return
    while frame is not None and len(stack) < 24:
        item = {'scope': None}
        stack.append(item)
        try:
            item['name'] = frame.name()
        except (gdb.error, RuntimeError) as error:
            item['name_unavailable'] = {'type': type(error).__name__, 'message': str(error)}
        try:
            item['pc'] = int(frame.pc())
        except (gdb.error, RuntimeError) as error:
            item['pc_unavailable'] = {'type': type(error).__name__, 'message': str(error)}
        try:
            block = frame.block()
            while block is not None and block.function is None:
                block = block.superblock
            if block is None:
                item['scope'] = None
            else:
                symbol = block.function
                item['scope'] = {
                    'name': symbol.name, 'linkage_name': symbol.linkage_name,
                    'source': None if symbol.symtab is None else symbol.symtab.fullname(),
                    'objfile': None if symbol.symtab is None else symbol.symtab.objfile.filename}
        except (gdb.error, RuntimeError) as error:
            item['scope_unavailable'] = {'type': type(error).__name__, 'message': str(error)}
        try:
            frame = frame.older()
        except (gdb.error, RuntimeError) as error:
            result['older_frame_unavailable'] = {'type': type(error).__name__, 'message': str(error)}
            break
    result['stack_truncated'] = frame is not None

def ipi_backtrace(label):
    try:
        print(gdb.execute('bt', to_string=True))
    except (gdb.error, RuntimeError) as error:
        print(label + json.dumps({'type': type(error).__name__, 'message': str(error)}, sort_keys=True))

def ipi_gate(name, passed, candidate):
    counts = ipi_eligibility['gates'].setdefault(name, {'passed': 0, 'rejected': 0})
    counts['passed' if passed else 'rejected'] += 1
    candidate['gates'][name] = bool(passed)
    if not passed and name not in ipi_eligibility['first_rejections']:
        snapshot = {**candidate, 'gates': dict(candidate['gates']), 'stack': []}
        ipi_eligibility['first_rejections'][name] = snapshot
        print('DMA_IPI_FIRST_REJECT ' + json.dumps({'gate': name, 'candidate': snapshot}, sort_keys=True))
        ipi_snapshot(snapshot)
        print('DMA_IPI_FIRST_REJECT_STACK ' + json.dumps({'gate': name, 'candidate': snapshot}, sort_keys=True))
    return passed

def ipi_integer(value):
    while value.type.strip_typedefs().code not in (gdb.TYPE_CODE_INT, gdb.TYPE_CODE_BOOL):
        names = [f.name for f in value.type.fields() if f.name]
        name = next((n for n in ('v', 'value', '__0', '0', 'inner') if n in names), None)
        if name is None and len(names) == 1:
            name = names[0]
        require(name is not None, 'unexpected IPI observation atomic layout')
        value = value[name]
    return int(value)

def ipi_local(hartid):
    # 本函数只在原发送者的kernel上下文读取；不携lazy值跨CPU切换。
    harts = gdb.parse_and_eval('kernel::hart::HART_LOCALS')
    lo, hi = harts.type.range()
    native = next(harts[i] for i in range(lo, hi + 1)
                  if ipi_integer(harts[i]['emergency_sp']) and ipi_integer(harts[i]['hartid']) == hartid)
    slot = ipi_integer(native['slot'])
    timers = gdb.parse_and_eval('kernel::sched::HART_TIMERS')[slot]['data']['value']
    first = timers['first']
    return {'slot': slot, 'hartid': hartid,
            'timer_first_ticks': None if int(first) == 0 else int(first.dereference()['links']['value']['deadline']),
            'timer_pending': bool(ipi_integer(gdb.parse_and_eval('kernel::sched::HART_TIMER_PENDING')[slot])),
            'quantum_deadline_ticks': physical_word(__QUANTUM__ + slot * 8)}

def ipi_send():
    global ipi_observation, failure
    candidate = None
    try:
        ipi_eligibility['calls'] += 1
        candidate = {'sequence': ipi_eligibility['calls'], 'watch': bool(watch_seen), 'gates': {}}
        sender = gdb.selected_thread()
        candidate['sender_thread'] = sender.num
        for name, register in (('mask', '$a0'), ('base', '$a1'), ('sender_raw_hartid', '$mhartid'),
                               ('eid', '$a7'), ('fid', '$a6')):
            candidate[name] = int(gdb.parse_and_eval(register))
        candidate['pc'] = int(gdb.parse_and_eval('$pc')) & ((1 << 64) - 1)
        candidate['requested'] = physical_word(__REQ__, 1)
        candidate['stopped'] = physical_word(__STOP__, 1)
        mask, base = candidate['mask'], candidate['base']
        if ipi_eligibility['first'] is None:
            ipi_eligibility['first'] = {**candidate, 'gates': dict(candidate['gates']), 'stack': []}
            print('DMA_IPI_FIRST_CANDIDATE ' + json.dumps(ipi_eligibility['first'], sort_keys=True))
            ipi_snapshot(ipi_eligibility['first'])
            print('DMA_IPI_FIRST_STACK ' + json.dumps(ipi_eligibility['first'], sort_keys=True))
        if not ipi_gate('watch', bool(watch_seen), candidate):
            return False
        if not ipi_gate('fid', candidate['fid'] == 0, candidate):
            return False
        # 固定virt四hart消费者；原registry逐个以mask=1/base=raw发送。
        if not ipi_gate('mask-base', mask == 1 and base in range(4), candidate):
            return False
        caller = gdb.newest_frame()
        while caller and caller.name() != 'kernel::registry::try_ipi_slots':
            caller = caller.older()
        if not ipi_gate('registry-caller', caller is not None, candidate):
            return False
        candidate['caller'] = {'name': caller.name(), 'pc': int(caller.pc()),
                               'sp': int(caller.read_register('sp')), 'locals': {}}
        for name in ('mask', 'targets', 'slot', 'hartid'):
            try:
                candidate['caller']['locals'][name] = int(caller.read_var(name))
            except (gdb.error, RuntimeError) as error:
                candidate['caller']['locals'][name] = {
                    'unavailable': {'type': type(error).__name__, 'message': str(error)}}
        require(sender.num not in ipi_calls, 'next IPI would overwrite an unconsumed original call')
        call = {'sender_thread': sender.num, 'sender': ipi_local(int(gdb.parse_and_eval('$mhartid'))),
                'target': ipi_local(base), 'mask': mask, 'base': base,
                'caller_sp': int(caller.read_register('sp')),
                'ecall_pc': int(gdb.parse_and_eval('$pc')) & ((1 << 64) - 1),
                'requested': candidate['requested'], 'stopped': candidate['stopped'],
                'caller': candidate['caller']}
        if ipi_observation is not None:
            call['injected'] = False
            ipi_calls[sender.num] = call
            print('DMA_IPI_REAL_SEND ' + json.dumps(call, sort_keys=True))
            return False
        if not ipi_gate('request-phase', call['requested'] == 1 and call['stopped'] == 0, candidate):
            return False
        if not ipi_gate('non-sender', base != call['sender']['hartid'], candidate):
            return False
        frame = gdb.newest_frame()
        while frame and not (frame.name() or '').startswith('kernel::runtime_stop::request'):
            frame = frame.older()
        if not ipi_gate('request-caller', frame is not None, candidate):
            return False
        candidate['request_caller'] = {'name': frame.name(), 'pc': int(frame.pc())}
        target = next((t for t in gdb.selected_inferior().threads() if t.ptid[1] == base + 1), None)
        candidate['target_thread'] = None if target is None else target.num
        ipi_gate('target-present', target is not None, candidate)
        require(target is not None, 'actual SBI target has no QEMU CPU')
        try:
            target.switch()
            gdb.execute('maintenance flush register-cache')
            pc = int(gdb.parse_and_eval('$pc')) & ((1 << 64) - 1)
            target_registers = {'thread': target.num, 'hartid': int(gdb.parse_and_eval('$mhartid')),
                                'pc': pc, 'sip': int(gdb.parse_and_eval('$sip')),
                                'sie': int(gdb.parse_and_eval('$sie')),
                                'sampled_ticks': int(gdb.parse_and_eval('$time'))}
        finally:
            sender.switch()
            gdb.execute('maintenance flush register-cache')
            # 原sender寄存器读回确保RSP general context也已恢复，再读内核事实。
            require(int(gdb.parse_and_eval('$mhartid')) == call['sender']['hartid'], 'IPI sender context changed')
        require(target_registers['hartid'] == base, 'SBI target/raw hart mismatch')
        candidate['target'] = call['target']
        candidate['target_registers'] = target_registers
        waiting = pc >= __DELTA__ and (physical_word(pc - __DELTA__, 4) == 0x10500073
                                      or physical_word(pc - __DELTA__ - 4, 4) == 0x10500073)
        waiting_ok = ipi_gate('wfi', waiting, candidate)
        quantum_ok = ipi_gate('no-quantum', call['target']['quantum_deadline_ticks'] == 0, candidate)
        if not waiting_ok or not quantum_ok:
            if len(ipi_rejected) < 4:
                ipi_rejected.append({'call': call, 'registers': target_registers})
            return False
        require(retained('before-IPI-failure') == before, 'DMA responsibility changed before failed IPI')
        call.update(injected=True, target_registers=target_registers)
        ipi_observation = call
        ipi_calls[sender.num] = call
        # 仅从已选本次IPI开始旁观返回，不在启动/常态TIME调用处持续截断。
        ipi_return.enabled = True
        ipi_consumed.enabled = True
        # 与既有IPI失败设施相同：仅跳过这一次真实ecall，返回SBI_ERR_FAILED。
        gdb.execute('set $pc = $pc + 4')
        gdb.execute('set $a0 = -1')
        gdb.execute('set $a1 = 0')
        print('DMA_IPI_INJECT ' + json.dumps(call, sort_keys=True))
        return False
    except Exception as error:
        failure = str(error)
        print('DMA_IPI_QUALIFICATION_FAILED ' + json.dumps({
            'error': failure, 'error_type': type(error).__name__, 'candidate': candidate,
            'eligibility': ipi_eligibility}, sort_keys=True))
        return True

class IpiReturn(gdb.Breakpoint):
    def stop(self):
        global failure
        call = ipi_calls.get(gdb.selected_thread().num)
        if call is None:
            return False
        try:
            require('error' not in call, 'original IPI raw return was observed twice')
            raw = int(gdb.parse_and_eval('$a0')) & ((1 << 64) - 1)
            call.update(error=raw if raw < (1 << 63) else raw - (1 << 64),
                        value=int(gdb.parse_and_eval('$a1')))
            frame = gdb.newest_frame()
            require(frame.name() == 'kernel::sbi::sbi_call', 'raw return is not in the original SBI wrapper')
            require(int(frame.read_var('error')) == call['error']
                    and int(frame.read_var('value')) == call['value'],
                    'SBI wrapper has not stored its actual error/value')
            require(call['error'] == (-1 if call['injected'] else 0), 'unexpected actual IPI return')
            if call['injected']:
                require(retained('after-IPI-failure') == before, 'failed IPI changed DMA responsibility')
            print('DMA_IPI_RAW_RETURN ' + json.dumps(call, sort_keys=True))
            return False
        except Exception as error:
            failure = str(error)
            print('DMA_IPI_RAW_FAILED ' + json.dumps({
                'error': failure, 'error_type': type(error).__name__, 'call': call}, sort_keys=True))
            ipi_backtrace('DMA_IPI_RAW_BT_UNAVAILABLE ')
            return True

class IpiConsumed(gdb.Breakpoint):
    def stop(self):
        global failure
        call = ipi_calls.get(gdb.selected_thread().num)
        if call is None:
            return False
        try:
            require('error' in call, 'original IPI caller consumed a result without its raw return observation')
            require(int(gdb.parse_and_eval('$sp')) == call['caller_sp'], 'IPI result belongs to a different caller frame')
            slot_address = call['caller_sp'] + __RESULT_SLOT__
            call['result_slot_address'] = slot_address
            slot_physical = ipi_stack_address(slot_address, 8, call['sender']['slot'])
            result_address = physical_word(slot_physical)
            result = ipi_result_snapshot(result_address, call['sender']['slot'])
            tag = result['tag']
            rejected = int(gdb.parse_and_eval('$a0'))
            call.update(result_address=result_address, result=result['text'],
                        result_memory=result, result_slot_physical=slot_physical, is_err=bool(rejected))
            require(rejected == int(call['error'] != 0), 'original Result::is_err did not consume the observed SBI result')
            if call['injected']:
                require(tag == 'Err', 'original SBI wrapper did not return an error')
                require(result['error'].endswith('SbiError::Failed'),
                        'original SBI wrapper did not classify SBI_ERR_FAILED')
            else:
                require(tag == 'Ok', 'later original IPI did not return Ok')
            call['consumed'] = True
            ipi_returns.append(dict(call))
            del ipi_calls[gdb.selected_thread().num]
            print('DMA_IPI_RETURN ' + json.dumps(call, sort_keys=True))
            if call['injected']:
                ipi_release_window()
            return False
        except Exception as error:
            failure = str(error)
            print('DMA_IPI_CONSUME_FAILED ' + json.dumps({
                'error': failure, 'error_type': type(error).__name__, 'call': call}, sort_keys=True))
            ipi_backtrace('DMA_IPI_CONSUME_BT_UNAVAILABLE ')
            return True

ipi_return = IpiReturn('*__RETURN__', type=gdb.BP_HARDWARE_BREAKPOINT)
ipi_return.enabled = False
ipi_consumed = IpiConsumed('*__CONSUMED__', type=gdb.BP_HARDWARE_BREAKPOINT)
ipi_consumed.enabled = False
ipi_request_end = IpiRequestEnd('*__CHECK__', type=gdb.BP_HARDWARE_BREAKPOINT)
ipi_request_end.enabled = False
'''

FINISH = r'''
print('DMA_IPI_ELIGIBILITY ' + json.dumps(ipi_eligibility, sort_keys=True))
print('DMA_IPI_REJECTED ' + json.dumps(ipi_rejected, sort_keys=True))
require(ipi_observation is not None, 'no eligible original request-broadcast IPI target')
require(any(call['injected'] and call['error'] == -1 for call in ipi_returns), 'failed IPI return was not observed')
require(any(not call['injected'] and call['stopped'] == 1 and call['error'] == 0
            and call['target']['hartid'] == ipi_observation['target']['hartid'] for call in ipi_returns),
        'later original stop broadcast to the same target was not observed')
require(ipi_observation['target_registers']['thread'] in parked, 'failed IPI target did not reach terminal park')
require(not ipi_calls, 'an original observed IPI still lacks caller consumption')
require(ipi_window is not None and ipi_window['released'], 'debugger request window was not released')
print('DMA_IPI_RETENTION_PASSED: same exposed mappings survive one request-broadcast IPI failure and four parks; other wake sources not excluded')
'''


def observation_points(kernel, symbols, ecall):
    """从当前ELF核原返回值存储和调用者is_err分支；不沿用上轮绝对地址。"""
    caller = symbols['kernel::registry::try_ipi_slots']
    end = min(value for value in symbols.values() if value > caller)
    code = subprocess.check_output([
        'riscv64-elf-gdb', '-q', '-nx', '--batch', str(kernel), '-ex', 'set language c',
        '-ex', f'disassemble /r {ecall:#x},{ecall+24:#x}',
        '-ex', f'disassemble /r {caller:#x},{end:#x}',
    ], text=True, stderr=subprocess.STDOUT)
    rows = [(int(address, 16), instruction.strip()) for address, instruction in
            re.findall(r'^\s*(0x[0-9a-f]+).*?:\s+[0-9a-f]+\s+(.+)$', code, re.M)]
    start = next(i for i, row in enumerate(rows) if row == (ecall, 'ecall'))
    stores = rows[start+1:start+3]
    if not (re.fullmatch(r'sd\s+a0,\d+\(sp\)', stores[0][1])
            and re.fullmatch(r'sd\s+a1,\d+\(sp\)', stores[1][1])):
        raise RuntimeError('SBI return does not store the actual error/value before observation')
    raw_return = rows[start+3][0]
    send = next(i for i, row in enumerate(rows) if f'# {symbols["kernel::sbi::send_ipi"]:#x} ' in row[1])
    load = re.fullmatch(r'ld\s+a0,(\d+)\(sp\)', rows[send+1][1])
    if load is None or '6is_err' not in rows[send+3][1] or not re.match(r'beqz\s+a0,', rows[send+4][1]):
        raise RuntimeError('IPI caller no longer consumes its original Result at the verified branch')
    if raw_return == ecall+4:
        raise RuntimeError('SBI raw return observation coincides with the injection landing')
    return {'kernel_sha256': hashlib.sha256(kernel.read_bytes()).hexdigest(),
            'ecall': ecall, 'raw_return': raw_return, 'consumed': rows[send+4][0],
            'result_slot': int(load[1]), 'code': code}


def program(kernel, symbols, ecall):
    points = observation_points(kernel, symbols, ecall)
    stack = {
        'window': symbols['STACK_WINDOW_VA_BASE'], 'size': symbols['STACK_SIZE'],
        'guard': symbols['STACK_GUARD'], 'emergency': symbols['EMERGENCY_SIZE'],
        'slots': symbols['HART_NUM_LIMIT'],
        'physical': symbols['__kernel_pa_end'] - symbols['STACK_SIZE'] * symbols['HART_NUM_LIMIT'],
    }
    substitutions = {
        'REQ': hex(support.physical_address(kernel, symbols['kernel::runtime_stop::REQUESTED'])),
        'STOP': hex(support.physical_address(kernel, symbols['kernel::runtime_stop::STOPPED'])),
        'QUANTUM': hex(support.physical_address(kernel, symbols['kernel::sched::HART_QUANTUM_DEADLINE'])),
        'DELTA': hex(ecall - support.physical_address(kernel, ecall)),
        'RETURN': hex(points['raw_return']),
        'CONSUMED': hex(points['consumed']),
        'RESULT_SLOT': str(points['result_slot']),
        'CHECK': hex(symbols['kernel::runtime_stop::check']),
        'STACK': repr(stack),
        'CODE': repr({name: value for name, value in points.items() if name != 'code'}),
    }
    setup = SETUP
    for name, value in substitutions.items():
        setup = setup.replace('__' + name + '__', value)
    return setup, FINISH
