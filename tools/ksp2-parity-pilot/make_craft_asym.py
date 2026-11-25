"""Build craft-asym.json for the KSP2 parity pilot's "reentryorbit asym" scenario: craft-asparagus.json with
its -X side taken off (the radial decoupler, booster tank, Swivel and both fuel lines there), so one booster
hangs off one side of the core. An asymmetric craft for the per-part reentry comparison (T11).

    python -I tools/ksp2-parity-pilot/make_craft_asym.py
"""

import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
GONE = {f"05e10000-0000-0000-0000-0000000000{k}" for k in ("05", "07", "09", "0c", "0d")}


def main() -> None:
    craft = json.loads((HERE / "craft-asparagus.json").read_text(encoding="utf-8-sig"))
    asm = craft["Assemblies"][0]["Assembly"]
    asm["parts"] = [p for p in asm["parts"] if p["PartGuid"]["Guid"] not in GONE]
    for staging in (asm["AssemblyOABConfig"]["StagingState"], asm["stagingState"]):
        for stage in staging["availableStages"]:
            stage["PartIds"] = [p for p in stage["PartIds"] if p["Guid"] not in GONE]
    asm["AssemblyDefinition"]["assemblyName"] = "Asym"
    text = json.dumps(craft, indent=1)
    assert not any(g in text for g in GONE)
    (HERE / "craft-asym.json").write_text(text + "\n", encoding="utf-8")
    print(f"craft-asym.json: {len(asm['parts'])} parts")


if __name__ == "__main__":
    main()
