from __future__ import annotations

from pathlib import Path
from types import SimpleNamespace

import tools.check_formal_methods as check_formal_methods


REPO_ROOT = Path(__file__).resolve().parents[1]


def test_check_inventory_returns_result() -> None:
    """Verify check_inventory runs without error (smoke test)."""
    result = check_formal_methods.check_inventory()
    assert isinstance(result, check_formal_methods.CheckResult)


def test_quint_manifest_covers_every_model_file() -> None:
    expected = {path.name for path in (REPO_ROOT / "formal" / "quint").glob("*.qnt")}
    actual = {model for model, _invariant, _steps in check_formal_methods.QUINT_MODELS}

    assert actual == expected


def test_quint_inventory_uses_blocking_gate_manifest() -> None:
    expected = [
        model for model, _invariant, _steps in check_formal_methods.QUINT_MODELS
    ]

    assert check_formal_methods.EXPECTED_QUINT_FILES == expected


def test_quint_manifest_entries_are_bounded_invariant_checks() -> None:
    seen: set[str] = set()
    for model, invariant, max_steps in check_formal_methods.QUINT_MODELS:
        assert model.endswith(".qnt")
        assert model not in seen
        assert invariant
        assert 0 < max_steps <= 20
        seen.add(model)


def test_quint_seed_matrix_is_deterministic_and_regression_bearing() -> None:
    seeds = check_formal_methods.QUINT_RUN_SEEDS

    assert len(seeds) == len(set(seeds))
    assert "0xefd9b00a0dfe6ba" in seeds
    assert "0x8ccc0ae2ed66b340" in seeds
    assert all(seed.startswith("0x") for seed in seeds)
    assert check_formal_methods.KNOWN_BAD_SEED.startswith("0x")


def test_known_bad_quint_uses_main_module_and_requires_violation(
    monkeypatch,
) -> None:
    calls: list[list[str]] = []

    def fake_guarded_completed_process(cmd, **_kwargs):
        calls.append(list(cmd))
        return SimpleNamespace(
            returncode=1,
            stdout="[violation] Found an issue\nerror: Invariant violated\n",
            stderr="",
        )

    monkeypatch.setattr(check_formal_methods.shutil, "which", lambda _name: "quint")
    monkeypatch.setattr(
        check_formal_methods.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = check_formal_methods.check_known_bad_model()

    assert result.passed
    assert len(calls) == 1
    cmd = calls[0]
    assert (
        str(check_formal_methods.QUINT_DIR / check_formal_methods.KNOWN_BAD_MODEL)
        in cmd
    )
    assert f"--main={check_formal_methods.KNOWN_BAD_MODULE}" in cmd
    assert not any("::" in part for part in cmd)


def test_known_bad_quint_rejects_infrastructure_failure(monkeypatch) -> None:
    def fake_guarded_completed_process(_cmd, **_kwargs):
        return SimpleNamespace(
            returncode=1,
            stdout="",
            stderr="TypeError: fetch failed\nNode.js v24.16.0\n",
        )

    monkeypatch.setattr(check_formal_methods.shutil, "which", lambda _name: "quint")
    monkeypatch.setattr(
        check_formal_methods.harness_memory_guard,
        "guarded_completed_process",
        fake_guarded_completed_process,
    )

    result = check_formal_methods.check_known_bad_model()

    assert not result.passed
    assert "INFRA-FAILURE" in result.detail
    assert "did not report an invariant violation" in result.detail
