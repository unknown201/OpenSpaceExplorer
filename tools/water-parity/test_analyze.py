import copy
import unittest
import analyze

def traces():
    return {name:[{'scenario':name,'t':i*.02,'alive':True,'splashed':True,'origin_alt_m':0.,'vertical_mps':0.,'horizontal_mps':0.,'bank_deg':0.} for i in range(round(duration/.02)+1)] for name,duration in analyze.CASES.items()}

class ComparisonTests(unittest.TestCase):
    def test_identical_complete_traces_pass(self):
        a=traces()
        self.assertTrue(all(c['pass'] for c in analyze.compare(a,a)))

    def test_depth_error_is_not_fitted_away(self):
        a=traces();b=copy.deepcopy(a)
        for r in b['drop']:r['origin_alt_m']+=.3
        self.assertFalse(analyze.compare(a,b)[0]['pass'])

    def test_survival_difference_fails(self):
        a=traces();b=copy.deepcopy(a)
        for r in b['vertical_destroy'][10:]:r['alive']=False
        result=next(c for c in analyze.compare(a,b) if c['scenario']=='vertical_destroy')
        self.assertFalse(result['pass'])

    def test_missing_tail_gap_and_nonfinite_are_incomplete(self):
        for change in ['tail','gap','nan']:
            a=traces()
            if change=='tail':a['drop']=a['drop'][:-20]
            elif change=='gap':del a['drop'][50:60]
            else:a['drop'][10]['origin_alt_m']=float('nan')
            with self.assertRaises(ValueError):analyze.validate(a,'test')

if __name__=='__main__':unittest.main()
