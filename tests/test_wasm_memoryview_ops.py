import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path
import tempfile

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_memoryview_ops_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "memoryview_ops.py"
    src.write_text(
        textwrap.dedent(
            """\
            b = b'one,two'
            mv = memoryview(b)
            print(len(mv))
            print(mv[1])
            data = mv.tobytes()
            print(data[0])
            print(data[-1])
            ba = bytearray(b'one,two')
            mv2 = memoryview(ba)
            print(len(mv2))
            print(mv2[1])
            data2 = mv2.tobytes()
            print(data2[0])
            print(data2[-1])
            ba2 = bytearray([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11])
            mv3 = memoryview(ba2).cast('B', shape=[3, 4])
            print(mv3.shape[0])
            print(mv3.shape[1])
            print(mv3.strides[0])
            print(mv3.strides[1])
            print(mv3[1, 2])
            print(mv3[-1, -1])
            ba0 = bytearray(b'a')
            mv0 = memoryview(ba0).cast('B', shape=[])
            print(mv0[()])
            mvc = memoryview(bytearray(b'abc')).cast('c')
            print(mvc[0][0])
            mvc[0] = b'z'
            print(mvc[0][0])
            """
        )
    )

    output_wasm = Path(tempfile.gettempdir()) / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(tmp_path, "run_wasm_memoryview_ops.js")

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [sys.executable, "-m", "molt.cli", "build", str(src), "--target", "wasm"],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    try:
        run = subprocess.run(
            ["node", str(runner), str(output_wasm)],
            cwd=root,
            capture_output=True,
            text=True,
        )
        assert run.returncode == 0, run.stderr
        assert (
            run.stdout.strip()
            == "7\n110\n111\n111\n7\n110\n111\n111\n3\n4\n4\n1\n6\n11\n97\n97\n122"
        )
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
