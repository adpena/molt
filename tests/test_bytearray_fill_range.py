from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process


SCRIPT = (
    "def build():\n"
    "    size = 16\n"
    "    data = bytearray(size)\n"
    "    i = 0\n"
    "    while i < size:\n"
    "        data[i] = 97\n"
    "        i += 1\n"
    "    data[-1] = 98\n"
    "    payload = bytes(data)\n"
    "    return payload.find(b'aaaa'), payload[0], payload[14], payload[15], i\n"
    "\n"
    "pos, first, penultimate, last, final_i = build()\n"
    "print(pos)\n"
    "print(first)\n"
    "print(penultimate)\n"
    "print(last)\n"
    "print(final_i)\n"
)


def _native_env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="bytearray-fill-range",
        session_id=os.environ.get("MOLT_SESSION_ID") or "bytearray-fill-range-test",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_BACKEND_DAEMON_CACHE_MB", "128")
    return env


def test_bytearray_counted_fill_native_parity(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native bytearray fill test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "bytearray_fill_native.py"
    src.write_text(SCRIPT, encoding="utf-8")

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            "--profile",
            "dev",
            str(src),
        ],
        cwd=root,
        env=_native_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert run.returncode == 0, run.stderr
    lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
    assert lines == ["0", "97", "97", "98", "16"]
