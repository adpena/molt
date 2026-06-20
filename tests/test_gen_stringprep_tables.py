"""Freshness guard for the Rust-native StringPrep generated tables."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
GEN = ROOT / "tools" / "gen_stringprep_tables.py"
OUT = ROOT / "runtime/molt-runtime-stringprep/src/tables.rs"


def _load_generator():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_stringprep_tables", GEN
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["molt_test_gen_stringprep_tables"] = module
    spec.loader.exec_module(module)
    return module


def test_generated_stringprep_tables_are_in_sync() -> None:
    gen = _load_generator()
    rendered = gen._rustfmt_text(gen.render())
    checked_in = OUT.read_text(encoding="utf-8")

    assert checked_in == rendered, (
        f"{OUT.relative_to(ROOT)} is stale; run "
        "`python3 tools/gen_stringprep_tables.py`."
    )
