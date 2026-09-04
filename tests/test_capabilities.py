from __future__ import annotations

import pytest

from molt import capabilities


def test_capability_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "")
    with pytest.raises(PermissionError):
        capabilities.require("websocket.connect")


def test_capability_present(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "websocket.connect,fs.read")
    assert capabilities.has("websocket.connect")
    assert capabilities.has("fs.read")
    capabilities.require("websocket.connect")


def test_format_caps() -> None:
    formatted = capabilities.format_caps(["b", "a", "b"])
    assert formatted == "a,b"


def test_legacy_trusted_environment_does_not_grant_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "")
    monkeypatch.setenv("MOLT_TRUSTED", "1")
    monkeypatch.setenv("MOLT_CAPABILITY_TIER", "none")
    assert not capabilities.trusted()
    assert not capabilities.has("fs.read")
    with pytest.raises(PermissionError):
        capabilities.require("fs.read")


def test_maximum_builtin_tier_is_finite_and_does_not_short_circuit_intrinsics(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_CAPABILITY_TIER", "full")
    assert capabilities.trusted()
    assert capabilities.has("fs.read")
    assert not capabilities.has("future.unregistered")

    with pytest.raises(PermissionError):
        capabilities.require("future.unregistered")
