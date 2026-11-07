"""Publish code-owned source locations at the final frontend boundary.

Location indices describe source positions, not interpreter bytecode. The bytes
use CPython's documented co_linetable format (Objects/locations.md); each row
covers one two-byte position. No source file is needed by the emitted program.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any

from molt.frontend.lowering.op_kinds_generated import (
    ExceptionEdgeKind,
    FRONTEND_ARBITRARY_HEAP_EFFECT,
    CHECK_EXCEPTION_SKIP_KINDS,
    FRONTEND_EFFECT_PURE_KINDS,
    FRONTEND_EFFECT_CONTROL_KINDS,
    FRONTEND_WIRE_KIND,
    WireKinds,
)


@dataclass(frozen=True, order=True)
class SourcePosition:
    line: int
    end_line: int
    column: int
    end_column: int

    @classmethod
    def from_operation(cls, op: dict[str, Any]) -> SourcePosition | None:
        line = op.get("source_line")
        end_line = op.get("source_end_line")
        if not isinstance(line, int) or line <= 0 or not isinstance(end_line, int):
            return None
        return cls(
            line,
            end_line,
            op.get("col_offset", -1),
            op.get("end_col_offset", -1),
        )


def _varint(value: int) -> bytes:
    if value < 0:
        raise ValueError("location varint must be nonnegative")
    out = bytearray()
    while value >= 64:
        out.append((value & 63) | 64)
        value >>= 6
    out.append(value)
    return bytes(out)


def encode_locations(first_line: int, rows: list[SourcePosition]) -> bytes:
    out = bytearray()
    previous = first_line
    for row in rows:
        delta = row.line - previous
        signed = (-delta * 2 + 1) if delta < 0 else delta * 2
        if row.column < 0 or row.end_column < 0:
            out.append(0xE8)  # no columns, one code unit
            out.extend(_varint(signed))
        else:
            out.append(0xF0)  # long form, one code unit
            out.extend(_varint(signed))
            out.extend(_varint(row.end_line - row.line))
            out.extend(_varint(row.column + 1))
            out.extend(_varint(row.end_column + 1))
        previous = row.line
    return bytes(out)


_PURE_WIRE = frozenset(
    FRONTEND_WIRE_KIND[kind]
    for kind in FRONTEND_EFFECT_PURE_KINDS
    if kind in FRONTEND_WIRE_KIND
)
_CONTROL_WIRE = frozenset(
    FRONTEND_WIRE_KIND[kind]
    for kind in FRONTEND_EFFECT_CONTROL_KINDS
    if kind in FRONTEND_WIRE_KIND
)
# Structural control and exception-state transport do not execute a source
# operation. Reuse the generated callback effects, independently of their
# control classification. Suspension, returns and Python truthiness remain
# observers. Region markers execute nothing despite their conservative effects.
_NO_LOCATION_OBSERVER = frozenset(
    FRONTEND_WIRE_KIND[kind]
    for kind in FRONTEND_EFFECT_CONTROL_KINDS
    if kind in FRONTEND_WIRE_KIND
    and kind in CHECK_EXCEPTION_SKIP_KINDS
    and FRONTEND_ARBITRARY_HEAP_EFFECT.get(kind) is False
    and FRONTEND_WIRE_KIND[kind] not in {WireKinds.RET, WireKinds.RET_VOID}
) | frozenset(
    {
        WireKinds.LINE,
        WireKinds.CHECK_EXCEPTION,
        WireKinds.TRY_START,
        WireKinds.TRY_END,
    }
)


def publish_frame_locations(
    functions: list[dict[str, Any]], *, module_code_symbol: str = "molt_main"
) -> None:
    declarations: dict[str, tuple[dict[str, Any], int]] = {}
    for function in functions:
        definitions = {op["out"]: op for op in function["ops"] if op.get("out")}
        reserved = set(definitions) | set(function.get("params", ()))
        tables: dict[int, dict[str, Any]] = {}
        for op in function["ops"]:
            if op["kind"] != WireKinds.CODE_SLOT_SET:
                continue
            code = definitions[op["args"][0]]
            if code["kind"] != WireKinds.CODE_NEW:
                raise ValueError("compiled code slot must publish its code constructor")
            # Constants are shared by frontend CSE. In particular this None
            # also supplies varargs/defaults and must never become table bytes.
            table_name = f"__molt_locations_{len(tables)}"
            while table_name in reserved:
                table_name += "_"
            reserved.add(table_name)
            table: dict[str, Any] = {"kind": WireKinds.CONST_BYTES, "out": table_name}
            code["args"] = list(code["args"])
            code["args"][3] = table_name
            tables[id(code)] = table
            first_line = definitions[code["args"][2]].get("value", 0)
            declarations[op["s_value"]] = (table, first_line)
        function["ops"] = [
            inserted
            for op in function["ops"]
            for inserted in ([tables[id(op)], op] if id(op) in tables else [op])
        ]

    owners: dict[str, str] = {}
    sites: dict[str, set[SourcePosition]] = {owner: set() for owner in declarations}
    for function in functions:
        entry = next(
            (op for op in function["ops"] if op["kind"] == WireKinds.TRACE_ENTER_SLOT),
            None,
        )
        owner = entry["s_value"] if entry else None
        if function.get("execution_context") == "inherited":
            owner = module_code_symbol
        if owner is None:
            continue
        if owner not in declarations:
            raise ValueError(f"frame entry has no code-location producer: {owner}")
        owners[function["name"]] = owner
        for op in function["ops"]:
            position = SourcePosition.from_operation(op)
            if position is not None:
                sites[owner].add(position)

    indices: dict[str, dict[SourcePosition, int]] = {}
    for owner, (table, first_line) in declarations.items():
        # Row zero is the stateful activation's observable prestart location.
        rows = [SourcePosition(first_line, first_line, -1, -1), *sorted(sites[owner])]
        indices[owner] = {row: index for index, row in enumerate(rows)}
        output = table["out"]
        table.clear()
        table.update(
            kind=WireKinds.CONST_BYTES,
            out=output,
            bytes=list(encode_locations(first_line, rows)),
        )

    for function in functions:
        owner = owners.get(function["name"])
        entered = function.get("execution_context") == "inherited"
        last_location: int | None = None
        rewritten: list[dict[str, Any]] = []
        for op in function["ops"]:
            position = SourcePosition.from_operation(op)
            # This frontend-only end-line fact is captured in the code table;
            # the explicit publication carries its index through every IR.
            op.pop("source_end_line", None)
            kind = op["kind"]
            if (
                owner is not None
                and entered
                and position is not None
                and kind not in _PURE_WIRE
                and kind not in _NO_LOCATION_OBSERVER
                # A scope restore forwards the incoming execution site. Its
                # release may run a finalizer, but is not a new source binding
                # statement; do not replace the incoming site with the scope's
                # synthetic coordinates, or split its landing/restore pair.
                and not (
                    op.get("exception_source") == ExceptionEdgeKind.UNWIND
                    and kind
                    in {
                        WireKinds.FRAME_HOME_STORE,
                        WireKinds.FRAME_HOME_CELL,
                        WireKinds.FRAME_HOME_PRIVATE_CELL,
                    }
                )
                and not (
                    kind in {WireKinds.RET, WireKinds.RET_VOID}
                    and rewritten
                    and rewritten[-1]["kind"] == WireKinds.TRACE_EXIT
                )
            ):
                location = indices[owner][position]
                if location != last_location:
                    rewritten.append(
                        {
                            "kind": WireKinds.FRAME_SET_LOCATION,
                            "value": location,
                            "source_line": position.line,
                            "col_offset": position.column,
                            "end_col_offset": position.end_column,
                        }
                    )
                    last_location = location
            rewritten.append(op)
            # A check's normal successor is this fallthrough. It changes no
            # location; its exceptional successor publishes at its own label.
            # Retain the known word here rather than repeating an identical
            # store after every operation in an authored statement.
            if kind in _CONTROL_WIRE and kind not in {
                WireKinds.TRACE_EXIT,
                WireKinds.CHECK_EXCEPTION,
            }:
                last_location = None
            if kind == WireKinds.TRACE_ENTER_SLOT:
                entered = True
        function["ops"] = rewritten
