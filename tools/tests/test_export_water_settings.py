import hashlib
import json
import sys
import unittest
from pathlib import Path
sys.path.insert(0,str(Path(__file__).resolve().parents[1]))
from export_water_settings import FIELDS,convert

class WaterSettingsTests(unittest.TestCase):
    def test_every_value_and_the_source_hash_are_preserved(self):
        data={name:float(index+1) for index,name in enumerate(FIELDS.values())}
        data["VersionString"]="fixture"
        raw=json.dumps(data).encode()
        converted=convert(raw)
        self.assertEqual(converted["source_sha256"],hashlib.sha256(raw).hexdigest())
        self.assertEqual(converted["version"],"fixture")
        for output,source in FIELDS.items():self.assertEqual(converted[output],data[source])

    def test_missing_negative_and_nonfinite_settings_fail(self):
        for value in [-1,float("nan"),float("inf")]:
            data={name:1 for name in FIELDS.values()};data["VersionString"]="fixture"
            data["BuoyancyScalar"]=value
            with self.assertRaises(ValueError):convert(json.dumps(data).encode())
        with self.assertRaises(KeyError):convert(b'{"VersionString":"fixture"}')

if __name__=="__main__":unittest.main()
