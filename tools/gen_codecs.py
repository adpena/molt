#!/usr/bin/env python3
"""Generate text codec mapping tables from CPython stdlib encodings."""

from __future__ import annotations

import argparse
import ast
import re
import sys
from dataclasses import dataclass
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "runtime/molt-runtime-text/src/codec_registry.rs"
ENCODINGS = ROOT / "src/molt/stdlib/encodings"
ALIASES = ENCODINGS / "aliases.py"
OUT_RS = ROOT / "runtime/molt-runtime-text/src/charmap_codecs_generated.rs"
OUT_ALIASES_RS = ROOT / "runtime/molt-runtime-text/src/codec_aliases_generated.rs"


@dataclass(frozen=True)
class CodecDescriptor:
    kind: str
    canonical_label: str
    module: str
    rust_name: str


@dataclass(frozen=True)
class CharmapCodec:
    kind: str
    module: str
    rust_name: str


@dataclass(frozen=True)
class CharmapTables:
    decode_table: tuple[int, ...]
    encode_high_table: tuple[int, ...]
    encode_extended_entries: tuple[int, ...]


def _rust_const_name(kind: str) -> str:
    out: list[str] = []
    prev_lower = False
    for idx, ch in enumerate(kind):
        if ch == "_":
            out.append("_")
            prev_lower = False
            continue
        if ch.isupper() and idx > 0 and prev_lower:
            out.append("_")
        out.append(ch.upper())
        prev_lower = ch.islower() or ch.isdigit()
    return "".join(out)


def _descriptor_rows(registry_text: str) -> tuple[CodecDescriptor, ...]:
    descriptors: list[CodecDescriptor] = []
    pattern = re.compile(
        r"descriptor\(\s*EncodingKind::(?P<kind>[A-Za-z0-9_]+),\s*"
        r'"(?P<label>[^"]+)",\s*"(?P<module>[^"]+)"',
        re.MULTILINE,
    )
    for match in pattern.finditer(registry_text):
        kind = match.group("kind")
        descriptors.append(
            CodecDescriptor(
                kind=kind,
                canonical_label=match.group("label"),
                module=match.group("module"),
                rust_name=_rust_const_name(kind),
            )
        )
    if not descriptors:
        raise ValueError("could not locate codec descriptors")
    return tuple(descriptors)


def load_codec_descriptors() -> tuple[CodecDescriptor, ...]:
    return _descriptor_rows(REGISTRY.read_text(encoding="utf-8"))


def _charmap_kinds(registry_text: str) -> tuple[str, ...]:
    arm_lines: list[str] = []
    for line in registry_text.splitlines():
        if "EncodingKind::" in line:
            arm_lines.append(line)
        if "=> CodecRuntimeClass::" not in line:
            continue
        if "=> CodecRuntimeClass::Charmap" in line:
            variants = re.findall(r"EncodingKind::([A-Za-z0-9_]+)", "\n".join(arm_lines))
            if variants:
                return tuple(variants)
        arm_lines.clear()
    raise ValueError("could not locate CodecRuntimeClass::Charmap variants")


def load_charmap_codecs() -> tuple[CharmapCodec, ...]:
    registry_text = REGISTRY.read_text(encoding="utf-8")
    descriptors = {descriptor.kind: descriptor for descriptor in _descriptor_rows(registry_text)}
    codecs: list[CharmapCodec] = []
    for kind in _charmap_kinds(registry_text):
        try:
            descriptor = descriptors[kind]
        except KeyError as exc:
            raise ValueError(f"missing codec descriptor for {kind}") from exc
        codecs.append(CharmapCodec(kind, descriptor.module, descriptor.rust_name))
    return tuple(codecs)


CODEC_DESCRIPTORS: tuple[CodecDescriptor, ...] = load_codec_descriptors()
CHARMAP_CODECS: tuple[CharmapCodec, ...] = load_charmap_codecs()


def _repo_encoding_assignments(module_name: str) -> dict[str, ast.AST]:
    path = ENCODINGS / f"{module_name}.py"
    if not path.exists():
        raise ValueError(f"missing repo encoding module: {path.relative_to(ROOT)}")
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    assignments: dict[str, ast.AST] = {}
    for stmt in tree.body:
        if not isinstance(stmt, ast.Assign):
            continue
        for target in stmt.targets:
            if isinstance(target, ast.Name):
                assignments[target.id] = stmt.value
    return assignments


def _raw_decoding_table(module_name: str) -> str:
    assignments = _repo_encoding_assignments(module_name)
    try:
        table = ast.literal_eval(assignments["decoding_table"])
    except (KeyError, ValueError) as exc:
        raise ValueError(f"{module_name}.py must expose a literal decoding_table") from exc
    if not isinstance(table, str) or len(table) != 256:
        raise ValueError(f"{module_name}.py must expose a 256-char decoding_table")
    return table


def _decode_table(module_name: str) -> tuple[int, ...]:
    table = _raw_decoding_table(module_name)
    non_ascii_identity = [
        idx for idx, ch in enumerate(table[:0x80]) if ord(ch) != idx
    ]
    if non_ascii_identity:
        formatted = ", ".join(f"0x{idx:02X}" for idx in non_ascii_identity[:8])
        raise ValueError(
            f"{module_name}.py is not ASCII-compatible in 0x00..0x7F: {formatted}"
        )
    return tuple(0xFFFF if ch == "\ufffe" else ord(ch) for ch in table[0x80:])


def _encoding_map(module_name: str) -> dict[int, int] | None:
    assignments = _repo_encoding_assignments(module_name)
    value = assignments.get("encoding_map")
    if value is None:
        return None
    try:
        mapping = ast.literal_eval(value)
    except ValueError as exc:
        raise ValueError(f"{module_name}.encoding_map must be a literal dict") from exc
    if not isinstance(mapping, dict):
        raise ValueError(f"{module_name}.encoding_map must be a literal dict")
    out: dict[int, int] = {}
    for code, byte in mapping.items():
        if not isinstance(code, int) or not isinstance(byte, int):
            raise ValueError(f"{module_name}.encoding_map must map int to int")
        out[code] = byte
    return out


def _encode_map(module_name: str, decode_table: tuple[int, ...]) -> dict[int, int]:
    entries: dict[int, int] = {}
    explicit = _encoding_map(module_name)
    if explicit is not None:
        entries.update(explicit)
    else:
        for offset, code in enumerate(decode_table):
            if code == 0xFFFF:
                continue
            byte = 0x80 + offset
            entries[code] = byte
    for code in range(0x80):
        entries.setdefault(code, code)
    return entries


def _tables(module_name: str) -> CharmapTables:
    decode_table = _decode_table(module_name)
    encode_map = _encode_map(module_name, decode_table)
    ambiguous_sentinels = [
        code for code, byte in encode_map.items() if code > 0x7F and byte == 0
    ]
    if ambiguous_sentinels:
        formatted = ", ".join(f"U+{code:04X}" for code in ambiguous_sentinels)
        raise ValueError(f"{module_name}.py maps non-ASCII codepoints to NUL: {formatted}")
    high = tuple(encode_map.get(code, 0) for code in range(0x80, 0x100))
    extended = tuple(
        (code << 8) | byte
        for code, byte in sorted(encode_map.items())
        if code > 0xFF
    )
    return CharmapTables(decode_table, high, extended)


def _rust_array(values: tuple[int, ...], *, width: int = 16) -> str:
    lines: list[str] = []
    for idx in range(0, len(values), width):
        chunk = values[idx : idx + width]
        lines.append("    " + ", ".join(f"0x{value:04X}" for value in chunk) + ",")
    return "\n".join(lines)


def _rust_u8_array(values: tuple[int, ...], *, width: int = 16) -> str:
    lines: list[str] = []
    for idx in range(0, len(values), width):
        chunk = values[idx : idx + width]
        lines.append("    " + ", ".join(f"0x{value:02X}" for value in chunk) + ",")
    return "\n".join(lines)


def _rust_u32_array(values: tuple[int, ...], *, width: int = 4) -> str:
    lines: list[str] = []
    for idx in range(0, len(values), width):
        chunk = values[idx : idx + width]
        lines.append("    " + ", ".join(f"0x{value:08X}" for value in chunk) + ",")
    return "\n".join(lines)


def _encoding_search_key(name: str) -> str:
    chars: list[str] = []
    punct = False
    for ch in name:
        if ch.isascii() and (ch.isalnum() or ch == "."):
            if punct and chars:
                chars.append("_")
            chars.append(ch.lower())
            punct = False
        else:
            punct = True
    return "".join(chars)


def _static_aliases() -> dict[str, str]:
    tree = ast.parse(ALIASES.read_text(encoding="utf-8"), filename=str(ALIASES))
    for stmt in tree.body:
        if not isinstance(stmt, ast.Assign):
            continue
        if any(isinstance(target, ast.Name) and target.id == "_STATIC_ALIASES" for target in stmt.targets):
            aliases = ast.literal_eval(stmt.value)
            if not isinstance(aliases, dict):
                raise ValueError("_STATIC_ALIASES must be a literal dict")
            out: dict[str, str] = {}
            for alias_name, module_name in aliases.items():
                if not isinstance(alias_name, str) or not isinstance(module_name, str):
                    raise ValueError("_STATIC_ALIASES must map str to str")
                out[alias_name] = module_name
            return out
    raise ValueError("could not locate _STATIC_ALIASES")


def _alias_entries() -> tuple[tuple[str, str], ...]:
    module_to_kind = {descriptor.module: descriptor.kind for descriptor in CODEC_DESCRIPTORS}
    entries: dict[str, str] = {}
    for descriptor in CODEC_DESCRIPTORS:
        for spelling in (descriptor.canonical_label, descriptor.module):
            key = _encoding_search_key(spelling)
            if key:
                entries[key] = descriptor.kind
    for alias_name, module_name in _static_aliases().items():
        kind = module_to_kind.get(module_name)
        if kind is None:
            continue
        key = _encoding_search_key(alias_name)
        if key:
            entries[key] = kind
    return tuple(sorted(entries.items()))


def _python_alias_entries() -> tuple[tuple[str, str], ...]:
    available_modules = {
        path.stem
        for path in ENCODINGS.glob("*.py")
        if path.name not in {"__init__.py", "aliases.py"}
    }
    entries: dict[str, str] = {}
    for alias_name, module_name in _static_aliases().items():
        if module_name not in available_modules:
            continue
        key = _encoding_search_key(alias_name)
        if key:
            entries[key] = module_name
    return tuple(sorted(entries.items()))


def render_aliases() -> str:
    parts: list[str] = [
        "//! @generated by tools/gen_codecs.py from codec_registry.rs and encodings/aliases.py.",
        "//! DO NOT EDIT.",
        "",
        "use crate::codec_registry::{EncodingAlias, EncodingKind, PythonEncodingAlias};",
        "",
        "const fn alias(alias: &'static str, kind: EncodingKind) -> EncodingAlias {",
        "    EncodingAlias { alias, kind }",
        "}",
        "",
        "const fn python_alias(alias: &'static str, module: &'static str) -> PythonEncodingAlias {",
        "    PythonEncodingAlias { alias, module }",
        "}",
        "",
        "pub const ENCODING_ALIASES: &[EncodingAlias] = &[",
    ]
    for alias_name, kind in _alias_entries():
        parts.append(f'    alias("{alias_name}", EncodingKind::{kind}),')
    parts.extend(
        [
            "];",
            "",
            "pub const PYTHON_ENCODING_ALIASES: &[PythonEncodingAlias] = &[",
        ]
    )
    for alias_name, module_name in _python_alias_entries():
        parts.append(f'    python_alias("{alias_name}", "{module_name}"),')
    parts.extend(
        [
            "];",
            "",
        ]
    )
    return "\n".join(parts)


def render() -> str:
    parts: list[str] = [
        "//! @generated by tools/gen_codecs.py from CPython stdlib encodings decoding_table.",
        "//! DO NOT EDIT.",
        "",
        "use crate::codec_errors::DecodeFailure;",
        "use crate::codec_registry::EncodingKind;",
        "use crate::wtf8::{push_backslash_bytes_vec, push_wtf8_codepoint};",
        "",
        "pub struct SingleByteCharmap {",
        "    decode_table: &'static [u16; 128],",
        "    encode_high_table: &'static [u8; 128],",
        "    encode_extended_entries: &'static [u32],",
        "}",
        "",
        "pub fn encode_single_byte_charmap_byte(table: &SingleByteCharmap, code: u32) -> Option<u8> {",
        "    if code <= 0x7F {",
        "        return Some(code as u8);",
        "    }",
        "    if (0x80..=0xFF).contains(&code) {",
        "        let byte = table.encode_high_table[(code - 0x80) as usize];",
        "        if byte != 0 {",
        "            return Some(byte);",
        "        }",
        "        return None;",
        "    }",
        "    table",
        "        .encode_extended_entries",
        "        .binary_search_by_key(&code, |entry| entry >> 8)",
        "        .ok()",
        "        .map(|idx| table.encode_extended_entries[idx] as u8)",
        "}",
        "",
        "pub fn decode_single_byte_charmap_with_errors(",
        "    bytes: &[u8],",
        "    kind: EncodingKind,",
        "    errors: &str,",
        ") -> Result<Vec<u8>, DecodeFailure> {",
        "    let Some(table) = single_byte_charmap(kind) else {",
        '        unreachable!("single-byte charmap decoder called for non-charmap encoding");',
        "    };",
        "    decode_charmap_with_errors(bytes, errors, table.decode_table)",
        "}",
        "",
        "pub fn single_byte_charmap(kind: EncodingKind) -> Option<&'static SingleByteCharmap> {",
        "    match kind {",
    ]
    for codec in CHARMAP_CODECS:
        parts.append(
            f"        EncodingKind::{codec.kind} => Some(&{codec.rust_name}_CHARMAP),"
        )
    parts.extend(
        [
            "        _ => None,",
            "    }",
            "}",
            "",
            "fn decode_charmap_with_errors(",
            "    bytes: &[u8],",
            "    errors: &str,",
            "    table: &[u16; 128],",
            ") -> Result<Vec<u8>, DecodeFailure> {",
            "    let mut out = Vec::with_capacity(bytes.len());",
            "    for (idx, &byte) in bytes.iter().enumerate() {",
            "        if byte <= 0x7F {",
            "            out.push(byte);",
            "            continue;",
            "        }",
            "        let code = table[(byte - 0x80) as usize];",
            "        if code != 0xFFFF {",
            "            push_wtf8_codepoint(&mut out, code as u32);",
            "            continue;",
            "        }",
            "        match errors {",
            '            "ignore" => {}',
            '            "replace" => push_wtf8_codepoint(&mut out, 0xFFFD),',
            '            "backslashreplace" => push_backslash_bytes_vec(&mut out, &[byte]),',
            '            "surrogateescape" => push_wtf8_codepoint(&mut out, 0xDC00 + byte as u32),',
            '            "strict" | "surrogatepass" => {',
            "                return Err(DecodeFailure::Byte {",
            "                    pos: idx,",
            "                    byte,",
            '                    message: "character maps to <undefined>",',
            "                });",
            "            }",
            "            other => {",
            "                return Err(DecodeFailure::UnknownErrorHandler(other.to_string()));",
            "            }",
            "        }",
            "    }",
            "    Ok(out)",
            "}",
            "",
        ]
    )
    for codec in CHARMAP_CODECS:
        tables = _tables(codec.module)
        parts.extend(
            [
                f"static {codec.rust_name}_CHARMAP: SingleByteCharmap = SingleByteCharmap {{",
                f"    decode_table: &{codec.rust_name}_DECODE_TABLE,",
                f"    encode_high_table: &{codec.rust_name}_ENCODE_HIGH_TABLE,",
                f"    encode_extended_entries: &{codec.rust_name}_ENCODE_EXTENDED_ENTRIES,",
                "};",
                "",
                f"static {codec.rust_name}_DECODE_TABLE: [u16; 128] = [",
                _rust_array(tables.decode_table),
                "];",
                "",
                f"static {codec.rust_name}_ENCODE_HIGH_TABLE: [u8; 128] = [",
                _rust_u8_array(tables.encode_high_table),
                "];",
                "",
                f"static {codec.rust_name}_ENCODE_EXTENDED_ENTRIES: [u32; {len(tables.encode_extended_entries)}] = [",
                _rust_u32_array(tables.encode_extended_entries),
                "];",
                "",
            ]
        )
    return "\n".join(parts).rstrip() + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="fail if generated output is stale")
    args = parser.parse_args()

    outputs = (
        (OUT_RS, render()),
        (OUT_ALIASES_RS, render_aliases()),
    )
    if args.check:
        for path, rendered in outputs:
            try:
                current = path.read_text(encoding="utf-8")
            except FileNotFoundError:
                print(f"missing generated file: {path.relative_to(ROOT)}", file=sys.stderr)
                return 1
            if current != rendered:
                print(
                    f"stale generated file: {path.relative_to(ROOT)}\n"
                    "  run `python tools/gen_codecs.py` to regenerate.",
                    file=sys.stderr,
                )
                return 1
        return 0

    for path, rendered in outputs:
        path.write_text(rendered, encoding="utf-8")
        print(f"generated {path.relative_to(ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
