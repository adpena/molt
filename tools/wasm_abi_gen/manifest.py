"""Manifest loading, normalization, and validation for WASM ABI generation."""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - Python < 3.11 fallback
    import tomli as tomllib  # type: ignore[no-redef]

from wasm_abi_gen.paths import (
    INTRINSIC_CATEGORIES,
    INTRINSICS_MANIFEST,
    MANIFEST,
    RUNTIME_ROOT,
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
    "materialize",
}
OP_LOOP_RUNTIME_SINKS = {
    "result_or_drop": "ResultOrDrop",
    "non_none_result_or_drop": "NonNoneResultOrDrop",
    "drop": "Drop",
    "none": "None",
}
CONTAINER_RUNTIME_SELECTOR_OPS = {
    "contains",
    "index",
    "len",
    "store_index",
}
CONTAINER_RUNTIME_SELECTOR_FACTS = {
    "dict",
    "flat_list_int",
    "list",
    "set",
    "str",
    "tuple",
}
WASM_BULK_MEMORY_INSTRUCTIONS = {
    "memory_copy": "Copy",
    "memory_fill": "Fill",
}
OBJECT_NEW_BOUND_SELECTOR_PAYLOADS = ("unsized", "sized")
METHOD_IC_SELECTOR_FAMILIES = ("method", "super_method")
METHOD_IC_MAX_EXTRA_ARGS = 4
NUMERIC_OP_LOOP_VARIANTS = (
    "Add",
    "Sub",
    "Mul",
    "TrueDiv",
    "FloorDiv",
    "Mod",
    "Matmul",
    "Pow",
    "PowMod",
    "Round",
    "Trunc",
    "BitAnd",
    "BitOr",
    "BitXor",
    "Invert",
    "Neg",
    "Pos",
    "LShift",
    "RShift",
    "Lt",
    "Le",
    "Gt",
    "Ge",
    "Eq",
    "Ne",
    "StringEq",
    "VectorReduction",
)
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


def _intrinsic_signature_rows() -> list[tuple[str, int, str]]:
    tree = ast.parse(INTRINSICS_MANIFEST.read_text(encoding="utf-8"))
    rows: list[tuple[str, int, str]] = []
    for node in tree.body:
        if not isinstance(node, ast.FunctionDef) or not node.name.startswith("molt_"):
            continue
        args = node.args
        arity = (
            len(args.posonlyargs)
            + len(args.args)
            + len(args.kwonlyargs)
            + (1 if args.vararg is not None else 0)
            + (1 if args.kwarg is not None else 0)
        )
        rows.append((node.name, arity, "i64"))
    return rows


def _static_type_index_by_signature(
    static_types: list[dict],
) -> dict[tuple[tuple[str, ...], tuple[str, ...]], int]:
    return {
        (tuple(entry["params"]), tuple(entry["results"])): idx
        for idx, entry in enumerate(static_types)
    }


def _runtime_callable_signature(arity: int, result: str) -> tuple[tuple[str, ...], tuple[str, ...]]:
    params = ("i64",) * arity
    results = () if result == "void" else ("i64",)
    return params, results


def _format_runtime_callable_signature(
    runtime_name: str, params: tuple[str, ...], result: str
) -> str:
    return f"{runtime_name}({', '.join(params)}) -> {result}"


def _intrinsic_manifest_names() -> set[str]:
    return {name for name, _, _ in _intrinsic_signature_rows()}


def _load_runtime_feature_gates_from_categories() -> list[tuple[str, str]]:
    raw = INTRINSIC_CATEGORIES.read_bytes()
    data = tomllib.loads(raw.decode())
    gates: list[tuple[str, str]] = []
    for _mod_name, mod_data in data.get("stdlib", {}).items():
        feature = mod_data.get("feature")
        if not isinstance(feature, str) or not feature:
            continue
        raw_prefixes = mod_data.get("feature_prefixes", mod_data.get("prefixes", []))
        if not isinstance(raw_prefixes, list):
            raise WasmAbiManifestError(
                "intrinsic categories feature_prefixes/prefixes must be lists"
            )
        for prefix in raw_prefixes:
            if not isinstance(prefix, str) or not prefix:
                raise WasmAbiManifestError(
                    "intrinsic categories feature prefixes must be non-empty strings"
                )
            gates.append((f"molt_{prefix}", feature))
    return gates


def _runtime_feature_gate_for_symbol(
    symbol: str,
    gates: list[tuple[str, str]],
) -> str | None:
    best: tuple[int, str] | None = None
    for prefix, feature in gates:
        if symbol.startswith(prefix):
            prefix_len = len(prefix)
            if best is None or prefix_len > best[0]:
                best = (prefix_len, feature)
    return best[1] if best is not None else None


def _annotate_runtime_callable_features(imports: list[dict]) -> None:
    gates = _load_runtime_feature_gates_from_categories()
    for idx, entry in enumerate(imports):
        if "runtime_feature" in entry:
            raise WasmAbiManifestError(
                "runtime_feature is generated from intrinsics/categories.toml; "
                f"remove manual runtime_feature from import entry {idx}"
            )
        runtime_name = entry.get("runtime_name")
        if not isinstance(runtime_name, str):
            continue
        feature = _runtime_feature_gate_for_symbol(runtime_name, gates)
        if feature is not None:
            entry["runtime_feature"] = feature


def _runtime_rust_files() -> list[Path]:
    roots = [
        child
        for child in RUNTIME_ROOT.iterdir()
        if child.is_dir() and child.name.startswith("molt-runtime")
    ]
    return sorted(path for root in roots for path in root.rglob("*.rs"))


def _read_runtime_rust_source(path: Path) -> str | None:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        # Atomic generator writes can briefly expose temp paths in rglob()
        # results. The ABI authority is the stable Rust source set; ignore
        # vanished temp files rather than failing nondeterministically.
        return None


def _rust_type_aliases() -> dict[str, str]:
    aliases: dict[str, str] = {}
    alias_re = re.compile(r"(?:pub\s+)?type\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*([^;]+);")
    for path in _runtime_rust_files():
        text = _read_runtime_rust_source(path)
        if text is None:
            continue
        for match in alias_re.finditer(text):
            aliases[match.group(1)] = match.group(2).strip()
    return aliases


def _normalize_rust_wasm_scalar(typ: str, aliases: dict[str, str]) -> str:
    typ = typ.strip().removeprefix("mut ").strip()
    seen: set[str] = set()
    while typ in aliases and typ not in seen:
        seen.add(typ)
        typ = aliases[typ].strip()
    if typ in {"u64", "i64"}:
        return "i64"
    if typ in {"()", ""}:
        return "void"
    return typ


def _rust_export_signatures() -> dict[str, set[tuple[tuple[str, ...], str]]]:
    aliases = _rust_type_aliases()
    fn_re = re.compile(
        r'pub\s+(?:unsafe\s+)?extern\s+"C"\s+fn\s+'
        r"(molt_[A-Za-z0-9_]+)\s*\((.*?)\)\s*(?:->\s*([^{\n]+?))?\s*\{",
        re.S,
    )
    exports: dict[str, set[tuple[tuple[str, ...], str]]] = {}
    for path in _runtime_rust_files():
        text = _read_runtime_rust_source(path)
        if text is None:
            continue
        for match in fn_re.finditer(text):
            params_text = match.group(2).strip()
            params: list[str] = []
            if params_text:
                for raw_param in params_text.split(","):
                    param = raw_param.strip()
                    if not param:
                        continue
                    if ":" not in param:
                        params.append(param)
                        continue
                    params.append(
                        _normalize_rust_wasm_scalar(
                            param.rsplit(":", 1)[1].strip(),
                            aliases,
                        )
                    )
            result = _normalize_rust_wasm_scalar(match.group(3) or "void", aliases)
            exports.setdefault(match.group(1), set()).add((tuple(params), result))
    return exports


def _rust_intrinsic_callable_result(
    rust_exports: dict[str, set[tuple[tuple[str, ...], str]]],
    runtime_name: str,
    arity: int,
) -> str:
    signatures = rust_exports.get(runtime_name, set())
    expected_params = ("i64",) * arity
    compatible = {
        result
        for params, result in signatures
        if params == expected_params and result in {"i64", "void"}
    }
    return next(iter(compatible)) if len(compatible) == 1 else "i64"


def _intrinsic_runtime_callable_imports(
    static_types: list[dict],
    imports: list[dict],
    non_runtime_callable_intrinsics: set[str],
) -> list[dict]:
    rust_exports = _rust_export_signatures()
    type_indices = _static_type_index_by_signature(static_types)
    explicit_imports_by_name = {
        entry["name"]: entry for entry in imports if isinstance(entry.get("name"), str)
    }
    explicit_runtime_names = {
        entry["runtime_name"]
        for entry in imports
        if isinstance(entry.get("runtime_name"), str)
    }
    synthesized: list[dict] = []
    missing_static_types: list[str] = []
    for runtime_name, arity, result in _intrinsic_signature_rows():
        if runtime_name in explicit_runtime_names:
            if runtime_name in non_runtime_callable_intrinsics:
                raise WasmAbiManifestError(
                    f"intrinsic {runtime_name!r} is both callable and "
                    "non-runtime-callable"
                )
            continue
        import_name = runtime_name.removeprefix("molt_")
        if runtime_name in non_runtime_callable_intrinsics:
            existing_entry = explicit_imports_by_name.get(import_name)
            if existing_entry is not None and existing_entry.get("runtime_name") is not None:
                raise WasmAbiManifestError(
                    f"intrinsic {runtime_name!r} is listed as "
                    "non-runtime-callable but its explicit import has "
                    "runtime_name"
                )
            continue
        result = _rust_intrinsic_callable_result(rust_exports, runtime_name, arity)
        params, results = _runtime_callable_signature(arity, result)
        type_idx = type_indices.get((params, results))
        if type_idx is None:
            missing_static_types.append(
                _format_runtime_callable_signature(runtime_name, params, result)
            )
            continue
        existing_entry = explicit_imports_by_name.get(import_name)
        if existing_entry is not None:
            existing_runtime = existing_entry.get("runtime_name")
            if existing_runtime not in (None, runtime_name):
                raise WasmAbiManifestError(
                    f"intrinsic runtime callable import name {import_name!r} "
                    f"maps to {runtime_name!r}, but explicit row maps to "
                    f"{existing_runtime!r}"
                )
            existing_type = existing_entry.get("type")
            if not isinstance(existing_type, int) or not (
                0 <= existing_type < len(static_types)
            ):
                raise WasmAbiManifestError(
                    f"explicit import {import_name!r} has invalid static type "
                    f"{existing_type!r}"
                )
            existing_signature = static_types[existing_type]
            if tuple(existing_signature["params"]) != params:
                raise WasmAbiManifestError(
                    f"intrinsic {runtime_name!r} collides with explicit import "
                    f"{import_name!r} using params "
                    f"{existing_signature['params']!r}; list it in "
                    "non_runtime_callable_intrinsic if that import is a raw "
                    "non-callable ABI"
                )
            existing_results = tuple(existing_signature["results"])
            if existing_results not in ((), ("i64",)):
                raise WasmAbiManifestError(
                    f"intrinsic {runtime_name!r} collides with explicit import "
                    f"{import_name!r} using results "
                    f"{existing_signature['results']!r}; list it in "
                    "non_runtime_callable_intrinsic if that import is a raw "
                    "non-callable ABI"
                )
            existing_entry["runtime_name"] = runtime_name
            existing_entry["callable_arity"] = arity
            if not existing_results:
                existing_entry["callable_result"] = "void"
            explicit_runtime_names.add(runtime_name)
            continue
        entry = {
            "name": import_name,
            "type": type_idx,
            "runtime_name": runtime_name,
            "callable_arity": arity,
        }
        if result == "void":
            entry["callable_result"] = "void"
        synthesized.append(entry)
        explicit_runtime_names.add(runtime_name)
    if missing_static_types:
        raise WasmAbiManifestError(
            "intrinsic runtime callables need WASM static_type rows: "
            + "; ".join(missing_static_types)
        )
    return synthesized


def _validate_reserved_runtime_callables(data: dict) -> list[dict]:
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
    return reserved_callables


def _non_reserved_import_name_references(data: dict, reserved_import_names: set[str]) -> set[str]:
    refs: set[str] = set()

    def visit(value: object) -> None:
        if isinstance(value, dict):
            import_name = value.get("import_name")
            if isinstance(import_name, str) and import_name in reserved_import_names:
                refs.add(import_name)
            deps = value.get("deps")
            if isinstance(deps, list):
                refs.update(
                    dep
                    for dep in deps
                    if isinstance(dep, str) and dep in reserved_import_names
                )
            for child in value.values():
                visit(child)
        elif isinstance(value, list):
            for child in value:
                visit(child)

    for section, value in data.items():
        if section in {"import", "reserved_runtime_callable"}:
            continue
        visit(value)
    return refs


def _validate_reserved_runtime_callable_import_absence(
    static_types: list[dict],
    imports: list[dict],
    reserved_callables: list[dict],
    non_reserved_import_refs: set[str],
) -> None:
    type_indices = _static_type_index_by_signature(static_types)
    explicit_imports_by_name = {
        entry["name"]: entry for entry in imports if isinstance(entry.get("name"), str)
    }
    explicit_runtime_names = {
        entry["runtime_name"]
        for entry in imports
        if isinstance(entry.get("runtime_name"), str)
    }
    missing_static_types: list[str] = []
    for entry in reserved_callables:
        runtime_name = entry["runtime_name"]
        import_name = entry["import_name"]
        arity = entry["callable_arity"]
        params, results = _runtime_callable_signature(arity, "i64")
        type_idx = type_indices.get((params, results))
        if type_idx is None:
            missing_static_types.append(
                _format_runtime_callable_signature(runtime_name, params, "i64")
            )
            continue
        existing_entry = explicit_imports_by_name.get(import_name)
        if existing_entry is not None:
            if import_name not in non_reserved_import_refs:
                raise WasmAbiManifestError(
                    f"reserved runtime callable {runtime_name!r} import name "
                    f"{import_name!r} must be owned only by reserved_runtime_callable, "
                    "not duplicated in [[import]]"
                )
            existing_type = existing_entry.get("type")
            if not isinstance(existing_type, int) or not (
                0 <= existing_type < len(static_types)
            ):
                raise WasmAbiManifestError(
                    f"reserved runtime callable {runtime_name!r} dual-use import "
                    f"{import_name!r} has invalid static type {existing_type!r}"
                )
            existing_signature = static_types[existing_type]
            if (
                tuple(existing_signature["params"]) != params
                or tuple(existing_signature["results"]) != results
            ):
                raise WasmAbiManifestError(
                    f"reserved runtime callable {runtime_name!r} dual-use import "
                    f"{import_name!r} uses static type {existing_type}; expected "
                    f"{_format_runtime_callable_signature(runtime_name, params, 'i64')}"
                )
            if (
                existing_entry.get("runtime_name") is not None
                or existing_entry.get("callable_arity") is not None
                or existing_entry.get("callable_result") is not None
            ):
                raise WasmAbiManifestError(
                    f"reserved runtime callable {runtime_name!r} dual-use import "
                    "must not duplicate callable metadata in [[import]]"
                )
        if runtime_name in explicit_runtime_names:
            raise WasmAbiManifestError(
                f"reserved runtime callable {runtime_name!r} must be owned only "
                "by reserved_runtime_callable, not duplicated in [[import]]"
            )
    if missing_static_types:
        raise WasmAbiManifestError(
            "reserved runtime callables need WASM static_type rows: "
            + "; ".join(missing_static_types)
        )


def _validate_intrinsic_runtime_callable_export_abi(imports: list[dict]) -> None:
    intrinsic_names = _intrinsic_manifest_names()
    rust_exports = _rust_export_signatures()
    mismatches: list[str] = []
    missing: list[str] = []
    for entry in imports:
        runtime_name = entry.get("runtime_name")
        if runtime_name not in intrinsic_names or "callable_arity" not in entry:
            continue
        signatures = rust_exports.get(runtime_name)
        if not signatures:
            missing.append(runtime_name)
            continue
        expected_params = ("i64",) * entry["callable_arity"]
        expected_result = entry.get("callable_result", "i64")
        expected = (expected_params, expected_result)
        if signatures != {expected}:
            rendered = ", ".join(
                f"({', '.join(params)}) -> {result}"
                for params, result in sorted(signatures)
            )
            mismatches.append(
                f"{runtime_name}: manifest ({', '.join(expected_params)}) -> "
                f"{expected_result}; Rust {rendered}"
            )
    if missing:
        raise WasmAbiManifestError(
            "intrinsic runtime callables missing Rust exports: "
            + ", ".join(sorted(missing))
        )
    if mismatches:
        raise WasmAbiManifestError(
            "intrinsic runtime callable ABI mismatches: " + "; ".join(mismatches)
        )


def _validate_op_loop_runtime_arg(section: str, idx: int, arg_idx: int, arg: object) -> str:
    if not isinstance(arg, str) or not arg:
        raise WasmAbiManifestError(
            f"{section} entry {idx} arg {arg_idx} must be a non-empty string"
        )
    if arg.startswith("local:"):
        local_idx = arg.removeprefix("local:")
        if not local_idx.isdecimal() or str(int(local_idx)) != local_idx:
            raise WasmAbiManifestError(
                f"{section} entry {idx} arg {arg_idx} has invalid local index {arg!r}"
            )
        return arg
    if arg.startswith("op_value_i64:"):
        message = arg.removeprefix("op_value_i64:")
        if not message:
            raise WasmAbiManifestError(
                f"{section} entry {idx} arg {arg_idx} has empty op_value_i64 message"
            )
        return arg
    raise WasmAbiManifestError(
        f"{section} entry {idx} arg {arg_idx} has invalid arg form {arg!r}"
    )


def _expand_op_loop_runtime_calls(data: dict) -> list[dict]:
    expanded: list[dict] = []
    explicit = data.get("op_loop_runtime_call", [])
    if not isinstance(explicit, list):
        raise WasmAbiManifestError("op_loop_runtime_call must be a list of tables")
    for idx, entry in enumerate(explicit):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"op_loop_runtime_call entry {idx} must be a table")
        expanded.append(dict(entry))

    groups = data.get("op_loop_runtime_call_group", [])
    if not isinstance(groups, list):
        raise WasmAbiManifestError("op_loop_runtime_call_group must be a list of tables")
    for idx, entry in enumerate(groups):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"op_loop_runtime_call_group entry {idx} must be a table"
            )
        kinds = _validate_string_list(
            f"op_loop_runtime_call_group entry {idx}", "kinds", entry.get("kinds")
        )
        arg_count = entry.get("arg_count")
        if not isinstance(arg_count, int) or arg_count < 0:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call_group entry {idx} has invalid arg_count"
            )
        sink = entry.get("sink")
        if sink not in OP_LOOP_RUNTIME_SINKS:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call_group entry {idx} has invalid sink {sink!r}"
            )
        import_name = entry.get("import_name")
        if import_name is not None and (not isinstance(import_name, str) or not import_name):
            raise WasmAbiManifestError(
                f"op_loop_runtime_call_group entry {idx} has invalid import_name"
            )
        args = [f"local:{arg_idx}" for arg_idx in range(arg_count)]
        for kind in kinds:
            expanded_import_name = import_name or kind
            expanded.append(
                {
                    "kind": kind,
                    "import_name": expanded_import_name,
                    "args": args,
                    "required_imports": [expanded_import_name],
                    "sink": sink,
                }
            )
    return expanded


def _validate_numeric_runtime_selectors(
    data: dict,
    seen_imports: set[str],
    lir_import_by_variant: dict[str, str],
) -> list[dict]:
    selectors = data.get("numeric_runtime_selector", [])
    if not isinstance(selectors, list):
        raise WasmAbiManifestError("numeric_runtime_selector must be a list of tables")
    seen_kinds: set[str] = set()
    for idx, entry in enumerate(selectors):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"numeric_runtime_selector entry {idx} must be a table"
            )
        kind = entry.get("kind")
        if not isinstance(kind, str) or not kind:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector entry {idx} has invalid kind"
            )
        if kind in seen_kinds:
            raise WasmAbiManifestError(
                f"duplicate numeric_runtime_selector kind {kind!r}"
            )
        seen_kinds.add(kind)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} has invalid import_name"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} references unknown import "
                f"{import_name!r}"
            )
        op_loop_variant = entry.get("op_loop_variant")
        if op_loop_variant not in NUMERIC_OP_LOOP_VARIANTS:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} has invalid "
                f"op_loop_variant {op_loop_variant!r}"
            )
        deps = entry.get("deps")
        if deps is None:
            deps = [import_name]
        else:
            deps = _validate_string_list(
                f"numeric_runtime_selector {kind!r}", "deps", deps
            )
        if import_name not in deps:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} deps must include selected "
                f"import {import_name!r}"
            )
        for dep in deps:
            if dep not in seen_imports:
                raise WasmAbiManifestError(
                    f"numeric_runtime_selector {kind!r} deps reference "
                    f"unknown import {dep!r}"
                )
        entry["deps"] = deps
        lir_variant = entry.get("lir_variant")
        lir_operand_count = entry.get("lir_operand_count")
        if lir_variant is None:
            if lir_operand_count is not None:
                raise WasmAbiManifestError(
                    f"numeric_runtime_selector {kind!r} cannot define "
                    "lir_operand_count without lir_variant"
                )
            continue
        if not isinstance(lir_variant, str) or lir_variant not in lir_import_by_variant:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} has invalid "
                f"lir_variant {lir_variant!r}"
            )
        if lir_import_by_variant[lir_variant] != import_name:
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} import {import_name!r} "
                f"does not match lir_variant {lir_variant!r} import "
                f"{lir_import_by_variant[lir_variant]!r}"
            )
        if lir_operand_count is not None and (
            not isinstance(lir_operand_count, int) or lir_operand_count < 0
        ):
            raise WasmAbiManifestError(
                f"numeric_runtime_selector {kind!r} has invalid "
                "lir_operand_count"
            )
    return selectors


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


def _validate_rust_variant(section: str, idx: int, value: object) -> str:
    if not isinstance(value, str) or not value:
        raise WasmAbiManifestError(f"{section} entry {idx} has invalid variant")
    first = value[0]
    if not ("A" <= first <= "Z"):
        raise WasmAbiManifestError(
            f"{section} entry {idx} variant {value!r} must start with A-Z"
        )
    for char in value:
        if not (
            "A" <= char <= "Z"
            or "a" <= char <= "z"
            or "0" <= char <= "9"
            or char == "_"
        ):
            raise WasmAbiManifestError(
                f"{section} entry {idx} variant {value!r} is not an ASCII Rust variant"
            )
    return value


def validate_loaded_manifest(data: dict) -> dict:
    table_layout = data.get("table_layout")
    if not isinstance(table_layout, dict):
        raise WasmAbiManifestError("manifest must define [table_layout]")
    legacy_table_base = table_layout.get("legacy_table_base")
    if not isinstance(legacy_table_base, int) or legacy_table_base <= 0:
        raise WasmAbiManifestError("[table_layout].legacy_table_base must be positive")
    table_ref_export_prefix = table_layout.get("table_ref_export_prefix")
    if not isinstance(table_ref_export_prefix, str) or not table_ref_export_prefix:
        raise WasmAbiManifestError(
            "[table_layout].table_ref_export_prefix must be a non-empty string"
        )
    if not table_ref_export_prefix.isascii():
        raise WasmAbiManifestError(
            "[table_layout].table_ref_export_prefix must be ASCII"
        )
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
    reserved_callables = _validate_reserved_runtime_callables(data)
    reserved_import_names = {
        entry["import_name"] for entry in reserved_callables
    }
    non_reserved_import_refs = _non_reserved_import_name_references(
        data, reserved_import_names
    )
    non_runtime_callable_intrinsics = set(
        _validate_string_list(
            "non_runtime_callable_intrinsic",
            "entries",
            data.get("non_runtime_callable_intrinsic", []),
        )
    )
    unknown_non_callable = non_runtime_callable_intrinsics - _intrinsic_manifest_names()
    if unknown_non_callable:
        raise WasmAbiManifestError(
            "non_runtime_callable_intrinsic contains unknown intrinsics: "
            + ", ".join(sorted(unknown_non_callable))
        )
    imports.extend(
        _intrinsic_runtime_callable_imports(
            static_types,
            imports,
            non_runtime_callable_intrinsics,
        )
    )
    _annotate_runtime_callable_features(imports)
    _validate_reserved_runtime_callable_import_absence(
        static_types,
        imports,
        reserved_callables,
        non_reserved_import_refs,
    )
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
    runtime_import_alias_collisions = seen_imports & seen_runtime_callables
    if runtime_import_alias_collisions:
        raise WasmAbiManifestError(
            "runtime import aliases collide with canonical import names: "
            + ", ".join(sorted(runtime_import_alias_collisions))
        )
    if seen_poll_slots:
        expected_poll_slots = set(range(1, max(seen_poll_slots) + 1))
        if seen_poll_slots != expected_poll_slots:
            missing = sorted(expected_poll_slots - seen_poll_slots)
            raise WasmAbiManifestError(
                "poll_table_slot values must be contiguous from 1; "
                f"missing {missing}"
            )
    _validate_intrinsic_runtime_callable_export_abi(imports)

    lir_runtime_calls = data.get("lir_runtime_call", [])
    if not isinstance(lir_runtime_calls, list) or not lir_runtime_calls:
        raise WasmAbiManifestError("manifest must define lir_runtime_call entries")
    seen_lir_variants: set[str] = set()
    lir_import_by_variant: dict[str, str] = {}
    seen_lir_imports: set[str] = set()
    seen_lir_preserved_copy_imports: set[str] = set()
    for idx, entry in enumerate(lir_runtime_calls):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(f"lir_runtime_call entry {idx} must be a table")
        variant = _validate_rust_variant("lir_runtime_call", idx, entry.get("variant"))
        if variant in seen_lir_variants:
            raise WasmAbiManifestError(f"duplicate LIR runtime-call variant {variant!r}")
        seen_lir_variants.add(variant)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"lir_runtime_call {variant!r} has invalid import_name"
            )
        if import_name in seen_lir_imports:
            raise WasmAbiManifestError(
                f"duplicate LIR runtime-call import {import_name!r}"
            )
        seen_lir_imports.add(import_name)
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"lir_runtime_call {variant!r} references unknown import "
                f"{import_name!r}"
            )
        lir_import_by_variant[variant] = import_name
        boxed_operand_count = entry.get("boxed_operand_count")
        if boxed_operand_count is not None and (
            not isinstance(boxed_operand_count, int) or boxed_operand_count < 0
        ):
            raise WasmAbiManifestError(
                f"lir_runtime_call {variant!r} has invalid boxed_operand_count"
            )
        preserved_copy_operand_count = entry.get("preserved_copy_operand_count")
        if preserved_copy_operand_count is None:
            continue
        if (
            not isinstance(preserved_copy_operand_count, int)
            or preserved_copy_operand_count < 0
        ):
            raise WasmAbiManifestError(
                f"lir_runtime_call {variant!r} has invalid "
                "preserved_copy_operand_count"
            )
        if import_name in seen_lir_preserved_copy_imports:
            raise WasmAbiManifestError(
                f"duplicate preserved-Copy LIR runtime-call import {import_name!r}"
            )
        seen_lir_preserved_copy_imports.add(import_name)

    op_loop_runtime_calls = _expand_op_loop_runtime_calls(data)
    seen_op_loop_runtime_kinds: set[str] = set()
    seen_op_loop_lir_variants: set[str] = set()
    for idx, entry in enumerate(op_loop_runtime_calls):
        kind = entry.get("kind")
        if not isinstance(kind, str) or not kind:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call entry {idx} has invalid kind"
            )
        if kind in seen_op_loop_runtime_kinds:
            raise WasmAbiManifestError(
                f"duplicate op_loop_runtime_call kind {kind!r}"
            )
        seen_op_loop_runtime_kinds.add(kind)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} has invalid import_name"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} references unknown import {import_name!r}"
            )
        required_imports = entry.get("required_imports")
        if required_imports is None:
            required_imports = [import_name]
        if not isinstance(required_imports, list) or not required_imports:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} must define required_imports as a non-empty list"
            )
        required_seen: set[str] = set()
        normalized_required_imports: list[str] = []
        for required_idx, required in enumerate(required_imports):
            if not isinstance(required, str) or not required:
                raise WasmAbiManifestError(
                    f"op_loop_runtime_call {kind!r} has invalid required_imports entry {required_idx}"
                )
            if required in required_seen:
                raise WasmAbiManifestError(
                    f"op_loop_runtime_call {kind!r} repeats required import {required!r}"
                )
            if required not in seen_imports:
                raise WasmAbiManifestError(
                    f"op_loop_runtime_call {kind!r} required_imports references "
                    f"unknown import {required!r}"
                )
            required_seen.add(required)
            normalized_required_imports.append(required)
        if import_name not in required_seen:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} required_imports must include emitted import {import_name!r}"
            )
        entry["required_imports"] = normalized_required_imports
        sink = entry.get("sink")
        if sink not in OP_LOOP_RUNTIME_SINKS:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} has invalid sink {sink!r}"
            )
        args = entry.get("args")
        if not isinstance(args, list):
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} must define args as a list"
            )
        entry["args"] = [
            _validate_op_loop_runtime_arg("op_loop_runtime_call", idx, arg_idx, arg)
            for arg_idx, arg in enumerate(args)
        ]
        lir_variant = entry.get("lir_variant")
        lir_operand_count = entry.get("lir_operand_count")
        if lir_variant is None and lir_operand_count is None:
            continue
        if not isinstance(lir_variant, str) or lir_variant not in lir_import_by_variant:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} has invalid lir_variant {lir_variant!r}"
            )
        if lir_variant in seen_op_loop_lir_variants:
            raise WasmAbiManifestError(
                f"duplicate op_loop_runtime_call lir_variant {lir_variant!r}"
            )
        seen_op_loop_lir_variants.add(lir_variant)
        if lir_import_by_variant[lir_variant] != import_name:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} import {import_name!r} does not "
                f"match lir_variant {lir_variant!r} import "
                f"{lir_import_by_variant[lir_variant]!r}"
            )
        if not isinstance(lir_operand_count, int) or lir_operand_count < 0:
            raise WasmAbiManifestError(
                f"op_loop_runtime_call {kind!r} has invalid lir_operand_count"
            )
    data["op_loop_runtime_call"] = op_loop_runtime_calls
    data.pop("op_loop_runtime_call_group", None)

    bulk_memory_ops = data.get("wasm_bulk_memory_op", [])
    if not isinstance(bulk_memory_ops, list) or not bulk_memory_ops:
        raise WasmAbiManifestError("manifest must define wasm_bulk_memory_op entries")
    seen_bulk_memory_kinds: set[str] = set()
    for idx, entry in enumerate(bulk_memory_ops):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"wasm_bulk_memory_op entry {idx} must be a table"
            )
        kind = entry.get("kind")
        if not isinstance(kind, str) or not kind:
            raise WasmAbiManifestError(
                f"wasm_bulk_memory_op entry {idx} has invalid kind"
            )
        if kind in seen_bulk_memory_kinds:
            raise WasmAbiManifestError(f"duplicate wasm_bulk_memory_op kind {kind!r}")
        seen_bulk_memory_kinds.add(kind)
        instruction = entry.get("instruction")
        if instruction not in WASM_BULK_MEMORY_INSTRUCTIONS:
            raise WasmAbiManifestError(
                f"wasm_bulk_memory_op {kind!r} has invalid instruction "
                f"{instruction!r}"
            )
        arg_count = entry.get("arg_count")
        if not isinstance(arg_count, int) or arg_count != 3:
            raise WasmAbiManifestError(
                f"wasm_bulk_memory_op {kind!r} must use arg_count = 3"
            )

    container_runtime_selectors = data.get("container_runtime_selector", [])
    if not isinstance(container_runtime_selectors, list):
        raise WasmAbiManifestError(
            "container_runtime_selector must be a list of tables"
        )
    seen_container_selectors: set[tuple[str, str]] = set()
    for idx, entry in enumerate(container_runtime_selectors):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"container_runtime_selector entry {idx} must be a table"
            )
        op = entry.get("op")
        if not isinstance(op, str) or op not in CONTAINER_RUNTIME_SELECTOR_OPS:
            raise WasmAbiManifestError(
                f"container_runtime_selector entry {idx} has invalid op {op!r}"
            )
        fact = entry.get("fact")
        if not isinstance(fact, str) or fact not in CONTAINER_RUNTIME_SELECTOR_FACTS:
            raise WasmAbiManifestError(
                f"container_runtime_selector entry {idx} has invalid fact {fact!r}"
            )
        selector_key = (op, fact)
        if selector_key in seen_container_selectors:
            raise WasmAbiManifestError(
                f"duplicate container_runtime_selector {selector_key!r}"
            )
        seen_container_selectors.add(selector_key)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"container_runtime_selector {selector_key!r} has invalid import_name"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"container_runtime_selector {selector_key!r} references "
                f"unknown import {import_name!r}"
            )
        lir_variant = entry.get("lir_variant")
        if lir_variant is None:
            continue
        if not isinstance(lir_variant, str) or lir_variant not in lir_import_by_variant:
            raise WasmAbiManifestError(
                f"container_runtime_selector {selector_key!r} has invalid "
                f"lir_variant {lir_variant!r}"
            )
        if lir_import_by_variant[lir_variant] != import_name:
            raise WasmAbiManifestError(
                f"container_runtime_selector {selector_key!r} import "
                f"{import_name!r} does not match lir_variant {lir_variant!r} "
                f"import {lir_import_by_variant[lir_variant]!r}"
            )

    object_new_bound_selectors = data.get("object_new_bound_selector", [])
    if not isinstance(object_new_bound_selectors, list):
        raise WasmAbiManifestError(
            "object_new_bound_selector must be a list of tables"
        )
    expected_object_new_bound_payloads = set(OBJECT_NEW_BOUND_SELECTOR_PAYLOADS)
    seen_object_new_bound_payloads: set[str] = set()
    for idx, entry in enumerate(object_new_bound_selectors):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"object_new_bound_selector entry {idx} must be a table"
            )
        payload = entry.get("payload")
        if (
            not isinstance(payload, str)
            or payload not in expected_object_new_bound_payloads
        ):
            raise WasmAbiManifestError(
                f"object_new_bound_selector entry {idx} has invalid payload {payload!r}"
            )
        if payload in seen_object_new_bound_payloads:
            raise WasmAbiManifestError(
                f"duplicate object_new_bound_selector payload {payload!r}"
            )
        seen_object_new_bound_payloads.add(payload)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"object_new_bound_selector {payload!r} has invalid import_name"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"object_new_bound_selector {payload!r} references unknown import "
                f"{import_name!r}"
            )
        lir_variant = entry.get("lir_variant")
        if not isinstance(lir_variant, str) or lir_variant not in lir_import_by_variant:
            raise WasmAbiManifestError(
                f"object_new_bound_selector {payload!r} has invalid lir_variant "
                f"{lir_variant!r}"
            )
        if lir_import_by_variant[lir_variant] != import_name:
            raise WasmAbiManifestError(
                f"object_new_bound_selector {payload!r} import {import_name!r} "
                f"does not match lir_variant {lir_variant!r} import "
                f"{lir_import_by_variant[lir_variant]!r}"
            )
    if seen_object_new_bound_payloads != expected_object_new_bound_payloads:
        missing = sorted(expected_object_new_bound_payloads - seen_object_new_bound_payloads)
        extra = sorted(seen_object_new_bound_payloads - expected_object_new_bound_payloads)
        raise WasmAbiManifestError(
            "object_new_bound_selector must declare exactly payloads "
            f"{sorted(expected_object_new_bound_payloads)}; missing={missing}, "
            f"extra={extra}"
        )

    method_ic_selectors = data.get("method_ic_selector", [])
    if not isinstance(method_ic_selectors, list):
        raise WasmAbiManifestError("method_ic_selector must be a list of tables")
    expected_method_ic_selectors = {
        (family, extra_arg_count)
        for family in METHOD_IC_SELECTOR_FAMILIES
        for extra_arg_count in range(METHOD_IC_MAX_EXTRA_ARGS + 1)
    }
    seen_method_ic_selectors: set[tuple[str, int]] = set()
    for idx, entry in enumerate(method_ic_selectors):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"method_ic_selector entry {idx} must be a table"
            )
        family = entry.get("family")
        if not isinstance(family, str) or family not in METHOD_IC_SELECTOR_FAMILIES:
            raise WasmAbiManifestError(
                f"method_ic_selector entry {idx} has invalid family {family!r}"
            )
        extra_arg_count = entry.get("extra_arg_count")
        if (
            not isinstance(extra_arg_count, int)
            or extra_arg_count < 0
            or extra_arg_count > METHOD_IC_MAX_EXTRA_ARGS
        ):
            raise WasmAbiManifestError(
                f"method_ic_selector {family!r} has invalid extra_arg_count "
                f"{extra_arg_count!r}"
            )
        selector_key = (family, extra_arg_count)
        if selector_key in seen_method_ic_selectors:
            raise WasmAbiManifestError(
                f"duplicate method_ic_selector {selector_key!r}"
            )
        seen_method_ic_selectors.add(selector_key)
        import_name = entry.get("import_name")
        if not isinstance(import_name, str) or not import_name:
            raise WasmAbiManifestError(
                f"method_ic_selector {selector_key!r} has invalid import_name"
            )
        if import_name not in seen_imports:
            raise WasmAbiManifestError(
                f"method_ic_selector {selector_key!r} references unknown import "
                f"{import_name!r}"
            )
    if seen_method_ic_selectors != expected_method_ic_selectors:
        missing = sorted(expected_method_ic_selectors - seen_method_ic_selectors)
        extra = sorted(seen_method_ic_selectors - expected_method_ic_selectors)
        raise WasmAbiManifestError(
            "method_ic_selector must declare exactly the method/super arity grid; "
            f"missing={missing}, extra={extra}"
        )

    numeric_runtime_selectors = _validate_numeric_runtime_selectors(
        data,
        seen_imports,
        lir_import_by_variant,
    )
    data["numeric_runtime_selector"] = numeric_runtime_selectors

    const_op_policies = data.get("const_op_policy", [])
    if not isinstance(const_op_policies, list):
        raise WasmAbiManifestError("const_op_policy must be a list of tables")
    seen_const_policy_kinds: set[str] = set()
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
        lir_fast = entry.get("lir_fast")
        if not isinstance(lir_fast, str) or not lir_fast:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} must define lir_fast"
            )
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
        if lir_fast == "materialize" and materializer_import is None:
            raise WasmAbiManifestError(
                f"const_op_policy {kind!r} cannot materialize in LIR-fast without materializer_import"
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

    gpu_intrinsic_manifest_names = data.get("gpu_intrinsic_manifest_name", [])
    if not isinstance(gpu_intrinsic_manifest_names, list) or not gpu_intrinsic_manifest_names:
        raise WasmAbiManifestError(
            "manifest must define gpu_intrinsic_manifest_name entries"
        )
    seen_gpu_intrinsic_manifest_names: set[str] = set()
    host_export_set = set(host_exports)
    for idx, entry in enumerate(gpu_intrinsic_manifest_names):
        if not isinstance(entry, dict):
            raise WasmAbiManifestError(
                f"gpu_intrinsic_manifest_name entry {idx} must be a table"
            )
        name = entry.get("name")
        if not isinstance(name, str) or not name.startswith("molt_gpu_"):
            raise WasmAbiManifestError(
                f"gpu_intrinsic_manifest_name entry {idx} has invalid name"
            )
        if name in seen_gpu_intrinsic_manifest_names:
            raise WasmAbiManifestError(
                f"duplicate GPU intrinsic manifest name {name!r}"
            )
        if name not in host_export_set:
            raise WasmAbiManifestError(
                f"GPU intrinsic manifest name {name!r} must also be a runtime host export"
            )
        seen_gpu_intrinsic_manifest_names.add(name)

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
