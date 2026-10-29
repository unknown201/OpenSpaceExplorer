"""Export only the reference's rigid-body inertia settings from its loaded JSON."""
import argparse
import hashlib
import json
import math
from pathlib import Path


def convert(raw):
    source=json.loads(raw)
    result={'version':source['VersionString'],'source_sha256':hashlib.sha256(raw).hexdigest(),
            'enabled':bool(source['ENABLE_INERTIA_TENSOR_SCALING'])}
    for key,name in [('scalar','GLOBAL_TENSOR_SCALAR'),('low_mass_scalar','GLOBAL_LOWMASS_TENSOR_SCALAR'),('low_mass_limit_t','GLOBAL_LOWMASS_TENSOR_LIMIT')]:
        value=float(source[name])
        if not math.isfinite(value) or value<=0:raise ValueError('invalid inertia setting: '+name)
        result[key]=value
    return result


if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--settings',type=Path,default=Path.home()/'AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Global/PhysicsSettings.json')
    p.add_argument('--out',type=Path,default=Path(__file__).resolve().parents[1]/'assets/own/inertia-settings.json')
    a=p.parse_args();a.out.write_text(json.dumps(convert(a.settings.read_bytes()),indent=2)+'\n')
