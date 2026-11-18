"""Import bedrock freeze-contract gates (design doc 69 §12.2) — PR1 set.

G1  test_module_registry_projection_digests      — every projection carries the
    same registry_digest, and `tools/check_module_registry.py --check` fails
    closed on any drift.
G3  test_init_reachable_only_via_table           — prepared native backend IR
    contains no direct calls of module init bodies and no isolate string_eq
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
from molt.compiler_analysis.python_import_redirects import NO_IMPORT_REDIRECTS
from molt import compiled_symbols

from molt.cli.build_output_layout import _runtime_facts_target
from molt.cli.config_resolution import _select_codegen_backend

from tests.process_guard_common import run_guarded_test_process

import json
import re
import struct
import sys
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.cli import module_registry as authority  # noqa: E402
from molt.cli import required_features  # noqa: E402
from molt.cli.models import _FrontendIntegrationState  # noqa: E402
from molt.cli.output import fail  # noqa: E402
from molt.frontend import SimpleTIRGenerator  # noqa: E402
from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION  # noqa: E402
from molt.cli.module_registry import (  # noqa: E402
    ModuleRegistry,
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
MOLT_IR_RS = ROOT / "runtime" / "molt-ir" / "src" / "ir.rs"
RUNTIME_SRC = ROOT / "runtime" / "molt-runtime" / "src"


def _sample_registry():
    return build_module_registry(
        [
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=True,
                name="demo",
                kind="source",
                init_symbol="pyinit.demo",
                origin="/app/demo.py",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=True,
                name="pkg",
                kind="source",
                init_symbol="pyinit.pkg",
                is_package=True,
                origin="/app/pkg/__init__.py",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="pkg.sub",
                kind="source",
                init_symbol="pyinit.pkg__sub",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="ext._native",
                kind="extension",
                init_symbol="pyinit.ext___native",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="os.path",
                kind="alias",
                alias_of="pkg.sub",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="sys",
                kind="runtime_builtin",
                init_symbol="pyinit.sys",
            ),
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="lazy_row",
                kind="source",
                init_symbol="",
            ),
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
    old_schema = dict(json_payload, schema=2)
    assert any(
        "schema mismatch" in issue for issue in check_registry_json_payload(old_schema)
    )
    changed_location = json.loads(json.dumps(json_payload))
    changed_location["rows"][0]["flags"] ^= authority.MODULE_FLAG_HAS_LOCATION
    assert any(
        "registry_digest mismatch" in issue
        for issue in check_registry_json_payload(changed_location)
    ), "file provenance participates in the existing digest"

    # The checker CLI is the out-of-build gate: clean file passes...
    clean_path = tmp_path / "module_registry.json"
    clean_path.write_text(json.dumps(json_payload), encoding="utf-8")
    check = run_guarded_test_process(
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
    corrupted["rows"][0]["init_symbol"] = "pyinit.smuggled"
    corrupt_path = tmp_path / "module_registry_corrupt.json"
    corrupt_path.write_text(json.dumps(corrupted), encoding="utf-8")
    check = run_guarded_test_process(
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
                "init_symbol": "pyinit.pkg__sub",
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


@pytest.mark.parametrize("flags", [None, False, True, -1, 256, 0x40, 0x80, 1.0, "0"])
def test_module_registry_json_checker_rejects_untyped_or_unknown_flags(flags) -> None:
    payload = _sample_registry().registry_json_payload()
    payload["rows"][0]["flags"] = flags
    assert any(
        "flags must be a required integer byte" in issue
        for issue in check_registry_json_payload(payload)
    )


def test_module_registry_json_checker_requires_flags_and_admits_all_known_bits() -> (
    None
):
    payload = _sample_registry().registry_json_payload()
    del payload["rows"][0]["flags"]
    assert any(
        "flags must be a required integer byte" in issue
        for issue in check_registry_json_payload(payload)
    )
    for flags in range(authority.MODULE_FLAG_KNOWN_MASK + 1):
        payload["rows"][0]["flags"] = flags
        assert not any(
            "flags must be a required integer byte" in issue
            for issue in check_registry_json_payload(payload)
        )


def test_runtime_builtin_declaration_requires_initializer() -> None:
    with pytest.raises(ValueError, match="runtime builtin.*requires init_symbol"):
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="builtins",
            kind="runtime_builtin",
        )


@pytest.mark.parametrize("missing", ["init_symbol", "flags"])
def test_runtime_builtin_projection_requires_body_even_with_valid_digest(missing):
    payload = _sample_registry().registry_json_payload()
    row = next(row for row in payload["rows"] if row["name"] == "sys")
    row[missing] = "" if missing == "init_symbol" else 0
    names = [row["name"] for row in payload["rows"]]
    canonical_rows = [
        {
            **row,
            "parent": None if row["parent"] is None else names[row["parent"]],
            "alias_target": (
                None if row["alias_target"] is None else names[row["alias_target"]]
            ),
        }
        for row in payload["rows"]
    ]
    payload["registry_digest"] = authority.registry_digest_for_rows(
        canonical_rows, schema=payload["schema"]
    )
    assert check_registry_json_payload(payload) == [
        "runtime builtin registry row 'sys' requires an init body"
    ]


def test_module_registry_blob_layout_matches_schema() -> None:
    registry = _sample_registry()
    payload = registry.backend_ir_payload()
    blob = bytes(payload["blob"])
    magic, schema, count = struct.unpack_from("<QII", blob, 0)
    assert magic == authority.MODULE_REGISTRY_MAGIC
    assert schema == authority.MODULE_REGISTRY_SCHEMA_VERSION
    assert count == len(registry.rows)
    assert len(payload["rows"]) == count
    (names_len,) = struct.unpack_from("<Q", blob, 32)
    (origins_len,) = struct.unpack_from("<Q", blob, 40)
    rows_end = (
        authority.MODULE_REGISTRY_HEADER_BYTES
        + count * authority.MODULE_REGISTRY_ROW_BYTES
    )
    (portions_len,) = struct.unpack_from("<Q", blob, 48)
    (globals_len,) = struct.unpack_from("<Q", blob, 56)
    roots_fingerprint, manifest_fingerprint, seed, buckets = struct.unpack_from(
        "<QQQI", blob, 64
    )
    assert roots_fingerprint == authority.RUNTIME_ROOTS_FINGERPRINT
    assert manifest_fingerprint == authority.INTRINSIC_MANIFEST_FINGERPRINT
    # The perfect hash tables follow the rows: every name hashes to its id.
    disps = [
        struct.unpack_from("<II", blob, rows_end + 8 * bucket)
        for bucket in range(buckets)
    ]
    slots_start = rows_end + 8 * buckets
    slots = struct.unpack_from(f"<{count}I", blob, slots_start)
    for row in registry.rows:
        g, h1, h2 = authority.phf_hashes(
            authority._fnv1a64(row.name.encode()), seed, count
        )
        slot = authority.phf_slot(h1, h2, *disps[g % buckets], count)
        assert slots[slot] == row.id
    rows_end = slots_start + 4 * count
    assert len(blob) == rows_end + names_len + origins_len + portions_len + globals_len
    # Name table is the sorted id-order concatenation.
    names = blob[rows_end : rows_end + names_len].decode("utf-8")
    offset = 0
    for index, row in enumerate(registry.rows):
        assert row.id == index
        assert payload["rows"][index] == {
            "name": row.name,
            "init_symbol": row.init_symbol or None,
        }
        assert names[offset : offset + len(row.name)] == row.name
        name_offset, name_length = struct.unpack_from(
            "<II",
            blob,
            authority.MODULE_REGISTRY_HEADER_BYTES
            + index * authority.MODULE_REGISTRY_ROW_BYTES,
        )
        assert (
            blob[rows_end + name_offset : rows_end + name_offset + name_length].decode(
                "utf-8"
            )
            == row.name
        )
        runtime_root = blob[
            authority.MODULE_REGISTRY_HEADER_BYTES
            + index * authority.MODULE_REGISTRY_ROW_BYTES
            + 26
        ]
        # A row the runtime imports carries its generated RuntimeRoot index.
        assert runtime_root == (
            authority.RUNTIME_ROOTS.index(row.name)
            if row.name in authority.RUNTIME_ROOTS
            else authority.NO_RUNTIME_ROOT
        )
        offset += len(row.name)
    origins = blob[rows_end + names_len : rows_end + names_len + origins_len].decode(
        "utf-8"
    )
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
    assert "pyinit.smuggled" not in reloc_symbols
    lazy_row = registry.row_of("lazy_row")
    assert lazy_row is not None and lazy_row.init_symbol == ""
    package_row = registry.row_of("pkg")
    assert package_row is not None
    assert package_row.flags & authority.MODULE_FLAG_PACKAGE
    assert package_row.flags & authority.MODULE_FLAG_HAS_LOCATION
    assert registry.row_of("demo").flags & authority.MODULE_FLAG_HAS_LOCATION
    assert not registry.row_of("pkg.sub").flags & authority.MODULE_FLAG_HAS_LOCATION
    assert not registry.row_of("sys").flags & authority.MODULE_FLAG_HAS_LOCATION
    assert set(payload["init_symbols"]) == reloc_symbols


def test_namespace_registry_portions_are_ordered_typed_digest_and_wire_facts() -> None:
    def registry(portions):
        return build_module_registry(
            [
                ModuleRegistryEntry(
                    name="pkg",
                    kind="namespace_parent",
                    has_location=False,
                    is_stdlib_provider=False,
                    is_package=True,
                    portions=portions,
                ),
            ]
        )

    first = registry(("/a/pkg", "/b/pkg"))
    reverse = registry(("/b/pkg", "/a/pkg"))
    assert first.digest != reverse.digest
    payload = first.registry_json_payload()
    assert check_registry_json_payload(payload) == []
    assert payload["rows"][0]["portions"] == ["/a/pkg", "/b/pkg"]
    blob, relocs = first._blob_and_relocs()
    assert not relocs
    # Independent wire oracle: ordered length-prefixed directories, no origin.
    assert blob[-20:] == b"\x06\x00\x00\x00/a/pkg\x06\x00\x00\x00/b/pkg"
    assert struct.unpack_from("<Q", blob, 40) == (0,)
    assert struct.unpack_from(
        "<II", blob, authority.MODULE_REGISTRY_HEADER_BYTES + 36
    ) == (0, 20)
    payload["rows"][0]["portions"].reverse()
    assert any(
        "registry_digest mismatch" in problem
        for problem in check_registry_json_payload(payload)
    )
    for changes in (
        {"kind": "source"},
        {"init_symbol": "pyinit.pkg"},
        {"origin": "/a/pkg"},
        {"has_location": True},
        {"is_package": False},
        {"portions": ()},
    ):
        fields = dict(
            name="pkg",
            kind="namespace_parent",
            has_location=False,
            is_stdlib_provider=False,
            is_package=True,
            portions=("/a/pkg",),
        )
        fields.update(changes)
        with pytest.raises(ValueError):
            ModuleRegistryEntry(**fields)


def test_global_slot_layout_is_a_sorted_typed_digest_and_wire_fact() -> None:
    def registry(names: tuple[str, ...]) -> authority.ModuleRegistry:
        return build_module_registry(
            [
                ModuleRegistryEntry(
                    name="demo",
                    kind="source",
                    has_location=True,
                    is_stdlib_provider=False,
                    init_symbol="pyinit.demo",
                    origin="/app/demo.py",
                    globals=names,
                ),
            ]
        )

    first = registry(("count", "math"))
    assert first.digest != registry(("count",)).digest
    payload = first.registry_json_payload()
    assert check_registry_json_payload(payload) == []
    assert payload["rows"][0]["globals"] == ["count", "math"]
    blob, _ = first._blob_and_relocs()
    # Independent wire oracle: the header's globals length, the row span, and
    # the length-prefixed names at the end of the blob.
    wire = b"\x05\x00\x00\x00count\x04\x00\x00\x00math"
    assert blob.endswith(wire)
    assert struct.unpack_from("<Q", blob, 56) == (len(wire),)
    assert struct.unpack_from(
        "<II", blob, authority.MODULE_REGISTRY_HEADER_BYTES + 48
    ) == (0, len(wire))
    payload["rows"][0]["globals"] = ["math", "count"]
    assert any("globals" in problem for problem in check_registry_json_payload(payload))
    for names in (("math", "count"), ("x", "x"), ("__name__",), ("",)):
        with pytest.raises(ValueError):
            registry(names)


def test_module_global_layout_collects_the_module_body_names() -> None:
    from molt.cli.frontend_integration import module_global_layout

    def const(out: str, value: str) -> dict[str, str]:
        return {"kind": "const_str", "out": out, "s_value": value}

    funcs = [
        {
            "name": "chunk",
            "params": ["__molt_module_obj__"],
            "ops": [
                const("a", "total"),
                {"kind": "module_get_global", "args": ["m", "a"], "out": "v"},
                const("b", "__name__"),
                {"kind": "module_set_attr", "args": ["__molt_module_obj__", "b", "v"]},
                const("c", "index"),
                {"kind": "module_set_attr", "args": ["__molt_module_obj__", "c", "v"]},
                const("d", "gone"),
                {"kind": "module_del_global", "args": ["__molt_module_obj__", "d"]},
                const("x", "foreign"),
                {"kind": "module_set_attr", "args": ["other_module", "x", "v"]},
            ],
        },
        {
            "name": "f",
            "ops": [
                {"kind": "call", "s_value": "molt_globals_builtin", "out": "g"},
                const("e", "counter"),
                {"kind": "dict_set", "args": ["g", "e", "v"]},
                const("f", "unrelated"),
                {"kind": "dict_set", "args": ["other", "f", "v"]},
            ],
        },
    ]
    assert module_global_layout(funcs) == ("counter", "gone", "index", "total")


def test_global_slot_stamps_name_each_slotted_read_and_store() -> None:
    from molt.cli.frontend_integration import module_global_layout, stamp_global_slots

    def const(out: str, value: str) -> dict[str, str]:
        return {"kind": "const_str", "out": out, "s_value": value}

    chunk = {
        "name": "chunk",
        "params": ["__molt_module_obj__"],
        "ops": [
            const("a", "total"),
            {"kind": "module_get_global", "args": ["__molt_module_obj__", "a"]},
            const("b", "index"),
            {"kind": "module_set_attr", "args": ["__molt_module_obj__", "b", "v"]},
            const("c", "__doc__"),
            {"kind": "module_set_attr", "args": ["__molt_module_obj__", "c", "v"]},
            const("d", "gone"),
            {"kind": "module_del_global", "args": ["__molt_module_obj__", "d"]},
        ],
    }
    func = {
        "name": "f",
        "ops": [
            {"kind": "call", "s_value": "molt_globals_builtin", "out": "g"},
            const("e", "total"),
            {"kind": "dict_set", "args": ["g", "e", "v"]},
        ],
    }
    layout = module_global_layout([chunk, func])
    assert layout == ("gone", "index", "total")
    functions = [chunk, func]
    stamp_global_slots(functions, {"demo": [chunk, func]}, {"demo": layout}, {"demo"})
    # Copy-on-write: the admitted bodies are unchanged; the program holds copies.
    assert all("global_slot" not in op for body in (chunk, func) for op in body["ops"])
    assert functions[0] is not chunk and functions[1] is not func
    stamped = [
        (op["kind"], op.get("global_slot"))
        for body in functions
        for op in body["ops"]
        if op["kind"] != "const_str" and op["kind"] != "call"
    ]
    assert stamped == [
        ("module_get_global", {"module": "demo", "index": 2}),
        ("module_set_attr", {"module": "demo", "index": 1}),
        ("module_set_attr", None),
        ("module_del_global", None),
        ("dict_set", {"module": "demo", "index": 2}),
    ]
    # A module without a registry row keeps every op by name.
    other = {
        "name": "o",
        "ops": [const("a", "x"), {"kind": "module_get_global", "args": ["m", "a"]}],
    }
    program = [other]
    stamp_global_slots(program, {"other": [other]}, {"other": ("x",)}, {"demo"})
    assert program[0] is other


# ─── G7: one writer, one reader — constants synchronized ────────────────────


def _rust_const(source: str, name: str) -> str:
    match = re.search(rf"const {name}[^=]*=\s*([^;]+);", source)
    assert match, f"missing const {name}"
    return match.group(1).strip()


def test_perfect_hash_function_agrees_with_the_runtime_reader() -> None:
    """Golden values shared with `perfect_hash_golden_values_match_the_writer`
    in module_table/tests.rs: the writer and the reader hash names alike."""
    assert authority.phf_hashes(authority._fnv1a64(b"os"), 1, 7) == (
        328570632,
        4,
        3,
    )
    assert authority.phf_hashes(authority._fnv1a64(b"importlib.util"), 42, 862) == (
        2304228059,
        774,
        151,
    )


def test_fixed_flag_is_the_redirect_fact_per_row() -> None:
    """M4: a row is Fixed exactly when ImportRedirects names no writer of it;
    a key writer of ``pkg`` redirects ``pkg`` and ``pkg.sub``, not ``pkgx``."""
    from molt.compiler_analysis.python_import_redirects import (
        ImportWriter,
        import_redirects,
    )

    writer = ImportWriter("main:3", 'sys.modules["pkg"] =')
    redirects = import_redirects((), (("pkg", writer),))
    entries = [
        ModuleRegistryEntry(
            name=name,
            kind="source",
            has_location=True,
            is_stdlib_provider=False,
            fixed=not redirects.redirect(name),
        )
        for name in ("pkg", "pkg.sub", "pkgx")
    ]
    registry = build_module_registry(entries)
    fixed = {
        row.name: bool(row.flags & authority.MODULE_FLAG_FIXED) for row in registry.rows
    }
    assert fixed == {"pkg": False, "pkg.sub": False, "pkgx": True}
    payload = registry.registry_json_payload()
    assert check_registry_json_payload(payload) == []


def test_module_registry_schema_authority_is_synchronized() -> None:
    reader = MODULE_TABLE_RS.read_text(encoding="utf-8")
    assert _rust_const(reader, "MODULE_REGISTRY_SCHEMA_VERSION") == str(
        authority.MODULE_REGISTRY_SCHEMA_VERSION
    )
    assert (
        _rust_const(reader, "MODULE_REGISTRY_MAGIC")
        == 'u64::from_le_bytes(*b"MOLTMOD7")'
    )
    assert authority.MODULE_REGISTRY_MAGIC == int.from_bytes(b"MOLTMOD7", "little")
    assert _rust_const(reader, "MODULE_FLAG_FIXED") == "0x20"
    assert authority.MODULE_FLAG_FIXED == 0x20
    assert _rust_const(reader, "NO_RUNTIME_ROOT") == "0xFF"
    assert authority.NO_RUNTIME_ROOT == 0xFF
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
    assert _rust_const(reader, "MODULE_FLAG_HAS_LOCATION") == "0x08"
    assert authority.MODULE_FLAG_STDLIB_PROVIDER == 0x10
    assert _rust_const(reader, "MODULE_FLAG_STDLIB_PROVIDER") == "0x10"
    assert authority.MODULE_FLAG_HAS_LOCATION == 0x08
    for kind_name, code in (
        ("MODULE_KIND_SOURCE", authority.MODULE_KIND_SOURCE),
        ("MODULE_KIND_EXTENSION", authority.MODULE_KIND_EXTENSION),
        ("MODULE_KIND_ALIAS", authority.MODULE_KIND_ALIAS),
        ("MODULE_KIND_NAMESPACE_PARENT", authority.MODULE_KIND_NAMESPACE_PARENT),
        ("MODULE_KIND_RUNTIME_BUILTIN", authority.MODULE_KIND_RUNTIME_BUILTIN),
    ):
        assert _rust_const(reader, kind_name) == str(code)


def test_module_registry_file_provenance_uses_existing_namespace_plan() -> None:
    from molt.cli import backend_ir

    # Identical-looking source paths do not imply identical roles. The
    # namespace fact is the same admitted input consumed by the frontend.
    graph = {
        "entry": Path("/virtual/entry.py"),
        "sys": Path("/virtual/sys.py"),
    }
    registry = backend_ir._build_module_registry(
        entry_module="entry",
        module_graph=graph,
        logical_source_path_by_module={name: str(path) for name, path in graph.items()},
        module_source_role_by_module={name: "user" for name in graph},
        namespace_portions_by_module={
            "namespace": ("/virtual/namespace", "/other/namespace")
        },
        module_order=list(graph),
        runtime_import_dispatch_roots=set(graph),
        native_module_init_specs=(),
        generated_native_init_modules=(),
        spawn_enabled=False,
        global_layouts={},
        import_redirects=NO_IMPORT_REDIRECTS,
    )
    for name in ("entry", "__main__", "sys"):
        assert registry.row_of(name).flags & authority.MODULE_FLAG_HAS_LOCATION
    namespace = registry.row_of("namespace")
    assert namespace.kind == "namespace_parent"
    # No writer redirects any row of this program, so every row is Fixed.
    assert (
        namespace.flags == authority.MODULE_FLAG_PACKAGE | authority.MODULE_FLAG_FIXED
    )
    assert namespace.origin == namespace.init_symbol == ""
    assert namespace.portions == ("/virtual/namespace", "/other/namespace")
    assert registry.ensure_lane_id("namespace") is None


def test_registry_provider_role_comes_from_resolved_source_not_module_or_path_spelling(
    tmp_path: Path,
) -> None:
    from molt.cli import backend_ir, module_graph

    root = tmp_path / "stdlib"
    external = tmp_path / "project"
    boundary = tmp_path / "stdlib-suffix"
    for directory in (root, external, boundary):
        directory.mkdir()
        (directory / "msilib.py").write_text("marker = 42\n", encoding="utf-8")
    for directory, expected in ((root, True), (external, False), (boundary, False)):
        graph = {"msilib": directory / "msilib.py"}
        metadata = module_graph._build_module_graph_metadata(
            graph,
            stdlib_root=root,
            entry_module="msilib",
            entry_execution_kind="script",
            namespace_portions_by_module={},
            scan_authority=None,
        )
        registry = backend_ir._build_module_registry(
            entry_module="msilib",
            module_graph=graph,
            logical_source_path_by_module=metadata.logical_source_path_by_module,
            module_source_role_by_module=metadata.module_source_role_by_module,
            namespace_portions_by_module=metadata.namespace_portions_by_module,
            module_order=list(graph),
            runtime_import_dispatch_roots=set(graph),
            native_module_init_specs=(),
            generated_native_init_modules=(),
            spawn_enabled=False,
            global_layouts={},
            import_redirects=NO_IMPORT_REDIRECTS,
        )
        assert (
            bool(
                registry.row_of("msilib").flags & authority.MODULE_FLAG_STDLIB_PROVIDER
            )
            is expected
        )
        # Only the fixed program-entry body is a direct-file capability.
        # The named catalog row above keeps its actual provider policy.
        assert not (
            registry.row_of("__main__").flags & authority.MODULE_FLAG_STDLIB_PROVIDER
        )
        assert (
            registry.row_of("__main__").init_symbol
            == registry.row_of("msilib").init_symbol
        )
        assert registry.row_of("__main__").origin == registry.row_of("msilib").origin
        assert check_registry_json_payload(registry.registry_json_payload()) == []


def test_source_provider_flag_is_required_closed_boolean_and_digest_fact() -> None:
    from dataclasses import replace

    base = ModuleRegistryEntry(
        name="msilib",
        kind="source",
        has_location=True,
        is_stdlib_provider=False,
        init_symbol="pyinit.msilib",
        origin="/admitted/msilib.py",
    )
    user = build_module_registry([base])
    stdlib = build_module_registry([replace(base, is_stdlib_provider=True)])
    assert user.digest != stdlib.digest
    assert not user.row_of("msilib").flags & authority.MODULE_FLAG_STDLIB_PROVIDER
    assert stdlib.row_of("msilib").flags & authority.MODULE_FLAG_STDLIB_PROVIDER
    for invalid in (None, 0, 1, "stdlib"):
        with pytest.raises(TypeError, match="is_stdlib_provider"):
            replace(base, is_stdlib_provider=invalid)


def test_module_registry_blob_symbol_is_one_name_everywhere() -> None:
    symbol = authority.MODULE_REGISTRY_BLOB_SYMBOL
    assert f'"{symbol}"' in MOLT_IR_RS.read_text(encoding="utf-8")
    for path in (CRANELIFT_EMITTER_RS, LLVM_EMITTER_RS):
        text = path.read_text(encoding="utf-8")
        assert "molt_ir::MODULE_REGISTRY_BLOB_SYMBOL" in text, (
            f"{path} must emit {symbol}"
        )
    stub = NATIVE_MAIN_STUB.read_text(encoding="utf-8")
    assert f"extern const unsigned char {symbol}[];" in stub
    ready = authority.MODULE_READY_SYMBOL
    assert f"extern unsigned long long {ready}[];" in stub
    assert f"molt_module_registry_install({symbol}, {ready});" in stub
    assert f'"{ready}"' in MOLT_IR_RS.read_text(encoding="utf-8")
    reader = MODULE_TABLE_RS.read_text(encoding="utf-8")
    assert 'pub extern "C" fn molt_module_registry_install' in reader


# ─── G3: init bodies reachable only through MODULE_INIT_TABLE ───────────────


def _prepare_binary_ir(
    tmp_path: Path,
    *,
    target: str = "native",
    gc_ops: list[dict] | None = None,
    builtins_ops: list[dict] | None = None,
    dispatch_roots: set[str] | None = None,
):
    from molt.cli import backend_ir as BACKEND_IR

    entry_path = tmp_path / "demo.py"
    gc_path = tmp_path / "gc.py"
    machinery_path = tmp_path / "machinery.py"
    sys_path = tmp_path / "sys.py"
    builtins_path = tmp_path / "builtins.py"
    unrelated_path = tmp_path / "unrelated.py"
    entry_path.write_text("import gc\n", encoding="utf-8")
    for path in (gc_path, machinery_path, sys_path, builtins_path, unrelated_path):
        path.write_text("", encoding="utf-8")
    module_graph = {
        "demo": entry_path,
        "gc": gc_path,
        "importlib.machinery": machinery_path,
        "sys": sys_path,
        "builtins": builtins_path,
        "unrelated": unrelated_path,
    }
    module_order = ["sys", "builtins", "gc", "importlib.machinery", "unrelated", "demo"]
    integration_state = _FrontendIntegrationState(
        functions=[
            {
                "name": compiled_symbols.module_init_symbol(module_name),
                "params": [],
                "return_abi": "void",
                "ops": (
                    [*(gc_ops or ()), {"kind": "ret_void"}]
                    if module_name == "gc"
                    else [*(builtins_ops or ()), {"kind": "ret_void"}]
                    if module_name == "builtins"
                    else [{"kind": "ret_void"}]
                ),
            }
            for module_name in module_order
        ],
        known_classes={},
    )
    prepared, error = BACKEND_IR._prepare_backend_ir(
        validate_inputs=lambda: None,
        entry_module="demo",
        module_graph=module_graph,
        logical_source_path_by_module={
            name: str(path) for name, path in module_graph.items()
        },
        namespace_portions_by_module={},
        module_source_role_by_module={name: "user" for name in module_graph},
        known_modules=set(module_graph),
        stdlib_allowlist=set(module_graph),
        integration_state=integration_state,
        fail=fail,
        json_output=True,
        module_order=module_order,
        runtime_import_dispatch_roots={"gc"}
        if dispatch_roots is None
        else dispatch_roots,
        spawn_enabled=False,
        pgo_profile_summary=None,
        runtime_feedback_summary=None,
        emit_ir_path=None,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        stdlib_profile="full",
        target=_select_codegen_backend(target, "auto", build_profile="release")[0],
        facts_target=_runtime_facts_target(
            _select_codegen_backend(target, "auto", build_profile="release")[0]
        ),
    )
    assert error is None, error
    assert prepared is not None
    return prepared


def _ensure_call_module_name(ops: list, ensure_call: dict) -> str | None:
    """Trace the actual ensure ABI argument to its symbolic module owner."""
    box_by_out = {
        op.get("out"): op
        for op in ops
        if isinstance(op, dict) and op.get("kind") == "box"
    }
    slot_by_out = {
        op.get("out"): op.get("s_value")
        for op in ops
        if isinstance(op, dict) and op.get("kind") == "module_slot"
    }
    assert all("value" not in op for op in ops if op.get("kind") == "module_slot")
    box_op = box_by_out.get(ensure_call["args"][0])
    if box_op is None:
        return None
    return slot_by_out.get(box_op["args"][0])


def test_init_reachable_only_via_table(tmp_path: Path) -> None:
    prepared = _prepare_binary_ir(tmp_path)
    ir = prepared.ir
    function_names = {func["name"] for func in ir["functions"]}
    # The string_eq dispatch chain is deleted on the native lane.
    assert "molt_isolate_import" not in function_names

    # Each init body is the function whose symbol is its own module's init.
    init_symbols = {
        func["name"]
        for func in ir["functions"]
        if isinstance(func.get("module"), str)
        and func["name"] == compiled_symbols.module_init_symbol(func["module"])
    }
    direct_init_calls = [
        (func["name"], op.get("s_value"))
        for func in ir["functions"]
        for op in func["ops"]
        if op.get("kind") == "call" and op.get("s_value") in init_symbols
    ]
    assert direct_init_calls == [], (
        "init bodies must be reachable only through MODULE_INIT_TABLE "
        f"(invariant I5); found direct call edges: {direct_init_calls}"
    )

    # The entry dispatch resolves the __main__ symbolic row at final emission.
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
    assert "__main__" in {
        _ensure_call_module_name(molt_main_ops, op) for op in ensure_calls
    }, "molt_main must ensure the __main__ registry row"

    # The IR document projection carries the digest (G7 cross-check shape).
    assert ir["module_registry"]["registry_digest"] == registry.digest
    assert set(ir["module_registry"]["init_symbols"]) == set(registry.init_symbols())

    # No trampoline: __main__ and the entry module share one init body.
    assert compiled_symbols.module_init_symbol("__main__") not in function_names
    demo_row = registry.row_of("demo")
    main_row = registry.row_of("__main__")
    assert demo_row is not None and main_row is not None
    assert demo_row.flags & authority.MODULE_FLAG_HAS_LOCATION
    assert main_row.flags & authority.MODULE_FLAG_HAS_LOCATION
    assert main_row.init_symbol == demo_row.init_symbol


def test_table_only_init_root_closes_required_runtime_features(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    closures: list[frozenset[str]] = []
    reachable = required_features.reachable_function_names

    def counting_reachable(
        functions: Sequence[Mapping[str, object]],
        *,
        extra_roots: Iterable[str] = (),
    ) -> frozenset[str]:
        closure = reachable(functions, extra_roots=extra_roots)
        closures.append(closure)
        return closure

    monkeypatch.setattr(
        required_features, "reachable_function_names", counting_reachable
    )
    prepared = _prepare_binary_ir(
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
    # One whole-program closure serves both the profile refusal and the link
    # features.
    assert len(closures) == 1


@pytest.mark.parametrize("target", ["native", "llvm", "wasm"])
def test_runtime_builtin_initializers_do_not_require_explicit_import_roots(
    tmp_path: Path, target: str
) -> None:
    prepared = _prepare_binary_ir(
        tmp_path,
        target=target,
        dispatch_roots=set(),
        builtins_ops=[
            {"kind": "builtin_func", "s_value": "molt_re_compile", "out": "v0"}
        ],
    )
    registry = prepared.module_registry
    assert registry is not None
    payload = registry.backend_ir_payload()
    for name in ("builtins", "sys"):
        row = registry.row_of(name)
        assert row is not None and row.kind == "runtime_builtin"
        assert row.flags & authority.MODULE_FLAG_HAS_BODY
        assert registry.ensure_lane_id(name) == row.id
        symbol = compiled_symbols.module_init_symbol(name)
        assert row.init_symbol == symbol
        assert symbol in payload["init_symbols"]
        assert payload["rows"][row.id] == {"name": row.name, "init_symbol": symbol}
        assert symbol in {reloc[1] for reloc in payload["relocs"]}
    unrelated = registry.row_of("unrelated")
    assert unrelated is not None and unrelated.kind == "source"
    assert not unrelated.flags & authority.MODULE_FLAG_HAS_BODY
    assert registry.ensure_lane_id("unrelated") is None
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
                    "s_value": "pyinit.smuggled_module",
                    "args": [],
                    "out": "v0",
                },
                {"kind": "ret_void"},
            ],
        },
        # An init body the registry does not own: reachable only by the
        # direct call above, which the rewrite must refuse.
        {
            "name": "pyinit.smuggled_module",
            "module": "smuggled_module",
            "params": [],
            "ops": [{"kind": "ret_void"}],
        },
    ]
    borrowed = functions[0]
    before = json.loads(json.dumps(borrowed))
    issue = BACKEND_IR._rewrite_native_import_lanes(functions, registry)
    assert borrowed == before
    assert functions[0] is borrowed
    assert issue is not None
    assert "pyinit.smuggled_module" in issue


def _fixed_registry(*fixed: str) -> ModuleRegistry:
    """``_sample_registry``'s rows with ``fixed`` rows marked Fixed."""
    rows = [
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=True,
            name="demo",
            kind="source",
            init_symbol="pyinit.demo",
            origin="/app/demo.py",
            fixed="demo" in fixed,
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=True,
            name="pkg",
            kind="source",
            init_symbol="pyinit.pkg",
            is_package=True,
            origin="/app/pkg/__init__.py",
            fixed="pkg" in fixed,
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="pkg.sub",
            kind="source",
            init_symbol="pyinit.pkg__sub",
            fixed="pkg.sub" in fixed,
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="nslane",
            kind="namespace_parent",
            is_package=True,
            portions=("/app/nslane",),
            fixed="nslane" in fixed,
        ),
    ]
    return build_module_registry(rows)


def _import_name_site(
    name: str,
    *,
    fromlist: tuple[str, ...] = (),
    level: int = 0,
    form: str = "dunder",
    out: str = "v9",
) -> list[dict[str, Any]]:
    """The frontend's ``import_name`` and its protocol prelude."""
    if form != "dunder":
        return [
            {"kind": "const_str", "s_value": name, "out": "v1"},
            {"kind": "const_none", "out": "v2"},
            {
                "kind": "import_name",
                "s_value": name,
                "value": 0,
                "fromlist": [],
                "import_form": form,
                "args": ["v1", "v2"],
                "out": out,
            },
        ]
    items = [
        {"kind": "const_str", "s_value": item, "out": f"v{20 + index}"}
        for index, item in enumerate(fromlist)
    ]
    return [
        {"kind": "const_str", "s_value": name, "out": "v1"},
        {"kind": "const_none", "out": "v2"},
        {"kind": "const_none", "out": "v3"},
        *items,
        {"kind": "tuple_new", "args": [item["out"] for item in items], "out": "v4"},
        {"kind": "const", "value": level, "out": "v5"},
        {
            "kind": "import_name",
            "s_value": name,
            "value": level,
            "fromlist": list(fromlist),
            "import_form": "dunder",
            "args": ["v1", "v2", "v3", "v4", "v5"],
            "out": out,
        },
    ]


def _lower(
    ops: list[dict[str, Any]], registry: ModuleRegistry | None
) -> tuple[list[dict[str, Any]], list[str]]:
    from molt.cli import backend_ir as BACKEND_IR
    from molt.compiler_analysis.python_import_redirects import NO_IMPORT_REDIRECTS

    functions = [{"name": "probe", "params": [], "ops": [*ops, {"kind": "ret_void"}]}]
    borrowed = json.loads(json.dumps(functions[0]))
    diagnostics = BACKEND_IR.lower_import_names(
        functions, registry, NO_IMPORT_REDIRECTS
    )
    lowered = functions[0]["ops"]
    assert all(op.get("kind") != "import_name" for op in lowered)
    assert json.loads(json.dumps(borrowed)) == borrowed
    return lowered, diagnostics


def _closed_rows(ops: list[dict[str, Any]]) -> list[str | None]:
    return [
        _ensure_call_module_name(ops, op)
        for op in ops
        if op.get("kind") == "module_import_closed"
    ]


def _protocol_calls(ops: list[dict[str, Any]]) -> list[dict[str, Any]]:
    return [
        op
        for op in ops
        if op.get("kind") == "call"
        and op.get("s_value")
        in {"molt_importlib_import_transaction", "molt_importlib_import_module"}
    ]


def test_a_fixed_import_lowers_to_the_closed_op_and_drops_its_prelude() -> None:
    """IC3: every row Fixed -> module_import_closed; no transaction call and
    no prelude constant survives (the warm path calls nothing)."""
    ops, diagnostics = _lower(_import_name_site("demo"), _fixed_registry("demo"))
    assert _closed_rows(ops) == ["demo"]
    assert _protocol_calls(ops) == []
    assert {op["kind"] for op in ops} == {
        "module_slot",
        "box",
        "module_import_closed",
        "ret_void",
    }
    assert diagnostics == []


def test_a_dotted_import_ensures_the_leaf_then_binds_the_root() -> None:
    ops, _ = _lower(
        _import_name_site("pkg.sub", out="bound"), _fixed_registry("pkg", "pkg.sub")
    )
    assert _closed_rows(ops) == ["pkg.sub", "pkg"]
    (bound,) = [op for op in ops if op.get("out") == "bound"]
    assert bound["kind"] == "module_import_closed"
    assert _ensure_call_module_name(ops, bound) == "pkg"


@pytest.mark.parametrize(
    ("site", "fixed"),
    [
        (_import_name_site("pkg.sub"), ("pkg.sub",)),  # the bound root has a writer
        (_import_name_site("demo"), ()),
        (_import_name_site("not_in_registry"), ("demo",)),
        (_import_name_site("nslane"), ("nslane",)),  # no init lane
        (_import_name_site("pkg.sub", level=1), ("pkg", "pkg.sub")),
        (_import_name_site("demo", form="module"), ()),
    ],
    ids=[
        "root-written",
        "written",
        "unregistered",
        "no-lane",
        "relative",
        "module-written",
    ],
)
def test_a_site_naming_a_written_row_keeps_its_exact_protocol_call(
    site: list[dict[str, Any]], fixed: tuple[str, ...]
) -> None:
    """IC3's other half: a site naming any non-Fixed row becomes its protocol
    call with the frontend's own operands, prelude intact."""
    (original,) = [op for op in site if op["kind"] == "import_name"]
    ops, _ = _lower(site, _fixed_registry(*fixed))
    assert _closed_rows(ops) == []
    (call,) = _protocol_calls(ops)
    assert call["args"] == original["args"]
    assert call["out"] == original["out"]
    assert [op for op in ops if op["kind"] != "call"][:-1] == [
        op for op in site if op["kind"] != "import_name"
    ]


def test_a_compiler_root_is_closed_whenever_the_row_has_an_init_lane() -> None:
    """M9: a compiler root never consults the finders; ensure owns it, and the
    runtime sets no word for a non-Fixed row, so Fixed is not required."""
    ops, _ = _lower(_import_name_site("demo", form="root"), _fixed_registry())
    assert _closed_rows(ops) == ["demo"]
    ops, _ = _lower(_import_name_site("nslane", form="root"), _fixed_registry())
    assert _closed_rows(ops) == []
    assert len(_protocol_calls(ops)) == 1


def test_a_closed_package_from_import_keeps_fromlist_preparation() -> None:
    """``from pkg import sub``: CPython's _handle_fromlist imports a missing
    child, so the closed site keeps the preparation with the prelude's name
    and tuple; a non-package module needs none."""
    ops, _ = _lower(
        _import_name_site("pkg", fromlist=("sub",), out="m"), _fixed_registry("pkg")
    )
    assert _closed_rows(ops) == ["pkg"]
    (prepare,) = [
        op for op in ops if op.get("s_value") == "molt_importlib_prepare_fromlist"
    ]
    assert prepare["args"] == ["v1", "m", "v4"]
    assert {op.get("out") for op in ops} >= {"v1", "v4", "v20"}
    ops, _ = _lower(_import_name_site("demo", fromlist=("x",)), _fixed_registry("demo"))
    assert _closed_rows(ops) == ["demo"]
    assert all(
        op.get("kind") == "module_slot" or op.get("kind") != "call" for op in ops
    )
    assert all(op.get("kind") != "tuple_new" for op in ops)


def test_without_a_registry_every_site_keeps_the_protocol() -> None:
    ops, _ = _lower(_import_name_site("demo"), None)
    assert len(_protocol_calls(ops)) == 1


def test_a_rebound_dunder_import_routes_statements_through_the_frame_hook() -> None:
    """D3: with a ``builtins.__import__`` writer, a statement that keeps the
    protocol reads the frame's ``__import__`` (CPython ``IMPORT_NAME``) with
    the same operands; a literal ``import_module`` never consults it."""
    from molt.cli import backend_ir as BACKEND_IR
    from molt.compiler_analysis.python_import_redirects import (
        ImportWriter,
        import_redirects,
    )

    writer = ImportWriter("main:3", "write of builtins.__import__")
    redirects = import_redirects((writer,), import_hook_writers=(writer,))
    functions = [
        {
            "name": "probe",
            "params": [],
            "ops": [
                *_import_name_site("demo"),
                *_import_name_site("demo", form="module", out="v21"),
                {"kind": "ret_void"},
            ],
        }
    ]
    diagnostics = BACKEND_IR.lower_import_names(functions, _fixed_registry(), redirects)
    calls = [op for op in functions[0]["ops"] if op.get("kind") == "call"]
    assert [call["s_value"] for call in calls] == [
        "molt_importlib_import_statement",
        "molt_importlib_import_module",
    ]
    assert calls[0]["args"] == ["v1", "v2", "v3", "v4", "v5"]
    assert diagnostics == [f"probe: import demo: protocol: {writer}"]
    # Without the hook writer the same written row keeps the transaction.
    functions[0]["ops"] = [*_import_name_site("demo"), {"kind": "ret_void"}]
    BACKEND_IR.lower_import_names(
        functions, _fixed_registry(), import_redirects((writer,))
    )
    assert [
        op["s_value"] for op in functions[0]["ops"] if op.get("kind") == "call"
    ] == ["molt_importlib_import_transaction"]


def test_rewrite_preserves_public_transactions_and_siteless_functions() -> None:
    from molt.cli import backend_ir as BACKEND_IR

    registry = _sample_registry()
    # `import pkg.sub` is this function's only import site: the transaction
    # call names its direct runtime callee and nothing else marks the function.
    plain_import: list[dict[str, Any]] = [
        {"kind": "const_str", "s_value": "pkg.sub", "out": "v1"},
        {"kind": "const_none", "out": "v2"},
        {"kind": "tuple_new", "args": [], "out": "v3"},
        {"kind": "const", "value": 0, "out": "v4"},
        {
            "kind": "call",
            "s_value": "molt_importlib_import_transaction",
            "args": ["v1", "v2", "v2", "v3", "v4"],
            "out": "v5",
        },
        {"kind": "ret_void"},
    ]
    siteless: list[dict[str, Any]] = [
        {"kind": "builtin_func", "s_value": "molt_len", "out": "v0"},
        {"kind": "call", "s_value": "molt_print_obj", "args": [], "out": "v1"},
        {"kind": "call_func", "args": ["v0"], "out": "v2"},
        {"kind": "ret_void"},
    ]
    functions = [
        {"name": "importer", "params": [], "ops": plain_import},
        {"name": "plain", "params": [], "ops": siteless},
    ]

    assert BACKEND_IR._rewrite_native_import_lanes(functions, registry) is None

    # Catalog admission does not prove the public finder binding. Preserve
    # the complete transaction (including globals/fromlist/level), not only a
    # replacement opcode that happens to return the same module on a cache hit.
    assert functions[0]["ops"] is plain_import
    assert functions[1]["ops"] is siteless


@pytest.mark.parametrize("module", ["pkg", "pkg.sub"])
def test_frontend_plain_imports_carry_their_protocol_operands(module: str) -> None:
    """The frontend emits one typed ``import_name`` whose operands are the
    public transaction's (name, globals, locals, fromlist, level); a site that
    keeps the protocol is that call, unchanged."""
    import ast
    from molt.cli import backend_ir
    from molt.compiler_analysis.python_import_redirects import NO_IMPORT_REDIRECTS

    generator = SimpleTIRGenerator(
        module_name="consumer",
        known_modules={"pkg", "pkg.sub"},
        stdlib_allowlist={"pkg", "pkg.sub"},
    )
    generator.visit(ast.parse(f"def probe():\n    import {module}\n"))
    functions = [
        function
        for function in generator.to_json()["functions"]
        if any(op.get("kind") == "import_name" for op in function["ops"])
    ]
    (original,) = functions
    (site,) = [op for op in original["ops"] if op.get("kind") == "import_name"]
    assert site["s_value"] == module
    assert site["value"] == 0 and site["fromlist"] == []
    assert site["import_form"] == "dunder"
    assert len(site["args"]) == 5
    assert all(
        op.get("s_value") != "molt_importlib_import_transaction"
        for op in original["ops"]
    )
    backend_ir.lower_import_names(functions, _sample_registry(), NO_IMPORT_REDIRECTS)
    (call,) = [
        op
        for op in functions[0]["ops"]
        if op.get("s_value") == "molt_importlib_import_transaction"
    ]
    assert call["args"] == site["args"] and call["out"] == site["out"]


@pytest.mark.parametrize("statement", ["from pkg import sub", "import missing"])
def test_frontend_non_fixed_or_unregistered_imports_keep_transactions(
    statement: str,
) -> None:
    import ast
    from molt.cli import backend_ir
    from molt.compiler_analysis.python_import_redirects import NO_IMPORT_REDIRECTS

    generator = SimpleTIRGenerator(
        module_name="consumer", known_modules={"pkg", "pkg.sub", "missing"}
    )
    generator.visit(ast.parse(f"def probe():\n    {statement}\n"))
    functions = generator.to_json()["functions"]
    backend_ir.lower_import_names(functions, _sample_registry(), NO_IMPORT_REDIRECTS)
    assert any(
        op.get("s_value") == "molt_importlib_import_transaction"
        for function in functions
        for op in function["ops"]
    )
    assert not any(
        op.get("kind") in {"import_name", "module_import_closed"}
        for function in functions
        for op in function["ops"]
    )


def test_observable_intrinsic_import_calls_keep_transaction_semantics() -> None:
    import ast
    from copy import deepcopy
    from molt.cli import backend_ir

    generator = SimpleTIRGenerator(module_name="consumer")
    generator.visit(
        ast.parse(
            "from _intrinsics import require_intrinsic\n"
            "transaction = require_intrinsic('molt_importlib_import_transaction')\n"
            "transaction('pkg.sub', globals(), None, (), 0)\n"
        )
    )
    functions = generator.to_json()["functions"]
    before = deepcopy(functions)
    assert any(
        op["kind"] == "builtin_func"
        and op.get("s_value") == "molt_importlib_import_transaction"
        for function in functions
        for op in function["ops"]
    )
    assert (
        backend_ir._rewrite_native_import_lanes(functions, _sample_registry()) is None
    )
    assert functions == before


def test_compiled_import_transaction_spelling_keeps_its_definition() -> None:
    from copy import deepcopy
    from molt.cli import backend_ir

    symbol = "molt_importlib_import_transaction"
    functions = [
        {
            "name": "caller",
            "params": [],
            "ops": [
                {"kind": "const_str", "s_value": "pkg", "out": "name"},
                {"kind": "const_none", "out": "none"},
                {"kind": "tuple_new", "args": [], "out": "fromlist"},
                {"kind": "const", "value": 0, "out": "level"},
                {
                    "kind": "call",
                    "s_value": symbol,
                    "args": ["name", "none", "none", "fromlist", "level"],
                    "out": "result",
                },
                {"kind": "ret", "args": ["result"]},
            ],
        },
        {
            "name": symbol,
            "params": ["name", "globals", "locals", "fromlist", "level"],
            "ops": [{"kind": "ret", "args": ["name"]}],
        },
    ]
    before = deepcopy(functions)
    assert (
        backend_ir._rewrite_native_import_lanes(functions, _sample_registry()) is None
    )
    assert functions == before


def test_literal_import_owner_survives_registry_growth_and_aliases() -> None:
    from copy import deepcopy
    from molt.cli import backend_ir
    from molt.compiler_analysis.python_import_redirects import NO_IMPORT_REDIRECTS

    entries = [
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="pkg",
            kind="source",
            init_symbol="pyinit.pkg",
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="pkg.sub",
            kind="source",
            init_symbol="pyinit.pkg_sub",
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="alias",
            kind="alias",
            alias_of="pkg.sub",
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="__main__",
            kind="source",
            init_symbol="pyinit.entry",
        ),
        ModuleRegistryEntry(
            is_stdlib_provider=False,
            has_location=False,
            name="entry",
            kind="source",
            init_symbol="pyinit.entry",
        ),
    ]
    original: list[dict[str, Any]] = [
        {
            "name": "consumer",
            "params": [],
            "ops": [
                {"kind": "const_str", "s_value": "alias", "out": "v0"},
                {"kind": "const_none", "out": "v10"},
                {
                    "kind": "import_name",
                    "s_value": "alias",
                    "value": 0,
                    "fromlist": [],
                    "import_form": "root",
                    "args": ["v0", "v10"],
                    "out": "v1",
                },
                {"kind": "call", "s_value": "pyinit.entry", "args": [], "out": "v2"},
                {"kind": "const_none", "out": "v3"},
                {"kind": "const_str", "s_value": "pkg.sub", "out": "v5"},
                {"kind": "tuple_new", "args": [], "out": "v6"},
                {"kind": "const", "value": 0, "out": "v7"},
                {
                    "kind": "call",
                    "s_value": "molt_importlib_import_transaction",
                    "args": ["v5", "v3", "v3", "v6", "v7"],
                    "out": "v8",
                },
                {"kind": "ret_void"},
            ],
        }
    ]
    snapshots = []
    bindings = []
    for additional in [
        [],
        [
            ModuleRegistryEntry(
                is_stdlib_provider=False,
                has_location=False,
                name="aaa",
                kind="source",
                init_symbol="pyinit.aaa",
            )
        ],
    ]:
        registry = build_module_registry(entries + additional)
        functions = deepcopy(original)
        assert backend_ir._rewrite_native_import_lanes(functions, registry) is None
        backend_ir.lower_import_names(functions, registry, NO_IMPORT_REDIRECTS)
        snapshots.append(functions)
        bindings.append(registry.id_of("alias"))
        assert registry.id_of("alias") != registry.id_of("pkg.sub")
        ops = functions[0]["ops"]
        calls = [op for op in ops if op.get("s_value") == "molt_module_ensure"]
        assert [_ensure_call_module_name(ops, op) for op in calls] == ["__main__"]
        assert _closed_rows(ops) == ["alias"]
        assert original[0]["ops"][-2] in ops, (
            "ordinary transaction is independent of catalog growth"
        )
    assert bindings[0] != bindings[1]
    assert snapshots[0] == snapshots[1]


# ─── Ensure is the only state-transition owner (structural gate) ────────────


def _strip_line_comments(text: str) -> str:
    return "\n".join(
        line.split("//", 1)[0] if "//" in line else line for line in text.splitlines()
    )


def test_module_ensure_is_the_only_state_transition_owner() -> None:
    owner = MODULE_TABLE_RS
    owner_tests = RUNTIME_SRC / "builtins" / "module_table" / "tests.rs"
    # Tests stay inside the same private owner module behind its actual
    # cfg(test) declaration; production files retain the strict scan below.
    owner_source = owner.read_text(encoding="utf-8")
    assert re.search(
        r'#\[cfg\(test\)\]\s*#\[path\s*=\s*"module_table/tests.rs"\]\s*pub\(crate\)\s+mod tests;',
        owner_source,
    )
    # Generated ABI wrappers forward to this one state-transition body.
    assert owner_source.count("fn molt_module_ensure_attached_body(") == 1
    offenders: list[str] = []
    for path in RUNTIME_SRC.rglob("*.rs"):
        if path in (owner, owner_tests):
            continue
        text = _strip_line_comments(path.read_text(encoding="utf-8", errors="replace"))
        if "fn molt_module_ensure_attached_body(" in text:
            offenders.append(f"{path}: defines molt_module_ensure_attached_body")
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
                    f"{needle} (only the module owner reconciles initialization metadata)"
                )
    # Publication and deferred detachment have exactly one sanctioned caller:
    # module_cache_set/remove in builtins/modules.rs. Detachment returns an
    # owned reference for release after every namespace owner agrees.
    modules_rs = (RUNTIME_SRC / "builtins" / "modules.rs").read_text(encoding="utf-8")
    assert "publish_initializing_namespace" in modules_rs
    assert "detach_cache_publication" in modules_rs
    for path in RUNTIME_SRC.rglob("*.rs"):
        if path in (owner, owner_tests, RUNTIME_SRC / "builtins" / "modules.rs"):
            continue
        text = _strip_line_comments(path.read_text(encoding="utf-8", errors="replace"))
        if (
            "publish_initializing_namespace" in text
            or "detach_cache_publication" in text
        ):
            offenders.append(f"{path}: unsanctioned initializer-publication caller")
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
            assert '#[link(wasm_import_module = "env")]' not in code
            assert (
                '#[link_name = molt_codegen_abi::attached_symbol!("molt_isolate_import")]'
                in code
            )
            assert (
                "fn molt_isolate_import(context: u64, module_id: u64) -> u64;" in code
            )
            assert "molt_isolate_import(_py.attached_context(), u64::from(id))" in code
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
