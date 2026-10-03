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
        slow = (wall is not None and wall >= threshold_ms) or (gap is not None and gap >= threshold_ms)
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


def analyze_stall_trace(directory, scenario, log, manifest, frames, starts, roles, top):
    header = re.search(r"^PROFILE stall_capture_schema=(\d+) stall_capture_mode=(\w+) threshold_ms=(\d+) pre_ticks=(\d+) post_ticks=(\d+) max_windows=(\d+)(?: cpu_sampling=(\w+))?$", log, re.M)
    declared = int((manifest or {}).get("CoreTriggerMilliseconds") or 0)
    path = directory / f"{scenario}.stall.csv"
    if not header:
        return None, ["stall_capture_metadata_missing"] if declared or path.exists() else []
    version, mode, threshold, pre, post, maximum, cpu_sampling = header.groups()
    cpu_sampling = cpu_sampling or "unspecified"
    threshold, pre, post, maximum = map(int, (threshold, pre, post, maximum))
    if version != "1" or mode != "phase_ring" or min(threshold, maximum) <= 0 or cpu_sampling not in ("unspecified", "tick_boundaries"):
        raise ValueError("Unsupported stall capture contract")
    summary = re.search(r"^PROFILE stall_windows=(\d+) stall_slow_ticks=(\d+) stall_unarmed_slow_ticks=(\d+) stall_retained_ticks=(\d+)$", log, re.M)
    if not summary:
        raise ValueError("Stall capture summary missing")
    windows, slow_count, unarmed, retained = map(int, summary.groups())
    if windows > maximum or slow_count < windows or unarmed > slow_count or retained > maximum * (pre + 1 + post) + pre:
        raise ValueError("Stall capture budget/count mismatch")
    if not all(start is not None for start in starts):
        raise ValueError("Stall capture requires aligned frame clocks")
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
    return dict(schema=1, mode=mode, cpu_sampling=cpu_sampling, threshold_ms=threshold, pre_ticks=pre, post_ticks=post,
                max_windows=maximum, windows=windows, observed_slow_ticks=slow_count,
                unarmed_slow_ticks=unarmed, retained_ticks=retained, partial=True,
                ticks=ticks, slow_ticks=sorted(ticks, key=lambda t: -t["wall_ms"])[:top]), issues


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
    context = render_context(log)
    reasons.extend(context["reasons"])
    stall_trace, stall_issues = analyze_stall_trace(directory, scenario, log, manifest, frames, starts, roles, top)
    reasons.extend(stall_issues)
    if not pools:
        reasons.append("actual_worker_counts_missing")
    seconds = int(frames[-1]["frame_end_ns"]) / 1e9 if aligned else float(frames[-1]["seconds"])
    meets_rate = scenario != "world-battle" or frame_ticks >= math.floor(seconds * (int(rate[1]) if rate else 30)) - 1
    slice_summary, slow_slices, episodes = summarize_slices(frames, aligned, top)
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
    return dict(schema=4, directory=str(directory), scenario=scenario, manifest=manifest,
        valid_for_comparison=bool(validity.get("ValidForComparison")) and not reasons,
        meets_simulation_rate=meets_rate,
        acceptance_eligible=bool(validity.get("ValidForComparison")) and not reasons and meets_rate and not bool((manifest or {}).get("CorePhases")) and stall_trace is None and not bool((manifest or {}).get("CoreTriggerMilliseconds")),
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
        task_families=task_families,
        core_trace_schema=core_schema,
        physics_task_sampling=sampling,
        physics_task_coverage=dict(traced_ticks=len(seen) - missing_physics_ticks, missing_ticks=missing_physics_ticks),
        frame_alignment="shared_monotonic_epoch" if aligned else "legacy_frame_delta_lags_phase_columns",
        trace_alignment=dict(unmapped_ticks=unmapped, missing_clock_ticks=missing_clock,
                             identity_mismatches=identity_mismatches, mapped_steps_per_frame=mapped_steps if core_path.exists() and aligned else None),
        phases=phase_summary, slow_ticks=slow, slow_frames=expensive_frames,
        slice_summary=slice_summary, slow_slices=slow_slices,
        stall_episodes=episodes, stall_trace=stall_trace,
        limitations=["wall_minus_cpu includes preemption, waits and coarse OS accounting; it is not lock wait time",
                    "physics slots identify caller stages (0 rubble, 1 debris, 2 laser, 3 heat exchange, 4 terrain heat), not actors; OS thread and family pair scopes",
                    "sampled traces retain the longest physics scope per OS thread, family and stage; complete job totals are validated separately",
                    "task family wall totals are inclusive across concurrent jobs, not elapsed phase time or CPU utilization",
                    "zero task CPU deltas can be coarse OS rounding; diagnostic callbacks add overhead",
                    "worker_covered_wall is a union per OS thread, not CPU utilization; do not add it to parent wall",
                    "unit intervals are between consecutive callbacks and can enclose other work",
                    "render_prepare/render_extract are latest render-thread atomic observations and can belong to another frame; repeated observations are not new jobs and their per-row sum/mean is not render CPU work",
                    "single_tick_ms is exact core wall only for one-step slices; multi-step slices expose an average, never invented individual tick durations",
                    "phase-ring capture retains selected phase/detail ticks and quiet tail, not all ticks or worker jobs; caller CPU is not process CPU and the instrumented run cannot qualify FPS",
                    "stall episodes group adjacent measured slices or following gaps >=50ms; core wall includes worker execution/preemption/waits and is not OS CPU time",
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
            "CpuThreads", "CorePhases", "EnvironmentSampleIntervalMs", "InputHashes", "RequestedWindow"]
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
             f"Observed render context: {report['actual_render_context']['initial'] or 'unavailable'}.",
             f"Render context records: {len(report['actual_render_context']['steps'])}; invalid change: {report['actual_render_context']['invalid_change']}.", "",
             "| Phase | Mean per actual traced tick, ms | p99 occurrence, ms | CPU counter samples |",
             "|---|---:|---:|---:|"]
    for p in report["phases"]:
        cpu = p["thread_cpu_occurrences"]
        lines.append(f"| {p['name']} | {p['mean_per_tick_ms']:.3f} | {p['wall_occurrences']['p99']:.3f} | {cpu['samples'] if cpu else 'unavailable'} |")
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
    episodes = report["stall_episodes"]
    if episodes is not None:
        lines += ["", f"Stall episodes: {episodes['count']} (adjacent measured slices or following gaps >= {episodes['threshold_ms']}ms; wall evidence, not an identified cause).", ""]
        for episode in episodes["retained"]:
            lines.append(f"- generation {episode['generation']}, rows {episode['first_row']}..{episode['last_row']}, {episode['simulation_ticks']} steps: measured wall {episode['measured_wall_ms']:.3f}ms, following gaps {episode['following_gap_ms']:.3f}ms, core wall {episode['core_ms']:.3f}ms; longest exact single tick {number(episode['longest_single_tick_ms'])}ms.")
    capture = report["stall_trace"]
    if capture is not None:
        lines += ["", f"Phase-ring capture: {capture['windows']} windows, {capture['observed_slow_ticks']} slow ticks, {capture['unarmed_slow_ticks']} unarmed crossings; {capture['retained_ticks']} selected ticks retained. CPU sampling: {capture['cpu_sampling']}. Diagnostic only; no worker/stack/wait evidence. Quiet tail remains even if no trigger fired.", ""]
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
