"""Rank disjoint bench_battle tick costs; keep nested details in separate tables."""
import argparse
import csv
import json
import math
import re
from collections import defaultdict
from pathlib import Path


PHASES = {
    'motion_us': 'Движение и навигация',
    'debris_us': 'Физика обломков и останков',
    'battle_orders_us': 'Приказы, прицеливание и стрельба',
    'rubble_us': 'Карта завалов и контакты с обломками',
    'collapse_us': 'Обрушение и применение правок мира',
    'battle_frontier_us': 'Фронт взрывов, отбор и чтение контактов',
    'battle_read_us': 'Чтение контактов волн',
    'battle_wave_step_us': 'Принятие контактов боевых волн',
    'battle_capture_us': 'Захват среды боевых волн',
    'battle_damage_us': 'Попадания и урон боевых снарядов',
    'battle_flight_us': 'Полёт боевых снарядов',
    'heat_exchange_us': 'Обмен теплом',
    'heat_flush_us': 'Применение тепловых изменений',
    'unit_fire_us': 'Горение юнитов',
    'battle_ai_us': 'Выбор решений ИИ',
    'kinetic_resume_us': 'Продолжение кинетических попаданий',
    'flight_us': 'Полёт полигонного снаряда',
    'capture_us': 'Захват среды полигонной волны',
    'wave_us': 'Полигонная волна и прочая работа волн',
    'accounting_us': 'Учёт и завершение тика',
}
DETAILS = {
    'motion_us': ('route_us', 'commit_us', 'feet_us', 'contact_us', 'felling_us'),
    'battle_orders_us': ('battle_pose_us', 'battle_los_us', 'battle_laser_us'),
    'battle_laser_us': ('battle_beam_us', 'battle_halo_us'),
    'collapse_us': tuple('collapse_' + name + '_us' for name in
                         ('admit', 'edits', 'labels', 'promote', 'resolve',
                          'detach', 'crumble', 'plate', 'rest')),
}
COUNTERS = ('searching', 'moving', 'blocked', 'fragments', 'flying', 'chunks',
            'felling', 'collapse_queued', 'collapse_buffer_peak',
            'felling_head_observed_age_ticks', 'collapse_backlog_observed_age_ticks',
            'pending_kinetic', 'waves', 'waves_held')


def stats(values):
    values = sorted(values)
    if not values:
        raise ValueError('empty sample')
    if any(not math.isfinite(v) or v < 0 for v in values):
        raise ValueError('invalid duration')
    n = len(values)
    return dict(samples=n, mean=sum(values) / n, p50=values[n // 2],
                p95=values[min(n - 1, n * 95 // 100)],
                p99=values[min(n - 1, n * 99 // 100)], max=values[-1])


def read_rows(path):
    with path.open(encoding='utf-8-sig', newline='') as f:
        rows = [{k: int(v) for k, v in r.items()} for r in csv.DictReader(f)]
    if not rows:
        raise ValueError(f'empty run: {path}')
    for row in rows:
        missing = (set(PHASES) | {'total_us', 'observed_phases_us'}) - row.keys()
        if missing:
            raise ValueError(f'{path}: missing {sorted(missing)}')
        known = sum(row[k] for k in PHASES)
        observed = row['observed_phases_us']
        total = row['total_us']
        if min(row.values()) < 0 or known > observed or observed > total:
            raise ValueError(f'{path}: non-additive tick {row.get("tick")}')
        row['observed_other_us'] = observed - known
        row['unobserved_us'] = total - observed
    return rows


def rank(rows):
    total = stats([r['total_us'] / 1000 for r in rows])
    if total['mean'] == 0:
        raise ValueError('tick timing has zero total duration')
    columns = PHASES | {'observed_other_us': 'Прочие размеченные интервалы',
                        'unobserved_us': 'Неразмеченная часть и наблюдатель'}
    phases = []
    for key, label in columns.items():
        s = stats([r[key] / 1000 for r in rows])
        phases.append(dict(key=key, label=label, share_pct=100 * s['mean'] / total['mean'], **s))
    phases.sort(key=lambda r: (-r['mean'], r['key']))
    if abs(sum(r['mean'] for r in phases) - total['mean']) > 1e-8:
        raise ValueError('top-level phases do not reconcile')
    return total, phases


def analyze(paths):
    groups = defaultdict(list)
    for path in paths:
        scenario = re.sub(r'-[ab]$', '', path.stem)
        groups[scenario].append((path, read_rows(path)))
    result = {}
    for scenario, runs in sorted(groups.items()):
        first = runs[0][1]
        fields = [k for k in first[0] if not k.endswith('_us')]
        hashes = []
        for path, rows in runs:
            if rows[0].keys() != first[0].keys():
                raise ValueError(f'{scenario}: measurement schemas differ')
            if len(rows) != len(first) or any(
                any(a[k] != b[k] for k in fields) for a, b in zip(first, rows)
            ):
                raise ValueError(f'{scenario}: non-timing counters differ')
            log = path.with_suffix('.log').read_text(encoding='utf-8-sig')
            found = re.search(r'^ticks=.*\bhash=([0-9a-f]{16})', log, re.M)
            if not found:
                raise ValueError(f'{path}: missing endpoint')
            hashes.append(found[1])
        if len(set(hashes)) != 1:
            raise ValueError(f'{scenario}: endpoint hashes differ')
        rows = [r for _, run in runs for r in run]
        total, phases = rank(rows)
        details = {parent: {key: stats([r[key] / 1000 for r in rows])
                            for key in keys} for parent, keys in DETAILS.items()}
        windows = []
        for begin in range(0, len(first), 600):
            window = [r for _, run in runs for r in run[begin:begin + 600]]
            t, p = rank(window)
            windows.append(dict(first_tick=window[0]['tick'], last_tick=window[-1]['tick'],
                                total=t, phases=p))
        result[scenario] = dict(runs=len(runs), ticks_per_run=len(first), total_ms=total,
            phases_ms=phases, details_ms=details, windows=windows, endpoint=hashes[0],
            over_33_ms=sum(r['total_us'] > 33333 for r in rows),
            counters={k: dict(mean=sum(r[k] for r in rows)/len(rows),
                              peak=max(r[k] for r in rows), last=first[-1][k])
                      for k in COUNTERS})
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    paths = sorted(args.directory.glob('*-[ab].csv'))
    if not paths:
        parser.error('no *-[ab].csv runs')
    result = analyze(paths)
    (args.directory / 'headless-analysis.json').write_text(
        json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    columns = ['scenario', 'key', 'label', 'unit', 'mean', 'share_pct', 'p95', 'p99', 'max']
    with (args.directory / 'headless-ranking.csv').open('w', encoding='utf-8-sig', newline='') as f:
        writer = csv.DictWriter(f, fieldnames=columns, extrasaction='ignore')
        writer.writeheader()
        for scene, data in result.items():
            for row in data['phases_ms']:
                writer.writerow(dict(scenario=scene, unit='ms/tick', **row))
    for scene, data in result.items():
        print(f'{scene}: runs={data["runs"]} ticks={data["ticks_per_run"]} '
              f'mean={data["total_ms"]["mean"]:.3f}ms '
              f'p99={data["total_ms"]["p99"]:.3f}ms hash={data["endpoint"]}')
        for row in data['phases_ms'][:6]:
            print(f'  {row["key"]}: {row["mean"]:.3f}ms {row["share_pct"]:.1f}%')


if __name__ == '__main__':
    main()
