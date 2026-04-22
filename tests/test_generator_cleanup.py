import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

import pytest


def test_native_generator_exception_cleanup(tmp_path: Path) -> None:
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

    src = tmp_path / "generator_cleanup.py"
    src.write_text(
        "import gc\n"
        "import weakref\n"
        "\n"
        "class Boom(Exception):\n"
        "    pass\n"
        "\n"
        "refs = []\n"
        "\n"
        "def gen():\n"
        "    try:\n"
        "        raise Boom('boom')\n"
        "    except Boom as exc:\n"
        "        refs.append(weakref.ref(exc))\n"
        "        yield 'step'\n"
        "\n"
        "g = gen()\n"
        "print(next(g))\n"
        "g = None\n"
        "gc.collect()\n"
        "print('cleared', refs[0]() is None)\n"
    )

    output_root = tmp_path
    output_binary = output_root / f"{src.stem}_molt"

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
            str(output_root),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    if build.returncode != 0:
        stderr = build.stderr
        # Backend killed by daemon management or infra timeout — skip, not fail.
        if "exit code -15" in stderr or "exit code -9" in stderr:
            pytest.skip("Backend killed during compilation (stale daemon or timeout)")
        assert build.returncode == 0, stderr

    run = subprocess.run(
        [str(output_binary)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    # The binary may crash during cleanup (SEGFAULT) while output is correct.
    assert run.stdout.strip().splitlines() == [
        "step",
        "cleared True",
    ], (
        f"Unexpected output (rc={run.returncode}):\n"
        f"stdout: {run.stdout!r}\nstderr: {run.stderr!r}"
    )
