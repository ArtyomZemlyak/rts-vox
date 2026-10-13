"""GPU publications are delayed batches, not render frame IDs or CPU intervals."""
import csv
import math
import re
import statistics


def stats(values):
    v = sorted(values)
    return dict(samples=len(v), mean=statistics.mean(v), p99=v[int(len(v)*.99)], max=v[-1]) if v else None


def capture(directory, scenario, log, manifest, frames, reasons):
    declared = bool((manifest or {}).get('GpuTimestamps'))
    header = re.findall(r'^PROFILE gpu_capture_schema=1 gpu_sample_limit=(\d+) gpu_samples_seen=(\d+) gpu_samples_retained=(\d+) gpu_samples_dropped=(\d+) gpu_samples_invalid=(\d+) gpu_clock=publication_without_source_frame$', log, re.M)
    summaries = {}
    for line in log.splitlines():
        if not line.startswith('GPU '):
            continue
        match = re.fullmatch(r'GPU (.+/elapsed_gpu) n=(\d+) p50_ms=(\S+) p95_ms=(\S+) p99_ms=(\S+) max_ms=(\S+) mean_ms=(\S+)', line)
        if not match:
            raise ValueError('Malformed GPU summary')
        path, count, *values = match.groups()
        values = list(map(float, values))
        if path in summaries or int(count) < 1 or any(not math.isfinite(v) or v < 0 for v in values):
            raise ValueError('Invalid GPU summary')
        summaries[path] = dict(samples=int(count), **dict(zip(('p50','p95','p99','max','mean'), values)))
    if not declared:
        if header or summaries or re.search(r'^PROFILE gpu_capture_schema=', log, re.M):
            reasons.append('gpu_capture_mode_mismatch')
        return None
    if not header:
        # Preserve old diagnostic evidence without inventing raw samples.
        if summaries and not re.search(r'^PROFILE gpu_capture_schema=', log, re.M):
            return dict(schema=0, status='legacy_aggregates_only', passes=summaries, groups=None,
                        limitation='No batch identity or raw samples; pass means cannot be added.')
        reasons.append('gpu_capture_metadata_missing')
        return None
    if len(header) != 1:
        raise ValueError('Duplicate GPU capture metadata')
    limit, seen, kept, dropped, invalid = map(int, header[0])
    if not 1 <= limit <= 1_000_000 or kept > limit or seen != kept+dropped+invalid:
        raise ValueError('GPU sample count mismatch')
    for count, reason in [(dropped, 'gpu_samples_truncated'), (invalid, 'gpu_samples_invalid')]:
        if count:
            reasons.append(reason)
    if not kept:
        reasons.append('gpu_samples_unavailable')
    file = directory / f'{scenario}.gpu.csv'
    if not file.exists():
        reasons.append('gpu_samples_artifact_missing')
        return None
    passes, batches, last = {}, {}, {}
    count = 0
    with file.open(encoding='utf-8-sig', newline='') as source:
        reader = csv.DictReader(source)
        if reader.fieldnames != ['observation_row','available_ns','path','elapsed_gpu_ms']:
            raise ValueError('GPU CSV schema mismatch')
        for sample in reader:
            count += 1
            if count > kept:
                raise ValueError('GPU retained count mismatch')
            row, available = int(sample['observation_row']), int(sample['available_ns'])
            path, value = sample['path'], float(sample['elapsed_gpu_ms'])
            if (not 0 <= row < len(frames) or available < 0 or
                    not math.isfinite(value) or value < 0 or not path.endswith('/elapsed_gpu')):
                raise ValueError('Invalid GPU sample')
            end = frames[row].get('frame_end_ns')
            if end is None or available > int(end):
                raise ValueError('GPU publication outside observation')
            if path in last and (available <= last[path][0] or row < last[path][1]):
                raise ValueError('GPU duplicate or regressing publication')
            last[path] = available, row
            passes.setdefault(path, []).append(value)
            batch = batches.setdefault(available, dict(row=row, values={}))
            if batch['row'] != row:
                raise ValueError('GPU batch observed on different rows')
            batch['values'][path] = value
    if count != kept:
        raise ValueError('GPU retained count mismatch')
    if set(passes) != set(summaries):
        raise ValueError('GPU raw/summary path mismatch')
    for path, values in passes.items():
        result = stats(values)
        summary = summaries[path]
        if result['samples'] != summary['samples'] or abs(result['mean']-summary['mean']) > 0.0000011:
            raise ValueError('GPU raw/summary value mismatch')
        ordered = sorted(values)
        for key, value in [('p50', ordered[len(ordered)//2]), ('p95', ordered[len(ordered)*95//100]), ('p99', result['p99']), ('max', result['max'])]:
            if abs(value-summary[key]) > 0.000501:
                raise ValueError('GPU raw/summary quantile mismatch')
    # Named spans can be nested. Only leaves contribute to the observed sum.
    names = {p.removesuffix('/elapsed_gpu'): p for p in passes}
    leaves = {path for name,path in names.items() if not any(other.startswith(name+'/') for other in names)}
    def family(path):
        name = path.split('/')[-2]
        if name == 'gas-base' or name.startswith('gas-region-'):
            return 'gas_raymarch'
        if name.startswith('terrain-'):
            return 'terrain'
        if name in ('gas-mask','gas-simulation','gas-display-samples'):
            return 'gas_compute'
        if name in ('voxel-edge','voxel-occlusion'):
            return 'voxel_postprocess'
        return 'other_measured_passes'
    groups = {family(p): [] for p in leaves}
    totals = []
    for batch in batches.values():
        sums = dict.fromkeys(groups, 0.0)
        for path,value in batch['values'].items():
            if path in leaves:
                sums[family(path)] += value
        for name,value in sums.items():
            groups[name].append(value)
        totals.append(sum(sums.values()))
    return dict(schema=1, status='raw_publications', retained=kept, dropped=dropped, invalid=invalid,
                batches=len(batches), main_frames=len(frames), batches_per_main_frame=len(batches)/len(frames),
                passes={k: stats(v) for k,v in passes.items()}, groups={k: stats(v) for k,v in groups.items()},
                observed_leaf_sum_ms=stats(totals), excluded_parent_scopes=sorted(set(passes)-leaves),
                limitation='Per publication batch, not per rendered frame. Missing passes are unobserved, not proven zero work. Sibling sums are measured scope work, not GPU frame elapsed or utilization. Bevy latest-batch delivery can omit GPU frames; no CPU hitch/GPU frame correlation.')


def frame_accounting(frames):
    """Disjoint main-thread wall partition; details and other threads stay nested."""
    phases = ('simulation_input_ms','unit_mesh_ms','fragments_ms','presentation_ms')
    values = {k: [] for k in (*phases, 'unmarked_update_ms', 'outside_update_ms')}
    if len(frames) < 2 or any(not r.get(k) for r in frames for k in (*phases,'frame_start_ns','frame_end_ns')):
        return dict(status='unavailable')
    wall = 0.0
    for row, following in zip(frames,frames[1:]):
        update = (int(row['frame_end_ns'])-int(row['frame_start_ns']))/1e6
        outside = (int(following['frame_start_ns'])-int(row['frame_end_ns']))/1e6
        parts = [float(row[k]) for k in phases]
        if any(not math.isfinite(v) or v < 0 for v in parts) or outside < 0 or sum(parts) > update+0.005:
            raise ValueError('Main wall partition overlaps or is invalid')
        for key,value in zip(phases,parts):
            values[key].append(value)
        values['unmarked_update_ms'].append(max(0.0, update-sum(parts)))
        values['outside_update_ms'].append(outside)
        wall += update+outside
    return dict(status='partitioned', intervals=len(frames)-1, wall_ms=wall,
                parts={k: dict(**stats(v), total_ms=sum(v), wall_fraction=sum(v)/wall if wall else None) for k,v in values.items()},
                limitation='Outside Update includes other Bevy work, extraction/handoff, OS scheduling and waits. It is not idle or GPU time. Terminal slice excluded; nested core/detail and parallel render timings must not be added.')
