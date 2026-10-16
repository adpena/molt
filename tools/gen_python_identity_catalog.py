#!/usr/bin/env python3
"""Generate compiler-only Python identities and their acquisition/write tables."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
import sys
import tomllib
from typing import cast

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.generator_io import generator_main  # noqa: E402
from tools.intrinsic_manifest import parse_intrinsic_manifest  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src/molt/compiler_analysis/python_identity_catalog.toml"
INTRINSIC_MANIFEST = ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
OUT_PYTHON = ROOT / "src/molt/compiler_analysis/python_identity_catalog_generated.py"
OUTPUTS = (OUT_PYTHON,)


class SchemaError(ValueError):
    """The identity authority contains an ambiguous or invalid declaration."""


@dataclass(frozen=True)
class Schema:
    identity_bits: tuple[tuple[str, int], ...]
    member_bits: tuple[tuple[str, int], ...]
    builtins: tuple[tuple[str, str, str, bool], ...]
    modules: tuple[tuple[str, str], ...]
    acquisitions: tuple[tuple[str, str, str, str], ...]
    writes: tuple[tuple[str, str, tuple[str, ...]], ...]
    target_properties: tuple[tuple[str, str], ...]
    intrinsic_callables: tuple[tuple[str, str, str], ...]


def _bits(raw: object, label: str) -> tuple[tuple[str, int], ...]:
    if not isinstance(raw, dict) or not raw:
        raise SchemaError(f"{label} must be a nonempty table")
    used: set[int] = set()
    rows: list[tuple[str, int]] = []
    for name, bit in raw.items():
        if re.fullmatch(r"[A-Z][A-Z0-9_]*", name) is None:
            raise SchemaError(f"invalid {label} name {name!r}")
        if type(bit) is not int or bit < 0:
            raise SchemaError(f"{label}.{name} bit must be a nonnegative integer")
        if bit in used:
            raise SchemaError(f"duplicate {label} bit {bit}")
        used.add(bit)
        rows.append((name, bit))
    return tuple(sorted(rows, key=lambda row: row[1]))


def _rows(raw: object, label: str, keys: set[str]) -> list[dict[str, object]]:
    if not isinstance(raw, list) or not raw:
        raise SchemaError(f"{label} must be a nonempty array of tables")
    if any(not isinstance(row, dict) or set(row) != keys for row in raw):
        raise SchemaError(f"{label} rows require exactly {sorted(keys)}")
    return cast(list[dict[str, object]], raw)


def _reference(raw: object, names: set[str], label: str) -> str:
    if not isinstance(raw, str) or raw not in names:
        raise SchemaError(f"unknown {label} {raw!r}")
    return raw


def _name(raw: object, label: str, *, dotted: bool = False) -> str:
    if not isinstance(raw, str) or not all(
        part.isidentifier() for part in (raw.split(".") if dotted else (raw,))
    ):
        raise SchemaError(f"invalid {label} {raw!r}")
    return raw


def load_schema(
    path: Path = SOURCE, *, intrinsic_manifest: Path = INTRINSIC_MANIFEST
) -> Schema:
    try:
        data = tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise SchemaError(str(exc)) from exc
    if set(data) != {
        "schema_version",
        "identity_bits",
        "member_bits",
        "builtin",
        "module",
        "acquisition",
        "write",
        "target_property",
        "intrinsic_callable",
    }:
        raise SchemaError("unknown or missing identity-catalog top-level keys")
    if type(data["schema_version"]) is not int or data["schema_version"] != 1:
        raise SchemaError("schema_version must be integer 1")
    identities = _bits(data["identity_bits"], "identity")
    members = _bits(data["member_bits"], "member")
    identity_names, member_names = set(dict(identities)), set(dict(members))
    if not {"OTHER", "UNBOUND"} <= identity_names:
        raise SchemaError("identity sentinels OTHER and UNBOUND are required")
    builtins: list[tuple[str, str, str, bool]] = []
    builtin_names: set[str] = set()
    shape_identities: set[str] = set()
    for row in _rows(
        data["builtin"], "builtin", {"name", "identity", "guard", "shape"}
    ):
        name = _name(row["name"], "builtin name")
        if name in builtin_names:
            raise SchemaError(f"duplicate builtin {name!r}")
        builtin_names.add(name)
        if type(row["shape"]) is not bool:
            raise SchemaError(f"builtin {name!r} shape must be boolean")
        identity = _reference(row["identity"], identity_names, "builtin identity")
        shape = row["shape"]
        if shape:
            if identity in shape_identities:
                raise SchemaError(f"ambiguous shape identity {identity!r}")
            shape_identities.add(identity)
        builtins.append(
            (
                name,
                identity,
                _reference(row["guard"], member_names, "builtin guard"),
                shape,
            )
        )
    modules: list[tuple[str, str]] = []
    module_names: set[str] = set()
    for row in _rows(data["module"], "module", {"name", "identity"}):
        name = _name(row["name"], "module name", dotted=True)
        if name in module_names:
            raise SchemaError(f"duplicate module {name!r}")
        module_names.add(name)
        modules.append(
            (name, _reference(row["identity"], identity_names, "module identity"))
        )
    module_map = dict(modules)
    if "builtins" not in module_map:
        raise SchemaError("the builtin acquisition owner is missing")
    acquisition_keys = {(module_map["builtins"], name) for name in builtin_names}
    acquisitions: list[tuple[str, str, str, str]] = []
    for row in _rows(
        data["acquisition"], "acquisition", {"owner", "name", "identity", "guard"}
    ):
        owner = _reference(row["owner"], set(module_map.values()), "acquisition owner")
        name = _name(row["name"], "acquisition member")
        if (owner, name) in acquisition_keys:
            raise SchemaError(
                f"duplicate or contradictory acquisition {(owner, name)!r}"
            )
        acquisition_keys.add((owner, name))
        acquisitions.append(
            (
                owner,
                name,
                _reference(row["identity"], identity_names, "acquisition identity"),
                _reference(row["guard"], member_names, "acquisition guard"),
            )
        )
    writes: list[tuple[str, str, tuple[str, ...]]] = []
    write_keys: set[tuple[str, str]] = set()
    for row in _rows(data["write"], "write", {"owner", "name", "guards"}):
        owner = _reference(row["owner"], identity_names, "write owner")
        name = "*" if row["name"] == "*" else _name(row["name"], "write member")
        if (owner, name) in write_keys:
            raise SchemaError(f"duplicate write {(owner, name)!r}")
        write_keys.add((owner, name))
        guards = row["guards"]
        if not isinstance(guards, list) or not guards:
            raise SchemaError("write guards must be a nonempty array")
        checked = tuple(
            _reference(guard, member_names, "write guard") for guard in guards
        )
        if len(set(checked)) != len(checked):
            raise SchemaError("duplicate write guard")
        writes.append((owner, name, checked))
    used_guards = {guard for _, _, guard, _ in builtins}
    used_guards.update(guard for _, _, _, guard in acquisitions)
    used_guards.update(guard for _, _, guards in writes for guard in guards)
    if used_guards != member_names:
        raise SchemaError(f"orphan member guards: {sorted(member_names - used_guards)}")
    properties = data["target_property"]
    if not isinstance(properties, dict) or not properties:
        raise SchemaError("target_property must be a nonempty table")
    target_properties: list[tuple[str, str]] = []
    property_values: set[str] = set()
    for name, value in properties.items():
        if re.fullmatch(r"[A-Z][A-Z0-9_]*", name) is None:
            raise SchemaError(f"invalid target property {name!r}")
        checked = _name(value, "target property value")
        if checked in property_values:
            raise SchemaError(f"duplicate target property value {checked!r}")
        property_values.add(checked)
        target_properties.append((name, checked))
    producers = {
        entry.name: entry
        for entry in parse_intrinsic_manifest(
            intrinsic_manifest.read_text(encoding="utf-8")
        )
    }
    intrinsic_callables: list[tuple[str, str, str]] = []
    intrinsic_names: set[str] = set()
    intrinsic_identities: set[str] = set()
    used_properties: set[str] = set()
    other_callable_identities = {identity for _, identity, _, _ in builtins}
    other_callable_identities.update(identity for _, identity in modules)
    other_callable_identities.update(identity for _, _, identity, _ in acquisitions)
    other_callable_identities.update(
        {"OTHER", "UNBOUND", "INERT_VALUE", "STATIC_FALSE"}
    )
    for row in _rows(
        data["intrinsic_callable"],
        "intrinsic_callable",
        {"name", "identity", "target_property"},
    ):
        name = _name(row["name"], "intrinsic callable name")
        identity = _reference(row["identity"], identity_names, "intrinsic identity")
        target_property = _reference(
            row["target_property"], set(properties), "target property"
        )
        if name in intrinsic_names:
            raise SchemaError(f"duplicate intrinsic callable {name!r}")
        if identity in intrinsic_identities or identity in other_callable_identities:
            raise SchemaError(f"ambiguous intrinsic identity {identity!r}")
        producer = producers.get(name)
        if producer is None:
            raise SchemaError(f"intrinsic callable {name!r} has no manifest producer")
        if producer.arity != 0:
            raise SchemaError(
                f"target property intrinsic {name!r} must take no arguments"
            )
        intrinsic_names.add(name)
        intrinsic_identities.add(identity)
        used_properties.add(target_property)
        intrinsic_callables.append((name, identity, target_property))
    if used_properties != set(properties):
        raise SchemaError(
            f"orphan target properties: {sorted(set(properties) - used_properties)}"
        )
    return Schema(
        identities,
        members,
        tuple(builtins),
        tuple(modules),
        tuple(acquisitions),
        tuple(writes),
        tuple(target_properties),
        tuple(intrinsic_callables),
    )


def render_python(schema: Schema) -> str:
    lines = [
        "# @generated by tools/gen_python_identity_catalog.py from\n",
        "# src/molt/compiler_analysis/python_identity_catalog.toml. DO NOT EDIT.\n",
        '"""Closed compiler-only identities, acquisitions and mutation guards."""\n\n',
        "from __future__ import annotations\n\n",
        "from enum import Enum, IntFlag\nfrom types import MappingProxyType\n",
        "from typing import Final, Mapping, TypeAlias\n\n\n",
    ]
    for name, rows in (
        ("PythonIdentity", schema.identity_bits),
        ("PythonMember", schema.member_bits),
    ):
        lines.append(f"class {name}(IntFlag):\n")
        lines.extend(f"    {member} = 1 << {bit}\n" for member, bit in rows)
        lines.append("\n\n")
    lines.append("class PythonTargetProperty(Enum):\n")
    lines.extend(
        f"    {name} = {value!r}\n" for name, value in schema.target_properties
    )
    lines.append("\n\n")
    lines.extend(
        [
            "IdentityMask: TypeAlias = int\nMemberMask: TypeAlias = int\n",
            "NO_IDENTITIES: Final[IdentityMask] = 0\n",
            "OTHER_IDENTITY: Final[IdentityMask] = int(PythonIdentity.OTHER)\n",
            "UNBOUND_IDENTITY: Final[IdentityMask] = int(PythonIdentity.UNBOUND)\n",
            "UNKNOWN_IDENTITY: Final[IdentityMask] = OTHER_IDENTITY | UNBOUND_IDENTITY\n",
            "SENTINEL_IDENTITIES: Final[IdentityMask] = UNKNOWN_IDENTITY\n",
            "NO_INVALID_MEMBERS: Final[MemberMask] = 0\n",
            f"ALL_INVALID_MEMBERS: Final[MemberMask] = {sum(1 << bit for _, bit in schema.member_bits)}\n\n",
        ]
    )

    def mapping(
        name: str, key_type: str, value_type: str, rows: list[tuple[str, str]]
    ) -> None:
        lines.append(
            f"{name}: Final[Mapping[{key_type}, {value_type}]] = MappingProxyType(\n    {{\n"
        )
        lines.extend(f"        {key}: {value},\n" for key, value in rows)
        lines.append("    }\n)\n\n")

    mapping(
        "INTRINSIC_CALLABLE_IDENTITIES",
        "str",
        "PythonIdentity",
        [
            (repr(name), f"PythonIdentity.{identity}")
            for name, identity, _ in schema.intrinsic_callables
        ],
    )
    mapping(
        "INTRINSIC_TARGET_PROPERTIES",
        "IdentityMask",
        "PythonTargetProperty",
        [
            (
                f"int(PythonIdentity.{identity})",
                f"PythonTargetProperty.{target_property}",
            )
            for _, identity, target_property in schema.intrinsic_callables
        ],
    )
    mapping(
        "BUILTIN_IDENTITIES",
        "str",
        "tuple[PythonIdentity, PythonMember]",
        [
            (repr(name), f"(PythonIdentity.{identity}, PythonMember.{guard})")
            for name, identity, guard, _shape in schema.builtins
        ],
    )
    shape = [row for row in schema.builtins if row[3]]
    lines.append(
        f"BUILTIN_SHAPE_NAMES: Final[frozenset[str]] = frozenset({tuple(row[0] for row in shape)!r})\n\n"
    )
    mapping(
        "BUILTIN_SHAPE_IDENTITIES",
        "str",
        "PythonIdentity",
        [(repr(name), f"PythonIdentity.{identity}") for name, identity, _, _ in shape],
    )
    mapping(
        "BUILTIN_SHAPE_NAMES_BY_IDENTITY",
        "IdentityMask",
        "str",
        [
            (f"int(PythonIdentity.{identity})", repr(name))
            for name, identity, _, _ in shape
        ],
    )
    mapping(
        "CANONICAL_IMPORT_IDENTITIES",
        "str",
        "PythonIdentity",
        [
            (repr(name), f"PythonIdentity.{identity}")
            for name, identity in schema.modules
        ],
    )
    builtin_owner = dict(schema.modules)["builtins"]
    acquisitions = [
        *schema.acquisitions,
        *(
            (builtin_owner, name, identity, guard)
            for name, identity, guard, _ in schema.builtins
        ),
    ]
    mapping(
        "CANONICAL_FROM_IMPORT_IDENTITIES",
        "tuple[str, str]",
        "tuple[PythonIdentity, PythonMember]",
        [
            (repr((module, name)), f"(PythonIdentity.{identity}, PythonMember.{guard})")
            for owner, name, identity, guard in acquisitions
            for module, module_identity in schema.modules
            if owner == module_identity
        ],
    )
    declarations: dict[str, list[str]] = {}
    member_writes: dict[str, dict[str, set[str]]] = {}
    wildcard: dict[str, set[str]] = {}
    for owner, name, identity, guard in acquisitions:
        declarations.setdefault(name, []).append(
            f"(PythonIdentity.{owner}, PythonIdentity.{identity}, PythonMember.{guard})"
        )
        member_writes.setdefault(name, {}).setdefault(owner, set()).add(guard)
    for owner, name, guards in schema.writes:
        target = (
            wildcard.setdefault(owner, set())
            if name == "*"
            else member_writes.setdefault(name, {}).setdefault(owner, set())
        )
        target.update(guards)
    mapping(
        "CANONICAL_MEMBER_IDENTITIES",
        "str",
        "tuple[tuple[PythonIdentity, PythonIdentity, PythonMember], ...]",
        [
            (repr(name), "(" + ", ".join(rows) + ",)")
            for name, rows in declarations.items()
        ],
    )
    masks = {name: 1 << bit for name, bit in schema.member_bits}
    mapping(
        "CANONICAL_MEMBER_WRITES",
        "str",
        "tuple[tuple[PythonIdentity, MemberMask], ...]",
        [
            (
                repr(name),
                "("
                + ", ".join(
                    f"(PythonIdentity.{owner}, {sum(masks[guard] for guard in guards)})"
                    for owner, guards in owners.items()
                )
                + ",)",
            )
            for name, owners in member_writes.items()
        ],
    )
    lines.append(
        "CANONICAL_WILDCARD_WRITES: Final[tuple[tuple[PythonIdentity, MemberMask], ...]] = (\n"
    )
    lines.extend(
        f"    (PythonIdentity.{owner}, {sum(masks[guard] for guard in guards)}),\n"
        for owner, guards in wildcard.items()
    )
    lines.append(")\n\n")
    identity_masks = {name: 1 << bit for name, bit in schema.identity_bits}
    any_module = 0
    for identity in dict(schema.modules).values():
        any_module |= identity_masks[identity]
    lines.append(f"ANY_CANONICAL_MODULE_IDENTITY: Final[IdentityMask] = {any_module}\n")
    return "".join(lines)


def render_all(schema: Schema) -> dict[Path, str]:
    return {OUT_PYTHON: render_python(schema)}


def generated_outputs() -> dict[Path, str]:
    return render_all(load_schema())


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
