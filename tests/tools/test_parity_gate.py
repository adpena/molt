from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
PARITY_GATE = REPO_ROOT / "tools" / "parity_gate.py"


def _load_parity_gate():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_parity_gate", PARITY_GATE
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_strict_molt_only_import_failure_fails() -> None:
    module = _load_parity_gate()

    result = module.compare(
        Path("case.py"),
        module.TIER_STRICT,
        "ok\n",
        "",
        0,
        "",
        "ModuleNotFoundError: No module named 'numpy'\n",
        1,
    )

    assert result.status == "fail"
    assert result.message == "molt-only import failure"


def test_relaxed_molt_only_import_failure_warns() -> None:
    module = _load_parity_gate()

    result = module.compare(
        Path("case.py"),
        module.TIER_RELAXED,
        "ok\n",
        "",
        0,
        "",
        "ImportError: cannot import name 'AxisError' from 'numpy.exceptions'\n",
        1,
    )

    assert result.status == "warn"
    assert result.message == "molt-only import failure"


def test_both_sides_same_import_failure_skips() -> None:
    module = _load_parity_gate()
    stderr = "ModuleNotFoundError: No module named 'not_installed_here'\n"

    result = module.compare(
        Path("case.py"),
        module.TIER_STRICT,
        "",
        stderr,
        1,
        "",
        stderr,
        1,
    )

    assert result.status == "skip"
    assert result.message == "both interpreters failed on the same import"


def test_both_sides_different_import_failure_fails() -> None:
    module = _load_parity_gate()

    result = module.compare(
        Path("case.py"),
        module.TIER_STRICT,
        "",
        "ModuleNotFoundError: No module named 'pandas'\n",
        1,
        "",
        "ModuleNotFoundError: No module named 'numpy'\n",
        1,
    )

    assert result.status == "fail"
    assert result.message == "molt-only import failure"


def test_run_one_runs_molt_before_import_skip(monkeypatch, tmp_path) -> None:
    module = _load_parity_gate()
    source = tmp_path / "case.py"
    source.write_text("import not_installed_here\n", encoding="utf-8")
    calls: list[str] = []
    stderr = "ModuleNotFoundError: No module named 'not_installed_here'\n"

    monkeypatch.setattr(
        module,
        "run_cpython",
        lambda path, *, timeout: ("", stderr, 1),
    )

    def fake_run_molt(path, molt_cmd, molt_env=None, *, timeout):
        calls.append("molt")
        return "", stderr, 1

    monkeypatch.setattr(module, "run_molt", fake_run_molt)

    result = module.run_one(source, ["molt"], timeout=1)

    assert calls == ["molt"]
    assert result.status == "skip"
    assert result.message == "both interpreters failed on the same import"
