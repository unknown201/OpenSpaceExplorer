import hashlib
import json
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from export_joint_settings import FIELDS,convert

class JointSettingsTests(unittest.TestCase):
    def test_independent_stack_surface_and_rigid_values_and_source_hash_are_preserved(self):
        data={name:float(index+1) for index,name in enumerate(FIELDS.values())}
        data["VersionString"]="fixture"
        raw=json.dumps(data).encode()
        converted=convert(raw)
        self.assertEqual(converted["source_sha256"],hashlib.sha256(raw).hexdigest())
        self.assertEqual(converted["version"],"fixture")
        for output,source in FIELDS.items():self.assertEqual(converted[output],data[source])
        self.assertNotEqual(converted["stack_attach_stiffness"],converted["surface_attach_stiffness"])

    def test_missing_negative_and_nonfinite_settings_fail(self):
        for value in [-1,float("nan"),float("inf")]:
            data={name:1 for name in FIELDS.values()};data["VersionString"]="fixture"
            data["JOINT_RIGIDITY"]=value
            with self.assertRaises(ValueError):convert(json.dumps(data).encode())
        with self.assertRaises(KeyError):convert(b'{"VersionString":"fixture"}')

if __name__=="__main__":unittest.main()
