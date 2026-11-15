"""Check sources stayed fixed and add tool/binary hashes to an existing run record."""
import argparse
import hashlib
from html.parser import HTMLParser
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('run', type=Path)
args = p.parse_args()
run = args.run
record = json.loads((run / 'provenance.json').read_text())
digest = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
changed = [path for path, old in record['sources'].items() if digest(Path(path)) != old]
assert not changed, f'Sources changed during comparison: {changed}'
record['source_hashes_verified_after_run'] = True
record['ose_replay_executable_sha256'] = digest(Path('target/debug/ose-joint-replay.exe'))
record['validation_tools'] = {
    str(f): digest(f) for f in Path('tools/joint-parity').rglob('*')
    if f.is_file() and not any(x in f.parts for x in ['obj', 'bin', '__pycache__'])
}
(run / 'provenance.json').write_text(json.dumps(record, indent=2), encoding='utf-8')


class Links(HTMLParser):
    def handle_starttag(self, tag, attrs):
        for key, value in attrs:
            if key in ['href', 'src']:
                assert (run / value).is_file(), f'Missing artifact: {value}'


Links().feed((run / 'raw-data.html').read_text())
print('Source hashes unchanged; all raw-data index links exist.')
