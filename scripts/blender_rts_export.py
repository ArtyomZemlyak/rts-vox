"""Strict Blender cuboid -> existing RTS VOX/RON format, offline only.

Objects must have custom string property rts_role (Armour/Core/Engine/Weapon/Sensor),
8 box vertices, no modifiers, and world-space corners on the unchanged cell grid.
Arbitrary meshes must go through a separate voxelizer; bounds are NEVER a substitute.
Usage inside Blender: export_pack(path, cell_m=0.1). No preference/add-on mutation.
"""
import argparse
import itertools
import sys
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parent))
from art_pipeline import ROLES, write_pack


def export_pack(path, cell_m, name='art_lab_scout', core_size=2):
    if cell_m <= 0:
        raise ValueError('cell size must be positive and match the target body')
    boxes = []
    for obj in sorted(bpy.context.scene.objects, key=lambda o: o.name):
        role = obj.get('rts_role')
        if role is None:
            continue
        if role not in ROLES or obj.type != 'MESH' or len(obj.data.vertices) != 8 or obj.modifiers:
            raise ValueError(f'{obj.name}: expected an unmodified role-tagged cuboid')
        points = []
        for vertex in obj.data.vertices:
            v = obj.matrix_world @ vertex.co
            raw = (v.x / cell_m, v.z / cell_m, -v.y / cell_m)
            p = tuple(round(c) for c in raw)
            if any(abs(c-i) > 1e-4 for c, i in zip(raw, p)):
                raise ValueError(f'{obj.name}: corner outside the fixed voxel grid')
            points.append(p)
        axes = [sorted({p[i] for p in points}) for i in range(3)]
        if any(len(axis) != 2 for axis in axes) or set(points) != set(itertools.product(*axes)):
            raise ValueError(f'{obj.name}: rotated or non-cuboid geometry')
        if len(obj.data.polygons) != 6 or any(len(face.vertices) != 4 for face in obj.data.polygons):
            raise ValueError(f'{obj.name}: expected six box faces')
        for face in obj.data.polygons:
            if not any(len({points[i][axis] for i in face.vertices}) == 1 for axis in range(3)):
                raise ValueError(f'{obj.name}: non-planar box face')
        boxes.append(dict(name=obj.name, role=role, min=[a[0] for a in axes], max=[a[1] for a in axes]))
    if not boxes:
        raise ValueError('no rts_role objects')
    return write_pack(path, boxes, name, core_size)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--cell-mm', type=int, required=True, help='Existing target body grid; this does not change game settings')
    parser.add_argument('--core-size', type=int, default=2)
    parser.add_argument('--name', default='art_lab_scout')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    size, cells = export_pack(args.out, args.cell_mm / 1000, args.name, args.core_size)
    print(f'RTS_EXPORT: {size}, {len(cells)} cells')
