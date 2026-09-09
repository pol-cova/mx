#!/usr/bin/env python3
"""Controlled stock/wallpaper/slim/restored measurements and prebuilt-app workflows."""
import argparse,json,time
from pathlib import Path
from experiment_contract import SCHEMA_VERSION, distribution, host_snapshot, save
from mcp_client import Client
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--device',required=True);p.add_argument('--app',required=True);p.add_argument('--cycles',type=int,default=2);a=p.parse_args()
root=Path(__file__).resolve().parent.parent;out=root/'docs/profiling/density-single.json';c=Client(root/'target/release/mx');report={'schema_version':SCHEMA_VERSION,'evidence_kind':'density_single','device':a.device,'app':str(Path(a.app).resolve()),'cycles':a.cycles,'phases':[], 'methodology':'Same owned device and prebuilt UIKit app. Every phase starts after reboot, settles 30 seconds, then samples 10 times at 1-second intervals. Five launch/action/stop workflows per phase. Process footprint is summed phys_footprint; separate host swap/pressure counters. CPU differences track PID and process start identity. No concurrent builds.'};active=False;profiled=False

def persist():save(out,report)
def measure(phase):
    print(f'Settling {phase["name"]} cycle {phase["cycle"]}',flush=True);time.sleep(30)
    rows=[];began=time.monotonic();phase['host_before']=host_snapshot()
    for _ in range(10):
        rows.append(c.call('mx_metrics',{'device':a.device}));time.sleep(1)
    elapsed=time.monotonic()-began;phase['host_after']=host_snapshot();phase['samples']=rows
    phase['physical_mib']=distribution([r['simulator_physical_bytes']/1024**2 for r in rows])
    phase['process_count']=distribution([r['simulator_process_count'] for r in rows])
    phase['median_mib']=phase['physical_mib']['median']
    phase['median_processes']=phase['process_count']['median']
    before={(p['pid'],p['start_time']):p['cpu_ns'] for p in rows[0]['processes']}
    phase['surviving_process_cpu_seconds']=sum(max(0,p['cpu_ns']-before[(p['pid'],p['start_time'])]) for p in rows[-1]['processes'] if (p['pid'],p['start_time']) in before)/1e9
    phase['sample_elapsed_seconds']=elapsed
    phase['top_processes']=rows[-1]['processes'][:20]
    print(f'{phase["name"]}: {phase["median_mib"]:.1f} MiB, {phase["median_processes"]} processes',flush=True)

def workflows(phase):
    global active
    runs=[]
    for i in range(5):
        start=time.monotonic();run=c.call('mx_launch',{'device':a.device,'app':report['app'],'inspect_ui':True});active=True
        c.call('mx_action',{'device':a.device,'actions':[{'action':'tap','selector':{'identifier':'increment'}}],'expect_label':'Count: 1'})
        if i==0:phase['running_app_metrics']=c.call('mx_metrics',{'device':a.device})
        if i==4:
            text='José 東京 🧉'
            c.call('mx_action',{'device':a.device,'actions':[{'action':'tap','selector':{'identifier':'name'}},{'action':'type','text':text},{'action':'tap','selector':{'identifier':'greet'}}],'expect_label':f'Hello, {text}!','timeout_ms':15000})
            path=root/'docs/profiling/screenshots'/f'density-{phase["name"]}-{phase["cycle"]}.png'
            c.call('mx_screenshot',{'device':a.device,'output':str(path),'inline':False})
        c.call('mx_stop',{'device':a.device,'bundle_id':'dev.mx.demo'});active=False
        runs.append({'seconds':time.monotonic()-start,'launch':run,'unicode_and_screenshot':i==4})
    phase['runs']=runs;phase['warm_workflow_seconds']=distribution([r['seconds'] for r in runs[:4]]);phase['warm_workflow_median_s']=phase['warm_workflow_seconds']['median'];phase['smoke_passed']=True

try:
    c.call('mx_boot',{'device':a.device,'max_booted':4})
    for cycle in range(1,a.cycles+1):
        names=['stock','wallpaper','slim','restored'] if cycle==1 else ['stock','slim','restored']
        for name in names:
            if profiled:
                c.call('mx_profile',{'device':a.device,'operation':'restore'});profiled=False
            if name in ['wallpaper','slim']:
                keep=['network']
                if name=='wallpaper':keep += [x['id'] for x in json.loads((root/'data/mx-runtime-ios-26.5.json').read_text())['categories'] if x['id']!='widgets']
                profiled=True;c.call('mx_profile',{'device':a.device,'operation':'apply','preset':'slim','keep':keep})
                c.call('mx_profile',{'device':a.device,'operation':'verify','preset':'slim','keep':keep})
            else:
                c.call('mx_shutdown',{'device':a.device});c.call('mx_boot',{'device':a.device,'max_booted':4})
            phase={'name':name,'cycle':cycle};report['phases'].append(phase);measure(phase);persist();workflows(phase);persist()
    report['passed']=True
finally:
    if active:
        try:c.call('mx_stop',{'device':a.device,'bundle_id':'dev.mx.demo'})
        except Exception as error:report['stop_error']=str(error)
    if profiled:
        try:report['restore']=c.call('mx_profile',{'device':a.device,'operation':'restore'})
        except Exception as error:report['restore_error']=str(error)
    persist();c.close()
print('Density profile comparison complete; device restored.',flush=True)
