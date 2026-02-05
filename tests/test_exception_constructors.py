import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_linked_runner import (
    build_wasm_linked,
    require_wasm_toolchain,
    run_wasm_linked,
)


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
    require_wasm_toolchain()

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "exception_keywords.py"
    _write_exception_program(src)

    output_wasm = build_wasm_linked(root, src, tmp_path)
    run = run_wasm_linked(root, output_wasm)
    assert run.returncode == 0, run.stderr
    assert run.stdout.strip() == "code 7\ntype KeywordError"
