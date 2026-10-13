import csv
from pathlib import Path
import tempfile
import unittest
import range_gpu_profile as gpu


class GpuCaptureTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.frames = [dict(frame_start_ns=str(i*20_000_000), frame_end_ns=str(i*20_000_000+10_000_000),
                            simulation_input_ms='4', unit_mesh_ms='1', fragments_ms='1', presentation_ms='2') for i in range(3)]
        self.rows = [(0, 1, 'render/gas-region-0/elapsed_gpu', 2.0),
                     (1, 21_000_000, 'render/gas-region-0/elapsed_gpu', 4.0),
                     (0, 1, 'render/terrain-colour/elapsed_gpu', 1.0)]

    def write(self, rows=None):
        rows = self.rows if rows is None else rows
        with (self.path/'world-battle.gpu.csv').open('w', newline='') as out:
            writer = csv.writer(out)
            writer.writerow(['observation_row','available_ns','path','elapsed_gpu_ms'])
            writer.writerows(rows)
        log = f'PROFILE gpu_capture_schema=1 gpu_sample_limit=1000000 gpu_samples_seen={len(rows)} gpu_samples_retained={len(rows)} gpu_samples_dropped=0 gpu_samples_invalid=0 gpu_clock=publication_without_source_frame\n'
        passes = {}
        for _,_,path,value in rows:
            passes.setdefault(path, []).append(value)
        for path,values in passes.items():
            v = sorted(values)
            log += f'GPU {path} n={len(v)} p50_ms={v[len(v)//2]:.3f} p95_ms={v[len(v)*95//100]:.3f} p99_ms={v[len(v)*99//100]:.3f} max_ms={v[-1]:.3f} mean_ms={sum(v)/len(v):.6f}\n'
        return log

    def analyze(self, log, declared=True):
        reasons = []
        result = gpu.capture(self.path,'world-battle',log,dict(GpuTimestamps=declared),self.frames,reasons)
        return result, reasons

    def test_sparse_pass_means_are_normalized_to_publications_and_keep_coverage(self):
        result, reasons = self.analyze(self.write())
        self.assertEqual(reasons, [])
        self.assertEqual(result['batches'], 2)
        self.assertEqual(result['groups']['gas_raymarch']['mean'], 3)
        self.assertEqual(result['passes']['render/terrain-colour/elapsed_gpu']['mean'], 1)
        self.assertEqual(result['groups']['terrain']['mean'], .5)
        self.assertEqual(result['observed_leaf_sum_ms']['mean'], 3.5)
        self.assertEqual(result['batches_per_main_frame'], 2/3)

    def test_nested_parent_is_not_added_to_children(self):
        rows = [(0,1,'render/elapsed_gpu',8), (0,1,'render/terrain-colour/elapsed_gpu',3)]
        result,_ = self.analyze(self.write(rows))
        self.assertEqual(result['observed_leaf_sum_ms']['mean'],3)
        self.assertEqual(result['excluded_parent_scopes'],['render/elapsed_gpu'])

    def test_empty_missing_truncated_and_undeclared_capture_are_not_accepted(self):
        empty = self.write([])
        self.assertIn('gpu_samples_unavailable',self.analyze(empty)[1])
        log = self.write()
        self.assertIn('gpu_capture_mode_mismatch',self.analyze(log,False)[1])
        self.assertIn('gpu_capture_metadata_missing',self.analyze('')[1])
        self.assertIn('gpu_samples_truncated',self.analyze(log.replace('gpu_samples_seen=3','gpu_samples_seen=4').replace('gpu_samples_dropped=0','gpu_samples_dropped=1'))[1])
        (self.path/'world-battle.gpu.csv').unlink()
        self.assertIn('gpu_samples_artifact_missing',self.analyze(log)[1])

    def test_raw_counts_values_and_batch_identity_cannot_be_fabricated(self):
        for rows, message in [
            (self.rows+[self.rows[0]], 'duplicate'),
            ([(0,100_000_000,'render/a/elapsed_gpu',1)], 'outside'),
            ([(0,1,'render/a/elapsed_gpu',-1)], 'summary'),
            ([(0,1,'render/a/elapsed_gpu',float('nan'))], 'summary'),
            ([(0,1,'render/a/elapsed_gpu',1),(1,1,'render/b/elapsed_gpu',2)], 'different rows')]:
            with self.subTest(message=message):
                with self.assertRaisesRegex(ValueError,message):
                    self.analyze(self.write(rows))
        log = self.write().replace('mean_ms=3.000000','mean_ms=3.100000')
        with self.assertRaisesRegex(ValueError,'value mismatch'):
            self.analyze(log)
        log = self.write().replace('gpu_samples_seen=3','gpu_samples_seen=4')
        with self.assertRaisesRegex(ValueError,'count mismatch'):
            self.analyze(log)

    def test_legacy_aggregates_are_explicitly_incomplete(self):
        log = self.write().split('\n',1)[1]
        result, reasons = self.analyze(log)
        self.assertEqual(reasons, [])
        self.assertEqual(result['status'],'legacy_aggregates_only')
        self.assertIsNone(result['groups'])

    def test_main_wall_partition_closes_without_nested_core_or_terminal_update(self):
        result = gpu.frame_accounting(self.frames)
        self.assertEqual(result['wall_ms'],40)
        self.assertEqual(result['parts']['outside_update_ms']['mean'],10)
        self.assertEqual(result['parts']['unmarked_update_ms']['mean'],2)
        self.assertAlmostEqual(sum(v['wall_fraction'] for v in result['parts'].values()),1)
        self.frames[0]['simulation_core_ms'] = '999'
        self.assertEqual(gpu.frame_accounting(self.frames),result)
        self.frames[-1]['simulation_input_ms'] = '999'
        self.assertEqual(gpu.frame_accounting(self.frames),result)
        self.frames[0]['simulation_input_ms'] = '999'
        with self.assertRaisesRegex(ValueError,'overlaps'):
            gpu.frame_accounting(self.frames)


if __name__ == '__main__':
    unittest.main()
