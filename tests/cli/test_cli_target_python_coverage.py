from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from molt.target_python import TargetPythonVersion, require_verified_subset_target


def test_python_interpreter_import_stays_outside_cli_package() -> None:
    repo_root = Path(__file__).resolve().parents[2]
    assert (repo_root / "src" / "molt" / "target_python.py").is_file()
    assert not (repo_root / "src" / "molt" / "cli" / "target_python.py").exists()

    env = os.environ.copy()
    env["PYTHONPATH"] = str(repo_root / "src")
    result = subprocess.run(
        [
            sys.executable,
            "-c",
            (
                "import sys\n"
                "import molt.python_interpreter\n"
                "assert 'molt.cli' not in sys.modules\n"
                "from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION\n"
                "print(_DEFAULT_TARGET_PYTHON_VERSION.tag)\n"
            ),
        ],
        cwd=repo_root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "py312\n"


def test_verified_windows_312_tuple_resolves() -> None:
    target = TargetPythonVersion(3, 12, 0)
    assert require_verified_subset_target(target, platform="windows") is target


def test_tuple_outside_required_matrix_fails_with_matrix_diagnostic() -> None:
    with pytest.raises(
        ValueError,
        match=r"CPython 3\.13 on freebsd.*CPython 3\.12 on windows",
    ):
        require_verified_subset_target(
            TargetPythonVersion(3, 13, 0), platform="freebsd"
        )
