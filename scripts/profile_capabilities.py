#!/usr/bin/env python3
"""Stock/profile/restored experiment on one owned clone; always attempts restoration."""
import argparse,time
from pathlib import Path
from experiment_contract import SCHEMA_VERSION, distribution, save
from mcp_client import Client
p=argparse.ArgumentParser(description=__doc__);p.add_argument('--device',required=True);p.add_argument('--mx',default='target/release/mx');p.add_argument('--preset',choices=['balanced','slim'],default='balanced');p.add_argument('--other-device',required=True);a=p.parse_args()
root=Path(__file__).resolve().parent.parent;c=Client(Path(a.mx).resolve());r={'schema_version':SCHEMA_VERSION,'evidence_kind':'capability_profile','device':a.device,'phases':{},'methodology':'One stock/profile/restored cycle. Each reboot settles 20 seconds then 10 metrics snapshots at 1-second intervals. Per-process physical-footprint sums, not unique system RAM. Exploratory, not a performance guarantee.'};applied=False
out=root/'docs/profiling/capability-profile.json'
def measure(name):
    print('Sampling '+name,flush=True);time.sleep(20)
    rows=[]
    for _ in range(10):
        rows.append(c.call('mx_metrics',{'device':a.device}));time.sleep(1)
    summary=distribution([x['simulator_physical_bytes'] for x in rows])
    r['phases'][name]={'physical_bytes':summary,'median_physical_bytes':summary['median'],'samples':rows}
    save(out,r)
try:
    c.call('mx_shutdown',{'device':a.other_device})
    c.call('mx_shutdown',{'device':a.device});c.call('mx_boot',{'device':a.device})
    measure('stock')
    applied=True
    r['apply']=c.call('mx_profile',{'device':a.device,'operation':'apply','preset':a.preset,'keep':['network']})
    measure('profile')
    run=c.call('mx_run',{'project':str(root/'examples/MxDemo'),'scheme':'MxDemo','device':a.device,'inspect_ui':True})
    assert run['ui'] and not run['ui_error'],run
    c.call('mx_action',{'device':a.device,'actions':[{'action':'tap','selector':{'identifier':'increment'}}],'expect_label':'Count: 1'})
    c.call('mx_stop',{'device':a.device,'bundle_id':run['bundle_id']});r['app_smoke_passed']=True
finally:
    if applied:
        r['restore']=c.call('mx_profile',{'device':a.device,'operation':'restore','keep':[]})
        measure('restored')
    c.close();save(out,r)
print('Profile restored; experiment saved.',flush=True)
