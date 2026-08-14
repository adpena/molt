"""Render the generated Python view of the canonical WASM ABI manifest."""

from __future__ import annotations

from collections.abc import Callable

from wasm_abi_gen.manifest import (
    CPYTHON_ABI_LINK_IMPORT_CLASS,
    WasmAbiManifestError,
    _call_indirect_imports,
    generator_cpython_abi_link_import_kinds,
    generator_cpython_abi_link_import_signatures,
)


def render_py(
    data: dict,
    *,
    _header: Callable[[str], str],
    _py_string: Callable[[str], str],
    _py_tuple: Callable[[list[str]], str],
    _runtime_export_name: Callable[[dict], str],
    _shared_runtime_callables: Callable[[dict], list[dict]],
    _split_runtime_external_export_name: Callable[[str], str],
) -> str:
    lines: list[str] = [_header("#")]
    reserved_callables = _shared_runtime_callables(data)
    non_runtime_callables = sorted(data.get("non_runtime_callable_intrinsic", []))
    lines.append(
        "WASM_STATIC_TYPES: tuple[tuple[tuple[str, ...], tuple[str, ...]], ...] = (\n"
    )
    for entry in data["static_type"]:
        lines.append(
            f"    ({_py_tuple(entry['params'])}, {_py_tuple(entry['results'])}),\n"
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLE_TRAMPOLINE_ABI_BY_RUNTIME: dict[str, str] = {\n"
    )
    for entry in reserved_callables:
        lines.append(
            f'    "{entry["runtime_name"]}": "{entry.get("trampoline_abi", "unpack_args")}",\n'
        )
    lines.append("}\n\n")
    lines.append(f"WASM_STATIC_TYPE_COUNT: int = {len(data['static_type'])}\n\n")
    lines.append("WASM_NON_RUNTIME_CALLABLE_INTRINSICS: frozenset[str] = frozenset({\n")
    for name in non_runtime_callables:
        lines.append(f'    "{name}",\n')
    lines.append("})\n\n")
    lines.append("WASM_IMPORT_REGISTRY: tuple[str, ...] = (\n")
    for entry in data["import"]:
        lines.append(f'    "{entry["name"]}",\n')
    lines.append(")\n\n")
    lines.append("WASM_BULK_MEMORY_OPS: tuple[tuple[str, str, int], ...] = (\n")
    for entry in data.get("wasm_bulk_memory_op", []):
        lines.append(
            f'    ("{entry["kind"]}", "{entry["instruction"]}", '
            f"{entry['arg_count']}),\n"
        )
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
        f"WASM_DEFAULT_APP_TABLE_BASE: int = {data['table_layout']['default_app_table_base']}\n\n"
    )
    lines.append(
        "WASM_CALLABLE_TABLE_SECTION_NAME: str = "
        f"{_py_string(data['callable_table_publication']['section_name'])}\n"
    )
    callable_table = data["callable_table_publication"]
    lines.append(
        f"WASM_CALLABLE_TABLE_SECTION_VERSION: int = {callable_table['version']}\n"
    )
    lines.append(
        "WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME: str = "
        f"{_py_string(callable_table['layout_section_name'])}\n"
    )
    lines.append(
        f"WASM_CALLABLE_TABLE_LAYOUT_VERSION: int = {callable_table['layout_version']}\n"
    )
    lines.append(
        f"WASM_CALLABLE_TABLE_ACTIVE_ELEMENT_ROLE: int = {callable_table['active_element_role']}\n"
    )
    lines.append(
        f"WASM_CALLABLE_TABLE_VALUE_TYPE_FORMAT: int = {callable_table['value_type_format']}\n\n"
    )
    lines.append(
        "WASM_RUNTIME_CALLABLE_IMPORTS: tuple[tuple[str, str, int, str], ...] = (\n"
    )
    for entry in data["import"]:
        if "callable_arity" not in entry:
            continue
        result = entry.get("callable_result", "i64")
        lines.append(
            f'    ("{entry["runtime_name"]}", "{entry["name"]}", '
            f'{entry["callable_arity"]}, "{result}"),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLES: "
        "tuple[tuple[int, str, str, int, str], ...] = (\n"
    )
    for entry in reserved_callables:
        lines.append(
            f'    ({entry["index"]}, "{entry["runtime_name"]}", '
            f'"{entry["import_name"]}", {entry["callable_arity"]}, '
            f'"{entry.get("callable_dispatch", "direct")}")'
            ",\n"
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RESERVED_RUNTIME_CALLABLE_COUNT: int = "
        "len(WASM_RESERVED_RUNTIME_CALLABLES)\n\n"
    )
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
            "WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_RUNTIME: dict[str, tuple[str, int, str]] = {\n",
            '    runtime_name: (import_name, arity, "i64")\n',
            "    for _index, runtime_name, import_name, arity, _dispatch in WASM_RESERVED_RUNTIME_CALLABLES\n",
            "}\n\n",
            "WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_IMPORT: dict[str, tuple[str, int, str]] = {\n",
            '    import_name: (runtime_name, arity, "i64")\n',
            "    for _index, runtime_name, import_name, arity, _dispatch in WASM_RESERVED_RUNTIME_CALLABLES\n",
            "}\n\n",
            "WASM_RUNTIME_CALLABLE_ARITY_BY_RUNTIME: dict[str, int] = {\n",
            "    **{\n",
            "        runtime_name: arity\n",
            "        for runtime_name, _import_name, arity, _result in WASM_RUNTIME_CALLABLE_IMPORTS\n",
            "    },\n",
            "    **{\n",
            "        runtime_name: spec[1]\n",
            "        for runtime_name, spec in WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_RUNTIME.items()\n",
            "    },\n",
            "}\n\n",
            "def wasm_runtime_callable_spec(name: str) -> tuple[str, int, str] | None:\n",
            "    return WASM_RUNTIME_CALLABLE_IMPORT_BY_RUNTIME.get(\n",
            "        name\n",
            "    ) or WASM_RUNTIME_CALLABLE_IMPORT_BY_IMPORT.get(\n",
            "        name\n",
            "    ) or WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_RUNTIME.get(\n",
            "        name\n",
            "    ) or WASM_RESERVED_RUNTIME_CALLABLE_SPEC_BY_IMPORT.get(name)\n\n",
            "def wasm_runtime_callable_arity(name: str) -> int | None:\n",
            "    spec = wasm_runtime_callable_spec(name)\n",
            "    return None if spec is None else spec[1]\n\n",
            "def wasm_runtime_callable_result(name: str) -> str | None:\n",
            "    spec = wasm_runtime_callable_spec(name)\n",
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
            f"{_py_tuple(signature['results'])}),\n"
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_RUNTIME_HOST_EXPORT_SIGNATURES: tuple[tuple[str, tuple[str, ...], tuple[str, ...]], ...] = (\n"
    )
    for entry in data["runtime_host_export_signature"]:
        lines.append(
            f'    ("{entry["name"]}", {_py_tuple(entry["params"])}, '
            f"{_py_tuple(entry['results'])}),\n"
        )
    lines.append(")\n\n")
    lines.extend(
        [
            "WASM_IMPORT_SIGNATURE_BY_NAME: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {\n",
            "    name: (params, results)\n",
            "    for name, params, results in WASM_IMPORT_SIGNATURES\n",
            "}\n\n",
            "WASM_RUNTIME_HOST_EXPORT_SIGNATURE_BY_NAME: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {\n",
            "    name: (params, results)\n",
            "    for name, params, results in WASM_RUNTIME_HOST_EXPORT_SIGNATURES\n",
            "}\n\n",
            "WASM_IMPORT_NAME_BY_LOOKUP: dict[str, str] = {\n",
            "    **{name: name for name, _params, _results in WASM_IMPORT_SIGNATURES},\n",
            "    **{\n",
        ]
    )
    for entry in data["import"]:
        runtime_name = entry.get("runtime_name")
        if runtime_name is None:
            continue
        lines.append(f'        "{runtime_name}": "{entry["name"]}",\n')
    lines.extend(
        [
            "    },\n",
            "}\n\n",
            "def wasm_import_name(name: str) -> str | None:\n",
            "    return WASM_IMPORT_NAME_BY_LOOKUP.get(name)\n\n",
            "WASM_RUNTIME_IMPORT_EXPORT_NAMES: tuple[tuple[str, str], ...] = (\n",
        ]
    )
    for entry in data["import"]:
        lines.append(f'    ("{entry["name"]}", "{_runtime_export_name(entry)}"),\n')
    lines.extend(
        [
            ")\n\n",
            "WASM_RUNTIME_EXPORT_BY_IMPORT: dict[str, str] = {\n",
            "    import_name: export_name\n",
            "    for import_name, export_name in WASM_RUNTIME_IMPORT_EXPORT_NAMES\n",
            "}\n\n",
            "WASM_RUNTIME_IMPORT_BY_EXPORT: dict[str, str] = {\n",
            "    export_name: import_name\n",
            "    for import_name, export_name in WASM_RUNTIME_IMPORT_EXPORT_NAMES\n",
            "}\n\n",
            "def wasm_runtime_import_name(name: str) -> str | None:\n",
            "    import_name = wasm_import_name(name)\n",
            "    if import_name is not None:\n",
            "        return import_name\n",
            "    import_name = WASM_RUNTIME_IMPORT_BY_EXPORT.get(name)\n",
            "    if import_name is not None:\n",
            "        return import_name\n",
            "    if name in WASM_RUNTIME_HOST_EXPORTS:\n",
            "        return name\n",
            "    return None\n\n",
            "def wasm_runtime_export_name(name: str) -> str | None:\n",
            "    import_name = wasm_runtime_import_name(name)\n",
            "    if import_name is None:\n",
            "        return None\n",
            "    export_name = WASM_RUNTIME_EXPORT_BY_IMPORT.get(import_name)\n",
            "    if export_name is not None:\n",
            "        return export_name\n",
            "    if import_name in WASM_RUNTIME_HOST_EXPORTS:\n",
            "        return import_name\n",
            "    return None\n\n",
            "def wasm_import_signature(name: str) -> tuple[tuple[str, ...], tuple[str, ...]] | None:\n",
            "    import_name = wasm_import_name(name)\n",
            "    if import_name is not None:\n",
            "        return WASM_IMPORT_SIGNATURE_BY_NAME.get(import_name)\n",
            "    return WASM_RUNTIME_HOST_EXPORT_SIGNATURE_BY_NAME.get(name)\n\n",
            "def wasm_import_result_kind(name: str) -> str | None:\n",
            "    signature = wasm_import_signature(name)\n",
            "    if signature is None:\n",
            "        return None\n",
            "    results = signature[1]\n",
            '    return "nil" if not results else ", ".join(results)\n\n',
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
            f"{entry['dispatch_runtime_seed']}, "
            f'{entry["parse_scalar_literal"]}, "{entry["raw_int_effect"]}", '
            f'"{entry["lir_fast"]}"),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_CONTAINER_RUNTIME_SELECTORS: tuple[tuple[str, str, str, str | None], ...] = (\n"
    )
    for entry in data.get("container_runtime_selector", []):
        lir_variant = entry.get("lir_variant")
        lir_variant_repr = "None" if lir_variant is None else f'"{lir_variant}"'
        lines.append(
            f'    ("{entry["op"]}", "{entry["fact"]}", '
            f'"{entry["import_name"]}", {lir_variant_repr}),\n'
        )
    lines.append(")\n\n")
    lines.append("WASM_METHOD_IC_SELECTORS: tuple[tuple[str, int, str], ...] = (\n")
    for entry in data.get("method_ic_selector", []):
        lines.append(
            f'    ("{entry["family"]}", {entry["extra_arg_count"]}, '
            f'"{entry["import_name"]}"),\n'
        )
    lines.append(")\n\n")
    lines.append(
        "WASM_NUMERIC_RUNTIME_SELECTORS: tuple[tuple[str, str, str, str | None, int | None, tuple[str, ...]], ...] = (\n"
    )
    for entry in data.get("numeric_runtime_selector", []):
        lir_variant = entry.get("lir_variant")
        lir_variant_repr = "None" if lir_variant is None else f'"{lir_variant}"'
        lir_operand_count = entry.get("lir_operand_count")
        lir_operand_count_repr = (
            "None" if lir_operand_count is None else str(lir_operand_count)
        )
        lines.append(
            f'    ("{entry["kind"]}", "{entry["import_name"]}", '
            f'"{entry["op_loop_variant"]}", {lir_variant_repr}, '
            f"{lir_operand_count_repr}, {_py_tuple(entry['deps'])}),\n"
        )
    lines.append(")\n\n")
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
        lines.append(f'    ("{entry["import"]}", {_py_tuple(entry["exports"])}),\n')
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
    lines.append("WASM_LINK_ALLOWED_IMPORT_PRIMITIVE_CLASSES: dict[str, str] = {\n")
    for entry in data.get("link_allowed_import", []):
        lines.append(f'    "{entry["name"]}": "{entry["primitive_class"]}",\n')
    lines.append("}\n\n")
    lines.append("WASM_EXTERNAL_NATIVE_LINK_IMPORTS: tuple[str, ...] = (\n")
    external_native_link_imports = {
        entry["name"]: entry["primitive_class"]
        for entry in data.get("link_allowed_import", [])
    }
    external_native_link_import_symbol_kinds = {
        name: kind for name, kind in generator_cpython_abi_link_import_kinds()
    }
    for entry in data.get("external_native_link_import", []):
        external_native_link_imports[entry["name"]] = entry["primitive_class"]
        symbol_kind = entry.get("symbol_kind")
        if isinstance(symbol_kind, str):
            external_native_link_import_symbol_kinds[entry["name"]] = symbol_kind
    for name in external_native_link_imports:
        lines.append(f'    "{name}",\n')
    lines.append(")\n\n")
    lines.append(
        "WASM_EXTERNAL_NATIVE_LINK_IMPORT_PRIMITIVE_CLASSES: dict[str, str] = {\n"
    )
    for name, primitive_class in external_native_link_imports.items():
        lines.append(f'    "{name}": "{primitive_class}",\n')
    lines.append("}\n\n")
    lines.append(
        "WASM_EXTERNAL_NATIVE_LINK_IMPORT_SPLIT_EXPORT_NAMES: dict[str, str] = {\n"
    )
    cpython_abi_split_import_by_export: dict[str, str] = {}
    for name, primitive_class in sorted(external_native_link_imports.items()):
        if primitive_class != CPYTHON_ABI_LINK_IMPORT_CLASS:
            continue
        export_name = _split_runtime_external_export_name(name)
        existing_name = cpython_abi_split_import_by_export.get(export_name)
        if existing_name is not None and existing_name != name:
            raise WasmAbiManifestError(
                "duplicate split-runtime CPython ABI export name "
                f"{export_name!r} for {existing_name!r} and {name!r}"
            )
        cpython_abi_split_import_by_export[export_name] = name
        lines.append(f'    "{name}": "{export_name}",\n')
    lines.append("}\n\n")
    lines.append(
        "WASM_EXTERNAL_NATIVE_LINK_IMPORT_BY_SPLIT_EXPORT_NAME: dict[str, str] = {\n"
    )
    for export_name, name in sorted(cpython_abi_split_import_by_export.items()):
        lines.append(f'    "{export_name}": "{name}",\n')
    lines.append("}\n\n")
    lines.append("WASM_EXTERNAL_NATIVE_LINK_IMPORT_SYMBOL_KINDS: dict[str, str] = {\n")
    for name, symbol_kind in sorted(external_native_link_import_symbol_kinds.items()):
        lines.append(f'    "{name}": "{symbol_kind}",\n')
    lines.append("}\n\n")
    lines.append(
        "WASM_EXTERNAL_NATIVE_LINK_IMPORT_FUNCTION_SIGNATURES: "
        "dict[str, dict[str, object]] = {\n"
    )
    for name, params, results in generator_cpython_abi_link_import_signatures():
        result = "nil" if not results else ", ".join(results)
        lines.append(
            f'    "{name}": {{"params": {list(params)!r}, "result": {result!r}}},\n'
        )
    lines.append("}\n\n")
    lines.append("WASM_STRIP_IMPORT_RULES: tuple[tuple[str, str, str, str], ...] = (\n")
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
