"""Strict numerical KSP2/OSE interaction comparison. Exit 1 means a parity gap.

The default reference is compiled KSP2 source plus installed configuration;
passing it does not establish live desktop/browser input calibration.
"""
import argparse
import json
import math
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
REFERENCE = ROOT / 'crates/ose-client/testdata/ksp2-interactions.json'
CATEGORIES = {'throttle', 'flight_camera', 'map_camera', 'vab_camera', 'vab_rotation', 'symmetry', 'warp', 'auto_drop', 'maneuver'}


def equal_value(expected, actual):
    if isinstance(expected, bool):
        return isinstance(actual, bool) and expected == actual
    if isinstance(expected, (int, float)):
        return (isinstance(actual, (int, float)) and not isinstance(actual, bool)
                and math.isfinite(expected) and math.isfinite(actual)
                and math.isclose(expected, actual, rel_tol=2e-6, abs_tol=1e-6))
    if isinstance(expected, list):
        return (isinstance(actual, list) and len(expected) == len(actual)
                and all(equal_value(a, b) for a, b in zip(expected, actual)))
    return type(expected) is type(actual) and expected == actual


def compare(reference, observed, require_live=False):
    if reference.get('schema') != 1:
        raise ValueError('unsupported reference schema')
    cases = reference['cases']
    ids = [case['id'] for case in cases]
    if len(ids) != len(set(ids)) or not cases:
        raise ValueError('reference cases must be nonempty and have unique IDs')
    if {c['category'] for c in cases} != CATEGORIES:
        raise ValueError('reference must cover every interaction category')
    rows = observed['cases']
    observed_ids = [row['id'] for row in rows]
    if len(observed_ids) != len(set(observed_ids)):
        raise ValueError('duplicate OSE case IDs')
    if set(ids) != set(observed_ids):
        raise ValueError('missing or extra OSE cases')
    actual = {row['id']: row['actual'] for row in rows}
    results = []
    for case in cases:
        results.append(dict(id=case['id'], category=case['category'], input=case['input'],
                            expected=case['expected'], actual=actual[case['id']], unit=case['unit'],
                            aligned=equal_value(case['expected'], actual[case['id']])))
    summary = {category: {'passed': sum(r['aligned'] for r in results if r['category'] == category),
                          'total': sum(r['category'] == category for r in results)} for category in sorted(CATEGORIES)}
    live = reference.get('provenance', {}).get('live_input_capture') is True
    aligned = all(r['aligned'] for r in results)
    return dict(schema=1, reference_kind=reference['provenance']['kind'], live_input_capture=live,
                numerical_alignment=aligned, passed=aligned and (live or not require_live),
                summary=summary, cases=results, boundaries=reference.get('boundaries', []))


def run_probe(reference):
    result = subprocess.run(['cargo', 'run', '--quiet', '--offline', '-p', 'ose-client', '--example', 'interaction_probe', '--', str(reference)],
                            cwd=ROOT, check=True, stdout=subprocess.PIPE, text=True)
    return json.loads(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--reference', type=Path, default=REFERENCE)
    parser.add_argument('--ose-results', type=Path, help='Use previously saved output from interaction_probe')
    parser.add_argument('--report', type=Path, default=ROOT / '.cache/interaction-parity-report.json')
    parser.add_argument('--require-live', action='store_true', help='Also reject source-only references')
    args = parser.parse_args()
    reference = json.loads(args.reference.read_text(encoding='utf-8'))
    observed = json.loads(args.ose_results.read_text(encoding='utf-8')) if args.ose_results else run_probe(args.reference)
    report = compare(reference, observed, args.require_live)
    args.report.parent.mkdir(parents=True, exist_ok=True)
    args.report.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
    for category, result in report['summary'].items():
        print(f'{category}: {result["passed"]}/{result["total"]} aligned')
    failures = [r for r in report['cases'] if not r['aligned']]
    for category in sorted(CATEGORIES):
        example = next((r for r in failures if r['category'] == category), None)
        if example:
            print(f'  {example["id"]}: KSP2={example["expected"]}, OSE={example["actual"]} {example["unit"]}')
    print(f'Reference: {report["reference_kind"]}; live input capture: {report["live_input_capture"]}')
    print(f'Full report: {args.report}')
    return 0 if report['passed'] else 1


if __name__ == '__main__':
    raise SystemExit(main())
