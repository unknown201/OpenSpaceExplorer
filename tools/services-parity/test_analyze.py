"""Capture-integrity checks only; these generated objects are not reference evidence."""
import json
from pathlib import Path
import tempfile
import unittest
from analyze import objects, span


class CaptureIntegrity(unittest.TestCase):
    def test_game_indented_and_compact_records(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'samples.jsonl'
            path.write_text(json.dumps({'ut': 1}, indent=2) + '\n' + json.dumps({'ut': 2}) + '\n')
            self.assertEqual(objects(path), [{'ut': 1}, {'ut': 2}])

    def test_truncated_record_is_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            path = Path(d) / 'samples.jsonl'
            path.write_text('{"ut": 1}\n{"ut":')
            with self.assertRaises(json.JSONDecodeError):
                objects(path)

    def test_nonfinite_measurement_is_rejected(self):
        with self.assertRaises(AssertionError):
            span([{'flow': float('nan')}], 'flow')

    def test_empty_measurement_is_rejected(self):
        with self.assertRaises(AssertionError):
            span([], 'flow')


if __name__ == '__main__':
    unittest.main()
