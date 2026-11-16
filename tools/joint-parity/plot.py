"""Export a static figure from comparison.json (requires matplotlib)."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

p = argparse.ArgumentParser()
p.add_argument('run', type=Path)
args = p.parse_args()
data = json.loads((args.run / 'comparison.json').read_text())
rows = [next(x for x in data['cases'] if x['craft'] == name and x['case'] == 'r0-x+' and x['mode'] == 'runtime-inertia')
        for name in ['stack-01', 'stack-06', 'stack-12']]
plt.rcParams.update({'font.family': 'DejaVu Sans', 'font.size': 11})
fig, ax = plt.subplots(figsize=(9, 5), layout='constrained')
for offset, key, label, color in [(-.18, 'native_break_load_kn', 'KSP2-built joints in native PhysX', '#276e88'),
                                 (.18, 'ose_break_load_kn', 'OSE, runtime mass and body inertia', '#bd5938')]:
    bars = ax.bar([i + offset for i in range(3)], [x[key] / 1000 for x in rows], width=.34, label=label, color=color)
    ax.bar_label(bars, fmt='%.1f', padding=4)
ax.set_xticks(range(3), ['1-tank stack', '6-tank stack', '12-tank stack'])
ax.set_ylabel('Applied lateral force at first joint break (MN)')
ax.set_title('OSE breaks at lower matched bending loads', loc='left', weight='bold', pad=16)
ax.set_ylim(0, 340)
ax.spines[['top', 'right']].set_visible(False)
ax.grid(axis='y', alpha=.18)
ax.set_axisbelow(True)
ax.legend(frameon=False, loc='upper left')
fig.text(.01, -.035, 'Controlled structural benchmark · 50 Hz · +X loading · 5% ramp steps · not launch failure thresholds', fontsize=9, color='#555555')
fig.savefig(args.run / 'tall-stack-comparison.png', dpi=180, bbox_inches='tight')
fig.savefig(args.run / 'tall-stack-comparison.svg', bbox_inches='tight')
print(args.run / 'tall-stack-comparison.png')
