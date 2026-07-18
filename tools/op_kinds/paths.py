from __future__ import annotations

import re
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


_RUST_FILE_MODULE_RE = re.compile(
    r"(?m)^(?P<attrs>(?:\s*#\[[^\n]+\]\s*\n)*)"
    r"\s*(?:pub(?:\([^\n)]*\))?\s+)?(?:unsafe\s+)?mod\s+"
    r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)\s*;"
)
_RUST_PATH_ATTR_RE = re.compile(r'#\s*\[\s*path\s*=\s*"(?P<path>[^"]+)"\s*\]')


def _rust_module_file(source: Path, attrs: str, name: str) -> Path:
    explicit_path = _RUST_PATH_ATTR_RE.search(attrs)
    if explicit_path is not None:
        candidate = source.parent / explicit_path.group("path")
        if candidate.is_file():
            return candidate
        raise FileNotFoundError(
            f"declared Rust module path does not exist: {candidate} (from {source})"
        )

    filesystem_name = name.removeprefix("r#")
    module_dir = (
        source.parent
        if source.name in {"lib.rs", "main.rs", "mod.rs"}
        else source.with_suffix("")
    )
    candidates = (
        module_dir / f"{filesystem_name}.rs",
        module_dir / filesystem_name / "mod.rs",
    )
    matches = tuple(candidate for candidate in candidates if candidate.is_file())
    if len(matches) == 1:
        return matches[0]
    if not matches:
        raise FileNotFoundError(
            f"declared Rust module {name!r} has no source file from {source}"
        )
    raise RuntimeError(
        f"declared Rust module {name!r} has ambiguous source files: {matches}"
    )


def read_rust_module_cluster(root_file: Path) -> str:
    """Read exactly the production file-module graph rooted at ``root_file``.

    Rust ``mod name;`` declarations, rather than a filesystem walk, own the
    cluster boundary.  This avoids both stale undeclared siblings and the
    silent partial results produced when glob/walk implementations suppress a
    directory-scan error.  Missing or ambiguous declared modules fail closed.
    Test-only modules are deliberately outside the production authority.
    """

    visited: set[Path] = set()
    source_text: dict[Path, str] = {}

    def visit(source: Path) -> None:
        source = source.resolve(strict=True)
        if source in visited:
            return
        visited.add(source)
        text = source.read_text(encoding="utf-8")
        source_text[source] = text
        for declaration in _RUST_FILE_MODULE_RE.finditer(text):
            attrs = declaration.group("attrs")
            if re.search(r"\bcfg\s*\([^\n)]*\btest\b", attrs):
                continue
            child = _rust_module_file(source, attrs, declaration.group("name"))
            visit(child)

    visit(root_file)
    root = root_file.resolve(strict=True)
    ordered = sorted(visited - {root}, key=lambda path: path.as_posix())
    ordered.append(root)
    return "\n".join(source_text[source] for source in ordered)


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
