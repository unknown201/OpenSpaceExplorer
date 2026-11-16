"""Summarize actual runtime records. Exit 1 for measured gaps, 2 for incomplete evidence."""
import argparse
from collections import defaultdict
import hashlib
import json
import math
from pathlib import Path


def objects(path):
    """Accept compact JSONL and the game's inherited indented JSON serializer."""
    text = path.read_text(encoding='utf-8-sig')
    decoder = json.JSONDecoder()
    result = []
    offset = 0
    while offset < len(text):
        while offset < len(text) and text[offset].isspace():
            offset += 1
        if offset == len(text):
            break
        value, offset = decoder.raw_decode(text, offset)
        result.append(value)
    return result


def span(rows, key):
    values = [r[key] for r in rows]
    assert values and all(math.isfinite(x) for x in values), key
    return [min(values), max(values)]


def analyze(root):
    native = root / 'native'
    assert (native / 'done.txt').is_file(), 'Native run has not completed'
    assert not (native / 'failed.txt').exists(), 'Native capture failed'
    events = objects(native / 'events.jsonl')
    samples = objects(native / 'samples.jsonl')
    assert len(samples) >= 100, 'Insufficient native samples'
    assert all(b['real_s'] >= a['real_s'] and b['ut'] >= a['ut'] for a, b in zip(samples, samples[1:])), 'Nonmonotonic sample clocks'
    assert any(e['action'] == 'complete' for e in events)
    conditions = next(e['detail'] for e in events if e['action'] == 'conditions')
    assert not conditions['infinitePower'], 'Infinite power invalidates EC measurements'
    ose_sim = json.loads((root / 'ose-sim.json').read_text())
    ose_ws = json.loads((root / 'ose-ws-summary.json').read_text())
    summary = {'native_conditions': conditions, 'native_samples': len(samples), 'solar': {}, 'link': [], 'transmissions': [], 'flags': [], 'ose_sim': ose_sim, 'ose_ws': ose_ws}
    phases = defaultdict(list)
    for s in samples:
        phases[s['phase']].append(s)
    solar_dir = root / 'native-solar'
    eclipse_rows = []
    if solar_dir.exists():
        assert (solar_dir / 'done.txt').exists() and not (solar_dir / 'failed.txt').exists(), 'Orbital solar run did not complete'
        solar_samples = objects(solar_dir / 'samples.jsonl')
        solar_events = objects(solar_dir / 'events.jsonl')
        assert all(b['ut'] >= a['ut'] for a, b in zip(solar_samples, solar_samples[1:])), 'Solar clock regressed'
        summary['solar_environments'] = [e['detail'] for e in solar_events if e['action'] == 'solar_environment']
        assert len(summary['solar_environments']) == 4, 'Missing orbital geometry records'
        for s in solar_samples:
            phases['solar_supplement/' + s['phase']].append(s)
        eclipse_rows = [s for s in solar_samples if s['solar'] and all(p['scale'] == 0 and p['blocker'] for p in s['solar'])]
        assert eclipse_rows and all(p['flow'] == 0 for s in eclipse_rows[-5:] for p in s['solar']), 'No settled native eclipse with zero output'
        assert any(p['scale'] > 0 for s in solar_samples for p in s['solar']), 'No lit orbital comparison'
        summary['native_eclipse_samples'] = len(eclipse_rows)
    for name, rows in phases.items():
        if not name.startswith('solar'):
            continue
        panels = defaultdict(list)
        for row in rows:
            for p in row['solar']:
                panels[p['part']].append(p)
        dt = rows[-1]['ut'] - rows[0]['ut']
        summary['solar'][name] = {'duration_sim_s': dt, 'duration_real_s': rows[-1]['real_s'] - rows[0]['real_s'],
            'ec_first': rows[0]['ec'], 'ec_last': rows[-1]['ec'], 'net_ec_per_sim_s': (rows[-1]['ec'] - rows[0]['ec']) / dt if dt else None,
            'panels': {part: {'flow_range': span(ps, 'flow'), 'scale_range': span(ps, 'scale'),
                             'exposures': sorted(set(p['exposure'] for p in ps)), 'states': sorted(set(p['state'] for p in ps)),
                             'blockers': sorted(set(p['blocker'] for p in ps)),
                             'first_pivot': ps[0]['pivot_rotation'], 'last_pivot': ps[-1]['pivot_rotation']} for part, ps in panels.items()}}
    for e in events:
        if e['action'] not in ('range_cut', 'range_restore'):
            continue
        desired = 'Connected' if e['action'] == 'range_restore' else 'Disconnected'
        # Match the first later sample with the requested observable link state.
        later = [s for s in samples if s['real_s'] >= e['real_s']]
        hit = next((s for s in later if s['link'] == desired), None)
        if hit:
            prior = [s for s in samples if e['real_s'] <= s['real_s'] < hit['real_s']]
            lower_real = max(0, prior[-1]['real_s'] - e['real_s']) if prior else 0
            summary['link'].append({'phase': e['phase'], 'action': e['action'], 'latency_real_s': hit['real_s'] - e['real_s'],
                                    'latency_real_lower_s': lower_real, 'latency_sim_s': hit['ut'] - e['ut']})
        else:
            summary['link'].append({'phase': e['phase'], 'action': e['action'], 'unobserved': True})
    summary['transmissions'] = [e for e in events if e['action'].startswith('transmi')]
    flags_dir = root / 'native-flags'
    if flags_dir.exists():
        assert (flags_dir / 'done.txt').exists() and not (flags_dir / 'failed.txt').exists(), 'Flag run did not complete'
        flag_events = objects(flags_dir / 'events.jsonl')
        summary['flag_run'] = 'Separate terrestrial OAB launch without teleport; see native-flags'
    else:
        flag_events = events
    summary['flags'] = [e for e in flag_events if e['action'].startswith('flag_')]
    terminal = [e for e in summary['transmissions'] if e['action'] == 'transmission_update' and e['detail']['state'] != 'Transmitting']
    summary['terminal_transmissions'] = terminal
    assert any(e['detail']['state'] == 'Complete' for e in terminal), 'No completed native transmission'
    assert any(e['detail']['state'] == 'OutOfResources' for e in terminal), 'No native power interruption'
    assert len(summary['link']) >= 6 and not any(x.get('unobserved') for x in summary['link']), 'Missing link transitions'
    assert any(e['action'] == 'flag_placed' for e in summary['flags']), 'No native flag placement'
    assert any(e['action'] == 'flag_recovered' for e in summary['flags']), 'No native flag recovery'
    # Derive the two comparable behavioral checks from measured results. A successful
    # capture and partially matched setup must never become an overall parity pass.
    ose_empty = [r for r in ose_ws if r['case'] == 'transmission' and r['starting_charge_fraction'] == 0]
    power_gap = any(r['reason'] is None and r['submitted_reports'] for r in ose_empty)
    ose_delay_max = max(r['latency_sim_s'] for r in ose_sim if r['case'] == 'link_transition')
    link_gap = any(r['latency_sim_s'] > ose_delay_max + 0.1 for r in summary['link'])
    placements = [r for r in ose_ws if r['case'] == 'flag_placement' and r['accepted']]
    pickup = next(e['detail'] for e in summary['flags'] if e['action'] == 'flag_recovered')
    flag_gap = len(placements) > 1 and pickup['carrying'] and pickup['remaining'] == 0
    summary['alignment'] = [
        {'area': 'solar incidence / tracking', 'status': 'partial', 'reason': 'Both runtimes measured; exact attitudes and tracking response are not matched.'},
        {'area': 'celestial shading / EC', 'status': 'aligned (on/off)' if eclipse_rows else 'unverified', 'reason': 'Native blocker/zero output and OSE geometric eclipse; lit flux normalization and attitudes are not matched.'},
        {'area': 'CommNet timing', 'status': 'mismatch' if link_gap else 'partial', 'reason': 'Compare measured transition delays in link and ose_sim; clocks and injection conditions are retained.'},
        {'area': 'transmission power / duration', 'status': 'mismatch' if power_gap else 'partial', 'reason': 'Native transmitter times payloads and stops on resource loss; compare OSE submissions at zero EC.'},
        {'area': 'flag placement / pickup', 'status': 'mismatch' if flag_gap else 'partial', 'reason': 'OSE repeats planting and has vessel recovery; native pickup restores the carried flag. Pickup intent availability also checked in source.'},
        {'area': 'flag persistence / transport reconnect', 'status': 'OSE verified', 'reason': 'OSE live WebSocket save/load/reconnect exercised; native save/reload not matched.'}]
    summary['provenance'] = {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
                             for p in [native / 'events.jsonl', native / 'samples.jsonl', root / 'ose-sim.json', root / 'ose-ws-summary.json', root / 'ose-ws.jsonl']}
    if flags_dir.exists():
        for name in ['events.jsonl','samples.jsonl']:
            path = flags_dir / name
            summary['provenance'][str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    if solar_dir.exists():
        for name in ['events.jsonl','samples.jsonl']:
            path = solar_dir / name
            summary['provenance'][str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    return summary


def main():
    p = argparse.ArgumentParser()
    p.add_argument('root', type=Path)
    args = p.parse_args()
    try:
        result = analyze(args.root)
    except (AssertionError, OSError, ValueError, StopIteration) as e:
        print('INCOMPLETE:', e)
        return 2
    (args.root / 'comparison.json').write_text(json.dumps(result, indent=2), encoding='utf-8')
    for row in result['alignment']:
        print(row['status'].upper() + ': ' + row['area'])
    if any(r['status'] == 'mismatch' for r in result['alignment']):
        return 1
    return 2 if any(r['status'] != 'aligned' for r in result['alignment']) else 0


if __name__ == '__main__':
    raise SystemExit(main())
