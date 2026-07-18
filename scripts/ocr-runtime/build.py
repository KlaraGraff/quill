#!/usr/bin/env python3
"""Build and verify a relocatable Lantern OCR runtime archive."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any
from uuid import UUID


SCRIPT_DIR = Path(__file__).resolve().parent
PACKAGE_ROOT_NAME = "lantern-ocr-runtime"
PACKAGE_ID = "lantern-ocr-runtime"
PACKAGE_VERSION = os.environ.get("OCR_RUNTIME_VERSION", "1.0.0")
PYTHON_PACKAGES = tuple(
    line.strip()
    for line in (SCRIPT_DIR / "requirements.lock").read_text(encoding="utf-8").splitlines()
    if line.strip() and not line.startswith("#")
)


def run(command: list[str | Path], **kwargs: Any) -> subprocess.CompletedProcess[str]:
    printable = " ".join(str(part) for part in command)
    print(f"+ {printable}", file=sys.stderr)
    return subprocess.run([str(part) for part in command], check=True, text=True, **kwargs)


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def download(url: str, expected_sha256: str, destination: Path) -> Path:
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != "https" or not parsed.netloc:
        raise RuntimeError(f"refusing non-HTTPS source: {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    request = urllib.request.Request(url, headers={"User-Agent": "Lantern-OCR-runtime-builder"})
    with urllib.request.urlopen(request, timeout=120) as response, destination.open("wb") as target:
        shutil.copyfileobj(response, target)
    actual = sha256(destination)
    if actual != expected_sha256:
        destination.unlink(missing_ok=True)
        raise RuntimeError(f"SHA-256 mismatch for {url}: expected {expected_sha256}, got {actual}")
    return destination


def safe_extract_tar(archive: Path, destination: Path) -> None:
    destination = destination.resolve()
    with tarfile.open(archive, "r:*") as bundle:
        for member in bundle.getmembers():
            candidate = (destination / member.name).resolve()
            if candidate != destination and destination not in candidate.parents:
                raise RuntimeError(f"archive path escapes destination: {member.name}")
            if member.issym() or member.islnk():
                link_target = (candidate.parent / member.linkname).resolve()
                if link_target != destination and destination not in link_target.parents:
                    raise RuntimeError(f"archive link escapes destination: {member.name}")
        bundle.extractall(destination, filter="data")


def platform_settings(platform: str) -> dict[str, str]:
    if platform == "macos-arm64":
        return {
            "platform": "macos",
            "arch": "arm64",
            "minimum_os_version": "12.0",
            "python": "python/bin/python3",
            "launcher": "bin/lantern-ocr",
            "tesseract": "tesseract",
            "triplet": "arm64-osx-lantern",
        }
    return {
        "platform": "windows",
        "arch": "x86_64",
        "minimum_os_version": "10.0.17763",
        "python": "python/python.exe",
        "launcher": "bin/lantern-ocr.exe",
        "tesseract": "tesseract.exe",
        "triplet": "x64-windows-lantern",
    }


def bootstrap_python(root: Path, source: dict[str, str], cache: Path) -> Path:
    archive = download(source["url"], source["sha256"], cache / Path(source["url"]).name)
    safe_extract_tar(archive, root)
    python_root = root / "python"
    if not python_root.is_dir():
        raise RuntimeError("python-build-standalone archive does not contain python/")
    return python_root


def install_python_packages(python: Path, root: Path) -> list[dict[str, Any]]:
    run([python, "-m", "pip", "install", "--disable-pip-version-check", "--no-compile", *PYTHON_PACKAGES])
    runtime_lib = root / "lib"
    runtime_lib.mkdir(parents=True, exist_ok=True)
    shutil.copy2(SCRIPT_DIR / "runtime" / "lantern_ocr.py", runtime_lib)
    shutil.copy2(SCRIPT_DIR / "runtime" / "lantern_progress.py", runtime_lib)
    return python_components(python)


def python_components(python: Path) -> list[dict[str, Any]]:
    script = """
import json
from importlib import metadata
items=[]
for dist in metadata.distributions():
    name=dist.metadata.get('Name')
    if name:
        items.append({'type':'library','name':name,'version':dist.version,'license':dist.metadata.get('License-Expression') or dist.metadata.get('License') or 'NOASSERTION','purl':f\"pkg:pypi/{name.lower().replace('_','-')}@{dist.version}\"})
print(json.dumps(sorted(items,key=lambda item:item['name'].lower())))
"""
    result = run([python, "-c", script], stdout=subprocess.PIPE)
    return json.loads(result.stdout)


def build_tesseract(root: Path, settings: dict[str, str], vcpkg_root: Path) -> list[dict[str, Any]]:
    triplet = settings["triplet"]
    triplet_file = SCRIPT_DIR / "triplets" / f"{triplet}.cmake"
    overlay = vcpkg_root / "triplets" / "community" / triplet_file.name
    shutil.copy2(triplet_file, overlay)
    vcpkg = vcpkg_root / ("vcpkg.exe" if os.name == "nt" else "vcpkg")
    run([vcpkg, "install", f"tesseract:{triplet}", "--clean-after-build"])

    installed = vcpkg_root / "installed" / triplet
    tool = installed / "tools" / "tesseract" / settings["tesseract"]
    if not tool.is_file():
        raise RuntimeError(f"vcpkg did not produce {tool}")
    destination = root / "bin" / settings["tesseract"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(tool, destination)
    if os.name != "nt":
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

    versions = run([destination, "--version"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT).stdout.splitlines()
    components = vcpkg_components(installed, root)
    for component in components:
        if component["name"] == "tesseract":
            component["version"] = versions[0].removeprefix("tesseract ").strip()
            component["license"] = "Apache-2.0"
    return components


def vcpkg_components(installed: Path, root: Path) -> list[dict[str, Any]]:
    status_file = installed.parent / "vcpkg" / "status"
    if not status_file.is_file():
        raise RuntimeError(f"missing vcpkg status file: {status_file}")
    components: list[dict[str, Any]] = []
    for paragraph in status_file.read_text(encoding="utf-8").split("\n\n"):
        fields: dict[str, str] = {}
        for line in paragraph.splitlines():
            if ": " in line:
                key, value = line.split(": ", 1)
                fields[key] = value
        name = fields.get("Package")
        version = fields.get("Version")
        if not name or not version or fields.get("Status") != "install ok installed":
            continue
        copyright_file = installed / "share" / name / "copyright"
        copied_name = None
        if copyright_file.is_file():
            licenses = root / "licenses"
            licenses.mkdir(parents=True, exist_ok=True)
            copied_name = f"vcpkg-{name}-copyright.txt"
            shutil.copy2(copyright_file, licenses / copied_name)
        components.append(
            {
                "type": "library",
                "name": name,
                "version": version,
                "license": "NOASSERTION",
                "license_files": [copied_name] if copied_name else [],
                "purl": f"pkg:generic/{urllib.parse.quote(name, safe='')}@{urllib.parse.quote(version, safe='')}?repository_url=https%3A%2F%2Fgithub.com%2Fmicrosoft%2Fvcpkg",
            }
        )
    if not any(component["name"] == "tesseract" for component in components):
        raise RuntimeError("vcpkg SBOM does not contain tesseract")
    return components


def compile_launcher(root: Path, settings: dict[str, str]) -> None:
    python_root = root / "python"
    destination = root / settings["launcher"]
    destination.parent.mkdir(parents=True, exist_ok=True)
    source = SCRIPT_DIR / "launcher" / "lantern_ocr.c"
    if os.name == "nt":
        include = python_root / "include"
        libs = python_root / "libs"
        run(
            [
                "cl.exe",
                "/nologo",
                "/O2",
                "/MT",
                "/utf-8",
                f"/I{include}",
                str(source),
                f"/link",
                f"/LIBPATH:{libs}",
                "python312.lib",
                f"/OUT:{destination}",
            ]
        )
        python_dll = python_root / "python312.dll"
        if not python_dll.is_file():
            raise RuntimeError(f"missing embedded Python DLL: {python_dll}")
        shutil.copy2(python_dll, destination.parent / python_dll.name)
    else:
        run(
            [
                "clang",
                "-O2",
                "-mmacosx-version-min=12.0",
                f"-I{python_root / 'include' / 'python3.12'}",
                source,
                "-o",
                destination,
                f"-L{python_root / 'lib'}",
                "-lpython3.12",
                "-ldl",
                "-framework",
                "CoreFoundation",
                "-Wl,-rpath,@executable_path/../python/lib",
            ]
        )
        destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def install_tessdata(root: Path, lock: dict[str, Any], cache: Path) -> list[dict[str, Any]]:
    destination = root / "share" / "tessdata"
    destination.mkdir(parents=True, exist_ok=True)
    for language, source in lock["models"].items():
        download(source["url"], source["sha256"], destination / f"{language}.traineddata")
    shutil.copytree(SCRIPT_DIR / "tessdata" / "configs", destination / "configs")
    download(lock["license"]["url"], lock["license"]["sha256"], cache / "tessdata-fast-LICENSE")
    return [
        {
            "type": "data",
            "name": f"tessdata-fast-{language}",
            "version": lock["version"],
            "license": "Apache-2.0",
            "sha256": source["sha256"],
            "purl": f"pkg:github/tesseract-ocr/tessdata_fast@{lock['version']}#{language}.traineddata",
        }
        for language, source in sorted(lock["models"].items())
    ]


def create_fixture(root: Path, python: Path) -> None:
    fixture_dir = root / "share" / "fixtures"
    fixture_dir.mkdir(parents=True, exist_ok=True)
    fixture = fixture_dir / "scan-fixture.pdf"
    script = """
from pathlib import Path
import sys
from PIL import Image, ImageDraw, ImageFont
image=Image.new('RGB',(1654,2339),'white')
draw=ImageDraw.Draw(image)
font=ImageFont.load_default(size=72)
draw.text((120,350),'Lantern OCR self test',fill='black',font=font)
image.save(sys.argv[1],'PDF',resolution=200.0)
"""
    run([python, "-c", script, fixture])


def write_runtime_metadata(
    root: Path,
    settings: dict[str, str],
    source_lock: dict[str, Any],
    components: list[dict[str, Any]],
) -> None:
    runtime = {
        "package_id": PACKAGE_ID,
        "version": PACKAGE_VERSION,
        "platform": settings["platform"],
        "arch": settings["arch"],
        "minimum_os_version": settings["minimum_os_version"],
        "entrypoint": settings["launcher"].replace("\\", "/"),
        "tessdata_prefix": "share/tessdata",
        "language_profile": "chi_sim+eng",
        "quality_profile": "fast",
        "sources": source_lock,
    }
    (root / "runtime.json").write_text(json.dumps(runtime, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    serial_seed = hashlib.sha256((settings["platform"] + PACKAGE_VERSION).encode()).digest()
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "serialNumber": f"urn:uuid:{UUID(bytes=serial_seed[:16])}",
        "version": 1,
        "metadata": {"component": {"type": "application", "name": PACKAGE_ID, "version": PACKAGE_VERSION}},
        "components": [_cyclonedx_component(component) for component in components],
    }
    (root / "SBOM.cdx.json").write_text(json.dumps(sbom, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def _cyclonedx_component(component: dict[str, Any]) -> dict[str, Any]:
    result = {
        key: value
        for key, value in component.items()
        if key in {"type", "name", "version", "purl"} and value
    }
    digest = component.get("sha256")
    if digest:
        result["hashes"] = [{"alg": "SHA-256", "content": digest}]
    declared_license = component.get("license", "")
    if re.fullmatch(r"[A-Za-z0-9-.+]+", declared_license) and declared_license != "NOASSERTION":
        result["licenses"] = [{"license": {"id": declared_license}}]
    elif declared_license:
        result["properties"] = [{"name": "lantern:declared-license", "value": declared_license[:1024]}]
    return result


def write_notices(root: Path, python: Path, components: list[dict[str, Any]], tessdata_license: Path) -> None:
    licenses = root / "licenses"
    licenses.mkdir(parents=True, exist_ok=True)
    shutil.copy2(tessdata_license, licenses / "tessdata-fast-Apache-2.0.txt")
    script = """
import json, pathlib, sys
from importlib import metadata
out=pathlib.Path(sys.argv[1]); result=[]
for dist in metadata.distributions():
    name=dist.metadata.get('Name') or 'unknown'; copied=[]
    for file in dist.files or ():
        leaf=pathlib.PurePosixPath(str(file)).name.lower()
        if leaf.startswith(('license','copying','notice')):
            source=pathlib.Path(dist.locate_file(file))
            if source.is_file() and source.stat().st_size <= 2_000_000:
                target=out / f\"{name}-{leaf}\"; target.write_bytes(source.read_bytes()); copied.append(target.name)
    result.append({'name':name,'version':dist.version,'files':sorted(set(copied))})
print(json.dumps(sorted(result,key=lambda item:item['name'].lower())))
"""
    copied = json.loads(run([python, "-c", script, licenses], stdout=subprocess.PIPE).stdout)
    lines = [
        "Lantern OCR runtime third-party notices",
        "========================================",
        "",
        "This runtime is distributed by the MIT-licensed Lantern project. It bundles",
        "independent third-party components under their respective licenses.",
        "Corresponding license texts are in licenses/. Source locations and exact",
        "versions are recorded in SBOM.cdx.json and runtime.json.",
        "",
    ]
    license_files = {item["name"].lower(): item["files"] for item in copied}
    for component in sorted(components, key=lambda item: item["name"].lower()):
        files = [*license_files.get(component["name"].lower(), []), *component.get("license_files", [])]
        license_name = " ".join(str(component.get("license", "NOASSERTION")).split())[:300]
        lines.append(f"- {component['name']} {component.get('version', '')}: {license_name}")
        if files:
            lines.append(f"  License files: {', '.join(files)}")
    lines.extend(
        [
            "- tessdata_fast 4.1.0: Apache-2.0",
            "  License file: tessdata-fast-Apache-2.0.txt",
            "- CPython 3.12.13: Python-2.0",
            "  License text is bundled in the python distribution.",
            "- Tesseract and its statically linked vcpkg dependencies: see SBOM.cdx.json",
            "  and https://github.com/microsoft/vcpkg/tree/52c9e08cdf8580d2d9762f547d22b96fd81e82f2/ports.",
            "",
        ]
    )
    (root / "THIRD_PARTY_NOTICES.txt").write_text("\n".join(lines), encoding="utf-8")


def installed_size(root: Path) -> int:
    return sum(path.stat().st_size for path in root.rglob("*") if path.is_file())


def archive_runtime(root: Path, output: Path) -> None:
    tar_path = output.with_suffix("")
    with tarfile.open(tar_path, "w", format=tarfile.PAX_FORMAT) as bundle:
        bundle.add(root, arcname=PACKAGE_ROOT_NAME, recursive=True)
    run(["zstd", "-19", "--threads=0", "--force", tar_path, "-o", output])
    tar_path.unlink()


def verify_relocated(root: Path, settings: dict[str, str]) -> None:
    with tempfile.TemporaryDirectory(prefix="lantern-ocr-relocation-") as temporary:
        relocated = Path(temporary) / "folder with spaces" / PACKAGE_ROOT_NAME
        relocated.parent.mkdir(parents=True)
        shutil.copytree(root, relocated)
        executable = relocated / settings["launcher"]
        run([executable, "--version"])
        result = run([executable, "--self-test"], stdout=subprocess.PIPE)
        payload = json.loads(result.stdout)
        if payload.get("ok") is not True:
            raise RuntimeError("runtime self-test did not report success")


def write_manifest(output_dir: Path, archive: Path, settings: dict[str, str], size: int) -> Path:
    if not archive.name.endswith(".tar.zst"):
        raise RuntimeError("runtime archive must be tar.zst")
    tag = f"ocr-runtime-v{PACKAGE_VERSION}"
    url = f"https://github.com/KlaraGraff/lantern/releases/download/{tag}/{archive.name}"
    manifest = {
        "package_id": PACKAGE_ID,
        "version": PACKAGE_VERSION,
        "platform": settings["platform"],
        "arch": settings["arch"],
        "minimum_os_version": settings["minimum_os_version"],
        "download_size": archive.stat().st_size,
        "installed_size": size,
        "sha256": sha256(archive),
        "url": url,
    }
    path = output_dir / f"{archive.name}.manifest.json"
    path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    target = "macos-arm64" if settings["platform"] == "macos" else "windows-x64"
    (output_dir / f"manifest-{target}.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    (output_dir / f"{archive.name}.sha256").write_text(f"{manifest['sha256']}  {archive.name}\n", encoding="ascii")
    return path


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--platform", choices=("macos-arm64", "windows-x86_64"), required=True)
    parser.add_argument("--vcpkg-root", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    arguments = parser.parse_args()
    settings = platform_settings(arguments.platform)
    if (settings["platform"] == "windows") != (os.name == "nt"):
        parser.error(f"{arguments.platform} must be built on its native runner")

    lock = json.loads((SCRIPT_DIR / "sources.lock.json").read_text(encoding="utf-8"))
    output_dir = arguments.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="lantern-ocr-build-") as temporary:
        workspace = Path(temporary)
        cache = workspace / "cache"
        root = workspace / PACKAGE_ROOT_NAME
        root.mkdir()
        bootstrap_python(root, lock["python"][arguments.platform], cache)
        python = root / settings["python"]
        components = [
            {
                "type": "application",
                "name": "CPython",
                "version": lock["python"][arguments.platform]["version"],
                "license": "Python-2.0",
                "sha256": lock["python"][arguments.platform]["sha256"],
                "purl": f"pkg:generic/cpython@{lock['python'][arguments.platform]['version']}",
            }
        ]
        components.extend(install_python_packages(python, root))
        components.extend(build_tesseract(root, settings, arguments.vcpkg_root.resolve()))
        components.extend(install_tessdata(root, lock["tessdata_fast"], cache))
        create_fixture(root, python)
        compile_launcher(root, settings)
        write_runtime_metadata(root, settings, lock, components)
        write_notices(root, python, components, cache / "tessdata-fast-LICENSE")
        verify_relocated(root, settings)
        package_name = f"{PACKAGE_ID}-{PACKAGE_VERSION}-{settings['platform']}-{settings['arch']}.tar.zst"
        archive = output_dir / package_name
        size = installed_size(root)
        archive_runtime(root, archive)
        manifest = write_manifest(output_dir, archive, settings, size)
        shutil.copy2(root / "SBOM.cdx.json", output_dir / f"{package_name}.sbom.cdx.json")
        shutil.copy2(root / "THIRD_PARTY_NOTICES.txt", output_dir / f"{package_name}.THIRD_PARTY_NOTICES.txt")
        print(manifest)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
