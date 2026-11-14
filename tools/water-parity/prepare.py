"""Make an isolated empty campaign and single-pod native craft from local reference assets."""
import argparse
import copy
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('--game', type=Path, required=True)
p.add_argument('--saves', type=Path, required=True)
p.add_argument('--seed', type=Path, required=True)
p.add_argument('--out', type=Path, default=Path('.cache/water-parity/inputs'))
a = p.parse_args()
load = lambda f: json.loads(f.read_text(encoding='utf-8-sig'))
seed = load(a.seed)
assert not seed['Vessels'] and seed['Metadata']['GameMode'] == 'SandboxMode'
dest = a.saves / 'OSE Water Validation/seed.json'
dest.parent.mkdir(parents=True, exist_ok=True)
seed['Metadata'].update(Name='Water validation seed', Filepath=str(dest), ThumbnailImagePath='')
if dest.exists():
    assert load(dest) == seed, 'Refusing to replace a changed validation seed'
else:
    dest.write_text(json.dumps(seed), encoding='utf-8')
base = load(a.game / 'BepInEx/ose-parity/craft-stack.json')
assembly = base['Assemblies'][0]['Assembly']
part = copy.deepcopy(next(p for p in assembly['parts'] if p['partName'] == 'pod_1v_conical_crew'))
part['partState']['localPosition'] = dict(x=0, y=0, z=0)
part['partState']['originalAttachLocalPosition'] = dict(x=0, y=0, z=0)
part['partState']['localRotation'] = part['partState']['UserRotation'] = dict(x=0,y=0,z=0,w=1)
part['partState']['attachNodeStates'] = []
assembly['parts'] = [part]
assembly['AssemblyOABConfig']['RootGuid'] = copy.deepcopy(part['PartGuid'])
assembly['AssemblyOABConfig']['SymmetrySets'] = []
assembly['AssemblyOABConfig']['PartRelationshipData'] = []
assembly['SymmetrySets'] = []
base['Metadata'].update(Name='Water validation',VehicleName='Water validation',Parts=1)
out = a.out
out.mkdir(parents=True,exist_ok=True)
(out/'craft.json').write_text(json.dumps(base), encoding='utf-8')
(out/'run.txt').write_text('controlled live water benchmark', encoding='ascii')
print(out)
