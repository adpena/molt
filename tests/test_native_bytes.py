import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


def test_native_bytes_build_and_run(tmp_path: Path) -> None:
    root = Path(__file__).resolve().parents[1]
    runtime_lib = root / "target" / "release" / "libmolt_runtime.a"
    if not runtime_lib.exists():
        pytest.skip("molt-runtime release library not built")
    if shutil.which("clang") is None:
        pytest.skip("clang not available")
    if shutil.which("cargo") is None:
        pytest.skip("cargo not available")
    if sys.platform == "darwin" and shutil.which("lipo") is not None:
        info = subprocess.run(
            ["lipo", "-info", str(runtime_lib)],
            capture_output=True,
            text=True,
        )
        arch = platform.machine()
        if info.returncode == 0 and arch not in info.stdout:
            pytest.skip("runtime lib architecture mismatch")

    src = tmp_path / "bytes_demo.py"
    src.write_text("print(b'hi' + b'!')\nprint(len(b'hi!'))\nprint(b'hello'[1:4])\n")

    output_binary = root / "hello_molt"
    artifacts = [root / "output.o", root / "main_stub.c", output_binary]
    existed = {path: path.exists() for path in artifacts}

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")

    try:
        build = subprocess.run(
            [sys.executable, "-m", "molt.cli", "build", str(src)],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
        )
        assert build.returncode == 0, build.stderr

        run = subprocess.run(
            [str(output_binary)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert run.stdout.strip().splitlines() == ["b'hi!'", "3", "b'ell'"]
    finally:
        for path in artifacts:
            if not existed[path] and path.exists():
                path.unlink()
