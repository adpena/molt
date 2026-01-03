from __future__ import annotations


import pytest

from molt import capabilities


def test_capability_missing(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "")
    with pytest.raises(PermissionError):
        capabilities.require("websocket:connect")


def test_capability_present(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "websocket:connect,fs.read")
    assert capabilities.has("websocket:connect")
    assert capabilities.has("fs.read")
    capabilities.require("websocket:connect")


def test_format_caps() -> None:
    formatted = capabilities.format_caps(["b", "a", "b"])
    assert formatted == "a,b"
