"""Hash the accepted run and dirty working tree inputs. No game-derived data is committed."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess

p=argparse.ArgumentParser()
p.add_argument('root',type=Path)
p.add_argument('--game',type=Path,required=True)
a=p.parse_args()
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
inputs=[Path('assets/own/parts-catalog.json'),Path('assets/own/kerbol-system.json'),Path('assets/own/resources.json'),
        Path('crates/ose-sim/src/water.rs'),Path('crates/ose-sim/src/surface.rs'),Path('crates/ose-sim/src/flight.rs'),
        Path('crates/ose-sim/src/structure.rs'),Path('crates/ose-sim/examples/water_probe.rs'),
        Path('tools/water-parity/WaterParity.cs'),Path('tools/water-parity/analyze.py'),
        a.root/'craft.json',a.root/'build/OSE.WaterParity.dll',a.game/'KSP2_x64.exe',a.game/'UnityPlayer.dll',
        a.game/'KSP2_x64_Data/Managed/Assembly-CSharp.dll']
inputs += list((a.root/'native').glob('*'))+[a.root/'ose-samples.jsonl',a.root/'ose-events.json']
record={'revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
        'dirty_tree':subprocess.check_output(['git','status','--porcelain'],text=True).splitlines(),
        'sha256':{str(f):sha(f) for f in inputs if f.is_file()}}
(a.root/'provenance.json').write_text(json.dumps(record,indent=2),encoding='utf-8')
print(a.root/'provenance.json')
