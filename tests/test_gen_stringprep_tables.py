"""Freshness guard for the Rust-native StringPrep generated tables."""

from __future__ import annotations

from pathlib import Path

from tools import gen_stringprep_tables


ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "runtime/molt-runtime-stringprep/src/tables.rs"


def test_generated_stringprep_tables_are_in_sync() -> None:
    gen = gen_stringprep_tables
    rendered = gen.generated_outputs()[OUT]
    checked_in = OUT.read_text(encoding="utf-8")

    assert checked_in == rendered, (
        f"{OUT.relative_to(ROOT)} is stale; run "
        "`python3 tools/gen_stringprep_tables.py --write`."
    )
