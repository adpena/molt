from __future__ import annotations

import contextlib
import os
import subprocess
from pathlib import Path

import molt.cli as cli


def test_runtime_cargo_features_native_vs_wasm(monkeypatch) -> None:
    cli._runtime_cargo_features_cached.cache_clear()
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "0")
    assert cli._runtime_cargo_features(None) == ()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    assert cli._runtime_cargo_features("aarch64-apple-darwin") == ("molt_tk_native",)
    assert cli._runtime_cargo_features("wasm32-wasip1") == ()


def test_runtime_cargo_features_is_cached(monkeypatch) -> None:
    cli._runtime_cargo_features_cached.cache_clear()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")

    first = cli._runtime_cargo_features(None)
    second = cli._runtime_cargo_features(None)

    info = cli._runtime_cargo_features_cached.cache_info()
    assert first == second == ("molt_tk_native",)
    assert info.hits >= 1
    assert info.currsize >= 1


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


def test_runtime_fingerprint_reuses_stored_hash_when_inputs_unchanged(
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
    assert baseline is not None

    calls = 0
    original = cli._hash_runtime_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(cli, "_hash_runtime_file", wrapped, raising=True)
    reused = cli._runtime_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        target_triple=None,
        rustflags="",
        runtime_features=(),
        stored_fingerprint=baseline,
    )
    assert reused == baseline
    assert calls == 0


def test_artifact_needs_rebuild_stats_artifact_once(tmp_path: Path, monkeypatch) -> None:
    artifact = tmp_path / "artifact.o"
    artifact.write_bytes(b"obj")
    original_stat = Path.stat
    calls = 0

    def wrapped_stat(self: Path) -> os.stat_result:
        nonlocal calls
        calls += 1
        return original_stat(self)

    monkeypatch.setattr(Path, "stat", wrapped_stat, raising=True)
    needs = cli._artifact_needs_rebuild(
        artifact,
        {"hash": "abc", "rustc": None, "inputs_digest": "x"},
        {"hash": "abc", "rustc": None, "inputs_digest": "x"},
    )

    assert needs is False
    assert calls == 1


def test_backend_fingerprint_reuses_stored_hash_when_inputs_unchanged(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "backend_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        cli, "_backend_source_paths", lambda _project_root, _features=(): [source], raising=True
    )
    monkeypatch.setattr(cli, "_rustc_version", lambda: "rustc-test", raising=True)

    baseline = cli._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
    )
    assert baseline is not None

    calls = 0
    original = cli._hash_runtime_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(cli, "_hash_runtime_file", wrapped, raising=True)
    reused = cli._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
        stored_fingerprint=baseline,
    )
    assert reused == baseline
    assert calls == 0


def test_rustc_version_is_cached(monkeypatch) -> None:
    cli._rustc_version.cache_clear()
    calls = 0

    def fake_run(*args, **kwargs) -> subprocess.CompletedProcess[str]:
        nonlocal calls
        del args, kwargs
        calls += 1
        return subprocess.CompletedProcess(
            ["rustc", "-Vv"],
            0,
            "release: 1.0.0\n",
            "",
        )

    monkeypatch.setattr(cli.subprocess, "run", fake_run, raising=True)
    first = cli._rustc_version()
    second = cli._rustc_version()
    assert first == "release: 1.0.0"
    assert second == first
    assert calls == 1
    cli._rustc_version.cache_clear()


def test_runtime_fingerprint_read_reuses_process_cache(
    tmp_path: Path, monkeypatch
) -> None:
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    cli._PERSISTED_JSON_OBJECT_CACHE.clear()
    cli._write_runtime_fingerprint(
        fingerprint_path,
        {"hash": "abc", "rustc": "rustc-test", "inputs_digest": "digest"},
    )

    first = cli._read_runtime_fingerprint(fingerprint_path)

    def fail_read_text(*args, **kwargs):  # type: ignore[no-untyped-def]
        raise AssertionError("unexpected runtime fingerprint reread")

    monkeypatch.setattr(Path, "read_text", fail_read_text)
    second = cli._read_runtime_fingerprint(fingerprint_path)

    assert first == second == {
        "version": 1,
        "hash": "abc",
        "rustc": "rustc-test",
        "inputs_digest": "digest",
    }
    assert first is second


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
        runtime_lib.write_bytes(b"runtime")
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
    features_str = seen_cmds[0][feature_index + 1]
    assert "molt_tk_native" in features_str.split(",")


def test_ensure_runtime_lib_does_not_probe_fingerprint_exists(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    seen_cmds: list[list[str]] = []
    original_exists = Path.exists

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
        "_read_runtime_fingerprint",
        lambda path: None if path == fingerprint_path else None,
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

    def guarded_exists(self: Path) -> bool:
        if self == fingerprint_path:
            raise AssertionError("unexpected fingerprint exists probe")
        return original_exists(self)

    monkeypatch.setattr(Path, "exists", guarded_exists, raising=True)

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
        runtime_lib.write_bytes(b"runtime")
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
        cargo_timeout=0.1,
    )
    assert seen_cmds
