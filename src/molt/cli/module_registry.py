"""Module registry authority — the import bedrock's compile-time artifact.

Design authority: ``docs/design/foundation/import_bedrock_frozen_module_layer.md``
(§3 "Compile-time artifacts").  Every projection of the per-build module graph
(the runtime registry blob, the native ``MODULE_INIT_TABLE`` relocations, the
``module_registry.json`` diagnostics file) is emitted from this one module and
stamped with the same ``registry_digest``.  The CLI front for out-of-build
checks lives in ``tools/check_module_registry.py`` and is a thin wrapper over
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

    header (HEADER_BYTES = 96):
        0..8    magic       u64  = b"MOLTMOD7"
        8..12   schema      u32
        12..16  count       u32
        16..32  digest      [u8; 16]  (first 16 bytes of the registry sha256)
        32..40  names_len   u64
        40..48  origins_len u64
        48..56  portions_len u64
        56..64  globals_len u64
        64..72  runtime_roots_fingerprint u64  (``RUNTIME_ROOTS_FINGERPRINT``)
        72..80  manifest_fingerprint      u64  (``INTRINSIC_MANIFEST_FINGERPRINT``)
        80..88  phf_seed    u64
        88..92  phf_buckets u32
        92..96  reserved    u32 = 0
    rows (count * ROW_BYTES = 56 each):
        0..4    name_off    u32   (byte offset into the names blob)
        4..8    name_len    u32
        8..16   init_ptr    u64   (function-address relocation; 0 = no init lane)
        16..20  parent_id   u32   (NO_MODULE_ID = none)
        20..24  alias_target u32  (NO_MODULE_ID = none)
        24      kind        u8
        25      flags       u8
        26      runtime_root u8   (RuntimeRoot index; NO_RUNTIME_ROOT = none)
        27      reserved    u8 = 0
        28..32  origin_off  u32   (byte offset into the origins blob)
        32..36  origin_len  u32
        36..40  portions_off u32  (byte offset into the portions blob)
        40..44  portions_len u32
        44..48  reserved    u32 = 0
        48..52  globals_off u32   (byte offset into the globals blob)
        52..56  globals_len u32
    phf_disps: phf_buckets pairs of u32 displacements (d1, d2)
    phf_slots: count u32 ids; ``phf_slots[slot]`` is the row whose name
               hashes to ``slot``
    names: concatenated UTF-8 module names in id (sorted-name) order
    origins: concatenated UTF-8 canonical source origins in id order
    portions: ordered namespace directories, each a u32 byte length followed
              by UTF-8 bytes; each namespace row owns one contiguous span
    globals: each row's global slot layout (``ModuleRegistryEntry.globals``):
             strictly sorted names, each a u32 byte length followed by UTF-8
             bytes; slot k of a module namespace table is the k-th name

Row ``id`` assignment is the sorted order of canonical dotted names. The
runtime maps a name to its id with the minimal perfect hash the blob carries
(bedrock §13 D5, IC5): one hash, one table read, one name comparison, the
same function here and in ``module_table.rs``. See :func:`phf_hashes`.

The ready block is not part of the blob. The backend that emits the blob also
emits ``molt_module_ready``: ``count`` writable, zeroed, 8-aligned words. The
application passes both to ``molt_module_registry_install``. Word ``i`` is a
borrowed pointer to ``sys.modules[name_i]`` while row ``i`` is done, else zero
(bedrock §13 D2, IC2); the runtime writes it only for ``MODULE_FLAG_FIXED``
rows, and every ``sys.modules`` mutation of the key clears it (D2a).
"""

from __future__ import annotations

import hashlib
import json
import struct
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from typing import Literal

from molt._intrinsic_symbols import (
    INTRINSIC_MANIFEST_FINGERPRINT,
    RUNTIME_ROOTS,
    RUNTIME_ROOTS_FINGERPRINT,
)

MODULE_REGISTRY_SCHEMA_VERSION = 7
MODULE_REGISTRY_BLOB_SYMBOL = "molt_module_registry_blob"
MODULE_READY_SYMBOL = "molt_module_ready"
MODULE_REGISTRY_MAGIC = int.from_bytes(b"MOLTMOD7", "little")
MODULE_REGISTRY_HEADER_BYTES = 96
MODULE_REGISTRY_ROW_BYTES = 56
MODULE_REGISTRY_ROW_INIT_PTR_OFFSET = 8
MODULE_REGISTRY_PHF_BUCKET_SIZE = 5
NO_MODULE_ID = 0xFFFF_FFFF
NO_RUNTIME_ROOT = 0xFF

MODULE_KIND_SOURCE = 0
MODULE_KIND_EXTENSION = 1
MODULE_KIND_ALIAS = 2
MODULE_KIND_NAMESPACE_PARENT = 3
MODULE_KIND_RUNTIME_BUILTIN = 4

ModuleRegistryKind = Literal[
    "source", "extension", "alias", "namespace_parent", "runtime_builtin"
]

_KIND_CODES: Mapping[ModuleRegistryKind, int] = {
    "source": MODULE_KIND_SOURCE,
    "extension": MODULE_KIND_EXTENSION,
    "alias": MODULE_KIND_ALIAS,
    "namespace_parent": MODULE_KIND_NAMESPACE_PARENT,
    "runtime_builtin": MODULE_KIND_RUNTIME_BUILTIN,
}


# Reinit policy after `del sys.modules[name]` (design §4.3 / parity row 5.8):
# source modules fully re-execute. The runtime extension initializer owns
# phase-aware reinit: legacy single-phase m_size==-1 uses its first successful
# dict snapshot; reinitializable single-phase and multi-phase modules rerun.
def is_dunder_name(name: str) -> bool:
    """Dunder names keep the by-name namespace path (metadata windows, PEP
    649 hooks, ModuleType descriptors); they never receive a global slot."""
    return len(name) > 4 and name.startswith("__") and name.endswith("__")


def _valid_global_layout(names: object) -> bool:
    return (
        isinstance(names, tuple)
        and all(
            isinstance(name, str) and name and not is_dunder_name(name)
            for name in names
        )
        and list(names) == sorted(set(names))
    )


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
MODULE_FLAG_PACKAGE = 0x02
MODULE_FLAG_HAS_BODY = 0x04
MODULE_FLAG_HAS_LOCATION = 0x08
MODULE_FLAG_STDLIB_PROVIDER = 0x10
# No code in the program can change what ``import name`` yields
# (``ImportRedirects.redirect(name)`` is empty; bedrock §13 D1, M4). Only a
# Fixed row's ready word is ever set.
MODULE_FLAG_FIXED = 0x20
MODULE_FLAG_KNOWN_MASK = (
    MODULE_FLAG_REINIT_RESURRECT
    | MODULE_FLAG_PACKAGE
    | MODULE_FLAG_HAS_BODY
    | MODULE_FLAG_HAS_LOCATION
    | MODULE_FLAG_STDLIB_PROVIDER
    | MODULE_FLAG_FIXED
)


_MASK64 = (1 << 64) - 1
_MASK32 = (1 << 32) - 1


def _fnv1a64(data: bytes) -> int:
    value = 0xCBF2_9CE4_8422_2325
    for byte in data:
        value = ((value ^ byte) * 0x0000_0100_0000_01B3) & _MASK64
    return value


def _splitmix64(value: int) -> int:
    value = (value + 0x9E37_79B9_7F4A_7C15) & _MASK64
    value = ((value ^ (value >> 30)) * 0xBF58_476D_1CE4_E5B9) & _MASK64
    value = ((value ^ (value >> 27)) * 0x94D0_49BB_1331_11EB) & _MASK64
    return value ^ (value >> 31)


def phf_hashes(name_hash: int, seed: int, count: int) -> tuple[int, int, int]:
    """``(g, h1, h2)`` for a name whose FNV-1a 64 hash is ``name_hash``: the
    bucket selector and two displacement inputs below ``count`` (the CHD
    scheme of rust-phf). ``ModuleRegistry::phf_hashes`` in ``module_table.rs``
    is the same function.
    """
    first = _splitmix64(name_hash ^ seed)
    second = _splitmix64(first)
    return first >> 32, (first & _MASK32) % count, (second & _MASK32) % count


def phf_slot(h1: int, h2: int, d1: int, d2: int, count: int) -> int:
    return (h2 + h1 * d1 + d2) % count


def build_phf(
    names: Sequence[str],
) -> tuple[int, tuple[tuple[int, int], ...], tuple[int, ...]]:
    """A minimal perfect hash over ``names``: ``(seed, disps, slots)``.

    Hash, displace and compress (Belazzougui et al. 2009) as rust-phf builds
    it: buckets of about five keys, largest first, each given the first
    displacement pair ``(d1, d2)`` that lands all its keys on free slots. For
    a fixed ``d1`` every ``d2`` shifts the bucket rigidly, so a candidate
    ``d2`` comes from a free slot rather than a scan of every pair.
    Ensures: ``slots[phf_slot(h1, h2, *disps[g % len(disps)], len(names))]``
    is ``i`` for the ``i``-th name.
    """
    count = len(names)
    if count == 0:
        return 0, ((0, 0),), ()
    bucket_count = (count + MODULE_REGISTRY_PHF_BUCKET_SIZE - 1) // (
        MODULE_REGISTRY_PHF_BUCKET_SIZE
    )
    name_hashes = [_fnv1a64(name.encode("utf-8")) for name in names]
    for seed in range(1, 1 << 16):
        hashes = [phf_hashes(value, seed, count) for value in name_hashes]
        buckets: list[list[int]] = [[] for _ in range(bucket_count)]
        for index, (g, _, _) in enumerate(hashes):
            buckets[g % bucket_count].append(index)
        disps = [(0, 0)] * bucket_count
        slots = [NO_MODULE_ID] * count
        order = sorted(range(bucket_count), key=lambda b: (-len(buckets[b]), b))
        cursor = 0
        placed = True
        for bucket in order:
            keys = buckets[bucket]
            if not keys:
                continue
            if len(keys) == 1:
                while slots[cursor] != NO_MODULE_ID:
                    cursor += 1
                slots[cursor] = keys[0]
                disps[bucket] = (0, (cursor - hashes[keys[0]][2]) % count)
                continue
            found = False
            for d1 in range(count):
                bases = [(hashes[k][2] + hashes[k][1] * d1) % count for k in keys]
                if len(set(bases)) != len(bases):
                    continue
                for first in range(count):
                    if slots[first] != NO_MODULE_ID:
                        continue
                    d2 = (first - bases[0]) % count
                    targets = [(base + d2) % count for base in bases]
                    if all(slots[target] == NO_MODULE_ID for target in targets):
                        for key, target in zip(keys, targets):
                            slots[target] = key
                        disps[bucket] = (d1, d2)
                        found = True
                        break
                if found:
                    break
            if not found:
                placed = False
                break
        if placed:
            return seed, tuple(disps), tuple(slots)
    raise ValueError("module registry perfect hash found no seed")


# Modules whose registry kind is RuntimeBuiltin: the runtime itself
# participates in their construction (sys populates argv/stdio during
# publication).  They still initialize through MODULE_INIT_TABLE.
_RUNTIME_BUILTIN_MODULES = frozenset({"sys", "builtins"})


@dataclass(frozen=True)
class ModuleRegistryEntry:
    """One module admitted to the binary image closure."""

    name: str
    kind: ModuleRegistryKind
    has_location: bool
    is_stdlib_provider: bool
    init_symbol: str = ""
    alias_of: str = ""
    deps: tuple[str, ...] = ()
    is_package: bool = False
    origin: str = ""
    portions: tuple[str, ...] = ()
    # The module namespace's global slot layout: strictly sorted non-dunder
    # names (call_anatomy.md 4.5). Slot k of the runtime table is globals[k].
    globals: tuple[str, ...] = ()
    # No code can change what ``import name`` yields (MODULE_FLAG_FIXED).
    fixed: bool = False

    def __post_init__(self) -> None:
        if not _valid_global_layout(self.globals):
            raise ValueError(
                f"module registry entry '{self.name}' globals must be strictly "
                "sorted non-dunder names"
            )
        if self.kind not in _KIND_CODES:
            raise ValueError(
                f"module registry entry '{self.name}' has unknown kind '{self.kind}'"
            )
        if not isinstance(self.has_location, bool):
            raise TypeError("module registry has_location must be bool")
        if not isinstance(self.is_stdlib_provider, bool):
            raise TypeError("module registry is_stdlib_provider must be bool")
        if self.kind == "namespace_parent":
            if (
                not isinstance(self.portions, tuple)
                or not self.portions
                or any(
                    not isinstance(portion, str) or not portion
                    for portion in self.portions
                )
                or not self.is_package
                or self.has_location
                or self.origin
                or self.init_symbol
                or self.is_stdlib_provider
            ):
                raise ValueError(
                    f"namespace registry entry '{self.name}' requires portions and a package with no origin, location or body"
                )
        elif self.portions:
            raise ValueError(
                f"non-namespace registry entry '{self.name}' carries portions"
            )
        if self.kind == "runtime_builtin" and not self.init_symbol:
            raise ValueError(
                f"runtime builtin registry entry '{self.name}' requires init_symbol"
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
    kind: ModuleRegistryKind
    kind_code: int
    parent: int | None
    alias_target: int | None
    init_symbol: str
    flags: int
    deps: tuple[str, ...] = ()
    origin: str = ""
    portions: tuple[str, ...] = ()
    globals: tuple[str, ...] = ()
    # Index into ``RUNTIME_ROOTS`` when the runtime imports this row.
    runtime_root: int | None = None

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

    def closed_import_id(self, name: str) -> int | None:
        """The row id when an import of ``name`` may take the closed lowering
        (bedrock §13 IC3): an init lane, and the row is Fixed (no redirect
        and no spec writer), so its ready word may be set."""
        row_id = self.ensure_lane_id(name)
        if row_id is None or not self.rows[row_id].flags & MODULE_FLAG_FIXED:
            return None
        return row_id

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
                    "origin": row.origin,
                    "portions": list(row.portions),
                    "globals": list(row.globals),
                    "runtime_root": row.runtime_root,
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
            "rows": [
                {"name": row.name, "init_symbol": row.init_symbol or None}
                for row in self.rows
            ],
        }

    def _blob_and_relocs(self) -> tuple[bytes, tuple[tuple[int, str], ...]]:
        names_blob = bytearray()
        origins_blob = bytearray()
        portions_blob = bytearray()
        globals_blob = bytearray()
        name_spans: list[tuple[int, int]] = []
        origin_spans: list[tuple[int, int]] = []
        portion_spans: list[tuple[int, int]] = []
        global_spans: list[tuple[int, int]] = []
        for row in self.rows:
            encoded = row.name.encode("utf-8")
            name_spans.append((len(names_blob), len(encoded)))
            names_blob.extend(encoded)
            encoded_origin = row.origin.encode("utf-8")
            origin_spans.append((len(origins_blob), len(encoded_origin)))
            origins_blob.extend(encoded_origin)
            start = len(portions_blob)
            for portion in row.portions:
                encoded_portion = portion.encode("utf-8")
                portions_blob.extend(struct.pack("<I", len(encoded_portion)))
                portions_blob.extend(encoded_portion)
            portion_spans.append((start, len(portions_blob) - start))
            start = len(globals_blob)
            for name in row.globals:
                encoded_name = name.encode("utf-8")
                globals_blob.extend(struct.pack("<I", len(encoded_name)))
                globals_blob.extend(encoded_name)
            global_spans.append((start, len(globals_blob) - start))
        seed, disps, slots = build_phf([row.name for row in self.rows])
        header = struct.pack(
            "<QII16sQQQQQQQII",
            MODULE_REGISTRY_MAGIC,
            self.schema,
            len(self.rows),
            bytes.fromhex(self.digest16_hex),
            len(names_blob),
            len(origins_blob),
            len(portions_blob),
            len(globals_blob),
            RUNTIME_ROOTS_FINGERPRINT,
            INTRINSIC_MANIFEST_FINGERPRINT,
            seed,
            len(disps),
            0,
        )
        assert len(header) == MODULE_REGISTRY_HEADER_BYTES
        rows_blob = bytearray()
        relocs: list[tuple[int, str]] = []
        for row, (name_off, name_len), (origin_off, origin_len), (
            portions_off,
            portions_len,
        ), (globals_off, globals_len) in zip(
            self.rows, name_spans, origin_spans, portion_spans, global_spans
        ):
            row_base = MODULE_REGISTRY_HEADER_BYTES + len(rows_blob)
            if row.init_symbol:
                relocs.append(
                    (row_base + MODULE_REGISTRY_ROW_INIT_PTR_OFFSET, row.init_symbol)
                )
            packed = struct.pack(
                "<IIQIIBBBBIIIIIII",
                name_off,
                name_len,
                0,  # init_ptr: relocation-filled
                NO_MODULE_ID if row.parent is None else row.parent,
                NO_MODULE_ID if row.alias_target is None else row.alias_target,
                row.kind_code,
                row.flags,
                NO_RUNTIME_ROOT if row.runtime_root is None else row.runtime_root,
                0,
                origin_off,
                origin_len,
                portions_off,
                portions_len,
                0,
                globals_off,
                globals_len,
            )
            assert len(packed) == MODULE_REGISTRY_ROW_BYTES
            rows_blob.extend(packed)
        blob = (
            bytes(header)
            + bytes(rows_blob)
            + b"".join(struct.pack("<II", d1, d2) for d1, d2 in disps)
            + b"".join(struct.pack("<I", slot) for slot in slots)
            + bytes(names_blob)
            + bytes(origins_blob)
            + bytes(portions_blob)
            + bytes(globals_blob)
        )
        return blob, tuple(relocs)


def registry_digest_for_rows(
    rows: Iterable[Mapping[str, object]], *, schema: int
) -> str:
    """Canonical registry digest: sha256 over the sorted canonical row JSON.

    ``rows`` must provide ``name``, ``kind``, ``parent``, ``alias_target``,
    ``init_symbol``, ``flags``, ``deps`` and ``runtime_root``.  ``id`` is derived (sorted-name
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
            "origin": row.get("origin", ""),
            "portions": list(_row_deps(row.get("portions", ()))),
            "globals": list(_row_deps(row.get("globals", ()))),
            "runtime_root": row.get("runtime_root"),
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
    if len(ordered_names) > NO_MODULE_ID:
        raise ValueError("module registry row count exceeds the u32 ModuleId domain")
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
        if entry.is_package:
            flags |= MODULE_FLAG_PACKAGE
        if entry.init_symbol:
            flags |= MODULE_FLAG_HAS_BODY
        if entry.has_location:
            flags |= MODULE_FLAG_HAS_LOCATION
        if entry.is_stdlib_provider:
            flags |= MODULE_FLAG_STDLIB_PROVIDER
        if entry.fixed:
            flags |= MODULE_FLAG_FIXED
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
                origin=entry.origin,
                portions=entry.portions,
                globals=entry.globals,
                runtime_root=(
                    RUNTIME_ROOTS.index(name) if name in RUNTIME_ROOTS else None
                ),
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
                "origin": row.origin,
                "portions": row.portions,
                "globals": row.globals,
                "runtime_root": row.runtime_root,
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


def runtime_builtin_kind(name: str) -> ModuleRegistryKind:
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
        init_symbol = row.get("init_symbol", "")
        flags = row.get("flags")
        portions = row.get("portions")
        if not isinstance(portions, list) or any(
            not isinstance(portion, str) or not portion for portion in portions
        ):
            problems.append(
                f"row {row_name!r} portions must be a required list of nonempty strings"
            )
        if row.get("kind") == "namespace_parent":
            if (
                not portions
                or init_symbol
                or row.get("origin")
                or not isinstance(flags, int)
                or flags & ~MODULE_FLAG_FIXED != MODULE_FLAG_PACKAGE
            ):
                problems.append(
                    f"namespace registry row {row_name!r} requires portions and no origin, location or body"
                )
        elif portions:
            problems.append(f"non-namespace registry row {row_name!r} carries portions")
        global_layout = row.get("globals")
        if not isinstance(global_layout, list) or not _valid_global_layout(
            tuple(global_layout)
        ):
            problems.append(
                f"row {row_name!r} globals must be a required strictly sorted list "
                "of non-dunder names"
            )
        valid_flags = (
            type(flags) is int
            and 0 <= flags <= 0xFF
            and flags & ~MODULE_FLAG_KNOWN_MASK == 0
        )
        if not valid_flags:
            problems.append(
                f"row {row_name!r} flags must be a required integer byte "
                f"containing only known bits: {flags!r}"
            )
        if row.get("kind") == "runtime_builtin" and (
            not isinstance(init_symbol, str)
            or not init_symbol
            or not valid_flags
            or not flags & MODULE_FLAG_HAS_BODY
        ):
            problems.append(
                f"runtime builtin registry row {row_name!r} requires an init body"
            )
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
        runtime_root = row.get("runtime_root")
        expected_root = (
            RUNTIME_ROOTS.index(str(row_name)) if row_name in RUNTIME_ROOTS else None
        )
        if runtime_root != expected_root:
            problems.append(
                f"row {row_name!r} runtime_root {runtime_root!r} != {expected_root!r}"
            )
        canonical_rows.append(
            {
                "name": row_name,
                "kind": row.get("kind"),
                "parent": None if parent is None else names[parent],
                "alias_target": None if alias_target is None else names[alias_target],
                "init_symbol": row.get("init_symbol", ""),
                "flags": flags,
                "deps": _row_deps(row.get("deps", ())),
                "origin": row.get("origin", ""),
                "portions": _row_deps(portions),
                "globals": _row_deps(global_layout),
                "runtime_root": runtime_root,
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
