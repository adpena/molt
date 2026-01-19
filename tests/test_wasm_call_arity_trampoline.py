import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner


CALL_SRC = textwrap.dedent(
    """\
    def add13(a1, a2, a3, a4, a5, a6, a7, a8, a9, a10, a11, a12, a13):
        return (
            a1
            + a2
            + a3
            + a4
            + a5
            + a6
            + a7
            + a8
            + a9
            + a10
            + a11
            + a12
            + a13
        )

    def main():
        args = (1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13)
        print(add13(1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13))
        fn = add13
        print(fn(*args))

    if __name__ == "__main__":
        main()
    """
)


def test_wasm_call_arity_trampoline(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "call_arity_trampoline.py"
    src.write_text(CALL_SRC)

    output_wasm = tmp_path / "output.wasm"

    runner = write_wasm_runner(tmp_path, "run_wasm_call_arity.js")

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
            "--codec",
            "json",
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
    assert run.stdout.strip() == "91\n91"
