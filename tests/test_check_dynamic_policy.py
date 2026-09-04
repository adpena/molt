from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import SimpleNamespace

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


def test_scope_discovery_uses_canonical_test_policy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    expected = frozenset(
        {
            "tests/differential/basic/exec_case.py",
            "tests/differential/basic/eval_case.py",
        }
    )
    observed: dict[str, object] = {}
    selectors = (
        ("tests/differential/policy-a", False),
        ("tests/differential/policy-b", True),
    )

    def verification_scope_paths(targets, *, scope, repo_root):
        observed.update(targets=targets, scope=scope, repo_root=repo_root)
        return expected

    monkeypatch.setattr(
        module.test_policy, "verification_scope_paths", verification_scope_paths
    )
    monkeypatch.setattr(
        module,
        "load_verified_subset_policy",
        lambda: SimpleNamespace(suite_selectors=selectors),
    )

    assert module._load_scope_paths() == tuple(sorted(expected))
    assert observed == {
        "targets": selectors,
        "scope": "dynamic_execution_policy",
        "repo_root": tmp_path,
    }


def test_scope_requires_expected_failure_metadata(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    exec_path = "tests/differential/basic/exec_case.py"
    eval_path = "tests/differential/basic/eval_case.py"
    _write_file(
        tmp_path / exec_path,
        "# MOLT_META: verified_subset_scope=dynamic_execution_policy "
        "expect_fail=molt expect_fail_reason=wrong_reason\n",
    )
    _write_file(
        tmp_path / eval_path,
        "# MOLT_META: verified_subset_scope=dynamic_execution_policy "
        "expect_fail=molt expect_fail_reason=wrong_reason\n",
    )

    assert module._check_scope((exec_path, eval_path)) == [
        "dynamic_execution_policy test must declare "
        f"expect_fail_reason=too_dynamic_policy: {exec_path}",
        "dynamic_execution_policy test must declare "
        f"expect_fail_reason=too_dynamic_policy: {eval_path}",
    ]


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


def test_runtime_policy_guard_requires_executable_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    module = _load_module()
    monkeypatch.setattr(module, "ROOT", tmp_path)
    for relative, snippets in module.RUNTIME_POLICY_EVIDENCE.items():
        _write_file(tmp_path / relative, "\n".join(snippets))

    assert module._check_runtime_policy_evidence() == []

    relative, snippets = next(iter(module.RUNTIME_POLICY_EVIDENCE.items()))
    _write_file(tmp_path / relative, "\n".join(snippets[1:]))

    errors = module._check_runtime_policy_evidence()
    assert errors == [
        f"runtime policy evidence missing snippet {snippets[0]!r}: {relative}"
    ]
