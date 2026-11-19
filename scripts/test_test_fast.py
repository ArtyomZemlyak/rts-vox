"""Runner guards: a green exit code alone is not evidence of a test."""
import importlib.util
import json
from pathlib import Path
import unittest
import tempfile
import subprocess
import sys
from unittest import mock
from argparse import Namespace

spec = importlib.util.spec_from_file_location("fast", Path(__file__).with_name("test-fast.py"))
fast = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fast)


class SelectionTests(unittest.TestCase):
    def test_changed_binary_fails_before_creating_any_pass_proof(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            binary = root / "fixture.exe"
            binary.write_bytes(b"original")
            args = Namespace(out=root / "result", package="fx", target="lib", filters=["a"],
                             suite=None, no_build=False, reuse_passed=False, list=False,
                             build_timeout=5, timeout=5)
            def fake_run(command, log, timeout, env):
                if "--no-run" in command:
                    return 0, .01, json.dumps(dict(reason="compiler-artifact", executable=str(binary), profile=dict(test=True)))
                if "--list" in command:
                    return 0, .01, "a: test\n"
                binary.write_bytes(b"replacement")
                return 0, .01, "test a ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;"
            with mock.patch.object(fast, "ROOT", root), mock.patch.object(fast, "source_hash", return_value="stable"), \
                    mock.patch.object(fast, "background_environment", return_value={}), \
                    mock.patch.object(fast, "run", side_effect=fake_run):
                self.assertEqual(fast.execute(args), 1)
            self.assertFalse((root / "target/fast-test-index/fx-lib-passed.json").exists())
            report = json.loads((args.out / "test-index.json").read_text())
            self.assertIn("changed during test execution", report["error"])

    def test_failed_test_log_preserves_unicode_without_masking_failure(self):
        import io
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            binary = root / "fixture.exe"
            binary.write_bytes(b"original")
            args = Namespace(out=root / "result", package="fx", target="lib", filters=["a"],
                             suite=None, no_build=False, reuse_passed=False, list=False,
                             build_timeout=5, timeout=5)
            def fake_run(command, log, timeout, env):
                if "--no-run" in command:
                    return 0, .01, json.dumps(dict(reason="compiler-artifact", executable=str(binary), profile=dict(test=True)))
                if "--list" in command:
                    return 0, .01, "a: test\n"
                return 101, .01, "test a ... FAILED\nНужен хотя бы один свой и один вражеский юнит\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"
            with mock.patch.object(fast, "ROOT", root), mock.patch.object(fast, "source_hash", return_value="stable"), \
                    mock.patch.object(fast, "background_environment", return_value={}), \
                    mock.patch.object(fast, "run", side_effect=fake_run), \
                    mock.patch.object(sys, "stdout", new=io.TextIOWrapper(io.BytesIO(), encoding="utf-8")) as out:
                self.assertEqual(fast.execute(args), 1)
                out.flush()
                self.assertIn("Нужен хотя бы один свой и один вражеский юнит", out.buffer.getvalue().decode("utf-8"))
            report = json.loads((args.out / "test-index.json").read_text())
            self.assertFalse(report["success"])
            self.assertEqual(report["results"][0]["failed"], 1)
            self.assertIn("Tests failed", report["error"])

    def test_suite_merge_builds_each_target_once_and_keeps_all_filters(self):
        entries = fast.suite_entries(["movement-quick", "smoke", "movement-quick"])
        self.assertEqual(len(entries), 4)
        self.assertEqual(entries[0][0:2], ("sim-core", "lib"))
        self.assertEqual(entries[0][2].count("gait::tests::"), 1)
        self.assertIn("walkers_keep_moving_across_small_reclaim_and_a_new_crater", entries[0][2])

    def test_same_counts_cannot_hide_wrong_duplicate_or_inconsistent_tests(self):
        good = "test a ... ok\ntest b ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored;"
        self.assertEqual(fast.completed_tests(good, ["a", "b"])["passed"], 2)
        for bad in [good.replace("test b", "test c"), good.replace("test b", "test a"),
                    good.replace("test a ... ok", "test a ... FAILED")]:
            with self.assertRaises(ValueError):
                fast.completed_tests(bad, ["a", "b"])

    def test_validation_slot_excludes_another_process_and_releases_on_error(self):
        code = """import importlib.util,sys,pathlib
s=importlib.util.spec_from_file_location('fast',sys.argv[2]);f=importlib.util.module_from_spec(s);s.loader.exec_module(f)
f.ROOT=pathlib.Path(sys.argv[1])
try:
 with f.validation_slot(): pass
except RuntimeError: sys.exit(6)
"""
        with tempfile.TemporaryDirectory() as folder, mock.patch.object(fast, "ROOT", Path(folder)):
            def child():
                return subprocess.run([sys.executable, "-c", code, folder, str(Path(fast.__file__).resolve())],
                                      capture_output=True, timeout=10, creationflags=subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0).returncode
            with self.assertRaisesRegex(ValueError, "intentional"):
                with fast.validation_slot():
                    self.assertEqual(child(), 6)
                    raise ValueError("intentional")
            self.assertEqual(child(), 0)

    def test_runtime_file_and_preview_tests_never_reuse_pure_sim_passes(self):
        self.assertTrue(fast.pure_test_selection("sim-core", "lib", ["gait::tests::a"]))
        self.assertFalse(fast.pure_test_selection("sim-core", "lib", ["gait::tests::a", "blast::felling_front_tests::reads_snapshot"]))
        self.assertFalse(fast.pure_test_selection("harness", "test:docs", ["docs"]))
        self.assertFalse(fast.pure_test_selection("modding", "lib", ["loads_runtime_file"]))

    def test_literal_compile_inputs_invalidate_cache_but_unrelated_docs_do_not(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            for name in ("Cargo.toml", "Cargo.lock", "scripts/cargo-local.ps1", "crates/x/src/lib.rs", "docs/canon.md", "crates/x/src/a.wgsl"):
                p = root / name
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_text("a", encoding="utf-8")
            (root / "crates/x/src/lib.rs").write_text('include_str!(\n"../../../docs/canon.md"\n); include_bytes!("a.wgsl");', encoding="utf-8")
            with mock.patch.object(fast, "ROOT", root):
                before = fast.source_hash()
                (root / "docs/notes.md").write_text("notes")
                self.assertEqual(before, fast.source_hash())
                (root / "docs/canon.md").write_text("b")
                changed = fast.source_hash()
                self.assertNotEqual(before, changed)
                (root / "crates/x/src/a.wgsl").write_text("b")
                self.assertNotEqual(changed, fast.source_hash())

    def test_reuse_requires_exact_binary_environment_runner_and_execution_log(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "proof.json"
            log = path.with_suffix(".log")
            log.write_text("test a ... ok\ntest b ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored;")
            proof = dict(binary_sha256="bin", environment={}, runner_sha256=fast.digest(fast.__file__),
                         selected=["a", "b"], log_sha256=fast.digest(log))
            path.write_text(json.dumps(proof))
            self.assertIsNotNone(fast.reusable_proof(path, "bin", {}, ["a"]))
            for binary, env, names in [("new", {}, ["a"]), ("bin", {"RUSTFLAGS": "new"}, ["a"]), ("bin", {}, ["c"])]:
                self.assertIsNone(fast.reusable_proof(path, binary, env, names))
            proof["runner_sha256"] = "old"
            path.write_text(json.dumps(proof))
            self.assertIsNone(fast.reusable_proof(path, "bin", {}, ["a"]))
            proof["runner_sha256"] = fast.digest(fast.__file__)
            path.write_text(json.dumps(proof))
            log.write_text("test result: ok. 1 passed; 0 failed; 1 ignored;")
            self.assertIsNone(fast.reusable_proof(path, "bin", {}, ["a"]))
            proof["log_sha256"] = fast.digest(log)
            path.write_text(json.dumps(proof))
            self.assertIsNone(fast.reusable_proof(path, "bin", {}, ["a"]))
    def test_real_inventory_excludes_benchmarks_and_summary(self):
        self.assertEqual(fast.inventory("a::b: test\nc: benchmark\n1 test, 1 benchmark"), ["a::b"])

    def test_every_filter_must_match_even_if_another_matches(self):
        with self.assertRaisesRegex(ValueError, "zero tests"):
            fast.select_tests(["gait::a"], ["gait", "typo"])
        self.assertEqual(fast.select_tests(["gait::a", "gait::b"], ["gait", "::a"]), ["gait::a", "gait::b"])

    def test_skip_is_explicit_and_rejects_unknown_or_empty_selection(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            binary = root / "fixture.exe"
            binary.write_bytes(b"original")
            args = Namespace(out=root / "result", package="fx", target="lib", filters=["a"],
                             skip=["heavy", "heavy"], suite=None, no_build=False, reuse_passed=False,
                             list=False, build_timeout=5, timeout=5)
            commands = []

            def fake_run(command, log, timeout, env):
                commands.append(command)
                if "--no-run" in command:
                    return 0, .01, json.dumps(dict(reason="compiler-artifact", executable=str(binary), profile=dict(test=True)))
                if "--list" in command:
                    return 0, .01, "a::heavy: test\na::small: test\n"
                if "a::heavy" in command:
                    return 0, .01, "test a::heavy ... ok\ntest a::small ... ok\ntest result: ok. 2 passed; 0 failed; 0 ignored;"
                return 0, .01, "test a::small ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;"

            with mock.patch.object(fast, "ROOT", root), mock.patch.object(fast, "source_hash", return_value="stable"), \
                    mock.patch.object(fast, "background_environment", return_value={}), \
                    mock.patch.object(fast, "run", side_effect=fake_run):
                self.assertEqual(fast.execute(args), 0)
                report = json.loads((args.out / "test-index.json").read_text())
                result = report["results"][0]
                self.assertEqual(report["skip_filters"], ["heavy"])
                self.assertEqual(result["selected"], ["a::small"])
                self.assertEqual(result["skipped"], ["a::heavy"])
                self.assertEqual(result["selected_count"], 1)
                self.assertNotIn("a::heavy", commands[-1])
                args.skip = ["absent"]
                args.out = root / "unknown"
                self.assertEqual(fast.execute(args), 1)
                report = json.loads((args.out / "test-index.json").read_text())
                self.assertFalse(report["success"])
                self.assertIn("Skip filters matched zero", report["error"])
                self.assertEqual(report["skip_filters"], ["absent"])
                args.skip = ["a::"]
                args.out = root / "all-skipped"
                before = len(commands)
                self.assertEqual(fast.execute(args), 1)
                report = json.loads((args.out / "test-index.json").read_text())
                self.assertFalse(report["success"])
                self.assertIn("No tests remain", report["error"])
                self.assertEqual(report["results"][0]["skipped"], ["a::heavy", "a::small"])
                self.assertEqual(report["results"][0]["selected"], [])
                self.assertEqual(len(commands) - before, 2)  # Build and inventory only.
                for empty in [[""], ["   "]]:
                    args.skip = empty
                    args.out = root / f"empty-{len(empty[0])}"
                    before = len(commands)
                    self.assertEqual(fast.execute(args), 1)
                    report = json.loads((args.out / "test-index.json").read_text())
                    self.assertFalse(report["success"])
                    self.assertIn("Empty --skip", report["error"])
                    self.assertEqual(len(commands), before)  # Rejected before build or inventory.
                with mock.patch.object(fast.sys, "argv", ["test-fast.py", "--suite", "fx", "--skip", ""]), \
                        mock.patch.object(fast.sys, "stderr", __import__("io").StringIO()):
                    with self.assertRaises(SystemExit) as exit_ctx:
                        fast.main()
                    self.assertEqual(exit_ctx.exception.code, 2)

    def test_should_panic_suffix_matches_inventory_without_hiding_failure(self):
        passing = "test math::overflow - should panic ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;"
        self.assertEqual(fast.completed_tests(passing, ["math::overflow"])["passed"], 1)
        failing = "test math::overflow - should panic ... FAILED\ntest result: FAILED. 0 passed; 1 failed; 0 ignored;"
        self.assertEqual(fast.completed_tests(failing, ["math::overflow"])["failed"], 1)
        with self.assertRaises(ValueError):
            fast.completed_tests(passing.replace(" - should panic", " - nearly should panic"), ["math::overflow"])

    def test_zero_or_ignored_only_is_not_a_pass(self):
        for text, expected in [("test result: ok. 0 passed; 0 failed; 0 ignored;", 1),
                               ("test result: ok. 0 passed; 0 failed; 1 ignored;", 1)]:
            with self.assertRaises(ValueError):
                fast.completed_counts(text, expected)

    def test_failures_and_partial_execution_are_visible(self):
        text = "test result: FAILED. 2 passed; 1 failed; 0 ignored;"
        self.assertEqual(fast.completed_counts(text, 3)["failed"], 1)
        with self.assertRaises(ValueError):
            fast.completed_counts(text, 4)

    def test_stale_sources_environment_and_changed_binary_are_rejected(self):
        with tempfile.TemporaryDirectory() as folder:
            binary = Path(folder) / "test.exe"
            binary.write_bytes(b"binary")
            stamp = dict(binary=str(binary), binary_sha256=fast.digest(binary), source_sha256="old", build_environment={})
            fast.verify_cached(stamp, "old", {})
            for fingerprint, env in [("new", {}), ("old", {"RUSTFLAGS": "changed"})]:
                with self.assertRaises(ValueError):
                    fast.verify_cached(stamp, fingerprint, env)
            binary.write_bytes(b"changed")
            with self.assertRaises(ValueError):
                fast.verify_cached(stamp, "old", {})

    def test_windows_shell_uses_available_powershell(self):
        with mock.patch.object(fast.shutil, "which", side_effect=["C:/pwsh", "C:/powershell"]):
            self.assertEqual(fast.windows_shell(), "C:/pwsh")
        with mock.patch.object(fast.shutil, "which", side_effect=[None, "C:/powershell"]):
            self.assertEqual(fast.windows_shell(), "C:/powershell")
        with mock.patch.object(fast.shutil, "which", return_value=None):
            with self.assertRaisesRegex(RuntimeError, "PowerShell"):
                fast.windows_shell()

    def test_cargo_local_uses_scoped_execution_policy(self):
        with mock.patch.object(fast, "windows_shell", return_value="powershell"):
            command = fast.cargo_local_command()
        self.assertEqual(command[:4], ["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass"])
        self.assertEqual(command[4], "-File")
        self.assertTrue(command[5].endswith("cargo-local.ps1"))


class TimeoutCleanupTests(unittest.TestCase):
    def check_windows_timeout(self, tree_failure, child_still_running=False):
        with tempfile.TemporaryDirectory() as folder:
            log = Path(folder) / "timeout.log"
            child = mock.Mock(pid=12345)
            child.poll.return_value = None
            child.wait.side_effect = [subprocess.TimeoutExpired(["owned"], 1),
                                     subprocess.TimeoutExpired(["owned"], 10) if child_still_running else 0]
            with mock.patch.object(fast.os, "name", "nt"), \
                    mock.patch.object(fast.subprocess, "CREATE_NO_WINDOW", 0, create=True), \
                    mock.patch.object(fast.subprocess, "Popen", return_value=child), \
                    mock.patch.object(fast.subprocess, "run", side_effect=tree_failure) as stop:
                message = "child cleanup did not finish" if child_still_running else "Timed out after 1s"
                with self.assertRaisesRegex(RuntimeError, message):
                    fast.run(["owned"], log, 1, {})
            self.assertEqual(stop.call_args.args[0], ["taskkill", "/PID", "12345", "/T", "/F"])
            self.assertEqual(stop.call_args.kwargs["timeout"], 10)
            child.kill.assert_called_once_with()
            self.assertEqual(child.wait.call_args_list, [mock.call(timeout=1), mock.call(timeout=10)])

    def test_failed_tree_cleanup_uses_owned_handle_and_bounded_join(self):
        self.check_windows_timeout(lambda *a, **k: subprocess.CompletedProcess(a[0], 1))

    def test_hung_tree_cleanup_still_terminates_the_owned_child(self):
        self.check_windows_timeout(lambda *a, **k: (_ for _ in ()).throw(subprocess.TimeoutExpired(a[0], 10)))

    def test_failed_direct_join_reports_incomplete_cleanup(self):
        self.check_windows_timeout(lambda *a, **k: subprocess.CompletedProcess(a[0], 1), True)


if __name__ == "__main__":
    unittest.main()
