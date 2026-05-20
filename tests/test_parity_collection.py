from __future__ import annotations

import os
import sys
from pathlib import Path

from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[1]


def test_parity_scripts_are_not_collected_as_pytest_modules() -> None:
    env = os.environ.copy()
    pythonpath = str(ROOT / "src")
    if env.get("PYTHONPATH"):
        pythonpath = f"{pythonpath}{os.pathsep}{env['PYTHONPATH']}"
    env["PYTHONPATH"] = pythonpath

    result = run_cli_test_process(
        [
            sys.executable,
            "-m",
            "pytest",
            "-q",
            "--collect-only",
            "tests/parity",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )

    assert result.returncode in {0, 5}, result.stdout + result.stderr
    assert "ERROR collecting" not in result.stdout + result.stderr
    assert "no tests collected" in result.stdout + result.stderr
