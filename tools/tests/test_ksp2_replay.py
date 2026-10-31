"""Replay every recorded KSP2 capture through the simulation and hold each to its declared status."""
import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).parents[2]
FIXTURES = ROOT / 'tools/ksp2-control-capture/fixtures'
spec = importlib.util.spec_from_file_location('compare_control_traces', ROOT / 'tools/compare_control_traces.py')
traces = importlib.util.module_from_spec(spec)
spec.loader.exec_module(traces)


def replay(craft, capture):
    craft_arg = str(FIXTURES / craft) if craft.endswith('.json') else craft
    out = subprocess.run(
        ['cargo', 'run', '-q', '-p', 'ose-sim', '--example', 'control_trace', '--', craft_arg, str(FIXTURES / capture)],
        cwd=ROOT, check=True, capture_output=True, text=True).stdout
    with tempfile.NamedTemporaryFile('w', suffix='.csv', delete=False, newline='') as f:
        f.write(out)
    try:
        return traces.read(Path(f.name))
    finally:
        Path(f.name).unlink()


class Ksp2ReplayTests(unittest.TestCase):
    def test_every_capture_is_registered(self):
        registered = {f['capture'] for f in json.loads((FIXTURES / 'replay.json').read_text())['fixtures']}
        captures = {p.name for p in FIXTURES.glob('*.csv') if p.name.count('.') == 1}
        self.assertEqual(captures - registered, set())

    def test_replays_meet_their_declared_status(self):
        for fixture in json.loads((FIXTURES / 'replay.json').read_text())['fixtures']:
            with self.subTest(fixture['capture']):
                reference = traces.read(FIXTURES / fixture['capture'])
                reference = traces.unity_to_ose(reference)
                result = traces.compare(reference, replay(fixture['craft'], fixture['capture']))
                if fixture['expect'] == 'pass':
                    self.assertTrue(result['passed'], result)
                else:
                    # A capped fixture that starts passing should be promoted, not left capped.
                    self.assertFalse(result['passed'], 'now passes: mark it "pass"')
                    self.assertLessEqual(result['max_angle_deg'], fixture['max_angle_deg'], result)
                    self.assertLessEqual(result['max_omega_rad_s'], fixture['max_omega_rad_s'], result)


if __name__ == '__main__':
    unittest.main()
