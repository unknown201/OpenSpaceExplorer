"""Export buoyancy, water drag and water-impact settings from the reference's loaded PhysicsSettings.json."""
import argparse
import hashlib
import json
import math
from pathlib import Path

FIELDS = {
    "buoyancy_scalar": "BuoyancyScalar",
    "scale_above_depth_m": "BuoyancyScaleAboveDepth",
    "default_volume_m3": "BuoyancyDefaultVolume",
    "min_crash_mult": "BuoyancyMinCrashMult",
    "crash_tolerance_mult": "BuoyancyCrashToleranceMult",
    "drag_scalar": "BuoyancyWaterDragScalar",
    "drag_scalar_end": "BuoyancyWaterDragScalarEnd",
    "drag_scalar_lerp": "BuoyancyWaterDragScalarLerp",
    "drag_scalar_lerp_dot_mult_base": "BuoyancyWaterDragScalarLerpDotMultBase",
    "drag_scalar_lerp_dot_mult": "BuoyancyWaterDragScalarLerpDotMult",
    "drag_min_vel": "BuoyancyWaterDragMinVel",
    "drag_min_vel_mult": "BuoyancyWaterDragMinVelMult",
    "drag_part_vel_greater_vessel_mult": "BuoyancyWaterDragPartVelGreaterVesselMult",
    "drag_timer_s": "BuoyancyWaterDragTimer",
    "drag_mult_min_for_min_dot": "BuoyancyWaterDragMultMinForMinDot",
    "angular_drag_min_control_sqr_mag": "BuoyancyAngularDragMinControlSqrMag",
    "angular_drag_slow": "BuoyancyWaterAngularDragSlow",
    "drag_slow": "BuoyancyWaterDragSlow",
}

def convert(raw):
    data = json.loads(raw)
    result = {"version": data["VersionString"], "source_sha256": hashlib.sha256(raw).hexdigest()}
    for key, source in FIELDS.items():
        value = float(data[source])
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"Invalid water setting: {source}")
        result[key] = value
    return result

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--settings", type=Path, default=Path.home()/"AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Global/PhysicsSettings.json")
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1]/"assets/own/water-settings.json")
    args = parser.parse_args()
    result = convert(args.settings.read_bytes())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8")
    print(f"Exported {len(FIELDS)} water settings, reference settings {result['version']}")

if __name__ == "__main__":
    main()
