"""Compare the KSP2 and OSE jet throttle-step traces (KSP2 parity T41).

    python tools/compare_jet_traces.py <ksp2 jet-trace.csv> <ose parity_jet.csv>

Both runs follow the same schedule (tools/ksp2-parity-pilot/Jet.cs, crates/ose-bot/examples/parity_jet.rs).
Times are aligned on the start of the "static-up" phase. Prints thrust and speed at fixed marks after
each phase starts, plus the spool-up time constant.
"""
import csv
import sys

PHASES = ["static-up", "static-down", "roll", "reverse", "stop"]
MARKS = [0.5, 1, 2, 3, 5, 10, 15, 20, 25]


def load(path, thrust_col):
    rows = []
    for r in csv.DictReader(open(path, encoding="utf-8")):
        rows.append({
            "t": float(r["t"]),
            "phase": r["phase"],
            "thrust": float(r[thrust_col]),
            "speed": float(r["srf_speed"]),
        })
    starts = {}
    for r in rows:
        starts.setdefault(r["phase"], r["t"])
    t0 = starts["static-up"]
    for r in rows:
        r["t"] -= t0
    return rows, {k: v - t0 for k, v in starts.items()}


def at(rows, t):
    best = min(rows, key=lambda r: abs(r["t"] - t))
    return best


def rise_time(rows, start, end, frac):
    """Seconds after `start` for thrust to reach `frac` of its value at `end`."""
    target = at(rows, end)["thrust"] * frac
    for r in rows:
        if start <= r["t"] <= end and r["thrust"] >= target:
            return r["t"] - start
    return float("nan")


def main():
    k_rows, k_start = load(sys.argv[1], "final_thrust_kn")
    o_rows, o_start = load(sys.argv[2], "thrust_kn")
    print(f"{'phase':12} {'+s':>5} | {'KSP2 kN':>8} {'OSE kN':>8} {'diff':>7} | {'KSP2 m/s':>8} {'OSE m/s':>8}")
    for i, ph in enumerate(PHASES):
        if ph not in k_start or ph not in o_start:
            continue
        nxt = k_start.get(PHASES[i + 1], 1e9) if i + 1 < len(PHASES) else k_start[ph] + 8
        for m in MARKS:
            if k_start[ph] + m >= nxt:
                break
            k = at(k_rows, k_start[ph] + m)
            o = at(o_rows, o_start[ph] + m)
            print(f"{ph:12} {m:5.1f} | {k['thrust']:8.2f} {o['thrust']:8.2f} {o['thrust'] - k['thrust']:7.2f} | "
                  f"{k['speed']:8.1f} {o['speed']:8.1f}")
    for name, rows, s in (("KSP2", k_rows, k_start), ("OSE", o_rows, o_start)):
        print(f"{name}: static-up reaches 63% of its 20 s thrust after {rise_time(rows, 0, 19.9, 0.632):.2f} s")


if __name__ == "__main__":
    main()
