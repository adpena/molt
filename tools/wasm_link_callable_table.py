"""Callable-table layout, identity reconciliation, and publication authority."""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from typing import cast

from molt._wasm_abi_generated import (
    WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME,
    WASM_RESERVED_RUNTIME_CALLABLE_BASE,
    WASM_RESERVED_RUNTIME_CALLABLES,
)
from molt.wasm_artifact import WasmSplitRuntimeCallableLayout
from wasm_link_format import (
    CallableTableLayout,
    _collect_func_names,
    _collect_function_exports,
    _collect_imports,
    _read_varuint,
    _write_varuint,
)


def _callable_layout_from_wasm_facts(
    facts: Mapping[str, object],
) -> CallableTableLayout | None:
    raw_layout = facts.get("callable_table_layout")
    if raw_layout is None:
        return None
    if not isinstance(raw_layout, dict):
        raise ValueError("WASM facts callable_table_layout must be an object or null")
    names = (
        "fixed_prefix_base",
        "fixed_prefix_len",
        "finalized_app_base",
        "app_entry_count",
    )
    values = tuple(raw_layout.get(name) for name in names)
    if not all(
        isinstance(value, int)
        and not isinstance(value, bool)
        and 0 <= value <= 0xFFFF_FFFF
        for value in values
    ):
        raise ValueError("WASM facts callable-table layout fields must be u32 integers")
    layout_values = tuple(cast(int, value) for value in values)
    return CallableTableLayout(*layout_values)


def _reconcile_split_callable_layout(
    app_layout: CallableTableLayout,
    runtime_layout: WasmSplitRuntimeCallableLayout,
) -> CallableTableLayout:
    if (
        app_layout.fixed_prefix_base != runtime_layout.runtime_callable_base
        or app_layout.fixed_prefix_len != runtime_layout.fixed_prefix_len
    ):
        raise ValueError(
            "app compiler callable prefix disagrees with the executable runtime: "
            f"app=({app_layout.fixed_prefix_base},{app_layout.fixed_prefix_len}) "
            f"runtime=({runtime_layout.runtime_callable_base},"
            f"{runtime_layout.fixed_prefix_len})"
        )
    if runtime_layout.runtime_occupied_end > app_layout.finalized_app_base:
        raise ValueError(
            "runtime callable entries overlap the app-owned callable region: "
            f"runtime_occupied_end={runtime_layout.runtime_occupied_end}, "
            f"app_base={app_layout.finalized_app_base}"
        )
    reconciled = CallableTableLayout(
        runtime_layout.runtime_callable_base,
        runtime_layout.fixed_prefix_len,
        app_layout.finalized_app_base,
        app_layout.app_entry_count,
    )
    reconciled.validate()
    return reconciled


def _callable_entry_export_name(slot: int) -> str:
    return f"{WASM_CALLABLE_TABLE_LAYOUT_SECTION_NAME}.entry.{slot}"


def _callable_app_end(layout: CallableTableLayout) -> int:
    layout.validate()
    return layout.finalized_app_base + layout.app_entry_count


def _monolithic_linked_callable_growth_base(
    layout: CallableTableLayout,
) -> int | None:
    """Return the first slot wasm-ld may allocate for address-taken functions.

    A compiler-owned fixed prefix or app row is immutable across the final
    monolithic link.  Linker-created table entries therefore belong after the
    entire published app region.  A truly empty layout has no occupancy anchor,
    so preserve wasm-ld's compact default allocation in that case.
    """

    layout.validate()
    if layout.fixed_prefix_len == 0 and layout.app_entry_count == 0:
        return None
    return _callable_app_end(layout)


class _CallableTableEntryPlan:
    __slots__ = (
        "app_indices",
        "fixed_indices",
        "owns_runtime_region",
        "preserved_fixed_indices",
    )

    def __init__(
        self,
        fixed_indices: tuple[int, ...],
        app_indices: tuple[int, ...],
        *,
        owns_runtime_region: bool,
        preserved_fixed_indices: tuple[int, ...] | None = None,
    ) -> None:
        self.fixed_indices = fixed_indices
        self.preserved_fixed_indices = (
            fixed_indices
            if preserved_fixed_indices is None
            else preserved_fixed_indices
        )
        self.app_indices = app_indices
        self.owns_runtime_region = owns_runtime_region

    def slot_function_indices(self, layout: CallableTableLayout) -> dict[int, int]:
        layout.validate()
        if len(self.fixed_indices) not in (0, layout.fixed_prefix_len):
            raise ValueError(
                "callable-table fixed entry plan disagrees with the published layout: "
                f"functions={len(self.fixed_indices)}, entries={layout.fixed_prefix_len}"
            )
        if len(self.app_indices) != layout.app_entry_count:
            raise ValueError(
                "callable-table app entry plan disagrees with the published layout: "
                f"functions={len(self.app_indices)}, entries={layout.app_entry_count}"
            )
        return {
            **{
                layout.fixed_prefix_base + offset: function_index
                for offset, function_index in enumerate(self.fixed_indices)
            },
            **{
                layout.finalized_app_base + offset: function_index
                for offset, function_index in enumerate(self.app_indices)
            },
        }

    def slot_allowed_function_indices(
        self, layout: CallableTableLayout
    ) -> dict[int, frozenset[int]]:
        allowed = {
            slot: frozenset((function_index,))
            for slot, function_index in self.slot_function_indices(layout).items()
        }
        if len(self.preserved_fixed_indices) != len(self.fixed_indices):
            raise ValueError(
                "callable-table preserved fixed identity plan disagrees with "
                "the publication plan"
            )
        for offset, function_index in enumerate(self.preserved_fixed_indices):
            slot = layout.fixed_prefix_base + offset
            allowed[slot] = allowed[slot] | frozenset((function_index,))
        return allowed


def _resolve_callable_table_entry_plan(
    data: bytes,
    layout: CallableTableLayout,
    *,
    entry_symbol_names: Sequence[str] | None,
    include_fixed_prefix: bool,
    override_reserved_direct: bool,
) -> _CallableTableEntryPlan:
    total_entry_count = layout.fixed_prefix_len + layout.app_entry_count
    if entry_symbol_names is not None and len(entry_symbol_names) != total_entry_count:
        raise ValueError(
            "callable-table entry symbol count disagrees with the published layout: "
            f"symbols={len(entry_symbol_names)}, entries={total_entry_count}"
        )
    exports = _collect_function_exports(data)
    named_indices: dict[str, set[int]] = {}
    if entry_symbol_names is not None:
        function_import_index = 0
        for _module, import_name, import_kind, _description in _collect_imports(data):
            if import_kind != 0:
                continue
            named_indices.setdefault(import_name, set()).add(function_import_index)
            function_import_index += 1
        for function_index, function_name in _collect_func_names(data).items():
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

    preserved_fixed_indices = (
        tuple(resolve_entry(slot) for slot in range(layout.fixed_prefix_len))
        if include_fixed_prefix
        else ()
    )
    fixed_indices = preserved_fixed_indices
    app_indices = tuple(
        resolve_entry(slot)
        for slot in range(layout.fixed_prefix_len, total_entry_count)
    )
    reserved_end = WASM_RESERVED_RUNTIME_CALLABLE_BASE + len(
        WASM_RESERVED_RUNTIME_CALLABLES
    )
    if (
        include_fixed_prefix
        and WASM_RESERVED_RUNTIME_CALLABLE_BASE < len(fixed_indices) < reserved_end
    ):
        raise ValueError("callable table truncates reserved runtime callable region")
    if (
        include_fixed_prefix
        and override_reserved_direct
        and len(fixed_indices) >= reserved_end
    ):
        mutable_fixed_indices = list(fixed_indices)
        for (
            index,
            runtime_name,
            _import_name,
            _arity,
            dispatch,
        ) in WASM_RESERVED_RUNTIME_CALLABLES:
            if dispatch != "direct":
                continue
            logical_slot = WASM_RESERVED_RUNTIME_CALLABLE_BASE + index
            runtime_function_index = exports.get(runtime_name)
            if runtime_function_index is None:
                candidates = named_indices.get(runtime_name, set())
                if len(candidates) == 1:
                    runtime_function_index = next(iter(candidates))
            if runtime_function_index is None:
                raise ValueError(
                    f"linked wasm is missing reserved runtime export {runtime_name}"
                )
            mutable_fixed_indices[logical_slot] = runtime_function_index
        fixed_indices = tuple(mutable_fixed_indices)
    return _CallableTableEntryPlan(
        fixed_indices,
        app_indices,
        owns_runtime_region=include_fixed_prefix,
        preserved_fixed_indices=preserved_fixed_indices,
    )


def _merge_linked_callable_table(
    raw_entries: object,
    layout: CallableTableLayout,
    entry_plan: _CallableTableEntryPlan,
) -> int:
    """Merge compiler-owned entries with final linker growth.

    Fixed shared-runtime entries may be present in both the compiler output and
    the relocatable runtime.  That overlap is valid only when both inputs resolve
    to the same canonical post-link function identity.  A monolithic link may
    also contain contiguous runtime-owned growth between the fixed prefix and the
    finalized app base.  With no fixed prefix there is no ABI occupancy anchor,
    so that region begins at its first observed runtime slot.  App entries are
    already embedded in code as table immediates and must retain their canonical
    identities.  Native/future linked entries form a second contiguous suffix at
    the immutable pre-link app end.
    """

    if not isinstance(raw_entries, list):
        raise ValueError("linked WASM facts omitted callable-table entries")
    rows_by_slot: dict[int, tuple[int, int, int]] = {}
    for entry in raw_entries:
        if (
            not isinstance(entry, list)
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
        slot, function_index, type_index, role = cast(list[int], entry)
        if slot in rows_by_slot:
            raise ValueError(
                f"linked WASM facts contain duplicate callable-table slot {slot}"
            )
        rows_by_slot[slot] = (function_index, type_index, role)

    expected_owned = entry_plan.slot_allowed_function_indices(layout)
    app_end = _callable_app_end(layout)
    for slot, expected_function_indices in expected_owned.items():
        actual = rows_by_slot.get(slot)
        if actual is None:
            # wasm-ld GC may omit an element row even though the canonical
            # linker symbol remains. The already-resolved entry plan is the
            # publication authority and will restore this slot immediately
            # after the merge check.
            continue
        if actual[0] not in expected_function_indices:
            raise ValueError(
                "linked WASM changed compiler-owned callable-table identity: "
                f"slot={slot}, expected_functions={sorted(expected_function_indices)}, "
                f"actual_function={actual[0]}"
            )

    fixed_end = layout.fixed_prefix_base + layout.fixed_prefix_len
    runtime_growth_slots: list[int] = []
    suffix_growth_slots: list[int] = []
    for slot in rows_by_slot:
        if slot in expected_owned:
            continue
        if (
            entry_plan.owns_runtime_region
            and fixed_end <= slot < layout.finalized_app_base
        ):
            runtime_growth_slots.append(slot)
            continue
        if slot < app_end:
            raise ValueError(
                "linked WASM entry without compiler identity overlaps the pre-link callable "
                f"region: slot={slot}, region=[0,{app_end})"
            )
        suffix_growth_slots.append(slot)
    runtime_growth_slots.sort()
    # An empty fixed prefix owns no starting slot. wasm-ld reserves slot zero
    # for the null function pointer, while other valid producers may leave a
    # larger leading hole. In that shape the first observed runtime row is the
    # artifact-local occupancy base; a non-empty ABI prefix remains anchored at
    # its declared end.
    runtime_growth_base = (
        fixed_end
        if layout.fixed_prefix_len > 0 or not runtime_growth_slots
        else runtime_growth_slots[0]
    )
    for offset, slot in enumerate(runtime_growth_slots):
        expected_slot = runtime_growth_base + offset
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
        raise ValueError("callable-table fixed prefix base must fit i32")
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
    include_fixed_prefix: bool = True,
    override_reserved_direct: bool = True,
    entry_plan: _CallableTableEntryPlan | None = None,
    _parse_sections: Callable[[bytes], list[tuple[int, bytes]]],
    _build_sections: Callable[[list[tuple[int, bytes]]], bytes],
) -> bytes:
    total_entry_count = layout.fixed_prefix_len + layout.app_entry_count
    if total_entry_count == 0:
        return data
    if entry_plan is None:
        entry_plan = _resolve_callable_table_entry_plan(
            data,
            layout,
            entry_symbol_names=entry_symbol_names,
            include_fixed_prefix=include_fixed_prefix,
            override_reserved_direct=override_reserved_direct,
        )
    fixed_indices = entry_plan.fixed_indices
    app_indices = entry_plan.app_indices
    sections = _parse_sections(data)
    element_indices = [
        index
        for index, (section_id, _payload) in enumerate(sections)
        if section_id == 9
    ]
    if len(element_indices) != 1:
        raise ValueError(
            "linked wasm must contain exactly one element section before fixed-prefix publication"
        )
    section_index = element_indices[0]
    section_id, payload = sections[section_index]
    segment_count, segment_offset = _read_varuint(payload, 0)
    added_segment_count = int(bool(fixed_indices)) + int(bool(app_indices))
    appended = bytearray(_write_varuint(segment_count + added_segment_count))
    appended.extend(payload[segment_offset:])
    for base, indices in (
        (layout.fixed_prefix_base, fixed_indices),
        (layout.finalized_app_base, app_indices),
    ):
        if not indices:
            continue
        appended.extend(_write_varuint(0))
        appended.append(0x41)
        appended.extend(_write_varsint32(base))
        appended.append(0x0B)
        appended.extend(_write_varuint(len(indices)))
        for function_index in indices:
            appended.extend(_write_varuint(function_index))
    sections[section_index] = (section_id, bytes(appended))
    return _build_sections(sections)
