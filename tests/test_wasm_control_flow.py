import os
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


def test_wasm_control_flow_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "if_else.py"
    src.write_text("x = 1\nif x < 2:\n    print(1)\nelse:\n    print(2)\n")

    output_wasm = root / "output.wasm"
    existed = output_wasm.exists()

    runner = write_wasm_runner(tmp_path, "run_wasm_if.js")

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
        assert run.stdout.strip() == "1"
    finally:
        if not existed and output_wasm.exists():
            output_wasm.unlink()
