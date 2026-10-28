"""Pack and verify Blender RGBA frames using the already bundled FFmpeg."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--out', type=Path, default=Path('out/art-lab-402/blender-fx'))
    args = parser.parse_args()
    out = args.out.resolve()
    root = Path(__file__).resolve().parents[1]
    exe = root/'cache/video-tools/imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe'
    if not exe.is_file():
        raise FileNotFoundError('Existing video-tools FFmpeg is required; see report 402')
    common = [str(exe), '-hide_banner', '-loglevel', 'error', '-y', '-threads', '2', '-filter_threads', '2']
    pattern = str(out/'frames/fx_%03d.png')
    def run(arguments):
        result = subprocess.run(common+arguments, capture_output=True, timeout=20)
        if result.returncode:
            raise RuntimeError(result.stderr.decode('utf-8', errors='replace'))
        return result.stdout
    raw = run(['-i',pattern,'-f','rawvideo','-pix_fmt','rgba','pipe:1'])
    framebytes = 256*256*4
    if len(raw) != 8*framebytes:
        raise ValueError('expected eight 256-square RGBA frames')
    checks = []
    for i in range(8):
        frame = raw[i*framebytes:(i+1)*framebytes]
        alpha = frame[3::4]
        visible = sum(v > 0 for v in alpha)
        partial = sum(0 < v < 255 for v in alpha)
        if not visible or not partial or min(alpha) != 0:
            raise ValueError(f'frame {i}: empty image or missing transparency')
        checks.append(dict(stage=i,visible_pixels=visible,partial_alpha_pixels=partial,
                           max_alpha=max(alpha),sha256=hashlib.sha256(frame).hexdigest()))
    if len({item['sha256'] for item in checks}) != 8:
        raise ValueError('duplicate animation frames')
    run(['-framerate','3','-i',pattern,'-vf','tile=4x2','-frames:v','1','-pix_fmt','rgba',str(out/'atlas.png')])
    run(['-f','lavfi','-i','color=c=0x1c2430:s=256x256:r=3:d=2.666667','-framerate','3','-i',pattern,
         '-filter_complex_threads','2','-filter_complex',
         '[0:v][1:v]overlay=shortest=1:format=auto,split[a][b];[a]palettegen[p];[b][p]paletteuse',
         '-loop','0',str(out/'preview.gif')])
    (out/'image-checks.json').write_text(json.dumps(checks,indent=2),encoding='utf-8')
    (out/'atlas.json').write_text(json.dumps(dict(frames=8,columns=4,rows=2,tile_size=[256,256],
        atlas_size=[1024,512],order='left-to-right, top-to-bottom',alpha='straight PNG RGBA8',
        example_playback_fps=3,rgba8_bytes=1024*512*4),indent=2),encoding='utf-8')
    print('Eight distinct RGBA frames, atlas and animated preview: PASS')


if __name__ == '__main__':
    main()
