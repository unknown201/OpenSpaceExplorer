"""Compare complete, matched live KSP2 and production OSE water traces. 0=match, 1=gap, 2=incomplete."""
import argparse
import bisect
from collections import defaultdict
import html
import json
import math
from pathlib import Path
import statistics

CASES = {'drop':45., 'bob':45., 'bank':45., 'drag':20., 'vertical_safe':6.,
         'vertical_destroy':3., 'horizontal_safe':3., 'horizontal_destroy':3.}
# Fixed before the accepted native run. Numerical/physical tolerances, not exact engine equivalence.
LIMITS = {'origin_alt_m':0.15, 'vertical_mps':0.6, 'horizontal_mps':0.5, 'bank_deg':3.0}
LABELS={'drop':'Gentle drop','bob':'Bobbing','bank':'25 degree righting','drag':'5 m/s lateral drag',
        'vertical_safe':'16 m/s downward','vertical_destroy':'22 m/s downward',
        'horizontal_safe':'100 m/s lateral','horizontal_destroy':'220 m/s lateral'}

def objects(path):
    text = Path(path).read_text(encoding='utf-8-sig')
    decoder = json.JSONDecoder()
    index = 0
    result = []
    while index < len(text):
        if text[index].isspace():
            index += 1
            continue
        value, index = decoder.raw_decode(text, index)
        result.append(value)
    return result

def group(rows):
    result = defaultdict(list)
    for row in rows:
        result[row['scenario']].append(row)
    return result

def validate(rows, label):
    for name, duration in CASES.items():
        trace = rows.get(name, [])
        if not trace or not trace[0]['alive'] or trace[0]['t'] > .1 or (trace[-1]['alive'] and trace[-1]['t'] < duration-.1):
            raise ValueError(f'{label}: incomplete {name}')
        previous = -math.inf
        dead_seen = False
        for row in trace:
            t = row['t']
            if not math.isfinite(t) or t <= previous:
                raise ValueError(f'{label}: unordered/nonfinite time in {name}')
            if previous != -math.inf and t-previous > .1:
                raise ValueError(f'{label}: sample gap in {name}')
            previous = t
            if row['alive']:
                if dead_seen:
                    raise ValueError(f'{label}: destroyed fixture revived in {name}')
                for metric in LIMITS:
                    if not math.isfinite(row[metric]):
                        raise ValueError(f'{label}: nonfinite {metric} in {name}')
            else:
                dead_seen = True
        if not any(r.get('splashed') or not r['alive'] for r in trace):
            raise ValueError(f'{label}: no water entry observed in {name}')

def interpolate(rows, t, key):
    times = [r['t'] for r in rows]
    right = bisect.bisect_left(times, t)
    if right == 0:
        return rows[0][key]
    if right == len(rows):
        return rows[-1][key]
    a,b = rows[right-1],rows[right]
    return a[key]+(b[key]-a[key])*(t-a['t'])/(b['t']-a['t'])

def metrics(trace):
    alive = [r for r in trace if r['alive']]
    final = trace[-1]
    result = {'survived':final['alive'], 'destruction_s':next((r['t'] for r in trace if not r['alive']), None)}
    if not alive:
        return result
    result['minimum_origin_alt_m'] = min(r['origin_alt_m'] for r in alive)
    if final['alive']:
        tail = [r for r in alive if r['t'] >= final['t']-5]
        result['equilibrium_origin_alt_m'] = statistics.mean(r['origin_alt_m'] for r in tail)
        result['final_bank_deg'] = statistics.mean(r['bank_deg'] for r in tail)
        result['tail_depth_std_m'] = statistics.pstdev(r['origin_alt_m'] for r in tail)
        result['final_horizontal_mps'] = final['horizontal_mps']
        # Consecutive downward crossings identify a full bob period after first water entry.
        wet = [r for r in alive if r.get('splashed')]
        crossings = [(a['t']+b['t'])/2 for a,b in zip(wet,wet[1:])
                     if a['vertical_mps'] > 0 >= b['vertical_mps'] and abs(a['vertical_mps']-b['vertical_mps']) > .002]
        result['bob_period_s'] = statistics.median(b-a for a,b in zip(crossings,crossings[1:])) if len(crossings)>1 else None
    return result

def compare(native, ose):
    validate(native,'native'); validate(ose,'OSE')
    results = []
    for name in CASES:
        n,o = native[name],ose[name]
        nm,om = metrics(n),metrics(o)
        checks = [{'metric':'survival','pass':nm['survived']==om['survived'],'native':nm['survived'],'ose':om['survived']}]
        if nm['destruction_s'] is not None and om['destruction_s'] is not None:
            error = abs(nm['destruction_s']-om['destruction_s'])
            checks.append({'metric':'destruction_time_s','pass':error<=.1,'absolute_error':error,'limit':.1})
        # Compare at native sample times, with no fitted time/height/angle shift.
        na = [r for r in n if r['alive']]
        oa = [r for r in o if r['alive']]
        for key,limit in LIMITS.items():
            errors = [r[key]-interpolate(oa,r['t'],key) for r in na if oa and oa[0]['t'] <= r['t'] <= oa[-1]['t']]
            if errors:
                rmse = math.sqrt(statistics.mean(x*x for x in errors))
                checks.append({'metric':key+'_rmse','pass':rmse<=limit,'rmse':rmse,'limit':limit})
                transient=[r[key]-interpolate(oa,r['t'],key) for r in na if oa and oa[0]['t']<=r['t']<=min(10.,oa[-1]['t'])]
                if transient:
                    error=math.sqrt(statistics.mean(x*x for x in transient))
                    checks.append({'metric':key+'_first_10s_rmse','pass':error<=limit,'rmse':error,'limit':limit})
        if nm['survived'] and om['survived']:
            error=abs(nm['minimum_origin_alt_m']-om['minimum_origin_alt_m'])
            checks.append({'metric':'peak_immersion_m','pass':error<=LIMITS['origin_alt_m'],'absolute_error':error,'limit':LIMITS['origin_alt_m']})
        if nm['survived'] and om['survived'] and CASES[name]>=45:
            error=abs(nm['equilibrium_origin_alt_m']-om['equilibrium_origin_alt_m'])
            checks.append({'metric':'equilibrium_origin_alt_m','pass':error<=.05,'absolute_error':error,'limit':.05})
            if name=='drop':
                checks.append({'metric':'observable_bobbing','pass':nm['bob_period_s'] is not None and om['bob_period_s'] is not None})
            if nm['bob_period_s'] is not None and om['bob_period_s'] is not None:
                error=abs(nm['bob_period_s']-om['bob_period_s'])
                checks.append({'metric':'bob_period_s','pass':error<=.2,'absolute_error':error,'limit':.2})
        results.append({'scenario':name,'pass':all(c['pass'] for c in checks),'checks':checks,'native':nm,'ose':om})
    return results

def render(root, result, native, ose):
    parts=['<!doctype html><meta charset="utf-8"><title>Water trajectory comparison</title>',
           '<style>body{font:15px system-ui;max-width:1150px;margin:40px auto;background:#101820;color:#dce8f0}svg{background:#192630;margin:8px}table{border-collapse:collapse}td,th{padding:8px;border:1px solid #48606e}.fail{color:#ff9d8a}.pass{color:#a9e8ad}polyline{fill:none;stroke-width:1.8}</style>',
           '<h1>Live KSP2 / OSE water comparison</h1><p>Blue: live KSP2. Orange: production OSE. Same inputs; no fitted time or height shift. Limits fixed before capture. A pass applies only to these fixtures.</p>']
    for case in result:
        name=case['scenario'];status='pass' if case['pass'] else 'fail'
        parts.append(f'<h2 class="{status}">{html.escape(LABELS[name])} — {status.upper()}</h2><table><tr><th>Check</th><th>Error / RMSE</th><th>Limit</th><th>Result</th></tr>')
        for c in case['checks']:
            error=c.get('rmse',c.get('absolute_error','—'))
            if isinstance(error,float):error=f'{error:.5g}'
            parts.append(f'<tr><td>{c["metric"]}</td><td>{error}</td><td>{c.get("limit","—")}</td><td>{c["pass"]}</td></tr>')
        parts.append('</table>')
        for key in LIMITS:
            traces=[[r for r in rows[name] if r['alive']] for rows in [native,ose]]
            values=[r[key] for trace in traces for r in trace]
            if not values:continue
            low,high=min(values),max(values)
            high=max(high,low+.01)
            parts.append(f'<svg viewBox="0 0 540 205" width="540" height="205"><text x="12" y="20" fill="#dce8f0">{key} [{low:.3g}, {high:.3g}]</text>')
            for trace,color in zip(traces,['#68b9ff','#ffb76b']):
                coords=' '.join(f'{20+r["t"]/CASES[name]*500:.2f},{180-(r[key]-low)/(high-low)*145:.2f}' for r in trace)
                parts.append(f'<polyline stroke="{color}" points="{coords}"/>')
            parts.append(f'<text x="450" y="200" fill="#dce8f0">{CASES[name]:g} s</text></svg>')
    (root/'comparison.html').write_text(''.join(parts),encoding='utf-8')

def main():
    p=argparse.ArgumentParser();p.add_argument('root',type=Path);a=p.parse_args();root=a.root
    try:
        if not (root/'native/done.txt').exists() or (root/'native/failed.txt').exists():
            raise ValueError('native run is unfinished or failed')
        events=objects(root/'native/events.jsonl')
        releases={e['scenario']:e['detail'] for e in events if e['action']=='release'}
        if set(releases)!=set(CASES) or not any(e['action']=='complete' for e in events):
            raise ValueError('native run missing releases/completion')
        ose_events={e['scenario']:e for e in json.loads((root/'ose-events.json').read_text())}
        for name,r in releases.items():
            if r['sample_phase']!='post_physics':
                raise ValueError(f'unmatched physics sample phase in {name}')
            if r['crew_count']!=ose_events[name]['crew_count']:
                raise ValueError(f'unmatched crew in {name}')
            if r['part']!='pod_1v_conical_crew' or abs(r['mass_t']-ose_events[name]['mass_t'])>.001:
                raise ValueError(f'unmatched native/OSE fixture mass or part in {name}')
            if abs(r['displacement']-ose_events[name]['volume_m3'])>.001:
                raise ValueError(f'unmatched displacement in {name}')
            for key in ('height','vy','vx','bank','duration'):
                if abs(r['input'][key]-ose_events[name]['input'][key])>1e-6:
                    raise ValueError(f'unmatched initial {key} in {name}')
            if abs(r['sample']['vertical_mps']-r['input']['vy'])>.05 or abs(r['sample']['horizontal_mps']-r['input']['vx'])>.05:
                raise ValueError(f'native release velocity was not applied in {name}')
            if abs(r['sample']['origin_alt_m']-r['input']['height'])>.01:
                raise ValueError(f'native release height was not applied in {name}')
            if abs(r['sample']['bank_deg']-r['input']['bank'])>.05:
                raise ValueError(f'native release bank was not applied in {name}')
        native=group(objects(root/'native/samples.jsonl'));ose=group(objects(root/'ose-samples.jsonl'))
        results=compare(native,ose)
        result={'status':'compatible' if all(c['pass'] for c in results) else 'measured_gaps', 'cases':results,'limits':LIMITS}
        render(root,results,native,ose)
        (root/'comparison.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
        for r in results:
            print(r['scenario'], 'PASS' if r['pass'] else 'GAP', ', '.join(c['metric'] for c in r['checks'] if not c['pass']))
        return 0 if result['status']=='compatible' else 1
    except (OSError, ValueError, KeyError, TypeError) as e:
        (root/'comparison.json').write_text(json.dumps({'status':'incomplete','reason':str(e)},indent=2),encoding='utf-8')
        print('INCOMPLETE:',e)
        return 2

if __name__=='__main__':
    raise SystemExit(main())
