import os
import hashlib
import importlib
import json
from pathlib import Path
import subprocess
import sys
from typing import Mapping

import pytest

import molt.cli as cli
from tests.cli.process_guard import run_cli_test_process


ROOT = Path(__file__).resolve().parents[2]
BACKEND_CACHE = importlib.import_module("molt.cli.backend_cache")
CACHE_KEYS = importlib.import_module("molt.cli.cache_keys")


def _cache_variant(
    backend_binary_identity: str = "",
    *,
    codegen_env: str = "codegen=v1",
) -> str:
    return cli._build_cache_variant(
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


def _compile_c_object(tmp_path: Path, name: str, source: str) -> Path:
    src = tmp_path / f"{name}.c"
    obj = tmp_path / f"{name}.o"
    src.write_text(source, encoding="utf-8")
    run_cli_test_process(
        ["clang", "-c", str(src), "-o", str(obj)],
        check=True,
        capture_output=True,
        text=True,
    )
    return obj


def _manifest(cache_key: str) -> str:
    return f'{{"cache_key":"{cache_key}"}}'


def _partition_manifest(name: str = "partition-a") -> str:
    return f'{{"body_hash":"{name}","function_count":1,"functions":["molt_init_sys"],"schema":"stdlib-partition-v1"}}'


def _write_object_digest_sidecar(stdlib_object: Path) -> None:
    cli._stdlib_object_digest_sidecar_path(stdlib_object).write_text(
        cli._sha256_file(stdlib_object) + "\n", encoding="utf-8"
    )


def _legacy_streamed_cache_digest(
    payload_ir: Mapping[str, object],
    *,
    target: str,
    target_triple: str | None,
    variant: str,
    schema_version: str,
) -> str:
    payload = json.dumps(
        payload_ir,
        sort_keys=True,
        separators=(",", ":"),
        default=cli._json_ir_default,
    ).encode("utf-8")
    suffix = target_triple or target
    if variant:
        suffix = f"{suffix}:{variant}"
    return hashlib.sha256(
        payload
        + b"|"
        + suffix.encode("utf-8")
        + b"|"
        + cli._cache_fingerprint().encode("utf-8")
        + b"|"
        + cli._cache_tooling_fingerprint().encode("utf-8")
        + b"|"
        + schema_version.encode("utf-8")
    ).hexdigest()


def test_shared_stdlib_cache_key_ignores_user_only_changes() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir_a = _ir_with_stdlib(
        user_ops=[{"kind": "const_int", "value": 1, "out": "v1"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[{"kind": "const_int", "value": 2, "out": "v1"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_streams_legacy_payload_digest() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    payload_ir = cli._shared_stdlib_cache_payload_ir(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        compiler_fingerprint="compiler-fingerprint",
    )
    assert cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple="aarch64-apple-darwin",
        cache_variant=variant,
        compiler_fingerprint="compiler-fingerprint",
    ) == _legacy_streamed_cache_digest(
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
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 843}],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_c = cli._shared_stdlib_cache_key(
        ir_a,
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
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-a",
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-b",
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_changes_with_capability_config() -> None:
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    base_variant = _cache_variant()
    caps_digest = cli._capability_config_cache_digest(
        capabilities_list=["fs.read"],
        capability_profiles=["fs"],
        manifest_env_vars={"MOLT_CAPABILITIES": "fs.read"},
    )
    variant_with_caps = cli._build_cache_variant(
        profile="dev",
        runtime_cargo="dev-fast",
        backend_cargo="dev-fast",
        emit="native",
        stdlib_split=True,
        codegen_env="codegen=v1",
        linked=False,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        capability_config_digest=caps_digest,
    )

    key_base = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=base_variant,
    )
    key_caps = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant_with_caps,
    )

    assert key_base != key_caps
    assert "capability_config=" in variant_with_caps


def test_capability_config_digest_changes_with_ambient_capability_env(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("MOLT_CAPABILITIES", "fs.read")
    digest_fs = cli._capability_config_cache_digest(
        capabilities_list=None,
        capability_profiles=None,
        manifest_env_vars=None,
    )
    monkeypatch.setenv("MOLT_CAPABILITIES", "env.read")
    digest_env = cli._capability_config_cache_digest(
        capabilities_list=None,
        capability_profiles=None,
        manifest_env_vars=None,
    )

    assert digest_fs != digest_env


def test_prepare_backend_cache_setup_threads_capability_config_to_stdlib_key(
    tmp_path: Path,
) -> None:
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        cache_enabled=True,
        ir=ir,
        target="native",
        target_triple=None,
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
    )

    setup_base = cli._prepare_backend_cache_setup(
        output_artifact=tmp_path / "base.o",
        **common,
    )
    setup_caps = cli._prepare_backend_cache_setup(
        output_artifact=tmp_path / "caps.o",
        capabilities_list=["fs.read"],
        capability_profiles=["fs"],
        manifest_env_vars={"MOLT_CAPABILITIES": "fs.read"},
        **common,
    )

    assert setup_base.stdlib_object_cache_key is not None
    assert setup_caps.stdlib_object_cache_key is not None
    assert setup_base.stdlib_object_cache_key != setup_caps.stdlib_object_cache_key


def test_prepare_backend_cache_setup_threads_ambient_capability_env_to_stdlib_key(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    module_graph_metadata = cli._ModuleGraphMetadata(
        logical_source_path_by_module={},
        entry_override_by_module={},
        module_is_namespace_by_module={},
        module_is_package_by_module={},
        frontend_module_costs=None,
        stdlib_like_by_module={"sys": True},
    )
    common = dict(
        cache_enabled=True,
        ir=ir,
        target="native",
        target_triple=None,
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
    )

    monkeypatch.delenv("MOLT_CAPABILITIES", raising=False)
    setup_without_env = cli._prepare_backend_cache_setup(
        output_artifact=tmp_path / "without-env.o",
        **common,
    )
    monkeypatch.setenv("MOLT_CAPABILITIES", "fs.read,fs.write,env.read")
    setup_with_env = cli._prepare_backend_cache_setup(
        output_artifact=tmp_path / "with-env.o",
        **common,
    )

    assert setup_without_env.stdlib_object_cache_key is not None
    assert setup_with_env.stdlib_object_cache_key is not None
    assert (
        setup_without_env.stdlib_object_cache_key
        != setup_with_env.stdlib_object_cache_key
    )


def test_shared_stdlib_cache_key_ignores_non_stdlib_top_level_extras() -> None:
    variant = _cache_variant()
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir_a = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    ir_a["entry_metadata"] = {"driver": "stage5"}
    ir_b["entry_metadata"] = {"driver": "stage8"}

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
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
                "ops": [{"kind": "code_slot_set", "value": 73}],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [{"kind": "code_slot_set", "value": 843}],
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
                "ops": [{"kind": "code_slot_set", "value": 73}],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [{"kind": "code_slot_set", "value": 843}],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
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
            "ops": [{"kind": "code_slot_set", "value": 73}],
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
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
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
                "ops": [{"kind": "code_slot_set", "value": 73}],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [{"kind": "code_slot_set", "value": 843}],
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
                "ops": [{"kind": "code_slot_set", "value": 73}],
            },
            {
                "name": "molt_init_json",
                "params": [],
                "ops": [{"kind": "code_slot_set", "value": 999999}],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
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
                "ops": [{"kind": "code_slot_set", "value": 73}],
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
                "ops": [{"kind": "code_slot_set", "value": 843}],
            },
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        stdlib_module_symbols=stdlib_modules,
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
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
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=_explicit_stdlib_modules("sys"),
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
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
        entry_module="app",
        stdlib_module_symbols=base_symbols,
        target_triple=None,
        cache_variant=variant,
    )
    key_support = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        stdlib_module_symbols=runtime_support_symbols,
        target_triple=None,
        cache_variant=variant,
    )
    payload = cli._shared_stdlib_cache_payload_ir(
        ir,
        entry_module="app",
        stdlib_module_symbols=runtime_support_symbols,
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
    stdlib_object = tmp_path / "stdlib_shared_test.o"
    stdlib_object.write_bytes(b"fake")

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
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
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

    backend_bin = tmp_path / "target" / "dev-fast" / "molt-backend"
    fingerprint = {"hash": "abc", "rustc": "rustc", "inputs_digest": "inputs"}
    build_cmds: list[list[str]] = []

    def fake_backend_fingerprint(*args: object, **kwargs: object) -> dict[str, str]:
        del args, kwargs
        return dict(fingerprint)

    def fake_run_cargo(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        build_cmds.append(list(cmd))
        backend_bin.parent.mkdir(parents=True, exist_ok=True)
        backend_bin.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
        backend_bin.chmod(0o755)
        return subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("MOLT_HOME", str(home_bin.parent))
    monkeypatch.setattr(cli, "_backend_fingerprint", fake_backend_fingerprint)
    monkeypatch.setattr(cli, "_codesign_binary", lambda _binary_path: None)
    monkeypatch.setattr(cli, "_run_cargo_with_sccache_retry", fake_run_cargo)

    assert cli._ensure_backend_binary(
        backend_bin,
        cargo_timeout=1.0,
        json_output=True,
        cargo_profile="dev-fast",
        project_root=project_root,
        backend_features=("native-backend",),
    )
    assert build_cmds == [
        [
            "cargo",
            "build",
            "--package",
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

    stdlib_object = cache_root / "stdlib_shared_active.o"
    stdlib_object.write_bytes(b"stdlib")
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


def test_validate_shared_stdlib_cache_contract_preserves_matching_key_despite_target_runtime_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    target_root = project_root / "target"
    target_triple = "aarch64-apple-darwin"
    cache_root = project_root / ".molt_cache"
    (project_root / "Cargo.toml").write_text("[workspace]\n", encoding="utf-8")
    cache_root.mkdir(parents=True)

    stdlib_object = cache_root / "stdlib_shared_target.o"
    stdlib_object.write_bytes(b"stdlib")
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

    active = cache_root / "stdlib_shared_active.o"
    active.write_bytes(b"active")
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
    stdlib_object = _compile_c_object(
        tmp_path,
        "stdlib_shared_active",
        "void molt_init_sys(void) {}\n",
    )
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
    stdlib_object = _compile_c_object(
        tmp_path,
        "stdlib_shared_active",
        "void molt_init_sys(void) {}\n",
    )
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

lock_path = cli._shared_stdlib_publish_lock_path(Path(sys.argv[1]))
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
    stdlib_object = tmp_path / ".molt_cache" / "stdlib_shared_active.o"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(b"stdlib")
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "wrong-key\n", encoding="utf-8"
    )
    candidate = tmp_path / ".molt_cache" / "module_cache.o"
    candidate.write_bytes(b"not inspected because stdlib mismatches")
    output_artifact = tmp_path / "target" / "out.o"
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
        is_wasm=False,
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


def test_stage_shared_stdlib_object_for_link_requires_matching_source_key_sidecar(
    tmp_path: Path,
) -> None:
    stdlib_object = tmp_path / "cache" / "stdlib_shared_test.o"
    stdlib_object.parent.mkdir(parents=True)
    stdlib_object.write_bytes(b"shared-stdlib")
    artifacts_root = tmp_path / "artifacts"

    with pytest.raises(OSError, match="Shared stdlib cache contract mismatch"):
        cli._stage_shared_stdlib_object_for_link(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            artifacts_root=artifacts_root,
        )

    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "wrong-key\n", encoding="utf-8"
    )
    with pytest.raises(OSError, match="Shared stdlib cache contract mismatch"):
        cli._stage_shared_stdlib_object_for_link(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            artifacts_root=artifacts_root,
        )

    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "abc123\n", encoding="utf-8"
    )
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "7\n", encoding="utf-8"
    )
    with pytest.raises(OSError, match="Shared stdlib cache contract mismatch"):
        cli._stage_shared_stdlib_object_for_link(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            artifacts_root=artifacts_root,
        )

    cli._stdlib_object_manifest_sidecar_path(stdlib_object).write_text(
        _manifest("abc123") + "\n", encoding="utf-8"
    )
    with pytest.raises(OSError, match="Shared stdlib cache contract mismatch"):
        cli._stage_shared_stdlib_object_for_link(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            artifacts_root=artifacts_root,
        )

    cli._stdlib_object_partition_manifest_sidecar_path(stdlib_object).write_text(
        _partition_manifest() + "\n", encoding="utf-8"
    )
    with pytest.raises(OSError, match="Shared stdlib cache contract mismatch"):
        cli._stage_shared_stdlib_object_for_link(
            stdlib_object,
            stdlib_object_cache_key="abc123",
            stdlib_object_manifest=_manifest("abc123"),
            artifacts_root=artifacts_root,
        )

    _write_object_digest_sidecar(stdlib_object)

    staged = cli._stage_shared_stdlib_object_for_link(
        stdlib_object,
        stdlib_object_cache_key="abc123",
        stdlib_object_manifest=_manifest("abc123"),
        artifacts_root=artifacts_root,
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


def test_native_object_symbol_sets_use_nm_candidate_ladder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    obj = tmp_path / "stdlib_shared_test.o"
    obj.write_bytes(b"coff")
    calls: list[str] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        calls.append(cmd[0])
        if cmd[0] == "broken-nm":
            return subprocess.CompletedProcess(cmd, 1, "", "unreadable COFF")
        return subprocess.CompletedProcess(
            cmd,
            0,
            "00000000 T __future_____Feature___init__\n"
            "         U molt_runtime_symbol\n",
            "",
        )

    monkeypatch.setattr(
        BACKEND_CACHE, "_nm_candidate_binaries", lambda: ["broken-nm", "llvm-nm"]
    )
    monkeypatch.setattr(
        BACKEND_CACHE, "_run_completed_command", fake_run_completed_command
    )

    symbols = cli._native_object_global_symbol_sets(obj)

    assert calls == ["broken-nm", "llvm-nm"]
    assert symbols is not None
    defined, undefined = symbols
    assert "__future_____Feature___init__" in defined
    assert "molt_runtime_symbol" in undefined


def test_native_symbol_normalization_is_platform_explicit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli.sys, "platform", "win32")
    assert (
        cli._normalize_native_symbol_name("__future_____Feature___init__")
        == "__future_____Feature___init__"
    )

    monkeypatch.setattr(cli.sys, "platform", "darwin")
    assert (
        cli._normalize_native_symbol_name("___future_____Feature___init__")
        == "__future_____Feature___init__"
    )


def test_cached_native_artifact_validation_uses_nm_candidate_ladder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    obj = tmp_path / "module_cache.o"
    obj.write_bytes(b"coff")
    calls: list[str] = []

    def fake_run_completed_command(
        cmd: list[str], **kwargs: object
    ) -> subprocess.CompletedProcess[str]:
        del kwargs
        calls.append(cmd[0])
        if cmd[0] == "broken-nm":
            return subprocess.CompletedProcess(cmd, 1, "", "unreadable COFF")
        return subprocess.CompletedProcess(cmd, 0, "00000000 T molt_main\n", "")

    monkeypatch.setattr(
        BACKEND_CACHE, "_nm_candidate_binaries", lambda: ["broken-nm", "llvm-nm"]
    )
    monkeypatch.setattr(
        BACKEND_CACHE, "_run_completed_command", fake_run_completed_command
    )

    assert cli._is_valid_cached_backend_artifact(obj, is_wasm=False)
    assert calls == ["broken-nm", "llvm-nm"]


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
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )

    variant_a = _cache_variant("/molt-backend|1700000000000000000|10000000")
    variant_b = _cache_variant("/molt-backend|1700000009000000000|10000000")

    key_a = cli._shared_stdlib_cache_key(
        ir,
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
    )

    # Identical IR, target, and compiler fingerprint — ONLY the backend binary
    # identity differs (a rebuild). The cache key (and the .o path) must change so
    # the stale object compiled by the prior binary is never linked.
    assert key_a != key_b
    assert cli._stdlib_object_cache_path(
        Path("cache"), key_a
    ) != cli._stdlib_object_cache_path(Path("cache"), key_b)


def test_shared_stdlib_cache_key_changes_with_relocatable_linker_identity(
    tmp_path: Path,
) -> None:
    stdlib_modules = _explicit_stdlib_modules("sys")
    ir = _ir_with_stdlib(
        user_ops=[{"kind": "call_internal", "s_value": "molt_init_sys"}],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    linker_a = tmp_path / "ld-a"
    linker_b = tmp_path / "ld-b"
    linker_a.write_text("a", encoding="utf-8")
    linker_b.write_text("b", encoding="utf-8")
    variant_a = _cache_variant(
        codegen_env=cli._backend_codegen_env_digest(
            is_wasm=False,
            env={"MOLT_LINKER": str(linker_a)},
        )
    )
    variant_b = _cache_variant(
        codegen_env=cli._backend_codegen_env_digest(
            is_wasm=False,
            env={"MOLT_LINKER": str(linker_b)},
        )
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
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
    )

    assert key_a != key_b
    assert cli._stdlib_object_cache_path(
        Path("cache"), key_a
    ) != cli._stdlib_object_cache_path(Path("cache"), key_b)


def test_backend_binary_identity_tracks_stat_and_fails_safe(tmp_path: Path) -> None:
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

    # Same bytes but a newer mtime (cargo relink with identical content) still
    # changes identity — mirrors backend_cache_dir_for's path+mtime convention.
    backend_bin.write_bytes(b"BBBBBBBB")
    os.utime(backend_bin, (1_700_000_020, 1_700_000_020))
    ident_touched = cli._backend_binary_identity(backend_bin)
    assert ident_touched != ident_big


def test_backend_features_for_target_single_source_of_truth() -> None:
    empty: dict[str, str] = {}
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=False,
        env=empty,
    ) == ("native-backend",)
    assert cli._backend_features_for_target(
        is_wasm=True,
        is_luau_transpile=False,
        is_rust_transpile=False,
        env=empty,
    ) == ("wasm-backend",)
    # luau implies rust-transpile too; luau wins.
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=True,
        is_rust_transpile=True,
        env=empty,
    ) == ("luau-backend",)
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=True,
        env=empty,
    ) == ("rust-backend",)
    # The llvm feature folds in and changes both codegen and the binary path.
    assert cli._backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=False,
        env={"MOLT_BACKEND": "llvm"},
    ) == ("native-backend", "llvm")
    # The target-string wrapper derives the booleans purely from `target`.
    assert cli._backend_features_for_build_target(
        target="native", is_wasm=False, env=empty
    ) == ("native-backend",)
    assert cli._backend_features_for_build_target(
        target="luau", is_wasm=False, env=empty
    ) == ("luau-backend",)
    assert cli._backend_features_for_build_target(
        target="rust", is_wasm=False, env=empty
    ) == ("rust-backend",)
    assert cli._backend_features_for_build_target(
        target="wasm", is_wasm=True, env=empty
    ) == ("wasm-backend",)
