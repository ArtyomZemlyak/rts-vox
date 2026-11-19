import csv
from pathlib import Path
import tempfile
import unittest
import gait_report as report


def sample(tick, planted, x=0):
    return dict(kind="test", tick=tick, leg=0, root_x_mm=0, root_y_mm=0, root_z_mm=0,
                ankle_x_mm=x, ankle_y_mm=0, ankle_z_mm=0, planted=planted, contact_error_mm=0)


class ReportTests(unittest.TestCase):
    def trace(self, rows):
        with tempfile.TemporaryDirectory() as folder:
            p = Path(folder) / "trace.csv"
            with p.open("w", newline="") as f:
                writer = csv.DictWriter(f, fieldnames=sorted(report.REQUIRED | {key for row in rows for key in row}))
                writer.writeheader()
                writer.writerows(rows)
            return report.load_trace(p)

    def test_empty_duplicate_and_incomplete_are_rejected(self):
        for rows in [[], [sample(0, 1), sample(0, 1)], [sample(0, 1), sample(2, 1)]]:
            with self.assertRaises(ValueError):
                self.trace(rows)

    def test_slide_does_not_include_swing_or_landing(self):
        takeoff = sample(2, 0, 100)
        takeoff["contact_error_mm"] = 7
        data = self.trace([sample(0, 1), sample(1, 1, 3), takeoff, sample(3, 1, 200)])
        result = report.metrics(data)["test"]
        self.assertEqual(result["zero_planted_ticks"], 1)
        self.assertEqual(result["liftoffs"], 1)
        self.assertEqual(result["total_consecutive_planted_slide_mm"], 3)
        self.assertEqual(result["max_liftoff_displacement_mm"], 97)
        self.assertEqual(result["max_liftoff_ik_error_mm"], 7)

    def test_html_and_svg_escape_labels(self):
        data = self.trace([sample(0, 1)])
        data["</script><bad>"] = data.pop("test")
        view = report.make_html(data, report.metrics(data))
        self.assertNotIn("</script><bad>", view)
        self.assertIn("&lt;/script&gt;", report.svg_sheet(data))

    def test_body_only_trace_does_not_invent_foot_metrics(self):
        data = self.trace([sample(0, 1)])
        data["test"][0][0]["has_feet"] = 0
        self.assertEqual(report.metrics(data)["test"], dict(ticks=1, contact_metrics_available=False))
        self.assertEqual(report.radius(data["test"]), 1200)
        self.assertIn('<circle', report.svg_sheet(data))

    def test_ready_is_distinct_from_geometric_planted(self):
        rows=[sample(0,1),sample(1,1),sample(2,0)]
        for i,row in enumerate(rows):
            row.update(ready=int(i==1),support_mask=511 if i==1 else 0,support_samples=9,
                       support_state="Ready" if i==1 else "Pending")
        result=report.metrics(self.trace(rows))["test"]
        self.assertTrue(result["readiness_metrics_available"])
        self.assertEqual(result["zero_ready_ticks"],2)
        self.assertEqual(result["zero_planted_ticks"],1)
        self.assertFalse(report.metrics(self.trace([sample(0,1)]))["test"]["readiness_metrics_available"])

    def test_ready_without_contact_or_evidence_is_rejected(self):
        for planted,mask,state in [(0,511,"Ready"),(1,0,"Ready"),(1,511,"Pending")]:
            row=sample(0,planted);row.update(ready=1,support_mask=mask,support_samples=9,support_state=state)
            with self.assertRaises(ValueError):self.trace([row])


if __name__ == "__main__":
    unittest.main()
