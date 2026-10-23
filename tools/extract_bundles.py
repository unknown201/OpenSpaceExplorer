"""Extract JSON TextAssets from the KSP2 Addressables bundles into assets/ksp2/data.

Catalog-driven: every Addressables key ending in .json is resolved to its bundle (part JSONs each live in a
hash-named bundle next to their prefab), then those bundles are opened with UnityPy and the TextAssets written out.

Usage: .venv/Scripts/python tools/extract_bundles.py [--install PATH] [--out assets/ksp2/data]
"""
from __future__ import annotations
import argparse, collections, json, sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from ksp2_catalog import Catalog  # noqa: E402

DEFAULT_INSTALL = r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2"


def group_for(key: str, bundle: str) -> str:
    if not bundle[:32].isalnum() or "_" in bundle:
        return bundle.split("_assets_all")[0]
    k = key.lower()
    if k.endswith("_kerman.json"):
        return "kerbals"
    return "parts"


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--install", default=DEFAULT_INSTALL)
    ap.add_argument("--out", default="assets/ksp2/data")
    a = ap.parse_args()
    import UnityPy
    aa = Path(a.install) / "KSP2_x64_Data/StreamingAssets/aa"
    cat = Catalog(str(aa / "catalog.json"))
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    wanted: dict[str, list[str]] = collections.defaultdict(list)
    for k in cat.string_keys():
        if k.endswith(".json"):
            b = cat.bundle_for(k)
            if b:
                wanted[b].append(k)
    n = 0
    index: dict[str, str] = {}
    for bundle, keys in sorted(wanted.items()):
        env = UnityPy.load(str(aa / "StandaloneWindows64" / bundle))
        keyset = {Path(k).name.lower() for k in keys}
        for obj in env.objects:
            if obj.type.name != "TextAsset":
                continue
            d = obj.read()
            name = d.m_Name
            fname = name if name.lower().endswith(".json") else name + ".json"
            if fname.lower() not in keyset:
                continue
            raw = d.m_Script
            text = raw if isinstance(raw, str) else bytes(raw).decode("utf-8", "replace")
            dest = out / group_for(fname, bundle) / fname
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(text, encoding="utf-8")
            index[fname] = f"{bundle}:{dest.relative_to(out).as_posix()}"
            n += 1
    (out / "_index.json").write_text(json.dumps(index, indent=1, sort_keys=True), encoding="utf-8")
    print(f"extracted {n} json files from {len(wanted)} bundles into {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
