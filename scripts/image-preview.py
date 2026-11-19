import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parent.parent
FFMPEG_FALLBACK = ROOT / "cache/video-tools/imageio_ffmpeg/binaries/ffmpeg-win-x86_64-v7.1.exe"


def ffmpeg_executable():
    executable = shutil.which("ffmpeg")
    if executable:
        return executable
    if FFMPEG_FALLBACK.is_file():
        return str(FFMPEG_FALLBACK)
    raise RuntimeError("FFmpeg not found on PATH or in cache/video-tools")


def preview_path(source):
    source = Path(source)
    return source.with_name(f"{source.stem}.preview.jpg")


def scale_filter(max_dimension):
    return (
        f"scale=w='min(iw,{max_dimension})':h='min(ih,{max_dimension})':"
        "force_original_aspect_ratio=decrease:force_divisible_by=2:flags=lanczos"
    )


def windows_preview(source, target, max_dimension):
    """Use Windows' image decoder when the optional FFmpeg cache is unavailable."""
    shell = shutil.which("powershell")
    if sys.platform != "win32" or not shell:
        raise RuntimeError("FFmpeg not found; Windows image decoder is unavailable")
    script = r"""
Add-Type -AssemblyName System.Drawing
$image = [Drawing.Image]::FromFile($env:RTS_PREVIEW_SOURCE)
try {
    $ratio = [Math]::Min(1.0, [double]$env:RTS_PREVIEW_BOUND / [Math]::Max($image.Width, $image.Height))
    $bitmap = New-Object Drawing.Bitmap ([Math]::Max(1, [int]($image.Width * $ratio))), ([Math]::Max(1, [int]($image.Height * $ratio)))
    try {
        $graphics = [Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([Drawing.Color]::Black)
            $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
            $graphics.DrawImage($image, 0, 0, $bitmap.Width, $bitmap.Height)
        } finally { $graphics.Dispose() }
        $bitmap.Save($env:RTS_PREVIEW_TARGET, [Drawing.Imaging.ImageFormat]::Jpeg)
    } finally { $bitmap.Dispose() }
} finally { $image.Dispose() }
"""
    env = os.environ.copy()
    env.update(RTS_PREVIEW_SOURCE=str(source), RTS_PREVIEW_TARGET=str(target),
               RTS_PREVIEW_BOUND=str(max_dimension))
    return subprocess.run([shell, "-NoProfile", "-NonInteractive", "-Command", script],
                          capture_output=True, text=True, encoding="utf-8", errors="replace",
                          timeout=120, env=env, creationflags=subprocess.CREATE_NO_WINDOW)


def create_preview(source, max_dimension=1920, quality=2):
    source = Path(source).resolve()
    if not source.is_file():
        raise ValueError(f"Image does not exist: {source}")
    target = preview_path(source)
    if target.is_file() and target.stat().st_size > 0 and target.stat().st_mtime_ns >= source.stat().st_mtime_ns:
        return target

    temporary = target.with_name(f".{target.stem}.{os.getpid()}.tmp.jpg")
    try:
        executable = ffmpeg_executable()
    except RuntimeError:
        executable = None
    command = [
        executable,
        "-hide_banner", "-loglevel", "error", "-y",
        "-i", str(source),
        "-vf", scale_filter(max_dimension),
        "-frames:v", "1", "-q:v", str(quality), "-update", "1",
        str(temporary),
    ]
    try:
        result = windows_preview(source, temporary, max_dimension) if executable is None else subprocess.run(
            command,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=120,
            creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0,
        )
        if result.returncode != 0:
            raise RuntimeError(f"FFmpeg failed for {source}: {result.stderr[-1000:].strip()}")
        if not temporary.is_file() or temporary.stat().st_size == 0:
            raise RuntimeError(f"FFmpeg produced no preview for {source}")
        os.replace(temporary, target)
    finally:
        temporary.unlink(missing_ok=True)
    return target


def main():
    parser = argparse.ArgumentParser(
        description="Create lightweight JPEG previews beside the original images without modifying them."
    )
    parser.add_argument("images", nargs="+", type=Path)
    parser.add_argument("--max-dimension", type=int, default=1920)
    parser.add_argument("--quality", type=int, default=2, help="FFmpeg JPEG quality: 1 is best, 31 is worst")
    args = parser.parse_args()
    if args.max_dimension < 2:
        parser.error("--max-dimension must be at least 2")
    if not 1 <= args.quality <= 31:
        parser.error("--quality must be between 1 and 31")
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8")

    try:
        for source in args.images:
            target = create_preview(source, args.max_dimension, args.quality)
            original_size = source.stat().st_size
            preview_size = target.stat().st_size
            print(f"{target} | {preview_size} bytes | {original_size / preview_size:.1f}x smaller")
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        parser.exit(1, f"error: {error}\n")


if __name__ == "__main__":
    main()
