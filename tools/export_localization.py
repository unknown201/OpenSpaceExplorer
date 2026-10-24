"""Export the original's English text and the tech-node icons for the web client, read-only on the install.

    assets/web/loc/en.json        every I2 localization term (I2Languages_*.asset across the bundles) to
                                  its English string, e.g. "Science/TechNodes/Names/tNode_0v_aero" ->
                                  "Aerial Drones"
    assets/web/ui/tech/<icon>.png the tech tree's node icons (technodeicons_assets_all.bundle)

Gitignored like the other exports; tools/push_assets.py copies loc/ and ui/ to the server.

    .venv/Scripts/python tools/export_localization.py
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT, Exporter  # noqa: E402

LOC_BUNDLES = ["defaultlocalgroup_assets_all.bundle", "research_assets_all.bundle"]
ICON_BUNDLE = "technodeicons_assets_all.bundle"


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default=str(ROOT / "assets" / "web"))
    a = ap.parse_args()
    out = Path(a.out)
    (out / "loc").mkdir(parents=True, exist_ok=True)
    (out / "ui" / "tech").mkdir(parents=True, exist_ok=True)
    t0 = time.time()
    ex = Exporter(Path(a.install), out, 1024, verbose=False)
    # Every bundle whose catalog keys name an I2Languages asset, plus the two known ones.
    bundles = set(LOC_BUNDLES)
    for k in ex.cat.string_keys():
        if "I2Languages" in k:
            b = ex.cat.bundle_for(k)
            if b:
                bundles.add(b)
    terms: dict[str, str] = {}
    sources = 0
    for b in sorted(bundles):
        env = ex.env(b)
        for o in env.objects:
            if o.type.name != "MonoBehaviour":
                continue
            try:
                r = o.read()
                if not str(getattr(r, "m_Name", "")).startswith("I2Languages"):
                    continue
                t = o.read_typetree()
            except Exception:  # noqa: BLE001
                continue
            src = t.get("mSource", {})
            langs = [lang.get("Name", "") for lang in src.get("mLanguages", [])]
            en = langs.index("English") if "English" in langs else 0
            for term in src.get("mTerms", []):
                vals = term.get("Languages", [])
                if term.get("Term") and en < len(vals) and vals[en]:
                    terms[term["Term"]] = vals[en]
            sources += 1
    (out / "loc" / "en.json").write_text(json.dumps(terms, ensure_ascii=False, sort_keys=True), encoding="utf-8")
    print(f"loc/en.json: {len(terms)} terms from {sources} sources in {len(bundles)} bundles")

    env = ex.env(ICON_BUNDLE)
    n = 0
    for o in env.objects:
        if o.type.name not in ("Texture2D", "Sprite"):
            continue
        try:
            r = o.read()
            name = r.m_Name
            if not name.startswith("ICO-") or (out / "ui" / "tech" / f"{name}.png").exists():
                continue
            r.image.save(out / "ui" / "tech" / f"{name}.png")
            n += 1
        except Exception:  # noqa: BLE001
            continue
    print(f"ui/tech: {n} icons; {time.time() - t0:.0f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
