"""Hash accepted run inputs and optional pinned baseline; never commit game-derived data."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def command(*args):
    return subprocess.check_output(args, text=True).strip()


def hashes(paths):
    return {str(f): hashlib.sha256(f.read_bytes()).hexdigest()
            for f in sorted(set(paths)) if f.is_file()}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('root', type=Path)
    parser.add_argument('--game', type=Path, required=True)
    parser.add_argument('--baseline-worktree', type=Path)
    parser.add_argument('--baseline-run', type=Path)
    args = parser.parse_args()
    if bool(args.baseline_worktree) != bool(args.baseline_run):
        parser.error('baseline worktree and run must be supplied together')
    inputs = [Path('assets/own') / name for name in (
        'parts-catalog.json', 'kerbol-system.json', 'resources.json',
        'water-settings.json', 'aero-settings.json', 'inertia-settings.json')]
    inputs += list(Path('crates/ose-sim/src').glob('*.rs'))
    inputs += [Path('crates/ose-sim/examples/water_probe.rs')]
    inputs += list(Path('tools/water-parity').glob('*.py'))
    inputs += [Path('tools/water-parity/WaterParity.cs'),
               Path('tools/water-parity/WaterParity.csproj'),
               Path('tools/water-parity/run.ps1')]
    inputs += [args.root / name for name in (
        'craft.json', 'build/OSE.WaterParity.dll', 'ose-samples.jsonl',
        'ose-events.json', 'comparison.json', 'comparison.html')]
    inputs += [f for folder in ('native', 'native-cases')
               for f in (args.root / folder).rglob('*')]
    inputs += [args.game / name for name in (
        'KSP2_x64.exe', 'UnityPlayer.dll', 'KSP2_x64_Data/Managed/Assembly-CSharp.dll')]
    record = {
        'revision': command('git', 'rev-parse', 'HEAD'),
        'dirty_tree': command('git', 'status', '--porcelain').splitlines(),
        'rustc': command('rustc', '--version'),
        'dotnet': command('dotnet', '--version'),
        'sha256': hashes(inputs),
    }
    if args.baseline_worktree:
        baseline = args.baseline_worktree
        record['baseline'] = {
            'revision': command('git', '-C', str(baseline), 'rev-parse', 'HEAD'),
            'dirty_tree': command('git', '-C', str(baseline), 'status', '--porcelain').splitlines(),
            'sha256': hashes([baseline / 'crates/ose-sim/examples/water_probe.rs']
                             + list(args.baseline_run.glob('*'))),
        }
    destination = args.root / 'provenance.json'
    destination.write_text(json.dumps(record, indent=2), encoding='utf-8')
    print(destination)


if __name__ == '__main__':
    main()