"""Bounded Blender effect authoring experiment; no fluid bake or game changes.

blender --background --factory-startup --threads 4 --python-exit-code 1
  --python scripts/art_lab_effects.py -- --out out/art-lab-402/blender-fx
"""
import argparse
import json
import math
from pathlib import Path
import sys
import time

import bpy
from mathutils import Vector


def material(name, rgb, opacity, emission):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    nodes = m.node_tree.nodes
    shader = nodes.get('Principled BSDF')
    shader.inputs['Base Color'].default_value = (*rgb, 1)
    shader.inputs['Roughness'].default_value = .95
    shader.inputs['Emission Color'].default_value = (*rgb, 1)
    shader.inputs['Emission Strength'].default_value = emission
    transparent = nodes.new('ShaderNodeBsdfTransparent')
    mix = nodes.new('ShaderNodeMixShader')
    mix.inputs[0].default_value = opacity
    links = m.node_tree.links
    links.new(transparent.outputs[0], mix.inputs[1])
    links.new(shader.outputs[0], mix.inputs[2])
    links.new(mix.outputs[0], nodes.get('Material Output').inputs['Surface'])
    return m


def cloud(stage):
    edge = .6
    phase = stage / 7
    h = 1 + phase * 6.5
    radius = .7 + phase * 3.1
    occupied = {}
    for z in range(18):
        for y in range(-8, 9):
            for x in range(-8, 9):
                px, py, pz = ((x+.5)*edge, (y+.5)*edge, (z+.5)*edge)
                r = math.hypot(px, py)
                noise = math.sin(px*2.7+phase*3)*math.sin(py*3.1-pz*1.3)*.12
                initial = stage <= 2 and (r/(.85+phase*3))**2 + ((pz-(.9+phase))/(.9+phase))**2 < 1+noise
                stem = stage > 0 and pz < h and r < (.5+.35*phase+noise)*(1+.3*math.sin(pz*2-phase))
                cap = stage > 0 and (r/radius)**2 + ((pz-h)/(.7+phase*.55))**2 < 1+noise
                dust = stage > 0 and pz < .6 and abs(r-(.8+phase*4.3)) < .32
                if initial or stem or cap or dust:
                    heat = stage <= 2 and (initial or (stem and pz < h*.5))
                    occupied[x, y, z] = 0 if heat else 1 + ((x*17+y*7+z*13) % 2)
    vertices, faces, indices = [], [], []
    directions = [((1,0,0),[(1,0,0),(1,1,0),(1,1,1),(1,0,1)]),
                  ((-1,0,0),[(0,0,0),(0,0,1),(0,1,1),(0,1,0)]),
                  ((0,1,0),[(0,1,0),(0,1,1),(1,1,1),(1,1,0)]),
                  ((0,-1,0),[(0,0,0),(1,0,0),(1,0,1),(0,0,1)]),
                  ((0,0,1),[(0,0,1),(1,0,1),(1,1,1),(0,1,1)]),
                  ((0,0,-1),[(0,0,0),(0,1,0),(1,1,0),(1,0,0)])]
    for p, ci in occupied.items():
        for offset, corners in directions:
            if tuple(p[i]+offset[i] for i in range(3)) in occupied:
                continue
            start = len(vertices)
            vertices.extend(tuple((p[i]+corner[i])*edge for i in range(3)) for corner in corners)
            faces.append(tuple(range(start, start+4)))
            indices.append(ci)
    mesh = bpy.data.meshes.new(f'cloud_stage_{stage}')
    mesh.from_pydata(vertices, [], faces)
    mesh.update()
    obj = bpy.data.objects.new(mesh.name, mesh)
    bpy.context.collection.objects.link(obj)
    alpha = .58 - .38*phase
    for m in (material(f'fire_{stage}',(1,.32,.035),.8,3.5*(1-phase)),
              material(f'smoke_light_{stage}',(.32,.34,.36),alpha,0),
              material(f'smoke_dark_{stage}',(.16,.18,.21),alpha,0)):
        mesh.materials.append(m)
    for polygon, ci in zip(mesh.polygons, indices):
        polygon.material_index = ci
    frame = stage*8+1
    for key, hidden in ((1,True),(frame,False),(frame+8,True)):
        obj.hide_render = hidden
        obj.hide_viewport = hidden
        obj.keyframe_insert(data_path='hide_render', frame=key)
        obj.keyframe_insert(data_path='hide_viewport', frame=key)
    return dict(stage=stage, frame=frame, cells=len(occupied), quads=len(faces), opacity=alpha)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args(sys.argv[sys.argv.index('--')+1:])
    out = args.out.resolve()
    (out/'frames').mkdir(parents=True, exist_ok=True)
    bpy.ops.object.select_all(action='SELECT')
    bpy.ops.object.delete(use_global=False)
    stages = [cloud(i) for i in range(8)]
    scene = bpy.context.scene
    scene.render.engine = 'CYCLES'
    scene.cycles.device = 'CPU'
    scene.cycles.samples = 64
    scene.cycles.use_denoising = False
    scene.render.threads_mode = 'FIXED'
    scene.render.threads = 4
    scene.render.resolution_x = 256
    scene.render.resolution_y = 256
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = 'PNG'
    scene.render.image_settings.color_mode = 'RGBA'
    scene.render.image_settings.color_depth = '8'
    scene.render.film_transparent = True
    scene.render.fps = 24
    scene.frame_start, scene.frame_end = 1,64
    scene.world.color = (.22,.22,.22)
    target = Vector((0,0,4.3))
    bpy.ops.object.camera_add(location=(15,-22,13))
    camera = bpy.context.object
    camera.rotation_euler = (target-camera.location).to_track_quat('-Z','Y').to_euler()
    camera.data.type = 'ORTHO'
    camera.data.ortho_scale = 13.8
    scene.camera = camera
    bpy.ops.object.light_add(type='AREA', location=(3,-6,14))
    lamp = bpy.context.object
    lamp.data.energy = 2200
    lamp.data.size = 8
    lamp.rotation_euler = (target-lamp.location).to_track_quat('-Z','Y').to_euler()
    scene.frame_set(1)
    bpy.ops.wm.save_as_mainfile(filepath=str(out/'cubic-blast.blend'))
    started = time.perf_counter()
    for stage in stages:
        scene.frame_set(stage['frame'])
        scene.render.filepath = str(out/'frames'/f"fx_{stage['stage']:03}.png")
        tick = time.perf_counter()
        bpy.ops.render.render(write_still=True)
        stage['render_seconds'] = round(time.perf_counter()-tick,4)
    result = dict(blender=bpy.app.version_string, resolution=[256,256], frames=stages,
                  samples=64, threads=4, cpu=True, total_render_seconds=time.perf_counter()-started)
    (out/'results.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print('RTS_FX_LAB '+json.dumps(result))


if __name__ == '__main__':
    main()
