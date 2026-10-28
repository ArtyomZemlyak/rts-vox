"""Reproduce bounded, sequential external converter tests (stdlib only)."""
import argparse
import json
from pathlib import Path
import subprocess
import time

from art_pipeline import read_vox, stats


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=Path('out/art-lab-401'))
    parser.add_argument('--tools', type=Path, default=Path.home() / 'Tools/rts-vox-art')
    args = parser.parse_args()
    out = args.out.resolve()
    tools = args.tools.resolve()
    exe = next((tools / 'voxconvert-0.6.0').rglob('vengi-voxconvert.exe'))
    tests = []
    def run(name, source, target, extra=()):
        command = [str(exe), '--force', *extra, '--input', str(source), '--output', str(target)]
        started = time.perf_counter()
        try:
            p = subprocess.run(command, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=45)
            log = p.stdout.decode('utf-8', errors='replace')
            record = dict(name=name, command=command, seconds=round(time.perf_counter()-started, 4), exit=p.returncode)
        except subprocess.TimeoutExpired as e:
            log = (e.stdout or b'').decode('utf-8', errors='replace')
            record = dict(name=name, command=command, seconds=45, timeout=True)
        (out / f'{name}.log').write_text(log, encoding='utf-8')
        if target.exists():
            record['bytes'] = target.stat().st_size
            if target.suffix == '.vox':
                record['voxel_stats'] = stats(target)
        tests.append(record)
        print(json.dumps(record), flush=True)
        return record
    source = out / 'exact-pack/units/art_lab_scout.vox'
    run('vox-roundtrip', source, out / 'scout-roundtrip.vox')
    run('vox-glb', source, out / 'scout-vengi.glb')
    run('vox-obj', source, out / 'scout-vengi.obj')
    run('glb-vox', out / 'scout-vengi.glb', out / 'scout-from-glb.vox', ['--merge'])
    run('solid-glb', out / 'solid-probe.vox', out / 'solid-probe.glb')
    for mode in (0, 1):
        run(f'solid-back-{mode}', out / 'solid-probe.glb', out / f'solid-back-{mode}.vox',
            ['--merge', '-set', 'voxformat_voxelizemode', str(mode), '-set', 'voxformat_fillhollow', 'true'])
    if (out / 'blockbench-probe.glb').exists():
        run('blockbench-vox', out / 'blockbench-probe.glb', out / 'blockbench-probe.vox',
            ['--merge', '-set', 'voxformat_scale', '16', '-set', 'voxformat_fillhollow', 'true'])
    mv = next((tools / 'magicavoxel-0.99.7.2').rglob('3x3x3.vox'))
    run('magicavoxel-roundtrip', mv, out / 'magicavoxel-3x3x3.vox')
    # Keep third-party samples under out; do not add them to game content.
    original_size, original = read_vox(source)[0][0]
    comparisons = []
    for name in ('scout-roundtrip.vox', 'scout-from-glb.vox'):
        models, _ = read_vox(out / name)
        size, cells = models[0]
        comparisons.append(dict(file=name, size_equal=size == original_size,
            occupancy_equal=set(cells) == set(original), role_indices_equal=cells == original,
            missing=len(set(original)-set(cells)), added=len(set(cells)-set(original)), models=len(models)))
    (out / 'conversion-results.json').write_text(json.dumps(dict(tests=tests, comparisons=comparisons), indent=2), encoding='utf-8')


if __name__ == '__main__':
    main()
