"""Module registry authority — the import bedrock's compile-time artifact.

Design authority: ``docs/design/foundation/69_import_bedrock_frozen_module_layer.md``
(§3 "Compile-time artifacts").  Every projection of the per-build module graph
(the runtime registry blob, the native ``MODULE_INIT_TABLE`` relocations, the
``module_registry.json`` diagnostics file) is emitted from this one module and
stamped with the same ``registry_digest``.  The CLI front for out-of-build
checks lives in ``tools/gen_module_registry.py`` and is a thin wrapper over
this module.

Layout contract
---------------
The binary blob emitted into the native application object under the exported
symbol ``molt_module_registry_blob`` has exactly two layout authorities: the
writer here and the reader in
``runtime/molt-runtime/src/builtins/module_table.rs``.  The structural gate
``tests/test_module_registry_gates.py`` asserts the two sets of layout
constants agree; bump ``MODULE_REGISTRY_SCHEMA_VERSION`` in both files in the
same arc for any layout change.

Blob layout (little-endian, 8-byte aligned):

    header (HEADER_BYTES = 48):
        0..8    magic       u64  = b"MOLTMOD1"
        8..12   schema      u32
        12..16  count       u32
        16..32  digest      [u8; 16]  (first 16 bytes of the registry sha256)
        32..40  names_len   u64
        40..48  reserved    u64 = 0
    rows (count * ROW_BYTES = 32 each):
        0..4    name_off    u32   (byte offset into the names blob)
        4..8    name_len    u32
        8..16   init_ptr    u64   (function-address relocation; 0 = no init lane)
        16..20  parent_id   u32   (NO_MODULE_ID = none)
        20..24  alias_target u32  (NO_MODULE_ID = none)
        24      kind        u8
        25      flags       u8
        26..28  reserved    u16 = 0
        28..32  reserved    u32 = 0
    names: concatenated UTF-8 module names in id (sorted-name) order

Row ``id`` assignment is the sorted order of canonical dotted names, so the
names blob is itself sorted and the runtime resolves name→id with a plain
binary search (design §4.2 sanctions sorted-array binary search as the
resolver; a PHF is an optimization with the same contract).
"""

from __future__ import annotations

import hashlib
import json
import struct
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field

MODULE_REGISTRY_SCHEMA_VERSION = 1
MODULE_REGISTRY_BLOB_SYMBOL = "molt_module_registry_blob"
MODULE_REGISTRY_MAGIC = int.from_bytes(b"MOLTMOD1", "little")
MODULE_REGISTRY_HEADER_BYTES = 48
MODULE_REGISTRY_ROW_BYTES = 32
MODULE_REGISTRY_ROW_INIT_PTR_OFFSET = 8
NO_MODULE_ID = 0xFFFF_FFFF

MODULE_KIND_SOURCE = 0
MODULE_KIND_EXTENSION = 1
MODULE_KIND_ALIAS = 2
MODULE_KIND_NAMESPACE_PARENT = 3
MODULE_KIND_RUNTIME_BUILTIN = 4

_KIND_CODES: Mapping[str, int] = {
    "source": MODULE_KIND_SOURCE,
    "extension": MODULE_KIND_EXTENSION,
    "alias": MODULE_KIND_ALIAS,
    "namespace_parent": MODULE_KIND_NAMESPACE_PARENT,
    "runtime_builtin": MODULE_KIND_RUNTIME_BUILTIN,
}

# Reinit policy after `del sys.modules[name]` (design §4.3 / parity row 5.8):
# source modules fully re-execute; extension modules resurrect from the
# first-init dict snapshot (snapshot custody lands in PR4; until then the
# runtime fails closed on extension reinit with a named diagnostic).
def _row_deps(value: object) -> tuple[object, ...]:
    if isinstance(value, list | tuple):
        return tuple(value)
    return ()


def _optional_row_index(
    value: object,
    *,
    row_name: object,
    field: str,
    names: Sequence[str],
    problems: list[str],
) -> int | None:
    if value is None:
        return None
    if not isinstance(value, int):
        problems.append(f"row {row_name!r} {field} is not an integer: {value!r}")
        return None
    if value < 0 or value >= len(names):
        problems.append(f"row {row_name!r} {field} index out of range: {value}")
        return None
    return value


MODULE_FLAG_REINIT_RESURRECT = 0x01

# Modules whose registry kind is RuntimeBuiltin: the runtime itself
# participates in their construction (sys populates argv/stdio during
# publication).  They still initialize through MODULE_INIT_TABLE.
_RUNTIME_BUILTIN_MODULES = frozenset({"sys", "builtins"})


@dataclass(frozen=True)
class ModuleRegistryEntry:
    """One module admitted to the binary image closure."""

    name: str
    kind: str
    init_symbol: str = ""
    alias_of: str = ""
    deps: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if self.kind not in _KIND_CODES:
            raise ValueError(
                f"module registry entry '{self.name}' has unknown kind '{self.kind}'"
            )
        if self.kind == "alias":
            if not self.alias_of:
                raise ValueError(
                    f"alias registry entry '{self.name}' requires alias_of"
                )
            if self.init_symbol:
                raise ValueError(
                    "alias registry entries own no init lane: "
                    f"'{self.name}' must not carry init_symbol "
                    "(alias resolution happens inside molt_module_ensure)"
                )
        elif self.alias_of:
            raise ValueError(
                f"registry entry '{self.name}' has alias_of but kind '{self.kind}'"
            )


@dataclass(frozen=True)
class ModuleRegistryRow:
    id: int
    name: str
    kind: str
    kind_code: int
    parent: int | None
    alias_target: int | None
    init_symbol: str
    flags: int
    deps: tuple[str, ...] = ()

    @property
    def leaf_attr(self) -> str:
        return self.name.rsplit(".", 1)[-1]


@dataclass(frozen=True)
class ModuleRegistry:
    schema: int
    digest: str
    rows: tuple[ModuleRegistryRow, ...]
    _id_by_name: Mapping[str, int] = field(
        repr=False, hash=False, compare=False, default_factory=dict
    )

    def id_of(self, name: str) -> int | None:
        return self._id_by_name.get(name)

    def row_of(self, name: str) -> ModuleRegistryRow | None:
        row_id = self.id_of(name)
        return None if row_id is None else self.rows[row_id]

    def ensure_lane_id(self, name: str) -> int | None:
        """The row id when ``molt_module_ensure(id)`` can fully own this
        import: the row (after alias resolution) must carry an init lane.
        Rows without one keep the dynamic import lane so the importlib
        fallback ladder (runtime-root spec imports) is preserved."""
        row_id = self.id_of(name)
        if row_id is None:
            return None
        terminal = self.rows[row_id]
        while terminal.alias_target is not None:
            terminal = self.rows[terminal.alias_target]
        return row_id if terminal.init_symbol else None

    @property
    def digest16_hex(self) -> str:
        return self.digest[:32]

    def init_symbols(self) -> tuple[str, ...]:
        return tuple(sorted({row.init_symbol for row in self.rows if row.init_symbol}))

    def registry_json_payload(self) -> dict[str, object]:
        """The ``module_registry.json`` diagnostics projection."""
        return {
            "schema": self.schema,
            "registry_digest": self.digest,
            "blob_symbol": MODULE_REGISTRY_BLOB_SYMBOL,
            "rows": [
                {
                    "id": row.id,
                    "name": row.name,
                    "kind": row.kind,
                    "parent": row.parent,
                    "alias_target": row.alias_target,
                    "init_symbol": row.init_symbol,
                    "flags": row.flags,
                    "deps": list(row.deps),
                }
                for row in self.rows
            ],
        }

    def backend_ir_payload(self) -> dict[str, object]:
        """The backend projection embedded in the IR document.

        Carries the fully serialized blob bytes (init-ptr slots zeroed) plus
        the relocation list ``[byte_offset, init_symbol]`` the backend applies
        with native function-address relocations, so the backend needs no
        layout knowledge of its own.
        """
        blob, relocs = self._blob_and_relocs()
        return {
            "schema": self.schema,
            "registry_digest": self.digest,
            "blob": list(blob),
            "relocs": [[offset, symbol] for offset, symbol in relocs],
            "init_symbols": list(self.init_symbols()),
        }

    def _blob_and_relocs(self) -> tuple[bytes, tuple[tuple[int, str], ...]]:
        names_blob = bytearray()
        name_spans: list[tuple[int, int]] = []
        for row in self.rows:
            encoded = row.name.encode("utf-8")
            name_spans.append((len(names_blob), len(encoded)))
            names_blob.extend(encoded)
        header = struct.pack(
            "<QII16sQQ",
            MODULE_REGISTRY_MAGIC,
            self.schema,
            len(self.rows),
            bytes.fromhex(self.digest16_hex),
            len(names_blob),
            0,
        )
        assert len(header) == MODULE_REGISTRY_HEADER_BYTES
        rows_blob = bytearray()
        relocs: list[tuple[int, str]] = []
        for row, (name_off, name_len) in zip(self.rows, name_spans):
            row_base = MODULE_REGISTRY_HEADER_BYTES + len(rows_blob)
            if row.init_symbol:
                relocs.append(
                    (row_base + MODULE_REGISTRY_ROW_INIT_PTR_OFFSET, row.init_symbol)
                )
            packed = struct.pack(
                "<IIQIIBBHI",
                name_off,
                name_len,
                0,  # init_ptr: relocation-filled
                NO_MODULE_ID if row.parent is None else row.parent,
                NO_MODULE_ID if row.alias_target is None else row.alias_target,
                row.kind_code,
                row.flags,
                0,
                0,
            )
            assert len(packed) == MODULE_REGISTRY_ROW_BYTES
            rows_blob.extend(packed)
        blob = bytes(header) + bytes(rows_blob) + bytes(names_blob)
        return blob, tuple(relocs)


def registry_digest_for_rows(
    rows: Iterable[Mapping[str, object]], *, schema: int
) -> str:
    """Canonical registry digest: sha256 over the sorted canonical row JSON.

    ``rows`` must provide ``name``, ``kind``, ``parent``, ``alias_target``,
    ``init_symbol``, ``flags`` and ``deps``.  ``id`` is derived (sorted-name
    order), so it does not participate in the digest.
    """
    canonical = [
        {
            "name": row["name"],
            "kind": row["kind"],
            "parent": row["parent"],
            "alias_target": row["alias_target"],
            "init_symbol": row["init_symbol"],
            "flags": row["flags"],
            "deps": list(_row_deps(row.get("deps", ()))),
        }
        for row in rows
    ]
    canonical.sort(key=lambda row: str(row["name"]))
    payload = json.dumps(
        {"schema": schema, "rows": canonical},
        sort_keys=True,
        separators=(",", ":"),
    ).encode("utf-8")
    return hashlib.sha256(payload).hexdigest()


def build_module_registry(
    entries: Iterable[ModuleRegistryEntry],
) -> ModuleRegistry:
    """Assemble the per-build registry from the closure plan entries.

    Ids are dense and assigned in sorted-canonical-name order (deterministic
    and reproducible-build-stable for a given closure, design §2.1).
    """
    by_name: dict[str, ModuleRegistryEntry] = {}
    for entry in entries:
        if not entry.name:
            raise ValueError("module registry entry with empty name")
        existing = by_name.get(entry.name)
        if existing is None:
            by_name[entry.name] = entry
            continue
        if existing == entry:
            continue
        raise ValueError(
            f"conflicting module registry entries for '{entry.name}': "
            f"{existing} vs {entry}"
        )
    ordered_names = sorted(by_name)
    id_by_name = {name: idx for idx, name in enumerate(ordered_names)}

    # Alias targets must exist and terminate (no alias-of-alias cycles).
    for name, entry in by_name.items():
        if entry.kind != "alias":
            continue
        seen = {name}
        cursor = entry.alias_of
        while True:
            target = by_name.get(cursor)
            if target is None:
                raise ValueError(
                    f"alias module '{name}' targets '{cursor}' which is not in "
                    "the compiled closure"
                )
            if target.kind != "alias":
                break
            if cursor in seen:
                raise ValueError(f"alias cycle through module '{cursor}'")
            seen.add(cursor)
            cursor = target.alias_of

    rows: list[ModuleRegistryRow] = []
    for idx, name in enumerate(ordered_names):
        entry = by_name[name]
        parent: int | None = None
        if "." in name:
            parent_name = name.rsplit(".", 1)[0]
            parent = id_by_name.get(parent_name)
        alias_target: int | None = None
        if entry.kind == "alias":
            alias_target = id_by_name[entry.alias_of]
        flags = 0
        if entry.kind == "extension":
            flags |= MODULE_FLAG_REINIT_RESURRECT
        rows.append(
            ModuleRegistryRow(
                id=idx,
                name=name,
                kind=entry.kind,
                kind_code=_KIND_CODES[entry.kind],
                parent=parent,
                alias_target=alias_target,
                init_symbol=entry.init_symbol,
                flags=flags,
                deps=tuple(sorted(set(entry.deps))),
            )
        )
    digest = registry_digest_for_rows(
        (
            {
                "name": row.name,
                "kind": row.kind,
                "parent": None if row.parent is None else rows[row.parent].name,
                "alias_target": (
                    None if row.alias_target is None else rows[row.alias_target].name
                ),
                "init_symbol": row.init_symbol,
                "flags": row.flags,
                "deps": row.deps,
            }
            for row in rows
        ),
        schema=MODULE_REGISTRY_SCHEMA_VERSION,
    )
    return ModuleRegistry(
        schema=MODULE_REGISTRY_SCHEMA_VERSION,
        digest=digest,
        rows=tuple(rows),
        _id_by_name=id_by_name,
    )


def runtime_builtin_kind(name: str) -> str:
    return "runtime_builtin" if name in _RUNTIME_BUILTIN_MODULES else "source"


def check_registry_json_payload(payload: Mapping[str, object]) -> list[str]:
    """G1/G7 check: re-derive the digest from a ``module_registry.json``
    payload and report every projection-consistency violation."""
    problems: list[str] = []
    schema = payload.get("schema")
    if schema != MODULE_REGISTRY_SCHEMA_VERSION:
        problems.append(
            f"schema mismatch: payload has {schema!r}, authority is "
            f"{MODULE_REGISTRY_SCHEMA_VERSION}"
        )
    raw_rows = payload.get("rows")
    if not isinstance(raw_rows, list):
        return problems + ["payload has no rows list"]
    rows: list[dict[str, object]] = []
    for idx, row in enumerate(raw_rows):
        if not isinstance(row, Mapping):
            problems.append(f"row {idx} is not an object")
            continue
        rows.append({str(key): value for key, value in row.items()})
    names: list[str] = []
    for idx, row in enumerate(rows):
        name = row.get("name")
        if not isinstance(name, str):
            problems.append(f"row {idx} has non-string name {name!r}")
            name = str(name)
        names.append(name)
    if names != sorted(names):  # id order is sorted-name order
        problems.append("rows are not in sorted-name (id) order")
    name_to_id = {name: idx for idx, name in enumerate(names)}
    canonical_rows = []
    for idx, row in enumerate(rows):
        if row.get("id") != idx:
            problems.append(f"row {row.get('name')!r} has id {row.get('id')} != {idx}")
        row_name = row.get("name")
        parent = _optional_row_index(
            row.get("parent"),
            row_name=row_name,
            field="parent",
            names=names,
            problems=problems,
        )
        alias_target = _optional_row_index(
            row.get("alias_target"),
            row_name=row_name,
            field="alias_target",
            names=names,
            problems=problems,
        )
        canonical_rows.append(
            {
                "name": row_name,
                "kind": row.get("kind"),
                "parent": None if parent is None else names[parent],
                "alias_target": None if alias_target is None else names[alias_target],
                "init_symbol": row.get("init_symbol", ""),
                "flags": row.get("flags", 0),
                "deps": _row_deps(row.get("deps", ())),
            }
        )
        expected_parent = (
            name_to_id.get(str(row_name).rsplit(".", 1)[0])
            if "." in str(row_name)
            else None
        )
        if parent != expected_parent:
            problems.append(
                f"row {row.get('name')!r} parent {parent} != derived {expected_parent}"
            )
    digest = registry_digest_for_rows(
        canonical_rows, schema=MODULE_REGISTRY_SCHEMA_VERSION
    )
    if payload.get("registry_digest") != digest:
        problems.append(
            "registry_digest mismatch: payload has "
            f"{payload.get('registry_digest')!r}, recomputed {digest!r}"
        )
    return problems
