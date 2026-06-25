from __future__ import annotations

import argparse
import importlib.util
import sys
from pathlib import Path

from tools import throughput_measurement


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPT_PATH = REPO_ROOT / "tools" / "throughput_matrix.py"


def _load_throughput_matrix():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_throughput_matrix", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _diff_args(*, child_rlimit_gb: float | None = None) -> argparse.Namespace:
    return argparse.Namespace(
        output_root=None,
        shared_target_dir=None,
        profiles=["dev"],
        wrappers=[0],
        python_version="3.12",
        diff_jobs=1,
        diff_timeout_sec=3.0,
        diff_child_rlimit_gb=child_rlimit_gb,
        diff_scripts=["tests/differential/basic/ellipsis_basic.py"],
    )


def test_diff_matrix_inherits_adaptive_child_rlimit_by_default(
    monkeypatch,
    tmp_path: Path,
) -> None:
    module = _load_throughput_matrix()
    assert module.CommandResult is throughput_measurement.CommandResult
    captured_envs: list[dict[str, str]] = []

    def fake_run_command(  # type: ignore[no-untyped-def]
        command,
        *,
        cwd,
        env,
        timeout_sec,
        progress_label=None,
        output_path=None,
    ):
        del command, cwd, timeout_sec, progress_label, output_path
        captured_envs.append(dict(env))
        return module.CommandResult(
            command=[],
            returncode=0,
            elapsed_sec=0.01,
            timed_out=False,
            stdout_tail="",
            stderr_tail="",
        )

    monkeypatch.delenv("MOLT_DIFF_RLIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_CHILD_RLIMIT_GB", raising=False)
    monkeypatch.setattr(module, "_run_command", fake_run_command)

    module._run_diff_matrix(_diff_args(), REPO_ROOT, tmp_path)

    assert len(captured_envs) == 1
    env = captured_envs[0]
    assert env["MOLT_DIFF_MEASURE_RSS"] == "1"
    assert "MOLT_DIFF_RLIMIT_GB" not in env
    assert "MOLT_DIFF_CHILD_RLIMIT_GB" not in env


def test_diff_matrix_explicit_child_rlimit_is_opt_in(
    monkeypatch,
    tmp_path: Path,
) -> None:
    module = _load_throughput_matrix()
    captured_envs: list[dict[str, str]] = []

    def fake_run_command(  # type: ignore[no-untyped-def]
        command,
        *,
        cwd,
        env,
        timeout_sec,
        progress_label=None,
        output_path=None,
    ):
        del command, cwd, timeout_sec, progress_label, output_path
        captured_envs.append(dict(env))
        return module.CommandResult(
            command=[],
            returncode=0,
            elapsed_sec=0.01,
            timed_out=False,
            stdout_tail="",
            stderr_tail="",
        )

    monkeypatch.delenv("MOLT_DIFF_RLIMIT_GB", raising=False)
    monkeypatch.delenv("MOLT_DIFF_CHILD_RLIMIT_GB", raising=False)
    monkeypatch.setattr(module, "_run_command", fake_run_command)

    module._run_diff_matrix(_diff_args(child_rlimit_gb=5.5), REPO_ROOT, tmp_path)

    assert len(captured_envs) == 1
    env = captured_envs[0]
    assert env["MOLT_DIFF_MEASURE_RSS"] == "1"
    assert "MOLT_DIFF_RLIMIT_GB" not in env
    assert env["MOLT_DIFF_CHILD_RLIMIT_GB"] == "5.5"
