from __future__ import annotations

from pathlib import Path

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
