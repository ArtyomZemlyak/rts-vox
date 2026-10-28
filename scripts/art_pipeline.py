"""Offline art-lab helpers. No runtime dependencies or changes to the game's grid.

python scripts/art_pipeline.py --out out/art-lab-401
The JSON cuboids are a test asset, not a replacement for the stock roster.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import struct

ROLES = {'Armour': 1, 'Core': 2, 'Engine': 3, 'Weapon': 4, 'Sensor': 5}
COLORS = [(88, 108, 99, 255), (245, 152, 55, 255), (58, 68, 76, 255),
          (118, 130, 133, 255), (67, 204, 206, 255)]


def chunk(tag, content=b'', children=b''):
    return tag + struct.pack('<II', len(content), len(children)) + content + children


def write_vox(path, size, cells):
    """Engine Y-up -> VOX Z-up; exact role indices, explicit editor palette."""
    if not all(0 < n <= 128 for n in size) or not 0 < len(cells) <= 1 << 20:
        raise ValueError('outside current game importer limits')
    records = []
    for (x, y, z), ci in sorted(cells.items(), key=lambda p: (p[0][2], p[0][1], p[0][0])):
        if not (0 <= x < size[0] and 0 <= y < size[1] and 0 <= z < size[2] and 1 <= ci <= 255):
            raise ValueError('invalid cell')
        records.append(bytes((x, z, y, ci)))
    palette = COLORS + [(128, 128, 128, 255)] * (256 - len(COLORS))
    children = chunk(b'SIZE', struct.pack('<III', size[0], size[2], size[1]))
    children += chunk(b'XYZI', struct.pack('<I', len(records)) + b''.join(records))
    children += chunk(b'RGBA', b''.join(bytes(c) for c in palette))
    Path(path).write_bytes(b'VOX ' + struct.pack('<I', 150) + chunk(b'MAIN', children=children))


def read_vox(path):
    """Inspect ALL models; deliberately does not flatten scene transforms."""
    data = Path(path).read_bytes()
    if data[:4] != b'VOX ' or len(data) < 20:
        raise ValueError('not a VOX file')
    models, pending, palette = [], None, None
    offset = 20
    while offset + 12 <= len(data):
        tag, size, children = struct.unpack_from('<4sII', data, offset)
        start = offset + 12
        if start + size + children > len(data):
            raise ValueError('truncated chunk')
        if tag == b'SIZE':
            x, z, y = struct.unpack_from('<III', data, start)
            pending = (x, y, z)
        elif tag == b'XYZI':
            count, = struct.unpack_from('<I', data, start)
            if pending is None or size != 4 + count * 4:
                raise ValueError('invalid model')
            cells = {}
            for i in range(count):
                x, z, y, ci = struct.unpack_from('<BBBB', data, start + 4 + i * 4)
                if (x, y, z) in cells:
                    raise ValueError('duplicate cell')
                cells[x, y, z] = ci
            models.append((pending, cells))
            pending = None
        elif tag == b'RGBA':
            palette = [tuple(data[start + i * 4:start + i * 4 + 4]) for i in range(256)]
        offset = start + size + children
    return models, palette


def rasterize_boxes(boxes):
    cells = {}
    for box in sorted(boxes, key=lambda b: (b['role'] != 'Armour', b['name'])):
        ci = ROLES[box['role']]
        lo, hi = box['min'], box['max']
        if any(type(v) is not int for v in lo + hi) or any(a >= b for a, b in zip(lo, hi)):
            raise ValueError('cuboids require integer, half-open bounds')
        if len(lo) != 3 or len(hi) != 3 or any(a < 0 or b > 128 for a, b in zip(lo, hi)):
            raise ValueError('cuboid outside current importer bounds; rejected before rasterization')
        for z in range(lo[2], hi[2]):
            for y in range(lo[1], hi[1]):
                for x in range(lo[0], hi[0]):
                    old = cells.get((x, y, z), 1)
                    if old not in (1, ci):
                        raise ValueError(f'conflicting subsystem roles at {(x,y,z)}')
                    cells[x, y, z] = ci
                if len(cells) > 1 << 20:
                    raise ValueError('too many cells for the current importer')
    return cells


def write_pack(out, boxes, name='art_lab_scout', core_size=2):
    out = Path(out)
    units = out / 'units'
    if not name or not name.isascii() or any(not (c.isalnum() or c in '_-') for c in name):
        raise ValueError('asset name must be an ASCII filename stem')
    cells = rasterize_boxes(boxes)
    if not cells:
        raise ValueError('empty model')
    if any(min(p) < 0 for p in cells):
        raise ValueError('negative engine coordinates: preserve the authoring origin')
    size = tuple(max(p[i] for p in cells) + 1 for i in range(3))
    if not all(0 < n <= 128 for n in size) or len(cells) > 1 << 20:
        raise ValueError('outside current game importer limits')
    core = [p for p, ci in cells.items() if ci == ROLES['Core']]
    if not core or type(core_size) is not int or not 0 < core_size <= min(size):
        raise ValueError('missing or invalid core')
    lo = tuple(min(p[i] for p in core) for i in range(3))
    if any(cells.get((x, y, z)) != ROLES['Core']
           for z in range(lo[2], lo[2]+core_size)
           for y in range(lo[1], lo[1]+core_size)
           for x in range(lo[0], lo[0]+core_size)):
        raise ValueError('core is not the complete cube required by UnitBody.seat_core')
    units.mkdir(parents=True, exist_ok=True)
    write_vox(units / f'{name}.vox', size, cells)
    roles = ', '.join(f'({ci}, {role})' for role, ci in ROLES.items())
    (units / f'{name}.ron').write_text(
        f'(name: "Art lab scout", model: "units/{name}.vox",\n'
        f' palette: [{roles}], core_size: {core_size})\n', encoding='utf-8')
    return size, cells


def fixture():
    boxes = []
    def add(name, role, lo, hi):
        boxes.append(dict(name=name, role=role, min=list(lo), max=list(hi)))
    add('hull', 'Armour', (3, 3, 3), (29, 9, 19))
    add('deck', 'Armour', (6, 9, 5), (26, 11, 17))
    for z in (0, 19):
        add(f'track_{z}', 'Armour', (2, 0, z), (30, 5, z+3))
        for x in (4, 10, 16, 22, 28):
            add(f'track_shoe_{x}_{z}', 'Armour', (x, 0, z), (x+1, 6, z+3))
    add('turret', 'Armour', (12, 11, 7), (24, 15, 15))
    add('barrel', 'Weapon', (22, 12, 10), (36, 14, 12))
    add('mantlet', 'Weapon', (21, 11, 9), (25, 15, 13))
    add('core', 'Core', (14, 5, 10), (16, 7, 12))
    add('engine', 'Engine', (4, 4, 6), (9, 8, 16))
    add('sensor', 'Sensor', (14, 15, 9), (17, 16, 13))
    return boxes


def stats(path):
    models, palette = read_vox(path)
    return dict(path=str(path), bytes=Path(path).stat().st_size, models=[dict(
        size=size, cells=len(cells), indices=dict(sorted(Counter(cells.values()).items())),
        colors={str(ci): palette[ci-1] for ci in sorted(set(cells.values()))} if palette else None
    ) for size, cells in models])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    boxes = fixture()
    source = dict(cell_m=0.1, axes='engine: x right, y up, z back', boxes=boxes)
    (args.out / 'scout-source.json').write_text(json.dumps(source, indent=2), encoding='utf-8')
    size, cells = write_pack(args.out / 'exact-pack', boxes)
    # Asymmetric solid probe exposes axis, cell-centre and interior-fill errors.
    probe = {(x, y, z): 1 for x in range(8) for y in range(5) for z in range(3)}
    probe[7, 4, 2] = 5
    write_vox(args.out / 'solid-probe.vox', (8, 5, 3), probe)
    print(json.dumps(dict(size=size, cells=len(cells), role_counts=Counter(cells.values()))))


if __name__ == '__main__':
    main()
