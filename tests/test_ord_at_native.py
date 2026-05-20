from __future__ import annotations

import os
import shutil
import sys
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process


PROGRAM = """
def show(label: str, fn) -> None:
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))

text = "AéZ"
show("ascii", lambda: ord(text[0]))
show("unicode", lambda: ord(text[1]))
show("negative", lambda: ord(text[-1]))
show("slice", lambda: ord(text[0:1]))
show("bytes-fallback", lambda: ord(b"A"[0]))
show("oob", lambda: ord(text[99]))
"""


def _env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    return env


def test_ord_at_native_semantics(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native ord_at test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "ord_at_native.py"
    src.write_text(PROGRAM, encoding="utf-8")

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
        env=_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert run.returncode == 0, run.stderr
    lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
    assert lines == [
        "ascii 65",
        "unicode 233",
        "negative 90",
        "slice 65",
        "bytes-fallback TypeError ord() expected string of length 1, but int found",
        "oob IndexError string index out of range",
    ]
