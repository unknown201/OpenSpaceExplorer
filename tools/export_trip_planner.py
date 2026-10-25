#!/usr/bin/env python
"""Copy the original's trip planner graph (data/ui/TripPlanner.json: delta-v between surfaces, orbits and
intercepts of every body, with aerobraking and transfer angles) to assets/web/ui/trip_planner.json for the
client's trip planner (docs/ui-parity-ksp2.md). Push with `tools/push_assets.py ui`.

    .venv/Scripts/python tools/export_trip_planner.py
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def main() -> int:
    src = ROOT / "assets" / "ksp2" / "data" / "ui" / "TripPlanner.json"
    if not src.exists():
        sys.exit(f"export_trip_planner: {src} is missing (run the data import first)")
    data = json.loads(src.read_text(encoding="utf-8-sig"))
    out = ROOT / "assets" / "web" / "ui" / "trip_planner.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")
    print(f"trip planner: {len(data['TripNodes'])} nodes, {len(data['TripEdges'])} edges -> {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
