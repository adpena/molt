import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_generator_protocol_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "generator_protocol.py"
    src.write_text(
        "def gen_send():\n"
        "    x = yield 1\n"
        "    yield x\n"
        "\n"
        "g = gen_send()\n"
        "print(g.send(None))\n"
        "print(g.send(10))\n"
        "try:\n"
        "    g.send(11)\n"
        "except StopIteration:\n"
        "    print(0)\n"
        "\n"
        "def gen_throw():\n"
        "    try:\n"
        "        yield 1\n"
        "    except ValueError:\n"
        "        print(7)\n"
        "        yield 2\n"
        "\n"
        "g2 = gen_throw()\n"
        "print(g2.send(None))\n"
        "print(g2.throw(ValueError('x')))\n"
        "try:\n"
        "    g2.send(None)\n"
        "except StopIteration:\n"
        "    print(3)\n"
        "\n"
        "def gen_close():\n"
        "    try:\n"
        "        yield 1\n"
        "    finally:\n"
        "        print(4)\n"
        "\n"
        "g3 = gen_close()\n"
        "print(g3.send(None))\n"
        "g3.close()\n"
    )

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(tmp_path, "run_wasm_generator_protocol.js")

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
        assert run.stdout.strip() == "\n".join(
            ["1", "10", "0", "1", "7", "2", "3", "1", "4"]
        )
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
