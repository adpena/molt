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
    # sccache silently skips incremental units — enabling it MUST force this off,
    # else the wrapper caches nothing (another silent degradation).
    assert env.get("CARGO_INCREMENTAL") == "0"


def test_failed_provision_attempts_download_at_most_once(monkeypatch, tmp_path):
    # Guard against re-hanging every build's env setup: a failed network download
    # must be memoized per process, not retried on each _install_dx_defaults call.
    monkeypatch.setattr(dx.shutil, "which", lambda name: None)
    monkeypatch.setattr(dx.Path, "home", staticmethod(lambda: tmp_path))
    monkeypatch.setattr(dx, "_sccache_download_failed", False, raising=False)
    calls = {"n": 0}

    def _boom(*a, **k):
        calls["n"] += 1
        raise OSError("offline")

    import urllib.request

    monkeypatch.setattr(urllib.request, "urlopen", _boom)
    results = [dx._provision_sccache() for _ in range(4)]
    assert all(r is None for r in results)
    assert calls["n"] == 1  # attempted exactly once, then short-circuits


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


def test_windows_auto_disables_sccache(monkeypatch, capsys):
    # On Windows sccache delivers 0 hits + crashes builds; "auto"/default must NOT
    # provision or wire it (negative-leverage cache), and must say so loudly.
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    tried = {"n": 0}
    monkeypatch.setattr(dx, "_provision_sccache", lambda: tried.__setitem__("n", tried["n"] + 1))
    env: dict[str, str] = {}  # mode defaults to "auto"
    dx._ensure_sccache_wrapper(env)
    assert "RUSTC_WRAPPER" not in env
    assert tried["n"] == 0  # must not even attempt provisioning
    assert "disabled by default on Windows" in capsys.readouterr().err


def test_windows_explicit_on_forces_sccache(monkeypatch):
    monkeypatch.setattr(dx.os, "name", "nt")
    monkeypatch.setattr(dx, "_provision_sccache", lambda: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}  # power-user override
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"


def test_non_windows_auto_enables_sccache(monkeypatch):
    monkeypatch.setattr(dx.os, "name", "posix")
    monkeypatch.setattr(dx, "_provision_sccache", lambda: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env: dict[str, str] = {}  # auto → on where sccache works
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"


def test_pinned_asset_url_is_wellformed():
    url = dx._sccache_asset_url()
    assert url is None or (
        url.startswith("https://github.com/mozilla/sccache/releases/download/")
        and dx._SCCACHE_VERSION in url
    )


def test_cargo_build_env_incremental_on_when_sccache_off(monkeypatch):
    # The warm-rebuild accelerator: with sccache OFF (Windows default), incremental
    # MUST be on — else every rebuild pays the full cold runtime compile (~15 min).
    import molt.cli.cargo_execution as ce

    monkeypatch.delenv("RUSTC_WRAPPER", raising=False)
    monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    env = ce._cargo_build_env()
    assert env["CARGO_INCREMENTAL"] == "1"


def test_cargo_build_env_incremental_off_when_sccache_wrapper(monkeypatch):
    import molt.cli.cargo_execution as ce

    monkeypatch.setenv("RUSTC_WRAPPER", "/opt/sccache")
    monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    env = ce._cargo_build_env()
    assert env["CARGO_INCREMENTAL"] == "0"  # sccache skips incremental units


def test_maybe_enable_sccache_forces_incremental_off(monkeypatch):
    import molt.cli.cargo_execution as ce

    monkeypatch.setattr(ce.shutil, "which", lambda name: "/opt/sccache")
    monkeypatch.setattr(ce, "_sccache_server_responsive", lambda p: True)
    monkeypatch.setattr(ce, "_SCCACHE_DIAG_EMITTED", True, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}  # forced on
    ce._maybe_enable_sccache(env)
    assert env.get("RUSTC_WRAPPER", "").endswith("sccache")
    assert env["CARGO_INCREMENTAL"] == "0"
