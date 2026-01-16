import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path
import tempfile

import pytest


def test_native_memoryview_build_and_run(tmp_path: Path) -> None:
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

    src = tmp_path / "memoryview_demo.py"
    src.write_text(
        "mv = memoryview(b'hello')\n"
        "print(len(mv))\n"
        "print(mv.tobytes())\n"
        "mv2 = memoryview(bytearray(b'abcd'))\n"
        "print(mv2.tobytes())\n"
        "print(mv2[1:3].tobytes())\n"
        "print(mv2[::2].tobytes())\n"
        "ba2 = bytearray([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])\n"
        "mv3 = memoryview(ba2).cast('B', shape=[3, 4])\n"
        "print(mv3.shape[0])\n"
        "print(mv3.shape[1])\n"
        "print(mv3.strides[0])\n"
        "print(mv3.strides[1])\n"
        "print(mv3[1, 2])\n"
        "print(mv3[-1, -1])\n"
        "mvh = memoryview(bytearray([0, 0, 0, 0])).cast('H')\n"
        "mvh[0] = 500\n"
        "print(mvh[0])\n"
        "mvc = memoryview(bytearray(b'abc')).cast('c')\n"
        "print(mvc[0])\n"
        "mvc[0] = b'z'\n"
        "print(mvc[0])\n"
    )

    output_root = Path(tempfile.gettempdir())
    output_binary = output_root / f"{src.stem}_molt"
    artifacts = [output_root / "output.o", output_root / "main_stub.c", output_binary]
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
        assert run.stdout.strip().splitlines() == [
            "5",
            "b'hello'",
            "b'abcd'",
            "b'bc'",
            "b'ac'",
            "3",
            "4",
            "4",
            "1",
            "6",
            "11",
            "500",
            "b'a'",
            "b'z'",
        ]
    finally:
        for path in artifacts:
            if not existed[path] and path.exists():
                path.unlink()
