import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def _write_exception_program(path: Path) -> None:
    path.write_text(
        textwrap.dedent(
            """\
            class KeywordError(Exception):
                def __init__(self, *, code):
                    self.code = code


            def main():
                try:
                    raise KeywordError(code=7)
                except KeywordError as exc:
                    print("code", exc.code)
                    print("type", type(exc).__name__)


            main()
            """
        )
    )


def test_native_exception_constructor_keywords(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    runtime_lib = root / "target" / "release" / "libmolt_runtime.a"
    if not runtime_lib.exists():
        pytest.skip("molt-runtime release library not built")
    if shutil.which("clang") is None:
        pytest.skip("clang not available")
    if shutil.which("cargo") is None:
        pytest.skip("cargo not available")

    src = tmp_path / "exception_keywords.py"
    _write_exception_program(src)

    output_binary = tmp_path / f"{src.stem}_molt"

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")

    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        [str(output_binary)],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "code 7\ntype KeywordError"


def test_wasm_exception_constructor_keywords(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "exception_keywords.py"
    _write_exception_program(src)

    output_wasm = tmp_path / "output.wasm"
    runner = write_wasm_runner(tmp_path, "run_wasm_exception_keywords.js")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "code 7\ntype KeywordError"
