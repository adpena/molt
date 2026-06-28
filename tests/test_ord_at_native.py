from __future__ import annotations

import os
import shutil
import sys
import textwrap
from pathlib import Path

import pytest

from molt.dx import development_artifact_env
from tests.native_process_guard import run_native_test_process
from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


PROGRAM = """
def show(label: str, fn) -> None:
    try:
        print(label, fn())
    except Exception as exc:
        print(label, type(exc).__name__, str(exc))

text = "AéZ"
show("ascii", lambda: ord(text[0]))
show("unicode", lambda: ord(text[1]))
show("bool-true", lambda: ord(text[True]))
show("bool-false", lambda: ord(text[False]))
show("negative", lambda: ord(text[-1]))
show("negative-unicode", lambda: ord(text[-2]))
show("slice", lambda: ord(text[0:1]))
show("bytes-fallback", lambda: ord(b"A"[0]))
show("float-index", lambda: ord(text[1.0]))
show("oob", lambda: ord(text[99]))
"""

LUAU_PROGRAM = """
text = "AéZ"
print("ascii", ord(text[0]))
print("unicode", ord(text[1]))
print("bool-true", ord(text[True]))
print("bool-false", ord(text[False]))
print("negative", ord(text[-1]))
print("negative-unicode", ord(text[-2]))
items = ["A", "é"]
print("list-fallback", ord(items[1]))
"""

EXPECTED_LINES = [
    "ascii 65",
    "unicode 233",
    "bool-true 233",
    "bool-false 65",
    "negative 90",
    "negative-unicode 233",
    "slice 65",
    "bytes-fallback TypeError ord() expected string of length 1, but int found",
    "float-index TypeError string indices must be integers, not 'float'",
    "oob IndexError string index out of range",
]

LUAU_EXPECTED = "\n".join(
    [
        "ascii 65",
        "unicode 233",
        "bool-true 233",
        "bool-false 65",
        "negative 90",
        "negative-unicode 233",
        "list-fallback 233",
    ]
)


def _env(root: Path) -> dict[str, str]:
    env = development_artifact_env(
        root,
        os.environ,
        session_prefix="ord-at-native",
        session_id=os.environ.get("MOLT_SESSION_ID") or "ord-at-native",
        create_dirs=True,
    )
    env["PYTHONPATH"] = str(root / "src")
    env.setdefault("CARGO_BUILD_JOBS", "1")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_BUILD_LOCK_TIMEOUT", "45")
    env.setdefault("MOLT_CARGO_TIMEOUT", "900")
    return env


def _write_program(tmp_path: Path, name: str, source: str = PROGRAM) -> Path:
    src = tmp_path / name
    src.write_text(textwrap.dedent(source), encoding="utf-8")
    return src


def _llvm_backend_available(root: Path) -> bool:
    from molt import cli as molt_cli

    major, toolchain = molt_cli._detect_llvm_backend_toolchain(root)
    return major is not None and toolchain is not None


def _assert_program_output(stdout: str) -> None:
    lines = [line.strip() for line in stdout.splitlines() if line.strip()]
    assert lines == EXPECTED_LINES


def test_ord_at_native_semantics(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for native ord_at test")

    root = Path(__file__).resolve().parents[1]
    src = _write_program(tmp_path, "ord_at_native.py")

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
    _assert_program_output(run.stdout)


def test_ord_at_wasm_semantics(tmp_path: Path) -> None:
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = _write_program(tmp_path, "ord_at_wasm.py")
    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    _assert_program_output(run.stdout)


def test_ord_at_llvm_semantics(tmp_path: Path) -> None:
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for LLVM ord_at test")

    root = Path(__file__).resolve().parents[1]
    if not _llvm_backend_available(root):
        pytest.skip("LLVM backend toolchain is unavailable")

    src = _write_program(tmp_path, "ord_at_llvm.py")
    binary_path = tmp_path / "ord_at_llvm_molt"
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            "--build-profile",
            "dev",
            "--backend",
            "llvm",
            str(src),
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr
    assert binary_path.exists(), f"expected binary at {binary_path}"

    run = run_native_test_process(
        [str(binary_path)],
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stderr
    _assert_program_output(run.stdout)


def test_ord_at_luau_semantics(tmp_path: Path) -> None:
    runner = shutil.which("luau") or shutil.which("lune")
    if runner is None:
        pytest.skip("luau or lune is required for Luau ord_at test")

    root = Path(__file__).resolve().parents[1]
    src = _write_program(tmp_path, "ord_at_luau.py", LUAU_PROGRAM)
    luau_path = tmp_path / "ord_at_luau.luau"
    build = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "luau",
            "--output",
            str(luau_path),
        ],
        cwd=root,
        env=_env(root),
        capture_output=True,
        text=True,
        timeout=900,
    )
    assert build.returncode == 0, build.stderr
    assert luau_path.exists(), f"expected Luau output at {luau_path}"

    run_cmd = (
        [runner, "run", str(luau_path)]
        if Path(runner).name == "lune"
        else [runner, str(luau_path)]
    )
    run = run_native_test_process(
        run_cmd,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == LUAU_EXPECTED
