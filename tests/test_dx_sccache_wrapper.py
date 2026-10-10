"""Gate: the sccache compilation-cache wrapper is enabled loudly, never silently.

Regression guard for the R73.3 metabug — sccache was configured but its absence
degraded SILENTLY to cold, memory-saturating builds. `_ensure_sccache_wrapper`
must (a) wire RUSTC_WRAPPER when the pinned sccache is provisioned, (b) DEGRADE
LOUDLY (stderr warning naming the provisioning command) when it is not, (c) never
download it, and (d) respect an explicit opt-out / pre-set wrapper. If this test
regresses, the cache silently turned off again or Molt installed a tool unasked.
"""

from __future__ import annotations

import os
from pathlib import Path
import subprocess

import pytest

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


def test_default_tool_root_without_sccache_means_no_pinned_sccache(
    tmp_path, monkeypatch
):
    # With no explicit root, discovery reads only the selected default root.
    empty = tmp_path / "target-root"
    custody = dx.CheckoutCustody(
        source_root=tmp_path,
        custody_root=tmp_path,
        toolchain_root=empty,
        kind="durable",
    )
    monkeypatch.setattr(dx, "checkout_custody", lambda *_a, **_k: custody)
    assert dx.pinned_sccache({}) is None
    assert not empty.exists()


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
    monkeypatch.setattr(ce, "_sccache_server_responsive", lambda _sccache, _env: True)
    monkeypatch.setattr(ce, "_SCCACHE_DIAG_EMITTED", True, raising=False)
    env = {"MOLT_USE_SCCACHE": "1"}  # forced on
    ce._maybe_enable_sccache(env)
    assert env.get("RUSTC_WRAPPER", "").endswith("sccache")
    assert env["CARGO_INCREMENTAL"] == "0"


# HF-105: the shared sccache server takes its temporary directory from the
# environment of the client that starts it and outlives that client. A build
# whose TMPDIR is one run's scratch (tests/molt_diff.py, guard scratch) must
# still start it with the durable directory beside the cache.


def _scratch_cargo_env(tmp_path: Path, **extra: str) -> dict[str, str]:
    scratch = tmp_path / "artifacts" / "tmp" / "molt_diff_run" / "tmp"
    scratch.mkdir(parents=True)
    return {
        "SCCACHE_DIR": str(tmp_path / "artifacts" / ".sccache"),
        "TMPDIR": str(scratch),
        "TMP": str(scratch),
        "TEMP": str(scratch),
        **extra,
    }


@pytest.mark.skipif(os.name == "nt", reason="POSIX shell fakes stand in for tools")
def test_sccache_client_under_cargo_gets_the_durable_server_temp_dir(tmp_path):
    from molt import process_guard

    bin_dir = tmp_path / "bin"
    bin_dir.mkdir()
    record = tmp_path / "client-temp.txt"
    # Cargo runs the compiler wrapper with Cargo's own environment; an sccache
    # client with no server running starts one that inherits that environment.
    for name, body in (
        ("cargo", 'exec "$RUSTC_WRAPPER" rustc -vV'),
        (
            "sccache",
            '[ -d "$TMPDIR" ] || exit 3\n'
            'printf "%s\\n%s\\n%s\\n" "$TMPDIR" "$TMP" "$TEMP" > "$CLIENT_RECORD"',
        ),
    ):
        script = bin_dir / name
        script.write_text(f"#!/bin/sh\n{body}\n", encoding="utf-8")
        script.chmod(0o755)
    env = _scratch_cargo_env(
        tmp_path,
        PATH=f"{bin_dir}{os.pathsep}/usr/bin{os.pathsep}/bin",
        RUSTC_WRAPPER=str(bin_dir / "sccache"),
        CLIENT_RECORD=str(record),
    )

    result = process_guard.run_completed_command(
        [str(bin_dir / "cargo"), "build"],
        env=env,
        memory_guard_prefix=None,
        capture_output=True,
        text=True,
        timeout=30,
    )

    assert result.returncode == 0, result.stderr
    server_temp = tmp_path / "artifacts" / ".sccache-tmp"
    assert record.read_text(encoding="utf-8").splitlines() == [str(server_temp)] * 3
    assert server_temp.is_dir()


def test_cli_sccache_probe_starts_a_server_with_the_durable_temp_dir(
    tmp_path, monkeypatch
):
    import molt.cli.cargo_execution as ce

    probes: list[dict[str, str]] = []

    def run(command, **kwargs):
        assert command == ["/opt/sccache", "--show-stats"]
        probes.append(dict(kwargs["env"]))
        return subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(ce, "pinned_sccache", lambda env: "/opt/sccache")
    monkeypatch.setattr(ce, "_run_completed_command", run)
    monkeypatch.setattr(ce, "_SCCACHE_DIAG_EMITTED", True, raising=False)
    env = _scratch_cargo_env(
        tmp_path,
        MOLT_USE_SCCACHE="1",
        MOLT_EXT_ROOT=str(tmp_path / "artifacts"),
    )
    del env["SCCACHE_DIR"]  # the CLI selects the cache beside its artifacts

    ce._maybe_enable_sccache(env)

    server_temp = str((tmp_path / "artifacts").resolve() / ".sccache-tmp")
    assert [
        {name: probe[name] for name in ("TMPDIR", "TMP", "TEMP")} for probe in probes
    ] == [{"TMPDIR": server_temp, "TMP": server_temp, "TEMP": server_temp}]
    assert Path(server_temp).is_dir()
    assert env["RUSTC_WRAPPER"] == "/opt/sccache"
    assert env["TMPDIR"] == server_temp


def test_cargo_environment_pins_only_a_configured_sccache_cache(tmp_path):
    from molt.cargo_execution_policy import (
        SCCACHE_INCREMENTAL_POLICY,
        SCCACHE_SERVER_TEMP_POLICY,
        normalize_cargo_environment,
    )

    scratch_env = _scratch_cargo_env(tmp_path, RUSTC_WRAPPER="/opt/sccache")
    scratch = scratch_env["TMPDIR"]
    server_temp = tmp_path / "artifacts" / ".sccache-tmp"

    pinned, applied = normalize_cargo_environment(scratch_env)
    assert applied == (SCCACHE_INCREMENTAL_POLICY, SCCACHE_SERVER_TEMP_POLICY)
    assert {pinned[name] for name in ("TMPDIR", "TMP", "TEMP")} == {str(server_temp)}
    assert scratch_env["TMPDIR"] == scratch  # the caller's mapping is unchanged
    # Every process given this environment can use its temp dir: the runtime
    # rustc probe runs sccache without passing a Cargo launch boundary.
    assert server_temp.is_dir()

    unconfigured = {**scratch_env}
    del unconfigured["SCCACHE_DIR"]
    kept, applied = normalize_cargo_environment(unconfigured)
    assert applied == (SCCACHE_INCREMENTAL_POLICY,)
    assert kept["TMPDIR"] == scratch

    direct = {**scratch_env}
    del direct["RUSTC_WRAPPER"]
    kept, applied = normalize_cargo_environment(direct)
    assert applied == ()
    assert kept["TMPDIR"] == scratch


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


def test_dx_sccache_provisioner_receives_the_selected_tool_root(tmp_path, monkeypatch):
    project = tmp_path / "project"
    project.mkdir()
    tools = tmp_path / "tools"
    artifacts = tmp_path / "artifacts"
    seen = []
    monkeypatch.setattr(
        dx,
        "pinned_sccache",
        lambda env: seen.append(Path(env["MOLT_TARGET_ROOT"])) or "/selected/sccache",
    )
    env = dx.RunContext(project).dx_env(
        {
            "MOLT_TARGET_ROOT": str(tools),
            "MOLT_EXT_ROOT": str(artifacts),
            "MOLT_USE_SCCACHE": "1",
        },
        create_dirs=False,
    )
    assert seen == [tools]
    assert env["MOLT_TARGET_ROOT"] == str(tools)
    assert env["RUSTC_WRAPPER"] == "/selected/sccache"
    assert not tools.exists() and not artifacts.exists()


def test_direct_cargo_sccache_uses_selected_environment_and_installed_home(
    tmp_path, monkeypatch
):
    from tests.test_tool_releases import _installed_demo
    from molt import tool_releases
    from molt.cli import cargo_execution

    discovery = _installed_demo(tmp_path, monkeypatch)
    source = tmp_path / "bundle/source"
    source.mkdir(parents=True)
    (source / "release-compiler-source.json").write_text("layout only")
    monkeypatch.setattr(tool_releases, "compiler_source_root", lambda: source)
    # Reuse the actual attested fixture generation at the generic release owner.
    monkeypatch.setattr(
        tool_releases,
        "load_tool_releases",
        lambda _root: {"sccache": discovery.release},
    )
    monkeypatch.setenv("MOLT_TARGET_ROOT", str(tmp_path / "wrong-ambient-root"))
    assert cargo_execution.pinned_sccache({"MOLT_HOME": str(tmp_path)}) == str(
        discovery.executable
    )
    assert (
        cargo_execution.pinned_sccache({"MOLT_TARGET_ROOT": str(tmp_path / "missing")})
        is None
    )
    invalid = tmp_path / "invalid"
    invalid.write_text("not a directory")
    import pytest

    with pytest.raises(tool_releases.ToolReleaseError, match="not a directory"):
        cargo_execution.pinned_sccache({"MOLT_TARGET_ROOT": str(invalid)})
