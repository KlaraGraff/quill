from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import tarfile
import tempfile
import unittest
from pathlib import Path
from unittest import mock


ROOT = Path(__file__).resolve().parents[1]


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


build = load_module("ocr_runtime_build", ROOT / "build.py")
validate = load_module("ocr_runtime_validate", ROOT / "validate_artifacts.py")


class RuntimeBuildTests(unittest.TestCase):
    def test_safe_extract_rejects_parent_traversal(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "escape.tar"
            with tarfile.open(archive, "w") as bundle:
                entry = tarfile.TarInfo("../escaped")
                entry.size = 1
                bundle.addfile(entry, io.BytesIO(b"x"))
            with self.assertRaisesRegex(RuntimeError, "escapes destination"):
                build.safe_extract_tar(archive, root / "out")

    def test_download_rejects_non_https_before_io(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            with self.assertRaisesRegex(RuntimeError, "non-HTTPS"):
                build.download("http://example.test/runtime", "00", Path(temporary) / "runtime")

    def test_download_rejects_sha256_mismatch(self) -> None:
        response = mock.MagicMock()
        response.__enter__.return_value = io.BytesIO(b"wrong bytes")
        response.__exit__.return_value = False
        with tempfile.TemporaryDirectory() as temporary, mock.patch.object(
            build.urllib.request, "urlopen", return_value=response
        ):
            destination = Path(temporary) / "runtime"
            with self.assertRaisesRegex(RuntimeError, "SHA-256 mismatch"):
                build.download("https://example.test/runtime", "0" * 64, destination)
            self.assertFalse(destination.exists())

    def test_manifest_records_actual_sizes_hash_and_https_url(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "lantern-ocr-runtime-1.0.0-macos-arm64.tar.zst"
            archive.write_bytes(b"runtime bytes")
            manifest_path = build.write_manifest(
                root,
                archive,
                build.platform_settings("macos-arm64"),
                1234,
            )
            manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
            self.assertEqual(manifest["download_size"], len(b"runtime bytes"))
            self.assertEqual(manifest["installed_size"], 1234)
            self.assertEqual(manifest["sha256"], hashlib.sha256(b"runtime bytes").hexdigest())
            self.assertTrue(manifest["url"].startswith("https://github.com/"))


class ArtifactValidatorTests(unittest.TestCase):
    def test_validator_accepts_complete_release_metadata(self) -> None:
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            archive = root / "lantern-ocr-runtime-1.0.0-macos-arm64.tar.zst"
            archive.write_bytes(b"archive")
            digest = hashlib.sha256(b"archive").hexdigest()
            manifest = {
                "package_id": "lantern-ocr-runtime",
                "version": "1.0.0",
                "platform": "macos",
                "arch": "arm64",
                "minimum_os_version": "12.0",
                "download_size": archive.stat().st_size,
                "installed_size": 42,
                "sha256": digest,
                "url": f"https://github.com/KlaraGraff/lantern/releases/download/ocr-runtime-v1.0.0/{archive.name}",
            }
            archive.with_name(archive.name + ".manifest.json").write_text(json.dumps(manifest))
            archive.with_name(archive.name + ".sha256").write_text(f"{digest}  {archive.name}\n")
            archive.with_name(archive.name + ".sbom.cdx.json").write_text(
                json.dumps({"bomFormat": "CycloneDX", "components": [{"name": "ocrmypdf"}]})
            )
            archive.with_name(archive.name + ".THIRD_PARTY_NOTICES.txt").write_text("notices")
            with mock.patch("sys.argv", ["validate_artifacts.py", str(root)]):
                self.assertEqual(validate.main(), 0)


if __name__ == "__main__":
    unittest.main()
