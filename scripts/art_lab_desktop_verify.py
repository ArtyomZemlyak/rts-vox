"""Validate files actually saved by the desktop editors in art lab 403.

This checks the recorded fixtures; it does not automate or simulate their GUIs.
Uses only stdlib, art_pipeline, and the existing bounded FFmpeg decoder.
"""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import struct
import subprocess

from art_pipeline import read_vox


def glb_info(path):
    data = path.read_bytes()
    magic, version, length = struct.unpack_from('<4sII', data)
    assert (magic, version, length) == (b'glTF', 2, len(data))
    size, tag = struct.unpack_from('<I4s', data, 12)
    assert tag == b'JSON'
    document = json.loads(data[20:20 + size])
    primitives = [p for m in document.get('meshes', []) for p in m['primitives']]
    triangles = 0
    for primitive in primitives:
        assert primitive.get('mode', 4) == 4
        index = primitive.get('indices', primitive['attributes']['POSITION'])
        count = document['accessors'][index]['count']
        assert count % 3 == 0
        triangles += count // 3
    return dict(meshes=len(document.get('meshes', [])), primitives=len(primitives),
                triangles=triangles, nodes=document.get('nodes', []))


def png_size(path):
    data = path.read_bytes()
    assert data[:8] == b'\x89PNG\r\n\x1a\n'
    return struct.unpack_from('>II', data, 16)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=Path('out/art-lab-403'))
    args = parser.parse_args()
    out = args.out
    source_models, palette = read_vox('out/art-lab-401/exact-pack/units/art_lab_scout.vox')
    original_size, original = source_models[0]
    assert len(source_models) == 1 and len(original) == 4302
    results = {}

    mv_models, mv_palette = read_vox(out / 'magicavoxel/scout-edited.vox')
    mv_size, mv_cells = mv_models[0]
    removed = sorted(set(original) - set(mv_cells))
    added = sorted(set(mv_cells) - set(original))
    assert len(mv_models) == 1 and mv_size == original_size
    assert len(removed) == 2 and not added and len(mv_cells) == 4300
    assert all(ci == original[pos] for pos, ci in mv_cells.items())
    assert all(palette[ci - 1] == mv_palette[ci - 1] for ci in set(original.values()))
    obj = (out / 'magicavoxel/scout-edited.obj').read_text().splitlines()
    faces = [line.split()[1:] for line in obj if line.startswith('f ')]
    assert faces and (out / 'magicavoxel/scout-edited.mtl').is_file()
    results['magicavoxel'] = dict(size=mv_size, cells=len(mv_cells), removed=removed,
        added=added, surviving_role_indices_exact=True, used_colors_exact=True,
        palette_changed_slots=[i + 1 for i, (a, b) in enumerate(zip(palette, mv_palette)) if a != b],
        obj_faces=len(faces), obj_triangles=sum(len(f) - 2 for f in faces),
        palette_texture_size=png_size(out / 'magicavoxel/scout-edited.png'))

    for name in ('scout-roundtrip.vox', 'scout-rotate360.vox'):
        models, pal = read_vox(out / 'voxedit' / name)
        assert len(models) == 1 and models[0] == (original_size, original)
        assert all(palette[ci - 1] == pal[ci - 1] for ci in set(original.values()))
    rotated, _ = read_vox(out / 'voxedit/scout-rotate90.vox')
    assert len(rotated) == 1 and len(rotated[0][1]) == len(original)
    assert Counter(rotated[0][1].values()) == Counter(original.values())
    mappings = {
        'x_to_minus_z': {(z, y, original_size[0] - 1 - x): ci for (x, y, z), ci in original.items()},
        'x_to_plus_z': {(original_size[2] - 1 - z, y, x): ci for (x, y, z), ci in original.items()},
    }
    rotation = next((name for name, cells in mappings.items() if cells == rotated[0][1]), None)
    assert rotation is not None
    vengi_glb = glb_info(out / 'voxedit/scout-editor.glb')
    assert vengi_glb['triangles'] == 244
    results['voxedit'] = dict(cells=len(original), roundtrip_exact=True, full_turn_exact=True,
        rotation90=rotation, rotated_size=rotated[0][0], glb=vengi_glb,
        viewport_capture='black; not a visual pass')

    source = json.loads((out / 'blockbench/probe-input.bbmodel').read_text())
    saved = json.loads((out / 'blockbench/probe-desktop.bbmodel').read_text())
    before, after = source['elements'][0], saved['elements'][0]
    assert len(saved['elements']) == 1 and before['vertices'] == after['vertices']
    assert before['faces'] == after['faces']
    assert before['origin'] == [0, 0, 0] and after['origin'] == [1, 0, 0]
    bb_glb = glb_info(out / 'blockbench/probe-desktop.glb')
    assert bb_glb['triangles'] == 12 and bb_glb['nodes'][0]['translation'] == [0.0625, 0, 0]
    results['blockbench'] = dict(meshes=1, vertices=len(after['vertices']), geometry_exact=True,
        saved_origin=after['origin'], export_scale=16, glb=bb_glb)

    atlas = out / 'effekseer/recorder-atlas.png'
    width, height = png_size(atlas)
    assert (width, height) == (1024, 2048)
    ffmpeg = Path('cache/video-tools/imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe')
    decoded = subprocess.run([str(ffmpeg), '-v', 'error', '-threads', '2', '-i', str(atlas),
        '-f', 'rawvideo', '-pix_fmt', 'rgba', '-threads', '2', 'pipe:1'],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=20, check=True).stdout
    assert len(decoded) == width * height * 4
    frames = []
    for i in range(32):
        column, row = i % 4, i // 4
        pixels = b''.join(decoded[((row * 256 + y) * width + column * 256) * 4:
            ((row * 256 + y) * width + (column + 1) * 256) * 4] for y in range(256))
        alpha = pixels[3::4]
        edge = (alpha[:256] + alpha[-256:] + alpha[::256] + alpha[255::256])
        frames.append(dict(index=i, sha256=hashlib.sha256(pixels).hexdigest(),
            alpha_min=min(alpha), alpha_max=max(alpha),
            nonzero_alpha=sum(a > 0 for a in alpha), partial_alpha=sum(0 < a < 255 for a in alpha),
            edge_alpha_max=max(edge)))
    unique = len({f['sha256'] for f in frames[:30]})
    results['effekseer_recorder'] = dict(size=[width, height], cell_size=[256, 256],
        recorded_frames=30, columns=4, rows=8, padding_cells=2,
        unique_frames=unique, nonempty_frames=sum(f['nonzero_alpha'] > 0 for f in frames[:30]),
        frames_touching_edge=sum(f['edge_alpha_max'] > 0 for f in frames[:30]),
        framing='cropped; requires a wider camera before production use',
        transparency='Generate alpha', frames=frames)
    (out / 'desktop-results.json').write_text(json.dumps(results, indent=2), encoding='utf-8')
    # An effect may fill a frame completely. That does not invalidate the PNG's
    # alpha channel, but edge coverage is reported as a separate framing issue.
    assert any(f['alpha_min'] == 0 and f['nonzero_alpha'] > 0 for f in frames[:30])
    assert all(f['nonzero_alpha'] > 0 for f in frames[:30])
    assert any(f['partial_alpha'] > 0 for f in frames[:30])
    assert all(f['nonzero_alpha'] == 0 for f in frames[30:])
    assert unique > 1
    print(json.dumps({k: {n: v for n, v in r.items() if n != 'frames'} for k, r in results.items()}, indent=2))


if __name__ == '__main__':
    main()
