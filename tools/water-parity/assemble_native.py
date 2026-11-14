"""Assemble exactly one completed fresh-process capture per declared case; preserve all inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
from analyze import CASES, objects, group, validate

def assemble(root):
    events, samples, manifest = [], [], {}
    for name in CASES:
        folder=root/'native-cases'/name
        if not (folder/'done.txt').exists() or (folder/'failed.txt').exists():
            raise ValueError(f'incomplete native case: {name}')
        case_events=objects(folder/'events.jsonl')
        releases=[e for e in case_events if e['action']=='release']
        ends=[e for e in case_events if e['action']=='end']
        if len(releases)!=1 or releases[0]['scenario']!=name or len(ends)!=1 or not any(e['action']=='complete' for e in case_events):
            raise ValueError(f'unmatched or repeated case: {name}')
        case_samples=objects(folder/'samples.jsonl')
        if {r['scenario'] for r in case_samples}!={name}:
            raise ValueError(f'contaminated samples: {name}')
        events.extend(case_events);samples.extend(case_samples)
        manifest[name]={f.name:hashlib.sha256(f.read_bytes()).hexdigest() for f in folder.iterdir() if f.is_file()}
    validate(group(samples),'native')
    out=root/'native'
    out.mkdir(exist_ok=False)
    for name in CASES:
        shot=root/'native-cases'/name/(name+'.png')
        if shot.exists():shutil.copyfile(shot,out/shot.name)
    for filename,rows in [('events.jsonl',events),('samples.jsonl',samples)]:
        (out/filename).write_text(''.join(json.dumps(r,separators=(',',':'))+'\n' for r in rows),encoding='utf-8')
    (out/'case-manifest.json').write_text(json.dumps(manifest,indent=2),encoding='utf-8')
    (out/'done.txt').write_text('eight complete fresh-process cases',encoding='ascii')
    print(f'Assembled eight native cases in {out}')

if __name__=='__main__':
    parser=argparse.ArgumentParser();parser.add_argument('root',type=Path)
    assemble(parser.parse_args().root)
