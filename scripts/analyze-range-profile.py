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


def camera_tour_capture(directory, scenario, log, manifest, frame_count, reasons):
    declared = bool((manifest or {}).get("CameraTour"))
    signature = "PROFILE camera_tour_schema=1 camera_tour_clock=wall camera_tour_route=bases_pan_zoom_v1"
    observed = bool(re.search(r"^PROFILE camera_tour_schema=", log, re.M))
    if declared != observed or (observed and signature not in log.splitlines()):
        reasons.append("camera_tour_mode_mismatch")
    if not declared:
        return None
    path = Path(directory) / f"{scenario}.camera.csv"
    if not path.exists():
        reasons.append("camera_tour_artifact_missing")
        return None
    rows = list(read_csv(path))
    if len(rows) != frame_count or not rows:
        reasons.append("camera_tour_frame_count_mismatch")
    times = [float(r["seconds"]) for r in rows]
    ticks = [int(r["tick"]) for r in rows]
    segments = [int(r["segment"]) for r in rows]
    if (any(not math.isfinite(v) or v < 0 for v in times) or
            times != sorted(times) or ticks != sorted(ticks) or segments != sorted(segments) or
            any(r["blocked"] not in ("true", "false") for r in rows)):
        reasons.append("camera_tour_clock_invalid")
    no_tick_seconds = [values[-1][1] - values[0][1]
                       for _, group in itertools.groupby(zip(ticks, times), key=lambda p: p[0])
                       for values in [list(group)] if values]
    work = {}
    for name in ("observe_ms", "stream_ms"):
        if rows and name in rows[0]:
            values = [float(r[name]) for r in rows]
            if any(not math.isfinite(v) or v < 0 for v in values):
                reasons.append("camera_tour_work_invalid")
            else:
                work[name] = stats(values)
    return dict(schema=1, clock="wall", route="bases_pan_zoom_v1", frames=len(rows),
                segments=sorted(set(segments)), blocked_frames=sum(r["blocked"] == "true" for r in rows),
                streaming_frames=sum(bool(r["progress"]) for r in rows),
                max_no_tick_seconds=max(no_tick_seconds, default=0),
                work_per_frame=work,
                window_origins=sorted({(int(r["origin_x"]), int(r["origin_z"])) for r in rows}))


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
                physics = key[1].startswith("physics_")
                jobs.append(dict(name=key[1], unit=None if physics else key[0],
                                 domain="physics" if physics else "actor",
                                 job_slot=key[0] if physics else None, thread=key[2],
                                 start=int(a["offset_ns"]), end=int(row["offset_ns"]),
                                 **interval(a, row)))
    if any(pending.values()):
        raise ValueError("Unfinished worker task")
    # The tail is explicit; phase totals partition the caller's tick only.
    phases.append(dict(name="unmarked_tail", **interval(last, end)))
    families = {}
    for job in jobs:
        family = families.setdefault(job["name"], dict(jobs=0, inclusive_wall_ms=0.0,
            max_wall_ms=0.0, sampled_cpu_ms=0.0, cpu_samples=0, cycles=0, cycle_samples=0))
        family["jobs"] += 1
        family["inclusive_wall_ms"] += job["wall_ms"]
        family["max_wall_ms"] = max(family["max_wall_ms"], job["wall_ms"])
        if job["thread_cpu_ms"] is not None:
            family["sampled_cpu_ms"] += job["thread_cpu_ms"]
            family["cpu_samples"] += 1
        if job["cycles"] is not None:
            family["cycles"] += job["cycles"]
            family["cycle_samples"] += 1
    return dict(generation=int(begin["generation"]), tick=int(begin["tick"]), **total,
                profile_begin_ns=optional_int(begin, "profile_offset_ns"),
                profile_end_ns=optional_int(end, "profile_offset_ns"), phases=phases,
                unit_intervals=sorted(units, key=lambda r: -r["wall_ms"])[:8],
                longest_tasks=sorted(jobs, key=lambda r: -r["wall_ms"])[:8],
                task_count=len(jobs), unidentified_task_events=unidentified_tasks,
                task_families=families,
                physics_samples=[job for job in jobs if job["domain"] == "physics"],
                worker_families={thread: sorted({j["name"] for j in jobs if j["thread"] == thread})
                                 for thread in {j["thread"] for j in jobs}},
                worker_covered_wall_ms=union_by_thread(jobs))


# Exclusive Update-system boundaries; the other columns are nested details or
# asynchronous render-thread observations, so they must not be added here.
SLICE_PHASES = ("simulation_input_ms", "unit_mesh_ms", "fragments_ms", "presentation_ms")
SLICE_DETAILS = ("simulation_core_ms", "occupancy_ms", "physics_ms", "terrain_ms", "atmosphere_ms")


def measured_slice(frames, idx, aligned):
    row = frames[idx]
    steps = int(row["simulation_ticks"])
    tick = int(row["tick"])
    def optional_ms(key):
        value = row.get(key)
        return float(value) if value not in (None, "") else None
    wall = (int(row["frame_end_ns"]) - int(row["frame_start_ns"])) / 1e6 if aligned else None
    following = frames[idx + 1] if idx + 1 < len(frames) and aligned else None
    core = float(row["simulation_core_ms"])
    return dict(row=idx, generation=int(row["generation"]), end_tick=tick,
                first_tick=(tick - steps + 1) % (1 << 32) if steps else None,
                simulation_ticks=steps, measured_wall_ms=wall, core_ms=core,
                single_tick_ms=core if steps == 1 else None,
                mean_core_ms_per_step=core / steps if steps else None,
                # The observed interval on the following row contains this
                # slice, not that following row's core or presentation work.
                following_interval_row=idx + 1 if following else None,
                following_interval_ms=(int(following["frame_start_ns"]) - int(row["frame_start_ns"])) / 1e6 if following else None,
                following_gap_ms=int(following["frame_gap_ns"]) / 1e6 if following else None,
                phases={key: optional_ms(key) for key in SLICE_PHASES},
                nested_details={key: optional_ms(key) for key in SLICE_DETAILS})


def stall_episodes(slices, top, threshold_ms=50.0):
    """Adjacent expensive main slices/gaps; wall evidence, never a wait cause."""
    episodes = []
    count = 0
    active = None
    def retain(episode):
        nonlocal count
        count += 1
        # Measured slices and following gaps are disjoint on the shared clock.
        episode["covered_wall_ms"] = episode["measured_wall_ms"] + episode["following_gap_ms"]
        episodes.append(episode)
        episodes.sort(key=lambda r: -r["covered_wall_ms"])
        del episodes[top:]
    for sample in slices:
        wall, gap = sample["measured_wall_ms"], sample["following_gap_ms"]
        interval = sample["following_interval_ms"]
        slow = ((wall is not None and wall >= threshold_ms) or (gap is not None and gap >= threshold_ms) or
                (interval is not None and interval >= threshold_ms))
        if active and (not slow or sample["generation"] != active["generation"]):
            retain(active)
            active = None
        if not slow:
            continue
        if active is None:
            active = dict(generation=sample["generation"], first_row=sample["row"],
                          last_row=sample["row"], first_tick=sample["first_tick"],
                          end_tick=sample["end_tick"], slices=0, simulation_ticks=0,
                          measured_wall_ms=0.0, following_gap_ms=0.0, core_ms=0.0,
                          longest_single_tick_ms=None)
        if active["first_tick"] is None:
            active["first_tick"] = sample["first_tick"]
        active["last_row"], active["end_tick"] = sample["row"], sample["end_tick"]
        active["slices"] += 1
        active["simulation_ticks"] += sample["simulation_ticks"]
        active["measured_wall_ms"] += wall or 0.0
        active["following_gap_ms"] += gap or 0.0
        active["core_ms"] += sample["core_ms"]
        if sample["single_tick_ms"] is not None:
            active["longest_single_tick_ms"] = max(active["longest_single_tick_ms"] or 0.0, sample["single_tick_ms"])
    if active:
        retain(active)
    return dict(threshold_ms=threshold_ms, count=count, retained=episodes)


def summarize_slices(frames, aligned, top):
    walls, singles, slow = [], [], []
    multi = 0
    totals = dict.fromkeys(SLICE_PHASES, 0.0)
    def samples():
        nonlocal multi
        for idx in range(len(frames)):
            sample = measured_slice(frames, idx, aligned)
            if sample["measured_wall_ms"] is not None:
                walls.append(sample["measured_wall_ms"])
                slow.append(sample)
                slow.sort(key=lambda r: -r["measured_wall_ms"])
                del slow[top:]
            if sample["single_tick_ms"] is not None:
                singles.append(sample["single_tick_ms"])
            multi += sample["simulation_ticks"] > 1
            for key, value in sample["phases"].items():
                totals[key] = totals[key] + value if totals[key] is not None and value is not None else None
            yield sample
    episodes = stall_episodes(samples(), top)
    summary = dict(measured_wall_ms=stats(walls), single_tick_ms=stats(singles),
                   multi_step_slices=multi, phase_totals_ms=totals)
    return summary, slow, episodes if aligned else None


def frame_cadence(frames, aligned):
    """Partition begin intervals by their preceding slice, never current work.

    Only offline CSV clocks are read. Gaps include other schedules, render and
    preemption; this partition identifies neither GPU time nor wait causes.
    """
    if not aligned or not frames:
        return None
    groups = {}
    for idx in range(len(frames) - 1):
        row, following = frames[idx], frames[idx + 1]
        key = int(row["generation"]), int(row["simulation_ticks"])
        values = groups.setdefault(key, dict(interval=[], update=[], gap=[], core=[], crossings=0))
        start, end = int(row["frame_start_ns"]), int(row["frame_end_ns"])
        next_start = int(following["frame_start_ns"])
        values["interval"].append(next_start - start)
        values["update"].append(end - start)
        values["gap"].append(next_start - end)
        values["core"].append(float(row["simulation_core_ms"]))
        values["crossings"] += int(row["generation"]) != int(following["generation"])
    covered_ns = sum(sum(v["interval"]) for v in groups.values())
    rows = []
    for (generation, steps), values in sorted(groups.items()):
        count = len(values["interval"])
        totals = {k + "_total_ms": sum(values[k]) / 1e6 for k in ("interval", "update", "gap")}
        rows.append(dict(generation=generation, steps_per_slice=steps,
            intervals=count, completed_steps=count * steps,
            generation_crossings=values["crossings"],
            interval_ms=stats([v / 1e6 for v in values["interval"]]),
            update_ms=stats([v / 1e6 for v in values["update"]]),
            following_gap_ms=stats([v / 1e6 for v in values["gap"]]),
            core_ms=stats(values["core"]), core_total_ms=sum(values["core"]),
            wall_fraction=sum(values["interval"]) / covered_ns if covered_ns else None,
            **totals))
    terminal = measured_slice(frames, len(frames) - 1, True)
    return dict(intervals=len(frames) - 1, covered_wall_ms=covered_ns / 1e6,
        update_total_ms=sum(r["update_total_ms"] for r in rows),
        following_gap_total_ms=sum(r["gap_total_ms"] for r in rows),
        core_total_ms=sum(r["core_total_ms"] for r in rows),
        completed_steps=sum(r["completed_steps"] for r in rows), groups=rows,
        terminal_slice={k: terminal[k] for k in ("row", "generation", "simulation_ticks", "measured_wall_ms", "core_ms")})


def analyze_stall_trace(directory, scenario, log, manifest, frames, starts, roles, top):
    header = re.search(r"^PROFILE stall_capture_schema=(\d+) stall_capture_mode=(\w+) threshold_ms=(\d+) pre_ticks=(\d+) post_ticks=(\d+) max_windows=(\d+)(?: cpu_sampling=(\w+))?$", log, re.M)
    declared = int((manifest or {}).get("CoreTriggerMilliseconds") or 0)
    path = directory / f"{scenario}.stall.csv"
    if not header:
        return None, ["stall_capture_metadata_missing"] if declared or path.exists() else []
    version, mode, threshold, pre, post, maximum, cpu_sampling = header.groups()
    cpu_sampling = cpu_sampling or "unspecified"
    threshold, pre, post, maximum = map(int, (threshold, pre, post, maximum))
    if version not in ("1", "2", "3") or mode != "phase_ring" or min(threshold, maximum) <= 0 or cpu_sampling not in ("unspecified", "tick_boundaries"):
        raise ValueError("Unsupported stall capture contract")
    summary = re.search(r"^PROFILE stall_windows=(\d+) stall_slow_ticks=(\d+) stall_unarmed_slow_ticks=(\d+) stall_retained_ticks=(\d+)$", log, re.M)
    if not summary:
        raise ValueError("Stall capture summary missing")
    windows, slow_count, unarmed, retained = map(int, summary.groups())
    extra = pre if version == "3" else 0
    if windows > maximum or (version == "1" and slow_count < windows) or unarmed > slow_count or retained > maximum * (pre + 1 + post) + pre + extra:
        raise ValueError("Stall capture budget/count mismatch")
    if not all(start is not None for start in starts):
        raise ValueError("Stall capture requires aligned frame clocks")
    frame_signals, signal_count, dropped_signals, unarmed_signals = [], None, None, None
    peak = None
    if version in ("2", "3"):
        counts = re.search(r"^PROFILE stall_frame_signals=(\d+) stall_retained_frame_signals=(\d+) stall_dropped_frame_signals=(\d+) stall_unarmed_frame_signals=(\d+)$", log, re.M)
        if not counts:
            raise ValueError("Frame signal summary missing")
        signal_count, saved_signals, dropped_signals, unarmed_signals = map(int, counts.groups())
        matches = list(re.finditer(r"^PROFILE stall_frame_signal source=(gap|update|interval) generation=(\d+) tick=(\d+) begin_ns=(\d+) end_ns=(\d+) window=(\d+)$", log, re.M))
        expected = sum(int(f["frame_end_ns"]) - int(f["frame_start_ns"]) >= threshold * 1000000 for f in frames)
        expected += sum(int(f["frame_gap_ns"]) >= threshold * 1000000 for f in frames[1:])
        expected += sum(starts[i] - starts[i - 1] >= threshold * 1000000 for i in range(1, len(starts)))
        if (signal_count != expected or len(matches) != saved_signals or saved_signals != min(signal_count, 64) or
                saved_signals + dropped_signals != signal_count or unarmed_signals > signal_count or windows > slow_count + signal_count):
            raise ValueError("Frame signal budget/count mismatch")
        def signal(source, values):
            generation, tick, begin, end, window = map(int, values)
            idx = bisect.bisect_left(starts, begin if source == "update" else end)
            if idx >= len(frames) or (source != "update" and idx == 0):
                raise ValueError("Frame signal outside frame clocks")
            frame = frames[idx]
            identity_frame = frame if source == "update" else frames[idx - 1]
            expected_begin = int(identity_frame["frame_end_ns" if source == "gap" else "frame_start_ns"])
            expected_end = int(frame["frame_end_ns"] if source == "update" else frame["frame_start_ns"])
            if (begin != expected_begin or end != expected_end or end - begin < threshold * 1000000 or
                    generation != int(identity_frame["generation"]) or tick != int(identity_frame["tick"]) or
                    window > windows):
                raise ValueError("Frame signal clock/identity mismatch")
            return dict(source=source, generation=generation, tick=tick, frame_row=idx,
                        begin_ns=begin, end_ns=end, wall_ms=(end - begin) / 1e6, window=window)
        identities = set()
        last_end = -1
        for match in matches:
            row = signal(match[1], match.groups()[1:])
            identity = (row["source"], row["begin_ns"], row["end_ns"])
            if row["end_ns"] < last_end or identity in identities:
                raise ValueError("Frame signal clock/identity mismatch")
            identities.add(identity)
            last_end = row["end_ns"]
            frame_signals.append(row)
        saved_unarmed = sum(s["window"] == 0 for s in frame_signals)
        if not saved_unarmed <= unarmed_signals <= saved_unarmed + dropped_signals:
            raise ValueError("Frame signal unarmed count mismatch")
        if version == "3":
            peaks = list(re.finditer(r"^PROFILE stall_peak_signal source=(gap|update|interval) generation=(\d+) tick=(\d+) begin_ns=(\d+) end_ns=(\d+) window=(\d+) context_ticks=(\d+)$", log, re.M))
            if len(peaks) != int(unarmed_signals > 0):
                raise ValueError("Late peak metadata missing or duplicated")
            if peaks:
                match = peaks[0]
                peak = signal(match[1], match.groups()[1:6])
                peak["context_tick_budget"] = int(match[7])
                if peak["window"] != 0 or peak["context_tick_budget"] != pre or any(
                        (s["window"] == 0 and s["wall_ms"] > peak["wall_ms"]) or
                        (s["source"] == peak["source"] and s["begin_ns"] == peak["begin_ns"] and s["end_ns"] == peak["end_ns"] and s["window"] != 0)
                        for s in frame_signals):
                    raise ValueError("Late peak contract mismatch")
    issues = []
    if declared != threshold or (manifest or {}).get("CorePhases"):
        issues.append("stall_capture_mode_mismatch")
    if retained and not path.exists():
        raise ValueError("Stall capture file missing")
    ticks, identities, mapped, last_begin = [], set(), {}, -1
    for key, group in itertools.groupby(read_csv(path) if path.exists() else [], key=lambda r: (r["generation"], r["tick"])):
        if key in identities:
            raise ValueError("Duplicated stall capture tick")
        identities.add(key)
        rows = list(group)
        if any(row["lane"] not in ("tick", "phase", "detail") for row in rows):
            raise ValueError("Worker/unit marks in phase-only stall capture")
        if cpu_sampling == "tick_boundaries" and any(
            row["lane"] != "tick" and any(row.get(k) not in (None, "") for k in ("thread_cpu_ns", "cpu_cycles")) for row in rows
        ):
            raise ValueError("Unexpected phase CPU counters in boundary-only capture")
        tick = summarize_tick(rows)
        tick.pop("physics_samples")
        begin, end = tick["profile_begin_ns"], tick["profile_end_ns"]
        if begin is None or end is None or begin < last_begin:
            raise ValueError("Stall capture clock missing or regressed")
        last_begin = begin
        idx = bisect.bisect_right(starts, begin) - 1
        if idx < 0 or end > int(frames[idx]["frame_end_ns"]):
            raise ValueError("Stall capture outside frame slice")
        frame = frames[idx]
        steps = int(frame["simulation_ticks"])
        distance = (tick["tick"] - int(frame["tick"]) + steps - 1) % (1 << 32)
        if tick["generation"] != int(frame["generation"]) or distance >= steps:
            raise ValueError("Stall capture frame/tick identity mismatch")
        mapped[idx] = mapped.get(idx, 0) + 1
        if mapped[idx] > steps:
            raise ValueError("Excess stall capture ticks in frame")
        tick["frame_row"] = idx
        if tick["os_thread"]:
            roles.setdefault(tick["os_thread"], set()).add("tick_caller")
        ticks.append(tick)
    if len(ticks) != retained or slow_count > sum(int(f["simulation_ticks"]) for f in frames):
        raise ValueError("Stall capture retained/observed count mismatch")
    if peak is not None:
        idx = peak["frame_row"] - int(peak["source"] != "update")
        expected = min(int(frames[idx]["simulation_ticks"]), peak["context_tick_budget"])
        if mapped.get(idx, 0) < expected:
            raise ValueError("Late peak phase context missing")
    return dict(schema=int(version), mode=mode, cpu_sampling=cpu_sampling, threshold_ms=threshold, pre_ticks=pre, post_ticks=post,
                max_windows=maximum, windows=windows, observed_slow_ticks=slow_count,
                unarmed_slow_ticks=unarmed, retained_ticks=retained, partial=True,
                observed_frame_signals=signal_count, dropped_frame_signals=dropped_signals, unarmed_frame_signals=unarmed_signals, frame_signals=frame_signals,
                late_peak=peak,
                ticks=ticks, slow_ticks=sorted(ticks, key=lambda t: -t["wall_ms"])[:top]), issues


def incident_coverage(episodes, frames, mapped_steps, mapped_wall, mapped_jobs, capture):
    """Context/quiet-tail ticks do not explain an incident unless clocks overlap."""
    if episodes is None:
        return
    if capture is not None:
        mapped_steps, mapped_wall = [0] * len(frames), [0.0] * len(frames)
        for tick in capture["ticks"]:
            idx = tick["frame_row"]
            mapped_steps[idx] += 1
            mapped_wall[idx] += tick["wall_ms"]
    for episode in episodes["retained"]:
        indices = range(episode["first_row"], episode["last_row"] + 1)
        traced = sum(mapped_steps[i] for i in indices)
        total = episode["simulation_ticks"]
        status = "no_core_ticks" if total == 0 else "complete" if traced == total else "partial" if traced else "none"
        episode["diagnostic_coverage"] = dict(
            tick_phases=status, traced_steps=traced, untraced_steps=total - traced,
            captured_tick_wall_ms=sum(mapped_wall[i] for i in indices),
            non_core_slice_ms=max(0.0, episode["measured_wall_ms"] - episode["core_ms"]),
            outside_update_ms=episode["following_gap_ms"],
            worker_scopes_available=capture is None and any(mapped_jobs[i] for i in indices),
            os_stacks_available=False,
            cause_identified=False)


RENDER_SPAN_LIMIT = 100_000
RENDER_MAX_SPANS = 1_000_000
RENDER_STAGES = {"pipeline_handoff", "extract_callback", "render_schedule", "render_prepare",
                 "render_system", "render_graph", "submit_present_tail"}


GAS_STAGES = {"gas_extract", "gas_prepare", "gas_views_uniform", "gas_terrain_upload", "gas_bricks", "gas_regions"}
GAS_WORK_FIELDS = ("patches", "words", "distant_words", "regions", "new_regions", "deposits", "terrain_writes", "terrain_bytes", "terrain_submits")
GAS_EXTRACT_FIELDS = ("dirty_ranges", "range_ns", "pack_ns", "clone_ns")


def journal_maintenance(log):
    rows = re.findall(r"^PROFILE terrain_journal_compact_calls=(\d+) total_ms=([\d.]+) peak_ms=([\d.]+)$", log, re.M)
    if not rows and "PROFILE terrain_journal_compact_calls=" not in log:
        return None  # Legacy run did not measure maintenance, not zero cost.
    if len(rows) != 1:
        raise ValueError("Terrain journal metadata missing or duplicated")
    calls, total, peak = int(rows[0][0]), float(rows[0][1]), float(rows[0][2])
    if not math.isfinite(total) or not math.isfinite(peak) or total < peak or (calls == 0 and total != 0):
        raise ValueError("Terrain journal counters invalid")
    return dict(calls=calls, total_ms=total, peak_ms=peak, mean_ms=total/calls if calls else None,
                scope="main Update wall, already included in slice; never add to frame time")


def merged_intervals(intervals):
    result = []
    for a, b in sorted(intervals):
        if result and a <= result[-1][1]:
            result[-1][1] = max(result[-1][1], b)
        else:
            result.append([a, b])
    return result


def interval_index(intervals):
    starts, ends, prefix = [], [], [0]
    for a, b in intervals:
        starts.append(a); ends.append(b); prefix.append(prefix[-1] + b - a)
    return starts, ends, prefix


def overlap_ns(index, a, b):
    starts, ends, prefix = index
    first, last = bisect.bisect_right(ends, a), bisect.bisect_left(starts, b)
    if first >= last:
        return 0
    return prefix[last] - prefix[first] - max(0, a - starts[first]) - max(0, ends[last - 1] - b)


def analyze_render_timeline(directory, scenario, log, manifest, frames, top, roles=None):
    """Source rows are frozen at extraction; time overlap is not a causal wait."""
    path = directory / f"{scenario}.render.csv"
    metadata = re.findall(r"^PROFILE render_timeline_schema=(\d+) render_span_limit=(\d+) render_spans_started=(\d+) render_spans_retained=(\d+) render_spans_dropped=(\d+) render_spans_incomplete=(\d+) render_cutoff_ns=(\d+) render_pipelined=(true|false)(?: render_gas_details=(true|false))?(?: render_span_bytes=(\d+))?$", log, re.M)
    declared = bool((manifest or {}).get("RenderTimeline"))
    if not path.exists() and not metadata and not declared:
        return None, []
    if not path.exists() or len(metadata) != 1:
        return None, ["render_timeline_artifact_or_metadata_missing"]
    if not declared:
        return None, ["render_timeline_mode_mismatch"]
    schema, limit, started, retained, dropped, incomplete, cutoff = map(int, metadata[0][:7])
    span_bytes = int(metadata[0][9]) if metadata[0][9] else None
    budget_valid = (limit == RENDER_SPAN_LIMIT and span_bytes is None) if schema < 4 else (
        1 <= limit <= RENDER_MAX_SPANS and span_bytes is not None and 1 <= span_bytes <= 1024)
    if schema not in (1, 2, 3, 4) or (schema == 1 and metadata[0][8]) or (schema >= 2 and not metadata[0][8]) or not budget_valid or retained != min(started, limit) or started != retained + dropped or incomplete > retained or retained == 0:
        raise ValueError("Render timeline metadata budget/count mismatch")
    issues = []
    if schema == 4 and (manifest or {}).get("RenderSpanLimit") != limit:
        issues.append("render_timeline_budget_manifest_mismatch")
    if schema == 4 and dropped:
        issues.append("render_timeline_truncated")
    if not all(r.get("frame_end_ns") for r in frames) or cutoff < int(frames[-1]["frame_end_ns"]):
        raise ValueError("Render timeline cutoff/frame clock mismatch")
    gas_details = metadata[0][8] == "true"
    allowed_stages = RENDER_STAGES | GAS_STAGES if gas_details else RENDER_STAGES
    groups, intervals, pending = {}, {}, 0
    count, last_begin = 0, -1
    for row in read_csv(path):
        count += 1
        if count > retained:
            raise ValueError("Render timeline count mismatch")
        idx = int(row["frame_row"])
        stage = row["stage"]
        a, b = int(row["begin_ns"]), optional_int(row, "end_ns")
        if idx < 0 or idx >= len(frames) or stage not in allowed_stages:
            raise ValueError("Render timeline source/stage invalid")
        frame = frames[idx]
        if int(row["generation"]) != int(frame["generation"]) or int(row["tick"]) != int(frame["tick"]):
            raise ValueError("Render timeline identity mismatch")
        if a < int(frame["frame_end_ns"]) or a < last_begin or a > cutoff or (b is not None and (b < a or b > cutoff)):
            raise ValueError("Render timeline clock mismatch")
        last_begin = a
        if not re.fullmatch(r"ThreadId\(\d+\)", row["begin_thread"]) or (
            b is None and row["end_thread"] or b is not None and row["end_thread"] != row["begin_thread"]):
            raise ValueError("Render timeline driving thread mismatch")
        if b is None:
            pending += 1
        group = groups.setdefault(idx, {})
        if stage in group:
            raise ValueError("Render timeline duplicate stage/source")
        fields = ("begin_os_thread", "end_os_thread", "begin_cpu_ns", "end_cpu_ns", "begin_cycles", "end_cycles")
        if not all(field in row for field in fields):
            raise ValueError("Render timeline counter columns missing")
        counters = {key: optional_int(row, key) for key in fields}
        if any(v is not None and v < 0 for v in counters.values()) or any(
            counters[key] == 0 for key in ("begin_os_thread", "end_os_thread")) or any(
            counters[side + "_os_thread"] is None and (counters[side + "_cpu_ns"] is not None or counters[side + "_cycles"] is not None)
            for side in ("begin", "end")):
            raise ValueError("Render timeline counter identity invalid")
        if b is None and any(counters[key] is not None for key in ("end_os_thread", "end_cpu_ns", "end_cycles")):
            raise ValueError("Render timeline open span has end counters")
        same = counters["begin_os_thread"] is not None and counters["begin_os_thread"] == counters["end_os_thread"]
        if counters["begin_os_thread"] is not None and counters["end_os_thread"] is not None and not same:
            raise ValueError("Render timeline OS thread changed")
        def delta(name):
            x, y = counters["begin_" + name], counters["end_" + name]
            if not same or x is None or y is None:
                return None
            if y < x:
                raise ValueError("Render timeline CPU counter regressed")
            return y - x
        cpu, cycles = delta("cpu_ns"), delta("cycles")
        group[stage] = dict(begin_ns=a, end_ns=b, thread=row["begin_thread"],
                           driver_cpu_ms=cpu / 1e6 if cpu is not None else None, cycles=cycles)
        if schema >= 2:
            work_fields = GAS_WORK_FIELDS + (GAS_EXTRACT_FIELDS if schema >= 3 else ())
            detail_fields = ("completed", "input_frame_row", *work_fields)
            if not all(key in row for key in detail_fields):
                raise ValueError("Render timeline detail columns missing")
            is_leaf = stage in GAS_STAGES
            if not is_leaf or b is None:
                if any(row[key] for key in detail_fields):
                    raise ValueError("Render timeline unexpected detail payload")
            else:
                if row["completed"] not in ("0", "1") or any(row[key] == "" for key in work_fields):
                    raise ValueError("Render timeline incomplete detail payload")
                work = {key: int(row[key]) for key in work_fields}
                payload_row = optional_int(row, "input_frame_row")
                if any(v < 0 for v in work.values()) or (payload_row is not None and not 0 <= payload_row <= idx):
                    raise ValueError("Render timeline detail payload invalid")
                if stage == "gas_extract" and row["completed"] == "1" and payload_row != idx:
                    raise ValueError("Render timeline extraction source identity mismatch")
                if stage not in ("gas_prepare", "gas_terrain_upload") and any(work[key] for key in ("terrain_writes", "terrain_bytes", "terrain_submits")):
                    raise ValueError("Render timeline transfer scope mismatch")
                if schema >= 3 and (stage != "gas_extract" and any(work[key] for key in GAS_EXTRACT_FIELDS) or
                        sum(work[key] for key in GAS_EXTRACT_FIELDS[1:]) > b - a):
                    raise ValueError("Render timeline extraction phase scope mismatch")
                group[stage].update(completed=row["completed"] == "1", work=work, input_frame_row=payload_row)
        if roles is not None and counters["begin_os_thread"] is not None:
            roles.setdefault(str(counters["begin_os_thread"]), set()).add(stage if stage.startswith("render_") else "render_" + stage)
        if b is not None:
            intervals.setdefault(stage, []).append((a, b))
    if count != retained or pending != incomplete:
        raise ValueError("Render timeline count mismatch")
    # Containment only for nested same-thread scopes. A renderer may start
    # before the main-thread handoff returns; those two are intentionally async.
    parents = {"extract_callback": "pipeline_handoff", "render_prepare": "render_schedule",
               "render_system": "render_schedule", "render_graph": "render_system",
               "submit_present_tail": "render_system"}
    gas_parents = {"gas_extract": "extract_callback", "gas_prepare": "render_prepare",
                   **{stage: "gas_prepare" for stage in GAS_STAGES - {"gas_extract", "gas_prepare"}}}
    parents.update(gas_parents)
    for idx, group in groups.items():
        for child_name, parent_name in parents.items():
            child, parent = group.get(child_name), group.get(parent_name)
            if child is None or parent is None:
                continue
            if (child_name not in gas_parents and child["thread"] != parent["thread"]) or child["begin_ns"] < parent["begin_ns"] or (
                parent["end_ns"] is not None and (child["end_ns"] is None or child["end_ns"] > parent["end_ns"])):
                raise ValueError("Render timeline nested scope mismatch")
        for stage in GAS_STAGES - {"gas_extract", "gas_prepare"}:
            if stage in group and "gas_prepare" in group and group[stage]["thread"] != group["gas_prepare"]["thread"]:
                raise ValueError("Render timeline gas call thread mismatch")
        leafs = ("gas_views_uniform", "gas_terrain_upload", "gas_bricks", "gas_regions")
        for before, after in (("extract_callback", "render_schedule"), ("render_prepare", "render_system"),
                              ("render_graph", "submit_present_tail"), *zip(leafs, leafs[1:])):
            x, y = group.get(before), group.get(after)
            if x and y and (x["end_ns"] is None or x["end_ns"] > y["begin_ns"]):
                raise ValueError("Render timeline stage order mismatch")
    # Validate completeness before the cutoff, except the trailing active frame
    # and explicitly dropped records. Missing interior stages are corrupt data.
    pipelined = metadata[0][7] == "true"
    expected = RENDER_STAGES if pipelined else RENDER_STAGES - {"pipeline_handoff"}
    if not pipelined and any("pipeline_handoff" in g for g in groups.values()):
        raise ValueError("Render timeline pipeline mode mismatch")
    def source_complete(group):
        coarse = expected.issubset(group) and all(group[s]["end_ns"] is not None for s in expected)
        if not gas_details:
            return coarse
        prepare = group.get("gas_prepare")
        if prepare is None or prepare["end_ns"] is None:
            return False
        if not prepare.get("completed"):
            return coarse  # Explicitly aborted call, never invent missing phases.
        required = GAS_STAGES - {"gas_extract"}
        return coarse and required.issubset(group) and all(group[s]["end_ns"] is not None and group[s].get("completed") for s in required)
    complete = [idx for idx, group in groups.items() if source_complete(group)]
    if not dropped and groups:
        for idx, group in groups.items():
            schedule = group.get("render_schedule")
            if schedule and schedule["end_ns"] is not None and idx not in complete:
                raise ValueError("Render timeline completed schedule coverage missing")
        for idx in range(max(groups)):
            if idx not in complete:
                raise ValueError("Render timeline interior coverage missing")
    if gas_details and not dropped:
        for group in groups.values():
            for stage in GAS_STAGES - {"gas_extract"}:
                call = group.get(stage)
                if not call or not call.get("completed") or call.get("input_frame_row") is None:
                    continue
                source = groups.get(call["input_frame_row"], {}).get("gas_extract")
                if not source or not source.get("completed"):
                    raise ValueError("Render timeline payload source extraction missing")
                for field in ("patches", "words", "distant_words", "regions"):
                    if source["work"][field] != call["work"][field]:
                        raise ValueError("Render timeline extracted payload facts mismatch")
    merged = {stage: merged_intervals(values) for stage, values in intervals.items()}
    total = lambda values: sum(b - a for a, b in values) / 1e6
    leaders = []
    for idx, group in groups.items():
        values = {stage: (span["end_ns"] - span["begin_ns"]) / 1e6
                  for stage, span in group.items() if span["end_ns"] is not None}
        unobserved = {}
        if gas_details:
            for parent, child in (("render_prepare", "gas_prepare"), ("extract_callback", "gas_extract")):
                p, c = group.get(parent), group.get(child)
                if p and p["end_ns"] is not None and (c is None or c["end_ns"] is not None) and not dropped:
                    own = 0 if c is None else c["end_ns"] - c["begin_ns"]
                    unobserved[parent] = (p["end_ns"] - p["begin_ns"] - own) / 1e6
                else:
                    unobserved[parent] = None
        leaders.append(dict(source_row=idx, other_or_unobserved_ms=unobserved, generation=int(frames[idx]["generation"]),
                            tick=int(frames[idx]["tick"]), spans_ms=values,
                            leaf_work={stage: dict(input_frame_row=v.get("input_frame_row"), completed=v.get("completed"), work=v.get("work"))
                                       for stage, v in group.items() if stage in GAS_STAGES and v["end_ns"] is not None},
                            driver_counters={stage: dict(cpu_ms=v["driver_cpu_ms"], cycles=v["cycles"])
                                             for stage, v in group.items() if v["end_ns"] is not None}))
    leaders.sort(key=lambda r: -r["spans_ms"].get("render_schedule", 0))
    # Rank actual calls independently: a slow extract need not have a slow
    # render schedule, and would disappear from schedule-only leaders.
    gas_leaders = {stage: sorted((frame for frame in leaders if stage in frame["leaf_work"]),
                                key=lambda frame: -frame["spans_ms"][stage])[:top]
                   for stage in ("gas_extract", "gas_prepare")} if gas_details else {}
    gaps = []
    indexed = {stage: interval_index(values) for stage, values in merged.items()}
    covered_index = interval_index(merged_intervals(merged.get("pipeline_handoff", []) +
        merged.get("extract_callback", []) + merged.get("render_schedule", [])))
    gap_rows = sorted(range(1, len(frames)), key=lambda i:
        -(int(frames[i]["frame_start_ns"]) - int(frames[i - 1]["frame_end_ns"])))[:top]
    for idx in gap_rows:
        a, b = int(frames[idx - 1]["frame_end_ns"]), int(frames[idx]["frame_start_ns"])
        spans = {stage: overlap_ns(values, a, b) / 1e6 for stage, values in indexed.items()}
        gaps.append(dict(row=idx, gap_ms=(b - a) / 1e6, overlap_ms=spans,
                         unobserved_or_other_ms=max(0, b - a - overlap_ns(covered_index, a, b)) / 1e6))
    gaps.sort(key=lambda r: -r["gap_ms"])
    gas_summary = None
    if gas_details:
        actual_extracts = sum("gas_extract" in g for g in groups.values())
        gas_summary = dict(actual_extract_calls=actual_extracts,
            source_rows_without_extract_call=len(groups) - actual_extracts,
            aborted_prepare_calls=sum(g.get("gas_prepare", {}).get("completed") is False for g in groups.values()),
            unknown_payload_source_calls=sum("gas_prepare" in g and g["gas_prepare"].get("input_frame_row") is None for g in groups.values()),
            reused_payload_calls=sum(g.get("gas_prepare", {}).get("input_frame_row") not in (None, idx) for idx, g in groups.items()),
            transfer_totals={field: sum(g.get("gas_terrain_upload", {}).get("work", {}).get(field, 0) for g in groups.values())
                             for field in ("terrain_writes", "terrain_bytes", "terrain_submits")})
    summary = dict(schema=schema, gas_details=gas_details, gas_summary=gas_summary, pipelined=pipelined, cutoff_ns=cutoff, retained=retained, dropped=dropped,
        span_limit=limit, span_bytes=span_bytes, reserved_span_bytes=limit * span_bytes if span_bytes else None,
        capture_not_truncated=dropped == 0, last_source_row=max(groups) if groups else None,
        last_span_begin_ns=last_begin,
        incomplete=incomplete, source_rows=len(groups), complete_source_rows=len(complete),
        missing_source_rows=len(frames) - len(groups),
        stage_ms={stage: stats([(b - a) / 1e6 for a, b in values]) for stage, values in intervals.items()},
        stage_union_ms={stage: total(values) for stage, values in merged.items()},
        stage_driver_cpu_ms={stage: stats([g[stage]["driver_cpu_ms"] for g in groups.values()
            if stage in g and g[stage]["driver_cpu_ms"] is not None]) for stage in allowed_stages},
        stage_driver_cycles={stage: stats([g[stage]["cycles"] for g in groups.values()
            if stage in g and g[stage]["cycles"] is not None]) for stage in allowed_stages},
        slow_render_frames=leaders[:top], slow_gas_calls=gas_leaders, slow_gaps=gaps[:top],
        slow_other_prepare=sorted((f for f in leaders if f["other_or_unobserved_ms"].get("render_prepare") is not None),
                                  key=lambda f: -f["other_or_unobserved_ms"]["render_prepare"])[:top],
        limitations=["CPU wall only: includes preemption, waits and diagnostic boundary overhead",
            "Rust ThreadId and optional OS thread/counters are distinct; missing counters remain null",
            "gas leaf CPU belongs to the actual task thread, coarse parent CPU excludes other workers",
            "gas input_frame_row identifies extracted payload; missing gas_extract may mean unchanged data and is not an invented new job",
            "terrain transfer totals use gas_terrain_upload only, never repeated parent/payload counters",
            "driver CPU excludes workers, is coarse (zero may be rounded), and wall-minus-CPU is not lock-wait time",
            "nested driver CPU/cycle deltas must not be added together; cycles are not milliseconds",
            "nested/concurrent stage totals overlap; do not add them or call overlap a cause",
            "render_system is a boundary scope around the Bevy function; pipeline cache/other scheduled Render jobs may run between hooks",
            "submit_present_tail includes final submission/readbacks/presentation/collection and scheduled boundary overhead, not pure present",
            "cutoff may retain an open terminal render frame; final main frame is not handed off yet",
            "GPU execution timestamps are not attached to source rows"])
    return summary, issues


RENDER_TASK_LIMIT = 2_000_000


def analyze_render_tasks(directory, scenario, log, manifest, frames, timeline, top, roles):
    """Actual cached SystemMeta entries, not scheduler launch/enqueue times."""
    path = directory / f"{scenario}.tasks.csv"
    inventory = directory / f"{scenario}.task-names.csv"
    declared = bool((manifest or {}).get("RenderTasks"))
    metadata = re.findall(r"^PROFILE render_tasks_schema=(\d+) render_task_limit=(\d+) render_tasks_started=(\d+) render_tasks_retained=(\d+) render_tasks_dropped=(\d+) render_tasks_incomplete=(\d+) render_task_cutoff_ns=(\d+) render_task_names=(\d+) render_task_excluded_names=(\d+) render_task_bytes=(\d+)(?: render_task_stride=(\d+))?$", log, re.M)
    if not declared and not metadata and not path.exists() and not inventory.exists():
        return None, []
    if not declared:
        return None, ["render_tasks_mode_mismatch"]
    if not timeline or not path.exists() or not inventory.exists() or len(metadata) != 1:
        return None, ["render_tasks_artifact_or_metadata_missing"]
    schema, limit, started, retained, dropped, incomplete, cutoff, names, excluded, size = map(int, metadata[0][:10])
    stride = int(metadata[0][10] or 1)
    if stride not in (1, 8):
        raise ValueError("Render tasks invalid whole-frame stride")
    if schema != 1 or limit != RENDER_TASK_LIMIT or retained != min(started, limit) or started != retained + dropped or incomplete > retained or names == 0 or not 1 <= size <= 1024 or cutoff < timeline["cutoff_ns"]:
        raise ValueError("Render tasks metadata budget/count/clock mismatch")
    issues = ["render_tasks_truncated"] if dropped else []
    if retained == 0:
        issues.append("render_tasks_empty_capture")
    if re.findall(r"^PROFILE render_tasks_build=(true|false)$", log, re.M) != ["true"]:
        issues.append("render_tasks_build_or_mode_missing")
    allowed, ignored = set(), set()
    for row in read_csv(inventory):
        if row["allowed"] not in ("0", "1") or not row["name"] or row["name"] in allowed | ignored:
            raise ValueError("Render tasks inventory invalid")
        (allowed if row["allowed"] == "1" else ignored).add(row["name"])
    if len(allowed) != names or len(ignored) != excluded:
        raise ValueError("Render tasks inventory count mismatch")
    parents = {int(r["frame_row"]): (int(r["begin_ns"]), optional_int(r, "end_ns"))
               for r in read_csv(directory / f"{scenario}.render.csv") if r["stage"] == "render_prepare"}
    count, opened, last_begin, last_row, parent_missing = 0, 0, -1, -1, 0
    jobs, leaders, sampled, source_count = {}, [], set(), 0
    for idx_text, rows in itertools.groupby(read_csv(path), key=lambda row: row["frame_row"]):
        idx = int(idx_text)
        if idx <= last_row or idx % stride or (idx not in parents and not timeline["dropped"]) or idx < 0 or idx >= len(frames):
            raise ValueError("Render tasks source order/parent missing")
        last_row = idx; source_count += 1
        if idx not in parents: parent_missing += 1
        parent_a, parent_b = parents.get(idx, (int(frames[idx]["frame_end_ns"]), None))
        intervals, longest, source_open = [], [], 0
        for row in rows:
            count += 1
            if count > retained:
                raise ValueError("Render tasks count mismatch")
            if int(row["generation"]) != int(frames[idx]["generation"]) or int(row["tick"]) != int(frames[idx]["tick"]) or row["name"] not in allowed or row["kind"] not in ("system", "commands"):
                raise ValueError("Render tasks identity/name/kind mismatch")
            a, b = int(row["begin_ns"]), optional_int(row, "end_ns")
            if a < max(parent_a, int(frames[idx]["frame_end_ns"])) or a < last_begin or a > cutoff or (b is not None and (b < a or b > cutoff)) or (parent_b is not None and (b is None or b > parent_b or a > parent_b)):
                raise ValueError("Render tasks clock/nesting mismatch")
            last_begin = a
            if not re.fullmatch(r"ThreadId\(\d+\)", row["begin_thread"]) or (row["end_thread"] != row["begin_thread"] if b is not None else bool(row["end_thread"])):
                raise ValueError("Render tasks thread mismatch")
            counters = {field: optional_int(row, field) for field in ("begin_os_thread", "end_os_thread", "begin_cpu_ns", "end_cpu_ns", "begin_cycles", "end_cycles")}
            if any(v is not None and v < 0 for v in counters.values()) or any(counters[f] == 0 for f in ("begin_os_thread", "end_os_thread")) or any(counters[side + "_os_thread"] is None and (counters[side + "_cpu_ns"] is not None or counters[side + "_cycles"] is not None) for side in ("begin", "end")):
                raise ValueError("Render tasks counter identity invalid")
            same = counters["begin_os_thread"] is not None and counters["begin_os_thread"] == counters["end_os_thread"]
            if counters["begin_os_thread"] is not None and counters["end_os_thread"] is not None and not same:
                raise ValueError("Render tasks OS thread changed")
            def delta(field):
                x, y = counters["begin_" + field], counters["end_" + field]
                if not same or x is None or y is None:
                    return None
                if y < x:
                    raise ValueError("Render tasks counter regressed")
                return y - x
            if b is None:
                if any(counters[k] is not None for k in ("end_os_thread", "end_cpu_ns", "end_cycles")):
                    raise ValueError("Render tasks open call has end counters")
                opened += 1; source_open += 1
                continue
            cpu, cycles, wall = delta("cpu_ns"), delta("cycles"), (b - a)/1e6
            key = (row["name"], row["kind"])
            values = jobs.setdefault(key, dict(wall=[], cpu=[], cycles=[]))
            values["wall"].append(wall)
            if cpu is not None: values["cpu"].append(cpu/1e6)
            if cycles is not None: values["cycles"].append(cycles)
            sampled.add(row["name"])
            if counters["begin_os_thread"] is not None:
                roles.setdefault(str(counters["begin_os_thread"]), set()).add("render_prepare_task")
            intervals.append((a, b))
            longest.append(dict(name=row["name"], kind=row["kind"], wall_ms=wall, cpu_ms=cpu/1e6 if cpu is not None else None, cycles=cycles, os_thread=counters["begin_os_thread"], thread=row["begin_thread"]))
        union = sum(b-a for a,b in merged_intervals(intervals))/1e6
        parent_wall = (parent_b-parent_a)/1e6 if parent_b is not None else None
        leaders.append(dict(source_row=idx, generation=int(frames[idx]["generation"]), tick=int(frames[idx]["tick"]), prepare_ms=parent_wall,
            task_union_ms=union, other_or_unobserved_ms=parent_wall-union if parent_wall is not None and not dropped and not source_open else None,
            longest_calls=sorted(longest, key=lambda c:-c["wall_ms"])[:3], incomplete=source_open))
    if count != retained or opened != incomplete:
        raise ValueError("Render tasks count/open mismatch")
    summary = dict(schema=schema, limit=limit, stride=stride, span_bytes=size, reserved_bytes=limit*size,
        retained=retained, started=started, dropped=dropped, incomplete=incomplete,
        allowed_names=names, excluded_names=sorted(ignored), sampled_names=len(sampled),
        source_rows=source_count, source_rows_without_parent=parent_missing,
        intentionally_unsampled_prepare_rows=sum(idx % stride != 0 for idx in parents),
        prepare_rows_without_calls=sum(idx % stride == 0 for idx in parents)-source_count+parent_missing,
        capture_not_truncated=dropped == 0, cutoff_ns=cutoff,
        jobs=sorted((dict(name=name, kind=kind, wall_ms=stats(v["wall"]), cpu_ms=stats(v["cpu"]), cycles=stats(v["cycles"]), inclusive_wall_ms=sum(v["wall"])) for (name,kind),v in jobs.items()), key=lambda job:-job["inclusive_wall_ms"]),
        slow_prepare_frames=sorted(leaders,key=lambda frame:-(frame["prepare_ms"] or 0))[:top],
        limitations=["Actual SystemMeta entry includes parameter access/function body; commands are separately labeled",
            "Prepare-phase only; skipped systems, conditions, non-SystemMeta work and ambiguous main/render names are not invented jobs",
            "Union covers observed task wall, not CPU utilization; concurrent/nested job wall totals must not be added to parent time",
            "Other/unobserved includes scheduling and unobserved work; no lock-wait or OS cause is inferred",
            "Feature build and per-task counters add overhead and cannot qualify FPS; coarse zero CPU counters are not idle"])
    summary["limitations"].append(f"Whole-frame stride {stride}: unsampled source rows have no task attribution, including rare stalls there")
    if parent_missing: issues.append("render_tasks_parent_coverage_missing")
    return summary, issues


def analyze(directory, scenario="world-battle", top=12):
    directory = Path(directory)
    log = (directory / f"{scenario}.log").read_text(encoding="utf-8-sig", errors="replace")
    sampling_match = re.search(r"^PROFILE physics_task_sampling=(\w+)$", log, re.M)
    sampling = sampling_match[1] if sampling_match else "all"
    summary_path = directory / f"{scenario}.physics.csv"
    summarized = sampling == "longest_per_thread_family_stage"
    summary_groups = iter(itertools.groupby(read_csv(summary_path), key=lambda r: (r["generation"], r["tick"]))) if summarized and summary_path.exists() else None
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
    mapped_wall = [0.0] * len(frames)
    mapped_jobs = [0] * len(frames)
    roles = {}
    task_families = {}
    missing_physics_ticks = 0
    schema_match = re.search(r"^PROFILE core_trace_schema=(\d+)$", log, re.M)
    core_path = directory / f"{scenario}.core.csv"
    core_schema = int(schema_match[1]) if schema_match else (1 if core_path.exists() else None)
    if core_path.exists():
        groups = itertools.groupby(read_csv(core_path), key=lambda r: (r["generation"], r["tick"]))
        for key, raw_rows in groups:
            if key in seen:
                raise ValueError("Non-contiguous or duplicated tick in trace")
            seen.add(key)
            tick = summarize_tick(list(raw_rows))
            samples = tick.pop("physics_samples")
            if summarized and summary_groups is not None:
                group = next(summary_groups, None)
                if group is None or group[0] != key:
                    raise ValueError("Physics summary tick identity mismatch")
                sample_keys = {(j["thread"], j["job_slot"], j["name"]): j for j in samples}
                if len(sample_keys) != len(samples):
                    raise ValueError("Duplicate retained physics scope")
                complete = {}
                for row in group[1]:
                    sample_key = (row["os_thread"], row["stage"], row["family"])
                    sample = sample_keys.pop(sample_key, None)
                    if sample is None:
                        raise ValueError("Physics summary scope mismatch")
                    values = {field: int(row[field]) for field in ("jobs", "inclusive_wall_ns", "max_wall_ns", "cpu_ns", "cpu_samples", "cycles", "cycle_samples")}
                    if (min(values.values()) < 0 or values["jobs"] < 1 or
                            max(values["cpu_samples"], values["cycle_samples"]) > values["jobs"] or
                            values["max_wall_ns"] > values["inclusive_wall_ns"] or
                            values["max_wall_ns"] != round(sample["wall_ms"] * 1e6) or
                            (sample["thread_cpu_ms"] is not None and (values["cpu_samples"] == 0 or values["cpu_ns"] < round(sample["thread_cpu_ms"] * 1e6))) or
                            (sample["cycles"] is not None and (values["cycle_samples"] == 0 or values["cycles"] < sample["cycles"])) or
                            (values["cpu_samples"] == 0 and values["cpu_ns"] != 0) or
                            (values["cycle_samples"] == 0 and values["cycles"] != 0)):
                        raise ValueError("Invalid physics summary counters")
                    family = complete.setdefault(row["family"], dict(jobs=0, inclusive_wall_ms=0.0,
                        max_wall_ms=0.0, sampled_cpu_ms=0.0, cpu_samples=0, cycles=0, cycle_samples=0))
                    family["jobs"] += values["jobs"]
                    family["inclusive_wall_ms"] += values["inclusive_wall_ns"] / 1e6
                    family["max_wall_ms"] = max(family["max_wall_ms"], values["max_wall_ns"] / 1e6)
                    family["sampled_cpu_ms"] += values["cpu_ns"] / 1e6
                    for field in ("cpu_samples", "cycles", "cycle_samples"):
                        family[field] += values[field]
                if sample_keys:
                    raise ValueError("Physics summary scopes missing")
                tick["task_families"].update(complete)
            required = {"physics_rubble_floor", "physics_rubble_columns"}
            if core_schema == 3:
                required.update(("physics_heat_exchange", "physics_ground_heat"))
                if any(p["name"] == "orders_laser" for p in tick["phases"]):
                    required.update(("physics_laser_beam", "physics_laser_boxes", "physics_laser_halo"))
            covered = required.issubset(tick["task_families"])
            if core_schema == 3:
                stages = {
                    "physics_rubble_floor": "0", "physics_rubble_columns": "0",
                    "physics_laser_beam": "2", "physics_laser_boxes": "2", "physics_laser_halo": "2",
                    "physics_heat_exchange": "3", "physics_ground_heat": "4",
                }
                scopes = {(job["name"], job["job_slot"]) for job in samples}
                covered = covered and all((name, stages[name]) in scopes for name in required)
            if not covered:
                missing_physics_ticks += 1
            if tick["os_thread"]:
                roles.setdefault(tick["os_thread"], set()).add("tick_caller")
            for thread, families in tick["worker_families"].items():
                roles.setdefault(thread, set()).update(families)
            for name, values in tick["task_families"].items():
                family = task_families.setdefault(name, dict.fromkeys(values, 0))
                for field, value in values.items():
                    family[field] = max(family[field], value) if field == "max_wall_ms" else family[field] + value
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
                    mapped_wall[idx] += tick["wall_ms"]
                    mapped_jobs[idx] += tick["task_count"]
                else:
                    unmapped += 1
            # Bound retained raw detail to the slowest top ticks.
            slow.append(tick)
            slow.sort(key=lambda r: -r["wall_ms"])
            del slow[top:]
    if summary_groups is not None and next(summary_groups, None) is not None:
        raise ValueError("Unexpected extra physics summary tick")
    phase_summary = []
    for name, values in phase_totals.items():
        cpu = [v["thread_cpu_ms"] for v in values if v["thread_cpu_ms"] is not None]
        wall = [v["wall_ms"] for v in values]
        phase_summary.append(dict(name=name, mean_per_tick_ms=sum(wall) / len(seen),
                                  wall_occurrences=stats(wall), thread_cpu_occurrences=stats(cpu)))
    phase_summary.sort(key=lambda r: -r["mean_per_tick_ms"])
    manifest_path = directory / f"{scenario}.manifest.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8-sig")) if manifest_path.exists() else None
    validity = manifest.get("Validity") or {} if manifest else {}
    reasons = list(validity.get("Reasons") or [])
    camera_tour = camera_tour_capture(directory, scenario, log, manifest, len(frames), reasons)
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
    if core_schema in (2, 3) and missing_physics_ticks:
        reasons.append("physics_task_coverage_missing")
    if core_schema in (2, 3) and not core_path.exists():
        reasons.append("core_trace_missing")
    if core_schema is not None and core_schema not in (1, 2, 3):
        reasons.append("core_trace_schema_unsupported")
    if summarized and not summary_path.exists():
        reasons.append("physics_task_summary_missing")
    if sampling not in ("all", "longest_per_thread_family_stage"):
        reasons.append("physics_task_sampling_unsupported")
    if aligned and core_path.exists() and mapped_steps != ticks_per_frame:
        reasons.append("frame_core_step_distribution_mismatch")
    hash_match = re.search(r"^PROFILE state_hash=([0-9a-f]+)$", log, re.M)
    adapter = re.search(r"^PROFILE adapter_name=.*$", log, re.M)
    rate = re.search(r"^PROFILE tick_rate=(\d+)", log, re.M)
    pools = re.search(r"^PROFILE physics_workers=(\d+) navigation_workers=(\d+)", log, re.M)
    compute_counts = re.findall(r"^PROFILE compute_workers=(\d+)$", log, re.M)
    compute = int(compute_counts[0]) if len(compute_counts) == 1 else None
    requested_compute = int((manifest or {}).get("ComputeWorkers") or 0)
    if requested_compute and (not 1 <= requested_compute <= 16 or compute != requested_compute or
            re.findall(r"^PROFILE compute_override=Some\((\d+)\)$", log, re.M) != [str(requested_compute)]):
        reasons.append("compute_workers_override_mismatch")
    context = render_context(log)
    reasons.extend(context["reasons"])
    stall_trace, stall_issues = analyze_stall_trace(directory, scenario, log, manifest, frames, starts, roles, top)
    reasons.extend(stall_issues)
    render_trace, render_issues = analyze_render_timeline(directory, scenario, log, manifest, frames, top, roles)
    render_tasks, task_issues = analyze_render_tasks(directory, scenario, log, manifest, frames, render_trace, top, roles)
    task_build_match = re.search(r"^PROFILE render_tasks_build=(true|false)$", log, re.M)
    task_build = task_build_match[1] == "true" if task_build_match else None
    reasons.extend(task_issues)
    reasons.extend(render_issues)
    if not pools:
        reasons.append("actual_worker_counts_missing")
    seconds = int(frames[-1]["frame_end_ns"]) / 1e9 if aligned else float(frames[-1]["seconds"])
    meets_rate = scenario != "world-battle" or frame_ticks >= math.floor(seconds * (int(rate[1]) if rate else 30)) - 1
    slice_summary, slow_slices, episodes = summarize_slices(frames, aligned, top)
    incident_coverage(episodes, frames, mapped_steps, mapped_wall, mapped_jobs, stall_trace)
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
    return dict(schema=6, directory=str(directory), scenario=scenario, manifest=manifest,
        valid_for_comparison=bool(validity.get("ValidForComparison")) and not reasons,
        meets_simulation_rate=meets_rate,
        acceptance_eligible=bool(validity.get("ValidForComparison")) and not reasons and meets_rate and not bool((manifest or {}).get("CorePhases")) and stall_trace is None and not bool((manifest or {}).get("CoreTriggerMilliseconds")) and render_trace is None and not bool((manifest or {}).get("RenderTimeline")) and not task_build and render_tasks is None and not bool((manifest or {}).get("RenderTasks")),
        reasons=sorted(set(reasons)), frames=len(frames), frame_ms=stats([v for v in frame_times if v is not None]),
        frame_timing_metric="profile_begin_interval" if aligned else "bevy_time_delta_unaligned",
        bevy_frame_ms=stats([float(r["frame_ms"]) for r in frames]),
        measured_steps=frame_ticks, trace_steps=len(seen), core_tick_ms=stats(core_totals),
        core_ms_per_step=sum(float(r["simulation_core_ms"]) for r in frames) / frame_ticks if frame_ticks else None,
        realtime_ratio=frame_ticks / (seconds * (int(rate[1]) if rate else 30)) if seconds > 0 else None,
        state_hash=hash_match[1] if hash_match else None, actual_adapter=adapter[0] if adapter else None,
        actual_workers=[int(pools[1]), int(pools[2])] if pools else None,
        actual_compute_workers=compute,
        actual_render_context=context,
        camera_tour=camera_tour,
        os_cpu_window=os_window(directory, scenario, log, frames, roles),
        terrain_journal_maintenance=journal_maintenance(log),
        task_families=task_families,
        core_trace_schema=core_schema,
        physics_task_sampling=sampling,
        physics_task_coverage=dict(traced_ticks=len(seen) - missing_physics_ticks, missing_ticks=missing_physics_ticks),
        frame_alignment="shared_monotonic_epoch" if aligned else "legacy_frame_delta_lags_phase_columns",
        trace_alignment=dict(unmapped_ticks=unmapped, missing_clock_ticks=missing_clock,
                             identity_mismatches=identity_mismatches, mapped_steps_per_frame=mapped_steps if core_path.exists() and aligned else None),
        phases=phase_summary, slow_ticks=slow, slow_frames=expensive_frames,
        slice_summary=slice_summary, slow_slices=slow_slices,
        frame_cadence=frame_cadence(frames, aligned),
        stall_episodes=episodes, stall_trace=stall_trace, render_timeline=render_trace,
        render_tasks=render_tasks, render_tasks_build=task_build,
        limitations=["wall_minus_cpu includes preemption, waits and coarse OS accounting; it is not lock wait time",
                    "physics slots identify caller stages (0 rubble, 1 debris, 2 laser, 3 heat exchange, 4 terrain heat, 5 unit heat), not actors; OS thread and family pair scopes",
                    "sampled traces retain the longest physics scope per OS thread, family and stage; complete job totals are validated separately",
                    "task family wall totals are inclusive across concurrent jobs, not elapsed phase time or CPU utilization",
                    "zero task CPU deltas can be coarse OS rounding; diagnostic callbacks add overhead",
                    "worker_covered_wall is a union per OS thread, not CPU utilization; do not add it to parent wall",
                    "unit intervals are between consecutive callbacks and can enclose other work",
                    "render_prepare/render_extract are latest render-thread atomic observations and can belong to another frame; repeated observations are not new jobs and their per-row sum/mean is not render CPU work",
                    "single_tick_ms is exact core wall only for one-step slices; multi-step slices expose an average, never invented individual tick durations",
                    "frame cadence groups preceding slices plus following gaps; terminal Update has no observed following interval; core is nested within Update and gaps have no CPU/GPU/wait attribution",
                    "phase-ring capture retains selected phase/detail ticks and quiet tail, not all ticks or worker jobs; caller CPU is not process CPU and the instrumented run cannot qualify FPS",
                    "stall episodes group adjacent slices, following gaps or begin intervals >=50ms; these overlap, and only disjoint slice/gap wall is added; core wall is not OS CPU time",
                    "OS counters use coarse sampling; UTC alignment is approximate, and only full in-window intervals are included",
                    "callback roles are accumulated per OS ID; thread CPU accounting keeps creation-time lifetimes separate",
                    "OS thread CPU includes unmarked work too; a callback role does not attribute all of that thread's CPU to one subsystem",
                    "OS process polling can miss workloads shorter than its interval; stack/wait traces remain separate"])


def compare(a, b):
    problems = []
    for label, run in [("before", a), ("after", b)]:
        if not run["valid_for_comparison"]:
            problems.append(f"{label}:not_valid")
        if not run["meets_simulation_rate"]:
            problems.append(f"{label}:simulation_not_realtime")
    keys = ["Scenario", "RequestedSeconds", "TargetFps", "RayonEnvironment", "NavigationWorkers", "GpuTimestamps",
            "CpuThreads", "CorePhases", "EnvironmentSampleIntervalMs", "InputHashes", "RequestedWindow", "RustLog"]
    if bool((a["manifest"] or {}).get("CameraTour")) != bool((b["manifest"] or {}).get("CameraTour")):
        problems.append("different:CameraTour")
    if bool((a["manifest"] or {}).get("RenderTimeline")) != bool((b["manifest"] or {}).get("RenderTimeline")):
        problems.append("different:RenderTimeline")
    if int((a["manifest"] or {}).get("ComputeWorkers") or 0) != int((b["manifest"] or {}).get("ComputeWorkers") or 0):
        problems.append("different:ComputeWorkers")
    if a.get("actual_compute_workers") != b.get("actual_compute_workers"):
        problems.append("different_or_missing:actual_compute_workers")
    if bool((a["manifest"] or {}).get("RenderTasks")) != bool((b["manifest"] or {}).get("RenderTasks")) or a.get("render_tasks_build") != b.get("render_tasks_build"):
        problems.append("different:RenderTasks_build_or_mode")
    if (a.get("render_tasks") or {}).get("stride") != (b.get("render_tasks") or {}).get("stride"):
        problems.append("different:render_tasks:stride")
    for key in ("schema", "gas_details", "span_limit", "span_bytes"):
        if (a.get("render_timeline") or {}).get(key) != (b.get("render_timeline") or {}).get(key):
            problems.append("different:render_timeline:" + key)
    for key in keys:
        if (a["manifest"] or {}).get(key) != (b["manifest"] or {}).get(key):
            problems.append(f"different:{key}")
    if int((a["manifest"] or {}).get("CoreTriggerMilliseconds") or 0) != int((b["manifest"] or {}).get("CoreTriggerMilliseconds") or 0):
        problems.append("different:CoreTriggerMilliseconds")
    for key in ("schema", "mode", "cpu_sampling", "threshold_ms", "pre_ticks", "post_ticks", "max_windows"):
        if (a["stall_trace"] or {}).get(key) != (b["stall_trace"] or {}).get(key):
            problems.append(f"different:stall_trace:{key}")
    for key in ["measured_steps", "actual_adapter", "actual_workers", "actual_render_context", "core_trace_schema", "physics_task_sampling", "frame_alignment", "frame_timing_metric", "state_hash"]:
        if a[key] != b[key] or (key in ("actual_adapter", "actual_workers", "state_hash") and a[key] is None):
            problems.append(f"different_or_missing:{key}")
    return {"eligible": not problems, "reasons": problems,
            "before_core_ms_per_step": a["core_ms_per_step"], "after_core_ms_per_step": b["core_ms_per_step"]}


def markdown(report):
    lines = ["# Range performance evidence", "",
             f"Diagnostic evidence valid: {report['valid_for_comparison']}. Reasons: {', '.join(report['reasons']) or 'none observed'}.",
             f"FPS acceptance eligible: {report['acceptance_eligible']}; simulation at real time: {report['meets_simulation_rate']}; ratio: {report['realtime_ratio']}.", "",
             f"Frames: {report['frames']}; real steps: {report['measured_steps']}; traced steps: {report['trace_steps']}.",
             f"Frame alignment: {report['frame_alignment']}; duration metric: {report['frame_timing_metric']}. Core ms per actual step: {report['core_ms_per_step']}.", "",
             f"Requested compute workers: {int((report['manifest'] or {}).get('ComputeWorkers') or 0)} (0 = default); actual compute workers: {report.get('actual_compute_workers')}. Actual physics/navigation workers: {report['actual_workers']}.", "",
             f"Observed render context: {report['actual_render_context']['initial'] or 'unavailable'}.",
             f"Render context records: {len(report['actual_render_context']['steps'])}; invalid change: {report['actual_render_context']['invalid_change']}.", "",
             "| Phase | Mean per actual traced tick, ms | p99 occurrence, ms | CPU counter samples |",
             "|---|---:|---:|---:|"]
    for p in report["phases"]:
        cpu = p["thread_cpu_occurrences"]
        lines.append(f"| {p['name']} | {p['mean_per_tick_ms']:.3f} | {p['wall_occurrences']['p99']:.3f} | {cpu['samples'] if cpu else 'unavailable'} |")
    if report.get("camera_tour"):
        tour = report["camera_tour"]
        lines += ["", f"Wall-clock camera tour: {tour['route']}; {len(tour['segments'])} segments; "
                  f"{len(tour['window_origins'])} published origins; {tour['blocked_frames']} blocked frames; "
                  f"longest sampled no-tick interval {tour['max_no_tick_seconds']:.6f} s."]
    lines += ["", f"Core trace schema: {report['core_trace_schema']}; sampling: {report['physics_task_sampling']}; physics coverage: {report['physics_task_coverage']}.", "",
              "Job totals are inclusive across threads; do not add them to phase time.", "",
              "| Task family | Jobs | Inclusive wall total, ms | Longest job, ms | Sampled CPU total, ms | CPU samples |",
              "|---|---:|---:|---:|---:|---:|"]
    for name, family in sorted(report["task_families"].items()):
        cpu = f"{family['sampled_cpu_ms']:.3f}" if family['cpu_samples'] else "unavailable"
        lines.append(f"| {name} | {family['jobs']} | {family['inclusive_wall_ms']:.3f} | {family['max_wall_ms']:.3f} | {cpu} | {family['cpu_samples']} |")
    lines += ["", "Slow ticks (CPU counter time is not lock wait time):", ""]
    for tick in report["slow_ticks"]:
        leaders = sorted(tick["phases"], key=lambda p: -p["wall_ms"])[:3]
        names = ", ".join(f"{p['name']}={p['wall_ms']:.3f}ms" for p in leaders)
        lines.append(f"- generation {tick['generation']}, tick {tick['tick']}: wall {tick['wall_ms']:.3f}ms, CPU {tick['thread_cpu_ms']}ms; {names}.")
        for job in tick["longest_tasks"][:3]:
            owner = f"physics stage {job['job_slot']}" if job['domain'] == 'physics' else f"unit {job['unit']}"
            lines.append(f"  - task {job['name']}, {owner}, OS thread {job['thread']}: wall {job['wall_ms']:.3f}ms, CPU {job['thread_cpu_ms']}ms (inclusive; not added to phase totals).")
    lines += ["", "Slow frames (monotonic begin-to-begin; previous measured slice plus current gap):", "",
              "| Row | Generation/tick | Observed interval, ms | Bevy delta, ms | Previous core, ms | Current core, ms | Previous measured slice, ms | Outside slices, ms |",
              "|---|---|---:|---:|---:|---:|---:|---:|"]
    def number(value):
        return "unavailable" if value is None else f"{value:.3f}"
    for frame in report["slow_frames"]:
        values = [number(frame[key]) for key in ("frame_delta_ms", "bevy_delta_ms", "previous_core_ms", "current_core_ms", "previous_measured_slice_ms", "outside_observed_slice_ms")]
        lines.append(f"| {frame['row']} | {frame['generation']}/{frame['tick']} | {' | '.join(values)} |")
    lines += ["", "Main slices (also available without detailed core trace):", "",
              "| Row | Generation / tick range | Steps | Measured wall, ms | Core wall, ms | Following interval row / ms | Following gap, ms | Exclusive Update phases, ms |",
              "|---|---|---:|---:|---:|---|---:|---|"]
    for sample in report["slow_slices"]:
        phases = ", ".join(f"{key.removesuffix('_ms')}={number(value)}" for key, value in sample["phases"].items())
        ticks = f"{sample['first_tick']}..{sample['end_tick']}" if sample["simulation_ticks"] else "no step"
        lines.append(f"| {sample['row']} | {sample['generation']} / {ticks} | {sample['simulation_ticks']} | {number(sample['measured_wall_ms'])} | {number(sample['core_ms'])} | {sample['following_interval_row']} / {number(sample['following_interval_ms'])} | {number(sample['following_gap_ms'])} | {phases} |")
    lines += ["", "Core/occupancy/physics/terrain/atmosphere are nested details, not additive Update phases. Render atomic observations are excluded from the slice breakdown. Multi-step core totals do not identify individual tick costs."]
    cadence = report.get("frame_cadence")
    if cadence is not None:
        lines += ["", "Frame cadence: preceding Update plus its following gap (not current-row work).",
                  f"Complete intervals: {cadence['intervals']}; covered wall {cadence['covered_wall_ms']:.3f}ms = Update {cadence['update_total_ms']:.3f}ms + following gaps {cadence['following_gap_total_ms']:.3f}ms.",
                  f"Completed steps in those slices: {cadence['completed_steps']}; terminal slice row {cadence['terminal_slice']['row']} has {cadence['terminal_slice']['simulation_ticks']} steps and no following interval. Core is nested inside Update; gaps do not identify a GPU or wait cause.", "",
                  "| Source generation | Steps in preceding slice | Intervals | Wall share | Mean interval, ms | Mean Update, ms | Mean following gap, ms | Mean nested core, ms | Generation crossings |",
                  "|---|---:|---:|---:|---:|---:|---:|---:|---:|"]
        for group in cadence["groups"]:
            lines.append(f"| {group['generation']} | {group['steps_per_slice']} | {group['intervals']} | {group['wall_fraction']:.1%} | {group['interval_ms']['mean']:.3f} | {group['update_ms']['mean']:.3f} | {group['following_gap_ms']['mean']:.3f} | {group['core_ms']['mean']:.3f} | {group['generation_crossings']} |")
    episodes = report["stall_episodes"]
    if episodes is not None:
        lines += ["", f"Stall episodes: {episodes['count']} (slices, following gaps or begin intervals >= {episodes['threshold_ms']}ms; disjoint slice/gap wall, not an identified cause).", ""]
        for episode in episodes["retained"]:
            lines.append(f"- generation {episode['generation']}, rows {episode['first_row']}..{episode['last_row']}, {episode['simulation_ticks']} steps: measured wall {episode['measured_wall_ms']:.3f}ms, following gaps {episode['following_gap_ms']:.3f}ms, core wall {episode['core_ms']:.3f}ms; longest exact single tick {number(episode['longest_single_tick_ms'])}ms.")
            coverage = episode["diagnostic_coverage"]
            lines.append(f"  Tick phase coverage: {coverage['tick_phases']} ({coverage['traced_steps']}/{episode['simulation_ticks']} steps); non-core slice {coverage['non_core_slice_ms']:.3f}ms; outside Update {coverage['outside_update_ms']:.3f}ms. Worker scopes: {coverage['worker_scopes_available']}; OS stacks: unavailable. Cause remains unproven.")
    capture = report["stall_trace"]
    if capture is not None:
        lines += ["", f"Phase-ring capture: {capture['windows']} windows, {capture['observed_slow_ticks']} slow ticks, {capture['unarmed_slow_ticks']} unarmed crossings; {capture['retained_ticks']} selected ticks retained. CPU sampling: {capture['cpu_sampling']}. Diagnostic only; no worker/stack/wait evidence. Quiet tail remains even if no trigger fired.", ""]
        if capture["schema"] >= 2:
            lines.append(f"Frame triggers: {capture['observed_frame_signals']} signals, {capture['unarmed_frame_signals']} unarmed, {capture['dropped_frame_signals']} records omitted by the 64-record bound. Window 0 means unarmed; a signal does not imply complete phase coverage.")
            for signal in capture["frame_signals"]:
                lines.append(f"- {signal['source']}: row {signal['frame_row']}, generation {signal['generation']}, tick {signal['tick']}, {signal['wall_ms']:.3f}ms, window {signal['window']}.")
        if capture["late_peak"] is not None:
            peak = capture["late_peak"]
            lines.append(f"Late peak reserve: {peak['source']} row {peak['frame_row']}, tick {peak['tick']}, {peak['wall_ms']:.3f}ms; up to {peak['context_tick_budget']} preceding/completed ticks retained, no post window. Overlapping ticks appear only once. This reserve is partial evidence, not an OS stack or cause.")
        for tick in capture["slow_ticks"]:
            phases = ", ".join(f"{p['name']}={p['wall_ms']:.3f}ms (caller CPU {number(p['thread_cpu_ms'])}ms)" for p in sorted(tick["phases"], key=lambda p: -p["wall_ms"])[:3])
            lines.append(f"- row {tick['frame_row']}, generation {tick['generation']}, tick {tick['tick']}: wall {tick['wall_ms']:.3f}ms, caller CPU {number(tick['thread_cpu_ms'])}ms; {phases}.")
    lines += ["", "Bevy delta may lag by multiple rows through the render pipeline; it is not used to select slow frames or attribute their phases. Outside slices is the monotonic gap from the previous measured end to the current measured start. It is not an identified GPU/lock wait. First-frame previous values and legacy clock fields remain unavailable."]
    lines += ["", *[f"- {s}." for s in report["limitations"]]]
    cpu = report["os_cpu_window"]
    if cpu["status"] == "aligned_intervals":
        lines += ["", f"OS CPU inside measurement window: {cpu['process_cpu_ms']:.3f}ms / {cpu['observed_ms']:.3f}ms; core equivalents={cpu['core_equivalents']}.", ""]
        for thread in cpu["sampled_threads"][:12]:
            lines.append(f"- OS thread {thread['os_thread']}: {thread['cpu_ms']:.3f}ms; observed callbacks: {', '.join(thread['observed_roles']) or 'unattributed'}.")
    timeline = report.get("render_timeline")
    tasks = report.get("render_tasks")
    if tasks:
        lines += ["", f"Actual Bevy prepare tasks: {tasks['retained']} records, drops {tasks['dropped']}, open {tasks['incomplete']}, source rows {tasks['source_rows']}; {tasks['sampled_names']}/{tasks['allowed_names']} names executed. Excluded names: {tasks['excluded_names']}.", "",
                  "| Source row | Parent prepare, ms | Task wall union, ms | Other/unobserved, ms | Longest actual calls |", "|---:|---:|---:|---:|---|"]
        for frame in tasks["slow_prepare_frames"]:
            calls = "; ".join(f"{c['kind']} {c['name']}: {c['wall_ms']:.3f}ms, OS {c['os_thread']}, CPU {c['cpu_ms']}, cycles {c['cycles']}" for c in frame['longest_calls'])
            lines.append(f"| {frame['source_row']} | {frame['prepare_ms']} | {frame['task_union_ms']:.3f} | {frame['other_or_unobserved_ms']} | {calls.replace('|', '/')} |")
        lines += ["", *tasks["limitations"]]
    if report.get("terrain_journal_maintenance") is not None:
        lines += ["", f"Terrain journal maintenance: {report['terrain_journal_maintenance']}.", ""]
    if timeline:
        lines += ["", "Render CPU timeline (shared epoch, immutable source frame; overlaps are not causes):", "",
                  f"Source rows complete {timeline['complete_source_rows']}/{report['frames']}, missing {timeline['missing_source_rows']}, open spans {timeline['incomplete']}, dropped {timeline['dropped']}.", "",
                  f"Capture not truncated: {timeline['capture_not_truncated']}; span budget {timeline['span_limit']}, reserved span bytes {timeline['reserved_span_bytes']}; last recorded source row {timeline['last_source_row']}. Terminal open/unhanded frames remain explicit.", "",
                  "| Stage | Samples | Mean wall, ms | p99, ms | Max, ms | Mean calling-thread CPU, ms |",
                  "|---|---:|---:|---:|---:|---:|"]
        for stage, values in sorted(timeline["stage_ms"].items()):
            cpu = timeline["stage_driver_cpu_ms"].get(stage)
            cpu_text = f"{cpu['mean']:.3f}" if cpu else "unavailable"
            lines.append(f"| {stage} | {values['samples']} | {values['mean']:.3f} | {values['p99']:.3f} | {values['max']:.3f} | {cpu_text} |")
        if timeline.get("gas_details"):
            lines += ["", f"Actual gas calls/work (transfer totals from upload leaf only): {timeline['gas_summary']}.", ""]
            lines += ["Actual extract and prepare calls ranked separately (CPU is coarse; missing is unavailable):", "",
                      "| Source row | Stage | Status | Payload row | Wall, ms | Calling-thread CPU, ms | Patches | Words | Terrain bytes |",
                      "|---:|---|---|---:|---:|---:|---:|---:|---:|"]
            for stage, frames in timeline["slow_gas_calls"].items():
                for frame in frames[:3]:
                    leaf = frame["leaf_work"][stage]
                    work = leaf["work"]
                    cpu = frame["driver_counters"][stage]["cpu_ms"]
                    cpu_text = "unavailable" if cpu is None else f"{cpu:.3f}"
                    payload = leaf["input_frame_row"]
                    lines.append(f"| {frame['source_row']} | {stage} | {'complete' if leaf['completed'] else 'aborted'} | {payload if payload is not None else 'unknown'} | {frame['spans_ms'][stage]:.3f} | {cpu_text} | {work['patches']} | {work['words']} | {work['terrain_bytes']} |")
            lines += ["", "Prepare outside gas ranked separately: parent wall outside the gas call includes other jobs/scheduling, not identified idle or wait time.", "",
                      "| Source row | Prepare parent, ms | Actual gas prepare, ms | Other/unobserved prepare, ms | Extract parent, ms | Actual gas extract, ms |",
                      "|---:|---:|---:|---:|---:|---:|"]
            def shown(value):
                return "unavailable" if value is None else f"{value:.3f}"
            for frame in timeline["slow_other_prepare"][:3]:
                spans = frame["spans_ms"]
                lines.append(f"| {frame['source_row']} | {shown(spans.get('render_prepare'))} | {shown(spans.get('gas_prepare'))} | {shown(frame['other_or_unobserved_ms'].get('render_prepare'))} | {shown(spans.get('extract_callback'))} | {shown(spans.get('gas_extract'))} |")
            if timeline["schema"] >= 3:
                lines += ["", "Extraction phases (same calling thread, wall including preemption; phases are nested in extract):", "",
                          "| Source row | Input dirty ranges | Output patches | Range copy/sort/merge, ms | Pack words, ms | Clone inputs, ms |",
                          "|---:|---:|---:|---:|---:|---:|"]
                for frame in timeline["slow_gas_calls"]["gas_extract"][:3]:
                    w = frame["leaf_work"]["gas_extract"]["work"]
                    lines.append(f"| {frame['source_row']} | {w['dirty_ranges']} | {w['patches']} | {w['range_ns']/1e6:.3f} | {w['pack_ns']/1e6:.3f} | {w['clone_ns']/1e6:.3f} |")
        lines += ["", "Slow gaps and timeline intersections (nested spans are never added):", ""]
        for gap in timeline["slow_gaps"]:
            lines.append(f"- row {gap['row']}: gap {gap['gap_ms']:.3f}ms, stage overlaps {gap['overlap_ms']}, unobserved/other {gap['unobserved_or_other_ms']:.3f}ms.")
        lines += ["", *timeline["limitations"]]
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
    print(f"Steps={result['measured_steps']} trace={result['trace_steps']} evidence_valid={result['valid_for_comparison']} fps_eligible={result['acceptance_eligible']} realtime={result['meets_simulation_rate']} reasons={result['reasons']}")
    if args.require_valid and (not result["valid_for_comparison"] or not result.get("comparison", {"eligible": True})["eligible"]):
        raise SystemExit(2)


if __name__ == "__main__":
    main()
