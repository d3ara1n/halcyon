#!/usr/bin/env python3
"""原IRQ消费者的一次平台时钟首因调查；仅两个错误路径只读快照。"""
import argparse
import ast
from contextlib import ExitStack
import hashlib
import json
import re
from pathlib import Path
import signal
import socket
import subprocess
import sys
import time

import qemu_gdb as debug


def program(kernel, logs, port):
    symbols = debug.symbols(kernel)
    name = '<kernel::clock::ClockState>::read::<kernel::sbi::read_time>'
    begin = symbols[name]
    end = min(value for value in symbols.values() if value > begin)
    closure = symbols[name + '::{closure#0}']
    closure_end = min(value for value in symbols.values() if value > closure)
    store = symbols['<core::sync::atomic::Atomic<bool>>::store']
    code = subprocess.check_output([
        'riscv64-elf-gdb', '-q', '-nx', '--batch', str(kernel), '-ex', 'set language c',
        '-ex', f'disassemble /r {begin:#x},{end:#x}',
        '-ex', f'disassemble /r {closure:#x},{closure_end:#x}',
    ], text=True, stderr=subprocess.STDOUT)
    (logs/'code.log').write_text(code)
    points=[];sizes=[]
    for low,high in ((begin,end),(closure,closure_end)):
        rows=[(int(pc,16),instruction.strip()) for pc,instruction in re.findall(
            r'^\s*(0x[0-9a-f]+).*?:\s+[0-9a-f]+\s+(.+)$',code,re.M) if low<=int(pc,16)<high]
        calls=[i for i,row in enumerate(rows) if f'# {store:#x} ' in row[1]]
        if len(calls)!=1 or not rows[calls[0]-1][1].startswith('auipc'):
            raise RuntimeError('clock failed-store path does not match the verified call sequence')
        size=re.fullmatch(r'addi\s+sp,sp,-(\d+)',rows[0][1])
        if size is None:raise RuntimeError('clock snapshot frame prologue changed')
        points.append(rows[calls[0]-1][0]);sizes.append(int(size[1]))
    lines=Path('os/kernel/src/clock.rs').read_text().splitlines()
    declaration=lambda text:next(i for i,line in enumerate(lines,1) if text in line)
    bindings={'prior':declaration('let prior ='),'sample_raw':declaration('let raw = sample();'),
              'effective_raw':declaration('let raw = raw.max('),'geometry':declaration('geometry: ClockGeometry'),
              'self':declaration('&self,'),'error':declaration('map_err(|error|')}
    facts={'status':'armed','kernel_sha256':hashlib.sha256(kernel.read_bytes()).hexdigest(),
           'points':dict(zip(('regression','conversion'),points)),
           'read_range':[begin,end],'frame_sizes':sizes,'declarations':bindings,
           'environment':'THROTTLE=100; four harts; no icount; cold -S; two error points; no IRQ probes',
           'old_environment_difference':'old IRQ run used THROTTLE=50 and user Entry breakpoint after attach'}
    (logs/'first-cause.json').write_text(json.dumps(facts,indent=2)+'\n')
    script=f'''set pagination off
set confirm off
set language rust
file "{kernel.resolve()}"
target remote 127.0.0.1:{port}
python
import gdb,json
from pathlib import Path
EVIDENCE=Path({json.dumps(str((logs/'first-cause.json').resolve()))})
facts=json.loads(EVIDENCE.read_text())
{debug.PHYSICAL_READ}
def require(value,message):
    if not value:raise gdb.GdbError(message)
def save():
    temporary=EVIDENCE.with_suffix('.next')
    temporary.write_text(json.dumps(facts,indent=2)+'\\n');temporary.replace(EVIDENCE)
def field_offset(kind,name):return next(f.bitpos//8 for f in gdb.lookup_type(kind).fields() if f.name==name)
failed_offset=field_offset('kernel::clock::ClockState','failed')
geometry_offset=field_offset('kernel::clock::GeometryCell','value')
published_offset=field_offset('kernel::clock::GeometryCell','state')
STATE={symbols['kernel::clock::STATE']}
STATE_PA={debug.physical_address(kernel,symbols['kernel::clock::STATE'])}
GEOMETRY_PA={debug.physical_address(kernel,symbols['kernel::clock::GEOMETRY'])}
def frames():
    items=[];frame=gdb.newest_frame()
    while frame is not None:items.append(frame);frame=frame.older()
    return items
def local(frame,name,line):
    block=frame.block()
    while block is not None:
        for symbol in block:
            if symbol.name==name and symbol.line==line and (symbol.is_variable or symbol.is_argument):return symbol.value(frame)
        if block.function is not None:break
        block=block.superblock
    raise gdb.GdbError('clock local unavailable: '+name+' declaration '+str(line))
def geometry(value):return {{name:int(value[name]) for name in ('frequency_hz','origin_ticks','max_deadline_ns')}}
class FailurePoint(gdb.Breakpoint):
    def __init__(self,pc,branch):
        super().__init__('*%#x'%pc,type=gdb.BP_HARDWARE_BREAKPOINT);self.branch=branch;self.silent=True
    def stop(self):
        destination=int(gdb.parse_and_eval('$a0'))&((1<<64)-1)
        if destination != STATE+failed_offset:return False
        try:
            stack=frames()
            facts['branch']=self.branch
            facts['store_arguments']={{'destination':destination,'value':int(gdb.parse_and_eval('$a1')),'ordering':int(gdb.parse_and_eval('$a2'))}}
            facts['registers']={{name:int(gdb.parse_and_eval('$'+name))&((1<<64)-1) for name in ('pc','mhartid','satp','sstatus','sp')}}
            facts['stack']=[{{'pc':hex(int(f.pc())),'function':f.name()}} for f in stack]
            facts['state']={{'address':STATE,'high_ticks_at_stop':physical_word(STATE_PA),
                            'high_ns_at_stop':physical_word(STATE_PA+8),'failed_before':physical_word(STATE_PA+failed_offset,1)}}
            facts['stage']='original read frame';save()
            read=next(f for f in stack if {begin}<=int(f.pc())<{end})
            sp=int(read.read_register('sp'))
            facts['read_frame']={{'sp':sp,'bytes':gdb.selected_inferior().read_memory(sp,{sizes[0]}).tobytes().hex()}}
            if self.branch=='conversion':
                sp=int(stack[0].read_register('sp'))
                facts['conversion_frame']={{'sp':sp,'bytes':gdb.selected_inferior().read_memory(sp,{sizes[1]}).tobytes().hex()}}
            facts['stage']='platform self';save()
            facts['self_address']=int(local(read,'self',{bindings['self']}))
            require(facts['self_address']==STATE,'clock read self is not the platform STATE')
            require(int(gdb.parse_and_eval('$a1'))==1 and int(gdb.parse_and_eval('$a2'))==1,'failed store arguments are not true/Release')
            require(facts['state']['failed_before']==0,'clock failure observation arrived after STATE.failed')
            for name,key in [('prior','prior'),('raw','sample_raw')]:
                facts['stage']='local '+key;save()
                facts[key]=int(local(read,name,facts['declarations'][key]))
            facts['stage']='passed geometry';save()
            facts['passed_geometry']=geometry(local(read,'geometry',{bindings['geometry']}))
            offsets={{f.name:f.bitpos//8 for f in gdb.lookup_type('erhino_shared::time::ClockGeometry').fields()}}
            facts['global_geometry']={{name:physical_word(GEOMETRY_PA+geometry_offset+offset) for name,offset in offsets.items()}}
            facts['geometry_published']=physical_word(GEOMETRY_PA+published_offset,1)
            if self.branch=='conversion':
                facts['stage']='effective raw58 / TimeError';save()
                facts['effective_raw']=int(local(read,'raw',{bindings['effective_raw']}))
                facts['conversion_error']=str(local(stack[0],'error',{bindings['error']}))
            else:
                facts['regression_relation']=facts['sample_raw']<facts['prior'] and facts['prior']-facts['sample_raw']>1
            facts['status']='captured';facts['stage']='complete';save()
            print('CLOCK_FIRST_CAUSE '+json.dumps(facts,sort_keys=True))
        except Exception as error:
            facts['status']='sampling-error';facts['error']=str(error);facts['backtrace']=gdb.execute('bt',to_string=True);save()
            print('CLOCK_SAMPLE_ERROR '+json.dumps(facts,sort_keys=True))
        for point in points:point.enabled=False
        return True
points=[FailurePoint({points[0]},'regression'),FailurePoint({points[1]},'conversion')]
end
continue
python
if facts['status']=='armed':facts['status']='not-triggered';save()
if gdb.selected_inferior().threads():gdb.execute('detach',to_string=True)
end
quit
'''
    for body in re.findall(r'\npython\n(.*?)\nend\n',script,re.S):ast.parse(body)
    return script


def stop_debugger(process):
    if process.poll() is None:
        process.terminate()
        try:process.wait(timeout=5)
        except subprocess.TimeoutExpired:process.kill();process.wait(timeout=5)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--kernel',type=Path,required=True)
    parser.add_argument('--init',type=Path,required=True)
    parser.add_argument('--boot-package',type=Path,required=True)
    parser.add_argument('--timeout',type=float,required=True)
    parser.add_argument('command',nargs=argparse.REMAINDER)
    args=parser.parse_args();command=args.command[1:] if args.command[:1]==['--'] else args.command
    if not command:parser.error('QEMU command missing')
    logs=debug.fresh_logs(Path('artifacts/clock-first-cause'))
    with socket.socket() as listener:listener.bind(('127.0.0.1',0));port=listener.getsockname()[1]
    script=program(args.kernel,logs,port);(logs/'clock.gdb').write_text(script)
    run={'status':'running','command':command,'timeout':args.timeout,
         **{name+'_sha256':hashlib.sha256(path.read_bytes()).hexdigest() for name,path in
            [('kernel',args.kernel),('init',args.init),('boot_package',args.boot_package)]},
         'script_sha256':hashlib.sha256(script.encode()).hexdigest()}
    (logs/'run.json').write_text(json.dumps(run,indent=2)+'\n')
    process=None;debugger=None;original=None
    try:
        with (logs/'qemu.log').open('w') as qlog,(logs/'gdb.log').open('w') as glog:
            process=subprocess.Popen(command+['-S','-gdb',f'tcp:127.0.0.1:{port}'],stdout=qlog,stderr=subprocess.STDOUT,start_new_session=True)
            debugger=subprocess.Popen(['riscv64-elf-gdb','-q','-nx','--batch','-x',str(logs/'clock.gdb')],stdout=glog,stderr=subprocess.STDOUT)
            end=time.monotonic()+args.timeout
            while time.monotonic()<end:
                guest=(logs/'qemu.log').read_text(errors='replace')
                facts=json.loads((logs/'first-cause.json').read_text())
                if facts['status']=='sampling-error' or (debugger.poll() is not None and debugger.returncode):
                    raise RuntimeError(f'clock observation failed: {logs}')
                if 'Runtime stopped: platform clock failed' in guest:
                    if facts['status']!='captured':raise RuntimeError(f'clock terminal lacks first-cause snapshot: {logs}')
                    run['status']='first-cause-captured';break
                if ('kernel acceptance failed:' in guest or 'system failure terminal entered:' in guest
                        or 'Kernel panicking' in guest or 'Runtime stopped:' in guest):
                    raise RuntimeError(f'consumer failed before a clock cause was observed: {logs}')
                if process.poll() is not None:
                    if process.returncode or 'system reset accepted: action Shutdown, reason Requested' not in guest:
                        raise RuntimeError(f'guest ended without its normal terminal: {logs}')
                    debugger.wait(timeout=max(.1,end-time.monotonic()))
                    facts=json.loads((logs/'first-cause.json').read_text())
                    if debugger.returncode or facts['status']!='not-triggered':raise RuntimeError(f'inconsistent normal clock investigation: {logs}')
                    if any('IRQ exit target closed: mode='+mode not in guest for mode in ('irq-wait-exit','irq-set-exit')):
                        raise RuntimeError(f'original IRQ consumers did not finish: {logs}')
                    run['status']='not-reproduced';break
                time.sleep(.05)
            else:raise TimeoutError(f'clock investigation deadline exceeded: {logs}')
    except BaseException as error:
        original=error;run['status']='failed';run['error']=str(error)
    finally:
        try:
            with ExitStack() as cleanup:
                for signum in (signal.SIGTERM,signal.SIGINT):
                    previous=signal.signal(signum,signal.SIG_IGN)
                    cleanup.callback(signal.signal,signum,previous)
                if debugger:cleanup.callback(stop_debugger,debugger)
                if process:cleanup.callback(debug.stop_group,process)
        except BaseException as error:
            run['cleanup_error']=str(error)
            if original is None:original=error
        (logs/'run.json').write_text(json.dumps(run,indent=2)+'\n')
    if original:raise original
    print(f"CLOCK investigation {run['status']}: {logs}")
    return 0


if __name__=='__main__':
    def interrupted(signum,_frame):raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM,interrupted);signal.signal(signal.SIGINT,interrupted)
    try:sys.exit(main())
    except Exception as error:print(f'CLOCK investigation failed: {error}',file=sys.stderr);sys.exit(1)
