import os
import time
from contextlib import contextmanager
import hashlib
import importlib
import json
from pathlib import Path
import subprocess
import sys
import traceback
from typing import Iterator, Mapping

from molt.backend_environment import CodegenSelection
from molt.cli import native_link_plan, native_symbol_inspection
import pytest

import molt.cli as cli
from molt.capability_manifest import resolve_runtime_policy_from_env
from molt.cli.native_main_stub import _render_native_main_stub
from molt.cli import backend_binary as cli_backend_binary
from molt.cli import backend_cache_setup as cli_backend_cache_setup
from molt.cli.backend_artifact_contract import resolve_backend_artifact_contract
from molt.cli.native_link_plan import resolve_native_target_spec
from molt.exact_json import canonical_json_sha256
from molt.cli.rust_cpu_policy import RustCpuPolicy
from tests.cli.process_guard import run_cli_test_process
from tests.process_guard_common import install_module_view
from tests.cli.native_link_test_support import (
    mock_symbol_reader_admission,
    static_archive_bytes,
)
from tests.native_artifact_fixtures import native_relocatable_object
from tests.compiler_identity_helper import (
    stub_compiler_admission,
    write_compiler_source,
)

# Key algebra consumes explicit content identities. Filesystem fingerprinting has
# separate producer tests; rereading the live compiler here both repeats expensive
# work and races concurrent source edits unrelated to the semantic key contract.
_KEY_FINGERPRINTS = {
    "cache_compiler_fingerprint": "compiler-fixture",
    "cache_tooling_fingerprint": "tooling-fixture",
}


ROOT = Path(__file__).resolve().parents[2]
BACKEND_CACHE = importlib.import_module("molt.cli.backend_cache")
BACKEND_EXECUTION = importlib.import_module("molt.cli.backend_execution")
CACHE_KEYS = importlib.import_module("molt.cli.cache_keys")


@pytest.fixture(autouse=True)
def _admit_mock_symbol_reader_commands(
    monkeypatch: pytest.MonkeyPatch, tmp_path_factory: pytest.TempPathFactory
) -> Iterator[None]:
    # Each test supplies a distinct external reader implementation.
    with mock_symbol_reader_admission(
        monkeypatch, tmp_path_factory.mktemp("symbol-facts")
    ):
        yield


@pytest.mark.parametrize(
    "matches,actual_key,evicted",
    [(False, "expected", True), (False, "other", False), (True, "expected", False)],
)
def test_stdlib_validation_and_eviction_share_publication_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, matches, actual_key, evicted
) -> None:
    artifact = tmp_path / "stdlib.a"
    _write_stdlib_archive_fixture(artifact, actual_key)
    events: list[str] = []
    locked = False

    @contextmanager
    def publication_lock(path):
        nonlocal locked
        assert path == artifact
        assert not locked
        locked = True
        events.append("lock")
        try:
            yield
        finally:
            events.append("unlock")
            locked = False
            # A replacement can be published as soon as the lock is released.
            artifact.write_bytes(b"next-generation")

    def validate(*args, **kwargs):
        assert locked
        events.append("validate")
        return matches

    def read_key(path):
        assert locked
        events.append("key")
        return actual_key

    def remove(path):
        assert locked
        events.append("evict")
        path.unlink()

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", publication_lock)
    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_matches_key", validate)
    monkeypatch.setattr(BACKEND_CACHE, "_read_stdlib_cache_key", read_key)
    monkeypatch.setattr(BACKEND_CACHE, "_remove_shared_stdlib_cache_artifacts", remove)
    assert (
        BACKEND_CACHE._validate_shared_stdlib_cache_contract(
            artifact, tmp_path, "expected", expected_manifest=_manifest("expected")
        )
        is matches
    )
    assert ("evict" in events) is evicted
    assert events[-1] == "unlock"
    assert artifact.read_bytes() == b"next-generation"


def _shared_stdlib_cleanup_paths(artifact: Path) -> tuple[Path, ...]:
    return (
        artifact,
        BACKEND_CACHE._stdlib_object_count_sidecar_path(artifact),
        BACKEND_CACHE._stdlib_object_key_sidecar_path(artifact),
        BACKEND_CACHE._stdlib_object_manifest_sidecar_path(artifact),
        BACKEND_CACHE._stdlib_object_partition_manifest_sidecar_path(artifact),
        BACKEND_CACHE._stdlib_object_digest_sidecar_path(artifact),
        BACKEND_CACHE._stdlib_object_symbol_contract_sidecar_path(artifact),
        native_symbol_inspection._native_object_symbol_facts_sidecar_path(artifact),
    )


def test_stdlib_cleanup_attempts_all_paths_and_reports_every_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact = tmp_path / "stdlib.a"
    paths = _shared_stdlib_cleanup_paths(artifact)
    for path in paths:
        path.write_bytes(b"owned-artifact")
    paths[2].unlink()  # Already absent is the only ignorable deletion failure.
    failures = {
        paths[0]: PermissionError("archive is held open"),
        paths[4]: OSError("partition metadata I/O failure"),
    }
    attempted: list[Path] = []
    unlink = Path.unlink

    def remove(path: Path, *args, **kwargs) -> None:
        attempted.append(path)
        if path in failures:
            raise failures[path]
        unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", remove)
    with pytest.raises(OSError, match="Failed to remove shared stdlib") as caught:
        BACKEND_CACHE._remove_shared_stdlib_cache_artifacts(artifact)
    assert attempted == list(paths)
    assert {path for path in paths if path.exists()} == set(failures)
    for path, failure in failures.items():
        assert str(path) in str(caught.value)
        assert str(failure) in str(caught.value)
    assert isinstance(caught.value.__cause__, ExceptionGroup)
    assert caught.value.__cause__.exceptions == tuple(failures.values())


def test_stdlib_cleanup_is_idempotent_for_missing_owned_artifacts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact = tmp_path / "stdlib.a"
    paths = _shared_stdlib_cleanup_paths(artifact)
    attempted: list[Path] = []
    unlink = Path.unlink

    def remove(path: Path, *args, **kwargs) -> None:
        attempted.append(path)
        unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", remove)
    BACKEND_CACHE._remove_shared_stdlib_cache_artifacts(artifact)
    BACKEND_CACHE._remove_shared_stdlib_cache_artifacts(artifact)
    assert attempted == [*paths, *paths]


@pytest.mark.parametrize("cleanup_fails", [False, True])
def test_stdlib_staging_preserves_primary_error_and_cleanup_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, cleanup_fails: bool
) -> None:
    source = tmp_path / "cache" / "stdlib.a"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"admitted archive")
    for sidecar in (
        BACKEND_CACHE._stdlib_object_key_sidecar_path,
        BACKEND_CACHE._stdlib_object_manifest_sidecar_path,
        BACKEND_CACHE._stdlib_object_partition_manifest_sidecar_path,
        BACKEND_CACHE._stdlib_object_digest_sidecar_path,
    ):
        sidecar(source).write_text("test metadata", encoding="utf-8")
    artifacts_root = tmp_path / "staged"
    staged = artifacts_root / "shared-stdlib-link" / source.name
    paths = _shared_stdlib_cleanup_paths(staged)
    primary = OSError("primary archive copy failure")
    cause = RuntimeError("underlying storage fault")
    attempted: list[Path] = []

    @contextmanager
    def publication_lock(path: Path):
        assert path in (source, staged)
        yield

    def copy(source_path: Path, destination: Path, *, identity) -> None:
        assert (source_path, destination) == (source, staged)
        raise primary from cause

    def remove(path: Path, *args, **kwargs) -> None:
        assert path.parent == staged.parent
        attempted.append(path)
        if cleanup_fails and path in (paths[0], paths[4]):
            raise PermissionError(f"cannot remove {path.name}")
        raise FileNotFoundError(path)

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", publication_lock)
    monkeypatch.setattr(
        BACKEND_CACHE, "_shared_stdlib_cache_matches_key", lambda *args, **kwargs: True
    )
    monkeypatch.setattr(BACKEND_CACHE, "_copy_verified_backend_artifact", copy)
    monkeypatch.setattr(Path, "unlink", remove)
    with pytest.raises(OSError, match="primary archive copy failure") as caught:
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            stdlib_object_cache_key="expected",
            stdlib_object_manifest="manifest",
            output_path=artifacts_root / "shared-stdlib-link" / source.name,
        )
    assert caught.value is primary
    assert primary.__cause__ is cause
    assert attempted == list(paths)
    rendered = "".join(traceback.format_exception(primary))
    assert "primary archive copy failure" in rendered
    assert "underlying storage fault" in rendered
    if cleanup_fails:
        assert len(primary.__notes__) == 1
        assert "staging cleanup also failed" in primary.__notes__[0]
        for path in (paths[0], paths[4]):
            assert str(path) in primary.__notes__[0]
            assert f"cannot remove {path.name}" in rendered
    else:
        assert not getattr(primary, "__notes__", ())


def test_stdlib_invalidation_propagates_cleanup_failure_under_publication_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    artifact = tmp_path / "stdlib.a"
    attempted: list[Path] = []
    locked = False

    @contextmanager
    def publication_lock(path: Path):
        nonlocal locked
        assert path == artifact
        locked = True
        try:
            yield
        finally:
            locked = False

    def remove(path: Path, *args, **kwargs) -> None:
        assert locked
        attempted.append(path)
        if path == artifact:
            raise PermissionError("corrupt archive cannot be removed")
        raise FileNotFoundError(path)

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", publication_lock)
    monkeypatch.setattr(
        BACKEND_CACHE, "_shared_stdlib_cache_matches_key", lambda *args, **kwargs: False
    )
    monkeypatch.setattr(
        BACKEND_CACHE, "_read_stdlib_cache_key", lambda path: "expected"
    )
    monkeypatch.setattr(Path, "unlink", remove)
    with pytest.raises(OSError, match="corrupt archive cannot be removed"):
        BACKEND_CACHE._validate_shared_stdlib_cache_contract(
            artifact, tmp_path, "expected", expected_manifest="manifest"
        )
    assert attempted == list(_shared_stdlib_cleanup_paths(artifact))
    assert not locked


def _cache_variant(
    backend_binary_identity: str = "",
    *,
    codegen_env: str = "codegen=v1",
) -> str:
    return cli_backend_cache_setup._build_cache_variant(
        profile="dev",
        runtime_cargo="dev-fast",
        backend_cargo="dev-fast",
        emit="native",
        stdlib_split=True,
        codegen_env=codegen_env,
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        backend_binary_identity=backend_binary_identity,
    )


def _explicit_stdlib_modules(*names: str) -> frozenset[str]:
    return frozenset(names)


def _ir_with_stdlib(*, user_ops: list[dict], stdlib_ops: list[dict]) -> dict:
    return {
        "profile": "dev-fast",
        "functions": [
            {"name": "molt_main", "params": [], "ops": user_ops},
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {"name": "molt_init_sys", "params": [], "ops": stdlib_ops},
        ],
    }


def _manifest(cache_key: str) -> str:
    return f'{{"cache_key":"{cache_key}"}}'


def _partition_manifest(name: str = "partition-a") -> str:
    return f'{{"body_hash":"{name}","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v2-exact-linkage-abi"}}'


def _write_object_digest_sidecar(stdlib_object: Path) -> None:
    cli._stdlib_object_digest_sidecar_path(stdlib_object).write_text(
        cli._sha256_file(stdlib_object) + "\n", encoding="utf-8"
    )


def _stdlib_archive_fixture(*, target_triple: str | None = None) -> bytes:
    return static_archive_bytes(
        native_relocatable_object(
            target_triple=target_triple, symbols=("molt_init_sys",)
        )
    )


def _write_stdlib_archive_fixture(
    path: Path, key: str, *, target_triple: str | None = None
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(_stdlib_archive_fixture(target_triple=target_triple))
    cli._stdlib_object_key_sidecar_path(path).write_text(key, encoding="utf-8")
    cli._stdlib_object_manifest_sidecar_path(path).write_text(
        _manifest(key), encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(path).write_text(
        _partition_manifest(), encoding="utf-8"
    )
    _write_object_digest_sidecar(path)


def _payload_digest_cache_key(
    payload_ir: Mapping[str, object],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
) -> str:
    payload_digest = hashlib.sha256(
        json.dumps(
            payload_ir,
            sort_keys=True,
            separators=(",", ":"),
            default=cli._json_ir_default,
        ).encode("utf-8")
    ).hexdigest()
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    return hashlib.sha256(
        b"payload-digest-v1"
        + b"|"
        + payload_digest.encode("utf-8")
        + b"|"
        + suffix.encode("utf-8")
        + b"|"
        + b"compiler-fixture"
        + b"|"
        + b"tooling-fixture"
        + b"|"
        + schema_version.encode("utf-8")
    ).hexdigest()


def test_shared_stdlib_cache_key_ignores_user_only_changes() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir_a = _ir_with_stdlib(
        user_ops=[{"kind": "const_int", "value": 1, "out": "v1"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[{"kind": "const_int", "value": 2, "out": "v1"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_dense_slot_rebinding_invalidates_app_keys_but_preserves_shared_payload() -> (
    None
):
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    first = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {"kind": "trace_enter_slot", "s_value": "molt_init_sys"},
            {"kind": "trace_exit"},
            {"kind": "ret_void"},
        ],
    )
    first["code_slots"] = {"molt_init_app": 0, "molt_init_sys": 1}
    rebound = {
        **first,
        "code_slots": {"molt_init_app": 1, "molt_init_sys": 0},
    }
    # The same reusable bodies can link into either dense assignment, but a
    # final application cache hit must never retain the other assignment.
    assert first["functions"] == rebound["functions"]
    for key_builder in (CACHE_KEYS._cache_key, CACHE_KEYS._function_cache_key):
        keys = [
            key_builder(
                program,
                "native",
                None,
                variant,
                compiler_fingerprint="compiler-fixture",
                tooling_fingerprint="tooling-fixture",
            )
            for program in (first, rebound)
        ]
        assert keys[0] != keys[1], key_builder.__name__

    payloads = [
        cli._shared_stdlib_cache_payload_ir(
            program,
            entry_module="app",
            stdlib_module_symbols=stdlib_modules,
            compiler_fingerprint="compiler-fingerprint",
        )
        for program in (first, rebound)
    ]
    assert payloads[0] == payloads[1]
    assert payloads[0]["functions"] == [first["functions"][-1]]
    assert "code_slots" not in payloads[0]
    keys = [
        cli._shared_stdlib_cache_key(
            program,
            **_KEY_FINGERPRINTS,
            entry_module="app",
            stdlib_module_symbols=stdlib_modules,
            target_triple=None,
            cache_variant=variant,
            compiler_fingerprint="compiler-fingerprint",
        )
        for program in (first, rebound)
    ]
    assert keys[0] == keys[1]


def test_shared_stdlib_cache_key_uses_payload_digest_authority() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    payload_ir = cli._shared_stdlib_cache_payload_ir(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        compiler_fingerprint="compiler-fingerprint",
    )
    assert cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple="aarch64-apple-darwin",
        cache_variant=variant,
        compiler_fingerprint="compiler-fingerprint",
    ) == _payload_digest_cache_key(
        payload_ir,
        target="native-stdlib",
        target_triple="aarch64-apple-darwin",
        variant=variant,
        schema_version=CACHE_KEYS._CACHE_KEY_SCHEMA_VERSION,
    )


def test_shared_stdlib_cache_key_changes_with_stdlib_payload_and_target() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir_a = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_json",
                "args": ["code", "globals"],
            }
        ],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_c = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple="aarch64-apple-darwin",
        cache_variant=variant,
    )

    assert key_a != key_b
    assert key_a != key_c
    assert cli._stdlib_object_cache_path(
        Path("cache"), key_a
    ) != cli._stdlib_object_cache_path(Path("cache"), key_b)


def test_shared_stdlib_cache_key_changes_with_compiler_fingerprint() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-a",
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-b",
    )

    assert key_a != key_b


@pytest.mark.parametrize(
    "changed_env",
    [
        {"MOLT_CAPABILITIES": "fs.read"},
        {"MOLT_CAPABILITY_TIER": "full"},
        {"MOLT_RESOURCE_MAX_MEMORY": "1MB"},
        {"MOLT_AUDIT_ENABLED": "1", "MOLT_AUDIT_SINK": "jsonl"},
        {"MOLT_IO_MODE": "virtual"},
    ],
)
def test_backend_code_reuse_keeps_final_runtime_policies_distinct(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    changed_env: dict[str, str],
) -> None:
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
        cache_enabled=True,
        ir=ir,
        target="native",
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="bin", target_triple=None
        ),
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(tmp_path / "cache"),
        warnings=[],
        entry_module="app",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="micro",
        # Compiler identity is an independent input, fixed here to avoid the
        # Rust toolchain while exercising the real backend cache setup.
        backend_compiler_fingerprint="test-compiler",
    )

    base_env = {"MOLT_CAPABILITY_TIER": "none", "MOLT_CAPABILITIES": ""}
    base_policy = resolve_runtime_policy_from_env(base_env)
    changed_policy = resolve_runtime_policy_from_env({**base_env, **changed_env})
    assert base_policy != changed_policy
    for key, value in base_policy.to_env_vars().items():
        monkeypatch.setenv(key, value)
    setup_base = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        output_artifact=tmp_path / "base.o",
        **common,
    )
    for key, value in changed_policy.to_env_vars().items():
        monkeypatch.setenv(key, value)
    setup_caps = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        output_artifact=tmp_path / "caps.o",
        **common,
    )

    for field in ("cache_key", "function_cache_key", "stdlib_object_cache_key"):
        assert getattr(setup_base, field) is not None
        assert getattr(setup_base, field) == getattr(setup_caps, field)
    # A cache hit never freezes the policy into shared machine code. The final
    # native link consumes the freshly rendered entrypoint, with current policy.
    assert _render_native_main_stub(
        resolved_capability_policy=base_policy,
    ) != _render_native_main_stub(resolved_capability_policy=changed_policy)


def test_prepare_backend_cache_setup_reuses_cache_fingerprints_for_backend_keys(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    calls = {"compiler": 0, "tooling": 0}
    payload_digest_calls = 0
    function_payload_digests: list[str | None] = []
    function_tooling_fingerprints: list[str | None] = []
    stage_timings_ms: dict[str, float] = {}
    compiler_backend_features: list[tuple[str, ...]] = []
    compiler_include_runtime_sources: list[object] = []

    def compiler_fingerprint(**kwargs: object) -> str:
        calls["compiler"] += 1
        backend_features = kwargs.get("backend_features")
        runtime_features = kwargs.get("runtime_features")
        assert backend_features is not None
        compiler_backend_features.append(tuple(backend_features))  # type: ignore[arg-type]
        assert runtime_features is None
        compiler_include_runtime_sources.append(kwargs.get("include_runtime_sources"))
        return "compiler-fingerprint"

    def unexpected_fingerprint_call() -> str:
        raise AssertionError("backend cache setup did not reuse fingerprint inputs")

    original_payload_digest = cli_backend_cache_setup._cache_payload_digest
    original_function_cache_key = cli_backend_cache_setup._function_cache_key

    def counting_payload_digest(payload_ir: dict[str, object]) -> str:
        nonlocal payload_digest_calls
        payload_digest_calls += 1
        return original_payload_digest(payload_ir)

    def recording_function_cache_key(*args: object, **kwargs: object) -> str:
        payload_digest = kwargs.get("payload_digest")
        tooling_fingerprint = kwargs.get("tooling_fingerprint")
        function_payload_digests.append(
            payload_digest if isinstance(payload_digest, str) else None
        )
        function_tooling_fingerprints.append(
            tooling_fingerprint if isinstance(tooling_fingerprint, str) else None
        )
        return original_function_cache_key(*args, **kwargs)

    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_cache_fingerprint",
        compiler_fingerprint,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_cache_payload_digest",
        counting_payload_digest,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_function_cache_key",
        recording_function_cache_key,
        raising=True,
    )
    monkeypatch.setattr(
        CACHE_KEYS,
        "_cache_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )
    monkeypatch.setattr(
        CACHE_KEYS,
        "_cache_tooling_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_cache_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_cache_tooling_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )

    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )

    setup = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        cache_enabled=True,
        ir=ir,
        target="native",
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="bin", target_triple=None
        ),
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(tmp_path / "cache"),
        output_artifact=tmp_path / "out.o",
        warnings=[],
        entry_module="app",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="micro",
        stage_timings_ms=stage_timings_ms,
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
    )

    assert setup.cache_key is not None
    assert setup.function_cache_key is not None
    assert setup.stdlib_object_cache_key is not None
    assert calls == {"compiler": 1, "tooling": 0}
    assert compiler_backend_features == [("native-backend",)]
    assert compiler_include_runtime_sources == [False]
    assert payload_digest_calls == 1
    assert function_payload_digests
    assert function_payload_digests[0] is not None
    assert function_tooling_fingerprints == [
        CACHE_KEYS._BACKEND_IR_PAYLOAD_TOOLING_FINGERPRINT
    ]
    assert {
        "backend_cache_stdlib_symbols",
        "backend_cache_backend_binary_identity",
        "backend_cache_variant",
        "backend_cache_payload_digest",
        "backend_cache_fingerprint_inputs",
        "backend_cache_module_key",
        "backend_cache_function_key",
        "backend_cache_root",
        "backend_cache_stdlib_key_manifest",
        "backend_cache_candidate_paths",
        "backend_cache_stdlib_contract",
        "backend_cache_try_candidates",
    }.issubset(stage_timings_ms)
    assert all(value >= 0.0 for value in stage_timings_ms.values())


def test_prepare_backend_cache_setup_custodies_free_threaded_mode_end_to_end(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    selected = ("native-backend",)
    seen: list[tuple[str, ...]] = []

    def selected_features(
        *, target: str, is_wasm: bool, codegen_backend: str
    ) -> tuple[str, ...]:
        assert target == "native"
        assert not is_wasm
        assert codegen_backend == "cranelift"
        return selected

    def compiler_fingerprint(**kwargs: object) -> str:
        features = tuple(kwargs["backend_features"])  # type: ignore[arg-type]
        seen.append(features)
        return "compiler-" + ",".join(features)

    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_backend_features_for_build_target",
        selected_features,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_cache_fingerprint",
        compiler_fingerprint,
    )
    ir = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
        cache_enabled=True,
        ir=ir,
        target="native",
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="bin", target_triple=None
        ),
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        project_root=tmp_path,
        cache_dir=str(tmp_path / "cache"),
        warnings=[],
        entry_module="app",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="micro",
    )

    default = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        output_artifact=tmp_path / "default.o",
        **common,
    )
    selected = ("native-backend", "free-threaded")
    free_threaded = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        output_artifact=tmp_path / "free-threaded.o",
        **common,
    )

    assert default.cache_key != free_threaded.cache_key
    assert default.function_cache_key != free_threaded.function_cache_key
    assert default.stdlib_object_cache_key != free_threaded.stdlib_object_cache_key
    assert seen == [
        ("native-backend",),
        ("native-backend", "free-threaded"),
    ]


def test_prepare_backend_cache_setup_caches_stdlib_key_material(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    build_state_root = tmp_path / "build-state"
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_build_state_root",
        lambda _project_root: build_state_root,
        raising=True,
    )
    key_calls = 0
    manifest_calls = 0
    original_key = cli_backend_cache_setup._shared_stdlib_cache_key
    original_manifest = cli_backend_cache_setup._shared_stdlib_manifest

    def counting_key(*args: object, **kwargs: object) -> str:
        nonlocal key_calls
        key_calls += 1
        return original_key(*args, **kwargs)

    def counting_manifest(*args: object, **kwargs: object) -> str | None:
        nonlocal manifest_calls
        manifest_calls += 1
        return original_manifest(*args, **kwargs)

    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_shared_stdlib_cache_key",
        counting_key,
        raising=True,
    )
    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_shared_stdlib_manifest",
        counting_manifest,
        raising=True,
    )
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        codegen=CodegenSelection(),
        rust_cpu=RustCpuPolicy.BASELINE,
        cache_enabled=True,
        ir=ir,
        target="native",
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="bin", target_triple=None
        ),
        profile="dev",
        runtime_cargo_profile="dev-fast",
        backend_cargo_profile="dev-fast",
        emit_mode="bin",
        is_wasm=False,
        linked=False,
        project_root=ROOT,
        cache_dir=str(tmp_path / "cache"),
        output_artifact=tmp_path / "out.o",
        warnings=[],
        entry_module="app",
        module_graph_metadata=module_graph_metadata,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="micro",
    )

    first_timings: dict[str, float] = {}
    first = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        stage_timings_ms=first_timings,
        **common,
    )
    second_timings: dict[str, float] = {}
    second = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend",
        stage_timings_ms=second_timings,
        **common,
    )

    assert first.stdlib_object_cache_key == second.stdlib_object_cache_key
    assert first.stdlib_object_manifest == second.stdlib_object_manifest
    assert key_calls == 1
    assert manifest_calls == 2
    paths = list(
        (build_state_root / "backend_cache_stdlib_key_material").rglob("*.json")
    )
    assert len(paths) == 1
    stored = json.loads(paths[0].read_text(encoding="utf-8"))
    assert "manifest" not in stored
    # A leftover memo projection cannot override the current archive contract.
    stored["manifest"] = "stale contract"
    paths[0].write_text(json.dumps(stored), encoding="utf-8")
    third = cli_backend_cache_setup._prepare_backend_cache_setup(
        backend_bin=tmp_path / "molt-backend", **common
    )
    assert third.stdlib_object_cache_key == first.stdlib_object_cache_key
    assert third.stdlib_object_manifest == first.stdlib_object_manifest
    assert key_calls == 1
    assert manifest_calls == 3
    assert "backend_cache_stdlib_key_manifest" in second_timings


def test_prepare_backend_cache_setup_uses_verified_backend_compiler_fingerprint(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def unexpected_fingerprint_call(*args: object, **kwargs: object) -> str:
        del args, kwargs
        raise AssertionError("verified backend fingerprint should be reused")

    monkeypatch.setattr(
        cli_backend_cache_setup,
        "_cache_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )
    monkeypatch.setattr(
        CACHE_KEYS,
        "_cache_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_cache_fingerprint",
        unexpected_fingerprint_call,
        raising=True,
    )

    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        module_execution_kind_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )

    def prepare(*, backend_compiler_fingerprint: str, output_name: str):
        return cli_backend_cache_setup._prepare_backend_cache_setup(
            backend_bin=tmp_path / "molt-backend",
            cache_enabled=True,
            ir=ir,
            target="native",
            artifact_contract=resolve_backend_artifact_contract(
                target="native", emit_mode="bin", target_triple=None
            ),
            profile="dev",
            runtime_cargo_profile="dev-fast",
            backend_cargo_profile="dev-fast",
            emit_mode="bin",
            is_wasm=False,
            linked=False,
            project_root=ROOT,
            cache_dir=str(tmp_path / "cache"),
            output_artifact=tmp_path / output_name,
            warnings=[],
            entry_module="app",
            module_graph_metadata=module_graph_metadata,
            target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
            stdlib_profile="micro",
            backend_compiler_fingerprint=backend_compiler_fingerprint,
            codegen=CodegenSelection(),
            rust_cpu=RustCpuPolicy.BASELINE,
        )

    setup_a = prepare(
        backend_compiler_fingerprint="verified-backend-compiler-a",
        output_name="a.o",
    )
    setup_b = prepare(
        backend_compiler_fingerprint="verified-backend-compiler-b",
        output_name="b.o",
    )

    assert setup_a.cache_key is not None
    assert setup_b.cache_key is not None
    assert setup_a.stdlib_object_cache_key is not None
    assert setup_b.stdlib_object_cache_key is not None
    assert setup_a.cache_key != setup_b.cache_key
    assert setup_a.stdlib_object_cache_key != setup_b.stdlib_object_cache_key


def test_shared_stdlib_cache_key_ignores_non_stdlib_top_level_extras() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir_a = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    ir_a["entry_metadata"] = {"driver": "stage5"}
    ir_b["entry_metadata"] = {"driver": "stage8"}

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_tracks_full_stdlib_module_partition() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys", "json")
    ir_a = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_sys",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_sys",
                        "args": ["code", "globals"],
                    }
                ],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_json",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }
    ir_b = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_json"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_sys",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_sys",
                        "args": ["code", "globals"],
                    }
                ],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_json",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_ignores_function_order_when_reachable_set_matches() -> (
    None
):
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys", "json")
    shared_stdlib = [
        {
            "name": "molt_init_sys",
            "params": [],
            "ops": [{"kind": "call_internal", "s_value": "molt_init_json"}],
        },
        {
            "name": "molt_init_json",
            "params": [],
            "ops": [
                {
                    "kind": "code_slot_set",
                    "s_value": "molt_init_sys",
                    "args": ["code", "globals"],
                }
            ],
        },
    ]
    ir_a = {
        "profile": "dev-fast",
        "entry_metadata": {"driver": "stage5"},
        "functions": [
            {"name": "helper_a", "params": [], "ops": []},
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            *shared_stdlib,
        ],
    }
    ir_b = {
        "profile": "dev-fast",
        "entry_metadata": {"driver": "stage8"},
        "functions": [
            {"name": "helper_b", "params": [], "ops": []},
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            *shared_stdlib,
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_changes_with_any_stdlib_module_body_change() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys", "json")
    ir_a = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_sys",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_sys",
                        "args": ["code", "globals"],
                    }
                ],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_json",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }
    ir_b = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_sys",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_sys",
                        "args": ["code", "globals"],
                    }
                ],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_changed",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_tracks_sanitized_stdlib_module_symbols() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("importlib_abc")
    ir_a = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [
                    {"kind": "call_internal", "s_value": "molt_init_importlib_abc"}
                ],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_importlib_abc",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_sys",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }
    ir_b = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [
                    {"kind": "call_internal", "s_value": "molt_init_importlib_abc"}
                ],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {
                "name": "molt_init_importlib_abc",
                "params": [],
                "ops": [
                    {
                        "kind": "code_slot_set",
                        "s_value": "molt_init_json",
                        "args": ["code", "globals"],
                    }
                ],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_tracks_stdlib_module_symbol_set() -> None:
    variant = _cache_variant()
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=_explicit_stdlib_modules("sys"),
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=_explicit_stdlib_modules(
            "importlib", "importlib_machinery", "importlib_util", "sys"
        ),
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_tracks_importlib_runtime_support_modules() -> None:
    variant = _cache_variant()
    ir = {
        "profile": "dev-fast",
        "functions": [
            {
                "name": "molt_main",
                "params": [],
                "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}],
            },
            {"name": "molt_init_app", "params": [], "ops": []},
            {"name": "app__module", "params": [], "ops": []},
            {"name": "molt_init_sys", "params": [], "ops": []},
            {"name": "molt_init_importlib", "params": [], "ops": []},
            {"name": "molt_init_importlib_util", "params": [], "ops": []},
            {"name": "molt_init_importlib_machinery", "params": [], "ops": []},
        ],
    }
    base_symbols = _explicit_stdlib_modules("sys")
    runtime_support_symbols = _explicit_stdlib_modules(
        "importlib", "importlib_machinery", "importlib_util", "sys"
    )

    key_base = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=base_symbols,
        target_triple=None,
        cache_variant=variant,
    )
    key_support = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=runtime_support_symbols,
        target_triple=None,
        cache_variant=variant,
    )
    payload = cli._shared_stdlib_cache_payload_ir(
        ir,
        entry_module="app",
        stdlib_module_symbols=runtime_support_symbols,
        compiler_fingerprint=BACKEND_CACHE._shared_stdlib_compiler_fingerprint(
            **_KEY_FINGERPRINTS
        ),
    )
    function_names = {func["name"] for func in payload["functions"]}

    assert key_base != key_support
    assert {
        "molt_init_importlib",
        "molt_init_importlib_util",
        "molt_init_importlib_machinery",
    } <= function_names


def test_shared_stdlib_cache_matches_key_requires_present_matching_contract(
    tmp_path: Path,
) -> None:
    stdlib_object = tmp_path / "stdlib_shared_test.a"
    stdlib_object.write_bytes(_stdlib_archive_fixture())

    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    key_sidecar = cli._stdlib_object_key_sidecar_path(stdlib_object)
    key_sidecar.write_text("wrong-key\n", encoding="utf-8")
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    key_sidecar.write_text("abc123\n", encoding="utf-8")
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    manifest_sidecar = cli._stdlib_object_manifest_sidecar_path(stdlib_object)
    manifest_sidecar.write_text(_manifest("wrong-key") + "\n", encoding="utf-8")
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    manifest_sidecar.write_text(_manifest("abc123") + "\n", encoding="utf-8")
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    _write_object_digest_sidecar(stdlib_object)
    assert cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )

    stdlib_object.write_bytes(b"changed")
    assert not cli._shared_stdlib_cache_matches_key(
        stdlib_object,
        "abc123",
        stdlib_object_manifest=_manifest("abc123"),
    )


def test_ensure_backend_binary_preserves_repo_local_shared_stdlib_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, runtime_fixture_root
) -> None:
    stub_compiler_admission(monkeypatch)
    write_compiler_source(tmp_path)
    project_root = tmp_path
    cache_root = project_root / ".molt_cache"
    home_bin = tmp_path / "molt-home" / "bin"
    cache_root.mkdir(parents=True)
    home_bin.mkdir(parents=True)
    home_bin_sentinel = home_bin / "preserve-me"
    home_bin_sentinel.write_text("owned by another session\n", encoding="utf-8")
    stdlib_object = cache_root / "stdlib_shared_deadbeef.o"
    stdlib_object.write_bytes(b"shared-stdlib")
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "2073\n", encoding="utf-8"
    )
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "stdlib-cache-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("stdlib-cache-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(stdlib_object)
    module_object = cache_root / "module_cache_old.o"
    module_object.write_bytes(b"module-object")
    wasm_object = cache_root / "module_cache_old.wasm"
    wasm_object.write_bytes(b"\x00asm")
    fingerprint_sidecar = cache_root / "module_cache_old.fingerprint"
    fingerprint_sidecar.write_text("old-fingerprint\n", encoding="utf-8")

    exe_suffix = ".exe" if os.name == "nt" else ""
    backend_bin = tmp_path / "target" / "dev-fast" / f"molt-backend{exe_suffix}"
    fingerprint = {
        "hash": canonical_json_sha256("shared-stdlib-backend-source"),
        "rustc": "rustc",
        "inputs_digest": canonical_json_sha256("shared-stdlib-backend-inputs"),
    }
    build_cmds: list[list[str]] = []

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fake_run_cargo(plan, **kwargs: object) -> subprocess.CompletedProcess[str]:
        del kwargs
        plan.verify()
        cmd = list(plan.command)
        build_cmds.append(cmd)
        runtime_fixture_root.native_executable(
            backend_bin.relative_to(tmp_path).as_posix()
        )
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("MOLT_HOME", str(home_bin.parent))
    monkeypatch.setattr(
        cli_backend_binary, "_backend_fingerprint", fake_backend_fingerprint
    )
    monkeypatch.setattr(cli_backend_binary, "_run_resolved_cargo_plan", fake_run_cargo)
    monkeypatch.setattr(
        cli_backend_binary,
        "_run_subprocess_captured_to_tempfiles",
        lambda cmd, **kwargs: subprocess.CompletedProcess(cmd, 0, b"", b""),
    )

    assert cli_backend_binary._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
        rust_cpu=RustCpuPolicy.BASELINE,
    )
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--locked",
            "--package",
            "molt-backend",
            "--bin",
            "molt-backend",
            "--profile",
            "dev-fast",
            "--no-default-features",
            "--features",
            "native-backend",
        ]
    ]
    assert stdlib_object.exists()
    assert cli._stdlib_object_count_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_key_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_manifest_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).exists()
    assert module_object.exists()
    assert wasm_object.exists()
    assert fingerprint_sidecar.exists()
    assert home_bin_sentinel.exists()


def test_validate_shared_stdlib_cache_contract_preserves_matching_key_despite_newer_artifacts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    cache_root = project_root / ".molt_cache"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    cache_root.mkdir(parents=True)

    stdlib_object = cache_root / "stdlib_shared_active.a"
    stdlib_object.write_bytes(_stdlib_archive_fixture())
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "active-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("active-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "1\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(stdlib_object)

    backend_bin = target_root / "dev-fast" / "molt-backend"
    runtime_lib = target_root / "release-output" / "libmolt_runtime.a"
    for artifact in (backend_bin, runtime_lib):
        artifact.parent.mkdir(parents=True, exist_ok=True)
        artifact.write_bytes(b"artifact")

    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    old = 1_700_000_000.0
    current = old + 10.0
    os.utime(stdlib_object, (old, old))
    os.utime(backend_bin, (current, current))
    os.utime(runtime_lib, (current, current))

    cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        project_root,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
    )

    assert stdlib_object.exists()
    assert cli._stdlib_object_key_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_count_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_manifest_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).exists()


def test_validate_shared_stdlib_cache_contract_reuses_symbol_contract_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    stdlib_object = tmp_path / ".molt_cache" / "stdlib_shared_active.o"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(_stdlib_archive_fixture())
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "active-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("active-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest("partition-a") + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(stdlib_object)
    calls = 0

    def fake_symbol_closure_issue(*args: object, **kwargs: object) -> None:
        nonlocal calls
        del args, kwargs
        calls += 1
        return None

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_native_symbol_closure_issue",
        fake_symbol_closure_issue,
    )

    assert cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        tmp_path,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
        stdlib_module_symbols={"sys"},
    )
    assert calls == 1
    assert BACKEND_CACHE._stdlib_object_symbol_contract_sidecar_path(
        stdlib_object
    ).exists()

    stage_timings_ms: dict[str, float] = {}
    assert cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        tmp_path,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
        stdlib_module_symbols={"sys"},
        stage_timings_ms=stage_timings_ms,
    )
    assert calls == 1
    assert "backend_cache_stdlib_contract_symbol_token" in stage_timings_ms
    assert "backend_cache_stdlib_contract_symbols" not in stage_timings_ms

    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest("partition-b") + "\n", encoding="utf-8"
    )
    assert cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        tmp_path,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
        stdlib_module_symbols={"sys"},
    )
    assert calls == 2


def test_validate_shared_stdlib_cache_contract_preserves_matching_key_despite_target_runtime_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    # The host's own object format: every host's native symbol reader must be
    # able to read the archive. A cross-format archive (Mach-O on a Linux host
    # with only GNU nm) correctly fails closed in symbol inspection.
    target_triple = resolve_native_target_spec(None).triple
    cache_root = project_root / ".molt_cache"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    cache_root.mkdir(parents=True)

    stdlib_object = cache_root / "stdlib_shared_target.a"
    stdlib_object.write_bytes(_stdlib_archive_fixture(target_triple=target_triple))
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "target-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("target-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "1\n", encoding="utf-8"
    )
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(stdlib_object)

    runtime_lib = (
        target_root / target_triple / "release-output" / "libmolt_runtime.stdlib_full.a"
    )
    runtime_lib.parent.mkdir(parents=True, exist_ok=True)
    runtime_lib.write_bytes(b"artifact")

    monkeypatch.delenv("MOLT_SESSION_ID", raising=False)
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root))

    old = 1_700_000_000.0
    current = old + 10.0
    os.utime(stdlib_object, (old, old))
    os.utime(runtime_lib, (current, current))

    cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        project_root,
        expected_key="target-key",
        expected_manifest=_manifest("target-key"),
        target_triple=target_triple,
    )

    assert stdlib_object.exists()
    assert cli._stdlib_object_key_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_count_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_manifest_sidecar_path(stdlib_object).exists()
    assert cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).exists()


def test_validate_shared_stdlib_cache_contract_preserves_other_keyed_siblings(
    tmp_path: Path,
) -> None:
    cache_root = tmp_path / ".molt_cache"
    cache_root.mkdir(parents=True)

    active = cache_root / "stdlib_shared_active.a"
    active.write_bytes(_stdlib_archive_fixture())
    cli._stdlib_object_key_sidecar_path(active).write_text(
        "active-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(active).write_text(
        _manifest("active-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(active).write_text("1\n", encoding="utf-8")
    cli._stdlib_object_partition_manifest_sidecar_path(active).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(active)

    sibling = cache_root / "stdlib_shared_sibling.o"
    sibling.write_bytes(b"sibling")
    cli._stdlib_object_key_sidecar_path(sibling).write_text(
        "sibling-key\n", encoding="utf-8"
    )
    cli._stdlib_object_manifest_sidecar_path(sibling).write_text(
        _manifest("sibling-key") + "\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(sibling).write_text("2\n", encoding="utf-8")
    cli._stdlib_object_partition_manifest_sidecar_path(sibling).write_text(
        _partition_manifest("partition-b") + "\n", encoding="utf-8"
    )
    _write_object_digest_sidecar(sibling)

    cli._validate_shared_stdlib_cache_contract(
        active,
        project_root=tmp_path,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
    )

    assert active.exists()
    assert sibling.exists()
    assert cli._stdlib_object_key_sidecar_path(sibling).exists()
    assert cli._stdlib_object_count_sidecar_path(sibling).exists()
    assert cli._stdlib_object_manifest_sidecar_path(sibling).exists()
    assert cli._stdlib_object_partition_manifest_sidecar_path(sibling).exists()


def test_validate_shared_stdlib_cache_contract_does_not_unlink_mismatched_exact_key_entry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    stdlib_object = tmp_path / "stdlib_shared_active.a"
    stdlib_object.write_bytes(_stdlib_archive_fixture())
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "wrong-key\n", encoding="utf-8"
    )

    def fail_remove(path: Path) -> None:
        raise AssertionError(f"unexpected stdlib cache deletion: {path}")

    monkeypatch.setattr(cli, "_remove_shared_stdlib_cache_artifacts", fail_remove)

    cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        project_root=tmp_path,
        expected_key="active-key",
        expected_manifest=_manifest("active-key"),
    )

    assert stdlib_object.exists()
    assert cli._stdlib_object_key_sidecar_path(stdlib_object).exists()


def test_shared_stdlib_cache_publish_lock_excludes_competing_process(
    tmp_path: Path,
) -> None:
    if os.name != "posix":
        pytest.skip("publish-lock exclusion uses POSIX flock")
    stdlib_object = tmp_path / "stdlib_shared_active.a"
    stdlib_object.write_bytes(_stdlib_archive_fixture())
    key = "active-key"
    manifest = _manifest(key)
    partition_manifest = _partition_manifest()
    env = os.environ.copy()
    env["PYTHONPATH"] = str(ROOT / "src")

    probe_code = r"""
import fcntl
import os
from pathlib import Path
import sys
import molt.cli as cli

from molt.default_paths import shared_stdlib_publish_lock_path
lock_path = shared_stdlib_publish_lock_path(Path(sys.argv[1]))
lock_path.parent.mkdir(parents=True, exist_ok=True)
fd = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o666)
try:
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except BlockingIOError:
        print("blocked")
        raise SystemExit(75)
    print("acquired")
finally:
    os.close(fd)
"""

    with cli._shared_stdlib_cache_lock(stdlib_object):
        blocked = run_cli_test_process(
            [sys.executable, "-c", probe_code, str(stdlib_object)],
            cwd=str(ROOT),
            env=env,
            check=False,
            capture_output=True,
            text=True,
        )
        assert blocked.returncode == 75, (blocked.stdout, blocked.stderr)
        assert blocked.stdout.strip() == "blocked"
        cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
            key + "\n", encoding="utf-8"
        )
        cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
            "1\n", encoding="utf-8"
        )
        cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
            manifest + "\n", encoding="utf-8"
        )
        cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
            partition_manifest + "\n", encoding="utf-8"
        )
        _write_object_digest_sidecar(stdlib_object)

    acquired = run_cli_test_process(
        [sys.executable, "-c", probe_code, str(stdlib_object)],
        cwd=str(ROOT),
        env=env,
        check=False,
        capture_output=True,
        text=True,
    )
    assert acquired.returncode == 0, (acquired.stdout, acquired.stderr)
    assert acquired.stdout.strip() == "acquired"

    cli._validate_shared_stdlib_cache_contract(
        stdlib_object,
        project_root=tmp_path,
        expected_key=key,
        expected_manifest=manifest,
    )

    assert stdlib_object.exists()
    assert (
        cli._stdlib_object_key_sidecar_path(stdlib_object)
        .read_text(encoding="utf-8")
        .strip()
        == key
    )
    assert (
        cli._stdlib_object_manifest_sidecar_path(stdlib_object)
        .read_text(encoding="utf-8")
        .strip()
        == manifest
    )
    assert cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).exists()


def test_try_cached_backend_candidates_skips_mismatched_stdlib_without_unlink(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    stdlib_object = tmp_path / ".molt_cache" / f"stdlib_shared_active{contract.suffix}"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(b"stdlib")
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "wrong-key\n", encoding="utf-8"
    )
    candidate = tmp_path / ".molt_cache" / f"module_cache{contract.suffix}"
    candidate.write_bytes(
        static_archive_bytes(
            native_relocatable_object(
                target_triple=contract.target_triple, symbols=("molt_main",)
            )
        )
    )
    output_artifact = tmp_path / "target" / f"out{contract.suffix}"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(b"old")
    warnings: list[str] = []

    def fail_remove(path: Path) -> None:
        raise AssertionError(f"unexpected stdlib cache deletion: {path}")

    monkeypatch.setattr(cli, "_remove_shared_stdlib_cache_artifacts", fail_remove)

    cache_hit, tier = cli._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=(("module", candidate),),
        output_artifact=output_artifact,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="active-key",
        stdlib_object_manifest=_manifest("active-key"),
        warnings=warnings,
    )

    assert (cache_hit, tier) == (False, None)
    assert any("mismatched contract" in warning for warning in warnings)
    assert stdlib_object.exists()
    assert cli._stdlib_object_key_sidecar_path(stdlib_object).exists()


@pytest.mark.parametrize("same_root", [False, True])
def test_stdlib_snapshot_pins_generation_until_after_publication_unlock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, same_root: bool
) -> None:
    artifacts = tmp_path / "build"
    source = (artifacts if same_root else tmp_path / "cache") / "stdlib.a"
    source.parent.mkdir(parents=True)
    source.write_bytes(b"admitted archive")
    for path_function, payload in (
        (BACKEND_CACHE._stdlib_object_key_sidecar_path, "expected"),
        (BACKEND_CACHE._stdlib_object_count_sidecar_path, "1"),
        (BACKEND_CACHE._stdlib_object_manifest_sidecar_path, "manifest"),
        (BACKEND_CACHE._stdlib_object_partition_manifest_sidecar_path, "partition"),
        (BACKEND_CACHE._stdlib_object_digest_sidecar_path, "digest"),
    ):
        path_function(source).write_text(payload, encoding="utf-8")
    locked = False
    validations = 0
    original_copy = BACKEND_CACHE._copy_verified_backend_artifact

    @contextmanager
    def publication_lock(path):
        nonlocal locked
        if path != source:
            assert path == artifacts / "shared-stdlib-link" / source.name
            assert locked
            yield
            return
        locked = True
        try:
            yield
        finally:
            locked = False
            replacement = source.with_name("next-generation.a")
            replacement.write_bytes(b"replacement archive")
            replacement.replace(source)

    def validate(*args, **kwargs):
        nonlocal validations
        assert locked
        validations += 1
        return True

    def copy(source_path, destination, *, identity):
        assert locked
        assert source_path.resolve() != destination.resolve()
        result = original_copy(source_path, destination, identity=identity)
        assert not source_path.samefile(destination)
        return result

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", publication_lock)
    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_matches_key", validate)
    monkeypatch.setattr(BACKEND_CACHE, "_copy_verified_backend_artifact", copy)
    staged = BACKEND_CACHE._hydrate_shared_stdlib_archive(
        source,
        stdlib_object_cache_key="expected",
        stdlib_object_manifest="manifest",
        output_path=artifacts / "shared-stdlib-link" / source.name,
    )
    assert validations == 1
    assert staged.parent == artifacts / "shared-stdlib-link"
    assert staged.read_bytes() == b"admitted archive"
    assert source.read_bytes() == b"replacement archive"


def test_stdlib_snapshot_rejects_source_alias_without_invalidation(
    tmp_path: Path,
) -> None:
    source = tmp_path / "shared-stdlib-link" / "stdlib.a"
    source.parent.mkdir()
    source.write_bytes(b"source must survive")
    with pytest.raises(OSError, match="distinct from its source"):
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            stdlib_object_cache_key="expected",
            stdlib_object_manifest="manifest",
            output_path=tmp_path / "shared-stdlib-link" / source.name,
        )
    assert source.read_bytes() == b"source must survive"


def test_hydrate_shared_stdlib_archive_requires_matching_source_key_sidecar(
    tmp_path: Path,
) -> None:
    stdlib_object = tmp_path / "cache" / "stdlib_shared_test.a"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(_stdlib_archive_fixture())
    artifacts_root = tmp_path / "artifacts"

    assert (
        cli._hydrate_shared_stdlib_archive(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
        )
        is None
    )

    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "wrong-key\n", encoding="utf-8"
    )
    assert (
        cli._hydrate_shared_stdlib_archive(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
        )
        is None
    )

    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "abc123\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "7\n", encoding="utf-8"
    )
    assert (
        cli._hydrate_shared_stdlib_archive(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
        )
        is None
    )

    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("abc123") + "\n", encoding="utf-8"
    )
    assert (
        cli._hydrate_shared_stdlib_archive(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
        )
        is None
    )

    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    assert (
        cli._hydrate_shared_stdlib_archive(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
        )
        is None
    )

    _write_object_digest_sidecar(stdlib_object)

    staged = cli._hydrate_shared_stdlib_archive(
        stdlib_object,
        stdlib_object_cache_key="abc123",
        stdlib_object_manifest=_manifest("abc123"),
        output_path=artifacts_root / "shared-stdlib-link" / stdlib_object.name,
    )

    assert staged.exists()
    assert (
        cli._stdlib_object_key_sidecar_path(staged).read_text(encoding="utf-8")
        == "abc123\n"
    )
    assert (
        cli._stdlib_object_count_sidecar_path(staged).read_text(encoding="utf-8")
        == "7\n"
    )
    assert (
        cli._stdlib_object_manifest_sidecar_path(staged).read_text(encoding="utf-8")
        == _manifest("abc123") + "\n"
    )
    assert (
        cli._stdlib_object_partition_manifest_sidecar_path(staged).read_text(
            encoding="utf-8"
        )
        == _partition_manifest() + "\n"
    )
    assert (
        cli._stdlib_object_digest_sidecar_path(staged).read_text(encoding="utf-8")
        == cli._sha256_file(staged) + "\n"
    )


def test_try_cached_backend_candidates_reuses_prevalidated_stdlib_contract_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    candidate = tmp_path / "cache" / f"module{contract.suffix}"
    candidate.parent.mkdir(parents=True)
    application_bytes = static_archive_bytes(
        native_relocatable_object(
            target_triple=contract.target_triple, symbols=("molt_main",)
        )
    )
    candidate.write_bytes(application_bytes)
    stdlib_object = tmp_path / "cache" / f"stdlib_shared_test{contract.suffix}"
    _write_stdlib_archive_fixture(
        stdlib_object, "stdlib-key", target_triple=contract.target_triple
    )
    output_artifact = tmp_path / "dist" / f"out{contract.suffix}"
    warnings: list[str] = []
    token = BACKEND_CACHE._shared_stdlib_cache_validation_token(
        stdlib_object,
        "stdlib-key",
        stdlib_object_manifest=_manifest("stdlib-key"),
        target_triple=contract.target_triple,
    )
    assert token is not None

    def unexpected_stdlib_revalidation(*args: object, **kwargs: object) -> bool:
        del args, kwargs
        raise AssertionError("prevalidated stdlib contract was revalidated")

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_matches_key",
        unexpected_stdlib_revalidation,
        raising=True,
    )

    cache_hit, tier = BACKEND_CACHE._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=(("module", candidate),),
        output_artifact=output_artifact,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=_manifest("stdlib-key"),
        warnings=warnings,
        stdlib_contract_validation_token=token,
    )

    assert (cache_hit, tier) == (True, "module")
    assert output_artifact.read_bytes() == application_bytes
    assert warnings == []


def test_try_cached_backend_candidates_reuses_synced_output_without_candidate_probe(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    contract = resolve_backend_artifact_contract(target="native", emit_mode="obj")
    output_artifact = tmp_path / "dist" / f"out{contract.suffix}"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(
        native_relocatable_object(
            target_triple=contract.target_triple, symbols=("molt_main",)
        )
    )
    state_path = BACKEND_CACHE._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    BACKEND_CACHE._write_artifact_sync_state(
        state_path,
        source_key=BACKEND_CACHE._backend_artifact_source_key(
            "module-key", stdlib_object_cache_key=None, artifact_contract=contract
        ),
        tier="module",
        artifact=output_artifact,
    )
    missing_candidate = tmp_path / "cache" / f"missing-module{contract.suffix}"
    warnings: list[str] = []
    stage_timings_ms: dict[str, float] = {}

    def unexpected_candidate_materialize(*args: object, **kwargs: object) -> bool:
        del args, kwargs
        raise AssertionError("synced output should not probe or materialize candidate")

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_materialize_cached_backend_artifact",
        unexpected_candidate_materialize,
        raising=True,
    )

    cache_hit, tier = BACKEND_CACHE._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=(("module", missing_candidate),),
        output_artifact=output_artifact,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=missing_candidate,
        stdlib_object_path=None,
        stdlib_object_cache_key=None,
        warnings=warnings,
        stage_timings_ms=stage_timings_ms,
    )

    assert (cache_hit, tier) == (True, "module")
    assert warnings == []
    assert "backend_cache_try_synced_output" in stage_timings_ms
    assert "backend_cache_try_candidate_exists" not in stage_timings_ms


def test_try_cached_backend_candidates_rejects_synced_output_with_bad_stdlib_contract(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    output_artifact = tmp_path / "dist" / f"out{contract.suffix}"
    output_artifact.parent.mkdir(parents=True)
    output_artifact.write_bytes(
        static_archive_bytes(
            native_relocatable_object(
                target_triple=contract.target_triple, symbols=("molt_main",)
            )
        )
    )
    state_path = BACKEND_CACHE._artifact_sync_state_path(tmp_path, output_artifact)
    state_path.parent.mkdir(parents=True, exist_ok=True)
    BACKEND_CACHE._write_artifact_sync_state(
        state_path,
        source_key=BACKEND_CACHE._backend_artifact_source_key(
            "module-key",
            stdlib_object_cache_key="stdlib-key",
            artifact_contract=contract,
        ),
        tier="module",
        artifact=output_artifact,
    )
    stdlib_object = tmp_path / "cache" / f"stdlib_shared_current{contract.suffix}"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(b"stdlib")
    warnings: list[str] = []
    stage_timings_ms: dict[str, float] = {}
    missing_candidate = tmp_path / "cache" / f"missing-module{contract.suffix}"

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_matches_key",
        lambda *args, **kwargs: False,
        raising=True,
    )
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_mismatch_detail",
        lambda *args, **kwargs: "bad-contract",
        raising=True,
    )

    cache_hit, tier = BACKEND_CACHE._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=(("module", missing_candidate),),
        output_artifact=output_artifact,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=missing_candidate,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=_manifest("stdlib-key"),
        warnings=warnings,
        stage_timings_ms=stage_timings_ms,
    )

    assert (cache_hit, tier) == (False, None)
    assert warnings == [
        "Ignoring shared stdlib cache with mismatched contract: bad-contract"
    ]
    assert "backend_cache_try_synced_output" not in stage_timings_ms


def test_try_cached_backend_candidates_revalidates_stale_stdlib_contract_token(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    candidate = tmp_path / "cache" / f"module{contract.suffix}"
    candidate.parent.mkdir(parents=True)
    application_bytes = static_archive_bytes(
        native_relocatable_object(
            target_triple=contract.target_triple, symbols=("molt_main",)
        )
    )
    candidate.write_bytes(application_bytes)
    stdlib_object = tmp_path / "cache" / f"stdlib_shared_test{contract.suffix}"
    _write_stdlib_archive_fixture(
        stdlib_object, "stdlib-key", target_triple=contract.target_triple
    )
    output_artifact = tmp_path / "dist" / f"out{contract.suffix}"
    warnings: list[str] = []
    calls = {"revalidate": 0}
    token = BACKEND_CACHE._shared_stdlib_cache_validation_token(
        stdlib_object,
        "stdlib-key",
        stdlib_object_manifest=_manifest("stdlib-key"),
        target_triple=contract.target_triple,
    )
    assert token is not None
    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest("replacement-generation"), encoding="utf-8"
    )
    validate = BACKEND_CACHE._shared_stdlib_cache_matches_key

    def revalidate(*args: object, **kwargs: object) -> bool:
        calls["revalidate"] += 1
        return validate(*args, **kwargs)

    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_matches_key",
        revalidate,
        raising=True,
    )

    cache_hit, tier = BACKEND_CACHE._try_cached_backend_candidates(
        project_root=tmp_path,
        cache_candidates=(("module", candidate),),
        output_artifact=output_artifact,
        artifact_contract=contract,
        cache_key="module-key",
        function_cache_key=None,
        cache_path=candidate,
        stdlib_object_path=stdlib_object,
        stdlib_object_cache_key="stdlib-key",
        stdlib_object_manifest=_manifest("stdlib-key"),
        warnings=warnings,
        stdlib_contract_validation_token=token,
    )

    assert (cache_hit, tier) == (True, "module")
    assert output_artifact.read_bytes() == application_bytes
    assert calls == {"revalidate": 1}
    assert warnings == []


def _no_symbol_subprocess(*args, **kwargs):
    pytest.fail("native objects are read in-process, never by a subprocess")


def _counted_symbol_reads(monkeypatch: pytest.MonkeyPatch) -> list[int]:
    reads: list[int] = []
    original = native_symbol_inspection.read_symbol_rows

    def read(reader, **kwargs):
        reads.append(reader.size)
        return original(reader, **kwargs)

    monkeypatch.setattr(native_symbol_inspection, "read_symbol_rows", read)
    monkeypatch.setattr(
        native_symbol_inspection, "_run_completed_command", _no_symbol_subprocess
    )
    return reads


@pytest.mark.parametrize(
    "target_triple",
    ["x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc", "aarch64-apple-darwin"],
)
def test_native_object_symbol_sets_read_target_objects_in_process(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    target_triple: str,
) -> None:
    # The target, not the host running the test, owns symbol decoration: the
    # Mach-O object stores "_" before every C symbol and the facts drop it.
    obj = tmp_path / "stdlib_shared_test.o"
    obj.write_bytes(
        native_relocatable_object(
            target_triple=target_triple,
            symbols=("__future_____Feature___init__",),
            undefined_symbols=("molt_runtime_symbol",),
        )
    )
    reads = _counted_symbol_reads(monkeypatch)

    symbols = cli._native_object_global_symbol_sets(obj, target_triple=target_triple)

    assert reads == [obj.stat().st_size]
    assert symbols is not None
    defined, undefined = symbols
    assert defined == {"__future_____Feature___init__"}
    assert undefined == {"molt_runtime_symbol"}


def test_native_object_symbol_sets_accept_empty_objects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    obj = tmp_path / "target_gated_empty.o"
    obj.write_bytes(native_relocatable_object())
    _counted_symbol_reads(monkeypatch)

    assert cli._native_object_global_symbol_sets(obj) == (set(), set())


def test_native_object_symbol_sets_reuse_content_bound_result_across_admission_siblings(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    obj = tmp_path / "module.o"
    obj.write_bytes(
        native_relocatable_object(
            symbols=("hello__molt_module_chunk_1",),
            undefined_symbols=("molt_runtime_symbol",),
        )
    )
    contract = resolve_backend_artifact_contract(target="native", emit_mode="obj")
    reads = _counted_symbol_reads(monkeypatch)

    assert cli._is_valid_cached_backend_artifact(
        obj,
        artifact_contract=contract,
    )
    assert not cli._native_object_has_unresolved_module_chunks(
        obj, None, target_triple=contract.target_triple
    )
    assert len(reads) == 1


@pytest.mark.parametrize("suffix", [".a", ".lib"])
@pytest.mark.parametrize(
    "application_providers,stdlib_providers,references,unresolved",
    [
        ({1}, None, {1}, False),
        (set(), {1}, {1}, False),
        ({1}, {2}, {1, 2}, False),
        ({1}, set(), {1, 2}, True),
        (set(), None, {1}, True),
    ],
)
def test_native_archive_chunk_closure_resolves_all_included_members(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    suffix: str,
    application_providers: set[int],
    stdlib_providers: set[int] | None,
    references: set[int],
    unresolved: bool,
) -> None:
    application = tmp_path / f"application{suffix}"
    stdlib = tmp_path / f"stdlib{suffix}" if stdlib_providers is not None else None

    def chunk_symbols(indices: set[int]) -> set[str]:
        return {f"demo__molt_module_chunk_{index}" for index in indices}

    application_defined = chunk_symbols(application_providers)
    application_undefined = chunk_symbols(references) | {"molt_runtime_external"}
    application.write_bytes(
        static_archive_bytes(
            native_relocatable_object(symbols=tuple(sorted(application_defined)))
        )
        + static_archive_bytes(
            native_relocatable_object(
                undefined_symbols=tuple(sorted(application_undefined))
            )
        )[8:]
    )
    if stdlib is not None:
        assert stdlib_providers is not None
        stdlib.write_bytes(
            static_archive_bytes(
                native_relocatable_object(
                    symbols=tuple(sorted(chunk_symbols(stdlib_providers)))
                )
            )
        )
    monkeypatch.setattr(
        native_symbol_inspection, "_run_completed_command", _no_symbol_subprocess
    )
    assert (
        BACKEND_CACHE._native_object_has_unresolved_module_chunks(application, stdlib)
        is unresolved
    )
    facts = native_symbol_inspection._native_object_global_symbol_facts(application)
    assert facts.members is not None
    assert [member.identity.ordinal for member in facts.members] == [0, 1]
    assert [member.identity.member.name for member in facts.members] == [
        "object.o",
        "object.o",
    ]
    assert facts.members[0].symbols.defined == application_defined
    assert facts.members[1].symbols.undefined == application_undefined


def test_native_object_symbol_sets_reuse_persistent_symbol_facts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt.default_paths import backend_cache_artifact_filename

    obj = (
        native_symbol_inspection._default_molt_cache()
        / backend_cache_artifact_filename("a" * 64 + "|artifact:" + "b" * 64, ".o")
    )
    obj.write_bytes(native_relocatable_object(symbols=("original",)))
    reads = _counted_symbol_reads(monkeypatch)

    assert (
        native_symbol_inspection._native_object_global_symbol_facts(obj, publish=True)
        is not None
    )
    assert cli._native_object_global_symbol_sets(obj) is not None
    assert len(reads) == 1
    assert native_symbol_inspection._native_object_symbol_facts_sidecar_path(
        obj
    ).exists()

    sidecar = native_symbol_inspection._native_object_symbol_facts_sidecar_path(obj)
    old = time.time() - 86400
    os.utime(sidecar, (old, old))
    before = sidecar.read_bytes()
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    assert cli._native_object_global_symbol_sets(obj) is not None
    assert len(reads) == 1
    assert sidecar.stat().st_mtime > old
    assert sidecar.read_bytes() == before

    # A digest refusal must not make a stale sidecar appear recently used.
    os.utime(sidecar, (old, old))
    obj.write_bytes(native_relocatable_object(symbols=("changed",)))
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    assert cli._native_object_global_symbol_sets(obj) == ({"changed"}, set())
    assert len(reads) == 2
    assert sidecar.stat().st_mtime == old


@pytest.mark.parametrize("archive", [False, True])
def test_stage_backend_output_warms_native_cache_symbol_facts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    archive: bool,
) -> None:
    backend_output = tmp_path / "backend.o"
    target_triple = "x86_64-unknown-linux-gnu"
    payload = native_relocatable_object(
        target_triple=target_triple, symbols=("molt_main",)
    )
    backend_output.write_bytes(static_archive_bytes(payload) if archive else payload)
    output_artifact = tmp_path / "out" / "output.o"
    cache_path = tmp_path / "cache" / "module.o"
    function_cache_path = tmp_path / "cache" / "function.o"
    monkeypatch.setattr(
        native_symbol_inspection,
        "_default_molt_cache",
        lambda: tmp_path / "symbol-cache",
    )
    err = BACKEND_CACHE._stage_backend_output_and_caches(
        tmp_path,
        backend_output,
        output_artifact,
        cache_path=cache_path,
        cache_key="module-key",
        stdlib_object_cache_key=None,
        function_cache_path=function_cache_path,
        warnings=[],
        artifact_contract=resolve_backend_artifact_contract(
            target="native",
            emit_mode="bin" if archive else "obj",
            target_triple=target_triple,
        ),
    )

    assert err is None
    assert output_artifact.exists()
    native_symbol_inspection._NATIVE_OBJECT_SYMBOL_SETS_CACHE.clear()
    native_symbol_inspection._NATIVE_ARCHIVE_SYMBOL_SETS_CACHE.clear()

    def unexpected_extraction(*args, **kwargs):
        pytest.fail("staged outputs must reuse persistent content-bound symbol facts")

    monkeypatch.setattr(
        native_symbol_inspection, "read_symbol_rows", unexpected_extraction
    )
    for path in (cache_path, function_cache_path):
        facts = native_symbol_inspection._native_object_global_symbol_facts(
            path, target_triple=target_triple
        )
        assert facts.defined_functions == {"molt_main"}
        assert (facts.members is not None) is archive
        assert native_symbol_inspection._native_object_symbol_facts_sidecar_path(
            path
        ).exists() is (not archive)


def test_native_symbol_normalization_is_platform_explicit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The host target triple is the only platform read on this path.
    install_module_view(monkeypatch, "sys", sys, native_link_plan, platform="win32")
    assert (
        cli._normalize_native_symbol_name("__future_____Feature___init__")
        == "__future_____Feature___init__"
    )

    monkeypatch.setattr(native_link_plan.sys, "platform", "darwin")
    assert (
        cli._normalize_native_symbol_name("___future_____Feature___init__")
        == "__future_____Feature___init__"
    )


def test_cached_native_artifact_validation_reads_symbols_in_process(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    obj = tmp_path / "module_cache.o"
    obj.write_bytes(native_relocatable_object(symbols=("molt_main",)))
    reads = _counted_symbol_reads(monkeypatch)

    assert cli._is_valid_cached_backend_artifact(
        obj,
        artifact_contract=resolve_backend_artifact_contract(
            target="native", emit_mode="obj", target_triple=None
        ),
    )
    assert reads == [obj.stat().st_size]


# --- Finding #4 confound: bind the shared-stdlib cache key to the backend binary
# identity so a rebuilt backend with different codegen never reuses stale objects.


def test_build_cache_variant_includes_backend_binary_identity() -> None:
    base = _cache_variant()
    keyed = _cache_variant("path/to/molt-backend|123|456")
    assert "backend_bin=" not in base
    assert "backend_bin=path/to/molt-backend|123|456" in keyed
    assert base != keyed


def test_shared_stdlib_cache_key_changes_with_backend_binary_identity() -> None:
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )

    variant_a = _cache_variant("/molt-backend|1700000000000000000|10000000")
    variant_b = _cache_variant("/molt-backend|1700000009000000000|10000000")

    key_a = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant_a,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant_b,
    )

    # Identical IR, target, and compiler fingerprint — ONLY the backend binary
    # identity differs (a rebuild). The cache key (and the .o path) must change so
    # the stale object compiled by the prior binary is never linked.
    assert key_a != key_b
    assert cli._stdlib_object_cache_path(
        Path("cache"), key_a
    ) != cli._stdlib_object_cache_path(Path("cache"), key_b)


def test_shared_stdlib_cache_key_ignores_retired_partial_linker(
    tmp_path: Path,
) -> None:
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[
            {
                "kind": "code_slot_set",
                "s_value": "molt_init_sys",
                "args": ["code", "globals"],
            }
        ],
    )
    linker_a = tmp_path / "ld-a"
    linker_b = tmp_path / "ld-b"
    linker_a.write_text("a", encoding="utf-8")
    linker_b.write_text("b", encoding="utf-8")
    variant_a = _cache_variant(
        codegen_env=BACKEND_EXECUTION._backend_codegen_env_digest(
            is_wasm=False,
            env={"MOLT_LINKER": str(linker_a)},
        )
    )
    variant_b = _cache_variant(
        codegen_env=BACKEND_EXECUTION._backend_codegen_env_digest(
            is_wasm=False,
            env={"MOLT_LINKER": str(linker_b)},
        )
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        **_KEY_FINGERPRINTS,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant_a,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant_b,
        **_KEY_FINGERPRINTS,
    )

    assert key_a == key_b
    assert cli._stdlib_object_cache_path(
        Path("cache"), key_a
    ) == cli._stdlib_object_cache_path(Path("cache"), key_b)


def test_backend_binary_identity_tracks_content_and_fails_safe(tmp_path: Path) -> None:
    backend_bin = tmp_path / "molt-backend"

    missing = cli._backend_binary_identity(backend_bin)
    assert missing.startswith("missing:")

    backend_bin.write_bytes(b"AAAA")
    os.utime(backend_bin, (1_700_000_000, 1_700_000_000))
    ident_small = cli._backend_binary_identity(backend_bin)
    assert not ident_small.startswith("missing:")
    assert ident_small != missing

    # A rebuild that changes content/size and bumps mtime must change identity.
    backend_bin.write_bytes(b"BBBBBBBB")
    os.utime(backend_bin, (1_700_000_010, 1_700_000_010))
    ident_big = cli._backend_binary_identity(backend_bin)
    assert ident_big != ident_small

    # A same-content relink does not invalidate a content-addressed cache.
    backend_bin.write_bytes(b"BBBBBBBB")
    os.utime(backend_bin, (1_700_000_020, 1_700_000_020))
    ident_touched = cli._backend_binary_identity(backend_bin)
    assert ident_touched == ident_big


@pytest.mark.parametrize("replace", [False, True])
def test_backend_binary_identity_and_daemon_selection_reject_preserved_metadata_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, replace: bool
) -> None:
    backend_bin = tmp_path / "molt-backend"
    backend_bin.write_bytes(b"old-binary")
    before = cli._backend_binary_identity(backend_bin)
    monkeypatch.setattr(
        BACKEND_EXECUTION, "_cache_tooling_fingerprint", lambda: "tooling"
    )
    daemon_before = BACKEND_EXECUTION._backend_daemon_config_digest(
        tmp_path,
        "release",
        backend_bin=backend_bin,
        env={"MOLT_BACKEND_COMPILER_FINGERPRINT": "admitted"},
    )
    metadata = backend_bin.stat()
    if replace:
        replacement = tmp_path / "replacement"
        replacement.write_bytes(b"new-binary")
        os.replace(replacement, backend_bin)
    else:
        backend_bin.write_bytes(b"new-binary")
    assert backend_bin.stat().st_size == metadata.st_size
    os.utime(backend_bin, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    after = cli._backend_binary_identity(backend_bin)
    daemon_after = BACKEND_EXECUTION._backend_daemon_config_digest(
        tmp_path,
        "release",
        backend_bin=backend_bin,
        env={"MOLT_BACKEND_COMPILER_FINGERPRINT": "admitted"},
    )
    assert before != after
    assert daemon_before != daemon_after


def test_backend_features_for_target_single_source_of_truth(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The process environment never selects the backend (HF-60).
    monkeypatch.setenv("MOLT_BACKEND", "llvm")
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=False,
        codegen_backend="cranelift",
    ) == ("native-backend",)
    assert cli._backend_features_for_target(
        is_wasm=True,
        is_luau_transpile=False,
        is_rust_transpile=False,
        codegen_backend="cranelift",
    ) == ("wasm-backend",)
    # luau implies rust-transpile too; luau wins.
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=True,
        is_rust_transpile=True,
        codegen_backend="cranelift",
    ) == ("luau-backend",)
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=True,
        codegen_backend="cranelift",
    ) == ("rust-backend",)
    # The llvm feature folds in and changes both codegen and the binary path.
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=False,
        codegen_backend="llvm",
    ) == ("native-backend", "llvm")
    with pytest.raises(ValueError, match="unknown codegen backend"):
        cli._backend_features_for_target(
            is_wasm=False,
            is_luau_transpile=False,
            is_rust_transpile=False,
            codegen_backend="gcc",  # type: ignore[arg-type]
        )
    # The target-string wrapper derives the booleans purely from `target`.
    assert cli._backend_features_for_build_target(
        target="native", is_wasm=False, codegen_backend="cranelift"
    ) == ("native-backend",)
    assert cli._backend_features_for_build_target(
        target="luau", is_wasm=False, codegen_backend="cranelift"
    ) == ("luau-backend",)
    assert cli._backend_features_for_build_target(
        target="rust", is_wasm=False, codegen_backend="cranelift"
    ) == ("rust-backend",)
    assert cli._backend_features_for_build_target(
        target="wasm", is_wasm=True, codegen_backend="cranelift"
    ) == ("wasm-backend",)


@pytest.mark.parametrize("failure", ["evicted", "reader"])
def test_optional_native_candidate_loss_is_a_miss_but_reader_failure_is_not(
    tmp_path, monkeypatch, failure
):
    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    candidate = tmp_path / "candidate.a"
    candidate.write_bytes(
        static_archive_bytes(
            native_relocatable_object(
                target_triple=contract.target_triple, symbols=("molt_main",)
            )
        )
    )
    output = tmp_path / "output.a"
    original = BACKEND_CACHE._native_object_has_unresolved_module_chunks

    def lose_candidate(path, *args, **kwargs):
        if failure == "reader":
            raise native_symbol_inspection.NativeSymbolInspectionError(
                path, ["reader unavailable"]
            )
        candidate.unlink()
        return original(path, *args, **kwargs)

    monkeypatch.setattr(
        BACKEND_CACHE, "_native_object_has_unresolved_module_chunks", lose_candidate
    )

    def admit():
        return BACKEND_CACHE._try_cached_backend_candidates(
            project_root=tmp_path,
            cache_candidates=[("module", candidate)],
            output_artifact=output,
            artifact_contract=contract,
            cache_key="module",
            function_cache_key=None,
            cache_path=candidate,
            stdlib_object_path=None,
            stdlib_object_cache_key=None,
            warnings=[],
        )

    if failure == "reader":
        with pytest.raises(
            native_symbol_inspection.NativeSymbolInspectionError,
            match="reader unavailable",
        ):
            admit()
        assert candidate.exists()
    else:
        assert admit() == (False, None)
        assert not candidate.exists()
    assert not output.exists()


def test_hydrated_required_stdlib_survives_optional_cache_eviction(tmp_path):
    from molt import cache_eviction
    from molt.default_paths import shared_stdlib_cache_entry

    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    cache = tmp_path / "cache"
    entry, _lock = shared_stdlib_cache_entry(cache, "a" * 64)
    source = entry / "stdlib.a"
    source.parent.mkdir(parents=True)
    _write_stdlib_archive_fixture(
        source, "a" * 64, target_triple=contract.target_triple
    )
    output = tmp_path / "build" / "owned.a"
    admitted = BACKEND_CACHE._hydrate_shared_stdlib_archive(
        source,
        output_path=output,
        stdlib_object_cache_key="a" * 64,
        stdlib_object_manifest=_manifest("a" * 64),
        target_triple=contract.target_triple,
    )
    assert admitted == output
    before = {
        path: path.read_bytes()
        for path in _shared_stdlib_cleanup_paths(output)
        if path.exists()
    }
    summary = cache_eviction._prune(cache, max_bytes=0, max_age_days=0, dry_run=False)
    assert summary["entries_removed"] == 1
    assert not entry.exists()
    BACKEND_CACHE._validate_shared_stdlib_output_for_link(
        output,
        stdlib_object_cache_key="a" * 64,
        stdlib_object_manifest=_manifest("a" * 64),
        target_triple=contract.target_triple,
    )
    assert {path: path.read_bytes() for path in before} == before


@pytest.mark.parametrize("key", ["aa" + "0" * 62, "bb" + "0" * 62])
@pytest.mark.parametrize("leaf", ["stdlib.a", "nested/owned.a"])
def test_hydration_refuses_required_output_in_evictable_entry_before_lock(
    tmp_path, monkeypatch, key, leaf
):
    source = tmp_path / "cache" / "shared-stdlib" / ("aa" + "1" * 62) / "stdlib.a"
    output = source.parent.parent / key / leaf

    def no_lock(*args, **kwargs):
        pytest.fail("invalid output must fail before acquiring either stripe")

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", no_lock)
    with pytest.raises(OSError, match="outside cache entries"):
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            output_path=output,
            stdlib_object_cache_key=None,
            stdlib_object_manifest=None,
        )
    assert not output.exists()


def test_hydration_refuses_parent_alias_to_cache_before_lock(tmp_path, monkeypatch):
    entry = tmp_path / "cache" / "shared-stdlib" / ("a" * 64)
    entry.mkdir(parents=True)
    alias = tmp_path / "alias"
    alias.symlink_to(entry, target_is_directory=True)

    def no_lock(*args, **kwargs):
        pytest.fail("physical cache alias must fail before locks")

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", no_lock)
    for output in (alias / "owned.a", entry / "unused" / ".." / "owned.a"):
        with pytest.raises(OSError, match="outside cache entries"):
            BACKEND_CACHE._hydrate_shared_stdlib_archive(
                entry / "stdlib.a",
                output_path=output,
                stdlib_object_cache_key=None,
                stdlib_object_manifest=None,
            )


@pytest.mark.parametrize("failed_lock", ["source", "destination"])
def test_hydration_lock_admission_failure_preserves_existing_owned_output(
    tmp_path, monkeypatch, failed_lock
):
    source = tmp_path / "source.a"
    output = tmp_path / "owned.a"
    output.write_bytes(b"existing required generation")

    @contextmanager
    def lock(path):
        if path == (source if failed_lock == "source" else output):
            raise OSError("lock admission failed")
        yield

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", lock)
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_remove_shared_stdlib_cache_artifacts",
        lambda path: pytest.fail("lock admission does not own invalidation"),
    )
    with pytest.raises(OSError, match="lock admission failed"):
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            output_path=output,
            stdlib_object_cache_key=None,
            stdlib_object_manifest=None,
        )
    assert output.read_bytes() == b"existing required generation"


def test_hydration_copy_failure_cleans_only_while_destination_lock_is_owned(
    tmp_path, monkeypatch
):
    source = tmp_path / "source.a"
    output = tmp_path / "owned.a"
    source.write_bytes(b"source")
    output.write_bytes(b"old")
    active = set()

    @contextmanager
    def lock(path):
        active.add(path)
        try:
            yield
        finally:
            active.remove(path)
            if path == output:
                output.write_bytes(b"next successful generation")

    monkeypatch.setattr(BACKEND_CACHE, "_shared_stdlib_cache_lock", lock)
    identity = native_symbol_inspection._native_symbol_artifact_identity(source)
    monkeypatch.setattr(
        BACKEND_CACHE,
        "_shared_stdlib_cache_generation_token",
        lambda *a, **k: ("generation", (identity,)),
    )
    monkeypatch.setattr(
        BACKEND_CACHE, "_shared_stdlib_cache_matches_key", lambda *a, **k: True
    )

    def fail_copy(*a, **k):
        raise OSError("copy failed")

    monkeypatch.setattr(BACKEND_CACHE, "_copy_verified_backend_artifact", fail_copy)
    original = BACKEND_CACHE._remove_shared_stdlib_cache_artifacts

    def cleanup(path):
        assert output in active
        original(path)

    monkeypatch.setattr(BACKEND_CACHE, "_remove_shared_stdlib_cache_artifacts", cleanup)
    with pytest.raises(OSError, match="copy failed"):
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            output_path=output,
            stdlib_object_cache_key=None,
            stdlib_object_manifest=None,
        )
    assert output.read_bytes() == b"next successful generation"


def test_shared_archive_hydration_refreshes_only_optional_entry_owner(tmp_path):
    from molt import cache_eviction
    from molt.default_paths import shared_stdlib_cache_entry

    contract = resolve_backend_artifact_contract(target="native", emit_mode="bin")
    cache = tmp_path / "cache"
    hot, _ = shared_stdlib_cache_entry(cache, "a" * 64)
    cold, _ = shared_stdlib_cache_entry(cache, "b" * 64)
    old = time.time() - 86400
    for index, entry in enumerate((hot, cold)):
        _write_stdlib_archive_fixture(
            entry / "stdlib.a", entry.name, target_triple=contract.target_triple
        )
        # Seed the real persisted symbol contract before aging the entry:
        # initial sidecar publication must not masquerade as read recency.
        assert BACKEND_CACHE._shared_stdlib_cache_matches_key_locked(
            entry / "stdlib.a",
            entry.name,
            stdlib_object_manifest=_manifest(entry.name),
            target_triple=contract.target_triple,
        )
        os.utime(entry, (old + index, old + index))
    source = hot / "stdlib.a"
    before = (source.read_bytes(), source.stat().st_mtime_ns, source.stat().st_ctime_ns)
    output = tmp_path / "build" / "owned.a"
    assert (
        BACKEND_CACHE._hydrate_shared_stdlib_archive(
            source,
            output_path=output,
            stdlib_object_cache_key=hot.name,
            stdlib_object_manifest=_manifest(hot.name),
            target_triple=contract.target_triple,
        )
        == output
    )
    result = cache_eviction._prune(
        cache,
        max_bytes=cache_eviction._entry_size_bytes(hot),
        max_age_days=None,
        dry_run=False,
    )
    assert result["entries_removed"] == 1
    assert hot.is_dir() and not cold.exists()
    assert (
        source.read_bytes(),
        source.stat().st_mtime_ns,
        source.stat().st_ctime_ns,
    ) == before
    assert output.read_bytes() == before[0]
