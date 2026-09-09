#!/usr/bin/env python3
"""Validate two independent simulators, Unicode, deltas, live logs, and inline screenshots."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
from pathlib import Path
import time
from mcp_client import Client

p=argparse.ArgumentParser(description=__doc__)
p.add_argument('--mx',default='target/release/mx');p.add_argument('--device-a',required=True);p.add_argument('--device-b',required=True)
a=p.parse_args();root=Path(__file__).resolve().parent.parent
out=root/'docs/profiling';out.mkdir(parents=True,exist_ok=True)
clients=[Client(Path(a.mx).resolve()),Client(Path(a.mx).resolve())]
devices=[a.device_a,a.device_b];report={'checks':[]};launched=[]
try:
    tools=clients[0].request('tools/list',{})
    (out/'tools.json').write_text(json.dumps(tools,indent=2)+'\n')
    def launch(pair):
        client,device=pair
        result=client.call('mx_run',{'project':str(root/'examples/MxDemo'),'scheme':'MxDemo','device':device,'inspect_ui':True})
        assert result['ui'] and not result['ui_error'],result
        return result
    started=time.monotonic()
    with ThreadPoolExecutor(max_workers=2) as executor:
        launched=list(executor.map(launch,zip(clients,devices)))
    report['parallel_launch_s']=time.monotonic()-started
    report['runs']=launched
    for index,(client,device,run) in enumerate(zip(clients,devices,launched)):
        initial=client.call('mx_observe',{'device':device})
        unchanged=client.call('mx_observe',{'device':device,'since':initial['revision']})
        assert unchanged['full'] is None and not unchanged['changed'] and not unchanged['added']
        stream=client.call('mx_logs_start',{'device':device,'pid':run['pid']})['stream_id']
        text=['José 東京 🧉 — Agent A','Zoë café 🌎 — Agent B'][index]
        result=client.call('mx_action',{'device':device,'actions':[
            {'action':'tap','selector':{'identifier':'name'}},{'action':'type','text':text},
            {'action':'tap','selector':{'identifier':'greet'}}],'expect_label':f'Hello, {text}!','timeout_ms':15000})
        assert any(e.get('label')==f'Hello, {text}!' for e in result['changed']),result
        cursor=0;events=[]
        for _ in range(30):
            batch=client.call('mx_logs_read',{'stream_id':stream,'after':cursor})
            events.extend(batch['entries']);cursor=batch['cursor']
            if any('Greeting displayed' in e['text'] for e in events):break
            time.sleep(.1)
        assert any('Greeting displayed' in e['text'] for e in events),events
        client.call('mx_logs_stop',{'stream_id':stream})
        image_path=out/'screenshots'/f'agent-{chr(97+index)}-running.png'
        image=client.request('tools/call',{'name':'mx_screenshot','arguments':{'device':device,'output':str(image_path)}})
        assert not image.get('isError') and any(c['type']=='image' for c in image['content']),image
        report['checks'].append({'device':device,'unicode':text,'delta':result,'live_log_entries':len(events),'screenshot':str(image_path.relative_to(root))})
    report['passed']=True
except Exception as error:
    report['error']=str(error)
    report['failure_ui']=[]
    for client,device in zip(clients,devices):
        try:report['failure_ui'].append(client.call('mx_ui',{'device':device}))
        except Exception as e:report['failure_ui'].append(str(e))
    raise
finally:
    for client,run in zip(clients,launched):
        try:client.call('mx_stop',{'device':run['device'],'bundle_id':run['bundle_id']})
        except Exception as error:report.setdefault('cleanup_errors',[]).append(str(error))
    for client in clients:client.close()
    (out/'sessions-validation.json').write_text(json.dumps(report,indent=2,ensure_ascii=False)+'\n')
print('Two independent sessions passed Unicode, deltas, live logs, and inline PNG capture.')
