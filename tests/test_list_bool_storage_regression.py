from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


SCRIPT = (
    "xs = [False] * 4\n"
    "print(xs)\n"
    "print(xs[1])\n"
    "xs[1] = True\n"
    "print(xs)\n"
    "print(xs[True])\n"
    "xs[2] = 1\n"
    "print(xs)\n"
    "print(xs[2])\n"
)

INT_SCRIPT = (
    "xs = [0] * 4\n"
    "print(xs)\n"
    "print(xs[1])\n"
    "xs[1] = 7\n"
    "print(xs)\n"
    "print(xs[1])\n"
    "xs[2] = 'x'\n"
    "print(xs)\n"
    "print(xs[2])\n"
    "print(xs[1:3])\n"
)


def _env(root: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_WASM_DISABLE_SCCACHE", "1")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    return env


def _expected_lines() -> list[str]:
    return [
        "[False, False, False, False]",
        "False",
        "[False, True, False, False]",
        "True",
        "[False, True, 1, False]",
        "1",
    ]


def _expected_int_lines() -> list[str]:
    return [
        "[0, 0, 0, 0]",
        "0",
        "[0, 7, 0, 0]",
        "7",
        "[0, 7, 'x', 0]",
        "x",
        "[7, 'x']",
    ]


def test_list_bool_storage_regression_native(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native regression test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_bool_storage_native.py"
    src.write_text(SCRIPT, encoding="utf-8")

    run = subprocess.run(
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
    assert lines == _expected_lines()


def test_list_int_storage_regression_native(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native regression test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_int_storage_native.py"
    src.write_text(INT_SCRIPT, encoding="utf-8")

    run = subprocess.run(
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
    assert lines == _expected_int_lines()


def test_list_bool_storage_regression_linked_wasm(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "list_bool_storage_wasm.py"
    src.write_text(SCRIPT, encoding="utf-8")

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    lines = [line.strip() for line in run.stdout.splitlines() if line.strip()]
    assert lines == _expected_lines()
