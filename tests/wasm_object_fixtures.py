"""Canonical synthetic relocatable-WASM objects for compiler integration tests."""

from __future__ import annotations

from collections.abc import Sequence

from molt._wasm_abi_generated import (
    WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES,
    WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES,
    wasm_import_signature,
)


def wasm_exporting_i64_unary_symbol(
    symbol: str,
    *,
    imports: tuple[str, ...] = (),
    memory_imports: tuple[str, ...] = (),
) -> bytes:
    return wasm_exporting_i64_unary_symbols(
        (symbol,), imports=imports, memory_imports=memory_imports
    )


def wasm_exporting_i64_unary_symbols(
    symbols: Sequence[str],
    *,
    imports: tuple[str, ...] = (),
    memory_imports: tuple[str, ...] = (),
    undefined_data_symbols: tuple[str, ...] = (),
    defined_data_symbols: tuple[str, ...] = (),
) -> bytes:
    """Build a valid relocatable module with canonical linking-symbol custody."""
    if not symbols:
        raise ValueError("at least one exported symbol is required")
    if any(not symbol for symbol in symbols) or len(set(symbols)) != len(symbols):
        raise ValueError("exported symbols must be non-empty and unique")
    if any(not name for name in (*imports, *memory_imports, *undefined_data_symbols)):
        raise ValueError("import names must be non-empty")
    if any(not name for name in defined_data_symbols) or len(
        set(defined_data_symbols)
    ) != len(defined_data_symbols):
        raise ValueError("defined data symbols must be non-empty and unique")
    if set(defined_data_symbols) & set((*symbols, *imports, *undefined_data_symbols)):
        raise ValueError("defined data symbols must not overlap other symbols")

    def uleb(value: int) -> bytes:
        if value < 0:
            raise ValueError("unsigned LEB128 values must be non-negative")
        out = bytearray()
        while True:
            byte = value & 0x7F
            value >>= 7
            out.append(byte | 0x80 if value else byte)
            if not value:
                return bytes(out)

    def wasm_string(value: str) -> bytes:
        encoded = value.encode("utf-8")
        return uleb(len(encoded)) + encoded

    def section(section_id: int, payload: bytes) -> bytes:
        return bytes([section_id]) + uleb(len(payload)) + payload

    value_types = {"i32": b"\x7f", "i64": b"\x7e", "f32": b"\x7d", "f64": b"\x7c"}

    def expected_signature(name: str) -> tuple[tuple[str, ...], tuple[str, ...]]:
        module = WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES.get(
            name, ("env", "function")
        )[0]
        external = WASM_EXTERNAL_NATIVE_ARTIFACT_FUNCTION_SIGNATURES.get((module, name))
        if external is not None:
            result = str(external["result"])
            return (
                tuple(str(value) for value in external["params"]),
                () if result == "nil" else tuple(result.split(", ")),
            )
        runtime = wasm_import_signature(name)
        return runtime if runtime is not None else (("i64",), ("i64",))

    def type_entry(signature: tuple[tuple[str, ...], tuple[str, ...]]) -> bytes:
        params, results = signature
        return (
            b"\x60"
            + uleb(len(params))
            + b"".join(value_types[value] for value in params)
            + uleb(len(results))
            + b"".join(value_types[value] for value in results)
        )

    defined_signature = (("i64",), ("i64",))
    type_signatures = [defined_signature]
    import_signatures = [expected_signature(name) for name in imports]
    for signature in import_signatures:
        if signature not in type_signatures:
            type_signatures.append(signature)
    type_section = uleb(len(type_signatures)) + b"".join(
        type_entry(signature) for signature in type_signatures
    )
    import_count = len(imports) + len(memory_imports)
    import_section = b""
    if import_count:
        import_section = section(
            2,
            uleb(import_count)
            + b"".join(
                wasm_string(
                    WASM_EXTERNAL_NATIVE_ARTIFACT_IMPORT_SHAPES.get(
                        import_name, ("env", "function")
                    )[0]
                )
                + wasm_string(import_name)
                + b"\x00"
                + uleb(type_signatures.index(signature))
                for import_name, signature in zip(
                    imports, import_signatures, strict=True
                )
            )
            + b"".join(
                wasm_string("env") + wasm_string(import_name) + b"\x02\x00" + uleb(1)
                for import_name in memory_imports
            ),
        )
    function_section = uleb(len(symbols)) + uleb(0) * len(symbols)
    export_section = uleb(len(symbols)) + b"".join(
        wasm_string(symbol) + b"\x00" + uleb(len(imports) + index)
        for index, symbol in enumerate(symbols)
    )
    body = uleb(0) + b"\x42\x00\x0b"
    code_section = uleb(len(symbols)) + (uleb(len(body)) + body) * len(symbols)
    linking_entries = [
        b"\x00" + uleb(0) + uleb(len(imports) + index) + wasm_string(symbol)
        for index, symbol in enumerate(symbols)
    ]
    linking_entries.extend(
        b"\x00" + uleb(0x50) + uleb(index) + wasm_string(import_name)
        for index, import_name in enumerate(imports)
    )
    linking_entries.extend(
        b"\x01" + uleb(0x10) + wasm_string(name) for name in undefined_data_symbols
    )
    linking_entries.extend(
        b"\x01" + uleb(0) + wasm_string(name) + uleb(0) + uleb(index) + uleb(1)
        for index, name in enumerate(defined_data_symbols)
    )
    linking_symbol_table = uleb(len(linking_entries)) + b"".join(linking_entries)
    linking_payload = (
        wasm_string("linking")
        + uleb(2)
        + (
            section(5, uleb(1) + wasm_string(".data.fixture") + uleb(0) + uleb(0))
            if defined_data_symbols
            else b""
        )
        + b"\x08"
        + uleb(len(linking_symbol_table))
        + linking_symbol_table
    )
    return (
        b"\x00asm\x01\x00\x00\x00"
        + section(0, linking_payload)
        + section(1, type_section)
        + import_section
        + section(3, function_section)
        + (
            section(5, b"\x01\x00\x01")
            if defined_data_symbols and not memory_imports
            else b""
        )
        + section(7, export_section)
        + section(10, code_section)
        + (
            section(
                11,
                b"\x01\x00\x41\x00\x0b"
                + uleb(len(defined_data_symbols))
                + bytes(len(defined_data_symbols)),
            )
            if defined_data_symbols
            else b""
        )
    )
