#!/usr/bin/env python3
"""Provision the exact checksum-verified actionlint release asset."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import sys
import tarfile
import tempfile
import tomllib
from urllib.request import urlopen
import zipfile
from contextlib import contextmanager
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # type: ignore  # noqa: E402
from molt.file_publication import atomic_write_bytes, durable_replace  # noqa: E402
from molt.file_locks import _acquire_file_lock, _release_file_lock  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_CI")

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "config" / "actionlint.toml"


def platform_key(system: str, machine: str) -> str:
    """Map an exact supported host tuple to the upstream asset spelling."""
    normalized_system = system.lower()
    normalized_machine = machine.lower()
    arch = {
        "amd64": "amd64",
        "x86_64": "amd64",
        "386": "386",
        "i386": "386",
        "i686": "386",
        "x86": "386",
        "arm64": "arm64",
        "aarch64": "arm64",
        "armv6": "armv6",
        "armv6l": "armv6",
    }.get(normalized_machine)
    key = f"{normalized_system}-{arch}" if arch else ""
    supported = {
        "darwin-amd64",
        "darwin-arm64",
        "freebsd-386",
        "freebsd-amd64",
        "linux-386",
        "linux-amd64",
        "linux-arm64",
        "linux-armv6",
        "windows-386",
        "windows-amd64",
        "windows-arm64",
    }
    if key not in supported:
        raise RuntimeError(f"unsupported actionlint platform: {system}-{machine}")
    return key


def _contract() -> tuple[str, str, str, str]:
    payload = tomllib.loads(MANIFEST.read_text(encoding="utf-8"))
    if payload.get("schema") != "molt.actionlint-toolchain.v1":
        raise RuntimeError("unsupported actionlint toolchain schema")
    key = platform_key(platform.system(), platform.machine())
    digest = payload.get("assets", {}).get(key)
    executable_digest = payload.get("executables", {}).get(key)
    if not isinstance(digest, str) or not isinstance(executable_digest, str):
        raise RuntimeError(f"unsupported actionlint platform: {key}")
    return str(payload["version"]), key, digest, executable_digest


def _install_root(version: str, platform_key: str) -> Path:
    base = Path(
        os.environ.get(
            "MOLT_TOOLCHAIN_CACHE",
            Path.home() / ".cache" / "molt" / "toolchains",
        )
    )
    return base / f"actionlint-{version}-{platform_key}"


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _valid(
    executable: Path,
    version: str,
    expected_archive: str,
    expected_executable: str,
) -> bool:
    receipt = executable.parent / "actionlint-receipt.json"
    if not executable.is_file() or not receipt.is_file():
        return False
    try:
        attestation = json.loads(receipt.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    if _sha256(executable) != expected_executable:
        return False
    if attestation != {
        "schema": "molt.actionlint-install.v1",
        "version": version,
        "archive_sha256": expected_archive,
        "executable_sha256": expected_executable,
    }:
        return False
    result = _COMMANDS.run(
        [str(executable), "-version"],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    lines = result.stdout.splitlines()
    return (
        result.returncode == 0
        and bool(lines)
        and lines[0]
        in {
            version,
            f"v{version}",
        }
    )


@contextmanager
def _install_lock(path: Path, timeout: float = 60.0):
    """Own the install deadline; shared file locks own thread/process custody."""
    handle = _acquire_file_lock(
        path,
        timeout_s=timeout,
        timeout_message=f"timed out waiting for actionlint install lock: {path}",
    )
    try:
        yield
    finally:
        _release_file_lock(handle)


def ensure_actionlint(*, install: bool = True) -> Path:
    version, platform_key, expected, expected_executable = _contract()
    executable_name = (
        "actionlint.exe" if platform_key.startswith("windows-") else "actionlint"
    )
    destination = _install_root(version, platform_key)
    executable = destination / executable_name
    if _valid(executable, version, expected, expected_executable):
        return executable
    if not install:
        raise RuntimeError(f"actionlint v{version} is not provisioned")

    suffix = ".zip" if platform_key.startswith("windows-") else ".tar.gz"
    asset = f"actionlint_{version}_{platform_key.replace('-', '_')}{suffix}"
    url = f"https://github.com/rhysd/actionlint/releases/download/v{version}/{asset}"
    destination.parent.mkdir(parents=True, exist_ok=True)
    lock = destination.with_name(destination.name + ".lock")
    with _install_lock(lock):
        if _valid(executable, version, expected, expected_executable):
            return executable
        destination.mkdir(parents=True, exist_ok=True)
        # Stage with the executable: TMPDIR and even the install root's parent
        # may be on another volume when the root is a mount point or alias.
        with tempfile.TemporaryDirectory(
            prefix=".actionlint-bootstrap-", dir=destination
        ) as tmp:
            temp_root = Path(tmp)
            archive = temp_root / asset
            with (
                urlopen(url, timeout=60) as response,
                archive.open("wb") as out,
            ):
                shutil.copyfileobj(response, out)
            actual = _sha256(archive)
            if actual != expected:
                raise RuntimeError(
                    f"actionlint asset digest mismatch: expected {expected}, got {actual}"
                )
            extracted = temp_root / executable_name
            if suffix == ".zip":
                with zipfile.ZipFile(archive) as bundle:
                    bundle.extract(executable_name, temp_root)
            else:
                with tarfile.open(archive, "r:gz") as bundle:
                    member = bundle.getmember("actionlint")
                    member.name = executable_name
                    bundle.extract(member, temp_root, filter="data")
            if not platform_key.startswith("windows-"):
                extracted.chmod(0o755)
            executable_sha = _sha256(extracted)
            if executable_sha != expected_executable:
                raise RuntimeError(
                    "actionlint executable digest mismatch: "
                    f"expected {expected_executable}, got {executable_sha}"
                )
            durable_replace(extracted, executable)
            atomic_write_bytes(
                destination / "actionlint-receipt.json",
                (
                    json.dumps(
                        {
                            "schema": "molt.actionlint-install.v1",
                            "version": version,
                            "archive_sha256": expected,
                            "executable_sha256": executable_sha,
                        },
                        indent=2,
                        sort_keys=True,
                    )
                    + "\n"
                ).encode("utf-8"),
            )
        if not _valid(executable, version, expected, expected_executable):
            raise RuntimeError("provisioned actionlint failed attestation verification")
        return executable


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        executable = ensure_actionlint(install=not args.check)
    except (OSError, RuntimeError) as exc:
        print(f"actionlint bootstrap: {exc}", file=sys.stderr)
        return 2
    print(executable)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
