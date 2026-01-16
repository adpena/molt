import os
import shutil
import subprocess
import sys
from pathlib import Path
import tempfile

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_channel_async_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "channel_async.py"
    src.write_text(
        "import asyncio\n"
        "\n"
        "async def main():\n"
        "    chan = molt_chan_new(1)\n"
        "    molt_chan_send(chan, 41)\n"
        "    print(molt_chan_recv(chan))\n"
        "    molt_chan_send(chan, 1)\n"
        "    print(molt_chan_recv(chan))\n"
        "\n"
        "asyncio.run(main())\n"
    )

    output_wasm = Path(tempfile.gettempdir()) / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(tmp_path, "run_wasm_channel_async.js")

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
        assert run.stdout.strip() == "\n".join(["41", "1"])
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
