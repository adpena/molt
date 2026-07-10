"""V2 stable dep-cache default-on-for-iteration matrix (doctrine 74 law 1)."""

from __future__ import annotations

import pytest

import molt.cli.runtime_build as rb


def _clear(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_WASM_INCREMENTAL", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_BUILD_PROFILE", raising=False)


def test_default_off_without_iteration_context(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear(monkeypatch)
    # Acceptance / final-green: no iteration knob -> deterministic session dir.
    assert rb._runtime_wasm_incremental_enabled() is False


def test_default_on_in_iteration_context(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear(monkeypatch)
    # An explicit iteration profile enables the stable cross-session dep cache
    # with no second env var.
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "dev-fast")
    assert rb._runtime_wasm_incremental_enabled() is True


def test_explicit_on_wins(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear(monkeypatch)
    monkeypatch.setenv("MOLT_RUNTIME_WASM_INCREMENTAL", "1")
    assert rb._runtime_wasm_incremental_enabled() is True


def test_explicit_off_overrides_iteration_context(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    _clear(monkeypatch)
    # Even in an iteration context, an explicit opt-out is honoured (e.g. to
    # force a deterministic build).
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "dev-fast")
    monkeypatch.setenv("MOLT_RUNTIME_WASM_INCREMENTAL", "0")
    assert rb._runtime_wasm_incremental_enabled() is False


def test_invalid_profile_does_not_enable(monkeypatch: pytest.MonkeyPatch) -> None:
    _clear(monkeypatch)
    # A typo'd profile is ignored by the override resolver, so it must NOT
    # silently flip the acceptance default.
    monkeypatch.setenv("MOLT_RUNTIME_BUILD_PROFILE", "not a valid profile!!")
    assert rb._runtime_wasm_incremental_enabled() is False
