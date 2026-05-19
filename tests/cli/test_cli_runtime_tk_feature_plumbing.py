from __future__ import annotations

import contextlib
import os
import subprocess
from pathlib import Path

import molt.cli as cli


def test_runtime_cargo_features_native_vs_wasm(monkeypatch) -> None:
    cli._runtime_cargo_features_cached.cache_clear()
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_WEBGPU", raising=False)
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "0")
    cli._runtime_cargo_features_cached.cache_clear()
    assert cli._runtime_cargo_features(None) == ()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    cli._runtime_cargo_features_cached.cache_clear()
    assert cli._runtime_cargo_features(None) == ("molt_tk_native",)
    assert cli._runtime_cargo_features("aarch64-apple-darwin") == ("molt_tk_native",)
    assert cli._runtime_cargo_features("wasm32-wasip1") == ("molt_gpu_primitives",)


def test_runtime_cargo_features_include_gpu_backend_flags(monkeypatch) -> None:
    cli._runtime_cargo_features_cached.cache_clear()
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    monkeypatch.delenv("MOLT_RUNTIME_GPU_WEBGPU", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_CUDA", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_HIP", raising=False)
    assert cli._runtime_cargo_features(None) == ("molt_tk_native", "molt_gpu_metal")

    monkeypatch.setenv("MOLT_RUNTIME_GPU_WEBGPU", "1")
    cli._runtime_cargo_features_cached.cache_clear()
    assert cli._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_gpu_metal",
        "molt_gpu_webgpu",
    )

    monkeypatch.setenv("MOLT_RUNTIME_GPU_CUDA", "1")
    monkeypatch.setenv("MOLT_RUNTIME_GPU_HIP", "1")
    cli._runtime_cargo_features_cached.cache_clear()
    assert cli._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_gpu_metal",
        "molt_gpu_webgpu",
        "molt_gpu_cuda",
        "molt_gpu_hip",
    )
    assert cli._runtime_cargo_features("wasm32-wasip1") == ("molt_gpu_primitives",)


def test_builtin_features_from_import_graph_uses_native_micro_surface() -> None:
    json_features = cli._builtin_features_from_import_graph({"json"}, "micro")
    tkinter_features = cli._builtin_features_from_import_graph(
        {"tkinter.constants", "tkinter._support"},
        "micro",
    )
    tinygrad_features = cli._builtin_features_from_import_graph(
        {"tinygrad.tensor", "molt.stdlib.tinygrad.examples.falcon_ocr"},
        "micro",
    )

    assert json_features == tkinter_features == tinygrad_features
    assert set(json_features) == set(cli._ALL_BUILTIN_FEATURES)
    assert "stdlib_tk" not in json_features
    assert "stdlib_net" not in json_features
    assert "stdlib_serial" not in json_features
    assert "molt_gpu_primitives" not in json_features


def test_runtime_builtin_features_exclude_native_only_wasm_domains() -> None:
    features = cli._runtime_builtin_features_for_profile(
        "micro",
        target_triple="wasm32-wasip1",
    )

    assert "stdlib_tk" not in features
    assert "stdlib_net" not in features
    assert "stdlib_ast" not in features
    assert "stdlib_unicode_names" not in features
    assert "stdlib_serial" in features


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


def test_runtime_fingerprint_rehashes_when_source_metadata_changes(
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

    source.write_text("pub fn marker() { let _changed = 1; }\n")
    stat = source.stat()
    os.utime(source, ns=(stat.st_atime_ns, stat.st_mtime_ns + 1_000_000))

    calls = 0
    original = cli._hash_runtime_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(cli, "_hash_runtime_file", wrapped, raising=True)
    changed = cli._runtime_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        target_triple=None,
        rustflags="",
        runtime_features=(),
        stored_fingerprint=baseline,
    )

    assert changed is not None
    assert changed["inputs_digest"] != baseline["inputs_digest"]
    assert changed["hash"] != baseline["hash"]
    assert calls == 1


def test_artifact_needs_rebuild_stats_artifact_once(
    tmp_path: Path, monkeypatch
) -> None:
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


def test_artifact_needs_rebuild_on_runtime_meta_digest_mismatch(tmp_path: Path) -> None:
    artifact = tmp_path / "libmolt_runtime.a"
    artifact.write_bytes(b"!<arch>\nfake-staticlib")

    assert cli._artifact_needs_rebuild(
        artifact,
        {"hash": "same", "rustc": "rustc-test", "meta_digest": "full-profile"},
        {"hash": "same", "rustc": "rustc-test", "meta_digest": "micro-profile"},
    )


def test_ensure_runtime_lib_full_profile_fingerprint_declares_default_stdlib(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"!<arch>\nfull")
    project_root = tmp_path / "repo"
    project_root.mkdir()
    captured_features: list[tuple[str, ...]] = []

    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: captured_features.append(
            tuple(kwargs["runtime_features"])
        )
        or {"hash": "ok", "rustc": "rustc-test"},
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "runtime.fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        cli, "_read_runtime_fingerprint", lambda path: {"hash": "ok"}, raising=True
    )
    monkeypatch.setattr(
        cli, "_artifact_needs_rebuild", lambda *args, **kwargs: False, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    try:
        assert cli._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        cli._RUNTIME_LIB_VERIFIED.clear()

    assert captured_features
    assert "stdlib_full" in captured_features[0]
    assert "default-features" in captured_features[0]
    assert "no-default-features" not in captured_features[0]


def test_ensure_runtime_lib_full_profile_passes_stdlib_full_to_cargo(
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
        "_read_runtime_fingerprint",
        lambda path: {"hash": "stale", "rustc": "rustc-test"},
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_artifact_needs_rebuild",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda *args, **kwargs: False,
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
        runtime_lib.parent.mkdir(parents=True, exist_ok=True)
        runtime_lib.write_bytes(b"!<arch>\nfull")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert cli._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        cli._RUNTIME_LIB_VERIFIED.clear()

    assert seen_cmds
    assert "--no-default-features" not in seen_cmds[0]
    feature_index = seen_cmds[0].index("--features")
    features = set(seen_cmds[0][feature_index + 1].split(","))
    assert {"molt_tk_native", "stdlib_full"} <= features
    assert "stdlib_micro" not in features


def test_prepare_backend_setup_warms_native_runtime_with_requested_stdlib_profile(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_state = cli._RuntimeArtifactState(runtime_lib=tmp_path / "libmolt_runtime.a")
    cache_setup = cli._BackendCacheSetup(
        cache_enabled=True,
        cache_key=None,
        function_cache_key=None,
        cache_path=None,
        function_cache_path=None,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        stdlib_object_manifest=None,
        cache_candidates=(),
        cache_hit=False,
        cache_hit_tier=None,
    )
    warmed_profiles: list[str | None] = []

    monkeypatch.setattr(
        cli,
        "_initialize_runtime_artifact_state",
        lambda *args, **kwargs: runtime_state,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_prepare_backend_cache_setup",
        lambda *args, **kwargs: cache_setup,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: warmed_profiles.append(kwargs["stdlib_profile"]),
        raising=True,
    )

    prepared, err = cli._prepare_backend_setup(
        is_rust_transpile=False,
        is_luau_transpile=False,
        is_wasm=False,
        emit_mode="bin",
        molt_root=tmp_path,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        json_output=True,
        cargo_timeout=1.0,
        target="native",
        profile="release",
        backend_cargo_profile="dev-fast",
        linked=False,
        project_root=tmp_path,
        cache_dir=None,
        output_artifact=tmp_path / "out",
        warnings=[],
        cache=True,
        ir={"functions": []},
        entry_module="__main__",
        module_graph_metadata=object(),  # type: ignore[arg-type]
        target_python="py312",
        stdlib_profile="full",
        resolved_modules={"molt_msgpack"},
    )

    assert err is None
    assert prepared is not None
    assert warmed_profiles == ["full"]


def test_ensure_runtime_lib_rebuilds_unfingerprinted_prebuilt_archive(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"!<arch>\nstale-profile")
    source = tmp_path / "runtime" / "molt-runtime" / "src" / "lib.rs"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text("pub fn marker() {}\n", encoding="utf-8")
    os.utime(source, ns=(1, 1))
    os.utime(runtime_lib, ns=(2_000_000_000, 2_000_000_000))
    project_root = tmp_path / "repo"
    project_root.mkdir()
    seen_cmds: list[list[str]] = []

    monkeypatch.setattr(cli, "_runtime_source_paths", lambda _root: [source])
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {"hash": "new", "rustc": "rustc-test"},
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "runtime.fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(
        cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True, raising=True
    )
    monkeypatch.setattr(
        cli,
        "_artifact_newer_than_sources",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("native runtime must not relabel an unfingerprinted archive")
        ),
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda *args, **kwargs: False,
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
        runtime_lib.write_bytes(b"!<arch>\nrebuilt")
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert cli._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        cli._RUNTIME_LIB_VERIFIED.clear()

    assert seen_cmds


def test_internal_batch_build_stdlib_profile_is_explicit_and_validated() -> None:
    assert cli._normalize_internal_batch_stdlib_profile({}) == ("micro", None)
    assert cli._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": "full"}
    ) == ("full", None)

    missing_value, type_error = cli._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": 1}
    )
    assert missing_value is None
    assert type_error == "stdlib_profile must be a string"

    invalid_value, choice_error = cli._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": "standard"}
    )
    assert invalid_value is None
    assert choice_error == "stdlib_profile must be 'micro' or 'full'"


def test_backend_fingerprint_reuses_stored_hash_when_inputs_unchanged(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "backend_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        cli,
        "_backend_source_paths",
        lambda _project_root, _features=(): [source],
        raising=True,
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

    assert (
        first
        == second
        == {
            "version": 2,
            "hash": "abc",
            "rustc": "rustc-test",
            "inputs_digest": "digest",
            "meta_digest": None,
        }
    )
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


def test_ensure_runtime_lib_rebuilds_when_stored_fingerprint_conflicts_with_requested_gpu_features(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"!<arch>\nfake-staticlib")
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    stale_fingerprint = {
        "version": 2,
        "hash": "stale",
        "rustc": "rustc-test",
        "inputs_digest": "digest",
        "meta_digest": "stale-meta",
    }
    seen_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    monkeypatch.setattr(
        cli, "_runtime_source_paths", lambda _project_root: [source], raising=True
    )
    monkeypatch.setattr(
        cli,
        "_runtime_fingerprint",
        lambda *args, **kwargs: {
            "hash": "new",
            "rustc": "rustc-test",
            "inputs_digest": "digest",
            "meta_digest": "new-meta",
        },
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
        lambda path: dict(stale_fingerprint) if path == fingerprint_path else None,
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
        cargo_timeout=1.0,
    )
    assert seen_cmds
    feature_index = seen_cmds[0].index("--features")
    features_str = seen_cmds[0][feature_index + 1]
    assert "molt_gpu_metal" in features_str.split(",")
