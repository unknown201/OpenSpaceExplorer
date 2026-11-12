"""Generate reference inputs from the locally installed pilot and extracted stock craft.

Game-derived inputs belong in ignored reference/, never in this tool's directory.
"""
import argparse
import copy
import json
from pathlib import Path
import uuid


def stack(template, count):
    craft = copy.deepcopy(template)
    assembly = craft['Assemblies'][0]['Assembly']
    old = assembly['parts']
    parts = [copy.deepcopy(old[0])]
    for i in range(count):
        p = copy.deepcopy(old[1])
        p['PartGuid']['Guid'] = str(uuid.uuid5(uuid.NAMESPACE_URL, f'ose-joint-{count}-{i}'))
        p['partState']['localPosition']['y'] = -1.2775 - i * 1.875
        p['partState']['originalAttachLocalPosition'] = copy.deepcopy(p['partState']['localPosition'])
        parts.append(p)
    engine = copy.deepcopy(old[-1])
    engine['partState']['localPosition']['y'] = -1.2775 - (count - 1) * 1.875 - 1.7394
    engine['partState']['originalAttachLocalPosition'] = copy.deepcopy(engine['partState']['localPosition'])
    parts.append(engine)
    for i, p in enumerate(parts):
        nodes = []
        for node, adjacent, other in [('top', i-1, 'bottom'), ('bottom', i+1, 'top')]:
            if 0 <= adjacent < len(parts):
                state = copy.deepcopy(old[1]['partState']['attachNodeStates'][0])
                state.update(nodeId=node, attachedPartNodeID=other)
                state['AttachedPartGuid']['Guid'] = parts[adjacent]['PartGuid']['Guid']
                nodes.append(state)
        p['partState']['attachNodeStates'] = nodes
    assembly['parts'] = parts
    assembly['AssemblyOABConfig']['SymmetrySets'] = []
    assembly['AssemblyOABConfig']['PartRelationshipData'] = []
    assembly['SymmetrySets'] = []
    craft['Metadata']['Parts'] = len(parts)
    return craft


def main():
    p = argparse.ArgumentParser()
    p.add_argument('--templates', type=Path, required=True)
    p.add_argument('--stock', type=Path, required=True)
    p.add_argument('--out', type=Path, required=True)
    args = p.parse_args()
    load = lambda path: json.loads(path.read_text(encoding='utf-8-sig'))
    base = load(args.templates / 'craft-stack.json')
    crafts = {f'stack-{n:02}': stack(base, n) for n in [1, 6, 12]}
    for name, file in [('radial', 'craft-asparagus.json'), ('braced', 'craft-struts.json'), ('jet', 'craft-jet.json')]:
        crafts[name] = load(args.templates / file)
    for name, file in [('wide-k2', 'Stock Kerbal-K2.json'), ('sparrow', 'Stock RC-Sparrow.json')]:
        crafts[name] = load(args.stock / file)
    args.out.mkdir(parents=True, exist_ok=True)
    for name, craft in crafts.items():
        craft['Metadata']['Name'] = craft['Metadata']['VehicleName'] = 'Joint validation ' + name
        (args.out / (name + '.json')).write_text(json.dumps(craft), encoding='utf-8')
    (args.out / 'run.txt').write_text('\n'.join(crafts), encoding='ascii')
    print('Prepared ' + ', '.join(crafts))


if __name__ == '__main__':
    main()
