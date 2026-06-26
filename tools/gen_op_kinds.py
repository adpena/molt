#!/usr/bin/env python3
"""Generate the op-kind registry artifacts from the canonical table.

Single source of truth: ``runtime/molt-tir/src/tir/op_kinds.toml``.

Cross-component op-"kind"-string drift is molt's most prolific silent-miscompile
bug class (see ``docs/design/foundation/25_op_kind_registry.md`` and
``tools/audit_op_kinds.py``). Five components independently keyed on the JSON wire
"kind" vocabulary, each with its own private table. This generator renders that
ONE table into every consumer so the tables can never drift:

  - ``runtime/molt-tir/src/tir/op_kinds_generated.rs`` — the data tables the
    backend's ``kind_to_opcode`` mapper, the ``CopyLowering`` classifier
    (``copy_kind_mints_fresh_owned_ref`` / ``classify_copy_kind`` /
    ``copy_kind_is_explicit_no_heap_move``), the generated ``ALL_OPCODES``
    enum-domain iterator, and the per-OpCode effect oracle
    (``opcode_may_throw`` / ``opcode_is_side_effecting`` /
    ``opcode_effects_table``)
    consume. The effect oracle is rendered as an EXHAUSTIVE match over the
    ``OpCode`` enum (no wildcard arm), so a newly added opcode fails to compile
    until it is given an explicit effect classification in the table — the
    structural kill for the ``matches!``-default-false trap.
  - ``src/molt/frontend/lowering/op_kinds_generated.py`` — the canonical wire
    spellings the frontend emitter (``map_ops_to_json``) uses, plus the
    frontend pre-serialization raising/skip/binop/effect tables consumed by the
    emitter and midend optimizer.

``tests/test_gen_op_kinds.py`` re-renders both files in memory and asserts byte
equality with the checked-in copies, turning any forgotten regeneration into a
test failure (the ``tests/test_gen_intrinsics.py`` pattern).

Usage::

    python3 tools/gen_op_kinds.py            # (re)write the generated files
    python3 tools/gen_op_kinds.py --check    # exit 1 if a generated file is stale
"""

# It also renders binary-image allocation/ownership classifier sets from the
# same OpCode and preserved-Copy ownership facts that feed TIR/refcount passes,
# so diagnostics and analysis capsules do not grow private hand-maintained
# allocation/refcount vocabularies.

from __future__ import annotations

import argparse
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools.op_kinds import render_python as _render_python
from tools.op_kinds import render_rust as _render_rust
from tools.op_kinds import schema as _schema
from tools.op_kinds import validate as _validate
from tools.op_kinds.paths import (
    OUT_PY,
    OUT_RS,
    RUSTFMT_TMP,
    TABLE,
    harness_memory_guard,
)

for _module in (_schema, _validate, _render_rust, _render_python):
    for _name in getattr(_module, '__all__', ()):  # re-export generator authority
        globals()[_name] = getattr(_module, _name)

OpKindTableError = _validate.OpKindTableError

def _sync_facade_hooks() -> None:
    _validate.TABLE = TABLE
    _render_rust.ROOT = ROOT
    _render_rust.RUSTFMT_TMP = RUSTFMT_TMP
    _render_rust.harness_memory_guard = harness_memory_guard

def load_table(table_path: Path | None = None) -> dict:
    _sync_facade_hooks()
    return _validate.load_table(TABLE if table_path is None else table_path)

def _rustfmt_rust_source(source: str) -> str:
    _sync_facade_hooks()
    return _render_rust._rustfmt_rust_source(source)

def render_rs(data: dict) -> str:
    _sync_facade_hooks()
    return _render_rust.render_rs(data)

def render_py(data: dict) -> str:
    return _render_python.render_py(data)

def _check(path: Path, rendered: str) -> bool:
    """Return True if *path* is in sync with *rendered* (prints a diff hint)."""
    if not path.exists():
        print(f"MISSING generated file: {path}", file=sys.stderr)
        return False
    current = path.read_bytes()
    expected = rendered.encode("utf-8")
    if current != expected:
        print(
            f"STALE generated file: {path}\n"
            f"  run `python3 tools/gen_op_kinds.py` to regenerate from "
            f"{TABLE.relative_to(ROOT)}",
            file=sys.stderr,
        )
        return False
    return True

def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if a generated file is stale (CI mode); do not write",
    )
    args = ap.parse_args(argv)

    data = load_table()
    rs = render_rs(data)
    py = render_py(data)

    if args.check:
        ok = _check(OUT_RS, rs)
        ok = _check(OUT_PY, py) and ok
        if ok:
            print("op-kind generated files: in sync")
        return 0 if ok else 1

    OUT_RS.write_text(rs, encoding="utf-8", newline="\n")
    OUT_PY.write_text(py, encoding="utf-8", newline="\n")
    print(f"wrote {OUT_RS.relative_to(ROOT)}")
    print(f"wrote {OUT_PY.relative_to(ROOT)}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
