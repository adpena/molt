"""Gate: the sccache compilation-cache wrapper is enabled loudly, never silently.

Regression guard for the R73.3 metabug — sccache was configured but its absence
degraded SILENTLY to cold, memory-saturating builds. `_ensure_sccache_wrapper`
must (a) wire RUSTC_WRAPPER when the pinned sccache is provisioned, (b) DEGRADE
LOUDLY (stderr warning naming the provisioning command) when it is not, (c) never
download it, and (d) respect an explicit opt-out / pre-set wrapper. If this test
regresses, the cache silently turned off again or Molt installed a tool unasked.
"""

from __future__ import annotations

from pathlib import Path
from tests.process_guard_common import install_module_os_view

import molt.dx as dx


def test_wires_rustc_wrapper_when_sccache_available(monkeypatch):
    monkeypatch.setattr(dx, "pinned_sccache", lambda env: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"
    # sccache silently skips incremental units — enabling it MUST force this off,
    # else the wrapper caches nothing (another silent degradation).
    assert env.get("CARGO_INCREMENTAL") == "0"


def test_a_missing_pinned_sccache_is_never_downloaded(monkeypatch, tmp_path):
    # Molt never installs a tool on its own: discovery finds nothing under an
    # empty toolchain root, and no network request is made.
    import urllib.request

    def _no_network(*args, **kwargs):
        raise AssertionError("sccache discovery must not download")

    monkeypatch.setattr(urllib.request, "urlopen", _no_network)
    env = {"MOLT_TARGET_ROOT": str(tmp_path / "target-root")}
    assert dx.pinned_sccache(env) is None
    assert not (tmp_path / "target-root").exists()


def test_no_toolchain_root_means_no_pinned_sccache():
    assert dx.pinned_sccache({}) is None


def test_degrades_loudly_when_unavailable(monkeypatch, capsys):
    monkeypatch.setattr(dx, "pinned_sccache", lambda env: None)
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}
    dx._ensure_sccache_wrapper(env)
    err = capsys.readouterr().err
    assert "not provisioned" in err and "cache is OFF" in err
    assert "python -m molt.tool_releases provision sccache" in err
    assert "RUSTC_WRAPPER" not in env  # must NOT fake a wrapper


def test_explicit_off_is_silent_noop(monkeypatch, capsys):
    monkeypatch.setattr(dx, "pinned_sccache", lambda env: None)
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "0"}
    dx._ensure_sccache_wrapper(env)
    err = capsys.readouterr().err
    assert "WARNING" not in err and "RUSTC_WRAPPER" not in env


def test_respects_preset_wrapper(monkeypatch):
    called = {"n": 0}

    def _boom(env):
        called["n"] += 1
        return None

    monkeypatch.setattr(dx, "pinned_sccache", _boom)
    env = {"MOLT_USE_SCCACHE": "1", "RUSTC_WRAPPER": "/custom/wrap"}
    dx._ensure_sccache_wrapper(env)
    assert env["RUSTC_WRAPPER"] == "/custom/wrap"
    assert called["n"] == 0  # short-circuits before discovery


def test_windows_auto_disables_sccache(monkeypatch, capsys):
    # On Windows sccache delivers 0 hits + crashes builds; "auto"/default must NOT
    # provision or wire it (negative-leverage cache), and must say so loudly.
    install_module_os_view(monkeypatch, dx, name="nt")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    tried = {"n": 0}
    monkeypatch.setattr(
        dx, "pinned_sccache", lambda env: tried.__setitem__("n", tried["n"] + 1)
    )
    env: dict[str, str] = {}  # mode defaults to "auto"
    dx._ensure_sccache_wrapper(env)
    assert "RUSTC_WRAPPER" not in env
    assert tried["n"] == 0  # must not even look for it
    assert "disabled by default on Windows" in capsys.readouterr().err


def test_windows_explicit_on_forces_sccache(monkeypatch):
    install_module_os_view(monkeypatch, dx, name="nt")
    monkeypatch.setattr(dx, "pinned_sccache", lambda env: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}  # power-user override
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"


def test_non_windows_auto_enables_sccache(monkeypatch):
    install_module_os_view(monkeypatch, dx, name="posix")
    monkeypatch.setattr(dx, "pinned_sccache", lambda env: "/opt/sccache")
    monkeypatch.setattr(dx, "_sccache_degrade_warned", False, raising=False)
    env: dict[str, str] = {}  # auto → on where sccache works
    dx._ensure_sccache_wrapper(env)
    assert env.get("RUSTC_WRAPPER") == "/opt/sccache"


def test_sccache_release_is_pinned_for_this_host():
    from molt import tool_releases

    release = tool_releases.tool_release("sccache")
    asset = tool_releases.host_asset(release)
    assert asset.url.startswith("https://github.com/mozilla/sccache/releases/download/")
    assert release.version in asset.url


def test_cargo_build_env_incremental_on_when_sccache_off(monkeypatch):
    # The warm-rebuild accelerator: with sccache OFF (Windows default), incremental
    # MUST be on — else every rebuild pays the full cold runtime compile (~15 min).
    import molt.cli.cargo_execution as ce

    monkeypatch.delenv("RUSTC_WRAPPER", raising=False)
    monkeypatch.delenv("CARGO_INCREMENTAL", raising=False)
    # "auto" enables a provisioned, responsive sccache; state "off" explicitly.
    monkeypatch.setenv("MOLT_USE_SCCACHE", "0")
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

    monkeypatch.setattr(ce, "pinned_sccache", lambda env: "/opt/sccache")
    monkeypatch.setattr(ce, "_sccache_server_responsive", lambda p: True)
    monkeypatch.setattr(ce, "_SCCACHE_DIAG_EMITTED", True, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}  # forced on
    ce._maybe_enable_sccache(env)
    assert env.get("RUSTC_WRAPPER", "").endswith("sccache")
    assert env["CARGO_INCREMENTAL"] == "0"


def test_lld_link_enabled_on_windows_when_available(monkeypatch):
    # Portable fast-linker: on Windows with LLVM lld-link on PATH, wire it as the
    # msvc-target linker (env var, not a rustflag) so cargo links with lld, not
    # the slow serial link.exe.
    import molt.cli.cargo_execution as ce

    install_module_os_view(monkeypatch, ce, name="nt")
    monkeypatch.setattr(
        ce,
        "llvm_linker_candidates",
        lambda role: (Path("C:/LLVM/bin/lld-link.exe"),) if role == "lld-link" else (),
    )
    env: dict[str, str] = {}
    ce._maybe_enable_lld_link(env)
    assert env["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"].endswith("lld-link.exe")


def test_lld_link_noop_when_absent(monkeypatch):
    # Portability: no lld-link -> keep link.exe (do NOT set a bogus linker).
    import molt.cli.cargo_execution as ce

    install_module_os_view(monkeypatch, ce, name="nt")
    monkeypatch.setattr(ce, "llvm_linker_candidates", lambda _role: ())
    env: dict[str, str] = {}
    ce._maybe_enable_lld_link(env)
    assert "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER" not in env


def test_lld_link_noop_non_windows(monkeypatch):
    import molt.cli.cargo_execution as ce

    install_module_os_view(monkeypatch, ce, name="posix")
    env: dict[str, str] = {}
    ce._maybe_enable_lld_link(env)
    assert "CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER" not in env


def test_lld_link_respects_explicit_override(monkeypatch):
    import molt.cli.cargo_execution as ce

    install_module_os_view(monkeypatch, ce, name="nt")
    env = {"CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER": "custom-linker"}
    ce._maybe_enable_lld_link(env)
    assert env["CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER"] == "custom-linker"
