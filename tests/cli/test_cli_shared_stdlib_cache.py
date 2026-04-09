from pathlib import Path
import subprocess

import pytest

import molt.cli as cli


def _cache_variant() -> str:
    return cli._build_cache_variant(
        profile="dev",
        runtime_cargo="dev-fast",
        backend_cargo="dev-fast",
        emit="native",
        stdlib_split=True,
        codegen_env="codegen=v1",
        linked=False,
    )


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


def test_shared_stdlib_cache_key_ignores_user_only_changes() -> None:
    variant = _cache_variant()
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
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_changes_with_stdlib_payload_and_target() -> None:
    variant = _cache_variant()
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
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )
    key_c = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        target_triple="aarch64-apple-darwin",
        cache_variant=variant,
    )

    assert key_a != key_b
    assert key_a != key_c
    assert cli._stdlib_object_cache_path(Path("cache"), key_a) != cli._stdlib_object_cache_path(
        Path("cache"), key_b
    )


def test_shared_stdlib_cache_key_changes_with_compiler_fingerprint() -> None:
    variant = _cache_variant()
    ir = _ir_with_stdlib(
        user_ops=[],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )

    key_a = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-a",
    )
    key_b = cli._shared_stdlib_cache_key(
        ir,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
        compiler_fingerprint="compiler-b",
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_ignores_non_stdlib_top_level_extras() -> None:
    variant = _cache_variant()
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
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_changes_with_reachable_stdlib_subset() -> None:
    variant = _cache_variant()
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
            {"name": "molt_init_sys", "params": [], "ops": [{"kind": "code_slot_set", "value": 73}]},
            {"name": "molt_init_json", "params": [], "ops": [{"kind": "code_slot_set", "value": 843}]},
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
            {"name": "molt_init_sys", "params": [], "ops": [{"kind": "code_slot_set", "value": 73}]},
            {"name": "molt_init_json", "params": [], "ops": [{"kind": "code_slot_set", "value": 843}]},
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a != key_b


def test_shared_stdlib_cache_key_ignores_function_order_when_reachable_set_matches() -> None:
    variant = _cache_variant()
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
            {"name": "molt_main", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}]},
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
            {"name": "molt_main", "params": [], "ops": [{"kind": "call_internal", "s_value": "molt_init_sys"}]},
            *shared_stdlib,
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_key_ignores_unreachable_stdlib_changes() -> None:
    variant = _cache_variant()
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
            {"name": "molt_init_sys", "params": [], "ops": [{"kind": "code_slot_set", "value": 73}]},
            {"name": "molt_init_json", "params": [], "ops": [{"kind": "code_slot_set", "value": 843}]},
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
            {"name": "molt_init_sys", "params": [], "ops": [{"kind": "code_slot_set", "value": 73}]},
            {"name": "molt_init_json", "params": [], "ops": [{"kind": "code_slot_set", "value": 999999}]},
        ],
    }

    key_a = cli._shared_stdlib_cache_key(
        ir_a,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )
    key_b = cli._shared_stdlib_cache_key(
        ir_b,
        entry_module="app",
        target_triple=None,
        cache_variant=variant,
    )

    assert key_a == key_b


def test_shared_stdlib_cache_matches_key_requires_present_matching_sidecar(
    tmp_path: Path,
) -> None:
    stdlib_object = tmp_path / "stdlib_shared_test.o"
    stdlib_object.write_bytes(b"fake")

    assert not cli._shared_stdlib_cache_matches_key(stdlib_object, "abc123")

    key_sidecar = cli._stdlib_object_key_sidecar_path(stdlib_object)
    key_sidecar.write_text("wrong-key\n", encoding="utf-8")
    assert not cli._shared_stdlib_cache_matches_key(stdlib_object, "abc123")

    key_sidecar.write_text("abc123\n", encoding="utf-8")
    assert cli._shared_stdlib_cache_matches_key(stdlib_object, "abc123")


def test_ensure_backend_binary_preserves_repo_local_shared_stdlib_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project_root = tmp_path
    cache_root = project_root / ".molt_cache"
    cache_root.mkdir(parents=True)
    stdlib_object = cache_root / "stdlib_shared_deadbeef.o"
    stdlib_object.write_bytes(b"shared-stdlib")
    cli._stdlib_object_count_sidecar_path(stdlib_object).write_text(
        "2073\n", encoding="utf-8"
    )
    cli._stdlib_object_key_sidecar_path(stdlib_object).write_text(
        "stdlib-cache-key\n", encoding="utf-8"
    )

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
