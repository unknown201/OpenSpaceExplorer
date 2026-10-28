"""Export joint spring and break multipliers from the reference's loaded PhysicsSettings.json."""
import argparse
import hashlib
import json
import math
from pathlib import Path

FIELDS = {
    "rigidity": "JOINT_RIGIDITY",
    "stack_node_factor": "JOINT_STACK_NODE_FACTOR",
    "surface_node_factor": "JOINT_SURFACE_NODE_FACTOR",
    "stack_attach_stiffness": "JOINT_STACK_ATTACH_STIFFNESS",
    "surface_attach_stiffness": "JOINT_SURFACE_ATTACH_STIFFNESS",
    "damping_factor": "JointDampingFactor",
    "break_force_factor": "JointBreakForceFactor",
    "break_torque_factor": "JointBreakTorqueFactor",
    "rigid_break_force_factor": "RigidJointBreakForceFactor",
    "rigid_break_torque_factor": "RigidJointBreakTorqueFactor",
}

def convert(raw):
    data = json.loads(raw)
    result = {"version": data["VersionString"], "source_sha256": hashlib.sha256(raw).hexdigest()}
    for key, source in FIELDS.items():
        value = float(data[source])
        if not math.isfinite(value) or value < 0:
            raise ValueError(f"Invalid joint setting: {source}")
        result[key] = value
    return result

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--settings", type=Path, default=Path.home()/"AppData/LocalLow/Intercept Games/Kerbal Space Program 2/Global/PhysicsSettings.json")
    parser.add_argument("--out", type=Path, default=Path(__file__).resolve().parents[1]/"assets/own/joint-settings.json")
    args = parser.parse_args()
    result = convert(args.settings.read_bytes())
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, indent=2)+"\n", encoding="utf-8")
    print(f"Exported {len(FIELDS)} joint settings, reference settings {result['version']}")

if __name__ == "__main__":
    main()
