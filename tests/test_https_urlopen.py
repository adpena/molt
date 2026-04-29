"""End-to-end TLS / HTTPS regression test.

Compiles a small Python program that uses ``urllib.request.urlopen`` against
``https://example.com`` and verifies that the rustls-backed network stack can
negotiate TLS, perform the request, and surface the response status to the
compiled binary.

The test is gated on internet access — if DNS resolution or the network is
unavailable, it skips. When the network is reachable, it must succeed; a
runtime error means the TLS plumbing has regressed.
"""

from __future__ import annotations

import os
import socket
import subprocess
import sys
import tempfile
from pathlib import Path

import pytest

MOLT_DIR = Path(__file__).resolve().parents[1]
ARTIFACT_ROOT = Path(os.environ.get("MOLT_EXT_ROOT", str(MOLT_DIR))).expanduser()


def _network_available() -> bool:
    try:
        with socket.create_connection(("example.com", 443), timeout=4):
            return True
    except OSError:
        return False


pytestmark = pytest.mark.skipif(
    not _network_available(),
    reason="network unavailable; cannot exercise live HTTPS endpoint",
)


_PROGRAM = """\
import urllib.request

with urllib.request.urlopen("https://example.com/", timeout=15) as resp:
    status = int(resp.status)
    body = resp.read()
print(f"STATUS={status}")
print(f"BODY_LEN={len(body)}")
print(f"HAS_DOCTYPE={b'<!doctype html>' in body.lower() or b'<!DOCTYPE html>' in body}")
"""


def _compile_and_run(source: str) -> str:
    with tempfile.TemporaryDirectory() as tmp:
        src_path = Path(tmp) / "https_demo.py"
        src_path.write_text(source)
        binary_path = Path(tmp) / "https_demo_molt"

        env = {
            **os.environ,
            "MOLT_EXT_ROOT": str(ARTIFACT_ROOT),
            "CARGO_TARGET_DIR": os.environ.get(
                "CARGO_TARGET_DIR", str(ARTIFACT_ROOT / "target")
            ),
            "PYTHONPATH": str(MOLT_DIR / "src"),
        }
        build = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                str(src_path),
                "--out-dir",
                str(tmp),
            ],
            capture_output=True,
            text=True,
            timeout=600,
            env=env,
            cwd=str(MOLT_DIR),
        )
        if build.returncode != 0:
            pytest.fail(
                "molt build failed:\n"
                f"stdout:\n{build.stdout[-2000:]}\n"
                f"stderr:\n{build.stderr[-2000:]}"
            )
        if not binary_path.exists():
            pytest.fail(f"compiled binary missing at {binary_path}")
        run = subprocess.run(
            [str(binary_path)],
            capture_output=True,
            text=True,
            timeout=30,
        )
        if run.returncode != 0:
            pytest.fail(
                f"compiled binary failed:\nstdout:\n{run.stdout}\nstderr:\n{run.stderr}"
            )
        return run.stdout


def test_urllib_request_urlopen_https() -> None:
    out = _compile_and_run(_PROGRAM)
    lines = {
        line.split("=", 1)[0]: line.split("=", 1)[1]
        for line in out.strip().splitlines()
        if "=" in line
    }
    assert lines.get("STATUS") == "200", out
    body_len = int(lines.get("BODY_LEN", "0"))
    assert body_len > 256, f"body too short ({body_len}): {out}"
    assert lines.get("HAS_DOCTYPE") == "True", out
