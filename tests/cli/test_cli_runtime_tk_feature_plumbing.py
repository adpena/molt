from __future__ import annotations

import contextlib
import hashlib
import importlib
import os
import subprocess
from pathlib import Path
from types import SimpleNamespace

import molt.cli as cli
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_cache_setup as cli_backend_cache_setup
from molt.cli import backend_compile as cli_backend_compile
from molt.cli import commands as cli_commands
from molt.cli import link_pipeline as cli_link_pipeline
from tests.cli.native_link_test_support import (
    SOURCE_FINGERPRINT,
    static_archive_bytes,
)

COMPILER_METADATA = importlib.import_module("molt.cli.compiler_metadata")
RUNTIME_FEATURES = importlib.import_module("molt.cli.runtime_features")
RUNTIME_BUILD = importlib.import_module("molt.cli.runtime_native_build")
RUNTIME_FINGERPRINTS = importlib.import_module("molt.cli.runtime_fingerprints")
RUNTIME_PATHS = importlib.import_module("molt.cli.runtime_paths")
CARGO_EXECUTION = importlib.import_module("molt.cli.cargo_execution")
FILE_HASHING = importlib.import_module("molt.file_hashing")


ROOT = Path(__file__).resolve().parents[2]
_NATIVE_STATICLIBS_NOTE = "note: native-static-libs: -lc\n"


def _source_fingerprint(hash_value: str) -> dict[str, object]:
    return {
        **SOURCE_FINGERPRINT,
        "hash": hashlib.sha256(hash_value.encode("utf-8")).hexdigest(),
    }


def _stub_backend_binary_ensure(monkeypatch, tmp_path: Path) -> Path:
    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend"
    monkeypatch.setattr(
        cli_backend_compile,
        "_backend_bin_path",
        lambda *args, **kwargs: backend_bin,
        raising=True,
    )

    def fake_ensure_backend_binary(*args, **kwargs):
        del args
        stage_timings_ms = kwargs.get("stage_timings_ms")
        if isinstance(stage_timings_ms, dict):
            stage_timings_ms["backend_binary_probe"] = 4.0
        backend_bin.parent.mkdir(parents=True, exist_ok=True)
        backend_bin.write_text("backend", encoding="utf-8")
        return cli_backend_binary._backend_ensure_success(
            fingerprint={
                "hash": "backend-hash",
                "rustc": "rustc-test",
                "inputs_digest": "backend-inputs",
                "meta_digest": "backend-meta",
            }
        )

    monkeypatch.setattr(
        cli_backend_binary,
        "_ensure_backend_binary",
        fake_ensure_backend_binary,
        raising=True,
    )
    return backend_bin


def test_runtime_cargo_features_native_vs_wasm(monkeypatch) -> None:
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_METAL", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_WEBGPU", raising=False)
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == ("molt_tk_native",)
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "0")
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == ()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == ("molt_tk_native",)
    assert RUNTIME_FEATURES._runtime_cargo_features("aarch64-apple-darwin") == (
        "molt_tk_native",
    )
    assert RUNTIME_FEATURES._runtime_cargo_features("wasm32-wasip1") == ()


def test_runtime_cargo_features_include_gpu_backend_flags(monkeypatch) -> None:
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    monkeypatch.delenv("MOLT_RUNTIME_TK_NATIVE", raising=False)
    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    monkeypatch.delenv("MOLT_RUNTIME_GPU_WEBGPU", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_CUDA", raising=False)
    monkeypatch.delenv("MOLT_RUNTIME_GPU_HIP", raising=False)
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_gpu_metal",
    )

    monkeypatch.setenv("MOLT_RUNTIME_GPU_WEBGPU", "1")
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_gpu_metal",
        "molt_gpu_webgpu",
    )

    monkeypatch.setenv("MOLT_RUNTIME_GPU_CUDA", "1")
    monkeypatch.setenv("MOLT_RUNTIME_GPU_HIP", "1")
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    assert RUNTIME_FEATURES._runtime_cargo_features(None) == (
        "molt_tk_native",
        "molt_gpu_metal",
        "molt_gpu_webgpu",
        "molt_gpu_cuda",
        "molt_gpu_hip",
    )
    assert RUNTIME_FEATURES._runtime_cargo_features("wasm32-wasip1") == ()


def test_builtin_features_from_import_graph_uses_native_micro_surface() -> None:
    json_features = RUNTIME_FEATURES._builtin_features_from_import_graph(
        {"json"}, "micro"
    )
    tkinter_features = RUNTIME_FEATURES._builtin_features_from_import_graph(
        {"tkinter.constants", "tkinter._support"},
        "micro",
    )
    tinygrad_features = RUNTIME_FEATURES._builtin_features_from_import_graph(
        {"tinygrad.tensor", "tinygrad.nn"},
        "micro",
    )

    assert json_features == tkinter_features == tinygrad_features
    assert set(json_features) == set(
        RUNTIME_FEATURES._ALL_BUILTIN_FEATURES
        + RUNTIME_FEATURES._MICRO_BASE_RUNTIME_FEATURES
    )
    assert "stdlib_tk" not in json_features
    assert "stdlib_net" not in json_features
    assert "stdlib_serial" not in json_features
    assert "molt_gpu_primitives" not in json_features


def test_wasm_runtime_feature_plan_requires_gpu_authority() -> None:
    builtin_features = RUNTIME_FEATURES._runtime_builtin_features_for_profile(
        "micro",
        target_triple="wasm32-wasip1",
    )

    _no_defaults, cargo_features, fingerprint_features = (
        RUNTIME_FEATURES._wasm_runtime_feature_plan(
            stdlib_profile="micro",
            runtime_features=(),
            builtin_features=builtin_features,
            resolved_modules=frozenset(),
        )
    )

    assert "molt_gpu_primitives" not in cargo_features
    assert "molt_gpu_primitives" not in fingerprint_features
    assert "builtin_set" not in cargo_features
    assert "stdlib_crypto" not in cargo_features
    assert "stdlib_serial" not in cargo_features
    assert "stdlib_micro" in cargo_features

    _no_defaults, required_cargo, _required_fingerprint = (
        RUNTIME_FEATURES._wasm_runtime_feature_plan(
            stdlib_profile="micro",
            runtime_features=(),
            builtin_features=builtin_features,
            resolved_modules=frozenset(),
            required_link_features={"builtin_set", "molt_gpu_primitives"},
        )
    )
    _no_defaults, resolved_cargo, _resolved_fingerprint = (
        RUNTIME_FEATURES._wasm_runtime_feature_plan(
            stdlib_profile="micro",
            runtime_features=(),
            builtin_features=builtin_features,
            resolved_modules={"tinygrad.tensor"},
        )
    )

    assert "molt_gpu_primitives" in required_cargo
    assert "builtin_set" in required_cargo
    assert "molt_gpu_primitives" in resolved_cargo


def test_runtime_source_paths_follow_runtime_feature_closure() -> None:
    micro_paths = {
        path.relative_to(ROOT).as_posix()
        for path in RUNTIME_FINGERPRINTS.runtime_source_paths(
            ROOT,
            runtime_features=("stdlib_micro", "no-default-features"),
        )
    }
    full_paths = {
        path.relative_to(ROOT).as_posix()
        for path in RUNTIME_FINGERPRINTS.runtime_source_paths(
            ROOT,
            runtime_features=("stdlib_full", "default-features"),
        )
    }

    common_paths = {
        "runtime/molt-runtime",
        "runtime/molt-cpython-abi",
        "runtime/molt-obj-model",
        "runtime/molt-runtime-core",
        "runtime/molt-runtime-vfs",
        "runtime/build_support",
        "runtime/Cargo.toml",
        "runtime/Cargo.lock",
        "Cargo.toml",
        "Cargo.lock",
        "LICENSE",
        "third_party/cpython/Modules/_decimal/libmpdec",
    }
    assert common_paths.issubset(micro_paths)
    assert common_paths.issubset(full_paths)
    assert "runtime/molt-runtime-logging" in micro_paths
    assert "runtime/molt-runtime-collections" in micro_paths
    assert "runtime/molt-runtime-asyncio" in micro_paths
    assert "runtime/molt-runtime-stringprep" not in micro_paths
    assert "runtime/molt-runtime-http" not in micro_paths
    assert "runtime/molt-runtime-tk" not in micro_paths
    assert "runtime/molt-runtime-stringprep" in full_paths
    assert "runtime/molt-runtime-http" in full_paths
    assert "runtime/molt-runtime-tk" in full_paths


def test_runtime_builtin_features_exclude_native_only_wasm_domains() -> None:
    features = RUNTIME_FEATURES._runtime_builtin_features_for_profile(
        "micro",
        target_triple="wasm32-wasip1",
    )

    assert "stdlib_tk" not in features
    assert "stdlib_net" not in features
    assert "stdlib_ast" not in features
    assert "stdlib_unicode_names" not in features
    assert "stdlib_logging_ext" in features
    assert "stdlib_serial" not in features
    assert "stdlib_crypto" not in features
    assert "stdlib_compression" not in features


def test_runtime_source_paths_follow_child_feature_activation() -> None:
    paths = {
        path.relative_to(ROOT).as_posix()
        for path in RUNTIME_FINGERPRINTS.runtime_source_paths(
            ROOT,
            runtime_features=(
                "stdlib_micro",
                "molt_tk_native",
                "no-default-features",
            ),
        )
    }

    assert "runtime/molt-runtime-tk" in paths


def test_runtime_builtin_features_wasm_full_is_linked_wasm_surface() -> None:
    features = RUNTIME_FEATURES._runtime_builtin_features_for_profile(
        "full",
        target_triple="wasm32-wasip1",
    )

    assert set(features) == set(
        RUNTIME_FEATURES._ALL_BUILTIN_FEATURES
    ) | RUNTIME_FEATURES.profile_link_features(
        "full",
        target_triple="wasm32-wasip1",
    )
    assert "sqlite" not in features
    assert "stdlib_tk" not in features
    assert "stdlib_net" not in features
    assert "stdlib_unicode_names" not in features
    assert "stdlib_crypto" in features
    assert "stdlib_compression" in features


def test_runtime_cargo_features_is_cached(monkeypatch) -> None:
    RUNTIME_FEATURES._runtime_cargo_features_cached.cache_clear()
    monkeypatch.setenv("MOLT_RUNTIME_TK_NATIVE", "1")

    first = RUNTIME_FEATURES._runtime_cargo_features(None)
    second = RUNTIME_FEATURES._runtime_cargo_features(None)

    info = RUNTIME_FEATURES._runtime_cargo_features_cached.cache_info()
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


def test_cargo_target_root_ignores_removed_legacy_target_root_env(
    tmp_path: Path,
    monkeypatch,
) -> None:
    legacy_key = "MOLT" + "_TARGET_ROOT"
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    for key in (
        "MOLT_EXT_ROOT",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_USE_EXTERNAL_ARTIFACTS",
    ):
        monkeypatch.delenv(key, raising=False)
    monkeypatch.setenv(legacy_key, str(tmp_path / "legacy-target"))
    monkeypatch.setenv("MOLT_SESSION_ID", "alpha/session:beta")

    assert cli._cargo_target_root(tmp_path) == (
        tmp_path / "target" / "sessions" / "alpha_session_beta"
    )


def test_cargo_target_root_uses_dx_external_session_target_when_required(
    tmp_path: Path,
    monkeypatch,
) -> None:
    external_root = tmp_path / "external" / "Molt"
    cli._cargo_target_root_cached.cache_clear()
    monkeypatch.delenv("CARGO_TARGET_DIR", raising=False)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(external_root))
    monkeypatch.setenv("MOLT_REQUIRE_EXTERNAL_ARTIFACTS", "1")
    monkeypatch.setenv("MOLT_ALLOW_C_DRIVE_ARTIFACTS", "1")
    monkeypatch.delenv("MOLT_SESSION_ID_GENERATED", raising=False)
    monkeypatch.setenv("MOLT_SESSION_ID", "agent-one")

    assert cli._cargo_target_root(tmp_path) == (
        external_root.resolve() / "target" / "sessions" / "agent-one"
    )


def test_cargo_build_env_preserves_public_cargo_defaults_without_dev_request(
    monkeypatch,
) -> None:
    for key in (
        "MOLT_EXT_ROOT",
        "CARGO_TARGET_DIR",
        "MOLT_REQUIRE_EXTERNAL_ARTIFACTS",
        "MOLT_PREFER_EXTERNAL_ARTIFACTS",
        "MOLT_USE_EXTERNAL_ARTIFACTS",
    ):
        monkeypatch.delenv(key, raising=False)

    env = CARGO_EXECUTION._cargo_build_env()

    assert "MOLT_EXT_ROOT" not in env
    assert "CARGO_TARGET_DIR" not in env


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
        "runtime_source_paths",
        lambda _project_root, **_kwargs: [source],
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
        "runtime_source_paths",
        lambda _project_root, **_kwargs: [source],
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
    original = FILE_HASHING._hash_source_tree_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(FILE_HASHING, "_hash_source_tree_file", wrapped, raising=True)
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


def test_runtime_fingerprint_reuses_clean_source_state_without_metadata_scan(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    source_state = {
        "schema_version": 1,
        "kind": "git-clean-pathspec",
        "pathspec_count": 1,
        "pathspec_digest": "abc123",
        "tracked_digest": "def456",
        "tracked_entry_count": 1,
    }
    pathspec_calls: list[tuple[str, ...]] = []

    def clean_pathspec_state(
        _project_root: Path, path_keys: tuple[str, ...]
    ) -> dict[str, str | int]:
        pathspec_calls.append(path_keys)
        return dict(source_state)

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _project_root, **_kwargs: [source],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_compiler_clean_pathspec_source_state",
        clean_pathspec_state,
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
    assert baseline["source_state"] == source_state
    assert pathspec_calls == [(str(source),)]

    def fail_metadata_scan(*_args, **_kwargs):
        raise AssertionError("clean source state should skip metadata scan")

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_hash_source_tree_metadata",
        fail_metadata_scan,
        raising=True,
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
    assert pathspec_calls == [(str(source),), (str(source),)]


def test_runtime_fingerprint_rehashes_when_source_metadata_changes(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _project_root, **_kwargs: [source],
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
    original = FILE_HASHING._hash_source_tree_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(FILE_HASHING, "_hash_source_tree_file", wrapped, raising=True)
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

    def wrapped_stat(self: Path, *, follow_symlinks: bool = True) -> os.stat_result:
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
    artifact.write_bytes(static_archive_bytes(b"fake-staticlib"))

    assert cli._artifact_needs_rebuild(
        artifact,
        {"hash": "same", "rustc": "rustc-test", "meta_digest": "full-profile"},
        {"hash": "same", "rustc": "rustc-test", "meta_digest": "micro-profile"},
    )


def test_runtime_artifact_match_reuses_stored_artifact_identity(
    tmp_path: Path, monkeypatch
) -> None:
    artifact = tmp_path / "libmolt_runtime.a"
    artifact.write_bytes(static_archive_bytes(b"fake-staticlib"))
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprint = {
        "hash": "runtime-hash",
        "rustc": "rustc-test",
        "inputs_digest": "inputs",
        "meta_digest": "meta",
    }

    RUNTIME_FINGERPRINTS._write_runtime_fingerprint(
        fingerprint_path,
        fingerprint,
        artifact=artifact,
    )
    stored = RUNTIME_FINGERPRINTS._read_runtime_fingerprint(fingerprint_path)
    assert stored is not None
    assert isinstance(stored.get("artifact_identity"), dict)

    def fail_hash(path: Path) -> str:
        del path
        raise AssertionError("artifact identity should avoid the staticlib hash")

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "artifact_content_identity",
        fail_hash,
        raising=True,
    )

    assert RUNTIME_FINGERPRINTS._runtime_artifact_fingerprint_matches(
        artifact,
        fingerprint,
        fingerprint_path,
        require_artifact_digest=True,
    )


def test_runtime_artifact_match_hashes_when_artifact_identity_is_stale(
    tmp_path: Path, monkeypatch
) -> None:
    artifact = tmp_path / "libmolt_runtime.a"
    artifact.write_bytes(static_archive_bytes(b"fake-staticlib"))
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprint = {
        "hash": "runtime-hash",
        "rustc": "rustc-test",
        "inputs_digest": "inputs",
        "meta_digest": "meta",
    }

    RUNTIME_FINGERPRINTS._write_runtime_fingerprint(
        fingerprint_path,
        fingerprint,
        artifact=artifact,
    )
    stored = RUNTIME_FINGERPRINTS._read_runtime_fingerprint(fingerprint_path)
    assert stored is not None
    artifact_digest = stored.get("artifact_content_identity")
    assert isinstance(artifact_digest, dict)
    hash_calls: list[Path] = []

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_runtime_artifact_identity",
        lambda path: {"path": f"stale:{path}"},
        raising=True,
    )

    def fake_hash(path: Path) -> dict[str, object]:
        hash_calls.append(path)
        return artifact_digest

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "artifact_content_identity",
        fake_hash,
        raising=True,
    )

    assert RUNTIME_FINGERPRINTS._runtime_artifact_fingerprint_matches(
        artifact,
        fingerprint,
        fingerprint_path,
        require_artifact_digest=True,
    )
    assert hash_calls == [artifact]


def test_runtime_fingerprint_metadata_refresh_preserves_artifact_identity(
    tmp_path: Path,
) -> None:
    artifact = tmp_path / "libmolt_runtime.a"
    artifact.write_bytes(b"!<arch>\n")
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprint = {
        "hash": "runtime-hash",
        "rustc": "rustc-test",
        "inputs_digest": "inputs",
        "meta_digest": "meta",
    }
    RUNTIME_FINGERPRINTS._write_runtime_fingerprint(
        fingerprint_path,
        fingerprint,
        artifact=artifact,
    )
    before = RUNTIME_FINGERPRINTS._read_runtime_fingerprint(fingerprint_path)
    assert before is not None
    assert before.get("artifact_content_identity")
    assert isinstance(before.get("artifact_identity"), dict)

    refreshed = {
        **fingerprint,
        "source_state": {
            "schema_version": 1,
            "kind": "git-clean-head",
            "head": "abc123",
        },
    }
    RUNTIME_FINGERPRINTS._refresh_runtime_fingerprint_metadata(
        fingerprint_path,
        refreshed,
    )
    after = RUNTIME_FINGERPRINTS._read_runtime_fingerprint(fingerprint_path)

    assert after is not None
    assert after.get("source_state") == refreshed["source_state"]
    assert after.get("artifact_content_identity") == before.get(
        "artifact_content_identity"
    )
    assert after.get("artifact_identity") == before.get("artifact_identity")


def test_ensure_runtime_lib_full_profile_fingerprint_declares_default_stdlib(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(static_archive_bytes(b"full"))
    project_root = tmp_path / "repo"
    project_root.mkdir()
    captured_features: list[tuple[str, ...]] = []

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda project_root, **kwargs: (
            captured_features.append(tuple(kwargs["runtime_features"]))
            or _source_fingerprint("ok")
        ),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "runtime.fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: _source_fingerprint("ok"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: kwargs["require_artifact_digest"] is True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_native_link_manifest_matches",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

    assert captured_features
    assert "stdlib_full" in captured_features[0]
    assert "default-features" in captured_features[0]
    assert "no-default-features" not in captured_features[0]


def test_ensure_runtime_lib_session_cache_is_source_fingerprint_qualified(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(static_archive_bytes(b"fake-staticlib"))
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
        RUNTIME_BUILD, "_runtime_fingerprint", fake_runtime_fingerprint, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
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
        RUNTIME_BUILD,
        "_runtime_artifact_fingerprint_matches",
        fake_runtime_artifact_fingerprint_matches,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_native_link_manifest_matches",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

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
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _source_fingerprint("new"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: _source_fingerprint("stale"),
        raising=True,
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_artifact_needs_rebuild",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
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
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(runtime_lib, None)
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(static_archive_bytes(b"full"))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

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
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _root, **_kwargs: [],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS, "_rustc_version", lambda: "rustc-test", raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
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
        del cwd, timeout, json_output, label
        joined = " ".join(cmd)
        profile = "micro" if "stdlib_micro" in joined else "full"
        cargo_profiles.append(profile)
        scratch = (
            Path(env["CARGO_TARGET_DIR"])
            / "dev-fast"
            / RUNTIME_PATHS._runtime_cargo_scratch_lib_name(None)
        )
        scratch.parent.mkdir(parents=True, exist_ok=True)
        scratch.write_bytes(static_archive_bytes(profile.encode("utf-8")))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
        assert RUNTIME_BUILD._ensure_runtime_lib(
            full_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
        assert RUNTIME_BUILD._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()
        assert RUNTIME_BUILD._ensure_runtime_lib(
            micro_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="micro",
        )
    finally:
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

    assert cargo_profiles == ["micro", "full"]
    assert micro_lib.read_bytes() == static_archive_bytes(b"micro")
    assert full_lib.read_bytes() == static_archive_bytes(b"full")


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

    def fake_build_native_link_plan(
        *,
        output_obj: Path,
        stub_path: Path,
        runtime_lib: Path,
        output_binary: Path,
        target_triple: str | None,
        sysroot_path: Path | None,
        profile: str,
        source_root: Path,
        source_fingerprint: dict[str, object],
        stdlib_obj_path: Path | None = None,
        export_molt_runtime_symbols: bool = False,
        bolt_requested: bool = False,
    ) -> SimpleNamespace:
        del output_obj, stub_path, target_triple, sysroot_path, profile
        del source_root, source_fingerprint
        del stdlib_obj_path
        del bolt_requested
        assert not export_molt_runtime_symbols
        captured_runtime_libs.append(runtime_lib)
        return SimpleNamespace(
            command=("clang", str(runtime_lib), "-o", str(output_binary)),
            linker_hint=None,
            normalized_target=None,
            policy=SimpleNamespace(strip_after_link=False, bolt_requested=False),
        )

    monkeypatch.setattr(
        cli_link_pipeline,
        "_build_native_link_plan",
        fake_build_native_link_plan,
        raising=True,
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_link_fingerprint",
        lambda *args, **kwargs: {
            "hash": "link",
            "rustc": None,
            "inputs_digest": None,
        },
        raising=True,
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(
        cli_link_pipeline, "_artifact_needs_rebuild", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(
        cli_link_pipeline,
        "_run_native_link_command",
        lambda **kwargs: subprocess.CompletedProcess(kwargs["link_cmd"], 0, "", ""),
    )

    prepared, error = cli_link_pipeline._prepare_native_link(
        output_artifact=output_obj,
        trusted=False,
        capabilities_list=None,
        artifacts_root=artifacts_root,
        json_output=True,
        output_binary=output_binary,
        runtime_lib=None,
        runtime_source_fingerprint=SOURCE_FINGERPRINT,
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
    _stub_backend_binary_ensure(monkeypatch, tmp_path)

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda *args, **kwargs: runtime_state,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        lambda *args, **kwargs: cache_setup,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_stage_runtime_callable_symbols_for_native_codegen",
        lambda *args, **kwargs: ("", None),
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: warmed_profiles.append(kwargs["stdlib_profile"]),
        raising=True,
    )

    prepared, err = cli_backend_compile._prepare_backend_setup(
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


def test_prepare_backend_setup_records_backend_stage_timings(
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
    stage_timings_ms: dict[str, float] = {}
    _stub_backend_binary_ensure(monkeypatch, tmp_path)

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        lambda *args, **kwargs: runtime_state,
        raising=True,
    )

    def fake_prepare_backend_cache_setup(*args, **kwargs):
        del args
        assert kwargs["stage_timings_ms"] is stage_timings_ms
        assert kwargs["backend_compiler_fingerprint"]
        stage_timings_ms["backend_cache_module_key"] = 4.0
        return cache_setup

    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        fake_prepare_backend_cache_setup,
        raising=True,
    )

    def fake_stage_runtime_callable_symbols(*args, **kwargs):
        del args
        assert kwargs["stage_timings_ms"] is stage_timings_ms
        stage_timings_ms["runtime_callable_symbols_ensure_runtime_lib"] = 1.0
        stage_timings_ms["runtime_callable_symbols_file"] = 2.0
        stage_timings_ms["runtime_callable_symbols_digest"] = 3.0
        return "a" * 64, None

    monkeypatch.setattr(
        cli_backend_compile,
        "_stage_runtime_callable_symbols_for_native_codegen",
        fake_stage_runtime_callable_symbols,
        raising=True,
    )

    prepared, err = cli_backend_compile._prepare_backend_setup(
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
        stage_timings_ms=stage_timings_ms,
    )

    assert err is None
    assert prepared is not None
    assert set(stage_timings_ms) == {
        "backend_setup_runtime_callable_symbols",
        "backend_setup_ensure_backend_binary",
        "backend_binary_probe",
        "backend_setup_prepare_cache",
        "backend_cache_module_key",
        "runtime_callable_symbols_ensure_runtime_lib",
        "runtime_callable_symbols_file",
        "runtime_callable_symbols_digest",
    }
    assert all(value >= 0.0 for value in stage_timings_ms.values())


def test_prepare_backend_setup_enables_source_loader_for_native_artifacts(
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
    captured_extra_features: list[tuple[str, ...]] = []
    artifact = cli._ExternalPackageNativeArtifact(
        package="nativepkg",
        module="nativepkg._native",
        package_dir=tmp_path / "nativepkg",
        path=tmp_path / "nativepkg" / "_native.so",
        manifest_path=tmp_path / "nativepkg" / "extension_manifest.json",
        extension_sha256="0" * 64,
        manifest_sha256="1" * 64,
        capabilities=("module.extension.exec",),
        abi_tag="molt_abi1",
        target_triple="x86_64-unknown-linux-gnu",
        platform_tag="x86_64_unknown_linux_gnu",
    )
    native_plan = cli._ExternalPackageNativeArtifactPlan(artifacts=(artifact,))
    _stub_backend_binary_ensure(monkeypatch, tmp_path)

    def fake_initialize_runtime_artifact_state(*args, **kwargs):
        del args
        captured_extra_features.append(tuple(kwargs.get("extra_runtime_features", ())))
        return runtime_state

    monkeypatch.setattr(
        cli_backend_compile,
        "_initialize_runtime_artifact_state",
        fake_initialize_runtime_artifact_state,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_prepare_backend_cache_setup",
        lambda *args, **kwargs: cache_setup,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_stage_runtime_callable_symbols_for_native_codegen",
        lambda *args, **kwargs: ("", None),
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_compile,
        "_maybe_start_native_runtime_lib_ready_async",
        lambda *args, **kwargs: None,
        raising=True,
    )

    prepared, err = cli_backend_compile._prepare_backend_setup(
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
        native_artifact_plan=native_plan,
        resolved_modules={"nativepkg"},
    )

    assert err is None
    assert prepared is not None
    assert captured_extra_features == [("source_extension_loader",)]


def test_ensure_runtime_lib_rebuilds_unfingerprinted_prebuilt_archive(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(static_archive_bytes(b"stale-profile"))
    source = tmp_path / "runtime" / "molt-runtime" / "src" / "lib.rs"
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text("pub fn marker() {}\n", encoding="utf-8")
    exfat_epoch_ns = 315_532_800 * 1_000_000_000
    os.utime(source, ns=(exfat_epoch_ns, exfat_epoch_ns))
    os.utime(
        runtime_lib, ns=(exfat_epoch_ns + 1_000_000_000, exfat_epoch_ns + 1_000_000_000)
    )
    project_root = tmp_path / "repo"
    project_root.mkdir()
    seen_cmds: list[list[str]] = []

    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _root, **_kwargs: [source],
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _source_fingerprint("new"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: tmp_path / "runtime.fingerprint.json",
        raising=True,
    )
    monkeypatch.setattr(RUNTIME_BUILD, "_read_runtime_fingerprint", lambda path: None)
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_artifact_needs_rebuild",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_artifact_newer_than_sources",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("native runtime must not relabel an unfingerprinted archive")
        ),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_maybe_hydrate_artifact_from_canonical_target",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
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
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(runtime_lib, None)
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(static_archive_bytes(b"rebuilt"))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    try:
        assert RUNTIME_BUILD._ensure_runtime_lib(
            runtime_lib,
            target_triple=None,
            json_output=True,
            cargo_profile="dev-fast",
            project_root=project_root,
            cargo_timeout=1.0,
            stdlib_profile="full",
        )
    finally:
        RUNTIME_BUILD._RUNTIME_LIB_VERIFIED.clear()

    assert seen_cmds


def test_internal_batch_build_stdlib_profile_is_explicit_and_validated() -> None:
    assert cli_commands._normalize_internal_batch_stdlib_profile({}) == ("auto", None)
    assert cli_commands._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": "standard"}
    ) == (
        "standard",
        None,
    )

    missing_value, type_error = cli_commands._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": 1}
    )
    assert missing_value is None
    assert type_error == "stdlib_profile must be a string"

    invalid_value, choice_error = cli_commands._normalize_internal_batch_stdlib_profile(
        {"stdlib_profile": "nonsense"}
    )
    assert invalid_value is None
    assert choice_error == (
        "stdlib_profile must be one of "
        "'auto', 'micro', 'edge', 'standard', 'server', 'full'"
    )


def test_backend_fingerprint_reuses_stored_hash_when_inputs_unchanged(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "backend_source.rs"
    source.write_text("pub fn marker() {}\n")
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_source_paths",
        lambda _project_root, _features=(): [source],
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_rustc_version", lambda: "rustc-test", raising=True
    )

    baseline = cli_backend_binary._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
    )
    assert baseline is not None

    calls = 0
    original = FILE_HASHING._hash_source_tree_file

    def wrapped(path: Path, root: Path, hasher: object) -> None:
        nonlocal calls
        calls += 1
        original(path, root, hasher)

    monkeypatch.setattr(FILE_HASHING, "_hash_source_tree_file", wrapped, raising=True)
    reused = cli_backend_binary._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
        stored_fingerprint=baseline,
    )
    assert reused == baseline
    assert calls == 0


def test_backend_fingerprint_reuses_clean_source_state_without_metadata_scan(
    tmp_path: Path, monkeypatch
) -> None:
    source = tmp_path / "backend_source.rs"
    source.write_text("pub fn marker() {}\n")
    source_state = {
        "schema_version": 1,
        "kind": "git-clean-head",
        "head": "abc123",
    }
    monkeypatch.setattr(
        cli_backend_binary,
        "_backend_source_paths",
        lambda _project_root, _features=(): [source],
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary,
        "_compiler_clean_source_state",
        lambda _project_root: dict(source_state),
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_binary, "_rustc_version", lambda: "rustc-test", raising=True
    )

    baseline = cli_backend_binary._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
    )
    assert baseline is not None
    assert baseline["source_state"] == source_state

    def fail_metadata_scan(*_args, **_kwargs):
        raise AssertionError("clean source state should skip metadata scan")

    monkeypatch.setattr(
        cli_backend_binary,
        "_hash_source_tree_metadata",
        fail_metadata_scan,
        raising=True,
    )
    reused = cli_backend_binary._backend_fingerprint(
        tmp_path,
        cargo_profile="dev-fast",
        rustflags="",
        backend_features=(),
        stored_fingerprint=baseline,
    )
    assert reused == baseline


def test_clean_source_state_uses_single_unguarded_git_status(
    tmp_path: Path, monkeypatch
) -> None:
    COMPILER_METADATA._compiler_clean_source_state_cached.cache_clear()
    calls: list[dict[str, object]] = []

    def fake_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        calls.append({"cmd": cmd, **kwargs})
        return subprocess.CompletedProcess(
            cmd,
            0,
            "\n".join(
                [
                    "# branch.oid abc123",
                    "# branch.head main",
                    "# branch.upstream origin/main",
                    "# branch.ab +0 -0",
                ]
            ),
            "",
        )

    monkeypatch.setattr(
        COMPILER_METADATA, "_run_completed_command", fake_run, raising=True
    )

    try:
        state = COMPILER_METADATA._compiler_clean_source_state(tmp_path)
        again = COMPILER_METADATA._compiler_clean_source_state(tmp_path)
    finally:
        COMPILER_METADATA._compiler_clean_source_state_cached.cache_clear()

    assert state == {
        "schema_version": 1,
        "kind": "git-clean-head",
        "head": "abc123",
    }
    assert again == state
    assert len(calls) == 1
    assert calls[0]["cmd"] == [
        "git",
        "-C",
        str(tmp_path.resolve()),
        "status",
        "--porcelain=v2",
        "--branch",
        "--untracked-files=all",
    ]
    assert calls[0]["memory_guard_prefix"] is None
    assert calls[0]["timeout"] == 5.0


def test_clean_source_state_fails_closed_when_status_reports_changes(
    tmp_path: Path, monkeypatch
) -> None:
    COMPILER_METADATA._compiler_clean_source_state_cached.cache_clear()

    def fake_run(
        cmd: list[str],
        **kwargs: object,
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        return subprocess.CompletedProcess(
            cmd,
            0,
            "\n".join(
                [
                    "# branch.oid abc123",
                    "# branch.head main",
                    "? untracked.py",
                ]
            ),
            "",
        )

    monkeypatch.setattr(
        COMPILER_METADATA, "_run_completed_command", fake_run, raising=True
    )

    try:
        state = COMPILER_METADATA._compiler_clean_source_state(tmp_path)
    finally:
        COMPILER_METADATA._compiler_clean_source_state_cached.cache_clear()

    assert state is None


def test_rustc_version_is_cached(tmp_path: Path, monkeypatch) -> None:
    COMPILER_METADATA._rustc_version.cache_clear()
    monkeypatch.setattr(
        COMPILER_METADATA,
        "_rustc_version_cache_path",
        lambda identity_digest: tmp_path / f"rustc-{identity_digest}.json",
        raising=True,
    )
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
    third = COMPILER_METADATA._rustc_version()
    assert third == first
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
            "version": 3,
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
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _source_fingerprint("new"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_artifact_needs_rebuild",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
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
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(runtime_lib, None)
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(static_archive_bytes(b"runtime"))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        cargo_timeout=5.0,
        extra_runtime_features=("source_extension_loader",),
    )
    assert seen_cmds
    assert "--features" in seen_cmds[0]
    feature_index = seen_cmds[0].index("--features")
    features_str = seen_cmds[0][feature_index + 1]
    assert "molt_tk_native" in features_str.split(",")
    assert "source_extension_loader" in features_str.split(",")


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
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _source_fingerprint("new"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: None if path == fingerprint_path else None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "_artifact_needs_rebuild",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
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
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(runtime_lib, None)
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(static_archive_bytes(b"runtime"))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        cargo_timeout=0.1,
    )
    assert seen_cmds


def test_ensure_runtime_lib_records_runtime_stage_timings_on_cache_hit(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(static_archive_bytes(b"fake-staticlib"))
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    fingerprint = {
        "hash": "new",
        "rustc": "rustc-test",
        "inputs_digest": "digest",
        "meta_digest": "meta",
    }
    stage_timings_ms: dict[str, float] = {}

    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: dict(fingerprint),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: dict(fingerprint) if path == fingerprint_path else None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_artifact_fingerprint_matches",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_native_link_manifest_matches",
        lambda *args, **kwargs: True,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
        runtime_lib,
        target_triple=None,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        cargo_timeout=0.1,
        stage_timings_ms=stage_timings_ms,
    )

    assert set(stage_timings_ms) == {
        "runtime_lib_read_fingerprint",
        "runtime_lib_compute_fingerprint",
        "runtime_lib_artifact_match",
    }
    assert all(value >= 0.0 for value in stage_timings_ms.values())


def test_ensure_runtime_lib_rebuilds_when_stored_fingerprint_conflicts_with_requested_gpu_features(
    tmp_path: Path, monkeypatch
) -> None:
    runtime_lib = tmp_path / "target" / "dev-fast" / "libmolt_runtime.a"
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(static_archive_bytes(b"fake-staticlib"))
    project_root = tmp_path / "repo"
    project_root.mkdir()
    fingerprint_path = tmp_path / "runtime.fingerprint.json"
    source = tmp_path / "runtime_source.rs"
    source.write_text("pub fn marker() {}\n")
    stale_fingerprint = {
        **_source_fingerprint("stale"),
        "version": 2,
    }
    seen_cmds: list[list[str]] = []

    monkeypatch.setenv("MOLT_RUNTIME_GPU_METAL", "1")
    monkeypatch.setattr(
        RUNTIME_FINGERPRINTS,
        "runtime_source_paths",
        lambda _project_root, **_kwargs: [source],
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint",
        lambda *args, **kwargs: _source_fingerprint("new"),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_runtime_fingerprint_path",
        lambda *args, **kwargs: fingerprint_path,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_read_runtime_fingerprint",
        lambda path: dict(stale_fingerprint) if path == fingerprint_path else None,
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_build_lock",
        lambda *args, **kwargs: contextlib.nullcontext(),
        raising=True,
    )
    monkeypatch.setattr(
        RUNTIME_BUILD, "_maybe_enable_sccache", lambda _env: None, raising=True
    )
    monkeypatch.setattr(
        RUNTIME_BUILD,
        "_write_runtime_fingerprint",
        lambda *args, **kwargs: None,
        raising=True,
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
        scratch_lib = RUNTIME_PATHS._runtime_cargo_scratch_lib_path(runtime_lib, None)
        scratch_lib.parent.mkdir(parents=True, exist_ok=True)
        scratch_lib.write_bytes(static_archive_bytes(b"fake-staticlib"))
        return subprocess.CompletedProcess(cmd, 0, "", _NATIVE_STATICLIBS_NOTE)

    monkeypatch.setattr(
        RUNTIME_BUILD, "_run_cargo_with_sccache_retry", fake_run_cargo, raising=True
    )

    assert RUNTIME_BUILD._ensure_runtime_lib(
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
