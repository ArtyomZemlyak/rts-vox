"""Offline Chrome/Perfetto export. No game instrumentation or third-party packages.

Tracks are logical wall scopes, not scheduler CPU tracks. All clocks come from
the capture's shared monotonic epoch. Aggregated physics totals have no start
time and are deliberately never turned into invented worker intervals.
"""
import csv
import bisect
import heapq
import itertools
import json
import math
from pathlib import Path


def rows(path):
    with Path(path).open(encoding="utf-8-sig", newline="") as source:
        yield from csv.DictReader(source)


class Writer:
    def __init__(self, output, limit, windows=None):
        self.output, self.limit = output, limit
        self.retained = self.dropped = 0
        self.filtered = 0
        self.windows = windows
        self.window_ends = [b for a, b in windows] if windows is not None else []
        self.tracks = {}
        self.first = True
        output.write('{"traceEvents":[')

    def raw(self, event):
        if not self.first:
            self.output.write(",\n")
        self.first = False
        self.output.write(json.dumps(event, ensure_ascii=False, allow_nan=False, separators=(",", ":")))

    def emit(self, track, event, interval=None):
        begin, end = interval or (round(event["ts"] * 1000), round(event["ts"] * 1000))
        if self.windows is not None:
            index = bisect.bisect_right(self.window_ends, begin)
            if index == len(self.windows) or self.windows[index][0] > end or (end > begin and self.windows[index][0] == end):
                self.filtered += 1
                return
        if self.retained >= self.limit:
            self.dropped += 1
            return
        if track not in self.tracks:
            self.tracks[track] = len(self.tracks) + 1
            self.raw(dict(ph="M", name="thread_name", pid=1, tid=self.tracks[track], args=dict(name=track)))
        self.raw(dict(pid=1, tid=self.tracks[track], **event))
        self.retained += 1

    def span(self, track, name, begin, end, category, args):
        if begin < 0 or end < begin:
            raise ValueError("Invalid trace interval")
        self.emit(track, dict(ph="X", name=name, cat=category, ts=begin / 1000,
                             dur=(end - begin) / 1000, args=args), interval=(begin, end))

    def instant(self, track, name, when, args):
        self.emit(track, dict(ph="i", s="t", name=name, ts=when / 1000, args=args))

    def finish(self, metadata):
        metadata.update(retained_events=self.retained, dropped_events=self.dropped,
                        event_limit=self.limit, truncated=self.dropped > 0, logical_tracks=len(self.tracks),
                        filtered_outside_focus=self.filtered)
        self.output.write('],"displayTimeUnit":"ms","otherData":')
        self.output.write(json.dumps(metadata, ensure_ascii=False, allow_nan=False))
        self.output.write("}\n")


def identity(row):
    return dict(generation=int(row["generation"]), tick=int(row["tick"]))


def focus_windows(path, count, context_ms):
    if count == 0:
        return None, []
    heap, previous = [], None
    for idx, row in enumerate(rows(path)):
        now = int(row["frame_start_ns"])
        if previous is not None:
            old_idx, begin = previous
            # Earlier row wins ties. Only top-N frame identities stay in memory.
            value = (now - begin, -old_idx, begin, now)
            heapq.heappush(heap, value)
            if len(heap) > count:
                heapq.heappop(heap)
        previous = idx, now
    padding = round(context_ms * 1e6)
    selected = sorted(heap, reverse=True)
    windows = []
    for begin, end in sorted((max(0, r[2] - padding), r[3] + padding) for r in selected):
        if windows and begin <= windows[-1][1]:
            windows[-1][1] = max(windows[-1][1], end)
        else:
            windows.append([begin, end])
    return windows, [-r[1] for r in selected]


def export_frames(directory, scenario, writer, threshold):
    journal = directory / f"{scenario}.slow-frames.jsonl"
    previous = None
    slow = intervals = 0
    with journal.open("w", encoding="utf-8", newline="\n") as out:
        for idx, row in enumerate(rows(directory / f"{scenario}.csv")):
            begin, end = int(row["frame_start_ns"]), int(row["frame_end_ns"])
            steps = int(row["simulation_ticks"])
            args = dict(row=idx, simulation_ticks=steps, **identity(row))
            writer.span("Main / measured Update", "Update", begin, end, "frame", args)
            workload = {k: int(row[k]) for k in (
                "fragment_records", "corpse_records", "material_claims", "material_jobs",
                "pending_kinetic", "felling_pending", "live_waves", "capturing_waves",
                "pending_wave_contacts", "pending_world_edits", "searching_units", "moving_units") if row.get(k) not in (None, "")}
            if workload:
                writer.emit("Workload / record counts", dict(ph="C", name="Record counts", ts=end / 1000, args=workload))
            if previous is not None:
                old_idx, old, old_begin, old_end, old_workload = previous
                if begin < old_end:
                    raise ValueError("Overlapping frame slices")
                interval_args = dict(row=old_idx, **identity(old))
                writer.span("Main / begin-to-begin intervals", "Frame interval", old_begin, begin, "frame", interval_args)
                writer.span("Main / following gap (cause unknown)", "Outside measured Update", old_end, begin, "gap", interval_args)
                duration = (begin - old_begin) / 1e6
                intervals += 1
                if duration >= threshold:
                    old_steps, tick = int(old["simulation_ticks"]), int(old["tick"])
                    record = dict(schema=1, **interval_args, begin_ns=old_begin, end_ns=begin,
                        frame_ms=duration, fps=1000 / duration if duration else None,
                        update_ms=(old_end - old_begin) / 1e6, following_gap_ms=(begin - old_end) / 1e6,
                        simulation_ticks=old_steps, first_tick=(tick - old_steps + 1) % (1 << 32) if old_steps else None,
                        core_ms=float(old["simulation_core_ms"]), workload=old_workload,
                        recorded_components_ms={k: float(old[k]) for k in (
                            "simulation_input_ms", "unit_mesh_ms", "fragments_ms", "presentation_ms",
                            "terrain_ms", "occupancy_ms", "physics_ms", "atmosphere_ms") if old.get(k) not in (None, "")},
                        note="Preceding Update plus its following gap. Components can overlap; gap cause is unknown.")
                    out.write(json.dumps(record, allow_nan=False) + "\n")
                    slow += 1
            previous = idx, row, begin, end, workload
    return dict(slow_frame_path=str(journal), slow_frames=slow, observed_frame_intervals=intervals,
                slow_frame_threshold_ms=threshold)


def export_core(path, writer, summarize_tick, sampling):
    if not path.exists():
        return 0
    count = 0
    for _, group in itertools.groupby(rows(path), key=lambda r: (r["generation"], r["tick"])):
        raw = list(group)  # One bounded native tick, never the complete raw trace.
        tick = summarize_tick(raw, sampling)
        begin = next(r for r in raw if r["lane"] == "tick" and r["event"] == "begin")
        end = next(r for r in raw if r["lane"] == "tick" and r["event"] == "end")
        origin = tick["profile_begin_ns"] - int(begin["offset_ns"])
        args = dict(**identity(begin), source=path.name, caller_os_thread=begin.get("os_thread") or None,
                    coordinator_os_thread=tick["phase_os_thread"], delegated=tick["delegated"])
        writer.span("Core / outer tick wall", "Tick", tick["profile_begin_ns"], tick["profile_end_ns"], "core", args)
        previous = begin
        for row in sorted((r for r in raw if r["lane"] == "phase"), key=lambda r: int(r["offset_ns"])) + [end]:
            name = row["event"] if row is not end else "unmarked_tail"
            writer.span("Core / phase boundary intervals", name, int(previous["profile_offset_ns"]),
                int(row["profile_offset_ns"]), "phase", dict(args,
                    begin_os_thread=previous.get("os_thread") or None, end_os_thread=row.get("os_thread") or None))
            previous = row
        for detail in tick["details"]:
            writer.span(f"Core detail / {detail['group']}", detail["name"], origin + detail["start"],
                origin + detail["end"], "detail", dict(args, thread_cpu_ms=detail["thread_cpu_ms"]))
        pending, jobs = {}, []
        for row in sorted(raw, key=lambda r: int(r["offset_ns"])):
            if row["lane"] != "task":
                continue
            event = row["event"]
            if not row.get("os_thread"):
                writer.instant("Worker / unknown OS thread", event, int(row["profile_offset_ns"]), args)
                continue
            suffix = "_begin" if event.endswith("_begin") else "_end"
            family = event[:-len(suffix)]
            key = row["os_thread"], row["unit"], family
            if suffix == "_begin":
                pending.setdefault(key, []).append(row)
            else:
                first = pending[key].pop()
                cpu_a, cpu_b = first.get("thread_cpu_ns"), row.get("thread_cpu_ns")
                cpu_ns = int(cpu_b) - int(cpu_a) if cpu_a and cpu_b else None
                jobs.append((int(first["profile_offset_ns"]), int(row["profile_offset_ns"]), key[0], family,
                    dict(args, os_thread=key[0], slot_or_actor=key[1], thread_cpu_ns=cpu_ns,
                         retained_physics_sampling=sampling if family.startswith("physics_") else "recorded_actor_scopes")))
        # Keep real nested scopes together, but separate crossing observer
        # intervals: Chrome duration slices must nest. Slot/actor is payload,
        # not a separate UI track for every actor on every worker.
        lanes = {}
        for start, stop, thread, family, job_args in sorted(jobs, key=lambda j: (j[0], -j[1])):
            choices = lanes.setdefault(thread, [])
            for index, stack in enumerate(choices):
                while stack and stack[-1] <= start:
                    stack.pop()
                if not stack or stop <= stack[-1]:
                    break
            else:
                index, stack = len(choices), []
                choices.append(stack)
            stack.append(stop)
            writer.span(f"Worker / OS {thread} / logical lane {index + 1}", family,
                start, stop, "worker", job_args)
        count += 1
    return count


def export_render(path, writer, tasks=False):
    if not path.exists():
        return 0
    count = 0
    for row in rows(path):
        name = row["name"] if tasks else row["stage"]
        os_thread = row.get("begin_os_thread") or row.get("begin_thread") or "unknown"
        kind = row.get("kind", "stage")
        track = f"{'Bevy task' if tasks else 'Render'} / {os_thread} / {kind} / {name}"
        args = dict(**identity(row), source_row=int(row["frame_row"]), os_thread=os_thread,
                    input_frame_row=row.get("input_frame_row") or None, source=path.name)
        for field in ("patches", "words", "terrain_bytes", "terrain_writes", "dirty_ranges"):
            if row.get(field) not in (None, ""):
                args[field] = int(row[field])
        begin = int(row["begin_ns"])
        if row.get("end_ns") in (None, ""):
            writer.instant(track, name + " (open at capture end)", begin, dict(args, incomplete=True))
        else:
            args["completed"] = row.get("completed") or None
            cpu_a, cpu_b = row.get("begin_cpu_ns"), row.get("end_cpu_ns")
            same = row.get("begin_os_thread") and row.get("begin_os_thread") == row.get("end_os_thread")
            args["thread_cpu_ns"] = int(cpu_b) - int(cpu_a) if same and cpu_a and cpu_b else None
            writer.span(track, name, begin, int(row["end_ns"]), "render_task" if tasks else "render", args)
        count += 1
    return count


def export(directory, scenario, report, *, summarize_tick, event_limit=500000, slow_frame_ms=50.0,
           focus_frames=12, context_ms=250.0):
    if (event_limit < 1 or not math.isfinite(slow_frame_ms) or slow_frame_ms <= 0 or
            focus_frames < 0 or not math.isfinite(context_ms) or context_ms < 0):
        raise ValueError("Invalid export budget/threshold")
    if report.get("frame_alignment") != "shared_monotonic_epoch":
        raise ValueError("Trace export requires shared monotonic frame timestamps")
    directory = Path(directory)
    path = directory / f"{scenario}.trace.json"
    windows, selected_rows = focus_windows(directory / f"{scenario}.csv", focus_frames, context_ms)
    # Only consume artifacts explicitly observed and validated by the analyzer.
    # A stale CSV next to an ordinary capture must not become new evidence.
    metadata = dict(schema=1, path=str(path), scenario=scenario, clock="profile_monotonic_ns_converted_to_us",
        valid_for_comparison=report["valid_for_comparison"], reasons=report["reasons"],
        binary_sha256=(report.get("manifest") or {}).get("BinarySha256"), state_hash=report.get("state_hash"),
        render_context=(report.get("actual_render_context") or {}).get("initial"),
        physics_sampling=report.get("physics_task_sampling"),
        focus_windows_ns=windows, focus_frame_rows=selected_rows, context_ms=context_ms,
        limitations=["Logical wall tracks; OS identities are labels, not scheduler execution or CPU utilization.",
            "Nested/concurrent intervals must not be added. Missing worker jobs remain missing.",
            "Frame intervals contain the preceding Update and following gap; terminal Update has no observed next interval.",
            "GPU publication aggregates have no shared interval clock and are not exported as GPU slices."])
    # Replace atomically: never leave a truncated JSON file advertised as a trace.
    temporary = path.with_suffix(".json.tmp")
    try:
        with temporary.open("w", encoding="utf-8", newline="\n") as output:
            writer = Writer(output, event_limit, windows)
            metadata.update(export_frames(directory, scenario, writer, slow_frame_ms))
            metadata["core_ticks"] = export_core(directory / f"{scenario}.core.csv", writer, summarize_tick,
                report.get("physics_task_sampling")) if report.get("trace_steps", 0) else 0
            stall = report.get("stall_trace")
            metadata["stall_ticks"] = export_core(directory / f"{scenario}.stall.csv", writer, summarize_tick,
                "phase_ring") if stall else 0
            metadata["render_records"] = export_render(directory / f"{scenario}.render.csv", writer) if report.get("render_timeline") else 0
            metadata["bevy_task_records"] = export_render(directory / f"{scenario}.tasks.csv", writer, tasks=True) if report.get("render_tasks") else 0
            writer.finish(metadata)
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)
    (directory / f"{scenario}.trace-export.json").write_text(json.dumps(metadata, indent=2), encoding="utf-8")
    return metadata
