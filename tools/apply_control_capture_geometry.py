"""Apply measured part geometry to a named stock craft, without fitting control gains.

The parts CSV must match the stock part order, type, position and mount rotation.
Runtime principal inertia is unscaled using the shipped inverse-mass scalar.
No game files are changed. Re-run the craft importer after regenerating its sidecar.
"""
import argparse
import csv
import json
from pathlib import Path
import numpy as np
from scipy.spatial.transform import Rotation

ROOT = Path(__file__).resolve().parents[1]


def vector(row, names):
    result=np.array([float(row[k]) for k in names])
    if not np.isfinite(result).all(): raise ValueError('nonfinite capture geometry')
    return result


def xyz(v):
    return dict(zip('xyz', map(float, v)))


def apply(craft, parts=None, surfaces=None, scalar=5.):
    if not np.isfinite(scalar) or scalar<=0:raise ValueError('invalid reference inertia scalar')
    selected=craft['parts']
    if parts is not None:
        if len(parts)!=len(selected): raise ValueError('part counts differ')
        for part, row in zip(selected,parts):
            if row['part_id']!=part['part_id']: raise ValueError('part types/order differ')
            if np.linalg.norm(vector(row,['px','py','pz'])-np.array([part['local_position'][k] for k in 'xyz']))>0.001:
                raise ValueError('mount positions differ')
            mount=vector(row,['qx','qy','qz','qw'])
            expected=np.array([part['local_rotation'][k] for k in 'xyzw'],dtype=float)
            if min(np.linalg.norm(mount),np.linalg.norm(expected))<1e-10:raise ValueError('zero mount quaternion')
            mount/=np.linalg.norm(mount);expected/=np.linalg.norm(expected)
            if abs(mount@expected)<0.999999: raise ValueError('mount rotations differ')
            rot=Rotation.from_quat(vector(row,['iqx','iqy','iqz','iqw'])).as_matrix()
            inertia=rot@np.diag(vector(row,['ix','iy','iz']))@rot.T/scalar
            dry=float(row['dry_t'])
            if not np.isfinite(dry) or not 0.001<=dry<=1e6:raise ValueError('invalid measured dry mass')
            if np.linalg.eigvalsh(inertia).min()<=0:raise ValueError('nonphysical measured inertia')
            part['flight']={'part_id':part['part_id'],'dry_mass_t':dry,
                            'mass_offset_m':xyz(vector(row,['com_x','com_y','com_z'])),
                            'inertia_per_t':[xyz(inertia[:,i]) for i in range(3)],'surface':None}
    if surfaces is not None:
        # The recorder enumerates the same PartOwner insertion order as the craft importer.
        expected=[p for p in selected if p['part_id'] in {r['part_id'] for r in surfaces}]
        if len(expected)!=len(surfaces): raise ValueError('surface counts differ')
        for part,row in zip(expected,surfaces):
            if part['part_id']!=row['part_id']: raise ValueError('surface part types/order differ')
            coefficients=vector(row,['fixed_coeff','moving_coeff','range_deg','speed'])
            if (coefficients<0).any():raise ValueError('invalid surface coefficient')
            f=part.setdefault('flight',None)
            if f is None:
                f={'part_id':part['part_id'],'dry_mass_t':None,'mass_offset_m':None,'inertia_per_t':None,'surface':None}
                part['flight']=f
            f['surface']={'fixed_lift_coeff':float(row['fixed_coeff']),'moving_lift_coeff':float(row['moving_coeff']),
                          'range_deg':float(row['range_deg']),'speed':float(row['speed']),
                          'pressure_offset_m':xyz(vector(row,['pressure_x','pressure_y','pressure_z'])),
                          'pivot_m':xyz(vector(row,['pivot_x','pivot_y','pivot_z']))}


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('craft');p.add_argument('--parts',type=Path);p.add_argument('--surfaces',type=Path)
    p.add_argument('--source-assembly',type=Path,required=True,help='Original stock assembly: measurements are restricted to these exact source bytes')
    a=p.parse_args()
    read=lambda path:list(csv.DictReader(path.open(newline=''))) if path else None
    path=ROOT/'assets/own/stock-craft.json';crafts=json.loads(path.read_text())
    craft=next(c for c in crafts if c['name']==a.craft)
    scalar=json.loads((ROOT/'assets/own/inertia-settings.json').read_text())['scalar']
    apply(craft,read(a.parts),read(a.surfaces),scalar)
    assembly=json.loads(a.source_assembly.read_text(encoding='utf-8-sig'))
    mirror={p['PartGuid']['Guid']:p['partState'].get('isMirrored',False) for p in assembly['Assemblies'][0]['Assembly']['parts']}
    for part in craft['parts']:
        if part.get('flight'):part['flight']['mirrored']=mirror[part['guid']]
    sidecar=ROOT/'assets/own/stock-flight-geometry.json'
    overrides=json.loads(sidecar.read_text()) if sidecar.exists() else {}
    h=0xcbf29ce484222325
    for byte in a.source_assembly.read_bytes():h=((h^byte)*0x100000001b3)&0xffffffffffffffff
    overrides[craft['name']]={'source_fnv1a64':f'{h:016x}', 'parts':[{'guid':p['guid'],'part_id':p['part_id'],'flight':p['flight']} for p in craft['parts'] if p.get('flight')]}
    path.write_text(json.dumps(crafts,separators=(',',':'))+'\n')
    sidecar.write_text(json.dumps(overrides,indent=2)+'\n')
    print('Applied measured geometry:',craft['name'],len(overrides[craft['name']]['parts']),'parts')


if __name__=='__main__':main()
