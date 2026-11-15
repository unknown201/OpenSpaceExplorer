"""Prepare local game-derived fixtures; never overwrite an existing campaign."""
import argparse
import copy
import json
from pathlib import Path
import uuid

p = argparse.ArgumentParser()
p.add_argument('--game', type=Path, required=True)
p.add_argument('--saves', type=Path, required=True)
p.add_argument('--seed', type=Path, required=True)
args = p.parse_args()
load = lambda f: json.loads(f.read_text(encoding='utf-8-sig'))
out = args.game / 'BepInEx/ose-services-validation'
out.mkdir(exist_ok=True)
seed = load(args.seed)
assert not seed['Vessels']
assert seed['Metadata']['GameMode'] == 'SandboxMode'
dest = args.saves / 'OSE Services Validation/seed.json'
dest.parent.mkdir(parents=True, exist_ok=True)
seed['Metadata'].update(Name='Services validation seed', Filepath=str(dest), ThumbnailImagePath='')
if dest.exists():
    existing = load(dest)
    assert not existing['Vessels'] and existing['Metadata']['Name'] == 'Services validation seed'
else:
    dest.write_text(json.dumps(seed), encoding='utf-8')
base = load(args.game / 'BepInEx/ose-parity/craft-stack.json')
templates = {}
for f in Path('assets/ksp2/data/stockassemblies').glob('*.json'):
    for part in load(f)['Assemblies'][0]['Assembly']['parts']:
        templates.setdefault(part['partName'], part)
assembly = base['Assemblies'][0]['Assembly']
ids = ['pod_1v_conical_crew', 'fueltank_1v_inline_methalox_1x2',
       'solararray_0v_flat', 'solararray_1v_shielded_1x6', 'antenna_0v_16']
parts = []
positions = [(0, 0, 0), (0, -1.2775, 0), (1, -1, 0), (-1, -1, 0), (0, -1, 1)]
for i, (name, xyz) in enumerate(zip(ids, positions)):
    part = copy.deepcopy(templates[name])
    part['PartGuid']['Guid'] = str(uuid.uuid5(uuid.NAMESPACE_URL, 'ose-services-' + name))
    state = part['partState']
    state['localPosition'] = dict(zip(('x', 'y', 'z'), xyz))
    state['originalAttachLocalPosition'] = copy.deepcopy(state['localPosition'])
    state['localRotation'] = state['UserRotation'] = dict(x=0, y=0, z=0, w=1)
    state['attachNodeStates'] = []
    parts.append(part)
node = copy.deepcopy(assembly['parts'][0]['partState']['attachNodeStates'][0])
for i, part in enumerate(parts):
    if i == 0:
        n = copy.deepcopy(node)
        n.update(nodeId='bottom', attachedPartNodeID='top')
        n['AttachedPartGuid']['Guid'] = parts[1]['PartGuid']['Guid']
        part['partState']['attachNodeStates'].append(n)
    else:
        n = copy.deepcopy(node)
        n.update(nodeId='top' if i == 1 else 'srfAttach', attachedPartNodeID='bottom' if i == 1 else 'srfAttach')
        n['AttachedPartGuid']['Guid'] = parts[0 if i == 1 else 1]['PartGuid']['Guid']
        part['partState']['attachNodeStates'].append(n)
assembly['parts'] = parts
assembly['AssemblyOABConfig']['RootGuid'] = copy.deepcopy(parts[0]['PartGuid'])
assembly['AssemblyOABConfig']['SymmetrySets'] = []
assembly['AssemblyOABConfig']['PartRelationshipData'] = []
assembly['SymmetrySets'] = []
base['Metadata'].update(Name='Services validation', VehicleName='Services validation', Parts=len(parts))
(out / 'craft.json').write_text(json.dumps(base), encoding='utf-8')
(out / 'run.txt').write_text('controlled runtime services audit', encoding='ascii')
print(out)
