import ast
import hashlib
import importlib.util
import json
import os
import subprocess
import threading
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest
from molt.cli.source_extension_link_requirements import (
    merge_source_extension_link_requirements,
    source_extension_link_requirements,
)
from molt.cli import link_fingerprints, native_symbol_inspection
from molt.cli.python_source_closure import LocalPythonSourceClosure
from molt.cli.wasm_link_args import wasm_link_output_arguments
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkCyclicGroup,
    SourceExtensionLinkInput,
    SourceExtensionLinkLoadingPolicy,
    SourceExtensionLinkRequirements,
    source_extension_link_file,
)
from tests.cli.native_link_test_support import static_archive_bytes
from tests.executable_test_support import write_mock_executable
from molt import wasm_artifact
from molt._wasm_runtime_exports import (
    wasm_split_runtime_export_name_for_import,
    wasm_split_runtime_import_signature,
)
from molt.cli.app_export_contract import app_export_call_abi, build_app_export_contract
from molt.cli import wasm_link_cache
from molt.frontend import SimpleTIRGenerator
from molt.wasm_artifact import parse_wasm_exports, parse_wasm_imports
from molt.wasm_linking_symbols import (
    FLAG_BINDING_GLOBAL,
    FLAG_BINDING_LOCAL,
    FLAG_EXPORTED,
    FLAG_NO_STRIP,
    FLAG_BINDING_WEAK,
    SYMBOL_KIND_DATA,
    SYMBOL_KIND_FUNCTION,
    SYMBOL_BINDING_MASK,
    SYMTAB_SUBSECTION_ID,
    parse_wasm_linking_symbols,
)
from molt.toolchain_identity import stable_regular_file_identity
from molt.temporary_artifacts import OwnedTemporaryDirectory


def _load_wasm_link():
    root = Path(__file__).resolve().parents[1]
    path = root / "tools" / "wasm_link.py"
    spec = importlib.util.spec_from_file_location("molt_wasm_link", path)
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


wasm_link = _load_wasm_link()
wasm_link_callable_table = importlib.import_module("wasm_link_callable_table")
wasm_archive = importlib.import_module("wasm_archive")
wasm_link_command = importlib.import_module("wasm_link_command")
wasm_link_edit = importlib.import_module("wasm_link_edit")
wasm_link_export_contract = importlib.import_module("wasm_link_export_contract")
wasm_link_fact_provider = importlib.import_module("wasm_link_fact_provider")
wasm_link_format = importlib.import_module("wasm_link_format")
wasm_link_native_inputs = importlib.import_module("wasm_link_native_inputs")
wasm_link_operations = importlib.import_module("wasm_link_operations")
wasm_link_optimize = importlib.import_module("wasm_link_optimize")
wasm_link_optimizer_policy = importlib.import_module("wasm_link_optimizer_policy")
wasm_link_pipeline = importlib.import_module("wasm_link_pipeline")
wasm_link_runtime_data = importlib.import_module("wasm_link_runtime_data")
wasm_link_transaction = importlib.import_module("wasm_link_transaction")
wasm_link_validation = importlib.import_module("wasm_link_validation")

_REAL_MAKE_RUST_WASM_FACTS_PROVIDER = (
    wasm_link_fact_provider.make_rust_wasm_facts_provider
)


@pytest.mark.parametrize(
    "stdout, message",
    [
        (
            '{"schema_version":7,"schema_version":8,"ok":true,'
            '"facts":{"schema_version":8}}',
            "duplicate JSON key",
        ),
        (
            '{"schema_version":8,"ok":true,"facts":{"schema_version":8}}',
            "unsupported response schema",
        ),
        (
            '{"schema_version":7,"ok":true,"facts":{"schema_version":7},'
            '"extension":true}',
            "invalid success response",
        ),
    ],
)
def test_wasm_facts_decoder_rejects_schema_ambiguity(
    stdout: str,
    message: str,
) -> None:
    process = subprocess.CompletedProcess(["scanner"], 0, stdout, "")

    with pytest.raises(ValueError, match=message):
        wasm_link_fact_provider._decode_wasm_facts_response(
            process,
            operation="test scan",
        )


def test_wasm_facts_decoder_rejects_facts_extensions() -> None:
    facts = _rust_facts_fixture(b"\0asm\x01\0\0\0")
    facts["extension"] = True
    process = subprocess.CompletedProcess(
        ["scanner"],
        0,
        json.dumps({"schema_version": 7, "ok": True, "facts": facts}),
        "",
    )

    with pytest.raises(ValueError, match="invalid schema-v7 field set"):
        wasm_link_fact_provider._decode_wasm_facts_response(
            process,
            operation="test scan",
        )


def test_wasm_facts_decoder_rejects_nested_projection_extensions() -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    imports[0]["extension"] = True
    process = subprocess.CompletedProcess(
        ["scanner"],
        0,
        json.dumps({"schema_version": 7, "ok": True, "facts": facts}),
        "",
    )

    with pytest.raises(ValueError, match="invalid field set"):
        wasm_link_fact_provider._decode_wasm_facts_response(
            process,
            operation="test scan",
        )


@pytest.mark.parametrize("duplicate_kind", (1, 2))
def test_wasm_link_facts_rejects_duplicate_import_identity(
    duplicate_kind: int,
) -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    duplicate = dict(imports[0])
    duplicate["index"] = 1
    if duplicate_kind == 1:
        duplicate["kind"] = 1
        duplicate["type"] = {
            "kind": "table",
            "table64": False,
            "shared": False,
            "minimum": 1,
            "maximum": None,
            "element_type": [0x70],
        }
    imports.append(duplicate)
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    with pytest.raises(
        ValueError,
        match=r"duplicate identity \('env', 'memory'\)",
    ):
        wasm_link_fact_provider.WasmLinkFacts(frozen)


def test_wasm_link_facts_scopes_import_identity_by_module() -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    distinct = dict(imports[0])
    distinct["module"] = "other"
    distinct["index"] = 1
    imports.append(distinct)
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    decoded = wasm_link_fact_provider.WasmLinkFacts(frozen)

    assert [(fact.module, fact.name) for fact in decoded.imports] == [
        ("env", "memory"),
        ("other", "memory"),
    ]


@pytest.mark.parametrize("field", ("module", "name"))
def test_wasm_link_facts_rejects_empty_import_identity(field: str) -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    imports[0][field] = ""
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    with pytest.raises(ValueError, match=rf"\.{field} must be a non-empty string"):
        wasm_link_fact_provider.WasmLinkFacts(frozen)


def test_wasm_link_facts_rejects_empty_export_name() -> None:
    facts = _rust_facts_fixture(_build_exported_runtime_module("exported"))
    exports = facts["canonical_export_types"]
    assert isinstance(exports, list)
    exports[0]["name"] = ""
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    with pytest.raises(ValueError, match=r"\.name must be a non-empty string"):
        wasm_link_fact_provider.WasmLinkFacts(frozen)


def test_wasm_link_facts_rejects_duplicate_per_kind_import_index() -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    duplicate_index = dict(imports[0])
    duplicate_index["module"] = "other"
    duplicate_index["name"] = "other_memory"
    imports.append(duplicate_index)
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    with pytest.raises(ValueError, match="duplicate kind 2 index 0"):
        wasm_link_fact_provider.WasmLinkFacts(frozen)


@pytest.mark.parametrize("index", (1, 3))
def test_wasm_link_facts_rejects_nonzero_or_gapped_import_index(index: int) -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    imports[0]["index"] = index
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    with pytest.raises(ValueError, match="indices must be contiguous from zero"):
        wasm_link_fact_provider.WasmLinkFacts(frozen)


def test_wasm_link_facts_indexes_each_external_kind_independently() -> None:
    facts = _rust_facts_fixture(_build_memory_import_ref_func_app_module(func_index=0))
    imports = facts["canonical_import_types"]
    assert isinstance(imports, list)
    imports.append(
        {
            "module": "env",
            "name": "table",
            "kind": 1,
            "index": 0,
            "type": {
                "kind": "table",
                "table64": False,
                "shared": False,
                "minimum": 1,
                "maximum": None,
                "element_type": [0x70],
            },
        }
    )
    frozen = wasm_link_fact_provider._freeze_json(facts)
    assert isinstance(frozen, dict)

    decoded = wasm_link_fact_provider.WasmLinkFacts(frozen)

    assert [(fact.kind, fact.index) for fact in decoded.imports] == [(2, 0), (1, 0)]


def test_split_link_pipeline_projects_typed_source_extension_requirements() -> None:
    source = Path(wasm_link_pipeline.__file__).read_text(encoding="utf-8")
    tree = ast.parse(source)
    direct_imports = {
        (node.module, imported.name)
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom)
        for imported in node.names
    }
    direct_calls = {
        node.func.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Name)
    }

    assert (
        "molt.cli.source_extension_link_requirements",
        "render_source_extension_link_arguments",
    ) in direct_imports
    assert ("wasm_stub_wasi", "stub_wasi_imports") in direct_imports
    assert "render_source_extension_link_arguments" in direct_calls
    assert "stub_wasi_imports" in direct_calls
    assert not any(
        isinstance(node, ast.Subscript)
        and isinstance(node.slice, ast.Constant)
        and node.slice.value == "render_source_extension_link_arguments"
        for node in ast.walk(tree)
    )
    assert not any(
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == "spec_from_file_location"
        for node in ast.walk(tree)
    )


def test_wasm_link_cache_bench_imports_owner_authorities_directly() -> None:
    bench_path = (
        Path(__file__).resolve().parents[1] / "tools" / "bench_wasm_link_cache.py"
    )
    tree = ast.parse(bench_path.read_text(encoding="utf-8"))
    direct_modules = {
        node.module
        for node in ast.walk(tree)
        if isinstance(node, ast.ImportFrom) and node.module is not None
    }
    imported_names = {
        alias.name
        for node in ast.walk(tree)
        if isinstance(node, (ast.Import, ast.ImportFrom))
        for alias in node.names
    }

    assert {
        "wasm_link_runtime_data",
        "wasm_link_fact_provider",
        "wasm_link_optimizer_policy",
        "wasm_metrics",
    }.issubset(direct_modules)
    assert "wasm_link" not in imported_names
    assert not any(
        isinstance(node, ast.Call)
        and isinstance(node.func, ast.Attribute)
        and node.func.attr == "spec_from_file_location"
        for node in ast.walk(tree)
    )


def test_callable_layout_rejects_unbounded_unattested_entry_counts() -> None:
    facts: dict[str, object] = {
        "callable_table_layout": {
            "fixed_prefix_base": 0,
            "fixed_prefix_len": 0,
            "finalized_app_base": 0,
            "app_entry_count": 0xFFFF_FFFF,
        },
        "callable_table_entries": [],
        "callable_table_attestation_present": False,
        "tables": [{"table_index": 0, "minimum": 1}],
    }

    with pytest.raises(ValueError, match="artifact-derived table capacity"):
        wasm_link_callable_table._callable_layout_from_wasm_facts(
            facts,
            artifact_role="plan",
        )


def test_callable_layout_accepts_unattested_plan_within_artifact_table() -> None:
    facts: dict[str, object] = {
        "callable_table_layout": {
            "fixed_prefix_base": 0,
            "fixed_prefix_len": 3,
            "finalized_app_base": 8,
            "app_entry_count": 2,
        },
        "callable_table_entries": [],
        "callable_table_attestation_present": False,
        "tables": [{"table_index": 0, "minimum": 10}],
    }

    assert wasm_link_callable_table._callable_layout_from_wasm_facts(
        facts,
        artifact_role="plan",
    ) == wasm_link_format.CallableTableLayout(0, 3, 8, 2)


def _wasm_optimizer_identity(
    path: str | Path,
    *,
    sha256: str = "a" * 64,
    version: str = "wasm-opt version 130 (version_130)",
):  # type: ignore[no-untyped-def]
    from molt.toolchain_identity import StableRegularFileIdentity

    return wasm_link_optimizer_policy.WasmOptimizerExecutableIdentity(
        executable=StableRegularFileIdentity(
            path=Path(path),
            size=0,
            sha256=sha256,
            _stat_identity=(0, 0, 0, 0, 0, 0),
            _content_change_time_ns=0,
        ),
        binaryen_version=version,
    )


def _write_app_export_contract(
    path: Path,
    *,
    entry_module: str,
    source: str,
    symbols: list[tuple[str, str]],
    known_func_kinds: dict[str, dict[str, str]] | None = None,
) -> Path:
    tree = ast.parse(source)
    kinds = known_func_kinds or {
        entry_module: {
            statement.name: "sync"
            for statement in tree.body
            if isinstance(statement, (ast.FunctionDef, ast.AsyncFunctionDef))
        }
    }
    generator = SimpleTIRGenerator(
        module_name=entry_module,
        entry_module=entry_module,
        known_modules=set(kinds) | {entry_module},
        known_func_kinds=kinds,
    )
    generator.visit(tree)
    payload = build_app_export_contract(
        entry_module=entry_module,
        ir=generator.to_json(),
        registry_digest="b" * 64,
    )
    actual = {binding["name"]: binding["symbol"] for binding in payload["bindings"]}
    assert actual == dict(symbols)
    path.write_text(json.dumps(payload), encoding="utf-8")
    return path


def test_external_wasm_ld_uses_response_file_beyond_windows_command_limit(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    if wasm_link_command.os.name != "nt":
        pytest.skip("Windows command-length response-file authority")
    observed: dict[str, object] = {}

    def guarded(_executor, command, **_kwargs):  # type: ignore[no-untyped-def]
        observed["command"] = command
        observed["response"] = Path(command[1][1:]).read_text(encoding="utf-8")
        return wasm_link_command.subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(
        wasm_link_command.CommandExecutor,
        "run",
        guarded,
    )
    arguments = ["--export-if-defined=" + "x" * 100 for _ in range(400)]

    result = wasm_link_command._run_external_tool(["wasm-ld.exe", *arguments])

    assert result.returncode == 0
    assert observed["command"][0] == "wasm-ld.exe"
    assert observed["command"][1].startswith("@")
    assert "\n".join(arguments) in observed["response"]


def _fixture_export_kinds(data: bytes) -> dict[str, tuple[int, int]]:
    result: dict[str, tuple[int, int]] = {}
    for section_id, payload in wasm_link_operations.parse_sections(data):
        if section_id != 7:
            continue
        _names, _functions, export_kinds = wasm_link_format._parse_export_payload(
            payload
        )
        result.update(export_kinds)
    return result


def _fixture_custom_names(data: bytes) -> list[str]:
    names: list[str] = []
    for section_id, payload in wasm_link_operations.parse_sections(data):
        if section_id != 0:
            continue
        name, _ = wasm_link_format._parse_custom_section(payload)
        names.append(name)
    return names


def _fixture_extern_type(
    kind: int,
    desc: bytes,
    function_types: list[tuple[tuple[bytes, ...], tuple[bytes, ...]]],
) -> dict[str, object]:
    if kind == 0:
        type_index, _ = wasm_link_format._read_varuint(desc, 0)
        params, results = function_types[type_index]
        return {
            "kind": "function",
            "exact": False,
            "params": [list(value) for value in params],
            "results": [list(value) for value in results],
        }
    if kind == 1:
        flags, minimum, maximum, _ = wasm_artifact.read_wasm_limits(desc, 1)
        return {
            "kind": "table",
            "table64": bool(flags & 0x04),
            "shared": bool(flags & 0x02),
            "minimum": minimum,
            "maximum": maximum,
            "element_type": [desc[0]],
        }
    if kind == 2:
        flags, minimum, maximum, _ = wasm_artifact.read_wasm_limits(desc, 0)
        return {
            "kind": "memory",
            "memory64": bool(flags & 0x04),
            "shared": bool(flags & 0x02),
            "minimum": minimum,
            "maximum": maximum,
            "page_size_log2": None,
        }
    if kind == 3:
        return {
            "kind": "global",
            "value_type": [desc[0]],
            "mutable": bool(desc[1] & 0x01),
            "shared": bool(desc[1] & 0x02),
        }
    if kind == 4:
        type_index, _ = wasm_link_format._read_varuint(desc, 1)
        params, results = function_types[type_index]
        return {
            "kind": "tag",
            "tag_kind": "exception",
            "params": [list(value) for value in params],
            "results": [list(value) for value in results],
        }
    raise AssertionError(f"unsupported fixture external kind {kind}")


def _fixture_export_extern_type(
    kind: int,
    index: int,
    *,
    indexed_function_types: list[int],
    function_types: list[tuple[tuple[bytes, ...], tuple[bytes, ...]]],
    table_min: int | None,
) -> dict[str, object]:
    if kind == 0:
        params, results = function_types[indexed_function_types[index]]
        return {
            "kind": "function",
            "exact": False,
            "params": [list(value) for value in params],
            "results": [list(value) for value in results],
        }
    if kind == 1:
        return {
            "kind": "table",
            "table64": False,
            "shared": False,
            "minimum": table_min or 0,
            "maximum": None,
            "element_type": [0x70],
        }
    if kind == 2:
        return {
            "kind": "memory",
            "memory64": False,
            "shared": False,
            "minimum": 0,
            "maximum": None,
            "page_size_log2": None,
        }
    if kind == 3:
        return {
            "kind": "global",
            "value_type": [0x7F],
            "mutable": False,
            "shared": False,
        }
    if kind == 4:
        return {"kind": "tag", "tag_kind": "exception", "params": [], "results": []}
    raise AssertionError(f"unsupported fixture external kind {kind}")


def _fixture_linking_symbols(data: bytes) -> list[dict[str, object]]:
    symbols = iter(
        parse_wasm_linking_symbols(
            data, wasm_imports=wasm_link_format._collect_imports(data)
        ).symbols
    )
    result: list[dict[str, object]] = []
    for sid, payload in wasm_link_operations.parse_sections(data):
        if sid != 0:
            continue
        name, custom = wasm_link_format._parse_custom_section(payload)
        if name != "linking":
            continue
        _version, subsections = wasm_link_format._parse_linking_payload(custom)
        for sub_id, table in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                continue
            count, offset = wasm_link_format._read_varuint(table, 0)
            for ordinal in range(count):
                kind = table[offset]
                flags, offset = wasm_link_format._read_varuint(table, offset + 1)
                if kind == 3:
                    _index, offset = wasm_link_format._read_varuint(table, offset)
                    continue
                if kind == 1:
                    _name, offset = wasm_link_format._read_string(table, offset)
                    if not flags & wasm_link_format.FLAG_UNDEFINED:
                        for _ in range(3):
                            _value, offset = wasm_link_format._read_varuint(
                                table, offset
                            )
                else:
                    _index, _name, offset = wasm_link_format._parse_indexed_symbol(
                        table, offset, flags
                    )
                symbol = next(symbols)
                result.append(
                    {
                        "symbol_index": ordinal,
                        **{
                            key: getattr(symbol, key)
                            for key in (
                                "name",
                                "kind",
                                "flags",
                                "index",
                                "segment_index",
                                "data_offset",
                                "size",
                            )
                        },
                    }
                )
    return result


def _rust_facts_fixture(data: bytes) -> dict[str, object]:
    sections = wasm_link_operations.parse_sections(data)
    import_count = wasm_link_format._count_func_imports(sections)
    function_types = wasm_link_format._parse_type_section(sections)
    imported_function_types: list[int] = []
    canonical_import_types: list[dict[str, object]] = []
    next_index_by_kind = {kind: 0 for kind in range(5)}
    for entry in wasm_link_format._collect_imports(data):
        module, name, kind, desc = (
            entry.module,
            entry.name,
            entry.kind,
            entry.description,
        )
        kind_index = next_index_by_kind[kind]
        next_index_by_kind[kind] += 1
        if kind == 0:
            type_index, _ = wasm_link_format._read_varuint(desc, 0)
            imported_function_types.append(type_index)
        canonical_import_types.append(
            {
                "module": module,
                "name": name,
                "kind": kind,
                "index": kind_index,
                "type": _fixture_extern_type(kind, desc, function_types),
            }
        )
    defined_count = 0
    defined_function_types: list[int] = []
    for section_id, payload in sections:
        if section_id == 3:
            defined_count, _ = wasm_link_format._read_varuint(payload, 0)
            _section_index, defined_function_types = (
                wasm_link_format._parse_func_type_indices(sections)
            )
            break
    export_kinds = _fixture_export_kinds(data)
    indexed_function_types = imported_function_types + defined_function_types
    canonical_export_types: list[dict[str, object]] = []
    table_min = next(
        (
            entry.minimum
            for entry in wasm_link_format._collect_imports(data)
            if entry.kind == 1
            and entry.module == "env"
            and entry.name == "__indirect_function_table"
        ),
        None,
    )
    for name, (kind, index) in export_kinds.items():
        canonical_export_types.append(
            {
                "name": name,
                "kind": kind,
                "index": index,
                "type": _fixture_export_extern_type(
                    kind,
                    index,
                    indexed_function_types=indexed_function_types,
                    function_types=function_types,
                    table_min=table_min,
                ),
            }
        )
    total_functions = import_count + defined_count
    exported_tables = sorted(
        index for kind, index in export_kinds.values() if kind == 1
    )
    app_base = table_min or 0
    table_facts = (
        []
        if table_min is None
        else [
            {
                "table_index": 0,
                "imported": True,
                "minimum": table_min,
                "maximum": None,
                "table64": False,
                "shared": False,
                "untyped_funcref": True,
                "encoded_element_type": [0x70],
            }
        ]
    )
    for section_id, payload in sections:
        if section_id != 4:
            continue
        count, offset = wasm_link_format._read_varuint(payload, 0)
        for _ in range(count):
            element_type = payload[offset]
            flags, minimum, maximum, offset = wasm_artifact.read_wasm_limits(
                payload, offset + 1
            )
            table_facts.append(
                {
                    "table_index": len(table_facts),
                    "imported": False,
                    "minimum": minimum,
                    "maximum": maximum,
                    "table64": bool(flags & 4),
                    "shared": bool(flags & 2),
                    "untyped_funcref": element_type == 0x70,
                    "encoded_element_type": [element_type],
                }
            )
    return {
        "schema_version": 7,
        "function_import_count": import_count,
        "defined_function_count": defined_count,
        "code_body_count": defined_count,
        "operator_count": 0,
        "reachable_function_indices": list(range(total_functions)),
        "referenced_function_indices": list(range(total_functions)),
        "main_module_init_direct_calls": [],
        "function_types": [
            {
                "type_index": index,
                "params": [list(value) for value in params],
                "results": [list(value) for value in results],
            }
            for index, (params, results) in enumerate(function_types)
        ],
        "function_type_indices": indexed_function_types,
        "active_element_segments": [],
        "active_function_elements": [],
        "callable_table_entries": [],
        "callable_table_attestation_present": True,
        "callable_table_layout": {
            "fixed_prefix_base": 0,
            "fixed_prefix_len": 0,
            "finalized_app_base": app_base,
            "app_entry_count": 0,
        },
        "table_mutations": [],
        "reachable_table_mutations": [],
        "forbidden_callable_alias_exports": [],
        "dynamic_table_dispatch": False,
        "reachable_dynamic_dispatch": False,
        "reachable_function_reference_dispatch": False,
        "reachable_indirect_call_tables": [],
        "reachable_table_reads": [],
        "exported_table_indices": exported_tables,
        "tables": table_facts,
        "defined_memory_count": sum(
            wasm_link_format._read_varuint(payload, 0)[0]
            for section_id, payload in sections
            if section_id == 5
        ),
        "custom_section_names": _fixture_custom_names(data),
        "linking_symbol_table_present": "linking" in _fixture_custom_names(data),
        "linking_symbols": _fixture_linking_symbols(data),
        "function_names": list(wasm_link_format._collect_func_names(data).items()),
        "split_runtime_got_data_globals": [],
        "canonical_import_types": sorted(
            canonical_import_types,
            key=lambda row: (str(row["module"]), str(row["name"])),
        ),
        "canonical_export_types": sorted(
            canonical_export_types,
            key=lambda row: str(row["name"]),
        ),
    }


@pytest.fixture(autouse=True)
def _native_symbol_reader_fixture(
    monkeypatch: pytest.MonkeyPatch, tmp_path_factory: pytest.TempPathFactory
) -> None:
    """Mock SDK selection and nm transport, retaining native artifact admission."""
    reader_path: Path | None = None
    real_tool_version = native_symbol_inspection._tool_version

    def tool_version(path: Path) -> str | None:
        # The mock reader is an llvm-nm transport: it answers the --version
        # probe with llvm-nm's banner, so reader admission names its family.
        if reader_path is not None and path == reader_path:
            return "llvm-nm, compatible with GNU nm"
        return real_tool_version(path)

    monkeypatch.setattr(native_symbol_inspection, "_tool_version", tool_version)

    def reader(*, nm_command, target_triple, requirement):
        nonlocal reader_path
        assert nm_command is None
        assert target_triple == "wasm32-wasip1"
        if reader_path is None:
            reader_path = write_mock_executable(
                tmp_path_factory.mktemp("wasm-native-reader") / "llvm-nm",
                b"mocked WASM llvm-nm transport v1\n",
            )
        candidate = native_symbol_inspection._native_symbol_reader_candidate(
            (str(reader_path),)
        )
        assert candidate.admission_error is None
        assert candidate.reader_family == "llvm"
        return native_symbol_inspection._NativeSymbolReader(
            (candidate,),
            (candidate.cache_identity(), requirement.cache_identity()),
            requirement,
        )

    def run_nm(command, **kwargs):
        assert reader_path is not None
        assert command[:3] == [str(reader_path), "-g", "--no-llvm-bc"]
        assert len(command) == 4
        path = Path(command[3])
        assert kwargs["cwd"] == path.parent
        rows: list[str] = []
        archive = path.read_bytes().startswith(wasm_archive.AR_MAGIC)
        for member in wasm_archive.iter_wasm_object_members(path):
            if archive:
                rows.append(f"{member.name}:")
            for symbol in _fixture_linking_symbols(member.data):
                flags = symbol["flags"]
                binding = flags & SYMBOL_BINDING_MASK
                if binding == FLAG_BINDING_LOCAL:
                    continue
                if flags & wasm_link_format.FLAG_UNDEFINED:
                    if binding == FLAG_BINDING_WEAK:
                        kind = "w" if symbol["kind"] == "function" else "v"
                    else:
                        kind = "U"
                elif symbol["kind"] == "function":
                    kind = "W" if binding == FLAG_BINDING_WEAK else "T"
                else:
                    kind = "V" if binding == FLAG_BINDING_WEAK else "D"
                rows.append(f"00000000 {kind} {symbol['name']}")
        return subprocess.CompletedProcess(command, 0, "\n".join(rows), "")

    monkeypatch.setattr(native_symbol_inspection, "_native_symbol_reader", reader)
    monkeypatch.setattr(native_symbol_inspection, "_run_completed_command", run_nm)


@pytest.fixture(autouse=True)
def _rust_facts_authority_fixture(monkeypatch: pytest.MonkeyPatch) -> None:
    def facts_provider(
        _scanner,
        _scratch_root,
        metrics=None,
        *,
        evidence_root=None,
        expected_sha256=None,
    ):  # type: ignore[no-untyped-def]
        if metrics is not None:
            metrics.update(
                {
                    "wasm_facts_hash_ms": 0.0,
                    "wasm_facts_scan_ms": 0.0,
                    "wasm_facts_scan_calls": 0.0,
                    "wasm_facts_cache_hits": 0.0,
                    "wasm_facts_input_bytes": 0.0,
                    "wasm_facts_response_chars": 0.0,
                }
            )
        return _facts_provider

    monkeypatch.setattr(
        wasm_link_fact_provider,
        "make_rust_wasm_facts_provider",
        facts_provider,
    )
    monkeypatch.setattr(wasm_link, "make_rust_wasm_facts_provider", facts_provider)


_REAL_RUN_WASM_LD = wasm_link._run_wasm_ld


def _run_wasm_ld_with_rust_facts(*args, **kwargs):  # type: ignore[no-untyped-def]
    kwargs.setdefault("wasm_facts_scanner", Path("rust-facts-fixture"))
    kwargs.setdefault("runtime_role", "shared")
    if kwargs.get("app_export_contract_path") is None:
        contract_path = Path(args[2]).with_name(
            f".{Path(args[2]).name}.test-app-export-contract.json"
        )
        kwargs["app_export_contract_path"] = _write_app_export_contract(
            contract_path,
            entry_module="test_app",
            source="",
            symbols=[],
        )
    if kwargs.get("split_runtime"):
        kwargs.setdefault("deploy_runtime_override", Path(args[1]))
    if not kwargs.get("split_runtime"):
        return _REAL_RUN_WASM_LD(*args, **kwargs)

    # These linker unit fixtures intentionally use minimal synthetic modules.
    # Bind their executable-runtime layout authority to the synthetic app facts
    # so the tests exercise linker behavior without weakening the production
    # reader's fail-closed WASM validation.
    output_layout = wasm_link_callable_table._callable_layout_from_wasm_facts(
        _rust_facts_fixture(Path(args[2]).read_bytes()),
        artifact_role="plan",
    )
    assert output_layout is not None
    runtime_layout = wasm_artifact.WasmSplitRuntimeCallableLayout(
        runtime_callable_base=output_layout.fixed_prefix_base,
        runtime_occupied_end=(
            output_layout.fixed_prefix_base + output_layout.fixed_prefix_len
        ),
        runtime_table_min=output_layout.finalized_app_base,
        fixed_prefix_len=output_layout.fixed_prefix_len,
    )
    real_split_app_global_base = wasm_link_runtime_data._split_app_global_base
    with pytest.MonkeyPatch.context() as patch:
        patch.setattr(
            wasm_link_pipeline,
            "read_wasm_split_runtime_callable_layout",
            lambda _path: runtime_layout,
        )
        patch.setattr(
            wasm_link_runtime_data,
            "_split_app_global_base",
            lambda output_data: (
                real_split_app_global_base(output_data)
                if wasm_link_runtime_data._active_data_segment_intervals(output_data)
                else 64 * 1024 * 1024
            ),
        )
        patch.setattr(
            wasm_link_runtime_data,
            "_validate_split_app_data_layout",
            lambda output_data, _linked_data, *, planned_base: (
                wasm_link_runtime_data._active_data_segment_intervals(output_data)
                or ((planned_base - 1, planned_base),),
                ((planned_base, planned_base + 1),),
            ),
        )
        # The synthetic runtime's own build ABI: every generated import whose
        # split export it actually defines.
        runtime_exports = set(
            wasm_link_format._collect_function_exports(Path(args[1]).read_bytes())
        )
        registry = wasm_link_runtime_data._runtime_exports
        kwargs.setdefault(
            "deploy_runtime_imports",
            tuple(
                name
                for name in registry.wasm_runtime_import_names()
                if registry.wasm_split_runtime_export_name_for_import(name)
                in runtime_exports
            ),
        )
        return _REAL_RUN_WASM_LD(*args, **kwargs)


class _FixtureWasmFactsProvider:
    authority_digest = "f" * 64

    def __call__(self, data: bytes):  # type: ignore[no-untyped-def]
        facts = wasm_link_fact_provider._freeze_json(_rust_facts_fixture(data))
        assert isinstance(facts, dict)
        return wasm_link_fact_provider.WasmLinkFacts(facts)

    def publish_in_place(
        self,
        artifact: Path,
        *,
        layout=None,  # type: ignore[no-untyped-def]
        role: str = "monolithic",
    ) -> wasm_link_fact_provider.WasmLinkFacts:
        published = _with_test_link_facts(artifact.read_bytes(), role=role)
        artifact.write_bytes(published)
        facts = _rust_facts_fixture(published)
        if layout is not None:
            app_entry_count = (
                len(facts["callable_table_entries"])
                if role == "app"
                else layout.app_entry_count
            )
            facts["callable_table_layout"] = {
                "fixed_prefix_base": layout.fixed_prefix_base,
                "fixed_prefix_len": layout.fixed_prefix_len,
                "finalized_app_base": layout.finalized_app_base,
                "app_entry_count": app_entry_count,
            }
        frozen = wasm_link_fact_provider._freeze_json(facts)
        assert isinstance(frozen, dict)
        return wasm_link_fact_provider.WasmLinkFacts(frozen)


_facts_provider = _FixtureWasmFactsProvider()


def _with_test_link_facts(data: bytes, *, role: str) -> bytes:
    sections = wasm_link_operations.parse_sections(data)
    sections.append(
        (
            0,
            wasm_link_format._build_custom_section(
                "molt.test-link-facts", role.encode("ascii")
            ),
        )
    )
    return wasm_link_operations.build_sections(sections)


def test_wasm_artifact_state_reuses_facts_until_bytes_change(tmp_path: Path) -> None:
    path = tmp_path / "artifact.wasm"
    initial = b"\0asm\x01\0\0\0"
    changed = wasm_link_operations.build_sections(
        [(0, wasm_link_format._build_custom_section("molt.changed", b"yes"))]
    )
    calls: list[bytes] = []

    def facts_provider(data: bytes):  # type: ignore[no-untyped-def]
        calls.append(data)
        return _facts_provider(data)

    state = wasm_link_transaction.WasmArtifactState.from_bytes(
        path,
        initial,
        facts_provider=facts_provider,
    )
    state.persist()
    assert path.read_bytes() == initial
    first = state.facts()

    assert state.facts() is first
    assert not state.replace(initial)
    assert state.facts() is first
    assert state.revision == 0
    assert state.replace(changed)
    assert path.read_bytes() == initial
    state.persist()
    assert path.read_bytes() == changed
    assert state.revision == 1
    assert state.facts() is not first
    assert calls == [initial, changed]


def test_wasm_artifact_state_atomic_path_mutation_invalidates_facts(
    tmp_path: Path,
) -> None:
    path = tmp_path / "artifact.wasm"
    initial = b"\0asm\x01\0\0\0"
    changed = wasm_link_operations.build_sections(
        [(0, wasm_link_format._build_custom_section("molt.external", b"yes"))]
    )
    path.write_bytes(initial)
    calls: list[bytes] = []

    def facts_provider(data: bytes):  # type: ignore[no-untyped-def]
        calls.append(data)
        return _facts_provider(data)

    state = wasm_link_transaction.WasmArtifactState.from_bytes(
        path,
        initial,
        facts_provider=facts_provider,
    )
    first = state.facts()

    def publish(artifact: Path) -> str:
        artifact.write_bytes(changed)
        return "published"

    assert state.apply_atomic_path_mutation(publish) == "published"
    assert state.data == changed
    assert path.read_bytes() == changed
    assert state.revision == 1
    changed_facts = state.facts()
    assert changed_facts is not first
    assert calls == [initial, changed]
    assert not state.apply_atomic_path_mutation(lambda _path: False)
    assert state.data == changed
    assert state.revision == 1

    committed_before_error = wasm_link_operations.build_sections(
        [(0, wasm_link_format._build_custom_section("molt.committed", b"yes"))]
    )

    def commit_then_raise(artifact: Path) -> None:
        artifact.write_bytes(committed_before_error)
        raise RuntimeError("publication response failed after commit")

    with pytest.raises(RuntimeError, match="response failed after commit"):
        state.apply_atomic_path_mutation(commit_then_raise)
    assert state.data == committed_before_error
    assert path.read_bytes() == committed_before_error
    assert state.revision == 2
    assert state.facts() is not changed_facts
    assert calls == [initial, changed, committed_before_error]


def test_rust_facts_publication_is_admitted_through_artifact_state(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    scanner = tmp_path / "molt-backend"
    scanner.write_bytes(b"scanner")
    path = tmp_path / "artifact.wasm"
    initial = b"\0asm\x01\0\0\0"
    published = wasm_link_operations.build_sections(
        [(0, wasm_link_format._build_custom_section("molt.attested", b"yes"))]
    )
    provider = _REAL_MAKE_RUST_WASM_FACTS_PROVIDER(
        scanner,
        tmp_path / "facts-scratch",
    )
    state = wasm_link_transaction.WasmArtifactState.from_bytes(
        path,
        initial,
        facts_provider=provider,
    )
    state.persist()
    scanner.write_bytes(b"different scanner generation")
    payload = json.dumps(
        {
            "schema_version": 7,
            "ok": True,
            "facts": _rust_facts_fixture(published),
        }
    )
    publication_paths: list[Path] = []

    def fake_run(_executor, command, **_kwargs):  # type: ignore[no-untyped-def]
        assert Path(command[0]) == provider.scanner_identity.path
        assert command[1:3] == ["--publish-wasm-link-facts", str(path)]
        output = Path(command[4])
        assert command[3] == "--output"
        assert output == path
        assert path.read_bytes() == initial
        output.write_bytes(published)
        publication_paths.append(output)
        return subprocess.CompletedProcess(command, 0, payload, "")

    monkeypatch.setattr(wasm_link_fact_provider.CommandExecutor, "run", fake_run)

    facts = state.apply_atomic_facts_publication(
        lambda artifact: provider.publish_in_place(artifact)
    )

    assert facts["callable_table_attestation_present"] is True
    assert state.facts() is facts
    assert state.data == published
    assert path.read_bytes() == published
    assert state.revision == 1
    assert publication_paths == [path]


def test_rust_facts_provider_attests_scan_cost_and_content_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    scanner = tmp_path / "molt-backend"
    scanner.write_bytes(b"scanner")
    scanner_facts = _rust_facts_fixture(b"\0asm\x01\0\0\0")
    scanner_facts["reachable_function_indices"] = [0]
    scanner_facts["referenced_function_indices"] = [0]
    payload = json.dumps(
        {
            "schema_version": 7,
            "ok": True,
            "facts": scanner_facts,
        }
    )
    calls = 0
    invoked_scanners: list[Path] = []

    def fake_run(_executor, command, **_kwargs):
        nonlocal calls
        calls += 1
        invoked_scanners.append(Path(command[0]))

        class Result:
            returncode = 0
            stdout = payload
            stderr = ""

        return Result()

    monkeypatch.setattr(wasm_link_fact_provider.CommandExecutor, "run", fake_run)
    metrics: dict[str, float] = {}
    provider = _REAL_MAKE_RUST_WASM_FACTS_PROVIDER(scanner, tmp_path, metrics)
    scanner.write_bytes(b"mutated-after-sealing")

    first = provider(b"representative-wasm")
    second = provider(b"representative-wasm")

    assert first is second
    assert invoked_scanners == [provider.scanner_identity.path]
    assert provider.scanner_identity.path != scanner
    assert provider.scanner_identity.path.read_bytes() == b"scanner"
    with pytest.raises(TypeError, match="WASM facts are immutable"):
        first["reachable_function_indices"] = ()
    assert first["reachable_function_indices"] == (0,)
    assert calls == 1
    assert metrics["wasm_facts_scan_calls"] == 1.0
    assert metrics["wasm_facts_cache_hits"] == 1.0
    assert metrics["wasm_facts_input_bytes"] == len(b"representative-wasm")
    assert metrics["wasm_facts_response_chars"] == len(payload)
    assert metrics["wasm_facts_hash_ms"] >= 0.0
    assert metrics["wasm_facts_scan_ms"] >= 0.0

    for index in range(wasm_link_fact_provider._WASM_FACTS_CACHE_ENTRIES + 1):
        provider(f"representative-wasm-{index}".encode("ascii"))
    assert len(provider._cache) == wasm_link_fact_provider._WASM_FACTS_CACHE_ENTRIES
    assert hashlib.sha256(b"representative-wasm").hexdigest() not in provider._cache


def test_wasm_link_uses_one_command_execution_module_identity() -> None:
    assert wasm_link_command.CommandExecutor is wasm_link_fact_provider.CommandExecutor


def test_snapshot_link_input_rejects_stable_invalid_prefix_without_retry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "output.wasm"
    source.write_bytes(b"partial")
    attempts = 0
    real_snapshot = wasm_link.snapshot_stable_regular_file

    def counting_snapshot(*args, **kwargs):  # type: ignore[no-untyped-def]
        nonlocal attempts
        attempts += 1
        return real_snapshot(*args, **kwargs)

    monkeypatch.setattr(wasm_link, "snapshot_stable_regular_file", counting_snapshot)

    with pytest.raises(OSError, match="stable prefix failed linker input contract"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="app",
            attempts=100,
            retry_delay_seconds=0,
            required_prefix=b"complete-molt-main",
        )

    assert attempts == 1
    assert not (tmp_path / "snapshots" / "app" / source.name).exists()


def test_stable_snapshot_streams_source_through_one_read_handle(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    payload = bytes(range(251)) * 16_777
    source.write_bytes(payload)
    source_open_count = 0
    real_open = identity_authority.open_stable_read_descriptor
    real_read_bytes = Path.read_bytes

    def counting_open(path):  # type: ignore[no-untyped-def]
        nonlocal source_open_count
        if Path(path) == source:
            source_open_count += 1
        return real_open(path)

    def reject_path_read_bytes(path: Path) -> bytes:
        if path in {source, snapshot}:
            raise AssertionError("stable snapshot must use its single open handle")
        return real_read_bytes(path)

    monkeypatch.setattr(
        identity_authority, "open_stable_read_descriptor", counting_open
    )
    monkeypatch.setattr(Path, "read_bytes", reject_path_read_bytes)
    monkeypatch.setattr(identity_authority, "_STABLE_SNAPSHOT_CHUNK_BYTES", 64 * 1024)

    captured = identity_authority.snapshot_stable_regular_file(
        source,
        snapshot,
        label="profiled source",
        capture_prefix_bytes=17,
    )

    assert source_open_count == 1
    assert captured.prefix == payload[:17]
    assert len(captured.prefix) == 17
    assert captured.source.size == captured.snapshot.size == len(payload)
    assert (
        captured.source.sha256
        == captured.snapshot.sha256
        == hashlib.sha256(payload).hexdigest()
    )


def test_stable_snapshot_preserves_unowned_existing_destination(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"new snapshot")
    snapshot.write_bytes(b"pre-existing authority")

    with pytest.raises(FileExistsError):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="owned source",
        )

    assert snapshot.read_bytes() == b"pre-existing authority"


def test_stable_snapshot_discard_preserves_replacement_file(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"owned snapshot")
    captured = identity_authority.snapshot_stable_regular_file(
        source,
        snapshot,
        label="owned source",
    )
    snapshot.unlink()
    snapshot.write_bytes(b"replacement authority")

    captured.discard()

    assert snapshot.read_bytes() == b"replacement authority"


def test_stable_snapshot_rejects_same_size_tamper_before_snapshot_attestation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"original")
    real_identity = identity_authority.stable_regular_file_identity
    tampered = False

    def tamper_before_identity(path: Path, *, label: str):  # type: ignore[no-untyped-def]
        nonlocal tampered
        if path == snapshot and not tampered:
            tampered = True
            snapshot.write_bytes(b"tampered")
        return real_identity(path, label=label)

    monkeypatch.setattr(
        identity_authority,
        "stable_regular_file_identity",
        tamper_before_identity,
    )

    with pytest.raises(
        identity_authority.StableRegularFileSnapshotError,
        match="snapshot content changed",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="tampered source",
        )

    assert tampered
    assert not snapshot.exists()


def test_stable_snapshot_rejects_midstream_source_change(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = (tmp_path / "source.bin").absolute()
    snapshot = (tmp_path / "snapshot.bin").absolute()
    source.write_bytes(b"abcdefgh")
    initial_stat = source.stat()
    real_sha256 = identity_authority.hashlib.sha256
    changed = False

    class ChangingHasher:
        def __init__(self) -> None:
            self._inner = real_sha256()

        def update(self, data: bytes) -> None:
            nonlocal changed
            self._inner.update(data)
            if not changed:
                changed = True
                os.utime(
                    source,
                    ns=(
                        initial_stat.st_atime_ns,
                        initial_stat.st_mtime_ns + 1_000_000_000,
                    ),
                )

        def hexdigest(self) -> str:
            return self._inner.hexdigest()

    monkeypatch.setattr(identity_authority, "_STABLE_SNAPSHOT_CHUNK_BYTES", 4)
    monkeypatch.setattr(identity_authority.hashlib, "sha256", ChangingHasher)

    with pytest.raises(
        identity_authority.StableRegularFileChangedError,
        match="changed during identity read",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="racing source",
        )

    assert changed
    assert not snapshot.exists()


def test_stable_snapshot_rejects_symlink_source(tmp_path: Path) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    source.write_bytes(b"source")
    indirect = tmp_path / "indirect.bin"
    try:
        indirect.symlink_to(source)
    except OSError as exc:
        pytest.skip(f"symlinks unavailable on this host: {exc}")

    with pytest.raises(
        identity_authority.StableRegularFileError,
        match="not one stable regular file",
    ):
        identity_authority.snapshot_stable_regular_file(
            indirect,
            tmp_path / "snapshot.bin",
            label="indirect source",
        )


def test_stable_snapshot_rejects_reparse_metadata_as_path_indirection(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    source.write_bytes(b"source")
    source_stat = source.lstat()
    source_identity = identity_authority._stat_identity(source_stat)
    real_is_reparse = identity_authority.metadata_is_link_like

    def mark_source_as_reparse(metadata: os.stat_result) -> bool:
        return identity_authority._stat_identity(
            metadata
        ) == source_identity or real_is_reparse(metadata)

    monkeypatch.setattr(
        identity_authority,
        "metadata_is_link_like",
        mark_source_as_reparse,
    )

    with pytest.raises(
        identity_authority.StableRegularFileError,
        match="not one stable regular file",
    ):
        identity_authority.snapshot_stable_regular_file(
            source,
            tmp_path / "snapshot.bin",
            label="reparse source",
        )


def test_reparse_metadata_capability_uses_platform_file_attributes(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    reparse_attribute = 0x00000400
    metadata = type(
        "_WindowsStatMetadata",
        (),
        {"st_file_attributes": reparse_attribute, "st_mode": 0o100600},
    )()
    monkeypatch.setattr(
        identity_authority.stat,
        "FILE_ATTRIBUTE_REPARSE_POINT",
        reparse_attribute,
        raising=False,
    )

    assert identity_authority.metadata_is_link_like(metadata)


def test_snapshot_link_input_retries_proven_concurrent_change(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"stable")
    real_snapshot = wasm_link.snapshot_stable_regular_file
    attempts = 0

    def race_once(*args, **kwargs):  # type: ignore[no-untyped-def]
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            raise wasm_link.StableRegularFileChangedError("concurrent mutation")
        return real_snapshot(*args, **kwargs)

    monkeypatch.setattr(wasm_link, "snapshot_stable_regular_file", race_once)

    snapshot = wasm_link._snapshot_link_input(
        source,
        tmp_path / "snapshots",
        label="racing-source",
        retry_delay_seconds=0,
    )

    assert attempts == 2
    assert snapshot.read_bytes() == b"stable"


def test_snapshot_link_input_does_not_retry_stable_custody_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    source = tmp_path / "source.bin"
    source.write_bytes(b"stable")
    attempts = 0

    def fail_once(*_args, **_kwargs):  # type: ignore[no-untyped-def]
        nonlocal attempts
        attempts += 1
        raise wasm_link.StableRegularFileError("permanent custody failure")

    monkeypatch.setattr(wasm_link, "snapshot_stable_regular_file", fail_once)

    with pytest.raises(OSError, match="permanent custody failure"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="unavailable-source",
            retry_delay_seconds=0,
        )

    assert attempts == 1


def test_snapshot_link_input_does_not_retry_snapshot_attestation_error(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    source.write_bytes(b"stable")
    attempts = 0

    def fail_attestation(*_args, **_kwargs):  # type: ignore[no-untyped-def]
        nonlocal attempts
        attempts += 1
        raise identity_authority.StableRegularFileSnapshotError(
            "snapshot attestation failure"
        )

    monkeypatch.setattr(wasm_link, "snapshot_stable_regular_file", fail_attestation)

    with pytest.raises(OSError, match="snapshot attestation failure"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="attestation-failure",
            attempts=100,
            retry_delay_seconds=0,
        )

    assert attempts == 1


def test_snapshot_link_input_rejects_deterministic_preflight_without_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "runtime_reloc.wasm"
    source.write_bytes(b"bad-generation")
    attempts = 0

    def reject_path(_snapshot: Path) -> bool:
        nonlocal attempts
        attempts += 1
        return False

    with pytest.raises(OSError, match="failed linker metadata preflight"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="runtime",
            attempts=100,
            retry_delay_seconds=0,
            accept_path=reject_path,
        )

    assert attempts == 1
    assert not (tmp_path / "snapshots" / "runtime" / source.name).exists()


def test_stable_snapshot_missing_change_time_is_permanent_custody_failure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import toolchain_identity as identity_authority

    source = tmp_path / "source.bin"
    snapshot = tmp_path / "snapshot.bin"
    source.write_bytes(b"stable")
    monkeypatch.setattr(
        identity_authority,
        "content_change_time_ns_from_fd",
        lambda *_args: None,
    )

    with pytest.raises(identity_authority.StableRegularFileError) as error:
        identity_authority.snapshot_stable_regular_file(
            source,
            snapshot,
            label="unsupported custody source",
        )

    assert type(error.value) is identity_authority.StableRegularFileError
    assert "change-time identity" in str(error.value)
    assert not snapshot.exists()


def test_split_contract_rejects_missing_app_owned_molt_main() -> None:
    module = b"\0asm\x01\0\0\0"

    with pytest.raises(
        ValueError,
        match="missing app-owned function export molt_main after symbol restoration at unspecified",
    ):
        wasm_link_export_contract._restore_split_runtime_contract_exports(
            module,
            artifact="app",
            facts_provider=_facts_provider,
        )


def test_output_export_symbol_map_rejects_ambiguous_alias_authority() -> None:
    module = _build_exported_runtime_module("molt_main")
    module = wasm_link_format._append_linking_function_symbols(
        module,
        [
            (
                "molt_main",
                0,
                FLAG_BINDING_GLOBAL
                | wasm_link_format.FLAG_EXPLICIT_NAME
                | FLAG_EXPORTED
                | FLAG_NO_STRIP,
            ),
            (
                "__molt_output_export_0",
                0,
                FLAG_BINDING_GLOBAL
                | wasm_link_format.FLAG_EXPLICIT_NAME
                | FLAG_EXPORTED
                | FLAG_NO_STRIP,
            ),
        ],
        facts_provider=_facts_provider,
    )
    assert module is not None

    with pytest.raises(ValueError, match="ambiguous linker symbol identity"):
        wasm_link_edit._collect_output_export_symbol_map(
            module, facts_provider=_facts_provider
        )


def test_output_export_symbol_map_accepts_shared_export_index_with_one_symbol() -> None:
    module = _build_exported_runtime_module("first")
    sections = wasm_link_operations.parse_sections(module)
    for section_index, (section_id, _payload) in enumerate(sections):
        if section_id != 7:
            continue
        export_payload = bytearray(wasm_link_format._write_varuint(2))
        for name in ("first", "second"):
            export_payload.extend(wasm_link_format._write_string(name))
            export_payload.append(0x00)
            export_payload.extend(wasm_link_format._write_varuint(0))
        sections[section_index] = (7, bytes(export_payload))
        break
    module = wasm_link_operations.build_sections(sections)
    module = wasm_link_format._append_linking_function_symbols(
        module,
        [
            (
                "canonical_fn_0",
                0,
                FLAG_BINDING_GLOBAL
                | wasm_link_format.FLAG_EXPLICIT_NAME
                | FLAG_EXPORTED
                | FLAG_NO_STRIP,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert module is not None

    assert wasm_link_edit._collect_output_export_symbol_map(
        module, facts_provider=_facts_provider
    ) == {
        "first": "canonical_fn_0",
        "second": "canonical_fn_0",
    }


def test_add_symtab_alias_uses_exact_parsed_symbol_identity() -> None:
    module = _build_exported_runtime_module("target")
    module = wasm_link_format._append_linking_function_symbols(
        module,
        [("prefix\x05aliassuffix", 0, wasm_link_format.FLAG_EXPLICIT_NAME)],
        facts_provider=_facts_provider,
    )
    assert module is not None

    updated = wasm_link_edit._add_symtab_alias(
        module,
        "alias",
        0,
        FLAG_BINDING_GLOBAL,
        facts_provider=_facts_provider,
    )

    assert updated is not None
    assert [
        symbol.name
        for symbol in parse_wasm_linking_symbols(updated).function_symbols
        if symbol.name == "alias"
    ] == ["alias"]


def _app_adapter_call_abi() -> dict[str, object]:
    contract = build_app_export_contract(
        entry_module="probe",
        ir={"functions": [{"app_callable_bindings": []}]},
        registry_digest="a" * 64,
    )
    return app_export_call_abi(contract)


def _build_app_adapter_input(
    arities: tuple[int, ...],
    *,
    target_result_type: int = 0x7E,
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []
    type_payload = bytearray(write_varuint(len(arities)))
    for arity in arities:
        type_payload.append(0x60)
        type_payload.extend(write_varuint(arity))
        type_payload.extend(bytes([0x7E]) * arity)
        type_payload.extend(write_varuint(1))
        type_payload.append(target_result_type)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray(write_varuint(len(arities)))
    for type_idx in range(len(arities)):
        func_payload.extend(write_varuint(type_idx))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray(write_varuint(len(arities)))
    for index in range(len(arities)):
        export_payload.extend(wasm_link_format._write_string(f"probe__f{index}"))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray(write_varuint(len(arities)))
    for _ in arities:
        body = bytes([0x00, 0x42, 0x00, 0x0B])
        code_payload.extend(write_varuint(len(body)))
        code_payload.extend(body)
    code_section_index = sum(1 for section_id, _payload in sections if section_id != 0)
    sections.append((10, bytes(code_payload)))
    sections.append(
        (
            0,
            wasm_link_format._build_custom_section(
                "reloc.CODE",
                write_varuint(code_section_index) + write_varuint(0),
            ),
        )
    )
    module = wasm_link_operations.build_sections(sections)
    symbols: list[tuple[str, int, int]] = []
    for index in range(len(arities)):
        symbols.append(
            (
                f"__molt_output_export_{index}",
                index,
                FLAG_BINDING_GLOBAL
                | wasm_link_format.FLAG_EXPLICIT_NAME
                | FLAG_EXPORTED
                | FLAG_NO_STRIP,
            )
        )
    with_symbols = wasm_link_format._append_linking_function_symbols(
        module, symbols, facts_provider=_facts_provider
    )
    assert with_symbols is not None
    return with_symbols


def _defined_function_bodies(wasm_bytes: bytes) -> list[bytes]:
    for section_id, payload in wasm_link_operations.parse_sections(wasm_bytes):
        if section_id != 10:
            continue
        count, offset = wasm_link_format._read_varuint(payload, 0)
        bodies: list[bytes] = []
        for _ in range(count):
            size, offset = wasm_link_format._read_varuint(payload, offset)
            bodies.append(payload[offset : offset + size])
            offset += size
        assert offset == len(payload)
        return bodies
    return []


def _replace_defined_function_body(
    wasm_bytes: bytes, defined_index: int, replacement: bytes
) -> bytes:
    sections = wasm_link_operations.parse_sections(wasm_bytes)
    rewritten: list[tuple[int, bytes]] = []
    for section_id, payload in sections:
        if section_id != 10:
            rewritten.append((section_id, payload))
            continue
        count, offset = wasm_link_format._read_varuint(payload, 0)
        bodies: list[bytes] = []
        for _ in range(count):
            size, offset = wasm_link_format._read_varuint(payload, offset)
            bodies.append(payload[offset : offset + size])
            offset += size
        assert offset == len(payload)
        bodies[defined_index] = replacement
        code_payload = bytearray(wasm_link_format._write_varuint(len(bodies)))
        for body in bodies:
            code_payload.extend(wasm_link_format._write_varuint(len(body)))
            code_payload.extend(body)
        rewritten.append((section_id, bytes(code_payload)))
    return wasm_link_operations.build_sections(rewritten)


def _code_relocations(wasm_bytes: bytes) -> list[tuple[int, int, int]]:
    for section_id, payload in wasm_link_operations.parse_sections(wasm_bytes):
        if section_id != 0:
            continue
        name, custom_payload = wasm_link_format._parse_custom_section(payload)
        if name != "reloc.CODE":
            continue
        _target_section, offset = wasm_link_format._read_varuint(custom_payload, 0)
        count, offset = wasm_link_format._read_varuint(custom_payload, offset)
        entries: list[tuple[int, int, int]] = []
        for _ in range(count):
            relocation_type = custom_payload[offset]
            relocation_offset, offset = wasm_link_format._read_varuint(
                custom_payload, offset + 1
            )
            symbol_index, offset = wasm_link_format._read_varuint(
                custom_payload, offset
            )
            if relocation_type in (4, 5):
                _addend, offset = wasm_link_format._read_varuint(custom_payload, offset)
            entries.append((relocation_type, relocation_offset, symbol_index))
        assert offset == len(custom_payload)
        return entries
    return []


def test_app_export_adapters_sweep_arity_and_forward_owned_result_boundary(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_app_adapter_input((0, 1, 3)))
    temp_dir = OwnedTemporaryDirectory(dir=tmp_path)
    try:
        adapted_path, adapter_map = wasm_link_edit._inject_app_export_adapters(
            output,
            temp_dir,
            public_export_names=("probe__f0", "probe__f1", "probe__f2"),
            call_abi=_app_adapter_call_abi(),
            facts_provider=_facts_provider,
        )
        adapted = adapted_path.read_bytes()
    finally:
        temp_dir.cleanup()

    prefix = wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX
    assert adapter_map == {
        "probe__f0": f"{prefix}probe__f0",
        "probe__f1": f"{prefix}probe__f1",
        "probe__f2": f"{prefix}probe__f2",
    }
    exports = wasm_link_format._collect_function_exports(adapted)
    assert exports[f"{prefix}probe__f0"] == 3
    assert exports[f"{prefix}probe__f1"] == 4
    assert exports[f"{prefix}probe__f2"] == 5
    assert set(adapter_map.values()).issubset(
        parse_wasm_linking_symbols(adapted).defined_names
    )
    adapter_symbols = {
        symbol.name: symbol
        for symbol in parse_wasm_linking_symbols(adapted).function_symbols
        if symbol.name in adapter_map.values()
    }
    assert set(adapter_symbols) == set(adapter_map.values())
    assert all(
        symbol.flags & SYMBOL_BINDING_MASK == FLAG_BINDING_GLOBAL
        and symbol.flags & SYMBOL_BINDING_MASK != FLAG_BINDING_WEAK
        for symbol in adapter_symbols.values()
    )
    assert _defined_function_bodies(adapted)[-3:] == [
        bytes.fromhex("001080808080000b"),
        bytes.fromhex("0020001081808080000b"),
        bytes.fromhex("002000200120021082808080000b"),
    ]
    relocations = _code_relocations(adapted)
    assert [entry[0] for entry in relocations] == [0] * 3
    assert [entry[2] for entry in relocations] == [0, 1, 2]


def test_app_export_adapter_validator_replaces_raw_target_identity(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_app_adapter_input((0, 1)))
    temp_dir = OwnedTemporaryDirectory(dir=tmp_path)
    try:
        adapted_path, adapter_map = wasm_link_edit._inject_app_export_adapters(
            output,
            temp_dir,
            public_export_names=("probe__f0", "probe__f1"),
            call_abi=_app_adapter_call_abi(),
            facts_provider=_facts_provider,
        )
        adapted = adapted_path.read_bytes()
    finally:
        temp_dir.cleanup()

    target_map = {
        "probe__f0": "__molt_output_export_0",
        "probe__f1": "__molt_output_export_1",
    }
    with pytest.raises(ValueError, match="points to raw target"):
        wasm_link_edit._validate_app_export_adapters(
            adapted,
            ("probe__f0", "probe__f1"),
            adapter_symbol_map=adapter_map,
            target_symbol_map=target_map,
            facts_provider=_facts_provider,
        )

    restored = wasm_link_export_contract._restore_public_output_exports(
        adapted,
        adapter_map,
        facts_provider=_facts_provider,
    )
    wasm_link_edit._validate_app_export_adapters(
        restored,
        ("probe__f0", "probe__f1"),
        adapter_symbol_map=adapter_map,
        target_symbol_map=target_map,
        facts_provider=_facts_provider,
    )
    wasm_link_edit._validate_app_export_adapters(
        restored, ("probe__f0", "probe__f1"), facts_provider=_facts_provider
    )
    exports = wasm_link_format._collect_function_exports(restored)
    symbols = {
        symbol.name: symbol.index
        for symbol in parse_wasm_linking_symbols(restored).function_symbols
        if symbol.name and symbol.index is not None
    }
    assert exports["probe__f0"] == symbols[adapter_map["probe__f0"]]
    assert exports["probe__f0"] != symbols[target_map["probe__f0"]]


def test_app_export_adapter_identity_survives_metadata_strip_and_rejects_wrong_call(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_app_adapter_input((1, 1)))
    temp_dir = OwnedTemporaryDirectory(dir=tmp_path)
    try:
        adapted_path, adapter_map = wasm_link_edit._inject_app_export_adapters(
            output,
            temp_dir,
            public_export_names=("probe__f0", "probe__f1"),
            call_abi=_app_adapter_call_abi(),
            facts_provider=_facts_provider,
        )
        adapted = wasm_link_export_contract._restore_public_output_exports(
            adapted_path.read_bytes(),
            adapter_map,
            facts_provider=_facts_provider,
        )
    finally:
        temp_dir.cleanup()

    target_map = {
        "probe__f0": "__molt_output_export_0",
        "probe__f1": "__molt_output_export_1",
    }
    (
        adapter_identity,
        target_identity,
        identity_exports,
    ) = wasm_link_export_contract._app_export_identity_maps(
        adapter_map,
        target_map,
    )
    with_identity = wasm_link_export_contract._publish_app_export_identity_markers(
        adapted,
        public_export_names=("probe__f0", "probe__f1"),
        adapter_symbol_map=adapter_map,
        target_symbol_map=target_map,
        identity_exports=identity_exports,
        facts_provider=_facts_provider,
    )
    metadata_free = wasm_link_operations.strip_publication_sections(
        with_identity,
        final_artifact=True,
        preserve_debug=False,
    )
    wasm_link_edit._validate_app_export_adapters(
        metadata_free,
        ("probe__f0", "probe__f1"),
        adapter_symbol_map=adapter_identity,
        target_symbol_map=target_identity,
        facts_provider=_facts_provider,
    )

    # Keep the canonical forwarding shape but redirect f0 from raw target f0
    # to raw target f1. Shape-only validation cannot identify that semantic
    # corruption; optimizer-stable identity exports must reject it.
    wrong_target = bytes.fromhex("0020001081808080000b")
    corrupted = _replace_defined_function_body(metadata_free, 2, wrong_target)
    wasm_link_edit._validate_app_export_adapters(
        corrupted, ("probe__f0",), facts_provider=_facts_provider
    )
    with pytest.raises(ValueError, match="does not call"):
        wasm_link_edit._validate_app_export_adapters(
            corrupted,
            ("probe__f0", "probe__f1"),
            adapter_symbol_map=adapter_identity,
            target_symbol_map=target_identity,
            facts_provider=_facts_provider,
        )

    # The monolithic keep set names only public exports; the split-app keep
    # set also names the identity roots it carried through the optimizer.
    for keep in (set(), set(identity_exports)):
        stripped = wasm_link_export_contract._strip_app_export_identity_markers(
            metadata_free,
            identity_exports=identity_exports,
            preserve_exports={"probe__f0", "probe__f1", *keep},
            facts_provider=_facts_provider,
        )
        exports = wasm_link_format._collect_function_exports(stripped).keys()
        assert not set(identity_exports) & exports
        assert {"probe__f0", "probe__f1"} <= exports


def test_app_export_adapters_have_no_ownership_import_dependency(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_app_adapter_input((0,)))
    temp_dir = OwnedTemporaryDirectory(dir=tmp_path)
    try:
        adapted_path, _adapter_map = wasm_link_edit._inject_app_export_adapters(
            output,
            temp_dir,
            public_export_names=("probe__f0",),
            call_abi=_app_adapter_call_abi(),
            facts_provider=_facts_provider,
        )
        imports = wasm_link_format._collect_imports(adapted_path.read_bytes())
        assert all(
            name != "molt_inc_ref_obj" for _module, name, _kind, _desc in imports
        )
    finally:
        temp_dir.cleanup()


def test_app_export_adapters_fail_closed_on_noncanonical_target_signature(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_app_adapter_input((0,), target_result_type=0x7F))
    temp_dir = OwnedTemporaryDirectory(dir=tmp_path)
    try:
        with pytest.raises(ValueError, match=r"canonical \(i64\.\.\.\) -> i64"):
            wasm_link_edit._inject_app_export_adapters(
                output,
                temp_dir,
                public_export_names=("probe__f0",),
                call_abi=_app_adapter_call_abi(),
                facts_provider=_facts_provider,
            )
    finally:
        temp_dir.cleanup()


def test_deduplicated_export_flags_preserve_first_contract_order() -> None:
    assert wasm_link_command._deduplicated_export_flags(
        ("--export-if-defined=molt_Py_None", "--export=molt_main"),
        ("--export-if-defined=molt_Py_None", "--export=PyInit_numpy"),
        ("--export=molt_main",),
    ) == [
        "--export-if-defined=molt_Py_None",
        "--export=molt_main",
        "--export=PyInit_numpy",
    ]


def test_relocatable_runtime_preflight_classifies_linker_crash(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    runtime.write_bytes(b"\0asm\x01\0\0\0")

    def fake_run(cmd, **_kwargs):  # type: ignore[no-untyped-def]
        return wasm_link_command.subprocess.CompletedProcess(
            cmd,
            3221225477,
            stdout="",
            stderr="PLEASE submit a bug report to llvm-project",
        )

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)

    error = wasm_link_command._preflight_relocatable_runtime(
        "wasm-ld", runtime, tmp_path
    )

    assert error is not None
    assert "linking/reloc custom-section indices" in error
    assert "returncode=3221225477" in error


def test_find_wasm_ld_uses_attested_toolchain_authority(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    identity = wasm_link_command.wasm_toolchain.WasmLinkerIdentity(
        Path("C:/wasi-sdk/bin/wasm-ld.exe"), "22.1.7", "22.1.0"
    )
    monkeypatch.setattr(
        wasm_link_command.wasm_toolchain, "resolve_wasm_linker", lambda: identity
    )

    assert wasm_link_command._find_wasm_ld() == str(identity.path)
    diagnostic = capsys.readouterr().err
    assert "version=22.1.7" in diagnostic
    assert "sha256=unattested" in diagnostic
    assert "wasi-sdk-llvm=22.1.0" in diagnostic


def _write_wasm_ld_output(cmd: list[str], data: bytes) -> Path | None:
    if "-o" not in cmd:
        return None
    for argument in cmd:
        if argument.startswith("--why-extract="):
            Path(argument.split("=", 1)[1]).write_text(
                "reference\textracted\tsymbol\n", encoding="utf-8"
            )
    output_path = Path(cmd[cmd.index("-o") + 1])
    if "--emit-relocs" in cmd:
        if "linking" not in _fixture_custom_names(data):
            symbol_payload = wasm_link_format._write_varuint(0)
            linking = wasm_link_format._build_custom_section(
                "linking",
                wasm_link_format._build_linking_payload(
                    2, [(SYMTAB_SUBSECTION_ID, symbol_payload)]
                ),
            )
            data = wasm_link_operations.build_sections(
                [*wasm_link_operations.parse_sections(data), (0, linking)]
            )
    output_path.write_bytes(data)
    return output_path


def test_wasm_link_external_tool_uses_command_executor(monkeypatch) -> None:
    captured: dict[str, object] = {}

    def fake_run(_executor, cmd, **kwargs):  # type: ignore[no-untyped-def]
        captured["cmd"] = cmd
        captured["kwargs"] = kwargs
        return wasm_link_command.subprocess.CompletedProcess(
            cmd, 0, stdout="ok\n", stderr=""
        )

    monkeypatch.setattr(wasm_link_command.CommandExecutor, "run", fake_run)

    result = wasm_link_command._run_external_tool(["wasm-tools", "validate", "x.wasm"])

    assert result.returncode == 0
    assert result.stdout == "ok\n"
    assert captured["cmd"] == ["wasm-tools", "validate", "x.wasm"]
    assert captured["kwargs"]["capture_output"] is True
    assert captured["kwargs"]["text"] is True


def test_wasm_link_external_tool_preserves_timeout_semantics(monkeypatch) -> None:
    def fake_run(_executor, cmd, **_kwargs):  # type: ignore[no-untyped-def]
        raise wasm_link_command.subprocess.TimeoutExpired(
            cmd,
            timeout=1,
            output="partial",
            stderr="memory_guard: timeout after 1.00s\n",
        )

    monkeypatch.setattr(wasm_link_command.CommandExecutor, "run", fake_run)

    with pytest.raises(wasm_link_command.subprocess.TimeoutExpired) as exc_info:
        wasm_link_command._run_external_tool(["wasm-opt", "x.wasm"], timeout=1)

    assert exc_info.value.cmd == ["wasm-opt", "x.wasm"]
    assert exc_info.value.output == "partial"
    assert exc_info.value.stderr == "memory_guard: timeout after 1.00s\n"


def test_wasm_link_default_artifact_paths_use_canonical_dist(monkeypatch) -> None:
    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == Path("dist") / "output.wasm"
    assert wasm_link._default_output_path() == Path("dist") / "output_linked.wasm"


def test_wasm_link_default_artifact_paths_follow_external_root(
    tmp_path: Path,
    monkeypatch,
) -> None:
    ext_root = tmp_path / "ext-root"
    ext_root.mkdir(parents=True, exist_ok=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext_root))
    monkeypatch.delenv("MOLT_WASM_RUNTIME_DIR", raising=False)

    assert wasm_link._default_input_path() == ext_root / "dist" / "output.wasm"
    assert wasm_link._default_output_path() == Path(
        ext_root / "dist" / "output_linked.wasm"
    )


def test_runtime_generation_has_no_hardcoded_hash_authority() -> None:
    root = Path(__file__).resolve().parents[1]
    assert not hasattr(wasm_link, "RUNTIME_EXPECTED_HASHES")
    assert not (root / "tools" / "update_runtime_hash.py").exists()


def _build_minimal_module(element_payload: bytes) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections = []

    # Type section: one empty function type.
    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    # Function section: one function of type 0.
    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    # Table section: one funcref table with min 1.
    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    # Code section: one empty function body.
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    # Element section. The payload is a wasm vector, so it must at least
    # encode its segment count; an empty payload would be an invalid module
    # that the strict module-facts parser rejects at link time.
    sections.append((9, element_payload or write_varuint(0)))

    return wasm_link_operations.build_sections(sections)


def _build_start_root_module() -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(2))
    func_payload.extend(write_varuint(0))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    sections.append((8, write_varuint(0)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)  # local decl count
    code_payload.append(0x10)  # call
    code_payload.extend(write_varuint(1))
    code_payload.append(0x0B)  # end
    code_payload.extend(write_varuint(3))
    code_payload.append(0x00)  # local decl count
    code_payload.append(0x01)  # nop
    code_payload.append(0x0B)  # end
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_exported_runtime_module(export_name: str) -> bytes:
    return _build_exported_runtime_module_many([export_name])


_TEST_WASM_VALUE_TYPES = {
    "i32": 0x7F,
    "i64": 0x7E,
    "f32": 0x7D,
    "f64": 0x7C,
    "v128": 0x7B,
    "funcref": 0x70,
    "externref": 0x6F,
}


def _test_wasm_function_signature(
    name: str,
    *,
    default: tuple[tuple[str, ...], tuple[str, ...]],
) -> tuple[tuple[str, ...], tuple[str, ...]]:
    return wasm_split_runtime_import_signature(name) or default


def _append_test_wasm_function_type(
    payload: bytearray,
    signature: tuple[tuple[str, ...], tuple[str, ...]],
) -> None:
    write_varuint = wasm_link_format._write_varuint
    params, results = signature
    payload.append(0x60)
    payload.extend(write_varuint(len(params)))
    payload.extend(_TEST_WASM_VALUE_TYPES[value] for value in params)
    payload.extend(write_varuint(len(results)))
    payload.extend(_TEST_WASM_VALUE_TYPES[value] for value in results)


def _test_wasm_zero_result_body(results: tuple[str, ...]) -> bytes:
    body = bytearray([0x00])
    for result in results:
        if result == "i32":
            body.extend((0x41, 0x00))
        elif result == "i64":
            body.extend((0x42, 0x00))
        elif result == "f32":
            body.extend((0x43, 0, 0, 0, 0))
        elif result == "f64":
            body.extend((0x44, 0, 0, 0, 0, 0, 0, 0, 0))
        else:
            raise ValueError(f"test helper cannot synthesize {result} result")
    body.append(0x0B)
    return bytes(body)


def _build_exported_runtime_module_many(export_names: list[str]) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(len(export_names)))
    signatures = [
        _test_wasm_function_signature(name, default=((), ("i64",)))
        for name in export_names
    ]
    for signature in signatures:
        _append_test_wasm_function_type(type_payload, signature)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(len(export_names)))
    for type_index in range(len(export_names)):
        func_payload.extend(write_varuint(type_index))
    sections.append((3, func_payload))

    export_payload = bytearray()
    export_payload.extend(write_varuint(len(export_names)))
    for index, export_name in enumerate(export_names):
        export_payload.extend(wasm_link_format._write_string(export_name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(len(export_names)))
    for _params, results in signatures:
        body = _test_wasm_zero_result_body(results)
        code_payload.extend(write_varuint(len(body)))
        code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def test_link_pipeline_requires_frontend_app_export_contract(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    module = _build_exported_runtime_module("molt_main")
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "linked.wasm"
    runtime.write_bytes(module)
    output.write_bytes(module)

    result = wasm_link_pipeline.run_wasm_ld_with_custodied_inputs(
        "wasm-ld",
        runtime,
        output,
        linked,
        runtime_role="shared",
        wasm_facts_scanner=Path("unused-rust-facts-scanner"),
        app_export_contract_path=None,  # type: ignore[arg-type]
    )

    assert result == 1
    assert "frontend app export contract is required" in capsys.readouterr().err


def test_install_callable_table_layout_appends_final_active_override() -> None:
    names = [
        wasm_link_callable_table._callable_entry_export_name(slot) for slot in range(2)
    ]
    sections = wasm_link_operations.parse_sections(
        _build_exported_runtime_module_many(names)
    )
    table_payload = (
        wasm_link_format._write_varuint(1)
        + b"\x70"
        + wasm_link_format._write_varuint(0)
        + wasm_link_format._write_varuint(8)
    )
    sections.insert(2, (4, table_payload))
    sections.insert(-1, (9, wasm_link_format._write_varuint(0)))
    module = wasm_link_operations.build_sections(sections)
    layout = wasm_link_format.CallableTableLayout(3, 2, 8, 0)

    updated = wasm_link_callable_table._install_callable_table_layout(
        module, layout, facts_provider=_facts_provider
    )
    element_payload = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(updated)
        if section_id == 9
    )
    segment_count, offset = wasm_link_format._read_varuint(element_payload, 0)
    flags, offset = wasm_link_format._read_varuint(element_payload, offset)
    assert (segment_count, flags) == (1, 0)
    assert element_payload[offset] == 0x41
    base, offset = wasm_link_format._read_varsint(element_payload, offset + 1)
    assert base == 3
    assert element_payload[offset] == 0x0B
    entry_count, offset = wasm_link_format._read_varuint(element_payload, offset + 1)
    function_indices = []
    for _ in range(entry_count):
        function_index, offset = wasm_link_format._read_varuint(element_payload, offset)
        function_indices.append(function_index)
    assert function_indices == [0, 1]
    assert offset == len(element_payload)


def test_install_callable_table_layout_requires_complete_export_authority() -> None:
    name = wasm_link_callable_table._callable_entry_export_name(0)
    sections = wasm_link_operations.parse_sections(_build_exported_runtime_module(name))
    sections.insert(2, (4, b"\x01\x70\x00\x08"))
    sections.insert(-1, (9, b"\x00"))

    with pytest.raises(ValueError, match="entry export.*entry.1"):
        wasm_link_callable_table._install_callable_table_layout(
            wasm_link_operations.build_sections(sections),
            wasm_link_format.CallableTableLayout(3, 2, 8, 0),
            facts_provider=_facts_provider,
        )


def test_install_callable_table_layout_can_publish_app_region_without_fixed_exports() -> (
    None
):
    name = wasm_link_callable_table._callable_entry_export_name(1)
    sections = wasm_link_operations.parse_sections(_build_exported_runtime_module(name))
    sections.insert(2, (4, b"\x01\x70\x00\x10"))
    sections.insert(-1, (9, b"\x00"))

    updated = wasm_link_callable_table._install_callable_table_layout(
        wasm_link_operations.build_sections(sections),
        wasm_link_format.CallableTableLayout(3, 1, 8, 1),
        include_fixed_prefix=False,
        facts_provider=_facts_provider,
    )
    element_payload = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(updated)
        if section_id == 9
    )
    segment_count, offset = wasm_link_format._read_varuint(element_payload, 0)
    flags, offset = wasm_link_format._read_varuint(element_payload, offset)
    assert (segment_count, flags) == (1, 0)
    base, offset = wasm_link_format._read_varsint(element_payload, offset + 1)
    assert base == 8
    assert element_payload[offset] == 0x0B
    entry_count, offset = wasm_link_format._read_varuint(element_payload, offset + 1)
    function_index, offset = wasm_link_format._read_varuint(element_payload, offset)
    assert (entry_count, function_index, offset) == (1, 0, len(element_payload))


def _build_native_growth_callable_module() -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray(write_varuint(1))
    type_payload.extend(b"\x60\x00\x01\x7e")
    sections.append((1, bytes(type_payload)))

    function_payload = bytearray(write_varuint(4))
    function_payload.extend(write_varuint(0) * 4)
    sections.append((3, bytes(function_payload)))

    table_payload = write_varuint(1) + b"\x70\x00" + write_varuint(12)
    sections.append((4, table_payload))

    exports = [
        (wasm_link_callable_table._callable_entry_export_name(0), 0),
        (wasm_link_callable_table._callable_entry_export_name(1), 1),
        ("invoke_compiler_slot", 3),
    ]
    export_payload = bytearray(write_varuint(len(exports)))
    for name, index in exports:
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(0)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    native_segment = bytearray(write_varuint(1))
    native_segment.extend(b"\x00\x41\x0a\x0b")
    native_segment.extend(write_varuint(1))
    native_segment.extend(write_varuint(2))
    sections.append((9, bytes(native_segment)))

    code_payload = bytearray(write_varuint(4))
    for value in (41, 42, 43):
        body = b"\x00\x42" + bytes((value,)) + b"\x0b"
        code_payload.extend(write_varuint(len(body)))
        code_payload.extend(body)
    invoke_body = b"\x00\x41\x08\x11\x00\x00\x0b"
    code_payload.extend(write_varuint(len(invoke_body)))
    code_payload.extend(invoke_body)
    sections.append((10, bytes(code_payload)))
    return wasm_link_operations.build_sections(sections)


def _simple_active_function_segments(data: bytes) -> list[tuple[int, list[int]]]:
    payload = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(data)
        if section_id == 9
    )
    segment_count, offset = wasm_link_format._read_varuint(payload, 0)
    segments: list[tuple[int, list[int]]] = []
    for _ in range(segment_count):
        flags, offset = wasm_link_format._read_varuint(payload, offset)
        assert flags == 0
        assert payload[offset] == 0x41
        base, offset = wasm_link_format._read_varsint(payload, offset + 1)
        assert payload[offset] == 0x0B
        entry_count, offset = wasm_link_format._read_varuint(payload, offset + 1)
        indices: list[int] = []
        for _ in range(entry_count):
            index, offset = wasm_link_format._read_varuint(payload, offset)
            indices.append(index)
        segments.append((base, indices))
    assert offset == len(payload)
    return segments


def test_native_link_growth_preserves_compiler_slots_and_indirect_callsite() -> None:
    layout = wasm_link_format.CallableTableLayout(0, 0, 8, 2)
    module = _build_native_growth_callable_module()
    entry_plan = wasm_link_callable_table._resolve_callable_table_entry_plan(
        module,
        layout,
        entry_symbol_names=None,
        include_fixed_prefix=False,
        override_reserved_direct=False,
        facts_provider=_facts_provider,
    )
    code_before = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(module)
        if section_id == 10
    )

    assert (
        wasm_link_callable_table._merge_linked_callable_table(
            [[10, 2, 0, 0], [8, 0, 0, 0], [9, 1, 0, 0]],
            layout,
            entry_plan,
        )
        == 11
    )
    published = wasm_link_callable_table._install_callable_table_layout(
        module,
        layout,
        include_fixed_prefix=False,
        entry_plan=entry_plan,
        facts_provider=_facts_provider,
    )

    code_after = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(published)
        if section_id == 10
    )
    assert code_after == code_before
    assert b"\x41\x08\x11\x00\x00" in code_after
    assert _simple_active_function_segments(published) == [
        (10, [2]),
        (8, [0, 1]),
    ]


@pytest.mark.parametrize(
    ("layout", "expected"),
    [
        (wasm_link_format.CallableTableLayout(0, 0, 2848, 3130), 5978),
        (wasm_link_format.CallableTableLayout(1, 81, 2794, 2), 2796),
        (wasm_link_format.CallableTableLayout(0, 0, 2848, 0), None),
    ],
)
def test_monolithic_linker_growth_starts_after_compiler_owned_rows(
    layout: wasm_link_format.CallableTableLayout,
    expected: int | None,
) -> None:
    assert (
        wasm_link_callable_table._monolithic_linked_callable_growth_base(layout)
        == expected
    )


def test_monolithic_callable_merge_preserves_realistic_runtime_gap_and_app() -> None:
    layout = wasm_link_format.CallableTableLayout(1, 81, 2794, 2)
    entry_plan = wasm_link_callable_table._CallableTableEntryPlan(
        tuple(range(1, 82)),
        (5000, 5001),
        owns_runtime_region=True,
    )
    final_rows = [
        *([slot, slot, 0, 0] for slot in range(1, 1850)),
        [2794, 5000, 0, 0],
        [2795, 5001, 0, 0],
        [2796, 6000, 0, 0],
        [2797, 6001, 0, 0],
    ]

    assert (
        wasm_link_callable_table._merge_linked_callable_table(
            list(reversed(final_rows)),
            layout,
            entry_plan,
        )
        == 2798
    )


@pytest.mark.parametrize("runtime_base", [1, 10])
def test_monolithic_empty_prefix_runtime_growth_starts_at_first_occupied_slot(
    runtime_base: int,
) -> None:
    # Slot 1 is wasm-ld's shape after its null-function-pointer hole; slot 10
    # is the existing Rust publication sibling proving that an empty prefix
    # declares no stronger occupancy base.
    layout = wasm_link_format.CallableTableLayout(0, 0, 20, 1)
    entry_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (), (200,), owns_runtime_region=True
    )
    rows = [
        [runtime_base, 100, 0, 0],
        [runtime_base + 1, 101, 0, 0],
        [20, 200, 0, 0],
    ]

    assert (
        wasm_link_callable_table._merge_linked_callable_table(rows, layout, entry_plan)
        == 21
    )


def test_monolithic_callable_merge_accepts_prelink_stub_then_publishes_direct_runtime() -> (
    None
):
    sections = wasm_link_operations.parse_sections(
        _build_exported_runtime_module_many(["compiler_stub", "runtime_direct"])
    )
    sections.insert(2, (4, b"\x01\x70\x00\x08"))
    sections.insert(-1, (9, b"\x00"))
    module = wasm_link_operations.build_sections(sections)
    layout = wasm_link_format.CallableTableLayout(1, 1, 8, 0)
    entry_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (1,),
        (),
        owns_runtime_region=True,
        preserved_fixed_indices=(0,),
    )

    assert (
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 0, 0, 0]], layout, entry_plan
        )
        == 8
    )
    assert (
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 1, 0, 0]], layout, entry_plan
        )
        == 8
    )
    with pytest.raises(ValueError, match="changed compiler-owned.*slot=1"):
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 2, 0, 0]], layout, entry_plan
        )
    published = wasm_link_callable_table._install_callable_table_layout(
        module,
        layout,
        entry_plan=entry_plan,
        facts_provider=_facts_provider,
    )
    assert _simple_active_function_segments(published) == [(1, [1])]


def _empty_callable_publication_module() -> bytes:
    sections = wasm_link_operations.parse_sections(
        _build_exported_runtime_module_many(
            [
                wasm_link_callable_table._callable_entry_export_name(slot)
                for slot in range(2)
            ]
        )
    )
    sections.insert(2, (4, b"\x01\x70\x00\x10"))
    sections.insert(-1, (9, b"\x00"))
    return wasm_link_operations.build_sections(sections)


def test_monolithic_callable_merge_republishes_gc_omitted_fixed_and_app_rows() -> None:
    module = _empty_callable_publication_module()
    layout = wasm_link_format.CallableTableLayout(1, 1, 8, 1)
    entry_plan = wasm_link_callable_table._resolve_callable_table_entry_plan(
        module,
        layout,
        entry_symbol_names=None,
        include_fixed_prefix=True,
        override_reserved_direct=False,
        facts_provider=_facts_provider,
    )

    assert (
        wasm_link_callable_table._merge_linked_callable_table([], layout, entry_plan)
        == 9
    )
    published = wasm_link_callable_table._install_callable_table_layout(
        module,
        layout,
        entry_plan=entry_plan,
        facts_provider=_facts_provider,
    )
    assert _simple_active_function_segments(published) == [(1, [0]), (8, [1])]


def test_split_callable_merge_republishes_gc_omitted_app_row_only() -> None:
    module = _empty_callable_publication_module()
    layout = wasm_link_format.CallableTableLayout(1, 1, 8, 1)
    entry_plan = wasm_link_callable_table._resolve_callable_table_entry_plan(
        module,
        layout,
        entry_symbol_names=None,
        include_fixed_prefix=False,
        override_reserved_direct=False,
        facts_provider=_facts_provider,
    )

    assert (
        wasm_link_callable_table._merge_linked_callable_table([], layout, entry_plan)
        == 9
    )
    published = wasm_link_callable_table._install_callable_table_layout(
        module,
        layout,
        include_fixed_prefix=False,
        override_reserved_direct=False,
        entry_plan=entry_plan,
        facts_provider=_facts_provider,
    )
    assert _simple_active_function_segments(published) == [(8, [1])]


def test_linked_callable_merge_rejects_identity_change_and_unowned_overlap() -> None:
    layout = wasm_link_format.CallableTableLayout(1, 2, 8, 2)
    entry_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (10, 11), (80, 81), owns_runtime_region=True
    )
    owned_rows = [[1, 10, 0, 0], [2, 11, 0, 0], [8, 80, 0, 0], [9, 81, 0, 0]]

    with pytest.raises(ValueError, match="changed compiler-owned.*slot=1"):
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 100, 0, 0], *owned_rows[1:]], layout, entry_plan
        )
    with pytest.raises(ValueError, match="without compiler identity.*slot=0"):
        wasm_link_callable_table._merge_linked_callable_table(
            [*owned_rows, [0, 0, 0, 0]], layout, entry_plan
        )


def test_linked_callable_merge_rejects_sparse_runtime_and_suffix_growth() -> None:
    layout = wasm_link_format.CallableTableLayout(1, 2, 8, 2)
    entry_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (10, 11), (80, 81), owns_runtime_region=True
    )
    owned_rows = [[1, 10, 0, 0], [2, 11, 0, 0], [8, 80, 0, 0], [9, 81, 0, 0]]

    with pytest.raises(
        ValueError, match="suffix callable-table growth is not contiguous"
    ):
        wasm_link_callable_table._merge_linked_callable_table(
            [*owned_rows, [10, 100, 0, 0], [12, 120, 0, 0]],
            layout,
            entry_plan,
        )
    with pytest.raises(
        ValueError, match="runtime callable-table growth is not contiguous"
    ):
        wasm_link_callable_table._merge_linked_callable_table(
            [*owned_rows, [4, 40, 0, 0]], layout, entry_plan
        )
    empty_prefix_layout = wasm_link_format.CallableTableLayout(0, 0, 8, 1)
    empty_prefix_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (), (80,), owns_runtime_region=True
    )
    with pytest.raises(
        ValueError, match="runtime callable-table growth is not contiguous"
    ):
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 10, 0, 0], [3, 30, 0, 0], [8, 80, 0, 0]],
            empty_prefix_layout,
            empty_prefix_plan,
        )


def test_split_callable_merge_owns_only_app_region() -> None:
    layout = wasm_link_format.CallableTableLayout(1, 2, 8, 2)
    app_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (), (80, 81), owns_runtime_region=False
    )

    assert (
        wasm_link_callable_table._merge_linked_callable_table(
            [[8, 80, 0, 0], [9, 81, 0, 0], [10, 100, 0, 0]],
            layout,
            app_plan,
        )
        == 11
    )
    with pytest.raises(ValueError, match="without compiler identity.*slot=1"):
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 10, 0, 0], [8, 80, 0, 0], [9, 81, 0, 0]],
            layout,
            app_plan,
        )
    empty_prefix_layout = wasm_link_format.CallableTableLayout(0, 0, 8, 1)
    empty_prefix_plan = wasm_link_callable_table._CallableTableEntryPlan(
        (), (80,), owns_runtime_region=False
    )
    with pytest.raises(ValueError, match="without compiler identity.*slot=1"):
        wasm_link_callable_table._merge_linked_callable_table(
            [[1, 10, 0, 0], [8, 80, 0, 0]],
            empty_prefix_layout,
            empty_prefix_plan,
        )


def _build_host_call_indirect_module(
    import_name: str = "molt_call_indirect3",
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(2))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(3))
    type_payload.extend(b"\x7e\x7e\x7e")
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string(import_name))
    import_payload.append(0x00)
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(1))
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(1))
    sections.append((9, bytes(element_payload)))
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_tag_then_host_call_indirect_import_module() -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(2))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(3))
    type_payload.extend(b"\x7e\x7e\x7e")
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(2))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("__cpp_exception"))
    import_payload.append(0x04)  # tag import
    import_payload.append(0x00)  # exception attribute
    import_payload.extend(write_varuint(0))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("molt_call_indirect3"))
    import_payload.append(0x00)  # function import
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(1))
    sections.append((3, bytes(func_payload)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(1))
    sections.append((9, bytes(element_payload)))

    return wasm_link_operations.build_sections(sections)


def _function_import_pairs(wasm_bytes: bytes) -> list[tuple[str, str]]:
    return [
        (wasm_import.module, wasm_import.name)
        for wasm_import in parse_wasm_imports(wasm_bytes, on_error="ignore")
        if wasm_import.kind == 0
    ]


def _function_export_pairs(wasm_bytes: bytes) -> list[tuple[str, int]]:
    return [
        (wasm_export.name, wasm_export.index)
        for wasm_export in parse_wasm_exports(wasm_bytes, kind=0, on_error="ignore")
    ]


def _parse_code_section_call_targets(wasm_bytes: bytes) -> list[list[int]]:
    targets: list[list[int]] = []
    offset = 8
    while offset < len(wasm_bytes):
        section_id = wasm_bytes[offset]
        offset += 1
        size, offset = wasm_link_format._read_varuint(wasm_bytes, offset)
        section_end = offset + size
        if section_id == 10:
            func_count, offset = wasm_link_format._read_varuint(wasm_bytes, offset)
            for _ in range(func_count):
                body_size, offset = wasm_link_format._read_varuint(wasm_bytes, offset)
                body_end = offset + body_size
                local_count, pos = wasm_link_format._read_varuint(wasm_bytes, offset)
                for _ in range(local_count):
                    _, pos = wasm_link_format._read_varuint(wasm_bytes, pos)
                    pos += 1
                func_targets: list[int] = []
                while pos < body_end:
                    opcode = wasm_bytes[pos]
                    pos += 1
                    if opcode in (0x10, 0x12):
                        idx, pos = wasm_link_format._read_varuint(wasm_bytes, pos)
                        func_targets.append(idx)
                    elif opcode == 0x0B:
                        break
                    else:
                        raise AssertionError(
                            f"unexpected opcode 0x{opcode:02x} in test helper"
                        )
                targets.append(func_targets)
                offset = body_end
            return targets
        offset = section_end
    return targets


def _build_runtime_import_strip_module() -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(2))
    for name in ("unused_runtime_fn", "live_runtime_fn"):
        import_payload.extend(wasm_link_format._write_string("molt_runtime"))
        import_payload.extend(wasm_link_format._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link_format._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(2))
    sections.append((7, bytes(export_payload)))

    body = bytearray()
    body.append(0x00)
    body.append(0x10)
    body.extend(write_varuint(1))
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_runtime_import_module(
    import_names: list[str], *, memory_min: int | None = None
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(len(import_names)))
    for name in import_names:
        _append_test_wasm_function_type(
            type_payload,
            _test_wasm_function_signature(name, default=(("i64",), ())),
        )
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(
        write_varuint(len(import_names) + (1 if memory_min is not None else 0))
    )
    for type_index, name in enumerate(import_names):
        import_payload.extend(wasm_link_format._write_string("molt_runtime"))
        import_payload.extend(wasm_link_format._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(type_index))
    if memory_min is not None:
        import_payload.extend(wasm_link_format._write_string("env"))
        import_payload.extend(wasm_link_format._write_string("memory"))
        import_payload.append(0x02)
        import_payload.append(0x00)
        import_payload.extend(write_varuint(memory_min))
    sections.append((2, bytes(import_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_split_runtime_app_module(
    import_names: list[str], *, memory_min: int = 1
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(len(import_names) + 1))
    for name in import_names:
        _append_test_wasm_function_type(
            type_payload,
            _test_wasm_function_signature(name, default=(("i64",), ())),
        )
    _append_test_wasm_function_type(type_payload, ((), ()))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(len(import_names) + 2))
    for type_index, name in enumerate(import_names):
        import_payload.extend(wasm_link_format._write_string("molt_runtime"))
        import_payload.extend(wasm_link_format._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(type_index))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("__indirect_function_table"))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(memory_min))
    sections.append((2, bytes(import_payload)))
    sections.append((3, write_varuint(1) + write_varuint(len(import_names))))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, kind, index in (
        ("molt_main", 0x00, len(import_names)),
        ("molt_table", 0x01, 0),
        ("molt_memory", 0x02, 0),
    ):
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(kind)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))
    code_payload = write_varuint(1) + write_varuint(2) + b"\x00\x0b"
    sections.append((10, code_payload))
    return wasm_link_operations.build_sections(sections)


def _strip_export(data: bytes, export_name: str) -> bytes:
    rebuilt_sections: list[tuple[int, bytes]] = []
    for section_id, payload in wasm_link_operations.parse_sections(data):
        if section_id != 7:
            rebuilt_sections.append((section_id, payload))
            continue
        count, offset = wasm_link_format._read_varuint(payload, 0)
        exports: list[tuple[str, int, int]] = []
        for _ in range(count):
            name, offset = wasm_link_format._read_string(payload, offset)
            kind = payload[offset]
            offset += 1
            index, offset = wasm_link_format._read_varuint(payload, offset)
            if name != export_name:
                exports.append((name, kind, index))
        rebuilt = bytearray(wasm_link_format._write_varuint(len(exports)))
        for name, kind, index in exports:
            rebuilt.extend(wasm_link_format._write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(wasm_link_format._write_varuint(index))
        rebuilt_sections.append((7, bytes(rebuilt)))
    return wasm_link_operations.build_sections(rebuilt_sections)


def _build_memory_import_ref_func_app_module(func_index: int = 0) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(1))
    sections.append((2, bytes(import_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    body = bytearray()
    body.extend(write_varuint(0))
    body.append(0xD2)
    body.extend(write_varuint(func_index))
    body.append(0x1A)
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_linked_ref_func_module(func_index: int = 0) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.append(0x00)
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(2))
    export_payload.extend(wasm_link_format._write_string("molt_memory"))
    export_payload.append(0x02)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link_format._write_string("molt_table"))
    export_payload.append(0x01)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    body = bytearray()
    body.extend(write_varuint(0))
    body.append(0xD2)
    body.extend(write_varuint(func_index))
    body.append(0x1A)
    body.append(0x0B)
    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_native_direct_import_module(import_name: str) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7F)
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("molt_native"))
    import_payload.extend(wasm_link_format._write_string(import_name))
    import_payload.append(0x00)
    import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_exported_function_module(
    export_name: str, *, trap_body: bool = False
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7F)
    sections.append((1, bytes(type_payload)))

    sections.append((3, write_varuint(1) + write_varuint(0)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link_format._write_string(export_name))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    body = bytes([0x00, 0x00, 0x0B]) if trap_body else bytes([0x00, 0x41, 0x01, 0x0B])
    code_payload = write_varuint(1) + write_varuint(len(body)) + body
    sections.append((10, code_payload))

    return wasm_link_operations.build_sections(sections)


def _build_runtime_import_data_module(
    import_names: list[str],
    *,
    memory_min: int,
    data_offset: int,
    table_min: int | None = None,
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(
        write_varuint(len(import_names) + 1 + (1 if table_min is not None else 0))
    )
    for name in import_names:
        import_payload.extend(wasm_link_format._write_string("molt_runtime"))
        import_payload.extend(wasm_link_format._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("memory"))
    import_payload.append(0x02)
    import_payload.append(0x00)
    import_payload.extend(write_varuint(memory_min))
    if table_min is not None:
        import_payload.extend(wasm_link_format._write_string("env"))
        import_payload.extend(
            wasm_link_format._write_string("__indirect_function_table")
        )
        import_payload.append(0x01)
        import_payload.append(0x70)
        import_payload.extend(write_varuint(0))
        import_payload.extend(write_varuint(table_min))
    sections.append((2, bytes(import_payload)))

    data_payload = bytearray()
    data_payload.extend(write_varuint(1))
    data_payload.extend(write_varuint(0))
    data_payload.append(0x41)
    data_payload.extend(write_varuint(data_offset))
    data_payload.append(0x0B)
    data_payload.extend(write_varuint(1))
    data_payload.extend(b"x")
    sections.append((11, bytes(data_payload)))

    return wasm_link_operations.build_sections(sections)


def _write_varsint32(value: int) -> bytes:
    out = bytearray()
    current = value
    while True:
        byte = current & 0x7F
        current >>= 7
        sign_bit = byte & 0x40
        done = (current == 0 and not sign_bit) or (current == -1 and sign_bit)
        out.append(byte if done else byte | 0x80)
        if done:
            return bytes(out)


def _build_data_segment_module(
    segments: list[tuple[int, int | None, bytes]],
) -> bytes:
    payload = bytearray(wasm_link_format._write_varuint(len(segments)))
    for flags, data_offset, content in segments:
        payload.extend(wasm_link_format._write_varuint(flags))
        if flags == 1:
            assert data_offset is None
        else:
            assert data_offset is not None
            if flags == 2:
                payload.extend(wasm_link_format._write_varuint(0))
            payload.append(0x41)
            payload.extend(_write_varsint32(data_offset))
            payload.append(0x0B)
        payload.extend(wasm_link_format._write_varuint(len(content)))
        payload.extend(content)
    return wasm_link_operations.build_sections([(11, bytes(payload))])


def test_split_app_global_base_uses_aligned_maximum_active_data_end() -> None:
    output = _build_data_segment_module(
        [
            (0, 0x3000, b"third"),
            (1, None, b"passive-does-not-own-an-address"),
            (2, 0x1000, b"first"),
            (0, 0x2000, b"second"),
        ]
    )

    assert wasm_link_runtime_data._active_data_segment_intervals(output) == (
        (0x1000, 0x1005),
        (0x2000, 0x2006),
        (0x3000, 0x3005),
    )
    assert wasm_link_runtime_data._split_app_global_base(output) == 0x3010


def test_split_app_global_base_preserves_exact_alignment() -> None:
    output = _build_data_segment_module([(0, 0x1000, b"x" * 16)])

    assert wasm_link_runtime_data._split_app_global_base(output) == 0x1010


def test_split_app_global_base_requires_active_data_authority() -> None:
    with pytest.raises(ValueError, match="no active data placement authority"):
        wasm_link_runtime_data._split_app_global_base(_build_data_segment_module([]))


def test_active_data_segment_intervals_reject_overlap() -> None:
    output = _build_data_segment_module(
        [(0, 0x1000, b"x" * 32), (2, 0x1010, b"y" * 32)]
    )

    with pytest.raises(ValueError, match="Active data segments overlap"):
        wasm_link_runtime_data._active_data_segment_intervals(output)


def test_active_data_segment_intervals_reject_wasm32_overflow() -> None:
    output = _build_data_segment_module([(0, -8, b"x" * 16)])

    with pytest.raises(ValueError, match="exceeds the wasm32 address space"):
        wasm_link_runtime_data._active_data_segment_intervals(output)


def test_validate_split_app_data_layout_attests_disjoint_extents() -> None:
    output = _build_data_segment_module(
        [(0, 0x1000, b"output-a"), (2, 0x2000, b"output-b")]
    )
    planned_base = wasm_link_runtime_data._split_app_global_base(output)
    linked = _build_data_segment_module(
        [(0, planned_base + 0x100, b"native-b"), (0, planned_base, b"native-a")]
    )

    assert wasm_link_runtime_data._validate_split_app_data_layout(
        output, linked, planned_base=planned_base
    ) == (
        ((0x1000, 0x1008), (0x2000, 0x2008)),
        (
            (planned_base, planned_base + 8),
            (planned_base + 0x100, planned_base + 0x108),
        ),
    )


def test_validate_split_app_data_layout_rejects_original_overlap() -> None:
    output = _build_data_segment_module([(0, 0x1000, b"output")])
    planned_base = wasm_link_runtime_data._split_app_global_base(output)
    linked = _build_data_segment_module([(0, planned_base - 1, b"native")])

    with pytest.raises(ValueError, match="overlaps output-owned active data"):
        wasm_link_runtime_data._validate_split_app_data_layout(
            output, linked, planned_base=planned_base
        )


def _build_defined_memory_module(min_pages: int) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(min_pages))
    return wasm_link_operations.build_sections([(5, bytes(memory_payload))])


def _defined_memory_min(wasm_bytes: bytes) -> int | None:
    for section_id, payload in wasm_link_operations.parse_sections(wasm_bytes):
        if section_id != 5:
            continue
        offset = 0
        count, offset = wasm_link_format._read_varuint(payload, offset)
        if count == 0:
            return None
        _flags, offset = wasm_link_format._read_varuint(payload, offset)
        minimum, _offset = wasm_link_format._read_varuint(payload, offset)
        return minimum
    return None


def _build_env_function_import_module(import_names: list[str]) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(len(import_names)))
    for name in import_names:
        import_payload.extend(wasm_link_format._write_string("env"))
        import_payload.extend(wasm_link_format._write_string(name))
        import_payload.append(0x00)
        import_payload.extend(write_varuint(0))
    sections.append((2, bytes(import_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_wasm_archive(*members: tuple[str, bytes]) -> bytes:
    archive = bytearray(b"!<arch>\n")
    for name, data in members:
        encoded_name = f"{name}/".encode("ascii")
        if len(encoded_name) > 16:
            raise ValueError("test archive member name exceeds the short-name field")
        archive.extend(encoded_name.ljust(16, b" "))
        archive.extend(b"0".ljust(12, b" "))
        archive.extend(b"0".ljust(6, b" "))
        archive.extend(b"0".ljust(6, b" "))
        archive.extend(b"100644".ljust(8, b" "))
        archive.extend(str(len(data)).encode("ascii").ljust(10, b" "))
        archive.extend(b"`\n")
        archive.extend(data)
        if len(data) & 1:
            archive.extend(b"\n")
    return bytes(archive)


def _build_symbol_subsection(entries: list[bytes]) -> bytes:
    return wasm_link_format._write_varuint(len(entries)) + b"".join(entries)


def _function_symbol_entry(*, flags: int, index: int | None, name: str | None) -> bytes:
    entry = bytearray()
    entry.append(SYMBOL_KIND_FUNCTION)
    entry.extend(wasm_link_format._write_varuint(flags))
    assert index is not None
    entry.extend(wasm_link_format._write_varuint(index))
    if not (flags & wasm_link_format.FLAG_UNDEFINED) or (
        flags & wasm_link_format.FLAG_EXPLICIT_NAME
    ):
        assert name is not None
        entry.extend(wasm_link_format._write_string(name))
    return bytes(entry)


def _data_symbol_entry(
    *,
    flags: int,
    name: str | None,
    segment_index: int = 0,
    offset: int = 0,
    size: int = 0,
) -> bytes:
    entry = bytearray()
    entry.append(1)
    entry.extend(wasm_link_format._write_varuint(flags))
    if flags & (wasm_link_format.FLAG_EXPLICIT_NAME | wasm_link_format.FLAG_UNDEFINED):
        assert name is not None
        entry.extend(wasm_link_format._write_string(name))
    if not (flags & wasm_link_format.FLAG_UNDEFINED):
        entry.extend(wasm_link_format._write_varuint(segment_index))
        entry.extend(wasm_link_format._write_varuint(offset))
        entry.extend(wasm_link_format._write_varuint(size))
    return bytes(entry)


def _module_with_linking_symbols(entries: list[bytes]) -> bytes:
    linking_payload = wasm_link_format._build_linking_payload(
        2,
        [(SYMTAB_SUBSECTION_ID, _build_symbol_subsection(entries))],
    )
    custom = wasm_link_format._build_custom_section("linking", linking_payload)
    return wasm_link_operations.build_sections([(0, custom)])


@pytest.mark.parametrize("archive", [False, True])
def test_native_reader_fixture_preserves_symbol_kinds_and_member_custody(
    tmp_path: Path, archive: bool
) -> None:
    module = _module_with_linking_symbols(
        [
            _function_symbol_entry(
                flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                index=0,
                name="fixture_function",
            ),
            _function_symbol_entry(
                flags=FLAG_BINDING_LOCAL | wasm_link_format.FLAG_EXPLICIT_NAME,
                index=1,
                name="fixture_local",
            ),
            _data_symbol_entry(
                flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                name="fixture_data",
            ),
            _data_symbol_entry(
                flags=wasm_link_format.FLAG_UNDEFINED,
                name="fixture_required",
            ),
            _data_symbol_entry(
                flags=FLAG_BINDING_WEAK | wasm_link_format.FLAG_UNDEFINED,
                name="fixture_optional",
            ),
        ]
    )
    path = tmp_path / ("fixture.a" if archive else "fixture.o")
    path.write_bytes(static_archive_bytes(module) if archive else module)
    read_facts = (
        native_symbol_inspection._native_archive_global_symbol_facts
        if archive
        else native_symbol_inspection._native_object_global_symbol_facts
    )
    facts = read_facts(path, target_triple="wasm32-wasip1")
    assert facts.defined == frozenset({"fixture_function", "fixture_data"})
    assert facts.defined_functions == frozenset({"fixture_function"})
    assert facts.undefined == frozenset({"fixture_required"})
    assert facts.weak_undefined == frozenset({"fixture_optional"})
    assert facts.artifact_digest == hashlib.sha256(path.read_bytes()).hexdigest()
    if archive:
        assert facts.members is not None and len(facts.members) == 1
        assert facts.members[0].identity.sha256 == hashlib.sha256(module).hexdigest()
    else:
        assert facts.members is None


def _linking_data_symbol_names(data: bytes) -> list[tuple[int, str]]:
    """Return ``(flags, name)`` for every ``data`` symbol in the linking symtab."""
    names: list[tuple[int, str]] = []
    for section_id, payload in wasm_link_operations.parse_sections(data):
        if section_id != 0:
            continue
        name, custom_payload = wasm_link_format._parse_custom_section(payload)
        if name != "linking":
            continue
        _version, subsections = wasm_link_format._parse_linking_payload(custom_payload)
        for sub_id, sub_payload in subsections:
            if sub_id != SYMTAB_SUBSECTION_ID:
                continue
            count, offset = wasm_link_format._read_varuint(sub_payload, 0)
            for _ in range(count):
                kind = sub_payload[offset]
                offset += 1
                flags, offset = wasm_link_format._read_varuint(sub_payload, offset)
                if kind == 1:
                    symbol_name, offset = wasm_link_format._read_string(
                        sub_payload, offset
                    )
                    names.append((flags, symbol_name))
                    if not (flags & wasm_link_format.FLAG_UNDEFINED):
                        _seg, offset = wasm_link_format._read_varuint(
                            sub_payload, offset
                        )
                        _off, offset = wasm_link_format._read_varuint(
                            sub_payload, offset
                        )
                        _sz, offset = wasm_link_format._read_varuint(
                            sub_payload, offset
                        )
                elif kind in (0, 2, 4, 5):
                    _idx, _nm, offset = wasm_link_format._parse_indexed_symbol(
                        sub_payload, offset, flags
                    )
                elif kind == 3:
                    _sec, offset = wasm_link_format._read_varuint(sub_payload, offset)
    return names


def _module_with_flattenable_rec_group_type() -> bytes:
    func_type = b"\x60\x00\x00"
    type_payload = bytearray()
    type_payload.extend(wasm_link_format._write_varuint(1))
    type_payload.append(0x4E)
    type_payload.extend(wasm_link_format._write_varuint(1))
    type_payload.extend(func_type)
    return wasm_link_operations.build_sections([(1, bytes(type_payload))])


def test_publication_strip_removes_link_metadata_after_export_rewrite() -> None:
    module = wasm_link_operations.build_sections(
        [
            (7, b"exports-already-canonical"),
            (0, wasm_link_format._build_custom_section("linking", b"symbols")),
            (0, wasm_link_format._build_custom_section("reloc.CODE", b"relocs")),
            (0, wasm_link_format._build_custom_section("name", b"debug names")),
        ]
    )

    stripped = wasm_link_operations.strip_publication_sections(
        module,
        final_artifact=True,
        preserve_debug=False,
    )

    sections = wasm_link_operations.parse_sections(stripped)
    assert sections == [(7, b"exports-already-canonical")]


def test_canonicalize_standard_section_order_moves_element_before_code_data() -> None:
    sections = [
        (1, b"type"),
        (7, b"export"),
        (10, b"code"),
        (11, b"data"),
        (9, b"elem"),
    ]
    module = wasm_link_operations.build_sections(sections)

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    assert [
        section_id for section_id, _ in wasm_link_operations.parse_sections(canonical)
    ] == [
        1,
        7,
        9,
        10,
        11,
    ]


def test_canonicalize_standard_section_order_places_tag_after_memory() -> None:
    module = wasm_link_operations.build_sections(
        [
            (6, b"\x00"),
            (13, b"\x00"),
            (5, b"\x00"),
            (7, b"\x00"),
        ]
    )

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    assert [
        section_id for section_id, _ in wasm_link_operations.parse_sections(canonical)
    ] == [
        5,
        13,
        6,
        7,
    ]


def test_canonicalize_standard_section_order_merges_duplicate_export_sections() -> None:
    first_export = (
        wasm_link_format._write_varuint(1)
        + wasm_link_format._write_string("molt_main")
        + bytes([0])
        + wasm_link_format._write_varuint(0)
    )
    second_export = (
        wasm_link_format._write_varuint(2)
        + wasm_link_format._write_string("molt_main")
        + bytes([0])
        + wasm_link_format._write_varuint(2)
        + wasm_link_format._write_string("PyInit__multiarray_umath")
        + bytes([0])
        + wasm_link_format._write_varuint(1)
    )
    module = wasm_link_operations.build_sections(
        [(1, bytes([1, 0x60, 0, 0])), (7, first_export), (7, second_export)]
    )

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    sections = wasm_link_operations.parse_sections(canonical)
    assert [section_id for section_id, _payload in sections].count(7) == 1
    assert wasm_link_edit._standard_section_order_error(canonical) is None
    assert _fixture_export_kinds(canonical) == {
        "molt_main": (0, 0),
        "PyInit__multiarray_umath": (0, 1),
    }


def test_canonicalize_standard_section_order_rejects_duplicate_start_sections() -> None:
    module = wasm_link_operations.build_sections([(8, bytes([0])), (8, bytes([1]))])

    with pytest.raises(ValueError, match="duplicate singleton standard section id 8"):
        wasm_link_edit._canonicalize_standard_section_order(module)


def _build_linked_host_table_module(table_import_name: str) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string(table_import_name))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(1))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    for name, kind, index in (
        ("molt_main", 0x00, 0),
        ("molt_table", 0x01, 0),
        ("molt_memory", 0x02, 0),
    ):
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(kind)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    return wasm_link_operations.build_sections(sections)


def _parse_data_segments(data: bytes) -> list[bytes]:
    sections = wasm_link_operations.parse_sections(data)
    for section_id, payload in sections:
        if section_id != 11:
            continue
        offset = 0
        seg_count, offset = wasm_link_format._read_varuint(payload, offset)
        out: list[bytes] = []
        parse_offset = offset
        for _ in range(seg_count):
            flags = payload[parse_offset]
            parse_offset += 1
            if flags == 0:
                parse_offset = wasm_link_format._skip_init_expr(payload, parse_offset)
            elif flags == 1:
                pass
            elif flags == 2:
                _, parse_offset = wasm_link_format._read_varuint(payload, parse_offset)
                parse_offset = wasm_link_format._skip_init_expr(payload, parse_offset)
            else:
                raise AssertionError(f"unexpected data segment flags: {flags}")
            data_len, parse_offset = wasm_link_format._read_varuint(
                payload, parse_offset
            )
            out.append(payload[parse_offset : parse_offset + data_len])
            parse_offset += data_len
        return out
    return []


def test_wasm_link_allows_ref_func_element_expr() -> None:
    write_varuint = wasm_link_format._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))  # count
    payload.extend(write_varuint(0x04))  # active, elemtype + exprs
    payload.extend(b"\x41\x00\x0b")  # i32.const 0; end
    payload.append(0x70)  # funcref
    payload.extend(write_varuint(1))
    payload.append(0xD2)  # ref.func
    payload.extend(write_varuint(0))
    payload.append(0x0B)  # end
    _declared, error = wasm_link_format._parse_element_payload(bytes(payload))
    assert error is None


def test_typed_rust_facts_capture_link_validation_surface() -> None:
    data = _build_memory_import_ref_func_app_module(func_index=0)

    facts = _facts_provider(data)

    assert [
        (fact.module, fact.name, fact.kind, fact.index) for fact in facts.imports
    ] == [("env", "memory", 2, 0)]
    assert facts.memory_import_minimum(module="env", name="memory") == 1


def test_python_whole_module_facts_authority_is_deleted() -> None:
    assert not hasattr(wasm_link_format, "WasmModuleFacts")
    assert not hasattr(wasm_link_format, "parse_wasm_module_facts")
    assert not hasattr(wasm_link_operations, "parse_module_facts")


def test_validate_linked_scans_typed_facts_once(
    tmp_path: Path,
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("__indirect_function_table"))
    calls: list[int] = []

    def facts_once(data: bytes):  # type: ignore[no-untyped-def]
        calls.append(len(data))
        return _facts_provider(data)

    assert wasm_link_validation._validate_linked(
        linked,
        facts_provider=facts_once,
    )
    assert calls == [len(linked.read_bytes())]


def test_validate_split_runtime_outputs_scans_each_artifact_once(
    tmp_path: Path,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))
    calls: list[int] = []

    def facts_once(data: bytes):  # type: ignore[no-untyped-def]
        calls.append(len(data))
        return _facts_provider(data)

    assert wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=facts_once,
    )
    assert calls == [len(app.read_bytes()), len(runtime.read_bytes())]


def test_validate_linked_accepts_known_host_table_contract(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("__indirect_function_table"))

    assert wasm_link_validation._validate_linked(
        linked,
        facts_provider=_facts_provider,
    )
    captured = capsys.readouterr()
    assert "host-table contract" in captured.err


def test_validate_linked_rejects_unexpected_table_import_contract(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_linked_host_table_module("mystery_table"))

    assert not wasm_link_validation._validate_linked(
        linked,
        facts_provider=_facts_provider,
    )
    captured = capsys.readouterr()
    assert "unsupported table" in captured.err


def test_validate_linked_rejects_only_manifest_call_indirect_imports(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(_build_host_call_indirect_module("molt_call_indirect3"))

    assert not wasm_link_validation._validate_linked(
        linked,
        facts_provider=_facts_provider,
    )
    captured = capsys.readouterr()
    assert "molt_call_indirect3" in captured.err

    linked.write_bytes(_build_host_call_indirect_module("molt_call_indirect99"))

    assert not wasm_link_validation._validate_linked(
        linked,
        facts_provider=_facts_provider,
    )
    captured = capsys.readouterr()
    assert "molt_call_indirect99" not in captured.err
    assert "missing exported memory" in captured.err


def test_validate_wasm_structural_fails_closed_on_scanner_error(
    capsys: pytest.CaptureFixture[str],
) -> None:
    module = _build_runtime_import_module([], memory_min=1)

    def failing_provider(_data: bytes) -> dict[str, object]:
        raise ValueError("scanner rejected module")

    assert not wasm_link_validation._validate_wasm_structural(
        module,
        description="Probe wasm",
        facts_provider=failing_provider,  # type: ignore[arg-type]
    )
    assert "Probe wasm failed structural validation: scanner rejected module" in (
        capsys.readouterr().err
    )


def test_stub_dead_functions_preserves_start_root_reachability() -> None:
    module = _build_start_root_module()
    assert (
        wasm_link_optimize._stub_dead_functions(module, _rust_facts_fixture(module))
        is None
    )


def test_tree_shake_runtime_preserves_required_function_exports() -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    shaken = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
    )
    exports = wasm_link_format._collect_function_exports(shaken)
    assert "molt_exception_pending" in exports


def test_tree_shake_runtime_preserves_direct_runner_exception_debug_exports() -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_alloc",
            "molt_handle_resolve",
            "molt_header_size",
            "molt_scratch_alloc",
            "molt_scratch_free",
            "molt_bytes_from_bytes",
            "molt_string_from_bytes",
            "molt_string_as_ptr",
            "molt_exception_kind",
            "molt_exception_message",
            "molt_exception_last",
            "molt_traceback_format_exc",
            "molt_type_tag_of_bits",
            "molt_len",
            "molt_index",
            "molt_profile_dump",
            "molt_dec_ref_obj",
        ]
    )
    shaken = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
    )
    exports = wasm_link_format._collect_function_exports(shaken)
    assert "molt_alloc" in exports
    assert "molt_handle_resolve" in exports
    assert "molt_header_size" in exports
    assert "molt_scratch_alloc" in exports
    assert "molt_scratch_free" in exports
    assert "molt_bytes_from_bytes" in exports
    assert "molt_string_from_bytes" in exports
    assert "molt_string_as_ptr" in exports
    assert "molt_exception_kind" in exports
    assert "molt_exception_message" in exports
    assert "molt_exception_last" in exports
    assert "molt_traceback_format_exc" in exports
    assert "molt_type_tag_of_bits" in exports
    assert "molt_len" in exports
    assert "molt_index" in exports
    assert "molt_profile_dump" in exports
    assert "molt_dec_ref_obj" in exports


def test_validate_split_runtime_outputs_rejects_stripped_contract_memory_export(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(
        _strip_export(
            _build_split_runtime_app_module(["molt_err_pending"]),
            "molt_memory",
        )
    )

    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )
    assert "missing contract export molt_memory (kind 2)" in capsys.readouterr().err


def test_restore_split_runtime_contract_exports_reemits_memory_and_table() -> None:
    app = _build_split_runtime_app_module([])
    stripped = _strip_export(_strip_export(app, "molt_memory"), "molt_table")

    restored = wasm_link_export_contract._restore_split_runtime_contract_exports(
        stripped,
        artifact="app",
        facts_provider=_facts_provider,
    )

    assert _fixture_export_kinds(restored) == {
        "molt_main": (0, 0),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_restore_split_runtime_contract_exports_scans_contract_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    app = _build_split_runtime_app_module([])
    stripped = _strip_export(_strip_export(app, "molt_memory"), "molt_table")
    parse_calls = 0

    def scan(_executor, command, **_kwargs):  # type: ignore[no-untyped-def]
        nonlocal parse_calls
        parse_calls += 1
        data = Path(command[-1]).read_bytes()
        return subprocess.CompletedProcess(
            command,
            0,
            json.dumps(
                {"schema_version": 7, "ok": True, "facts": _rust_facts_fixture(data)}
            ),
            "",
        )

    scanner = tmp_path / "scanner"
    scanner.write_bytes(b"scanner")
    monkeypatch.setattr(wasm_link_fact_provider.CommandExecutor, "run", scan)
    provider = _REAL_MAKE_RUST_WASM_FACTS_PROVIDER(scanner, tmp_path)

    restored = wasm_link_export_contract._restore_split_runtime_contract_exports(
        stripped,
        artifact="app",
        facts_provider=provider,
    )

    assert parse_calls == 1
    assert _fixture_export_kinds(restored) == {
        "molt_main": (0, 0),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_split_runtime_contract_keep_set_includes_all_external_kinds() -> None:
    assert wasm_link_export_contract._split_runtime_contract_export_names("app") == {
        "__indirect_function_table",
        "memory",
        "molt_main",
        "molt_memory",
        "molt_table",
    }

    assert wasm_link_export_contract._split_artifact_contract_keep_set(
        "app",
        public_export_map={"user_export": "internal_user_export"},
        required_native_direct_symbols=("PyInit__demo",),
    ) == {
        "__indirect_function_table",
        "memory",
        "molt_main",
        "molt_memory",
        "molt_table",
        "PyInit__demo",
        "user_export",
    }


def test_split_app_post_link_preserves_and_restores_contract_exports() -> None:
    app = _build_split_runtime_app_module([])
    molt_main_index = wasm_link_format._collect_function_exports(app)["molt_main"]
    app = wasm_link_format._append_linking_function_symbols(
        app,
        [
            (
                "molt_main",
                molt_main_index,
                FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert app is not None
    optimized = wasm_link_optimize._post_link_optimize(
        app,
        reference_data=_build_exported_runtime_module_many(["reference_only"]),
        preserve_exports=wasm_link_export_contract._split_runtime_contract_export_names(
            "app"
        ),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    assert _fixture_export_kinds(optimized) == {
        "molt_main": (0, molt_main_index),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }

    masked = _strip_export(
        _strip_export(_strip_export(app, "molt_main"), "molt_memory"),
        "molt_table",
    )
    restored = wasm_link_export_contract._restore_split_runtime_contract_exports(
        masked,
        artifact="app",
        stage="test-post-link-mask",
        facts_provider=_facts_provider,
    )

    assert _fixture_export_kinds(restored) == {
        "molt_main": (0, molt_main_index),
        "molt_memory": (2, 0),
        "molt_table": (1, 0),
    }


def test_split_app_optimization_cache_eliminates_repeat_wasm_opt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    app = _build_split_runtime_app_module([])
    optimize_calls = 0
    cache_root = tmp_path / "cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "session-a"))
    selected_optimizer = {"path": str((tmp_path / "binaryen-a" / "wasm-opt").resolve())}
    binaryen_version = "wasm-opt version 130 (version_130)"
    cold_identity = _wasm_optimizer_identity(
        selected_optimizer["path"], version=binaryen_version
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )

    def fake_optimize(path: Path, **kwargs) -> bool:  # type: ignore[no-untyped-def]
        nonlocal optimize_calls
        optimize_calls += 1
        kwargs["attestation"].update(
            ok=True,
            status="success",
            binaryen_version=binaryen_version,
            pipeline=list(wasm_link_optimizer_policy.wasm_link_policy("Oz").pipeline),
            wasm_opt_sha256="a" * 64,
        )
        kwargs["telemetry"].update(
            wasm_opt_path=selected_optimizer["path"],
            wasm_opt_wall_ms=123.0,
            wasm_opt_peak_rss_kb=456,
        )
        return True

    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_run_wasm_opt_via_optimize", fake_optimize
    )
    cold_counts: dict[str, int] = {}
    warm_counts: dict[str, int] = {}
    cold_attestation: dict[str, object] = {}
    warm_attestation: dict[str, object] = {"stale": True}
    cold_telemetry: dict[str, object] = {}
    warm_telemetry: dict[str, object] = {}

    cold = wasm_link_optimizer_policy._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        attestation=cold_attestation,
        telemetry=cold_telemetry,
        operation_counts=cold_counts,
        facts_provider=_facts_provider,
        optimizer_identity=cold_identity,
    )
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "session-b"))
    selected_optimizer["path"] = str(
        (tmp_path / "relocated-binaryen" / "wasm-opt").resolve()
    )
    warm_identity = _wasm_optimizer_identity(
        selected_optimizer["path"], version=binaryen_version
    )
    warm = wasm_link_optimizer_policy._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        attestation=warm_attestation,
        telemetry=warm_telemetry,
        operation_counts=warm_counts,
        facts_provider=_facts_provider,
        optimizer_identity=warm_identity,
    )

    assert warm == cold
    assert optimize_calls == 1
    assert cold_counts["split_app_optimize_requests"] == 1
    assert cold_counts["split_app_optimize_cache_misses"] == 1
    assert cold_counts["split_app_wasm_opt_runs"] == 1
    assert cold_counts["split_app_optimize_cache_bytes_written"] == len(cold)
    assert warm_counts["split_app_optimize_requests"] == 1
    assert warm_counts["split_app_optimize_cache_hits"] == 1
    assert warm_counts["split_app_optimize_cache_bytes_read"] == len(warm)
    assert warm_attestation["pipeline"] == list(
        wasm_link_optimizer_policy.wasm_link_policy("Oz").pipeline
    )
    assert warm_attestation["binaryen_version"] == binaryen_version
    assert warm_attestation["wasm_opt_sha256"] == "a" * 64
    assert warm_attestation == cold_attestation
    assert "wasm_opt_path" not in warm_attestation
    assert "cache_hit" not in warm_attestation
    assert warm_telemetry == {
        "cache_hit": True,
        "wasm_opt_path": selected_optimizer["path"],
    }
    assert cold_telemetry["cache_hit"] is False
    assert cold_telemetry["wasm_opt_wall_ms"] == 123.0
    assert next((cache_root / "wasm_link").rglob("artifact.wasm")).read_bytes() == cold
    assert not (tmp_path / "session-a" / ".molt_state" / "wasm_link_cache").exists()
    assert not (tmp_path / "session-b" / ".molt_state" / "wasm_link_cache").exists()


def test_split_app_optimizer_failure_is_fail_closed_and_not_cached(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    app = _build_split_runtime_app_module([])
    cache_root = tmp_path / "cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    binaryen_version = "wasm-opt version 130 (version_130)"
    optimizer_identity = _wasm_optimizer_identity("wasm-opt", version=binaryen_version)
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )

    def fail(*_args, **kwargs):  # type: ignore[no-untyped-def]
        kwargs["telemetry"].update(error="wasm-opt timed out after 300s")
        return False

    monkeypatch.setattr(wasm_link_optimizer_policy, "_run_wasm_opt_via_optimize", fail)

    with pytest.raises(RuntimeError, match="required split-app wasm optimization"):
        wasm_link_optimizer_policy._optimize_split_app_module(
            app,
            reference_data=None,
            optimize=True,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_provider=_facts_provider,
            optimizer_identity=optimizer_identity,
        )

    assert not list((cache_root / "wasm_link").rglob("artifact.wasm"))


@pytest.mark.parametrize(
    "corruption",
    ("artifact", "optimizer-output-digest", "published-output-digest"),
)
def test_split_app_optimization_cache_rejects_corrupt_provenance_or_artifact(
    corruption: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    app = _build_split_runtime_app_module([])
    calls = 0
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    binaryen_version = "wasm-opt version 130 (version_130)"
    optimizer_identity = _wasm_optimizer_identity("wasm-opt", version=binaryen_version)
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )

    def fake_optimize(path: Path, **kwargs) -> bool:  # type: ignore[no-untyped-def]
        nonlocal calls
        calls += 1
        kwargs["attestation"].update(
            ok=True,
            status="success",
            binaryen_version=binaryen_version,
            pipeline=list(wasm_link_optimizer_policy.wasm_link_policy("Oz").pipeline),
            wasm_opt_sha256="a" * 64,
        )
        return True

    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_run_wasm_opt_via_optimize", fake_optimize
    )
    first = wasm_link_optimizer_policy._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        facts_provider=_facts_provider,
        optimizer_identity=optimizer_identity,
    )
    artifact = next((tmp_path / "cache" / "wasm_link").rglob("artifact.wasm"))
    if corruption == "artifact":
        artifact.write_bytes(first + b"corrupt")
    else:
        metadata_path = artifact.with_name("metadata.json")
        metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
        field = (
            "optimizer_output_sha256"
            if corruption == "optimizer-output-digest"
            else "published_output_sha256"
        )
        metadata["payload"][field] = "0" * 64
        metadata_path.write_text(
            json.dumps(metadata, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    counts: dict[str, int | float] = {}
    second = wasm_link_optimizer_policy._optimize_split_app_module(
        app,
        reference_data=None,
        optimize=True,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        operation_counts=counts,
        facts_provider=_facts_provider,
        optimizer_identity=optimizer_identity,
    )

    assert second == first
    assert calls == 2
    assert counts["split_app_optimize_cache_corruptions"] == 1
    assert counts["split_app_optimize_cache_misses"] == 1


def test_split_app_optimization_cache_serializes_concurrent_producers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    app = _build_split_runtime_app_module([])
    calls = 0
    calls_lock = threading.Lock()
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    binaryen_version = "wasm-opt version 130 (version_130)"
    optimizer_identity = _wasm_optimizer_identity("wasm-opt", version=binaryen_version)
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )

    def fake_optimize(path: Path, **kwargs) -> bool:  # type: ignore[no-untyped-def]
        nonlocal calls
        with calls_lock:
            calls += 1
        kwargs["attestation"].update(
            ok=True,
            status="success",
            binaryen_version=binaryen_version,
            pipeline=list(wasm_link_optimizer_policy.wasm_link_policy("Oz").pipeline),
            wasm_opt_sha256="a" * 64,
        )
        time.sleep(0.05)
        return True

    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_run_wasm_opt_via_optimize", fake_optimize
    )

    def optimize() -> bytes:
        return wasm_link_optimizer_policy._optimize_split_app_module(
            app,
            reference_data=None,
            optimize=True,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_provider=_facts_provider,
            optimizer_identity=optimizer_identity,
        )

    with ThreadPoolExecutor(max_workers=4) as executor:
        outputs = list(executor.map(lambda _index: optimize(), range(4)))

    assert outputs == [outputs[0]] * 4
    assert calls == 1


def test_python_callable_table_consumer_rejects_dynamic_active_offset() -> None:
    write_varuint = wasm_link_format._write_varuint
    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x23\x00\x0b")  # global.get 0; end
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    module = _build_minimal_module(bytes(element_payload))

    with pytest.raises(ValueError, match="Dynamic active wasm element offsets"):
        wasm_artifact._collect_wasm_active_table_function_slots(module)


def test_split_contract_restoration_keeps_function_exports_when_adding_memory_and_table() -> (
    None
):
    required_native = ("PyInit__demo",)
    app = _build_split_runtime_app_module([])
    main_index = wasm_link_format._collect_function_exports(app)["molt_main"]
    app = wasm_link_export_contract._ensure_export_by_index(
        app, name=required_native[0], kind=0, index=main_index
    )
    assert app is not None
    app = _strip_export(_strip_export(app, "molt_memory"), "molt_table")

    restored = wasm_link_export_contract._restore_split_runtime_contract_exports(
        app,
        artifact="app",
        stage="mask-proof",
        required_native_direct_symbols=required_native,
        facts_provider=_facts_provider,
    )

    export_kinds = _fixture_export_kinds(restored)
    assert export_kinds[required_native[0]][0] == 0
    assert export_kinds["molt_main"][0] == 0
    assert export_kinds["molt_memory"][0] == 2
    assert export_kinds["molt_table"][0] == 1


def test_split_combined_post_link_preserves_linker_memory_and_table_aliases() -> None:
    linked = wasm_link_edit._rename_export_names(
        _build_linked_ref_func_module(),
        {
            "molt_memory": "memory",
            "molt_table": "__indirect_function_table",
        },
    )
    assert linked is not None

    optimized = wasm_link_optimize._post_link_optimize(
        linked,
        preserve_exports=wasm_link_export_contract._split_runtime_contract_export_names(
            "app"
        ),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    assert _fixture_export_kinds(optimized) == {
        "memory": (2, 0),
        "__indirect_function_table": (1, 0),
    }


def test_split_combined_post_link_restores_real_defined_memory_export() -> None:
    linked = _strip_export(_build_linked_ref_func_module(), "molt_memory")
    optimized = wasm_link_optimize._post_link_optimize(
        linked,
        preserve_exports=wasm_link_export_contract._split_runtime_contract_export_names(
            "app"
        ),
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )

    optimized_facts = _facts_provider(optimized)
    restored = wasm_link_export_contract._ensure_defined_memory_export(
        optimized,
        facts=optimized_facts,
    )

    assert restored is not None
    facts = _facts_provider(restored)
    assert not [entry for entry in facts.imports if entry.kind == 2]
    assert facts.exports["molt_memory"].kind == 2
    assert facts.exports["molt_memory"].index == 0


def test_defined_memory_export_restoration_rejects_imported_memory() -> None:
    app = _strip_export(_build_split_runtime_app_module([]), "molt_memory")

    with pytest.raises(
        ValueError, match="cannot restore linked memory export from an imported memory"
    ):
        wasm_link_export_contract._ensure_defined_memory_export(
            app,
            facts=_facts_provider(app),
        )


def test_split_app_shared_memory_contract_passes_split_validation(
    tmp_path: Path,
) -> None:
    app = tmp_path / "app.wasm"
    runtime = tmp_path / "molt_runtime.wasm"
    app.write_bytes(_build_split_runtime_app_module([]))
    runtime.write_bytes(_build_exported_runtime_module_many([]))

    assert wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )


def test_validate_split_runtime_outputs_requires_shared_app_memory(
    tmp_path: Path,
    capsys,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))

    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))
    assert wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )

    app.write_bytes(_build_defined_memory_module(1))
    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )
    captured = capsys.readouterr()
    assert "Split-runtime app must import env.memory" in captured.err


def test_validate_split_runtime_outputs_rejects_same_name_wrong_signature(
    tmp_path: Path,
    capsys: pytest.CaptureFixture[str],
) -> None:
    app = tmp_path / "app.wasm"
    runtime = tmp_path / "runtime.wasm"
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"]))
    sections = wasm_link_operations.parse_sections(
        _build_exported_runtime_module("unknown_probe")
    )
    export_payload = (
        wasm_link_format._write_varuint(1)
        + wasm_link_format._write_string("molt_err_pending")
        + b"\x00"
        + wasm_link_format._write_varuint(0)
    )
    runtime.write_bytes(
        wasm_link_operations.build_sections(
            [
                (section_id, export_payload if section_id == 7 else payload)
                for section_id, payload in sections
            ]
        )
    )

    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )
    assert "split-runtime ABI type mismatch" in capsys.readouterr().err


def test_validate_split_runtime_outputs_rejects_structurally_invalid_app(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))

    seen: list[str] = []

    def validate(data: bytes, *, description: str, facts_provider):  # type: ignore[no-untyped-def]
        seen.append(description)
        return None if description == "Split-runtime app" else facts_provider(data)

    monkeypatch.setattr(wasm_link_validation, "_validate_wasm_structural", validate)

    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )
    assert seen == ["Split-runtime app"]


def test_validate_split_runtime_outputs_rejects_structurally_invalid_runtime(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    app = tmp_path / "app.wasm"
    runtime.write_bytes(_build_exported_runtime_module_many(["molt_err_pending"]))
    app.write_bytes(_build_split_runtime_app_module(["molt_err_pending"], memory_min=1))

    seen: list[str] = []

    def validate(data: bytes, *, description: str, facts_provider):  # type: ignore[no-untyped-def]
        seen.append(description)
        return (
            None
            if description == "Split-runtime shared runtime"
            else facts_provider(data)
        )

    monkeypatch.setattr(wasm_link_validation, "_validate_wasm_structural", validate)

    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )
    assert seen == ["Split-runtime app", "Split-runtime shared runtime"]


def test_tree_shake_runtime_preserves_dynamic_required_exports(monkeypatch) -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_gpu_linear_contiguous",
            "molt_gpu_tensor__tensor_scaled_dot_product_attention",
            "molt_gpu_turboquant_attention_packed",
        ]
    )
    monkeypatch.setenv(
        "MOLT_WASM_DYNAMIC_REQUIRED_EXPORTS",
        "molt_gpu_linear_contiguous,molt_gpu_tensor__tensor_scaled_dot_product_attention,molt_gpu_turboquant_attention_packed",
    )
    shaken = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
    )
    exports = wasm_link_format._collect_function_exports(shaken)
    assert "molt_gpu_linear_contiguous" in exports
    assert "molt_gpu_tensor__tensor_scaled_dot_product_attention" in exports
    assert "molt_gpu_turboquant_attention_packed" in exports


def test_tree_shake_runtime_reuses_cached_result(
    tmp_path: Path,
    monkeypatch,
) -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    target_root = tmp_path / "target"
    final_runtime = b"\x00asm\x01\x00\x00\x00tree-shaken-runtime"
    calls = {"count": 0}
    cache_root = tmp_path / "cache"

    def fake_structural_optimize(_data: bytes, **_kwargs) -> bytes:
        calls["count"] += 1
        return final_runtime

    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root / "session-a"))
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", fake_structural_optimize
    )

    cold_counts: dict[str, int | float] = {}
    first = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
        operation_counts=cold_counts,
    )

    assert first == final_runtime
    assert calls["count"] == 1

    monkeypatch.setattr(
        wasm_link_optimize,
        "_post_link_optimize",
        lambda *args, **kwargs: (_ for _ in ()).throw(
            AssertionError("structural optimizer should not rerun on a cache hit")
        ),
    )
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target_root / "session-b"))

    warm_counts: dict[str, int | float] = {}
    second = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
        operation_counts=warm_counts,
    )

    assert second == final_runtime
    assert cold_counts["runtime_tree_shake_cache_misses"] == 1
    assert cold_counts["runtime_tree_shake_cache_bytes_written"] == len(first)
    assert warm_counts["runtime_tree_shake_cache_hits"] == 1
    assert warm_counts["runtime_tree_shake_cache_bytes_read"] == len(second)
    assert next((cache_root / "wasm_link").rglob("artifact.wasm")).read_bytes() == first
    assert not (target_root / "session-a" / ".molt_state" / "wasm_link_cache").exists()
    assert not (target_root / "session-b" / ".molt_state" / "wasm_link_cache").exists()


def test_tree_shake_runtime_single_flights_concurrent_producers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    calls = 0
    calls_lock = threading.Lock()
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))

    def fake_structural_optimize(data: bytes, **_kwargs: object) -> bytes:
        nonlocal calls
        with calls_lock:
            calls += 1
        time.sleep(0.05)
        return data

    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_post_link_optimize",
        fake_structural_optimize,
    )

    def transform(_index: int) -> bytes:
        return wasm_link_optimizer_policy._tree_shake_runtime(
            module,
            {"exception_pending"},
            facts_provider=_facts_provider,
        )

    with ThreadPoolExecutor(max_workers=4) as executor:
        outputs = list(executor.map(transform, range(4)))

    assert outputs == [outputs[0]] * 4
    assert calls == 1


def test_tree_shake_runtime_does_not_invoke_binaryen(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    module = _build_exported_runtime_module("molt_exception_pending")
    cache_root = tmp_path / "cache"

    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_run_wasm_opt_via_optimize",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("shared-runtime link cleanup must not invoke Binaryen")
        ),
    )

    counts: dict[str, int | float] = {}
    result = wasm_link_optimizer_policy._tree_shake_runtime(
        module,
        {"exception_pending"},
        facts_provider=_facts_provider,
        operation_counts=counts,
    )

    assert result.startswith(b"\x00asm")
    assert counts["runtime_tree_shake_cache_bytes_written"] == len(result)


def test_wasm_link_cache_root_is_canonical_molt_cache(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    cache_root = tmp_path / "shared-cache"
    monkeypatch.setenv("MOLT_CACHE", str(cache_root))
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "legacy-state"))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(tmp_path / "session-target"))

    assert (
        wasm_link_optimizer_policy._wasm_link_cache_root() == cache_root / "wasm_link"
    )


@pytest.mark.parametrize("extension", ("unknown-root-key", "duplicate-root-key"))
def test_wasm_link_cache_metadata_rejects_json_root_extensions(
    extension: str,
    tmp_path: Path,
) -> None:
    entry = wasm_link_cache._wasm_link_cache_entry(
        "runtime_tree_shake",
        "test-v1",
        "a" * 64,
        cache_root=tmp_path,
    )
    wasm_link_cache._publish_wasm_link_cache_entry(
        entry,
        b"\x00asm\x01\x00\x00\x00",
    )
    metadata = json.loads(entry.metadata.read_text(encoding="utf-8"))
    assert set(metadata) == {"schema", "cache", "payload"}
    assert metadata["schema"] == wasm_link_cache.WASM_LINK_CACHE_ENTRY_SCHEMA
    if extension == "unknown-root-key":
        metadata["unknown"] = True
        entry.metadata.write_text(json.dumps(metadata), encoding="utf-8")
    else:
        encoded = entry.metadata.read_text(encoding="utf-8")
        entry.metadata.write_text(
            encoded.replace("{", '{"schema":"duplicate",', 1),
            encoding="utf-8",
        )

    assert wasm_link_cache._read_wasm_link_cache_entry(entry).status == "corrupt"


def test_each_wasm_link_cache_source_authority_invalidates_both_cache_keys(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repository_root = Path(__file__).resolve().parents[1]
    authority_root = tmp_path / "authority"
    authority_relatives = (
        "src/molt/cli/wasm_link_cache.py",
        "src/molt/toolchain_identity.py",
        "src/molt/wasm_optimizer_identity.py",
        "tools/wasm_link_optimizer_policy.py",
    )
    for relative_text in authority_relatives:
        source = repository_root.joinpath(*Path(relative_text).parts)
        destination = authority_root.joinpath(*Path(relative_text).parts)
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(source.read_bytes())
    (authority_root / "tools" / "wasm_link.py").write_text(
        "from molt.cli import wasm_link_cache\n"
        "from molt import toolchain_identity, wasm_optimizer_identity\n"
        "import wasm_link_optimizer_policy\n",
        encoding="utf-8",
    )

    authority_digest = wasm_link_optimizer_policy._wasm_link_cache_authority_digest
    baseline_authority = authority_digest(repo_root=authority_root)

    def keys(authority: str) -> tuple[str | None, str]:
        monkeypatch.setattr(
            wasm_link_optimizer_policy,
            "_wasm_link_cache_authority_digest",
            lambda: authority,
        )
        split = wasm_link_optimizer_policy._split_app_optimize_cache_key(
            app_data=b"app",
            reference_data=b"reference",
            optimize=False,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_authority_digest="facts",
        )
        tree = wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
            runtime_data=b"runtime",
            normalized_required_exports={"molt_main"},
            facts_authority_digest="facts",
        )
        return split, tree

    baseline_keys = keys(baseline_authority)
    assert baseline_keys[0] is not None
    for relative_text in authority_relatives:
        authority = authority_root.joinpath(*Path(relative_text).parts)
        original = authority.read_bytes()
        authority.write_bytes(original + b"\n# cache-authority-invalidation-probe\n")
        changed_authority = authority_digest(repo_root=authority_root)
        assert changed_authority != baseline_authority, relative_text
        assert keys(changed_authority) != baseline_keys, relative_text
        authority.write_bytes(original)
        assert authority_digest(repo_root=authority_root) == baseline_authority, (
            relative_text
        )


def test_scanner_authority_invalidates_both_cache_keys_without_scanning(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_wasm_link_cache_authority_digest",
        lambda: "transform-authority",
    )

    def keys(provider: object) -> tuple[str | None, str]:
        facts_digest = wasm_link_optimizer_policy._wasm_facts_cache_authority_digest(
            provider,
        )
        return (
            wasm_link_optimizer_policy._split_app_optimize_cache_key(
                app_data=b"app",
                reference_data=None,
                optimize=False,
                optimize_level="O1",
                contract_keep_set={"molt_main"},
                facts_authority_digest=facts_digest,
            ),
            wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
                runtime_data=b"runtime",
                normalized_required_exports={"molt_main"},
                facts_authority_digest=facts_digest,
            ),
        )

    class FactsProvider:
        def __init__(self, authority_digest: str) -> None:
            self.authority_digest = authority_digest

        def __call__(self, _data: bytes):  # type: ignore[no-untyped-def]
            raise AssertionError("cache identity must not rescan unchanged inputs")

    facts_a = FactsProvider("a" * 64)
    facts_b = FactsProvider("a" * 64)
    assert keys(facts_a) == keys(facts_b)
    facts_b.authority_digest = "b" * 64
    assert keys(facts_a) != keys(facts_b)


def test_debug_preservation_policy_invalidates_both_optimizer_cache_keys(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_wasm_link_cache_authority_digest",
        lambda: "a" * 64,
    )
    common = {
        "facts_authority_digest": "b" * 64,
        "preserve_debug": False,
    }
    split_without_debug = wasm_link_optimizer_policy._split_app_optimize_cache_key(
        app_data=b"app",
        reference_data=None,
        optimize=False,
        optimize_level="Oz",
        contract_keep_set={"molt_main"},
        **common,
    )
    runtime_without_debug = wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
        runtime_data=b"runtime",
        normalized_required_exports={"molt_main"},
        **common,
    )
    common["preserve_debug"] = True

    assert split_without_debug != (
        wasm_link_optimizer_policy._split_app_optimize_cache_key(
            app_data=b"app",
            reference_data=None,
            optimize=False,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            **common,
        )
    )
    assert runtime_without_debug != (
        wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
            runtime_data=b"runtime",
            normalized_required_exports={"molt_main"},
            **common,
        )
    )


def test_wasm_optimizer_cache_fact_and_outer_admission_bind_binaryen_version(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import wasm_optimizer_identity as identity_authority
    from molt.cli.non_native_output import _load_optimizer_publication

    optimizer_identity = _wasm_optimizer_identity(
        (tmp_path / "wasm-opt").resolve(),
        version="wasm-opt version 130 (version_130)",
    )
    resolutions = []

    def resolve_cache_identity(selected, *, verify_managed_installation=True):
        resolutions.append((selected, verify_managed_installation))
        return optimizer_identity

    monkeypatch.setattr(identity_authority, "find_wasm_opt", lambda: "wasm-opt")
    monkeypatch.setattr(
        identity_authority, "wasm_optimizer_executable_identity", resolve_cache_identity
    )
    cache_fact = identity_authority.wasm_optimizer_cache_fact()
    assert set(cache_fact) == {"tool", "sha256", "binaryen_version"}
    assert resolutions == [("wasm-opt", False)]
    artifact = tmp_path / "linked.wasm"
    artifact.write_bytes(b"published")
    sidecar = tmp_path / "optimizer.json"
    policy = wasm_link_optimizer_policy.wasm_link_policy("Oz")
    attestation = identity_authority.build_wasm_optimizer_attestation(
        {
            "ok": True,
            "wasm_opt_sha256": cache_fact["sha256"],
            "binaryen_version": cache_fact["binaryen_version"],
            "optimization_level": policy.level,
            "optimization_converge": policy.converge,
            "optimization_apply_level": policy.apply_level,
            "optimization_preserve_debug": False,
            "optimization_extra_passes": list(policy.extra_passes),
            "pipeline": list(policy.pipeline),
            "optimizer_input_sha256": "1" * 64,
            "optimizer_output_sha256": "2" * 64,
        },
        published_output=b"published",
    )
    sidecar.write_text(
        identity_authority.encode_wasm_optimizer_attestation(attestation),
        encoding="utf-8",
    )
    outputs = {"optimizer": sidecar, "linked": artifact}
    assert (
        _load_optimizer_publication(
            outputs,
            cache_fact=cache_fact,
            level="Oz",
            split=False,
            preserve_debug=False,
        )
        == attestation
    )
    for invalid in (
        {**cache_fact, "path": str(tmp_path / "other-host")},
        {**cache_fact, "sha256": "f" * 64},
        {**cache_fact, "binaryen_version": "wasm-opt version 129 (version_129)"},
    ):
        with pytest.raises(ValueError, match="differs from the admitted tool"):
            _load_optimizer_publication(
                outputs,
                cache_fact=invalid,
                level="Oz",
                split=False,
                preserve_debug=False,
            )
    artifact.write_bytes(b"replaced")
    with pytest.raises(ValueError, match="differs from the admitted tool"):
        _load_optimizer_publication(
            outputs,
            cache_fact=cache_fact,
            level="Oz",
            split=False,
            preserve_debug=False,
        )


def test_wasm_opt_binary_content_invalidates_only_optimizer_cache(
    tmp_path: Path,
) -> None:
    executable = tmp_path / "wasm-opt"
    executable.write_bytes(b"binaryen-build-a")

    def keys() -> tuple[str | None, str]:
        identity = _wasm_optimizer_identity(
            executable.resolve(),
            sha256=hashlib.sha256(executable.read_bytes()).hexdigest(),
            version="wasm-opt version 130 (version_130)",
        )
        split = wasm_link_optimizer_policy._split_app_optimize_cache_key(
            app_data=b"app",
            reference_data=b"reference",
            optimize=True,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_authority_digest="facts",
            optimizer_identity=identity,
        )
        tree = wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
            runtime_data=b"runtime",
            normalized_required_exports={"molt_main"},
            facts_authority_digest="facts",
        )
        return split, tree

    first = keys()
    executable.write_bytes(b"binaryen-build-b-with-different-content")
    second = keys()

    assert first[0] is not None
    assert second[0] is not None
    assert first[0] != second[0]
    assert first[1] == second[1]


def test_wasm_opt_version_invalidates_optimizer_cache_with_same_binary(
    tmp_path: Path,
) -> None:
    executable = (tmp_path / "wasm-opt").resolve()
    cache_key_args = {
        "app_data": b"app",
        "reference_data": b"reference",
        "optimize": True,
        "optimize_level": "Oz",
        "contract_keep_set": {"molt_main"},
        "facts_authority_digest": "facts",
    }
    version_129 = wasm_link_optimizer_policy._split_app_optimize_cache_key(
        **cache_key_args,
        optimizer_identity=_wasm_optimizer_identity(
            executable,
            version="wasm-opt version 129 (version_129)",
        ),
    )
    version_130 = wasm_link_optimizer_policy._split_app_optimize_cache_key(
        **cache_key_args,
        optimizer_identity=_wasm_optimizer_identity(
            executable,
            version="wasm-opt version 130 (version_130)",
        ),
    )

    assert version_129 is not None
    assert version_130 is not None
    assert version_129 != version_130


def test_split_app_optimization_fails_closed_without_invocation_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_run_wasm_opt_via_optimize",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("unstable optimizer identity must prevent execution")
        ),
    )

    assert (
        wasm_link_optimizer_policy._split_app_optimize_cache_key(
            app_data=b"app",
            reference_data=None,
            optimize=True,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_authority_digest="facts",
            optimizer_identity=None,
        )
        is None
    )
    counts: dict[str, int | float] = {}
    app = _build_split_runtime_app_module([])
    with pytest.raises(RuntimeError, match="executable identity"):
        wasm_link_optimizer_policy._optimize_split_app_module(
            app,
            reference_data=None,
            optimize=True,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            operation_counts=counts,
            facts_provider=_facts_provider,
        )
    assert counts["split_app_optimize_cache_requests"] == 1
    assert counts["split_app_optimize_cache_identity_errors"] == 1
    assert not list((tmp_path / "cache").rglob("artifact.wasm"))


def test_link_transaction_cleans_owned_resources_when_command_planning_fails(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    class CountingTemporaryDirectory(OwnedTemporaryDirectory):
        cleanup_calls = 0

        def cleanup(self) -> None:
            self.cleanup_calls += 1
            super().cleanup()

    owned_temp_dir = CountingTemporaryDirectory(dir=tmp_path, prefix="transaction-")
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    staged_linked = tmp_path / "staged-output.wasm"
    timings = {}
    real_staged_output_path = wasm_link_pipeline.artifact_publish.staged_output_path
    loaded = wasm_link_pipeline._LoadedLinkContract(
        runtime_exports=frozenset({"molt_main"}),
        output_data=b"\0asm\x01\0\0\0",
        app_export_contract={},
        app_call_abi={},
        facts_provider=_facts_provider,
        output_callable_layout=None,
        split_callable_layout=None,
        monolithic_callable_layout=None,
        deploy_runtime_path=None,
        output_memory_min=None,
        output_table_min=None,
        callable_entry_export_names=(),
        reserved_runtime_link_exports=(),
    )
    prepared = wasm_link_pipeline._PreparedLinkInputs(
        required_native_direct_symbols=(),
        export_symbol_map={},
        callable_entry_symbol_names=(),
        callable_entry_symbol_names_by_slot=(),
        contract_app_exports=(),
        app_target_symbol_map={},
        app_adapter_symbol_map={},
        app_adapter_identity_map={},
        app_target_identity_map={},
        app_identity_exports={},
        preserved_output_exports=(),
        user_export_symbol_names=(),
        rewritten_path=output,
        rewritten_requirements=_native_link_requirements(),
        force_exports=(),
        base_allowlist=tmp_path / "allowlist",
        allowlist=tmp_path / "allowlist",
        linked_rewritten_path=output,
        linked_requirements=_native_link_requirements(),
    )

    def staged_output_path(destination: Path) -> Path:
        if destination == linked:
            staged_linked.write_bytes(b"orphan")
            return staged_linked
        return real_staged_output_path(destination)

    monkeypatch.setattr(
        wasm_link_pipeline,
        "OwnedTemporaryDirectory",
        lambda **_kwargs: owned_temp_dir,
    )
    monkeypatch.setattr(
        wasm_link_pipeline,
        "_load_link_contract_stage",
        lambda *_args, **_kwargs: loaded,
    )
    monkeypatch.setattr(
        wasm_link_pipeline,
        "_prepare_link_inputs_stage",
        lambda *_args, **_kwargs: prepared,
    )
    monkeypatch.setattr(
        wasm_link_pipeline,
        "_plan_link_commands_stage",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        wasm_link_pipeline.artifact_publish,
        "staged_output_path",
        staged_output_path,
    )

    result = wasm_link_pipeline.run_wasm_ld_with_custodied_inputs(
        "wasm-ld",
        tmp_path / "runtime.wasm",
        output,
        linked,
        runtime_role="shared",
        phase_timings_ms=timings,
        wasm_facts_scanner=tmp_path / "facts-scanner",
        app_export_contract_path=tmp_path / "app-export-contract.json",
        facts_provider=_facts_provider,
    )

    assert result == 1
    assert owned_temp_dir.cleanup_calls == 1
    assert not Path(owned_temp_dir.name).exists()
    assert not staged_linked.exists()
    assert timings["wasm_link_total"] >= 0
    assert timings["wasm_whole_artifact_section_walks"] == 0


def test_run_wasm_ld_split_runtime_uses_explicit_deploy_runtime_over_stale_env(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_split_runtime_app_module([])
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    control_linked = tmp_path / "control_linked.wasm"
    control_split_dir = tmp_path / "control_split"
    timings_path = tmp_path / "phase_timings.json"
    stale_runtime = tmp_path / "missing-runtime.wasm"

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    wasm_ld_commands: list[list[str]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_commands.append(list(cmd))
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setenv("MOLT_WASM_DEPLOY_RUNTIME", str(stale_runtime))
    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_kwargs: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        deploy_runtime_override=runtime,
        phase_timings_file=timings_path,
    )
    first_link_commands = list(wasm_ld_commands)
    control_rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        control_linked,
        split_runtime=True,
        split_output_dir=control_split_dir,
        deploy_runtime_override=runtime,
    )

    assert rc == 0
    assert control_rc == 0
    assert len(first_link_commands) == 2, (
        "split-runtime builds without native objects must run wasm-ld for both "
        "the monolithic artifact and the split app"
    )
    assert all("--no-entry" in cmd for cmd in first_link_commands)
    expected_runtime = _with_test_link_facts(
        wasm_link_operations.strip_publication_sections(
            runtime.read_bytes(), final_artifact=True, preserve_debug=False
        ),
        role="runtime",
    )
    actual_runtime = wasm_link_operations.strip_publication_sections(
        (split_dir / "molt_runtime.wasm").read_bytes(),
        final_artifact=True,
        preserve_debug=False,
    )
    assert actual_runtime == expected_runtime
    assert linked.read_bytes() == control_linked.read_bytes()
    assert (split_dir / "app.wasm").read_bytes() == (
        control_split_dir / "app.wasm"
    ).read_bytes()
    assert (split_dir / "molt_runtime.wasm").read_bytes() == (
        control_split_dir / "molt_runtime.wasm"
    ).read_bytes()
    size_attestation = json.loads(
        (split_dir / "wasm_size_attestation.json").read_text(encoding="utf-8")
    )
    assert size_attestation["published"]["app"]["sections"]["export"] > 0
    assert size_attestation["published"]["runtime"]["sections"]["export"] > 0
    timings = json.loads(timings_path.read_text(encoding="utf-8"))
    assert set(timings) >= {
        "wasm_link_total",
        "split_runtime_processing",
        "wasm_strip",
        "fail_closed_validation",
        "wasm_facts_hash_ms",
        "wasm_facts_scan_ms",
        "wasm_facts_scan_calls",
        "wasm_facts_cache_hits",
        "wasm_facts_input_bytes",
        "wasm_facts_response_chars",
        "runtime_tree_shake_cache_hits",
        "runtime_tree_shake_cache_misses",
        "runtime_tree_shake_cache_wall_ms",
        "split_app_optimize_cache_hits",
        "split_app_optimize_cache_misses",
        "split_app_optimize_cache_wall_ms",
        "split_app_optimize_cache_optimizer_peak_total_rss_kb",
    }


def test_explicit_deploy_runtime_outranks_ambient_and_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    reloc = tmp_path / "molt_runtime_reloc.wasm"
    sibling = tmp_path / "molt_runtime.wasm"
    explicit = tmp_path / "explicit_runtime.wasm"
    ambient = tmp_path / "ambient_runtime.wasm"
    for path in (reloc, sibling, explicit, ambient):
        path.write_bytes(b"\0asm\x01\0\0\0")
    monkeypatch.setenv("MOLT_WASM_DEPLOY_RUNTIME", str(ambient))

    assert wasm_link_runtime_data._resolve_deploy_runtime(explicit) == explicit
    with pytest.raises(FileNotFoundError, match="explicit split deploy runtime"):
        wasm_link_runtime_data._resolve_deploy_runtime(tmp_path / "missing.wasm")


def test_split_callable_layout_preserves_conservative_app_boundary() -> None:
    app_layout = wasm_link_format.CallableTableLayout(
        fixed_prefix_base=1,
        fixed_prefix_len=81,
        finalized_app_base=2_794,
        app_entry_count=8_440,
    )
    final_runtime_layout = wasm_artifact.WasmSplitRuntimeCallableLayout(
        runtime_callable_base=1,
        runtime_occupied_end=1_849,
        runtime_table_min=1_849,
        fixed_prefix_len=81,
    )

    assert (
        wasm_link_callable_table._reconcile_split_callable_layout(
            app_layout,
            final_runtime_layout,
        )
        == app_layout
    )


def test_split_callable_layout_rejects_final_runtime_overlap() -> None:
    app_layout = wasm_link_format.CallableTableLayout(
        fixed_prefix_base=1,
        fixed_prefix_len=81,
        finalized_app_base=2_794,
        app_entry_count=8_440,
    )
    overlapping_runtime_layout = wasm_artifact.WasmSplitRuntimeCallableLayout(
        runtime_callable_base=1,
        runtime_occupied_end=2_795,
        runtime_table_min=2_795,
        fixed_prefix_len=81,
    )

    with pytest.raises(ValueError, match="overlap the app-owned callable region"):
        wasm_link_callable_table._reconcile_split_callable_layout(
            app_layout,
            overlapping_runtime_layout,
        )


def test_run_wasm_ld_honors_explicit_reloc_role_for_immutable_generation_member(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = wasm_link_format._append_linking_function_symbols(
        _build_exported_runtime_module("molt_exception_pending"),
        [
            (
                "molt_exception_pending",
                0,
                FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert runtime_bytes is not None
    runtime = tmp_path / "molt_runtime_reloc.wasm.deadbeef.runtime-wasm-member"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    wasm_ld_inputs: list[str] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_kwargs: data
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        runtime_role="reloc",
        runtime_identity=stable_regular_file_identity(
            runtime,
            label="test immutable runtime member",
        ),
    )

    assert rc == 0
    assert any(Path(part).name == runtime.name for part in wasm_ld_inputs)


def test_run_wasm_ld_links_staged_native_objects(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    wasm_ld_inputs: list[str] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_module_with_linking_symbols([]))

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_kwargs: data
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=merge_source_extension_link_requirements(
            (
                _native_link_requirements(*(native_object,)),
                source_extension_link_requirements(
                    ("--undefined=PyInit__ndimage",), target_triple="wasm32-wasip1"
                ),
            ),
            target_triple="wasm32-wasip1",
        ),
    )

    assert rc == 0
    staged_native = [
        part for part in wasm_ld_inputs if Path(part).name == native_object.name
    ]
    assert len(staged_native) == 1
    assert Path(staged_native[0]) != native_object
    assert wasm_ld_inputs.index("--undefined=PyInit__ndimage") < wasm_ld_inputs.index(
        staged_native[0]
    )
    assert "--undefined=PyInit__ndimage" in wasm_ld_inputs


def test_run_wasm_ld_rejects_signature_mismatch_warning(
    tmp_path: Path,
    monkeypatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            _write_wasm_ld_output(cmd, output_bytes)
            return wasm_link_command.subprocess.CompletedProcess(
                cmd,
                0,
                "",
                "wasm-ld: warning: function signature mismatch: molt_guarded_class_def\n",
            )
        return wasm_link_command.subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 1
    assert (
        "function signature mismatch: molt_guarded_class_def" in capsys.readouterr().err
    )


def test_run_wasm_ld_links_rewritten_native_runtime_imports(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_add")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.molt.wasm"
    wasm_ld_inputs: list[str] = []
    rewritten_native_imports: list[list[tuple[str, str]]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_build_env_function_import_module(["molt_add", "malloc"]))

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
            for part in cmd:
                path = Path(part)
                if path.name.startswith("native_runtime_imports_"):
                    rewritten_native_imports.append(
                        _function_import_pairs(path.read_bytes())
                    )
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_kwargs: data
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=_native_link_requirements(*(native_object,)),
    )

    assert rc == 0
    assert str(native_object) not in wasm_ld_inputs
    assert rewritten_native_imports == [
        [
            ("molt_runtime", "molt_add"),
            ("env", "malloc"),
        ]
    ]


def test_run_wasm_ld_rejects_missing_native_object(
    tmp_path: Path,
    monkeypatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    missing_native_object = tmp_path / "external_static_packages" / "missing.o"

    missing_native_object.parent.mkdir()
    missing_native_object.write_bytes(_build_minimal_module(b""))
    requirements = _native_link_requirements(missing_native_object)
    missing_native_object.unlink()

    runtime.write_bytes(_build_exported_runtime_module("molt_exception_pending"))
    output.write_bytes(_build_minimal_module(b""))
    monkeypatch.setattr(
        wasm_link_command,
        "_run_external_tool",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-ld must not run without staged native input")
        ),
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=requirements,
    )

    assert rc == 1
    captured = capsys.readouterr()
    assert (
        "Failed to establish wasm linker input custody: Native WASM link input is unavailable"
    ) in captured.err
    assert "is unavailable" in captured.err
    assert str(missing_native_object) in captured.err


def test_split_native_app_uses_unique_molt_main_restoration_alias(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    runtime_bytes = _build_exported_runtime_module_many(["molt_main"])
    output_bytes = _build_exported_runtime_module("molt_main")
    output_bytes = wasm_link_format._append_linking_function_symbols(
        output_bytes,
        [
            (
                "__molt_output_export_0",
                0,
                FLAG_BINDING_GLOBAL
                | wasm_link_format.FLAG_EXPLICIT_NAME
                | FLAG_EXPORTED
                | FLAG_NO_STRIP,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert output_bytes is not None
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "native.molt.wasm"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.write_bytes(_module_with_linking_symbols([]))
    commands: list[list[str]] = []

    def fake_run(cmd, **_kwargs):
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            commands.append(list(cmd))
        _write_wasm_ld_output(cmd, output_bytes)
        return wasm_link_command.subprocess.CompletedProcess(cmd, 0, "", "")

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _path, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_split_runtime_outputs",
        lambda *_a, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_a, **_k: runtime_bytes,
    )

    assert (
        _run_wasm_ld_with_rust_facts(
            "wasm-ld",
            runtime,
            output,
            linked,
            split_runtime=True,
            split_output_dir=tmp_path / "split",
            native_link_requirements=_native_link_requirements(*(native_object,)),
        )
        == 0
    )
    split_cmd = commands[1]
    assert "--export=molt_main" not in split_cmd
    assert "--export=__molt_output_export_0" in split_cmd


def test_run_wasm_ld_split_runtime_links_native_objects_into_app(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_err_pending")
    app_data_offset = 2 * 65536
    app_table_base = 4096
    output_bytes = _build_runtime_import_data_module(
        [], memory_min=37, data_offset=app_data_offset, table_min=app_table_base
    )
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    link_calls: list[list[str]] = []
    app_link_bytes = _build_split_runtime_app_module([], memory_min=2)

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_module_with_linking_symbols([]))

    def fake_run(cmd, **kwargs):
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
        link_output = app_link_bytes if len(link_calls) == 2 else output_bytes
        _write_wasm_ld_output(cmd, link_output)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_split_runtime_outputs",
        lambda *_a, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_optimize_split_app_module", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        native_link_requirements=merge_source_extension_link_requirements(
            (
                _native_link_requirements(*(native_object,)),
                source_extension_link_requirements(
                    ("--undefined=PyInit__ndimage",), target_triple="wasm32-wasip1"
                ),
            ),
            target_triple="wasm32-wasip1",
        ),
    )

    assert rc == 0
    assert len(link_calls) == 2
    monolithic_cmd, split_app_cmd = link_calls
    assert any(Path(part).name == native_object.name for part in monolithic_cmd)
    assert any(Path(part).name == native_object.name for part in split_app_cmd)
    assert "--undefined=PyInit__ndimage" in monolithic_cmd
    assert "--undefined=PyInit__ndimage" in split_app_cmd
    assert any(Path(part).name == runtime.name for part in monolithic_cmd)
    assert not any(Path(part).name == runtime.name for part in split_app_cmd)
    assert "--stack-first" in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--emit-relocs" not in monolithic_cmd
    assert "--emit-relocs" in split_app_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert f"--global-base={app_data_offset + 16}" in split_app_cmd
    assert f"--table-base={app_table_base}" in split_app_cmd
    assert not any("molt_runtime_stub" in part for part in monolithic_cmd)
    assert not any("molt_runtime_stub" in part for part in split_app_cmd)
    app_wasm = (split_dir / "app.wasm").read_bytes()
    assert (
        wasm_link_edit._memory_import_min(app_wasm, facts_provider=_facts_provider)
        == 37
    )
    assert _defined_memory_min(app_wasm) is None


def test_run_wasm_ld_split_runtime_forces_native_direct_symbols(
    tmp_path: Path,
    monkeypatch,
) -> None:
    symbol = "PyInit__demo"
    sealed_symbol = "PyInit__sealed_only"
    runtime_bytes = _build_exported_runtime_module_many(["molt_main"])
    output_bytes = _build_native_direct_import_module(symbol)
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "_demo.molt.wasm"
    link_calls: list[list[str]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_module_with_linking_symbols([]))
    native_object.with_name(native_object.name + ".extension_manifest.json").write_text(
        json.dumps(
            {
                "module": "nativepkg._demo",
                "init_symbol": sealed_symbol,
            }
        ),
        encoding="utf-8",
    )

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
            linked_input = Path(cmd[cmd.index("-o") + 2]).read_bytes()
            function_imports = {
                (entry.module, entry.name)
                for entry in wasm_link_format._collect_imports(linked_input)
                if entry.kind == 0
            }
            assert ("env", symbol) in function_imports
            assert ("molt_native", symbol) not in function_imports
        _write_wasm_ld_output(
            cmd,
            _build_exported_runtime_module_many([symbol, sealed_symbol]),
        )

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_split_runtime_outputs",
        lambda *_a, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_optimize_split_app_module", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        native_link_requirements=_native_link_requirements(*(native_object,)),
    )

    assert rc == 0
    assert len(link_calls) == 2
    for cmd in link_calls:
        assert f"--export={symbol}" in cmd
        assert f"--export={sealed_symbol}" in cmd
        assert f"--undefined={symbol}" not in cmd
        assert f"--export-if-defined={symbol}" not in cmd


def test_sealed_native_init_symbols_fail_closed_on_invalid_manifest(
    tmp_path: Path,
) -> None:
    native_object = tmp_path / "_demo.molt.wasm"
    native_object.write_bytes(b"\x00asm\x01\x00\x00\x00")
    native_object.with_name(native_object.name + ".extension_manifest.json").write_text(
        '{"init_symbol": "demo"}', encoding="utf-8"
    )

    with pytest.raises(ValueError, match="invalid init_symbol"):
        wasm_link_native_inputs._sealed_native_init_symbols((native_object,))


def test_public_export_restoration_preserves_sealed_init_symbol() -> None:
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)

    restored = wasm_link_export_contract._restore_public_output_exports(
        linked,
        {"nativepkg___demo": symbol},
        preserved_symbol_names=(symbol,),
        facts_provider=_facts_provider,
    )

    exports = wasm_link_format._collect_function_exports(restored)
    assert symbol in exports


def test_public_export_restoration_recovers_sealed_init_symbol_from_linking_name() -> (
    None
):
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)
    linked = wasm_link_format._append_linking_function_symbols(
        linked,
        [
            (
                symbol,
                0,
                FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert linked is not None
    stripped = wasm_link_edit._strip_internal_exports(linked)
    assert stripped is not None
    assert symbol not in wasm_link_format._collect_function_exports(stripped)

    restored = wasm_link_export_contract._restore_public_output_exports(
        stripped,
        {},
        preserved_symbol_names=(symbol,),
        facts_provider=_facts_provider,
    )

    assert wasm_link_format._collect_function_exports(restored)[symbol] == 0
    assert (
        wasm_link_native_inputs._validate_required_native_direct_symbols(
            restored,
            (symbol,),
            description="Split-runtime native app link",
            facts_provider=_facts_provider,
        )
        is None
    )


def test_native_direct_contract_restoration_recovers_stripped_real_body_by_name() -> (
    None
):
    symbol = "PyInit__demo"
    linked = _build_exported_function_module(symbol)
    linked = wasm_link_format._append_linking_function_symbols(
        linked,
        [
            (
                symbol,
                0,
                FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert linked is not None
    function_index = wasm_link_format._collect_function_exports(linked)[symbol]
    stripped = wasm_link_edit._strip_internal_exports(linked)
    assert stripped is not None

    restored = wasm_link_export_contract._restore_public_output_exports(
        stripped,
        {},
        preserved_symbol_names=(symbol,),
        facts_provider=_facts_provider,
    )

    assert (
        wasm_link_format._collect_function_exports(restored)[symbol] == function_index
    )
    assert (
        wasm_link_native_inputs._validate_required_native_direct_symbols(
            restored,
            (symbol,),
            description="Split-runtime native app link",
            facts_provider=_facts_provider,
        )
        is None
    )


def test_native_direct_symbol_validation_rejects_trap_stub() -> None:
    error = wasm_link_native_inputs._validate_required_native_direct_symbols(
        _build_exported_function_module("PyInit__demo", trap_body=True),
        ("PyInit__demo",),
        description="Split-runtime native app link",
        facts_provider=_facts_provider,
    )

    assert error is not None
    assert "trap stub(s): PyInit__demo" in error


def test_run_wasm_ld_split_runtime_uses_linked_and_deploy_import_namespaces(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = wasm_link_operations.build_sections(
        [
            *wasm_link_operations.parse_sections(
                _add_data_address_global_exports(
                    _build_exported_runtime_module_many(["molt_err_pending"]),
                    {"PyLong_Type": 4096},
                )
            ),
            (
                0,
                wasm_link_format._build_custom_section(
                    "linking",
                    wasm_link_format._build_linking_payload(
                        2,
                        [
                            (
                                SYMTAB_SUBSECTION_ID,
                                _build_symbol_subsection(
                                    [
                                        _function_symbol_entry(
                                            flags=(
                                                FLAG_BINDING_GLOBAL
                                                | wasm_link_format.FLAG_EXPLICIT_NAME
                                            ),
                                            index=0,
                                            name="molt_err_pending",
                                        ),
                                        _data_symbol_entry(
                                            flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                                            name="PyLong_Type",
                                            segment_index=0,
                                            offset=4096,
                                            size=208,
                                        ),
                                    ]
                                ),
                            )
                        ],
                    ),
                ),
            ),
        ]
    )
    output_bytes = _build_runtime_import_module(["molt_err_pending"])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.molt.wasm"
    link_calls: list[list[str]] = []
    linked_app_imports: list[list[tuple[str, str]]] = []
    deployed_native_imports: list[list[tuple[str, str]]] = []
    data_alias_symbols: list[list[tuple[int, str]]] = []
    allowlists: list[set[str]] = []
    compiler_rt_snapshots: list[tuple[str, bytes]] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(
        wasm_link_operations.build_sections(
            [
                *wasm_link_operations.parse_sections(
                    _build_env_function_import_module(
                        ["molt_err_pending", "malloc", "__trunctfdf2"]
                    )
                ),
                (
                    0,
                    wasm_link_format._build_custom_section(
                        "linking",
                        wasm_link_format._build_linking_payload(
                            2,
                            [
                                (
                                    SYMTAB_SUBSECTION_ID,
                                    _build_symbol_subsection(
                                        [
                                            _data_symbol_entry(
                                                flags=(
                                                    wasm_link_format.FLAG_UNDEFINED
                                                    | wasm_link_format.FLAG_EXPLICIT_NAME
                                                ),
                                                name="PyLong_Type",
                                            )
                                        ]
                                    ),
                                )
                            ],
                        ),
                    ),
                ),
            ]
        )
    )
    compiler_rt_provider = tmp_path / "rustlib" / "libcompiler_builtins-x.rlib"
    compiler_rt_provider.parent.mkdir()
    compiler_rt_provider.write_bytes(_build_compiler_rt_provider_archive())

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld" and "-r" not in cmd:
            link_calls.append(list(cmd))
            for part in cmd:
                path = Path(part)
                if path.name == "output_linked_runtime_imports.wasm":
                    linked_app_imports.append(_function_import_pairs(path.read_bytes()))
                if path.name.startswith("native_runtime_imports_"):
                    deployed_native_imports.append(
                        _function_import_pairs(path.read_bytes())
                    )
                if path.name == "split_runtime_data_aliases.wasm":
                    data_alias_symbols.append(
                        _linking_data_symbol_names(path.read_bytes())
                    )
                if path.name == compiler_rt_provider.name:
                    compiler_rt_snapshots.append((str(path), path.read_bytes()))
                if part.startswith("--allow-undefined-file="):
                    allowlists.append(_parse_allowlist(Path(part.split("=", 1)[1])))
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = "\n".join(
                str(Path(part).resolve())
                for part in cmd[1:]
                if not part.startswith("-") and Path(part).is_file()
            )

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_split_runtime_outputs",
        lambda *_a, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_optimize_split_app_module", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda: compiler_rt_provider,
        raising=True,
    )

    def provider_symbols(*, primitive_classes=None, **_kwargs):
        symbols: set[str] = set()
        if (
            primitive_classes is None
            or wasm_link_native_inputs.WASM_LIBC_LINK_IMPORT_CLASS in primitive_classes
        ):
            symbols.add("malloc")
        if (
            primitive_classes is None
            or wasm_link_native_inputs.WASM_COMPILER_RT_LINK_IMPORT_CLASS
            in primitive_classes
        ):
            symbols.add("__trunctfdf2")
        return frozenset(symbols)

    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        provider_symbols,
        raising=True,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
        runtime_role="reloc",
        native_link_requirements=_native_link_requirements(*(native_object,)),
    )

    assert rc == 0
    assert len(link_calls) == 2
    monolithic_cmd, split_app_cmd = link_calls
    assert any(Path(part).name == native_object.name for part in monolithic_cmd)
    assert any(Path(part).name == compiler_rt_provider.name for part in monolithic_cmd)
    assert any(Path(part).name == runtime.name for part in monolithic_cmd)
    assert not any(Path(part).name == native_object.name for part in split_app_cmd)
    assert any(Path(part).name == compiler_rt_provider.name for part in split_app_cmd)
    assert compiler_rt_snapshots
    assert all(
        path != str(compiler_rt_provider) for path, _data in compiler_rt_snapshots
    )
    assert all(
        data == compiler_rt_provider.read_bytes()
        for _path, data in compiler_rt_snapshots
    )
    assert "--import-memory" not in monolithic_cmd
    assert "--import-memory" in split_app_cmd
    assert "--stack-first" in monolithic_cmd
    assert "--no-stack-first" in split_app_cmd
    assert "--stack-first" not in split_app_cmd
    assert "--global-base=67108864" in split_app_cmd
    assert any(
        Path(part).name == "output_linked_runtime_imports.wasm"
        for part in monolithic_cmd
    )
    assert not any("molt_runtime_stub" in part for part in monolithic_cmd)
    assert not any(
        Path(part).name.startswith("native_runtime_imports_") for part in monolithic_cmd
    )
    assert not any(
        Path(part).name == "split_runtime_data_aliases.wasm" for part in monolithic_cmd
    )
    assert any(
        Path(part).name.startswith("native_runtime_imports_") for part in split_app_cmd
    )
    assert any(
        Path(part).name == "split_runtime_data_aliases.wasm" for part in split_app_cmd
    )
    assert linked_app_imports == [[("env", "molt_err_pending")]]
    assert deployed_native_imports == [
        [
            ("molt_runtime", "molt_err_pending"),
            ("env", "malloc"),
            ("env", "__trunctfdf2"),
        ]
    ]
    assert data_alias_symbols == [
        [(wasm_link_format.FLAG_EXPLICIT_NAME, "molt_PyLong_Type")]
    ]
    assert "molt_err_pending" not in allowlists[0]
    assert "molt_err_pending" in allowlists[1]
    assert "malloc" in allowlists[1]
    assert "__trunctfdf2" not in allowlists[0]
    assert "__trunctfdf2" not in allowlists[1]


def test_canonical_split_runtime_required_exports_uses_generated_authority() -> None:
    module = _build_exported_runtime_module_many(
        [
            "molt_exception_pending",
            "molt_object_field_get",
            "molt_object_field_set",
            "molt_guarded_field_get",
        ]
    )

    exports = wasm_link_runtime_data._canonical_split_runtime_required_exports(
        module,
        runtime_imports=(
            "object_field_get",
            "object_field_set",
            "guarded_field_get",
        ),
        facts_provider=_facts_provider,
    )

    assert exports == {
        "molt_object_field_get",
        "molt_object_field_set",
        "molt_guarded_field_get",
    }


def test_canonical_split_runtime_required_exports_rejects_missing_generated_export() -> (
    None
):
    module = _build_exported_runtime_module("molt_object_field_get")

    with pytest.raises(ValueError, match="molt_object_field_set"):
        wasm_link_runtime_data._canonical_split_runtime_required_exports(
            module,
            runtime_imports=("object_field_get", "object_field_set"),
            facts_provider=_facts_provider,
        )


def test_run_wasm_opt_via_optimize_enforces_current_export_contract(
    tmp_path: Path,
    monkeypatch,
) -> None:
    linked = tmp_path / "linked.wasm"
    linked.write_bytes(
        _build_exported_runtime_module_many(["molt_main", "molt_host_init"])
    )
    optimizer_identity = _wasm_optimizer_identity(tmp_path / "wasm-opt")
    seen: dict[str, object] = {}

    def fake_optimize(
        input_path,
        *,
        output_path,
        level,
        extra_passes,
        converge,
        required_exports,
        apply_level,
        optimizer_identity,
        preserve_debug=False,
    ):
        seen["input_path"] = input_path
        seen["level"] = level
        seen["converge"] = converge
        seen["required_exports"] = set(required_exports)
        seen["apply_level"] = apply_level
        seen["optimizer_identity"] = optimizer_identity
        output_path.write_bytes(input_path.read_bytes())
        digest = hashlib.sha256(input_path.read_bytes()).hexdigest()
        return {
            "ok": True,
            "status": "success",
            "output_bytes": output_path.stat().st_size,
            "binaryen_version": optimizer_identity.binaryen_version,
            "wasm_opt_path": str(optimizer_identity.path),
            "wasm_opt_sha256": optimizer_identity.sha256,
            "pipeline": list(
                wasm_link_optimizer_policy.wasm_link_policy(level).pipeline
            ),
            "before": {
                "file_bytes": input_path.stat().st_size,
                "sha256": digest,
            },
            "after": {
                "file_bytes": output_path.stat().st_size,
                "sha256": digest,
            },
            "error": "",
        }

    monkeypatch.setattr(wasm_link_optimizer_policy, "optimize_wasm", fake_optimize)

    attestation: dict[str, object] = {}
    telemetry: dict[str, object] = {}
    assert wasm_link_optimizer_policy._run_wasm_opt_via_optimize(
        linked,
        level="Oz",
        required_exports=set(_facts_provider(linked.read_bytes()).function_exports),
        attestation=attestation,
        telemetry=telemetry,
        optimizer_identity=optimizer_identity,
    )
    assert seen["required_exports"] == {"molt_main", "molt_host_init"}
    assert seen["apply_level"] is True
    assert seen["optimizer_identity"] is optimizer_identity
    assert "wasm_opt_path" not in attestation
    assert "wasm_opt_wall_ms" not in attestation
    assert (
        attestation["optimizer_input_sha256"]
        == hashlib.sha256(linked.read_bytes()).hexdigest()
    )
    assert telemetry["wasm_opt_path"] == str(optimizer_identity.path)
    assert telemetry["wasm_opt_wall_ms"] == 0.0


def test_optimize_reuses_invocation_identity_without_rediscovery(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    from molt import wasm_optimizer_identity as identity_authority

    optimizer_module = importlib.import_module(
        wasm_link_optimizer_policy.optimize_wasm.__module__
    )
    executable = (tmp_path / "wasm-opt").resolve()
    executable.write_bytes(b"binaryen-test-executable")
    stable = identity_authority.stable_regular_file_identity(
        executable,
        label="test wasm-opt",
    )
    optimizer_identity = wasm_link_optimizer_policy.WasmOptimizerExecutableIdentity(
        executable=stable,
        binaryen_version="wasm-opt version 130 (version_130)",
    )
    source = tmp_path / "input.wasm"
    destination = tmp_path / "output.wasm"
    source.write_bytes(_module_with_linking_symbols([]))

    monkeypatch.setattr(
        optimizer_module,
        "find_wasm_opt",
        lambda: (_ for _ in ()).throw(
            AssertionError("invocation-scoped identity must bypass discovery")
        ),
    )
    monkeypatch.setattr(
        optimizer_module,
        "wasm_metrics",
        lambda path: {"file_bytes": Path(path).stat().st_size},
    )
    monkeypatch.setattr(optimizer_module, "_export_names", lambda _path: frozenset())

    def run_optimizer(command, **_kwargs):  # type: ignore[no-untyped-def]
        staged_output = Path(command[command.index("-o") + 1])
        staged_output.write_bytes(source.read_bytes())
        return wasm_link_command.subprocess.CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(
        optimizer_module.harness_memory_guard,
        "guarded_completed_process",
        run_optimizer,
    )

    result = optimizer_module.optimize(
        source,
        output_path=destination,
        level="Oz",
        optimizer_identity=optimizer_identity,
    )

    assert result["ok"] is True
    assert result["wasm_opt_path"] == str(executable)
    assert result["wasm_opt_sha256"] == optimizer_identity.sha256
    assert destination.read_bytes() == source.read_bytes()


def test_oz_publication_pipeline_is_bounded_and_size_focused() -> None:
    assert list(wasm_link_optimizer_policy.wasm_link_policy("Oz").extra_passes) == [
        "--remove-unused-module-elements",
        "--strip-debug",
        "--strip-producers",
        "--dae-optimizing",
        "--simplify-locals",
        "--merge-blocks",
        "--dce",
        "--vacuum",
        "--zero-filled-memory",
        "--memory-packing",
    ]


def test_neutralize_dead_element_entries_preserves_host_call_indirect_modules() -> None:
    facts = {
        "reachable_function_indices": [],
        "active_function_elements": [],
        "reachable_dynamic_dispatch": True,
        "reachable_function_reference_dispatch": False,
        "exported_table_indices": [],
        "table_mutations": [],
    }
    assert (
        wasm_link_optimize._neutralize_dead_element_entries(
            _build_host_call_indirect_module(), facts
        )
        is None
    )


def test_neutralize_dead_element_entries_uses_reachable_roots_and_fail_closed_controls() -> (
    None
):
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = [
        (1, write_varuint(1) + b"\x60\x00\x00"),
        (3, write_varuint(2) + write_varuint(0) + write_varuint(0)),
        (4, write_varuint(1) + b"\x70\x00" + write_varuint(1)),
        (
            9,
            write_varuint(1)
            + b"\x00\x41\x00\x0b"
            + write_varuint(1)
            + write_varuint(1),
        ),
        (10, write_varuint(2) + b"\x02\x00\x0b\x02\x00\x0b"),
    ]
    module = wasm_link_operations.build_sections(sections)
    facts = {
        "reachable_function_indices": [0],
        "active_function_elements": [[0, 0, 1]],
        "reachable_dynamic_dispatch": False,
        "reachable_function_reference_dispatch": False,
        "exported_table_indices": [],
        "table_mutations": [],
    }

    neutralized = wasm_link_optimize._neutralize_dead_element_entries(module, facts)
    assert neutralized is not None
    element_payload = next(
        payload
        for section_id, payload in wasm_link_operations.parse_sections(neutralized)
        if section_id == 9
    )
    assert element_payload.endswith(b"\x00")
    assert wasm_link_optimize._reachable_function_indices(facts) == {0}

    observable = dict(facts)
    observable["exported_table_indices"] = [0]
    observable["reachable_function_indices"] = [0, 1]
    assert wasm_link_optimize._reachable_function_indices(observable) == {0, 1}
    nonzero_observable = dict(facts)
    nonzero_observable["exported_table_indices"] = [1]
    assert wasm_link_optimize._reachable_function_indices(nonzero_observable) == {0}
    dynamic = dict(facts)
    dynamic["reachable_dynamic_dispatch"] = True
    dynamic["reachable_function_indices"] = [0, 1]
    assert wasm_link_optimize._reachable_function_indices(dynamic) == {0, 1}
    reachable_ref = dict(facts)
    reachable_ref["reachable_function_indices"] = [0, 1]
    assert wasm_link_optimize._reachable_function_indices(reachable_ref) == {0, 1}
    table_init = dict(facts)
    table_init["table_mutations"] = [[0, "table.init", 0, None]]
    table_init["reachable_function_indices"] = [0, 1]
    assert wasm_link_optimize._reachable_function_indices(table_init) == {0, 1}

    for override in (
        {"reachable_dynamic_dispatch": True},
        {"reachable_function_reference_dispatch": True},
        {"exported_table_indices": [0]},
        {"table_mutations": [[0, "table.init", 0, None]]},
    ):
        controlled = dict(facts)
        controlled.update(override)
        assert (
            wasm_link_optimize._neutralize_dead_element_entries(module, controlled)
            is None
        )

    opaque_ref_dispatch = dict(facts)
    opaque_ref_dispatch["reachable_function_reference_dispatch"] = True
    assert wasm_link_optimize._stub_dead_functions(module, opaque_ref_dispatch) is None


def test_import_walkers_handle_tag_imports_before_host_call_indirect() -> None:
    module = _build_tag_then_host_call_indirect_import_module()
    sections = wasm_link_operations.parse_sections(module)

    assert wasm_link_format._count_func_imports(sections) == 1


def test_strip_unused_module_function_imports_remaps_indices() -> None:
    name_map = (
        wasm_link_format._write_varuint(1)
        + wasm_link_format._write_varuint(2)
        + wasm_link_format._write_string("molt_main")
    )
    module = wasm_link_operations.build_sections(
        [
            *wasm_link_operations.parse_sections(_build_runtime_import_strip_module()),
            (
                0,
                wasm_link_format._build_custom_section(
                    "name",
                    b"\x01" + wasm_link_format._write_varuint(len(name_map)) + name_map,
                ),
            ),
            (0, wasm_link_format._build_custom_section(".debug_info", b"debug")),
        ]
    )
    facts = _rust_facts_fixture(module)
    facts["reachable_function_indices"] = [1, 2]
    facts["referenced_function_indices"] = [1, 2]

    def facts_provider(data: bytes):  # type: ignore[no-untyped-def]
        if data == module:
            frozen = wasm_link_fact_provider._freeze_json(facts)
            assert isinstance(frozen, dict)
            return wasm_link_fact_provider.WasmLinkFacts(frozen)
        return _facts_provider(data)

    stripped = wasm_link_optimize._strip_unused_module_function_imports(
        module,
        module_name="molt_runtime",
        facts_provider=facts_provider,
    )

    imports_after = _function_import_pairs(stripped)
    assert imports_after == [("molt_runtime", "live_runtime_fn")]

    exports_after = _function_export_pairs(stripped)
    assert exports_after == [("molt_main", 1)]

    call_targets = _parse_code_section_call_targets(stripped)
    assert call_targets == [[0]]
    assert "name" not in _fixture_custom_names(stripped)
    assert ".debug_info" in _fixture_custom_names(stripped)


def test_rewrite_output_imports_uses_generated_runtime_export_names(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output.wasm"
    output.write_bytes(_build_runtime_import_module(["socket_drop", "molt_alloc"]))

    owned_temp_dir = OwnedTemporaryDirectory()
    rewritten = wasm_link_edit._rewrite_output_imports(
        output,
        {"molt_socket_drop", "molt_alloc"},
        owned_temp_dir,
    )

    assert rewritten is not None
    rewritten_path, temp_dir, force_exports = rewritten
    try:
        assert force_exports == []
        assert _function_import_pairs(rewritten_path.read_bytes()) == [
            ("molt_runtime", "molt_socket_drop"),
            ("molt_runtime", "molt_alloc"),
        ]
    finally:
        temp_dir.cleanup()


def test_rewrite_native_runtime_imports_canonicalizes_env_molt_abi_only(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["molt_add", "malloc"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {"molt_add"},
            temp_dir,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_add"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "molt_add"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_routes_canonical_cpython_abi_symbols(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(
        _build_env_function_import_module(
            [
                "PyErr_Format",
                "PyArg_ParseTuple",
                "PyObject_CallFunction",
                "PyArg_ParseTupleAndKeywords",
                "PyTuple_Pack",
                "molt_cpython_abi_date_from_date",
                "malloc",
            ]
        )
    )

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {
                "PyErr_Format",
                "PyArg_ParseTuple",
                "PyObject_CallFunction",
                "PyArg_ParseTupleAndKeywords",
                "PyTuple_Pack",
                "molt_cpython_abi_date_from_date",
            },
            temp_dir,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "PyErr_Format"),
            ("molt_runtime", "PyArg_ParseTuple"),
            ("molt_runtime", "PyObject_CallFunction"),
            ("molt_runtime", "PyArg_ParseTupleAndKeywords"),
            ("molt_runtime", "PyTuple_Pack"),
            ("molt_runtime", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyErr_Format"),
            ("env", "PyArg_ParseTuple"),
            ("env", "PyObject_CallFunction"),
            ("env", "PyArg_ParseTupleAndKeywords"),
            ("env", "PyTuple_Pack"),
            ("env", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_split_runtime_uses_public_cpython_abi_exports(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(
        _build_env_function_import_module(
            [
                "PyType_Ready",
                "Py_DECREF",
                "molt_cpython_abi_date_from_date",
                "malloc",
            ]
        )
    )

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {
                "molt_PyType_Ready",
                "molt_Py_DECREF",
                "molt_cpython_abi_date_from_date",
            },
            temp_dir,
            split_runtime=True,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_PyType_Ready"),
            ("molt_runtime", "molt_Py_DECREF"),
            ("molt_runtime", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyType_Ready"),
            ("env", "Py_DECREF"),
            ("env", "molt_cpython_abi_date_from_date"),
            ("env", "malloc"),
        ]


def test_rewrite_native_runtime_imports_split_runtime_prefixes_cpython_abi_data_symbols(
    tmp_path: Path,
) -> None:
    # CPython ABI type objects (PyLong_Type, PyType_Type, ...) surface as
    # undefined *data* symbols in the relocatable object's linking symtab, not
    # as function imports. The deployed split app must carry them as the
    # molt_-prefixed public export names the shared split runtime provides.
    native = tmp_path / "multiarray.molt.wasm"
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="PyLong_Type",
                ),
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="PyType_Type",
                ),
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="local_defined_datum",
                    segment_index=0,
                    offset=4,
                    size=8,
                ),
            ]
        )
    )

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {"molt_PyLong_Type", "molt_PyType_Type"},
            temp_dir,
            split_runtime=True,
        )

        assert force_exports == []
        assert len(rewritten_paths) == 1
        assert rewritten_paths[0] != native
        assert _linking_data_symbol_names(rewritten_paths[0].read_bytes()) == [
            (
                wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
                "molt_PyLong_Type",
            ),
            (
                wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
                "molt_PyType_Type",
            ),
            (wasm_link_format.FLAG_EXPLICIT_NAME, "local_defined_datum"),
        ]
    # The original relocatable object is never mutated in place.
    assert _linking_data_symbol_names(native.read_bytes()) == [
        (
            wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
            "PyLong_Type",
        ),
        (
            wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
            "PyType_Type",
        ),
        (wasm_link_format.FLAG_EXPLICIT_NAME, "local_defined_datum"),
    ]


def test_rewrite_native_runtime_imports_reloc_keeps_unprefixed_cpython_abi_data_symbols(
    tmp_path: Path,
) -> None:
    # The monolithic runnable statically links native objects against the
    # relocatable runtime, whose CPython ABI type objects are the real
    # unprefixed `#[no_mangle]` symbols. The reloc naming convention
    # (split_runtime=False) must leave the data symbol names untouched so
    # wasm-ld resolves them directly against the relocatable runtime.
    native = tmp_path / "multiarray.molt.wasm"
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="PyLong_Type",
                ),
            ]
        )
    )

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {"molt_PyLong_Type"},
            temp_dir,
            split_runtime=False,
        )

    # No molt_-prefix churn under the reloc convention: the data symbol name is
    # left untouched (it already matches the relocatable runtime symbol) so the
    # object is returned unchanged. The unprefixed name is flagged for
    # force-export so wasm-ld retains it from the relocatable runtime.
    assert force_exports == ["PyLong_Type"]
    assert rewritten_paths == (native,)
    assert _linking_data_symbol_names(native.read_bytes()) == [
        (
            wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
            "PyLong_Type",
        ),
    ]


def test_split_runtime_data_alias_object_uses_deploy_runtime_export_addresses(
    tmp_path: Path,
) -> None:
    # The alias must point the native object's undefined (split-renamed) data
    # symbol at the DEPLOY runtime's canonical address, read from the runtime's
    # exported address global â€” NOT the relocatable runtime's segment-relative
    # offset (which is not a final address).
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    native.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="molt_PyLong_Type",
                ),
            ]
        )
    )
    reloc_runtime.write_bytes(_build_defined_data_symbol_object({"PyLong_Type": 208}))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        alias = wasm_link_runtime_data._split_runtime_data_alias_object(
            native_link_requirements=_native_link_requirements(*(native,)),
            deploy_runtime=deploy_runtime,
            temp_dir=temp_dir,
            reloc_runtime=reloc_runtime,
            facts_provider=_facts_provider,
        )

        assert alias is not None
        assert alias.artifact != native
        alias_bytes = alias.artifact.read_bytes()
        assert _linking_data_symbol_names(alias_bytes) == [
            (wasm_link_format.FLAG_EXPLICIT_NAME, "molt_PyLong_Type"),
        ]
        # The aliased symbol resolves to the deploy runtime's exported address.
        seg_addresses = _alias_segment_addresses(alias_bytes)
        assert seg_addresses[0] == 0x2E1000
        data_sections = [
            payload
            for section_id, payload in wasm_link_operations.parse_sections(alias_bytes)
            if section_id == 11
        ]
        assert data_sections
        assert b"PyLong_Type" not in data_sections[0]
        assert alias.symbol_sizes == (("molt_PyLong_Type", 208),)


def test_split_runtime_data_alias_object_preserves_wasm32_high_bit_address(
    tmp_path: Path,
) -> None:
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x8000_0000})
    )
    reloc_runtime.write_bytes(_build_defined_data_symbol_object({"PyLong_Type": 208}))
    native.write_bytes(_build_undefined_data_symbol_object(["molt_PyLong_Type"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        alias = wasm_link_runtime_data._split_runtime_data_alias_object(
            native_link_requirements=_native_link_requirements(*(native,)),
            deploy_runtime=deploy_runtime,
            temp_dir=temp_dir,
            reloc_runtime=reloc_runtime,
            facts_provider=_facts_provider,
        )
        assert alias is not None
        assert _alias_segment_addresses(alias.artifact.read_bytes()) == {0: 0x8000_0000}


def test_split_runtime_data_alias_requires_relocatable_size_authority(
    tmp_path: Path,
) -> None:
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    native.write_bytes(_build_undefined_data_symbol_object(["molt_PyLong_Type"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        with pytest.raises(
            ValueError, match="exact relocatable runtime size authority"
        ):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=_native_link_requirements(*(native,)),
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
                facts_provider=_facts_provider,
            )


def test_split_runtime_data_alias_rejects_duplicate_relocatable_size_authority(
    tmp_path: Path,
) -> None:
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    duplicate = _data_symbol_entry(
        flags=wasm_link_format.FLAG_EXPLICIT_NAME,
        name="PyLong_Type",
        segment_index=0,
        offset=0,
        size=208,
    )
    reloc_runtime.write_bytes(_module_with_linking_symbols([duplicate, duplicate]))
    native.write_bytes(_build_undefined_data_symbol_object(["molt_PyLong_Type"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        with pytest.raises(ValueError, match="duplicate defined runtime data symbol"):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=_native_link_requirements(*(native,)),
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
                reloc_runtime=reloc_runtime,
                facts_provider=_facts_provider,
            )


@pytest.mark.parametrize("size", [0, 0x1_0000_0000])
def test_split_runtime_data_alias_rejects_invalid_relocatable_symbol_size(
    tmp_path: Path,
    size: int,
) -> None:
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    reloc_runtime.write_bytes(
        _module_with_linking_symbols(
            [
                _data_symbol_entry(
                    flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                    name="PyLong_Type",
                    segment_index=0,
                    offset=0,
                    size=size,
                )
            ]
        )
    )
    native.write_bytes(_build_undefined_data_symbol_object(["molt_PyLong_Type"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        with pytest.raises(ValueError, match="invalid size"):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=_native_link_requirements(*(native,)),
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
                reloc_runtime=reloc_runtime,
                facts_provider=_facts_provider,
            )


def test_split_runtime_data_alias_rejects_missing_relocatable_symbol_size(
    tmp_path: Path,
) -> None:
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    native = tmp_path / "native_runtime_imports_0.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"PyLong_Type": 0x2E1000})
    )
    reloc_runtime.write_bytes(_module_with_linking_symbols([]))
    native.write_bytes(_build_undefined_data_symbol_object(["molt_PyLong_Type"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        with pytest.raises(ValueError, match="missing exact size"):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=_native_link_requirements(*(native,)),
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
                reloc_runtime=reloc_runtime,
                facts_provider=_facts_provider,
            )


def test_split_runtime_data_alias_rejects_malformed_native_wasm(tmp_path: Path) -> None:
    native = tmp_path / "native_runtime_imports_0.wasm"
    native.write_bytes(b"\0asm\x01\0\0\0malformed")

    with pytest.raises(ValueError, match="cannot decode native object linking symbols"):
        wasm_link_runtime_data._undefined_cpython_abi_data_symbols(
            (native,), facts_provider=_facts_provider
        )


def test_rewrite_native_runtime_imports_rejects_non_manifest_raw_c_api_symbol(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["PyArray_NDIM"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            {"PyArray_NDIM"},
            temp_dir,
        )

        assert force_exports == []
        assert rewritten_paths == (native,)
        assert _function_import_pairs(native.read_bytes()) == [
            ("env", "PyArray_NDIM"),
        ]


def test_rewrite_native_runtime_imports_forces_generated_runtime_exports(
    tmp_path: Path,
) -> None:
    native = tmp_path / "ndimage.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["add"]))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()

        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (native,),
            set(),
            temp_dir,
        )

        assert force_exports == ["molt_add"]
        assert _function_import_pairs(rewritten_paths[0].read_bytes()) == [
            ("molt_runtime", "molt_add"),
        ]


def test_native_wasm_archive_members_share_runtime_and_provider_analysis(
    tmp_path: Path,
) -> None:
    member_data = _build_env_function_import_module(["add", "__trunctfdf2"])
    raw = tmp_path / "member.wasm"
    archive = tmp_path / "libnative.a"
    raw.write_bytes(member_data)
    archive.write_bytes(_build_wasm_archive(("member.o", member_data)))

    compiler_rt_symbols = frozenset({"__trunctfdf2"})
    assert (
        wasm_link_native_inputs._compiler_rt_imports_from_wasm(
            raw,
            compiler_rt_symbols,
            facts_provider=_facts_provider,
        )
        == wasm_link_native_inputs._compiler_rt_imports_from_wasm(
            archive,
            compiler_rt_symbols,
            facts_provider=_facts_provider,
        )
        == frozenset({"__trunctfdf2"})
    )

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        rewritten_paths, force_exports = wasm_link_edit._rewrite_native_runtime_imports(
            (archive,),
            set(),
            temp_dir,
            split_runtime=True,
        )

    assert rewritten_paths == (archive,)
    assert force_exports == []
    members = tuple(wasm_archive.iter_wasm_archive_members(archive))
    assert [member.name for member in members] == ["member.o"]
    assert _function_import_pairs(members[0].data) == [
        ("env", "add"),
        ("env", "__trunctfdf2"),
    ]


def test_native_wasm_archive_rejects_uninspected_member_kinds(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "libnative.a"
    archive.write_bytes(_build_wasm_archive(("opaque.o", b"not-wasm")))

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        with pytest.raises(ValueError, match="non-WASM object member"):
            wasm_link_edit._rewrite_native_runtime_imports(
                (archive,),
                set(),
                temp_dir,
                split_runtime=True,
            )


def test_native_wasm_archive_members_share_cpython_data_symbol_analysis(
    tmp_path: Path,
) -> None:
    member_data = _build_undefined_data_symbol_object(["molt_PyLong_Type"])
    raw = tmp_path / "member.wasm"
    archive = tmp_path / "libnative.a"
    raw.write_bytes(member_data)
    archive.write_bytes(_build_wasm_archive(("member.o", member_data)))

    assert wasm_link_runtime_data._undefined_cpython_abi_data_symbols(
        (raw,),
        facts_provider=_facts_provider,
    ) == wasm_link_runtime_data._undefined_cpython_abi_data_symbols(
        (archive,), facts_provider=_facts_provider
    )


def test_split_app_finalization_routes_archive_runtime_imports() -> None:
    artifact = wasm_link_transaction.WasmArtifactState.from_bytes(
        Path("split-app.wasm"),
        _build_env_function_import_module(["add", "__trunctfdf2"]),
        facts_provider=_facts_provider,
    )

    wasm_link_pipeline._normalize_split_app_runtime_imports(
        artifact,
        frozenset(),
    )

    assert _function_import_pairs(artifact.data) == [
        ("molt_runtime", "molt_add"),
        ("env", "__trunctfdf2"),
    ]


def test_split_runtime_typed_edges_reject_runtime_abi_imports_left_in_env() -> None:
    app_facts = _facts_provider(_build_env_function_import_module(["add"]))
    runtime_facts = _facts_provider(_build_exported_runtime_module("molt_add"))

    assert wasm_link_validation._validate_split_runtime_typed_edges(
        app_facts,
        runtime_facts,
    ) == (
        "split-runtime app retains a runtime ABI import in env instead of "
        "molt_runtime: add"
    )


def test_split_runtime_validation_uses_generated_runtime_export_names(
    tmp_path: Path,
) -> None:
    app = tmp_path / "app.wasm"
    runtime = tmp_path / "runtime.wasm"
    app.write_bytes(
        _build_split_runtime_app_module(["socket_drop", "unknown_probe"], memory_min=1)
    )
    runtime.write_bytes(_build_exported_runtime_module("molt_socket_drop"))

    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )

    app.write_bytes(_build_split_runtime_app_module(["socket_drop"], memory_min=1))
    assert wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )

    app.write_bytes(_build_split_runtime_app_module(["PyType_Ready"], memory_min=1))
    runtime.write_bytes(_build_exported_runtime_module("PyType_Ready"))
    assert not wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )

    runtime.write_bytes(_build_exported_runtime_module("molt_PyType_Ready"))
    assert wasm_link_validation._validate_split_runtime_outputs(
        app,
        runtime,
        facts_provider=_facts_provider,
    )


def test_post_link_optimize_split_app_drops_numeric_table_aliases() -> None:
    table_ref = "__molt_table_ref_7"
    module = _build_exported_runtime_module_many(
        ["dead_user_export", "molt_main", table_ref]
    )

    default_optimized = wasm_link_optimize._post_link_optimize(
        module,
        reference_data=module,
        facts_provider=_facts_provider,
    )
    assert "dead_user_export" in wasm_link_format._collect_exports(default_optimized)

    split_app_optimized = wasm_link_optimize._post_link_optimize(
        module,
        reference_data=module,
        preserve_exports={"molt_main"},
        preserve_reference_exports=False,
        facts_provider=_facts_provider,
    )
    split_exports = wasm_link_format._collect_exports(split_app_optimized)
    assert "dead_user_export" not in split_exports
    assert "molt_main" in split_exports
    assert table_ref not in split_exports


def test_linking_symbol_authority_parses_defined_and_undefined_functions() -> None:
    data = _module_with_linking_symbols(
        [
            _data_symbol_entry(
                flags=wasm_link_format.FLAG_EXPLICIT_NAME,
                name="not_a_function",
                segment_index=3,
                offset=12,
                size=8,
            ),
            _function_symbol_entry(
                flags=FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
                index=7,
                name="molt_call_indirect0",
            ),
            _function_symbol_entry(
                flags=wasm_link_format.FLAG_UNDEFINED
                | wasm_link_format.FLAG_EXPLICIT_NAME,
                index=11,
                name="molt_call_indirect13",
            ),
        ]
    )

    symbols = parse_wasm_linking_symbols(data).function_symbols

    assert [(symbol.flags, symbol.index, symbol.name) for symbol in symbols] == [
        (
            FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            7,
            "molt_call_indirect0",
        ),
        (
            wasm_link_format.FLAG_UNDEFINED | wasm_link_format.FLAG_EXPLICIT_NAME,
            11,
            "molt_call_indirect13",
        ),
    ]


def test_restore_output_export_aliases_renames_user_exports() -> None:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(
        wasm_link_format._write_string(
            f"{wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens"
        )
    )
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    restored = wasm_link_edit._restore_output_export_aliases(
        wasm_link_operations.build_sections(sections)
    )
    assert restored is not None
    exports = wasm_link_format._collect_exports(restored)
    assert "main_molt__ocr_tokens" in exports
    assert (
        f"{wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens"
        not in exports
    )


def test_run_wasm_ld_preserves_runtime_entrypoint_without_prelink_alias_object(
    tmp_path: Path,
    monkeypatch,
) -> None:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link_format._write_string("molt_isolate_import"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(4))
    code_payload.append(0x00)
    code_payload.append(0x20)
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link_format._build_linking_payload(
        2,
        [
            (
                SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL,
                            index=0,
                            name="func0",
                        )
                    ]
                ),
            )
        ],
    )
    sections.append(
        (0, wasm_link_format._build_custom_section("linking", linking_payload))
    )

    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(
        _build_exported_runtime_module_many(["molt_main", "molt_isolate_import"])
    )
    output.write_bytes(wasm_link_operations.build_sections(sections))

    captured_cmds: list[list[str]] = []

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        captured_cmds.append(list(cmd))
        if cmd and cmd[0] == "wasm-ld":
            _write_wasm_ld_output(cmd, Path(cmd[-2]).read_bytes())

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_kwargs: data
    )

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 0
    cmd = next(cmd for cmd in captured_cmds if cmd and cmd[0] == "wasm-ld")
    assert not any("output_runtime_aliases.wasm" in part for part in cmd)
    assert "molt_isolate_import" in wasm_link_format._collect_function_exports(
        linked.read_bytes()
    )


def test_restore_public_output_exports_renames_native_split_alias_exports() -> None:
    alias_name = f"{wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}molt_isolate_import"
    module = _build_exported_runtime_module(alias_name)

    restored = wasm_link_export_contract._restore_public_output_exports(
        module,
        {"molt_isolate_import": alias_name},
        facts_provider=_facts_provider,
    )

    exports = wasm_link_format._collect_function_exports(restored)
    assert exports["molt_isolate_import"] == 0
    assert alias_name not in exports


def test_run_wasm_ld_force_exports_user_module_exports(
    tmp_path: Path, monkeypatch
) -> None:
    write_varuint = wasm_link_format._write_varuint

    sections: list[tuple[int, bytes]] = []
    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))
    func_payload = write_varuint(4) + b"".join(write_varuint(0) for _ in range(4))
    sections.append((3, bytes(func_payload)))
    export_payload = bytearray()
    export_payload.extend(write_varuint(4))
    for name, index in (
        ("main_molt__init", 0),
        ("main_molt__ocr_tokens", 1),
        ("main_molt___private_helper", 2),
        ("molt_main", 3),
    ):
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))
    code_payload = bytearray()
    code_payload.extend(write_varuint(4))
    for _ in range(4):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))
    sections.append(
        (
            0,
            wasm_link_format._build_custom_section(
                "reloc.CODE", write_varuint(3) + write_varuint(0)
            ),
        )
    )
    linking_payload = wasm_link_format._build_linking_payload(
        2,
        [
            (
                SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=0,
                            name="__molt_output_export_0",
                        ),
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=1,
                            name="__molt_output_export_1",
                        ),
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=2,
                            name="__molt_output_export_2",
                        ),
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=3,
                            name="molt_main",
                        ),
                    ]
                ),
            )
        ],
    )
    sections.append(
        (0, wasm_link_format._build_custom_section("linking", linking_payload))
    )
    output_bytes = wasm_link_operations.build_sections(sections)

    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(_build_exported_runtime_module("runtime_only"))
    output.write_bytes(output_bytes)
    contract_path = _write_app_export_contract(
        tmp_path / "app_export_contract.json",
        entry_module="main_molt",
        source=(
            "def init():\n    return 1\n"
            "def ocr_tokens():\n    return 2\n"
            "def _private_helper():\n    return 3\n"
        ),
        symbols=[
            ("init", "main_molt__init"),
            ("ocr_tokens", "main_molt__ocr_tokens"),
            ("_private_helper", "main_molt___private_helper"),
        ],
    )

    captured_cmds: list[list[str]] = []

    def fake_run(cmd, **kwargs):
        captured_cmds.append(list(cmd))
        emitted = output_bytes
        if cmd and cmd[0] == "wasm-ld":
            output_flag = cmd.index("-o")
            emitted = Path(cmd[output_flag + 2]).read_bytes()
        _write_wasm_ld_output(cmd, emitted)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        app_export_contract_path=contract_path,
    )
    assert rc == 0
    cmd = next(cmd for cmd in captured_cmds if cmd and cmd[0] == "wasm-ld")
    assert (
        f"--export={wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__init" in cmd
    )
    assert (
        f"--export={wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt__ocr_tokens"
        in cmd
    )
    assert (
        f"--export={wasm_link_format._OUTPUT_EXPORT_ALIAS_PREFIX}main_molt___private_helper"
        not in cmd
    )
    assert "main_molt___private_helper" not in wasm_link_format._collect_exports(
        linked.read_bytes()
    )


def test_run_wasm_ld_repairs_linked_host_init_export(
    tmp_path: Path, monkeypatch
) -> None:
    write_varuint = wasm_link_format._write_varuint

    def _module(*, include_host_init_export: bool) -> bytes:
        sections: list[tuple[int, bytes]] = []
        type_payload = bytearray()
        type_payload.extend(write_varuint(1))
        type_payload.append(0x60)
        type_payload.extend(write_varuint(0))
        type_payload.extend(write_varuint(0))
        sections.append((1, bytes(type_payload)))

        func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
        sections.append((3, bytes(func_payload)))

        exports: list[tuple[str, int]] = [("molt_main", 1)]
        if include_host_init_export:
            exports.insert(0, ("molt_host_init", 0))
        export_payload = bytearray()
        export_payload.extend(write_varuint(len(exports)))
        for name, index in exports:
            export_payload.extend(wasm_link_format._write_string(name))
            export_payload.append(0x00)
            export_payload.extend(write_varuint(index))
        sections.append((7, bytes(export_payload)))

        code_payload = bytearray()
        code_payload.extend(write_varuint(2))
        for _ in range(2):
            code_payload.extend(write_varuint(2))
            code_payload.append(0x00)
            code_payload.append(0x0B)
        sections.append((10, bytes(code_payload)))

        linking_payload = wasm_link_format._build_linking_payload(
            2,
            [
                (
                    SYMTAB_SUBSECTION_ID,
                    _build_symbol_subsection(
                        [
                            _function_symbol_entry(
                                flags=FLAG_BINDING_GLOBAL
                                | wasm_link_format.FLAG_EXPLICIT_NAME
                                | FLAG_EXPORTED
                                | FLAG_NO_STRIP,
                                index=0,
                                name="molt_host_init",
                            ),
                            _function_symbol_entry(
                                flags=FLAG_BINDING_GLOBAL
                                | wasm_link_format.FLAG_EXPLICIT_NAME
                                | FLAG_EXPORTED
                                | FLAG_NO_STRIP,
                                index=1,
                                name="molt_main",
                            ),
                        ]
                    ),
                )
            ],
        )
        sections.append(
            (0, wasm_link_format._build_custom_section("linking", linking_payload))
        )
        return wasm_link_operations.build_sections(sections)

    output_bytes = _module(include_host_init_export=True)
    linked_without_host_init = _module(include_host_init_export=False)
    runtime = tmp_path / "runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(output_bytes)
    output.write_bytes(output_bytes)

    def fake_run(cmd, **_kwargs):
        _write_wasm_ld_output(cmd, linked_without_host_init)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 0
    exports = wasm_link_format._collect_function_exports(linked.read_bytes())
    assert "molt_host_init" in exports


def test_call_indirect_symbol_discovery_does_not_require_wasm_tools(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime = tmp_path / "runtime_reloc.wasm"
    runtime.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    index=3,
                    name="_ZN4molt19molt_call_indirect1317hfeedfaceE",
                ),
                _function_symbol_entry(
                    flags=wasm_link_format.FLAG_UNDEFINED
                    | wasm_link_format.FLAG_EXPLICIT_NAME,
                    index=4,
                    name="_ZN4molt19molt_call_indirect9917hfeedfaceE",
                ),
            ]
        )
    )
    output = tmp_path / "output.wasm"
    output.write_bytes(
        _module_with_linking_symbols(
            [
                _function_symbol_entry(
                    flags=FLAG_BINDING_GLOBAL
                    | wasm_link_format.FLAG_EXPLICIT_NAME
                    | FLAG_EXPORTED,
                    index=41,
                    name="molt_call_indirect13",
                ),
                _function_symbol_entry(
                    flags=FLAG_BINDING_GLOBAL
                    | wasm_link_format.FLAG_EXPLICIT_NAME
                    | FLAG_EXPORTED,
                    index=42,
                    name="molt_call_indirect99",
                ),
            ]
        )
    )
    mangled = wasm_link_command._find_call_indirect_mangled(
        runtime, facts_provider=_facts_provider
    )
    output_symbols = wasm_link_command._find_output_call_indirect_symbol(
        output, facts_provider=_facts_provider
    )

    assert mangled == {
        "molt_call_indirect13": "_ZN4molt19molt_call_indirect1317hfeedfaceE"
    }
    assert "molt_call_indirect99" not in output_symbols
    assert output_symbols["molt_call_indirect13"] == (
        41,
        FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME | FLAG_EXPORTED,
    )


def test_run_wasm_ld_split_runtime_preserves_old_outputs_if_linked_validation_fails(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_err_pending")
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    split_dir.mkdir()
    linked.write_bytes(b"old-linked")
    (split_dir / "app.wasm").write_bytes(b"old-app")
    (split_dir / "molt_runtime.wasm").write_bytes(b"old-runtime")

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: False,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_optimize_split_app_module", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=True,
        split_output_dir=split_dir,
    )

    assert rc == 1
    assert linked.read_bytes() == b"old-linked"
    assert (split_dir / "app.wasm").read_bytes() == b"old-app"
    assert (split_dir / "molt_runtime.wasm").read_bytes() == b"old-runtime"


def test_run_wasm_ld_preserves_old_output_if_linked_validation_fails(
    tmp_path: Path,
    monkeypatch,
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_err_pending")
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    linked.write_bytes(b"old-linked")

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: False,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )

    rc = _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked)

    assert rc == 1
    assert linked.read_bytes() == b"old-linked"


def test_run_wasm_ld_enabled_then_disabled_retires_optimizer_attestation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_err_pending")
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    optimizer = (tmp_path / "wasm-opt").resolve()
    optimizer_identity = _wasm_optimizer_identity(
        optimizer,
        version="wasm-opt version 130 (version_130)",
    )
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    optimizer.write_bytes(b"optimizer")

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    def fake_optimize(
        path: Path,
        *,
        attestation: dict[str, object],
        **_kwargs: object,
    ) -> bool:
        policy = wasm_link_optimizer_policy.wasm_link_policy("Oz")
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        attestation.update(
            {
                "ok": True,
                "binaryen_version": "wasm-opt version 130 (version_130)",
                "wasm_opt_sha256": "a" * 64,
                "optimization_level": policy.level,
                "optimization_converge": policy.converge,
                "optimization_apply_level": policy.apply_level,
                "optimization_preserve_debug": False,
                "optimization_extra_passes": list(policy.extra_passes),
                "pipeline": list(policy.pipeline),
                "optimizer_input_sha256": digest,
                "optimizer_output_sha256": digest,
            }
        )
        return True

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(
        wasm_link_validation,
        "_validate_linked",
        lambda _p, **_kwargs: True,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_run_wasm_opt_via_optimize", fake_optimize
    )
    monkeypatch.setattr(
        wasm_link_pipeline,
        "wasm_optimizer_invocation_identity",
        lambda: optimizer_identity,
    )

    assert (
        _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked, optimize=True)
        == 0
    )
    attestation_path = wasm_link_pipeline.wasm_optimizer_attestation_path(linked)
    assert attestation_path.exists()
    published_attestation = json.loads(attestation_path.read_text(encoding="utf-8"))
    assert "wasm_opt_path" not in published_attestation
    assert "cache_hit" not in published_attestation

    assert _run_wasm_ld_with_rust_facts("wasm-ld", runtime, output, linked) == 0
    assert not attestation_path.exists()


@pytest.mark.parametrize("preserve_debug_sections", (False, True))
def test_run_wasm_ld_split_runtime_reuses_one_optimizer_identity_and_publishes_atomically(
    tmp_path: Path,
    monkeypatch,
    preserve_debug_sections: bool,
) -> None:
    debug_sections = [
        (0, wasm_link_format._build_custom_section(".debug_info", b"debug")),
        (0, wasm_link_format._build_custom_section("name", b"names")),
    ]
    runtime_bytes = wasm_link_operations.build_sections(
        [
            *wasm_link_operations.parse_sections(
                _build_exported_runtime_module("molt_err_pending")
            ),
            *debug_sections,
        ]
    )
    output_bytes = wasm_link_operations.build_sections(
        [
            *wasm_link_operations.parse_sections(_module_with_linking_symbols([])),
            *debug_sections,
        ]
    )
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    split_dir.mkdir()
    linked.write_bytes(b"old-linked")
    app_wasm = split_dir / "app.wasm"
    rt_wasm = split_dir / "molt_runtime.wasm"
    app_wasm.write_bytes(b"old-app")
    rt_wasm.write_bytes(b"old-runtime")
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    optimizer_identity = _wasm_optimizer_identity(
        (tmp_path / "wasm-opt").resolve(),
        version="wasm-opt version 130 (version_130)",
    )
    identity_resolutions = 0
    optimizer_identities: list[object] = []
    validate_seen: list[Path] = []
    split_validate_seen: list[tuple[Path, Path]] = []

    def resolve_optimizer_identity():  # type: ignore[no-untyped-def]
        nonlocal identity_resolutions
        identity_resolutions += 1
        return optimizer_identity

    def optimize_with_identity(
        path: Path,
        *,
        attestation: dict[str, object],
        optimizer_identity,
        **_kwargs: object,
    ) -> bool:
        optimizer_identities.append(optimizer_identity)
        policy = wasm_link_optimizer_policy.wasm_link_policy(
            "Oz", preserve_debug=preserve_debug_sections
        )
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        attestation.update(
            {
                "ok": True,
                "binaryen_version": optimizer_identity.binaryen_version,
                "wasm_opt_sha256": optimizer_identity.sha256,
                "optimization_level": policy.level,
                "optimization_converge": policy.converge,
                "optimization_apply_level": policy.apply_level,
                "optimization_preserve_debug": preserve_debug_sections,
                "optimization_extra_passes": list(policy.extra_passes),
                "pipeline": list(policy.pipeline),
                "optimizer_input_sha256": digest,
                "optimizer_output_sha256": digest,
            }
        )
        return True

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    def validate_linked(path: Path, **_kwargs) -> bool:
        validate_seen.append(path)
        assert path != linked
        assert linked.read_bytes() == b"old-linked"
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    def validate_split(app_stage: Path, rt_stage: Path, **_kwargs) -> bool:
        split_validate_seen.append((app_stage, rt_stage))
        assert app_stage != app_wasm
        assert rt_stage != rt_wasm
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link_validation, "_validate_linked", validate_linked)
    monkeypatch.setattr(
        wasm_link_validation, "_validate_split_runtime_outputs", validate_split
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )
    monkeypatch.setattr(
        wasm_link_pipeline,
        "wasm_optimizer_invocation_identity",
        resolve_optimizer_identity,
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_run_wasm_opt_via_optimize",
        optimize_with_identity,
    )

    def tree_shake_runtime(data: bytes, required_exports, **kwargs) -> bytes:  # type: ignore[no-untyped-def]
        return wasm_link_optimize._post_link_optimize(
            data,
            preserve_exports=set(required_exports)
            | wasm_link_format._ESSENTIAL_EXPORTS,
            preserve_debug=kwargs["preserve_debug"],
            facts_provider=kwargs["facts_provider"],
        )

    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        tree_shake_runtime,
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        optimize=True,
        split_runtime=True,
        split_output_dir=split_dir,
        preserve_debug_sections=preserve_debug_sections,
        phase_timings_file=tmp_path / "phase_timings.json",
    )

    assert rc == 0
    assert identity_resolutions == 1
    assert len(optimizer_identities) == 2
    assert all(identity is optimizer_identity for identity in optimizer_identities)
    phase_timings = json.loads(
        (tmp_path / "phase_timings.json").read_text(encoding="utf-8")
    )
    assert phase_timings["wasm_optimizer_identity_resolutions"] == 1
    assert phase_timings["wasm_optimizer_identity_wall_ms"] >= 0
    assert validate_seen
    assert split_validate_seen
    assert linked.read_bytes() != b"old-linked"
    expected_app = _with_test_link_facts(
        wasm_link_operations.strip_publication_sections(
            output_bytes,
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        ),
        role="app",
    )
    expected_runtime = _with_test_link_facts(
        wasm_link_operations.strip_publication_sections(
            runtime_bytes,
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        ),
        role="runtime",
    )
    assert (
        wasm_link_operations.strip_publication_sections(
            app_wasm.read_bytes(),
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        )
        == expected_app
    )
    assert (
        wasm_link_operations.strip_publication_sections(
            rt_wasm.read_bytes(),
            final_artifact=True,
            preserve_debug=preserve_debug_sections,
        )
        == expected_runtime
    )
    expected_debug_names = (".debug_info", "name") if preserve_debug_sections else ()

    def published_debug_names(path: Path) -> tuple[str, ...]:
        names = wasm_artifact.wasm_custom_section_names(path.read_bytes())
        assert "linking" not in names
        return tuple(
            name for name in names if wasm_artifact.is_wasm_debug_custom_section(name)
        )

    assert published_debug_names(linked) == expected_debug_names
    assert published_debug_names(app_wasm) == expected_debug_names
    assert published_debug_names(rt_wasm) == expected_debug_names
    durable_optimizer = json.loads(
        wasm_link_pipeline.wasm_optimizer_attestation_path(linked).read_text(
            encoding="utf-8"
        )
    )
    size_attestation = json.loads(
        (split_dir / "wasm_size_attestation.json").read_text(encoding="utf-8")
    )
    app_optimizer = json.loads(
        wasm_link_pipeline.wasm_optimizer_attestation_path(app_wasm).read_text(
            encoding="utf-8"
        )
    )
    assert size_attestation["optimizer"] == app_optimizer
    assert (
        durable_optimizer["published_output_sha256"]
        == hashlib.sha256(linked.read_bytes()).hexdigest()
    )
    assert (
        app_optimizer["published_output_sha256"]
        == hashlib.sha256(app_wasm.read_bytes()).hexdigest()
    )
    assert (
        not {
            "wasm_opt_path",
            "cache_hit",
            "wasm_opt_wall_ms",
            "wasm_opt_peak_rss_kb",
            "wasm_opt_peak_total_rss_kb",
        }
        & durable_optimizer.keys()
    )


def test_wasm_link_allows_ref_null_element_expr() -> None:
    write_varuint = wasm_link_format._write_varuint
    payload = bytearray()
    payload.extend(write_varuint(1))
    payload.extend(write_varuint(0x04))
    payload.extend(b"\x41\x00\x0b")
    payload.append(0x70)
    payload.extend(write_varuint(1))
    payload.append(0xD0)  # ref.null
    payload.append(0x70)  # funcref
    payload.append(0x0B)
    _declared, error = wasm_link_format._parse_element_payload(bytes(payload))
    assert error is None


def test_strip_internal_exports_preserves_user_module_exports() -> None:
    write_varuint = wasm_link_format._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(3))
    table_ref = "__molt_table_ref_7"
    export_payload.extend(wasm_link_format._write_string(table_ref))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link_format._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(0))
    export_payload.extend(wasm_link_format._write_string("main_molt__ocr_tokens"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(1))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link_operations.build_sections(sections)
    updated = wasm_link_edit._strip_internal_exports(
        data, preserve_exports={"main_molt__ocr_tokens"}
    )
    exports = wasm_link_format._collect_function_exports(updated or data)
    assert table_ref not in exports
    assert "molt_main" in exports
    assert "main_molt__ocr_tokens" in exports


def test_strip_internal_exports_keeps_linked_host_call_helpers() -> None:
    data = _build_exported_runtime_module_many(
        [
            "molt_main",
            "molt_scratch_alloc",
            "molt_scratch_free",
            "molt_bytes_from_bytes",
            "molt_string_from_bytes",
            "molt_list_builder_new",
            "molt_list_builder_append",
            "molt_list_builder_finish",
            "molt_object_repr",
            "molt_len",
            "molt_index",
            "molt_profile_dump",
            "dead_internal_export",
        ]
    )
    updated = wasm_link_edit._strip_internal_exports(data)
    exports = wasm_link_format._collect_function_exports(updated or data)
    assert "molt_scratch_alloc" in exports
    assert "molt_scratch_free" in exports
    assert "molt_bytes_from_bytes" in exports
    assert "molt_string_from_bytes" in exports
    assert "molt_list_builder_new" in exports
    assert "molt_list_builder_append" in exports
    assert "molt_list_builder_finish" in exports
    assert "molt_object_repr" in exports
    assert "molt_len" in exports
    assert "molt_index" in exports
    assert "molt_profile_dump" in exports
    assert "dead_internal_export" not in exports


def test_strip_internal_exports_dedupes_duplicate_export_names() -> None:
    write_varuint = wasm_link_format._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(2) + write_varuint(0) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(4))
    table_ref = "__molt_table_ref_7"
    for name, index in (
        (table_ref, 0),
        (table_ref, 1),
        ("molt_main", 0),
        ("molt_main", 1),
    ):
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(0x00)
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link_operations.build_sections(sections)
    updated = wasm_link_edit._strip_internal_exports(data)
    exports = wasm_link_format._collect_function_exports(updated or data)
    assert table_ref not in exports
    assert list(name for name in exports if name == "molt_main") == ["molt_main"]


def test_required_linked_table_min_respects_final_active_elements() -> None:
    write_varuint = wasm_link_format._write_varuint

    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    import_payload = bytearray()
    import_payload.extend(write_varuint(1))
    import_payload.extend(wasm_link_format._write_string("env"))
    import_payload.extend(wasm_link_format._write_string("__indirect_function_table"))
    import_payload.append(0x01)
    import_payload.append(0x70)
    import_payload.extend(write_varuint(0))
    import_payload.extend(write_varuint(10))
    sections.append((2, bytes(import_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, bytes(func_payload)))

    element_payload = (
        write_varuint(1)
        + b"\x00\x41"
        + write_varuint(20)
        + b"\x0b"
        + write_varuint(1)
        + write_varuint(0)
    )
    sections.append((9, element_payload))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    data = wasm_link_operations.build_sections(sections)

    facts = {"callable_table_entries": [[20, 0, 0, 0]]}
    assert wasm_link_edit._table_import_min(data, facts_provider=_facts_provider) == 10
    assert (
        wasm_link_edit._required_linked_table_min(
            data, 5, facts, facts_provider=_facts_provider
        )
        == 21
    )
    updated = wasm_link_edit._rewrite_table_import_min(
        data,
        wasm_link_edit._required_linked_table_min(
            data, 5, facts, facts_provider=_facts_provider
        ),
    )
    assert updated is not None
    assert (
        wasm_link_edit._table_import_min(updated, facts_provider=_facts_provider) == 21
    )


def test_neutralize_dead_element_entries_skips_modules_with_call_indirect() -> None:
    write_varuint = wasm_link_format._write_varuint
    sections = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))

    func_payload = write_varuint(1) + write_varuint(0)
    sections.append((3, func_payload))

    table_payload = bytearray()
    table_payload.extend(write_varuint(1))
    table_payload.append(0x70)
    table_payload.extend(write_varuint(0))
    table_payload.extend(write_varuint(1))
    sections.append((4, bytes(table_payload)))

    element_payload = bytearray()
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    element_payload.extend(b"\x41\x00\x0b")
    element_payload.extend(write_varuint(1))
    element_payload.extend(write_varuint(0))
    sections.append((9, bytes(element_payload)))

    code_payload = bytearray()
    body = bytearray()
    body.extend(write_varuint(0))  # local decl count
    body.extend(b"\x41\x00")  # i32.const 0
    body.extend(b"\x11\x00\x00")  # call_indirect type 0 table 0
    body.append(0x0B)  # end
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(len(body)))
    code_payload.extend(body)
    sections.append((10, bytes(code_payload)))

    data = wasm_link_operations.build_sections(sections)
    facts = {
        "reachable_function_indices": [0],
        "active_function_elements": [[0, 0, 0]],
        "reachable_dynamic_dispatch": True,
        "reachable_function_reference_dispatch": False,
        "exported_table_indices": [],
        "table_mutations": [],
    }
    assert wasm_link_optimize._neutralize_dead_element_entries(data, facts) is None


def test_post_link_optimization_preserves_user_data_segment_bytes() -> None:
    write_varuint = wasm_link_format._write_varuint
    sections = []

    memory_payload = bytearray()
    memory_payload.extend(write_varuint(1))
    memory_payload.append(0x00)
    memory_payload.extend(write_varuint(1))
    sections.append((5, bytes(memory_payload)))

    path_and_adjacent = b"/Users/alice/project/tmp/class_method_probe.pyf__name__hi"
    second_segment = b"/rustc/0123456789abcdef0123/library/core/src/panic.rs"

    data_payload = bytearray()
    data_payload.extend(write_varuint(2))
    for offset, raw in ((0, path_and_adjacent), (128, second_segment)):
        data_payload.append(0x00)
        data_payload.extend(b"\x41")
        data_payload.extend(write_varuint(offset))
        data_payload.extend(b"\x0b")
        data_payload.extend(write_varuint(len(raw)))
        data_payload.extend(raw)
    sections.append((11, bytes(data_payload)))

    data = wasm_link_operations.build_sections(sections)
    scan_calls = 0

    def facts_provider(payload: bytes) -> dict[str, object]:
        nonlocal scan_calls
        scan_calls += 1
        return _rust_facts_fixture(payload)

    optimized = wasm_link_optimize._post_link_optimize(
        data,
        reference_data=None,
        preserve_reference_exports=False,
        facts_provider=facts_provider,
    )

    assert optimized == data
    assert scan_calls == 1
    assert _parse_data_segments(optimized) == [path_and_adjacent, second_segment]


# ---------------------------------------------------------------------------
# Allowlist validation
# ---------------------------------------------------------------------------


def _parse_allowlist(path: Path) -> set[str]:
    lines = path.read_text(encoding="utf-8").splitlines()
    return {
        line.strip()
        for line in lines
        if line.strip() and not line.strip().startswith("#")
    }


def test_allowlist_file_exists():
    """The WASI allowlist must exist and contain the expected symbols."""
    allowlist = (
        Path(__file__).resolve().parents[1] / "tools" / "wasm_allowed_imports.txt"
    )
    assert allowlist.exists(), f"Missing allowlist: {allowlist}"
    symbols = _parse_allowlist(allowlist)
    from molt._wasm_abi_generated import WASM_CALL_INDIRECT_IMPORTS

    # Must contain core WASI symbols
    assert "fd_write" in symbols
    assert "proc_exit" in symbols
    assert "__indirect_function_table" in symbols
    # Must contain indirect call trampolines
    assert set(WASM_CALL_INDIRECT_IMPORTS) <= symbols
    # Must NOT contain molt_runtime namespace symbols (those are resolved by linking),
    # except for serialization/compression builtins that are direct WASM imports.
    _ALLOWED_MOLT_PREFIXES = (
        "molt_call_indirect",
        "molt_cbor_",
        "molt_msgpack_",
        "molt_deflate_",
        "molt_inflate_",
    )
    runtime_syms = {
        s
        for s in symbols
        if s.startswith("molt_")
        and not any(s.startswith(p) for p in _ALLOWED_MOLT_PREFIXES)
    }
    assert runtime_syms == set(), (
        f"Unexpected molt_runtime symbols in allowlist: {runtime_syms}"
    )


def test_native_object_link_allowlist_includes_generated_external_imports(tmp_path):
    base = tmp_path / "base_allowlist.txt"
    base.write_text("fd_write\n", encoding="utf-8")
    native = tmp_path / "extension.molt.wasm"
    native.write_bytes(b"\0asm\x01\0\0\0")

    with OwnedTemporaryDirectory() as raw_tmp:
        temp_dir = type("_Tmp", (), {"name": raw_tmp})()
        assert (
            wasm_link_native_inputs._compose_wasm_ld_allowlist(
                base_allowlist=base,
                native_link_requirements=_native_link_requirements(),
                temp_dir=temp_dir,
            )
            == base
        )

        composed = wasm_link_native_inputs._compose_wasm_ld_allowlist(
            base_allowlist=base,
            native_link_requirements=_native_link_requirements(native),
            temp_dir=temp_dir,
        )

        symbols = _parse_allowlist(composed)
        assert "fd_write" in symbols
        assert "__cpp_exception" in symbols
        assert "malloc" in symbols
        assert "__trunctfdf2" not in symbols
        assert "__cpp_exception" not in _parse_allowlist(base)


# --- Split-runtime CPython-ABI data-symbol aliasing ------------------------
#
# A native extension (numpy/scipy) references the runtime's canonical
# singletons/type/exception objects (Py_None, Py_False, PyExc_*, Py*_Type) as
# *undefined data symbols*. In a split-runtime build the app links against
# imported memory and must resolve those references to the DEPLOY runtime's
# single copy â€” otherwise the extension links its own zero-initialized
# duplicate and pointer-identity bridges (pyobj_to_handle) fail. The deploy
# runtime publishes each address as an exported i32 global (wasm-ld's encoding
# for --export-if-defined of a defined data symbol); the aliaser reads those
# addresses. See tools/wasm_link_runtime_data.py::_split_runtime_data_alias_object.


def _build_data_address_export_runtime(addresses: dict[str, int]) -> bytes:
    """Synthetic deploy runtime exporting each data symbol as an immutable i32
    global whose init value is the symbol's linear-memory address."""
    write_varuint = wasm_link_format._write_varuint
    names = list(addresses)
    sections: list[tuple[int, bytes]] = []

    # Address-bearing exports are valid only when they point inside a declared
    # linear memory.  Model the wasm-ld runtime shape instead of relying on a
    # parser-only synthetic module with no memory authority.
    highest_address = max(addresses.values(), default=0)
    memory_pages = max(1, highest_address // 0x10000 + 1)
    sections.append((5, write_varuint(1) + b"\x00" + write_varuint(memory_pages)))

    global_payload = bytearray()
    global_payload.extend(write_varuint(len(names)))
    for name in names:
        global_payload.append(0x7F)  # i32
        global_payload.append(0x00)  # immutable
        global_payload.append(0x41)  # i32.const
        address = addresses[name]
        signed_address = address if address < 0x8000_0000 else address - 0x1_0000_0000
        global_payload.extend(wasm_link_runtime_data._write_sleb128(signed_address))
        global_payload.append(0x0B)  # end
    sections.append((6, bytes(global_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(len(names)))
    for index, name in enumerate(names):
        export_payload.extend(wasm_link_format._write_string(name))
        export_payload.append(0x03)  # global export
        export_payload.extend(write_varuint(index))
    sections.append((7, bytes(export_payload)))

    return wasm_link_operations.build_sections(sections)


def _build_single_data_export_shape(
    name: str,
    *,
    value_type: int = 0x7F,
    mutable: int = 0,
    initializer: bytes | None = None,
    export_kind: int = 0x03,
) -> bytes:
    write_varuint = wasm_link_format._write_varuint
    initializer = initializer or (b"\x41" + write_varuint(0x2E0000) + b"\x0b")
    global_payload = write_varuint(1) + bytes((value_type, mutable)) + initializer
    export_payload = (
        write_varuint(1)
        + wasm_link_format._write_string(name)
        + bytes((export_kind,))
        + write_varuint(0)
    )
    return wasm_link_operations.build_sections(
        [(6, global_payload), (7, export_payload)]
    )


def _add_data_address_global_exports(module: bytes, addresses: dict[str, int]) -> bytes:
    """Return *module* with an added i32 global (init = address) exported under
    each name â€” the wasm-ld shape for --export-if-defined of a defined data
    symbol. Appends to any existing global/export sections."""
    write_varuint = wasm_link_format._write_varuint
    sections = wasm_link_operations.parse_sections(module)

    existing_global_count = 0
    for section_id, payload in sections:
        if section_id == 6:
            existing_global_count, _ = wasm_link_format._read_varuint(payload, 0)
            break

    names = list(addresses)
    new_globals = bytearray()
    for name in names:
        new_globals.append(0x7F)  # i32
        new_globals.append(0x00)  # immutable
        new_globals.append(0x41)  # i32.const
        new_globals.extend(write_varuint(addresses[name]))
        new_globals.append(0x0B)  # end

    new_export_entries = bytearray()
    for offset, name in enumerate(names):
        new_export_entries.extend(wasm_link_format._write_string(name))
        new_export_entries.append(0x03)  # global export
        new_export_entries.extend(write_varuint(existing_global_count + offset))

    rebuilt: list[tuple[int, bytes]] = []
    saw_global = False
    saw_export = False
    for section_id, payload in sections:
        if section_id == 6:
            saw_global = True
            count, offset = wasm_link_format._read_varuint(payload, 0)
            merged = bytearray()
            merged.extend(write_varuint(count + len(names)))
            merged.extend(payload[offset:])
            merged.extend(new_globals)
            rebuilt.append((6, bytes(merged)))
        elif section_id == 7:
            saw_export = True
            count, offset = wasm_link_format._read_varuint(payload, 0)
            merged = bytearray()
            merged.extend(write_varuint(count + len(names)))
            merged.extend(payload[offset:])
            merged.extend(new_export_entries)
            rebuilt.append((7, bytes(merged)))
        else:
            rebuilt.append((section_id, payload))
    if not saw_global:
        global_section = bytearray()
        global_section.extend(write_varuint(len(names)))
        global_section.extend(new_globals)
        rebuilt.append((6, bytes(global_section)))
    if not saw_export:
        export_section = bytearray()
        export_section.extend(write_varuint(len(names)))
        export_section.extend(new_export_entries)
        rebuilt.append((7, bytes(export_section)))
    return wasm_link_operations.build_sections(rebuilt)


def _build_undefined_data_symbol_object(names: list[str]) -> bytes:
    """Synthetic relocatable native object with the given undefined data
    symbols in its linking symtab (the shape numpy's *.molt.wasm carries)."""
    symbol_entries: list[bytes] = []
    for name in names:
        entry = bytearray()
        entry.append(SYMBOL_KIND_DATA)
        entry.extend(
            wasm_link_format._write_varuint(
                wasm_link_format.FLAG_EXPLICIT_NAME | wasm_link_format.FLAG_UNDEFINED
            )
        )
        entry.extend(wasm_link_format._write_string(name))
        symbol_entries.append(bytes(entry))
    symbol_payload = wasm_link_format._write_varuint(len(symbol_entries)) + b"".join(
        symbol_entries
    )
    linking = wasm_link_format._build_custom_section(
        "linking",
        wasm_link_format._build_linking_payload(
            2, [(SYMTAB_SUBSECTION_ID, symbol_payload)]
        ),
    )
    return wasm_link_operations.build_sections([(0, linking)])


def _build_defined_data_symbol_object(sizes: dict[str, int]) -> bytes:
    entries = [
        _data_symbol_entry(
            flags=wasm_link_format.FLAG_EXPLICIT_NAME,
            name=name,
            segment_index=index,
            offset=index * 16,
            size=size,
        )
        for index, (name, size) in enumerate(sizes.items())
    ]
    return _module_with_linking_symbols(entries)


def _alias_segment_addresses(alias_bytes: bytes) -> dict[int, int]:
    """Map data-segment index -> i32.const address for an alias object."""
    addresses: dict[int, int] = {}
    for section_id, payload in wasm_link_operations.parse_sections(alias_bytes):
        if section_id != 11:
            continue
        offset = 0
        count, offset = wasm_link_format._read_varuint(payload, offset)
        for index in range(count):
            flags, offset = wasm_link_format._read_varuint(payload, offset)
            if flags == 2:
                _memory_index, offset = wasm_link_format._read_varuint(payload, offset)
            address, offset = wasm_link_runtime_data._read_const_i32_init_expr(
                payload, offset
            )
            size, offset = wasm_link_format._read_varuint(payload, offset)
            offset += size
            addresses[index] = address & 0xFFFF_FFFF
    return addresses


@pytest.mark.parametrize(
    ("bits", "value"),
    [
        (32, -(1 << 31)),
        (32, (1 << 31) - 1),
        (33, -(1 << 32)),
        (33, (1 << 32) - 1),
        (64, -(1 << 63)),
        (64, (1 << 63) - 1),
    ],
)
def test_wasm_signed_leb128_round_trips_boundary_values(bits: int, value: int) -> None:
    encoded = wasm_artifact._encode_wasm_varint(value, bits)
    decoded, offset = wasm_artifact._read_wasm_varint(encoded, 0, bits)
    assert (decoded, offset) == (value, len(encoded))


def test_wasm_signed_leb128_rejects_overlong_width() -> None:
    with pytest.raises(ValueError, match="varint too large"):
        wasm_artifact._read_wasm_varint(b"\x80\x80\x80\x80\x80\x00", 0, 32)


def test_runtime_exported_data_symbol_addresses_reads_global_exports() -> None:
    runtime = _build_data_address_export_runtime(
        {
            "Py_None": 0x2E1688,
            "_Py_FalseStruct": 0x2E1680,
            "PyExc_ValueError": 0x2E1500,
        }
    )
    addresses = wasm_link_runtime_data._runtime_exported_data_symbol_addresses(
        runtime, facts_provider=_facts_provider
    )
    assert addresses == {
        "Py_None": 0x2E1688,
        "_Py_FalseStruct": 0x2E1680,
        "PyExc_ValueError": 0x2E1500,
    }


def test_runtime_exported_data_symbol_addresses_normalizes_full_split_family() -> None:
    canonical_symbols = wasm_link_runtime_data.wasm_cpython_abi_data_symbol_names()
    assert "PyType_Type" in canonical_symbols
    expected = {
        name: 0x200000 + index * 8 for index, name in enumerate(canonical_symbols)
    }
    split_exports: dict[str, int] = {}
    for name, address in expected.items():
        split_name = wasm_split_runtime_export_name_for_import(name)
        assert split_name is not None, name
        split_exports[split_name] = address

    runtime = _build_data_address_export_runtime(split_exports)
    assert (
        wasm_link_runtime_data._runtime_exported_data_symbol_addresses(
            runtime, facts_provider=_facts_provider
        )
        == expected
    )


def test_runtime_exported_data_symbol_addresses_rejects_alias_drift() -> None:
    runtime = _build_data_address_export_runtime(
        {"PyType_Type": 0x2E0000, "molt_PyType_Type": 0x2F0000}
    )
    with pytest.raises(ValueError, match="conflicting addresses.*PyType_Type"):
        wasm_link_runtime_data._runtime_exported_data_symbol_addresses(
            runtime, facts_provider=_facts_provider
        )


def test_runtime_export_validation_counts_data_address_globals(
    tmp_path: Path,
) -> None:
    from molt.cli.runtime_wasm_validation import (
        _runtime_wasm_missing_exports,
        _split_runtime_wasm_missing_exports,
    )

    canonical = tmp_path / "runtime-reloc.wasm"
    canonical.write_bytes(_build_data_address_export_runtime({"PyType_Type": 0x2E0000}))
    split = tmp_path / "runtime-shared.wasm"
    split.write_bytes(
        _build_data_address_export_runtime({"molt_PyType_Type": 0x2E0000})
    )

    assert _runtime_wasm_missing_exports(canonical, {"PyType_Type"}) == set()
    assert _split_runtime_wasm_missing_exports(split, {"PyType_Type"}) == set()


@pytest.mark.parametrize(
    ("split", "export_name"),
    [(False, "PyType_Type"), (True, "molt_PyType_Type")],
)
def test_runtime_export_validation_rejects_function_impersonating_data(
    tmp_path: Path, split: bool, export_name: str
) -> None:
    from molt.cli.runtime_wasm_validation import (
        _runtime_wasm_missing_exports,
        _split_runtime_wasm_missing_exports,
    )

    runtime = tmp_path / "runtime.wasm"
    runtime.write_bytes(_build_exported_runtime_module(export_name))
    missing = (
        _split_runtime_wasm_missing_exports(runtime, {"PyType_Type"})
        if split
        else _runtime_wasm_missing_exports(runtime, {"PyType_Type"})
    )
    assert missing


@pytest.mark.parametrize(
    ("split", "export_name"),
    [(False, "molt_add"), (True, "molt_add")],
)
def test_runtime_export_validation_rejects_global_impersonating_function(
    tmp_path: Path, split: bool, export_name: str
) -> None:
    from molt.cli.runtime_wasm_validation import (
        _runtime_wasm_missing_exports,
        _split_runtime_wasm_missing_exports,
    )

    runtime = tmp_path / "runtime.wasm"
    runtime.write_bytes(_build_single_data_export_shape(export_name))
    missing = (
        _split_runtime_wasm_missing_exports(runtime, {"molt_add"})
        if split
        else _runtime_wasm_missing_exports(runtime, {"molt_add"})
    )
    assert missing


@pytest.mark.parametrize(
    ("kwargs", "diagnostic"),
    [
        ({"mutable": 1}, "immutable"),
        ({"value_type": 0x7E, "initializer": b"\x42\x01\x0b"}, "i32 type"),
        ({"initializer": b"\x23\x00\x0b"}, "i32.const initializer"),
        ({"initializer": b"\x41\x80\x00\x0b"}, "canonical i32.const initializer"),
    ],
)
def test_runtime_data_address_exports_fail_closed_on_noncanonical_global_shape(
    kwargs: dict[str, object], diagnostic: str
) -> None:
    runtime = _build_single_data_export_shape("PyType_Type", **kwargs)
    with pytest.raises(ValueError, match=diagnostic):
        wasm_link_runtime_data._runtime_exported_data_symbol_addresses(
            runtime, facts_provider=_facts_provider
        )


def test_split_runtime_data_alias_points_at_deploy_runtime_addresses(
    tmp_path: Path,
) -> None:
    # A real CPython-ABI data-symbol subset numpy references undefined.
    names = ["Py_None", "PyExc_ValueError", "PyList_Type"]
    deploy_addresses = {
        "Py_None": 0x2E1680,
        "PyExc_ValueError": 0x2E1400,
        "PyList_Type": 0x2E0000,
    }
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    deploy_runtime.write_bytes(_build_data_address_export_runtime(deploy_addresses))
    reloc_runtime.write_bytes(
        _build_defined_data_symbol_object({name: 8 for name in names})
    )
    native = tmp_path / "ext.molt.wasm"
    native.write_bytes(_build_undefined_data_symbol_object(names))

    # Precondition: the object surfaces exactly these undefined data symbols.
    assert set(
        wasm_link_runtime_data._undefined_cpython_abi_data_symbols(
            [native], facts_provider=_facts_provider
        )
    ) == set(names)

    with OwnedTemporaryDirectory() as tmp:
        temp_dir = type("_TD", (), {"name": tmp})()
        alias_plan = wasm_link_runtime_data._split_runtime_data_alias_object(
            native_link_requirements=_native_link_requirements(*[native]),
            deploy_runtime=deploy_runtime,
            temp_dir=temp_dir,
            reloc_runtime=reloc_runtime,
            facts_provider=_facts_provider,
        )
        assert alias_plan is not None
        alias_bytes = alias_plan.artifact.read_bytes()

    # Each aliased data symbol must resolve to the deploy runtime's address.
    # The alias emits symtab entries and data segments in the same order, so the
    # Nth defined symbol owns the Nth segment.
    ordered_names = [
        name
        for name, _offset, _size in wasm_link_runtime_data._iter_linking_data_symbols(
            alias_bytes,
            undefined=False,
            facts_provider=_facts_provider,
        )
    ]
    seg_addresses = _alias_segment_addresses(alias_bytes)
    assert set(ordered_names) == set(names)
    for index, name in enumerate(ordered_names):
        assert seg_addresses[index] == deploy_addresses[name], name


def test_split_runtime_data_alias_fails_loud_on_missing_deploy_export(
    tmp_path: Path,
) -> None:
    # Deploy runtime is missing an address global for PyExc_ValueError -> must raise
    # rather than silently emit a wrong/zero address (M34: degrade loudly).
    deploy_runtime = tmp_path / "molt_runtime.wasm"
    reloc_runtime = tmp_path / "molt_runtime_reloc.wasm"
    deploy_runtime.write_bytes(
        _build_data_address_export_runtime({"Py_None": 0x2E1680})
    )
    native = tmp_path / "ext.molt.wasm"
    native.write_bytes(
        _build_undefined_data_symbol_object(["Py_None", "PyExc_ValueError"])
    )
    reloc_runtime.write_bytes(
        _build_defined_data_symbol_object({"Py_None": 8, "PyExc_ValueError": 8})
    )

    with OwnedTemporaryDirectory() as tmp:
        temp_dir = type("_TD", (), {"name": tmp})()
        with pytest.raises(ValueError, match="PyExc_ValueError"):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=_native_link_requirements(*[native]),
                deploy_runtime=deploy_runtime,
                temp_dir=temp_dir,
                reloc_runtime=reloc_runtime,
                facts_provider=_facts_provider,
            )


def test_shared_runtime_exports_cpython_abi_data_symbols_as_globals() -> None:
    # The shared/deploy runtime link surface must publish the canonical
    # CPython-ABI data symbols so the aliaser can read their addresses.
    from molt._wasm_runtime_exports import (
        wasm_cpython_abi_data_symbol_names,
        wasm_runtime_shared_export_link_args,
    )

    data_symbols = wasm_cpython_abi_data_symbol_names()
    assert {"Py_None", "PyExc_ValueError", "PyList_Type"}.issubset(data_symbols)
    args = wasm_runtime_shared_export_link_args()
    for name in ("Py_None", "PyBool_Type", "PyExc_ValueError", "PyList_Type"):
        assert f"--export-if-defined={name}" in args, name


@pytest.mark.parametrize(
    "collision", ["app-input", "runtime-input", "split-role", "deploy-input"]
)
def test_final_link_rejects_original_path_aliases_before_custody(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, collision: str
) -> None:
    runtime = tmp_path / "runtime-input.wasm"
    app = tmp_path / "app-input.wasm"
    deploy = tmp_path / "molt_runtime.wasm"
    for path in (runtime, app, deploy):
        path.write_bytes(b"original input")
    linked = {
        "app-input": app,
        "runtime-input": runtime,
        "split-role": tmp_path / "app.wasm",
        "deploy-input": tmp_path / "linked.wasm",
    }[collision]
    monkeypatch.setattr(
        wasm_link,
        "_snapshot_link_input",
        lambda *args, **kwargs: pytest.fail(
            "alias must fail before input snapshot/link"
        ),
    )
    result = wasm_link._run_wasm_ld(
        "unused-wasm-ld",
        runtime,
        app,
        linked,
        runtime_role="reloc",
        split_runtime=collision in {"split-role", "deploy-input"},
        split_output_dir=tmp_path,
        deploy_runtime_override=deploy,
        wasm_facts_scanner=tmp_path / "unused-scanner",
    )
    assert result == 1
    assert "alias" in capsys.readouterr().err
    assert all(
        path.read_bytes() == b"original input" for path in (runtime, app, deploy)
    )


def _native_link_requirements(
    *paths: Path,
    retained_symbols: tuple[str, ...] = (),
) -> SourceExtensionLinkRequirements:
    return SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        tuple(source_extension_link_file(path) for path in paths),
        retained_symbols,
    )


def _build_compiler_rt_provider_archive() -> bytes:
    symbol = "__trunctfdf2"
    module = wasm_link_format._append_linking_function_symbols(
        _build_exported_function_module(symbol),
        [(symbol, 0, FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME)],
        facts_provider=_facts_provider,
    )
    assert module is not None
    return static_archive_bytes(module)


def test_snapshot_link_input_retries_until_source_is_stable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "output.wasm"
    source.write_bytes(b"partial")
    reads = 0
    original_snapshot = wasm_link.snapshot_stable_regular_file

    def racing_snapshot(path: Path, snapshot: Path, **kwargs):
        nonlocal reads
        if path == source:
            reads += 1
            if reads == 1:
                source.write_bytes(b"complete-molt-main")
                raise wasm_link.StableRegularFileChangedError(
                    "source changed during snapshot"
                )
        return original_snapshot(path, snapshot, **kwargs)

    monkeypatch.setattr(wasm_link, "snapshot_stable_regular_file", racing_snapshot)

    snapshot = wasm_link._snapshot_link_input(
        source,
        tmp_path / "snapshots",
        label="app",
        retry_delay_seconds=0,
        required_prefix=b"complete-molt-main",
    )

    assert snapshot.read_bytes() == b"complete-molt-main"
    assert snapshot != source


def test_snapshot_link_input_rejects_failed_path_attestation_without_retry(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source = tmp_path / "runtime_reloc.wasm"
    source.write_bytes(b"bad-generation")
    attempts = 0

    def accept_path(snapshot: Path) -> bool:
        nonlocal attempts
        attempts += 1
        if attempts == 1:
            source.write_bytes(b"good-generation")
            return False
        return snapshot.read_bytes() == b"good-generation"

    with pytest.raises(
        OSError, match="stable snapshot failed linker metadata preflight"
    ):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="runtime",
            retry_delay_seconds=0,
            accept_path=accept_path,
        )
    assert attempts == 1
    assert not (tmp_path / "snapshots/runtime/runtime_reloc.wasm").exists()


def test_snapshot_link_input_rejects_stable_stripped_restoration_source(
    tmp_path: Path,
) -> None:
    source = tmp_path / "output.wasm"
    source.write_bytes(b"stable-but-stripped")

    with pytest.raises(OSError, match="stable prefix failed linker input contract"):
        wasm_link._snapshot_link_input(
            source,
            tmp_path / "snapshots",
            label="app",
            attempts=2,
            retry_delay_seconds=0,
            required_prefix=b"molt-main",
        )


@pytest.mark.parametrize("failure_stage", ["preflight", "link"])
def test_relocatable_input_has_one_admission_and_retains_failure_timings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, failure_stage: str
) -> None:
    runtime = tmp_path / "runtime.wasm"
    app = tmp_path / "app.wasm"
    linked = tmp_path / "linked.wasm"
    timings = tmp_path / "timings.json"
    runtime_data = wasm_link_format._append_linking_function_symbols(
        _build_exported_function_module("molt_exception_pending"),
        [("molt_exception_pending", 0, wasm_link_format.FLAG_EXPLICIT_NAME)],
        facts_provider=_facts_provider,
    )
    assert runtime_data is not None
    runtime.write_bytes(runtime_data)
    app_data = _build_minimal_module(b"")
    app.write_bytes(app_data)
    linked.write_bytes(b"previous published deployment")
    calls: list[list[str]] = []

    def run(command, **_kwargs):
        calls.append(command)
        preflight = "-r" in command
        assert runtime not in [Path(item) for item in command]
        return subprocess.CompletedProcess(
            command,
            9 if preflight and failure_stage == "preflight" else 0 if preflight else 7,
            "",
            "preflight sentinel" if preflight else "link sentinel",
        )

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", run)
    result = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        app,
        linked,
        runtime_role="reloc",
        phase_timings_file=timings,
    )
    assert result == (1 if failure_stage == "preflight" else 7)
    assert sum("-r" in command for command in calls) == 1
    linker_calls = [command for command in calls if command[0] == "wasm-ld"]
    assert len(linker_calls) == (1 if failure_stage == "preflight" else 2)
    assert runtime.read_bytes() == runtime_data
    assert app.read_bytes() == app_data
    assert linked.read_bytes() == b"previous published deployment"
    recorded = json.loads(timings.read_text(encoding="utf-8"))
    assert recorded["wasm_reloc_preflight_invocations"] == 1
    assert recorded["wasm_reloc_preflight"] >= 0
    assert ("wasm_link_total" in recorded) == (failure_stage == "link")
    assert failure_stage + " sentinel" in capsys.readouterr().err


@pytest.mark.parametrize("relocatable", [False, True])
def test_wasm_module_identity_survives_distinct_staging_paths(
    tmp_path: Path, relocatable: bool
) -> None:
    """Exercise lld's name emission, including the output-name negative control."""
    linker = wasm_link_command.wasm_toolchain.resolve_wasm_linker()
    if linker is None:
        pytest.skip("WASM linker is not available")
    source = tmp_path / "input.o"
    data = wasm_link_format._append_linking_function_symbols(
        _build_exported_runtime_module("user_entry"),
        [
            (
                "user_entry",
                0,
                FLAG_BINDING_GLOBAL | wasm_link_format.FLAG_EXPLICIT_NAME,
            )
        ],
        facts_provider=_facts_provider,
    )
    assert data is not None
    source.write_bytes(data)
    flags = ["-r"] if relocatable else ["--no-entry", "--export=user_entry"]
    stable_outputs: list[bytes] = []
    implicit_outputs: list[bytes] = []
    for attempt in ("first transaction", "second transaction"):
        directory = tmp_path / attempt
        directory.mkdir()
        published = directory / "user program.wasm"
        staged = directory / f"{attempt}.tmp"
        for explicit_name, outputs in (
            (False, implicit_outputs),
            (True, stable_outputs),
        ):
            output_args = (
                wasm_link_output_arguments(published, staged_output=staged)
                if explicit_name
                else ("-o", str(staged))
            )
            result = wasm_link_command._run_external_tool(
                [str(linker.path), *flags, *output_args, str(source)],
                capture_output=True,
                text=True,
            )
            assert result.returncode == 0, result.stderr
            outputs.append(staged.read_bytes())
        assert not published.exists(), "the linker must write only its staging output"
    assert implicit_outputs[0] != implicit_outputs[1]
    assert stable_outputs[0] == stable_outputs[1]
    names = dict(
        wasm_link_format._parse_custom_section(payload)
        for section_id, payload in wasm_link_operations.parse_sections(
            stable_outputs[0]
        )
        if section_id == 0
    )["name"]
    assert b"user_entry" in names, "function debug names must survive"
    module_names = []
    offset = 0
    while offset < len(names):
        subsection = names[offset]
        size, offset = wasm_link_format._read_varuint(names, offset + 1)
        end = offset + size
        if subsection == 0:
            name, consumed = wasm_link_format._read_string(names, offset)
            assert consumed == end
            module_names.append(name)
        offset = end
    assert module_names == ["user program.wasm"]


def test_post_link_canonicalization_flattens_plain_function_rec_groups() -> None:
    module = _module_with_flattenable_rec_group_type()

    canonical = wasm_link_validation._canonicalize_wasm_ld_output(
        module,
        description="test output",
    )

    assert canonical != module
    assert wasm_link_operations.parse_sections(canonical) == [(1, b"\x01\x60\x00\x00")]


def test_strip_debug_sections_removes_all_dwarf_custom_sections() -> None:
    debug_info = wasm_link_format._build_custom_section(".debug_info", b"old")
    debug_line_str = wasm_link_format._build_custom_section(".debug_line_str", b"new")
    keep = wasm_link_format._build_custom_section("molt.keep", b"payload")
    module = wasm_link_operations.build_sections(
        [
            (0, debug_info),
            (0, debug_line_str),
            (0, keep),
        ]
    )

    stripped = wasm_link_operations.strip_publication_sections(
        module, final_artifact=False, preserve_debug=False
    )

    assert stripped != module
    custom_names = [
        wasm_link_format._parse_custom_section(payload)[0]
        for section_id, payload in wasm_link_operations.parse_sections(stripped)
        if section_id == 0
    ]
    assert custom_names == ["molt.keep"]
    assert (
        wasm_link_operations.strip_publication_sections(
            module, final_artifact=False, preserve_debug=True
        )
        == module
    )


def test_post_link_optimizer_leaves_debug_for_final_publication() -> None:
    app = _build_split_runtime_app_module([])
    app = wasm_link_operations.build_sections(
        wasm_link_operations.parse_sections(app)
        + [(0, wasm_link_format._build_custom_section(".debug_info", b"source"))]
    )
    optimized = wasm_link_optimize._post_link_optimize(
        app,
        preserve_debug=True,
        preserve_exports=wasm_link_export_contract._split_runtime_contract_export_names(
            "app"
        ),
        facts_provider=_facts_provider,
    )
    assert ".debug_info" in wasm_artifact.wasm_custom_section_names(optimized)
    assert ".debug_info" in wasm_artifact.wasm_custom_section_names(
        wasm_link_operations.strip_publication_sections(
            optimized, final_artifact=True, preserve_debug=True
        )
    )
    assert ".debug_info" not in wasm_artifact.wasm_custom_section_names(
        wasm_link_operations.strip_publication_sections(
            optimized, final_artifact=True, preserve_debug=False
        )
    )


def _custom_section(name: str, body: bytes = b"") -> tuple[int, bytes]:
    return (0, wasm_link_format._build_custom_section(name, body))


def _function_export_section(*entries: tuple[str, int]) -> tuple[int, bytes]:
    payload = bytearray(wasm_link_format._write_varuint(len(entries)))
    for name, index in entries:
        payload.extend(wasm_link_format._write_string(name))
        payload.append(0)
        payload.extend(wasm_link_format._write_varuint(index))
    return (7, bytes(payload))


def test_section_canonicalization_keeps_customs_after_their_predecessors() -> None:
    lead = _custom_section("lead", b"\x00before-type")
    between = _custom_section("between", b"after-type")
    name = _custom_section("name", b"\x01function-names")
    debug = _custom_section(".debug_info", b"dwarf")
    module = wasm_link_operations.build_sections(
        [
            lead,
            (1, b"type"),
            between,
            (3, b"function"),
            (10, b"code"),
            (11, b"data"),
            (9, b"elem"),
            name,
            debug,
        ]
    )

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    # Only the element section moves; every custom keeps its bytes and still
    # follows each standard section it followed, so `name` stays after data.
    assert wasm_link_operations.parse_sections(canonical) == [
        lead,
        (1, b"type"),
        between,
        (3, b"function"),
        (9, b"elem"),
        (10, b"code"),
        (11, b"data"),
        name,
        debug,
    ]
    assert wasm_link_edit._standard_section_order_error(canonical) is None
    assert wasm_link_edit._canonicalize_standard_section_order(canonical) is None


def test_section_canonicalization_never_hoists_custom_over_predecessor() -> None:
    after_code = _custom_section("after_code", b"code-relative")
    module = wasm_link_operations.build_sections(
        [(1, b"type"), (10, b"code"), after_code, (9, b"elem"), (7, b"export")]
    )

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    assert wasm_link_operations.parse_sections(canonical) == [
        (1, b"type"),
        (7, b"export"),
        (9, b"elem"),
        (10, b"code"),
        after_code,
    ]


def test_section_canonicalization_leaves_ordered_customs_in_place() -> None:
    # wasm-ld's shape: customs lead, sit between and trail ordered standards.
    module = wasm_link_operations.build_sections(
        [
            _custom_section("dylink.0", b"\x01"),
            (1, b"type"),
            (2, b"import"),
            (3, b"function"),
            _custom_section("molt.callable_table.layout", b"layout"),
            (7, b"export"),
            (10, b"code"),
            (11, b"data"),
            _custom_section("name", b"\x01names"),
            _custom_section(".debug_info", b"dwarf"),
            _custom_section("producers", b"\x00"),
            _custom_section("target_features", b"\x00"),
        ]
    )

    assert wasm_link_edit._canonicalize_standard_section_order(module) is None


def test_section_canonicalization_merges_duplicates_around_customs() -> None:
    after_export = _custom_section("after_export", b"a")
    after_code = _custom_section("after_code", b"b")
    trailing = _custom_section("trailing", b"c")
    type_section = (1, bytes([1, 0x60, 0, 0]))
    module = wasm_link_operations.build_sections(
        [
            type_section,
            _function_export_section(("molt_main", 0)),
            after_export,
            (10, b"code"),
            after_code,
            _function_export_section(("molt_main", 2), ("PyInit__demo", 1)),
            trailing,
        ]
    )

    canonical = wasm_link_edit._canonicalize_standard_section_order(module)

    assert canonical is not None
    # Strict parsing proves one export section; the first export of a name wins.
    assert wasm_link_operations.parse_sections(canonical) == [
        type_section,
        _function_export_section(("molt_main", 0), ("PyInit__demo", 1)),
        after_export,
        (10, b"code"),
        after_code,
        trailing,
    ]
    assert wasm_link_edit._canonicalize_standard_section_order(canonical) is None


@pytest.mark.parametrize("name", ["linking", "reloc.CODE"])
def test_section_canonicalization_refuses_relocation_metadata(name: str) -> None:
    metadata = _custom_section(name, b"\x02")
    reordered = wasm_link_operations.build_sections(
        [(1, b"type"), (10, b"code"), (9, b"elem"), metadata]
    )
    ordered = wasm_link_operations.build_sections(
        [(1, b"type"), (9, b"elem"), (10, b"code"), metadata]
    )

    with pytest.raises(ValueError, match="relocation metadata"):
        wasm_link_edit._canonicalize_standard_section_order(reordered)
    assert wasm_link_edit._canonicalize_standard_section_order(ordered) is None


def test_section_canonicalization_rejects_unknown_standard_sections() -> None:
    module = wasm_link_operations.build_sections(
        [(10, b"code"), (1, b"type"), (14, b"?")]
    )

    with pytest.raises(ValueError, match="unknown standard section id 14"):
        wasm_link_edit._canonicalize_standard_section_order(module)


@pytest.fixture
def small_split_optimizer_transform_authority(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Use real transitive source identity without scanning the product graph."""
    root = tmp_path / "transform-authority"
    tools = root / "tools"
    tools.mkdir(parents=True)
    entry = tools / "entry.py"
    dependency = tools / "dependency.py"
    entry.write_text("import dependency\n", encoding="utf-8")
    dependency.write_text("VERSION = 1\n", encoding="utf-8")
    closure = wasm_link_optimizer_policy.local_python_import_closure(root, (entry,))
    assert dependency in closure.paths
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_wasm_link_cache_authority_digest",
        lambda: closure.content_digest,
    )


def test_split_app_size_attestation_excludes_optimizer_execution_telemetry(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    small_split_optimizer_transform_authority: None,
) -> None:
    app = _build_split_runtime_app_module([])
    executable = tmp_path / "wasm-opt"
    executable.write_bytes(b"optimizer")
    identity = _wasm_optimizer_identity(
        executable.resolve(), sha256=hashlib.sha256(b"optimizer").hexdigest()
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )
    optimizer_calls = 0

    def fake_optimize(path: Path, **_kwargs: object) -> dict[str, object]:
        nonlocal optimizer_calls
        optimizer_calls += 1
        # The second invocation simulates reuse inside the optimizer after the
        # split-app cache is deliberately moved to a different root.
        return {
            "ok": True,
            "status": "success" if optimizer_calls == 1 else "cache-hit",
            "output_bytes": path.stat().st_size,
            "pipeline": list(
                wasm_link_optimizer_policy.wasm_link_policy("Oz").pipeline
            ),
            "before": {
                "file_bytes": path.stat().st_size,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            },
            "after": {
                "file_bytes": path.stat().st_size,
                "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            },
            "wasm_opt_path": str(identity.path),
            "wasm_opt_sha256": identity.sha256,
            "binaryen_version": identity.binaryen_version,
            "elapsed_s": 1.25 if optimizer_calls == 1 else 0.0,
            "peak_rss_kb": 800 if optimizer_calls == 1 else None,
            "peak_total_rss_kb": 1200 if optimizer_calls == 1 else None,
            "cache_hit": optimizer_calls > 1,
        }

    monkeypatch.setattr(wasm_link_optimizer_policy, "optimize_wasm", fake_optimize)

    def run(
        cache_name: str,
    ) -> tuple[bytes, dict[str, object], dict[str, int | float]]:
        monkeypatch.setenv("MOLT_CACHE", str(tmp_path / cache_name))
        attestation: dict[str, object] = {}
        counts: dict[str, int | float] = {}
        output = wasm_link_optimizer_policy._optimize_split_app_module(
            app,
            reference_data=None,
            optimize=True,
            optimize_level="Oz",
            optimizer_identity=identity,
            contract_keep_set={"molt_main"},
            attestation=attestation,
            operation_counts=counts,
            facts_provider=_facts_provider,
        )
        return output, attestation, counts

    cold, cold_attestation, cold_counts = run("cache-a")
    warm, warm_attestation, warm_counts = run("cache-a")
    nested_reuse, nested_attestation, nested_counts = run("cache-b")

    assert cold == warm == nested_reuse
    assert optimizer_calls == 2
    assert (
        json.dumps(cold_attestation, sort_keys=True)
        == json.dumps(warm_attestation, sort_keys=True)
        == json.dumps(nested_attestation, sort_keys=True)
    )
    assert (
        not {
            "cache_hit",
            "wasm_opt_wall_ms",
            "wasm_opt_peak_rss_kb",
            "wasm_opt_peak_total_rss_kb",
        }
        & cold_attestation.keys()
    )
    assert cold_counts["split_app_optimize_cache_optimizer_wall_ms"] == 1250.0
    assert cold_counts["split_app_optimize_cache_optimizer_peak_rss_kb"] == 800.0
    assert cold_counts["split_app_optimize_cache_optimizer_peak_total_rss_kb"] == 1200.0
    assert warm_counts["split_app_optimize_cache_hits"] == 1
    assert "split_app_optimize_cache_optimizer_wall_ms" not in warm_counts
    assert nested_counts["split_app_optimize_cache_misses"] == 1
    assert nested_counts["split_app_optimize_cache_optimizer_wall_ms"] == 0.0


def test_split_app_size_attestation_without_wasm_opt_is_cache_independent(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    small_split_optimizer_transform_authority: None,
) -> None:
    app = _build_split_runtime_app_module([])
    monkeypatch.setenv("MOLT_CACHE", str(tmp_path / "cache"))
    monkeypatch.setattr(
        wasm_link_optimize, "_post_link_optimize", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimize,
        "_strip_unused_module_function_imports",
        lambda *_args, **_kwargs: None,
    )
    attestations: list[dict[str, object]] = []
    counts: list[dict[str, int | float]] = []
    for run in range(2):
        # This fact belongs to an earlier link stage, not the optimizer cache.
        attestation: dict[str, object] = {"upstream_fact": run}
        operation_counts: dict[str, int | float] = {}
        assert (
            wasm_link_optimizer_policy._optimize_split_app_module(
                app,
                reference_data=None,
                optimize=False,
                optimize_level="Oz",
                contract_keep_set={"molt_main"},
                attestation=attestation,
                operation_counts=operation_counts,
                facts_provider=_facts_provider,
            )
            == app
        )
        attestations.append(attestation)
        counts.append(operation_counts)
    assert attestations == [{"upstream_fact": 0}, {"upstream_fact": 1}]
    assert counts[0]["split_app_optimize_cache_misses"] == 1
    assert counts[1]["split_app_optimize_cache_hits"] == 1


def test_debug_policy_partitions_split_app_and_runtime_cache_keys() -> None:
    app_kwargs = {
        "app_data": b"app",
        "reference_data": None,
        "optimize": False,
        "optimize_level": "O1",
        "contract_keep_set": {"molt_main"},
        "facts_authority_digest": "facts",
    }
    assert wasm_link_optimizer_policy._split_app_optimize_cache_key(
        **app_kwargs, preserve_debug=False
    ) != wasm_link_optimizer_policy._split_app_optimize_cache_key(
        **app_kwargs, preserve_debug=True
    )
    runtime_kwargs = {
        "runtime_data": b"runtime",
        "normalized_required_exports": {"molt_main"},
        "facts_authority_digest": "facts",
    }
    assert wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
        **runtime_kwargs, preserve_debug=False
    ) != wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
        **runtime_kwargs, preserve_debug=True
    )


def test_transform_authority_digest_invalidates_both_cache_keys(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    entry = tools / "entry.py"
    dependency = tools / "dependency.py"
    entry.write_text("import dependency\n", encoding="utf-8")
    dependency.write_text("VERSION = 1\n", encoding="utf-8")
    first = wasm_link_optimizer_policy.local_python_import_closure(tmp_path, (entry,))
    assert dependency in first.paths
    dependency.write_text("VERSION = 2\n", encoding="utf-8")
    second = wasm_link_optimizer_policy.local_python_import_closure(tmp_path, (entry,))
    assert first.paths == second.paths
    assert first.content_digest != second.content_digest

    def keys(authority: str) -> tuple[str, str]:
        monkeypatch.setattr(
            wasm_link_optimizer_policy,
            "_wasm_link_cache_authority_digest",
            lambda: authority,
        )
        split = wasm_link_optimizer_policy._split_app_optimize_cache_key(
            app_data=b"app",
            reference_data=b"reference",
            optimize=False,
            optimize_level="Oz",
            contract_keep_set={"molt_main"},
            facts_authority_digest="facts",
        )
        tree = wasm_link_optimizer_policy._tree_shake_runtime_cache_key(
            runtime_data=b"runtime",
            normalized_required_exports={"molt_main"},
            facts_authority_digest="facts",
        )
        return split, tree

    assert keys(first.content_digest) != keys(second.content_digest)


def test_wasm_link_cache_authority_uses_entry_module_import_closure(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    repo_root = wasm_link.TOOLS_ROOT.parent
    entry = Path(wasm_link.__file__).resolve()
    optimizer_policy = Path(wasm_link_optimizer_policy.__file__).resolve()
    # Real repository reachability belongs to the slow closure integration test.
    expected = LocalPythonSourceClosure(
        paths=(entry, optimizer_policy),
        source_sha256={},
        content_digest="captured-tooling-digest",
        source_bytes=0,
    )
    calls: list[tuple[Path, tuple[Path, ...]]] = []

    def closure(root: Path, seeds: tuple[Path, ...]):
        calls.append((root, seeds))
        return expected

    def forbidden(*_args, **_kwargs):
        raise AssertionError(
            "linker must use captured closure identity without rereads"
        )

    monkeypatch.setattr(
        wasm_link_optimizer_policy, "local_python_import_closure", closure
    )
    with monkeypatch.context() as patch:
        patch.setattr(Path, "read_bytes", forbidden)
        digest = wasm_link_optimizer_policy._wasm_link_cache_authority_digest()
    assert digest == expected.content_digest
    assert calls == [(repo_root, (entry,))]
    assert optimizer_policy.name == "wasm_link_optimizer_policy.py"


def test_transform_authority_digest_reuses_only_the_current_operation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    tools = tmp_path / "tools"
    tools.mkdir()
    entry = tools / "entry.py"
    dependency = tools / "dependency.py"
    entry.write_text("import dependency\n", encoding="utf-8")
    dependency.write_text("VERSION = 1\n", encoding="utf-8")
    real_closure = wasm_link_optimizer_policy.local_python_import_closure
    receipts = []

    def closure(root: Path, seeds: tuple[Path, ...]):
        assert root == wasm_link.TOOLS_ROOT.parent
        assert seeds == (Path(wasm_link.__file__),)
        receipt = real_closure(tmp_path, (entry,))
        receipts.append(receipt)
        return receipt

    monkeypatch.setattr(
        wasm_link_optimizer_policy, "local_python_import_closure", closure
    )
    with wasm_link.local_python_import_graph_transaction():
        first = wasm_link_optimizer_policy._wasm_link_cache_authority_digest()
        dependency.write_text("VERSION = 2\n", encoding="utf-8")
        assert wasm_link_optimizer_policy._wasm_link_cache_authority_digest() == first
        assert len(receipts) == 2
        assert receipts[0] is receipts[1]

    with wasm_link.local_python_import_graph_transaction():
        second = wasm_link_optimizer_policy._wasm_link_cache_authority_digest()
        assert second != first
        assert wasm_link_optimizer_policy._wasm_link_cache_authority_digest() == second
        assert len(receipts) == 4
        assert receipts[2] is receipts[3]
        assert receipts[2] is not receipts[0]


def test_run_wasm_ld_preserves_ordered_staged_native_plan(
    tmp_path: Path,
    monkeypatch,
) -> None:
    output_bytes = _build_minimal_module(b"")
    runtime_bytes = _build_exported_runtime_module("molt_exception_pending")
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native_object = tmp_path / "external_static_packages" / "ndimage_edt.o"
    lazy_archive = tmp_path / "external_static_packages" / "support.a"
    wasm_ld_inputs: list[str] = []

    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    native_object.parent.mkdir()
    native_object.write_bytes(_module_with_linking_symbols([]))
    lazy_archive.write_bytes(b"!<arch>\n")

    def fake_run(cmd, **kwargs):  # type: ignore[no-untyped-def]
        del kwargs
        if cmd and cmd[0] == "wasm-ld":
            wasm_ld_inputs.extend(cmd)
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link_validation, "_validate_linked", lambda _p, **_: True)
    monkeypatch.setattr(
        wasm_link_optimize, "_post_link_optimize", lambda data, **_kwargs: data
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )

    requirements = SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        (
            source_extension_link_file(native_object),
            SourceExtensionLinkCyclicGroup(
                (
                    source_extension_link_file(
                        lazy_archive,
                        loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
                    ),
                    source_extension_link_file(native_object),
                )
            ),
            source_extension_link_file(lazy_archive),
        ),
        ("ndimage_edt",),
    )
    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=requirements,
    )

    assert rc == 0
    output_index = wasm_ld_inputs.index("-o") + 2
    typed_operands = [
        part
        for part in wasm_ld_inputs[output_index + 2 :]
        if part != "--trace" and not part.startswith("--why-extract=")
    ]
    assert typed_operands[0] == "--undefined=ndimage_edt"
    assert [
        Path(part).name if not part.startswith("--") else part
        for part in typed_operands[1:]
    ] == [
        native_object.name,
        "--whole-archive",
        lazy_archive.name,
        "--no-whole-archive",
        native_object.name,
        lazy_archive.name,
    ]
    assert typed_operands[1] == typed_operands[5]
    assert typed_operands[3] == typed_operands[6]


def test_run_wasm_ld_rejects_native_plan_for_wrong_wasm_target(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    runtime.write_bytes(_build_exported_runtime_module("molt_exception_pending"))
    output.write_bytes(_build_minimal_module(b""))
    monkeypatch.setattr(
        wasm_link_command,
        "_run_external_tool",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-ld must not run with a cross-target plan")
        ),
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=SourceExtensionLinkRequirements(
            "wasm32-unknown-unknown"
        ),
    )

    assert rc == 1
    assert "native WASM link requirements target mismatch" in capsys.readouterr().err


@pytest.mark.parametrize("changed_input", [False, True])
def test_run_wasm_ld_rejects_conflicting_repeated_input_digests(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    changed_input: bool,
) -> None:
    runtime = tmp_path / "molt_runtime.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    native = tmp_path / "native.o"
    runtime.write_bytes(_build_exported_runtime_module("molt_exception_pending"))
    output.write_bytes(_build_minimal_module(b""))
    native.write_bytes(b"\0asm\x01\0\0\0native")
    admitted = source_extension_link_file(native)
    conflicting = SourceExtensionLinkInput(
        admitted.path,
        "0" * 64 if admitted.sha256 != "0" * 64 else "1" * 64,
    )
    if changed_input:
        native.write_bytes(b"\0asm\x01\0\0\0changed")
    monkeypatch.setattr(
        wasm_link_command,
        "_run_external_tool",
        lambda *_args, **_kwargs: (_ for _ in ()).throw(
            AssertionError("wasm-ld must not run with contradictory digests")
        ),
    )

    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        native_link_requirements=SourceExtensionLinkRequirements(
            "wasm32-wasip1", (admitted,) if changed_input else (admitted, conflicting)
        ),
    )

    assert rc == 1
    diagnostic = (
        "link requirement checksum mismatch"
        if changed_input
        else "conflicting link requirement checksum claims"
    )
    assert diagnostic in capsys.readouterr().err


def test_split_runtime_native_allowlist_is_the_split_runtime_export_surface(
    tmp_path: Path,
) -> None:
    base = tmp_path / "wasm_allowed_imports.txt"
    base.write_text("# base\nhost_only\n", encoding="utf-8")
    native_object = tmp_path / "ext.molt.wasm"
    native_object.write_bytes(b"\0asm\x01\0\0\0")
    temp_dir = OwnedTemporaryDirectory()
    try:
        composed = wasm_link_native_inputs._compose_split_runtime_native_allowlist(
            base_allowlist=base,
            native_link_requirements=_native_link_requirements(native_object),
            split_runtime_exports={"molt_PyType_Ready", "molt_err_pending"},
            temp_dir=temp_dir,
        )
        symbols = _parse_allowlist(composed)
    finally:
        temp_dir.cleanup()
    assert {"host_only", "molt_PyType_Ready", "molt_err_pending"} <= symbols
    assert "PyType_Ready" not in symbols
    assert (
        wasm_link_native_inputs._compose_split_runtime_native_allowlist(
            base_allowlist=base,
            native_link_requirements=_native_link_requirements(),
            split_runtime_exports={"molt_PyType_Ready"},
            temp_dir=OwnedTemporaryDirectory(),
        )
        == base
    )


def test_ensure_function_exports_by_symbol_names_adds_public_exports() -> None:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(2))
    func_payload.extend(write_varuint(0))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    export_payload = bytearray()
    export_payload.extend(write_varuint(1))
    export_payload.extend(wasm_link_format._write_string("molt_main"))
    export_payload.append(0x00)
    export_payload.extend(write_varuint(1))
    sections.append((7, bytes(export_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(2))
    for _ in range(2):
        code_payload.extend(write_varuint(2))
        code_payload.append(0x00)
        code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    linking_payload = wasm_link_format._build_linking_payload(
        2,
        [
            (
                SYMTAB_SUBSECTION_ID,
                _build_symbol_subsection(
                    [
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=0,
                            name="__molt_output_export_0",
                        ),
                        _function_symbol_entry(
                            flags=FLAG_BINDING_GLOBAL
                            | wasm_link_format.FLAG_EXPLICIT_NAME
                            | FLAG_EXPORTED
                            | FLAG_NO_STRIP,
                            index=1,
                            name="molt_main",
                        ),
                    ]
                ),
            )
        ],
    )
    sections.append(
        (0, wasm_link_format._build_custom_section("linking", linking_payload))
    )

    updated = wasm_link_edit._ensure_function_exports_by_symbol_names(
        wasm_link_operations.build_sections(sections),
        {"main_molt__init": "__molt_output_export_0"},
        facts_provider=_facts_provider,
    )
    assert updated is not None
    exports = wasm_link_format._collect_exports(updated)
    assert "main_molt__init" in exports
    assert "molt_main" in exports


def test_ensure_function_exports_by_symbol_names_uses_name_section_fallback() -> None:
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []

    type_payload = bytearray()
    type_payload.extend(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(1))
    type_payload.append(0x7E)
    sections.append((1, bytes(type_payload)))

    func_payload = bytearray()
    func_payload.extend(write_varuint(1))
    func_payload.extend(write_varuint(0))
    sections.append((3, bytes(func_payload)))

    code_payload = bytearray()
    code_payload.extend(write_varuint(1))
    code_payload.extend(write_varuint(2))
    code_payload.append(0x00)
    code_payload.append(0x0B)
    sections.append((10, bytes(code_payload)))

    func_name_subsection = bytearray()
    func_name_subsection.extend(write_varuint(1))
    func_name_subsection.extend(write_varuint(0))
    func_name_subsection.extend(
        wasm_link_format._write_string("__molt_output_export_1900")
    )
    name_custom_payload = bytearray()
    name_custom_payload.append(1)
    name_custom_payload.extend(write_varuint(len(func_name_subsection)))
    name_custom_payload.extend(func_name_subsection)
    sections.append(
        (0, wasm_link_format._build_custom_section("name", bytes(name_custom_payload)))
    )

    updated = wasm_link_edit._ensure_function_exports_by_symbol_names(
        wasm_link_operations.build_sections(sections),
        {"main_molt__init": "__molt_output_export_1900"},
        facts_provider=_facts_provider,
    )
    assert updated is not None
    exports = wasm_link_format._collect_exports(updated)
    assert "main_molt__init" in exports


def _module_with_tag_section(
    *, export_section: bytes | None, with_table: bool = False
) -> bytes:
    """A module whose Tag section (id 13) sits between Memory and Global, as
    wasm-ld emits for exception-handling objects: numerically the largest
    section id, canonically the sixth section."""
    write_varuint = wasm_link_format._write_varuint
    sections: list[tuple[int, bytes]] = []
    type_payload = bytearray(write_varuint(1))
    type_payload.append(0x60)
    type_payload.extend(write_varuint(0))
    type_payload.extend(write_varuint(0))
    sections.append((1, bytes(type_payload)))
    sections.append((3, write_varuint(1) + write_varuint(0)))
    if with_table:
        sections.append((4, write_varuint(1) + bytes([0x70, 0x00]) + write_varuint(1)))
    sections.append((5, write_varuint(1) + bytes([0x00]) + write_varuint(1)))
    sections.append((13, write_varuint(1) + bytes([0x00]) + write_varuint(0)))
    sections.append((6, write_varuint(1) + bytes([0x7F, 0x01, 0x41, 0x00, 0x0B])))
    if export_section is not None:
        sections.append((7, export_section))
    sections.append((10, write_varuint(1) + write_varuint(2) + bytes([0x00, 0x0B])))
    func_name_subsection = bytearray(write_varuint(1))
    func_name_subsection.extend(write_varuint(0))
    func_name_subsection.extend(
        wasm_link_format._write_string("__molt_output_export_7")
    )
    name_payload = bytearray([1])
    name_payload.extend(write_varuint(len(func_name_subsection)))
    name_payload.extend(func_name_subsection)
    sections.append(
        (0, wasm_link_format._build_custom_section("name", bytes(name_payload)))
    )
    return wasm_link_operations.build_sections(sections)


def _standard_section_ids(data: bytes) -> list[int]:
    return [
        sid for sid, _payload in wasm_link_operations.parse_sections(data) if sid != 0
    ]


def test_ensure_function_exports_inserts_the_export_section_after_a_tag_section() -> (
    None
):
    updated = wasm_link_edit._ensure_function_exports_by_symbol_names(
        _module_with_tag_section(export_section=None),
        {"main_molt__init": "__molt_output_export_7"},
        facts_provider=_facts_provider,
    )
    assert updated is not None
    assert wasm_link_edit._standard_section_order_error(updated) is None
    assert _standard_section_ids(updated) == [1, 3, 5, 13, 6, 7, 10]
    assert "main_molt__init" in wasm_link_format._collect_exports(updated)


def test_ensure_function_exports_rewrites_an_export_section_after_a_tag_section() -> (
    None
):
    existing = (
        wasm_link_format._write_varuint(1)
        + wasm_link_format._write_string("molt_main")
        + bytes([0x00])
        + wasm_link_format._write_varuint(0)
    )
    updated = wasm_link_edit._ensure_function_exports_by_symbol_names(
        _module_with_tag_section(export_section=existing),
        {"main_molt__init": "__molt_output_export_7"},
        facts_provider=_facts_provider,
    )
    assert updated is not None
    assert wasm_link_edit._standard_section_order_error(updated) is None
    assert _standard_section_ids(updated) == [1, 3, 5, 13, 6, 7, 10]
    exports = wasm_link_format._collect_exports(updated)
    assert set(exports) == {"molt_main", "main_molt__init"}


def test_ensure_table_export_inserts_the_export_section_after_a_tag_section() -> None:
    updated = wasm_link_format._ensure_table_export(
        _module_with_tag_section(export_section=None, with_table=True),
        facts_provider=_facts_provider,
    )
    assert updated is not None
    assert wasm_link_edit._standard_section_order_error(updated) is None
    assert _standard_section_ids(updated) == [1, 3, 4, 5, 13, 6, 7, 10]
    assert "molt_table" in wasm_link_format._collect_exports(updated)


def test_ensure_export_by_index_inserts_export_without_moving_customs() -> None:
    module = _module_with_tag_section(export_section=None)

    updated = wasm_link_export_contract._ensure_export_by_index(
        module, name="molt_main", kind=0, index=0
    )

    assert updated is not None
    sections = wasm_link_operations.parse_sections(updated)
    assert [section_id for section_id, _payload in sections] == [
        1,
        3,
        5,
        13,
        6,
        7,
        10,
        0,
    ]
    assert [section for section in sections if section[0] != 7] == (
        wasm_link_operations.parse_sections(module)
    )
    assert _fixture_export_kinds(updated) == {"molt_main": (0, 0)}


def test_insert_standard_section_refuses_a_duplicate_standard_section() -> None:
    sections = wasm_link_operations.parse_sections(
        _module_with_tag_section(export_section=b"\x00")
    )
    with pytest.raises(ValueError, match="duplicate standard section id 7"):
        wasm_link_format._insert_standard_section(sections, 7, b"\x00")


@pytest.mark.parametrize("split", [False, True])
def test_run_wasm_ld_publishes_receipt_with_validated_candidates(
    tmp_path: Path,
    monkeypatch,
    split: bool,
) -> None:
    runtime_bytes = _build_exported_runtime_module("molt_err_pending")
    output_bytes = _module_with_linking_symbols([])
    runtime = tmp_path / "molt_runtime_reloc.wasm"
    output = tmp_path / "output.wasm"
    linked = tmp_path / "output_linked.wasm"
    split_dir = tmp_path / "split"
    runtime.write_bytes(runtime_bytes)
    output.write_bytes(output_bytes)
    split_dir.mkdir()
    linked.write_bytes(b"old-linked")
    app_wasm = split_dir / "app.wasm"
    rt_wasm = split_dir / "molt_runtime.wasm"
    app_wasm.write_bytes(b"old-app")
    rt_wasm.write_bytes(b"old-runtime")
    validate_seen: list[Path] = []
    split_validate_seen: list[tuple[Path, Path]] = []

    def fake_run(cmd, **kwargs):
        _write_wasm_ld_output(cmd, output_bytes)

        class Result:
            returncode = 0
            stderr = ""
            stdout = ""

        return Result()

    def validate_linked(path: Path, *, facts_provider) -> bool:
        assert facts_provider is not None
        validate_seen.append(path)
        assert path != linked
        assert linked.read_bytes() == b"old-linked"
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    def validate_split(app_stage: Path, rt_stage: Path, *, facts_provider) -> bool:
        assert facts_provider is not None
        split_validate_seen.append((app_stage, rt_stage))
        assert app_stage != app_wasm
        assert rt_stage != rt_wasm
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return True

    monkeypatch.setattr(wasm_link_command, "_run_external_tool", fake_run)
    monkeypatch.setattr(wasm_link_validation, "_validate_linked", validate_linked)
    monkeypatch.setattr(
        wasm_link_validation, "_validate_split_runtime_outputs", validate_split
    )
    monkeypatch.setattr(
        wasm_link_export_contract,
        "_restore_split_runtime_contract_exports",
        lambda data, **_kwargs: data,
    )
    monkeypatch.setattr(
        wasm_link_format, "_ensure_table_export", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_edit, "_restore_output_export_aliases", lambda data, **_kwargs: None
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy, "_optimize_split_app_module", lambda data, **_: data
    )
    monkeypatch.setattr(
        wasm_link_optimizer_policy,
        "_tree_shake_runtime",
        lambda *_args, **_kwargs: runtime_bytes,
    )
    monkeypatch.setattr(wasm_link_format, "_collect_custom_names", lambda _data: [])

    fingerprint = link_fingerprints._link_fingerprint(
        project_root=tmp_path, inputs=[runtime, output], link_cmd=["fixture-wasm-ld"]
    )
    sidecar = tmp_path / "state" / "link.fingerprint"
    rc = _run_wasm_ld_with_rust_facts(
        "wasm-ld",
        runtime,
        output,
        linked,
        split_runtime=split,
        split_output_dir=split_dir,
        link_receipt=link_fingerprints.FinalLinkReceiptRequest.from_fingerprint(
            sidecar, fingerprint
        ),
    )

    assert rc == 0
    assert validate_seen
    assert bool(split_validate_seen) == split
    assert linked.read_bytes() != b"old-linked"
    outputs = wasm_link.wasm_link_output_paths(
        linked, split_output_dir=split_dir if split else None
    )
    receipt = link_fingerprints._read_link_fingerprint(sidecar)
    assert receipt is not None
    assert link_fingerprints._link_outputs_match(
        outputs=outputs, fingerprint=fingerprint, receipt_path=sidecar
    )
    if not split:
        assert app_wasm.read_bytes() == b"old-app"
        assert rt_wasm.read_bytes() == b"old-runtime"
        return
    expected_app = _with_test_link_facts(
        wasm_link_operations.strip_publication_sections(
            output_bytes,
            final_artifact=True,
            preserve_debug=False,
        ),
        role="app",
    )
    expected_runtime = _with_test_link_facts(
        wasm_link_operations.strip_publication_sections(
            runtime_bytes,
            final_artifact=True,
            preserve_debug=False,
        ),
        role="runtime",
    )
    assert (
        wasm_link_operations.strip_publication_sections(
            app_wasm.read_bytes(), final_artifact=True, preserve_debug=False
        )
        == expected_app
    )
    assert (
        wasm_link_operations.strip_publication_sections(
            rt_wasm.read_bytes(), final_artifact=True, preserve_debug=False
        )
        == expected_runtime
    )


def test_resolve_native_link_inputs_adds_compiler_rt_provider(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native = tmp_path / "native.molt.wasm"
    provider = tmp_path / "rustlib" / "wasm32-wasip1" / "libcompiler_builtins-x.rlib"
    native.write_bytes(_build_env_function_import_module(["__trunctfdf2", "malloc"]))
    provider.parent.mkdir(parents=True)
    provider.write_bytes(_build_compiler_rt_provider_archive())

    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda: provider,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
        raising=True,
    )

    requirements = wasm_link_native_inputs._resolve_native_link_requirements(
        _native_link_requirements(native),
        source_paths={native: native},
        facts_provider=_facts_provider,
    )

    assert tuple(Path(item.path) for item in requirements.inputs) == (native, provider)
    assert requirements.inputs[1].sha256 == source_extension_link_file(provider).sha256


def test_resolve_native_link_inputs_rejects_missing_compiler_rt_provider(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    native = tmp_path / "native.molt.wasm"
    native.write_bytes(_build_env_function_import_module(["__trunctfdf2"]))

    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda: None,
        raising=True,
    )
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "wasm_external_link_provider_symbols",
        lambda **_kwargs: frozenset({"__trunctfdf2"}),
        raising=True,
    )

    with pytest.raises(ValueError, match="wasm_compiler_rt_link_import"):
        wasm_link_native_inputs._resolve_native_link_requirements(
            _native_link_requirements(native),
            source_paths={native: native},
            facts_provider=_facts_provider,
        )


def test_wasm_archive_projection_preserves_duplicate_member_order_and_source_custody(
    tmp_path: Path,
) -> None:
    first = _build_env_function_import_module(["add"])
    second = _build_env_function_import_module(["__trunctfdf2"])
    archive = tmp_path / "duplicates.a"
    archive.write_bytes(_build_wasm_archive(("same.o", first), ("same.o", second)))
    assert [
        (member.name, member.data)
        for member in wasm_archive.iter_wasm_archive_members(archive)
    ] == [
        ("same.o", first),
        ("same.o", second),
    ]
    iterator = wasm_archive.iter_wasm_archive_members(archive)
    assert next(iterator).data == first
    updated = _build_wasm_archive(("same.o", second), ("same.o", first))
    try:
        if os.name == "nt":
            # A suspended iterator owns a reader that excludes Windows writes.
            with pytest.raises(PermissionError):
                archive.write_bytes(updated)
            assert next(iterator).data == second
        else:
            archive.write_bytes(updated)
            with pytest.raises(ValueError, match="changed|stable"):
                tuple(iterator)
    finally:
        iterator.close()
    # Closing the iterator releases custody and admits the new ordered bytes.
    archive.write_bytes(updated)
    assert [
        (member.name, member.data)
        for member in wasm_archive.iter_wasm_archive_members(archive)
    ] == [("same.o", second), ("same.o", first)]


def test_lazy_archive_compiler_rt_candidates_do_not_require_an_unused_provider(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive = tmp_path / "lazy.a"
    archive.write_bytes(
        _build_wasm_archive(
            ("dormant.o", _build_env_function_import_module(["__trunctfdf2"]))
        )
    )
    monkeypatch.setattr(
        wasm_link_native_inputs,
        "_compiler_rt_link_imports",
        lambda: frozenset({"__trunctfdf2"}),
    )
    monkeypatch.setattr(
        wasm_link_native_inputs.wasm_link_inputs,
        "wasm_compiler_builtins_archive",
        lambda: None,
    )
    requirements = _native_link_requirements(archive)
    assert (
        wasm_link_native_inputs._resolve_native_link_requirements(
            requirements,
            source_paths={archive: archive},
            facts_provider=_facts_provider,
        )
        == requirements
    )
    eager = SourceExtensionLinkRequirements(
        "wasm32-wasip1",
        (
            source_extension_link_file(
                archive, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
            ),
        ),
    )
    with pytest.raises(ValueError, match="missing provider"):
        wasm_link_native_inputs._resolve_native_link_requirements(
            eager,
            source_paths={archive: archive},
            facts_provider=_facts_provider,
        )


def test_lazy_archive_data_candidates_do_not_require_unused_runtime_addresses(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "lazy-data.a"
    archive.write_bytes(
        _build_wasm_archive(
            ("dormant.o", _build_undefined_data_symbol_object(["molt_PyLong_Type"]))
        )
    )
    runtime = tmp_path / "runtime.wasm"
    runtime.write_bytes(b"\0asm\x01\0\0\0")
    requirements = _native_link_requirements(archive)
    with OwnedTemporaryDirectory(dir=tmp_path) as scratch:
        temp_dir = type("_Tmp", (), {"name": scratch})()
        assert (
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=requirements,
                deploy_runtime=runtime,
                reloc_runtime=runtime,
                temp_dir=temp_dir,
                facts_provider=_facts_provider,
            )
            is None
        )
        eager = SourceExtensionLinkRequirements(
            "wasm32-wasip1",
            (
                source_extension_link_file(
                    archive, loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                ),
            ),
        )
        with pytest.raises(ValueError, match="exports no address global"):
            wasm_link_runtime_data._split_runtime_data_alias_object(
                native_link_requirements=eager,
                deploy_runtime=runtime,
                reloc_runtime=runtime,
                temp_dir=temp_dir,
                facts_provider=_facts_provider,
            )


def test_complete_generated_runtime_registry_is_validated_before_root_filtering() -> (
    None
):
    registry = wasm_link_runtime_data._runtime_exports
    generated = {
        registry.wasm_split_runtime_export_name_for_import(name)
        for name in registry.wasm_runtime_import_names()
    }
    assert None not in generated
    assert generated & wasm_link_format._ESSENTIAL_EXPORTS
    module = _build_exported_runtime_module_many(sorted(generated))
    runtime_imports = registry.wasm_runtime_import_names()
    roots = wasm_link_runtime_data._canonical_split_runtime_required_exports(
        module,
        runtime_imports=runtime_imports,
        facts_provider=_facts_provider,
    )
    assert roots == generated - wasm_link_format._ESSENTIAL_EXPORTS - {
        "molt_exception_pending"
    }
    omitted = next(iter(generated & wasm_link_format._ESSENTIAL_EXPORTS))
    with pytest.raises(ValueError, match=omitted):
        wasm_link_runtime_data._canonical_split_runtime_required_exports(
            _build_exported_runtime_module_many(sorted(generated - {omitted})),
            runtime_imports=runtime_imports,
            facts_provider=_facts_provider,
        )


@pytest.mark.parametrize(
    "mutable, shared, value_type",
    [
        (False, False, 0x7F),
        (True, False, 0x7F),
        (False, True, 0x7F),
        (False, False, 0x7E),
    ],
)
def test_split_runtime_data_edges_use_generated_global_kind(
    mutable: bool, shared: bool, value_type: int
) -> None:
    empty = b"\0asm\x01\0\0\0"
    app = _rust_facts_fixture(empty)
    runtime = _rust_facts_fixture(empty)
    extern_type = {
        "kind": "global",
        "value_type": [value_type],
        "mutable": mutable,
        "shared": shared,
    }
    export_name = wasm_split_runtime_export_name_for_import("Py_None")
    assert export_name is not None
    app["canonical_import_types"] = [
        {
            "module": "molt_runtime",
            "name": export_name,
            "kind": 3,
            "index": 0,
            "type": extern_type,
        }
    ]
    runtime["canonical_export_types"] = [
        {"name": export_name, "kind": 3, "index": 0, "type": extern_type}
    ]
    error = wasm_link_validation._validate_split_runtime_typed_edges(
        wasm_link_fact_provider.WasmLinkFacts(app),
        wasm_link_fact_provider.WasmLinkFacts(runtime),
    )
    if not mutable and not shared and value_type == 0x7F:
        assert error is None
    else:
        assert error is not None and "generated ABI signature" in error


def test_post_link_transform_consumes_link_metadata_before_rescanning() -> None:
    # A populated symbol table and code relocation must never survive into a
    # phase that can change function bodies or indices.
    source = wasm_link_format._append_linking_function_symbols(
        _build_exported_runtime_module("molt_main"),
        [("molt_main", 0, wasm_link_format.FLAG_EXPLICIT_NAME)],
        facts_provider=_facts_provider,
    )
    assert source is not None
    sections = [
        (sid, b"\x01\x08\x00\x10\x80\x80\x80\x80\x00\x0b" if sid == 10 else payload)
        for sid, payload in wasm_link_operations.parse_sections(source)
    ]
    code_ordinal = next(
        index for index, (sid, _payload) in enumerate(sections) if sid == 10
    )
    relocation = wasm_link_format._write_varuint(code_ordinal) + b"\x01\x00\x04\x00"
    source = wasm_link_operations.build_sections(
        [
            *sections,
            (0, wasm_link_format._build_custom_section("reloc.CODE", relocation)),
            (0, wasm_link_format._build_custom_section(".debug_info", b"debug")),
        ]
    )
    observed: list[bytes] = []

    def provider(data: bytes):
        assert "linking" not in _fixture_custom_names(data)
        assert "reloc.CODE" not in _fixture_custom_names(data)
        observed.append(data)
        return _facts_provider(data)

    result = wasm_link_optimize._post_link_optimize(
        source, facts_provider=provider, preserve_debug=True
    )
    assert observed
    assert _facts_provider(result).function_exports == {"molt_main": 0}
    assert _fixture_custom_names(result) == [".debug_info"]


def test_scanner_expected_identity_is_checked_before_snapshot(tmp_path: Path) -> None:
    scanner = tmp_path / "scanner"
    scanner.write_bytes(b"replacement scanner")
    scratch = tmp_path / "snapshot"
    with pytest.raises(ValueError, match="expected input identity"):
        wasm_link_fact_provider._snapshot_rust_wasm_facts_scanner(
            scanner,
            scratch,
            expected_sha256="0" * 64,
        )
    assert not scratch.exists()
