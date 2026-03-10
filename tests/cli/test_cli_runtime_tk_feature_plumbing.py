from __future__ import annotations

import contextlib
import subprocess
from pathlib import Path

import molt.cli as cli


def test_runtime_cargo_features_native_vs_wasm(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_HOSTED_EXTENSION", raising=False)
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "0")
    assert cli._runtime_cargo_features(None) == ()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    assert cli._runtime_cargo_features("aarch64-apple-darwin") == ("molt_tk_native",)
    assert cli._runtime_cargo_features("wasm32-wasip1") == ()
    monkeypatch.setenv("MOLT_RUNTIME_HOSTED_EXTENSION", "1")
    assert cli._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_hosted_extension",
    )
    assert cli._runtime_cargo_features("wasm32-wasip1") == ()


def test_runtime_fingerprint_changes_with_runtime_features(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        cli, "_runtime_source_paths", lambda _project_root: [source], raising=True
    )
    monkeypatch.setattr(cli, "_rustc_version", lambda: "rustc-test", raising=True)
    baseline = cli._runtime_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        target_triple=None,
        rustflags="",
        runtime_features=(),
    )
    tk_native = cli._runtime_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        target_triple=None,
        rustflags="",
        runtime_features=("molt_tk_native",),
    )
    assert baseline is not None
    assert tk_native is not None
    assert baseline["hash"] != tk_native["hash"]


def test_is_valid_static_archive_checks_ar_magic(tmp_path: Path) -> None:
    archive = tmp_path / "libmolt_runtime.a"
    archive.write_bytes(b"!<arch>\nmember")
    assert cli._is_valid_static_archive(archive)

    invalid = tmp_path / "invalid.a"
    invalid.write_bytes(b"runtime")
    assert not cli._is_valid_static_archive(invalid)


def test_cargo_target_root_uses_workspace_config_target_dir(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    (tmp_path / ".cargo").mkdir()
    configured_target_dir = tmp_path / "external-target"
    (tmp_path / ".cargo" / "config.toml").write_text(
        f"[build]\ntarget-dir = {str(configured_target_dir)!r}\n"
    )
    assert cli._cargo_target_root(tmp_path) == configured_target_dir


def test_cargo_target_root_env_override_wins_over_workspace_config(
    tmp_path: Path, monkeypatch
) -> None:
    (tmp_path / ".cargo").mkdir()
    (tmp_path / ".cargo" / "config.toml").write_text("[build]\ntarget-dir = 'cfg-target'\n")
    env_target_dir = tmp_path / "env-target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(env_target_dir))
    assert cli._cargo_target_root(tmp_path) == env_target_dir


def test_ensure_runtime_lib_passes_tk_feature_to_native_build(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    seen_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "new", "rustc": "rustc-test"},
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True, raising=True
    )
    monkeypatch.setattr(cli, "_maybe_enable_sccache", lambda _env: None, raising=True)
    monkeypatch.setattr(
        cli, "_write_runtime_fingerprint", lambda *args, **kwargs: None, raising=True
    )

    def fake_run_cargo(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        json_output: bool,
        label: str,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, timeout, json_output, label
        seen_cmds.append(list(cmd))
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        runtime_lib.write_bytes(b"!<arch>\nmember")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    assert cli._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        cargo_timeout=5.0,
    )
    assert seen_cmds
    assert "--features" in seen_cmds[0]
    feature_index = seen_cmds[0].index("--features")
    assert seen_cmds[0][feature_index + 1] == "molt_tk_native"


def test_ensure_runtime_lib_rebuilds_invalid_cached_archive(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"runtime")
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprint_path.write_text('{"hash":"same","rustc":"rustc-test"}')
    seen_cmds: list[list[str]] = []

    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "same", "rustc": "rustc-test"},
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(cli, "_maybe_enable_sccache", lambda _env: None, raising=True)
    monkeypatch.setattr(
        cli, "_write_runtime_fingerprint", lambda *args, **kwargs: None, raising=True
    )

    def fake_run_cargo(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        json_output: bool,
        label: str,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, env, timeout, json_output, label
        seen_cmds.append(list(cmd))
        runtime_lib.write_bytes(b"!<arch>\nmember")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    assert cli._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        cargo_timeout=5.0,
    )
    assert seen_cmds
    assert cli._is_valid_static_archive(runtime_lib)
