#!/usr/bin/env python3
"""File reproducible benchmark summaries without discarding the raw evidence."""
import gzip,json,shutil,statistics
from pathlib import Path
root=Path(__file__).resolve().parent.parent;out=root/'docs/profiling'
for name in ['baseline','final']:
    src=root/'.mx/profiles'/f'{name}-run';r=json.loads((src/'raw.json').read_text())
    summary=json.loads((src/'summary.json').read_text())
    cpu=[];child=[];previous=None
    for row in r['runs']:
        if previous and row['kind']=='warm':
            cpu.append((row['cpu']['cpu_ns']-previous['cpu']['cpu_ns'])/1e9)
            child.append((row['cpu']['reaped_children_cpu_ns']-previous['cpu']['reaped_children_cpu_ns'])/1e9)
        previous=row
    summary['warm_median_mx_cpu_s']=statistics.median(cpu)
    summary['warm_median_reaped_children_cpu_s']=statistics.median(child)
    (out/f'{name}-summary.json').write_text(json.dumps(summary,indent=2)+'\n')
    with (src/'raw.json').open('rb') as source,gzip.open(out/f'{name}-raw.json.gz','wb') as target:shutil.copyfileobj(source,target)
    print(name,summary['idle'],summary['warm_median_mx_cpu_s'],summary['warm_median_reaped_children_cpu_s'])
