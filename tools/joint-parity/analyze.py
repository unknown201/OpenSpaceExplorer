"""Summarize matched native/OSE joint tests; never turn a mismatch into a pass."""
import argparse
from collections import defaultdict
import csv
import hashlib
import html
import json
import math
from pathlib import Path


def rows(path):
    with path.open(newline='', encoding='utf-8') as f:
        return list(csv.DictReader(f))


def grouped(data, key='case'):
    out = defaultdict(list)
    for row in data:
        out[row[key]].append(row)
    return out


def main():
    p = argparse.ArgumentParser()
    p.add_argument('run', type=Path)
    args = p.parse_args()
    run = args.run
    source = json.loads((run / 'ose/summary.json').read_text())
    results, designs, integrity = [], [], []
    for specimen in sorted((run / 'native').glob('*-specimen.json')):
        name = specimen.name.removesuffix('-specimen.json')
        s = json.loads(specimen.read_text())
        native = grouped(rows(run / 'native' / f'{name}-native.csv'))
        inputs = grouped(rows(run / 'native' / f'{name}-inputs.csv'))
        assert len(native) == 16, f'Incomplete native matrix: {name}'
        for case, data in native.items():
            assert all(r['alive'] == '1' for r in data if int(r['tick']) < 120), f'Pre-ramp break: {name} {case}'
            assert all(math.isfinite(float(r['angle_rad'])) for r in data), f'Nonfinite angle: {name} {case}'
            assert all(math.isfinite(float(r[k])) for r in data if r['alive'] == '1'
                       for k in ['force_kn', 'torque_knm', 'anchor_error_m']), f'Nonfinite native reaction: {name} {case}'
            repeat = case.replace('r0-', 'r1-')
            if case.startswith('r0-'):
                a = [(r['tick'], r['joint'], r['alive']) for r in data]
                b = [(r['tick'], r['joint'], r['alive']) for r in native[repeat]]
                assert a == b, f'Native repetition differs: {name} {case}'
                delta = max(abs(float(x['angle_rad']) - float(y['angle_rad'])) for x, y in zip(data, native[repeat]))
                integrity.append({'craft': name, 'case': case, 'repeat_angle_max_abs_rad': delta})
        pairs = {tuple(sorted((j['host'], j['connected']))) for j in s['joints']}
        reinforced = sum(float(j['properties']['breakForce']) > 1e30 for j in s['joints'])
        craft_results = []
        for record in (x for x in source if x['craft'] == name):
            case, mode = record['case'], record['mode']
            data = native[case]
            native_break = next((int(r['tick']) for r in data if r['alive'] == '0'), None)
            ose_break = record['first_break']['tick'] if record['first_break'] else None
            assert ose_break is None or ose_break >= 120, f'OSE pre-ramp break: {name} {case}'
            load_by_tick = {int(r['tick']): float(r['load_kn']) for r in data}
            native_load = load_by_tick[native_break] if native_break is not None else None
            ose_load = load_by_tick[ose_break] if ose_break is not None else None
            if ose_break is None and native_break is None:
                outcome = 'both survived tested range'
            elif ose_break is None:
                outcome = 'OSE survived native first break'
            elif native_break is None:
                outcome = 'OSE broke; native survived tested range'
            elif abs(ose_break - native_break) <= 1:
                outcome = 'break within one tick'
            elif ose_break < native_break:
                outcome = 'OSE broke earlier'
            else:
                outcome = 'OSE broke later'
            ose_rows = [r for r in rows_cache(run, name, mode) if r['case'] == case]
            assert all(math.isfinite(float(r['force_kn'])) and math.isfinite(float(r['bend_rad'])) for r in ose_rows), f'Nonfinite OSE: {name} {case}'
            pulse_native = max(float(r['angle_rad']) for r in data if 30 <= int(r['tick']) < 60)
            pulse_ose = max(float(r['bend_rad']) for r in ose_rows if 30 <= int(r['tick']) < 60)
            # Exact same per-part force and offset sequence; hashes allow later independent checks.
            result = dict(craft=name, mode=mode, case=case, outcome=outcome,
                          native_break_tick=native_break, ose_break_tick=ose_break,
                          native_break_load_kn=native_load, ose_break_load_kn=ose_load,
                          native_over_ose_break_load=(native_load / ose_load if native_load and ose_load else None),
                          native_max_tested_load_kn=max(load_by_tick.values()),
                          pulse_native_peak_joint_angle_rad=pulse_native,
                          pulse_ose_peak_joint_angle_rad=pulse_ose,
                          native_mass_t=record['native_mass_t'], ose_original_mass_t=record['ose_initial_mass_t'])
            results.append(result)
            if mode == 'runtime-inertia':
                craft_results.append(result)
        designs.append(dict(craft=name, native_bodies=len(s['bodies']), native_joints=len(s['joints']),
                            native_connections=len(pairs), unbreakable_native_joints=reinforced,
                            native_parts_without_rigidbodies=s.get('partsWithoutRigidbodies', []),
                            runtime_inertia_outcomes={outcome: sum(x['outcome'] == outcome for x in craft_results) for outcome in sorted({x['outcome'] for x in craft_results})}))
    expected = [x.strip() for x in (run / 'inputs/run.txt').read_text().splitlines() if x.strip()]
    assert sorted(expected) == sorted(x['craft'] for x in designs), 'Missing craft captures'
    report = dict(designs=designs, cases=results, integrity=integrity,
                  native_cases=sum(16 for _ in designs), ose_cases=len(results),
                  hashes={str(p.relative_to(run)): hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(run.glob('native/*')) if p.is_file()})
    (run / 'comparison.json').write_text(json.dumps(report, indent=2), encoding='utf-8')
    with (run / 'comparison.csv').open('w', newline='', encoding='utf-8') as f:
        w = csv.DictWriter(f, fieldnames=list(results[0])); w.writeheader(); w.writerows(results)
    lines = ['| Design | Bodies / native joints | OSE earlier or native survives | Within 1 tick | OSE survives native break |',
             '|---|---:|---:|---:|---:|']
    for d in designs:
        group = [x for x in results if x['craft'] == d['craft'] and x['mode'] == 'runtime-inertia']
        earlier = sum(x['outcome'] in ['OSE broke earlier', 'OSE broke; native survived tested range'] for x in group)
        same = sum(x['outcome'] == 'break within one tick' for x in group)
        later = sum(x['outcome'] == 'OSE survived native first break' for x in group)
        lines.append(f"| {d['craft']} | {d['native_bodies']} / {d['native_joints']} | {earlier}/16 | {same}/16 | {later}/16 |")
    (run / 'comparison-table.md').write_text('\n'.join(lines) + '\n', encoding='utf-8')
    cells = []
    for d in designs:
        name = d['craft']
        links = [('Specimen', f'native/{name}-specimen.json'),
                 ('Applied inputs', f'native/{name}-inputs.csv'),
                 ('Native samples', f'native/{name}-native.csv'),
                 ('OSE original', f'ose/{name}-original.csv'),
                 ('OSE native masses', f'ose/{name}-mass-matched.csv'),
                 ('OSE native masses/body inertia', f'ose/{name}-runtime-inertia.csv'),
                 ('Craft screenshot', f'native/{name}.png')]
        cells.append('<tr><th>' + html.escape(name) + '</th><td>' + ' · '.join(
            f'<a href="{html.escape(href)}">{html.escape(label)}</a>' for label, href in links) + '</td></tr>')
    (run / 'raw-data.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8">
<title>KSP2 / OSE joint measurements</title><style>
body{font:16px/1.6 system-ui;max-width:1100px;margin:40px auto;padding:0 24px;color:#172b37}
table{border-collapse:collapse;width:100%}td,th{padding:14px;text-align:left;border-bottom:1px solid #ccd6dc}
a{color:#176987}img{max-width:100%}small{color:#576570}</style>
<h1>KSP2 / OSE joint measurements</h1>
<p>128 native cases; 384 OSE replays across eight designs. Controlled structural benchmark at 50 Hz,
with exact per-part applied force histories. These are not matched-flight or launch failure thresholds.</p>
<p><a href="comparison.csv">All comparison results (CSV)</a> · <a href="comparison.json">Results, integrity checks and hashes (JSON)</a>
 · <a href="native/settings.json">Runtime settings</a> · <a href="native/steps.log">Completion log</a>
 · <a href="provenance.json">Source and executable provenance</a></p>
<img src="tall-stack-comparison.png" alt="Native and OSE first-break loads for three stacks">
<table><tr><th>Design</th><th>Raw measurements and inputs</th></tr>''' + ''.join(cells) + '''</table>
<p><small>Unity native force/torque fields exclude drive actuation; they cannot be directly equated to OSE subtree force or spring torque.
Native survivors are censored at the test ceiling, and OSE survivors at the native first break are censored at that time.
The pulse-angle fields include initial constraint settling. See the repository note for scope and limitations.</small></p></html>''', encoding='utf-8')
    print('\n'.join(lines))
    print(f"Integrity checked: {len(integrity)} paired native repetitions; {len(results)} OSE replays")


_cache = {}
def rows_cache(run, name, mode):
    key = (str(run), name, mode)
    if key not in _cache:
        _cache[key] = rows(run / 'ose' / f'{name}-{mode}.csv')
    return _cache[key]


if __name__ == '__main__':
    main()
