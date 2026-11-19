"""Evidence guards, independent of the expensive real SIM acceptance run."""
import csv
from contextlib import nullcontext
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest import mock
import gait_report

spec = importlib.util.spec_from_file_location("motion_lab", Path(__file__).with_name("motion-lab.py"))
lab = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lab)


class MotionLabTests(unittest.TestCase):
    def run_fixture(self, snapshots, extra_summary="", analysis_error=False, replace_binary=False):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            binary = root / "fake.exe"
            binary.write_bytes(b"fixture")
            # These files must never rescue an incomplete subsequent run.
            for tick in (15, 70, 85, 100, 150, 170, 225, 295):
                (root / f"gait-commander-{tick}.bin").write_bytes(b"old")
            def fake_run(command, log, timeout, env):
                self.assertEqual(command[-2:], ["--kind", "commander"])
                prefix = Path(command[command.index("--out") + 1])
                self.assertEqual(prefix.parent.parent, root / "runs")
                with prefix.with_suffix(".csv").open("w", newline="") as out:
                    fields = sorted(gait_report.REQUIRED)
                    writer = csv.DictWriter(out, fieldnames=fields)
                    writer.writeheader()
                    for tick in range(300):
                        row = dict.fromkeys(fields, 0)
                        row.update(kind="commander", tick=tick, planted=1)
                        writer.writerow(row)
                for tick in (15, 70, 85, 100, 150, 170, 225, 295)[:snapshots]:
                    Path(f"{prefix}-commander-{tick}.bin").write_bytes(b"new")
                if replace_binary:
                    binary.write_bytes(b"replacement")
                return 0, .01, "gait_meta tick_rate=20 world_mm=100 ticks=300 restore_at=45\ngait kind=commander ticks=300\n" + extra_summary
            original_analysis = lab.gait_report.main
            with mock.patch.object(sys, "argv", ["motion-lab.py", "--kind", "commander", "--out", str(root), "--binary", str(binary)]), \
                    mock.patch.object(lab.fast, "background_environment", return_value={}), \
                    mock.patch.object(lab.fast, "validation_slot", side_effect=nullcontext), \
                    mock.patch.object(lab.gait_report, "main", side_effect=ValueError("analysis failed") if analysis_error else original_analysis), \
                    mock.patch.object(lab.fast, "run", side_effect=fake_run):
                code = lab.main()
            return code, json.loads((root / "manifest.json").read_text())

    def test_single_unit_requires_its_300_ticks_and_eight_fresh_snapshots(self):
        code, report = self.run_fixture(8)
        self.assertEqual(code, 0)
        self.assertEqual(report["selected_kinds"], ["commander"])
        self.assertEqual(len(report["snapshots"]), 8)

    def test_old_output_cannot_hide_missing_snapshots(self):
        code, report = self.run_fixture(7)
        self.assertEqual(code, 1)
        self.assertFalse(report["success"])
        self.assertIn("Missing staged snapshots", report["error"])

    def test_duplicate_summaries_are_rejected(self):
        code, report = self.run_fixture(8, extra_summary="gait kind=commander ticks=300\n")
        self.assertEqual(code, 1)
        self.assertFalse(report["success"])
        self.assertIn("Duplicate", report["error"])

    def test_analysis_failure_cannot_leave_success_manifest(self):
        code, report = self.run_fixture(8, analysis_error=True)
        self.assertEqual(code, 1)
        self.assertFalse(report["success"])
        self.assertEqual(report["error"], "analysis failed")

    def test_changed_executable_cannot_be_attributed_to_old_hash(self):
        code, report = self.run_fixture(8, replace_binary=True)
        self.assertEqual(code, 1)
        self.assertFalse(report["success"])
        self.assertIn("Executable changed", report["error"])

    def test_unknown_kind_is_rejected_before_simulation(self):
        with mock.patch.object(sys, "argv", ["motion-lab.py", "--kind", "typo"]), \
                mock.patch.object(lab.fast, "run") as run:
            with self.assertRaises(SystemExit):
                lab.main()
            run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
