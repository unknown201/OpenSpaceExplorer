"""Validate that the strict parity gate cannot silently lose coverage or pass drift."""
import copy
import json
import unittest

from check_interaction_parity import REFERENCE, compare, equal_value


class InteractionParityGateTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.reference = json.loads(REFERENCE.read_text(encoding='utf-8'))

    def observed(self):
        # Synthetic aligned input only tests the comparator, never game parity.
        return {'cases': [{'id': c['id'], 'actual': copy.deepcopy(c['expected'])} for c in self.reference['cases']]}

    def test_aligned_numeric_input_passes_but_does_not_claim_live_capture(self):
        report = compare(self.reference, self.observed())
        self.assertTrue(report['numerical_alignment'])
        self.assertFalse(report['live_input_capture'])
        self.assertFalse(compare(self.reference, self.observed(), require_live=True)['passed'])

    def test_drift_in_every_category_fails(self):
        for category in {c['category'] for c in self.reference['cases']}:
            with self.subTest(category=category):
                observed = self.observed()
                index = next(i for i, c in enumerate(self.reference['cases']) if c['category'] == category)
                observed['cases'][index]['actual'] = None
                self.assertFalse(compare(self.reference, observed)['passed'])

    def test_missing_extra_and_duplicate_cases_fail(self):
        observed = self.observed()
        observed['cases'].pop()
        with self.assertRaises(ValueError):
            compare(self.reference, observed)
        observed = self.observed()
        observed['cases'].append({'id': 'unexpected', 'actual': 0})
        with self.assertRaises(ValueError):
            compare(self.reference, observed)
        observed = self.observed()
        observed['cases'].append(observed['cases'][0])
        with self.assertRaises(ValueError):
            compare(self.reference, observed)

    def test_removing_reference_category_fails(self):
        reference = copy.deepcopy(self.reference)
        reference['cases'] = [c for c in reference['cases'] if c['category'] != 'maneuver']
        with self.assertRaises(ValueError):
            compare(reference, self.observed())

    def test_numbers_booleans_nonfinite_and_float_tolerance(self):
        self.assertFalse(equal_value(True, 1))
        self.assertFalse(equal_value(1, True))
        self.assertFalse(equal_value(float('nan'), float('nan')))
        self.assertFalse(equal_value(1, float('inf')))
        self.assertFalse(equal_value([1, 2], [1, 2, 3]))
        self.assertTrue(equal_value(0.4, 0.400000006))
        self.assertFalse(equal_value(0.4, 0.401))

    def test_reference_units_and_nonlinear_maneuver_hold_are_preserved(self):
        cases = {case['id']: case for case in self.reference['cases']}
        self.assertAlmostEqual(cases['vab-orbit-False']['expected'], 22.918312, places=4)
        self.assertAlmostEqual(cases['vab-orbit-True']['expected'], 5.729578, places=4)
        self.assertEqual(cases['maneuver-96-48-1']['expected'], 0.25)
        self.assertEqual(cases['maneuver-96-48-60']['expected'], 15)
        self.assertEqual(cases['maneuver-192-48-1']['expected'], 0.0625)
        self.assertEqual(cases['rotation-False-False']['expected'], 90)


if __name__ == '__main__':
    unittest.main()
