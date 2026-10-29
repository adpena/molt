"""The typed exception identity equals runtime/exception_kinds.toml (HF-207)."""

from __future__ import annotations

import importlib
import re
import tomllib
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_exception_kinds")


def _enum_variants(source: str, name: str) -> list[tuple[str, int]]:
    body = source.split(f"pub enum {name} {{", 1)[1].split("\n}", 1)[0]
    return [
        (match.group(1), int(match.group(2)))
        for match in re.finditer(r"^\s*(\w+) = (\d+),$", body, flags=re.MULTILINE)
    ]


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.generated_outputs().items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run "
            "`python tools/gen_exception_kinds.py --write`"
        )


def test_closed_types_equal_the_table() -> None:
    """Independent of the renderer: parse the TOML and the Rust enums."""
    raw = tomllib.loads(
        (ROOT / "runtime" / "exception_kinds.toml").read_text(encoding="utf-8")
    )
    source = (
        ROOT / "runtime/molt-obj-model/src/exception_kinds_generated.rs"
    ).read_text(encoding="utf-8")
    builtin_rows = [row.get("variant", row["name"]) for row in raw["builtin"]]
    module_rows = [row["variant"] for row in raw["module"]]
    builtins = _enum_variants(source, "BuiltinExc")
    modules = _enum_variants(source, "ModuleExc")
    assert [variant for variant, _ in builtins] == builtin_rows
    assert [variant for variant, _ in modules] == module_rows
    codes = [code for _, code in builtins + modules]
    # One dense code space; 0 is the reserved invalid code.
    assert codes == list(range(1, len(codes) + 1))
    assert len(codes) < 1 << 16


def test_owner_members_follow_row_order() -> None:
    raw = tomllib.loads(
        (ROOT / "runtime" / "exception_kinds.toml").read_text(encoding="utf-8")
    )
    source = (
        ROOT / "runtime/molt-obj-model/src/exception_kinds_generated.rs"
    ).read_text(encoding="utf-8")
    gen = _gen()
    for owner in gen.owners(gen.load_table()):
        expected = [row["variant"] for row in raw["module"] if row["owner"] == owner]
        arm = re.search(
            rf"Self::{gen.owner_variant(owner)} => &\[([^\]]*)\]",
            source,
            flags=re.DOTALL,
        )
        assert arm is not None, owner
        members = [
            name.strip().removeprefix("ModuleExc::") for name in arm.group(1).split(",")
        ]
        assert [name for name in members if name] == expected


def test_frontend_codes_match_builtin_discriminants() -> None:
    from molt.frontend.exception_kinds_generated import BUILTIN_EXCEPTION_CODES

    source = (
        ROOT / "runtime/molt-obj-model/src/exception_kinds_generated.rs"
    ).read_text(encoding="utf-8")
    raw = tomllib.loads(
        (ROOT / "runtime" / "exception_kinds.toml").read_text(encoding="utf-8")
    )
    by_variant = dict(_enum_variants(source, "BuiltinExc"))
    expected = {
        row["name"]: by_variant[row.get("variant", row["name"])]
        for row in raw["builtin"]
        if row.get("name_lookup", True)
    }
    assert BUILTIN_EXCEPTION_CODES == expected


def test_schema_rejects_bad_rows(tmp_path: Path) -> None:
    gen = _gen()
    cases = {
        "unknown base": '[[builtin]]\nname = "BaseException"\nbases = []\n'
        '[[builtin]]\nname = "X"\nbases = ["Missing"]\n',
        "duplicate variant": '[[builtin]]\nname = "BaseException"\nbases = []\n'
        '[[builtin]]\nname = "BaseException"\nvariant = "BaseException"\nbases = ["BaseException"]\n',
        "module base": '[[builtin]]\nname = "BaseException"\nbases = []\n'
        '[[module]]\nowner = "m"\nname = "E"\nvariant = "ME"\nbase = "Nope"\n',
        "foreign binding": '[[builtin]]\nname = "BaseException"\nbases = []\n'
        '[[builtin]]\nname = "E"\nbases = ["BaseException"]\nmodule = "m"\nbuiltin_binding = true\n',
    }
    for label, body in cases.items():
        path = tmp_path / f"{label.replace(' ', '_')}.toml"
        path.write_text("schema_version = 1\n" + body, encoding="utf-8")
        with pytest.raises(gen.ExceptionKindTableError):
            gen.load_table(path)


def test_frontend_names_are_the_builtins_binding_superset() -> None:
    """Target-gated spellings stay in the superset; module classes never do."""
    from molt.frontend.exception_kinds_generated import BUILTIN_EXCEPTION_NAMES

    for name in ("WindowsError", "PythonFinalizationError", "IOError", "OSError"):
        assert name in BUILTIN_EXCEPTION_NAMES
    for name in ("CancelledError", "UnsupportedOperation", "StatisticsError", "Error"):
        assert name not in BUILTIN_EXCEPTION_NAMES


def test_aliases_must_name_a_canonical_row(tmp_path: Path) -> None:
    gen = _gen()
    path = tmp_path / "alias.toml"
    path.write_text(
        "schema_version = 1\n"
        '[[builtin]]\nname = "BaseException"\nbases = []\n'
        '[[alias]]\nname = "IOError"\ncanonical = "OSError"\n',
        encoding="utf-8",
    )
    with pytest.raises(gen.ExceptionKindTableError):
        gen.load_table(path)
