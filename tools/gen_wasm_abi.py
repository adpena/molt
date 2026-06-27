#!/usr/bin/env python3
"""Generate WASM ABI/import registry artifacts from the canonical manifest."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback
    import tomli as tomllib  # type: ignore[no-redef]

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_manifest.toml"
OUT_RS = ROOT / "runtime/molt-backend-wasm/src/wasm_abi_generated.rs"
OUT_PY = ROOT / "src/molt/_wasm_abi_generated.py"
OUT_TABLE_LAYOUT_INC = ROOT / "runtime/wasm_table_layout.inc"
OUT_POLL_INC = ROOT / "runtime/wasm_poll_callables.inc"
OUT_RESERVED_INC = ROOT / "runtime/wasm_runtime_callables.inc"


class WasmAbiManifestError(ValueError):
    pass


def load_manifest(path: Path = MANIFEST) -> dict:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    table_layout = data.get("table_layout")
    if not isinstance(table_layout, dict):
        raise WasmAbiManifestError("manifest must define [table_layout]")
    legacy_table_base = table_layout.get("legacy_table_base")
    if not isinstance(legacy_table_base, int) or legacy_table_base <= 0:
        raise WasmAbiManifestError("[table_layout].legacy_table_base must be positive")
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
    return data


def _header(comment: str) -> str:
    return (
        f"{comment} @generated by tools/gen_wasm_abi.py from\n"
        f"{comment} runtime/molt-backend-wasm/src/wasm_abi_manifest.toml\n"
        f"{comment} DO NOT EDIT BY HAND.\n\n"
    )


def render_rs(data: dict) -> str:
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


def render_py(data: dict) -> str:
    lines: list[str] = [_header("#")]
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


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)

    data = load_manifest()
    rendered_rs = render_rs(data)
    rendered_py = render_py(data)
    rendered_table_layout_inc = render_table_layout_inc(data)
    rendered_poll_inc = render_poll_inc(data)
    rendered_reserved_inc = render_reserved_inc(data)
    if args.check:
        return (
            0
            if _check(OUT_RS, rendered_rs)
            and _check(OUT_PY, rendered_py)
            and _check(OUT_TABLE_LAYOUT_INC, rendered_table_layout_inc)
            and _check(OUT_POLL_INC, rendered_poll_inc)
            and _check(OUT_RESERVED_INC, rendered_reserved_inc)
            else 1
        )
    OUT_RS.write_text(rendered_rs, encoding="utf-8", newline="\n")
    OUT_PY.write_text(rendered_py, encoding="utf-8", newline="\n")
    OUT_TABLE_LAYOUT_INC.write_text(rendered_table_layout_inc, encoding="utf-8", newline="\n")
    OUT_POLL_INC.write_text(rendered_poll_inc, encoding="utf-8", newline="\n")
    OUT_RESERVED_INC.write_text(rendered_reserved_inc, encoding="utf-8", newline="\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
