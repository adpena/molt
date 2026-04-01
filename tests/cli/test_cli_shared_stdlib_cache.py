from pathlib import Path

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
        user_ops=[],
        stdlib_ops=[{"kind": "code_slot_set", "value": 73}],
    )
    ir_b = _ir_with_stdlib(
        user_ops=[],
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
