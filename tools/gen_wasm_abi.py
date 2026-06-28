#!/usr/bin/env python3
"""Generate WASM ABI/import registry artifacts from the canonical manifest."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml"
LEGACY_OUT_RS = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_generated.rs"
OUT_RS_DIR = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_generated"
OUT_RS_FILES = {
    "mod.rs": OUT_RS_DIR / "mod.rs",
    "call_indirect.rs": OUT_RS_DIR / "call_indirect.rs",
    "static_types.rs": OUT_RS_DIR / "static_types.rs",
    "imports.rs": OUT_RS_DIR / "imports.rs",
    "const_policy.rs": OUT_RS_DIR / "const_policy.rs",
    "runtime_surface.rs": OUT_RS_DIR / "runtime_surface.rs",
    "runtime_callables.rs": OUT_RS_DIR / "runtime_callables.rs",
    "pure_profile.rs": OUT_RS_DIR / "pure_profile.rs",
}
OUT_RUNTIME_CALLABLES_RS = (
    ROOT / "runtime/molt-runtime/src/builtins/functions/wasm_callables_generated.rs"
)
OUT_PY = ROOT / "src/molt/_wasm_abi_generated.py"
OUT_TABLE_LAYOUT_INC = ROOT / "runtime/wasm_table_layout.inc"
OUT_ALLOWED_IMPORTS = ROOT / "tools/wasm_allowed_imports.txt"
REMOVED_GENERATED_FILES = (
    ROOT / "runtime/wasm_poll_callables.inc",
    ROOT / "runtime/wasm_runtime_callables.inc",
)
WASM_VAL_TYPES = {
    "i32": "I32",
    "i64": "I64",
    "f32": "F32",
    "f64": "F64",
}
STRIP_IMPORT_CATEGORIES = {
    "essential",
    "io_stdout",
    "io_filesystem",
    "process",
    "database",
    "websocket",
    "socket",
    "time",
    "indirect_call",
    "table",
}
CONST_POLICY_INLINE_SEEDS = {
    "none",
    "int",
    "bool",
    "float",
    "none_value",
}
CONST_POLICY_LITERAL_PAYLOADS = {
    "none",
    "string",
    "bigint_decimal",
    "bytes",
}
CONST_POLICY_SCALAR_PAYLOADS = {
    "none",
    "int",
    "bool",
    "float",
}
CONST_POLICY_RAW_INT_EFFECTS = {
    "set_int",
    "clear",
}
CONST_POLICY_LIR_FAST = {
    "lower",
    "bail_generic",
}
CALL_INDIRECT_IMPORT_PREFIX = "molt_call_indirect"


class WasmAbiManifestError(ValueError):
    pass


def _parse_call_indirect_import_arity(name: str) -> int | None:
    if not name.startswith(CALL_INDIRECT_IMPORT_PREFIX):
        return None
    suffix = name.removeprefix(CALL_INDIRECT_IMPORT_PREFIX)
    if not suffix.isdecimal():
        return None
    arity = int(suffix)
    return arity if str(arity) == suffix else None


def _call_indirect_imports(data: dict) -> list[tuple[int, str]]:
    imports: list[tuple[int, str]] = []
    for entry in data.get("link_allowed_import", []):
        name = entry["name"]
        arity = _parse_call_indirect_import_arity(name)
        if arity is not None:
            imports.append((arity, name))
    return sorted(imports)


def _validate_val_type_list(
    entry_kind: str, entry_idx: int, field: str, value: object
) -> list[str]:
    if not isinstance(value, list):
        raise WasmAbiManifestError(
            f"{entry_kind} entry {entry_idx} field {field!r} must be a list"
        )
    vals: list[str] = []
    for val_idx, val in enumerate(value):
        if not isinstance(val, str) or val not in WASM_VAL_TYPES:
            raise WasmAbiManifestError(
                f"{entry_kind} entry {entry_idx} field {field!r} has invalid "
                f"WASM value type at index {val_idx}: {val!r}"
            )
        vals.append(val)
    return vals


def _validate_string_list(section: str, field: str, value: object) -> list[str]:
    if not isinstance(value, list):
        raise WasmAbiManifestError(f"{section}.{field} must be a list")
    items: list[str] = []
    seen: set[str] = set()
    for idx, item in enumerate(value):
        if not isinstance(item, str) or not item:
            raise WasmAbiManifestError(
                f"{section}.{field} entry {idx} must be a non-empty string"
            )
        if item in seen:
            raise WasmAbiManifestError(
                f"{section}.{field} repeats string {item!r}"
            )
        seen.add(item)
        items.append(item)
    return items


def validate_loaded_manifest(data: dict) -> dict:
    table_layout = data.get("table_layout")
    if not isinstance(table_layout, dict):
        raise WasmAbiManifestError("manifest must define [table_layout]")
    legacy_table_base = table_layout.get("legacy_table_base")
    if not isinstance(legacy_table_base, int) or legacy_table_base <= 0:
        raise WasmAbiManifestError("[table_layout].legacy_table_base must be positive")
    static_types = data.get("static_type")
    if not isinstance(static_types, list) or not static_types:
        raise WasmAbiManifestError("manifest must define static_type entries")
    for idx, entry in enumerate(static_types):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"static_type entry {idx} must be a table")
        entry["params"] = _validate_val_type_list(
            "static_type", idx, "params", entry.get("params")
        )
        entry["results"] = _validate_val_type_list(
            "static_type", idx, "results", entry.get("results")
        )
    if len(static_types) <= 1 or static_types[1] != {
        "params": ["i64"],
        "results": [],
    }:
        raise WasmAbiManifestError(
            "static_type index 1 must remain the (i64) -> () exception-tag ABI"
        )
    static_type_count = len(static_types)
    imports = data.get("import")
    if not isinstance(imports, list) or not imports:
        raise WasmAbiManifestError("manifest must define at least one [[import]]")
    seen_imports: set[str] = set()
    seen_runtime_callables: set[str] = set()
    seen_poll_slots: set[int] = set()
    for idx, entry in enumerate(imports):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"import entry {idx} must be a table")
        name = entry.get("name")
        type_idx = entry.get("type")
        if not isinstance(name, str) or not name:
            raise WasmAbiManifestError(f"import entry {idx} has invalid name")
        if name in seen_imports:
            raise WasmAbiManifestError(f"duplicate import name {name!r}")
        seen_imports.add(name)
        if not isinstance(type_idx, int) or type_idx < 0:
            raise WasmAbiManifestError(f"import {name!r} has invalid type index")
        if type_idx >= static_type_count:
            raise WasmAbiManifestError(
                f"import {name!r} references static type index {type_idx}, "
                f"but only {static_type_count} static types are declared"
            )
        runtime_name = entry.get("runtime_name")
        callable_arity = entry.get("callable_arity")
        callable_result = entry.get("callable_result", "i64")
        has_callable_field = runtime_name is not None or callable_arity is not None
        if has_callable_field:
            if not isinstance(runtime_name, str) or not runtime_name:
                raise WasmAbiManifestError(f"import {name!r} has invalid runtime_name")
            if runtime_name in seen_runtime_callables:
                raise WasmAbiManifestError(f"duplicate runtime callable {runtime_name!r}")
            seen_runtime_callables.add(runtime_name)
            if not isinstance(callable_arity, int) or callable_arity < 0:
                raise WasmAbiManifestError(f"import {name!r} has invalid callable_arity")
            if callable_result not in {"i64", "void"}:
                raise WasmAbiManifestError(
                    f"import {name!r} has invalid callable_result {callable_result!r}"
                )
        elif callable_result != "i64":
            raise WasmAbiManifestError(
                f"import {name!r} cannot set callable_result without callable_arity"
            )
        poll_table_slot = entry.get("poll_table_slot")
        if poll_table_slot is not None:
            if not isinstance(poll_table_slot, int) or poll_table_slot <= 0:
                raise WasmAbiManifestError(
                    f"import {name!r} has invalid poll_table_slot"
                )
            if poll_table_slot in seen_poll_slots:
                raise WasmAbiManifestError(
                    f"duplicate poll table slot {poll_table_slot}"
                )
            seen_poll_slots.add(poll_table_slot)
    if seen_poll_slots:
        expected_poll_slots = set(range(1, max(seen_poll_slots) + 1))
        if seen_poll_slots != expected_poll_slots:
            missing = sorted(expected_poll_slots - seen_poll_slots)
            raise WasmAbiManifestError(
                "poll_table_slot values must be contiguous from 1; "
                f"missing {missing}"
            )

    op_import_deps = data.get("op_import_dep", [])
    if not isinstance(op_import_deps, list):
        raise WasmAbiManifestError("op_import_dep must be a list of tables")
    seen_op_import_kinds: set[str] = set()
    for idx, entry in enumerate(op_import_deps):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"op_import_dep entry {idx} must be a table")
        kind = entry.get("kind")
        deps = entry.get("deps")
        if not isinstance(kind, str) or not kind:
            raise WasmAbiManifestError(f"op_import_dep entry {idx} has invalid kind")
        if kind in seen_op_import_kinds:
            raise WasmAbiManifestError(f"duplicate op_import_dep kind {kind!r}")
        seen_op_import_kinds.add(kind)
        if not isinstance(deps, list):
            raise WasmAbiManifestError(
                f"op_import_dep {kind!r} must define deps as a list"
            )
        seen_deps: set[str] = set()
        for dep_idx, dep in enumerate(deps):
            if not isinstance(dep, str) or not dep:
                raise WasmAbiManifestError(
                    f"op_import_dep {kind!r} has invalid dep at index {dep_idx}"
                )
            if dep in seen_deps:
                raise WasmAbiManifestError(
                    f"op_import_dep {kind!r} repeats import {dep!r}"
                )
            seen_deps.add(dep)
            if dep not in seen_imports:
                raise WasmAbiManifestError(
                    f"op_import_dep {kind!r} references unknown import {dep!r}"
                )

    const_op_policies = data.get("const_op_policy", [])
    if not isinstance(const_op_policies, list):
        raise WasmAbiManifestError("const_op_policy must be a list of tables")
    seen_const_policy_kinds: set[str] = set()
    op_deps_by_kind = {entry["kind"]: entry["deps"] for entry in op_import_deps}
    for idx, entry in enumerate(const_op_policies):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"const_op_policy entry {idx} must be a table")
        kind = entry.get("kind")
        if not isinstance(kind, str) or not kind:
            raise WasmAbiManifestError(f"const_op_policy entry {idx} has invalid kind")
        if kind in seen_const_policy_kinds:
            raise WasmAbiManifestError(f"duplicate const_op_policy kind {kind!r}")
        seen_const_policy_kinds.add(kind)

        inline_seed = entry.get("inline_seed", "none")
        if inline_seed not in CONST_POLICY_INLINE_SEEDS:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} has invalid inline_seed {inline_seed!r}"
            )
        literal_payload = entry.get("literal_payload", "none")
        if literal_payload not in CONST_POLICY_LITERAL_PAYLOADS:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} has invalid literal_payload {literal_payload!r}"
            )
        scalar_payload = entry.get("scalar_payload", "none")
        if scalar_payload not in CONST_POLICY_SCALAR_PAYLOADS:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} has invalid scalar_payload {scalar_payload!r}"
            )
        raw_int_effect = entry.get("raw_int_effect", "clear")
        if raw_int_effect not in CONST_POLICY_RAW_INT_EFFECTS:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} has invalid raw_int_effect {raw_int_effect!r}"
            )
        lir_fast = entry.get("lir_fast", "bail_generic")
        if lir_fast not in CONST_POLICY_LIR_FAST:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} has invalid lir_fast {lir_fast!r}"
            )
        materializer_import = entry.get("materializer_import")
        if materializer_import is not None:
            if not isinstance(materializer_import, str) or not materializer_import:
                raise WasmAbiManifestError(
                    f"const_op_policy {kind!r} has invalid materializer_import"
                )
            if materializer_import not in seen_imports:
                raise WasmAbiManifestError(
                    f"const_op_policy {kind!r} references unknown import "
                    f"{materializer_import!r}"
                )
            if materializer_import not in op_deps_by_kind.get(kind, []):
                raise WasmAbiManifestError(
                    f"const_op_policy {kind!r} materializer_import "
                    f"{materializer_import!r} must appear in op_import_dep deps"
                )

        parse_scalar_literal = entry.get("parse_scalar_literal", False)
        if not isinstance(parse_scalar_literal, bool):
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} parse_scalar_literal must be a bool"
            )
        dispatch_seed = entry.get("dispatch_runtime_seed", materializer_import is not None)
        if not isinstance(dispatch_seed, bool):
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} dispatch_runtime_seed must be a bool"
            )
        if literal_payload == "none" and parse_scalar_literal:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} cannot parse scalar literals without payload"
            )
        if literal_payload != "none" and materializer_import is None:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} literal payload requires materializer_import"
            )
        if dispatch_seed and materializer_import is None:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} dispatch runtime seed requires materializer_import"
            )
        expected_scalar_payload = {
            "int": "int",
            "bool": "bool",
            "float": "float",
            "none": "none",
            "none_value": "none",
        }[inline_seed]
        if scalar_payload != expected_scalar_payload:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} scalar_payload {scalar_payload!r} "
                f"must match inline_seed {inline_seed!r}"
            )
        if lir_fast == "lower" and inline_seed == "none":
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} cannot lower in LIR-fast without inline_seed"
            )
        entry["inline_seed"] = inline_seed
        entry["literal_payload"] = literal_payload
        entry["scalar_payload"] = scalar_payload
        entry["raw_int_effect"] = raw_int_effect
        entry["lir_fast"] = lir_fast
        entry["parse_scalar_literal"] = parse_scalar_literal
        entry["dispatch_runtime_seed"] = dispatch_seed
    prefixes = data.get("pure_skip_prefix", [])
    if not isinstance(prefixes, list):
        raise WasmAbiManifestError("pure_skip_prefix must be a list of tables")
    seen_prefixes: set[str] = set()
    for idx, entry in enumerate(prefixes):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"pure_skip_prefix entry {idx} must be a table")
        prefix = entry.get("prefix")
        if not isinstance(prefix, str) or not prefix:
            raise WasmAbiManifestError(f"pure_skip_prefix entry {idx} has invalid prefix")
        if prefix in seen_prefixes:
            raise WasmAbiManifestError(f"duplicate Pure-profile skip prefix {prefix!r}")
        seen_prefixes.add(prefix)

    required_prefixes = data.get("runtime_required_import_prefix", [])
    if not isinstance(required_prefixes, list):
        raise WasmAbiManifestError(
            "runtime_required_import_prefix must be a list of tables"
        )
    seen_required_prefixes: set[str] = set()
    for idx, entry in enumerate(required_prefixes):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"runtime_required_import_prefix entry {idx} must be a table"
            )
        prefix = entry.get("prefix")
        if not isinstance(prefix, str) or not prefix:
            raise WasmAbiManifestError(
                f"runtime_required_import_prefix entry {idx} has invalid prefix"
            )
        if prefix in seen_required_prefixes:
            raise WasmAbiManifestError(
                f"duplicate runtime-required import prefix {prefix!r}"
            )
        if not any(import_name.startswith(prefix) for import_name in seen_imports):
            raise WasmAbiManifestError(
                f"runtime-required import prefix {prefix!r} matches no imports"
            )
        seen_required_prefixes.add(prefix)

    required_singletons = data.get("runtime_required_import_singleton", [])
    if not isinstance(required_singletons, list):
        raise WasmAbiManifestError(
            "runtime_required_import_singleton must be a list of tables"
        )
    seen_required_singletons: set[str] = set()
    for idx, entry in enumerate(required_singletons):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"runtime_required_import_singleton entry {idx} must be a table"
            )
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise WasmAbiManifestError(
                f"runtime_required_import_singleton entry {idx} has invalid name"
            )
        if name in seen_required_singletons:
            raise WasmAbiManifestError(
                f"duplicate runtime-required import singleton {name!r}"
            )
        if name not in seen_imports:
            raise WasmAbiManifestError(
                f"runtime-required import singleton {name!r} is not a known import"
            )
        matching_prefixes = [
            prefix
            for prefix in seen_required_prefixes
            if name.startswith(prefix)
        ]
        if matching_prefixes:
            raise WasmAbiManifestError(
                f"runtime-required import singleton {name!r} is already covered "
                f"by prefix {sorted(matching_prefixes)[0]!r}"
            )
        seen_required_singletons.add(name)

    reserved_callables = data.get("reserved_runtime_callable", [])
    if not isinstance(reserved_callables, list):
        raise WasmAbiManifestError("reserved_runtime_callable must be a list of tables")
    seen_reserved_indices: set[int] = set()
    seen_reserved_runtime_names: set[str] = set()
    seen_reserved_import_names: set[str] = set()
    for idx, entry in enumerate(reserved_callables):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"reserved_runtime_callable entry {idx} must be a table"
            )
        table_index = entry.get("index")
        runtime_name = entry.get("runtime_name")
        import_name = entry.get("import_name")
        callable_arity = entry.get("callable_arity")
        if not isinstance(table_index, int) or table_index < 0:
            raise WasmAbiManifestError(
                f"reserved_runtime_callable entry {idx} has invalid index"
            )
        if table_index in seen_reserved_indices:
            raise WasmAbiManifestError(
                f"duplicate reserved runtime callable index {table_index}"
            )
        seen_reserved_indices.add(table_index)
        if not isinstance(runtime_name, str) or not runtime_name.startswith("molt_"):
            raise WasmAbiManifestError(
                f"reserved_runtime_callable entry {idx} has invalid runtime_name"
            )
        if runtime_name in seen_reserved_runtime_names:
            raise WasmAbiManifestError(
                f"duplicate reserved runtime callable {runtime_name!r}"
            )
        seen_reserved_runtime_names.add(runtime_name)
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"reserved runtime callable {runtime_name!r} has invalid import_name"
            )
        if import_name in seen_reserved_import_names:
            raise WasmAbiManifestError(
                f"duplicate reserved runtime import {import_name!r}"
            )
        seen_reserved_import_names.add(import_name)
        if not isinstance(callable_arity, int) or callable_arity < 0:
            raise WasmAbiManifestError(
                f"reserved runtime callable {runtime_name!r} has invalid callable_arity"
            )
    expected_reserved_indices = set(range(len(reserved_callables)))
    if seen_reserved_indices != expected_reserved_indices:
        raise WasmAbiManifestError(
            "reserved runtime callable indices must be contiguous from zero"
        )
    output_export_policy = data.get("output_export_policy")
    if not isinstance(output_export_policy, dict):
        raise WasmAbiManifestError("manifest must define [output_export_policy]")
    alias_prefix = output_export_policy.get("alias_prefix")
    if not isinstance(alias_prefix, str) or not alias_prefix.startswith("__molt_"):
        raise WasmAbiManifestError(
            "[output_export_policy].alias_prefix must be a non-empty Molt-private prefix"
        )
    essential_exports = _validate_string_list(
        "output_export_policy",
        "essential_exports",
        output_export_policy.get("essential_exports"),
    )
    runtime_export_aliases = _validate_string_list(
        "output_export_policy",
        "runtime_export_aliases",
        output_export_policy.get("runtime_export_aliases"),
    )
    internal_output_export_prefixes = _validate_string_list(
        "output_export_policy",
        "internal_output_export_prefixes",
        output_export_policy.get("internal_output_export_prefixes"),
    )
    required_essential_exports = {
        "memory",
        "molt_memory",
        "molt_main",
        "molt_table",
        "molt_table_init",
        "__indirect_function_table",
    }
    missing_essential_exports = required_essential_exports - set(essential_exports)
    if missing_essential_exports:
        raise WasmAbiManifestError(
            "output_export_policy essential_exports missing required split-runtime "
            f"exports: {sorted(missing_essential_exports)}"
        )
    alias_overlap = set(runtime_export_aliases) & set(essential_exports)
    if alias_overlap:
        raise WasmAbiManifestError(
            "output_export_policy runtime aliases must not duplicate essential "
            f"exports: {sorted(alias_overlap)}"
        )
    if any(prefix.startswith(alias_prefix) for prefix in internal_output_export_prefixes):
        raise WasmAbiManifestError(
            "output_export_policy internal prefixes must not overlap the alias prefix"
        )

    runtime_export_policy = data.get("runtime_export_policy")
    if not isinstance(runtime_export_policy, dict):
        raise WasmAbiManifestError("manifest must define [runtime_export_policy]")
    host_exports = _validate_string_list(
        "runtime_export_policy",
        "host_exports",
        runtime_export_policy.get("host_exports"),
    )
    for name in host_exports:
        if not name.startswith("molt_"):
            raise WasmAbiManifestError(
                f"runtime_export_policy host export {name!r} must start with 'molt_'"
            )

    fallback_entries = data.get("runtime_import_fallback", [])
    if not isinstance(fallback_entries, list):
        raise WasmAbiManifestError("runtime_import_fallback must be a list of tables")
    seen_fallback_imports: set[str] = set()
    for idx, entry in enumerate(fallback_entries):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"runtime_import_fallback entry {idx} must be a table"
            )
        import_name = entry.get("import")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"runtime_import_fallback entry {idx} has invalid import"
            )
        if import_name in seen_fallback_imports:
            raise WasmAbiManifestError(
                f"duplicate runtime import fallback {import_name!r}"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"runtime import fallback {import_name!r} is not a known import"
            )
        seen_fallback_imports.add(import_name)
        strategy = entry.get("strategy")
        if strategy not in {"call_bind_ic", "direct_export"}:
            raise WasmAbiManifestError(
                f"runtime import fallback {import_name!r} has invalid strategy "
                f"{strategy!r}"
            )
        call_arity = entry.get("call_arity")
        if strategy == "call_bind_ic":
            if not isinstance(call_arity, int) or call_arity < 0:
                raise WasmAbiManifestError(
                    f"runtime import fallback {import_name!r} must define "
                    "non-negative call_arity for call_bind_ic"
                )
        elif call_arity is not None:
            raise WasmAbiManifestError(
                f"runtime import fallback {import_name!r} cannot define "
                "call_arity for direct_export"
            )
        exports = _validate_string_list(
            "runtime_import_fallback",
            "exports",
            entry.get("exports"),
        )
        for export_name in exports:
            if not export_name.startswith("molt_"):
                raise WasmAbiManifestError(
                    f"runtime import fallback {import_name!r} export "
                    f"{export_name!r} must start with 'molt_'"
                )
    link_allowed = data.get("link_allowed_import", [])
    if not isinstance(link_allowed, list):
        raise WasmAbiManifestError("link_allowed_import must be a list of tables")
    seen_link_allowed: set[str] = set()
    for idx, entry in enumerate(link_allowed):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"link_allowed_import entry {idx} must be a table")
        name = entry.get("name")
        if not isinstance(name, str) or not name:
            raise WasmAbiManifestError(
                f"link_allowed_import entry {idx} has invalid name"
            )
        if name in seen_link_allowed:
            raise WasmAbiManifestError(f"duplicate linker allowlist import {name!r}")
        seen_link_allowed.add(name)
    call_indirect_imports = _call_indirect_imports(data)
    call_indirect_arities = [arity for arity, _name in call_indirect_imports]
    if not call_indirect_arities:
        raise WasmAbiManifestError("link_allowed_import must define call_indirect imports")
    expected_call_indirect_arities = list(
        range(call_indirect_arities[-1] + 1)
    )
    if call_indirect_arities != expected_call_indirect_arities:
        raise WasmAbiManifestError(
            "call_indirect import arities must be contiguous from 0: "
            f"{call_indirect_arities!r}"
        )

    def validate_strip_rule(
        section: str, idx: int, entry: object, *, prefix_rule: bool
    ) -> None:
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"{section} entry {idx} must be a table")
        module = entry.get("module")
        key = entry.get("prefix" if prefix_rule else "name")
        category = entry.get("category")
        description = entry.get("description")
        if not isinstance(module, str) or not module:
            raise WasmAbiManifestError(f"{section} entry {idx} has invalid module")
        if not isinstance(key, str) or not key:
            field = "prefix" if prefix_rule else "name"
            raise WasmAbiManifestError(f"{section} entry {idx} has invalid {field}")
        if not isinstance(category, str) or category not in STRIP_IMPORT_CATEGORIES:
            raise WasmAbiManifestError(
                f"{section} entry {idx} has invalid category {category!r}"
            )
        if not isinstance(description, str) or not description:
            raise WasmAbiManifestError(
                f"{section} entry {idx} has invalid description"
            )

    strip_rules = data.get("strip_import_rule", [])
    if not isinstance(strip_rules, list):
        raise WasmAbiManifestError("strip_import_rule must be a list of tables")
    seen_strip_rules: set[tuple[str, str]] = set()
    for idx, entry in enumerate(strip_rules):
        validate_strip_rule("strip_import_rule", idx, entry, prefix_rule=False)
        key = (entry["module"], entry["name"])
        if key in seen_strip_rules:
            raise WasmAbiManifestError(f"duplicate strip import rule {key!r}")
        seen_strip_rules.add(key)

    strip_prefix_rules = data.get("strip_import_prefix_rule", [])
    if not isinstance(strip_prefix_rules, list):
        raise WasmAbiManifestError(
            "strip_import_prefix_rule must be a list of tables"
        )
    seen_strip_prefix_rules: set[tuple[str, str]] = set()
    for idx, entry in enumerate(strip_prefix_rules):
        validate_strip_rule(
            "strip_import_prefix_rule", idx, entry, prefix_rule=True
        )
        key = (entry["module"], entry["prefix"])
        if key in seen_strip_prefix_rules:
            raise WasmAbiManifestError(f"duplicate strip import prefix rule {key!r}")
        seen_strip_prefix_rules.add(key)
    return data


def load_manifest(path: Path = MANIFEST) -> dict:
    return validate_loaded_manifest(tomllib.loads(path.read_text(encoding="utf-8")))


def _header(comment: str) -> str:
    return (
        f"{comment} @generated by tools/gen_wasm_abi.py from\n"
        f"{comment} runtime/molt-backend-wasm/src/wasm_abi_manifest.toml\n"
        f"{comment} DO NOT EDIT BY HAND.\n\n"
    )


def _rust_val_type(val: str) -> str:
    return f"ValType::{WASM_VAL_TYPES[val]}"


def _rust_val_slice(vals: list[str]) -> str:
    if not vals:
        return "&[]"
    return "&[" + ", ".join(_rust_val_type(val) for val in vals) + "]"


def _rustfmt(module_name: str, source: str) -> str:
    try:
        proc = subprocess.run(
            ["rustfmt", "--edition", "2024", "--emit", "stdout"],
            input=source,
            text=True,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            check=False,
        )
    except FileNotFoundError as exc:
        raise RuntimeError(
            "rustfmt is required to generate canonical WASM ABI Rust modules"
        ) from exc
    if proc.returncode != 0:
        raise RuntimeError(
            f"rustfmt failed for generated WASM ABI module {module_name}:\n"
            f"{proc.stderr.strip()}"
        )
    return proc.stdout.rstrip() + "\n"


def _py_tuple(vals: list[str]) -> str:
    if not vals:
        return "()"
    if len(vals) == 1:
        return f'("{vals[0]}",)'
    return "(" + ", ".join(f'"{val}"' for val in vals) + ")"


def _rust_pascal_variant(value: str) -> str:
    return "".join(part.capitalize() for part in value.split("_"))


def _rust_option_str(value: str | None) -> str:
    return "None" if value is None else f'Some("{value}")'


def _render_rs_mod() -> str:
    lines: list[str] = [_header("//")]
    lines.extend(
        [
            "mod call_indirect;\n",
            "mod const_policy;\n",
            "mod imports;\n",
            "mod pure_profile;\n",
            "mod runtime_callables;\n",
            "mod runtime_surface;\n",
            "mod static_types;\n\n",
            "pub(crate) use call_indirect::{\n",
            "    CALL_INDIRECT_IMPORTS, CALL_INDIRECT_MAX_ARITY,\n",
            "};\n",
            "pub(crate) use const_policy::{\n",
            "    wasm_const_op_policy, WasmConstInlineSeed, WasmConstLirFastPolicy,\n",
            "    WasmConstLiteralPayload, WasmConstOpPolicySpec, WasmConstRawIntEffect,\n",
            "    WasmConstScalarValue,\n",
            "};\n",
            "pub(crate) use imports::{IMPORT_REGISTRY, OP_IMPORT_DEPS};\n",
            "pub(crate) use pure_profile::pure_profile_skips_import;\n",
            "pub(crate) use runtime_callables::{\n",
            "    POLL_TABLE_IMPORTS, RESERVED_RUNTIME_CALLABLE_COUNT, RESERVED_RUNTIME_CALLABLE_SPECS,\n",
            "    RUNTIME_CALLABLE_IMPORTS, RuntimeCallableResult,\n",
            "};\n",
            "pub(crate) use runtime_surface::runtime_surface_requires_direct_import;\n",
            "pub(crate) use static_types::{\n",
            "    STATIC_FUNC_TYPES, STATIC_TYPE_COUNT,\n",
            "};\n",
        ]
    )
    return "".join(lines)


def _render_rs_call_indirect(data: dict) -> str:
    call_indirect_imports = _call_indirect_imports(data)
    max_arity = call_indirect_imports[-1][0]
    lines: list[str] = [_header("//")]
    lines.extend(
        [
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct CallIndirectImportSpec {\n",
            "    pub(crate) arity: usize,\n",
            "    pub(crate) import_name: &'static str,\n",
            "}\n\n",
            "pub(crate) const CALL_INDIRECT_IMPORTS: &[CallIndirectImportSpec] = &[\n",
        ]
    )
    for arity, import_name in call_indirect_imports:
        lines.extend(
            [
                "    CallIndirectImportSpec {\n",
                f"        arity: {arity},\n",
                f'        import_name: "{import_name}",\n',
                "    },\n",
            ]
        )
    lines.extend(
        [
            "];\n\n",
            f"pub(crate) const CALL_INDIRECT_MAX_ARITY: usize = {max_arity};\n",
        ]
    )
    return "".join(lines)


def _render_rs_const_policy(data: dict) -> str:
    lines: list[str] = [_header("//")]
    lines.extend(
        [
            "use molt_codegen_abi::{box_bool_bits, box_float_bits, box_int_bits, box_none_bits};\n",
            "use molt_tir::tir::ops::{AttrValue, TirOp};\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum WasmConstInlineSeed {\n",
            "    None,\n",
            "    Int,\n",
            "    Bool,\n",
            "    Float,\n",
            "    NoneValue,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum WasmConstLiteralPayload {\n",
            "    None,\n",
            "    String,\n",
            "    BigintDecimal,\n",
            "    Bytes,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum WasmConstScalarPayload {\n",
            "    None,\n",
            "    Int,\n",
            "    Bool,\n",
            "    Float,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, PartialEq)]\n",
            "pub(crate) enum WasmConstScalarValue {\n",
            "    Int(i64),\n",
            "    Bool(bool),\n",
            "    Float(f64),\n",
            "    NoneValue,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum WasmConstRawIntEffect {\n",
            "    SetInt,\n",
            "    Clear,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum WasmConstLirFastPolicy {\n",
            "    Lower,\n",
            "    BailGeneric,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct WasmConstOpPolicySpec {\n",
            "    pub(crate) kind: &'static str,\n",
            "    pub(crate) inline_seed: WasmConstInlineSeed,\n",
            "    pub(crate) materializer_import: Option<&'static str>,\n",
            "    pub(crate) literal_payload: WasmConstLiteralPayload,\n",
            "    pub(crate) scalar_payload: WasmConstScalarPayload,\n",
            "    pub(crate) dispatch_runtime_seed: bool,\n",
            "    pub(crate) parse_scalar_literal: bool,\n",
            "    pub(crate) raw_int_effect: WasmConstRawIntEffect,\n",
            "    pub(crate) lir_fast: WasmConstLirFastPolicy,\n",
            "}\n\n",
            "pub(crate) const WASM_CONST_OP_POLICIES: &[WasmConstOpPolicySpec] = &[\n",
        ]
    )
    for entry in data.get("const_op_policy", []):
        inline_seed = _rust_pascal_variant(entry["inline_seed"])
        literal_payload = _rust_pascal_variant(entry["literal_payload"])
        scalar_payload = _rust_pascal_variant(entry["scalar_payload"])
        raw_int_effect = _rust_pascal_variant(entry["raw_int_effect"])
        lir_fast = _rust_pascal_variant(entry["lir_fast"])
        dispatch_seed = "true" if entry["dispatch_runtime_seed"] else "false"
        parse_scalar = "true" if entry["parse_scalar_literal"] else "false"
        lines.extend(
            [
                "    WasmConstOpPolicySpec {\n",
                f'        kind: "{entry["kind"]}",\n',
                f"        inline_seed: WasmConstInlineSeed::{inline_seed},\n",
                "        materializer_import: "
                f"{_rust_option_str(entry.get('materializer_import'))},\n",
                f"        literal_payload: WasmConstLiteralPayload::{literal_payload},\n",
                f"        scalar_payload: WasmConstScalarPayload::{scalar_payload},\n",
                f"        dispatch_runtime_seed: {dispatch_seed},\n",
                f"        parse_scalar_literal: {parse_scalar},\n",
                f"        raw_int_effect: WasmConstRawIntEffect::{raw_int_effect},\n",
                f"        lir_fast: WasmConstLirFastPolicy::{lir_fast},\n",
                "    },\n",
            ]
        )
    lines.extend(
        [
            "];\n\n",
            "#[inline]\n",
            "pub(crate) fn wasm_const_op_policy(\n",
            "    kind: &str,\n",
            ") -> Option<&'static WasmConstOpPolicySpec> {\n",
            "    WASM_CONST_OP_POLICIES\n",
            "        .iter()\n",
            "        .find(|policy| policy.kind == kind)\n",
            "}\n\n",
            "impl WasmConstOpPolicySpec {\n",
            "    pub(crate) fn required_simple_ir_inline_seed_bits(\n",
            "        &self,\n",
            "        op: &crate::OpIR,\n",
            "    ) -> i64 {\n",
            "        match self.scalar_payload {\n",
            "            WasmConstScalarPayload::Int => box_int_bits(op.value.unwrap_or_else(|| {\n",
            "                panic!(\"WASM const policy {} requires int scalar payload\", self.kind)\n",
            "            })),\n",
            "            WasmConstScalarPayload::Bool => box_bool_bits(op.value.unwrap_or_else(|| {\n",
            "                panic!(\"WASM const policy {} requires bool scalar payload\", self.kind)\n",
            "            })),\n",
            "            WasmConstScalarPayload::Float => box_float_bits(op.f_value.unwrap_or_else(|| {\n",
            "                panic!(\"WASM const policy {} requires float scalar payload\", self.kind)\n",
            "            })),\n",
            "            WasmConstScalarPayload::None => match self.inline_seed {\n",
            "                WasmConstInlineSeed::NoneValue => box_none_bits(),\n",
            "                _ => panic!(\n",
            "                    \"WASM const policy {} has no scalar payload for inline seed {:?}\",\n",
            "                    self.kind, self.inline_seed\n",
            "                ),\n",
            "            },\n",
            "        }\n",
            "    }\n\n",
            "    pub(crate) fn required_tir_scalar_value(\n",
            "        &self,\n",
            "        op: &TirOp,\n",
            "    ) -> WasmConstScalarValue {\n",
            "        match self.scalar_payload {\n",
            "            WasmConstScalarPayload::Int => match op.attrs.get(\"value\") {\n",
            "                Some(AttrValue::Int(value)) => WasmConstScalarValue::Int(*value),\n",
            "                _ => panic!(\"WASM const policy {} requires int scalar payload\", self.kind),\n",
            "            },\n",
            "            WasmConstScalarPayload::Bool => match op.attrs.get(\"value\") {\n",
            "                Some(AttrValue::Bool(value)) => WasmConstScalarValue::Bool(*value),\n",
            "                _ => panic!(\"WASM const policy {} requires bool scalar payload\", self.kind),\n",
            "            },\n",
            "            WasmConstScalarPayload::Float => match op\n",
            "                .attrs\n",
            "                .get(\"f_value\")\n",
            "                .or_else(|| op.attrs.get(\"value\"))\n",
            "            {\n",
            "                Some(AttrValue::Float(value)) => WasmConstScalarValue::Float(*value),\n",
            "                _ => panic!(\"WASM const policy {} requires float scalar payload\", self.kind),\n",
            "            },\n",
            "            WasmConstScalarPayload::None => match self.inline_seed {\n",
            "                WasmConstInlineSeed::NoneValue => WasmConstScalarValue::NoneValue,\n",
            "                _ => panic!(\n",
            "                    \"WASM const policy {} has no scalar payload for inline seed {:?}\",\n",
            "                    self.kind, self.inline_seed\n",
            "                ),\n",
            "            },\n",
            "        }\n",
            "    }\n",
            "}\n\n",
        ]
    )
    return "".join(lines)


def _render_rs_static_types(data: dict) -> str:
    lines: list[str] = [_header("//")]
    lines.append("use wasm_encoder::ValType;\n\n")
    lines.extend(
        [
            "#[derive(Clone, Copy)]\n",
            "pub(crate) struct StaticFuncTypeSpec {\n",
            "    pub(crate) params: &'static [ValType],\n",
            "    pub(crate) results: &'static [ValType],\n",
            "}\n\n",
            "pub(crate) const STATIC_FUNC_TYPES: &[StaticFuncTypeSpec] = &[\n",
        ]
    )
    for entry in data["static_type"]:
        lines.extend(
            [
                "    StaticFuncTypeSpec {\n",
                f"        params: {_rust_val_slice(entry['params'])},\n",
                f"        results: {_rust_val_slice(entry['results'])},\n",
                "    },\n",
            ]
        )
    lines.extend(
        [
            "];\n\n",
            f"pub(crate) const STATIC_TYPE_COUNT: u32 = {len(data['static_type'])};\n\n",
        ]
    )
    return "".join(lines)


def _render_rs_imports(data: dict) -> str:
    lines: list[str] = [_header("//")]
    lines.append("pub(crate) const IMPORT_REGISTRY: &[(&str, u32)] = &[\n")
    for entry in data["import"]:
        lines.append(f'    ("{entry["name"]}", {entry["type"]}),\n')
    lines.append("];\n\n")
    lines.append("pub(crate) const OP_IMPORT_DEPS: &[(&str, &[&str])] = &[\n")
    for entry in data.get("op_import_dep", []):
        kind = entry["kind"]
        deps = entry["deps"]
        if not deps:
            lines.append(f'    ("{kind}", &[]),\n')
            continue
        lines.append(f'    ("{kind}", &[\n')
        for dep in deps:
            lines.append(f'        "{dep}",\n')
        lines.append("    ]),\n")
    lines.append("];\n\n")
    return "".join(lines)


def _render_rs_runtime_surface(data: dict) -> str:
    lines: list[str] = [_header("//")]
    lines.append("pub(crate) const REQUIRED_RUNTIME_IMPORT_PREFIXES: &[&str] = &[\n")
    for entry in data.get("runtime_required_import_prefix", []):
        lines.append(f'    "{entry["prefix"]}",\n')
    lines.append("];\n\n")
    lines.append("pub(crate) const REQUIRED_RUNTIME_IMPORT_SINGLETONS: &[&str] = &[\n")
    for entry in data.get("runtime_required_import_singleton", []):
        lines.append(f'    "{entry["name"]}",\n')
    lines.append("];\n\n")
    lines.extend(
        [
            "#[allow(dead_code)]\n",
            "pub(crate) const RUNTIME_HOST_EXPORTS: &[&str] = &[\n",
        ]
    )
    for name in data["runtime_export_policy"]["host_exports"]:
        lines.append(f'    "{name}",\n')
    lines.append("];\n\n")
    lines.extend(
        [
            "#[allow(dead_code)]\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct RuntimeImportFallbackSpec {\n",
            "    pub(crate) import_name: &'static str,\n",
            "    pub(crate) strategy: &'static str,\n",
            "    pub(crate) call_arity: Option<usize>,\n",
            "    pub(crate) fallback_exports: &'static [&'static str],\n",
            "}\n\n",
            "#[allow(dead_code)]\n",
            "pub(crate) const RUNTIME_IMPORT_FALLBACK_EXPORTS: &[RuntimeImportFallbackSpec] = &[\n",
        ]
    )
    for entry in data.get("runtime_import_fallback", []):
        lines.extend(
            [
                "    RuntimeImportFallbackSpec {\n",
                f'        import_name: "{entry["import"]}",\n',
                f'        strategy: "{entry["strategy"]}",\n',
                "        call_arity: "
                + (
                    f"Some({entry['call_arity']})"
                    if "call_arity" in entry
                    else "None"
                )
                + ",\n",
                "        fallback_exports: &[\n",
            ]
        )
        for export_name in entry["exports"]:
            lines.append(f'            "{export_name}",\n')
        lines.extend(
            [
                "        ],\n",
                "    },\n",
            ]
        )
    lines.append("];\n\n")
    lines.extend(
        [
            "#[inline]\n",
            "pub(crate) fn runtime_surface_requires_direct_import(kind: &str) -> bool {\n",
            "    REQUIRED_RUNTIME_IMPORT_PREFIXES\n",
            "        .iter()\n",
            "        .any(|prefix| kind.starts_with(prefix))\n",
            "        || REQUIRED_RUNTIME_IMPORT_SINGLETONS.contains(&kind)\n",
            "}\n\n",
        ]
    )
    return "".join(lines)


def _render_rs_runtime_callables(data: dict) -> str:
    lines: list[str] = [_header("//")]
    poll_imports = sorted(
        (
            (entry["poll_table_slot"], entry["name"])
            for entry in data["import"]
            if "poll_table_slot" in entry
        ),
        key=lambda item: item[0],
    )
    lines.extend(
        [
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct PollTableImportSpec {\n",
            "    pub(crate) table_slot: u32,\n",
            "    pub(crate) import_name: &'static str,\n",
            "}\n\n",
            "pub(crate) const POLL_TABLE_IMPORTS: &[PollTableImportSpec] = &[\n",
        ]
    )
    for slot, name in poll_imports:
        lines.extend(
            [
                "    PollTableImportSpec {\n",
                f"        table_slot: {slot},\n",
                f'        import_name: "{name}",\n',
                "    },\n",
            ]
        )
    lines.extend(
        [
            "];\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) enum RuntimeCallableResult {\n",
            "    I64,\n",
            "    Void,\n",
            "}\n\n",
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct RuntimeCallableImportSpec {\n",
            "    pub(crate) runtime_name: &'static str,\n",
            "    pub(crate) import_name: &'static str,\n",
            "    pub(crate) arity: usize,\n",
            "    pub(crate) result: RuntimeCallableResult,\n",
            "}\n\n",
            "pub(crate) const RUNTIME_CALLABLE_IMPORTS: &[RuntimeCallableImportSpec] = &[\n",
        ]
    )
    for entry in data["import"]:
        if "callable_arity" not in entry:
            continue
        result = "Void" if entry.get("callable_result") == "void" else "I64"
        lines.extend(
            [
                "    RuntimeCallableImportSpec {\n",
                f'        runtime_name: "{entry["runtime_name"]}",\n',
                f'        import_name: "{entry["name"]}",\n',
                f'        arity: {entry["callable_arity"]},\n',
                f"        result: RuntimeCallableResult::{result},\n",
                "    },\n",
            ]
        )
    lines.append("];\n\n")
    lines.extend(
        [
            "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n",
            "pub(crate) struct ReservedRuntimeCallableSpec {\n",
            "    pub(crate) index: u32,\n",
            "    pub(crate) runtime_name: &'static str,\n",
            "    pub(crate) import_name: &'static str,\n",
            "    pub(crate) arity: usize,\n",
            "}\n\n",
            "pub(crate) const RESERVED_RUNTIME_CALLABLE_SPECS: &[ReservedRuntimeCallableSpec] = &[\n",
        ]
    )
    for entry in data.get("reserved_runtime_callable", []):
        lines.extend(
            [
                "    ReservedRuntimeCallableSpec {\n",
                f"        index: {entry['index']},\n",
                f'        runtime_name: "{entry["runtime_name"]}",\n',
                f'        import_name: "{entry["import_name"]}",\n',
                f"        arity: {entry['callable_arity']},\n",
                "    },\n",
            ]
        )
    lines.extend(
        [
            "];\n\n",
            "pub(crate) const RESERVED_RUNTIME_CALLABLE_COUNT: u32 =\n",
            "    RESERVED_RUNTIME_CALLABLE_SPECS.len() as u32;\n\n",
        ]
    )
    return "".join(lines)


def render_runtime_callables_rs(data: dict) -> str:
    lines: list[str] = [_header("//")]
    poll_imports = sorted(
        (
            (entry["poll_table_slot"], entry["name"])
            for entry in data["import"]
            if "poll_table_slot" in entry
        ),
        key=lambda item: item[0],
    )
    reserved_callables = sorted(
        data.get("reserved_runtime_callable", []),
        key=lambda entry: entry["index"],
    )
    lines.extend(
        [
            "#![allow(dead_code)]\n\n",
            "use super::*;\n\n",
            "pub(crate) const RUNTIME_CALLABLE_KEY_BASE: u64 = 0xFFFF_FF00_0000_0000;\n",
            "pub(crate) const RUNTIME_POLL_CALLABLE_KEY_BASE: u64 =\n",
            "    RUNTIME_CALLABLE_KEY_BASE + 0x100;\n\n",
            "pub(crate) const WASM_POLL_SLOT_MAX_OFFSET: u64 = ",
            f"{max((slot for slot, _name in poll_imports), default=0)};\n\n",
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub(crate) const RESERVED_WASM_RUNTIME_CALLABLE_BASE: u64 = ",
            f"1 + {max((slot for slot, _name in poll_imports), default=0)};\n",
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub(crate) const RESERVED_WASM_RUNTIME_CALLABLE_COUNT: u64 = ",
            f"{len(reserved_callables)};\n",
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub(crate) const RESERVED_WASM_RUNTIME_TRAMPOLINE_BASE: u64 =\n",
            "    RESERVED_WASM_RUNTIME_CALLABLE_BASE + RESERVED_WASM_RUNTIME_CALLABLE_COUNT;\n\n",
            "#[inline]\n",
            "pub(crate) fn runtime_callable_key_from_symbol_name(symbol_name: &str) -> Option<u64> {\n",
            "    runtime_reserved_callable_key_from_symbol_name(symbol_name)\n",
            "        .or_else(|| runtime_poll_callable_key_from_symbol_name(symbol_name))\n",
            "}\n\n",
            "#[inline]\n",
            "pub(crate) fn wasm_poll_table_slot_from_symbol_name(symbol_name: &str) -> Option<u64> {\n",
            "    match symbol_name {\n",
        ]
    )
    for slot, import_name in poll_imports:
        lines.append(f'        "molt_{import_name}" => Some({slot}),\n')
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "#[inline]\n",
            "fn runtime_reserved_callable_key_from_symbol_name(symbol_name: &str) -> Option<u64> {\n",
            "    match symbol_name {\n",
        ]
    )
    for entry in reserved_callables:
        lines.append(
            f'        "{entry["runtime_name"]}" => '
            f"Some(RUNTIME_CALLABLE_KEY_BASE + {entry['index']}),\n"
        )
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "#[inline]\n",
            "fn runtime_poll_callable_key_from_symbol_name(symbol_name: &str) -> Option<u64> {\n",
            "    match symbol_name {\n",
        ]
    )
    for slot, import_name in poll_imports:
        lines.append(
            f'        "molt_{import_name}" => '
            f"Some(RUNTIME_POLL_CALLABLE_KEY_BASE + {slot}),\n"
        )
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "#[inline]\n",
            "pub(crate) fn runtime_callable_target_ptr(fn_ptr: u64) -> Option<*const ()> {\n",
            "    runtime_reserved_callable_target_ptr(fn_ptr)\n",
            "        .or_else(|| runtime_poll_callable_target_ptr(fn_ptr))\n",
            "}\n\n",
            "#[inline]\n",
            "fn runtime_reserved_callable_target_ptr(fn_ptr: u64) -> Option<*const ()> {\n",
            "    match fn_ptr.checked_sub(RUNTIME_CALLABLE_KEY_BASE)? {\n",
        ]
    )
    for entry in reserved_callables:
        lines.append(
            f"        {entry['index']} => Some({entry['runtime_name']} as *const ()),\n"
        )
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "#[inline]\n",
            "fn runtime_poll_callable_target_ptr(fn_ptr: u64) -> Option<*const ()> {\n",
            "    match fn_ptr.checked_sub(RUNTIME_POLL_CALLABLE_KEY_BASE)? {\n",
        ]
    )
    for slot, import_name in poll_imports:
        lines.append(
            f"        {slot} => Some(crate::molt_{import_name} as *const ()),\n"
        )
    lines.extend(
        [
            "        _ => None,\n",
            "    }\n",
            "}\n\n",
            "#[cfg(target_arch = \"wasm32\")]\n",
            "pub(crate) fn reserved_wasm_runtime_callable_info(\n",
            "    fn_ptr: u64,\n",
            ") -> Option<(u64, &'static str, &'static str, usize)> {\n",
        ]
    )
    for entry in reserved_callables:
        lines.extend(
            [
                f"    if fn_ptr == fn_addr!({entry['runtime_name']}) {{\n",
                "        return Some((\n",
                f"            {entry['index']},\n",
                f'            "{entry["runtime_name"]}",\n',
                f'            "{entry["import_name"]}",\n',
                f"            {entry['callable_arity']},\n",
                "        ));\n",
                "    }\n",
            ]
        )
    lines.extend(
        [
            "    None\n",
            "}\n\n",
            "#[cfg(test)]\n",
            "pub(crate) fn assert_reserved_runtime_symbols_resolve() {\n",
        ]
    )
    for entry in reserved_callables:
        lines.append(f"    let _ = {entry['runtime_name']} as *const ();\n")
    lines.extend(
        [
            "}\n",
        ]
    )
    return _rustfmt("wasm_callables_generated.rs", "".join(lines))


def _render_rs_pure_profile(data: dict) -> str:
    lines: list[str] = [_header("//")]
    lines.append("pub(crate) const PURE_PROFILE_SKIP_PREFIXES: &[&str] = &[\n")
    for entry in data.get("pure_skip_prefix", []):
        lines.append(f'    "{entry["prefix"]}",\n')
    lines.append("];\n\n")
    lines.extend(
        [
            "#[inline]\n",
            "pub(crate) fn pure_profile_skips_import(name: &str) -> bool {\n",
            "    PURE_PROFILE_SKIP_PREFIXES\n",
            "        .iter()\n",
            "        .any(|prefix| name.starts_with(prefix))\n",
            "}\n",
        ]
    )
    return "".join(lines)


def render_rs_modules(data: dict) -> dict[str, str]:
    modules = {
        "mod.rs": _render_rs_mod(),
        "call_indirect.rs": _render_rs_call_indirect(data),
        "const_policy.rs": _render_rs_const_policy(data),
        "static_types.rs": _render_rs_static_types(data),
        "imports.rs": _render_rs_imports(data),
        "runtime_surface.rs": _render_rs_runtime_surface(data),
        "runtime_callables.rs": _render_rs_runtime_callables(data),
        "pure_profile.rs": _render_rs_pure_profile(data),
    }
    return {name: _rustfmt(name, rendered) for name, rendered in modules.items()}


def render_py(data: dict) -> str:
    lines: list[str] = [_header("#")]
    lines.append(
        "WASM_STATIC_TYPES: tuple[tuple[tuple[str, ...], tuple[str, ...]], ...] = (\n"
    )
    for entry in data["static_type"]:
        lines.append(
            f"    ({_py_tuple(entry['params'])}, {_py_tuple(entry['results'])}),\n"
        )
    lines.append(")\n\n")
    lines.append(f"WASM_STATIC_TYPE_COUNT: int = {len(data['static_type'])}\n\n")
    lines.append("WASM_IMPORT_REGISTRY: tuple[str, ...] = (\n")
    for entry in data["import"]:
        lines.append(f'    "{entry["name"]}",\n')
    lines.append(")\n\n")
    poll_imports = sorted(
        (
            (entry["poll_table_slot"], entry["name"])
            for entry in data["import"]
            if "poll_table_slot" in entry
        ),
        key=lambda item: item[0],
    )
    lines.append("WASM_POLL_TABLE_IMPORTS: tuple[tuple[int, str], ...] = (\n")
    for slot, name in poll_imports:
        lines.append(f'    ({slot}, "{name}"),\n')
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLE_BASE: int = "
        "1 + max((slot for slot, _name in WASM_POLL_TABLE_IMPORTS), default=0)\n\n"
    )
    lines.append(
        "WASM_LEGACY_TABLE_BASE: int = "
        f"{data['table_layout']['legacy_table_base']}\n\n"
    )
    lines.append("WASM_RUNTIME_CALLABLE_IMPORTS: tuple[tuple[str, str, int, str], ...] = (\n")
    for entry in data["import"]:
        if "callable_arity" not in entry:
            continue
        result = entry.get("callable_result", "i64")
        lines.append(
            f'    ("{entry["runtime_name"]}", "{entry["name"]}", '
            f'{entry["callable_arity"]}, "{result}"),\n'
        )
    lines.append(")\n\n")
    lines.extend(
        [
            "WASM_RUNTIME_CALLABLE_IMPORT_BY_RUNTIME: dict[str, tuple[str, int, str]] = {\n",
            "    runtime_name: (import_name, arity, result)\n",
            "    for runtime_name, import_name, arity, result in WASM_RUNTIME_CALLABLE_IMPORTS\n",
            "}\n\n",
            "WASM_RUNTIME_CALLABLE_IMPORT_BY_IMPORT: dict[str, tuple[str, int, str]] = {\n",
            "    import_name: (runtime_name, arity, result)\n",
            "    for runtime_name, import_name, arity, result in WASM_RUNTIME_CALLABLE_IMPORTS\n",
            "}\n\n",
            "def wasm_runtime_callable_spec(runtime_name: str) -> tuple[str, int, str] | None:\n",
            "    return WASM_RUNTIME_CALLABLE_IMPORT_BY_RUNTIME.get(runtime_name)\n\n",
            "def wasm_runtime_callable_import_name(runtime_name: str) -> str | None:\n",
            "    spec = wasm_runtime_callable_spec(runtime_name)\n",
            "    return None if spec is None else spec[0]\n\n",
            "def wasm_runtime_callable_arity(runtime_name: str) -> int | None:\n",
            "    spec = wasm_runtime_callable_spec(runtime_name)\n",
            "    return None if spec is None else spec[1]\n\n",
            "def wasm_runtime_callable_result(runtime_name: str) -> str | None:\n",
            "    spec = wasm_runtime_callable_spec(runtime_name)\n",
            "    return None if spec is None else spec[2]\n\n",
        ]
    )
    lines.append(
        "WASM_IMPORT_SIGNATURES: tuple[tuple[str, tuple[str, ...], tuple[str, ...]], ...] = (\n"
    )
    static_types = data["static_type"]
    for entry in data["import"]:
        signature = static_types[entry["type"]]
        lines.append(
            f'    ("{entry["name"]}", {_py_tuple(signature["params"])}, '
            f'{_py_tuple(signature["results"])}),\n'
        )
    lines.append(")\n\n")
    lines.extend(
        [
            "WASM_IMPORT_SIGNATURE_BY_NAME: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {\n",
            "    name: (params, results)\n",
            "    for name, params, results in WASM_IMPORT_SIGNATURES\n",
            "}\n\n",
            "def wasm_import_signature(import_name: str) -> tuple[tuple[str, ...], tuple[str, ...]] | None:\n",
            "    return WASM_IMPORT_SIGNATURE_BY_NAME.get(import_name)\n\n",
            "def wasm_import_result_kind(import_name: str) -> str | None:\n",
            "    signature = wasm_import_signature(import_name)\n",
            "    if signature is None:\n",
            "        return None\n",
            "    results = signature[1]\n",
            "    return \"nil\" if not results else \", \".join(results)\n\n",
        ]
    )
    lines.append("WASM_CALL_INDIRECT_IMPORTS: tuple[str, ...] = (\n")
    for _arity, import_name in _call_indirect_imports(data):
        lines.append(f'    "{import_name}",\n')
    lines.append(")\n\n")
    lines.append(
        "WASM_CONST_OP_POLICIES: tuple[tuple[str, str, str | None, str, str, bool, bool, str, str], ...] = (\n"
    )
    for entry in data.get("const_op_policy", []):
        materializer = entry.get("materializer_import")
        materializer_repr = "None" if materializer is None else f'"{materializer}"'
        lines.append(
            f'    ("{entry["kind"]}", "{entry["inline_seed"]}", {materializer_repr}, '
            f'"{entry["literal_payload"]}", "{entry["scalar_payload"]}", '
            f'{entry["dispatch_runtime_seed"]}, '
            f'{entry["parse_scalar_literal"]}, "{entry["raw_int_effect"]}", '
            f'"{entry["lir_fast"]}"),\n'
        )
    lines.append(")\n\n")
    lines.append("WASM_REQUIRED_RUNTIME_IMPORT_PREFIXES: tuple[str, ...] = (\n")
    for entry in data.get("runtime_required_import_prefix", []):
        lines.append(f'    "{entry["prefix"]}",\n')
    lines.append(")\n\n")
    lines.append("WASM_REQUIRED_RUNTIME_IMPORT_SINGLETONS: tuple[str, ...] = (\n")
    for entry in data.get("runtime_required_import_singleton", []):
        lines.append(f'    "{entry["name"]}",\n')
    lines.append(")\n\n")
    lines.extend(
        [
            "def runtime_surface_requires_direct_import(kind: str) -> bool:\n",
            "    return any(\n",
            "        kind.startswith(prefix)\n",
            "        for prefix in WASM_REQUIRED_RUNTIME_IMPORT_PREFIXES\n",
            "    ) or kind in WASM_REQUIRED_RUNTIME_IMPORT_SINGLETONS\n\n",
        ]
    )
    lines.append("WASM_RESERVED_RUNTIME_CALLABLES: tuple[tuple[int, str, str, int], ...] = (\n")
    for entry in data.get("reserved_runtime_callable", []):
        lines.append(
            f'    ({entry["index"]}, "{entry["runtime_name"]}", '
            f'"{entry["import_name"]}", {entry["callable_arity"]}),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLE_COUNT: int = "
        "len(WASM_RESERVED_RUNTIME_CALLABLES)\n\n"
    )
    output_export_policy = data["output_export_policy"]
    lines.append(
        "WASM_OUTPUT_EXPORT_ALIAS_PREFIX: str = "
        f'"{output_export_policy["alias_prefix"]}"\n\n'
    )
    lines.append("WASM_OUTPUT_RUNTIME_EXPORT_ALIASES: tuple[str, ...] = (\n")
    for name in output_export_policy["runtime_export_aliases"]:
        lines.append(f'    "{name}",\n')
    lines.append(")\n\n")
    lines.append("WASM_INTERNAL_OUTPUT_EXPORT_PREFIXES: tuple[str, ...] = (\n")
    for prefix in output_export_policy["internal_output_export_prefixes"]:
        lines.append(f'    "{prefix}",\n')
    lines.append(")\n\n")
    lines.append("WASM_ESSENTIAL_EXPORTS: frozenset[str] = frozenset(\n")
    lines.append("    {\n")
    for name in output_export_policy["essential_exports"]:
        lines.append(f'        "{name}",\n')
    lines.append("    }\n")
    lines.append(")\n\n")
    lines.append("WASM_RUNTIME_HOST_EXPORTS: frozenset[str] = frozenset(\n")
    lines.append("    {\n")
    for name in data["runtime_export_policy"]["host_exports"]:
        lines.append(f'        "{name}",\n')
    lines.append("    }\n")
    lines.append(")\n\n")
    lines.append(
        "WASM_RUNTIME_IMPORT_FALLBACK_EXPORTS: tuple[tuple[str, tuple[str, ...]], ...] = (\n"
    )
    for entry in data.get("runtime_import_fallback", []):
        lines.append(
            f'    ("{entry["import"]}", {_py_tuple(entry["exports"])}),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RUNTIME_IMPORT_FALLBACK_SPECS: tuple[tuple[str, str, int | None, tuple[str, ...]], ...] = (\n"
    )
    for entry in data.get("runtime_import_fallback", []):
        call_arity = entry.get("call_arity")
        call_arity_repr = "None" if call_arity is None else str(call_arity)
        lines.append(
            f'    ("{entry["import"]}", "{entry["strategy"]}", '
            f"{call_arity_repr}, {_py_tuple(entry['exports'])}),\n"
        )
    lines.append(")\n\n")
    lines.append("WASM_LINK_ALLOWED_IMPORTS: tuple[str, ...] = (\n")
    for entry in data.get("link_allowed_import", []):
        lines.append(f'    "{entry["name"]}",\n')
    lines.append(")\n\n")
    lines.append(
        "WASM_STRIP_IMPORT_RULES: tuple[tuple[str, str, str, str], ...] = (\n"
    )
    for entry in data.get("strip_import_rule", []):
        lines.append(
            f'    ("{entry["module"]}", "{entry["name"]}", '
            f'"{entry["category"]}", "{entry["description"]}"),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_STRIP_IMPORT_PREFIX_RULES: tuple[tuple[str, str, str, str], ...] = (\n"
    )
    for entry in data.get("strip_import_prefix_rule", []):
        lines.append(
            f'    ("{entry["module"]}", "{entry["prefix"]}", '
            f'"{entry["category"]}", "{entry["description"]}"),\n'
        )
    lines.append(")\n\n")
    lines.append("PURE_PROFILE_SKIP_PREFIXES: tuple[str, ...] = (\n")
    for entry in data.get("pure_skip_prefix", []):
        lines.append(f'    "{entry["prefix"]}",\n')
    lines.append(")\n\n")
    lines.extend(
        [
            "def pure_profile_skips_import(name: str) -> bool:\n",
            "    return any(name.startswith(prefix) for prefix in PURE_PROFILE_SKIP_PREFIXES)\n",
        ]
    )
    return "".join(lines)


def render_table_layout_inc(data: dict) -> str:
    lines = [_header("//")]
    lines.append("#[allow(dead_code)]\n")
    lines.append(
        "pub(crate) const WASM_TABLE_BASE_FALLBACK: u64 = "
        f"{data['table_layout']['legacy_table_base']};\n"
    )
    return "".join(lines)


def render_allowed_imports(data: dict) -> str:
    lines = [_header("#")]
    for entry in data.get("link_allowed_import", []):
        lines.append(f'{entry["name"]}\n')
    return "".join(lines)


def _check(path: Path, rendered: str) -> bool:
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    if path.read_text(encoding="utf-8") != rendered:
        print(
            f"STALE generated file: {path}\n"
            "  run `python tools/gen_wasm_abi.py` to regenerate.",
            file=sys.stderr,
        )
        return False
    return True


def _check_absent(path: Path) -> bool:
    if path.exists():
        print(
            f"STALE removed generated file: {path}\n"
            "  run `python tools/gen_wasm_abi.py` to remove it.",
            file=sys.stderr,
        )
        return False
    return True


def _unexpected_rs_files() -> list[Path]:
    if not OUT_RS_DIR.exists():
        return []
    expected = set(OUT_RS_FILES.values())
    return sorted(
        path
        for path in OUT_RS_DIR.glob("*.rs")
        if path not in expected
    )


def _check_rs_modules(rendered_modules: dict[str, str]) -> bool:
    ok = True
    if LEGACY_OUT_RS.exists():
        print(
            f"STALE legacy generated file: {LEGACY_OUT_RS}\n"
            "  run `python tools/gen_wasm_abi.py` to regenerate split modules.",
            file=sys.stderr,
        )
        ok = False
    if set(rendered_modules) != set(OUT_RS_FILES):
        missing = sorted(set(OUT_RS_FILES) - set(rendered_modules))
        extra = sorted(set(rendered_modules) - set(OUT_RS_FILES))
        print(
            "BUG: Rust WASM ABI module renderer does not match OUT_RS_FILES "
            f"(missing={missing}, extra={extra})",
            file=sys.stderr,
        )
        ok = False
    for name, rendered in rendered_modules.items():
        path = OUT_RS_FILES[name]
        ok = _check(path, rendered) and ok
    for path in _unexpected_rs_files():
        print(
            f"STALE generated module: {path}\n"
            "  run `python tools/gen_wasm_abi.py` to remove stale split modules.",
            file=sys.stderr,
        )
        ok = False
    return ok


def _write_rs_modules(rendered_modules: dict[str, str]) -> None:
    if LEGACY_OUT_RS.exists():
        LEGACY_OUT_RS.unlink()
    OUT_RS_DIR.mkdir(parents=True, exist_ok=True)
    for path in _unexpected_rs_files():
        path.unlink()
    for name, rendered in rendered_modules.items():
        OUT_RS_FILES[name].write_text(rendered, encoding="utf-8", newline="\n")


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)

    data = load_manifest()
    rendered_rs_modules = render_rs_modules(data)
    rendered_runtime_callables_rs = render_runtime_callables_rs(data)
    rendered_py = render_py(data)
    rendered_table_layout_inc = render_table_layout_inc(data)
    rendered_allowed_imports = render_allowed_imports(data)
    if args.check:
        return (
            0
            if _check_rs_modules(rendered_rs_modules)
            and _check(OUT_RUNTIME_CALLABLES_RS, rendered_runtime_callables_rs)
            and _check(OUT_PY, rendered_py)
            and _check(OUT_TABLE_LAYOUT_INC, rendered_table_layout_inc)
            and _check(OUT_ALLOWED_IMPORTS, rendered_allowed_imports)
            and all(_check_absent(path) for path in REMOVED_GENERATED_FILES)
            else 1
        )
    _write_rs_modules(rendered_rs_modules)
    OUT_RUNTIME_CALLABLES_RS.write_text(
        rendered_runtime_callables_rs, encoding="utf-8", newline="\n"
    )
    OUT_PY.write_text(rendered_py, encoding="utf-8", newline="\n")
    OUT_TABLE_LAYOUT_INC.write_text(rendered_table_layout_inc, encoding="utf-8", newline="\n")
    OUT_ALLOWED_IMPORTS.write_text(
        rendered_allowed_imports, encoding="utf-8", newline="\n"
    )
    for path in REMOVED_GENERATED_FILES:
        path.unlink(missing_ok=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
