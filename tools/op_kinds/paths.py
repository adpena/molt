from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from tools import harness_memory_guard  # noqa: E402
from molt.rust_source_scan import mask_rust_comments_and_strings  # noqa: E402

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


# Reader searches begin at file start or immediately after scope/declaration
# punctuation, never inside a whitespace run. Preserve its full attribute span
# while preventing retries from rescanning every suffix of indentation.
_RUST_MODULE_OR_SCOPE_RE = re.compile(
    r"(?P<attrs>(?:(?<!\s)\s*#\[[^\]]*\]\s*)*)"
    r"\b(?:pub(?:\([^)]*\))?\s+)?(?:unsafe\s+)?mod\s+"
    r"(?P<name>r#[A-Za-z_][A-Za-z0-9_]*|[A-Za-z_][A-Za-z0-9_]*)"
    r"\s*(?P<body>[;{])|(?P<scope>[{}])"
)
_RUST_PATH_ATTR_RE = re.compile(r'#\s*\[\s*path\s*=\s*"(?P<path>[^"]+)"\s*\]')
_RUST_TEST_CFG_RE = re.compile(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]")


def _rust_module_file(
    source: Path, module_dir: Path, explicit_dir: Path, attrs: str, name: str
) -> Path:
    explicit_path = _RUST_PATH_ATTR_RE.search(attrs)
    if explicit_path is not None:
        candidate = explicit_dir / explicit_path.group("path")
        if candidate.is_file():
            return candidate
        raise FileNotFoundError(
            f"declared Rust module path does not exist: {candidate} (from {source})"
        )

    filesystem_name = name.removeprefix("r#")
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
        masked = mask_rust_comments_and_strings(text)
        # The shared lexer preserves offsets. Pair braces once so skipping a
        # test module, function, or macro body cannot interpret its contents as
        # production module declarations (nor recurse over it repeatedly).
        ends: dict[int, int] = {}
        stack: list[int] = []
        for brace in re.finditer(r"[{}]", masked):
            if brace.group() == "{":
                stack.append(brace.start())
            elif stack:
                ends[stack.pop()] = brace.end()
            else:
                raise ValueError(f"unmatched Rust closing brace in {source}")
        if stack:
            raise ValueError(f"unclosed Rust scope in {source}")
        excluded: list[tuple[int, int]] = []

        def modules(start: int, end: int, module_dir: Path, explicit_dir: Path) -> None:
            # Searches start at file entry or after '{', ';', or '}'. Keep this
            # cursor contract: the attribute regex must never begin halfway
            # through a whitespace run, or its exact leading span would change.
            cursor = start
            while declaration := _RUST_MODULE_OR_SCOPE_RE.search(masked, cursor, end):
                cursor = declaration.end()
                if declaration.group("scope"):
                    if declaration.group("scope") == "{":
                        cursor = ends[declaration.start()]
                    continue
                name = declaration.group("name")
                attr_start, attr_end = declaration.span("attrs")
                attrs = text[attr_start:attr_end]
                test_only = (
                    _RUST_TEST_CFG_RE.search(masked[attr_start:attr_end]) is not None
                )
                inline = declaration.group("body") == "{"
                if inline:
                    cursor = ends[declaration.end() - 1]
                if test_only:
                    excluded.append((declaration.start(), cursor))
                elif inline:
                    path_attr = _RUST_PATH_ATTR_RE.search(attrs)
                    child_dir = (
                        explicit_dir / path_attr.group("path")
                        if path_attr is not None
                        else module_dir / name.removeprefix("r#")
                    )
                    modules(declaration.end(), cursor - 1, child_dir, child_dir)
                else:
                    visit(
                        _rust_module_file(source, module_dir, explicit_dir, attrs, name)
                    )

        module_dir = (
            source.parent
            if source.name in {"lib.rs", "main.rs", "mod.rs"}
            else source.with_suffix("")
        )
        modules(0, len(text), module_dir, source.parent)
        # Exclude test-only source itself, not merely its external children.
        chunks: list[str] = []
        cursor = 0
        for start, end in sorted(excluded):
            chunks.extend(
                (text[cursor:start], re.sub(r"[^\r\n]", " ", text[start:end]))
            )
            cursor = end
        chunks.append(text[cursor:])
        source_text[source] = "".join(chunks)

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
