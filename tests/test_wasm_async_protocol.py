import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_async_protocol_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "async_protocol.py"
    src.write_text(
        "import asyncio\n"
        "\n"
        "class Counter:\n"
        "    def __init__(self, n):\n"
        "        self.i = 1\n"
        "        self.n = n\n"
        "    def __aiter__(self):\n"
        "        return self\n"
        "    async def __anext__(self):\n"
        "        if self.i > self.n:\n"
        "            raise StopAsyncIteration\n"
        "        val = self.i\n"
        "        self.i += 1\n"
        "        await asyncio.sleep(0)\n"
        "        return val\n"
        "\n"
        "async def main():\n"
        "    async for item in Counter(3):\n"
        "        print(item)\n"
        "    async for item in [20, 30]:\n"
        "        print(item)\n"
        "    it = aiter(Counter(1))\n"
        "    print(await anext(it))\n"
        "    try:\n"
        "        await anext(it)\n"
        "    except StopAsyncIteration:\n"
        "        print('done')\n"
        "    it2 = aiter(Counter(0))\n"
        "    print(await anext(it2, 7))\n"
        "\n"
        "asyncio.run(main())\n"
    )

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(tmp_path, "run_wasm_async_protocol.js")

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
            ["1", "2", "3", "20", "30", "1", "done", "7"]
        )
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
