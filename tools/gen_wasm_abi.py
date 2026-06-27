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
    "static_types.rs": OUT_RS_DIR / "static_types.rs",
    "imports.rs": OUT_RS_DIR / "imports.rs",
    "runtime_surface.rs": OUT_RS_DIR / "runtime_surface.rs",
    "runtime_callables.rs": OUT_RS_DIR / "runtime_callables.rs",
    "pure_profile.rs": OUT_RS_DIR / "pure_profile.rs",
}
OUT_PY = ROOT / "src/molt/_wasm_abi_generated.py"
OUT_TABLE_LAYOUT_INC = ROOT / "runtime/wasm_table_layout.inc"
OUT_POLL_INC = ROOT / "runtime/wasm_poll_callables.inc"
OUT_RESERVED_INC = ROOT / "runtime/wasm_runtime_callables.inc"
OUT_ALLOWED_IMPORTS = ROOT / "tools/wasm_allowed_imports.txt"
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


class WasmAbiManifestError(ValueError):
    pass


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


def load_manifest(path: Path = MANIFEST) -> dict:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
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
    expected_poll_slots = set(range(1, len(seen_poll_slots) + 1))
    if seen_poll_slots != expected_poll_slots:
        raise WasmAbiManifestError("poll table slots must be contiguous from one")

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


def _render_rs_mod() -> str:
    lines: list[str] = [_header("//")]
    lines.extend(
        [
            "mod imports;\n",
            "mod pure_profile;\n",
            "mod runtime_callables;\n",
            "mod runtime_surface;\n",
            "mod static_types;\n\n",
            "pub(crate) use imports::{IMPORT_REGISTRY, OP_IMPORT_DEPS};\n",
            "pub(crate) use pure_profile::pure_profile_skips_import;\n",
            "pub(crate) use runtime_callables::{\n",
            "    POLL_TABLE_FUNCS, RUNTIME_CALLABLE_IMPORTS, RuntimeCallableResult,\n",
            "};\n",
            "pub(crate) use runtime_surface::runtime_surface_requires_direct_import;\n",
            "pub(crate) use static_types::{\n",
            "    STATIC_FUNC_TYPES, STATIC_TYPE_COUNT,\n",
            "};\n",
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
    lines.append("pub(crate) const POLL_TABLE_FUNCS: &[&str] = &[\n")
    for _slot, name in poll_imports:
        lines.append(f'    "{name}",\n')
    lines.append("];\n\n")
    lines.extend(
        [
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
    return "".join(lines)


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
    lines.append("WASM_POLL_TABLE_IMPORTS: tuple[str, ...] = (\n")
    for _slot, name in poll_imports:
        lines.append(f'    "{name}",\n')
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLE_BASE: int = "
        "1 + len(WASM_POLL_TABLE_IMPORTS)\n\n"
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


def render_poll_inc(data: dict) -> str:
    lines = [_header("//")]
    poll_imports = sorted(
        (
            (entry["poll_table_slot"], entry["name"])
            for entry in data["import"]
            if "poll_table_slot" in entry
        ),
        key=lambda item: item[0],
    )
    lines.append("entry_list! {\n")
    for slot, import_name in poll_imports:
        lines.append(f'    ({slot}, molt_{import_name}, "{import_name}")\n')
    lines.append("}\n")
    return "".join(lines)


def render_reserved_inc(data: dict) -> str:
    lines = [_header("//")]
    lines.append("entry_list! {\n")
    for entry in data.get("reserved_runtime_callable", []):
        lines.append(
            f'    ({entry["index"]}, {entry["runtime_name"]}, '
            f'"{entry["import_name"]}", {entry["callable_arity"]})\n'
        )
    lines.append("}\n")
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
    rendered_py = render_py(data)
    rendered_table_layout_inc = render_table_layout_inc(data)
    rendered_poll_inc = render_poll_inc(data)
    rendered_reserved_inc = render_reserved_inc(data)
    rendered_allowed_imports = render_allowed_imports(data)
    if args.check:
        return (
            0
            if _check_rs_modules(rendered_rs_modules)
            and _check(OUT_PY, rendered_py)
            and _check(OUT_TABLE_LAYOUT_INC, rendered_table_layout_inc)
            and _check(OUT_POLL_INC, rendered_poll_inc)
            and _check(OUT_RESERVED_INC, rendered_reserved_inc)
            and _check(OUT_ALLOWED_IMPORTS, rendered_allowed_imports)
            else 1
        )
    _write_rs_modules(rendered_rs_modules)
    OUT_PY.write_text(rendered_py, encoding="utf-8", newline="\n")
    OUT_TABLE_LAYOUT_INC.write_text(rendered_table_layout_inc, encoding="utf-8", newline="\n")
    OUT_POLL_INC.write_text(rendered_poll_inc, encoding="utf-8", newline="\n")
    OUT_RESERVED_INC.write_text(rendered_reserved_inc, encoding="utf-8", newline="\n")
    OUT_ALLOWED_IMPORTS.write_text(
        rendered_allowed_imports, encoding="utf-8", newline="\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
