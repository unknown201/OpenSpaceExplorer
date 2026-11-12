"""Copy an empty KSC sandbox into a dedicated validation campaign."""
import argparse
import json
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument('source', type=Path)
p.add_argument('destination', type=Path)
args = p.parse_args()
d = json.loads(args.source.read_text(encoding='utf-8-sig'))
assert not d['Vessels'], 'Use an empty sandbox at KSC'
assert d['Metadata']['GameMode'] == 'SandboxMode'
for player in d['CampaignPlayers']:
    assert player['StartingGameState'] == 'KerbalSpaceCenter'
d['Metadata']['Name'] = 'Joint validation seed'
d['Metadata']['Filepath'] = str(args.destination.resolve())
d['Metadata']['ThumbnailImagePath'] = ''
args.destination.parent.mkdir(parents=True, exist_ok=True)
assert not args.destination.exists(), 'Preserve an existing seed'
args.destination.write_text(json.dumps(d), encoding='utf-8')
