"""Extract the science data the original loads by Addressables label into assets/ksp2/data/science.

Labels (from `ScienceExperimentsDataStore` and `ScienceRegionsDataProvider`): `scienceExperiment` (experiment
definitions), `science_region` (per-body region definitions), `science_region_discoverables` (discoverable
sites) as JSON TextAssets, and `science_region_map` (`CelestialBodyBakedScienceRegionMap`, the baked per-body
region index map) dumped as its type tree.

Usage: .venv/Scripts/python tools/extract_science.py [--install PATH] [--out assets/ksp2/data/science]
"""
from __future__ import annotations
import argparse, json, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from ksp2_catalog import Catalog  # noqa: E402

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"
LABELS = ["scienceExperiment", "science_region", "science_region_discoverables", "science_region_map"]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default="assets/ksp2/data/science")
    a = ap.parse_args()
    import UnityPy
    aa = Path(a.install) / "KSP2_x64_Data/StreamingAssets/aa"
    cat = Catalog(str(aa / "catalog.json"))
    out = Path(a.out)
    for label in LABELS:
        bundles = set()
        for _internal, _provider, deps in cat.resolve(label):
            for dep in deps:
                if dep.endswith(".bundle"):
                    bundles.add(dep.split("\\")[-1])
        print(label, "->", len(bundles), "bundles")
        d = out / label
        d.mkdir(parents=True, exist_ok=True)
        for b in sorted(bundles):
            path = aa / "StandaloneWindows64" / b
            if not path.exists():
                print("  missing", b)
                continue
            env = UnityPy.load(str(path))
            for obj in env.objects:
                t = obj.type.name
                if t == "TextAsset":
                    data = obj.read()
                    name = data.m_Name
                    text = data.m_Script
                    if isinstance(text, bytes):
                        text = text.decode("utf-8", "replace")
                    (d / f"{name}.json").write_text(text, encoding="utf-8")
                elif t == "MonoBehaviour" and label == "science_region_map":
                    try:
                        tree = obj.read_typetree()
                    except Exception as e:  # noqa: BLE001
                        print("  typetree failed", b, e)
                        continue
                    name = tree.get("m_Name") or f"map_{obj.path_id}"
                    (d / f"{name}.json").write_text(json.dumps(tree), encoding="utf-8")
        print("  wrote", len(list(d.iterdir())), "files to", d)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
