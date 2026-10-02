"""Native range diagnostics. Standard library only; at most one raw tick in memory.

Wall intervals from different lanes/threads overlap. CPU time is an OS counter,
not a measurement of lock waits. Cycle counts are never converted to milliseconds.
"""
import argparse
import bisect
import csv
import datetime
import itertools
import json
import math
from pathlib import Path
import re
import statistics


def read_csv(path):
    with Path(path).open(encoding="utf-8-sig", newline="") as source:
        yield from csv.DictReader(source)


def optional_int(row, key):
    value = row.get(key)
    return int(value) if value not in (None, "") else None


def stats(values):
    values = sorted(values)
    if not values:
        return None
    return {"samples": len(values), "mean": statistics.mean(values),
            "p99": values[min(len(values) - 1, math.floor(len(values) * .99))],
            "max": values[-1]}


def render_context(log):
    initial = re.search(r"^PROFILE render_context=(RenderContext \{[^\r\n]+\})$", log, re.M)
    steps = [dict(step=int(step), value=value) for step, value in
             re.findall(r"^PROFILE render_context_step=(\d+) value=(RenderContext \{[^\r\n]+\})$", log, re.M)]
    changed = re.search(r"^PROFILE render_context_changed=(true|false)$", log, re.M)
    reasons = []
    if not initial or not steps or not changed or not re.search(r"^PROFILE render_context_schema=2$", log, re.M):
        reasons.append("render_context_missing")
    elif initial[1] != steps[0]["value"]:
        reasons.append("render_context_initial_mismatch")
    if changed and changed[1] == "true":
        reasons.append("render_context_changed_during_run")
    contexts = ([initial[1]] if initial else []) + [step["value"] for step in steps]
    focus = [re.search(r"\bfocused: (true|false)(?:,| \})", value) for value in contexts]
    if not focus or any(value is None for value in focus):
        reasons.append("render_focus_missing")
    elif len({value[1] for value in focus}) > 1:
        reasons.append("render_focus_changed_during_run")
    return dict(initial=initial[1] if initial else None, steps=steps,
                invalid_change=changed[1] == "true" if changed else None, reasons=reasons)


def interval(a, b):
    wall = int(b["offset_ns"]) - int(a["offset_ns"])
    if wall < 0:
        raise ValueError("Non-monotonic trace boundary")
    same = a.get("os_thread") and a.get("os_thread") == b.get("os_thread")
    def delta(key):
        x, y = optional_int(a, key), optional_int(b, key)
        if not same or x is None or y is None:
            return None
        if y < x:
            raise ValueError(f"CPU counter regressed: {key}")
        return y - x
    cpu = delta("thread_cpu_ns")
    return {"wall_ms": wall / 1e6, "thread_cpu_ms": None if cpu is None else cpu / 1e6,
            "wall_minus_cpu_ms": None if cpu is None else (wall - cpu) / 1e6,
            "cycles": delta("cpu_cycles"), "os_thread": b.get("os_thread") or None}


def union_by_thread(jobs):
    """Inclusive parent/child intervals on one OS thread count only once."""
    result = {}
    for thread, group in itertools.groupby(sorted(jobs, key=lambda j: (j["thread"], j["start"])),
                                            key=lambda j: j["thread"]):
        merged = []
        for job in group:
            a, b = job["start"], job["end"]
            if merged and a <= merged[-1][1]:
                merged[-1][1] = max(merged[-1][1], b)
            else:
                merged.append([a, b])
        result[thread] = sum(b - a for a, b in merged) / 1e6
    return result


def os_window(directory, scenario, log, frames, roles):
    """Keep only entire 1Hz intervals inside the native measurement window."""
    path = directory / f"{scenario}-cpu"
    epoch = re.search(r"epoch_unix_ns=(\d+)", log)
    if (not epoch or not frames or optional_int(frames[-1], "frame_end_ns") is None
            or not path.joinpath("cpu-process-samples.csv").exists()):
        return {"status": "unavailable_or_legacy"}
    begin = int(epoch[1])
    end = begin + int(frames[-1]["frame_end_ns"])
    previous = None
    accepted, cpu_ms, wall_ms = set(), 0.0, 0.0
    for row in read_csv(path / "cpu-process-samples.csv"):
        if not row.get("Utc"):
            continue
        now = int(datetime.datetime.fromisoformat(row["Utc"].replace("Z", "+00:00")).timestamp() * 1e9)
        if previous and begin <= previous[0] <= now <= end:
            elapsed = (float(row["Seconds"]) - float(previous[1]["Seconds"])) * 1000
            delta = float(row["CpuMs"]) - float(previous[1]["CpuMs"])
            if elapsed > 0 and delta >= 0:
                cpu_ms += delta; wall_ms += elapsed; accepted.add(row["Seconds"])
        previous = now, row
    threads = {}
    thread_path = path / "cpu-threads.csv"
    if thread_path.exists() and thread_path.stat().st_size:
        for row in read_csv(thread_path):
            if row["Seconds"] in accepted:
                # Creation time prevents OS thread-ID reuse from merging lives.
                key = row["ThreadId"], row["CreatedAt"]
                threads[key] = threads.get(key, 0.0) + float(row["CpuMs"])
    ranked = [dict(os_thread=t, created_at=c, cpu_ms=value,
                   observed_roles=sorted(roles.get(t, []))) for (t, c), value in threads.items()]
    ranked.sort(key=lambda r: -r["cpu_ms"])
    return dict(status="aligned_intervals", intervals=len(accepted), observed_ms=wall_ms,
                process_cpu_ms=cpu_ms, core_equivalents=cpu_ms / wall_ms if wall_ms else None,
                sampled_threads=ranked,
                process_minus_sampled_threads_ms=cpu_ms - sum(threads.values()))


def summarize_tick(rows):
    begins = [r for r in rows if r["lane"] == "tick" and r["event"] == "begin"]
    ends = [r for r in rows if r["lane"] == "tick" and r["event"] == "end"]
    if len(begins) != 1 or len(ends) != 1:
        raise ValueError("Tick needs exactly one begin and end")
    begin, end = begins[0], ends[0]
    total = interval(begin, end)
    origin = optional_int(begin, "profile_offset_ns")
    if origin is not None:
        origin -= int(begin["offset_ns"])
    phases, units, jobs = [], [], []
    last = begin
    unit_last = {}
    pending = {}
    unidentified_tasks = 0
    for row in sorted(rows, key=lambda r: int(r["offset_ns"])):
        if not int(begin["offset_ns"]) <= int(row["offset_ns"]) <= int(end["offset_ns"]):
            raise ValueError("Event lies outside its tick")
        if origin is not None and optional_int(row, "profile_offset_ns") != origin + int(row["offset_ns"]):
            raise ValueError("Trace clocks disagree")
        if row["lane"] == "phase":
            if begin.get("os_thread") and row.get("os_thread") and row["os_thread"] != begin["os_thread"]:
                raise ValueError("Phase boundary is not on the tick caller")
            phases.append(dict(name=row["event"], **interval(last, row)))
            last = row
        elif row["lane"] == "unit":
            if row["unit"] in unit_last:
                units.append(dict(unit=row["unit"], name=row["event"],
                                  **interval(unit_last[row["unit"]], row)))
            unit_last[row["unit"]] = row
        elif row["lane"] == "task":
            if not row.get("os_thread"):
                unidentified_tasks += 1
                continue
            event = row["event"]
            if event.endswith("_begin"):
                key = (row["unit"], event[:-6], row["os_thread"])
                pending.setdefault(key, []).append(row)
            elif event.endswith("_end"):
                key = (row["unit"], event[:-4], row["os_thread"])
                if not pending.get(key):
                    raise ValueError(f"Unmatched task end: {key}")
                a = pending[key].pop()
                jobs.append(dict(name=key[1], unit=key[0], thread=key[2],
                                 start=int(a["offset_ns"]), end=int(row["offset_ns"]),
                                 **interval(a, row)))
    if any(pending.values()):
        raise ValueError("Unfinished worker task")
    # The tail is explicit; phase totals partition the caller's tick only.
    phases.append(dict(name="unmarked_tail", **interval(last, end)))
    return dict(generation=int(begin["generation"]), tick=int(begin["tick"]), **total,
                profile_begin_ns=optional_int(begin, "profile_offset_ns"),
                profile_end_ns=optional_int(end, "profile_offset_ns"), phases=phases,
                unit_intervals=sorted(units, key=lambda r: -r["wall_ms"])[:8],
                longest_tasks=sorted(jobs, key=lambda r: -r["wall_ms"])[:8],
                task_count=len(jobs), unidentified_task_events=unidentified_tasks,
                worker_families={thread: sorted({j["name"] for j in jobs if j["thread"] == thread})
                                 for thread in {j["thread"] for j in jobs}},
                worker_covered_wall_ms=union_by_thread(jobs))


def analyze(directory, scenario="world-battle", top=12):
    directory = Path(directory)
    frames = list(read_csv(directory / f"{scenario}.csv"))
    if not frames:
        raise ValueError("No frame samples")
    if any(not math.isfinite(float(r["frame_ms"])) or float(r["frame_ms"]) <= 0 for r in frames):
        raise ValueError("Invalid frame duration")
    starts = [optional_int(r, "frame_start_ns") for r in frames]
    aligned = all(s is not None for s in starts)
    clock_fields = ("frame_start_ns", "frame_end_ns", "frame_gap_ns")
    if any(optional_int(r, key) is not None for r in frames for key in clock_fields):
        if not all(optional_int(r, key) is not None for r in frames for key in clock_fields):
            raise ValueError("Incomplete frame clock")
    if aligned and starts != sorted(starts):
        raise ValueError("Frame clock regressed")
    if aligned:
        for idx, row in enumerate(frames):
            start, end, gap = [int(row[key]) for key in clock_fields]
            if min(start, end, gap) < 0 or end < start:
                raise ValueError("Invalid frame clock interval")
            expected_gap = start - int(frames[idx - 1]["frame_end_ns"]) if idx else 0
            if expected_gap < 0 or gap != expected_gap:
                raise ValueError("Frame slices overlap or gap disagrees")
    frame_times = []
    for idx, row in enumerate(frames):
        observed = row.get("observed_frame_ms")
        supplied = float(observed) if observed not in (None, "") else None
        derived = (starts[idx] - starts[idx - 1]) / 1e6 if aligned and idx else None
        if derived is not None and derived <= 0:
            raise ValueError("Frame clock did not advance")
        if "observed_frame_ms" in row and idx and supplied is None:
            raise ValueError("Missing observed frame duration")
        if supplied is not None and (not math.isfinite(supplied) or supplied <= 0):
            raise ValueError("Invalid observed frame duration")
        if supplied is not None and derived is not None and not math.isclose(supplied, derived, rel_tol=0, abs_tol=0.000002):
            raise ValueError("Observed frame duration disagrees with clock")
        frame_times.append(derived if derived is not None else supplied if aligned else float(row["frame_ms"]))
    ticks_per_frame = [int(r["simulation_ticks"]) for r in frames]
    frame_ticks = sum(ticks_per_frame)
    if min(ticks_per_frame) < 0:
        raise ValueError("Negative step count")
    core_totals, phase_totals, slow, seen = [], {}, [], set()
    unmapped = 0
    identity_mismatches = 0
    missing_clock = 0
    mapped_steps = [0] * len(frames)
    roles = {}
    core_path = directory / f"{scenario}.core.csv"
    if core_path.exists():
        groups = itertools.groupby(read_csv(core_path), key=lambda r: (r["generation"], r["tick"]))
        for key, raw_rows in groups:
            if key in seen:
                raise ValueError("Non-contiguous or duplicated tick in trace")
            seen.add(key)
            tick = summarize_tick(list(raw_rows))
            if tick["os_thread"]:
                roles.setdefault(tick["os_thread"], set()).add("tick_caller")
            for thread, families in tick["worker_families"].items():
                roles.setdefault(thread, set()).update(families)
            core_totals.append(tick["wall_ms"])
            for phase in tick["phases"]:
                phase_totals.setdefault(phase["name"], []).append(phase)
            if aligned:
                if tick["profile_begin_ns"] is None or tick["profile_end_ns"] is None:
                    missing_clock += 1
                    idx = -1
                else:
                    idx = bisect.bisect_right(starts, tick["profile_begin_ns"]) - 1
                if idx >= 0 and tick["profile_end_ns"] <= int(frames[idx]["frame_end_ns"]):
                    tick["frame_row"] = idx
                    # A matching total is insufficient: resets and multiple
                    # steps in one frame must preserve each frame's identity.
                    expected_tick = (int(frames[idx]["tick"]) - ticks_per_frame[idx] + mapped_steps[idx] + 1) % (1 << 32)
                    if tick["generation"] != int(frames[idx]["generation"]) or tick["tick"] != expected_tick:
                        identity_mismatches += 1
                    mapped_steps[idx] += 1
                else:
                    unmapped += 1
            # Bound retained raw detail to the slowest top ticks.
            slow.append(tick)
            slow.sort(key=lambda r: -r["wall_ms"])
            del slow[top:]
    phase_summary = []
    for name, values in phase_totals.items():
        cpu = [v["thread_cpu_ms"] for v in values if v["thread_cpu_ms"] is not None]
        wall = [v["wall_ms"] for v in values]
        phase_summary.append(dict(name=name, mean_per_tick_ms=sum(wall) / len(seen),
                                  wall_occurrences=stats(wall), thread_cpu_occurrences=stats(cpu)))
    phase_summary.sort(key=lambda r: -r["mean_per_tick_ms"])
    log = (directory / f"{scenario}.log").read_text(encoding="utf-8-sig", errors="replace")
    manifest_path = directory / f"{scenario}.manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8-sig")) if manifest_path.exists() else None
    validity = manifest.get("Validity") or {} if manifest else {}
    reasons = list(validity.get("Reasons") or [])
    if not manifest:
        reasons.append("legacy_run_without_manifest")
    elif not manifest.get("BinarySha256") or not manifest.get("InputHashes"):
        reasons.append("incomplete_provenance")
    if not frame_ticks:
        reasons.append("simulation_did_not_advance")
    if not aligned:
        reasons.append("monotonic_frame_clock_missing")
    if core_path.exists() and len(seen) != frame_ticks:
        reasons.append("frame_core_tick_count_mismatch")
    if manifest and manifest.get("CorePhases") and not core_path.exists():
        reasons.append("core_trace_missing")
    if unmapped:
        reasons.append("core_ticks_outside_frame_slice")
    if missing_clock:
        reasons.append("core_tick_clock_missing")
    if identity_mismatches:
        reasons.append("frame_core_tick_identity_mismatch")
    if aligned and core_path.exists() and mapped_steps != ticks_per_frame:
        reasons.append("frame_core_step_distribution_mismatch")
    hash_match = re.search(r"^PROFILE state_hash=([0-9a-f]+)$", log, re.M)
    adapter = re.search(r"^PROFILE adapter_name=.*$", log, re.M)
    rate = re.search(r"^PROFILE tick_rate=(\d+)", log, re.M)
    pools = re.search(r"^PROFILE physics_workers=(\d+) navigation_workers=(\d+)", log, re.M)
    context = render_context(log)
    reasons.extend(context["reasons"])
    if not pools:
        reasons.append("actual_worker_counts_missing")
    seconds = int(frames[-1]["frame_end_ns"]) / 1e9 if aligned else float(frames[-1]["seconds"])
    expensive_frames = []
    for idx in sorted((i for i, value in enumerate(frame_times) if value is not None), key=lambda i: -frame_times[i])[:top]:
        row = frames[idx]
        prev = frames[idx - 1] if idx else None
        expensive_frames.append(dict(row=idx, tick=int(row["tick"]), generation=int(row["generation"]),
            frame_delta_ms=frame_times[idx], bevy_delta_ms=float(row["frame_ms"]), current_core_ms=float(row["simulation_core_ms"]),
            previous_core_ms=float(prev["simulation_core_ms"]) if prev else None,
            previous_measured_slice_ms=(int(prev["frame_end_ns"]) - int(prev["frame_start_ns"])) / 1e6 if aligned and prev else None,
            begin_to_begin_ms=(int(row["frame_start_ns"]) - int(prev["frame_start_ns"])) / 1e6 if aligned and prev else None,
            outside_observed_slice_ms=int(row["frame_gap_ns"]) / 1e6 if aligned else None))
    return dict(schema=3, directory=str(directory), scenario=scenario, manifest=manifest,
        valid_for_comparison=bool(validity.get("ValidForComparison")) and not reasons,
        reasons=sorted(set(reasons)), frames=len(frames), frame_ms=stats([v for v in frame_times if v is not None]),
        frame_timing_metric="profile_begin_interval" if aligned else "bevy_time_delta_unaligned",
        bevy_frame_ms=stats([float(r["frame_ms"]) for r in frames]),
        measured_steps=frame_ticks, trace_steps=len(seen), core_tick_ms=stats(core_totals),
        core_ms_per_step=sum(float(r["simulation_core_ms"]) for r in frames) / frame_ticks if frame_ticks else None,
        realtime_ratio=frame_ticks / (seconds * (int(rate[1]) if rate else 30)) if seconds > 0 else None,
        state_hash=hash_match[1] if hash_match else None, actual_adapter=adapter[0] if adapter else None,
        actual_workers=[int(pools[1]), int(pools[2])] if pools else None,
        actual_render_context=context,
        os_cpu_window=os_window(directory, scenario, log, frames, roles),
        frame_alignment="shared_monotonic_epoch" if aligned else "legacy_frame_delta_lags_phase_columns",
        trace_alignment=dict(unmapped_ticks=unmapped, missing_clock_ticks=missing_clock,
                             identity_mismatches=identity_mismatches, mapped_steps_per_frame=mapped_steps if core_path.exists() and aligned else None),
        phases=phase_summary, slow_ticks=slow, slow_frames=expensive_frames,
        limitations=["wall_minus_cpu includes preemption, waits and coarse OS accounting; it is not lock wait time",
                    "worker_covered_wall is a union per OS thread, not CPU utilization; do not add it to parent wall",
                    "unit intervals are between consecutive callbacks and can enclose other work",
                    "render_prepare/render_extract are latest render-thread atomic observations and can belong to another frame",
                    "OS counters use coarse sampling; UTC alignment is approximate, and only full in-window intervals are included",
                    "callback roles are accumulated per OS ID; thread CPU accounting keeps creation-time lifetimes separate",
                    "OS thread CPU includes unmarked work too; a callback role does not attribute all of that thread's CPU to one subsystem",
                    "OS process polling can miss workloads shorter than its interval; stack/wait traces remain separate"])


def compare(a, b):
    problems = []
    for label, run in [("before", a), ("after", b)]:
        if not run["valid_for_comparison"]:
            problems.append(f"{label}:not_valid")
    keys = ["Scenario", "RequestedSeconds", "TargetFps", "RayonEnvironment", "NavigationWorkers", "GpuTimestamps",
            "CpuThreads", "CorePhases", "EnvironmentSampleIntervalMs", "InputHashes", "RequestedWindow"]
    for key in keys:
        if (a["manifest"] or {}).get(key) != (b["manifest"] or {}).get(key):
            problems.append(f"different:{key}")
    for key in ["measured_steps", "actual_adapter", "actual_workers", "actual_render_context", "frame_alignment", "frame_timing_metric", "state_hash"]:
        if a[key] != b[key] or (key in ("actual_adapter", "actual_workers", "state_hash") and a[key] is None):
            problems.append(f"different_or_missing:{key}")
    return {"eligible": not problems, "reasons": problems,
            "before_core_ms_per_step": a["core_ms_per_step"], "after_core_ms_per_step": b["core_ms_per_step"]}


def markdown(report):
    lines = ["# Range performance evidence", "",
             f"Comparison eligible: {report['valid_for_comparison']}. Reasons: {', '.join(report['reasons']) or 'none observed'}.",
             f"Frames: {report['frames']}; real steps: {report['measured_steps']}; traced steps: {report['trace_steps']}.",
             f"Frame alignment: {report['frame_alignment']}; duration metric: {report['frame_timing_metric']}. Core ms per actual step: {report['core_ms_per_step']}.", "",
             f"Observed render context: {report['actual_render_context']['initial'] or 'unavailable'}.",
             f"Render context records: {len(report['actual_render_context']['steps'])}; invalid change: {report['actual_render_context']['invalid_change']}.", "",
             "| Phase | Mean per actual traced tick, ms | p99 occurrence, ms | CPU counter samples |",
             "|---|---:|---:|---:|"]
    for p in report["phases"]:
        cpu = p["thread_cpu_occurrences"]
        lines.append(f"| {p['name']} | {p['mean_per_tick_ms']:.3f} | {p['wall_occurrences']['p99']:.3f} | {cpu['samples'] if cpu else 'unavailable'} |")
    lines += ["", "Slow ticks (CPU counter time is not lock wait time):", ""]
    for tick in report["slow_ticks"]:
        leaders = sorted(tick["phases"], key=lambda p: -p["wall_ms"])[:3]
        names = ", ".join(f"{p['name']}={p['wall_ms']:.3f}ms" for p in leaders)
        lines.append(f"- generation {tick['generation']}, tick {tick['tick']}: wall {tick['wall_ms']:.3f}ms, CPU {tick['thread_cpu_ms']}ms; {names}.")
        for job in tick["longest_tasks"][:3]:
            lines.append(f"  - task {job['name']}, unit {job['unit']}, OS thread {job['thread']}: wall {job['wall_ms']:.3f}ms, CPU {job['thread_cpu_ms']}ms (inclusive; not added to phase totals).")
    lines += ["", "Slow frames (monotonic begin-to-begin; previous measured slice plus current gap):", "",
              "| Row | Generation/tick | Observed interval, ms | Bevy delta, ms | Previous core, ms | Current core, ms | Previous measured slice, ms | Outside slices, ms |",
              "|---|---|---:|---:|---:|---:|---:|---:|"]
    def number(value):
        return "unavailable" if value is None else f"{value:.3f}"
    for frame in report["slow_frames"]:
        values = [number(frame[key]) for key in ("frame_delta_ms", "bevy_delta_ms", "previous_core_ms", "current_core_ms", "previous_measured_slice_ms", "outside_observed_slice_ms")]
        lines.append(f"| {frame['row']} | {frame['generation']}/{frame['tick']} | {' | '.join(values)} |")
    lines += ["", "Bevy delta may lag by multiple rows through the render pipeline; it is not used to select slow frames or attribute their phases. Outside slices is the monotonic gap from the previous measured end to the current measured start. It is not an identified GPU/lock wait. First-frame previous values and legacy clock fields remain unavailable."]
    lines += ["", *[f"- {s}." for s in report["limitations"]]]
    cpu = report["os_cpu_window"]
    if cpu["status"] == "aligned_intervals":
        lines += ["", f"OS CPU inside measurement window: {cpu['process_cpu_ms']:.3f}ms / {cpu['observed_ms']:.3f}ms; core equivalents={cpu['core_equivalents']}.", ""]
        for thread in cpu["sampled_threads"][:12]:
            lines.append(f"- OS thread {thread['os_thread']}: {thread['cpu_ms']:.3f}ms; observed callbacks: {', '.join(thread['observed_roles']) or 'unattributed'}.")
    if "comparison" in report:
        lines += ["", f"Pair comparison: {json.dumps(report['comparison'])}"]
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--scenario", default="world-battle")
    parser.add_argument("--compare", type=Path)
    parser.add_argument("--top", type=int, default=12)
    parser.add_argument("--require-valid", action="store_true")
    args = parser.parse_args()
    if args.top < 1:
        parser.error("--top must be positive")
    result = analyze(args.directory, args.scenario, args.top)
    if args.compare:
        result["comparison"] = compare(analyze(args.compare, args.scenario, args.top), result)
    prefix = args.directory / f"{args.scenario}.analysis"
    prefix.with_suffix(".analysis.json").write_text(json.dumps(result, indent=2), encoding="utf-8")
    prefix.with_suffix(".analysis.md").write_text(markdown(result), encoding="utf-8")
    print(f"Steps={result['measured_steps']} trace={result['trace_steps']} eligible={result['valid_for_comparison']} reasons={result['reasons']}")
    if args.require_valid and (not result["valid_for_comparison"] or not result.get("comparison", {"eligible": True})["eligible"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
