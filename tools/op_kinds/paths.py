from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402

TABLE = ROOT / "runtime/molt-tir/src/tir/op_kinds.toml"
OUT_RS = ROOT / "runtime/molt-tir/src/tir/op_kinds_generated.rs"
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"
RUSTFMT_TMP = ROOT / "tmp" / "gen_op_kinds"

__all__ = [
    "ROOT",
    "TABLE",
    "OUT_RS",
    "OUT_PY",
    "RUSTFMT_TMP",
    "harness_memory_guard",
]
