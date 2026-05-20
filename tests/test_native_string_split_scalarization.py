from __future__ import annotations

import os
import sys
import textwrap
from pathlib import Path

from tests.native_process_guard import run_native_test_process


ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"


def test_native_string_split_constant_field_scalarization_runs(tmp_path: Path) -> None:
    src = tmp_path / "string_split_scalarization.py"
    src.write_text(
        textwrap.dedent(
            """\
            def main() -> None:
                row = "alpha|beta|gamma"
                fields = row.split("|")
                print(fields[0])
                print(fields[2])
                print("x--y--z".split("--")[1])
                repeated = "a-b|c-d".split("|")
                print(repeated[0] is repeated[0])

            main()
            """
        )
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_SESSION_ID", "test-native-string-split-scalarization")
    env.setdefault("CARGO_BUILD_JOBS", "1")

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            str(src),
            "--profile",
            "dev",
            "--rebuild",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
    )

    assert run.returncode == 0, run.stderr
    assert run.stdout.strip().splitlines() == ["alpha", "gamma", "y", "True"]


def test_native_string_split_scalarization_preserves_split_point_exceptions(
    tmp_path: Path,
) -> None:
    src = tmp_path / "string_split_scalarization_exceptions.py"
    src.write_text(
        textwrap.dedent(
            """\
            def main() -> None:
                parts = "alpha|beta".split("")
                print("after split")
                print(parts[0])

            main()
            """
        )
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_SESSION_ID", "test-native-string-split-scalarization")
    env.setdefault("CARGO_BUILD_JOBS", "1")

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            str(src),
            "--profile",
            "dev",
            "--rebuild",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
    )

    assert run.returncode != 0
    assert "after split" not in run.stdout
    assert "ValueError" in run.stderr
    assert "empty separator" in run.stderr


def test_native_string_split_scalarization_preserves_field_index_error(
    tmp_path: Path,
) -> None:
    src = tmp_path / "string_split_scalarization_index_error.py"
    src.write_text(
        textwrap.dedent(
            """\
            def main() -> None:
                parts = "alpha|beta".split("|")
                print(parts[2])

            main()
            """
        )
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_SESSION_ID", "test-native-string-split-scalarization")
    env.setdefault("CARGO_BUILD_JOBS", "1")

    run = run_native_test_process(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "run",
            str(src),
            "--profile",
            "dev",
            "--rebuild",
        ],
        cwd=ROOT,
        env=env,
        capture_output=True,
        text=True,
        timeout=180,
    )

    assert run.returncode != 0
    assert "IndexError" in run.stderr
    assert "list index out of range" in run.stderr
