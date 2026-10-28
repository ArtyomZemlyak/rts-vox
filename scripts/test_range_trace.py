"""Offline export regression tests: clocks, attribution, nesting and budgets."""
import csv
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

import range_trace

spec = importlib.util.spec_from_file_location("profile_for_trace", Path(__file__).with_name("analyze-range-profile.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.path = Path(self.temp.name)
        self.report = dict(frame_alignment="shared_monotonic_epoch", valid_for_comparison=True,
            reasons=[], state_hash="abcd", physics_task_sampling="longest_per_thread_family_stage",
            trace_steps=0, manifest=dict(BinarySha256="a" * 64))
        self.frames = [dict(frame_start_ns="1000", frame_end_ns="5001000", generation="0", tick="7",
            simulation_ticks="2", simulation_core_ms="3", frame_ms="999", fragment_records="4"),
            dict(frame_start_ns="60001000", frame_end_ns="61001000", generation="0", tick="7",
            simulation_ticks="0", simulation_core_ms="0", frame_ms="1", fragment_records="9")]
        self.write("csv", self.frames)

    def write(self, extension, data):
        with (self.path / f"world-battle.{extension}").open("w", newline="", encoding="utf-8") as out:
            writer = csv.DictWriter(out, list(data[0]))
            writer.writeheader()
            writer.writerows(data)

    def export(self, **kwargs):
        meta = range_trace.export(self.path, "world-battle", self.report,
            summarize_tick=profile.summarize_tick, **kwargs)
        data = json.loads((self.path / "world-battle.trace.json").read_text(encoding="utf-8"))
        return meta, data

    def test_intervals_use_preceding_update_and_keep_terminal_slice(self):
        meta, data = self.export()
        events = data["traceEvents"]
        interval = next(e for e in events if e["name"] == "Frame interval")
        self.assertEqual((interval["ts"], interval["dur"]), (1, 60000))
        self.assertEqual(interval["args"]["row"], 0)
        self.assertEqual(len([e for e in events if e["name"] == "Update"]), 2)
        self.assertEqual(len([e for e in events if e["name"] == "Frame interval"]), 1)
        records = [json.loads(s) for s in (self.path / "world-battle.slow-frames.jsonl").read_text().splitlines()]
        self.assertEqual(len(records), 1)
        row = records[0]
        self.assertEqual((row["frame_ms"], row["update_ms"], row["following_gap_ms"]), (60, 5, 55))
        self.assertEqual((row["first_tick"], row["tick"], row["simulation_ticks"]), (6, 7, 2))
        self.assertEqual(row["workload"], {"fragment_records": 4})
        self.assertEqual(meta["slow_frames"], 1)
        self.assertEqual(data["otherData"]["state_hash"], "abcd")

    def core(self):
        def mark(lane, event, offset, thread="1", unit=""):
            return dict(generation="0", tick="1", lane=lane, event=event, offset_ns=str(offset),
                profile_offset_ns=str(1000 + offset), os_thread=thread, unit=unit,
                thread_cpu_ns="", cpu_cycles="")
        self.write("core.csv", [mark("tick", "begin", 0),
            mark("task", "physics_parent_begin", 5, "2", "6"),
            mark("task", "physics_child_begin", 10, "2", "6"),
            mark("task", "physics_child_end", 20, "2", "6"),
            mark("task", "physics_parent_end", 30, "2", "6"),
            mark("phase", "work", 40, "2"), mark("tick", "end", 50)])
        self.report["trace_steps"] = 1

    def test_worker_samples_keep_exact_clocks_identity_and_sampling_warning(self):
        self.core()
        _, data = self.export()
        jobs = [e for e in data["traceEvents"] if e.get("cat") == "worker"]
        self.assertEqual(len(jobs), 2)
        child = next(e for e in jobs if e["name"] == "physics_child")
        self.assertAlmostEqual(child["ts"], 1.01)
        self.assertAlmostEqual(child["dur"], .01)
        self.assertEqual(child["args"]["os_thread"], "2")
        self.assertIsNone(child["args"]["thread_cpu_ns"])
        self.assertEqual(child["args"]["retained_physics_sampling"], "longest_per_thread_family_stage")
        self.assertEqual(jobs[0]["tid"], jobs[1]["tid"])  # Nested scopes stay together.
        tick = next(e for e in data["traceEvents"] if e["name"] == "Tick")
        self.assertTrue(tick["args"]["delegated"])
        # Complete per-family totals have no time placement; never manufacture intervals.
        (self.path / "world-battle.physics.csv").write_text("not a timed trace")
        self.assertEqual(self.export()[1], data)

    def test_crossing_observer_intervals_get_separate_logical_lanes(self):
        self.core()
        p = self.path / "world-battle.core.csv"
        source = list(range_trace.rows(p))
        a = next(r for r in source if r["event"] == "physics_parent_end")
        a["offset_ns"], a["profile_offset_ns"] = "15", "1015"
        self.write("core.csv", source)
        _, data = self.export()
        jobs = [e for e in data["traceEvents"] if e.get("cat") == "worker"]
        self.assertNotEqual(jobs[0]["tid"], jobs[1]["tid"])

    def test_budget_is_explicit_and_metadata_does_not_consume_data_event_limit(self):
        self.core()
        meta, data = self.export(event_limit=2)
        self.assertEqual(meta["retained_events"], 2)
        self.assertGreater(meta["dropped_events"], 0)
        self.assertTrue(meta["truncated"])
        self.assertEqual(len([e for e in data["traceEvents"] if e["ph"] != "M"]), 2)
        self.assertEqual(data["otherData"], meta)
        self.assertEqual(json.loads((self.path / "world-battle.trace-export.json").read_text()), meta)
        self.assertTrue(meta["valid_for_comparison"])  # Offline export cannot change raw validity.

    def test_focus_selects_worst_interval_and_full_mode_restores_other_events(self):
        self.frames.append(dict(self.frames[-1], frame_start_ns="160001000", frame_end_ns="161001000"))
        self.write("csv", self.frames)
        meta, focused = self.export(focus_frames=1, context_ms=0)
        self.assertEqual(meta["focus_frame_rows"], [1])
        self.assertEqual(meta["focus_windows_ns"], [[60001000, 160001000]])
        self.assertGreater(meta["filtered_outside_focus"], 0)
        self.assertFalse(meta["truncated"])
        intervals = [e for e in focused["traceEvents"] if e["name"] == "Frame interval"]
        self.assertEqual([e["args"]["row"] for e in intervals], [1])
        meta, full = self.export(focus_frames=0)
        self.assertIsNone(meta["focus_windows_ns"])
        self.assertEqual(meta["filtered_outside_focus"], 0)
        self.assertEqual(len([e for e in full["traceEvents"] if e["name"] == "Frame interval"]), 2)
        # Journal covers the entire capture, independently of timeline focus.
        self.assertEqual(meta["slow_frames"], 2)

    def test_stale_optional_files_are_ignored_and_open_render_is_an_instant(self):
        self.write("render.csv", [dict(frame_row="0", generation="0", tick="7", stage="render_prepare",
            begin_ns="5001000", end_ns="", begin_os_thread="12")])
        self.assertEqual(self.export()[0]["render_records"], 0)
        self.report["render_timeline"] = dict(schema=5)
        meta, data = self.export()
        self.assertEqual(meta["render_records"], 1)
        opened = next(e for e in data["traceEvents"] if e["name"].endswith("(open at capture end)"))
        self.assertEqual(opened["ph"], "i")
        self.assertTrue(opened["args"]["incomplete"])
        self.assertNotIn("dur", opened)

    def test_legacy_clocks_invalid_limits_and_broken_ticks_fail_explicitly(self):
        for options in [dict(event_limit=0), dict(slow_frame_ms=float("nan")), dict(slow_frame_ms=0)]:
            with self.assertRaises(ValueError): self.export(**options)
        self.report["frame_alignment"] = "legacy"
        with self.assertRaises(ValueError): self.export()
        self.report["frame_alignment"] = "shared_monotonic_epoch"
        self.core()
        p = self.path / "world-battle.core.csv"
        p.write_text(p.read_text().replace("physics_child_end", "physics_missing_end"))
        with self.assertRaises(ValueError): self.export()
        self.assertFalse((self.path / "world-battle.trace.json.tmp").exists())


if __name__ == "__main__":
    unittest.main()
