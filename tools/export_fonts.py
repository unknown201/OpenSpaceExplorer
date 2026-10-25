"""Copy KSP2's UI font (Realtime Rounded, regular and bold) out of the game install into assets/web/ui/fonts.

The files stay out of git (assets/web is ignored) and reach the server with `tools/push_assets.py ui`, as the
other assets taken from the install do. The dot-matrix titles (SB Liquid) ship only as TextMeshPro atlases, so
the client draws those in Realtime Rounded too.

    python tools/export_fonts.py [install dir]
"""

import glob
import os
import sys
from pathlib import Path

import UnityPy

sys.path.insert(0, str(Path(__file__).parent))
from export_web_assets import DEFAULT_INSTALL, ROOT  # noqa: E402

WANT = ("realtimerounded-regular", "realtimerounded-bold")


def main() -> None:
    install = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_INSTALL
    out = ROOT / "assets" / "web" / "ui" / "fonts"
    out.mkdir(parents=True, exist_ok=True)
    found = set()
    for bundle in glob.glob(os.path.join(install, "**", "*.bundle"), recursive=True):
        if os.path.getsize(bundle) > 300_000_000:
            continue
        try:
            env = UnityPy.load(bundle)
        except Exception:
            continue
        for obj in env.objects:
            if obj.type.name != "Font":
                continue
            font = obj.read()
            if font.m_Name in WANT and font.m_Name not in found:
                data = bytes(font.m_FontData)
                ext = ".otf" if data[:4] == b"OTTO" else ".ttf"
                (out / (font.m_Name + ext)).write_bytes(data)
                found.add(font.m_Name)
                print(f"{font.m_Name}{ext}: {len(data)} bytes")
    missing = set(WANT) - found
    if missing:
        sys.exit(f"not found in {install}: {sorted(missing)}")


if __name__ == "__main__":
    main()
