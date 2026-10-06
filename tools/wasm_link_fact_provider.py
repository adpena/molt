"""Rust WASM-facts scan, decode, and publication authority."""

from __future__ import annotations

import contextlib
from collections import OrderedDict
from collections.abc import Iterator, Mapping
from dataclasses import dataclass, field
import hashlib
from pathlib import Path
import stat
import subprocess
from molt.temporary_artifacts import new_temporary_directory
import threading
import time
from types import MappingProxyType
from typing import Protocol, cast

from command_execution import CommandExecutor

from molt.exact_json import loads_exact
from molt.toolchain_identity import (
    StableRegularFileIdentity,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from wasm_link_format import CallableTableLayout
from molt.wasm_linking_symbols import (
    WasmLinkingSymbol,
    WasmLinkingSymbolTable,
    WasmLinkingSymbolKind,
)


_COMMANDS = CommandExecutor.for_file(__file__)
WASM_LINK_FACTS_SCHEMA_VERSION = 7
_WASM_FACTS_CACHE_ENTRIES = 16
_SUCCESS_RESPONSE_KEYS = frozenset({"schema_version", "ok", "facts"})
_FAILURE_RESPONSE_KEYS = frozenset({"schema_version", "ok", "error"})
_FACT_KEYS = frozenset(
    {
        "schema_version",
        "function_import_count",
        "defined_function_count",
        "code_body_count",
        "operator_count",
        "reachable_function_indices",
        "referenced_function_indices",
        "main_module_init_direct_calls",
        "function_types",
        "function_type_indices",
        "active_element_segments",
        "active_function_elements",
        "callable_table_entries",
        "callable_table_attestation_present",
        "callable_table_layout",
        "table_mutations",
        "reachable_table_mutations",
        "forbidden_callable_alias_exports",
        "dynamic_table_dispatch",
        "reachable_dynamic_dispatch",
        "reachable_function_reference_dispatch",
        "reachable_indirect_call_tables",
        "reachable_table_reads",
        "exported_table_indices",
        "tables",
        "defined_memory_count",
        "custom_section_names",
        "linking_symbol_table_present",
        "linking_symbols",
        "function_names",
        "split_runtime_got_data_globals",
        "canonical_import_types",
        "canonical_export_types",
    }
)
_IMPORT_FACT_KEYS = frozenset({"module", "name", "kind", "index", "type"})
_EXPORT_FACT_KEYS = frozenset({"name", "kind", "index", "type"})
_EXTERN_TYPE_KEYS = {
    "function": frozenset({"kind", "exact", "params", "results"}),
    "global": frozenset({"kind", "value_type", "mutable", "shared"}),
    "memory": frozenset(
        {
            "kind",
            "memory64",
            "shared",
            "minimum",
            "maximum",
            "page_size_log2",
        }
    ),
    "table": frozenset(
        {"kind", "table64", "shared", "minimum", "maximum", "element_type"}
    ),
    "tag": frozenset({"kind", "tag_kind", "params", "results"}),
}
_EXTERNAL_KIND_BY_TYPE = {
    "function": 0,
    "table": 1,
    "memory": 2,
    "global": 3,
    "tag": 4,
}


class WasmFactsProvider(Protocol):
    """Typed callable authority for decoded WASM-link facts."""

    @property
    def authority_digest(self) -> str: ...

    def __call__(self, data: bytes) -> WasmLinkFacts: ...

    def publish_in_place(
        self,
        artifact: Path,
        *,
        layout: CallableTableLayout | None = None,
        role: str = "monolithic",
    ) -> WasmLinkFacts: ...


class _FrozenJsonDict(dict[str, object]):
    """JSON-serializable mapping whose recursively decoded facts cannot drift."""

    @staticmethod
    def _immutable(*_args: object, **_kwargs: object) -> None:
        raise TypeError("WASM facts are immutable")

    __delitem__ = _immutable
    __ior__ = _immutable
    __setitem__ = _immutable
    clear = _immutable
    pop = _immutable
    popitem = _immutable
    setdefault = _immutable
    update = _immutable


def _freeze_json(value: object) -> object:
    if isinstance(value, dict):
        return _FrozenJsonDict(
            {str(key): _freeze_json(item) for key, item in value.items()}
        )
    if isinstance(value, list):
        return tuple(_freeze_json(item) for item in value)
    return value


def _index(value: object, *, label: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool) or value < 0:
        raise ValueError(f"{label} must be a non-negative integer")
    return value


def _text(value: object, *, label: str) -> str:
    if not isinstance(value, str) or not value:
        raise ValueError(f"{label} must be a non-empty string")
    return value


def _extern_type(
    value: object,
    *,
    external_kind: int,
    label: str,
) -> Mapping[str, object]:
    if not isinstance(value, Mapping):
        raise ValueError(f"{label} must be an object")
    type_kind = value.get("kind")
    expected_keys = (
        _EXTERN_TYPE_KEYS.get(type_kind) if isinstance(type_kind, str) else None
    )
    if expected_keys is None or set(value) != expected_keys:
        raise ValueError(f"{label} has an invalid external type shape")
    if _EXTERNAL_KIND_BY_TYPE[type_kind] != external_kind:
        raise ValueError(f"{label} kind disagrees with its external type")
    return value


@dataclass(frozen=True, slots=True)
class WasmImportFact:
    module: str
    name: str
    kind: int
    index: int
    extern_type: Mapping[str, object]


@dataclass(frozen=True, slots=True)
class WasmExportFact:
    name: str
    kind: int
    index: int
    extern_type: Mapping[str, object]


@dataclass(frozen=True, slots=True)
class WasmLinkingSymbolFact(WasmLinkingSymbol):
    symbol_index: int = 0


@dataclass(frozen=True, slots=True, init=False)
class WasmLinkFacts(Mapping[str, object]):
    """Immutable schema-v7 facts plus typed linker-facing projections."""

    _raw: Mapping[str, object]
    imports: tuple[WasmImportFact, ...]
    exports: Mapping[str, WasmExportFact]
    custom_section_names: tuple[str, ...]
    defined_memory_count: int
    linking_symbols: WasmLinkingSymbolTable
    function_names: Mapping[int, str]

    def __init__(self, raw: Mapping[str, object]) -> None:
        if set(raw) != _FACT_KEYS:
            raise ValueError("WASM facts contain an invalid schema-v7 field set")
        if raw.get("schema_version") != WASM_LINK_FACTS_SCHEMA_VERSION:
            raise ValueError("WASM facts declare an unsupported schema")
        object.__setattr__(self, "_raw", raw)
        object.__setattr__(
            self, "imports", self._decode_imports(raw.get("canonical_import_types"))
        )
        object.__setattr__(
            self, "exports", self._decode_exports(raw.get("canonical_export_types"))
        )
        object.__setattr__(
            self,
            "linking_symbols",
            self._decode_linking_symbols(raw.get("linking_symbols")),
        )
        names = raw.get("function_names")
        if not isinstance(names, (list, tuple)):
            raise ValueError("WASM facts function_names must be an array")
        decoded_names: dict[int, str] = {}
        for row in names:
            if not isinstance(row, (list, tuple)) or len(row) != 2:
                raise ValueError("WASM facts function_names rows must have width 2")
            index = _index(row[0], label="function name index")
            if index in decoded_names:
                raise ValueError("WASM facts duplicate function name index")
            if not isinstance(row[1], str):
                raise ValueError("WASM facts function name must be a string")
            decoded_names[index] = row[1]
        object.__setattr__(self, "function_names", MappingProxyType(decoded_names))
        custom_names = raw.get("custom_section_names")
        if not isinstance(custom_names, (list, tuple)) or not all(
            isinstance(name, str) for name in custom_names
        ):
            raise ValueError("WASM facts custom_section_names must be a string array")
        object.__setattr__(self, "custom_section_names", tuple(custom_names))
        object.__setattr__(
            self,
            "defined_memory_count",
            _index(
                raw.get("defined_memory_count"),
                label="WASM facts defined_memory_count",
            ),
        )

    @property
    def function_exports(self) -> dict[str, int]:
        return {
            name: fact.index for name, fact in self.exports.items() if fact.kind == 0
        }

    def module_imports(self, module: str) -> set[str]:
        return {
            fact.name
            for fact in self.imports
            if fact.module == module and fact.kind == 0
        }

    @staticmethod
    def _decode_linking_symbols(value: object) -> WasmLinkingSymbolTable:
        if not isinstance(value, (list, tuple)):
            raise ValueError("WASM facts linking_symbols must be an array")
        keys = {
            "symbol_index",
            "name",
            "kind",
            "flags",
            "index",
            "segment_index",
            "data_offset",
            "size",
        }
        result: list[WasmLinkingSymbol] = []
        seen: set[int] = set()
        for row in value:
            if not isinstance(row, Mapping) or set(row) != keys:
                raise ValueError("WASM facts linking symbol has an invalid field set")
            symbol_index = _index(row["symbol_index"], label="linking symbol ordinal")
            if symbol_index in seen:
                raise ValueError("WASM facts duplicate linking symbol ordinal")
            seen.add(symbol_index)
            if row["kind"] not in {"function", "data", "global", "table", "tag"}:
                raise ValueError("WASM facts linking symbol has invalid kind")
            if not isinstance(row["name"], str):
                raise ValueError("WASM facts linking symbol name must be a string")
            result.append(
                WasmLinkingSymbolFact(
                    symbol_index=symbol_index,
                    name=row["name"],
                    kind=cast(WasmLinkingSymbolKind, row["kind"]),
                    flags=_index(row["flags"], label="linking symbol flags"),
                    **{
                        key: None
                        if row[key] is None
                        else _index(row[key], label=f"linking symbol {key}")
                        for key in ("index", "segment_index", "data_offset", "size")
                    },
                )
            )
        return WasmLinkingSymbolTable(tuple(result))

    @staticmethod
    def _decode_imports(value: object) -> tuple[WasmImportFact, ...]:
        if not isinstance(value, (list, tuple)):
            raise ValueError("WASM facts canonical_import_types must be an array")
        result: list[WasmImportFact] = []
        identities: set[tuple[str, str]] = set()
        indices_by_kind: dict[int, set[int]] = {}
        for position, row in enumerate(value):
            label = f"WASM facts canonical_import_types[{position}]"
            if not isinstance(row, Mapping) or set(row) != _IMPORT_FACT_KEYS:
                raise ValueError(f"{label} has an invalid field set")
            kind = _index(row.get("kind"), label=f"{label}.kind")
            if kind not in _EXTERNAL_KIND_BY_TYPE.values():
                raise ValueError(f"{label}.kind is unsupported")
            fact = WasmImportFact(
                module=_text(row.get("module"), label=f"{label}.module"),
                name=_text(row.get("name"), label=f"{label}.name"),
                kind=kind,
                index=_index(row.get("index"), label=f"{label}.index"),
                extern_type=_extern_type(
                    row.get("type"), external_kind=kind, label=f"{label}.type"
                ),
            )
            identity = (fact.module, fact.name)
            if identity in identities:
                raise ValueError(
                    "WASM facts canonical_import_types contains duplicate "
                    f"identity {identity!r}"
                )
            identities.add(identity)
            kind_indices = indices_by_kind.setdefault(fact.kind, set())
            if fact.index in kind_indices:
                raise ValueError(
                    "WASM facts canonical_import_types contains duplicate "
                    f"kind {fact.kind} index {fact.index}"
                )
            kind_indices.add(fact.index)
            result.append(fact)
        for kind, indices in indices_by_kind.items():
            if indices != set(range(len(indices))):
                raise ValueError(
                    "WASM facts canonical_import_types kind "
                    f"{kind} indices must be contiguous from zero"
                )
        return tuple(result)

    @staticmethod
    def _decode_exports(value: object) -> Mapping[str, WasmExportFact]:
        if not isinstance(value, (list, tuple)):
            raise ValueError("WASM facts canonical_export_types must be an array")
        result: dict[str, WasmExportFact] = {}
        for position, row in enumerate(value):
            label = f"WASM facts canonical_export_types[{position}]"
            if not isinstance(row, Mapping) or set(row) != _EXPORT_FACT_KEYS:
                raise ValueError(f"{label} has an invalid field set")
            name = _text(row.get("name"), label=f"{label}.name")
            if name in result:
                raise ValueError(f"WASM facts contain duplicate export {name!r}")
            kind = _index(row.get("kind"), label=f"{label}.kind")
            if kind not in _EXTERNAL_KIND_BY_TYPE.values():
                raise ValueError(f"{label}.kind is unsupported")
            result[name] = WasmExportFact(
                name=name,
                kind=kind,
                index=_index(row.get("index"), label=f"{label}.index"),
                extern_type=_extern_type(
                    row.get("type"), external_kind=kind, label=f"{label}.type"
                ),
            )
        return MappingProxyType(result)

    def __getitem__(self, key: str) -> object:
        return self._raw[key]

    def __setitem__(self, _key: str, _value: object) -> None:
        raise TypeError("WASM facts are immutable")

    def __iter__(self) -> Iterator[str]:
        return iter(self._raw)

    def __len__(self) -> int:
        return len(self._raw)

    @property
    def raw(self) -> Mapping[str, object]:
        return self._raw

    def import_index(self, *, module: str, name: str, kind: int) -> int | None:
        matches = [
            fact.index
            for fact in self.imports
            if fact.module == module and fact.name == name and fact.kind == kind
        ]
        if len(matches) > 1:
            raise ValueError(
                f"WASM facts contain ambiguous {module}::{name} kind {kind} imports"
            )
        return matches[0] if matches else None

    def memory_import_minimum(self, *, module: str, name: str) -> int | None:
        matches = [
            fact
            for fact in self.imports
            if fact.module == module and fact.name == name and fact.kind == 2
        ]
        if len(matches) > 1:
            raise ValueError(f"WASM facts contain ambiguous {module}::{name} memories")
        if not matches:
            return None
        return _index(
            matches[0].extern_type.get("minimum"),
            label=f"WASM facts {module}::{name} memory minimum",
        )


def _decode_wasm_facts_response(
    process: subprocess.CompletedProcess[str],
    *,
    operation: str,
) -> WasmLinkFacts:
    try:
        payload = loads_exact(process.stdout)
    except (TypeError, ValueError) as exc:
        raise ValueError(f"{operation} returned invalid JSON: {exc}") from exc
    if not isinstance(payload, dict):
        raise ValueError(f"{operation} returned a non-object response")
    if payload.get("schema_version") != WASM_LINK_FACTS_SCHEMA_VERSION:
        raise ValueError(f"{operation} returned an unsupported response schema")
    if process.returncode != 0:
        if set(payload) != _FAILURE_RESPONSE_KEYS or payload.get("ok") is not False:
            raise ValueError(f"{operation} returned an invalid failure response")
        error = payload.get("error")
        detail = error if isinstance(error, str) and error else process.stderr.strip()
        raise ValueError(f"{operation} failed: {detail or 'unknown scanner error'}")
    if set(payload) != _SUCCESS_RESPONSE_KEYS or payload.get("ok") is not True:
        raise ValueError(f"{operation} returned an invalid success response")
    facts = payload.get("facts")
    if (
        not isinstance(facts, dict)
        or facts.get("schema_version") != WASM_LINK_FACTS_SCHEMA_VERSION
    ):
        raise ValueError(f"{operation} returned inconsistent facts schema")
    frozen = _freeze_json(facts)
    assert isinstance(frozen, Mapping)
    return WasmLinkFacts(frozen)


def _rust_wasm_facts_scanner_identity(scanner: Path) -> StableRegularFileIdentity:
    if not scanner.is_file():
        raise ValueError(f"WASM facts scanner is not a file: {scanner}")
    try:
        resolved_scanner = scanner.resolve(strict=True)
    except OSError as exc:
        raise ValueError(f"WASM facts scanner is unreadable: {scanner}") from exc
    return stable_regular_file_identity(
        resolved_scanner,
        label="WASM facts scanner",
    )


def _snapshot_rust_wasm_facts_scanner(
    scanner: Path,
    scratch_root: Path,
    *,
    expected_sha256: str | None = None,
) -> StableRegularFileIdentity:
    """Copy the attested scanner bytes to one invocation-private launch path."""

    source_identity = _rust_wasm_facts_scanner_identity(scanner)
    if expected_sha256 is not None and source_identity.sha256 != expected_sha256:
        raise ValueError("WASM facts scanner differs from expected input identity")
    try:
        source_mode = source_identity.path.stat().st_mode
        source_bytes = source_identity.path.read_bytes()
    except OSError as exc:
        raise ValueError(f"WASM facts scanner is unreadable: {scanner}") from exc
    verify_stable_regular_file_identity(
        source_identity,
        label="WASM facts scanner during snapshot",
    )
    if (
        len(source_bytes) != source_identity.size
        or hashlib.sha256(source_bytes).hexdigest() != source_identity.sha256
    ):
        raise ValueError("WASM facts scanner bytes changed during snapshot")
    scratch_root.mkdir(parents=True, exist_ok=True)
    snapshot_dir = new_temporary_directory(scratch_root, prefix="wasm-facts-scanner-")
    snapshot = snapshot_dir / source_identity.path.name
    try:
        snapshot.write_bytes(source_bytes)
        snapshot.chmod(stat.S_IMODE(source_mode))
        snapshot_identity = stable_regular_file_identity(
            snapshot,
            label="snapshotted WASM facts scanner",
        )
    except (OSError, ValueError) as exc:
        raise ValueError("failed to seal WASM facts scanner snapshot") from exc
    if (
        snapshot_identity.size != source_identity.size
        or snapshot_identity.sha256 != source_identity.sha256
    ):
        raise ValueError("snapshotted WASM facts scanner identity mismatch")
    return snapshot_identity


@dataclass(frozen=True, slots=True)
class RustWasmFactsProvider:
    """One scanner-bound, invocation-local WASM facts provider."""

    scanner_identity: StableRegularFileIdentity
    scratch_root: Path
    authority_digest: str
    metrics: dict[str, float] | None = field(default=None, repr=False, compare=False)
    evidence_root: Path | None = field(default=None, repr=False, compare=False)
    _cache: OrderedDict[str, WasmLinkFacts] = field(
        default_factory=OrderedDict,
        init=False,
        repr=False,
        compare=False,
    )
    _lock: threading.Lock = field(
        default_factory=threading.Lock,
        init=False,
        repr=False,
        compare=False,
    )

    def __call__(self, data: bytes) -> WasmLinkFacts:
        with self._lock:
            return self._provide(data)

    def _provide(self, data: bytes) -> WasmLinkFacts:
        hash_start = time.perf_counter()
        digest = hashlib.sha256(data).hexdigest()
        if self.metrics is not None:
            self.metrics["wasm_facts_hash_ms"] += max(
                0.0, (time.perf_counter() - hash_start) * 1000.0
            )
        cached = self._cache.get(digest)
        if cached is not None:
            self._cache.move_to_end(digest)
            if self.metrics is not None:
                self.metrics["wasm_facts_cache_hits"] += 1.0
            return cached
        verify_stable_regular_file_identity(
            self.scanner_identity,
            label="WASM facts scanner before execution",
        )
        artifact = self.scratch_root / f"wasm-facts-{digest}.wasm"
        artifact.write_bytes(data)
        scan_start = time.perf_counter()
        try:
            process = _COMMANDS.run(
                [
                    str(self.scanner_identity.path),
                    "--scan-wasm-link-facts",
                    str(artifact),
                ],
                text=True,
                encoding="utf-8",
                errors="replace",
                capture_output=True,
                check=False,
            )
            verify_stable_regular_file_identity(
                self.scanner_identity,
                label="WASM facts scanner after execution",
            )
            if self.metrics is not None:
                self.metrics["wasm_facts_scan_ms"] += max(
                    0.0, (time.perf_counter() - scan_start) * 1000.0
                )
                self.metrics["wasm_facts_scan_calls"] += 1.0
                self.metrics["wasm_facts_input_bytes"] += float(len(data))
                self.metrics["wasm_facts_response_chars"] += float(len(process.stdout))
            try:
                facts = _decode_wasm_facts_response(
                    process,
                    operation=f"Rust WASM facts scan for {artifact.name}",
                )
            except ValueError as exc:
                evidence_dir = self.evidence_root or self.scratch_root
                evidence_dir.mkdir(parents=True, exist_ok=True)
                evidence = evidence_dir / (artifact.name + ".rejected")
                artifact.replace(evidence)
                raise ValueError(f"{exc}; rejected input kept at {evidence}") from exc
            self._cache[digest] = facts
            while len(self._cache) > _WASM_FACTS_CACHE_ENTRIES:
                self._cache.popitem(last=False)
            return facts
        finally:
            with contextlib.suppress(OSError):
                artifact.unlink()

    def publish_in_place(
        self,
        artifact: Path,
        *,
        layout: CallableTableLayout | None = None,
        role: str = "monolithic",
    ) -> WasmLinkFacts:
        """Publish through this invocation's single captured scanner identity."""

        with self._lock:
            return self._publish_in_place(artifact, layout=layout, role=role)

    def _publish_in_place(
        self,
        artifact: Path,
        *,
        layout: CallableTableLayout | None,
        role: str,
    ) -> WasmLinkFacts:
        if role not in {"monolithic", "app", "runtime"}:
            raise ValueError(f"unknown callable-table artifact role: {role}")
        if role != "monolithic" and layout is None:
            raise ValueError(f"callable-table {role} publication requires a layout")
        command = [
            str(self.scanner_identity.path),
            "--publish-wasm-link-facts",
            str(artifact),
            "--output",
            str(artifact),
        ]
        if layout is not None:
            command.extend(
                [
                    "--callable-table-layout",
                    ",".join(
                        str(value)
                        for value in (
                            layout.fixed_prefix_base,
                            layout.fixed_prefix_len,
                            layout.finalized_app_base,
                            layout.app_entry_count,
                        )
                    ),
                ]
            )
        command.extend(["--callable-table-role", role])
        verify_stable_regular_file_identity(
            self.scanner_identity,
            label="WASM facts scanner before publication",
        )
        process = _COMMANDS.run(
            command,
            text=True,
            encoding="utf-8",
            errors="replace",
            capture_output=True,
            check=False,
        )
        verify_stable_regular_file_identity(
            self.scanner_identity,
            label="WASM facts scanner after publication",
        )
        facts = _decode_wasm_facts_response(
            process,
            operation=f"Rust WASM facts publication for {artifact}",
        )
        if facts.get("callable_table_attestation_present") is not True:
            raise ValueError("Rust WASM facts publication omitted final attestation")
        return facts


def make_rust_wasm_facts_provider(
    scanner: Path,
    scratch_root: Path,
    metrics: dict[str, float] | None = None,
    *,
    evidence_root: Path | None = None,
    expected_sha256: str | None = None,
) -> RustWasmFactsProvider:
    scanner_identity = _snapshot_rust_wasm_facts_scanner(
        scanner, scratch_root, expected_sha256=expected_sha256
    )
    authority_digest = hashlib.sha256(
        f"molt-wasm-link-facts-scanner\0{WASM_LINK_FACTS_SCHEMA_VERSION}\0{scanner_identity.sha256}".encode(
            "ascii"
        )
    ).hexdigest()
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
    return RustWasmFactsProvider(
        scanner_identity=scanner_identity,
        scratch_root=scratch_root,
        authority_digest=authority_digest,
        metrics=metrics,
        evidence_root=evidence_root,
    )
