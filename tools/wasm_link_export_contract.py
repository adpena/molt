"""Public export identity and split-runtime contract restoration authority."""

from __future__ import annotations
from collections.abc import Mapping, Sequence
import hashlib
from typing import Any

_API: Mapping[str, Any] | None = None


def configure_api(api: Mapping[str, Any]) -> None:
    global _API
    _API = api


def _api(name: str) -> Any:
    if _API is None:
        raise RuntimeError("WASM link export-contract API is not configured")
    return _API[name]


def _public_output_export_symbol_map(
    output_data: bytes,
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
) -> bytes:
    """Remove optimizer identity roots and reject any publication leak."""

    updated = _api("_strip_internal_exports")(data, preserve_exports=preserve_exports)
    stripped = data if updated is None else updated
    leaked = sorted(
        set(identity_exports) & set(_api("_collect_function_exports")(stripped))
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
) -> bytes:
    """Prove exact pre-optimizer identities, then publish durable markers."""

    _api("_validate_app_export_adapters")(
        data,
        public_export_names,
        adapter_symbol_map=adapter_symbol_map,
        target_symbol_map=target_symbol_map,
    )
    updated = _api("_ensure_function_exports_by_symbol_names")(
        data,
        dict(identity_exports),
    )
    marked = data if updated is None else updated
    missing = sorted(
        set(identity_exports) - set(_api("_collect_function_exports")(marked))
    )
    if missing:
        raise ValueError("optimizer identity exports are absent: " + ", ".join(missing))
    return marked


def _app_export_surface_error(
    data: bytes,
    contract: Mapping[str, object] | None,
    *,
    stage: str,
) -> str | None:
    if contract is None:
        return None
    exports = set(_api("_collect_function_exports")(data))
    expected = set(_api("exported_app_symbols")(contract))
    missing = sorted(expected - exports)
    forbidden = sorted(set(_api("excluded_app_symbols")(contract)) & exports)
    details: list[str] = []
    if missing:
        details.append("missing=" + ",".join(missing))
    if forbidden:
        details.append("excluded-exported=" + ",".join(forbidden))
    if not missing:
        try:
            call_abi = _api("app_export_call_abi")(contract)
            adapter = call_abi.get("adapter")
            if (
                isinstance(adapter, Mapping)
                and adapter.get("strategy") == "forward-owned-result"
            ):
                _api("_validate_app_export_adapters")(data, tuple(sorted(expected)))
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
) -> bytes:
    restored = data
    updated = _api("_ensure_function_exports_by_symbol_names")(
        restored, dict(public_export_map)
    )
    if updated is not None:
        restored = updated
    rename_map = {
        symbol_name: public_name
        for public_name, symbol_name in public_export_map.items()
        if symbol_name != public_name and symbol_name not in preserved_symbol_names
    }
    updated = _api("_rename_export_names")(restored, rename_map)
    if updated is not None:
        restored = updated
    updated = _api("_restore_output_export_aliases")(restored)
    if updated is not None:
        restored = updated
    updated = _api("_ensure_function_exports_by_symbol_names")(
        restored,
        {name: name for name in preserved_symbol_names},
    )
    if updated is not None:
        restored = updated
    return restored


def _import_index_for_kind(
    data: bytes,
    *,
    module: str,
    name: str,
    kind: int,
) -> int | None:
    index = 0
    for import_module, import_name, import_kind, _desc in _api("_collect_imports")(
        data
    ):
        if import_kind != kind:
            continue
        if import_module == module and import_name == name:
            return index
        index += 1
    return None


def _ensure_export_by_index(
    data: bytes,
    *,
    name: str,
    kind: int,
    index: int,
) -> bytes | None:
    sections = _api("_parse_sections")(data)
    rebuilt_sections: list[tuple[int, bytes]] = []
    inserted = False
    for section_id, payload in sections:
        if section_id == 7:
            count, offset = _api("_read_varuint")(payload, 0)
            rebuilt = bytearray(_api("_write_varuint")(count + 1))
            rebuilt.extend(payload[offset:])
            rebuilt.extend(_api("_write_string")(name))
            rebuilt.append(kind)
            rebuilt.extend(_api("_write_varuint")(index))
            rebuilt_sections.append((section_id, bytes(rebuilt)))
            inserted = True
            continue
        rebuilt_sections.append((section_id, payload))
    if not inserted:
        export_payload = bytearray(_api("_write_varuint")(1))
        export_payload.extend(_api("_write_string")(name))
        export_payload.append(kind)
        export_payload.extend(_api("_write_varuint")(index))
        rebuilt_sections.append((7, bytes(export_payload)))
    rebuilt = _api("_build_sections")(rebuilt_sections)
    canonical = _api("_canonicalize_standard_section_order")(rebuilt)
    return rebuilt if canonical is None else canonical


def _ensure_defined_memory_export(data: bytes) -> bytes | None:
    facts = _api("parse_wasm_module_facts")(data)
    if any(
        facts.export_kinds.get(name, (None, None))[0] == 2
        for name in ("molt_memory", "memory")
    ):
        return None
    memory_imports = [entry for entry in facts.imports if entry[2] == 2]
    if memory_imports:
        raise ValueError("cannot restore linked memory export from an imported memory")
    memory_sections = [
        payload
        for section_id, payload in _api("_parse_sections")(data)
        if section_id == 5
    ]
    if not memory_sections:
        return None
    if len(memory_sections) != 1:
        raise ValueError(
            "cannot restore linked memory export without exactly one memory section"
        )
    memory_count, _ = _api("_read_varuint")(memory_sections[0], 0)
    if memory_count != 1:
        raise ValueError(
            "cannot restore linked memory export without exactly one defined memory"
        )
    return _api("_ensure_export_by_index")(data, name="molt_memory", kind=2, index=0)


def _restore_split_runtime_contract_exports(
    data: bytes,
    *,
    artifact: str,
    stage: str = "unspecified",
    public_export_map: Mapping[str, str] | None = None,
    required_native_direct_symbols: Sequence[str] = (),
    operation_counts: dict[str, int | float] | None = None,
) -> bytes:
    function_symbols = _api("_split_artifact_contract_function_symbols")(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    input_exports = _api("_collect_function_exports")(data)
    input_bodies = _api("_function_body_payloads_by_index")(data)
    contract_function_bodies = {
        public_name: input_bodies[index]
        for public_name, symbol_name in function_symbols.items()
        if (index := input_exports.get(public_name, input_exports.get(symbol_name)))
        is not None
        and index in input_bodies
        and input_bodies[index] != _api("_TRAP_FUNC_BODY")
    }
    restored = _api("_restore_public_output_exports")(
        data,
        public_export_map or {},
        preserved_symbol_names=required_native_direct_symbols,
    )
    updated = _api("_ensure_function_exports_by_symbol_names")(
        restored, function_symbols
    )
    if updated is not None:
        restored = updated
    current_exports = _api("_collect_function_exports")(restored)
    current_bodies = _api("_function_body_payloads_by_index")(restored)
    body_indices: dict[bytes, list[int]] = {}
    for index, body in current_bodies.items():
        if body != _api("_TRAP_FUNC_BODY"):
            body_indices.setdefault(body, []).append(index)
    for public_name, body in contract_function_bodies.items():
        if public_name in current_exports:
            continue
        matches = body_indices.get(body, [])
        if len(matches) != 1:
            continue
        updated = _api("_ensure_export_by_index")(
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
    contract = _api("_split_runtime_export_contract")(artifact)
    facts = _api("parse_wasm_module_facts")(restored)
    export_kinds = dict(facts.export_kinds)
    if operation_counts is not None:
        eliminated = max(0, len(contract) - 1)
        operation_counts["wasm_whole_artifact_redundant_parses_eliminated"] = (
            operation_counts.get("wasm_whole_artifact_redundant_parses_eliminated", 0)
            + eliminated
        )
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
        index = _api("_import_index_for_kind")(
            restored,
            module="env",
            name=import_name,
            kind=entry.kind,
        )
        if index is None:
            raise ValueError(
                f"Split-runtime {artifact} cannot restore {entry.canonical_name}: "
                f"missing env.{import_name} kind {entry.kind} import"
            )
        updated = _api("_ensure_export_by_index")(
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
    operation_counts: dict[str, int | float] | None = None,
) -> bytes:
    keep_set = _api("_split_artifact_contract_keep_set")(
        artifact,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
    )
    stripped = _api("strip_wasm_publication_sections")(
        data,
        final_artifact=True,
        preserve_debug=preserve_debug,
    )
    restored = _api("_restore_split_runtime_contract_exports")(
        stripped,
        artifact=artifact,
        stage=stage,
        public_export_map=public_export_map,
        required_native_direct_symbols=required_native_direct_symbols,
        operation_counts=operation_counts,
    )
    facts = _api("parse_wasm_module_facts")(restored)
    missing = sorted(
        name
        for name in keep_set
        if name not in facts.export_kinds
        and name not in _api("_split_runtime_contract_export_names")(artifact)
    )
    if missing:
        raise ValueError(
            f"Split-runtime {artifact} publication lost required export(s) at "
            f"{stage}: {', '.join(missing)}"
        )
    return restored
