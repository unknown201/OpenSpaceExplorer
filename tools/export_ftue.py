#!/usr/bin/env python
"""Export the original's first-time hints (data/firsttimeuserexperience/FTUE_*.json: PAIGE's message boxes)
to assets/web/ui/ftue.json for the client: per script id, the message loc keys and PAIGE's emote, in order.
The client shows them on the matching first visit (docs/ui-parity-ksp2.md). Push with `push_assets.py ui`.

    .venv/Scripts/python tools/export_ftue.py
"""
from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def messages(node, out):
    """Every MessageBoxAction in the script, in file order."""
    if isinstance(node, dict):
        if "MessageBoxAction" in str(node.get("$type", "")) and node.get("message"):
            keys = re.findall(r"\[([^\]]+)\]", node["message"])
            for k in keys:
                out.append({"key": k, "image": node.get("imageKey") or ""})
        for v in node.values():
            messages(v, out)
    elif isinstance(node, list):
        for v in node:
            messages(v, out)


def main() -> int:
    src = ROOT / "assets" / "ksp2" / "data" / "firsttimeuserexperience"
    if not src.is_dir():
        sys.exit(f"export_ftue: {src} is missing")
    scripts = {}
    for f in sorted(src.glob("FTUE_*.json")):
        data = json.loads(f.read_text(encoding="utf-8-sig"))
        out: list = []
        messages(data, out)
        if out:
            scripts[data.get("ID", f.stem)] = {"name": data.get("name", ""), "messages": out}
    dest = ROOT / "assets" / "web" / "ui" / "ftue.json"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_text(json.dumps(scripts, separators=(",", ":")), encoding="utf-8")
    print(f"ftue: {len(scripts)} scripts, {sum(len(s['messages']) for s in scripts.values())} messages -> {dest}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
