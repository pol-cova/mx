#!/usr/bin/env python3
"""Repeated release-binary profiling, with macOS physical footprint and CPU counters."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import time
from experiment_contract import SCHEMA_VERSION, distribution, environment
from mcp_client import Client

# Layout from the Xcode SDK sys/resource.h rusage_info_v4, not RSS approximations.
FIELDS = '''user_time system_time pkg_idle_wkups interrupt_wkups pageins wired_size resident_size phys_footprint
proc_start_abstime proc_exit_abstime child_user_time child_system_time child_pkg_idle_wkups child_interrupt_wkups
child_pageins child_elapsed_abstime diskio_bytesread diskio_byteswritten cpu_time_qos_default cpu_time_qos_maintenance
cpu_time_qos_background cpu_time_qos_utility cpu_time_qos_legacy cpu_time_qos_user_initiated cpu_time_qos_user_interactive
billed_system_time serviced_system_time logical_writes lifetime_max_phys_footprint instructions cycles billed_energy
serviced_energy interval_max_phys_footprint runnable_time'''.split()
class Usage(ctypes.Structure):
    _fields_ = [('uuid', ctypes.c_ubyte * 16)] + [(name, ctypes.c_uint64) for name in FIELDS]
LIB = ctypes.CDLL('/usr/lib/libproc.dylib')
LIB.proc_pid_rusage.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_void_p]

def usage(pid):
    value = Usage()
    if LIB.proc_pid_rusage(pid, 4, ctypes.byref(value)) != 0:
        return None
    return {'pid':pid, 'physical_bytes':value.phys_footprint, 'rss_bytes':value.resident_size,
        'cpu_ns':value.user_time + value.system_time,
        'reaped_children_cpu_ns':value.child_user_time + value.child_system_time,
        'start':value.proc_start_abstime}

class Sampler:
    def __init__(self, pid, device):
        self.pid, self.device = pid, device
        self.rows = []
        self.done = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()
    def run(self):
        while not self.done.is_set():
            processes = {}
            simulator = None
            output = subprocess.check_output(['ps','-axo','pid=,ppid=,command='],text=True)
            for line in output.splitlines():
                parts = line.strip().split(None,2)
                if len(parts) < 3:
                    continue
                pid, parent = map(int,parts[:2])
                processes[pid] = parent
                if parts[2].startswith('launchd_sim ') and self.device in parts[2]:
                    simulator = pid
            def tree(root):
                if root is None: return []
                selected = {root}
                while True:
                    new = {pid for pid,parent in processes.items() if parent in selected} - selected
                    if not new: break
                    selected.update(new)
                return [value for pid in selected if (value := usage(pid)) is not None]
            self.rows.append({'at':time.monotonic(), 'mx':usage(self.pid),
                'children':[p for p in tree(self.pid) if p['pid'] != self.pid], 'simulator':tree(simulator)})
            self.done.wait(.25)
    def close(self):
        self.done.set()
        self.thread.join(timeout=5)

def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--mx',required=True);p.add_argument('--device',required=True);p.add_argument('--output',required=True)
    p.add_argument('--warm-runs',type=int,default=10);p.add_argument('--cold-runs',type=int,default=3)
    p.add_argument('--clean-runs',type=int,default=3);p.add_argument('--idle-seconds',type=int,default=15)
    a=p.parse_args(); root=Path(__file__).resolve().parent.parent; destination=Path(a.output).resolve()
    destination.mkdir(parents=True,exist_ok=False)
    project=destination/'MxDemo'; shutil.copytree(root/'examples/MxDemo',project,ignore=shutil.ignore_patterns('.mx'))
    binary=Path(a.mx).resolve(); report={'schema_version':SCHEMA_VERSION,'evidence_kind':'binary_profile','binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest(),
        'binary_bytes':binary.stat().st_size,'device':a.device,'config':vars(a),'runs':[],
        'environment':environment(),
        'methodology':'Warmup excluded. Cold means simulator reboot with warm DerivedData; clean means removed workload DerivedData with booted simulator. No cache purge or device erase. Sample interval 250ms. Physical footprint from proc_pid_rusage v4; sums include per-process accounting, not system-wide unique memory. Short-lived children can fall between samples. CPU uses cumulative nanoseconds; Mx reaped-child counters captured separately.'}
    error=None; sampler=None; client=None
    def sim(*args): subprocess.run(['xcrun','simctl',*args],check=True,stdout=subprocess.DEVNULL)
    with (destination/'stderr.log').open('w') as log:
        try:
            client=Client(binary,stderr=log); sampler=Sampler(client.process.pid,a.device)
            tools=client.request('tools/list',{})
            report['tool_count']=len(tools['tools'])
            before=usage(client.process.pid); time.sleep(a.idle_seconds); after=usage(client.process.pid)
            report['idle']={'seconds':a.idle_seconds,'cpu_seconds':(after['cpu_ns']-before['cpu_ns'])/1e9,
                'physical_bytes':after['physical_bytes'],'rss_bytes':after['rss_bytes']}
            def run(kind,index):
                if kind=='cold_boot': sim('shutdown',a.device)
                if kind=='clean_build': shutil.rmtree(project/'.mx',ignore_errors=True)
                started=time.monotonic(); first_record=len(client.records)
                launched=client.call('mx_run',{'project':str(project),'scheme':'MxDemo','device':a.device})
                def expect(text):
                    deadline=time.monotonic()+15
                    while True:
                        screen=client.call('mx_ui',{'device':a.device})
                        if any(e.get('label')==text for e in screen['elements']): return
                        if time.monotonic()>deadline: raise AssertionError(screen)
                        time.sleep(.25)
                expect('Count: 0')
                client.call('mx_tap',{'device':a.device,'selector':{'identifier':'increment'}}); expect('Count: 1')
                client.call('mx_stop',{'device':a.device,'bundle_id':'dev.mx.demo'})
                row={'kind':kind,'index':index,'elapsed_s':time.monotonic()-started,'run':launched,
                    'calls':client.records[first_record:],'cpu':usage(client.process.pid)}
                report['runs'].append(row)
                print(f'{kind} {index}: {row["elapsed_s"]:.3f}s',flush=True)
            run('warmup',0)
            for kind,count in [('warm',a.warm_runs),('cold_boot',a.cold_runs),('clean_build',a.clean_runs)]:
                for i in range(count): run(kind,i+1)
            report['passed']=True
        except BaseException as exception:
            report['passed']=False; report['error']=repr(exception);error=exception
        finally:
            if sampler: sampler.close(); report['samples']=sampler.rows
            if client: report['requests']=client.records;client.close()
    report['summaries']={kind:distribution([r['elapsed_s'] for r in report['runs'] if r['kind']==kind])
        for kind in ['warm','cold_boot','clean_build']}
    report['tool_latency_s']={name:distribution([c['elapsed_s'] for r in report['runs'] if r['kind']=='warm' for c in r['calls'] if c['method']==name])
        for name in ['mx_run','mx_ui','mx_tap','mx_stop']}
    report['sampled_peaks_bytes']={'mx':max((r['mx']['physical_bytes'] for r in report.get('samples',[]) if r['mx']),default=0),
        'children':max((sum(p['physical_bytes'] for p in r['children']) for r in report.get('samples',[])),default=0),
        'simulator':max((sum(p['physical_bytes'] for p in r['simulator']) for r in report.get('samples',[])),default=0)}
    (destination/'raw.json').write_text(json.dumps(report,indent=2)+'\n')
    compact={k:v for k,v in report.items() if k not in ['samples','requests','runs']}
    (destination/'summary.json').write_text(json.dumps(compact,indent=2)+'\n')
    if error: raise error
    print(json.dumps(report['summaries'],indent=2))

if __name__=='__main__': main()
