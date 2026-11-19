import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest import mock

spec = importlib.util.spec_from_file_location("image_preview", Path(__file__).with_name("image-preview.py"))
preview = importlib.util.module_from_spec(spec)
spec.loader.exec_module(preview)


class PreviewTests(unittest.TestCase):
    def test_preview_has_a_companion_name(self):
        self.assertEqual(
            preview.preview_path(Path("frames/0000.png")),
            Path("frames/0000.preview.jpg"),
        )

    def test_scaling_never_upscales_and_bounds_both_dimensions(self):
        value = preview.scale_filter(1920)
        self.assertIn("min(iw,1920)", value)
        self.assertIn("min(ih,1920)", value)
        self.assertIn("force_original_aspect_ratio=decrease", value)

    def test_missing_image_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "does not exist"):
            preview.create_preview("missing-image.png")

    def test_conversion_preserves_source_and_reuses_current_preview(self):
        with tempfile.TemporaryDirectory() as folder:
            source = Path(folder) / "кадр 0000.png"
            source.write_bytes(b"original")

            def fake_run(command, **kwargs):
                Path(command[-1]).write_bytes(b"preview")
                return mock.Mock(returncode=0, stderr="")

            with mock.patch.object(preview, "ffmpeg_executable", return_value="ffmpeg"), \
                    mock.patch.object(preview.subprocess, "run", side_effect=fake_run) as run:
                target = preview.create_preview(source)
                self.assertEqual(source.read_bytes(), b"original")
                self.assertEqual(target.read_bytes(), b"preview")
                self.assertEqual(preview.create_preview(source), target)
                self.assertEqual(run.call_count, 1)
                self.assertIn("scale=", run.call_args.args[0][run.call_args.args[0].index("-vf") + 1])


if __name__ == "__main__":
    unittest.main()
