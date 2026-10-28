"""Blender background test: source -> strict VOX pack / GLB / OBJ / preview.

blender --background --factory-startup --threads 4 --python scripts/art_lab_blender.py -- --out out/art-lab-401
Uses only bundled bpy and the local stdlib helpers. No plugins or preferences changed.
"""
import argparse
import json
import math
from pathlib import Path
import sys
import time

import bpy
from mathutils import Vector

sys.path.insert(0, str(Path(__file__).resolve().parent))
from art_pipeline import COLORS, ROLES, read_vox
from blender_rts_export import export_pack


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--render', action='store_true')
    args = parser.parse_args(sys.argv[sys.argv.index('--') + 1:])
    out = args.out.resolve()
    src = json.loads((out / 'scout-source.json').read_text(encoding='utf-8'))
    cell = src['cell_m']
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete(use_global=False)
    materials = {}
    for role, ci in ROLES.items():
        mat = bpy.data.materials.new(role)
        # Palette entries are sRGB; Blender's shader inputs are linear.
        rgb = [((v / 255 + .055) / 1.055) ** 2.4 if v > 10 else v / 3294.6 for v in COLORS[ci-1][:3]]
        mat.diffuse_color = (*rgb, 1)
        mat.use_nodes = True
        shader = mat.node_tree.nodes.get('Principled BSDF')
        shader.inputs['Base Color'].default_value = (*rgb, 1)
        shader.inputs['Roughness'].default_value = .72
        shader.inputs['Metallic'].default_value = .3
        materials[role] = mat
    for box in src['boxes']:
        lo, hi = box['min'], box['max']
        center = [(a+b)*cell/2 for a,b in zip(lo, hi)]
        bpy.ops.mesh.primitive_cube_add(size=1, location=(center[0], -center[2], center[1]))
        obj = bpy.context.object
        obj.name = box['name']
        obj.dimensions = ((hi[0]-lo[0])*cell, (hi[2]-lo[2])*cell, (hi[1]-lo[1])*cell)
        obj['rts_role'] = box['role']
        obj.data.materials.append(materials[box['role']])
    bpy.context.view_layer.update()
    start = time.perf_counter()
    size, cells = export_pack(out / 'blender-pack', cell)
    expected_size, expected = read_vox(out / 'exact-pack/units/art_lab_scout.vox')[0][0]
    assert size == expected_size and cells == expected, 'exact Blender export differs from the fixed-grid source'
    checks = {'version': bpy.app.version_string, 'exact_export_seconds': time.perf_counter()-start,
              'size': size, 'cells': len(cells), 'exact_roles_and_coordinates': True}
    # Reject a moved-off-grid part and a rotated box instead of silently making it huge armour.
    obj = bpy.data.objects['sensor']
    location = obj.location.copy()
    obj.location.x += cell * .25
    bpy.context.view_layer.update()
    try:
        export_pack(out / 'must-not-export', cell)
        raise AssertionError('off-grid object accepted')
    except ValueError:
        checks['reject_off_grid'] = True
    obj.location = location
    obj.rotation_euler.z = math.radians(25)
    bpy.context.view_layer.update()
    try:
        export_pack(out / 'must-not-export', cell)
        raise AssertionError('rotated object accepted')
    except ValueError:
        checks['reject_rotated_box'] = True
    obj.rotation_euler.z = 0
    bpy.context.view_layer.update()
    bpy.ops.export_scene.gltf(filepath=str(out / 'scout-blender.glb'), export_format='GLB', export_extras=True)
    bpy.ops.wm.obj_export(filepath=str(out / 'scout-blender.obj'))
    bpy.context.scene['rts_cell_mm'] = round(cell*1000)
    note=bpy.data.texts.new('RTS_EXPORT_README')
    note.write('Art-lab prototype; not a stock roster replacement.\n'
               'Keep corners on the existing 100 mm fixture grid. Each cuboid has rts_role.\n'
               'Use scripts/blender_rts_export.py in background mode; see report 401.\n'
               'Body/weapon/load paths, articulated joints and finishes require game-side authoring.\n')
    bpy.ops.wm.save_as_mainfile(filepath=str(out / 'scout.blend'))
    # A separate surface-preview scene must not pollute the role-tagged authoring source.
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete(use_global=False)
    bpy.ops.import_scene.gltf(filepath=str(out / 'scout-vengi.glb'))
    visual = [obj for obj in bpy.context.scene.objects if obj.type == 'MESH']
    checks['vengi_glb_import_meshes'] = len(visual)
    checks['vengi_glb_import_triangles'] = sum(sum(len(p.vertices)-2 for p in obj.data.polygons) for obj in visual)
    bpy.ops.export_scene.gltf(filepath=str(out/'scout-vengi-clean.glb'), export_format='GLB')
    # Vengi GLB uses one unit per voxel, our Blender source uses metres.
    for obj in visual:
        if obj.parent is None:
            obj.scale *= cell
    bpy.context.view_layer.update()
    if args.render:
        points = [obj.matrix_world @ Vector(v) for obj in visual for v in obj.bound_box]
        low = Vector(tuple(min(v[i] for v in points) for i in range(3)))
        high = Vector(tuple(max(v[i] for v in points) for i in range(3)))
        target = (low + high) / 2
        extent = max(high-low)
        bpy.ops.mesh.primitive_plane_add(size=200, location=(target.x, target.y, low.z-.005))
        plane = bpy.context.object
        ground = bpy.data.materials.new('studio_ground')
        ground.diffuse_color = (.13, .16, .18, 1)
        plane.data.materials.append(ground)
        bpy.ops.object.camera_add(location=target+Vector((1.1,-1.45,.95))*extent)
        camera = bpy.context.object
        camera.rotation_euler = (target-camera.location).to_track_quat('-Z','Y').to_euler()
        camera.data.type = 'ORTHO'
        camera.data.ortho_scale = extent*1.48
        scene = bpy.context.scene
        scene.camera = camera
        scene.render.engine = 'CYCLES'
        scene.cycles.device = 'CPU'
        scene.cycles.samples = 12
        scene.render.threads_mode = 'FIXED'
        scene.render.threads = 4
        scene.render.resolution_x = 960
        scene.render.resolution_y = 640
        scene.render.resolution_percentage = 100
        scene.world.color = (.35,.35,.35)
        for position, power, size in [((1,-2,4),900,4),((-3,-1,2),450,3),((2,3,3),700,3)]:
            bpy.ops.object.light_add(type='AREA', location=target+Vector(position)*extent)
            lamp = bpy.context.object
            lamp.data.energy = power*extent*extent
            lamp.data.shape = 'DISK'
            lamp.data.size = size*extent
            lamp.rotation_euler = (target-lamp.location).to_track_quat('-Z','Y').to_euler()
        scene.render.filepath = str(out / 'scout-preview.png')
        start=time.perf_counter()
        bpy.ops.render.render(write_still=True)
        checks['render_seconds']=time.perf_counter()-start
        bpy.ops.wm.save_as_mainfile(filepath=str(out/'scout-preview.blend'))
    (out / 'blender-results.json').write_text(json.dumps(checks, indent=2), encoding='utf-8')
    print('RTS_ART_LAB '+json.dumps(checks))


if __name__ == '__main__':
    main()
