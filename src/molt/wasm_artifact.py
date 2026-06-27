from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping, Sequence

WASM_HEADER = b"\x00asm\x01\x00\x00\x00"

WASM_SECTION_NAMES: dict[int, str] = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data_count",
}

_VALUE_TYPE_NAMES = {
    0x7F: "i32",
    0x7E: "i64",
    0x7D: "f32",
    0x7C: "f64",
    0x7B: "v128",
    0x70: "funcref",
    0x6F: "externref",
}


def wasm_section_name(section_id: int) -> str:
    return WASM_SECTION_NAMES.get(section_id, f"unknown({section_id})")


@dataclass(frozen=True)
class WasmSectionSpan:
    id: int
    offset: int
    size: int
    custom_name: str = ""

    @property
    def name(self) -> str:
        return wasm_section_name(self.id)

    def to_dict(self) -> dict[str, object]:
        data: dict[str, object] = {
            "id": self.id,
            "name": self.name,
            "offset": self.offset,
            "size": self.size,
        }
        if self.custom_name:
            data["custom_name"] = self.custom_name
        return data


@dataclass(frozen=True)
class WasmImport:
    module: str
    name: str
    kind: int
    type_index: int | None = None
    minimum: int | None = None


@dataclass(frozen=True)
class WasmExport:
    name: str
    kind: int
    index: int

    def to_tuple(self) -> tuple[str, int, int]:
        return self.name, self.kind, self.index


@dataclass(frozen=True)
class WasmCodeMetrics:
    defined_function_count: int
    code_section_size: int

    def to_tuple(self) -> tuple[int, int]:
        return self.defined_function_count, self.code_section_size


@dataclass(frozen=True)
class WasmFunctionBody:
    index: int
    offset: int
    body_size_bytes: int
    name: str | None = None

    def to_dict(self) -> dict[str, object]:
        data: dict[str, object] = {
            "index": self.index,
            "offset": self.offset,
            "body_size_bytes": self.body_size_bytes,
        }
        if self.name is not None:
            data["name"] = self.name
        return data


def _read_wasm_varuint(data: bytes, offset: int) -> tuple[int, int]:
    result = 0
    shift = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading wasm varuint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if byte & 0x80 == 0:
            return result, offset
        shift += 7
        if shift > 63:
            raise ValueError("wasm varuint is too large")


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    length, offset = _read_wasm_varuint(data, offset)
    end = offset + length
    if end > len(data):
        raise ValueError("Unexpected EOF while reading wasm string")
    return data[offset:end].decode("utf-8"), end


def _write_wasm_varuint(value: int) -> bytes:
    if value < 0:
        raise ValueError("wasm varuint must be non-negative")
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        if value:
            out.append(byte | 0x80)
        else:
            out.append(byte)
            return bytes(out)


def _write_wasm_string(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _write_wasm_varuint(len(encoded)) + encoded


def _read_wasm_custom_section_name(payload: bytes) -> str:
    if not payload:
        return ""
    try:
        name, _ = _read_wasm_string(payload, 0)
    except (UnicodeDecodeError, ValueError):
        return "<unparseable>"
    return name


def parse_wasm_section_spans(data: bytes) -> list[WasmSectionSpan]:
    if len(data) < len(WASM_HEADER) or data[: len(WASM_HEADER)] != WASM_HEADER:
        raise ValueError("Invalid wasm binary")
    offset = len(WASM_HEADER)
    sections: list[WasmSectionSpan] = []
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_wasm_varuint(data, offset)
        section_end = offset + section_size
        if section_end > len(data):
            raise ValueError("Invalid wasm section length")
        payload = data[offset:section_end]
        custom_name = _read_wasm_custom_section_name(payload) if section_id == 0 else ""
        sections.append(
            WasmSectionSpan(
                id=section_id,
                offset=offset,
                size=section_size,
                custom_name=custom_name,
            )
        )
        offset = section_end
    return sections


def read_wasm_section_spans(path: Path) -> list[WasmSectionSpan]:
    return parse_wasm_section_spans(path.read_bytes())


def _parse_wasm_sections(data: bytes) -> list[tuple[int, bytes]]:
    return [
        (span.id, data[span.offset : span.offset + span.size])
        for span in parse_wasm_section_spans(data)
    ]


def _parse_wasm_file_sections(path: Path) -> list[tuple[int, bytes]]:
    return _parse_wasm_sections(path.read_bytes())


def inspect_wasm_binary(path: Path) -> Literal["missing", "invalid", "valid"]:
    try:
        read_wasm_section_spans(path)
    except OSError:
        return "missing"
    except ValueError:
        return "invalid"
    return "valid"


def is_valid_wasm_binary(path: Path) -> bool:
    return inspect_wasm_binary(path) == "valid"


def has_nonempty_wasm_code_section(path: Path) -> bool:
    return read_wasm_code_metrics(path).defined_function_count > 0


def _build_wasm_sections(sections: Sequence[tuple[int, bytes]]) -> bytes:
    out = bytearray(WASM_HEADER)
    for section_id, payload in sections:
        out.append(section_id)
        out.extend(_write_wasm_varuint(len(payload)))
        out.extend(payload)
    return bytes(out)


def _read_wasm_limits(data: bytes, offset: int) -> tuple[int, int, int]:
    flags, offset = _read_wasm_varuint(data, offset)
    minimum, offset = _read_wasm_varuint(data, offset)
    if flags & 0x1:
        _, offset = _read_wasm_varuint(data, offset)
    return flags, minimum, offset


def _read_wasm_import_description(
    payload: bytes, cursor: int, *, module: str, name: str, kind: int
) -> tuple[WasmImport, int]:
    if kind == 0:
        type_index, cursor = _read_wasm_varuint(payload, cursor)
        return WasmImport(module, name, kind, type_index=type_index), cursor
    if kind == 1:
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading table type")
        cursor += 1
        _, minimum, cursor = _read_wasm_limits(payload, cursor)
        return WasmImport(module, name, kind, minimum=minimum), cursor
    if kind == 2:
        _, minimum, cursor = _read_wasm_limits(payload, cursor)
        return WasmImport(module, name, kind, minimum=minimum), cursor
    if kind == 3:
        if cursor + 2 > len(payload):
            raise ValueError("Unexpected EOF while reading global type")
        return WasmImport(module, name, kind), cursor + 2
    if kind == 4:
        if cursor >= len(payload):
            raise ValueError("Unexpected EOF while reading tag attribute")
        cursor += 1
        _, cursor = _read_wasm_varuint(payload, cursor)
        return WasmImport(module, name, kind), cursor
    raise ValueError(f"Unknown wasm import kind {kind}")


def _iter_wasm_imports(
    sections: Sequence[tuple[int, bytes]],
) -> list[WasmImport]:
    imports: list[WasmImport] = []
    for section_id, payload in sections:
        if section_id != 2:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for _ in range(count):
            module, cursor = _read_wasm_string(payload, cursor)
            name, cursor = _read_wasm_string(payload, cursor)
            if cursor >= len(payload):
                raise ValueError("Unexpected EOF while reading import")
            kind = payload[cursor]
            cursor += 1
            wasm_import, cursor = _read_wasm_import_description(
                payload, cursor, module=module, name=name, kind=kind
            )
            imports.append(wasm_import)
        break
    return imports


def parse_wasm_imports(
    data: bytes, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmImport]:
    try:
        return _iter_wasm_imports(_parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return []
        raise


def read_wasm_imports(
    path: Path, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmImport]:
    try:
        return parse_wasm_imports(path.read_bytes(), on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return []
        raise


def _skip_wasm_init_expr(data: bytes, offset: int) -> tuple[int, int | None]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm init expr")
    opcode = data[offset]
    offset += 1
    value: int | None = None
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varuint(data, offset)
    elif opcode == 0x23:  # global.get
        _, offset = _read_wasm_varuint(data, offset)
    else:
        raise ValueError(f"Unsupported wasm init expr opcode 0x{opcode:02x}")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Malformed wasm init expr")
    return offset + 1, value


def _read_wasm_ref_func_expr(data: bytes, offset: int) -> tuple[int, int | None]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm element expr")
    opcode = data[offset]
    offset += 1
    func_index: int | None = None
    if opcode == 0xD2:  # ref.func
        func_index, offset = _read_wasm_varuint(data, offset)
    elif opcode == 0xD0:  # ref.null
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading ref.null type")
        offset += 1
    else:
        raise ValueError(f"Unsupported wasm element expr opcode 0x{opcode:02x}")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Malformed wasm ref.func expr")
    return offset + 1, func_index


def _collect_wasm_active_table_function_slots(data: bytes) -> dict[int, int]:
    sections = _parse_wasm_sections(data)
    slots: dict[int, int] = {}
    for section_id, payload in sections:
        if section_id != 9:
            continue
        offset = 0
        count, offset = _read_wasm_varuint(payload, offset)
        for _ in range(count):
            flags, offset = _read_wasm_varuint(payload, offset)
            table_index = 0
            base_offset: int | None = None
            if flags == 0:
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    func_index, offset = _read_wasm_varuint(payload, offset)
                    if base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 1:
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    _, offset = _read_wasm_varuint(payload, offset)
            elif flags == 2:
                table_index, offset = _read_wasm_varuint(payload, offset)
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    func_index, offset = _read_wasm_varuint(payload, offset)
                    if table_index == 0 and base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 3:
                if offset < len(payload) and payload[offset] == 0x00:
                    offset += 1
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    _, offset = _read_wasm_varuint(payload, offset)
            elif flags == 4:
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    offset, func_index = _read_wasm_ref_func_expr(payload, offset)
                    if func_index is not None and base_offset is not None:
                        slots[base_offset + elem_index] = func_index
            elif flags == 5:
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    offset, _ = _read_wasm_ref_func_expr(payload, offset)
            elif flags == 6:
                table_index, offset = _read_wasm_varuint(payload, offset)
                offset, base_offset = _skip_wasm_init_expr(payload, offset)
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for elem_index in range(elem_count):
                    offset, func_index = _read_wasm_ref_func_expr(payload, offset)
                    if (
                        table_index == 0
                        and func_index is not None
                        and base_offset is not None
                    ):
                        slots[base_offset + elem_index] = func_index
            elif flags == 7:
                offset += 1  # reftype
                elem_count, offset = _read_wasm_varuint(payload, offset)
                for _ in range(elem_count):
                    offset, _ = _read_wasm_ref_func_expr(payload, offset)
            else:
                raise ValueError(f"Unsupported wasm element flags {flags}")
    return slots


def _collect_wasm_export_names(path: Path) -> set[str]:
    return {
        export.name for export in read_wasm_function_exports(path, on_error="ignore")
    }


def _wasm_import_minima(path: Path) -> tuple[int | None, int | None]:
    memory_min: int | None = None
    table_min: int | None = None
    for wasm_import in _iter_wasm_imports(_parse_wasm_file_sections(path)):
        if (
            wasm_import.kind == 1
            and wasm_import.module == "env"
            and wasm_import.name == "__indirect_function_table"
        ):
            table_min = wasm_import.minimum
        elif (
            wasm_import.kind == 2
            and wasm_import.module == "env"
            and wasm_import.name == "memory"
        ):
            memory_min = wasm_import.minimum
    return memory_min, table_min


def _read_wasm_varint(data: bytes, offset: int, bits: int) -> tuple[int, int]:
    result = 0
    shift = 0
    byte = 0
    while True:
        if offset >= len(data):
            raise ValueError("Unexpected EOF while reading varint")
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        shift += 7
        if byte & 0x80 == 0:
            break
        if shift > bits + 7:
            raise ValueError("varint too large")
    if shift < bits and (byte & 0x40):
        result |= -1 << shift
    return result, offset


def _read_wasm_const_expr_i32(data: bytes, offset: int) -> tuple[int, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading const expr")
    opcode = data[offset]
    offset += 1
    if opcode == 0x41:  # i32.const
        value, offset = _read_wasm_varint(data, offset, 32)
    elif opcode == 0x42:  # i64.const
        value, offset = _read_wasm_varint(data, offset, 64)
    else:
        raise ValueError("Unsupported const expr opcode")
    if offset >= len(data) or data[offset] != 0x0B:
        raise ValueError("Invalid const expr terminator")
    offset += 1
    return value, offset


def _read_wasm_table_min(path: Path) -> int | None:
    try:
        imports = _iter_wasm_imports(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return None
    for wasm_import in imports:
        if (
            wasm_import.kind == 1
            and wasm_import.module == "env"
            and wasm_import.name == "__indirect_function_table"
        ):
            return wasm_import.minimum
    return None


def _read_wasm_data_end(path: Path) -> int | None:
    try:
        sections = _parse_wasm_file_sections(path)
    except (OSError, ValueError):
        return None
    max_end = None
    try:
        for section_id, payload in sections:
            if section_id != 11:
                continue
            cursor = 0
            count, cursor = _read_wasm_varuint(payload, cursor)
            for _ in range(count):
                if cursor >= len(payload):
                    raise ValueError("Unexpected EOF while reading data segment")
                flags = payload[cursor]
                cursor += 1
                is_passive = flags & 0x1
                has_memidx = flags & 0x2
                if has_memidx:
                    _, cursor = _read_wasm_varuint(payload, cursor)
                if is_passive:
                    size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                    cursor += size_bytes
                    continue
                offset_val, cursor = _read_wasm_const_expr_i32(payload, cursor)
                size_bytes, cursor = _read_wasm_varuint(payload, cursor)
                cursor += size_bytes
                if offset_val < 0:
                    continue
                end_val = offset_val + size_bytes
                if max_end is None or end_val > max_end:
                    max_end = end_val
    except ValueError:
        return None
    return max_end


def _read_wasm_memory_min_bytes(path: Path) -> int | None:
    try:
        sections = _parse_wasm_file_sections(path)
    except (OSError, ValueError):
        return None
    memory_pages: int | None = None
    try:
        for wasm_import in _iter_wasm_imports(sections):
            if (
                wasm_import.kind == 2
                and wasm_import.module == "env"
                and wasm_import.name == "memory"
            ):
                memory_pages = max(memory_pages or 0, wasm_import.minimum or 0)
        for section_id, payload in sections:
            cursor = 0
            if section_id == 5:
                count, cursor = _read_wasm_varuint(payload, cursor)
                for _ in range(count):
                    _, minimum, cursor = _read_wasm_limits(payload, cursor)
                    memory_pages = max(memory_pages or 0, minimum)
    except ValueError:
        return None
    if memory_pages is None:
        return None
    return memory_pages * 65536


def _collect_wasm_module_import_names(path: Path, module_name: str) -> set[str]:
    try:
        imports = _iter_wasm_imports(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return set()
    return {
        wasm_import.name for wasm_import in imports if wasm_import.module == module_name
    }


def _read_wasm_import_metrics(path: Path) -> dict[str, int] | None:
    try:
        imports = _iter_wasm_imports(_parse_wasm_file_sections(path))
    except (OSError, ValueError):
        return None
    return {
        "total": len(imports),
        "functions": sum(1 for wasm_import in imports if wasm_import.kind == 0),
        "tables": sum(1 for wasm_import in imports if wasm_import.kind == 1),
    }


def _read_wasm_value_type(data: bytes, offset: int) -> tuple[str, int]:
    if offset >= len(data):
        raise ValueError("Unexpected EOF while reading wasm value type")
    code = data[offset]
    offset += 1
    name = _VALUE_TYPE_NAMES.get(code)
    if name is None:
        raise ValueError(f"Unsupported wasm value type 0x{code:02x}")
    return name, offset


def _read_wasm_value_type_vec(data: bytes, offset: int) -> tuple[list[str], int]:
    count, offset = _read_wasm_varuint(data, offset)
    values: list[str] = []
    for _ in range(count):
        value, offset = _read_wasm_value_type(data, offset)
        values.append(value)
    return values, offset


def _format_wasm_result_kind(results: Sequence[str]) -> str:
    if not results:
        return "nil"
    return ", ".join(results)


def _read_wasm_type_signatures(
    sections: Sequence[tuple[int, bytes]],
) -> dict[int, tuple[list[str], str]]:
    signatures: dict[int, tuple[list[str], str]] = {}
    for section_id, payload in sections:
        if section_id != 1:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for type_index in range(count):
            if cursor >= len(payload) or payload[cursor] != 0x60:
                raise ValueError("Unsupported wasm type form")
            cursor += 1
            params, cursor = _read_wasm_value_type_vec(payload, cursor)
            results, cursor = _read_wasm_value_type_vec(payload, cursor)
            signatures[type_index] = (params, _format_wasm_result_kind(results))
        break
    return signatures


def _read_wasm_import_function_type_indices(
    sections: Sequence[tuple[int, bytes]],
) -> tuple[list[tuple[str, str, int]], int]:
    imports: list[tuple[str, str, int]] = []
    import_function_count = 0
    for wasm_import in _iter_wasm_imports(sections):
        if wasm_import.kind != 0:
            continue
        if wasm_import.type_index is None:
            raise ValueError(
                f"Missing wasm function import type for {wasm_import.name}"
            )
        imports.append((wasm_import.module, wasm_import.name, wasm_import.type_index))
        import_function_count += 1
    return imports, import_function_count


def _count_wasm_function_imports(sections: Sequence[tuple[int, bytes]]) -> int:
    return sum(
        1 for wasm_import in _iter_wasm_imports(sections) if wasm_import.kind == 0
    )


def _read_wasm_function_type_indices(
    sections: Sequence[tuple[int, bytes]],
    import_function_type_indices: Sequence[int],
) -> dict[int, int]:
    function_type_indices = {
        index: type_index
        for index, type_index in enumerate(import_function_type_indices)
    }
    function_index = len(import_function_type_indices)
    for section_id, payload in sections:
        if section_id != 3:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for _ in range(count):
            type_index, cursor = _read_wasm_varuint(payload, cursor)
            function_type_indices[function_index] = type_index
            function_index += 1
        break
    return function_type_indices


def _read_wasm_function_name_map(
    data: bytes,
    section_spans: Sequence[WasmSectionSpan],
) -> dict[int, str]:
    name_map: dict[int, str] = {}
    for span in section_spans:
        if span.id != 0 or span.custom_name != "name":
            continue
        pos = span.offset + len(_write_wasm_string("name"))
        end = span.offset + span.size
        while pos < end:
            try:
                subsection_id = data[pos]
                pos += 1
                subsection_size, pos = _read_wasm_varuint(data, pos)
                subsection_end = pos + subsection_size
                if subsection_end > end:
                    raise ValueError("WASM name subsection extends past section")
                if subsection_id == 1:
                    count, pos = _read_wasm_varuint(data, pos)
                    for _ in range(count):
                        if pos >= subsection_end:
                            break
                        func_index, pos = _read_wasm_varuint(data, pos)
                        func_name, pos = _read_wasm_string(data, pos)
                        name_map[func_index] = func_name
                pos = subsection_end
            except (UnicodeDecodeError, ValueError, IndexError):
                break
    return name_map


def read_wasm_function_bodies(path: Path) -> list[WasmFunctionBody]:
    data = path.read_bytes()
    section_spans = parse_wasm_section_spans(data)
    sections = [
        (span.id, data[span.offset : span.offset + span.size]) for span in section_spans
    ]
    imported_function_count = _count_wasm_function_imports(sections)
    name_map = _read_wasm_function_name_map(data, section_spans)

    bodies: list[WasmFunctionBody] = []
    for span in section_spans:
        if span.id != 10:
            continue
        pos = span.offset
        end = span.offset + span.size
        count, pos = _read_wasm_varuint(data, pos)
        for body_index in range(count):
            body_size, pos = _read_wasm_varuint(data, pos)
            body_start = pos
            body_end = body_start + body_size
            if body_end > end:
                raise ValueError("WASM function body extends past code section")
            function_index = imported_function_count + body_index
            bodies.append(
                WasmFunctionBody(
                    index=function_index,
                    offset=body_start,
                    body_size_bytes=body_size,
                    name=name_map.get(function_index),
                )
            )
            pos = body_end
        break
    return bodies


def read_wasm_code_metrics(path: Path) -> WasmCodeMetrics:
    try:
        data = path.read_bytes()
        for span in parse_wasm_section_spans(data):
            if span.id != 10:
                continue
            count, _ = _read_wasm_varuint(data, span.offset)
            return WasmCodeMetrics(count, span.size)
    except (OSError, ValueError, IndexError):
        return WasmCodeMetrics(0, 0)
    return WasmCodeMetrics(0, 0)


def _iter_wasm_exports(
    sections: Sequence[tuple[int, bytes]],
) -> list[WasmExport]:
    exports: list[WasmExport] = []
    for section_id, payload in sections:
        if section_id != 7:
            continue
        cursor = 0
        count, cursor = _read_wasm_varuint(payload, cursor)
        for _ in range(count):
            name, cursor = _read_wasm_string(payload, cursor)
            if cursor >= len(payload):
                raise ValueError("Unexpected EOF while reading export")
            kind = payload[cursor]
            cursor += 1
            index, cursor = _read_wasm_varuint(payload, cursor)
            exports.append(WasmExport(name, kind, index))
        break
    return exports


def parse_wasm_exports(
    data: bytes,
    *,
    kind: int | None = None,
    on_error: Literal["raise", "ignore"] = "raise",
) -> list[WasmExport]:
    try:
        exports = _iter_wasm_exports(_parse_wasm_sections(data))
    except (UnicodeDecodeError, ValueError, IndexError):
        if on_error == "ignore":
            return []
        raise
    if kind is None:
        return exports
    return [export for export in exports if export.kind == kind]


def read_wasm_exports(
    path: Path,
    *,
    kind: int | None = None,
    on_error: Literal["raise", "ignore"] = "raise",
) -> list[WasmExport]:
    try:
        return parse_wasm_exports(path.read_bytes(), kind=kind, on_error=on_error)
    except OSError:
        if on_error == "ignore":
            return []
        raise


def read_wasm_function_exports(
    path: Path, *, on_error: Literal["raise", "ignore"] = "raise"
) -> list[WasmExport]:
    return read_wasm_exports(path, kind=0, on_error=on_error)


def _read_wasm_function_export_map(
    sections: Sequence[tuple[int, bytes]],
) -> dict[str, int]:
    return {
        export.name: export.index
        for export in _iter_wasm_exports(sections)
        if export.kind == 0
    }


def _wasm_export_function_signatures(
    path: Path, *, export_name_prefix: str
) -> dict[str, dict[str, object]]:
    sections = _parse_wasm_file_sections(path)
    type_signatures = _read_wasm_type_signatures(sections)
    imports, _ = _read_wasm_import_function_type_indices(sections)
    function_type_indices = _read_wasm_function_type_indices(
        sections, [type_index for _, _, type_index in imports]
    )
    exports = _read_wasm_function_export_map(sections)

    export_signatures: dict[str, dict[str, object]] = {}
    for export_name, function_index in exports.items():
        if not export_name.startswith(export_name_prefix):
            continue
        type_index = function_type_indices.get(function_index)
        if type_index is None:
            raise ValueError(
                f"Missing wasm function type index for export {export_name}"
            )
        signature = type_signatures.get(type_index)
        if signature is None:
            raise ValueError(
                f"Missing wasm type index {type_index} for export {export_name}"
            )
        params, result_kind = signature
        export_signatures[export_name] = {
            "params": list(params),
            "result": result_kind,
        }
    return export_signatures


def _infer_wasm_table_base_from_export_names(
    export_signatures: Mapping[str, Mapping[str, object]],
    *,
    export_name_prefix: str,
) -> int | None:
    slots: list[int] = []
    for name in export_signatures:
        if not name.startswith(export_name_prefix):
            continue
        raw = name[len(export_name_prefix) :]
        try:
            slot = int(raw)
        except ValueError:
            continue
        if slot > 0:
            slots.append(slot)
    if not slots:
        return None
    return min(slots)
