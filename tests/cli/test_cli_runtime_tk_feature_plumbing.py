from __future__ import annotations

import contextlib
import importlib
import os
import subprocess
from pathlib import Path

import molt.cli as cli

COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")
RUNTIME_FINGERPRINTS = importlib.import_module("molt.cli.runtime_fingerprints")


ROOT = Path(__file__).resolve().parents[2]


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
    assert set(json_features) == set(
        cli._ALL_BUILTIN_FEATURES + cli._MICRO_BASE_RUNTIME_FEATURES
    )
    assert "stdlib_tk" not in json_features
    assert "stdlib_net" not in json_features
    assert "stdlib_serial" not in json_features
    assert "molt_gpu_primitives" not in json_features


def test_runtime_source_paths_include_runtime_leaf_crates() -> None:
    RUNTIME_FINGERPRINTS._runtime_source_paths_cached.cache_clear()

    paths = set(RUNTIME_FINGERPRINTS._runtime_source_paths(ROOT))

    assert ROOT / "runtime/molt-runtime-stringprep/src" in paths
    assert ROOT / "runtime/molt-runtime-stringprep/Cargo.toml" in paths
    assert ROOT / "runtime/molt-runtime-http/src" in paths
    assert ROOT / "runtime/Cargo.toml" in paths
    assert ROOT / "runtime/Cargo.lock" in paths


def test_runtime_builtin_features_exclude_native_only_wasm_domains() -> None:
    features = cli._runtime_builtin_features_for_profile(
        "micro",
        target_triple="wasm32-wasip1",
    )

    assert "stdlib_tk" not in features
    assert "stdlib_net" not in features
    assert "stdlib_ast" not in features
    assert "stdlib_unicode_names" not in features
    assert "stdlib_logging_ext" in features
    assert "stdlib_serial" in features


def test_runtime_builtin_features_wasm_full_is_linked_wasm_surface() -> None:
    features = cli._runtime_builtin_features_for_profile(
        "full",
        target_triple="wasm32-wasip1",
    )

    assert set(features) == set(cli._WASM_RUNTIME_FULL_FEATURES)
    assert "sqlite" not in features
    assert "stdlib_tk" not in features
    assert "stdlib_net" not in features
    assert "stdlib_ast" not in features
    assert "stdlib_unicode_names" not in features
    assert "stdlib_crypto" in features
    assert "stdlib_compression" in features


def test_runtime_cargo_features_is_cached(monkeypatch) -> None:
    cli._runtime_cargo_features_cached.cache_clear()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")

    first = cli._runtime_cargo_features(None)
    second = cli._runtime_cargo_features(None)

    info = cli._runtime_cargo_features_cached.cache_info()
    assert first == second == ("molt_tk_native",)
    assert info.hits >= 1
    assert info.currsize >= 1


def test_runtime_lib_path_is_stdlib_profile_qualified(
    tmp_path: Path, monkeypatch
) -> None:
    cli._runtime_lib_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "target"))

    micro = cli._runtime_lib_path(
        tmp_path,
        "dev-fast",
        None,
        stdlib_profile="micro",
    )
    full = cli._runtime_lib_path(
        tmp_path,
        "dev-fast",
        None,
        stdlib_profile="full",
    )
    target_micro = cli._runtime_lib_path(
        tmp_path,
        "dev-fast",
        "aarch64-apple-darwin",
        stdlib_profile="micro",
    )

    assert micro != full
    assert micro.name == cli._runtime_lib_archive_name("micro", None)
    assert full.name == cli._runtime_lib_archive_name("full", None)
    assert target_micro == (
        tmp_path
        / "target"
        / "aarch64-apple-darwin"
        / "dev-fast"
        / cli._runtime_lib_archive_name("micro", "aarch64-apple-darwin")
    )


def test_runtime_fingerprint_path_is_stdlib_profile_qualified(tmp_path: Path) -> None:
    target_root = tmp_path / "target" / "dev-fast"
    micro = target_root / "libmolt_runtime.stdlib_micro.a"
    full = target_root / "libmolt_runtime.stdlib_full.a"

    micro_fingerprint = cli._runtime_fingerprint_path(
        tmp_path,
        micro,
        "dev-fast",
        None,
    )
    full_fingerprint = cli._runtime_fingerprint_path(
        tmp_path,
        full,
        "dev-fast",
        None,
    )

    assert micro_fingerprint != full_fingerprint
    assert "libmolt_runtime.stdlib_micro.a" in micro_fingerprint.name
    assert "libmolt_runtime.stdlib_full.a" in full_fingerprint.name


def test_runtime_fingerprint_changes_with_runtime_features(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_runtime_source_paths",
        lambda _project_root: [source],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc-test", raising=True
    )
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
        RUNTIME_FINGERPRINTS,
        "_runtime_source_paths",
        lambda _project_root: [source],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc-test", raising=True
    )

    baseline = cli._runtime_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        target_triple=None,
        rustflags="",
        runtime_features=(),
    )
    assert baseline is not None

    calls = 0
    original = RUNTIME_FINGERPRINTS._hash_runtime_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_hash_runtime_file", wrapped, raising=True
    )
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
        RUNTIME_FINGERPRINTS,
        "_runtime_source_paths",
        lambda _project_root: [source],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc-test", raising=True
    )

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
    original = RUNTIME_FINGERPRINTS._hash_runtime_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_hash_runtime_file", wrapped, raising=True
    )
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

    def wrapped_stat(
        self: Path, *, follow_symlinks: bool = True
    ) -> os.stat_result:
        nonlocal calls
        calls += 1
        return original_stat(self, follow_symlinks=follow_symlinks)

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
        lambda project_root, **kwargs: (
            captured_features.append(tuple(kwargs["runtime_features"]))
            or {"hash": "ok", "rustc": "rustc-test"}
        ),
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
        cli,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: kwargs["require_artifact_digest"] is True,
        raising=True,
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


def test_ensure_runtime_lib_session_cache_is_source_fingerprint_qualified(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"!<arch>\nfake-staticlib")
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprints = [
        {
            "hash": "runtime-hash-a",
            "rustc": "rustc-test",
            "inputs_digest": "inputs-a",
            "meta_digest": "meta",
        },
        {
            "hash": "runtime-hash-b",
            "rustc": "rustc-test",
            "inputs_digest": "inputs-b",
            "meta_digest": "meta",
        },
    ]
    fingerprint_calls: list[str | None] = []
    artifact_checks: list[str | None] = []

    def fake_runtime_fingerprint(*args, **kwargs):  # type: ignore[no-untyped-def]
        del args, kwargs
        fingerprint = fingerprints[len(fingerprint_calls)]
        fingerprint_calls.append(fingerprint["hash"])
        return fingerprint

    def fake_runtime_artifact_fingerprint_matches(
        artifact: Path,
        fingerprint: dict[str, str | None] | None,
        fingerprint_path: Path,
        *,
        require_artifact_digest: bool,
    ) -> bool:
        del artifact, fingerprint_path
        assert require_artifact_digest is True
        assert fingerprint is not None
        artifact_checks.append(fingerprint.get("hash"))
        return True

    monkeypatch.setattr(
        cli, "_runtime_fingerprint", fake_runtime_fingerprint, raising=True
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
        lambda path: {
            "hash": "runtime-hash-a",
            "rustc": "rustc-test",
            "inputs_digest": "inputs-a",
            "meta_digest": "meta",
        },
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_runtime_artifact_fingerprint_matches",
        fake_runtime_artifact_fingerprint_matches,
        raising=True,
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

    assert fingerprint_calls == ["runtime-hash-a", "runtime-hash-b"]
    assert artifact_checks == ["runtime-hash-a", "runtime-hash-b"]


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


def test_ensure_runtime_lib_materializes_stdlib_profile_aliases_without_rebuilding_final_micro(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    cli._runtime_lib_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()
    cli._build_state_root_cached.cache_clear()

    micro_lib = cli._runtime_lib_path(
        project_root,
        "dev-fast",
        None,
        stdlib_profile="micro",
    )
    full_lib = cli._runtime_lib_path(
        project_root,
        "dev-fast",
        None,
        stdlib_profile="full",
    )
    cargo_profiles: list[str] = []

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_runtime_source_paths", lambda _root: [], raising=True
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc-test", raising=True
    )
    monkeypatch.setattr(
        cli,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(cli, "_maybe_enable_sccache", lambda _env: None, raising=True)

    def fake_run_cargo(
        cmd: list[str],
        *,
        cwd: Path,
        env: dict[str, str],
        timeout: float | None,
        json_output: bool,
        label: str,
    ) -> subprocess.CompletedProcess[str]:
        del cwd, timeout, json_output, label
        joined = " ".join(cmd)
        profile = "micro" if "stdlib_micro" in joined else "full"
        cargo_profiles.append(profile)
        scratch = Path(env["CARGO_TARGET_DIR"]) / "dev-fast" / "libmolt_runtime.a"
        scratch.parent.mkdir(parents=True, exist_ok=True)
        scratch.write_bytes(f"!<arch>\n{profile}".encode("utf-8"))
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(
        cli, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert cli._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
        assert cli._ensure_runtime_lib(
            full_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
        assert cli._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
        cli._RUNTIME_LIB_VERIFIED.clear()
        assert cli._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
    finally:
        cli._RUNTIME_LIB_VERIFIED.clear()

    assert cargo_profiles == ["micro", "full"]
    assert micro_lib.read_bytes() == b"!<arch>\nmicro"
    assert full_lib.read_bytes() == b"!<arch>\nfull"


def test_prepare_native_link_resolves_runtime_alias_for_stdlib_profile(
    tmp_path: Path, monkeypatch
) -> None:
    project_root = tmp_path / "repo"
    project_root.mkdir()
    target_root = tmp_path / "target"
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))
    cli._runtime_lib_path_cached.cache_clear()
    cli._cargo_target_root_cached.cache_clear()

    output_obj = tmp_path / "output.o"
    output_obj.write_bytes(b"\x7fELFobject")
    output_binary = tmp_path / "app"
    artifacts_root = tmp_path / "artifacts"
    artifacts_root.mkdir()
    captured_runtime_libs: list[Path] = []

    def fake_build_native_link_command(
        *,
        output_obj: Path,
        stub_path: Path,
        runtime_lib: Path,
        output_binary: Path,
        target_triple: str | None,
        sysroot_path: Path | None,
        profile: str,
        stdlib_obj_path: Path | None = None,
    ) -> tuple[list[str], str | None, str | None]:
        del output_obj, stub_path, output_binary, target_triple, sysroot_path, profile
        del stdlib_obj_path
        captured_runtime_libs.append(runtime_lib)
        return ["clang", str(runtime_lib)], None, None

    monkeypatch.setattr(
        cli,
        "_build_native_link_command",
        fake_build_native_link_command,
        raising=True,
    )
    monkeypatch.setattr(
        cli,
        "_link_fingerprint",
        lambda *args, **kwargs: {
            "hash": "link",
            "rustc": None,
            "inputs_digest": None,
        },
        raising=True,
    )
    monkeypatch.setattr(cli, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(cli, "_artifact_needs_rebuild", lambda *args, **kwargs: True)
    monkeypatch.setattr(
        cli,
        "_run_native_link_command",
        lambda **kwargs: subprocess.CompletedProcess(kwargs["link_cmd"], 0, "", ""),
    )

    prepared, error = cli._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=True,
        output_binary=output_binary,
        runtime_lib=None,
        molt_root=project_root,
        runtime_cargo_profile="dev-fast",
        target_triple=None,
        sysroot_path=None,
        profile="dev",
        project_root=project_root,
        diagnostics_enabled=False,
        phase_starts={},
        link_timeout=None,
        warnings=[],
        stdlib_profile="full",
    )

    expected = cli._runtime_lib_path(
        project_root,
        "dev-fast",
        None,
        stdlib_profile="full",
    )
    assert error is None
    assert prepared is not None
    assert captured_runtime_libs == [expected]
    assert prepared.runtime_lib == expected
    assert str(expected) in prepared.link_cmd


def test_prepare_backend_setup_warms_native_runtime_with_requested_stdlib_profile(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_state = cli._RuntimeArtifactState(
        runtime_lib=tmp_path / "libmolt_runtime.a"
    )
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

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_runtime_source_paths", lambda _root: [source]
    )
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
    assert cli._normalize_internal_batch_stdlib_profile({"stdlib_profile": "full"}) == (
        "full",
        None,
    )

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
    COMPILER_METADATA._rustc_version.cache_clear()
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

    monkeypatch.setattr(
        COMPILER_METADATA, "_run_completed_command", fake_run, raising=True
    )
    first = COMPILER_METADATA._rustc_version()
    second = COMPILER_METADATA._rustc_version()
    assert first == "release: 1.0.0"
    assert second == first
    assert calls == 1
    COMPILER_METADATA._rustc_version.cache_clear()


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
        RUNTIME_FINGERPRINTS,
        "_runtime_source_paths",
        lambda _project_root: [source],
        raising=True,
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
