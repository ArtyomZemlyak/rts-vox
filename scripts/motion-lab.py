"""One command: real SIM -> replay checks -> CSV/snapshots -> offline HTML/SVG.

No graphical process, no wall-clock tick pacing, no second physics model.
"""
import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import sys
import time
import uuid
import gait_report

spec = importlib.util.spec_from_file_location("fast", Path(__file__).with_name("test-fast.py"))
fast = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fast)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--scenario", choices=["flat", "terrain", "tracks"], default="flat")
    parser.add_argument("--out", type=Path, default=fast.ROOT / "out/motion-lab/latest")
    parser.add_argument("--binary", type=Path, help="Explicit frozen bench_gait; provenance marked archival")
    parser.add_argument("--kind", help="Only one unit from the selected scenario (e.g. commander)")
    parser.add_argument("--hard-turns", action="store_true", help="Alternate full steering every six ticks in window 200–260")
    parser.add_argument("--reason-trace", action="store_true", help="Bounded opt-in anchor reasons through tick 260, including placement")
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    if args.timeout < 1:
        parser.error("--timeout must be positive")
    kinds = ["tank", "eidolontank", "artillery", "lightningtank", "artilleryt2", "crawler"] if args.scenario == "tracks" else ["commander", "walker04", "strider", "spider"]
    if args.kind:
        args.kind = args.kind.lower()
        if args.kind not in kinds:
            parser.error("--kind must be one of: " + ", ".join(kinds))
    args.out = args.out.resolve()
    try:
        with fast.validation_slot():
            return execute(args, kinds)
    except (OSError, RuntimeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


def execute(args, kinds):
    started = time.perf_counter()
    args.out.mkdir(parents=True, exist_ok=True)
    env = fast.background_environment()
    report = dict(schema_version=1, success=False, scenario=args.scenario, headless=True,
                  tick_rate=None, world_mm=None, ticks_per_unit=300, restore_at=45,
                  navigation_workers=[1, 2], physics_workers=[1, 2],
                  scenario_source="bench_gait.rs + data/range-motion-yard.ron",
                  source_sha256=fast.source_hash(), runner_sha256=fast.digest(__file__),
                  selected_kinds=[args.kind] if args.kind else kinds)
    report.update(hard_turns=args.hard_turns, reason_trace=args.reason_trace)
    try:
        if args.binary:
            binary = args.binary.resolve()
            report["provenance"] = "explicit frozen binary; current source hash is contextual, not proof of binary inputs"
            report["build_seconds"] = 0
        else:
            command = ["cargo", "build", "--profile", "sim-test", "--offline", "-p", "sim-core",
                       "--bin", "bench_gait", "--message-format=json"]
            if os.name == "nt":
                command = fast.cargo_local_command() + command[1:]
            code, seconds, output = fast.run(command, args.out / "build.log", 600, env)
            report["build_seconds"] = seconds
            if code:
                raise RuntimeError("bench_gait build failed; see build.log")
            artifacts = []
            for line in output.splitlines():
                try:
                    item = json.loads(line)
                except ValueError:
                    continue
                if item.get("reason") == "compiler-artifact" and item.get("executable") and item["target"]["name"] == "bench_gait":
                    artifacts.append(item["executable"])
            if len(artifacts) != 1:
                raise ValueError("Expected exactly one bench_gait executable")
            binary = Path(artifacts[0])
            if fast.source_hash() != report["source_sha256"]:
                raise ValueError("Sources changed during build")
            report["provenance"] = "fresh Cargo artifact with unchanged compile-input fingerprint"
        report.update(binary=str(binary), binary_sha256=fast.digest(binary))
        # New run cannot borrow staged snapshots left by an earlier execution.
        run_dir = args.out / "runs" / uuid.uuid4().hex
        run_dir.mkdir(parents=True)
        report["run_directory"] = str(run_dir)
        prefix = run_dir / "gait"
        command = [str(binary), "--out", str(prefix)]
        if args.scenario != "flat":
            command += ["--" + args.scenario]
        if args.kind:
            command += ["--kind", args.kind]
        if args.hard_turns:
            command += ["--hard-turns"]
        if args.reason_trace:
            command += ["--reason-trace"]
        code, seconds, output = fast.run(command, args.out / "simulation.log", args.timeout, env)
        report["simulation_seconds"] = seconds
        if code:
            raise RuntimeError("Simulation/replay assertion failed; see simulation.log")
        meta = [line for line in output.splitlines() if line.startswith("gait_meta ")]
        if len(meta) == 1:
            values = dict(re.findall(r"(\w+)=(\d+)", meta[0]))
            report.update(tick_rate=int(values["tick_rate"]), world_mm=int(values["world_mm"]))
            if int(values["ticks"]) != 300 or int(values["restore_at"]) != 45:
                raise ValueError("Unexpected bench_gait scenario version")
        elif not args.binary:
            raise ValueError("Fresh executable did not report its clock/scale metadata")
        else:
            report["metadata_warning"] = "Archival executable does not export its clock/scale"
        summaries = {}
        for line in output.splitlines():
            if line.startswith("gait kind="):
                values = dict(re.findall(r"(\w+)=([^\s]+)", line))
                kind = values.pop("kind")
                if kind in summaries or values.get("ticks") != "300":
                    raise ValueError("Duplicate or incomplete unit summary")
                summaries[kind] = values
        expected_kinds = set(report["selected_kinds"])
        groups = gait_report.load_trace(prefix.with_suffix(".csv"))
        if set(summaries) != expected_kinds or set(summaries) != set(groups) or any(len(v) != 300 for v in groups.values()):
            raise ValueError("Incomplete scenario: wrong unit/tick count")
        report.update(replay_passed=True, timing=summaries,
                      trace_sha256=fast.digest(prefix.with_suffix(".csv")),
                      snapshots={p.name: fast.digest(p) for p in sorted(run_dir.glob("gait-*.bin"))})
        if args.reason_trace:
            reason_path=prefix.with_suffix(".reasons.csv")
            report["reason_trace_sha256"]=fast.digest(reason_path)
            report["reason_summary"]=[line for line in output.splitlines() if line.startswith("gait_reasons ")]
            shutil.copyfile(reason_path,args.out/"gait.reasons.csv")
        expected_snapshots = {f"gait-{kind}-{tick}.bin" for kind in expected_kinds
                              for tick in (15, 70, 85, 100, 150, 170, 225, 295)}
        if set(report["snapshots"]) != expected_snapshots:
            raise ValueError("Missing staged snapshots")
        if not args.binary and fast.source_hash() != report["source_sha256"]:
            raise ValueError("Sources changed during simulation")
        # Keep existing viewer/capture commands compatible. Validation above
        # counts only files in this run, never these convenient latest copies.
        shutil.copyfile(prefix.with_suffix(".csv"), args.out / "gait.csv")
        for name in expected_snapshots:
            shutil.copyfile(run_dir / name, args.out / name)
        (args.out / "manifest.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
        # Analyze the already recorded trace, without running SIM again.
        saved = sys.argv
        try:
            sys.argv = ["gait_report.py", "--input", str(prefix.with_suffix(".csv")), "--out", str(args.out),
                        "--run-manifest", str(args.out / "manifest.json")]
            gait_report.main()
        finally:
            sys.argv = saved
        if fast.digest(binary) != report["binary_sha256"]:
            raise ValueError("Executable changed during simulation")
        if fast.digest(__file__) != report["runner_sha256"]:
            raise ValueError("Runner changed during simulation")
        report["success"] = True
    except (OSError, ValueError, RuntimeError) as error:
        report.update(success=False, error=str(error))
        print(f"ERROR: {error}", file=sys.stderr)
    finally:
        report["total_seconds"] = time.perf_counter() - started
        if report["success"]:
            metrics_path = args.out / "metrics.json"
            metrics = json.loads(metrics_path.read_text(encoding="utf-8"))
            metrics["run"] = report
            metrics_path.write_text(json.dumps(metrics, ensure_ascii=False, indent=2), encoding="utf-8")
        (args.out / "manifest.json").write_text(json.dumps(report, indent=2), encoding="utf-8")
    return 0 if report["success"] else 1


if __name__ == "__main__":
    sys.exit(main())
