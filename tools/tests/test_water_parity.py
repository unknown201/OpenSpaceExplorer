import copy
import math
import unittest
import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location('water_parity_analyze', Path(__file__).resolve().parents[1] / 'water-parity/analyze.py')
analyze = importlib.util.module_from_spec(spec)
spec.loader.exec_module(analyze)

def traces():
    result={name:[{'scenario':name,'t':i*.02,'alive':True,'splashed':True,'origin_alt_m':0.,'vertical_mps':0.,'horizontal_mps':0.,'bank_deg':0.} for i in range(round(duration/.02)+1)] for name,duration in analyze.CASES.items()}
    for row in result['drop']:
        row['vertical_mps']=math.cos(math.tau*row['t'])
        row['origin_alt_m']=math.sin(math.tau*row['t'])/math.tau
    return result

class ComparisonTests(unittest.TestCase):
    def test_identical_complete_traces_pass(self):
        a=traces()
        self.assertTrue(all(c['pass'] for c in analyze.compare(a,a)))

    def test_stationary_traces_do_not_establish_bobbing(self):
        a=traces()
        for row in a['drop']:row['vertical_mps']=0.;row['origin_alt_m']=0.
        self.assertFalse(analyze.compare(a,a)[0]['pass'])

    def test_terminal_destruction_can_end_a_trace_early(self):
        a=traces()
        a['horizontal_destroy']=a['horizontal_destroy'][:2]
        a['horizontal_destroy'][-1]['alive']=False
        analyze.validate(a,'test')

    def test_depth_error_is_not_fitted_away(self):
        a=traces();b=copy.deepcopy(a)
        for r in b['drop']:r['origin_alt_m']+=.3
        self.assertFalse(analyze.compare(a,b)[0]['pass'])

    def test_quiet_tail_cannot_hide_a_splashdown_error(self):
        a=traces();b=copy.deepcopy(a)
        for r in b['drop']:
            if r['t']<1.:r['origin_alt_m']+=.5
        checks=analyze.compare(a,b)[0]['checks']
        full=next(c for c in checks if c['metric']=='origin_alt_m_rmse')
        transient=next(c for c in checks if c['metric']=='origin_alt_m_first_10s_rmse')
        self.assertTrue(full['pass'])
        self.assertFalse(transient['pass'])

    def test_baseline_adapter_only_removes_new_interfaces(self):
        path=Path(__file__).resolve().parents[1]/'water-parity/prepare_baseline.py'
        spec=importlib.util.spec_from_file_location('water_parity_baseline',path)
        baseline=importlib.util.module_from_spec(spec);spec.loader.exec_module(baseline)
        source=(path.parents[1].parent/'crates/ose-sim/examples/water_probe.rs').read_text(encoding='utf-8')
        old=baseline.adapt(source)
        self.assertNotIn('engine.water_reference',old)
        self.assertIn('s.configure_surface',old)
        self.assertIn('u.set_crew',old)

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
