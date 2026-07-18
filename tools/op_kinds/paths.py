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
        if candidate.suffix == ".rs":
            split_module = candidate.with_suffix("") / "mod.rs"
            if split_module.exists():
                return split_module
    return TIR_SRC.joinpath(*parts)


def read_rust_module_cluster(root_file: Path) -> str:
    """Read a Rust module root and its extracted production module tree.

    The bounded, sorted traversal is the shared authority for op-kind audits and
    their tests.  Unlike ``os.walk`` it does not silently discard traversal
    errors, which would turn a partial source read into a false drift verdict.
    """

    parts: list[str] = []
    module_dir = (
        root_file.parent if root_file.name == "mod.rs" else root_file.with_suffix("")
    )
    if module_dir.is_dir():
        for child in sorted(module_dir.rglob("*.rs")):
            if child == root_file or child.name == "tests.rs":
                continue
            if "tests" in child.relative_to(module_dir).parts:
                continue
            parts.append(child.read_text(encoding="utf-8"))
    parts.append(root_file.read_text(encoding="utf-8"))
    return "\n".join(parts)


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
    "read_rust_module_cluster",
]
