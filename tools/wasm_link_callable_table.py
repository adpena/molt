"""Callable-table layout, identity reconciliation, and publication authority.

The table holds the runtime's own address-taken functions and, after them,
the app's region. No slot is reserved for runtime callables: they are keys
the runtime resolves itself (HF-699 K1, K3).
"""

from __future__ import annotations

from tools.wasm_link_fact_provider import WasmFactsProvider

from collections.abc import Mapping, Sequence
from typing import Literal, cast

from molt._wasm_abi_generated import WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME
from molt.wasm_artifact import WasmSplitRuntimeCallableLayout
from tools.wasm_link_format import CallableTableLayout, _read_varuint, _write_varuint
from tools.wasm_link_facts import callable_table_entry_rows
from tools.wasm_link_operations import build_sections, parse_sections


def _callable_layout_from_wasm_facts(
    facts: Mapping[str, object],
    *,
    artifact_role: Literal["plan", "app"],
) -> CallableTableLayout | None:
    raw_layout = facts.get("callable_table_layout")
    if raw_layout is None:
        return None
    if not isinstance(raw_layout, dict):
        raise ValueError("WASM facts callable_table_layout must be an object or null")
    names = ("finalized_app_base", "app_entry_count")
    values = tuple(raw_layout.get(name) for name in names)
    if not all(
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= 0xFFFF_FFFF
        for value in values
    ):
        raise ValueError("WASM facts callable-table layout fields must be u32 integers")
    layout_values = tuple(cast(int, value) for value in values)
    layout = CallableTableLayout(*layout_values)
    layout.validate()
    raw_entries = facts.get("callable_table_entries")
    if not isinstance(raw_entries, (list, tuple)):
        raise ValueError("WASM facts callable_table_entries must be an array")
    entry_rows = callable_table_entry_rows(facts)
    if artifact_role == "app":
        if facts.get("callable_table_attestation_present") is not True:
            raise ValueError("final app callable-table layout is not attested")
        expected_slots = list(
            range(
                layout.finalized_app_base,
                layout.finalized_app_base + layout.app_entry_count,
            )
        )
        observed_slots = [slot for slot, _function, _type, _role in entry_rows]
        if observed_slots != expected_slots:
            raise ValueError(
                "final app callable-table layout differs from its artifact-derived "
                f"entry facts: expected={expected_slots!r}, observed={observed_slots!r}"
            )
        return layout

    raw_tables = facts.get("tables")
    if not isinstance(raw_tables, (list, tuple)):
        raise ValueError("WASM facts tables must be an array")
    table_zero_minimum: int | None = None
    for raw_table in raw_tables:
        if not isinstance(raw_table, Mapping):
            raise ValueError("WASM facts tables rows must be objects")
        if raw_table.get("table_index") != 0:
            continue
        minimum = raw_table.get("minimum")
        if (
            table_zero_minimum is not None
            or not isinstance(minimum, int)
            or isinstance(minimum, bool)
            or minimum < 0
        ):
            raise ValueError("WASM facts table-0 minimum is invalid")
        table_zero_minimum = minimum
    required_end = layout.finalized_app_base + layout.app_entry_count
    if (table_zero_minimum is None and required_end != 0) or (
        table_zero_minimum is not None and required_end > table_zero_minimum
    ):
        raise ValueError(
            "callable-table plan exceeds its artifact-derived table capacity: "
            f"required={required_end}, table0_min={table_zero_minimum!r}"
        )
    return layout


def _reconcile_split_callable_layout(
    app_layout: CallableTableLayout,
    runtime_layout: WasmSplitRuntimeCallableLayout,
) -> CallableTableLayout:
    """The final split layout: the app's region after the runtime's own
    entries. Neither side reserves a runtime region (HF-699 K3)."""
    if runtime_layout.runtime_occupied_end > app_layout.finalized_app_base:
        raise ValueError(
            "runtime callable entries overlap the app-owned callable region: "
            f"runtime_occupied_end={runtime_layout.runtime_occupied_end}, "
            f"app_base={app_layout.finalized_app_base}"
        )
    app_layout.validate()
    return app_layout


def _callable_entry_export_name(slot: int) -> str:
    return f"{WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME}.entry.{slot}"


def _callable_app_end(layout: CallableTableLayout) -> int:
    layout.validate()
    return layout.finalized_app_base + layout.app_entry_count


def _monolithic_linked_callable_growth_base(
    layout: CallableTableLayout,
) -> int | None:
    """Return the first slot wasm-ld may allocate for address-taken functions.

    A compiler-owned app row is immutable across the final monolithic link.
    Linker-created table entries therefore belong after the entire published
    app region. An empty app region has no occupancy anchor, so preserve
    wasm-ld's compact default allocation in that case.
    """

    layout.validate()
    if layout.app_entry_count == 0:
        return None
    return _callable_app_end(layout)


class _CallableTableEntryPlan:
    """The final function of each app slot, and whether this link also owns
    the runtime's own entries below the app base (a monolithic link does; a
    split app does not)."""

    __slots__ = ("app_indices", "owns_runtime_region")

    def __init__(
        self,
        app_indices: tuple[int, ...],
        *,
        owns_runtime_region: bool,
    ) -> None:
        self.app_indices = app_indices
        self.owns_runtime_region = owns_runtime_region

    def slot_function_indices(self, layout: CallableTableLayout) -> dict[int, int]:
        layout.validate()
        if len(self.app_indices) != layout.app_entry_count:
            raise ValueError(
                "callable-table app entry plan disagrees with the published layout: "
                f"functions={len(self.app_indices)}, entries={layout.app_entry_count}"
            )
        return {
            layout.finalized_app_base + offset: function_index
            for offset, function_index in enumerate(self.app_indices)
        }


def _resolve_app_entry_indices(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None,
    facts_provider: WasmFactsProvider,
) -> tuple[int, ...]:
    if (
        entry_symbol_names is not None
        and len(entry_symbol_names) != layout.app_entry_count
    ):
        raise ValueError(
            "callable-table entry symbol count disagrees with the published layout: "
            f"symbols={len(entry_symbol_names)}, entries={layout.app_entry_count}"
        )
    exports = facts_provider(data).function_exports
    named_indices: dict[str, set[int]] = {}
    if entry_symbol_names is not None:
        for wasm_import in facts_provider(data).imports:
            if wasm_import.kind != 0:
                continue
            named_indices.setdefault(wasm_import.name, set()).add(wasm_import.index)
        for function_index, function_name in facts_provider(
            data
        ).function_names.items():
            named_indices.setdefault(function_name, set()).add(function_index)

    def resolve_entry(logical_slot: int) -> int:
        name = _callable_entry_export_name(logical_slot)
        function_index = exports.get(name)
        symbol_name = (
            entry_symbol_names[logical_slot] if entry_symbol_names is not None else None
        )
        if function_index is None and symbol_name is not None:
            function_index = exports.get(symbol_name)
        if function_index is None and symbol_name is not None:
            candidates = named_indices.get(symbol_name, set())
            if len(candidates) > 1:
                raise ValueError(
                    "linked wasm has ambiguous callable-table function identity "
                    f"for {symbol_name}: {candidates}"
                )
            if candidates:
                function_index = next(iter(candidates))
        if function_index is None:
            suffix = (
                f" or retained linker symbol {symbol_name}"
                if symbol_name is not None
                else ""
            )
            raise ValueError(
                f"linked wasm is missing callable-table entry export {name}{suffix}"
            )
        return function_index

    return tuple(resolve_entry(slot) for slot in range(layout.app_entry_count))


def _resolve_callable_table_entry_plan(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None,
    owns_runtime_region: bool,
    facts_provider: WasmFactsProvider,
) -> _CallableTableEntryPlan:
    return _CallableTableEntryPlan(
        _resolve_app_entry_indices(
            data,
            layout,
            entry_symbol_names=entry_symbol_names,
            facts_provider=facts_provider,
        ),
        owns_runtime_region=owns_runtime_region,
    )


def _merge_linked_callable_table(
    raw_entries: object,
    layout: CallableTableLayout,
    entry_plan: _CallableTableEntryPlan,
) -> int:
    """Merge compiler-owned entries with final linker growth.

    A monolithic link contains the runtime's own address-taken functions as
    contiguous growth below the finalized app base. No runtime slot is fixed,
    so that region begins at its first observed slot. App entries are already
    embedded in code as table immediates and must retain their canonical
    identities. Native/future linked entries form a second contiguous suffix at
    the immutable pre-link app end.
    """

    if not isinstance(raw_entries, (list, tuple)):
        raise ValueError("linked WASM facts omitted callable-table entries")
    rows_by_slot: dict[int, tuple[int, int, int]] = {}
    for entry in raw_entries:
        if (
            not isinstance(entry, (list, tuple))
            or len(entry) != 4
            or any(
                not isinstance(value, int)
                or isinstance(value, bool)
                or value < 0
                or value > 0xFFFF_FFFF
                for value in entry
            )
        ):
            raise ValueError(
                "linked WASM facts contain an invalid callable-table entry"
            )
        slot, function_index, type_index, role = cast(Sequence[int], entry)
        if slot in rows_by_slot:
            raise ValueError(
                f"linked WASM facts contain duplicate callable-table slot {slot}"
            )
        rows_by_slot[slot] = (function_index, type_index, role)

    expected_owned = entry_plan.slot_function_indices(layout)
    app_end = _callable_app_end(layout)
    for slot, expected_function_index in expected_owned.items():
        actual = rows_by_slot.get(slot)
        if actual is None:
            # wasm-ld GC may omit an element row even though the canonical
            # linker symbol remains. The already-resolved entry plan is the
            # publication authority and will restore this slot immediately
            # after the merge check.
            continue
        if actual[0] != expected_function_index:
            raise ValueError(
                "linked WASM changed compiler-owned callable-table identity: "
                f"slot={slot}, expected_function={expected_function_index}, "
                f"actual_function={actual[0]}"
            )

    runtime_growth_slots: list[int] = []
    suffix_growth_slots: list[int] = []
    for slot in rows_by_slot:
        if slot in expected_owned:
            continue
        if entry_plan.owns_runtime_region and slot < layout.finalized_app_base:
            runtime_growth_slots.append(slot)
            continue
        if slot < app_end:
            raise ValueError(
                "linked WASM entry without compiler identity overlaps the pre-link callable "
                f"region: slot={slot}, region=[0,{app_end})"
            )
        suffix_growth_slots.append(slot)
    runtime_growth_slots.sort()
    # wasm-ld reserves slot zero for the null function pointer, while other
    # valid producers may leave a larger leading hole, so the first observed
    # runtime row is the artifact-local occupancy base.
    for offset, slot in enumerate(runtime_growth_slots):
        expected_slot = runtime_growth_slots[0] + offset
        if slot != expected_slot:
            raise ValueError(
                "linked WASM runtime callable-table growth is not contiguous from "
                f"its ownership base: expected_slot={expected_slot}, actual_slot={slot}"
            )
    suffix_growth_slots.sort()
    for offset, slot in enumerate(suffix_growth_slots):
        expected_slot = app_end + offset
        if slot != expected_slot:
            raise ValueError(
                "linked WASM suffix callable-table growth is not contiguous from "
                f"the pre-link app boundary: expected_slot={expected_slot}, "
                f"actual_slot={slot}"
            )
    return app_end + len(suffix_growth_slots)


def _write_varsint32(value: int) -> bytes:
    if value < -(1 << 31) or value >= 1 << 31:
        raise ValueError("callable-table app base must fit i32")
    out = bytearray()
    remaining = value
    while True:
        byte = remaining & 0x7F
        remaining >>= 7
        done = (remaining == 0 and byte & 0x40 == 0) or (
            remaining == -1 and byte & 0x40 != 0
        )
        out.append(byte if done else byte | 0x80)
        if done:
            return bytes(out)


def _install_callable_table_layout(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None = None,
    app_indices: tuple[int, ...] | None = None,
    facts_provider: WasmFactsProvider,
) -> bytes:
    """Append the app region as one final active element segment at the app
    base, so the published entries override whatever the link left there."""
    if layout.app_entry_count == 0:
        return data
    if app_indices is None:
        app_indices = _resolve_app_entry_indices(
            data,
            layout,
            entry_symbol_names=entry_symbol_names,
            facts_provider=facts_provider,
        )
    sections = parse_sections(data)
    element_indices = [
        index
        for index, (section_id, _payload) in enumerate(sections)
        if section_id == 9
    ]
    if len(element_indices) != 1:
        raise ValueError(
            "linked wasm must contain exactly one element section before callable-table publication"
        )
    section_index = element_indices[0]
    section_id, payload = sections[section_index]
    segment_count, segment_offset = _read_varuint(payload, 0)
    appended = bytearray(_write_varuint(segment_count + 1))
    appended.extend(payload[segment_offset:])
    appended.extend(_write_varuint(0))
    appended.append(0x41)
    appended.extend(_write_varsint32(layout.finalized_app_base))
    appended.append(0x0B)
    appended.extend(_write_varuint(len(app_indices)))
    for function_index in app_indices:
        appended.extend(_write_varuint(function_index))
    sections[section_index] = (section_id, bytes(appended))
    return build_sections(sections)
