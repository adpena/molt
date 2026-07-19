#!/usr/bin/env python3
"""Generate heap-kind IDs and lifetime descriptors from runtime/heap_kinds.toml.

The table is the sole authority for the cross-crate numeric ABI and for cold-path
lifetime policy. Fixed builtins resolve through a dense array; subtype-specific
TYPE_ID_OBJECT behavior is selected by an immutable shape descriptor without
enlarging the hot object header.

Usage::

    python tools/gen_heap_kinds.py
    python tools/gen_heap_kinds.py --check
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import tomllib
from pathlib import Path

from generator_io import generated_file_matches, write_generated_text
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

ROOT = Path(__file__).resolve().parents[1]
TABLE = ROOT / "runtime" / "heap_kinds.toml"
OUT_CODEGEN = ROOT / "runtime" / "molt-codegen-abi" / "src" / "heap_kinds_generated.rs"
OUT_CORE = ROOT / "runtime" / "molt-runtime-core" / "src" / "heap_kinds_generated.rs"
OUT_RUNTIME = (
    ROOT / "runtime" / "molt-runtime" / "src" / "object" / "heap_kinds_generated.rs"
)
OUT_AUDIT = ROOT / "runtime" / "heap_kinds.generated.json"
OUT_PYTHON = ROOT / "src" / "molt" / "heap_kinds_generated.py"
OUTPUTS = (OUT_CODEGEN, OUT_CORE, OUT_RUNTIME, OUT_AUDIT, OUT_PYTHON)

FIELDS = ("layout", "edges", "cycle", "weakref", "shape", "drop", "metrics")
PUBLICATION_POLICIES = {"python", "linear_unpublished"}
EXTERNAL_GC_POLICIES = {"none", "opaque_rust_arc", "cpython_bridge"}
ACYCLIC_CAPABILITIES = {"none", "int_triplet", "code_metadata"}
ACYCLIC_EDGE_DOMAINS = {"int", "str", "bytes_or_none", "str_tuple", "str_or_none"}
ACYCLIC_SLOT_SCHEMAS = {
    "RANGE": (
        ("start", "int"),
        ("stop", "int"),
        ("step", "int"),
    ),
    "CODE": (
        ("filename", "str"),
        ("name", "str"),
        ("linetable", "bytes_or_none"),
        ("varnames", "str_tuple"),
        ("names", "str_tuple"),
        ("arg_names", "str_tuple"),
        ("posonly", "int"),
        ("kwonly", "str_tuple"),
        ("vararg", "str_or_none"),
        ("varkw", "str_or_none"),
    ),
}
ALLOWED = {
    "layout": {
        "async_generator",
        "boxed",
        "boxed_bits",
        "boxed_i64",
        "boxed_u8",
        "code",
        "dict",
        "dynamic",
        "exception",
        "fixed_bits",
        "foreign",
        "function",
        "inline",
        "inline_rust",
        "iterator",
        "memoryview",
        "module",
        "object",
        "set",
        "tuple",
        "type",
        "vec_bits",
        "vec_u8",
        "generator",
    },
    "edges": {"none", "fixed", "dynamic", "shape", "custom"},
    "cycle": {"never", "always", "dynamic"},
    "weakref": {"deny", "allow", "class"},
    "shape": {"fixed", "class", "sidecar"},
    "drop": {
        "none",
        "object_shape",
        "string",
        "list",
        "list_builder",
        "dict",
        "dict_builder",
        "tuple",
        "dict_view",
        "iter",
        "bytearray",
        "range",
        "slice",
        "exception",
        "dataclass",
        "buffer2d",
        "context_manager",
        "file_handle",
        "memoryview",
        "function",
        "bound_method",
        "module",
        "type",
        "generator",
        "classmethod",
        "staticmethod",
        "property",
        "super",
        "set",
        "set_builder",
        "frozenset",
        "bigint",
        "enumerate",
        "callargs",
        "call_iter",
        "reversed",
        "zip",
        "map",
        "filter",
        "code",
        "generic_alias",
        "async_generator",
        "union",
        "list_int",
        "list_bool",
        "traceback_payload",
        "native_handle",
        "glob_iter",
        "foreign",
        "weak_container",
    },
    "metrics": {
        "none",
        "object",
        "string",
        "list",
        "dict",
        "tuple",
        "exception",
        "callargs",
        "bigint",
    },
}
TRACK_PROJECTIONS = {
    "never",
    "always",
    "dict_dynamic",
    "tuple_dynamic",
}
OBJECT_SHAPE_FAMILIES = {
    "plain",
    "task",
    "dict_subclass",
    "operator",
    "functools",
    "types",
    "itertools",
}
OBJECT_SHAPE_RESOURCE_SLOTS = {"none", "io_socket", "websocket"}


def _variant(value: str) -> str:
    return "".join(part.capitalize() for part in value.split("_"))


def load_table(path: Path = TABLE) -> list[dict[str, object]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if data.get("schema_version") != 1:
        raise ValueError("heap-kind schema_version must be 1")
    kinds = data.get("kind")
    if not isinstance(kinds, list) or not kinds:
        raise ValueError("heap-kind table must contain [[kind]] rows")
    names: set[str] = set()
    ids: set[int] = set()
    for row in kinds:
        missing = {"name", "id", *FIELDS} - row.keys()
        if missing:
            raise ValueError(f"heap-kind row missing {sorted(missing)}: {row!r}")
        name = row["name"]
        type_id = row["id"]
        if not isinstance(name, str) or re.fullmatch(r"[A-Z][A-Z0-9_]*", name) is None:
            raise ValueError(f"invalid heap-kind name: {name!r}")
        if not isinstance(type_id, int) or not (0 < type_id <= 0xFFFF_FFFF):
            raise ValueError(f"invalid type id for {name}: {type_id!r}")
        if name in names or type_id in ids:
            raise ValueError(f"duplicate heap-kind name or id: {name}={type_id}")
        for field, allowed in ALLOWED.items():
            if row[field] not in allowed:
                raise ValueError(f"invalid {field} for {name}: {row[field]!r}")
        publication = row.get("publication", "python")
        if publication not in PUBLICATION_POLICIES:
            raise ValueError(f"invalid publication policy for {name}: {publication!r}")
        row["publication"] = publication
        external_gc = row.get("external_gc", "none")
        if external_gc not in EXTERNAL_GC_POLICIES:
            raise ValueError(f"invalid external GC policy for {name}: {external_gc!r}")
        if external_gc != "none" and row["edges"] != "none":
            raise ValueError(
                f"external-custody kind {name} cannot declare Molt-owned edges"
            )
        row["external_gc"] = external_gc
        if row["cycle"] == "dynamic" and "track" not in row:
            raise ValueError(
                f"dynamic heap kind {name} requires an explicit track projection"
            )
        if row["cycle"] != "dynamic" and "track" in row:
            raise ValueError(
                f"non-dynamic heap kind {name} must derive track from cycle"
            )
        projection = row.get("track", row["cycle"])
        if projection not in TRACK_PROJECTIONS:
            raise ValueError(f"invalid track projection for {name}: {projection!r}")
        row["track"] = projection
        requires_acyclic_capability = (
            row["cycle"] == "never"
            and row["edges"] != "none"
            and publication != "linear_unpublished"
        )
        acyclic = row.get("acyclic_capability", "none")
        if acyclic not in ACYCLIC_CAPABILITIES:
            raise ValueError(f"invalid acyclic capability for {name}: {acyclic!r}")
        if requires_acyclic_capability and acyclic == "none":
            raise ValueError(f"GREEN ref-holder {name} requires an acyclic capability")
        if not requires_acyclic_capability and acyclic != "none":
            raise ValueError(
                f"heap kind {name} cannot carry acyclic capability {acyclic!r}"
            )
        expected_acyclic = {"RANGE": "int_triplet", "CODE": "code_metadata"}.get(
            str(name), "none"
        )
        if acyclic != expected_acyclic:
            raise ValueError(
                f"heap kind {name} requires acyclic capability {expected_acyclic!r}, got {acyclic!r}"
            )
        row["acyclic_capability"] = acyclic
        expected_slots = dict(ACYCLIC_SLOT_SCHEMAS.get(str(name), ()))
        slots = row.get("acyclic_slots", {})
        if not isinstance(slots, dict) or not all(
            isinstance(slot, str) and isinstance(domain, str)
            for slot, domain in slots.items()
        ):
            raise ValueError(f"invalid acyclic slot table for {name}: {slots!r}")
        unknown_domains = set(slots.values()) - ACYCLIC_EDGE_DOMAINS
        if unknown_domains:
            raise ValueError(
                f"invalid acyclic edge domains for {name}: {sorted(unknown_domains)!r}"
            )
        if slots != expected_slots:
            raise ValueError(
                f"heap kind {name} requires exact acyclic slots {expected_slots!r}, got {slots!r}"
            )
        row["acyclic_slots"] = slots
        expected_external = {
            "NATIVE_HANDLE": "opaque_rust_arc",
            "FOREIGN": "cpython_bridge",
        }.get(str(name), "none")
        if external_gc != expected_external:
            raise ValueError(
                f"heap kind {name} requires external GC capability {expected_external!r}, got {external_gc!r}"
            )
        names.add(name)
        ids.add(type_id)
    kinds.sort(key=lambda row: int(row["id"]))
    if [(row["name"], row["id"]) for row in kinds if int(row["id"]) < 200] != [
        ("OBJECT", 100)
    ]:
        raise ValueError("OBJECT=100 must be the only sparse pre-200 heap kind")
    dense_ids = [int(row["id"]) for row in kinds if int(row["id"]) >= 200]
    if dense_ids != list(range(200, dense_ids[-1] + 1)):
        raise ValueError(
            "builtin heap IDs must remain dense from 200 through MAX_HEAP_TYPE_ID"
        )
    pins = {"OBJECT": 100, "FUNCTION": 221, "TYPE": 224, "LIST_BOOL": 250}
    by_name = {str(row["name"]): int(row["id"]) for row in kinds}
    for name, expected in pins.items():
        if by_name.get(name) != expected:
            raise ValueError(f"codegen ABI pin drift: {name} must remain {expected}")
    return kinds


def load_object_shapes(path: Path = TABLE) -> list[dict[str, object]]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    rows = data.get("object_shape")
    if not isinstance(rows, list) or not rows:
        raise ValueError("heap-kind table must contain [[object_shape]] rows")
    names: set[str] = set()
    ids: set[int] = set()
    for row in rows:
        name = row.get("name")
        shape_id = row.get("id")
        family = row.get("family")
        resource_slot0 = row.get("resource_slot0")
        if not isinstance(name, str) or re.fullmatch(r"[A-Z][A-Z0-9_]*", name) is None:
            raise ValueError(f"invalid object-shape name: {name!r}")
        if not isinstance(shape_id, int) or not (0 <= shape_id <= 0xFFFF):
            raise ValueError(f"invalid object-shape id for {name}: {shape_id!r}")
        if name in names or shape_id in ids:
            raise ValueError(f"duplicate object-shape name or id: {name}={shape_id}")
        if family not in OBJECT_SHAPE_FAMILIES:
            raise ValueError(f"invalid object-shape family for {name}: {family!r}")
        if resource_slot0 not in OBJECT_SHAPE_RESOURCE_SLOTS:
            raise ValueError(
                f"invalid object-shape resource_slot0 for {name}: {resource_slot0!r}"
            )
        if resource_slot0 != "none" and family != "task":
            raise ValueError(f"resource-bearing object shape {name} must be a task")
        names.add(name)
        ids.add(shape_id)
    rows.sort(key=lambda row: int(row["id"]))
    if (rows[0]["name"], rows[0]["id"]) != ("PLAIN", 0):
        raise ValueError("PLAIN=0 must be the default object shape")
    return rows


def _header() -> str:
    return "// @generated by tools/gen_heap_kinds.py from runtime/heap_kinds.toml. DO NOT EDIT.\n\n"


def render_constants(kinds: list[dict[str, object]], visibility: str = "pub") -> str:
    lines = [_header()]
    for row in kinds:
        lines.append(f"{visibility} const TYPE_ID_{row['name']}: u32 = {row['id']};\n")
    lines.append("\n")
    lines.append(f"{visibility} const MIN_HEAP_TYPE_ID: u32 = TYPE_ID_STRING;\n")
    lines.append(
        f"{visibility} const MAX_HEAP_TYPE_ID: u32 = TYPE_ID_{kinds[-1]['name']};\n"
    )
    lines.append(
        f"{visibility} const ALL_HEAP_TYPE_IDS: [u32; {len(kinds)}] = ["
        + ", ".join(f"TYPE_ID_{row['name']}" for row in kinds)
        + "];\n"
    )
    return "".join(lines)


def render_object_shapes(shapes: list[dict[str, object]]) -> str:
    lines = [
        "\n#[repr(u16)]\n",
        "#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]\n",
        "pub enum ObjectShapeId {\n",
    ]
    for row in shapes:
        default = "    #[default]\n" if row["id"] == 0 else ""
        lines.append(
            f"{default}    {_variant(str(row['name']).lower())} = {row['id']},\n"
        )
    lines.append("}\n\nimpl ObjectShapeId {\n")
    lines.append(f"    pub const MAX_ID: u16 = {shapes[-1]['id']};\n\n")
    lines.append(
        "    #[inline(always)]\n    pub const fn from_u16(value: u16) -> Option<Self> {\n"
    )
    lines.append("        Some(match value {\n")
    for row in shapes:
        lines.append(
            f"            {row['id']} => Self::{_variant(str(row['name']).lower())},\n"
        )
    lines.append("            _ => return None,\n        })\n    }\n}\n\n")
    lines.append(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n"
        "pub enum ObjectShapeLifecycleFamily {\n"
    )
    for family in sorted(OBJECT_SHAPE_FAMILIES):
        lines.append(f"    {_variant(family)},\n")
    lines.append("}\n\n")
    lines.append(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n"
        "pub enum ObjectShapeResourceSlot {\n"
    )
    for resource in sorted(OBJECT_SHAPE_RESOURCE_SLOTS):
        lines.append(f"    {_variant(resource)},\n")
    lines.append("}\n\n")
    lines.append(
        "#[inline(always)]\n"
        "pub const fn object_shape_lifecycle_family(shape: ObjectShapeId) "
        "-> ObjectShapeLifecycleFamily {\n    match shape {\n"
    )
    for row in shapes:
        lines.append(
            f"        ObjectShapeId::{_variant(str(row['name']).lower())} => "
            f"ObjectShapeLifecycleFamily::{_variant(str(row['family']))},\n"
        )
    lines.append("    }\n}\n\n")
    lines.append(
        "#[inline(always)]\n"
        "pub const fn object_shape_resource_slot(shape: ObjectShapeId) "
        "-> ObjectShapeResourceSlot {\n    match shape {\n"
    )
    for row in shapes:
        lines.append(
            f"        ObjectShapeId::{_variant(str(row['name']).lower())} => "
            f"ObjectShapeResourceSlot::{_variant(str(row['resource_slot0']))},\n"
        )
    lines.append("    }\n}\n\n")
    lines.append(
        "#[inline(always)]\n"
        "pub const fn object_shape_is_task(shape: ObjectShapeId) -> bool {\n"
        "    matches!(object_shape_lifecycle_family(shape), ObjectShapeLifecycleFamily::Task)\n"
        "}\n"
    )
    return "".join(lines)


def _enum(name: str, values: list[str]) -> str:
    variants = "\n".join(f"    {_variant(value)}," for value in values)
    return f"#[derive(Clone, Copy, Debug, Eq, PartialEq)]\npub(crate) enum {name} {{\n{variants}\n}}\n\n"


def render_runtime(kinds: list[dict[str, object]]) -> str:
    lines = [render_constants(kinds, "pub(crate)")]
    enum_fields = {
        "layout": "HeapLayoutPolicy",
        "edges": "HeapEdgePolicy",
        "cycle": "HeapCyclePolicy",
        "weakref": "HeapWeakrefPolicy",
        "shape": "HeapShapePolicy",
        "drop": "HeapDropPolicy",
        "metrics": "HeapMetricsPolicy",
    }
    for field, rust_name in enum_fields.items():
        values = sorted({str(row[field]) for row in kinds})
        lines.append(_enum(rust_name, values))
    lines.append(_enum("HeapPublicationPolicy", sorted(PUBLICATION_POLICIES)))
    lines.append(_enum("HeapExternalGcPolicy", sorted(EXTERNAL_GC_POLICIES)))
    lines.append(_enum("HeapAcyclicCapability", sorted(ACYCLIC_CAPABILITIES)))
    lines.append(_enum("HeapAcyclicEdgeDomain", sorted(ACYCLIC_EDGE_DOMAINS)))
    acyclic_slots = [
        (kind, slot, domain)
        for kind, slots in ACYCLIC_SLOT_SCHEMAS.items()
        for slot, domain in slots
    ]
    lines.append(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n"
        "pub(crate) enum HeapAcyclicSlot {\n"
        + "".join(
            f"    {_variant(kind.lower())}{_variant(slot)},\n"
            for kind, slot, _domain in acyclic_slots
        )
        + "}\n\n"
    )
    lines.append(_enum("HeapTrackProjection", sorted(TRACK_PROJECTIONS)))
    lines.append(
        _enum("HeapLifecycleHandler", [str(row["name"]).lower() for row in kinds])
    )
    lines.append(
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n"
        "pub(crate) struct HeapKindDescriptor {\n"
        "    pub(crate) type_id: u32,\n"
        "    pub(crate) name: &'static str,\n"
        "    pub(crate) layout: HeapLayoutPolicy,\n"
        "    pub(crate) edges: HeapEdgePolicy,\n"
        "    pub(crate) cycle: HeapCyclePolicy,\n"
        "    pub(crate) weakref: HeapWeakrefPolicy,\n"
        "    pub(crate) shape: HeapShapePolicy,\n"
        "    pub(crate) drop: HeapDropPolicy,\n"
        "    pub(crate) metrics: HeapMetricsPolicy,\n"
        "    pub(crate) track: HeapTrackProjection,\n"
        "    pub(crate) handler: HeapLifecycleHandler,\n"
        "    pub(crate) publication: HeapPublicationPolicy,\n"
        "    pub(crate) external_gc: HeapExternalGcPolicy,\n"
        "    pub(crate) acyclic: HeapAcyclicCapability,\n"
        "}\n\n"
    )
    lines.append(
        f"pub(crate) const HEAP_KIND_DESCRIPTORS: [HeapKindDescriptor; {len(kinds)}] = [\n"
    )
    for row in kinds:
        fields = ", ".join(
            f"{field}: {enum_fields[field]}::{_variant(str(row[field]))}"
            for field in enum_fields
        )
        fields += (
            f", track: HeapTrackProjection::{_variant(str(row['track']))}"
            f", handler: HeapLifecycleHandler::{_variant(str(row['name']).lower())}"
        )
        fields += (
            f", publication: HeapPublicationPolicy::{_variant(str(row['publication']))}"
        )
        fields += (
            f", external_gc: HeapExternalGcPolicy::{_variant(str(row['external_gc']))}"
        )
        fields += f", acyclic: HeapAcyclicCapability::{_variant(str(row['acyclic_capability']))}"
        lines.append(
            "    HeapKindDescriptor { "
            f'type_id: TYPE_ID_{row["name"]}, name: "{row["name"]}", {fields}'
            " },\n"
        )
    lines.append("];\n\n")
    lines.append(
        "#[inline(always)]\n"
        "pub(crate) const fn heap_kind_descriptor(type_id: u32) -> Option<&'static HeapKindDescriptor> {\n"
        "    if type_id == TYPE_ID_OBJECT {\n"
        "        return Some(&HEAP_KIND_DESCRIPTORS[0]);\n"
        "    }\n"
        "    if type_id < MIN_HEAP_TYPE_ID || type_id > MAX_HEAP_TYPE_ID {\n"
        "        return None;\n"
        "    }\n"
        "    Some(&HEAP_KIND_DESCRIPTORS[(type_id - MIN_HEAP_TYPE_ID) as usize + 1])\n"
        "}\n\n"
        "#[inline(always)]\n"
        "pub(crate) const fn is_valid_heap_type_id(type_id: u32) -> bool {\n"
        "    type_id == TYPE_ID_OBJECT || (type_id >= MIN_HEAP_TYPE_ID && type_id <= MAX_HEAP_TYPE_ID)\n"
        "}\n"
    )
    lines.append(
        "\n#[inline(always)]\n"
        "pub(crate) const fn heap_track_projection(type_id: u32) -> Option<HeapTrackProjection> {\n"
        "    match type_id {\n"
    )
    for row in kinds:
        lines.append(
            f"        TYPE_ID_{row['name']} => Some(HeapTrackProjection::{_variant(str(row['track']))}),\n"
        )
    lines.append("        _ => None,\n    }\n}\n")
    direct_policies = {
        "drop": "HeapDropPolicy",
        "metrics": "HeapMetricsPolicy",
        "weakref": "HeapWeakrefPolicy",
        "cycle": "HeapCyclePolicy",
        "layout": "HeapLayoutPolicy",
        "shape": "HeapShapePolicy",
        "publication": "HeapPublicationPolicy",
        "external_gc": "HeapExternalGcPolicy",
        "acyclic_capability": "HeapAcyclicCapability",
    }
    for field, rust_name in direct_policies.items():
        lines.append(
            "\n#[inline(always)]\n"
            f"pub(crate) const fn heap_{field}_policy(type_id: u32) -> Option<{rust_name}> {{\n"
            "    match type_id {\n"
        )
        for row in kinds:
            lines.append(
                f"        TYPE_ID_{row['name']} => Some({rust_name}::{_variant(str(row[field]))}),\n"
            )
        lines.append("        _ => None,\n    }\n}\n")
    lines.append(
        "\n#[inline(always)]\n"
        "pub(crate) const fn heap_acyclic_slot_domain(slot: HeapAcyclicSlot) "
        "-> HeapAcyclicEdgeDomain {\n"
        "    match slot {\n"
    )
    for kind, slot, domain in acyclic_slots:
        lines.append(
            f"        HeapAcyclicSlot::{_variant(kind.lower())}{_variant(slot)} => "
            f"HeapAcyclicEdgeDomain::{_variant(domain)},\n"
        )
    lines.append("    }\n}\n")
    lines.append(
        "\n#[inline(always)]\n"
        "pub(crate) const fn heap_lifecycle_handler(type_id: u32) -> Option<HeapLifecycleHandler> {\n"
        "    match type_id {\n"
    )
    for row in kinds:
        lines.append(
            f"        TYPE_ID_{row['name']} => Some(HeapLifecycleHandler::{_variant(str(row['name']).lower())}),\n"
        )
    lines.append("        _ => None,\n    }\n}\n")
    lines.append(
        "\n#[inline(always)]\n"
        "pub(crate) const fn heap_kind_uses_object_layout(type_id: u32) -> bool {\n"
        "    matches!(type_id, "
        + " | ".join(
            f"TYPE_ID_{row['name']}" for row in kinds if row["layout"] == "object"
        )
        + ")\n"
        "}\n\n"
        "pub(crate) fn heap_kind_id_by_name(name: &str) -> Option<u32> {\n"
        "    match name {\n"
    )
    for row in kinds:
        lines.append(f'        "{row["name"]}" => Some(TYPE_ID_{row["name"]}),\n')
    lines.append("        _ => None,\n    }\n}\n")
    return "".join(lines)


def render_audit(
    kinds: list[dict[str, object]],
    shapes: list[dict[str, object]],
) -> str:
    return (
        json.dumps(
            {
                "schema_version": 1,
                "source": "runtime/heap_kinds.toml",
                "kinds": kinds,
                "object_shapes": shapes,
            },
            indent=2,
            sort_keys=True,
        )
        + "\n"
    )


def render_python(kinds: list[dict[str, object]]) -> str:
    lines = [
        "# @generated by tools/gen_heap_kinds.py from runtime/heap_kinds.toml. DO NOT EDIT.\n\n"
    ]
    for row in kinds:
        lines.append(f"TYPE_ID_{row['name']} = {row['id']}\n")
    return "".join(lines)


def _format_rust(source: str) -> str:
    """Return the canonical repository rustfmt representation of generated Rust."""
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        raise RuntimeError("rustfmt is required to generate heap-kind Rust authorities")
    completed = _COMMANDS.run(
        [rustfmt, "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        text=True,
        encoding="utf-8",
        errors="replace",
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(
            f"rustfmt failed for generated heap-kind authority:\n{completed.stderr}"
        )
    return completed.stdout


def render_all(kinds: list[dict[str, object]]) -> dict[Path, str]:
    shapes = load_object_shapes()
    return {
        OUT_CODEGEN: _format_rust(render_constants(kinds)),
        OUT_CORE: _format_rust(render_constants(kinds) + render_object_shapes(shapes)),
        OUT_RUNTIME: _format_rust(render_runtime(kinds)),
        OUT_AUDIT: render_audit(kinds, shapes),
        OUT_PYTHON: render_python(kinds),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if generated outputs are stale"
    )
    args = parser.parse_args(argv)
    rendered = render_all(load_table())
    stale = False
    for path, source in rendered.items():
        if args.check:
            if not generated_file_matches(path, source):
                print(
                    f"STALE generated file: {path.relative_to(ROOT)}", file=sys.stderr
                )
                stale = True
        else:
            write_generated_text(path, source)
    return 1 if stale else 0


if __name__ == "__main__":
    raise SystemExit(main())
