"""Regression tests for attribution, null counters, trace integrity and A/B gates."""
import csv
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("range_profile", Path(__file__).with_name("analyze-range-profile.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


def mark(lane, event, ns, thread="1", unit="", cpu=None, tick=1, generation=0):
    return dict(generation=str(generation), tick=str(tick), lane=lane, event=event,
                unit=unit, offset_ns=str(ns), os_thread=thread, cpu_cycles="",
                thread_cpu_ns="" if cpu is None else str(cpu), profile_offset_ns=str(ns + 1000))


def trace():
    return [mark("tick", "begin", 0, cpu=100),
            mark("task", "parent_begin", 10, thread="2", unit="0", cpu=100),
            mark("task", "child_begin", 20, thread="2", unit="0", cpu=110),
            mark("task", "child_end", 50, thread="2", unit="0", cpu=140),
            mark("task", "parent_end", 70, thread="2", unit="0", cpu=160),
            mark("phase", "motion", 80, cpu=120), mark("tick", "end", 100, cpu=130)]


class TraceTests(unittest.TestCase):
    def test_inclusive_tasks_not_double_counted(self):
        tick = profile.summarize_tick(trace())
        self.assertEqual(tick["task_count"], 2)
        self.assertAlmostEqual(tick["worker_covered_wall_ms"]["2"], 60 / 1e6)
        self.assertAlmostEqual(sum(p["wall_ms"] for p in tick["phases"]), tick["wall_ms"])
        self.assertEqual(tick["cycles"], None)
        self.assertEqual(tick["thread_cpu_ms"], 30 / 1e6)

    def test_same_job_on_two_workers_pairs_by_os_thread(self):
        rows = trace() + [mark("task", "parent_begin", 15, "3", "0", 200),
                          mark("task", "parent_end", 60, "3", "0", 210)]
        tick = profile.summarize_tick(rows)
        self.assertEqual(tick["task_count"], 3)
        self.assertAlmostEqual(tick["worker_covered_wall_ms"]["3"], 45 / 1e6)

    def test_missing_cpu_is_not_zero_and_coarse_difference_not_clamped(self):
        a, b = mark("tick", "begin", 0), mark("tick", "end", 10)
        self.assertIsNone(profile.interval(a, b)["thread_cpu_ms"])
        a["thread_cpu_ns"], b["thread_cpu_ns"] = "0", "20"
        self.assertLess(profile.interval(a, b)["wall_minus_cpu_ms"], 0)
        b["os_thread"] = "2"
        self.assertIsNone(profile.interval(a, b)["thread_cpu_ms"])

    def test_broken_trace_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "Unfinished"):
            profile.summarize_tick([r for r in trace() if r["event"] != "child_end"])
        with self.assertRaisesRegex(ValueError, "regressed"):
            profile.interval(mark("tick", "begin", 0, cpu=100), mark("tick", "end", 10, cpu=1))

    def test_unknown_worker_ids_are_not_paired_as_one_thread(self):
        rows = [dict(r, os_thread="") if r["lane"] == "task" else r for r in trace()]
        result = profile.summarize_tick(rows)
        self.assertEqual(result["unidentified_task_events"], 4)
        self.assertEqual(result["worker_covered_wall_ms"], {})

    def test_trace_boundaries_and_both_clocks_must_agree(self):
        before = trace() + [mark("task", "bad_begin", -1, "2", "0")]
        with self.assertRaisesRegex(ValueError, "outside"):
            profile.summarize_tick(before)
        clocks = trace()
        clocks[-1]["profile_offset_ns"] = "9999"
        with self.assertRaisesRegex(ValueError, "clocks disagree"):
            profile.summarize_tick(clocks)
        worker_phase = trace()
        worker_phase[-2]["os_thread"] = "2"
        with self.assertRaisesRegex(ValueError, "tick caller"):
            profile.summarize_tick(worker_phase)


class ReportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.frames = [dict(seconds="1", tick="1", generation="0", frame_ms="10", simulation_ticks="1",
                            simulation_core_ms="2", frame_start_ns="900", frame_end_ns="1200", frame_gap_ns="0"),
                       dict(seconds="2", tick="1", generation="0", frame_ms="250", simulation_ticks="0",
                            simulation_core_ms="0", frame_start_ns="2000000", frame_end_ns="2000100", frame_gap_ns="1998800")]

    def write(self, frames=None, rows=None, manifest=True):
        for name, data in [("csv", frames or self.frames), ("core.csv", rows or trace())]:
            with (self.path / f"world-battle.{name}").open("w", newline="") as dst:
                out = csv.DictWriter(dst, list(data[0])); out.writeheader(); out.writerows(data)
        (self.path / "world-battle.log").write_text('PROFILE tick_rate=30\nPROFILE physics_workers=8 navigation_workers=8\nPROFILE state_hash=0123456789abcdef\nPROFILE adapter_name="GPU" adapter_backend=Vulkan adapter_device_type=DiscreteGpu\n')
        if manifest:
            (self.path / "world-battle.manifest.json").write_text(json.dumps({"BinarySha256": "a" * 64, "InputHashes": [{"Path": "data/fixture.ron", "Sha256": "b" * 64}], "Validity": {"ValidForComparison": True, "Reasons": []}}))

    def test_denominator_is_real_steps_and_frame_gap_does_not_blame_current_tick(self):
        self.write()
        r = profile.analyze(self.path)
        self.assertEqual(r["core_ms_per_step"], 2)
        self.assertEqual(r["slow_frames"][0]["current_core_ms"], 0)
        self.assertEqual(r["slow_frames"][0]["previous_core_ms"], 2)
        self.assertEqual(r["slow_ticks"][0]["frame_row"], 0)
        self.assertTrue(r["valid_for_comparison"])

    def test_missing_ticks_and_legacy_manifest_never_qualify_a_comparison(self):
        frames = [dict(self.frames[0], simulation_ticks="2")]
        self.write(frames=frames, manifest=False)
        r = profile.analyze(self.path)
        self.assertFalse(r["valid_for_comparison"])
        self.assertIn("frame_core_tick_count_mismatch", r["reasons"])
        self.assertIn("legacy_run_without_manifest", r["reasons"])

    def test_reset_uses_generation_and_invalid_pairs_preserve_numbers(self):
        rows = trace() + [dict(r, generation="1", profile_offset_ns=str(int(r["profile_offset_ns"]) + 10000)) for r in trace()]
        frames = [self.frames[0], dict(self.frames[0], seconds="2", generation="1", frame_start_ns="10900", frame_end_ns="11200", frame_gap_ns="9700")]
        self.write(frames, rows)
        r = profile.analyze(self.path)
        self.assertEqual(r["trace_steps"], 2)
        b = dict(r, state_hash="different")
        comparison = profile.compare(r, b)
        self.assertFalse(comparison["eligible"])
        self.assertIn("different_or_missing:state_hash", comparison["reasons"])
        self.assertIsNotNone(comparison["after_core_ms_per_step"])

    def test_total_step_count_does_not_hide_wrong_generation_or_tick(self):
        for key, wrong in [("generation", "99"), ("tick", "99")]:
            with self.subTest(key=key):
                self.write(rows=[dict(r, **{key: wrong}) for r in trace()])
                report = profile.analyze(self.path)
                self.assertEqual(report["measured_steps"], report["trace_steps"])
                self.assertFalse(report["valid_for_comparison"])
                self.assertIn("frame_core_tick_identity_mismatch", report["reasons"])

    def test_multiple_steps_need_correct_order_and_frame_distribution(self):
        rows = trace() + [dict(r, tick="2", profile_offset_ns=str(int(r["profile_offset_ns"]) + 100)) for r in trace()]
        self.write(frames=[dict(self.frames[0], tick="2", simulation_ticks="2")], rows=rows)
        self.assertTrue(profile.analyze(self.path)["valid_for_comparison"])
        wrong = [dict(self.frames[0], tick="2"), dict(self.frames[1], tick="2", simulation_ticks="1")]
        self.write(frames=wrong, rows=rows)
        report = profile.analyze(self.path)
        self.assertEqual(report["measured_steps"], report["trace_steps"])
        self.assertIn("frame_core_step_distribution_mismatch", report["reasons"])

    def test_frame_clocks_and_missing_trace_clock_cannot_silently_join(self):
        for change in [dict(frame_end_ns="800"), dict(frame_gap_ns="1"), dict(frame_end_ns="")]:
            with self.subTest(change=change):
                self.write(frames=[dict(self.frames[0], **change)])
                with self.assertRaisesRegex(ValueError, "frame|Frame"):
                    profile.analyze(self.path)
        self.write(rows=[dict(r, profile_offset_ns="") for r in trace()])
        report = profile.analyze(self.path)
        self.assertFalse(report["valid_for_comparison"])
        self.assertIn("core_tick_clock_missing", report["reasons"])

    def test_plain_report_exposes_frame_stall_and_worker_details(self):
        self.write()
        report = profile.analyze(self.path)
        frame = report["slow_frames"][0]
        self.assertAlmostEqual(frame["previous_measured_slice_ms"] + frame["outside_observed_slice_ms"], frame["begin_to_begin_ms"])
        rendered = profile.markdown(report)
        self.assertIn("| 1 | 0/1 | 1.999 | 250.000 | 2.000 | 0.000 |", rendered)
        self.assertIn("task parent, unit 0, OS thread 2", rendered)
        self.assertIn("not an identified GPU/lock wait", rendered)

    def test_pipeline_lag_cannot_shift_stall_or_fps(self):
        frames = [dict(self.frames[0], frame_start_ns="0", frame_end_ns="30000000", observed_frame_ms="", frame_ms="4"),
                  dict(self.frames[1], frame_start_ns="32000000", frame_end_ns="33000000", frame_gap_ns="2000000", observed_frame_ms="32", frame_ms="4"),
                  dict(self.frames[1], frame_start_ns="34000000", frame_end_ns="35000000", frame_gap_ns="1000000", observed_frame_ms="2", frame_ms="32")]
        self.write(frames=frames)
        report = profile.analyze(self.path)
        self.assertTrue(report["valid_for_comparison"])
        self.assertEqual(report["slow_frames"][0]["row"], 1)
        self.assertEqual(report["slow_frames"][0]["previous_core_ms"], 2)
        self.assertEqual(report["slow_frames"][0]["frame_delta_ms"], 32)
        self.assertEqual(report["frame_ms"]["mean"], 17)
        frames[1]["observed_frame_ms"] = "4"
        self.write(frames=frames)
        with self.assertRaisesRegex(ValueError, "disagrees"):
            profile.analyze(self.path)

    def test_legacy_os_samples_without_frame_epoch_remain_unavailable(self):
        folder = self.path / 'world-battle-cpu'; folder.mkdir()
        (folder / 'cpu-process-samples.csv').write_text('Utc,Seconds,CpuMs\n')
        result = profile.os_window(self.path, 'world-battle', 'epoch_unix_ns=2000000000',
                                   [dict(frame_ms='10')], {})
        self.assertEqual(result['status'], 'unavailable_or_legacy')

    def test_duplicate_tick_is_rejected_instead_of_silently_merged(self):
        rows = trace() + [dict(r, generation="1") for r in trace()] + trace()
        self.write(rows=rows)
        with self.assertRaisesRegex(ValueError, "duplicated"):
            profile.analyze(self.path)

    def test_os_window_excludes_load_dump_and_tags_observed_threads(self):
        folder = self.path / 'world-battle-cpu'; folder.mkdir()
        samples = [dict(Seconds=str(i), Utc=f'1970-01-01T00:00:0{i}Z', CpuMs=str(v))
                   for i, v in [(1, 100), (2, 200), (3, 250), (4, 800)]]
        with (folder / 'cpu-process-samples.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, samples[0]); writer.writeheader(); writer.writerows(samples)
        thread_rows = [dict(Seconds='3', ThreadId='2', CreatedAt='a', CpuMs='30'),
                       dict(Seconds='4', ThreadId='2', CreatedAt='b', CpuMs='400')]
        with (folder / 'cpu-threads.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, thread_rows[0]); writer.writeheader(); writer.writerows(thread_rows)
        result = profile.os_window(self.path, 'world-battle', 'epoch_unix_ns=2000000000',
                                   [dict(frame_end_ns='1000000000')], {'2': {'local_advance'}})
        self.assertEqual(result['intervals'], 1)
        self.assertEqual(result['process_cpu_ms'], 50)
        self.assertEqual(result['sampled_threads'][0]['observed_roles'], ['local_advance'])
        self.assertEqual(result['process_minus_sampled_threads_ms'], 20)


if __name__ == "__main__":
    unittest.main()
