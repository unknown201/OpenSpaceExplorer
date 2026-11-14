"""Trim a `reentryorbit` KSP2 run (reference/ksp2-reentryorbit-*) into the small files the ose-sim parity test
reads (crates/ose-sim/tests/data/ksp2-reentryorbit-*):

  start.csv     the trace row where the craft first passes 71 km on the way down (where OSE starts)
  attitude.csv  from there on, every `every` ticks: KSP2's nose as components along the orbital velocity, the
                local vertical square to it, and the side (the attitude OSE replays)
  parts.csv     each part's temperature every second (by craft index; the names are in events.csv) (and every tick in the 2 s before a part is lost)
  events.csv    the part list changes, as logged

    python -B tools/ksp2-parity-pilot/trim_reentry_orbit.py <run dir> <out dir> [every] [start altitude, m]
"""

import csv
import math
import os
import sys

START_ALT_M = 71_000.0


def unit(v):
    n = math.sqrt(sum(c * c for c in v))
    return [c / n for c in v]


def cross(a, b):
    return [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]


def dot(a, b):
    return sum(x * y for x, y in zip(a, b))


def nose_components(r):
    x, y, z, w = (float(r[k]) for k in ("qx", "qy", "qz", "qw"))
    # the control transform's up axis (0, 1, 0) turned by the quaternion
    nose = [2 * (x * y - w * z), 1 - 2 * (x * x + z * z), 2 * (y * z + w * x)]
    along = unit([float(r[k]) for k in ("vx", "vy", "vz")])
    down = unit([float(r[k]) for k in ("gx", "gy", "gz")])
    side = unit(cross(along, [-c for c in down]))
    up = cross(side, along)
    return dot(nose, along), dot(nose, up), dot(nose, side)


def main(run, out, every=5, start_alt=START_ALT_M):
    every, start_alt = int(every), float(start_alt)
    os.makedirs(out, exist_ok=True)
    with open(os.path.join(run, "reentryorbit-trace.csv"), newline="") as f:
        trace = list(csv.DictReader(f))
    k0 = next(i for i, r in enumerate(trace) if float(r["alt_m"]) <= start_alt and float(r["vspeed"]) < 0)
    t0 = float(trace[k0]["t"])
    keep = ["t", "alt_m", "vspeed", "srf_speed", "mach", "density", "temp_max_k", "situation"]
    keep += ["qx", "qy", "qz", "qw", "wx", "wy", "wz", "vx", "vy", "vz", "gx", "gy", "gz"]
    with open(os.path.join(out, "start.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(keep)
        w.writerow([trace[k0][k] for k in keep])
    with open(os.path.join(out, "attitude.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(["t", "along", "up", "side"])
        for r in trace[max(0, k0 - 5)::every]:
            a, b, s = nose_components(r)
            w.writerow([f"{float(r['t']):.2f}", f"{a:.4f}", f"{b:.4f}", f"{s:.4f}"])
    with open(os.path.join(run, "reentryorbit-events.csv"), newline="") as f:
        events = f.read()
    with open(os.path.join(out, "events.csv"), "w", newline="") as f:
        f.write(events)
    losses = [float(line.split(",")[0]) for line in events.splitlines()[2:] if line.strip()]
    with open(os.path.join(run, "reentryorbit-parts.csv"), newline="") as f:
        parts = list(csv.DictReader(f))
    cols = ["t", "alt_m", "srf_speed", "index", "temp_k", "reentry_flux"]
    if parts and "exposed" in parts[0]:
        cols.append("exposed")
    with open(os.path.join(out, "parts.csv"), "w", newline="") as f:
        w = csv.writer(f)
        w.writerow(cols)
        last = None
        for r in parts:
            t = float(r["t"])
            if t < t0 - 1:
                continue
            near_loss = any(lt - 2 <= t <= lt for lt in losses)
            if near_loss or last is None or t - last >= 1.0 or t == last:
                last = t
                row = dict(r)
                row["alt_m"] = f"{float(r['alt_m']):.1f}"
                row["srf_speed"] = f"{float(r['srf_speed']):.2f}"
                row["temp_k"] = f"{float(r['temp_k']):.2f}"
                row["reentry_flux"] = f"{float(r['reentry_flux']):.3f}"
                if "exposed" in row:
                    row["exposed"] = f"{float(r['exposed']):.4f}"
                w.writerow([row[c] for c in cols])
    print(f"{out}: starts at t {t0:.2f} ({float(trace[k0]['alt_m']):.0f} m)")


if __name__ == "__main__":
    main(*sys.argv[1:])
