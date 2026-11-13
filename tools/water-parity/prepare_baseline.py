"""Build the same probe adapter against the pinned pre-fix revision without new API fields."""
import argparse
from pathlib import Path
import subprocess

def adapt(source):
    # Only remove adapter accesses to APIs absent in the baseline. Its production physics stays intact.
    for anchor in ('        let body_orbit =', '        let definition =', '            engine.water_reference ='):
        if source.count(anchor)!=1:raise ValueError(f'probe interface changed: {anchor}')
        start=source.index(anchor);end=source.index(';',start)+1
        source=source[:start]+source[end:]
    field='"buoyancy_center_m":definition.buoyancy_center_m'
    if source.count(field)!=1:raise ValueError('probe metadata interface changed')
    return source.replace(field,'"buoyancy_center_m":DVec3::ZERO')

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--worktree',type=Path,required=True);p.add_argument('--base',required=True)
    args=p.parse_args()
    head=subprocess.check_output(['git','-C',str(args.worktree),'rev-parse','HEAD'],text=True).strip()
    if head!=args.base:raise ValueError('baseline worktree is not the pinned revision')
    source=Path(__file__).resolve().parents[2]/'crates/ose-sim/examples/water_probe.rs'
    relative='crates/ose-sim/examples/water_probe.rs'
    tracked=subprocess.run(['git','-C',str(args.worktree),'ls-files','--error-unmatch',relative],capture_output=True)
    if tracked.returncode==0:raise ValueError('refusing to overwrite a tracked baseline file')
    target=args.worktree/relative
    target.write_text(adapt(source.read_text(encoding='utf-8')),encoding='utf-8')
    print(target)
