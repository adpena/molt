"""Split-runtime data-symbol and address-global authority."""

from __future__ import annotations

from wasm_link_fact_provider import WasmFactsProvider

import os
from molt.temporary_artifacts import OwnedTemporaryDirectory
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path

from molt import _wasm_runtime_exports as _runtime_exports
from molt._wasm_runtime_exports import (
    wasm_cpython_abi_data_symbol_names,
    wasm_split_runtime_import_name_for_export,
)
from molt.wasm_artifact import (
    WASM_EXTERN_KIND_GLOBAL,
    WASM_VALUE_TYPE_I32,
    parse_wasm_defined_globals,
)
from molt.wasm_linking_symbols import (
    SYMBOL_KIND_DATA,
    FLAG_BINDING_WEAK,
    FLAG_BINDING_LOCAL,
    FLAG_UNDEFINED,
)
from wasm_archive import iter_wasm_object_members
from wasm_link_native_inputs import _eager_native_link_paths
from molt.cli.source_extension_link_requirements import SourceExtensionLinkRequirements
from wasm_link_format import (
    FLAG_EXPLICIT_NAME,
    SYMTAB_SUBSECTION_ID,
    _ESSENTIAL_EXPORTS,
    _build_custom_section,
    _build_linking_payload,
    _read_varsint,
    _read_varuint,
    _skip_init_expr,
    _write_string,
    _write_varuint,
)
from wasm_link_operations import (
    build_sections as _build_sections,
    parse_sections as _parse_sections,
)


@dataclass(frozen=True, slots=True)
class SplitRuntimeDataAliasPlan:
    """Exact linker input and CPython-ABI data-symbol obligations."""

    artifact: Path
    required_symbols: tuple[str, ...]
    symbol_sizes: tuple[tuple[str, int], ...]


def _canonical_split_runtime_required_exports(
    runtime_data: bytes,
    *,
    runtime_imports: Sequence[str],
    facts_provider: WasmFactsProvider,
) -> set[str]:
    """Validate and return the deploy runtime's complete generated function ABI.

    ``runtime_imports`` is the generated ABI of the exact runtime build being
    deployed (``RuntimeWasmGeneration.shared_runtime_import_names``): the full
    registry narrowed only by that build's own feature gates and target.
    """

    observed = set(facts_provider(runtime_data).function_exports)
    missing = _runtime_exports.wasm_split_runtime_missing_required_exports(
        observed,
        runtime_imports,
    )
    if missing:
        raise ValueError(
            "split deploy runtime is missing generated canonical function export(s): "
            + ", ".join(sorted(missing))
        )
    return observed - _ESSENTIAL_EXPORTS - {"molt_exception_pending"}


def _iter_linking_data_symbols(
    data: bytes,
    *,
    undefined: bool,
    facts_provider: WasmFactsProvider,
) -> Iterable[tuple[str, int | None, int | None]]:
    """Yield linking-section data symbols from a wasm object.

    For defined data symbols, yields ``(name, data_offset, size)``. For
    undefined symbols, ``data_offset`` and ``size`` are ``None``.
    """
    for symbol in facts_provider(data).linking_symbols.data_symbols:
        if undefined != (not symbol.is_defined):
            continue
        yield symbol.name, symbol.data_offset, symbol.size


def _defined_runtime_data_symbol_offsets(
    runtime_data: bytes,
    *,
    facts_provider: WasmFactsProvider,
) -> dict[str, tuple[int, int]]:
    symbols: dict[str, tuple[int, int]] = {}
    for name, offset, size in _iter_linking_data_symbols(
        runtime_data, undefined=False, facts_provider=facts_provider
    ):
        if offset is None or size is None:
            raise ValueError(
                f"defined runtime data symbol {name!r} has no exact offset/size authority"
            )
        if name in symbols:
            raise ValueError(f"duplicate defined runtime data symbol authority: {name}")
        if not 0 <= offset <= 0xFFFF_FFFF:
            raise ValueError(
                f"defined runtime data symbol {name!r} has invalid offset {offset}"
            )
        if size <= 0 or size > 0xFFFF_FFFF:
            raise ValueError(
                f"defined runtime data symbol {name!r} has invalid size {size}"
            )
        symbols[name] = (offset, size)
    return symbols


def _read_const_i32_init_expr(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading data offset expression")
    opcode = data[offset]
    offset += 1
    if opcode != 0x41:
        raise ValueError(
            f"Unsupported data offset expression opcode 0x{opcode:02x}; "
            "expected i32.const"
        )
    value, offset = _read_varsint(data, offset)
    if offset >= len(data):
        raise ValueError("Unexpected EOF after data offset expression")
    terminator = data[offset]
    offset += 1
    if terminator != 0x0B:
        raise ValueError(
            f"Unsupported data offset expression terminator 0x{terminator:02x}; "
            "expected end"
        )
    return value, offset


def _active_data_segment_intervals(data: bytes) -> tuple[tuple[int, int], ...]:
    """Return validated, ordered ``[start, end)`` active data intervals."""
    intervals: list[tuple[int, int]] = []
    for section_id, payload in _parse_sections(data):
        if section_id != 11:
            continue
        offset = 0
        count, offset = _read_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_varuint(payload, offset)
            if flags == 1:
                size, offset = _read_varuint(payload, offset)
                if size > len(payload) - offset:
                    raise ValueError(
                        "Passive data segment extends beyond the data section payload: "
                        f"size={size}, remaining={len(payload) - offset}"
                    )
                offset += size
                continue
            if flags == 2:
                _memory_index, offset = _read_varuint(payload, offset)
            elif flags != 0:
                raise ValueError(f"Unsupported data segment flags: {flags}")
            data_offset, offset = _read_const_i32_init_expr(payload, offset)
            data_offset &= 0xFFFF_FFFF
            size, offset = _read_varuint(payload, offset)
            if size > len(payload) - offset:
                raise ValueError(
                    "Active data segment extends beyond the data section payload: "
                    f"offset={data_offset}, size={size}, "
                    f"remaining={len(payload) - offset}"
                )
            offset += size
            data_end = data_offset + size
            if data_end > 1 << 32:
                raise ValueError(
                    "Active data segment exceeds the wasm32 address space: "
                    f"offset={data_offset}, size={size}, end={data_end}"
                )
            if size:
                intervals.append((data_offset, data_end))
        if offset != len(payload):
            raise ValueError(
                "Data section has trailing bytes after its declared segments: "
                f"parsed={offset}, size={len(payload)}"
            )

    intervals.sort()
    for previous, current in zip(intervals, intervals[1:], strict=False):
        if current[0] < previous[1]:
            raise ValueError(
                "Active data segments overlap: "
                f"previous=[{previous[0]}, {previous[1]}), "
                f"current=[{current[0]}, {current[1]})"
            )
    return tuple(intervals)


def _split_app_global_base(output_data: bytes) -> int:
    """Place native linked data after every output-owned active data byte."""
    intervals = _active_data_segment_intervals(output_data)
    if not intervals:
        raise ValueError("split app output has no active data placement authority")
    active_end = max(end for _start, end in intervals)
    aligned_end = (active_end + 15) & ~15
    if aligned_end >= 1 << 32:
        raise ValueError(
            "Aligned split-app data end exceeds the wasm32 address space: "
            f"active_end={active_end}, aligned_end={aligned_end}"
        )
    return aligned_end


def _validate_split_app_data_layout(
    output_data: bytes,
    linked_data: bytes,
    *,
    planned_base: int,
) -> tuple[tuple[tuple[int, int], ...], tuple[tuple[int, int], ...]]:
    """Validate and return original/final active-data intervals."""
    original = _active_data_segment_intervals(output_data)
    linked = _active_data_segment_intervals(linked_data)
    if not original:
        raise ValueError("split app output has no active data placement authority")
    if not linked:
        raise ValueError("linked split app has no active data segments")
    original_extent = (original[0][0], max(end for _start, end in original))
    linked_extent = (linked[0][0], max(end for _start, end in linked))
    if planned_base != (original_extent[1] + 15) & ~15:
        raise ValueError(
            "split app native data base does not match the aligned output data end: "
            f"output_end={original_extent[1]}, planned_base={planned_base}"
        )
    if linked_extent[0] < planned_base:
        raise ValueError(
            "linked split app data overlaps output-owned active data: "
            f"output_extent=[{original_extent[0]}, {original_extent[1]}), "
            f"linked_extent=[{linked_extent[0]}, {linked_extent[1]}), "
            f"planned_base={planned_base}"
        )
    return original, linked


def _runtime_exported_data_symbol_addresses(
    runtime_data: bytes, *, facts_provider: WasmFactsProvider
) -> dict[str, int]:
    """Map canonical data-symbol name to its runtime linear-memory address.

    A published runtime can expose the address global under either its canonical
    link name or its split-runtime export name. Both forms normalize through the
    generated CPython-ABI authority and must agree if both are present.
    """
    cpython_data_symbols = frozenset(wasm_cpython_abi_data_symbol_names())
    globals_by_index = {
        global_.index: global_ for global_ in parse_wasm_defined_globals(runtime_data)
    }
    addresses: dict[str, int] = {}
    for export in facts_provider(runtime_data).exports.values():
        canonical = wasm_split_runtime_import_name_for_export(export.name)
        if canonical not in cpython_data_symbols:
            canonical = export.name
        if canonical not in cpython_data_symbols:
            continue
        if export.kind != WASM_EXTERN_KIND_GLOBAL:
            raise ValueError(
                "runtime CPython-ABI data symbol must be exported as a global: "
                f"{export.name} has wasm export kind {export.kind}"
            )
        global_ = globals_by_index.get(export.index)
        if global_ is None:
            raise ValueError(
                "runtime CPython-ABI data symbol must reference a defined global: "
                f"{export.name} references global index {export.index}"
            )
        if global_.value_type != WASM_VALUE_TYPE_I32:
            raise ValueError(
                "runtime CPython-ABI data symbol address global must have i32 type: "
                f"{export.name} has value type 0x{global_.value_type:02x}"
            )
        if global_.mutable:
            raise ValueError(
                "runtime CPython-ABI data symbol address global must be immutable: "
                f"{export.name}"
            )
        if (
            global_.initializer_opcode != 0x41
            or global_.i32_const is None
            or not global_.i32_const_canonical
        ):
            raise ValueError(
                "runtime CPython-ABI data symbol address global must use a direct "
                f"canonical i32.const initializer: {export.name}"
            )
        value = global_.i32_const & 0xFFFF_FFFF
        previous = addresses.get(canonical)
        if previous is not None and previous != value:
            raise ValueError(
                "runtime exports conflicting addresses for canonical "
                f"CPython-ABI data symbol {canonical}: "
                f"{previous} != {value} (export {export.name})"
            )
        addresses[canonical] = value
    return addresses


def _undefined_cpython_abi_data_symbols(
    native_objects: Sequence[Path],
    *,
    facts_provider: WasmFactsProvider,
) -> tuple[str, ...]:
    cpython_data_symbols = frozenset(wasm_cpython_abi_data_symbol_names())
    symbols: set[str] = set()
    for native_object in native_objects:
        try:
            for member in iter_wasm_object_members(native_object):
                undefined_symbols = _iter_linking_data_symbols(
                    member.data,
                    undefined=True,
                    facts_provider=facts_provider,
                )
                for split_name, _offset, _size in undefined_symbols:
                    canonical = wasm_split_runtime_import_name_for_export(split_name)
                    if canonical is None:
                        canonical = split_name
                    if canonical in cpython_data_symbols:
                        symbols.add(split_name)
        except ValueError as exc:
            raise ValueError(
                "cannot decode native object linking symbols while resolving "
                f"CPython-ABI data symbols: {native_object}: {exc}"
            ) from exc
    return tuple(sorted(symbols))


def _data_symbol_entry(*, name: str, segment_index: int, size: int) -> bytes:
    entry = bytearray()
    entry.append(SYMBOL_KIND_DATA)
    entry.extend(_write_varuint(FLAG_EXPLICIT_NAME))
    entry.extend(_write_string(name))
    entry.extend(_write_varuint(segment_index))
    entry.extend(_write_varuint(0))
    entry.extend(_write_varuint(size))
    return bytes(entry)


def _build_runtime_data_alias_object(
    symbol_offsets: Sequence[tuple[str, int, int]],
) -> bytes:
    """Build a wasm object defining runtime-owned data symbols at given addresses."""
    sections: list[tuple[int, bytes]] = []

    import_payload = bytearray()
    import_payload.extend(_write_varuint(1))
    import_payload.extend(_write_string("env"))
    import_payload.extend(_write_string("memory"))
    import_payload.append(2)  # memory import
    import_payload.append(0)  # min-only limits
    import_payload.extend(_write_varuint(1))
    sections.append((2, bytes(import_payload)))

    data_payload = bytearray()
    data_payload.extend(_write_varuint(len(symbol_offsets)))
    symbol_entries: list[bytes] = []
    for segment_index, (name, address, size) in enumerate(symbol_offsets):
        data_payload.append(0)  # active segment, memory 0 implicit
        data_payload.append(0x41)  # i32.const
        data_payload.extend(_write_wasm32_address(address, symbol=name))
        data_payload.append(0x0B)  # end
        data_payload.extend(_write_varuint(0))  # empty payload
        symbol_entries.append(
            _data_symbol_entry(name=name, segment_index=segment_index, size=size)
        )
    sections.append((11, bytes(data_payload)))

    symbol_payload = _write_varuint(len(symbol_entries)) + b"".join(symbol_entries)
    sections.append(
        (
            0,
            _build_custom_section(
                "linking",
                _build_linking_payload(2, [(SYMTAB_SUBSECTION_ID, symbol_payload)]),
            ),
        )
    )
    return _build_sections(sections)


def _resolve_deploy_runtime(deploy_runtime_override: Path | None) -> Path:
    """Resolve the explicitly trusted deploy-ready split runtime."""
    if deploy_runtime_override is not None:
        if not deploy_runtime_override.is_file():
            raise FileNotFoundError(
                f"explicit split deploy runtime not found: {deploy_runtime_override}"
            )
        return deploy_runtime_override
    env_deploy_runtime = os.environ.get("MOLT_WASM_DEPLOY_RUNTIME", "").strip()
    if env_deploy_runtime:
        ambient = Path(env_deploy_runtime).expanduser()
        if ambient.is_file():
            return ambient
    raise FileNotFoundError(
        "split deploy runtime requires one explicit trusted shared member"
    )


def _split_runtime_data_alias_object(
    *,
    native_link_requirements: SourceExtensionLinkRequirements,
    deploy_runtime: Path,
    temp_dir: OwnedTemporaryDirectory,
    reloc_runtime: Path | None = None,
    facts_provider: WasmFactsProvider,
) -> SplitRuntimeDataAliasPlan | None:
    """Build runtime-address aliases for undefined CPython-ABI data symbols."""
    candidate_symbols = _undefined_cpython_abi_data_symbols(
        tuple(Path(item.path) for item in native_link_requirements.inputs),
        facts_provider=facts_provider,
    )
    required_symbols = set(
        _undefined_cpython_abi_data_symbols(
            _eager_native_link_paths(native_link_requirements),
            facts_provider=facts_provider,
        )
    )
    if not candidate_symbols:
        return None
    deploy_addresses = _runtime_exported_data_symbol_addresses(
        deploy_runtime.read_bytes(),
        facts_provider=facts_provider,
    )
    if reloc_runtime is None:
        raise ValueError(
            "split-runtime native data symbol bridge requires the exact relocatable "
            "runtime size authority"
        )
    if not reloc_runtime.is_file():
        raise ValueError(
            "split-runtime native data symbol bridge relocatable runtime is unavailable: "
            f"{reloc_runtime}"
        )
    reloc_sizes = _defined_runtime_data_symbol_offsets(
        reloc_runtime.read_bytes(), facts_provider=facts_provider
    )
    alias_symbols: list[tuple[str, int, int]] = []
    missing: list[str] = []
    for split_name in candidate_symbols:
        canonical = wasm_split_runtime_import_name_for_export(split_name)
        if canonical is None:
            canonical = split_name
        address = deploy_addresses.get(canonical)
        if address is None:
            if split_name in required_symbols:
                missing.append(f"{split_name} (canonical {canonical})")
            continue
        size_authority = reloc_sizes.get(canonical)
        if size_authority is None:
            if split_name in required_symbols:
                missing.append(
                    f"{split_name} (canonical {canonical}, missing exact size)"
                )
            continue
        size = size_authority[1]
        alias_symbols.append((split_name, address, size))
    if missing:
        raise ValueError(
            "split-runtime native data symbol bridge: deploy runtime "
            f"{deploy_runtime.name} exports no address global for CPython-ABI "
            "data symbol(s): "
            + ", ".join(missing)
            + " — the shared runtime must publish these via "
            "--export-if-defined (wasm_cpython_abi_data_symbol_names / "
            "wasm_runtime_shared_export_link_args)."
        )
    if not alias_symbols:
        return None
    alias_path = Path(temp_dir.name) / "split_runtime_data_aliases.wasm"
    alias_path.write_bytes(_build_runtime_data_alias_object(alias_symbols))
    return SplitRuntimeDataAliasPlan(
        artifact=alias_path,
        required_symbols=tuple(name for name, _address, _size in alias_symbols),
        symbol_sizes=tuple((name, size) for name, _address, size in alias_symbols),
    )


def _write_sleb128(value: int) -> bytes:
    """Encode a signed integer as the LEB128 immediate of ``i32.const``."""
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if (value == 0 and not (byte & 0x40)) or (value == -1 and (byte & 0x40)):
            out.append(byte)
            return bytes(out)
        out.append(byte | 0x80)


def _write_wasm32_address(value: int, *, symbol: str) -> bytes:
    """Encode one unsigned wasm32 linear-memory address as an i32 immediate."""
    if (
        isinstance(value, bool)
        or not isinstance(value, int)
        or not 0 <= value <= 0xFFFF_FFFF
    ):
        raise ValueError(
            f"runtime data symbol address must fit wasm32: {symbol}={value}"
        )
    signed = value if value < 0x8000_0000 else value - 0x1_0000_0000
    return _write_sleb128(signed)


def _rewrite_global_section_i32_inits(
    data: bytes, new_inits: Mapping[int, int]
) -> bytes:
    """Replace selected direct ``i32.const`` defined-global initializers."""
    if not new_inits:
        return data
    new_sections: list[tuple[int, bytes]] = []
    rewritten_indices: set[int] = set()
    global_sections = 0
    for section_id, payload in _parse_sections(data):
        if section_id != 6:  # global section
            new_sections.append((section_id, payload))
            continue
        global_sections += 1
        if global_sections > 1:
            raise ValueError("split app contains duplicate global sections")
        offset = 0
        count, offset = _read_varuint(payload, offset)
        rebuilt = bytearray()
        rebuilt.extend(_write_varuint(count))
        for defined_index in range(count):
            valtype = payload[offset]
            mut = payload[offset + 1]
            body_start = offset + 2
            if payload[body_start] == 0x41:  # i32.const
                _value, after = _read_const_i32_init_expr(payload, body_start)
            else:
                after = _skip_init_expr(payload, body_start)
            if defined_index in new_inits:
                if valtype != 0x7F:
                    raise ValueError(
                        "cannot retarget non-i32 GOT data global at defined index "
                        f"{defined_index}"
                    )
                if mut != 0:
                    raise ValueError(
                        "cannot retarget mutable GOT data global at defined index "
                        f"{defined_index}"
                    )
                if payload[body_start] != 0x41:
                    raise ValueError(
                        "cannot retarget non-i32.const GOT data global at "
                        f"defined index {defined_index}"
                    )
                rebuilt.append(valtype)
                rebuilt.append(mut)
                rebuilt.append(0x41)  # i32.const
                rebuilt.extend(
                    _write_wasm32_address(
                        new_inits[defined_index],
                        symbol=f"defined global {defined_index}",
                    )
                )
                rebuilt.append(0x0B)  # end
                rewritten_indices.add(defined_index)
            else:
                rebuilt.extend(payload[offset:after])
            offset = after
        new_sections.append((section_id, bytes(rebuilt)))
    missing_indices = set(new_inits) - rewritten_indices
    if missing_indices:
        raise ValueError(
            "split app GOT data facts reference missing defined global index(es): "
            + ", ".join(str(index) for index in sorted(missing_indices))
        )
    return _build_sections(new_sections)


def _split_runtime_got_data_global_facts(
    wasm_facts: Mapping[str, object],
) -> dict[str, tuple[int, int | None, int, bool]]:
    if wasm_facts.get("linking_symbol_table_present") is not True:
        raise ValueError(
            "split app has no retained linker symbol-table authority for GOT data globals"
        )
    raw_rows = wasm_facts.get("split_runtime_got_data_globals")
    if not isinstance(raw_rows, Sequence) or isinstance(raw_rows, (str, bytes)):
        raise ValueError("split app WASM facts omitted GOT data global rows")
    rows: dict[str, tuple[int, int | None, int, bool]] = {}
    global_indices: set[int] = set()
    for raw_row in raw_rows:
        if (
            not isinstance(raw_row, Sequence)
            or isinstance(raw_row, (str, bytes))
            or len(raw_row) != 5
            or not isinstance(raw_row[0], str)
            or not raw_row[0]
            or not isinstance(raw_row[1], int)
            or isinstance(raw_row[1], bool)
            or not 0 <= raw_row[1] <= 0xFFFF_FFFF
            or (
                raw_row[2] is not None
                and (
                    not isinstance(raw_row[2], int)
                    or isinstance(raw_row[2], bool)
                    or not 0 <= raw_row[2] <= 0xFFFF_FFFF
                )
            )
            or type(raw_row[3]) is not int
            or not 0 <= raw_row[3] <= 0xFFFF_FFFF
            or type(raw_row[4]) is not bool
        ):
            raise ValueError(
                "split app WASM facts contain an invalid GOT data global row"
            )
        symbol, global_index, initial_address, flags, defined = raw_row
        if symbol in rows:
            raise ValueError(f"duplicate split app GOT data symbol fact: {symbol}")
        if global_index in global_indices:
            raise ValueError(
                f"duplicate split app GOT data global index fact: {global_index}"
            )
        rows[symbol] = (global_index, initial_address, flags, defined)
        global_indices.add(global_index)
    return rows


def _rewrite_split_app_got_data_globals(
    data: bytes,
    *,
    runtime_addresses: Mapping[str, int],
    alias_plan: SplitRuntimeDataAliasPlan,
    wasm_facts: Mapping[str, object],
    description: str,
) -> tuple[bytes, int]:
    """Retarget every linker-attested CPython-ABI GOT global exactly once."""

    got_facts = _split_runtime_got_data_global_facts(wasm_facts)
    defined_globals = {
        global_.index: (defined_index, global_)
        for defined_index, global_ in enumerate(parse_wasm_defined_globals(data))
    }
    cpython_abi_data_symbols = set(wasm_cpython_abi_data_symbol_names())
    required = set(alias_plan.required_symbols)
    new_inits: dict[int, int] = {}
    missing: list[str] = []
    unexpected: list[str] = []
    for split_name, (
        global_index,
        initial_address,
        flags,
        defined,
    ) in got_facts.items():
        canonical = wasm_split_runtime_import_name_for_export(split_name)
        if canonical is None:
            canonical = split_name
        if canonical not in cpython_abi_data_symbols:
            continue
        if not defined or flags & FLAG_UNDEFINED:
            raise ValueError(
                f"{description}: split-runtime GOT data symbol {split_name} is undefined"
            )
        if flags & (FLAG_BINDING_WEAK | FLAG_BINDING_LOCAL):
            raise ValueError(
                f"{description}: split-runtime GOT data symbol {split_name} must have exact global binding"
            )
        if split_name not in required:
            unexpected.append(split_name)
            continue
        address = runtime_addresses.get(canonical)
        if address is None:
            missing.append(f"{split_name} (canonical {canonical})")
            continue
        defined_global = defined_globals.get(global_index)
        if defined_global is None:
            missing.append(f"{split_name} (imported or missing global, unexpected)")
            continue
        defined_index, global_ = defined_global
        if global_.value_type != WASM_VALUE_TYPE_I32:
            raise ValueError(
                f"{description}: split-runtime GOT data fact {split_name} "
                f"references non-i32 global index {global_index}"
            )
        if global_.mutable:
            raise ValueError(
                f"{description}: split-runtime GOT data fact {split_name} "
                f"references mutable global index {global_index}"
            )
        if (
            global_.initializer_opcode != 0x41
            or global_.i32_const is None
            or not global_.i32_const_canonical
        ):
            raise ValueError(
                f"{description}: split-runtime GOT data fact {split_name} "
                f"references global index {global_index} without a direct canonical "
                "i32.const initializer"
            )
        observed_initial_address = global_.i32_const & 0xFFFF_FFFF
        if observed_initial_address != initial_address:
            raise ValueError(
                f"{description}: split-runtime GOT data fact {split_name} has "
                "stale initializer authority: "
                f"facts={initial_address}, artifact={observed_initial_address}"
            )
        new_inits[defined_index] = address
    if unexpected:
        raise ValueError(
            f"{description}: split-runtime GOT data facts contain unexpected "
            "CPython-ABI symbol(s): " + ", ".join(sorted(unexpected))
        )
    if missing:
        raise ValueError(
            f"{description}: split-runtime GOT data bridge — deploy runtime "
            "publishes no canonical address for CPython-ABI data symbol(s): "
            + ", ".join(sorted(missing))
            + " — the shared runtime must export these via --export-if-defined "
            "(wasm_cpython_abi_data_symbol_names / "
            "wasm_runtime_shared_export_link_args)."
        )
    rewritten = _rewrite_global_section_i32_inits(data, new_inits)
    return rewritten, len(new_inits)
