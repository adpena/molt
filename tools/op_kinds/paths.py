from __future__ import annotations

import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402

TIR_SRC_CANDIDATES = (
    ROOT / "runtime/molt-ir/src/tir",
    ROOT / "runtime/molt-passes/src/tir",
    ROOT / "runtime/molt-passes/src/tir/passes",
    ROOT / "runtime/molt-tir/src/tir",
)
TIR_SRC = next(
    (path for path in TIR_SRC_CANDIDATES if path.exists()), TIR_SRC_CANDIDATES[0]
)


def tir_path(relative: str) -> Path:
    parts = Path(relative).parts
    for base in TIR_SRC_CANDIDATES:
        candidate = base.joinpath(*parts)
        if candidate.exists():
            return candidate
    return TIR_SRC.joinpath(*parts)


TABLE = tir_path("op_kinds.toml")
OUT_RS = tir_path("op_kinds_generated.rs")
OUT_PY = ROOT / "src/molt/frontend/lowering/op_kinds_generated.py"

__all__ = [
    "ROOT",
    "TIR_SRC_CANDIDATES",
    "TIR_SRC",
    "tir_path",
    "TABLE",
    "OUT_RS",
    "OUT_PY",
    "harness_memory_guard",
]
