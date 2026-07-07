"""Gate: the sccache compilation-cache wrapper is enabled loudly, never silently.

Regression guard for the R73.3 metabug — sccache was configured but its absence
degraded SILENTLY to cold, memory-saturating builds. `_ensure_sccache_wrapper`
must (a) wire RUSTC_WRAPPER when sccache is available, (b) DEGRADE LOUDLY (stderr
warning) when it cannot be provisioned, and (c) respect an explicit opt-out /
pre-set wrapper. If this test regresses, the cache silently turned off again.
"""

from __future__ import annotations

import molt.dx as dx


def test_wires_rustc_wrapper_when_sccache_available(monkeypatch):
    monkeypatch.setattr(dx, "_provision_sccache", lambda: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"


def test_degrades_loudly_when_unavailable(monkeypatch, capsys):
    monkeypatch.setattr(dx, "_provision_sccache", lambda: None)
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}
    dx._ensure_sccache_wrapper(env)
    err = capsys.readouterr().err
    assert "sccache unavailable" in err and "cache is OFF" in err
    assert "RUSTC_WRAPPER" not in env  # must NOT fake a wrapper


def test_explicit_off_is_silent_noop(monkeypatch, capsys):
    monkeypatch.setattr(dx, "_provision_sccache", lambda: None)
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "0"}
    dx._ensure_sccache_wrapper(env)
    err = capsys.readouterr().err
    assert "WARNING" not in err and "RUSTC_WRAPPER" not in env


def test_respects_preset_wrapper(monkeypatch):
    called = {"n": 0}

    def _boom():
        called["n"] += 1
        return None

    monkeypatch.setattr(dx, "_provision_sccache", _boom)
    env = {"MOLT_USE_SCCACHE": "1", "RUSTC_WRAPPER": "/custom/wrap"}
    dx._ensure_sccache_wrapper(env)
    assert env["RUSTC_WRAPPER"] == "/custom/wrap"
    assert called["n"] == 0  # short-circuits before provisioning


def test_pinned_asset_url_is_wellformed():
    url = dx._sccache_asset_url()
    assert url is None or (
        url.startswith("https://github.com/mozilla/sccache/releases/download/")
        and dx._SCCACHE_VERSION in url
    )
