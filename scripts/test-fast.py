"""Bounded, GPU-free checks against the real engine. Python standard library only.

Build each libtest once; select from its actual inventory; never accept 0 tests.
The sim-test profile optimizes execution without release's whole-program link.
"""
import argparse
import ctypes
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]


def windows_shell():
    for name in ("pwsh", "powershell"):
        executable = shutil.which(name)
        if executable:
            return executable
    raise RuntimeError("PowerShell is required for scripts/cargo-local.ps1 on Windows")


def cargo_local_command():
    return [windows_shell(), "-NoProfile", "-ExecutionPolicy", "Bypass", "-File",
            str(ROOT / "scripts/cargo-local.ps1")]


SUITES = {
    "ui-animation": [("render", "bin:blast", ["ui_motion::", "optical_ui::tests::",
                       "construction::production_hud::tests::"])],
    "walking-blasts": [("sim-core", "lib", ["walking_blast_tests::"]),
                       ("nav", "lib", ["support_route_change_tests::"])],
    "supports": [("sim-core", "lib", ["gait::tests::support_", "live_support_tests::",
                    "support_catalogue_tracks_real", "support_identity_events_do_not", "support_range_snapshot",
                    "rubble_floor_proofs_follow_edits_geometry_and_source_compaction",
                    "rubble_row_cache_invalidates_cover_geometry_and_removed_maximum",
                    "rubble_span_rows_match_cell_enumeration_with_covers_and_reused_storage"]),
                 ("nav", "lib", ["vehicle::support::tests::"]),
                 ("modding", "lib", ["walking_terrain::tests::"])],
    "movement-quick": [("sim-core", "lib", ["gait::tests::",
                    "commander_reverses_through_forest_pebbles_and_a_gentle_hill",
                    "walkers_cross_small_stairs_both_ways_and_still_start_immediately",
                    "walkers_keep_moving_across_small_reclaim_and_a_new_crater"]),
                 ("nav", "lib", ["walking_detail_tests::"])],
    "smoke": [("sim-core", "lib", ["gait::tests::"]),
              ("harness", "test:architecture", [""]),
              ("harness", "test:docs", [""])],
    "movement": [("sim-core", "lib", ["height_tests::", "tracked_motion_tests::",
                    "traffic_index::tests::", "gait::tests::", "biped_support_tests::",
                    "body_inertia_tests::", "leg_clearance_tests::",
                    "prepared_ground_and_deferred_feet_match_serial_motion_across_workers"]),
                 ("nav", "lib", ["walking_detail_tests::"])],
    "ai": [("sim-core", "lib", ["enemy_ai_move_orders_keep_firing_for_every_chassis",
             "economic_ai::tests::", "ai_frame::tests::"])],
    "craters": [("sim-core", "lib", ["multi_crater_tests::", "nuclear_crater_tests::",
             "crater_route_reacts_before_contact", "crater_route_ignores_remote_edits",
             "crater_route_detours_a_new_deep_pit",
             "different_work_budgets_produce_the_same_crater",
             "a_capture_cut_into_slices_leaves_the_same_crater",
             "a_tank_falls_to_the_floor_of_a_real_explosion_crater",
             "a_shell_crater_in_flat_soil_is_round",
             "small_fragment_pool_pulverises_or_defers_without_changing_the_crater",
             "wave_phase_capacity_preserves_mixed_ground_claims_and_partial_confirmation",
             "canister_salvo_destruction_lands_within_five_ticks",
             "canister_salvo_digs_less_soil_and_still_finishes",
             "lab_physics_actual_buried_detonation_keeps_replay_and_energy_bounds"])],
    "docs": [("harness", "test:docs", [""])],
    "architecture": [("harness", "test:architecture", [""])],
    "scenes": [("demo", "test:scenes", ["match"])],
}


def digest(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def source_hash():
    """Include compile inputs, including include_str RON and untracked Rust files."""
    paths = [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "scripts/cargo-local.ps1"]
    paths += [ROOT / name for name in ("rust-toolchain", "rust-toolchain.toml") if (ROOT / name).exists()]
    for folder, extensions in [("crates", {".rs", ".toml"}),
                               ("data", {".ron"}), (".cargo", {".toml"})]:
        base = ROOT / folder
        for directory, dirs, files in os.walk(base):
            dirs[:] = sorted(d for d in dirs if d not in {"target", ".git"})
            paths.extend(Path(directory) / f for f in files if Path(f).suffix in extensions)
    # Compile-time inputs may be outside Rust/RON (e.g. the economy canon or
    # WGSL). Include literal include_str/include_bytes inputs as well. Cargo
    # remains authoritative for dynamic macros and external toolchain inputs.
    for path in list(paths):
        if path.suffix == ".rs":
            for relative in re.findall(r'include_(?:str|bytes)!\s*\(\s*"([^"\\]+)"\s*\)',
                                       path.read_text(encoding="utf-8")):
                included = (path.parent / relative).resolve()
                if included.is_relative_to(ROOT) and included.is_file():
                    paths.append(included)
    h = hashlib.sha256()
    for path in sorted(set(paths)):
        h.update(path.relative_to(ROOT).as_posix().encode())
        h.update(bytes.fromhex(digest(path)))
    return h.hexdigest()


def build_environment(env):
    """Compiler/profile overrides are inputs too, including an alternate toolchain."""
    names = {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "RUSTUP_TOOLCHAIN", "RUSTC", "RUSTC_WRAPPER",
             "RUSTC_WORKSPACE_WRAPPER", "CARGO_BUILD_TARGET", "CARGO_TARGET_DIR", "CARGO_INCREMENTAL"}
    return {k: v for k, v in sorted(env.items()) if k in names or k.startswith("CARGO_PROFILE_")}


def verify_cached(saved, fingerprint, environment):
    if (saved["source_sha256"] != fingerprint or saved.get("build_environment") != environment
            or digest(saved["binary"]) != saved["binary_sha256"]):
        raise ValueError("Cached test binary is stale or changed; rerun without --no-build")


def inventory(text):
    return [line[:-6] for line in text.splitlines() if line.endswith(": test")]


def select_tests(names, filters):
    selected = set()
    for pattern in filters:
        found = [name for name in names if pattern in name]
        if not found:
            raise ValueError(f"Filter matched zero tests: {pattern!r}")
        selected.update(found)
    if not selected:
        raise ValueError("No tests selected")
    return sorted(selected)


def completed_counts(text, expected):
    matches = re.findall(r"test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", text)
    if len(matches) != 1:
        raise ValueError("Missing or ambiguous libtest completion summary")
    passed, failed, ignored = map(int, matches[0])
    if passed + failed + ignored != expected or passed + failed == 0:
        raise ValueError(f"Expected {expected} selected tests, got {passed}/{failed}/{ignored}")
    return dict(passed=passed, failed=failed, ignored=ignored)


def completed_tests(text, names):
    counts = completed_counts(text, len(names))
    rows = re.findall(r"^test (\S+)(?: - should panic)? \.\.\. (ok|FAILED|ignored)\b", text, re.MULTILINE)
    if len(rows) != len(names) or {n for n, _ in rows} != set(names):
        raise ValueError("Execution log does not contain the exact selected test names")
    if [sum(v == state for _, v in rows) for state in ("ok", "FAILED", "ignored")] != [counts[k] for k in ("passed", "failed", "ignored")]:
        raise ValueError("Per-test results disagree with completion summary")
    return counts


def suite_entries(suites):
    merged = {}
    for suite in suites or ["smoke"]:
        for package, target, filters in SUITES[suite]:
            dest = merged.setdefault((package, target), [])
            dest.extend(f for f in filters if f not in dest)
    return [(p, t, filters) for (p, t), filters in merged.items()]


@contextmanager
def validation_slot():
    """One heavyweight validator per workspace, released by the OS on exit."""
    path = ROOT / "target/validation.lock"
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a+b") as handle:
        if path.stat().st_size == 0:
            handle.write(b"0")
            handle.flush()
        handle.seek(0)
        try:
            if os.name == "nt":
                import msvcrt
                msvcrt.locking(handle.fileno(), msvcrt.LK_NBLCK, 1)
            else:
                import fcntl
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        except OSError as error:
            raise RuntimeError("Another validation is running in this workspace; retry after it finishes") from error
        try:
            yield
        finally:
            handle.seek(0)
            if os.name == "nt":
                msvcrt.locking(handle.fileno(), msvcrt.LK_UNLCK, 1)
            else:
                fcntl.flock(handle.fileno(), fcntl.LOCK_UN)


def reusable_proof(path, binary_sha, environment, names):
    """Only pure libtests opt in. Preserve actual execution evidence separately."""
    try:
        proof = json.loads(path.read_text(encoding="utf-8"))
        if (proof["binary_sha256"] != binary_sha or proof["environment"] != environment
                or proof["runner_sha256"] != digest(__file__)
                or not set(names).issubset(proof["selected"])):
            return None
        log = path.with_suffix(".log")
        if digest(log) != proof["log_sha256"]:
            return None
        counts = completed_tests(log.read_text(encoding="utf-8"), proof["selected"])
        if counts["failed"] or counts["ignored"] or counts["passed"] != len(proof["selected"]):
            return None
        return proof
    except (OSError, ValueError, KeyError, TypeError):
        return None


def pure_test_selection(package, target, names):
    if target != "lib":
        return False
    if package in {"nav", "fx"}:
        return True
    # Other SIM tests can read runtime files/environment or produce preview
    # snapshots. Reuse only audited mechanics modules, never an entire crate.
    prefixes = ("gait::tests::", "blast::motion::height_tests::",
                "blast::motion::biped_support_tests::", "blast::motion::body_inertia_tests::",
                "blast::motion::leg_clearance_tests::", "blast::motion::tracked_motion_tests::",
                "blast::motion::tests::prepared_ground_and_deferred_feet_match_serial_motion_across_workers")
    return package == "sim-core" and bool(names) and all(n.startswith(prefixes) for n in names)


def background_environment():
    env = os.environ.copy()
    env.update(CARGO_BUILD_JOBS="2", RAYON_NUM_THREADS="1", RUST_TEST_THREADS="1")
    if os.name == "nt":
        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        kernel.GetCurrentProcess.restype = ctypes.c_void_p
        kernel.SetPriorityClass.argtypes = [ctypes.c_void_p, ctypes.c_uint32]
        kernel.SetProcessAffinityMask.argtypes = [ctypes.c_void_p, ctypes.c_size_t]
        handle = kernel.GetCurrentProcess()
        if not kernel.SetPriorityClass(handle, 0x4000) or not kernel.SetProcessAffinityMask(handle, 5):
            raise RuntimeError("Could not apply bounded background CPU settings")
    return env


def run(command, log, timeout, env):
    start = time.perf_counter()
    flags = subprocess.CREATE_NO_WINDOW if os.name == "nt" else 0
    with log.open("w", encoding="utf-8") as out:
        p = subprocess.Popen(command, cwd=ROOT, env=env, stdout=out,
                             stderr=subprocess.STDOUT, creationflags=flags)
        try:
            code = p.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            # Only this runner's child and descendants, never another Cargo/game.
            cleanup_note = ""
            if os.name == "nt":
                try:
                    stopped = subprocess.run(
                        ["taskkill", "/PID", str(p.pid), "/T", "/F"],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                        creationflags=flags, timeout=10)
                    if stopped.returncode:
                        cleanup_note = f"; tree cleanup rc={stopped.returncode}, descendant cleanup unverified"
                except (subprocess.TimeoutExpired, OSError) as error:
                    cleanup_note = f"; tree cleanup failed ({type(error).__name__}), descendant cleanup unverified"
            # The Popen handle belongs to this runner: direct termination does
            # not depend on WMI/process enumeration, unlike taskkill /T.
            try:
                if p.poll() is None:
                    p.kill()
                p.wait(timeout=10)
            except (subprocess.TimeoutExpired, OSError) as error:
                raise RuntimeError(
                    f"Timed out after {timeout}s: {log}; child cleanup did not finish"
                    f" ({type(error).__name__}){cleanup_note}") from error
            raise RuntimeError(f"Timed out after {timeout}s: {log}{cleanup_note}")
    elapsed = time.perf_counter() - start
    output = log.read_text(encoding="utf-8", errors="replace")
    return code, elapsed, output


def main():
    # Native test failures can contain Russian diagnostics even when Windows
    # launches Python with a legacy output encoding. Preserve the real failure.
    for stream in (sys.stdout, sys.stderr):
        if hasattr(stream, "reconfigure"):
            stream.reconfigure(errors="backslashreplace")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--suite", choices=SUITES, action="append", help="Repeat to merge suites without duplicate tests")
    parser.add_argument("--package", help="Custom package; requires --filter")
    parser.add_argument("--target", default="lib", help="lib, test:NAME or bin:NAME")
    parser.add_argument("--filter", action="append", dest="filters")
    parser.add_argument("--skip", action="append", default=[], metavar="NAME",
                        help="Skip matching tests after selection; report names and reject unmatched filters")
    parser.add_argument("--no-build", action="store_true", help="Require verified current cached binary")
    parser.add_argument("--reuse-passed", action="store_true",
                        help="After Cargo validation, reuse recorded passes of identical pure libtests")
    parser.add_argument("--list", action="store_true", help="Resolve exact inventory without running tests")
    parser.add_argument("--out", type=Path, default=ROOT / "out/fast-tests/latest")
    parser.add_argument("--timeout", type=int, default=180, help="Seconds per test process")
    parser.add_argument("--build-timeout", type=int, default=600)
    args = parser.parse_args()
    if args.timeout < 1 or args.build_timeout < 1:
        parser.error("Timeouts must be positive")
    if bool(args.package) != bool(args.filters):
        parser.error("--package and --filter must be used together")
    if args.target != "lib" and not args.target.startswith(("test:", "bin:")):
        parser.error("--target must be lib, test:NAME or bin:NAME")
    if args.reuse_passed and args.no_build:
        parser.error("--reuse-passed requires Cargo validation; omit --no-build")
    if args.package and args.suite:
        parser.error("Use either --suite or --package/--filter")
    if any(not pattern or not pattern.strip() for pattern in args.skip):
        parser.error("Empty --skip pattern is not allowed")
    try:
        with validation_slot():
            return execute(args)
    except (OSError, RuntimeError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1


def execute(args):
    started = time.perf_counter()
    args.out.mkdir(parents=True, exist_ok=True)
    cache = ROOT / "target/fast-test-index"
    cache.mkdir(parents=True, exist_ok=True)
    env = background_environment()
    requested_skips = set(getattr(args, "skip", []))
    matched_skips = set()
    report = dict(schema_version=1, profile="sim-test", headless=True,
                  source_sha256=source_hash(), runner_sha256=digest(__file__),
                  build_environment=build_environment(env), skip_filters=sorted(requested_skips),
                  results=[], success=False)
    report_path = args.out / "test-index.json"
    try:
        if any(not pattern or not pattern.strip() for pattern in requested_skips):
            raise ValueError("Empty --skip pattern is not allowed")
        entries = [(args.package, args.target, args.filters)] if args.package else suite_entries(args.suite)
        for package, target, filters in entries:
            tag = f"{package}-{target.replace(':', '-')}"
            record = dict(package=package, target=target, filters=filters)
            report["results"].append(record)
            stamp = cache / f"{tag}.json"
            if args.no_build:
                saved = json.loads(stamp.read_text(encoding="utf-8"))
                binary = Path(saved["binary"])
                verify_cached(saved, report["source_sha256"], report["build_environment"])
                record["build_seconds"] = 0
            else:
                command = ["cargo", "test", "--profile", "sim-test", "--offline", "-p", package]
                command += ["--lib"] if target == "lib" else ["--" + target.split(":", 1)[0], target.split(":", 1)[1]]
                command += ["--no-run", "--message-format=json"]
                if os.name == "nt":
                    command = cargo_local_command() + command[1:]
                code, seconds, output = run(command, args.out / f"{tag}-build.log", args.build_timeout, env)
                record["build_seconds"] = seconds
                if code:
                    raise RuntimeError(f"Build failed ({code}); see {tag}-build.log")
                artifacts = []
                for line in output.splitlines():
                    try:
                        item = json.loads(line)
                    except ValueError:
                        continue
                    if item.get("reason") == "compiler-artifact" and item.get("executable") and item["profile"].get("test"):
                        artifacts.append(item["executable"])
                if len(set(artifacts)) != 1:
                    raise ValueError(f"Expected one test executable, found {artifacts}")
                binary = Path(artifacts[0])
                if source_hash() != report["source_sha256"]:
                    raise ValueError("Sources changed while building; result cannot be stamped")
                saved = dict(binary=str(binary), binary_sha256=digest(binary),
                             source_sha256=report["source_sha256"], profile="sim-test",
                             build_environment=report["build_environment"])
                stamp.write_text(json.dumps(saved, indent=2), encoding="utf-8")
            record.update(saved)
            code, seconds, output = run([str(binary), "--list", "--format", "terse"],
                                        args.out / f"{tag}-inventory.log", 30, env)
            if code:
                raise RuntimeError("Could not enumerate tests")
            record["inventory_seconds"] = seconds
            names = select_tests(inventory(output), filters)
            skipped = [name for name in names
                       if any(pattern in name for pattern in requested_skips)]
            names = [name for name in names if name not in skipped]
            matched_skips.update(pattern for pattern in requested_skips
                                 if any(pattern in name for name in skipped))
            record["skipped"] = skipped
            record["selected"] = names
            record["selected_count"] = len(names)
            if not names:
                raise ValueError("No tests remain after --skip")
            print(f"{tag}: {len(names)} tests selected, {len(skipped)} skipped; build {record['build_seconds']:.2f}s", flush=True)
            if not args.list:
                can_reuse = pure_test_selection(package, target, names)
                proof_path = cache / f"{tag}-passed.json"
                proof = reusable_proof(proof_path, saved["binary_sha256"], report["build_environment"], names) if args.reuse_passed and can_reuse else None
                if proof:
                    evidence = args.out / f"{tag}-reused-proof.json"
                    shutil.copyfile(proof_path, evidence)
                    shutil.copyfile(proof_path.with_suffix(".log"), evidence.with_suffix(".log"))
                    record.update(reused_passed=len(names), evidence=str(proof_path),
                                  evidence_sha256=digest(evidence), test_seconds=0)
                    record["evidence"] = str(evidence.resolve())
                    print(f"  {len(names)} prior passes reused; identical executable and verified execution log", flush=True)
                    continue
                if can_reuse:
                    # A fresh failure/timeout must invalidate an earlier pass.
                    proof_path.unlink(missing_ok=True)
                code, seconds, output = run([str(binary), "--exact", *names, "--test-threads=1"],
                                            args.out / f"{tag}-tests.log", args.timeout, env)
                record.update(completed_tests(output, names))
                record.update(test_seconds=seconds, exit_code=code)
                print(f"  {record['passed']} passed / {record['failed']} failed / {record['ignored']} ignored in {seconds:.3f}s", flush=True)
                if code or record["failed"]:
                    sys.stdout.buffer.write(output[-6000:].encode("utf-8", errors="replace"))
                    sys.stdout.buffer.write(b"\n")
                    sys.stdout.buffer.flush()
                    raise RuntimeError(f"Tests failed; see {tag}-tests.log")
                if digest(binary) != saved["binary_sha256"] or digest(__file__) != report["runner_sha256"]:
                    raise ValueError("Executable or runner changed during test execution; pass cannot be cached")
                if can_reuse and not record["ignored"]:
                    proof_log = proof_path.with_suffix(".log")
                    proof_log.write_text(output, encoding="utf-8")
                    proof_path.write_text(json.dumps(dict(binary_sha256=saved["binary_sha256"],
                        environment=report["build_environment"], runner_sha256=report["runner_sha256"],
                        selected=names, log_sha256=digest(proof_log),
                        source_report=str(report_path.resolve()), test_seconds=seconds), indent=2), encoding="utf-8")
        if requested_skips != matched_skips:
            missing = sorted(requested_skips - matched_skips)
            raise ValueError(f"Skip filters matched zero selected tests: {missing}")
        if source_hash() != report["source_sha256"]:
            raise ValueError("Sources changed during validation; rerun on a stable tree")
        if digest(__file__) != report["runner_sha256"]:
            raise ValueError("Runner changed during validation")
        for record in report["results"]:
            if digest(record["binary"]) != record["binary_sha256"]:
                raise ValueError("Executable changed during validation")
        report["success"] = True
    except (OSError, ValueError, RuntimeError) as error:
        report["error"] = str(error)
        print(f"ERROR: {error}", file=sys.stderr)
    finally:
        report["total_seconds"] = time.perf_counter() - started
        report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2), encoding="utf-8")
        print(f"Report: {report_path}", flush=True)
    return 0 if report["success"] else 1


if __name__ == "__main__":
    sys.exit(main())
