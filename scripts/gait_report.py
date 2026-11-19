"""Read-only gait trace inspection. No game, GPU, Pillow or repeated simulation.

CSV is produced by the real bench_gait. Ankle/root proxies are NOT full meshes,
load-bearing support or a collision oracle. lift_mm uses the initial floor datum.
"""
import argparse
import csv
import hashlib
import html
import json
import math
from pathlib import Path
import time

REQUIRED = {"kind", "tick", "leg", "root_x_mm", "root_y_mm", "root_z_mm",
            "ankle_x_mm", "ankle_y_mm", "ankle_z_mm", "planted", "contact_error_mm"}


def load_trace(path):
    groups = {}
    with Path(path).open(encoding="utf-8-sig", newline="") as f:
        reader = csv.DictReader(f)
        if not REQUIRED.issubset(reader.fieldnames or []):
            raise ValueError(f"Missing columns: {sorted(REQUIRED - set(reader.fieldnames or []))}")
        for row in reader:
            kind = row["kind"]
            if not kind or len(kind) > 80:
                raise ValueError("Invalid unit name")
            values = {key: int(row[key]) for key in REQUIRED - {"kind"}}
            values["has_feet"] = int(row.get("has_feet", "1"))
            values["ready"] = int(row["ready"]) if "ready" in row else None
            if values["ready"] is not None:
                mask, samples = int(row["support_mask"]), int(row["support_samples"])
                if values["ready"] not in (0, 1) or not 0 <= samples <= 9 or not 0 <= mask < 1 << samples:
                    raise ValueError("Invalid support evidence")
                if values["ready"] and (not values["planted"] or not mask or row["support_state"] != "Ready"):
                    raise ValueError("Ready requires a planted contact and sole evidence")
            if values["has_feet"] not in (0, 1):
                raise ValueError("Invalid has_feet state")
            if values["planted"] not in (0, 1) or values["tick"] < 0 or values["leg"] < 0:
                raise ValueError("Invalid tick, leg or planted state")
            ticks = groups.setdefault(kind, {})
            frame = ticks.setdefault(values["tick"], {})
            if values["leg"] in frame:
                raise ValueError("Duplicate tick/leg in trace")
            frame[values["leg"]] = values
    if not groups:
        raise ValueError("Empty trace is not a successful experiment")
    for kind, ticks in groups.items():
        if sorted(ticks) != list(range(min(ticks), max(ticks) + 1)):
            raise ValueError(f"{kind}: missing ticks")
        legs = set(next(iter(ticks.values())))
        if len({v["has_feet"] for frame in ticks.values() for v in frame.values()}) != 1:
            raise ValueError(f"{kind}: inconsistent contact schema")
        for frame in ticks.values():
            if set(frame) != legs:
                raise ValueError(f"{kind}: missing leg samples")
            if len({tuple(v[f"root_{a}_mm"] for a in "xyz") for v in frame.values()}) != 1:
                raise ValueError(f"{kind}: inconsistent root within one tick")
    return groups


def metrics(groups):
    result = {}
    for kind, ticks in groups.items():
        if not next(iter(next(iter(ticks.values())).values()))["has_feet"]:
            result[kind] = dict(ticks=len(ticks), contact_metrics_available=False)
            continue
        previous, total_slide, max_slide, liftoffs, max_liftoff_jump, max_liftoff_error = {}, 0.0, 0.0, 0, 0.0, 0
        counts, residuals = [], []
        for tick in sorted(ticks):
            frame = ticks[tick]
            counts.append(sum(v["planted"] for v in frame.values()))
            for leg, v in frame.items():
                old = previous.get(leg)
                residuals.append(v["contact_error_mm"])
                if old and old["planted"] and v["planted"]:
                    slide = math.sqrt(sum((v[f"ankle_{a}_mm"] - old[f"ankle_{a}_mm"]) ** 2 for a in "xyz"))
                    total_slide += slide
                    max_slide = max(max_slide, slide)
                if old and old["planted"] and not v["planted"]:
                    liftoffs += 1
                    jump = math.sqrt(sum((v[f"ankle_{a}_mm"] - old[f"ankle_{a}_mm"]) ** 2 for a in "xyz"))
                    max_liftoff_jump = max(max_liftoff_jump, jump)
                    max_liftoff_error = max(max_liftoff_error, v["contact_error_mm"])
                previous[leg] = v
        result[kind] = dict(ticks=len(ticks), contact_metrics_available=True, leg_samples=len(residuals),
                            planted_min=min(counts), zero_planted_ticks=counts.count(0),
                            liftoffs=liftoffs, max_contact_error_mm=max(residuals),
                            max_liftoff_displacement_mm=round(max_liftoff_jump, 3),
                            max_liftoff_ik_error_mm=max_liftoff_error,
                            max_consecutive_planted_slide_mm=round(max_slide, 3),
                            total_consecutive_planted_slide_mm=round(total_slide, 3))
        ready = [sum(v["ready"] for v in frame.values()) for frame in ticks.values()
                 if all(v["ready"] is not None for v in frame.values())]
        result[kind]["readiness_metrics_available"] = len(ready) == len(ticks)
        if len(ready) == len(ticks):
            result[kind].update(ready_min=min(ready), zero_ready_ticks=ready.count(0))
    return result


def radius(ticks):
    extent = max((abs(v[f"ankle_{a}_mm"] - v[f"root_{a}_mm"])
                  for frame in ticks.values() for v in frame.values() if v["has_feet"] for a in "xyz"), default=0)
    return max(1000, extent) * 1.2


def svg_sheet(groups):
    width, cell_w, cell_h = 1440, 240, 240
    parts = [f'<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{64 + cell_h * len(groups)}" viewBox="0 0 {width} {64 + cell_h * len(groups)}">',
             '<rect width="100%" height="100%" fill="#0c151d"/>',
             '<g font-family="monospace" fill="#d5e8ee"><text x="16" y="23">SIM trace / root-to-ankle proxies / top + side / mm</text>',
             '<text x="16" y="46">green = formal planted; amber = swing. Lines are guides, not bones or support forces.</text>']
    for row_index, (kind, ticks) in enumerate(groups.items()):
        sequence = sorted(ticks)
        chosen = [sequence[round(i * (len(sequence) - 1) / 5)] for i in range(6)]
        scale = 50 / radius(ticks)
        for col, tick in enumerate(chosen):
            x, y = col * cell_w, 64 + row_index * cell_h
            parts.append(f'<text x="{x + 10}" y="{y + 17}">{html.escape(kind)} / {tick}</text>')
            for axis, cy in [("z", y + 75), ("y", y + 175)]:
                cx = x + 120
                parts.append(f'<path d="M {cx-90} {cy} H {cx+90} M {cx} {cy-38} V {cy+38}" stroke="#273f4b" fill="none"/>')
                for v in ticks[tick].values():
                    if not v["has_feet"]:
                        parts.append(f'<circle cx="{cx}" cy="{cy}" r="4" fill="#75bbdc"/>')
                        continue
                    px = cx + (v["ankle_x_mm"] - v["root_x_mm"]) * scale
                    py = cy - (v[f"ankle_{axis}_mm"] - v[f"root_{axis}_mm"]) * scale
                    color = "#48d7a5" if v["planted"] else "#f3bf60"
                    parts.append(f'<path d="M {cx} {cy} L {px:.2f} {py:.2f}" stroke="{color}" opacity=".6"/><circle cx="{px:.2f}" cy="{py:.2f}" r="3" fill="{color}"/>')
    return "".join(parts) + "</g></svg>"


def make_html(groups, summary):
    # JSON cannot close the script element even with an untrusted CSV kind.
    payload = json.dumps(groups).replace("<", "\\u003c")
    return '''<!doctype html><meta charset="utf-8"><title>Motion trace</title>
<style>body{background:#0c151d;color:#d5e8ee;font:16px monospace;margin:24px}canvas{background:#11212b;max-width:100%;border:1px solid #36505e}input{width:65%}select,button{padding:8px;background:#203747;color:inherit}pre{white-space:pre-wrap}label{display:block;margin:14px 0}</style>
<h1>Походка: записанная трасса SIM</h1>
<p>Схема корня и голеностопов. Линии — направляющие, не кости. Зелёный — formal planted, жёлтый — перенос.</p>
<p>Это диагностика контактов, а не проверка полной геометрии, несущих сил или визуала игры.</p>
<select id="kind"></select> <button id="play">▶</button>
<label>Тик <input id="tick" type="range" min="0" value="0"><output id="value"></output></label>
<canvas id="view" width="1200" height="500"></canvas>
<p>Слева: сверху X/Z; справа: сбоку X/Y. Одинаковый масштаб по всем тикам модели.</p>
<a href="poses.svg">Лист ключевых поз</a> · <a href="metrics.json">Числовой отчёт</a>
<pre id="stats"></pre><script>
const data=PAYLOAD, summary=SUMMARY, sel=document.getElementById('kind'), slider=document.getElementById('tick');
for(const name of Object.keys(data)){const o=document.createElement('option');o.textContent=name;o.value=name;sel.append(o)}
let frames=[],radius=1,playing=false;
function select(){frames=Object.keys(data[sel.value]).map(Number).sort((a,b)=>a-b);slider.max=frames.length-1;slider.value=0;radius=1000;
for(const frame of Object.values(data[sel.value]))for(const v of Object.values(frame))if(v.has_feet!==0)for(const a of ['x','y','z'])radius=Math.max(radius,Math.abs(v['ankle_'+a+'_mm']-v['root_'+a+'_mm']));radius*=1.2;
document.getElementById('stats').textContent=JSON.stringify(summary[sel.value],null,2);draw()}
function draw(){const tick=frames[Number(slider.value)],frame=data[sel.value][tick],c=document.getElementById('view').getContext('2d');c.clearRect(0,0,1200,500);document.getElementById('value').textContent=tick;
for(const [axis,cx] of [['z',300],['y',900]]){const cy=220,scale=190/radius;c.strokeStyle='#36505e';c.beginPath();c.moveTo(cx-250,cy);c.lineTo(cx+250,cy);c.moveTo(cx,20);c.lineTo(cx,450);c.stroke();
for(const v of Object.values(frame)){if(v.has_feet===0){c.fillStyle='#75bbdc';c.fillText('body only / no feet',cx-80,cy-12);continue}const x=cx+(v.ankle_x_mm-v.root_x_mm)*scale,y=cy-(v['ankle_'+axis+'_mm']-v['root_'+axis+'_mm'])*scale;c.strokeStyle=c.fillStyle=v.planted?'#48d7a5':'#f3bf60';c.beginPath();c.moveTo(cx,cy);c.lineTo(x,y);c.stroke();c.beginPath();c.arc(x,y,6,0,Math.PI*2);c.fill();c.fillText('leg '+v.leg,x+8,y)}}}
sel.onchange=select;slider.oninput=draw;document.getElementById('play').onclick=()=>{playing=!playing};setInterval(()=>{if(playing){slider.value=(Number(slider.value)+1)%frames.length;draw()}},50);select();
</script>'''.replace("PAYLOAD", payload).replace("SUMMARY", json.dumps(summary).replace("<", "\\u003c"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--baseline", type=Path, help="Previous CSV; compare measured values, not FPS")
    parser.add_argument("--run-manifest", type=Path, help="Provenance emitted by motion-lab.py")
    args = parser.parse_args()
    started = time.perf_counter()
    groups = load_trace(args.input)
    summary = metrics(groups)
    report = dict(schema_version=1, trace_sha256=hashlib.sha256(args.input.read_bytes()).hexdigest(),
                  units=summary, limitations=["Formal planted is not load-bearing support",
                  "CSV positions are rounded to mm; slide is quantized",
                  "No full mesh/sole penetration/angle or support source oracle in this CSV",
                  "lift_mm is relative to the initial floor, not the live terrain gap"])
    if args.run_manifest:
        report["run"] = json.loads(args.run_manifest.read_text(encoding="utf-8"))
        if report["run"].get("trace_sha256") != report["trace_sha256"]:
            raise ValueError("Run manifest does not describe this trace")
    else:
        report["provenance"] = "unverified archival CSV; no claim about current build"
    if args.baseline:
        old = metrics(load_trace(args.baseline))
        if set(old) != set(summary) or any(old[k]["ticks"] != summary[k]["ticks"] for k in old):
            raise ValueError("A/B unit sets and tick counts must match")
        if any(set(old[k]) != set(summary[k]) for k in old):
            raise ValueError("A/B contact schemas differ")
        report["baseline_sha256"] = hashlib.sha256(args.baseline.read_bytes()).hexdigest()
        report["delta"] = {k: {m: round(summary[k][m] - old[k][m], 3) for m in summary[k]} for k in old}
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "poses.svg").write_text(svg_sheet(groups), encoding="utf-8")
    (args.out / "index.html").write_text(make_html(groups, summary), encoding="utf-8")
    report["analysis_seconds"] = time.perf_counter() - started
    (args.out / "metrics.json").write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
    print(json.dumps(dict(output=str(args.out), seconds=report["analysis_seconds"], units=summary), ensure_ascii=False))


if __name__ == "__main__":
    main()
