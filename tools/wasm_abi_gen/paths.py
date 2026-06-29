"""Filesystem paths for the WASM ABI generator."""

from __future__ import annotations

from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
MANIFEST = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml"
LEGACY_OUT_RS = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_generated.rs"
OUT_RS_DIR = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_generated"
OUT_RS_FILES = {
    "mod.rs": OUT_RS_DIR / "mod.rs",
    "bulk_memory_ops.rs": OUT_RS_DIR / "bulk_memory_ops.rs",
    "call_indirect.rs": OUT_RS_DIR / "call_indirect.rs",
    "static_types.rs": OUT_RS_DIR / "static_types.rs",
    "import_tokens.rs": OUT_RS_DIR / "import_tokens.rs",
    "import_metadata.rs": OUT_RS_DIR / "import_metadata.rs",
    "import_specs.rs": OUT_RS_DIR / "import_specs.rs",
    "import_queries.rs": OUT_RS_DIR / "import_queries.rs",
    "lir_runtime_calls.rs": OUT_RS_DIR / "lir_runtime_calls.rs",
    "container_runtime_selector.rs": OUT_RS_DIR / "container_runtime_selector.rs",
    "object_new_bound_selector.rs": OUT_RS_DIR / "object_new_bound_selector.rs",
    "method_ic_selector.rs": OUT_RS_DIR / "method_ic_selector.rs",
    "numeric_runtime_selector.rs": OUT_RS_DIR / "numeric_runtime_selector.rs",
    "const_policy.rs": OUT_RS_DIR / "const_policy.rs",
    "runtime_surface.rs": OUT_RS_DIR / "runtime_surface.rs",
    "poll_table_imports.rs": OUT_RS_DIR / "poll_table_imports.rs",
    "runtime_callable_imports.rs": OUT_RS_DIR / "runtime_callable_imports.rs",
    "reserved_runtime_callables.rs": OUT_RS_DIR / "reserved_runtime_callables.rs",
    "runtime_callable_queries.rs": OUT_RS_DIR / "runtime_callable_queries.rs",
    "pure_profile.rs": OUT_RS_DIR / "pure_profile.rs",
}
OUT_RUNTIME_CALLABLES_RS = (
    ROOT / "runtime/molt-runtime/src/builtins/functions/wasm_callables_generated.rs"
)
OUT_PY = ROOT / "src/molt/_wasm_abi_generated.py"
INTRINSICS_MANIFEST = ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
INTRINSIC_CATEGORIES = ROOT / "runtime/molt-runtime/src/intrinsics/categories.toml"
RUNTIME_ROOT = ROOT / "runtime"
OUT_TABLE_LAYOUT_INC = ROOT / "runtime/wasm_table_layout.inc"
OUT_ALLOWED_IMPORTS = ROOT / "tools/wasm_allowed_imports.txt"
REMOVED_GENERATED_FILES = (
    ROOT / "runtime/wasm_poll_callables.inc",
    ROOT / "runtime/wasm_runtime_callables.inc",
)
