from __future__ import annotations

from concurrent.futures import ThreadPoolExecutor
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import zipfile

import pytest

from tools import bootstrap_actionlint as bootstrap


@pytest.mark.parametrize(
    ("system", "machine", "expected"),
    [
        ("Linux", "i686", "linux-386"),
        ("Linux", "x86_64", "linux-amd64"),
        ("Linux", "aarch64", "linux-arm64"),
        ("Linux", "armv6l", "linux-armv6"),
        ("Windows", "AMD64", "windows-amd64"),
        ("Windows", "ARM64", "windows-arm64"),
        ("Windows", "i386", "windows-386"),
        ("Darwin", "x86_64", "darwin-amd64"),
        ("Darwin", "arm64", "darwin-arm64"),
        ("FreeBSD", "i386", "freebsd-386"),
        ("FreeBSD", "amd64", "freebsd-amd64"),
    ],
)
def test_platform_key_is_exact(system: str, machine: str, expected: str) -> None:
    assert bootstrap.platform_key(system, machine) == expected


def test_platform_key_rejects_unknown_architecture() -> None:
    with pytest.raises(RuntimeError, match="unsupported"):
        bootstrap.platform_key("Linux", "mips64")


def test_valid_rejects_tampered_executable(tmp_path: Path, monkeypatch) -> None:
    executable = tmp_path / "actionlint.exe"
    executable.write_bytes(b"trusted")
    receipt = {
        "schema": "molt.actionlint-install.v1",
        "version": "1.7.12",
        "archive_sha256": "archive",
        "executable_sha256": hashlib.sha256(b"trusted").hexdigest(),
    }
    (tmp_path / "actionlint-receipt.json").write_text(
        json.dumps(receipt), encoding="utf-8"
    )
    monkeypatch.setattr(
        bootstrap.subprocess,
        "run",
        lambda *_args, **_kwargs: subprocess.CompletedProcess([], 0, "v1.7.12\n", ""),
    )
    executable_digest = hashlib.sha256(b"trusted").hexdigest()
    assert bootstrap._valid(
        executable, "1.7.12", "archive", executable_digest
    ) is True
    executable.write_bytes(b"tampered")
    assert bootstrap._valid(
        executable, "1.7.12", "archive", executable_digest
    ) is False


def _zip_asset(payload: bytes) -> bytes:
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w") as archive:
        archive.writestr("actionlint.exe", payload)
    return output.getvalue()


def test_bootstrap_rejects_bad_archive_checksum(tmp_path: Path, monkeypatch) -> None:
    archive = _zip_asset(b"binary")
    monkeypatch.setattr(
        bootstrap,
        "_contract",
        lambda: ("1.7.12", "windows-amd64", "0" * 64, "1" * 64),
    )
    monkeypatch.setattr(bootstrap, "_install_root", lambda *_args: tmp_path / "tool")
    monkeypatch.setattr(
        bootstrap.urllib.request, "urlopen", lambda *_args, **_kwargs: io.BytesIO(archive)
    )

    with pytest.raises(RuntimeError, match="digest mismatch"):
        bootstrap.ensure_actionlint()


def test_concurrent_bootstrap_publishes_once(tmp_path: Path, monkeypatch) -> None:
    archive = _zip_asset(b"binary")
    expected = hashlib.sha256(archive).hexdigest()
    downloads = 0

    def urlopen(*_args, **_kwargs):
        nonlocal downloads
        downloads += 1
        return io.BytesIO(archive)

    monkeypatch.setattr(
        bootstrap,
        "_contract",
        lambda: (
            "1.7.12",
            "windows-amd64",
            expected,
            hashlib.sha256(b"binary").hexdigest(),
        ),
    )
    monkeypatch.setattr(bootstrap, "_install_root", lambda *_args: tmp_path / "tool")
    monkeypatch.setattr(bootstrap.urllib.request, "urlopen", urlopen)
    monkeypatch.setattr(
        bootstrap,
        "_valid",
        lambda executable, *_args: executable.exists()
        and (executable.parent / "actionlint-receipt.json").exists(),
    )

    with ThreadPoolExecutor(max_workers=2) as executor:
        paths = list(executor.map(lambda _index: bootstrap.ensure_actionlint(), range(2)))

    assert paths[0] == paths[1]
    assert downloads == 1


def test_kernel_lock_recovers_after_owner_process_exit(tmp_path: Path) -> None:
    lock = tmp_path / "install.lock"
    code = (
        "import os; from pathlib import Path; "
        "from tools.bootstrap_actionlint import _install_lock; "
        f"cm=_install_lock(Path({str(lock)!r})); cm.__enter__(); os._exit(0)"
    )
    completed = subprocess.run([sys.executable, "-c", code], cwd=bootstrap.ROOT)
    assert completed.returncode == 0
    with bootstrap._install_lock(lock, timeout=1):
        assert lock.exists()
