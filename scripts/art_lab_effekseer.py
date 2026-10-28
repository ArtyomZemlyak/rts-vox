"""Bounded Effekseer CUI/API and GLB model compatibility checks, offline only."""
import argparse
import json
from pathlib import Path
import shutil
import struct
import subprocess
import time
import xml.etree.ElementTree as ET


def glb_info(path):
    data = path.read_bytes()
    if data[:4] != b'glTF':
        raise ValueError('not a GLB')
    count, = struct.unpack_from('<I',data,12)
    doc = json.loads(data[20:20+count])
    return dict(primitives=sum(len(m['primitives']) for m in doc.get('meshes',[])),
        meshes=len(doc.get('meshes',[])),
        triangles=sum(doc['accessors'][p['indices']]['count']//3 for m in doc.get('meshes',[]) for p in m['primitives']),
        extensions=doc.get('extensionsUsed',[]))


def model_info(path):
    data = path.read_bytes()
    version, scale, models, frames, vertices = struct.unpack_from('<ifiii',data)
    if version != 6 or frames != 1 or models != 1:
        raise ValueError('unexpected EFKMODEL version/layout for this test')
    triangles, = struct.unpack_from('<i',data,20+vertices*68)
    if len(data) != 24+vertices*68+triangles*12:
        raise ValueError('invalid EFKMODEL size')
    colors=[list(color) for color in sorted({tuple(data[20+i*68+64:20+i*68+68]) for i in range(vertices)})]
    return dict(version=version,scale=scale,vertices=vertices,triangles=triangles,colors=colors,bytes=len(data))


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out',type=Path,default=Path('out/art-lab-402/effekseer'))
    parser.add_argument('--tools',type=Path,default=Path.home()/'Tools/rts-vox-art')
    args=parser.parse_args()
    out=args.out.resolve()
    out.mkdir(parents=True,exist_ok=True)
    tools=args.tools.resolve()
    root=Path(__file__).resolve().parents[1]
    installed=tools/'effekseer-1.80.7/Effekseer1.80.7Win'
    exe=installed/'Tool/bin/Effekseer.exe'
    converter=installed/'Tool/bin/tools/EffekseerResourceConverter.exe'
    sample=installed/'Sample/00_Basic/Laser01.efkefc'
    calls=[]
    def run(name,command,cwd=None):
        started=time.perf_counter()
        process=subprocess.run([str(p) for p in command],cwd=cwd,stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,timeout=25,creationflags=getattr(subprocess,'CREATE_NO_WINDOW',0))
        (out/(name+'.log')).write_bytes(process.stdout)
        calls.append(dict(name=name,command=[str(p) for p in command],exit=process.returncode,
                          seconds=round(time.perf_counter()-started,4)))
        if process.returncode:
            raise RuntimeError(f'{name}: see log')
    if not sample.is_file():
        raise FileNotFoundError(sample)
    run('roundtrip',[exe,'-cui','-in',sample,'-o',out/'laser-roundtrip.efkefc','-e',out/'laser-runtime.efk'],exe.parent)
    if not (out/'laser-roundtrip.efkefc').read_bytes().startswith(b'EFKE'):
        raise ValueError('missing editor project')
    run('reimport',[exe,'-cui','-in',out/'laser-roundtrip.efkefc','-e',out/'laser-runtime-reimport.efk'],exe.parent)
    if (out/'laser-runtime.efk').read_bytes() != (out/'laser-runtime-reimport.efk').read_bytes():
        raise ValueError('runtime effect changed across save/reload')
    pwsh=shutil.which('pwsh')
    if not pwsh:
        raise FileNotFoundError('PowerShell 7 is required for the public Core API probe')
    run('xml-api',[pwsh,'-NoProfile','-File',root/'scripts/art-effekseer-xml.ps1',
        '-InputFile',out/'laser-roundtrip.efkefc','-OutputFile',out/'laser-readable.efkproj','-Tools',tools])
    tree=ET.parse(out/'laser-readable.efkproj')
    nodes=tree.findall('.//Node')
    if len(nodes)!=5:
        raise ValueError('expected five imported effect nodes')
    # Edit a copy and make its texture references self-contained before exporting.
    texture_dir=out/'Texture'
    texture_dir.mkdir(exist_ok=True)
    textures=set()
    for element in tree.iter():
        if element.tag.endswith('Texture') and element.text and element.text.endswith('.png'):
            source=(out/element.text).resolve()
            if not source.is_file():
                raise FileNotFoundError(source)
            shutil.copyfile(source,texture_dir/source.name)
            element.text='Texture/'+source.name
            textures.add(source.name)
    for index,node in enumerate(nodes):
        name=node.find('Name')
        if name is None:
            name=ET.SubElement(node,'Name')
        name.text=f'RTS_Lab_{index}'
    life=nodes[0].find('./CommonValues/Life')
    for name in ('Center','Max','Min'):
        life.find(name).text='30'
    nodes[0].find('./CommonValues/MaxGeneration/Value').text='12'
    tree.write(out/'edited-project.efkproj',encoding='utf-8',xml_declaration=True)
    run('edited-export',[exe,'-cui','-in',out/'edited-project.efkproj','-o',out/'edited-project.efkefc',
        '-e',out/'edited-runtime.efk'],exe.parent)
    run('edited-verify',[pwsh,'-NoProfile','-File',root/'scripts/art-effekseer-xml.ps1',
        '-InputFile',out/'edited-project.efkefc','-OutputFile',out/'edited-reloaded.efkproj','-Tools',tools])
    restored=ET.parse(out/'edited-reloaded.efkproj').findall('.//Node')
    if len(restored)!=5 or restored[0].find('./CommonValues/Life/Center').text!='30':
        raise ValueError('editor did not preserve the authored parameters')
    if restored[0].find('./CommonValues/MaxGeneration/Value').text!='12':
        raise ValueError('particle generation limit lost')
    if any(node.find('Name').text!=f'RTS_Lab_{i}' for i,node in enumerate(restored)):
        raise ValueError('node names lost')
    run('effect-glb',[exe,'-cui','-in',out/'edited-project.efkefc','-f','glb','-e',out/'laser-effect.glb'],exe.parent)
    effect_glb=glb_info(out/'laser-effect.glb')
    # Negative control: this version returns zero and exports a default effect!
    if (out/'not-existing-input.efkefc').exists():
        raise ValueError('negative-control input unexpectedly exists')
    run('missing-input',[exe,'-cui','-in',out/'not-existing-input.efkefc','-e',out/'negative-control.efk'],exe.parent)
    negative=(out/'negative-control.efk').exists()
    source=root/'out/art-lab-401/scout-vengi-clean.glb'
    run('model-multiple-primitives',[converter,source,'-o',out/'scout.efkmodel'])
    lossy=model_info(out/'scout.efkmodel')
    voxconvert=next((tools/'voxconvert-0.6.0').rglob('vengi-voxconvert.exe'))
    run('vertexcolor-export',[voxconvert,'--force','-set','voxformat_withmaterials','false',
        '-set','voxformat_withtexcoords','false','-set','voxformat_withnormals','true',
        '-set','voxformat_withcolor','true','--input',root/'out/art-lab-401/exact-pack/units/art_lab_scout.vox',
        '--output',out/'scout-vertexcolors.glb'])
    run('model-vertexcolors',[converter,out/'scout-vertexcolors.glb','-o',out/'scout-vertexcolors.efkmodel'])
    exact=model_info(out/'scout-vertexcolors.efkmodel')
    if exact['triangles']!=244 or exact['colors']!=[[67,204,206,255],[88,108,99,255],[118,130,133,255]]:
        raise ValueError('geometry or palette colors lost by workaround')
    result=dict(calls=calls,editor_nodes=5,runtime_exact_roundtrip=True,edited_life=30,
        self_contained_textures=sorted(textures),effect_glb=effect_glb,
        missing_input_false_success=negative,lossy_model=lossy,vertexcolor_model=exact)
    (out/'pipeline-results.json').write_text(json.dumps(result,indent=2),encoding='utf-8')
    print(json.dumps(dict(nodes=5,edited_life=30,textures=len(textures),runtime_roundtrip=True,
        effect_glb=effect_glb,lossy_triangles=lossy['triangles'],fixed_triangles=exact['triangles'],
        missing_input_false_success=negative)))


if __name__=='__main__':
    main()
