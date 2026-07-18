"""Import bedrock freeze-contract gates (design doc 69 §12.2) — PR1 set.

G1  test_module_registry_projection_digests      — every projection carries the
    same registry_digest, and `tools/check_module_registry.py --check` fails
    closed on any drift.
G3  test_init_reachable_only_via_table           — prepared native backend IR
    contains no direct `call molt_init_*` edges and no isolate string_eq
    dispatch chain; init bodies are reachable only through the registry's
    MODULE_INIT_TABLE relocations.
G4  lives in Rust: runtime/molt-runtime/src/builtins/module_table.rs
    (`g4_ensure_state_machine_transitions`), proof-queue cargo lane.
G7  test_module_registry_schema_authority_is_synchronized — the blob layout
    has exactly one writer (molt.cli.module_registry) and one reader
    (module_table.rs); their constants must agree, and the native emitters and
    the C stub must reference the same blob symbol.
+   test_module_ensure_is_the_only_state_transition_owner — structural gate:
    no code outside module_table.rs defines ensure or mutates table state.
"""

from __future__ import annotations

import json
import re
import struct
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.cli import module_registry as authority  # noqa: E402
from molt.cli.module_registry import (  # noqa: E402
    ModuleRegistryEntry,
    build_module_registry,
    check_registry_json_payload,
)

MODULE_TABLE_RS = (
    ROOT / "runtime" / "molt-runtime" / "src" / "builtins" / "module_table.rs"
)
CRANELIFT_EMITTER_RS = (
    ROOT
    / "runtime"
    / "molt-backend-native"
    / "src"
    / "native_backend"
    / "simple_backend"
    / "module_registry.rs"
)
LLVM_EMITTER_RS = (
    ROOT
    / "runtime"
    / "molt-backend-native"
    / "src"
    / "llvm_backend"
    / "module_registry.rs"
)
NATIVE_MAIN_STUB = ROOT / "src" / "molt" / "cli" / "native_main_stub.py"
RUNTIME_SRC = ROOT / "runtime" / "molt-runtime" / "src"


def _sample_registry():
    return build_module_registry(
        [
            ModuleRegistryEntry(
                name="demo",
                kind="source",
                init_symbol="molt_init_demo",
                origin="/app/demo.py",
            ),
            ModuleRegistryEntry(
                name="pkg",
                kind="source",
                init_symbol="molt_init_pkg",
                is_package=True,
                origin="/app/pkg/__init__.py",
            ),
            ModuleRegistryEntry(
                name="pkg.sub", kind="source", init_symbol="molt_init_pkg__sub"
            ),
            ModuleRegistryEntry(
                name="ext._native",
                kind="extension",
                init_symbol="molt_init_ext___native",
            ),
            ModuleRegistryEntry(name="os.path", kind="alias", alias_of="pkg.sub"),
            ModuleRegistryEntry(
                name="sys", kind="runtime_builtin", init_symbol="molt_init_sys"
            ),
            ModuleRegistryEntry(name="lazy_row", kind="source", init_symbol=""),
        ]
    )


# ─── G1: projection digest discipline ───────────────────────────────────────


def test_module_registry_projection_digests(tmp_path: Path) -> None:
    registry = _sample_registry()
    json_payload = registry.registry_json_payload()
    backend_payload = registry.backend_ir_payload()

    # One digest across every projection.
    assert json_payload["registry_digest"] == registry.digest
    assert backend_payload["registry_digest"] == registry.digest
    blob = bytes(backend_payload["blob"])
    embedded_digest = blob[16:32]
    assert embedded_digest == bytes.fromhex(registry.digest[:32]), (
        "the blob header must embed the leading digest bytes"
    )

    # The emitted diagnostics projection re-derives cleanly.
    assert check_registry_json_payload(json_payload) == []

    # The checker CLI is the out-of-build gate: clean file passes...
    clean_path = tmp_path / "module_registry.json"
    clean_path.write_text(json.dumps(json_payload), encoding="utf-8")
    check = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools" / "check_module_registry.py"),
            "--check",
            str(clean_path),
        ],
        capture_output=True,
        text=True,
    )
    assert check.returncode == 0, check.stdout + check.stderr

    # ...and a single mutated row fails closed with a digest complaint.
    corrupted = json.loads(json.dumps(json_payload))
    corrupted["rows"][0]["init_symbol"] = "molt_init_smuggled"
    corrupt_path = tmp_path / "module_registry_corrupt.json"
    corrupt_path.write_text(json.dumps(corrupted), encoding="utf-8")
    check = subprocess.run(
        [
            sys.executable,
            str(ROOT / "tools" / "check_module_registry.py"),
            "--check",
            str(corrupt_path),
        ],
        capture_output=True,
        text=True,
    )
    assert check.returncode == 1, check.stdout + check.stderr
    assert "registry_digest mismatch" in check.stdout


def test_module_registry_json_checker_rejects_malformed_rows() -> None:
    payload = {
        "schema": authority.MODULE_REGISTRY_SCHEMA_VERSION,
        "registry_digest": "not-the-real-digest",
        "rows": [
            "not-a-row",
            {
                "id": 0,
                "name": "pkg.sub",
                "kind": "source",
                "parent": "pkg",
                "alias_target": 99,
                "init_symbol": "molt_init_pkg__sub",
                "flags": 0,
                "deps": "not-a-dependency-list",
            },
        ],
    }

    problems = check_registry_json_payload(payload)

    assert "row 0 is not an object" in problems
    assert "row 'pkg.sub' parent is not an integer: 'pkg'" in problems
    assert "row 'pkg.sub' alias_target index out of range: 99" in problems
    assert any(problem.startswith("registry_digest mismatch") for problem in problems)


def test_module_registry_blob_layout_matches_schema() -> None:
    registry = _sample_registry()
    payload = registry.backend_ir_payload()
    blob = bytes(payload["blob"])
    magic, schema, count = struct.unpack_from("<QII", blob, 0)
    assert magic == authority.MODULE_REGISTRY_MAGIC
    assert schema == authority.MODULE_REGISTRY_SCHEMA_VERSION
    assert count == len(registry.rows)
    (names_len,) = struct.unpack_from("<Q", blob, 32)
    (origins_len,) = struct.unpack_from("<Q", blob, 40)
    rows_end = (
        authority.MODULE_REGISTRY_HEADER_BYTES
        + count * authority.MODULE_REGISTRY_ROW_BYTES
    )
    assert len(blob) == rows_end + names_len + origins_len
    # Name table is the sorted id-order concatenation (binary-search contract).
    names = blob[rows_end : rows_end + names_len].decode("utf-8")
    offset = 0
    for row in registry.rows:
        assert names[offset : offset + len(row.name)] == row.name
        offset += len(row.name)
    origins = blob[rows_end + names_len :].decode("utf-8")
    offset = 0
    for row in registry.rows:
        assert origins[offset : offset + len(row.origin)] == row.origin
        offset += len(row.origin)
    # Every init relocation lands on a row's 8-aligned init-pointer slot.
    for reloc_offset, symbol in payload["relocs"]:
        row_offset = reloc_offset - authority.MODULE_REGISTRY_HEADER_BYTES
        assert row_offset % authority.MODULE_REGISTRY_ROW_BYTES == (
            authority.MODULE_REGISTRY_ROW_INIT_PTR_OFFSET
        )
        assert reloc_offset % 8 == 0
        row = registry.rows[row_offset // authority.MODULE_REGISTRY_ROW_BYTES]
        assert row.init_symbol == symbol
    # Rows without init symbols contribute no relocation.
    reloc_symbols = {symbol for _, symbol in payload["relocs"]}
    assert "molt_init_smuggled" not in reloc_symbols
    lazy_row = registry.row_of("lazy_row")
    assert lazy_row is not None and lazy_row.init_symbol == ""
    package_row = registry.row_of("pkg")
    assert package_row is not None
    assert package_row.flags & authority.MODULE_FLAG_PACKAGE
    assert set(payload["init_symbols"]) == reloc_symbols


# ─── G7: one writer, one reader — constants synchronized ────────────────────


def _rust_const(source: str, name: str) -> str:
    match = re.search(rf"const {name}[^=]*=\s*([^;]+);", source)
    assert match, f"missing const {name}"
    return match.group(1).strip()


def test_module_registry_schema_authority_is_synchronized() -> None:
    reader = MODULE_TABLE_RS.read_text(encoding="utf-8")
    assert _rust_const(reader, "MODULE_REGISTRY_SCHEMA_VERSION") == str(
        authority.MODULE_REGISTRY_SCHEMA_VERSION
    )
    assert (
        _rust_const(reader, "MODULE_REGISTRY_MAGIC")
        == 'u64::from_le_bytes(*b"MOLTMOD2")'
    )
    assert authority.MODULE_REGISTRY_MAGIC == int.from_bytes(b"MOLTMOD2", "little")
    assert _rust_const(reader, "MODULE_REGISTRY_HEADER_BYTES") == str(
        authority.MODULE_REGISTRY_HEADER_BYTES
    )
    assert _rust_const(reader, "MODULE_REGISTRY_ROW_BYTES") == str(
        authority.MODULE_REGISTRY_ROW_BYTES
    )
    assert _rust_const(reader, "NO_MODULE_ID") == "u32::MAX"
    assert authority.NO_MODULE_ID == 0xFFFF_FFFF
    assert _rust_const(reader, "MODULE_FLAG_PACKAGE") == "0x02"
    assert authority.MODULE_FLAG_PACKAGE == 0x02
    assert _rust_const(reader, "MODULE_FLAG_HAS_BODY") == "0x04"
    assert authority.MODULE_FLAG_HAS_BODY == 0x04
    for kind_name, code in (
        ("MODULE_KIND_SOURCE", authority.MODULE_KIND_SOURCE),
        ("MODULE_KIND_EXTENSION", authority.MODULE_KIND_EXTENSION),
        ("MODULE_KIND_ALIAS", authority.MODULE_KIND_ALIAS),
        ("MODULE_KIND_NAMESPACE_PARENT", authority.MODULE_KIND_NAMESPACE_PARENT),
        ("MODULE_KIND_RUNTIME_BUILTIN", authority.MODULE_KIND_RUNTIME_BUILTIN),
    ):
        assert _rust_const(reader, kind_name) == str(code)


def test_module_registry_blob_symbol_is_one_name_everywhere() -> None:
    symbol = authority.MODULE_REGISTRY_BLOB_SYMBOL
    for path in (CRANELIFT_EMITTER_RS, LLVM_EMITTER_RS):
        text = path.read_text(encoding="utf-8")
        assert f'"{symbol}"' in text, f"{path} must emit {symbol}"
    stub = NATIVE_MAIN_STUB.read_text(encoding="utf-8")
    assert f"extern const unsigned char {symbol}[];" in stub
    assert f"molt_module_registry_install({symbol});" in stub
    reader = MODULE_TABLE_RS.read_text(encoding="utf-8")
    assert 'pub extern "C" fn molt_module_registry_install' in reader


# ─── G3: init bodies reachable only through MODULE_INIT_TABLE ───────────────


def _prepare_native_ir(tmp_path: Path, *, gc_ops: list[dict] | None = None):
    cli = pytest.importorskip("molt.cli")
    from molt.cli import backend_ir as BACKEND_IR

    entry_path = tmp_path / "demo.py"
    gc_path = tmp_path / "gc.py"
    machinery_path = tmp_path / "machinery.py"
    sys_path = tmp_path / "sys.py"
    entry_path.write_text("import gc\n", encoding="utf-8")
    for path in (gc_path, machinery_path, sys_path):
        path.write_text("", encoding="utf-8")
    module_graph = {
        "demo": entry_path,
        "gc": gc_path,
        "importlib.machinery": machinery_path,
        "sys": sys_path,
    }
    module_order = ["sys", "gc", "importlib.machinery", "demo"]
    integration_state = cli._FrontendIntegrationState(
        functions=[
            {
                "name": cli.SimpleTIRGenerator.module_init_symbol(module_name),
                "params": [],
                "ops": (
                    [*(gc_ops or ()), {"kind": "ret_void"}]
                    if module_name == "gc"
                    else [{"kind": "ret_void"}]
                ),
            }
            for module_name in module_order
        ],
        known_classes={},
    )
    diagnostics_state = cli._MidendDiagnosticsState(
        policy_outcomes_by_function={},
        pass_stats_by_function={},
    )
    prepared, error = BACKEND_IR._prepare_backend_ir(
        entry_module="demo",
        module_graph=module_graph,
        parse_codec="json",
        type_hint_policy="ignore",
        fallback_policy="error",
        type_facts=None,
        enable_phi=True,
        known_modules=set(module_graph),
        known_classes={},
        stdlib_allowlist=set(module_graph),
        known_func_defaults={},
        known_func_kinds={},
        module_chunking=False,
        module_chunk_max_ops=0,
        optimization_profile="dev",
        pgo_hot_function_names=set(),
        frontend_phase_timeout=None,
        integration_state=integration_state,
        diagnostics_state=diagnostics_state,
        record_frontend_timing=lambda **_: None,
        fail=cli._fail,
        json_output=True,
        module_order=module_order,
        runtime_import_dispatch_roots={"gc"},
        generated_module_source_paths={},
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=cli._DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="full",
    )
    assert error is None, error
    assert prepared is not None
    return cli, prepared


def _ensure_call_module_id(ops: list, ensure_call: dict) -> int | None:
    """Trace an ensure call's boxed-int argument back to its const module id
    (`const → box → call molt_module_ensure`, the literal-site ABI shape)."""
    box_by_out = {
        op.get("out"): op
        for op in ops
        if isinstance(op, dict) and op.get("kind") == "box"
    }
    const_by_out = {
        op.get("out"): op.get("value")
        for op in ops
        if isinstance(op, dict) and op.get("kind") == "const"
    }
    box_op = box_by_out.get(ensure_call["args"][0])
    if box_op is None:
        return None
    return const_by_out.get(box_op["args"][0])


def test_init_reachable_only_via_table(tmp_path: Path) -> None:
    cli, prepared = _prepare_native_ir(tmp_path)
    ir = prepared.ir
    function_names = {func["name"] for func in ir["functions"]}
    # The string_eq dispatch chain is deleted on the native lane.
    assert "molt_isolate_import" not in function_names

    direct_init_calls = [
        (func["name"], op.get("s_value"))
        for func in ir["functions"]
        for op in func["ops"]
        if op.get("kind") == "call"
        and isinstance(op.get("s_value"), str)
        and op["s_value"].startswith("molt_init_")
    ]
    assert direct_init_calls == [], (
        "init bodies must be reachable only through MODULE_INIT_TABLE "
        f"(invariant I5); found direct call edges: {direct_init_calls}"
    )

    # The entry dispatch is ensure(const ModuleId).
    molt_main_ops = next(
        func["ops"] for func in ir["functions"] if func["name"] == "molt_main"
    )
    ensure_calls = [
        op
        for op in molt_main_ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_module_ensure"
    ]
    assert ensure_calls, "molt_main must enter the program through molt_module_ensure"
    registry = prepared.module_registry
    assert registry is not None
    main_id = registry.id_of("__main__")
    assert main_id is not None
    assert main_id in {
        _ensure_call_module_id(molt_main_ops, op) for op in ensure_calls
    }, "molt_main must ensure the __main__ registry row"

    # The IR document projection carries the digest (G7 cross-check shape).
    assert ir["module_registry"]["registry_digest"] == registry.digest
    assert set(ir["module_registry"]["init_symbols"]) == set(registry.init_symbols())

    # No trampoline: __main__ and the entry module share one init body.
    assert cli.SimpleTIRGenerator.module_init_symbol("__main__") not in function_names
    demo_row = registry.row_of("demo")
    main_row = registry.row_of("__main__")
    assert demo_row is not None and main_row is not None
    assert main_row.init_symbol == demo_row.init_symbol


def test_table_only_init_root_closes_required_runtime_features(tmp_path: Path) -> None:
    _, prepared = _prepare_native_ir(
        tmp_path,
        gc_ops=[
            {
                "kind": "builtin_func",
                "s_value": "molt_re_compile",
                "out": "v0",
            }
        ],
    )
    assert prepared.required_link_features == frozenset({"stdlib_regex"})


def test_rewrite_fails_closed_on_unregistered_init_call(tmp_path: Path) -> None:
    from molt.cli import backend_ir as BACKEND_IR

    registry = _sample_registry()
    functions = [
        {
            "name": "molt_main",
            "params": [],
            "ops": [
                {
                    "kind": "call",
                    "s_value": "molt_init_smuggled_module",
                    "args": [],
                    "out": "v0",
                },
                {"kind": "ret_void"},
            ],
        }
    ]
    issue = BACKEND_IR._rewrite_native_import_lanes(functions, registry)
    assert issue is not None
    assert "molt_init_smuggled_module" in issue


def test_rewrite_lowers_literal_module_import_to_ensure() -> None:
    from molt.cli import backend_ir as BACKEND_IR

    registry = _sample_registry()
    functions = [
        {
            "name": "molt_init_demo",
            "params": [],
            "ops": [
                {"kind": "const_str", "s_value": "pkg.sub", "out": "v0"},
                {"kind": "module_import", "args": ["v0"], "out": "v1"},
                {"kind": "check_exception", "value": 1},
                {"kind": "const_str", "s_value": "not_in_registry", "out": "v2"},
                {"kind": "module_import", "args": ["v2"], "out": "v3"},
                {"kind": "ret_void"},
            ],
        }
    ]
    issue = BACKEND_IR._rewrite_native_import_lanes(functions, registry)
    assert issue is None
    ops = functions[0]["ops"]
    ensure_calls = [
        op
        for op in ops
        if op.get("kind") == "call" and op.get("s_value") == "molt_module_ensure"
    ]
    assert len(ensure_calls) == 1
    assert ensure_calls[0]["out"] == "v1"
    assert _ensure_call_module_id(ops, ensure_calls[0]) == registry.id_of("pkg.sub")
    # Registry misses keep the dynamic module_import lane.
    remaining_imports = [op for op in ops if op.get("kind") == "module_import"]
    assert len(remaining_imports) == 1
    assert remaining_imports[0]["args"] == ["v2"]
    # The pre-existing exception edge is preserved.
    assert {"kind": "check_exception", "value": 1} in ops


# ─── Ensure is the only state-transition owner (structural gate) ────────────


def _strip_line_comments(text: str) -> str:
    return "\n".join(
        line.split("//", 1)[0] if "//" in line else line for line in text.splitlines()
    )


def test_module_ensure_is_the_only_state_transition_owner() -> None:
    owner = MODULE_TABLE_RS
    offenders: list[str] = []
    for path in RUNTIME_SRC.rglob("*.rs"):
        if path == owner:
            continue
        text = _strip_line_comments(path.read_text(encoding="utf-8", errors="replace"))
        if "fn molt_module_ensure" in text:
            offenders.append(f"{path}: defines molt_module_ensure")
        if "fn molt_module_registry_install" in text:
            offenders.append(f"{path}: defines molt_module_registry_install")
        # The table state bytes and slots may only be touched inside the
        # owner module; other code goes through the ensure/publication API.
        for needle in ("STATE_INITIALIZING", "STATE_TOMBSTONE", "STATE_REPLACED"):
            if needle in text:
                offenders.append(f"{path}: references module table state {needle}")
        for needle in ("module_table_view_replace", "module_table_view_tombstone"):
            if needle in text:
                offenders.append(
                    f"{path}: calls the sys.modules view mutation entry point "
                    f"{needle} (PR2 owns wiring it to the dict view)"
                )
    # The publication bridges have exactly one sanctioned caller: the
    # module_cache_set/del store writes in builtins/modules.rs.
    modules_rs = (RUNTIME_SRC / "builtins" / "modules.rs").read_text(encoding="utf-8")
    assert "publish_from_cache_set" in modules_rs
    assert "unpublish_from_cache_del" in modules_rs
    for path in RUNTIME_SRC.rglob("*.rs"):
        if path in (owner, RUNTIME_SRC / "builtins" / "modules.rs"):
            continue
        text = _strip_line_comments(path.read_text(encoding="utf-8", errors="replace"))
        if "publish_from_cache_set" in text or "unpublish_from_cache_del" in text:
            offenders.append(f"{path}: unsanctioned publication-bridge caller")
    assert offenders == [], "\n".join(offenders)


def test_isolate_import_is_only_the_wasm_module_id_projection() -> None:
    """The old name/string bridge cannot return on either target.

    WASM owns exactly one integer ModuleId projection at the module-table
    boundary; native reaches the same table through its relocation column.
    """
    offenders: list[str] = []
    for path in RUNTIME_SRC.rglob("*.rs"):
        code = _strip_line_comments(path.read_text(encoding="utf-8", errors="replace"))
        if "molt_isolate_import" not in code:
            continue
        if path == RUNTIME_SRC / "builtins" / "module_table.rs":
            assert '#[cfg(target_arch = "wasm32")]' in code
            assert '#[link(wasm_import_module = "env")]' in code
            assert "fn molt_isolate_import(module_id: u64) -> u64;" in code
            call_sites = [
                idx
                for idx in range(len(code))
                if code.startswith("molt_isolate_import(", idx)
            ]
            assert len(call_sites) == 2, "one declaration and one ModuleId call"
            continue
        offenders.append(str(path))
    assert offenders == [], (
        f"name/string isolate-import authority must not come back: {offenders}"
    )
