"""Compare two KSP2 captures of the same fixture against each other, in the vessel's body frame.

    python tools/ksp2_repeatability.py first.csv second.csv

Captures start from different orbital attitudes, so world-frame attitudes are not comparable; body-frame
angular velocity is. The result is KSP2's own run-to-run spread for identical craft and inputs.
"""
import csv
import json
import math
import sys


def body_rates(path):
    rows = list(csv.DictReader(open(path, newline='')))
    out = []
    for r in rows:
        x, y, z = (-float(r[k]) for k in ('qx', 'qy', 'qz'))
        w = float(r['qw'])
        v = [float(r[k]) for k in ('wx', 'wy', 'wz')]
        # rotate the world rate by the conjugate attitude
        tx, ty, tz = 2 * (y * v[2] - z * v[1]), 2 * (z * v[0] - x * v[2]), 2 * (x * v[1] - y * v[0])
        out.append((v[0] + w * tx + y * tz - z * ty, v[1] + w * ty + z * tx - x * tz, v[2] + w * tz + x * ty - y * tx))
    inputs = [(r['pitch'], r['yaw'], r['roll']) for r in rows]
    return out, inputs


def main():
    a, ia = body_rates(sys.argv[1])
    b, ib = body_rates(sys.argv[2])
    if len(a) != len(b) or ia != ib:
        raise SystemExit('captures differ in length or inputs')
    d = [math.dist(p, q) for p, q in zip(a, b)]
    print(json.dumps({'samples': len(d), 'max_body_omega_rad_s': max(d),
                      'max_body_omega_first_half_rad_s': max(d[:len(d) // 2])}, indent=2))


if __name__ == '__main__':
    main()
