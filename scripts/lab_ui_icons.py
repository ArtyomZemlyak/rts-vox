"""Regenerate the embedded native editor icons from the pinned Lucide SVGs.

Optional art tooling: Pillow and resvg-py. No game runtime dependency.
Use --fetch to refresh sources and licenses from the pinned upstream tag.
"""
import argparse
from pathlib import Path
import urllib.request

NAMES = """mouse-pointer-2 move rotate-cw rotate-ccw box plus trash-2 copy
crosshair pipette play pause step-forward undo-2 save folder-open flask-conical
grid-3x3 layers chevron-left chevron-right x circle-help hammer tree-pine
shield settings-2 file-text camera search flame zap radiation sparkles bomb""".split()
TAG = "0.468.0"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fetch", action="store_true")
    parser.add_argument("--large", action="store_true", help="192px event-library icons; retain the 40px toolbar pack")
    args = parser.parse_args()
    from PIL import Image
    import resvg_py
    import io

    root = Path(__file__).resolve().parents[1] / "data/ui/icons"
    root.mkdir(parents=True, exist_ok=True)
    upstream = f"https://raw.githubusercontent.com/lucide-icons/lucide/{TAG}"
    names = ["bomb", "radiation", "hammer", "zap", "flame", "sparkles", "crosshair", "layers"] if args.large else NAMES
    size = 192 if args.large else 40
    output = root / "large" if args.large else root
    output.mkdir(parents=True, exist_ok=True)
    for name in names:
        path = root / f"{name}.svg"
        if args.fetch:
            path.write_bytes(urllib.request.urlopen(f"{upstream}/icons/{name}.svg", timeout=30).read())
        source = path.read_text(encoding="utf-8").replace("currentColor", "#dce9f5")
        png = resvg_py.svg_to_bytes(svg_string=source, width=size, height=size)
        (output / f"{name}.png").write_bytes(png)
        (output / f"{name}.rgba").write_bytes(Image.open(io.BytesIO(png)).convert("RGBA").tobytes())
    if args.fetch:
        (root / "LICENSE").write_bytes(urllib.request.urlopen(f"{upstream}/LICENSE", timeout=30).read())
        feather = "https://raw.githubusercontent.com/feathericons/feather/v4.29.2/LICENSE"
        (root / "LICENSE-Feather").write_bytes(urllib.request.urlopen(feather, timeout=30).read())
    print(f"Generated {len(names)} pinned Lucide icons at {size}px")


if __name__ == "__main__":
    main()
