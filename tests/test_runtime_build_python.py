from __future__ import annotations

import re
import sys
import unicodedata
from pathlib import Path

import pytest

from molt.process_environment import NameRule, ProcessEnvironment

ROOT = Path(__file__).resolve().parents[1]


def test_surrogate_differential_witness_reaches_every_indexing_case():
    """Matching early failures cannot stand in for the claimed Unicode matrix."""
    import os
    from collections import Counter

    from tests.process_guard_common import run_guarded_test_process

    result = run_guarded_test_process(
        [
            sys.executable,
            "-B",
            str(ROOT / "tests/differential/basic/unicode_surrogate_handling.py"),
        ],
        env=ProcessEnvironment(os.environ).updated(
            {"PYTHONIOENCODING": "utf-8:strict"}
        ),
    )
    assert result.returncode == 0, result.stderr
    assert not result.stderr
    lines = result.stdout.splitlines()
    cases = Counter(
        tuple(line.split()[:2])
        for line in lines
        if line.startswith(("short ", "empty ", "long "))
    )
    assert cases == {
        (length, operation): count
        for length in ("short", "empty", "long")
        for operation, count in (("index", 8), ("slice", 11))
    }
    assert lines[-1].startswith("long slice ")


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_python_loader_policy_uses_the_callers_name_rule(rule):
    from molt.python_runtime_admission import _loader_environment

    environment = ProcessEnvironment(
        {
            "PATH": "canonical path",
            "Path": "alias path",
            "LD_LIBRARY_PATH": "Canonical Loader",
            "ld_library_path": "Alias Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
            "PYTHONPATH": "isolated startup ignores this",
            "pythonexecutable": "alias executable",
        },
        rule=rule,
    )
    selected = _loader_environment(environment)
    assert selected.rule is rule
    assert dict(selected) == (
        {
            "PATH": "canonical path",
            "LD_LIBRARY_PATH": "Canonical Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
        }
        if rule is NameRule.EXACT
        else {
            "PATH": "alias path",
            "LD_LIBRARY_PATH": "Alias Loader",
            "DYLD_INSERT_LIBRARIES": "Case-Sensitive File.dylib",
            "PYTHONEXECUTABLE": "alias executable",
        }
    )


def _emitted_unicode_tables(source: str, target: tuple[int, int]):
    """Decode emitted Rust constants independently of the source-table generator."""
    import ast
    import struct

    major, minor = target

    def array(name):
        raw = re.search(rf"const {name}: [^=]+ = &\[(.*?)\];", source, re.S)[1]
        return ast.literal_eval("[" + raw + "]")

    records = []
    for body in re.findall(r"^    Record \{ (.*?) \},$", source, re.M):
        row = {}
        for name, value in re.findall(r"(\w+): (\[[^]]*\]|-?\d+)", body):
            row[name] = ast.literal_eval(value)
        records.append(row)
    cases = [
        tuple(ast.literal_eval(deltas)[: int(length)])
        for deltas, length in re.findall(
            r"CaseSequence \{ deltas: (\[[^]]*\]), len: (\d+) \}", source
        )
    ]
    numeric = [
        struct.unpack(">d", bytes.fromhex(bits))[0]
        for bits in re.findall(r"f64::from_bits\(0x([0-9a-f]{16})\)", source)
    ]
    version, raw = re.search(
        rf'static DATA_{major}_{minor}: UnicodeData = UnicodeData \{{ version: "([^"]+)", index: &\[(.*?)\] \}};',
        source,
        re.S,
    ).groups()
    index = ast.literal_eval("[" + raw + "]")
    shift = int(re.search(r"const SHIFT: usize = (\d+)", source)[1])
    return (
        version,
        shift,
        index,
        array("PAGES"),
        records,
        cases,
        numeric,
        array("CATEGORIES"),
        array("BIDIRECTIONAL"),
        array("WIDTHS"),
    )


def test_unicode_facts_match_every_running_cpython_codepoint():
    """The public interpreter is the oracle, never the generator's C parser."""
    source = (ROOT / "runtime/molt-obj-model/src/unicode_generated.rs").read_text(
        encoding="utf-8"
    )
    version, shift, index, pages, records, cases, numbers, categories, bidis, widths = (
        _emitted_unicode_tables(source, sys.version_info[:2])
    )
    assert version == unicodedata.unidata_version
    assert len(index) << shift == 0x110000
    assert len(pages) % (1 << shift) == 0
    for code in range(0x110000):
        row = records[
            pages[(index[code >> shift] << shift) + (code & ((1 << shift) - 1))]
        ]
        ch = chr(code)
        flags = row["flags"]
        actual = tuple(
            bool(flags & mask)
            for mask in (1, 2, 4, 8, 16, 32, 64, 128, 512, 1024, 2048)
        )
        expected = (
            ch.isalpha(),
            ch.isdecimal(),
            ch.isdigit(),
            ch.islower(),
            ch.splitlines() == [""],
            ch.isspace(),
            unicodedata.category(ch) == "Lt",
            ch.isupper(),
            ("a" + ch).isidentifier(),
            ch.isprintable(),
            ch.isnumeric(),
        )
        assert actual == expected, (hex(code), "predicates", actual, expected)
        assert (ch == "_" or bool(flags & 256)) == ch.isidentifier(), (
            hex(code),
            "identifier",
        )
        cased = ch.islower() or ch.isupper() or unicodedata.category(ch) == "Lt"
        assert bool(flags & 8192) == cased, (hex(code), "cased")
        # Final sigma exposes Case_Ignorable through public contextual behavior,
        # including characters that are simultaneously cased and ignorable.
        ignored = (
            ("." + ch + "Σ").lower()[-1] != "ς"
            if cased
            else ("A" + ch + "Σ").lower()[-1] == "ς"
        )
        assert bool(flags & 4096) == ignored, (hex(code), "case-ignorable")
        for mapping, expected in zip(
            row["case"],
            (ch.lower(), ch.upper(), ch.title(), ch.casefold()),
            strict=True,
        ):
            assert "".join(chr(code + delta) for delta in cases[mapping]) == expected, (
                hex(code),
                "case",
            )
        actual = (
            row["decimal"],
            row["digit"],
            widths[row["width"]],
            categories[row["category"]],
            bidis[row["bidi"]],
            row["combining"],
            row["mirrored"],
            numbers[row["numeric"]].hex() if flags & 2048 else None,
        )
        expected = (
            unicodedata.decimal(ch, -1),
            unicodedata.digit(ch, -1),
            unicodedata.east_asian_width(ch),
            unicodedata.category(ch),
            unicodedata.bidirectional(ch),
            unicodedata.combining(ch),
            unicodedata.mirrored(ch),
            float(unicodedata.numeric(ch)).hex() if ch.isnumeric() else None,
        )
        assert actual == expected, (hex(code), "basic properties", actual, expected)


def test_unicode_generation_consumes_only_pinned_sources(monkeypatch):
    from tools import gen_unicode_width

    # Poison former build-host selection. The complete output must still be the
    # catalog-derived byte sequence, with no interpreter resolution or spawn.
    monkeypatch.setenv("MOLT_BUILD_PYTHON", "/nonexistent/unicode-frontend")
    monkeypatch.setenv("PYTHON", "/nonexistent/other-frontend")
    assert gen_unicode_width.generated_outputs() == {
        gen_unicode_width.OUTPUT: gen_unicode_width.OUTPUT.read_text(encoding="utf-8")
    }


def test_unicode_generation_refuses_unadmitted_target_or_changed_record_layout():
    from dataclasses import replace
    from tools import gen_unicode_width

    snapshots = gen_unicode_width.sources.pinned_snapshots(("3.12",))
    with pytest.raises(ValueError, match="exactly every pinned source cohort"):
        gen_unicode_width.render(snapshots=snapshots.values())
    snapshot = snapshots["3.12"]
    changed = replace(
        snapshot,
        inputs=tuple(
            replace(
                item, text=item.text.replace("const int upper;", "const int changed;")
            )
            if item.role == "unicode_type_implementation"
            else item
            for item in snapshot.inputs
        ),
    )
    with pytest.raises(ValueError, match="type record changed"):
        next(gen_unicode_width._records(changed))


@pytest.mark.parametrize(
    "role, suffix, message",
    [
        ("unicode_type_table", "\n#define SHIFT 7\n", "exactly one SHIFT"),
        ("unicode_database_table", "\n#define SHIFT 7\n", "exactly one SHIFT"),
        ("unicode_generator", "\nLINEBREAK_MASK = 0x40\n", "whitespace flags"),
        ("unicode_generator", "\nSPACE_MASK = 0x40\n", "whitespace flags"),
    ],
)
def test_unicode_generation_refuses_ambiguous_source_layout(role, suffix, message):
    from dataclasses import replace
    from tools import gen_unicode_width

    snapshot = gen_unicode_width.sources.pinned_snapshots(("3.12",))["3.12"]
    changed = replace(
        snapshot,
        inputs=tuple(
            replace(item, text=item.text + suffix) if item.role == role else item
            for item in snapshot.inputs
        ),
    )
    with pytest.raises(ValueError, match=message):
        next(gen_unicode_width._records(changed))


def test_unicode_ascii_fact_invariant_rejects_target_drift():
    from tools import gen_unicode_width

    # Distinct record IDs model a semantic table change, not a target list.
    gen_unicode_width._assert_ascii_invariant({"first": [0] * 128, "second": [0] * 128})
    changed = [0] * 128
    changed[65] = 1
    with pytest.raises(ValueError, match="ASCII records disagree"):
        gen_unicode_width._assert_ascii_invariant(
            {"first": [0] * 128, "second": changed}
        )


def test_unicode_source_projection_preserves_full_coordinates(monkeypatch):
    from dataclasses import replace
    from tools import gen_unicode_width

    # Synthetic coordinates test projection identity, not guest admission.
    snapshot = gen_unicode_width.sources.pinned_snapshots(("3.12",))["3.12"]
    snapshots = [replace(snapshot, python=version) for version in ("3.0", "4.0")]
    monkeypatch.setattr(
        gen_unicode_width.sources,
        "load_cpython_sources",
        lambda _path: {"3.0": None, "4.0": None},
    )
    monkeypatch.setattr(gen_unicode_width, "_MAX_CODE", 128)
    row = (0, -1, -1, "N", (0,), (0,), (0,), (0,), "Cn", "", 0, 0, None)
    monkeypatch.setattr(
        gen_unicode_width, "_records", lambda _snapshot: iter([row] * 128)
    )
    output = gen_unicode_width.render(snapshots=snapshots)
    assert "(3, 0) => Some(&DATA_3_0)" in output
    assert "(4, 0) => Some(&DATA_4_0)" in output
    assert "match target" in output
    with pytest.raises(ValueError, match="exactly every pinned source cohort"):
        gen_unicode_width.render(snapshots=snapshots[:1])
