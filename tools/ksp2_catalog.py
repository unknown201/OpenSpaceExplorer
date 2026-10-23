"""Parse a Unity Addressables catalog.json (KSP2 build) and resolve keys to bundle files.

Usage as a script: python tools/ksp2_catalog.py CATALOG [KEY ...]
With no keys it prints, per bundle, how many *.json keys it holds.
"""
from __future__ import annotations
import base64, collections, json, struct, sys

PREFIX = "{UnityEngine.AddressableAssets.Addressables.RuntimePath}" + "\\StandaloneWindows64\\"


class Catalog:
    def __init__(self, path: str):
        c = json.load(open(path, encoding="utf-8"))
        self.ids = c["m_InternalIds"]
        self.providers = c["m_ProviderIds"]
        self.keys = self._read_keys(base64.b64decode(c["m_KeyDataString"]))
        bd = base64.b64decode(c["m_BucketDataString"])
        nb = struct.unpack_from("<i", bd, 0)[0]; off = 4; self.buckets = []
        for _ in range(nb):
            off += 4
            ne = struct.unpack_from("<i", bd, off)[0]; off += 4
            self.buckets.append(list(struct.unpack_from(f"<{ne}i", bd, off))); off += 4 * ne
        ed = base64.b64decode(c["m_EntryDataString"])
        ne = struct.unpack_from("<i", ed, 0)[0]; off = 4; self.entries = []
        for _ in range(ne):
            iid, pidx, depkey, _dephash, dataidx, primkey, rtype = struct.unpack_from("<7i", ed, off); off += 28
            self.entries.append((iid, pidx, depkey, dataidx, primkey, rtype))
        self.key_index = {k: i for i, k in enumerate(self.keys) if isinstance(k, str)}

    @staticmethod
    def _read_keys(b: bytes) -> list:
        n = struct.unpack_from("<i", b, 0)[0]; off = 4; keys = []
        for _ in range(n):
            t = b[off]; off += 1
            if t == 0:
                l = struct.unpack_from("<i", b, off)[0]; off += 4; keys.append(b[off:off + l].decode("ascii", "replace")); off += l
            elif t == 1:
                l = struct.unpack_from("<i", b, off)[0]; off += 4; keys.append(b[off:off + l].decode("utf-16-le", "replace")); off += l
            elif t == 2: keys.append(("u16", struct.unpack_from("<H", b, off)[0])); off += 2
            elif t == 3: keys.append(("u32", struct.unpack_from("<I", b, off)[0])); off += 4
            elif t == 4: keys.append(("i32", struct.unpack_from("<i", b, off)[0])); off += 4
            elif t == 5: keys.append(("hash", b[off:off + 16])); off += 16
            elif t == 6:
                l = struct.unpack_from("<i", b, off)[0]; off += 4; keys.append(("type", None)); off += l
            elif t == 7:
                l = struct.unpack_from("<i", b, off)[0]; off += 4; keys.append(("json", None)); off += l
            else:
                raise ValueError(f"unknown key type {t} at {off}")
        return keys

    def string_keys(self) -> list[str]:
        return list(self.key_index)

    def resolve(self, key: str) -> list[tuple[str, str, list[str]]]:
        """-> [(internal_id, provider, [dependency bundle paths])]"""
        ki = self.key_index.get(key)
        if ki is None:
            return []
        out = []
        for ei in self.buckets[ki]:
            iid, pidx, depkey, *_ = self.entries[ei]
            deps = [self.ids[self.entries[d][0]] for d in self.buckets[depkey]] if depkey >= 0 else []
            out.append((self.ids[iid], self.providers[pidx], deps))
        return out

    def bundle_for(self, key: str) -> str | None:
        """Bundle file name (without directory) that holds a bundled asset key."""
        for _iid, _prov, deps in self.resolve(key):
            for d in deps:
                if d.endswith(".bundle"):
                    return d.replace(PREFIX, "")
        return None


def main() -> int:
    cat = Catalog(sys.argv[1])
    wanted = sys.argv[2:]
    if wanted:
        for w in wanted:
            print(w, "->", cat.bundle_for(w), json.dumps(cat.resolve(w), indent=1))
        return 0
    by = collections.Counter(cat.bundle_for(k) for k in cat.string_keys() if k.endswith(".json"))
    for b, n in by.most_common():
        print(n, b)
    return 0


if __name__ == "__main__":
    sys.exit(main())
