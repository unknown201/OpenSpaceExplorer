"""Export the original's sounds (Wwise soundbanks) as Ogg files for the web client.

The game ships Wwise banks under StreamingAssets/Audio/GeneratedSoundBanks/Windows: `*_events.bnk` hold the
hierarchy (HIRC: events, actions, sounds, containers, music), `*_media.bnk` hold small sounds in DIDX/DATA,
and long or streamed sounds are loose `<id>.wem` files. Event ids are the 32-bit FNV-1 hash of the lowercased
event name; names come from the Unity assets that post them (`.cache/audio_event_names.json`, a scan of every
Addressables bundle for `Play_...` strings).

For each event in EVENTS this resolves Event -> Play actions -> target node -> descendants (a node is a child
of the node its DirectParentID names, found as the first known object id in its body) -> wem sources, decodes
the chosen wem with vgmstream-cli and re-encodes it with ffmpeg (libvorbis) to assets/web/audio/<key>.ogg, and
writes assets/web/audio/sounds.json: {key: {file, event, loop, gain}}.

    python tools/export_audio.py [--scan-names] [--list PATTERN]

Needs vgmstream-cli (r2117, under $OSE_CACHE/tools) and ffmpeg on PATH.

`--list` prints every named event matching PATTERN with its resolved wems and durations, to choose from.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GAME = Path(r"C:\Program Files (x86)\Steam\steamapps\common\Kerbal Space Program 2\KSP2_x64_Data")
BANKS = GAME / "StreamingAssets" / "Audio" / "GeneratedSoundBanks" / "Windows"
AA = GAME / "StreamingAssets" / "aa" / "StandaloneWindows64"
CACHE = Path(os.environ.get("OSE_CACHE", ROOT / ".cache"))
VGMSTREAM = CACHE / "tools" / "vgmstream-r2117" / "vgmstream-cli.exe"
NAMES = CACHE / "audio_event_names.json"
OUT = ROOT / "assets" / "web" / "audio"

# key: (event, pick, loop, gain, states_from). pick is "first" (the first source found), "loop" (the longest
# looping stereo source: an engine's own loop rather than the shared rumble layers) or a wem id of the event.
# Music is the main menu's music switch played with the states that a scene's Set_State event sets.
MUSIC = "Play_amb_main_menu"
EVENTS: dict[str, tuple[str, object, bool, float, str | None]] = {
    "music_menu": (MUSIC, 774547221, True, 0.5, None),  # its playlist opens with a silent 12-minute track
    "music_ksc": (MUSIC, "first", True, 0.5, "Set_State_State_KerbalSpaceCenter"),
    "music_vab": (MUSIC, "first", True, 0.5, "Set_State_State_VehicleAssemblyBuilder"),
    "music_kerbin": (MUSIC, "first", True, 0.4, "Set_State_State_SOI_K_Kerbin"),
    "music_mun": (MUSIC, "first", True, 0.4, "Set_State_State_SOI_K_Kerbin_Moon_Mun"),
    # The facility screens play their own music switch from the state event itself.
    "music_tracking": ("Set_Music_State_Menu_TrackingCenter", 952431349, True, 0.5, None),
    "music_rnd": ("Set_Music_State_Menu_RnD", "first", True, 0.5, None),
    "music_mission": ("Set_Music_State_Menu_Mission_Control", 218510696, True, 0.5, None),
    "ui_select": ("Play_ui_extended_button_select", "first", False, 0.5, None),
    "ui_esc_open": ("Play_EM_Gameplay_EscapeMenu_opened", "first", False, 0.5, None),
    "rd_research": ("Play_ui_RD_button_select_BIG", "first", False, 0.6, None),
    "ksc_launchpad": ("Play_point_emitter_Buildings_launchpad", "loop", True, 0.5, None),
    "vab_ambience": ("Play_VAB_amb_numbers_01", "loop", True, 0.4, None),
    "launch_seq": ("Play_ksc_launch_seq_start", "first", False, 0.7, None),
    "stage": ("Play_EM_Staging_StageActivatedMessage", "first", False, 0.7, None),
    "decouple": ("Play_decouple", "first", False, 0.8, None),
    "engine_liquid": ("Play_u_ENGINE_methalox_1v_Terrier", "loop", True, 0.7, None),
    "engine_liquid_stop": ("Play_u_ENGINE_methalox_1v_Terrier_stop", "first", False, 0.6, None),
    "engine_srb": ("Play_u_ENGINE_boosters_1v", "loop", True, 0.8, None),
    "explosion": ("Play_A_explosion_v_00", "first", False, 0.9, None),
    "parachute": ("Play_parachutes_deployed", "first", False, 0.7, None),
    "chute_cut": ("Play_parachutes_cut", "first", False, 0.6, None),
    "reentry": ("Play_reentry_on", "first", True, 0.6, None),
    "sonic_boom": ("Play_sonic_boom", "first", False, 0.8, None),
    "overheat": ("Play_overheat_part_start", "first", False, 0.6, None),
    "leg_deploy": ("Play_An_Landing_Leg_Small_Extensible_LT_1_deploy_1", "first", False, 0.6, None),
    "leg_retract": ("Play_An_Landing_Leg_Small_Extensible_LT_1_retract_1", "first", False, 0.6, None),
    "panel_open": ("Play_An_SolarArray_1x6_Shielded_open_1", "first", False, 0.5, None),
    "panel_close": ("Play_An_SolarArray_1x6_Shielded_close_1", "first", False, 0.5, None),
    # The wind bank's events carry no names in the assets: its longest looping gust by id.
    "wind": ("wem:222060491", "first", True, 0.6, None),
}


# HIRC types that can be a node's DirectParentID: random/sequence, switch, actor-mixer, layer, music segment,
# track, switch and playlist. Buses, attenuations and effects also appear in a node's body and are skipped.
CONTAINERS = {5, 6, 7, 9, 10, 11, 12, 13}


def fnv1(name: str) -> int:
    h = 2166136261
    for b in name.lower().encode():
        h = (h * 16777619) & 0xFFFFFFFF
        h ^= b
    return h


def chunks(data: bytes):
    p = 0
    while p + 8 <= len(data):
        tag, n = data[p : p + 4], struct.unpack_from("<I", data, p + 4)[0]
        yield tag, data[p + 8 : p + 8 + n], p + 8
        p += 8 + n


class Banks:
    """Every HIRC object and every wem (embedded or loose) across the game's banks."""

    def __init__(self) -> None:
        self.objs: dict[int, tuple[int, bytes]] = {}  # id -> (type, body)
        self.embedded: dict[int, tuple[Path, int, int]] = {}  # wem id -> (bank, offset, size)
        self.loose: dict[int, Path] = {int(p.stem): p for p in BANKS.rglob("*.wem") if p.stem.isdigit()}
        for bank in sorted(BANKS.rglob("*.bnk")):
            data = bank.read_bytes()
            didx: list[tuple[int, int, int]] = []
            data_off = None
            for tag, body, off in chunks(data):
                if tag == b"HIRC":
                    (count,) = struct.unpack_from("<I", body, 0)
                    p = 4
                    for _ in range(count):
                        t, n = body[p], struct.unpack_from("<I", body, p + 1)[0]
                        b = body[p + 5 : p + 5 + n]
                        self.objs[struct.unpack_from("<I", b, 0)[0]] = (t, b)
                        p += 5 + n
                elif tag == b"DIDX":
                    didx = [struct.unpack_from("<III", body, i) for i in range(0, len(body), 12)]
                elif tag == b"DATA":
                    data_off = off
            if data_off is not None:
                for wid, o, n in didx:
                    self.embedded.setdefault(wid, (bank, data_off + o, n))
        self.wems = set(self.embedded) | set(self.loose)
        self._children: dict[int, list[int]] = {}
        for oid, (t, b) in self.objs.items():
            if t in (3, 4):  # actions and events are not in the node tree
                continue
            parent = self._first_ref(oid, b)
            if parent is not None:
                self._children.setdefault(parent, []).append(oid)

    def _refs(self, b: bytes, pool) -> list[int]:
        out = []
        for i in range(4, len(b) - 3):
            v = struct.unpack_from("<I", b, i)[0]
            if v in pool:
                out.append(v)
        return out

    def _first_ref(self, oid: int, b: bytes):
        for i in range(4, len(b) - 3):
            v = struct.unpack_from("<I", b, i)[0]
            if v != oid and v in self.objs and self.objs[v][0] in CONTAINERS:
                return v
        return None

    def _actions(self, eid: int, done: set[int]):
        """(kind, target, body) of every action of an event, following actions that post other events."""
        ev = self.objs.get(eid)
        if not ev or ev[0] != 4 or eid in done:
            return
        done.add(eid)
        for aid in self._refs(ev[1], self.objs):
            t, a = self.objs[aid]
            if t != 3 or len(a) < 10:
                continue
            kind = struct.unpack_from("<H", a, 4)[0] >> 8
            target = struct.unpack_from("<I", a, 6)[0]
            if self.objs.get(target, (None,))[0] == 4:
                yield from self._actions(target, done)
            else:
                yield kind, target, a

    def event_wems(self, name: str, states_from: str | None = None) -> list[int]:
        """Wems an event plays, in playlist order. Switch containers follow the switch the event sets (SetSwitch
        actions); music switches follow the states it sets (SetState), or those of `states_from`, falling back
        to each argument's default branch."""
        acts = list(self._actions(fnv1(name), set()))
        switches = {struct.unpack_from("<I", a, len(a) - 4)[0] for k, _, a in acts if k == 0x19}
        states: dict[int, int] = {}
        for k, _, a in acts + (list(self._actions(fnv1(states_from), set())) if states_from else []):
            if k == 0x12:
                g, st = struct.unpack_from("<II", a, len(a) - 8)
                states[g] = st
        out: list[int] = []
        seen: set[int] = set()

        def visit(n: int) -> None:
            if n in seen or n not in self.objs:
                return
            seen.add(n)
            t, b = self.objs[n]
            for w in self._refs(b, self.wems):
                if w not in out:
                    out.append(w)
            if t == 12:
                leaf = self._music_leaf(b, states)
                if leaf is not None:
                    visit(leaf)
                return
            kids = self._children.get(n, [])
            if t == 6 and switches:
                kids = self._switch_kids(b, switches, kids)
            for c in kids:
                visit(c)

        for k, target, _ in acts:
            if k == 0x04:  # Play
                visit(target)
        return out

    def _switch_kids(self, b: bytes, switches: set[int], kids: list[int]) -> list[int]:
        """A switch container's children for the chosen switch: its switch list is (state, count, ids...)."""
        kid_set = set(kids)
        for i in range(4, len(b) - 8):
            if struct.unpack_from("<I", b, i)[0] not in switches:
                continue
            (n,) = struct.unpack_from("<I", b, i + 4)
            if 0 < n <= 64 and i + 8 + 4 * n <= len(b):
                ids = list(struct.unpack_from(f"<{n}I", b, i + 8))
                if all(x in kid_set for x in ids):
                    return ids
        return kids

    def _music_leaf(self, b: bytes, states: dict[int, int]):
        """Walk a music switch's decision tree (depth, group ids, group types, size, mode, 12-byte nodes) with
        the given states; a missing state takes the default (key 0) branch."""
        for i in range(4, len(b) - 8):
            depth = struct.unpack_from("<I", b, i)[0]
            if not 1 <= depth <= 8 or i + 4 + 5 * depth + 5 > len(b):
                continue
            groups = struct.unpack_from(f"<{depth}I", b, i + 4)
            if not any(g in states for g in groups):
                continue
            p = i + 4 + 5 * depth
            size = struct.unpack_from("<I", b, p)[0]
            p += 5
            if size % 12 or p + size > len(b):
                continue
            nodes = [struct.unpack_from("<IIHH", b, p + 12 * j) for j in range(size // 12)]
            idx = 0
            for level in range(depth):
                val = nodes[idx][1]
                first, count = val & 0xFFFF, val >> 16
                kids = range(first, min(first + count, len(nodes)))
                want = states.get(groups[level], 0)
                pick = [j for j in kids if nodes[j][0] == want] or [j for j in kids if nodes[j][0] == 0]
                if not pick:
                    return None
                idx = pick[0]
            return nodes[idx][1]
        return None

    def wem_bytes(self, wid: int) -> bytes:
        if wid in self.loose:
            return self.loose[wid].read_bytes()
        bank, off, n = self.embedded[wid]
        with open(bank, "rb") as f:
            f.seek(off)
            return f.read(n)


def scan_names() -> None:
    """Collect event names from every Addressables bundle (serialized `Play_...` strings) into NAMES."""
    import UnityPy

    pat = re.compile(rb"(?:Play|Stop|Set|Pause|Resume|Mute|Unmute)_[A-Za-z0-9_]{3,90}")
    found: dict[str, str] = {}
    for f in sorted(AA.glob("*.bundle")):
        try:
            env = UnityPy.load(str(f))
            for bf in env.files.values():
                for k, v in (getattr(bf, "files", None) or {}).items():
                    r = getattr(v, "reader", None)
                    if r is None or k.endswith((".resS", ".resource")):
                        continue
                    for m in pat.findall(bytes(r.bytes)):
                        found.setdefault(m.decode(), f.name)
        except Exception as e:  # noqa: BLE001 - a broken bundle only loses its names
            print("skip", f.name, e, file=sys.stderr)
    NAMES.parent.mkdir(parents=True, exist_ok=True)
    NAMES.write_text(json.dumps(found, indent=0, sort_keys=True))
    print(f"{len(found)} event names -> {NAMES}")


def resolve_names() -> list[str]:
    raw = json.loads(NAMES.read_text()) if NAMES.exists() else {}
    names = set()
    for s in raw:
        # Strings were scanned out of serialized data and can carry a trailing byte; keep every prefix, and
        # only the ones that hash to an event are used.
        for k in range(len(s), 4, -1):
            names.add(s[:k])
    return sorted(names)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", metavar="PATTERN")
    ap.add_argument("--scan-names", action="store_true", help="rescan the bundles for event names first")
    args = ap.parse_args()
    if args.scan_names or not NAMES.exists():
        scan_names()
    banks = Banks()
    tmpdir = Path(tempfile.mkdtemp(prefix="ose-audio-"))
    cache: dict[int, tuple[float, bool, int]] = {}

    def wem_file(wid: int) -> Path:
        p = tmpdir / f"{wid}.wem"
        if not p.exists():
            p.write_bytes(banks.wem_bytes(wid))
        return p

    def info(wid: int) -> tuple[float, bool, int]:
        """(seconds, loops, channels) from vgmstream's metadata."""
        if wid not in cache:
            o = subprocess.run([str(VGMSTREAM), "-m", str(wem_file(wid))], capture_output=True, text=True).stdout
            rate = re.search(r"sample rate: (\d+)", o)
            total = re.search(r"stream total samples: (\d+)", o)
            ch = re.search(r"channels: (\d+)", o)
            secs = int(total.group(1)) / int(rate.group(1)) if rate and total else 0.0
            cache[wid] = (secs, "loop start" in o, int(ch.group(1)) if ch else 0)
        return cache[wid]

    if args.list:
        pat = re.compile(args.list, re.I)
        for name in resolve_names():
            if not pat.search(name) or fnv1(name) not in banks.objs:
                continue
            wems = banks.event_wems(name)
            print(name, [(w, *info(w)) for w in wems[:12]], "..." if len(wems) > 12 else "")
        return

    OUT.mkdir(parents=True, exist_ok=True)
    manifest = {}
    for key, (event, pick, loop, gain, states_from) in EVENTS.items():
        wems = [int(event[4:])] if event.startswith("wem:") else banks.event_wems(event, states_from)
        if not wems:
            print(f"{key}: {event} resolved to no sounds, skipped", file=sys.stderr)
            continue
        if pick == "loop":
            looped = [w for w in wems if info(w)[1] and info(w)[2] == 2] or wems
            wid = max(looped, key=lambda w: info(w)[0])
        elif isinstance(pick, int):
            if pick not in wems:
                print(f"{key}: {pick} is not a sound of {event}, skipped", file=sys.stderr)
                continue
            wid = pick
        else:
            wid = wems[0]
        wav = tmpdir / f"{wid}.wav"
        subprocess.run([str(VGMSTREAM), "-o", str(wav), str(wem_file(wid))], check=True, capture_output=True)
        out = OUT / f"{key}.ogg"
        quality = "2" if key.startswith("music") else "4"
        subprocess.run(
            ["ffmpeg", "-y", "-loglevel", "error", "-i", str(wav), "-c:a", "libvorbis", "-q:a", quality, str(out)],
            check=True,
        )
        manifest[key] = {"file": f"audio/{key}.ogg", "event": event, "wem": wid, "loop": loop, "gain": gain}
        print(f"{key}: {event} -> {wid} ({info(wid)[0]:.2f}s, {out.stat().st_size // 1024} KB)")
    (OUT / "sounds.json").write_text(json.dumps(manifest, indent=1))
    print(f"wrote {len(manifest)} sounds to {OUT}")


if __name__ == "__main__":
    main()
