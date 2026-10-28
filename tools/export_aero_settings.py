"""Export simulation aerodynamic settings from KSP2's loaded Global/PhysicsSettings.json.

Reads only the selected aerodynamic fields; never copies local paths, general settings or source code.
"""
import argparse
import hashlib
import json
import math
from pathlib import Path


def curve(value):
    f = value["fCurve"]
    if any(f.get(k, "ClampForever") != "ClampForever" for k in ["preWrapMode", "postWrapMode"]):
        raise ValueError("Only clamped reference curves are supported")
    keys = []
    for k in f["keys"]:
        if k.get("weightedMode", "None") != "None":
            raise ValueError("Weighted reference curves require a weighted evaluator")
        row = {a: float(k[b]) for a, b in {"t": "time", "v": "value", "in_tangent": "inTangent", "out_tangent": "outTangent"}.items()}
        if not all(math.isfinite(v) for v in row.values()) or keys and row["t"] <= keys[-1]["t"]:
            raise ValueError("Invalid or unordered curve keys")
        keys.append(row)
    if not keys:
        raise ValueError("Empty reference curve")
    return {"keys": keys}


def convert(raw):
    data = json.loads(raw)
    result = {"version": data["VersionString"], "source_sha256": hashlib.sha256(raw).hexdigest()}
    for name in ["LiftMultiplier", "LiftDragMultiplier", "BodyLiftMultiplier", "DragMultiplier", "DragCubeMultiplier"]:
        snake = {"LiftMultiplier":"lift_multiplier", "LiftDragMultiplier":"lift_drag_multiplier", "BodyLiftMultiplier":"body_lift_multiplier", "DragMultiplier":"drag_multiplier", "DragCubeMultiplier":"drag_cube_multiplier"}[name]
        result[snake] = float(data[name])
        if not math.isfinite(result[snake]) or result[snake] < 0:
            raise ValueError("Invalid aerodynamic multiplier")
    result["drag_curves"] = {key: curve(data[name]) for key, name in {
        "tip":"DragCurveTip", "surface":"DragCurveSurface", "tail":"DragCurveTail", "multiplier":"DragCurveMultiplier",
        "cd":"DragCurveCd", "cd_power":"DragCurveCdPower", "pseudo_reynolds":"DragCurvePseudoReynolds"}.items()}
    result["lift_curves"] = {}
    for name in ["DefaultLiftingSurfaceCurve", "BodyLiftLiftingSurfaceCurve", "SpeedBrakeLiftingSurfaceCurve", "CapsuleBottomLiftingSurfaceCurve"]:
        value = data[name]
        result["lift_curves"][value["name"]] = {key: curve(value[source]) for key, source in {
            "lift":"liftCurve", "lift_mach":"liftMachCurve", "drag":"dragCurve", "drag_mach":"dragMachCurve"}.items()}
    return result


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--settings", type=Path, default=Path.home()/"AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Global/PhysicsSettings.json")
    p.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1]/"assets/own/aero-settings.json")
    a = p.parse_args()
    result = convert(a.settings.read_bytes())
    a.out.parent.mkdir(parents=True, exist_ok=True)
    a.out.write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8")
    print(f"Exported {len(result['drag_curves'])} drag curves and {len(result['lift_curves'])} lift families, reference settings {result['version']}")


if __name__ == "__main__":
    main()
