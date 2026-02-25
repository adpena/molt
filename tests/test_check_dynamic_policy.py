from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_dynamic_policy.py"


def _load_module():
    spec = importlib.util.spec_from_file_location(
        "check_dynamic_policy_under_test", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_dynamic_policy_guard_passes_for_repo_state() -> None:
    module = _load_module()
    assert module.main() == 0


def _write_file(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def test_runpy_expected_failure_paths_must_exist(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    missing_path = "tests/differential/stdlib/runpy_missing_case.py"

    errors = module._check_runpy_policy_lanes((missing_path,))

    assert errors == [f"runpy expected-failure path does not exist: {missing_path}"]


def test_empty_runpy_expected_failure_lane_allowed_with_doc_note(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    _write_file(
        tmp_path / "docs/spec/STATUS.md",
        (
            "runpy dynamic-lane expected failures are currently empty because "
            "supported lanes moved to intrinsic support."
        ),
    )

    errors = module._check_runpy_policy_lanes(())

    assert errors == []


def test_empty_runpy_expected_failure_lane_requires_doc_note(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    _write_file(tmp_path / "docs/spec/STATUS.md", "runpy policy lanes are tracked.")
    _write_file(tmp_path / "ROADMAP.md", "policy update pending.")

    errors = module._check_runpy_policy_lanes(())

    assert len(errors) == 1
    assert "runpy policy lane governance missing" in errors[0]
