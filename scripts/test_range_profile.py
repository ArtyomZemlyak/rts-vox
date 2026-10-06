"""Regression tests for attribution, null counters, trace integrity and A/B gates."""
import csv
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("range_profile", Path(__file__).with_name("analyze-range-profile.py"))
profile = importlib.util.module_from_spec(spec)
spec.loader.exec_module(profile)
CONTEXT = 'RenderContext { physical_size: [1600, 900], focused: true, scale_factor: 1.0, present_mode: AutoNoVsync, window_mode: Windowed, view: 1, camera_sweep: false, camera_controls: [0.0, 0.0, 1.0, 0.0, 0.0], zone: 0, overlays: [true, false], drawer: 0 }'


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


class OsPressureTests(unittest.TestCase):
    def rows(self):
        before = dict(OsPressureReadUs='10', SystemIdle100ns='0', SystemKernel100ns='0',
            SystemUser100ns='0', SystemCpuGroup='0', SystemCpuProcessors='8', MachineProcessors='8',
            PageFaults='4294967290', PhysicalAvailableBytes='42', SystemCommitBytes='50', SystemCommitLimitBytes='100')
        after = dict(before, SystemIdle100ns='60000000', SystemKernel100ns='70000000',
            SystemUser100ns='10000000', PageFaults='5')
        return before, after

    def test_group_cpu_excludes_idle_and_faults_wrap_as_dword(self):
        a, b = self.rows()
        r = profile.pressure_interval(a, b, 0, 1_000_000_000, 250)
        self.assertEqual(r['system_busy_percent'], 25)
        self.assertEqual(r['system_minus_process_cpu_ms'], 1750)
        self.assertEqual(r['page_fault_delta'], 11)
        self.assertEqual(r['commit_percent'], 50)

    def test_missing_fields_are_unavailable_and_coarse_difference_stays_signed(self):
        a, b = self.rows()
        self.assertIsNone(profile.pressure_interval({}, {}, 0, 1, 0))
        self.assertEqual(profile.pressure_interval(a, b, 0, 1, 2500)['system_minus_process_cpu_ms'], -500)
        b['PageFaults'] = ''; b['SystemUser100ns'] = ''
        r = profile.pressure_interval(a, b, 0, 1, 0)
        self.assertIsNone(r['page_fault_delta'])
        self.assertIsNone(r['system_busy_percent'])

    def test_group_change_or_counter_reset_cannot_look_like_low_cpu(self):
        a, b = self.rows()
        for changes in [dict(SystemCpuGroup='1'), dict(SystemIdle100ns='80000001')]:
            r = profile.pressure_interval(a, dict(b, **changes), 0, 1, 0)
            self.assertTrue(r['cpu_counter_discontinuity'])
            self.assertIsNone(r['system_busy_percent'])
        a['SystemKernel100ns'] = '80000000'
        self.assertTrue(profile.pressure_interval(a, b, 0, 1, 0)['cpu_counter_discontinuity'])

    def test_partial_machine_group_does_not_subtract_whole_process_cpu(self):
        a, b = self.rows()
        a['MachineProcessors'] = b['MachineProcessors'] = '128'
        r = profile.pressure_interval(a, b, 0, 1, 250)
        self.assertEqual(r['system_busy_percent'], 25)
        self.assertIsNone(r['system_minus_process_cpu_ms'])

    def test_invalid_faults_commit_and_read_cost_are_rejected(self):
        a, b = self.rows()
        for changes in [dict(OsPressureReadUs='nan'), dict(OsPressureReadUs='-1'),
                        dict(PageFaults='4294967296'), dict(SystemCommitBytes='101'), dict(SystemCommitLimitBytes='0'),
                        dict(SystemKernel100ns='-1'), dict(OsSamplerWorkMs='nan'), dict(OsSamplerWorkMs='-1')]:
            with self.assertRaises(ValueError): profile.pressure_interval(a, dict(b, **changes), 0, 1, 0)

    def test_episode_overlap_does_not_apportion_whole_interval_counters(self):
        a, b = self.rows()
        sample = profile.pressure_interval(a, b, 0, 1_000_000_000, 0)
        episodes = dict(retained=[dict(first_row=0, last_row=0)])
        frames = [dict(frame_start_ns='100000000', frame_end_ns='101000000')]
        profile.attach_os_pressure(episodes, frames, dict(pressure=dict(intervals=[sample])))
        overlap = episodes['retained'][0]['os_pressure']
        self.assertEqual(overlap['maximum_page_fault_delta'], 11)
        self.assertEqual(overlap['intervals'], 1)
        self.assertNotIn('cause_identified', overlap)


class CameraTourTests(unittest.TestCase):
    def test_camera_work_leaves_require_frame_identity_and_exclusive_parent(self):
        signature = ('PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1\n'
                     'PROFILE camera_work_schema=1 camera_work_scope=same_frame_exclusive_wall')
        header = ('seconds,tick,segment,origin_x,origin_z,progress,blocked,observe_ms,stream_ms,'
                  'frame_row,generation,work_kind,allocate_ms,rows_ms,publish_ms,retire_ms,occupancy_ms,advance_ms,shift_ms\n')
        with tempfile.TemporaryDirectory() as name:
            path = Path(name)/'world-battle.camera.csv'
            good = '0,30,0,0,0,,false,0,4,0,7,publish,0,1,1,.5,1,.25,0\n'
            main = [dict(tick='30', generation='7')]
            def analyze(row, log=signature, frames=main):
                path.write_text(header + row)
                reasons = []
                result = profile.camera_tour_capture(name, 'world-battle', log, {'CameraTour':True}, 1, reasons, frames)
                return result, reasons
            result, reasons = analyze(good)
            self.assertEqual(reasons, [])
            detail = result['work_details']['kinds']['publish']
            self.assertEqual(detail['phases']['occupancy_ms']['mean'], 1)
            self.assertEqual(detail['unscoped_ms']['mean'], .25)
            camera_rows = list(csv.DictReader((header + good + good.replace(',0,7,publish,', ',1,7,shift,')).splitlines()))
            peaks = [dict(row=0), dict(row=1)]
            profile.attach_camera_work(peaks, camera_rows)
            self.assertIsNone(peaks[0]['camera_previous_update'])
            self.assertEqual(peaks[1]['camera_previous_update']['kind'], 'publish')
            self.assertEqual(peaks[1]['camera_current_update']['kind'], 'shift')
            self.assertEqual(peaks[1]['camera_previous_update']['phases']['rows_ms'], 1)
            for bad in [good.replace(',0,7,publish,', ',1,7,publish,'),
                        good.replace(',0,7,publish,', ',0,6,publish,'),
                        good.replace(',publish,', ',unknown,'),
                        good.replace(',1,.5,1,', ',nan,.5,1,'),
                        good.replace(',1,.5,1,', ',9,.5,1,')]:
                self.assertIn('camera_tour_work_alignment_invalid', analyze(bad)[1])
            self.assertIn('camera_tour_work_alignment_invalid', analyze(good, frames=[dict(tick='31',generation='7')])[1])
            self.assertIn('camera_tour_work_schema_mismatch', analyze(good, log=signature.splitlines()[0])[1])

    def test_wall_clock_tour_retains_stopped_simulation_and_all_tick_groups(self):
        signature = 'PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1'
        with tempfile.TemporaryDirectory() as name:
            path = Path(name)/'world-battle.camera.csv'
            with path.open('w', newline='') as out:
                writer = csv.writer(out)
                writer.writerow('seconds,tick,segment,focus_x,focus_z,height,origin_x,origin_z,progress,blocked'.split(','))
                for seconds, tick, segment in [(0, 0, 0), (3, 0, 0), (4, 1, 0), (5, 1, 0), (6, 2, 1)]:
                    writer.writerow([seconds,tick,segment,100,100,240,0,0,50,'true'])
            reasons=[]
            tour=profile.camera_tour_capture(name,'world-battle',signature,{'CameraTour':True},5,reasons)
            self.assertEqual(reasons, [])
            self.assertEqual(tour['max_no_tick_seconds'], 3)
            self.assertEqual(tour['blocked_frames'], 5)
            self.assertEqual(tour['segments'], [0,1])
            profile.camera_tour_capture(name,'world-battle',signature,{'CameraTour':False},5,reasons)
            self.assertIn('camera_tour_mode_mismatch',reasons)
            reasons=[]
            profile.camera_tour_capture(name,'world-battle',signature,{'CameraTour':True},6,reasons)
            self.assertIn('camera_tour_frame_count_mismatch',reasons)

    def test_declared_tour_requires_runtime_mode_and_camera_artifact(self):
        with tempfile.TemporaryDirectory() as name:
            reasons=[]
            self.assertIsNone(profile.camera_tour_capture(name,'world-battle','',{'CameraTour':True},1,reasons))
            self.assertEqual(reasons,['camera_tour_mode_mismatch','camera_tour_artifact_missing'])

    def test_camera_work_is_per_frame_and_rejects_nonfinite_cost(self):
        signature = 'PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1'
        with tempfile.TemporaryDirectory() as name:
            path = Path(name) / 'world-battle.camera.csv'
            header = 'seconds,tick,segment,origin_x,origin_z,progress,blocked,observe_ms,stream_ms\n'
            path.write_text(header + '0,0,0,0,0,,false,0.5,2\n1,30,0,0,0,,false,1.5,4\n')
            reasons = []
            tour = profile.camera_tour_capture(name, 'world-battle', signature, {'CameraTour': True}, 2, reasons)
            self.assertEqual(reasons, [])
            self.assertEqual(tour['work_per_frame']['observe_ms']['mean'], 1)
            self.assertEqual(tour['work_per_frame']['stream_ms']['max'], 4)
            path.write_text(header + '0,0,0,0,0,,false,nan,2\n')
            profile.camera_tour_capture(name, 'world-battle', signature, {'CameraTour': True}, 1, reasons)
            self.assertIn('camera_tour_work_invalid', reasons)


class TraceTests(unittest.TestCase):
    def test_detail_groups_partition_only_their_parent_and_keep_scope_bounds(self):
        rows = [mark('tick', 'begin', 0), mark('phase', 'rubble', 5),
                mark('detail', 'motion_begin', 5),
                mark('task', 'route_begin', 10, '2', '0'),
                mark('task', 'nested_begin', 15, '2', '0'),
                mark('task', 'nested_end', 25, '2', '0'),
                mark('task', 'route_end', 30, '2', '0'),
                mark('detail', 'motion_routes', 40),
                mark('detail', 'motion_ground', 60), mark('phase', 'motion', 70),
                mark('phase', 'battle', 80), mark('detail', 'debris_begin', 80),
                mark('detail', 'settled_boxes', 90), mark('phase', 'debris', 100),
                mark('tick', 'end', 110)]
        tick = profile.summarize_tick(rows)
        self.assertEqual([d['group'] for d in tick['details']], ['motion'] * 3 + ['debris'] * 2)
        self.assertEqual([(d['start'], d['end']) for d in tick['details']], [(5,40),(40,60),(60,70),(80,90),(90,100)])
        first = tick['details'][0]['scope_coverage']
        self.assertAlmostEqual(first['observed_scope_ms'], 20 / 1e6)
        self.assertAlmostEqual(first['unobserved_scope_ms'], 15 / 1e6)
        self.assertAlmostEqual(first['longest_unobserved_scope_ms'], 10 / 1e6)
        self.assertEqual(first['per_thread_scope_ms'], {'2': 20 / 1e6})
        self.assertEqual(first['caller_scope_ms'], 0)
        actor = {r['family']:r for r in tick['actor_thread_totals']}
        self.assertEqual(set(actor), {'route','nested'})
        self.assertEqual(actor['route']['os_thread'], '2')
        self.assertAlmostEqual(actor['route']['inclusive_wall_ms'], 20 / 1e6)
        self.assertAlmostEqual(actor['nested']['inclusive_wall_ms'], 10 / 1e6)
        self.assertEqual(actor['route']['cpu_samples'], 0)
        self.assertAlmostEqual(sum(d['wall_ms'] for d in tick['details']), 85 / 1e6)

    def test_scope_clipping_concurrency_caller_and_compacted_lower_bound(self):
        jobs = [dict(start=0, end=25, thread='1'), dict(start=10, end=40, thread='2'),
                dict(start=20, end=30, thread='3'), dict(start=60, end=80, thread='3')]
        result = profile.scope_coverage(jobs, 15, 70, '1', 'longest_per_thread_family_stage', 2)
        self.assertEqual(result['kind'], 'retained_scopes_lower_bound')
        self.assertAlmostEqual(result['observed_scope_ms'], 35 / 1e6)
        self.assertAlmostEqual(result['unobserved_scope_ms'], 20 / 1e6)
        self.assertAlmostEqual(result['longest_unobserved_scope_ms'], 20 / 1e6)
        self.assertEqual(result['observed_threads'], 3)
        self.assertEqual(result['unidentified_task_events'], 2)
        self.assertAlmostEqual(result['caller_scope_ms'], 10 / 1e6)
        self.assertEqual(profile.scope_coverage([],0,10,'1','all_scopes',0)['kind'], 'unavailable_no_identified_scopes')

    def test_detail_unknown_begin_zero_interval_and_tick_tail_remain_explicit(self):
        rows = [mark('tick', 'begin', 0), mark('detail', 'unexpected', 3),
                mark('detail', 'next', 3), mark('detail', 'heat_begin', 5),
                mark('detail', 'heat_units', 8), mark('tick', 'end', 10)]
        details = profile.summarize_tick(rows)['details']
        self.assertEqual([d['group'] for d in details], [None,None,'heat','heat'])
        self.assertEqual([d['end']-d['start'] for d in details], [0,2,3,2])
        self.assertEqual(details[-1]['name'], 'heat_unmarked_tail')
        self.assertIsNone(details[0]['thread_cpu_ms'])

    def test_detail_wrong_caller_is_rejected(self):
        with self.assertRaisesRegex(ValueError, 'Detail boundary'):
            profile.summarize_tick(trace() + [mark('detail','motion_begin',5,'2')])

    def test_physics_batches_are_not_actor_zero(self):
        rows = trace() + [
            mark("task", "physics_translate_begin", 15, "3", "0", 200),
            mark("task", "physics_translate_end", 60, "3", "0", 210),
            mark("task", "physics_translate_begin", 16, "4", "0", 300),
            mark("task", "physics_translate_end", 65, "4", "0", 320),
        ]
        result = profile.summarize_tick(rows)
        jobs = [j for j in result["longest_tasks"] if j["domain"] == "physics"]
        self.assertEqual(len(jobs), 2)
        self.assertTrue(all(j["unit"] is None and j["job_slot"] == "0" for j in jobs))
        self.assertEqual(result["worker_families"]["3"], ["physics_translate"])
        self.assertAlmostEqual(result["worker_covered_wall_ms"]["4"], 49 / 1e6)
        family = result["task_families"]["physics_translate"]
        self.assertEqual(family["jobs"], 2)
        self.assertEqual(family["cpu_samples"], 2)
        self.assertAlmostEqual(family["inclusive_wall_ms"], 94 / 1e6)
        self.assertAlmostEqual(family["sampled_cpu_ms"], 30 / 1e6)
        self.assertAlmostEqual(family["max_wall_ms"], 49 / 1e6)

    def test_physics_batch_missing_end_is_not_hidden_by_another_thread(self):
        rows = trace() + [mark("task", "physics_wake_begin", 15, "3", "0"),
                          mark("task", "physics_wake_end", 60, "4", "0")]
        with self.assertRaisesRegex(ValueError, "Unmatched"):
            profile.summarize_tick(rows)

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


class FrameCadenceTests(unittest.TestCase):
    def frames(self):
        # Deliberately different current/preceding step counts and a reset.
        return [dict(generation=str(g), tick=str(t), simulation_ticks=str(n),
                     simulation_core_ms=str(core), frame_start_ns=str(a * 1000000),
                     frame_end_ns=str(b * 1000000), frame_gap_ns=str(gap * 1000000))
                for g, t, n, core, a, b, gap in [
                    (0, 1, 1, 2, 0, 3, 0), (0, 1, 0, 0, 10, 11, 7),
                    (1, 3, 3, 9, 15, 27, 4), (1, 4, 1, 8, 30, 40, 3)]]

    def test_group_uses_preceding_work_and_following_gap(self):
        result = profile.frame_cadence(self.frames(), True)
        idle, single, multi = result["groups"]
        self.assertEqual((idle["generation"], idle["steps_per_slice"]), (0, 0))
        # Sorted by source generation/steps: idle interval is 15 - 10.
        self.assertEqual(idle["interval_ms"]["mean"], 5)
        self.assertEqual(idle["following_gap_ms"]["mean"], 4)
        self.assertEqual((single["steps_per_slice"], single["interval_ms"]["mean"]), (1, 10))
        self.assertEqual(single["following_gap_ms"]["mean"], 7)
        self.assertEqual(multi["steps_per_slice"], 3)

    def test_partition_covers_each_begin_interval_once(self):
        result = profile.frame_cadence(self.frames(), True)
        self.assertEqual(result["intervals"], 3)
        self.assertEqual(result["covered_wall_ms"], 30)
        self.assertEqual(result["update_total_ms"], 16)
        self.assertEqual(result["following_gap_total_ms"], 14)
        self.assertEqual(result["core_total_ms"], 11)  # Nested, not added to 30.
        self.assertAlmostEqual(sum(r["wall_fraction"] for r in result["groups"]), 1)
        for group in result["groups"]:
            self.assertEqual(group["interval_total_ms"], group["update_total_ms"] + group["gap_total_ms"])

    def test_multistep_is_slice_total_without_inventing_tick_costs(self):
        group = profile.frame_cadence(self.frames(), True)["groups"][-1]
        self.assertEqual(group["completed_steps"], 3)
        self.assertEqual(group["core_ms"]["samples"], 1)
        self.assertEqual(group["core_ms"]["mean"], 9)
        self.assertNotIn("single_tick_ms", group)

    def test_terminal_work_is_explicit_without_fabricated_following_gap(self):
        result = profile.frame_cadence(self.frames(), True)
        self.assertEqual(result["completed_steps"], 4)
        self.assertEqual(result["terminal_slice"], dict(row=3, generation=1,
                         simulation_ticks=1, measured_wall_ms=10, core_ms=8))

    def test_reset_crossing_belongs_to_source_generation(self):
        result = profile.frame_cadence(self.frames(), True)
        crossing = next(g for g in result["groups"] if g["generation_crossings"])
        self.assertEqual((crossing["generation"], crossing["steps_per_slice"]), (0, 0))
        self.assertEqual(sum(g["generation_crossings"] for g in result["groups"]), 1)

    def test_single_slice_does_not_invent_an_interval_or_share(self):
        result = profile.frame_cadence(self.frames()[:1], True)
        self.assertEqual(result["groups"], [])
        self.assertEqual(result["covered_wall_ms"], 0)
        self.assertEqual(result["completed_steps"], 0)
        self.assertEqual(result["terminal_slice"]["simulation_ticks"], 1)

    def test_legacy_and_empty_inputs_are_unavailable(self):
        self.assertIsNone(profile.frame_cadence(self.frames(), False))
        self.assertIsNone(profile.frame_cadence([], True))


class ReportTests(unittest.TestCase):
    def test_build_context_is_explicit_and_compares_toolchain_flags_and_refresh(self):
        self.write()
        before = profile.analyze(self.path)
        self.assertEqual(profile.compare(before, before)["build_context_status"], "legacy_unrecorded")
        after = json.loads(json.dumps(before))
        evidence = dict(Toolchain=dict(Rustc=["rustc test"], Cargo=["cargo test"],
            BuildEnvironment={"RUSTFLAGS": None}, CargoLocalSha256="AA"), TrackedInputsRefreshed=True)
        after["manifest"]["BuildEvidence"] = dict(Evidence=evidence)
        self.assertIn("different_or_missing:recorded_build_context", profile.compare(before, after)["reasons"])
        self.assertEqual(profile.compare(after, after)["build_context_status"], "matched_recorded")
        for field, value in [("Rustc", ["other"]), ("Cargo", ["other"]),
                             ("BuildEnvironment", {"RUSTFLAGS": "-C opt-level=1"}), ("CargoLocalSha256", "BB")]:
            changed = json.loads(json.dumps(after))
            changed["manifest"]["BuildEvidence"]["Evidence"]["Toolchain"][field] = value
            self.assertIn("different_or_missing:recorded_build_context", profile.compare(after, changed)["reasons"])
        changed = json.loads(json.dumps(after))
        changed["manifest"]["BuildEvidence"]["Evidence"]["TrackedInputsRefreshed"] = False
        self.assertFalse(profile.compare(after, changed)["eligible"])

    def test_incomplete_build_context_cannot_claim_recorded_match(self):
        self.write()
        report = profile.analyze(self.path)
        for context in [None, [], {}, {"Rustc": [], "Cargo": ["cargo"]}]:
            report["manifest"]["BuildEvidence"] = dict(Evidence=dict(Toolchain=context))
            self.assertIsNone(profile.recorded_build_context(report))
            self.assertEqual(profile.compare(report, report)["build_context_status"], "legacy_unrecorded")
        for envelope in [None, [], "unknown", dict(Evidence=[]), dict(Evidence="unknown")]:
            report["manifest"]["BuildEvidence"] = envelope
            self.assertIsNone(profile.recorded_build_context(report))

    def test_camera_tour_has_separate_comparison_gate(self):
        self.write()
        before = profile.analyze(self.path)
        after = json.loads(json.dumps(before))
        after['manifest']['CameraTour'] = True
        self.assertIn('different:CameraTour', profile.compare(before, after)['reasons'])

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
        with (self.path / 'world-battle.log').open('a') as out:
            out.write(f'PROFILE render_context_schema=2\nPROFILE render_context={CONTEXT}\nPROFILE render_context_step=0 value={CONTEXT}\nPROFILE render_context_changed=false\n')
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
        cadence = r["frame_cadence"]
        self.assertEqual(cadence["completed_steps"], 1)
        self.assertEqual(cadence["groups"][0]["steps_per_slice"], 1)
        self.assertAlmostEqual(cadence["groups"][0]["following_gap_ms"]["mean"], 1.9988)
        self.assertIn("preceding Update plus its following gap", profile.markdown(r))

    def test_compute_override_checks_actual_pool_and_cannot_be_silently_ignored(self):
        for observed, marker, valid in [
                (8, 'Some(8)', True), (4, 'Some(8)', False),
                (8, 'None', False), (None, 'Some(8)', False)]:
            with self.subTest(observed=observed, marker=marker):
                self.write()
                path = self.path / 'world-battle.manifest.json'
                manifest = json.loads(path.read_text())
                manifest['ComputeWorkers'] = 8
                path.write_text(json.dumps(manifest))
                with (self.path / 'world-battle.log').open('a') as out:
                    if observed is not None:
                        out.write(f'PROFILE compute_workers={observed}\n')
                    out.write(f'PROFILE compute_override={marker}\n')
                result = profile.analyze(self.path)
                self.assertEqual(result['valid_for_comparison'], valid)
                self.assertEqual(result['actual_compute_workers'], observed)
                self.assertIn(f'actual compute workers: {observed}', profile.markdown(result))
                if not valid:
                    self.assertIn('compute_workers_override_mismatch', result['reasons'])

    def test_compute_experiment_parameters_are_comparison_keys_and_legacy_zero_is_default(self):
        self.write()
        before = profile.analyze(self.path)
        path = self.path / 'world-battle.manifest.json'
        manifest = json.loads(path.read_text())
        manifest['ComputeWorkers'] = 0
        path.write_text(json.dumps(manifest))
        after = profile.analyze(self.path)
        self.assertNotIn('different:ComputeWorkers', profile.compare(before, after)['reasons'])
        after['actual_compute_workers'] = 8
        before['actual_compute_workers'] = 4
        self.assertIn('different_or_missing:actual_compute_workers', profile.compare(before, after)['reasons'])
        manifest['ComputeWorkers'] = 8
        path.write_text(json.dumps(manifest))
        with (self.path / 'world-battle.log').open('a') as out:
            out.write('PROFILE compute_workers=8\nPROFILE compute_override=Some(8)\n')
        self.assertIn('different:ComputeWorkers', profile.compare(before, profile.analyze(self.path))['reasons'])

    def ordinary_stall_frames(self):
        frames = []
        for start, end, gap, tick, steps, core, input_ms in [
            (0, 750, 0, 1, 1, 700, 720),
            (800, 810, 50, 1, 0, 0, 1),
            (820, 900, 10, 3, 2, 60, 65),
            (910, 915, 10, 3, 0, 0, 1),
        ]:
            frames.append(dict(seconds=str(end / 1000), generation="0", tick=str(tick),
                               frame_ms="10", simulation_ticks=str(steps), simulation_core_ms=str(core),
                               frame_start_ns=str(start * 1000000), frame_end_ns=str(end * 1000000),
                               frame_gap_ns=str(gap * 1000000), simulation_input_ms=str(input_ms),
                               unit_mesh_ms="1", fragments_ms="1", presentation_ms="1",
                               occupancy_ms="2", physics_ms=str(core), terrain_ms="3", atmosphere_ms="1",
                               # A stale render observation on every row must
                               # never be attributed to these Update slices.
                               render_extract_ms="999", render_prepare_ms="888"))
        return frames

    def ordinary_report(self, frames):
        self.write(frames=frames)
        (self.path / "world-battle.core.csv").unlink()
        return profile.analyze(self.path)

    def test_ordinary_profile_exposes_correct_slice_without_core_trace_or_render_attribution(self):
        result = self.ordinary_report(self.ordinary_stall_frames())
        self.assertEqual(result["slow_ticks"], [])
        sample = result["slow_slices"][0]
        self.assertEqual(sample["row"], 0)
        self.assertEqual(sample["measured_wall_ms"], 750)
        self.assertEqual(sample["core_ms"], 700)
        self.assertEqual(sample["following_interval_row"], 1)
        self.assertEqual(sample["following_interval_ms"], 800)
        self.assertEqual(sample["following_gap_ms"], 50)
        self.assertEqual(sample["phases"]["simulation_input_ms"], 720)
        self.assertNotIn("render_extract_ms", sample["phases"])
        self.assertNotIn("physics_ms", sample["phases"])
        self.assertEqual(sample["nested_details"]["physics_ms"], 700)
        self.assertEqual(result["slice_summary"]["single_tick_ms"]["max"], 700)
        self.assertEqual(result["slice_summary"]["multi_step_slices"], 1)
        multi = next(s for s in result["slow_slices"] if s["simulation_ticks"] == 2)
        self.assertIsNone(multi["single_tick_ms"])
        self.assertEqual(multi["mean_core_ms_per_step"], 30)
        self.assertIn("Main slices", profile.markdown(result))

    def test_equal_slow_runs_keep_diagnostic_evidence_but_cannot_qualify_a_speed_pair(self):
        result = self.ordinary_report(self.ordinary_stall_frames())
        self.assertTrue(result["valid_for_comparison"])
        self.assertFalse(result["meets_simulation_rate"])
        self.assertFalse(result["acceptance_eligible"])
        pair = profile.compare(result, result)
        self.assertFalse(pair["eligible"])
        self.assertIn("before:simulation_not_realtime", pair["reasons"])
        self.assertIn("after:simulation_not_realtime", pair["reasons"])

    def test_stall_episodes_do_not_double_count_following_gaps_or_invent_multi_step_ticks(self):
        result = self.ordinary_report(self.ordinary_stall_frames())
        episodes = result["stall_episodes"]
        self.assertEqual(episodes["count"], 2)
        first, second = episodes["retained"]
        self.assertEqual((first["first_row"], first["last_row"]), (0, 0))
        self.assertEqual(first["covered_wall_ms"], 800)
        self.assertEqual(first["longest_single_tick_ms"], 700)
        self.assertEqual(second["covered_wall_ms"], 90)
        self.assertEqual(second["simulation_ticks"], 2)
        self.assertIsNone(second["longest_single_tick_ms"])

    def test_gap_only_stall_keeps_small_measured_slice_and_does_not_blame_core(self):
        frames = self.ordinary_stall_frames()
        frames[0].update(frame_end_ns="10000000", simulation_core_ms="2", simulation_input_ms="3")
        frames[1]["frame_gap_ns"] = "790000000"
        result = self.ordinary_report(frames)
        episode = result["stall_episodes"]["retained"][0]
        self.assertEqual(episode["measured_wall_ms"], 10)
        self.assertEqual(episode["following_gap_ms"], 790)
        self.assertEqual(episode["core_ms"], 2)
        self.assertEqual(episode["covered_wall_ms"], 800)

    def test_stall_episodes_split_generations_and_include_final_slice_without_next_interval(self):
        frames = self.ordinary_stall_frames()[:2]
        frames[1].update(generation="1", tick="0", simulation_ticks="2", simulation_core_ms="80", frame_end_ns="900000000")
        result = self.ordinary_report(frames)
        self.assertEqual(result["stall_episodes"]["count"], 2)
        final = next(s for s in result["slow_slices"] if s["generation"] == 1)
        self.assertEqual(final["first_tick"], (1 << 32) - 1)
        self.assertIsNone(final["following_interval_ms"])
        self.assertIsNone(final["following_gap_ms"])
        self.assertEqual(final["measured_wall_ms"], 100)

    def test_slice_and_episode_detail_is_bounded_without_losing_total_episode_count(self):
        frames = self.ordinary_stall_frames()
        summary, slow, episodes = profile.summarize_slices(frames, True, 1)
        self.assertEqual(len(slow), 1)
        self.assertEqual(len(episodes["retained"]), 1)
        self.assertEqual(episodes["count"], 2)
        self.assertEqual(summary["measured_wall_ms"]["samples"], 4)
        self.assertEqual(summary["phase_totals_ms"]["simulation_input_ms"], 787)

    def test_legacy_slices_keep_missing_clocks_and_phases_unavailable(self):
        frames = self.ordinary_stall_frames()
        for row in frames:
            for key in ("frame_start_ns", "frame_end_ns", "frame_gap_ns", *profile.SLICE_PHASES):
                del row[key]
        result = self.ordinary_report(frames)
        self.assertEqual(result["slow_slices"], [])
        self.assertIsNone(result["stall_episodes"])
        self.assertIsNone(result["slice_summary"]["measured_wall_ms"])
        self.assertTrue(all(value is None for value in result["slice_summary"]["phase_totals_ms"].values()))

    def write_stall_capture(self, rows=None, retained=1, threshold=50):
        self.write()
        (self.path / "world-battle.core.csv").unlink()
        marks = rows if rows is not None else [r for r in trace() if r["lane"] != "task"]
        with (self.path / "world-battle.stall.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(marks[0])); writer.writeheader(); writer.writerows(marks)
        with (self.path / "world-battle.log").open("a") as out:
            out.write(f"PROFILE stall_capture_schema=1 stall_capture_mode=phase_ring threshold_ms={threshold} pre_ticks=16 post_ticks=16 max_windows=4\n")
            out.write(f"PROFILE stall_windows=0 stall_slow_ticks=0 stall_unarmed_slow_ticks=0 stall_retained_ticks={retained}\n")
        path = self.path / "world-battle.manifest.json"
        manifest = json.loads(path.read_text()); manifest["CoreTriggerMilliseconds"] = threshold
        path.write_text(json.dumps(manifest))

    def test_phase_ring_is_partial_diagnostic_with_correct_tick_identity(self):
        self.write_stall_capture()
        # Another actual step is intentionally outside the retained context.
        frames = [dict(r) for r in self.frames]; frames[1].update(tick="2", simulation_ticks="1")
        with (self.path / "world-battle.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(frames[0])); writer.writeheader(); writer.writerows(frames)
        result = profile.analyze(self.path)
        self.assertTrue(result["valid_for_comparison"])
        self.assertFalse(result["acceptance_eligible"])
        self.assertEqual(result["trace_steps"], 0)
        self.assertEqual(result["measured_steps"], 2)
        capture = result["stall_trace"]
        self.assertTrue(capture["partial"])
        self.assertEqual(capture["retained_ticks"], 1)
        self.assertEqual(capture["ticks"][0]["frame_row"], 0)
        self.assertEqual(capture["ticks"][0]["task_count"], 0)
        self.assertIn("Phase-ring capture", profile.markdown(result))

    def test_phase_ring_rejects_declared_mode_mismatch_and_missing_metadata(self):
        self.write_stall_capture()
        path = self.path / "world-battle.manifest.json"
        manifest = json.loads(path.read_text()); manifest["CoreTriggerMilliseconds"] = 0
        path.write_text(json.dumps(manifest))
        result = profile.analyze(self.path)
        self.assertIn("stall_capture_mode_mismatch", result["reasons"])
        self.assertFalse(result["acceptance_eligible"])
        self.write_stall_capture()
        log = self.path / "world-battle.log"
        log.write_text("\n".join(line for line in log.read_text().splitlines() if "stall_capture_schema" not in line))
        self.assertIn("stall_capture_metadata_missing", profile.analyze(self.path)["reasons"])

    def test_phase_ring_rejects_worker_marks_or_wrong_frame_identity(self):
        self.write_stall_capture(rows=trace())
        with self.assertRaisesRegex(ValueError, "Worker/unit"):
            profile.analyze(self.path)
        self.write_stall_capture(rows=[dict(r, tick="2") for r in trace() if r["lane"] != "task"])
        with self.assertRaisesRegex(ValueError, "identity mismatch"):
            profile.analyze(self.path)

    def test_phase_ring_checks_retained_count_missing_file_and_budget(self):
        self.write_stall_capture(retained=2)
        with self.assertRaisesRegex(ValueError, "count mismatch"):
            profile.analyze(self.path)
        self.write_stall_capture()
        (self.path / "world-battle.stall.csv").unlink()
        with self.assertRaisesRegex(ValueError, "file missing"):
            profile.analyze(self.path)
        self.write_stall_capture(retained=1000)
        with self.assertRaisesRegex(ValueError, "budget/count"):
            profile.analyze(self.path)

    def test_phase_ring_duplicate_and_clock_overrun_are_not_silently_accepted(self):
        rows = [r for r in trace() if r["lane"] != "task"]
        self.write_stall_capture(rows=rows + rows)
        with self.assertRaisesRegex(ValueError, "exactly one begin"):
            profile.analyze(self.path)
        rows = [dict(r) for r in rows]
        rows[-1].update(offset_ns="1000", profile_offset_ns="2000")
        self.write_stall_capture(rows=rows)
        with self.assertRaisesRegex(ValueError, "outside frame"):
            profile.analyze(self.path)

    def test_boundary_only_capture_leaves_phase_cpu_unavailable_and_rejects_inconsistent_rows(self):
        self.write_stall_capture()
        legacy = profile.analyze(self.path)
        log = self.path / "world-battle.log"
        log.write_text(log.read_text().replace("max_windows=4", "max_windows=4 cpu_sampling=tick_boundaries"))
        # Existing fixture has phase CPU counters; the declaration must agree.
        with self.assertRaisesRegex(ValueError, "Unexpected phase CPU"):
            profile.analyze(self.path)
        path = self.path / "world-battle.stall.csv"
        rows = list(profile.read_csv(path))
        for row in rows:
            if row["lane"] != "tick":
                row["thread_cpu_ns"] = row["cpu_cycles"] = ""
        with path.open("w", newline="") as out:
            writer = csv.DictWriter(out, list(rows[0])); writer.writeheader(); writer.writerows(rows)
        after = profile.analyze(self.path)
        tick = after["stall_trace"]["ticks"][0]
        self.assertIsNotNone(tick["thread_cpu_ms"])
        self.assertTrue(all(p["thread_cpu_ms"] is None for p in tick["phases"]))
        self.assertIn("different:stall_trace:cpu_sampling", profile.compare(legacy, after)["reasons"])

    def test_trigger_thresholds_are_explicit_pair_condition(self):
        self.write_stall_capture(threshold=50)
        before = profile.analyze(self.path)
        self.write_stall_capture(threshold=100)
        after = profile.analyze(self.path)
        self.assertIn("different:CoreTriggerMilliseconds", profile.compare(before, after)["reasons"])

    def frame_capture(self, source="gap"):
        self.write_stall_capture()
        frames = [dict(r) for r in self.frames]
        if source == "gap":
            frames[1].update(frame_start_ns="200000000", frame_end_ns="200000100", frame_gap_ns="199998800")
            begin, end, tick = 1200, 200000000, 1
        else:
            frames[1].update(frame_start_ns="2000000", frame_end_ns="202000000")
            begin, end, tick = 2000000, 202000000, 1
        with (self.path / "world-battle.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(frames[0])); writer.writeheader(); writer.writerows(frames)
        log = self.path / "world-battle.log"
        interval_signal = ("PROFILE stall_frame_signal source=interval generation=0 tick=1 begin_ns=900 end_ns=200000000 window=1\n" if source == "gap" else "")
        count = 2 if source == "gap" else 1
        log.write_text(log.read_text().replace("stall_capture_schema=1", "stall_capture_schema=2").replace("stall_windows=0", "stall_windows=1") +
            f"PROFILE stall_frame_signals={count} stall_retained_frame_signals={count} stall_dropped_frame_signals=0 stall_unarmed_frame_signals=0\n" + interval_signal +
            f"PROFILE stall_frame_signal source={source} generation=0 tick={tick} begin_ns={begin} end_ns={end} window=1\n")

    def test_frame_trigger_catches_gap_with_fast_core_and_never_blames_the_gap_on_core(self):
        self.frame_capture()
        result = profile.analyze(self.path)
        self.assertTrue(result["valid_for_comparison"])
        self.assertFalse(result["acceptance_eligible"])
        capture = result["stall_trace"]
        self.assertEqual(capture["windows"], 1)
        self.assertEqual(capture["observed_slow_ticks"], 0)
        self.assertEqual(capture["frame_signals"][0]["frame_row"], 1)
        coverage = result["stall_episodes"]["retained"][0]["diagnostic_coverage"]
        self.assertEqual(coverage["tick_phases"], "complete")
        self.assertGreater(coverage["outside_update_ms"], 199)
        self.assertFalse(coverage["worker_scopes_available"])
        self.assertFalse(coverage["cause_identified"])
        self.assertIn("Frame triggers: 2 signals", profile.markdown(result))

    def test_frame_trigger_without_any_simulation_step_is_still_valid(self):
        self.frame_capture("update")
        result = profile.analyze(self.path)
        coverage = result["stall_episodes"]["retained"][0]["diagnostic_coverage"]
        self.assertEqual(coverage["tick_phases"], "no_core_ticks")
        self.assertEqual(coverage["non_core_slice_ms"], 200)
        self.assertFalse(result["acceptance_eligible"])

    def test_frame_signal_metadata_clock_identity_and_counts_are_validated(self):
        for old, new, message in [
            ("begin_ns=1200", "begin_ns=1201", "clock/identity"),
            ("source=gap generation=0", "source=gap generation=1", "clock/identity"),
            ("window=1", "window=2", "clock/identity"),
            ("stall_frame_signals=2", "stall_frame_signals=3", "budget/count"),
            ("stall_retained_frame_signals=2", "stall_retained_frame_signals=0", "budget/count"),
        ]:
            with self.subTest(new=new):
                self.frame_capture()
                log = self.path / "world-battle.log"
                log.write_text(log.read_text().replace(old, new))
                with self.assertRaisesRegex(ValueError, message):
                    profile.analyze(self.path)
        self.frame_capture()
        log = self.path / "world-battle.log"
        log.write_text("\n".join(l for l in log.read_text().splitlines() if not l.startswith("PROFILE stall_frame_signals=")))
        with self.assertRaisesRegex(ValueError, "summary missing"):
            profile.analyze(self.path)

    def test_combined_interval_triggers_when_slice_and_gap_are_both_below_threshold(self):
        self.write_stall_capture()
        frames = [dict(r) for r in self.frames]
        frames[0]["frame_end_ns"] = "30000900"
        frames[1].update(frame_start_ns="60000900", frame_end_ns="60001000", frame_gap_ns="30000000")
        with (self.path / "world-battle.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(frames[0])); writer.writeheader(); writer.writerows(frames)
        log = self.path / "world-battle.log"
        log.write_text(log.read_text().replace("stall_capture_schema=1", "stall_capture_schema=2").replace("stall_windows=0", "stall_windows=1") +
            "PROFILE stall_frame_signals=1 stall_retained_frame_signals=1 stall_dropped_frame_signals=0 stall_unarmed_frame_signals=0\n" +
            "PROFILE stall_frame_signal source=interval generation=0 tick=1 begin_ns=900 end_ns=60000900 window=1\n")
        result = profile.analyze(self.path)
        episode = result["stall_episodes"]["retained"][0]
        self.assertEqual(episode["covered_wall_ms"], 60)
        self.assertEqual(episode["measured_wall_ms"], 30)
        self.assertEqual(episode["following_gap_ms"], 30)
        self.assertEqual(result["stall_trace"]["frame_signals"][0]["source"], "interval")

    def test_incident_coverage_does_not_use_quiet_tail_as_evidence_for_earlier_stall(self):
        frames = self.ordinary_stall_frames()
        _, _, episodes = profile.summarize_slices(frames, True, 12)
        quiet_tail = dict(ticks=[dict(frame_row=3, wall_ms=0.1)])
        profile.incident_coverage(episodes, frames, [0] * 4, [0.0] * 4, [0] * 4, quiet_tail)
        self.assertTrue(all(e["diagnostic_coverage"]["tick_phases"] == "none" for e in episodes["retained"]))
        self.assertEqual(episodes["retained"][0]["diagnostic_coverage"]["untraced_steps"], 1)

    def test_partial_multi_step_coverage_and_ordinary_missing_trace_are_explicit(self):
        frames = self.ordinary_stall_frames()
        ordinary = self.ordinary_report(frames)
        self.assertTrue(all(e["diagnostic_coverage"]["tick_phases"] == "none" for e in ordinary["stall_episodes"]["retained"]))
        _, _, episodes = profile.summarize_slices(frames, True, 12)
        profile.incident_coverage(episodes, frames, [0, 0, 1, 0], [0, 0, 25, 0], [0, 0, 2, 0], None)
        coverage = episodes["retained"][1]["diagnostic_coverage"]
        self.assertEqual(coverage["tick_phases"], "partial")
        self.assertEqual(coverage["untraced_steps"], 1)
        self.assertEqual(coverage["captured_tick_wall_ms"], 25)
        self.assertTrue(coverage["worker_scopes_available"])

    def test_old_and_frame_trigger_contracts_cannot_be_speed_pairs(self):
        self.write_stall_capture()
        before = profile.analyze(self.path)
        self.frame_capture()
        after = profile.analyze(self.path)
        self.assertIn("different:stall_trace:schema", profile.compare(before, after)["reasons"])

    def test_zero_elapsed_first_slice_still_owns_its_trace_and_frame_signal(self):
        self.frame_capture()
        path = self.path / "world-battle.csv"
        frames = list(profile.read_csv(path)); frames[0]["seconds"] = "0.0"
        with path.open("w", newline="") as out:
            writer = csv.DictWriter(out, list(frames[0])); writer.writeheader(); writer.writerows(frames)
        result = profile.analyze(self.path)
        self.assertTrue(result["valid_for_comparison"])
        self.assertEqual(result["stall_trace"]["ticks"][0]["frame_row"], 0)

    def late_peak_capture(self):
        self.write_stall_capture(rows=[dict(r, generation="4", profile_offset_ns=str(880000000 + int(r['profile_offset_ns']))) for r in trace() if r['lane'] != 'task'])
        frames = [dict(self.frames[0], generation=str(i), tick="1", frame_start_ns=str(i * 220000000),
                       frame_end_ns=str(i * 220000000 + 200000000), frame_gap_ns="20000000" if i else "0") for i in range(5)]
        with (self.path / 'world-battle.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, list(frames[0])); writer.writeheader(); writer.writerows(frames)
        log = self.path / 'world-battle.log'
        lines = [l for l in log.read_text().splitlines() if not l.startswith('PROFILE stall_')]
        lines += ['PROFILE stall_capture_schema=3 stall_capture_mode=phase_ring threshold_ms=50 pre_ticks=16 post_ticks=16 max_windows=4',
                  'PROFILE stall_windows=4 stall_slow_ticks=0 stall_unarmed_slow_ticks=0 stall_retained_ticks=1',
                  'PROFILE stall_frame_signals=9 stall_retained_frame_signals=9 stall_dropped_frame_signals=0 stall_unarmed_frame_signals=1']
        for i in range(5):
            if i:
                lines.append(f'PROFILE stall_frame_signal source=interval generation={i-1} tick=1 begin_ns={(i-1)*220000000} end_ns={i*220000000} window={i}')
            lines.append(f'PROFILE stall_frame_signal source=update generation={i} tick=1 begin_ns={i*220000000} end_ns={i*220000000+200000000} window={i+1 if i<4 else 0}')
        lines.append('PROFILE stall_peak_signal source=update generation=4 tick=1 begin_ns=880000000 end_ns=1080000000 window=0 context_ticks=16')
        log.write_text('\n'.join(lines) + '\n')

    def test_late_peak_is_separate_from_event_record_limit_and_keeps_phase_evidence(self):
        self.late_peak_capture()
        result = profile.analyze(self.path)
        peak = result['stall_trace']['late_peak']
        self.assertEqual(peak['frame_row'], 4)
        self.assertEqual(peak['wall_ms'], 200)
        self.assertEqual(result['stall_trace']['ticks'][0]['frame_row'], 4)
        self.assertFalse(result['acceptance_eligible'])
        self.assertIn('Late peak reserve', profile.markdown(result))

    def test_late_peak_missing_duplicate_invalid_clocks_and_wrong_budget_are_rejected(self):
        for old, new, error in [('PROFILE stall_peak_signal', 'IGNORED stall_peak_signal', 'metadata'),
                                ('window=0 context_ticks=16', 'window=1 context_ticks=16', 'contract'),
                                ('context_ticks=16', 'context_ticks=17', 'contract'),
                                ('end_ns=1080000000 window=0 context', 'end_ns=1080000001 window=0 context', 'clock/identity')]:
            with self.subTest(new=new):
                self.late_peak_capture()
                log = self.path / 'world-battle.log'; log.write_text(log.read_text().replace(old, new))
                with self.assertRaisesRegex(ValueError, error): profile.analyze(self.path)
        self.late_peak_capture()
        log = self.path / 'world-battle.log'
        log.write_text(log.read_text() + log.read_text().splitlines()[-1] + '\n')
        with self.assertRaisesRegex(ValueError, 'metadata'): profile.analyze(self.path)

    def test_late_peak_metadata_without_its_actual_tick_context_is_not_evidence(self):
        self.late_peak_capture()
        rows = [r for r in trace() if r['lane'] != 'task']
        with (self.path / 'world-battle.stall.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, list(rows[0])); writer.writeheader(); writer.writerows(rows)
        with self.assertRaisesRegex(ValueError, 'phase context missing'):
            profile.analyze(self.path)

    def test_new_schema_requires_physics_scopes_and_cannot_pair_with_legacy(self):
        self.write()
        legacy = profile.analyze(self.path)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE core_trace_schema=2\n")
        missing = profile.analyze(self.path)
        self.assertIn("physics_task_coverage_missing", missing["reasons"])
        self.assertFalse(missing["valid_for_comparison"])
        self.assertEqual(missing["physics_task_coverage"]["missing_ticks"], 1)
        rows = trace()
        for family in ("physics_rubble_floor", "physics_rubble_columns"):
            rows += [mark("task", family + "_begin", 15, "3", "0"),
                     mark("task", family + "_end", 60, "3", "0")]
        self.write(rows=rows)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE core_trace_schema=2\n")
        complete = profile.analyze(self.path)
        self.assertTrue(complete["valid_for_comparison"])
        self.assertEqual(complete["physics_task_coverage"]["traced_ticks"], 1)
        self.assertEqual(complete["task_families"]["physics_rubble_floor"]["cpu_samples"], 0)
        self.assertIn("unavailable", profile.markdown(complete))
        self.assertIn("different_or_missing:core_trace_schema", profile.compare(legacy, complete)["reasons"])

    def test_schema_three_checks_thermal_and_laser_coverage(self):
        for laser in (False, True):
            base = {"physics_rubble_floor", "physics_rubble_columns", "physics_heat_exchange", "physics_ground_heat"}
            if laser:
                base.update(("physics_laser_beam", "physics_laser_boxes", "physics_laser_halo"))
            for missing in (None, *sorted(base)):
                with self.subTest(laser=laser, missing=missing):
                    rows = trace()
                    if laser:
                        rows.append(mark("phase", "orders_laser", 90, cpu=125))
                    for family in sorted(base - {missing}):
                        stage = "0" if "rubble" in family else "4" if "ground_heat" in family else "3" if "heat_exchange" in family else "2"
                        rows += [mark("task", family + "_begin", 15, "3", stage),
                                 mark("task", family + "_end", 60, "3", stage)]
                    self.write(rows=rows)
                    with (self.path / "world-battle.log").open("a") as out:
                        out.write("PROFILE core_trace_schema=3\n")
                    report = profile.analyze(self.path)
                    self.assertEqual(report["valid_for_comparison"], missing is None)
                    if missing:
                        self.assertIn("physics_task_coverage_missing", report["reasons"])

    def test_schema_three_rejects_thermal_scopes_in_wrong_stage(self):
        rows = trace()
        for family in ("physics_rubble_floor", "physics_rubble_columns", "physics_heat_exchange", "physics_ground_heat"):
            rows += [mark("task", family + "_begin", 15, "3", "0"),
                     mark("task", family + "_end", 60, "3", "0")]
        self.write(rows=rows)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE core_trace_schema=3\n")
        report = profile.analyze(self.path)
        self.assertFalse(report["valid_for_comparison"])
        self.assertIn("physics_task_coverage_missing", report["reasons"])

    def test_unit_heat_readers_and_ordered_admission_are_separate_task_families(self):
        rows = trace()
        for family, stage in (("physics_rubble_floor", "0"), ("physics_rubble_columns", "0"),
                              ("physics_heat_exchange", "3"), ("physics_ground_heat", "4"),
                              ("physics_unit_heat", "5"), ("physics_unit_heat_admit", "5")):
            rows += [mark("task", family + "_begin", 15, "3", stage),
                     mark("task", family + "_end", 60, "3", stage)]
        for thread in ("7", "8"):
            rows += [mark("task", "physics_unit_heat_read_begin", 20, thread, "5"),
                     mark("task", "physics_unit_heat_read_end", 50, thread, "5")]
        self.write(rows=rows)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE core_trace_schema=3\n")
        report = profile.analyze(self.path)
        self.assertTrue(report["valid_for_comparison"])
        self.assertEqual(report["task_families"]["physics_unit_heat_read"]["jobs"], 2)
        self.assertEqual(report["task_families"]["physics_unit_heat_admit"]["jobs"], 1)
        self.assertIn("5 unit heat", profile.markdown(report))

    def bounded_physics_fixture(self):
        rows, summary = trace(), []
        for family in ("physics_rubble_floor", "physics_rubble_columns"):
            rows += [mark("task", family + "_begin", 15, "3", "0"),
                     mark("task", family + "_end", 60, "3", "0")]
            summary.append(dict(generation="0", tick="1", os_thread="3", stage="0", family=family,
                jobs="3", inclusive_wall_ns="60", max_wall_ns="45", cpu_ns="0", cpu_samples="0",
                cycles="0", cycle_samples="0"))
        self.write(rows=rows)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE core_trace_schema=2\nPROFILE physics_task_sampling=longest_per_thread_family_stage\n")
        return summary

    def write_physics(self, rows):
        with (self.path / "world-battle.physics.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(rows[0]))
            writer.writeheader(); writer.writerows(rows)

    def test_bounded_physics_uses_complete_totals_and_keeps_null_counters(self):
        self.write_physics(self.bounded_physics_fixture())
        report = profile.analyze(self.path)
        self.assertTrue(report["valid_for_comparison"])
        self.assertEqual(report["task_families"]["physics_rubble_floor"]["jobs"], 3)
        self.assertEqual(report["task_families"]["physics_rubble_floor"]["cpu_samples"], 0)
        self.assertAlmostEqual(report["task_families"]["physics_rubble_floor"]["inclusive_wall_ms"], 60 / 1e6)
        self.assertIn("physics stage 0", profile.markdown(report))
        totals = report['physics_thread_totals']
        self.assertEqual(len(totals), 2)
        self.assertTrue(all(r['jobs'] == 3 and r['os_thread'] == '3' and r['stage'] == '0' for r in totals))
        self.assertTrue(all(r['cpu_samples'] == 0 and r['cpu_ns'] == 0 for r in totals))
        self.assertEqual(report['slow_ticks'][0]['physics_thread_totals'], totals)
        self.assertIn('Physics per-thread complete summary totals', profile.markdown(report))

    def test_native_details_are_aggregated_and_retained_in_plain_report(self):
        rows = trace() + [mark('detail','motion_begin',5), mark('detail','motion_routes',75)]
        self.write(rows=rows)
        path = self.path / 'world-battle.manifest.json'
        manifest = json.loads(path.read_text())
        manifest['CorePhases'] = True
        path.write_text(json.dumps(manifest))
        report = profile.analyze(self.path)
        self.assertEqual([d['name'] for d in report['details']], ['motion_routes', 'motion_unmarked_tail'])
        self.assertAlmostEqual(report['details'][0]['mean_per_tick_ms'], 70 / 1e6)
        self.assertEqual(report['slow_ticks'][0]['details'][0]['scope_coverage']['observed_threads'], 1)
        markdown = profile.markdown(report)
        self.assertIn('Nested caller details', markdown)
        self.assertIn('detail motion/motion_routes', markdown)
        self.assertEqual({r['family'] for r in report['actor_thread_totals']}, {'parent','child'})
        self.assertIn('Actor/navigation per-thread recorded totals', markdown)
        self.assertFalse(report['acceptance_eligible'])

    def test_complete_thread_totals_preserve_threads_instead_of_longest_samples(self):
        summary = self.bounded_physics_fixture()
        path = self.path / 'world-battle.core.csv'
        rows = list(profile.read_csv(path)) + [
            mark('task','physics_rubble_floor_begin',20,'4','0'),
            mark('task','physics_rubble_floor_end',55,'4','0')]
        with path.open('w',newline='') as out:
            writer = csv.DictWriter(out,list(rows[0])); writer.writeheader(); writer.writerows(rows)
        summary.append(dict(generation='0',tick='1',os_thread='4',stage='0',family='physics_rubble_floor',
                            jobs='2',inclusive_wall_ns='50',max_wall_ns='35',cpu_ns='10',cpu_samples='1',cycles='0',cycle_samples='0'))
        self.write_physics(summary)
        report = profile.analyze(self.path)
        totals = [r for r in report['physics_thread_totals'] if r['family'] == 'physics_rubble_floor']
        self.assertEqual({r['os_thread']:r['jobs'] for r in totals}, {'3':3,'4':2})
        self.assertEqual(sum(r['inclusive_wall_ns'] for r in totals),110)
        self.assertEqual(sum(r['cpu_ns'] for r in totals),10)
        self.assertEqual(report['task_families']['physics_rubble_floor']['jobs'],5)

    def test_missing_physics_totals_do_not_pass(self):
        self.bounded_physics_fixture()
        report = profile.analyze(self.path)
        self.assertFalse(report["valid_for_comparison"])
        self.assertIn("physics_task_summary_missing", report["reasons"])

    def test_corrupt_physics_totals_are_rejected(self):
        for field, value in (("jobs", "0"), ("tick", "2"), ("stage", "1"), ("max_wall_ns", "44"),
                             ("inclusive_wall_ns", "44"), ("cpu_samples", "4"), ("cpu_ns", "1")):
            with self.subTest(field=field):
                rows = self.bounded_physics_fixture()
                rows[0][field] = value
                self.write_physics(rows)
                with self.assertRaisesRegex(ValueError, "[Pp]hysics summary"):
                    profile.analyze(self.path)

    def test_duplicate_missing_and_extra_summary_rows_are_rejected(self):
        for variant in ("duplicate", "missing", "extra"):
            with self.subTest(variant=variant):
                rows = self.bounded_physics_fixture()
                if variant == "duplicate": rows.append(rows[0])
                elif variant == "missing": rows = rows[:1]
                else: rows += [dict(row, tick="2") for row in rows]
                self.write_physics(rows)
                with self.assertRaisesRegex(ValueError, "[Pp]hysics summary"):
                    profile.analyze(self.path)

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

    def test_render_conditions_cannot_hide_behind_same_gpu_and_final_hash(self):
        self.write()
        before = profile.analyze(self.path)
        log = self.path / 'world-battle.log'
        for old, new in [('[1600, 900]', '[960, 540]'), ('AutoNoVsync', 'AutoVsync'),
                         ('1.0, 0.0, 0.0]', '2.0, 0.0, 0.0]')]:
            with self.subTest(change=new):
                self.write()
                log.write_text(log.read_text().replace(old, new))
                after = profile.analyze(self.path)
                self.assertTrue(after['valid_for_comparison'])
                self.assertEqual(before['state_hash'], after['state_hash'])
                self.assertIn('different_or_missing:actual_render_context', profile.compare(before, after)['reasons'])

    def test_transient_render_change_and_missing_metadata_are_invalid(self):
        self.write()
        log = self.path / 'world-battle.log'
        log.write_text(log.read_text().replace('render_context_changed=false', 'render_context_changed=true'))
        result = profile.analyze(self.path)
        self.assertFalse(result['valid_for_comparison'])
        self.assertIn('render_context_changed_during_run', result['reasons'])
        self.write()
        log.write_text('\n'.join(line for line in log.read_text().splitlines() if 'render_context' not in line))
        self.assertIn('render_context_missing', profile.analyze(self.path)['reasons'])

    def test_stable_focus_is_an_explicit_pair_condition(self):
        self.write()
        before = profile.analyze(self.path)
        log = self.path / 'world-battle.log'
        log.write_text(log.read_text().replace('focused: true', 'focused: false'))
        after = profile.analyze(self.path)
        self.assertTrue(after['valid_for_comparison'])
        self.assertIn('different_or_missing:actual_render_context', profile.compare(before, after)['reasons'])

    def test_losing_and_restoring_focus_is_invalid_even_with_a_false_change_flag(self):
        self.write()
        log = self.path / 'world-battle.log'
        with log.open('a') as out:
            out.write(f'PROFILE render_context_step=3 value={CONTEXT.replace("focused: true", "focused: false")}\n')
            out.write(f'PROFILE render_context_step=4 value={CONTEXT}\n')
        result = profile.analyze(self.path)
        self.assertFalse(result['valid_for_comparison'])
        self.assertIn('render_focus_changed_during_run', result['reasons'])

    def test_legacy_context_and_missing_focus_still_show_timings_but_never_qualify(self):
        for old, new, reason in [('render_context_schema=2', 'render_context_schema=1', 'render_context_missing'),
                                 ('focused: true, ', '', 'render_focus_missing')]:
            with self.subTest(reason=reason):
                self.write()
                log = self.path / 'world-battle.log'
                log.write_text(log.read_text().replace(old, new))
                result = profile.analyze(self.path)
                self.assertFalse(result['valid_for_comparison'])
                self.assertIn(reason, result['reasons'])
                self.assertIsNotNone(result['frame_ms'])

    def test_scripted_context_sequence_is_compared_without_frame_timestamps(self):
        self.write()
        log = self.path / 'world-battle.log'
        with log.open('a') as out:
            out.write(f'PROFILE render_context_step=3 value={CONTEXT.replace("zone: 0", "zone: 7")}\n')
        before = profile.analyze(self.path)
        self.assertTrue(before['valid_for_comparison'])
        self.assertTrue(profile.compare(before, profile.analyze(self.path))['eligible'])
        log.write_text(log.read_text().replace('render_context_step=3', 'render_context_step=4'))
        self.assertFalse(profile.compare(before, profile.analyze(self.path))['eligible'])

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

    def test_os_pressure_capture_is_diagnostic_and_requires_actual_samples(self):
        self.write()
        manifest_path = self.path / 'world-battle.manifest.json'
        manifest = json.loads(manifest_path.read_text()); manifest['OsPressure'] = True
        manifest_path.write_text(json.dumps(manifest))
        result = profile.analyze(self.path)
        self.assertFalse(result['acceptance_eligible'])
        self.assertIn('os_pressure_artifact_missing', result['reasons'])

    def test_os_pressure_window_ignores_startup_and_dump_samples(self):
        folder = self.path / 'world-battle-cpu'; folder.mkdir()
        a, b = OsPressureTests().rows()
        samples = [dict(a if i < 3 else b, Seconds=str(i), Utc=f'1970-01-01T00:00:0{i}Z', CpuMs=str(i * 10)) for i in range(1, 5)]
        with (folder / 'cpu-process-samples.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, samples[0]); writer.writeheader(); writer.writerows(samples)
        result = profile.os_window(self.path, 'world-battle', 'epoch_unix_ns=2000000000',
                                   [dict(frame_end_ns='1000000000')], {})
        self.assertEqual(len(result['pressure']['intervals']), 1)
        self.assertEqual(result['pressure']['intervals'][0]['page_fault_delta'], 11)
        self.assertEqual(result['pressure']['intervals'][0]['begin_ns'], 0)
        self.assertEqual(result['pressure']['intervals'][0]['end_ns'], 1_000_000_000)

    def test_observed_pressure_requires_declared_mode_and_never_accepts_fps(self):
        frame = dict(seconds='1', tick='30', generation='0', frame_ms='1000', simulation_ticks='30',
            simulation_core_ms='2', frame_start_ns='0', frame_end_ns='1000000000', frame_gap_ns='0')
        self.ordinary_report([frame])
        with (self.path / 'world-battle.log').open('a') as out:
            out.write('PROFILE epoch_unix_ns=2000000000\n')
        folder = self.path / 'world-battle-cpu'; folder.mkdir()
        a, b = OsPressureTests().rows()
        samples = [dict(r, Seconds=str(i), Utc=f'1970-01-01T00:00:0{i}Z', CpuMs=str(i * 10))
                   for i, r in [(2, a), (3, b)]]
        with (folder / 'cpu-process-samples.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, samples[0]); writer.writeheader(); writer.writerows(samples)
        result = profile.analyze(self.path)
        self.assertIn('os_pressure_mode_mismatch', result['reasons'])
        path = self.path / 'world-battle.manifest.json'
        manifest = json.loads(path.read_text()); manifest['OsPressure'] = True
        path.write_text(json.dumps(manifest))
        result = profile.analyze(self.path)
        self.assertTrue(result['valid_for_comparison'])
        self.assertTrue(result['meets_simulation_rate'])
        self.assertFalse(result['acceptance_eligible'])
        self.assertFalse(profile.compare(result, result)['fps_acceptance_eligible'])
        self.assertIn('not stall attribution', profile.markdown(result))

    def test_os_process_deltas_join_only_accepted_intervals_and_reject_bad_values(self):
        folder = self.path / 'world-battle-cpu'; folder.mkdir()
        a, b = OsPressureTests().rows()
        samples = [dict(r, Seconds=str(i), Utc=f'1970-01-01T00:00:0{i}Z', CpuMs=str(i * 10))
                   for i, r in [(2, a), (3, b)]]
        with (folder / 'cpu-process-samples.csv').open('w', newline='') as out:
            writer = csv.DictWriter(out, samples[0]); writer.writeheader(); writer.writerows(samples)
        process = dict(Seconds='3', ProcessId='12', Name='example', StartedUtc='lifetime-1',
                       CpuMs='250', IsTarget='false', IsSampler='false')
        def joined(rows):
            with (folder / 'cpu-os-processes.csv').open('w', newline='') as out:
                writer = csv.DictWriter(out, process); writer.writeheader(); writer.writerows(rows)
            return profile.os_window(self.path, 'world-battle', 'epoch_unix_ns=2000000000',
                                     [dict(frame_end_ns='1000000000')], {})
        result = joined([dict(process, Seconds='4', CpuMs='nan'), process])
        self.assertEqual(result['pressure']['intervals'][0]['top_processes'][0]['cpu_ms'], 250)
        self.assertEqual(result['pressure']['intervals'][0]['top_processes'][0]['started_utc'], 'lifetime-1')
        for changes in [dict(CpuMs='nan'), dict(CpuMs='-1'), dict(IsTarget='unknown')]:
            with self.assertRaisesRegex(ValueError, 'OS process pressure'):
                joined([dict(process, **changes)])


    def render_fixture(self, spans=None, **metadata):
        self.write()
        values = [("pipeline_handoff", 1300, 2000, "1"), ("extract_callback", 1400, 1600, "1"),
                  ("render_schedule", 1700, 3000, "2"), ("render_prepare", 1800, 1900, "2"),
                  ("render_system", 2000, 2900, "2"), ("render_graph", 2100, 2600, "2"),
                  ("submit_present_tail", 2600, 2800, "2")]
        if spans is None:
            spans = [dict(frame_row="0", generation="0", tick="1", stage=stage,
                          begin_ns=str(a), end_ns=str(b), begin_thread=f"ThreadId({thread})",
                          end_thread=f"ThreadId({thread})", begin_os_thread="", end_os_thread="",
                          begin_cpu_ns="", end_cpu_ns="", begin_cycles="", end_cycles="")
                     for stage, a, b, thread in values]
        with (self.path / "world-battle.render.csv").open("w", newline="") as out:
            writer = csv.DictWriter(out, list(spans[0])); writer.writeheader(); writer.writerows(spans)
        fields = dict(render_timeline_schema=1, render_span_limit=100000, render_spans_started=len(spans),
                      render_spans_retained=len(spans), render_spans_dropped=0,
                      render_spans_incomplete=sum(not r["end_ns"] for r in spans), render_cutoff_ns=2000101,
                      render_pipelined="true")
        fields.update(metadata)
        with (self.path / "world-battle.log").open("a") as out:
            out.write("PROFILE " + " ".join(f"{k}={v}" for k, v in fields.items()) + "\n")
        path = self.path / "world-battle.manifest.json"
        manifest = json.loads(path.read_text()); manifest["RenderTimeline"] = True
        if fields["render_timeline_schema"] == 4:
            manifest["RenderSpanLimit"] = fields["render_span_limit"]
        path.write_text(json.dumps(manifest))
        return spans

    def test_render_source_identity_and_async_overlap_are_not_double_counted(self):
        self.render_fixture()
        result = profile.analyze(self.path)
        self.assertTrue(result["valid_for_comparison"])
        self.assertFalse(result["acceptance_eligible"])
        timeline = result["render_timeline"]
        self.assertEqual(timeline["complete_source_rows"], 1)
        self.assertEqual(timeline["missing_source_rows"], 1)
        gap = timeline["slow_gaps"][0]
        self.assertAlmostEqual(gap["gap_ms"] - gap["unobserved_or_other_ms"], .0017)
        self.assertEqual(timeline["slow_render_frames"][0]["source_row"], 0)
        self.assertIn("Render CPU timeline", profile.markdown(result))
        ordinary = dict(result, manifest=dict(result["manifest"], RenderTimeline=False))
        self.assertIn("different:RenderTimeline", profile.compare(result, ordinary)["reasons"])

    def test_render_timeline_rejects_reset_identity_and_invalid_clocks(self):
        for field, value in (("frame_row", "1"), ("generation", "1"), ("tick", "2"),
                             ("begin_ns", "1000"), ("end_ns", "2000102"), ("end_thread", "ThreadId(4)")):
            with self.subTest(field=field):
                spans = self.render_fixture(); spans[1][field] = value
                self.render_fixture(spans)
                with self.assertRaisesRegex(ValueError, "Render timeline"):
                    profile.analyze(self.path)

    def test_render_timeline_partial_terminal_spans_stay_explicit(self):
        spans = self.render_fixture()
        for row in spans:
            if row["stage"] in ("render_system", "render_schedule"):
                row["end_ns"] = ""; row["end_thread"] = ""
        self.render_fixture(spans)
        result = profile.analyze(self.path)["render_timeline"]
        self.assertEqual(result["incomplete"], 2)
        self.assertEqual(result["complete_source_rows"], 0)
        self.assertNotIn("render_system", result["stage_ms"])

    def test_render_timeline_missing_or_undeclared_artifacts_cannot_pass(self):
        self.render_fixture(); (self.path / "world-battle.render.csv").unlink()
        result = profile.analyze(self.path)
        self.assertIn("render_timeline_artifact_or_metadata_missing", result["reasons"])
        self.assertFalse(result["acceptance_eligible"])
        self.render_fixture()
        path = self.path / "world-battle.manifest.json"
        manifest = json.loads(path.read_text()); del manifest["RenderTimeline"]
        path.write_text(json.dumps(manifest))
        self.assertIn("render_timeline_mode_mismatch", profile.analyze(self.path)["reasons"])

    def test_render_timeline_budget_counts_duplicates_and_nesting_rejected(self):
        for field, value in (("render_spans_retained", 8), ("render_spans_started", 9),
                             ("render_spans_incomplete", 1), ("render_span_limit", 99999),
                             ("render_timeline_schema", 2), ("render_cutoff_ns", 2000099)):
            with self.subTest(field=field):
                self.render_fixture(**{field: value})
                with self.assertRaisesRegex(ValueError, "Render timeline"):
                    profile.analyze(self.path)
        spans = self.render_fixture(); self.render_fixture(spans + [spans[-1]])
        with self.assertRaisesRegex(ValueError, "duplicate"):
            profile.analyze(self.path)
        spans = self.render_fixture(); spans[-1]["end_ns"] = "2999"
        self.render_fixture(spans)
        with self.assertRaisesRegex(ValueError, "nested"):
            profile.analyze(self.path)

    def test_render_timeline_explicit_drops_do_not_invent_coverage(self):
        spans = self.render_fixture()[:3]
        self.render_fixture(spans, render_span_limit=3, render_spans_started=20, render_spans_dropped=17)
        with patch.object(profile, "RENDER_SPAN_LIMIT", 3):
            result = profile.analyze(self.path)["render_timeline"]
        self.assertEqual(result["dropped"], 17)
        self.assertEqual(result["complete_source_rows"], 0)

    def test_render_timeline_nonpipelined_mode_has_no_handoff(self):
        spans = self.render_fixture()[1:]
        self.render_fixture(spans, render_pipelined="false")
        result = profile.analyze(self.path)["render_timeline"]
        self.assertFalse(result["pipelined"])
        self.assertEqual(result["complete_source_rows"], 1)

    def test_duration_budget_is_recorded_and_a_separate_contract(self):
        rows = self.gas_fixture(schema=3)
        before = profile.analyze(self.path)
        self.render_fixture(rows, render_timeline_schema=4, render_gas_details="true",
                            render_span_limit=629200, render_span_bytes=256)
        result = profile.analyze(self.path)
        timeline = result["render_timeline"]
        self.assertTrue(result["valid_for_comparison"])
        self.assertTrue(timeline["capture_not_truncated"])
        self.assertEqual(timeline["reserved_span_bytes"], 629200 * 256)
        self.assertEqual(timeline["last_source_row"], 0)
        self.assertIn("Capture not truncated: True", profile.markdown(result))
        self.assertIn("different:render_timeline:span_limit", profile.compare(before, result)["reasons"])

    def test_new_budget_overflow_retains_evidence_but_cannot_pass_comparison(self):
        rows = self.gas_fixture(schema=3)[:3]
        self.render_fixture(rows, render_timeline_schema=4, render_gas_details="true",
                            render_span_limit=3, render_span_bytes=256,
                            render_spans_started=20, render_spans_dropped=17)
        result = profile.analyze(self.path)
        self.assertFalse(result["valid_for_comparison"])
        self.assertIn("render_timeline_truncated", result["reasons"])
        self.assertFalse(result["render_timeline"]["capture_not_truncated"])
        self.assertEqual(result["render_timeline"]["dropped"], 17)
        self.assertFalse(result["render_timeline"]["slow_other_prepare"])

    def test_duration_budget_rejects_bad_sizes_and_manifest_mismatch(self):
        rows = self.gas_fixture(schema=3)
        good = dict(render_timeline_schema=4, render_gas_details="true",
                    render_span_limit=629200, render_span_bytes=256)
        for key, value in (("render_span_limit", 0), ("render_span_limit", 1000001),
                           ("render_span_bytes", 0), ("render_span_bytes", 1025)):
            with self.subTest(key=key, value=value):
                self.render_fixture(rows, **dict(good, **{key: value}))
                with self.assertRaisesRegex(ValueError, "Render timeline"):
                    profile.analyze(self.path)
        self.render_fixture(rows, **dict(good, render_span_bytes=""))
        self.assertIn("render_timeline_artifact_or_metadata_missing", profile.analyze(self.path)["reasons"])
        self.render_fixture(rows, **good)
        path = self.path / "world-battle.manifest.json"
        manifest = json.loads(path.read_text()); manifest["RenderSpanLimit"] = 100000
        path.write_text(json.dumps(manifest))
        result = profile.analyze(self.path)
        self.assertIn("render_timeline_budget_manifest_mismatch", result["reasons"])
        self.assertFalse(result["valid_for_comparison"])

    def test_prepare_residual_ranking_survives_schedule_only_top_limit(self):
        first = self.gas_fixture(schema=3)
        second = [dict(row, frame_row="1", begin_ns=str(int(row["begin_ns"]) + 2000000),
                       end_ns=str(int(row["end_ns"]) + 2000000),
                       input_frame_row="1" if row["input_frame_row"] else "") for row in first]
        # Frame 0 has the slower graph/schedule; frame 1 has larger non-gas prepare.
        next(row for row in second if row["stage"] == "render_schedule")["end_ns"] = "2002990"
        next(row for row in second if row["stage"] == "render_prepare")["begin_ns"] = "2001710"
        self.render_fixture(sorted(first + second, key=lambda row: int(row["begin_ns"])),
                            render_timeline_schema=4, render_gas_details="true", render_span_limit=629200,
                            render_span_bytes=256, render_cutoff_ns=2005000)
        result = profile.analyze(self.path, top=1)
        self.assertEqual(result["render_timeline"]["slow_render_frames"][0]["source_row"], 0)
        self.assertEqual(result["render_timeline"]["slow_other_prepare"][0]["source_row"], 1)
        self.assertIn("Prepare outside gas ranked separately", profile.markdown(result))

    def test_render_overlap_index_matches_exhaustive_intersections(self):
        spans = profile.merged_intervals([(5, 10), (8, 20), (25, 30), (40, 50), (45, 48)])
        index = profile.interval_index(spans)
        for a in range(0, 55):
            for b in range(a, 56):
                expected = sum(max(0, min(y, b) - max(x, a)) for x, y in spans)
                self.assertEqual(profile.overlap_ns(index, a, b), expected)


    def test_render_completed_schedule_cannot_hide_missing_stages(self):
        spans = [r for r in self.render_fixture() if r["stage"] != "render_graph"]
        self.render_fixture(spans)
        with self.assertRaisesRegex(ValueError, "coverage missing"):
            profile.analyze(self.path)


    def test_render_driver_cpu_preserves_null_zero_and_rounding(self):
        spans = self.render_fixture()
        spans[0].update(begin_os_thread="7", end_os_thread="7", begin_cpu_ns="0", end_cpu_ns="1000",
                        begin_cycles="500", end_cycles="900")
        spans[1].update(begin_os_thread="7", end_os_thread="7", begin_cpu_ns="1000", end_cpu_ns="1000")
        self.render_fixture(spans)
        result = profile.analyze(self.path)["render_timeline"]
        self.assertAlmostEqual(result["stage_driver_cpu_ms"]["pipeline_handoff"]["mean"], .001)
        self.assertEqual(result["stage_driver_cpu_ms"]["extract_callback"]["mean"], 0)
        self.assertIsNone(result["stage_driver_cpu_ms"]["render_system"])
        self.assertEqual(result["stage_driver_cycles"]["pipeline_handoff"]["mean"], 400)

    def test_render_counter_corruption_cannot_pass(self):
        for changes in (dict(begin_cpu_ns="1"), dict(begin_os_thread="7", end_os_thread="8"),
                        dict(begin_os_thread="7", end_os_thread="7", begin_cpu_ns="2", end_cpu_ns="1"),
                        dict(begin_os_thread="7", end_os_thread="7", begin_cycles="-1")):
            with self.subTest(changes=changes):
                spans = self.render_fixture(); spans[0].update(changes); self.render_fixture(spans)
                with self.assertRaisesRegex(ValueError, "Render timeline"):
                    profile.analyze(self.path)


    def test_render_drops_cannot_hide_missing_records_before_capacity(self):
        self.render_fixture(render_spans_started=20, render_spans_dropped=13)
        with self.assertRaisesRegex(ValueError, "budget/count"):
            profile.analyze(self.path)


    def gas_fixture(self, prepare_complete=True, schema=2):
        spans = self.render_fixture()
        for row in spans:
            row.update(completed="", input_frame_row="", **{key: "" for key in profile.GAS_WORK_FIELDS})
            if schema >= 3:
                row.update({key: "" for key in profile.GAS_EXTRACT_FIELDS})
        leafs = [("gas_extract", 1450, 1500, "3", "extract_callback"),
                 ("gas_prepare", 1810, 1890, "4", "render_prepare")]
        if prepare_complete:
            leafs += [("gas_views_uniform", 1820, 1830, "4", "gas_prepare"),
                      ("gas_terrain_upload", 1830, 1850, "4", "gas_prepare"),
                      ("gas_bricks", 1850, 1860, "4", "gas_prepare"),
                      ("gas_regions", 1860, 1880, "4", "gas_prepare")]
        for stage, a, b, thread, parent in leafs:
            row = dict(spans[0], stage=stage, begin_ns=str(a), end_ns=str(b),
                       begin_thread=f"ThreadId({thread})", end_thread=f"ThreadId({thread})",
                       completed="1" if stage != "gas_prepare" or prepare_complete else "0", input_frame_row="0",
                       **{key: "0" for key in profile.GAS_WORK_FIELDS})
            if stage in ("gas_prepare", "gas_terrain_upload"):
                row.update(terrain_writes="2", terrain_bytes="128", terrain_submits="1")
            if schema >= 3:
                row.update({key: "0" for key in profile.GAS_EXTRACT_FIELDS})
                if stage == "gas_extract":
                    row.update(dirty_ranges="100", range_ns="20", pack_ns="10", clone_ns="5")
            spans.append(row)
        spans.sort(key=lambda r: int(r["begin_ns"]))
        self.render_fixture(spans, render_timeline_schema=schema, render_gas_details="true")
        return spans

    def test_actual_extract_phases_and_raw_ranges_are_preserved(self):
        self.gas_fixture(schema=3)
        report = profile.analyze(self.path)
        work = report["render_timeline"]["slow_gas_calls"]["gas_extract"][0]["leaf_work"]["gas_extract"]["work"]
        self.assertEqual(work["dirty_ranges"], 100)
        self.assertEqual(work["range_ns"], 20)
        self.assertIn("Extraction phases", profile.markdown(report))
        for stage,field,value in [("gas_extract","range_ns","100"),("gas_extract","dirty_ranges","-1"),
                                  ("gas_regions","range_ns","1"),("gas_extract","clone_ns","")]:
            with self.subTest(stage=stage,field=field):
                rows=self.gas_fixture(schema=3)
                next(r for r in rows if r["stage"]==stage)[field]=value
                self.render_fixture(rows,render_timeline_schema=3,render_gas_details="true")
                with self.assertRaisesRegex(ValueError,"Render timeline"):
                    profile.analyze(self.path)

    def task_fixture(self, rows=None, **metadata):
        self.gas_fixture(schema=3)
        if rows is None:
            rows = [dict(frame_row="0", generation="0", tick="1", name=name, kind=kind,
                         begin_ns=str(a), end_ns=str(b), begin_thread=f"ThreadId({thread})", end_thread=f"ThreadId({thread})",
                         begin_os_thread=str(thread), end_os_thread=str(thread), begin_cpu_ns="100", end_cpu_ns="100",
                         begin_cycles="1000", end_cycles="1100")
                    for name,kind,a,b,thread in [("task<A,B>","system",1820,1860,3),("other","system",1830,1880,4),("task<A,B>","commands",1840,1850,3)]]
        with (self.path / 'world-battle.tasks.csv').open('w',newline='') as out:
            writer=csv.DictWriter(out,list(rows[0]));writer.writeheader();writer.writerows(rows)
        with (self.path / 'world-battle.task-names.csv').open('w',newline='') as out:
            writer=csv.DictWriter(out,['name','allowed']);writer.writeheader()
            writer.writerows([dict(name=n,allowed=a) for n,a in [('task<A,B>',1),('other',1),('ambiguous',0)]])
        fields=dict(render_tasks_schema=1,render_task_limit=2000000,render_tasks_started=len(rows),render_tasks_retained=len(rows),render_tasks_dropped=0,render_tasks_incomplete=sum(not row['end_ns'] for row in rows),render_task_cutoff_ns=2000101,render_task_names=2,render_task_excluded_names=1,render_task_bytes=192)
        fields.update(metadata)
        with (self.path / 'world-battle.log').open('a') as out:
            out.write('PROFILE render_tasks_build=true\nPROFILE '+ ' '.join(f'{k}={v}' for k,v in fields.items())+'\n')
        path=self.path/'world-battle.manifest.json';manifest=json.loads(path.read_text());manifest['RenderTasks']=True;path.write_text(json.dumps(manifest))
        return rows

    def test_render_tasks_parallel_nested_wall_is_a_union_not_a_sum(self):
        self.task_fixture();report=profile.analyze(self.path)
        tasks=report['render_tasks'];frame=tasks['slow_prepare_frames'][0]
        self.assertAlmostEqual(frame['task_union_ms'],.00006)
        self.assertAlmostEqual(frame['other_or_unobserved_ms'],.00004)
        self.assertEqual(tasks['sampled_names'],2)
        self.assertEqual(tasks['excluded_names'],['ambiguous'])
        self.assertEqual(frame['longest_calls'][0]['os_thread'],4)
        self.assertEqual(frame['longest_calls'][0]['cpu_ms'],0)
        self.assertTrue(report['valid_for_comparison']);self.assertFalse(report['acceptance_eligible'])
        self.assertIn('Actual Bevy prepare tasks',profile.markdown(report))

    def test_render_tasks_whole_frame_stride_is_explicit_and_compared(self):
        self.task_fixture();full=profile.analyze(self.path)
        self.task_fixture(render_task_stride=8);sampled=profile.analyze(self.path)
        self.assertTrue(sampled['valid_for_comparison'])
        self.assertEqual(sampled['render_tasks']['stride'],8)
        self.assertIn('different:render_tasks:stride',profile.compare(full,sampled)['reasons'])
        self.assertIn('unsampled source rows',profile.markdown(sampled))
        self.task_fixture(render_task_stride=2)
        with self.assertRaisesRegex(ValueError,'whole-frame stride'):profile.analyze(self.path)
        rows=self.task_fixture();rows[0]['frame_row']='1';self.task_fixture(rows,render_task_stride=8)
        with self.assertRaisesRegex(ValueError,'Render tasks source'):profile.analyze(self.path)

    def test_render_tasks_conflicting_build_markers_are_invalid(self):
        self.task_fixture()
        with (self.path/'world-battle.log').open('a') as out:out.write('PROFILE render_tasks_build=false\n')
        self.assertIn('render_tasks_build_or_mode_missing',profile.analyze(self.path)['reasons'])

    def test_render_tasks_reject_identity_clock_threads_and_inventory(self):
        for field,value in [('frame_row','1'),('generation','1'),('tick','2'),('name','ambiguous'),('kind','launch'),('begin_ns','1799'),('end_ns','1901'),('begin_os_thread','0'),('end_os_thread','9'),('end_cycles','999'),('end_thread','ThreadId(9)')]:
            with self.subTest(field=field,value=value):
                rows=self.task_fixture();rows[0][field]=value;self.task_fixture(rows)
                with self.assertRaisesRegex(ValueError,'Render tasks'):profile.analyze(self.path)

    def test_render_tasks_bad_counts_missing_and_undeclared_cannot_pass(self):
        for key,value in [('render_task_limit',2000001),('render_tasks_retained',4),('render_tasks_started',4),('render_tasks_incomplete',1),('render_task_names',3),('render_task_bytes',1025),('render_task_cutoff_ns',2000100)]:
            with self.subTest(key=key,value=value):
                self.task_fixture(**{key:value})
                with self.assertRaisesRegex(ValueError,'Render tasks'):profile.analyze(self.path)
        self.task_fixture();(self.path/'world-battle.task-names.csv').unlink()
        self.assertIn('render_tasks_artifact_or_metadata_missing',profile.analyze(self.path)['reasons'])
        self.task_fixture();path=self.path/'world-battle.manifest.json';manifest=json.loads(path.read_text());manifest['RenderTasks']=False;path.write_text(json.dumps(manifest))
        self.assertIn('render_tasks_mode_mismatch',profile.analyze(self.path)['reasons'])

    def test_render_task_feature_build_and_modes_are_a_separate_contract(self):
        self.task_fixture();after=profile.analyze(self.path)
        # Retain a report snapshot then compare with a legacy no-task build.
        for suffix in ['tasks.csv','task-names.csv']:(self.path/f'world-battle.{suffix}').unlink()
        self.gas_fixture(schema=3);before=profile.analyze(self.path)
        self.assertIn('different:RenderTasks_build_or_mode',profile.compare(before,after)['reasons'])
        self.write()
        with (self.path/'world-battle.log').open('a') as out:out.write('PROFILE render_tasks_build=true\n')
        ordinary=profile.analyze(self.path)
        self.assertFalse(ordinary['acceptance_eligible'])

    def test_render_tasks_open_terminal_call_is_not_complete_coverage(self):
        rows=self.task_fixture();rows=rows[:1];rows[0]['end_ns']='';rows[0]['end_thread']=''
        for key in ['end_os_thread','end_cpu_ns','end_cycles']:rows[0][key]=''
        self.task_fixture(rows)
        # Open prepare/schedule in the terminal source row, with no post-prepare stages.
        path=self.path/'world-battle.render.csv';spans=list(profile.read_csv(path));kept=[]
        for row in spans:
            if row['stage'] in ('render_system','render_graph','submit_present_tail'):continue
            if row['stage'] in ('render_prepare','render_schedule'):
                row['end_ns']='';row['end_thread']=''
            kept.append(row)
        with path.open('w',newline='') as out:
            writer=csv.DictWriter(out,list(kept[0]));writer.writeheader();writer.writerows(kept)
        logpath=self.path/'world-battle.log';log=logpath.read_text();log=log.replace('render_spans_started=13','render_spans_started=10').replace('render_spans_retained=13','render_spans_retained=10').replace('render_spans_incomplete=0','render_spans_incomplete=2');logpath.write_text(log)
        result=profile.analyze(self.path)['render_tasks']
        self.assertEqual(result['incomplete'],1)
        self.assertIsNone(result['slow_prepare_frames'][0]['other_or_unobserved_ms'])

    def test_extract_phase_schema_is_a_separate_diagnostic_contract(self):
        self.gas_fixture(schema=2);before=profile.analyze(self.path)
        self.gas_fixture(schema=3);after=profile.analyze(self.path)
        self.assertIn("different:render_timeline:schema",profile.compare(before,after)["reasons"])

    def test_journal_maintenance_is_inclusive_wall_and_legacy_is_unknown(self):
        self.assertIsNone(profile.journal_maintenance(''))
        result=profile.journal_maintenance('PROFILE terrain_journal_compact_calls=10 total_ms=2.000 peak_ms=0.900')
        self.assertEqual(result['mean_ms'],0.2)
        self.assertIn('already included',result['scope'])
        for line in ['PROFILE terrain_journal_compact_calls=0 total_ms=1 peak_ms=0',
                     'PROFILE terrain_journal_compact_calls=1 total_ms=1 peak_ms=2',
                     'PROFILE terrain_journal_compact_calls=1 total_ms=NaN peak_ms=0']:
            with self.assertRaisesRegex(ValueError,'Terrain journal'):profile.journal_maintenance(line)

    def test_actual_gas_calls_are_on_worker_threads_and_transfers_not_doubled(self):
        self.gas_fixture()
        result = profile.analyze(self.path)["render_timeline"]
        self.assertTrue(result["gas_details"])
        self.assertEqual(result["complete_source_rows"], 1)
        self.assertEqual(result["gas_summary"]["actual_extract_calls"], 1)
        self.assertEqual(result["gas_summary"]["transfer_totals"]["terrain_bytes"], 128)
        self.assertIn("gas_extract", result["slow_render_frames"][0]["leaf_work"])

    def test_gas_payload_reuse_keeps_original_extraction_identity(self):
        first = self.gas_fixture()
        second = [dict(row, frame_row="1", begin_ns=str(int(row["begin_ns"]) + 2000000),
                       end_ns=str(int(row["end_ns"]) + 2000000))
                  for row in first if row["stage"] != "gas_extract"]
        self.render_fixture(sorted(first + second, key=lambda r: int(r["begin_ns"])),
                            render_timeline_schema=2, render_gas_details="true", render_cutoff_ns=2005000)
        result = profile.analyze(self.path)["render_timeline"]
        self.assertEqual(result["complete_source_rows"], 2)
        self.assertEqual(result["gas_summary"]["actual_extract_calls"], 1)
        self.assertEqual(result["gas_summary"]["reused_payload_calls"], 1)
        # Work happened again, even though the extracted payload was reused.
        self.assertEqual(result["gas_summary"]["transfer_totals"]["terrain_bytes"], 256)
        reused = next(r for r in result["slow_gas_calls"]["gas_prepare"] if r["source_row"] == 1)
        self.assertEqual(reused["leaf_work"]["gas_prepare"]["input_frame_row"], 0)

    def test_render_markdown_labels_cpu_and_actual_call_work(self):
        self.gas_fixture()
        report = profile.analyze(self.path)
        rendered = profile.markdown(report)
        self.assertIn("Mean calling-thread CPU, ms", rendered)
        self.assertIn("Actual extract and prepare calls ranked separately", rendered)
        self.assertIn("other jobs/scheduling", rendered)
        self.assertIn("unavailable", rendered)
        self.assertNotIn("actual leaf calls {", rendered)

    def test_slow_extract_survives_schedule_only_top_limit(self):
        first = self.gas_fixture()
        second = [dict(row, frame_row="1", begin_ns=str(int(row["begin_ns"]) + 2000000),
                       end_ns=str(int(row["end_ns"]) + 2000000),
                       input_frame_row="1" if row["input_frame_row"] else "") for row in first]
        next(r for r in second if r["stage"] == "gas_extract")["end_ns"] = "2001590"
        self.render_fixture(sorted(first + second, key=lambda r: int(r["begin_ns"])),
                            render_timeline_schema=2, render_gas_details="true", render_cutoff_ns=2005000)
        result = profile.analyze(self.path, top=1)["render_timeline"]
        self.assertEqual(result["slow_render_frames"][0]["source_row"], 0)
        self.assertEqual(result["slow_gas_calls"]["gas_extract"][0]["source_row"], 1)

    def test_missing_gas_extract_is_not_an_invented_call(self):
        spans = [r for r in self.gas_fixture() if r["stage"] != "gas_extract"]
        for row in spans:
            if row["stage"] in profile.GAS_STAGES: row["input_frame_row"] = ""
        self.render_fixture(spans, render_timeline_schema=2, render_gas_details="true")
        result = profile.analyze(self.path)["render_timeline"]
        self.assertEqual(result["complete_source_rows"], 1)
        self.assertEqual(result["gas_summary"]["actual_extract_calls"], 0)
        self.assertEqual(result["gas_summary"]["source_rows_without_extract_call"], 1)

    def test_aborted_prepare_is_explicit_and_does_not_invent_remaining_stages(self):
        self.gas_fixture(False)
        result = profile.analyze(self.path)["render_timeline"]
        self.assertEqual(result["gas_summary"]["aborted_prepare_calls"], 1)
        self.assertNotIn("gas_regions", result["stage_ms"])

    def test_gas_detail_integrity_rejects_missing_stages_clock_payload_or_threads(self):
        for stage, changes in (("gas_regions", dict(begin_thread="ThreadId(5)", end_thread="ThreadId(5)")),
                               ("gas_extract", dict(input_frame_row="1")),
                               ("gas_bricks", dict(terrain_bytes="128")),
                               ("gas_prepare", dict(words="-1")),
                               ("gas_prepare", dict(completed="")),
                               ("gas_regions", dict(end_ns="1950"))):
            with self.subTest(stage=stage, changes=changes):
                spans=self.gas_fixture()
                next(r for r in spans if r["stage"] == stage).update(changes)
                self.render_fixture(spans, render_timeline_schema=2, render_gas_details="true")
                with self.assertRaisesRegex(ValueError, "Render timeline"):
                    profile.analyze(self.path)
        spans=[r for r in self.gas_fixture() if r["stage"] != "gas_bricks"]
        self.render_fixture(spans, render_timeline_schema=2, render_gas_details="true")
        with self.assertRaisesRegex(ValueError, "coverage missing"):
            profile.analyze(self.path)

    def test_gas_contract_and_legacy_render_contract_do_not_form_equal_pair(self):
        self.render_fixture(); before=profile.analyze(self.path)
        self.gas_fixture(); after=profile.analyze(self.path)
        self.assertIn("different:render_timeline:schema", profile.compare(before, after)["reasons"])


    def test_payload_reference_requires_actual_extract_with_same_facts(self):
        spans = [r for r in self.gas_fixture() if r["stage"] != "gas_extract"]
        self.render_fixture(spans, render_timeline_schema=2, render_gas_details="true")
        with self.assertRaisesRegex(ValueError, "source extraction missing"):
            profile.analyze(self.path)
        spans=self.gas_fixture()
        next(r for r in spans if r["stage"] == "gas_prepare")["words"]="32"
        self.render_fixture(spans, render_timeline_schema=2, render_gas_details="true")
        with self.assertRaisesRegex(ValueError, "payload facts mismatch"):
            profile.analyze(self.path)


if __name__ == "__main__":
    unittest.main()
