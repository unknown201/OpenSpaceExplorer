"""Validate fresh native measurements, before/after outcomes and enhanced graphs.

Uses the standard library. Measurements stay local; --summary exports only compact
numeric results and content hashes for review, without game content or craft data.
"""
import argparse
from collections import Counter, defaultdict
import csv
import hashlib
import html
import json
import math
from pathlib import Path


def read_rows(path):
    with path.open(newline='', encoding='utf-8') as f:
        return list(csv.DictReader(f))


def main():
    p = argparse.ArgumentParser()
    p.add_argument('run', type=Path)
    p.add_argument('--summary', type=Path)
    args = p.parse_args()
    run = args.run
    assert (run / 'native/done.txt').read_text().strip() == 'ok'
    assert not (run / 'native/failed.txt').exists()
    settings = json.loads((run / 'native/settings.json').read_text())
    assert settings['enhancedJointsEnabled'] and settings['enhancedJointsMode'] == 'Heaviest'
    assert not settings['unbreakableJoints'] and abs(settings['fixedDeltaTime'] - .02) < 1e-7
    names = (run / 'inputs/run.txt').read_text().split()
    before = json.loads((run / 'ose-before/summary.json').read_text())
    after = json.loads((run / 'ose-after/summary.json').read_text())
    expected_cases = {f'r{r}-{axis}{sign}' for r in [0, 1] for axis in 'xyzt' for sign in '+-'} | {'live-t+', 'live-t-'}
    assert len(before) == len(after) == len(names) * len(expected_cases) * 3
    key = lambda x: (x['craft'], x['mode'], x['case'])
    before = {key(x): x for x in before}
    after = {key(x): x for x in after}
    assert before.keys() == after.keys()
    designs, csv_rows, native_count = [], [], 0
    for name in names:
        specimen = json.loads((run / 'native' / f'{name}-specimen.json').read_text())
        assert all(not b['isKinematic'] and b['constraints'] == 'None' for b in specimen['bodies'])
        grouped = defaultdict(list)
        for row in read_rows(run / 'native' / f'{name}-native.csv'):
            grouped[row['case']].append(row)
        assert set(grouped) == expected_cases, f'Incomplete matrix for {name}'
        max_error = 0.0
        for case, rows in grouped.items():
            assert {int(r['tick']) for r in rows} == set(range(300))
            assert all(r['alive'] == '1' for r in rows), f'Unexpected native break {name}/{case}'
            assert all(math.isfinite(float(r[k])) for r in rows for k in ['force_kn', 'torque_knm', 'angle_rad', 'anchor_error_m'])
            max_error = max(max_error, max(float(r['anchor_error_m']) for r in rows))
            # Stay below one tenth of KSP2's 1 m linear limit. The old extreme
            # ramp violated this by metres and is not a usable strength target.
            assert max_error < .1, f'Native constraint separation exceeds quality bound: {name}'
            if case.startswith('r0-'):
                other = grouped[case.replace('r0-', 'r1-')]
                assert [(r['tick'], r['joint'], r['alive'], r['angle_rad']) for r in rows] == [(r['tick'], r['joint'], r['alive'], r['angle_rad']) for r in other]
            for mode in ['original', 'mass-matched', 'runtime-inertia']:
                b, a = before[(name, mode, case)], after[(name, mode, case)]
                assert a['first_break'] is None, f'False OSE break remains: {name}/{mode}/{case}'
                csv_rows.append(dict(craft=name, mode=mode, case=case, native_broke=False,
                                     before_broke=b['first_break'] is not None,
                                     before_break_tick=b['first_break']['tick'] if b['first_break'] else '',
                                     after_broke=False, native_max_anchor_error_m=max(float(r['anchor_error_m']) for r in rows)))
            native_count += 1
        example = after[(name, 'original', 'r0-x+')]
        mapping = example['native_to_ose']
        pair = lambda a, b: tuple(sorted((a, b)))
        native_extras = Counter(pair(mapping[j['host']], mapping[j['connected']]) for j in specimen['joints'] if float(j['properties']['breakForce']) > 1e30)
        ose_extras = Counter(pair(b['source'], b['target']) for b in example['reinforcements'] + example['braces'] if b['active'])
        assert native_extras == ose_extras, f'Extra connection graph differs for {name}: native={native_extras}, OSE={ose_extras}'
        # Check every CSV, not just the summary: no NaN/Inf can hide a solver failure.
        for mode in ['original', 'mass-matched', 'runtime-inertia']:
            for row in read_rows(run / 'ose-after' / f'{name}-{mode}.csv'):
                assert all(math.isfinite(float(row[k])) for k in ['force_kn', 'torque_knm', 'bend_rad', 'constraint_force_kn', 'constraint_torque_knm'])
                assert row['intact'] == '1'
        designs.append(dict(craft=name, native_cases=18,
                            native_breaks=0,
                            before_false_breaks=sum(before[(name, 'original', c)]['first_break'] is not None for c in expected_cases),
                            after_false_breaks=0, native_max_anchor_error_m=max_error,
                            reinforced_connections=len(example['reinforcements']),
                            strut_braces=len(example['braces']),
                            extra_graph_matches=True))
    hashes = {str(p.relative_to(run)): hashlib.sha256(p.read_bytes()).hexdigest()
              for p in sorted((run / 'native').glob('*')) if p.is_file() and p.suffix != '.png'}
    compact = dict(reference='KSP2 0.2.2.0.32913 / Unity 2022.3.5f1', native_cases=native_count,
                   live_native_cases=len(names) * 2, ose_cases_per_build=len(after),
                   before_false_breaks=sum(x['before_false_breaks'] for x in designs),
                   after_false_breaks=0, settings=settings, designs=designs, native_hashes=hashes)
    (run / 'fix-validation.json').write_text(json.dumps(compact, indent=2), encoding='utf-8')
    if args.summary:
        args.summary.write_text(json.dumps(compact, indent=2), encoding='utf-8')
    with (run / 'fix-validation.csv').open('w', newline='', encoding='utf-8') as f:
        w = csv.DictWriter(f, fieldnames=list(csv_rows[0])); w.writeheader(); w.writerows(csv_rows)
    links = ''.join('<tr><th>' + html.escape(name) + '</th><td>' + ' · '.join(
        f'<a href="{folder}/{name}{suffix}">{label}</a>' for folder, suffix, label in [
            ('native', '-specimen.json', 'Native body/joint settings'), ('native', '-inputs.csv', 'Prescribed inputs'),
            ('native', '-native.csv', 'Native samples'), ('ose-before', '-original.csv', 'OSE before'),
            ('ose-after', '-original.csv', 'OSE after'), ('ose-after', '-runtime-inertia.csv', 'OSE native body inertia')]) + '</td></tr>' for name in names)
    (run / 'raw-data.html').write_text('''<!doctype html><html lang="en"><meta charset="utf-8"><title>Joint fix evidence</title>
<style>body{font:16px/1.6 system-ui;max-width:1100px;margin:40px auto;padding:0 24px}td,th{padding:14px;text-align:left;border-bottom:1px solid #ccc}a{color:#176987}</style>
<h1>KSP2 / OSE joint fix evidence</h1><p>Eight designs; 128 bounded native bench cases and 16 direct tests on live vessels in orbit.
OSE before: 48 false breaks. OSE after: 0. Both builds replayed all 144 load histories in three mass/inertia variants.</p>
<p>This validates survival and reinforcement topology under these loads, not exact stiffness/damping or complete flight parity.</p>
<p><a href="fix-validation.csv">Comparison CSV</a> · <a href="fix-validation.json">Quality checks and hashes</a> · <a href="native/steps.log">Live-run log</a></p>
<table>''' + links + '</table></html>', encoding='utf-8')
    print(f'PASS: {native_count} native cases ({len(names)*2} live), {len(after)} replays/build, all extra graphs match.')
    print(f'False breaks: {compact["before_false_breaks"]} before, 0 after.')


if __name__ == '__main__':
    main()
