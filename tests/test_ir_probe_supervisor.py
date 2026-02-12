from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "ir_probe_supervisor.py"


def _load_module():
    spec = importlib.util.spec_from_file_location("ir_probe_supervisor", SCRIPT_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_dry_run_prints_attempts_and_build_timeout_env(
    tmp_path: Path, capsys, monkeypatch
) -> None:
    module = _load_module()
    argv = [
        "ir_probe_supervisor.py",
        "--dry-run",
        "--python",
        "3.12",
        "--jobs",
        "2",
        "--retry-jobs",
        "1",
        "--diff-build-timeout",
        "900",
        "--run-root-base",
        str(tmp_path / "runs"),
        "--cache-root",
        str(tmp_path / "cache"),
    ]
    monkeypatch.setattr(sys, "argv", argv)

    rc = module.main()
    out = capsys.readouterr().out

    assert rc == 0
    assert "attempt 1:" in out
    assert "attempt 2:" in out
    assert "dry-run env: MOLT_DIFF_BUILD_TIMEOUT=900" in out
    assert "ir-probe-supervisor: dry-run complete" in out
