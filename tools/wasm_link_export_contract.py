"""Public export identity and split-runtime contract restoration authority."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass
import hashlib

from molt.cli.app_export_contract import (
    app_export_call_abi,
    excluded_app_symbols,
    exported_app_symbols,
)
from wasm_link_edit import (
    _ensure_function_exports_by_symbol_names,
    _rename_export_names,
    _restore_output_export_aliases,
    _strip_internal_exports,
    _validate_app_export_adapters,
)
from wasm_link_fact_provider import WasmFactsProvider, WasmLinkFacts
from wasm_link_format import (
    _insert_standard_section,
    _read_varuint,
    _write_string,
    _write_varuint,
)
from wasm_link_operations import (
    build_sections as _build_sections,
    parse_sections as _parse_sections,
    strip_publication_sections as strip_wasm_publication_sections,
)


@dataclass(frozen=True, slots=True)
class _SplitRuntimeExportContractEntry:
    artifact: str
    kind: int
    canonical_name: str
    accepted_names: tuple[str, ...]


_SPLIT_RUNTIME_EXPORT_CONTRACT = (
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=0,
        canonical_name="molt_main",
        accepted_names=("molt_main",),
    ),
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=2,
        canonical_name="molt_memory",
        accepted_names=("molt_memory", "memory"),
    ),
    _SplitRuntimeExportContractEntry(
        artifact="app",
        kind=1,
        canonical_name="molt_table",
        accepted_names=("molt_table", "__indirect_function_table"),
    ),
)


def _split_runtime_export_contract(
    artifact: str,
) -> tuple[_SplitRuntimeExportContractEntry, ...]:
    return tuple(
        entry for entry in _SPLIT_RUNTIME_EXPORT_CONTRACT if entry.artifact == artifact
    )


def _split_runtime_contract_export_names(artifact: str) -> set[str]:
    return {
        name
        for entry in _split_runtime_export_contract(artifact)
        for name in entry.accepted_names
    }


def _split_artifact_contract_keep_set(
    artifact: str,
    *,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
) -> set[str]:
    """Return the external export contract for a split publication artifact."""

    return (
        _split_runtime_contract_export_names(artifact)
        | set(public_export_map or ())
        | set(required_native_direct_symbols)
    )


def _split_artifact_contract_function_symbols(
    artifact: str,
    *,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
) -> dict[str, str]:
    export_map = public_export_map or {}
    keep = _split_artifact_contract_keep_set(
        artifact,
        public_export_map=export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    function_symbols = {
        public_name: symbol_name
        for public_name, symbol_name in export_map.items()
        if public_name in keep
    }
    function_symbols.update({name: name for name in required_native_direct_symbols})
    for entry in _split_runtime_export_contract(artifact):
        if entry.kind == 0:
            function_symbols.setdefault(entry.canonical_name, entry.canonical_name)
    return function_symbols


_TRAP_FUNC_BODY = bytes([0x00, 0x00, 0x0B])


def _function_body_payloads_by_index(
    data: bytes, *, facts_provider: WasmFactsProvider
) -> dict[int, bytes]:
    sections = _parse_sections(data)
    import_count = int(facts_provider(data)["function_import_count"])
    for section_id, payload in sections:
        if section_id != 10:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        bodies: dict[int, bytes] = {}
        for local_index in range(count):
            body_size, body_start = _read_varuint(payload, offset)
            body_end = body_start + body_size
            if body_end > len(payload):
                raise ValueError("Unexpected EOF while reading function body")
            bodies[import_count + local_index] = payload[body_start:body_end]
            offset = body_end
        return bodies
    return {}


def _public_output_export_symbol_map(
    *,
    preserved_output_exports: Sequence[str],
    export_symbol_map: Mapping[str, str],
) -> dict[str, str]:
    public_export_map = {
        name: export_symbol_map[name]
        for name in preserved_output_exports
        if name in export_symbol_map
    }
    public_export_map.update(
        {
            name: export_symbol_map[name]
            for name in (
                "molt_host_init",
                "molt_main",
                "molt_set_wasm_table_base",
            )
            if name in export_symbol_map
        }
    )
    return public_export_map


_APP_EXPORT_IDENTITY_PREFIX = "__molt_app_export_identity__"


def _app_export_identity_maps(
    adapter_symbol_map: Mapping[str, str],
    target_symbol_map: Mapping[str, str],
) -> tuple[dict[str, str], dict[str, str], dict[str, str]]:
    """Create optimizer-stable exports for exact adapter call identity.

    Binaryen may discard linker/name metadata and renumber functions. Temporary
    exports are WebAssembly semantic roots, so their post-optimizer indices are
    the durable identity channel. They are removed after exact validation and
    never enter a published artifact.
    """

    adapter_identity: dict[str, str] = {}
    target_identity: dict[str, str] = {}
    identity_exports: dict[str, str] = {}
    for public_name, adapter_symbol in adapter_symbol_map.items():
        token = hashlib.sha256(public_name.encode("utf-8")).hexdigest()
        adapter_export = f"{_APP_EXPORT_IDENTITY_PREFIX}adapter_{token}"
        target_export = f"{_APP_EXPORT_IDENTITY_PREFIX}target_{token}"
        target_symbol = target_symbol_map.get(public_name)
        if target_symbol is None:
            raise ValueError(f"app export {public_name!r} has no raw-target identity")
        adapter_identity[public_name] = adapter_export
        target_identity[public_name] = target_export
        identity_exports[adapter_export] = adapter_symbol
        identity_exports[target_export] = target_symbol
    return (
        adapter_identity,
        target_identity,
        identity_exports,
    )


def _strip_app_export_identity_markers(
    data: bytes,
    *,
    identity_exports: Mapping[str, str],
    preserve_exports: set[str],
    facts_provider: WasmFactsProvider,
) -> bytes:
    """Remove optimizer identity roots and reject any publication leak.

    Identity roots are always removed, even when the caller's keep set still
    names them: the split-app keep set must carry them through the optimizer,
    and must not also publish them.
    """

    updated = _strip_internal_exports(
        data, preserve_exports=set(preserve_exports) - set(identity_exports)
    )
    stripped = data if updated is None else updated
    leaked = sorted(
        set(identity_exports) & set(facts_provider(stripped).function_exports)
    )
    if leaked:
        raise ValueError(
            "internal adapter identity exports leaked: " + ", ".join(leaked)
        )
    return stripped


def _publish_app_export_identity_markers(
    data: bytes,
    *,
    public_export_names: Sequence[str],
    adapter_symbol_map: Mapping[str, str],
    target_symbol_map: Mapping[str, str],
    identity_exports: Mapping[str, str],
    facts_provider: WasmFactsProvider,
) -> bytes:
    """Prove exact pre-optimizer identities, then publish durable markers."""

    _validate_app_export_adapters(
        data,
        public_export_names,
        adapter_symbol_map=adapter_symbol_map,
        target_symbol_map=target_symbol_map,
        facts_provider=facts_provider,
    )
    updated = _ensure_function_exports_by_symbol_names(
        data,
        dict(identity_exports),
        facts_provider=facts_provider,
    )
    marked = data if updated is None else updated
    missing = sorted(
        set(identity_exports) - set(facts_provider(marked).function_exports)
    )
    if missing:
        raise ValueError("optimizer identity exports are absent: " + ", ".join(missing))
    return marked


def _app_export_surface_error(
    data: bytes,
    contract: Mapping[str, object] | None,
    *,
    stage: str,
    facts_provider: WasmFactsProvider,
) -> str | None:
    if contract is None:
        return None
    exports = set(facts_provider(data).function_exports)
    expected = set(exported_app_symbols(contract))
    missing = sorted(expected - exports)
    forbidden = sorted(set(excluded_app_symbols(contract)) & exports)
    details: list[str] = []
    if missing:
        details.append("missing=" + ",".join(missing))
    if forbidden:
        details.append("excluded-exported=" + ",".join(forbidden))
    if not missing:
        try:
            call_abi = app_export_call_abi(contract)
            adapter = call_abi.get("adapter")
            if (
                isinstance(adapter, Mapping)
                and adapter.get("strategy") == "forward-owned-result"
            ):
                _validate_app_export_adapters(
                    data, tuple(sorted(expected)), facts_provider=facts_provider
                )
        except ValueError as exc:
            details.append(f"adapter-invalid={exc}")
    if not details:
        return None
    return f"app callable export contract mismatch at {stage}: " + "; ".join(details)


def _restore_public_output_exports(
    data: bytes,
    public_export_map: Mapping[str, str],
    *,
    preserved_symbol_names: Sequence[str] = (),
    facts_provider: WasmFactsProvider,
) -> bytes:
    restored = data
    updated = _ensure_function_exports_by_symbol_names(
        restored,
        dict(public_export_map),
        facts_provider=facts_provider,
    )
    if updated is not None:
        restored = updated
    rename_map = {
        symbol_name: public_name
        for public_name, symbol_name in public_export_map.items()
        if symbol_name != public_name and symbol_name not in preserved_symbol_names
    }
    updated = _rename_export_names(restored, rename_map)
    if updated is not None:
        restored = updated
    updated = _restore_output_export_aliases(restored)
    if updated is not None:
        restored = updated
    updated = _ensure_function_exports_by_symbol_names(
        restored,
        {name: name for name in preserved_symbol_names},
        facts_provider=facts_provider,
    )
    if updated is not None:
        restored = updated
    return restored


def _import_index_for_kind(
    facts: WasmLinkFacts,
    *,
    module: str,
    name: str,
    kind: int,
) -> int | None:
    return facts.import_index(module=module, name=name, kind=kind)


def _ensure_export_by_index(
    data: bytes,
    *,
    name: str,
    kind: int,
    index: int,
) -> bytes | None:
    sections = _parse_sections(data)
    rebuilt_sections: list[tuple[int, bytes]] = []
    inserted = False
    for section_id, payload in sections:
        if section_id == 7:
            count, offset = _read_varuint(payload, 0)
            rebuilt = bytearray(_write_varuint(count + 1))
            rebuilt.extend(payload[offset:])
            rebuilt.extend(_write_string(name))
            rebuilt.append(kind)
            rebuilt.extend(_write_varuint(index))
            rebuilt_sections.append((section_id, bytes(rebuilt)))
            inserted = True
            continue
        rebuilt_sections.append((section_id, payload))
    if not inserted:
        export_payload = bytearray(_write_varuint(1))
        export_payload.extend(_write_string(name))
        export_payload.append(kind)
        export_payload.extend(_write_varuint(index))
        rebuilt_sections = _insert_standard_section(
            rebuilt_sections, 7, bytes(export_payload)
        )
    return _build_sections(rebuilt_sections)


def _ensure_defined_memory_export(
    data: bytes,
    *,
    facts: WasmLinkFacts,
) -> bytes | None:
    if any(
        facts.exports.get(name) is not None and facts.exports[name].kind == 2
        for name in ("molt_memory", "memory")
    ):
        return None
    memory_imports = [entry for entry in facts.imports if entry.kind == 2]
    if memory_imports:
        raise ValueError("cannot restore linked memory export from an imported memory")
    if facts.defined_memory_count == 0:
        return None
    if facts.defined_memory_count != 1:
        raise ValueError(
            "cannot restore linked memory export without exactly one memory section"
        )
    return _ensure_export_by_index(data, name="molt_memory", kind=2, index=0)


def _restore_split_runtime_contract_exports(
    data: bytes,
    *,
    artifact: str,
    stage: str = "unspecified",
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
    facts_provider: WasmFactsProvider,
) -> bytes:
    function_symbols = _split_artifact_contract_function_symbols(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    input_exports = facts_provider(data).function_exports
    input_bodies = _function_body_payloads_by_index(data, facts_provider=facts_provider)
    contract_function_bodies = {
        public_name: input_bodies[index]
        for public_name, symbol_name in function_symbols.items()
        if (index := input_exports.get(public_name, input_exports.get(symbol_name)))
        is not None
        and index in input_bodies
        and input_bodies[index] != _TRAP_FUNC_BODY
    }
    restored = _restore_public_output_exports(
        data,
        public_export_map or {},
        preserved_symbol_names=required_native_direct_symbols,
        facts_provider=facts_provider,
    )
    updated = _ensure_function_exports_by_symbol_names(
        restored, function_symbols, facts_provider=facts_provider
    )
    if updated is not None:
        restored = updated
    current_exports = facts_provider(restored).function_exports
    current_bodies = _function_body_payloads_by_index(
        restored, facts_provider=facts_provider
    )
    body_indices: dict[bytes, list[int]] = {}
    for index, body in current_bodies.items():
        if body != _TRAP_FUNC_BODY:
            body_indices.setdefault(body, []).append(index)
    for public_name, body in contract_function_bodies.items():
        if public_name in current_exports:
            continue
        matches = body_indices.get(body, [])
        if len(matches) != 1:
            continue
        updated = _ensure_export_by_index(
            restored,
            name=public_name,
            kind=0,
            index=matches[0],
        )
        if updated is not None:
            restored = updated
            current_exports[public_name] = matches[0]
    missing_native_direct = sorted(
        set(required_native_direct_symbols) - set(current_exports)
    )
    if missing_native_direct:
        details = []
        for name in missing_native_direct:
            body = contract_function_bodies.get(name)
            details.append(
                f"{name}(input_export={name in input_exports}, "
                f"body_matches={len(body_indices.get(body, [])) if body else 0})"
            )
        raise ValueError(
            f"Split-runtime {artifact} cannot relocate required native direct "
            f"function export(s) at {stage}: {', '.join(details)}"
        )
    import_names = {1: "__indirect_function_table", 2: "memory"}
    contract = _split_runtime_export_contract(artifact)
    facts = facts_provider(restored)
    export_kinds = {
        name: (fact.kind, fact.index) for name, fact in facts.exports.items()
    }
    for entry in contract:
        if any(
            export_kinds.get(name, (None, None))[0] == entry.kind
            for name in entry.accepted_names
        ):
            continue
        if entry.kind == 0:
            raise ValueError(
                f"Split-runtime {artifact} is missing app-owned function export "
                f"{entry.canonical_name} after symbol restoration at {stage}"
            )
        import_name = import_names.get(entry.kind)
        if import_name is None:
            raise ValueError(
                f"Split-runtime {artifact} has no restoration source for export "
                f"{entry.canonical_name} kind {entry.kind}"
            )
        index = _import_index_for_kind(
            facts,
            module="env",
            name=import_name,
            kind=entry.kind,
        )
        if index is None:
            raise ValueError(
                f"Split-runtime {artifact} cannot restore {entry.canonical_name}: "
                f"missing env.{import_name} kind {entry.kind} import"
            )
        updated = _ensure_export_by_index(
            restored,
            name=entry.canonical_name,
            kind=entry.kind,
            index=index,
        )
        if updated is not None:
            restored = updated
            export_kinds[entry.canonical_name] = (entry.kind, index)
    return restored


def _strip_and_restore_split_artifact(
    data: bytes,
    *,
    artifact: str,
    stage: str,
    preserve_debug: bool,
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
    facts_provider: WasmFactsProvider,
) -> bytes:
    keep_set = _split_artifact_contract_keep_set(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    stripped = strip_wasm_publication_sections(
        data,
        final_artifact=True,
        preserve_debug=preserve_debug,
    )
    restored = _restore_split_runtime_contract_exports(
        stripped,
        artifact=artifact,
        stage=stage,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
        facts_provider=facts_provider,
    )
    facts = facts_provider(restored)
    missing = sorted(
        name
        for name in keep_set
        if name not in facts.exports
        and name not in _split_runtime_contract_export_names(artifact)
    )
    if missing:
        raise ValueError(
            f"Split-runtime {artifact} publication lost required export(s) at "
            f"{stage}: {', '.join(missing)}"
        )
    return restored
