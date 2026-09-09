#!/usr/bin/env python3
"""Provision slim owned simulators sequentially, then exercise them concurrently from one build."""
import argparse,json,time,subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from experiment_contract import SCHEMA_VERSION, host_snapshot, save
from mcp_client import Client
from profile import usage
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--devices',nargs='+',required=True);p.add_argument('--template',required=True);p.add_argument('--app',required=True);p.add_argument('--count',type=int,default=4);p.add_argument('--compact-device-type');p.add_argument('--budget-mib',type=int,default=6144);a=p.parse_args()
root=Path(__file__).resolve().parent.parent;out=root/'docs/profiling/fleet-validation.json';binary=root/'target/release/mx';control=Client(binary);devices=list(a.devices);report={'schema_version':SCHEMA_VERSION,'evidence_kind':'fleet_validation','devices':devices,'created':[],'provisioned':[],'budget_mib':a.budget_mib,'next_slim_device_reserve_mib':1500,'runs':[]};clients=[]
def shared_host_processes():
    rows=[]
    for line in subprocess.check_output(['/bin/ps','-axo','pid=,comm='],text=True).splitlines():
        parts=line.strip().split(None,1)
        if len(parts)!=2:continue
        name=Path(parts[1]).name
        if name in ['Simulator','CoreSimulatorService','com.apple.CoreSimulator.CoreSimulatorService','simdiskimaged','WindowServer']:
            rows.append({'name':name,'pid':int(parts[0]),'usage':usage(int(parts[0]))})
    return rows

def persist():save(out,report)
try:
    for device in devices:
        state=control.call('mx_status',{'device':device})
        if state.get('state')=='Booted':control.call('mx_shutdown',{'device':device})
    while len(devices)<a.count:
        request={'template':a.template,'name':f'Density {len(devices)+1}'}
        if a.compact_device_type and len(devices)==a.count-1:request['device_type']=a.compact_device_type
        created=control.call('mx_create',request)
        devices.append(created['device']);report['created'].append(created);persist()
    for device in devices:
        print('Provisioning '+device,flush=True)
        control.call('mx_boot',{'device':device,'max_booted':a.count,'memory_budget':{'limit_mib':a.budget_mib,'next_device_mib':4096}})
        try:
            verified=control.call('mx_profile',{'device':device,'operation':'verify','preset':'slim','keep':['network']})
        except RuntimeError:
            control.call('mx_profile',{'device':device,'operation':'apply','preset':'slim','keep':['network']})
            verified=control.call('mx_profile',{'device':device,'operation':'verify','preset':'slim','keep':['network']})
        report['provisioned'].append(verified);persist();control.call('mx_shutdown',{'device':device})
    for device in devices:
        control.call('mx_boot',{'device':device,'max_booted':a.count,'memory_budget':{'limit_mib':a.budget_mib,'next_device_mib':1500}})
    print(f'{len(devices)} slim simulators booted; settling fleet',flush=True);time.sleep(30)
    report['shared_host_before']=shared_host_processes()
    report['host_before_workflows']=host_snapshot()
    report['booted_metrics']=[control.call('mx_metrics',{'device':d}) for d in devices];persist()
    clients=[Client(binary) for _ in devices]
    def run(item):
        index,client,device=item;started=time.monotonic()
        launch=client.call('mx_launch',{'device':device,'app':str(Path(a.app).resolve()),'inspect_ui':True})
        text=f'Agent {index+1} — café 東京'
        result=client.call('mx_action',{'device':device,'actions':[{'action':'tap','selector':{'identifier':'name'}},{'action':'type','text':text},{'action':'tap','selector':{'identifier':'greet'}}],'expect_label':f'Hello, {text}!','timeout_ms':15000})
        path=root/'docs/profiling/screenshots'/f'fleet-{index+1}.png'
        client.call('mx_screenshot',{'device':device,'output':str(path),'inline':False})
        return {'device':device,'launch':launch,'result':result,'seconds':time.monotonic()-started,'screenshot':str(path.relative_to(root))}
    started=time.monotonic()
    with ThreadPoolExecutor(max_workers=len(devices)) as executor:report['runs']=list(executor.map(run,zip(range(len(devices)),clients,devices)))
    report['parallel_workflow_s']=time.monotonic()-started
    report['active_metrics']=[control.call('mx_metrics',{'device':d}) for d in devices]
    report['active_total_mib']=sum(m['simulator_physical_bytes'] for m in report['active_metrics'])/1024**2
    report['shared_host_after']=shared_host_processes()
    report['host_after_workflows']=host_snapshot()
    report['passed']=True;persist()
except Exception as error:
    report['error']=str(error)
    raise
finally:
    for client,device in zip(clients,devices):
        try:client.call('mx_stop',{'device':device,'bundle_id':'dev.mx.demo'})
        except Exception as e:report.setdefault('cleanup_errors',[]).append(str(e))
        client.close()
    for device in devices:
        try:control.call('mx_shutdown',{'device':device})
        except Exception as e:report.setdefault('cleanup_errors',[]).append(str(e))
    control.close();persist()
print('Concurrent fleet passed; slim devices parked (shutdown), profiles retained.',flush=True)
