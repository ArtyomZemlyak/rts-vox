import csv
import importlib.util
import tempfile
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    'audit', Path(__file__).with_name('analyze-performance-audit.py'))
audit = importlib.util.module_from_spec(spec)
spec.loader.exec_module(audit)


class AccountingTests(unittest.TestCase):
    def row(self):
        row = dict.fromkeys(audit.PHASES, 0)
        row.update(tick=1, total_us=10000, observed_phases_us=9000,
                   motion_us=6000, debris_us=2000, route_us=5000)
        return row

    def load(self, row):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / 'world-a.csv'
            with path.open('w', newline='', encoding='utf-8') as f:
                writer = csv.DictWriter(f, fieldnames=row.keys())
                writer.writeheader()
                writer.writerow(row)
            return audit.read_rows(path)

    def test_nested_routes_are_not_added_to_motion(self):
        rows = self.load(self.row())
        total, ranked = audit.rank(rows)
        self.assertEqual(total['mean'], 10)
        self.assertEqual(sum(r['share_pct'] for r in ranked), 100)
        values = {r['key']: r['mean'] for r in ranked}
        self.assertEqual(values['motion_us'], 6)
        self.assertNotIn('route_us', values)
        self.assertEqual(values['observed_other_us'], 1)
        self.assertEqual(values['unobserved_us'], 1)

    def test_invalid_accounting_is_rejected(self):
        for changes in (dict(total_us=8000), dict(observed_phases_us=7000),
                        dict(motion_us=-1)):
            row = self.row() | changes
            with self.subTest(changes=changes), self.assertRaises(ValueError):
                self.load(row)

    def test_missing_phase_is_rejected(self):
        row = self.row()
        del row['battle_ai_us']
        with self.assertRaises(ValueError):
            self.load(row)

    def test_statistics_include_zeros_and_use_pooled_samples(self):
        values = audit.stats([0] * 99 + [100])
        self.assertEqual(values['mean'], 1)
        self.assertEqual(values['p99'], 100)
        self.assertEqual(values['samples'], 100)
        with self.assertRaises(ValueError):
            audit.stats([float('nan')])

    def test_zero_duration_ranking_is_rejected(self):
        row = dict.fromkeys(audit.PHASES, 0)
        row.update(total_us=0, observed_other_us=0, unobserved_us=0)
        with self.assertRaisesRegex(ValueError, 'zero total'):
            audit.rank([row])


class TrajectoryTests(unittest.TestCase):
    def analyze_pair(self, mutate=None, second_hash='1234567890abcdef'):
        with tempfile.TemporaryDirectory() as directory:
            paths = []
            for suffix in ('a', 'b'):
                row = AccountingTests().row()
                for keys in audit.DETAILS.values():
                    for key in keys:
                        row.setdefault(key, 0)
                row.update(dict.fromkeys(audit.COUNTERS, 0))
                if suffix == 'b' and mutate:
                    mutate(row)
                path = Path(directory) / f'world-{suffix}.csv'
                with path.open('w', newline='', encoding='utf-8') as f:
                    writer = csv.DictWriter(f, fieldnames=row.keys())
                    writer.writeheader()
                    writer.writerow(row)
                endpoint = '1234567890abcdef' if suffix == 'a' else second_hash
                path.with_suffix('.log').write_text(f'ticks=1 hash={endpoint}\n')
                paths.append(path)
            return audit.analyze(paths)

    def test_paired_run_rejects_divergent_counters(self):
        with self.assertRaisesRegex(ValueError, 'counters differ'):
            self.analyze_pair(lambda row: row.update(felling=1))

    def test_paired_run_rejects_divergent_endpoint(self):
        with self.assertRaisesRegex(ValueError, 'endpoint hashes differ'):
            self.analyze_pair(second_hash='0000000000000000')

    def test_paired_run_rejects_uncompared_counter_schema(self):
        with self.assertRaisesRegex(ValueError, 'schemas differ'):
            self.analyze_pair(lambda row: row.update(uncompared_counter=0))

    def test_identical_runs_publish_full_disjoint_ranking(self):
        report = self.analyze_pair()['world']
        self.assertEqual(report['runs'], 2)
        self.assertEqual(report['total_ms']['samples'], 2)
        self.assertEqual(sum(p['share_pct'] for p in report['phases_ms']), 100)


if __name__ == '__main__':
    unittest.main()
