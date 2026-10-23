#!/usr/bin/env python3
"""Generate shared target Unicode properties and case facts from pinned CPython source.

No target interpreter or build-host Unicode database participates in generation.
CPython's two-level split table is retained, with records, pages and case sequences
interned across every admitted target version. Reference Python is only an oracle.
"""

from __future__ import annotations

import ast
from array import array
import json
from pathlib import Path
import re
import struct
import sys

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))
from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)
from tools import gen_stdlib_module_union as sources  # noqa: E402
from tools.generator_io import generator_main  # noqa: E402

ROOT = _REPO_ROOT
OUTPUT = ROOT / "runtime/molt-obj-model/src/unicode_generated.rs"
_MAX_CODE = 0x110000
_MASKS = {
    "ALPHA": 1,
    "DECIMAL": 2,
    "DIGIT": 4,
    "LOWER": 8,
    "TITLE": 64,
    "UPPER": 128,
    "XID_START": 256,
    "XID_CONTINUE": 512,
    "PRINTABLE": 1024,
    "NUMERIC": 2048,
    "CASE_IGNORABLE": 4096,
    "CASED": 8192,
    "EXTENDED_CASE": 16384,
}
_DB_FIELDS = (
    "category",
    "combining",
    "bidirectional",
    "mirrored",
    "east_asian_width",
    "normalization_quick_check",
)
_DB_EXTRA = ("grapheme_cluster_break", "incb", "ext_pict")


def _body(text: str, name: str) -> str:
    matches = re.findall(r"\b" + re.escape(name) + r"\[\]\s*=\s*\{(.*?)\};", text, re.S)
    if len(matches) != 1:
        raise ValueError(f"expected one CPython Unicode array {name}")
    return matches[0]


def _numbers(text: str, name: str) -> tuple[int, ...]:
    raw = _body(text, name)
    if re.fullmatch(r"[\s\d,\-]*", raw) is None:
        raise ValueError(f"unsupported integer array {name}")
    return tuple(int(item) for item in raw.split(",") if item.strip())


def _rows(text: str, name: str, size: int) -> tuple[tuple[int, ...], ...]:
    raw = _body(text, name)
    chunks = re.findall(r"\{([^{}]*)\}", raw)
    if re.fullmatch(r"[\s,]*", re.sub(r"\{[^{}]*\}", "", raw)) is None:
        raise ValueError(f"unsupported record table {name}")
    rows = tuple(
        tuple(int(item) for item in row.split(",") if item.strip()) for row in chunks
    )
    if not rows or any(len(row) != size for row in rows):
        raise ValueError(f"invalid record width {name}")
    return rows


def _strings(text: str, name: str) -> tuple[str, ...]:
    raw = _body(text, name)
    tokens = re.findall(r'"[^"\\]*"', raw)
    if re.fullmatch(r"[\s,]*(?:NULL[\s,]*)?", re.sub(r'"[^"\\]*"', "", raw)) is None:
        raise ValueError(f"unsupported string table {name}")
    return tuple(ast.literal_eval(token) for token in tokens)


def _numeric(text: str) -> dict[int, float]:
    raw = text.split("double _PyUnicode_ToNumeric(", 1)[1].split(
        "int _PyUnicode_IsWhitespace", 1
    )[0]
    raw = raw.split("switch (ch) {", 1)[1].split("}", 1)[0]
    pattern = r"((?:\s*case 0x[0-9A-F]+:)+)\s*return \(double\) ([\-+\deE.]+)(?:/([\-+\deE.]+))?;"
    values = {}
    for cases, numerator, denominator in re.findall(pattern, raw):
        value = float(numerator) / (float(denominator) if denominator else 1)
        for spelling in re.findall(r"0x([0-9A-F]+)", cases):
            code = int(spelling, 16)
            if code in values:
                raise ValueError("duplicate numeric code point")
            values[code] = value
    if re.sub(pattern, "", raw).strip():
        raise ValueError("unsupported CPython numeric expression")
    return values


def _records(snapshot: sources.SourceSnapshot):
    evidence = {item.role: item.text for item in snapshot.inputs}
    td, db = evidence["unicode_type_table"], evidence["unicode_database_table"]
    implementation = evidence["unicode_type_implementation"]
    masks = {
        name: int(value, 16)
        for name, value in re.findall(
            r"#define (\w+)_MASK (0x[0-9A-Fa-f]+)", implementation
        )
    }
    if masks != _MASKS:
        raise ValueError("CPython Unicode type flags changed")
    generator_mask_rows = re.findall(
        r"^(LINEBREAK|SPACE)_MASK = (0x[0-9A-Fa-f]+)$",
        evidence["unicode_generator"],
        re.M,
    )
    generator_masks = {name: int(value, 16) for name, value in generator_mask_rows}
    if len(generator_mask_rows) != 2 or generator_masks != {
        "LINEBREAK": 0x10,
        "SPACE": 0x20,
    }:
        raise ValueError("CPython Unicode generator whitespace flags changed")
    db_impl = re.sub(
        r"/\*.*?\*/", "", evidence["unicode_database_implementation"], flags=re.S
    )
    declaration = re.findall(
        r"typedef struct\s*\{([^{}]*)\}\s*_PyUnicode_DatabaseRecord;", db_impl, re.S
    )
    if len(declaration) != 1:
        raise ValueError("CPython Unicode database record is missing")
    fields = tuple(re.findall(r"const unsigned char (\w+);", declaration[0]))
    if fields not in (_DB_FIELDS, _DB_FIELDS + _DB_EXTRA):
        raise ValueError("CPython Unicode database record changed")
    type_declaration = re.findall(
        r"typedef struct\s*\{([^{}]*)\}\s*_PyUnicode_TypeRecord;",
        re.sub(r"/\*.*?\*/", "", implementation, flags=re.S),
        re.S,
    )
    if len(type_declaration) != 1 or tuple(
        re.findall(
            r"const (?:int|unsigned char|unsigned short) (\w+);", type_declaration[0]
        )
    ) != ("upper", "lower", "title", "decimal", "digit", "flags"):
        raise ValueError("CPython Unicode type record changed")
    version = re.search(r'#define UNIDATA_VERSION "([^"\n]+)"', db)[1]
    if (
        re.search(
            r'^UNIDATA_VERSION = "([^"\n]+)"', evidence["unicode_generator"], re.M
        )[1]
        != version
    ):
        raise ValueError("CPython Unicode generator/database versions disagree")
    types = _rows(td, "_PyUnicode_TypeRecords", 6)
    records = _rows(db, "_PyUnicode_Database_Records", len(fields))
    extended = _numbers(td, "_PyUnicode_ExtendedCase")
    t1, t2 = _numbers(td, "index1"), _numbers(td, "index2")
    d1, d2 = _numbers(db, "index1"), _numbers(db, "index2")

    def shift(source):
        values = re.findall(r"^#define SHIFT (\d+)$", source, re.M)
        if len(values) != 1:
            raise ValueError("CPython Unicode table must define exactly one SHIFT")
        return int(values[0])

    ts, ds = shift(td), shift(db)
    if not 0 < ts < 16 or not 0 < ds < 16:
        raise ValueError("unsupported Unicode table shift")
    categories = _strings(db, "_PyUnicode_CategoryNames")
    bidis = _strings(db, "_PyUnicode_BidirectionalNames")
    widths = _strings(db, "_PyUnicode_EastAsianWidthNames")
    numeric = _numeric(td)
    for code in range(_MAX_CODE):
        upper, lower, title, decimal, digit, flags = types[
            t2[(t1[code >> ts] << ts) + (code & ((1 << ts) - 1))]
        ]
        cat, comb, bidi, mirrored, width = records[
            d2[(d1[code >> ds] << ds) + (code & ((1 << ds) - 1))]
        ][:5]
        mappings = []
        for value in (lower, upper, title):
            mappings.append(
                tuple(
                    c - code
                    for c in extended[value & 65535 : (value & 65535) + (value >> 24)]
                )
                if flags & 16384
                else (value,)
            )
        if flags & 16384 and (lower >> 20) & 7:
            pos = (lower & 65535) + (lower >> 24)
            mappings.append(
                tuple(c - code for c in extended[pos : pos + ((lower >> 20) & 7)])
            )
        else:
            mappings.append(mappings[0])
        if not all(
            1 <= len(mapping) <= 3
            and all(0 <= code + delta < _MAX_CODE for delta in mapping)
            for mapping in mappings
        ):
            raise ValueError("invalid full case mapping")
        if bool(flags & 2048) != (code in numeric):
            raise ValueError("numeric values disagree with numeric flags")
        yield (
            flags & ~16384,
            decimal if flags & 2 else -1,
            digit if flags & 4 else -1,
            widths[width],
            *mappings,
            categories[cat],
            bidis[bidi],
            comb,
            mirrored,
            numeric.get(code),
        )


def _intern(values, lookup, value):
    index = lookup.get(value)
    if index is None:
        index = len(values)
        lookup[value] = index
        values.append(value)
    return index


def _assert_ascii_invariant(versions):
    rows = iter(versions.values())
    expected = next(rows)[:128]
    if any(row[:128] != expected for row in rows):
        raise ValueError("target Unicode ASCII records disagree")


def render(*, snapshots=None) -> str:
    versions_to_emit = tuple(sources.load_cpython_sources(sources.PINS_PATH))
    if snapshots is None:
        snapshots = sources.pinned_snapshots(versions_to_emit).values()
    selected = {snapshot.python: snapshot for snapshot in snapshots}
    if set(selected) != set(versions_to_emit):
        raise ValueError(
            "Unicode facts require exactly every pinned source cohort member"
        )
    records, record_ids, versions = [], {}, {}
    for version in versions_to_emit:
        versions[version] = array(
            "H",
            (_intern(records, record_ids, row) for row in _records(selected[version])),
        )
    _assert_ascii_invariant(versions)
    # Measure every supported split, then choose deterministically by byte size.
    candidates = []
    for shift in range(4, 11):
        pages = {
            tuple(ids[start : start + (1 << shift)])
            for ids in versions.values()
            for start in range(0, len(ids), 1 << shift)
        }
        candidates.append(
            (
                len(versions) * (_MAX_CODE >> shift) * 2
                + len(pages) * (1 << shift) * 2,
                shift,
            )
        )
    shift = min(candidates)[1]
    pages, page_ids, indexes = [], {}, {}
    for version, ids in versions.items():
        indexes[version] = [
            _intern(pages, page_ids, tuple(ids[start : start + (1 << shift)]))
            for start in range(0, len(ids), 1 << shift)
        ]
    cases, case_ids, numeric, numeric_ids = [], {}, [], {}
    cats = sorted({row[8] for row in records})
    bidis = sorted({row[9] for row in records})
    widths = sorted({row[3] for row in records})
    record_lines = []
    for (
        flags,
        decimal,
        digit,
        width,
        lower,
        upper,
        title,
        fold,
        cat,
        bidi,
        comb,
        mirrored,
        value,
    ) in records:
        case = [
            _intern(cases, case_ids, mapping) for mapping in (lower, upper, title, fold)
        ]
        number = _intern(
            numeric, numeric_ids, struct.pack(">d", 0.0 if value is None else value)
        )
        record_lines.append(
            f"    Record {{ flags: {flags}, case: {case}, numeric: {number}, decimal: {decimal}, digit: {digit}, category: {cats.index(cat)}, bidi: {bidis.index(bidi)}, combining: {comb}, mirrored: {mirrored}, width: {widths.index(width)} }},"
        )
    if max(len(records), len(cases), len(pages), len(numeric)) > 65536:
        raise ValueError("Unicode facts exceed the u16 index representation")
    lines = [
        "// Generated by tools/gen_unicode_width.py from pinned CPython source; do not edit.",
        "// Third-party notices: src/molt/stdlib/collections/LICENSE.cpython.",
        "use super::{CaseSequence, Record, UnicodeData};",
        f"pub(super) const SHIFT: usize = {shift};",
    ]
    for version, snapshot in selected.items():
        lines.append(f"// CPython {snapshot.tag} ({snapshot.revision})")
        for item in snapshot.inputs:
            if item.role.startswith("unicode_"):
                lines.append(f"// {item.path} sha256={item.sha256()}")
    for name, strings in [
        ("CATEGORIES", cats),
        ("BIDIRECTIONAL", bidis),
        ("WIDTHS", widths),
    ]:
        lines.append(
            f"pub(super) const {name}: &[&str] = &" + json.dumps(strings) + ";"
        )
    lines += [
        "pub(super) const RECORDS: &[Record] = &[",
        *record_lines,
        "];",
        "pub(super) const CASE: &[CaseSequence] = &[",
    ]
    for case in cases:
        lines.append(
            f"    CaseSequence {{ deltas: {list(case) + (3 - len(case)) * [0]}, len: {len(case)} }},"
        )
    lines += ["];", "pub(super) const NUMERIC: &[f64] = &["]
    lines += [f"    f64::from_bits(0x{value.hex()})," for value in numeric]
    lines += ["];", "pub(super) const PAGES: &[u16] = &["]
    for page in pages:
        for start in range(0, len(page), 16):
            lines.append("    " + ", ".join(map(str, page[start : start + 16])) + ",")
    lines += ["];"]
    for version, index in indexes.items():
        name = version.replace(".", "_")
        db = next(
            item.text
            for item in selected[version].inputs
            if item.role == "unicode_database_table"
        )
        unicode_version = re.search(r'#define UNIDATA_VERSION "([^"\n]+)"', db)[1]
        lines.append(
            f'static DATA_{name}: UnicodeData = UnicodeData {{ version: "{unicode_version}", index: &['
        )
        for start in range(0, len(index), 16):
            lines.append("    " + ", ".join(map(str, index[start : start + 16])) + ",")
        lines.append("] };")
    lines += [
        "pub(super) fn for_version(target: (i64, i64)) -> Option<&'static UnicodeData> {",
        "    match target {",
    ]
    for version in versions:
        lines.append(
            f"        ({', '.join(version.split('.'))}) => Some(&DATA_{version.replace('.', '_')}),"
        )
    lines += ["        _ => None,", "    }", "}", ""]
    return "\n".join(lines)


def generated_outputs() -> dict[Path, str]:
    return {OUTPUT: render()}


def main(argv=None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
