"""Render measured traces into a standalone local HTML report (standard library only)."""
import argparse
import html
import json
from pathlib import Path
from analyze import objects

p = argparse.ArgumentParser()
p.add_argument('root', type=Path)
args = p.parse_args()
root = args.root
comparison = json.loads((root / 'comparison.json').read_text())
samples = objects(root / 'native/samples.jsonl')


def plot(title, rows, key, color, unit):
    start = rows[0]['ut']
    duration = max(0.02, rows[-1]['ut'] - start)
    maximum = max(1, max(r[key] or 0 for r in rows))
    points = []
    previous = None
    for r in rows:
        command = 'M' if previous is None or r['ut'] - previous > 0.25 else 'L'
        points.append(f'{command}{65+(r["ut"]-start)/duration*810:.2f},{210-(r[key] or 0)/maximum*155:.2f}')
        previous = r['ut']
    return f'''<svg viewBox="0 0 940 270" role="img" aria-label="{html.escape(title)}">
    <rect width="940" height="270" fill="#fff"/><text x="65" y="27" font-size="18">{html.escape(title)}</text>
    <path d="M65 50V210H885" fill="none" stroke="#8895a3"/>
    <text x="15" y="61">{maximum:.2f}</text><text x="39" y="215">0</text>
    <text x="65" y="238">0</text><text x="830" y="238">{duration:.2f} s</text>
    <text x="340" y="260">Simulation seconds from this panel's start</text>
    <text x="66" y="46" font-size="12">{unit}</text>
    <path d="{' '.join(points)}" fill="none" stroke="{color}" stroke-width="2"/></svg>'''


tx = [s for s in samples if s['phase'].startswith('tx_')]
links = comparison['link']
table = ''.join('<tr>' + ''.join(f'<td>{html.escape(str(row.get(k,"")))}</td>' for k in ('phase','action','latency_real_lower_s','latency_real_s','latency_sim_s')) + '</tr>' for row in links)
alignment = ''.join(f'<tr><td>{html.escape(r["area"])}</td><td>{html.escape(r["status"])}</td><td>{html.escape(r["reason"])}</td></tr>' for r in comparison['alignment'])
solar_table = ''
for name, row in comparison['solar'].items():
    producers = '<br>'.join(html.escape(part.split('.prefab')[0]) + ': ' + ', '.join(f'{x:.7g}' for x in panel['flow_range']) + ' EC/s; blocker ' + html.escape(', '.join(panel.get('blockers', [])) or 'none') for part, panel in row['panels'].items())
    solar_table += f'<tr><td>{html.escape(name)}</td><td>{row["duration_sim_s"]:.3f}</td><td>{row["ec_last"]-row["ec_first"]:.9g}</td><td>{producers}</td></tr>'
shots = ''.join(f'<figure><img src="{f.relative_to(root).as_posix()}"><figcaption>KSP2: {html.escape(f.stem)}</figcaption></figure>' for folder in ['native','native-flags','native-solar'] for f in sorted((root/folder).glob('*.png')))
document = f'''<!doctype html><meta charset="utf-8"><title>Live services alignment audit</title>
<style>body{{font:16px system-ui;color:#172637;background:#eef2f6;max-width:1120px;margin:40px auto;padding:0 20px}}h1{{font-size:32px}}table{{border-collapse:collapse;background:white;width:100%;font-size:14px}}td,th{{padding:10px;border:1px solid #ccd5df;text-align:left}}svg{{width:100%;margin:12px 0}}figure{{display:inline-block;width:46%;vertical-align:top;margin:2%}}img{{width:100%}}pre{{white-space:pre-wrap;background:white;padding:16px}}a{{color:#075bae}}</style>
<h1>Live services alignment audit</h1><p>2026-11-12 · Isolated KSP2 runtime + production OSE simulation and local WebSocket server.</p>
<p><strong>Alignment is incomplete.</strong> Scripted range/resource changes, synthetic native payloads and teleports are recorded. OSE uses actual research reports. Camera, attitudes and payload sizes are not an exact numerical pair. Capture integrity passing is separate from parity.</p>
<table><tr><th>Area</th><th>Result</th><th>Evidence limit</th></tr>{alignment}</table>
<h2>Native solar output and charge</h2><p>The orbital supplement records actual position/star vectors at four phases of a 100 km circular orbit. Longitude teleports alone did not establish an eclipse. Panel attitudes remain unmatched with OSE.</p>
<table><tr><th>Scenario</th><th>Sim seconds</th><th>Δ EC</th><th>Measured producer flow range / blocker</th></tr>{solar_table}</table>
<h2>Native transmission traces</h2><p>EC resets are deliberate fixture actions. Transmitter progress is fractional packets, as exposed by the running game; the trace does not imply discrete delivery every frame.</p>
{plot('KSP2 battery charge through transmission scenarios',tx,'ec','#1565c0','EC')}
{plot('KSP2 active transmission progress',tx,'tx_completed','#c05700','packets')}
<h2>Native CommNet transition latency</h2><p>Real-time transition brackets use the last old-state and first new-state samples. UT deltas are shown separately. These are range changes through module data and normal telemetry refresh, not flown range crossings.</p>
<table><tr><th>Phase</th><th>Action</th><th>Real lower (s)</th><th>Real upper (s)</th><th>UT upper (s)</th></tr>{table}</table>
<h2>Reference captures</h2>{shots}
<h2>OSE live results</h2><pre>{html.escape(json.dumps(comparison['ose_ws'],indent=2))}</pre>
<p>Raw evidence: <a href="comparison.json">comparison</a> · <a href="native/events.jsonl">native events</a> · <a href="native/samples.jsonl">native samples</a> · <a href="ose-sim.json">OSE simulation</a> · <a href="ose-ws.jsonl">OSE WebSocket capture</a> · <a href="provenance.json">provenance</a></p>'''
(root/'comparison.html').write_text(document, encoding='utf-8')
print(root/'comparison.html')
