"""Scene transform regressions: no game install or UnityPy required."""
from pathlib import Path
import math
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from export_menu_scene import decompose, mul, trs  # noqa: E402


def rot_y(a):
    c, s = math.cos(a), math.sin(a)
    return [[c, 0.0, s, 0.0], [0.0, 1.0, 0.0, 0.0], [-s, 0.0, c, 0.0], [0.0, 0.0, 0.0, 1.0]]


def scale(x, y, z):
    return [[x, 0.0, 0.0, 0.0], [0.0, y, 0.0, 0.0], [0.0, 0.0, z, 0.0], [0.0, 0.0, 0.0, 1.0]]


def translate(m, t):
    m = [row[:] for row in m]
    for i in range(3):
        m[i][3] = t[i]
    return m


class DecomposeTest(unittest.TestCase):
    def assert_same(self, a, b):
        for i in range(3):
            for j in range(4):
                self.assertAlmostEqual(a[i][j], b[i][j], places=9)

    def test_a_turned_prefab_round_trips(self):
        m = translate(mul(rot_y(0.7), scale(1.1, 1.2, 1.2)), (10.0, -5.0, 3.0))
        t, q, s = decompose(m)
        self.assertTrue(all(x > 0 for x in s))
        self.assert_same(trs(t, q, s), m)

    def test_a_mirrored_prefab_keeps_its_mirror_in_the_scale_not_a_tilt(self):
        # the KSC dock's warehouse, crane and HVAC units and the pads' hypergolic facilities are placed with a
        # negative scale; read as a rotation the reflection came out tilted
        for sx, sy, sz in [(-1.0, 1.0, 1.0), (1.0, 1.0, -1.0), (-1.087, 1.164, 1.164)]:
            m = translate(mul(rot_y(2.1), scale(sx, sy, sz)), (2836.9, -193.5, -1459.7))
            t, q, s = decompose(m)
            self.assertLess(s[0], 0.0)
            self.assert_same(trs(t, q, s), m)
            # still standing: the rotation keeps the model's up axis up
            up = trs((0.0, 0.0, 0.0), q, (1.0, 1.0, 1.0))
            self.assertAlmostEqual(abs(up[1][1]), 1.0, places=9)


if __name__ == "__main__":
    unittest.main()
