#!/usr/bin/env python3
"""Validate OCR runtime release metadata without unpacking platform binaries."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path
from urllib.parse import urlparse


REQUIRED = {
    "package_id",
    "version",
    "platform",
    "arch",
    "minimum_os_version",
    "download_size",
    "installed_size",
    "sha256",
    "url",
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=Path)
    parser.add_argument("--expected-platforms", nargs="*", default=[])
    arguments = parser.parse_args()
    manifests = sorted(arguments.root.rglob("*.manifest.json"))
    if not manifests:
        parser.error("no manifests found")
    seen: set[str] = set()
    for manifest_path in manifests:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        missing = REQUIRED - manifest.keys()
        if missing:
            raise RuntimeError(f"{manifest_path}: missing keys {sorted(missing)}")
        if manifest["package_id"] != "lantern-ocr-runtime":
            raise RuntimeError(f"{manifest_path}: unexpected package_id")
        identity = f"{manifest['platform']}/{manifest['arch']}"
        if identity in seen:
            raise RuntimeError(f"duplicate manifest for {identity}")
        seen.add(identity)
        archive_name = manifest_path.name.removesuffix(".manifest.json")
        archive = manifest_path.with_name(archive_name)
        if not archive.is_file():
            raise RuntimeError(f"{manifest_path}: missing archive {archive_name}")
        if manifest["download_size"] != archive.stat().st_size:
            raise RuntimeError(f"{manifest_path}: download_size mismatch")
        if manifest["installed_size"] <= 0:
            raise RuntimeError(f"{manifest_path}: invalid installed_size")
        if manifest["sha256"] != sha256(archive):
            raise RuntimeError(f"{manifest_path}: SHA-256 mismatch")
        parsed = urlparse(manifest["url"])
        if parsed.scheme != "https" or parsed.netloc != "github.com" or not parsed.path.endswith("/" + archive_name):
            raise RuntimeError(f"{manifest_path}: invalid HTTPS release URL")
        checksum = archive.with_name(archive.name + ".sha256")
        if checksum.read_text(encoding="ascii") != f"{manifest['sha256']}  {archive.name}\n":
            raise RuntimeError(f"{manifest_path}: checksum sidecar mismatch")
        for suffix in (".sbom.cdx.json", ".THIRD_PARTY_NOTICES.txt"):
            if not archive.with_name(archive.name + suffix).is_file():
                raise RuntimeError(f"{manifest_path}: missing {suffix}")
        sbom = json.loads(archive.with_name(archive.name + ".sbom.cdx.json").read_text(encoding="utf-8"))
        if sbom.get("bomFormat") != "CycloneDX" or not sbom.get("components"):
            raise RuntimeError(f"{manifest_path}: invalid SBOM")
    expected = set(arguments.expected_platforms)
    if expected and seen != expected:
        raise RuntimeError(f"expected {sorted(expected)}, found {sorted(seen)}")
    print(json.dumps({"ok": True, "platforms": sorted(seen)}, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except Exception as error:
        print(f"artifact validation failed: {error}", file=sys.stderr)
        raise SystemExit(1) from error
