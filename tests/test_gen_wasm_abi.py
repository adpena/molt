from __future__ import annotations

import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GEN_WASM_ABI = ROOT / "tools" / "gen_wasm_abi.py"


def _load_gen_wasm_abi():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_wasm_abi", GEN_WASM_ABI
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_wasm_abi_generated_files_are_in_sync() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    assert gen.OUT_RS.read_text(encoding="utf-8") == gen.render_rs(data)
    assert gen.OUT_PY.read_text(encoding="utf-8") == gen.render_py(data)


def test_wasm_abi_manifest_feeds_runtime_export_registry() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    manifest_names = {entry["name"] for entry in data["import"]}

    runtime_exports_path = ROOT / "src/molt/_wasm_runtime_exports.py"
    text = runtime_exports_path.read_text(encoding="utf-8")
    assert "wasm_imports.rs" not in text
    assert "WASM_IMPORT_REGISTRY" in text
    assert {"alloc", "runtime_init", "socket_connect", "task_new"} <= manifest_names


def test_wasm_abi_manifest_owns_pure_profile_prefixes() -> None:
    gen = _load_gen_wasm_abi()
    data = gen.load_manifest()
    prefixes = {entry["prefix"] for entry in data["pure_skip_prefix"]}
    assert {"process_", "socket", "db_", "ws_", "time_"} <= prefixes
    rendered_rs = gen.render_rs(data)
    assert "pure_profile_skips_import" in rendered_rs
    assert "PURE_PROFILE_SKIP_PREFIXES" in rendered_rs
