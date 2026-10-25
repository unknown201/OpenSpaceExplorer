#!/usr/bin/env python
"""Export the original's Kerbal Space Academy lessons (data/tutorials/KSP2Tutorial_*.json, grouped by
KSP2MainTutorial_*.json) to assets/web/ui/tutorials.json for the Training Center's Lessons tab: each group's
title and description, and each lesson's title, description and PAIGE's messages stage by stage (loc keys
and emotes). The lessons' conditions and video clips are left out. Push with `push_assets.py ui`.

    .venv/Scripts/python tools/export_tutorials.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def messages(node, out):
    """Every MessageBoxAction under `node`, in file order: its loc keys and PAIGE's emote."""
    if isinstance(node, dict):
        if "MessageBoxAction" in str(node.get("$type", "")) and node.get("message"):
            for key in re.findall(r"\[([^\]]+)\]", node["message"]):
                image = node.get("imageKey") or ""
                # One message names a loose picture of PAIGE's shocked face; use the emote it matches.
                if image and not image.startswith("PAIGE_emote_"):
                    image = "PAIGE_emote_03_shocked_0001"
                out.append({"key": key.strip(), "image": image})
        for v in node.values():
            messages(v, out)
    elif isinstance(node, list):
        for v in node:
            messages(v, out)


def main() -> int:
    src = ROOT / "assets" / "ksp2" / "data" / "tutorials"
    if not src.is_dir():
        sys.exit(f"export_tutorials: {src} is missing")
    groups = []
    for f in sorted(src.glob("KSP2MainTutorial_*.json")):
        g = json.loads(f.read_text(encoding="utf-8-sig"))
        groups.append({"id": g["ID"], "title": g.get("Title", g["ID"]), "order": g.get("Order", 0)})
    groups.sort(key=lambda g: g["order"])
    lessons = []
    for f in sorted(src.glob("KSP2Tutorial_*.json")):
        t = json.loads(f.read_text(encoding="utf-8-sig"))
        stages = []
        for st in t.get("missionStages", []):
            out: list = []
            messages(st.get("actions", []), out)
            if out:
                stages.append({"messages": out})
        if not stages or t.get("hidden"):
            continue
        lessons.append({
            "id": t["ID"],
            "group": t.get("MissionGroup", ""),
            "name": t.get("name", t["ID"]),
            "description": (t.get("description") or "").strip(),
            "stages": stages,
        })
    lessons.sort(key=lambda l: (l["group"], "Animation" in l["id"], l["id"]))
    dest = ROOT / "assets" / "web" / "ui" / "tutorials.json"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps({"groups": groups, "lessons": lessons}, separators=(",", ":")), encoding="utf-8")
    n = sum(len(s["messages"]) for l in lessons for s in l["stages"])
    print(f"tutorials: {len(groups)} groups, {len(lessons)} lessons, {n} messages -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
