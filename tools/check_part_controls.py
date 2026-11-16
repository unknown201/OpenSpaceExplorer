"""Audit exported control animations, authored audio cues and lamp metadata.

Run after export_web_assets.py and export_audio.py. Requires the user's local assets.
"""
import json
from pathlib import Path
import struct

ROOT = Path(__file__).resolve().parents[1]


def main():
    catalog = json.loads((ROOT / "assets/own/parts-catalog.json").read_text(encoding="utf-8"))
    sounds = json.loads((ROOT / "assets/web/audio/sounds.json").read_text())
    counts = {"gear": 0, "panels": 0, "antennas": 0, "lights": 0, "cues": 0}
    for part in catalog:
        deployment = part.get("deployable")
        control = "antennas" if deployment and deployment["antenna"] and deployment["extendable"] else "panels" if part["electric"]["solar_deployable"] else "gear" if (part.get("landing_gear") or {}).get("retractable") else None
        if not control and not part.get("light"):
            continue
        file = ROOT / "assets/web/parts" / (part["id"] + ".glb")
        raw = file.read_bytes()
        model = json.loads(raw[20:20 + struct.unpack_from("<I", raw, 12)[0]])
        extras = model["extras"]
        if control:
            spec = extras["ose_deployment"]
            assert spec["control"] == control, part["id"]
            counts[control] += 1
            indices = [spec["clip"]]
            if deployment["retractable"]:
                indices.append(spec["close_clip"])
            for index in indices:
                clip = model["animations"][index]
                assert clip["channels"], (part["id"], "no moving channels")
                for cue in clip["extras"]["ose_audio_events"]:
                    sound = sounds[cue["sound"]]
                    assert (ROOT / "assets/web" / sound["file"]).is_file(), cue
                    counts["cues"] += 1
        if part.get("light"):
            assert extras["ose_lights"], part["id"]
            assert extras["ose_light_emission"] > 0, part["id"]
            counts["lights"] += 1
    for key in ["light_on", "light_off"]:
        assert (ROOT / "assets/web" / sounds[key]["file"]).is_file()
    print(json.dumps(counts, indent=2))


if __name__ == "__main__":
    main()
