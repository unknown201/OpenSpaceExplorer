import copy
import sys
import unittest
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from export_aero_settings import curve


class AeroCurveTests(unittest.TestCase):
    def fixture(self):
        return {"fCurve": {"preWrapMode":"ClampForever", "postWrapMode":"ClampForever", "keys":[
            {"time":0,"value":2,"inTangent":1,"outTangent":3,"weightedMode":"None"},
            {"time":1,"value":4,"inTangent":5,"outTangent":7,"weightedMode":"None"}]}}

    def test_curve_preserves_authored_values_and_both_derivatives(self):
        self.assertEqual(curve(self.fixture()), {"keys":[
            {"t":0.0,"v":2.0,"in_tangent":1.0,"out_tangent":3.0},
            {"t":1.0,"v":4.0,"in_tangent":5.0,"out_tangent":7.0}]})

    def test_unsupported_interpolation_and_invalid_keys_fail_instead_of_being_approximated(self):
        cases=[]
        value=self.fixture();value["fCurve"]["postWrapMode"]="Loop";cases.append(value)
        value=self.fixture();value["fCurve"]["keys"][0]["weightedMode"]="Both";cases.append(value)
        value=self.fixture();value["fCurve"]["keys"][1]["time"]=0;cases.append(value)
        value=self.fixture();value["fCurve"]["keys"][0]["value"]=float("nan");cases.append(value)
        value=self.fixture();value["fCurve"]["keys"]=[];cases.append(value)
        for value in cases:
            with self.assertRaises(ValueError):curve(value)


if __name__=="__main__":unittest.main()
