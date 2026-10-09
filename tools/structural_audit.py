#!/usr/bin/env python3
"""Whole-tree structural audit — the ranked cleanup board + a fail-loud ratchet.

The op-kind registry (``op_kinds.toml`` → ``tools/gen_op_kinds.py``) proved the
thesis: *repeated semantics belong in one generated table, not hand-maintained
across passes*. Its effect oracle is an EXHAUSTIVE Rust ``match`` (no wildcard),
so a new opcode that forgets a row fails to COMPILE — drift is impossible there.

This tool finds the places that have NOT yet reached that bar — where a semantic
property is still decided by a hand-written list with a silent default, where a
file has grown into a god-object, where multiple large top-level regions make a
file a structural god-file, where workaround/debt markers accumulate, and where
two authorities classify the same thing. It answers the council's
structural-sweep questions #1 (duplicate semantic authorities), #2 (backend-local
semantic guesses), and #8 (legacy paths now coverable by generated facts) with a
RANKED BOARD, and — critically — a ``--check`` RATCHET so the numbers can only go
down: adding a new hand-maintained semantic fallthrough, growing a god-file past
its ceiling, adding top-level extraction-region pressure, or adding debt markers
fails CI.

This is deliberately NOT a re-check of what the compiler already enforces. The
exhaustive generated tables are rustc-gated; auditing them would false-flag
proven-correct work. The signal lives in the NON-exhaustive remainder.

Modes (mirrors tools/gen_op_kinds.py / tools/audit_op_kinds.py CI convention):
  structural_audit.py                  human-readable ranked board (stdout)
  structural_audit.py --json           machine-readable findings (stdout)
  structural_audit.py --path FILE      path-scoped diagnostic findings/metrics
  structural_audit.py --write-board    regenerate docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md
  structural_audit.py --check          fail (exit 1) if any ratchet metric regressed vs baseline
  structural_audit.py --update-baseline  re-pin tools/structural_audit_baseline.json

Wire into .github/workflows/ci.yml next to gen_op_kinds.py --check.
"""

from __future__ import annotations

import argparse
import ast
import io
import json
import re
import sys
from collections.abc import Callable, Iterator
from contextlib import contextmanager
from dataclasses import dataclass, asdict, field as dataclass_field
from functools import wraps
from threading import local
from pathlib import Path
import tokenize
from typing import TypeVar, cast

_T = TypeVar("_T")

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

ROOT_DEFAULT = bind_repository_imports(__file__)

from molt.rust_source_scan import (  # noqa: E402
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    rust_test_only_source_files,
    project_rust_source,
    prewarm_rust_item_projections,
    scan_memo,
)
from tools import release_criterion_receipt as release_receipt  # noqa: E402
from tools import compatibility_error_protocol as compatibility_errors  # noqa: E402
from tools.structural_audit_rust_admission import (  # noqa: E402
    proven_admitted_wire_domain,
)
from tools.structural_audit_rust_domains import (  # noqa: E402
    BranchProjection,
    proven_branch_projection,
)

BASELINE_PATH_REL = "tools/structural_audit_baseline.json"
BOARD_PATH_REL = "docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md"

# --- scope ----------------------------------------------------------------

# Directory segments that are never source-of-truth: VCS, build outputs,
# vendored trybuild fixtures, virtualenvs, agent worktrees, recovery scratch.
_EXCLUDE_SEGMENTS = {
    ".git",
    "target",
    "target-oswalk-impl",
    "trybuild",
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    ".mypy_cache",
    ".pytest_cache",
    "worktrees",
    ".claude",
}
_EXCLUDE_PREFIXES = (".worktree_recovery_", "wt_")
# memory/recovery holds preserved WIP patches, not live source.
_EXCLUDE_PATH_FRAGMENTS = ("memory/recovery/", "memory/index_snapshots")

# Source roots actually owned by the project.
_SOURCE_ROOTS = ("runtime", "src", "tools")


def _is_excluded(path: Path, root: Path) -> bool:
    try:
        rel = path.relative_to(root)
    except ValueError:
        return True
    parts = rel.parts
    for seg in parts:
        if seg in _EXCLUDE_SEGMENTS:
            return True
        if seg.startswith(_EXCLUDE_PREFIXES):
            return True
    rel_str = rel.as_posix()
    return any(frag in rel_str for frag in _EXCLUDE_PATH_FRAGMENTS)


_GENERATED_FILE_MARKER_RE = re.compile(
    r"(?im)^\s*(?://|#|/\*|\*)\s*(?:@generated\b|.*\bDO NOT EDIT\b)"
)


def _manifest_declared_outputs(root: Path) -> frozenset[str]:
    """Manifest authority for this explicit root and this audit operation.

    Absent/malformed manifests retain the standalone audit's header heuristic;
    the owning generator checker separately validates the manifest itself.
    """

    def read() -> frozenset[str]:
        outputs: set[str] = set()
        manifest = root / "tools" / "generator_manifest.toml"
        try:
            import tomllib

            data = tomllib.loads(manifest.read_text(encoding="utf-8"))
            for row in data.get("generator", []):
                for out in row.get("outputs", []):
                    if isinstance(out, str):
                        outputs.add(out)
            for row in data.get("orphan_generated", []):
                path = row.get("path")
                if isinstance(path, str):
                    outputs.add(path)
        except (OSError, ValueError):
            outputs = set()
        return frozenset(outputs)

    return _run_cached(("manifest_outputs", _resolved(root)), read)


def _is_generated(path: Path, root: Path) -> bool:
    name = path.name
    if name.endswith("_generated.rs") or name.endswith("_generated.py"):
        return True
    if path.as_posix().endswith("intrinsics/generated.rs"):
        return True
    # Authoritative manifest list (doc 59 F1) — a declared generated output is
    # generated even if its @generated header were ever stripped.
    try:
        rel = _resolved(path).relative_to(_resolved(root)).as_posix()
        if rel in _manifest_declared_outputs(root):
            return True
    except (ValueError, OSError):
        pass
    try:
        head = _source_text(path)[:400]
    except OSError:
        return False
    return bool(_GENERATED_FILE_MARKER_RE.search(head))


@dataclass
class _AuditOperation:
    root: Path
    path_scope: frozenset[str] | None
    cache: dict[tuple[object, ...], object] = dataclass_field(default_factory=dict)


_AUDIT_STATE = local()


@contextmanager
def audit_operation(
    root: Path, path_scope: frozenset[str] | None = None
) -> Iterator[None]:
    """Own one synchronous filesystem view and lexical memo in this thread.

    Every explicit operation gets fresh filesystem/manifest state. Nested
    operations restore the outer view, including its source scope; only pure,
    content-keyed lexical results may be shared with that outer operation.
    """
    previous = getattr(_AUDIT_STATE, "operation", None)
    _AUDIT_STATE.operation = _AuditOperation(root.resolve(), path_scope)
    try:
        with scan_memo():
            yield
    finally:
        if previous is None:
            del _AUDIT_STATE.operation
        else:
            _AUDIT_STATE.operation = previous


def _audit_probe(probe: Callable[..., _T]) -> Callable[..., _T]:
    @wraps(probe)
    def scoped(root: Path, *args, **kwargs) -> _T:
        operation = getattr(_AUDIT_STATE, "operation", None)
        if operation is not None and operation.root == root.resolve():
            return probe(root, *args, **kwargs)
        with audit_operation(root):
            return probe(root, *args, **kwargs)

    return scoped


def _source_scope(root: Path) -> frozenset[str] | None:
    operation = getattr(_AUDIT_STATE, "operation", None)
    if operation is not None and operation.root == root.resolve():
        return operation.path_scope
    return None


def _run_cached(key: tuple[object, ...], compute: Callable[[], _T]) -> _T:
    operation = getattr(_AUDIT_STATE, "operation", None)
    if operation is None:
        return compute()
    if key not in operation.cache:
        operation.cache[key] = compute()
    return cast(_T, operation.cache[key])


def _resolved(path: Path) -> Path:
    """The file's resolved path, computed once per run_all pass."""
    return _run_cached(("resolved", path), path.resolve)


def _source_text(path: Path) -> str:
    """The file's text, read once per run_all pass."""
    return _run_cached(
        ("text", path), lambda: path.read_text(errors="replace", encoding="utf-8")
    )


def _walk_pruned_files(base: Path, root: Path, suffixes: tuple[str, ...]) -> list[Path]:
    out: list[Path] = []
    stack = [base]
    while stack:
        current = stack.pop()
        try:
            entries = sorted(current.iterdir(), key=lambda path: path.name)
        except OSError:
            continue
        dirs: list[Path] = []
        for path in entries:
            if _is_excluded(path, root):
                continue
            if path.is_dir() and not path.is_symlink():
                dirs.append(path)
            elif path.is_file() and path.suffix in suffixes:
                out.append(path)
        stack.extend(reversed(dirs))
    return out


def _iter_pruned_files(base: Path, root: Path, suffixes: tuple[str, ...]) -> list[Path]:
    return list(
        _run_cached(
            ("pruned", base, root, suffixes),
            lambda: _walk_pruned_files(base, root, suffixes),
        )
    )


def _iter_source_files(root: Path, suffixes: tuple[str, ...]) -> list[Path]:
    scope = _source_scope(root)
    if scope is not None:
        scoped: list[Path] = []
        for rel_str in sorted(scope):
            path = root / rel_str
            if not path.is_file() or path.suffix not in suffixes:
                continue
            if _is_excluded(path, root):
                continue
            scoped.append(path)
        return scoped

    out: list[Path] = []
    for sub in _SOURCE_ROOTS:
        base = root / sub
        if not base.is_dir():
            continue
        out.extend(_iter_pruned_files(base, root, suffixes))
    return sorted(out, key=lambda path: path.relative_to(root).as_posix())


def _is_project_source_file(root: Path, path: Path) -> bool:
    if not path.is_file():
        return False
    try:
        rel = path.relative_to(root)
    except ValueError:
        return False
    if not rel.parts or rel.parts[0] not in _SOURCE_ROOTS:
        return False
    if path.suffix not in (".rs", ".py"):
        return False
    return not _is_excluded(path, root)


def resolve_path_scope(root: Path, paths: list[Path]) -> frozenset[str]:
    """Resolve a path-scoped diagnostic request into project-source files.

    This is intentionally diagnostic-only. The CI ratchet remains whole-tree;
    path scope exists so a local decomposition can explain which findings are
    attributable to touched files without paying a full-tree scan.
    """

    selected: set[str] = set()
    for raw_path in paths:
        candidate = raw_path if raw_path.is_absolute() else root / raw_path
        try:
            candidate = candidate.resolve()
        except OSError:
            candidate = candidate.absolute()

        if candidate.is_dir():
            for suffix in (".rs", ".py"):
                for path in candidate.rglob(f"*{suffix}"):
                    if _is_project_source_file(root, path):
                        selected.add(path.relative_to(root).as_posix())
            continue

        if candidate.is_file():
            if _is_project_source_file(root, candidate):
                selected.add(candidate.relative_to(root).as_posix())
            continue

        raise ValueError(f"path does not exist: {raw_path}")

    if not selected:
        raise ValueError(
            "path scope selected no project-owned .rs/.py files under runtime/, src/, or tools/"
        )
    return frozenset(sorted(selected))


# --- findings -------------------------------------------------------------

# Severity ranks for board ordering and so --check can weight regressions.
_SEV_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3, "info": 4}


@dataclass
class Finding:
    probe: str
    severity: str
    title: str
    location: str
    detail: str
    suggested_action: str
    class_retired: str = ""
    metric: float = 0.0  # used for ranking within a probe

    def sort_key(self) -> tuple[int, float, str, str, str, str]:
        return (
            _SEV_ORDER.get(self.severity, 9),
            -self.metric,
            self.probe,
            self.location,
            self.title,
            self.detail,
        )


@dataclass(frozen=True)
class SourceRegion:
    kind: str
    name: str
    start_line: int
    end_line: int

    @property
    def span(self) -> int:
        return max(1, self.end_line - self.start_line + 1)


@dataclass(frozen=True)
class DebtMarkerHit:
    line: int
    marker: str


@dataclass(frozen=True)
class ImplementationGapHit:
    line: int
    marker: str


@dataclass(frozen=True)
class LargeSourceFile:
    path: Path
    rel: str
    text: str
    line_count: int
    ceiling: int
    suffix: str


_LARGE_SOURCE_REGION_LINES = 250
_STRUCTURAL_GOD_MIN_LARGE_REGIONS = 3
_STRUCTURAL_GOD_MIXED_KIND_MIN_SCORE = 500
_DECOMPOSITION_PACKAGE_MIN_SIBLINGS = 4
_COHESIVE_DECOMPOSITION_CEILING_FACTOR = 1.5


# --- robust Rust scanning -------------------------------------------------


def _balanced_block(text: str, open_idx: int) -> tuple[int, str]:
    """Return (end_index, block_text) for the brace block starting at open_idx
    (which must index a '{'). end_index points just past the matching '}'."""
    depth = 0
    i = open_idx
    n = len(text)
    while i < n:
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth -= 1
            if depth == 0:
                return i + 1, text[open_idx : i + 1]
        i += 1
    return n, text[open_idx:n]


# A `match` whose scrutinee is opcode/kind-like.
_MATCH_HEAD_RE = re.compile(
    r"\bmatch\s+([^\{]*?)\{",
    re.DOTALL,
)
_OPCODE_ARM_RE = re.compile(r"\bOpCode::[A-Za-z0-9_]+")
_KIND_SCRUTINEE_RE = re.compile(r"\.opcode\b|\.kind\b|_original_kind|opcode\b|kind\b")
_GENERATED_OPCODE_TABLE_SCRUTINEE_RE = re.compile(r"\bopcode_[A-Za-z0-9_]*_table\s*\(")
# matches!(scrutinee, PATTERN) — capture the whole call's argument region.
_MATCHES_MACRO_RE = re.compile(r"\bmatches!\s*\(")

# Pass/file criticality: a fallthrough in an RC/alias/escape/effect/codegen path
# is a latent UAF/miscompile; in a loop/gvn/numeric pass it is merely a missed
# optimization. Weighted so the board surfaces the dangerous ones first.
_CRITICAL_FILE_HINTS = (
    "alias_analysis",
    "escape_analysis",
    "drop_insertion",
    "refcount",
    "effects",
    "exception",
    "ownership",
    "lower_to_lir",
    "function_compiler",
    "llvm_backend",
    "wasm.rs",
    "callable",
    "ic",
    "inline",
)

# A default arm that FAILS LOUD is the correct fail-closed dispatch pattern
# (a new opcode panics, never silently miscompiles) — NOT drift, excluded.
_FAILLOUD_RE = re.compile(
    r"panic!|unimplemented!|unreachable!|todo!|bail!|return\s+Err|Err\s*\(|"
    r"\.expect\s*\(|assert(_eq|_ne)?!|abort\b"
)
# A default that EMITS code (calls into the backend/builder) is a mechanical
# lowering dispatch, not a semantic classification — excluded from the drift
# surface (it cannot encode a wrong *fact*, only route a missing *lowering*,
# and the missing-lowering case is caught by backend_support_audit instead).
_EMITTER_RE = re.compile(
    r"\bself\.|builder|\.build_|emit_|into_(int|float|pointer)_value"
)
# An "optimistic" default token asserts the *absence* of a hazard (no-alias,
# no-escape, precise-type, pure) for an UNKNOWN opcode — the shape that turns a
# new opcode into a silent miscompile. Conservative tokens (true/GlobalEscape/
# Opaque) over-approximate the hazard and are merely imprecise (missed opt).
_OPTIMISTIC_DEFAULT_RE = re.compile(
    r"^\s*(false|None|TransparentAlias|NoEscape|EscapeState::NoEscape|"
    r"DynBox|TirType::DynBox|Pure|Effect::None)\b"
)


def _file_is_critical(path: Path) -> bool:
    s = path.as_posix()
    return any(h in s for h in _CRITICAL_FILE_HINTS)


def _top_level_wildcard_arm_start(block: str) -> int | None:
    """Return the wildcard arm start for this match block, if it has one.

    `block` includes the outer match braces. A nested `match` inside an arm may
    legitimately contain `_ => fallback` for local data decoding; that is not a
    wildcard arm of the opcode classifier. Track delimiter depth and only accept
    `_` at the first token of a top-level arm.
    """
    depth = 0
    line_start = True
    i = 0
    n = len(block)
    while i < n:
        c = block[i]
        if c == "," and depth == 1:
            line_start = True
            i += 1
            continue
        if c in "([{":
            depth += 1
            line_start = depth == 1
            i += 1
            continue
        if c in ")]}":
            depth = max(depth - 1, 0)
            line_start = c == "}" and depth == 1
            i += 1
            continue
        if c == "\n":
            line_start = True
            i += 1
            continue
        if line_start and c in " \t\r":
            i += 1
            continue
        if depth == 1 and line_start and c == "_":
            j = i + 1
            while j < n and block[j] in " \t\r\n":
                j += 1
            if block.startswith("=>", j):
                return i
        line_start = False
        i += 1
    return None


def _default_arm_body(block: str, wildcard_start: int) -> str:
    """Extract just the body of the `_ => …` arm (block or expression), so
    classification of the default does not see the rest of the match."""
    arrow = block.find("=>", wildcard_start)
    if arrow < 0:
        return ""
    i = arrow + 2
    while i < len(block) and block[i] in " \t\r\n":
        i += 1
    if i < len(block) and block[i] == "{":
        _, body = _balanced_block(block, i)
        return body
    depth = 0
    start = i
    while i < len(block):
        c = block[i]
        if c in "([{":
            depth += 1
        elif c in ")]}":
            if depth == 0:
                break
            depth -= 1
        elif c == "," and depth == 0:
            break
        i += 1
    return block[start:i]


def _scan_matches_macro(text: str, start: int) -> tuple[int, str] | None:
    """From the index of `matches!`, return (end, arg_text) by paren-balancing."""
    paren = text.find("(", start)
    if paren < 0:
        return None
    depth = 0
    i = paren
    n = len(text)
    while i < n:
        c = text[i]
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i + 1, text[paren + 1 : i]
        i += 1
    return None


def _line_count(text: str) -> int:
    return text.count("\n") + 1


def _line_of_offset(text: str, offset: int) -> int:
    return text.count("\n", 0, offset) + 1


def _line_start_depths(text: str) -> dict[int, int]:
    depths = {0: 0}
    depth = 0
    i = 0
    n = len(text)
    while i < n:
        c = text[i]
        if c == "{":
            depth += 1
        elif c == "}":
            depth = max(depth - 1, 0)
        elif c == "\n":
            depths[i + 1] = depth
        i += 1
    return depths


def _line_start_for_offset(text: str, offset: int) -> int:
    return text.rfind("\n", 0, offset) + 1


_RUST_TOP_LEVEL_ITEM_RE = re.compile(
    r"(?m)^[^\S\r\n]*"
    r"(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:async\s+|unsafe\s+|extern\s+)*"
    r"(?P<kind>fn|impl|trait|struct|enum|mod)\b"
)


def _rust_top_level_regions(text: str) -> list[SourceRegion]:
    code = mask_rust_comments_and_strings(mask_rust_test_items(text))
    depths = _line_start_depths(code)
    regions: list[SourceRegion] = []
    for m in _RUST_TOP_LEVEL_ITEM_RE.finditer(code):
        line_start = _line_start_for_offset(text, m.start())
        if depths.get(line_start, 0) != 0:
            continue
        kind = m.group("kind")
        name = _rust_region_name(text, m.end(), kind)
        end_offset = _rust_region_end(code, m.end())
        regions.append(
            SourceRegion(
                kind=kind,
                name=name,
                start_line=_line_of_offset(text, m.start()),
                end_line=_line_of_offset(text, max(m.start(), end_offset - 1)),
            )
        )
    return regions


def _rust_region_name(text: str, start: int, kind: str) -> str:
    line_end = text.find("\n", start)
    if line_end < 0:
        line_end = len(text)
    tail = text[start:line_end].strip()
    if kind == "impl":
        return " ".join(tail.split())[:80] or "impl"
    m = re.match(r"([A-Za-z_][A-Za-z0-9_]*)", tail)
    return m.group(1) if m else kind


def _rust_region_end(text: str, start: int) -> int:
    brace = text.find("{", start)
    semi = text.find(";", start)
    if semi >= 0 and (brace < 0 or semi < brace):
        return semi + 1
    if brace >= 0:
        end, _ = _balanced_block(text, brace)
        return end
    line_end = text.find("\n", start)
    return len(text) if line_end < 0 else line_end


def _python_top_level_regions(text: str) -> list[SourceRegion]:
    try:
        tree = ast.parse(text)
    except SyntaxError:
        return _python_top_level_regions_fallback(text)
    regions: list[SourceRegion] = []
    for node in tree.body:
        if isinstance(node, ast.FunctionDef | ast.AsyncFunctionDef | ast.ClassDef):
            end = getattr(node, "end_lineno", None) or node.lineno
            kind = (
                "class"
                if isinstance(node, ast.ClassDef)
                else "async def"
                if isinstance(node, ast.AsyncFunctionDef)
                else "def"
            )
            regions.append(
                SourceRegion(
                    kind=kind,
                    name=node.name,
                    start_line=node.lineno,
                    end_line=end,
                )
            )
    return regions


def _python_top_level_regions_fallback(text: str) -> list[SourceRegion]:
    starts: list[tuple[str, str, int]] = []
    for line_no, line in enumerate(text.splitlines(), start=1):
        m = re.match(
            r"(?P<kind>class|async\s+def|def)\s+"
            r"(?P<name>[A-Za-z_][A-Za-z0-9_]*)\b",
            line,
        )
        if m:
            starts.append((" ".join(m.group("kind").split()), m.group("name"), line_no))
    regions: list[SourceRegion] = []
    for i, (kind, name, start) in enumerate(starts):
        end = starts[i + 1][2] - 1 if i + 1 < len(starts) else _line_count(text)
        regions.append(
            SourceRegion(kind=kind, name=name, start_line=start, end_line=end)
        )
    return regions


def _top_level_regions(path: Path, text: str) -> list[SourceRegion]:
    if path.suffix == ".rs":
        return _rust_top_level_regions(text)
    if path.suffix == ".py":
        return _python_top_level_regions(text)
    return []


def _structural_god_score(regions: list[SourceRegion]) -> int:
    return sum(max(0, region.span - _LARGE_SOURCE_REGION_LINES) for region in regions)


def _large_source_regions(regions: list[SourceRegion]) -> list[SourceRegion]:
    return [region for region in regions if region.span >= _LARGE_SOURCE_REGION_LINES]


def _is_structural_god_region_set(large_regions: list[SourceRegion]) -> bool:
    if len(large_regions) >= _STRUCTURAL_GOD_MIN_LARGE_REGIONS:
        return True
    kind_count = len({region.kind for region in large_regions})
    return (
        len(large_regions) >= 2
        and kind_count >= 2
        and _structural_god_score(large_regions) >= _STRUCTURAL_GOD_MIXED_KIND_MIN_SCORE
    )


def _region_summary(regions: list[SourceRegion], limit: int = 6) -> str:
    ranked = sorted(regions, key=lambda region: (-region.span, region.start_line))
    parts = [
        f"{region.kind} {region.name} {region.span} lines" for region in ranked[:limit]
    ]
    if len(ranked) > limit:
        parts.append(f"{len(ranked) - limit} more")
    return "; ".join(parts)


def _large_source_files(
    root: Path,
    ceiling: int = 4000,
    py_ceiling: int = 2500,
) -> list[LargeSourceFile]:
    return list(
        _run_cached(
            ("large", root, ceiling, py_ceiling, _source_scope(root)),
            lambda: _scan_large_source_files(root, ceiling, py_ceiling),
        )
    )


def _scan_large_source_files(
    root: Path, ceiling: int, py_ceiling: int
) -> list[LargeSourceFile]:
    files: list[LargeSourceFile] = []
    test_paths = _rust_test_source_paths(root)
    for suffix, lang_ceiling in ((".rs", ceiling), (".py", py_ceiling)):
        for path in _iter_source_files(root, (suffix,)):
            if _is_generated(path, root) or (
                suffix == ".rs" and _resolved(path) in test_paths
            ):
                continue
            try:
                text = _source_text(path)
            except OSError:
                continue
            line_count = _line_count(text)
            if line_count < lang_ceiling:
                continue
            files.append(
                LargeSourceFile(
                    path=path,
                    rel=path.relative_to(root).as_posix(),
                    text=text,
                    line_count=line_count,
                    ceiling=lang_ceiling,
                    suffix=suffix,
                )
            )
    return files


def _source_sibling_count(root: Path, directory: Path, suffix: str) -> int:
    if not directory.is_dir():
        return 0
    count = 0
    for path in directory.iterdir():
        if not path.is_file() or path.suffix != suffix:
            continue
        if _is_excluded(path, root) or _is_generated(path, root):
            continue
        count += 1
    return count


def _decomposition_context(item: LargeSourceFile, root: Path) -> str | None:
    same_dir_siblings = _source_sibling_count(root, item.path.parent, item.suffix)
    if same_dir_siblings >= _DECOMPOSITION_PACKAGE_MIN_SIBLINGS:
        return f"sibling-rich package ({same_dir_siblings} {item.suffix} files)"

    stem_dir = item.path.with_suffix("")
    stem_dir_siblings = _source_sibling_count(root, stem_dir, item.suffix)
    if stem_dir_siblings >= _DECOMPOSITION_PACKAGE_MIN_SIBLINGS:
        return f"decomposition directory `{stem_dir.name}/` ({stem_dir_siblings} {item.suffix} files)"

    return None


def _cohesive_decomposition_ceiling(item: LargeSourceFile) -> int:
    return int(item.ceiling * _COHESIVE_DECOMPOSITION_CEILING_FACTOR)


@_audit_probe
def probe_semantic_fallthroughs(root: Path) -> list[Finding]:
    """Hand-maintained semantic classifications over OpCode/kind that drift
    silently: `match {.. _ => default}` and `matches!(x, OpCode::A | B | ..)`.

    Each is a row the op-semantics ladder (op_kinds.toml) could absorb, deleting
    a drift point. EXHAUSTIVE matches (no wildcard) are rustc-gated and SKIPPED —
    they cannot drift, so flagging them would be noise."""
    findings: list[Finding] = []
    test_paths = _rust_test_source_paths(root)
    for path in _iter_source_files(root, (".rs",)):
        if _is_generated(path, root) or _resolved(path) in test_paths:
            continue
        try:
            raw = _source_text(path)
        except OSError:
            continue
        if "OpCode::" not in raw:
            continue
        text = mask_rust_comments_and_strings(mask_rust_test_items(raw))
        rel = path.relative_to(root).as_posix()
        critical = _file_is_critical(path)

        # (a) match blocks with a wildcard default over opcode-like scrutinee.
        for m in _MATCH_HEAD_RE.finditer(text):
            scrutinee = m.group(1)
            if _GENERATED_OPCODE_TABLE_SCRUTINEE_RE.search(scrutinee):
                # Generated opcode tables are exhaustive and rustc-gated at the
                # authority boundary. A consumer matching their role enum may
                # still mention OpCode for operand-shape details; that is not
                # a hand-maintained opcode membership list.
                continue
            brace_idx = m.end() - 1
            _, block = _balanced_block(text, brace_idx)
            opcode_arms = len(set(_OPCODE_ARM_RE.findall(block)))
            if opcode_arms < 2:
                continue
            if not _KIND_SCRUTINEE_RE.search(scrutinee):
                continue
            wildcard_start = _top_level_wildcard_arm_start(block)
            if wildcard_start is None:
                continue  # exhaustive → compiler-gated → safe, skip
            default_body = _default_arm_body(block, wildcard_start)
            if _FAILLOUD_RE.search(default_body):
                continue  # fail-closed dispatch switchboard → correct, not drift
            if _EMITTER_RE.search(default_body):
                continue  # mechanical lowering route → not a semantic *fact*
            # Survivors: a classifier with a silent VALUE default — the genuine
            # hand-maintained-opcode-fact surface the op-semantics ladder retires.
            # Ranked by OBJECTIVE signals only (arm-count × file-criticality); the
            # default polarity (false vs None) is context-dependent — reported in
            # `detail` as context but NOT used to claim miscompile-risk, which
            # would misfire on conservative-safe defaults (e.g. licm `is_hoistable`
            # → false) and idiomatic Option special-case lookups (→ None).
            line = text.count("\n", 0, m.start()) + 1
            default_txt = " ".join(default_body.split())[:60]
            big = opcode_arms >= 6
            if critical and big:
                sev = "high"
            elif critical or big:
                sev = "medium"
            else:
                sev = "low"
            findings.append(
                Finding(
                    probe="semantic_fallthrough",
                    severity=sev,
                    title=f"hand-classified `match` over {opcode_arms} opcodes (silent default)",
                    location=f"{rel}:{line}",
                    detail=f"scrutinee `{scrutinee.strip()[:50]}`; default `{default_txt}`",
                    suggested_action="if this encodes op semantics, migrate into "
                    "op_kinds.toml ([[opcode]] row / classifier set) "
                    "and read the generated predicate",
                    class_retired="hand-maintained-opcode-fact",
                    metric=opcode_arms + (50 if critical else 0),
                )
            )

        # (b) matches!(x, OpCode::A | OpCode::B | ..) — implicit-false hand-set.
        for mm in _MATCHES_MACRO_RE.finditer(text):
            res = _scan_matches_macro(text, mm.start())
            if not res:
                continue
            _, arg = res
            arms = set(_OPCODE_ARM_RE.findall(arg))
            if len(arms) < 3:
                continue  # 1-2 opcode guards are legitimate structural checks
            line = text.count("\n", 0, mm.start()) + 1
            sev = "medium" if critical else "low"
            findings.append(
                Finding(
                    probe="semantic_fallthrough",
                    severity=sev,
                    title=f"`matches!` hand-set of {len(arms)} opcodes (implicit-false default)",
                    location=f"{rel}:{line}",
                    detail=f"set: {', '.join(sorted(a.split('::')[1] for a in arms))[:80]}",
                    suggested_action="if this encodes a semantic property, add a "
                    "classifier set to op_kinds.toml and query the "
                    "generated predicate instead of a literal list",
                    class_retired="missed-fact-on-new-opcode",
                    metric=len(arms),
                )
            )
    return findings


@_audit_probe
def probe_large_source_files(
    root: Path,
    ceiling: int = 4000,
    py_ceiling: int = 2500,
) -> list[Finding]:
    """Board-only size signal for large files.

    Raw line count remains useful triage, but it is not a ratchet: a correct
    decomposition can temporarily increase the number of cohesive large files.
    The CI gate ratchets concern-mixing and undecomposed debt instead.
    """
    findings: list[Finding] = []
    for item in _large_source_files(root, ceiling=ceiling, py_ceiling=py_ceiling):
        context = _decomposition_context(item, root)
        sev = (
            "high"
            if item.line_count >= item.ceiling * 3
            else "medium"
            if item.line_count >= item.ceiling * 1.5
            else "low"
        )
        findings.append(
            Finding(
                probe="large_source_file",
                severity=sev,
                title=f"{item.line_count} lines (ceiling {item.ceiling})",
                location=item.rel,
                detail=(
                    f"{item.line_count} lines; "
                    f"{context or 'no decomposition context detected'}"
                ),
                suggested_action=(
                    "use as a human size triage signal only; CI ratchets "
                    "kitchen_sink_file and undecomposed_god_file"
                ),
                class_retired="raw-size-triage-only",
                metric=item.line_count,
            )
        )
    return findings


@_audit_probe
def probe_kitchen_sink_files(
    root: Path,
    ceiling: int = 4000,
    py_ceiling: int = 2500,
) -> list[Finding]:
    """Oversized files with concern-mixing top-level regions."""
    findings: list[Finding] = []
    for item in _large_source_files(root, ceiling=ceiling, py_ceiling=py_ceiling):
        large_regions = _large_source_regions(_top_level_regions(item.path, item.text))
        if not _is_structural_god_region_set(large_regions):
            continue
        score = _structural_god_score(large_regions)
        large_region_count = len(large_regions)
        sev = (
            "high"
            if score >= item.ceiling and large_region_count >= 4
            else "medium"
            if score >= item.ceiling // 2 or large_region_count >= 4
            else "low"
        )
        context = _decomposition_context(item, root)
        findings.append(
            Finding(
                probe="kitchen_sink_file",
                severity=sev,
                title=(
                    f"{large_region_count} large top-level regions "
                    f"({score} excess lines)"
                ),
                location=item.rel,
                detail=(
                    f"{item.line_count} lines; "
                    f"{context or 'no decomposition context detected'}; "
                    f"large_regions={_region_summary(large_regions)}"
                ),
                suggested_action=(
                    "extract the mixed top-level regions into cohesive modules; "
                    "do not add more authority to this concern-mixing file"
                ),
                class_retired="multi-region-kitchen-sink",
                metric=float(score),
            )
        )
    return findings


@_audit_probe
def probe_undecomposed_god_files(
    root: Path,
    ceiling: int = 4000,
    py_ceiling: int = 2500,
) -> list[Finding]:
    """Oversized source files with no sibling decomposition context."""
    findings: list[Finding] = []
    for item in _large_source_files(root, ceiling=ceiling, py_ceiling=py_ceiling):
        context = _decomposition_context(item, root)
        if context is not None:
            continue
        sev = (
            "high"
            if item.line_count >= item.ceiling * 3
            else "medium"
            if item.line_count >= item.ceiling * 1.5
            else "low"
        )
        findings.append(
            Finding(
                probe="undecomposed_god_file",
                severity=sev,
                title=f"{item.line_count} undecomposed lines (ceiling {item.ceiling})",
                location=item.rel,
                detail=(
                    f"{item.line_count} lines; no sibling decomposition package "
                    "or stem directory detected"
                ),
                suggested_action=(
                    "create a cohesive decomposition package and move every "
                    "sibling concern needed to delete the monolith lane"
                ),
                class_retired="lone-undecomposed-god-file",
                metric=float(item.line_count),
            )
        )
    return findings


_COMMENT_DEBT_RE = re.compile(
    r"\b(TODO|FIXME|HACK|XXX|WORKAROUND|KLUDGE)\b|"
    r"\bfor now\b|"
    r"\btemporar(?:y|ily)\s+"
    r"(?:"
    r"allow|allowed|bypass|bypassed|compat|defer|deferred|disable|disabled|"
    r"fallback|guard|hack|ignore|ignored|placeholder|relax|relaxed|shim|"
    r"skip|skipped|special-case|stub|stubbed|workaround"
    r")\b",
    re.IGNORECASE,
)
_CODE_DEBT_RE = re.compile(r"\b(unimplemented!|todo!)\s*\(")


def _python_comment_segments(text: str) -> list[tuple[int, str]]:
    try:
        tokens = tokenize.generate_tokens(io.StringIO(text).readline)
        return [
            (tok.start[0], tok.string) for tok in tokens if tok.type == tokenize.COMMENT
        ]
    except tokenize.TokenError:
        return [
            (line_no, line)
            for line_no, line in enumerate(text.splitlines(), start=1)
            if line.lstrip().startswith("#")
        ]


def _debt_marker_hits(path: Path, text: str) -> list[DebtMarkerHit]:
    if path.suffix == ".py":
        # A comment is a substring of the text, so a file whose text has no
        # marker has no commented one either and is not tokenized.
        comment_segments = (
            _python_comment_segments(text) if _COMMENT_DEBT_RE.search(text) else []
        )
        code_text = ""
    elif path.suffix == ".rs":
        projection = project_rust_source(text)
        comment_segments = projection.comments
        code_text = projection.masked_code
    else:
        comment_segments = []
        code_text = ""

    hits: list[DebtMarkerHit] = []
    for line, comment in comment_segments:
        for match in _COMMENT_DEBT_RE.finditer(comment):
            if _is_stdlib_upstream_advisory_marker(path, comment, match):
                continue
            hits.append(DebtMarkerHit(line=line, marker=match.group(0)))
    if code_text:
        for match in _CODE_DEBT_RE.finditer(code_text):
            hits.append(
                DebtMarkerHit(
                    line=_line_of_offset(code_text, match.start()),
                    marker=match.group(1),
                )
            )
    return sorted(hits, key=lambda hit: (hit.line, hit.marker.lower()))


def _is_stdlib_upstream_advisory_marker(
    path: Path, comment: str, match: re.Match[str]
) -> bool:
    """Ignore CPython-style ``XXX`` notes in vendored stdlib mirrors.

    The debt ratchet is for Molt-owned workaround and implementation debt. A
    large fraction of ``src/molt/stdlib`` mirrors upstream CPython files, whose
    old ``XXX`` editorial questions are not Molt compatibility lanes. Keep
    counting any owned debt token in the same comment through its own regex
    match; only suppress the advisory ``XXX`` token itself.
    """
    rel = path.as_posix()
    if "/src/molt/stdlib/" not in f"/{rel}":
        return False
    marker = match.group(0)
    return marker.upper() == "XXX"


@_audit_probe
def probe_debt_markers(root: Path) -> list[Finding]:
    """Workaround/debt markers — the CLAUDE.md zero-workaround policy made
    machine-checkable. Reported per file (ranked), ratcheted in aggregate."""
    findings: list[Finding] = []
    for path in _iter_source_files(root, (".rs", ".py")):
        if _is_generated(path, root):
            continue
        try:
            text = _source_text(path)
        except OSError:
            continue
        hits = _debt_marker_hits(path, text)
        count = len(hits)
        if count == 0:
            continue
        rel = path.relative_to(root).as_posix()
        sev = "medium" if count >= 15 else "low"
        first_line = hits[0].line
        examples = ", ".join(f"L{hit.line}:{hit.marker}" for hit in hits[:5])
        findings.append(
            Finding(
                probe="debt_marker",
                severity=sev,
                title=f"{count} debt/workaround markers",
                location=f"{rel}:{first_line}",
                detail=examples,
                suggested_action="resolve in place (zero-workaround policy) or convert "
                "to a tracked task with a structural fix",
                class_retired="accumulating-technical-debt",
                metric=count,
            )
        )
    return findings


_INTRINSIC_FIRST_STUB_RE = re.compile(
    r"not fully lowered yet; only an intrinsic-first stub is available|"
    r"intrinsic-first (?:top-level )?stdlib .*stub|"
    r"stub-only for now",
    re.IGNORECASE,
)


def _python_raise_is_notimplemented(node: ast.Raise) -> bool:
    exc = node.exc
    if isinstance(exc, ast.Call):
        exc = exc.func
    if isinstance(exc, ast.Name):
        return exc.id == "NotImplementedError"
    if isinstance(exc, ast.Attribute):
        return exc.attr == "NotImplementedError"
    return False


def _python_string_constants(node: ast.AST | None) -> list[ast.Constant]:
    if node is None:
        return []
    return [
        child
        for child in ast.walk(node)
        if isinstance(child, ast.Constant) and isinstance(child.value, str)
    ]


def _python_raise_nodes(tree: ast.Module, text: str) -> list[ast.Raise]:
    """Every ``raise`` statement, from one walk shared by the stub probes.

    A ``Raise`` node needs the literal ``raise`` keyword in the source, so a
    file without it has none and its tree is not walked.
    """
    if "raise" not in text:
        return []
    return [node for node in ast.walk(tree) if isinstance(node, ast.Raise)]


def _python_intrinsic_stub_surface_hit(
    tree: ast.Module, raises: list[ast.Raise]
) -> ImplementationGapHit | None:
    hits: list[ImplementationGapHit] = []
    if tree.body:
        first = tree.body[0]
        if (
            isinstance(first, ast.Expr)
            and isinstance(first.value, ast.Constant)
            and isinstance(first.value.value, str)
            and _INTRINSIC_FIRST_STUB_RE.search(first.value.value)
        ):
            hits.append(
                ImplementationGapHit(
                    line=getattr(first, "lineno", 1),
                    marker="intrinsic-first stub",
                )
            )
    for node in raises:
        for string_node in _python_string_constants(node.exc):
            if _INTRINSIC_FIRST_STUB_RE.search(str(string_node.value)):
                hits.append(
                    ImplementationGapHit(
                        line=getattr(string_node, "lineno", getattr(node, "lineno", 1)),
                        marker="intrinsic-first stub",
                    )
                )
                break
    if not hits:
        return None
    return sorted(hits, key=lambda hit: (hit.line, hit.marker))[0]


def _python_stub_surface_hits(path: Path, text: str) -> list[ImplementationGapHit]:
    hits: list[ImplementationGapHit] = []
    try:
        tree = ast.parse(text)
    except SyntaxError:
        first_stub_match = _INTRINSIC_FIRST_STUB_RE.search(text)
        if first_stub_match is not None:
            hits.append(
                ImplementationGapHit(
                    line=_line_of_offset(text, first_stub_match.start()),
                    marker="intrinsic-first stub",
                )
            )
        return hits
    raises = _python_raise_nodes(tree, text)
    intrinsic_stub_hit = _python_intrinsic_stub_surface_hit(tree, raises)
    if intrinsic_stub_hit is not None:
        hits.append(intrinsic_stub_hit)
    for node in raises:
        if _python_raise_is_notimplemented(node):
            hits.append(
                ImplementationGapHit(
                    line=getattr(node, "lineno", 1),
                    marker="raise NotImplementedError",
                )
            )
    return sorted(
        {(hit.line, hit.marker): hit for hit in hits}.values(),
        key=lambda hit: (hit.line, hit.marker),
    )


def _compatibility_findings(path: Path, root: Path, inventory) -> list[Finding]:
    return [
        Finding(
            probe="compatibility_error_outcome"
            if hit.proved
            else "compatibility_error_applicability",
            severity="info" if hit.proved else "high",
            title=(
                "proved CPython error outcome"
                if hit.proved
                else "unproved compatibility error"
            )
            + ": "
            + hit.outcome,
            location=f"{path.relative_to(root).as_posix()}:{hit.line}",
            detail=hit.detail,
            suggested_action="retain the canonical runtime predicate and differential witness"
            if hit.proved
            else "restore the canonical protocol and prove this outcome",
            class_retired="compatibility-error-applicability",
            metric=0 if hit.proved else 1,
        )
        for hit in inventory
    ]


def _compatibility_projection_inventory(path: Path, root: Path, proved: bool):
    rel = path.relative_to(root).as_posix()
    if rel not in compatibility_errors.projections():
        return None
    python = rel == compatibility_errors.PYTHON_PATH
    return [
        compatibility_errors.InventoryHit(
            1,
            name,
            proved,
            compatibility_errors.describe(name)
            if proved
            else "unproved runtime compatibility projection: " + name,
        )
        for name in compatibility_errors.OUTCOMES
        if name.startswith("Memoryview") != python
    ]


@_audit_probe
def probe_python_stub_surfaces(root: Path) -> list[Finding]:
    """Python implementation-gap surfaces as a first-class ratchet.

    Generic comment TODOs are useful, but an executable stub or a direct
    NotImplementedError is a stronger signal: the runtime has an admitted gap.
    Keep this separate so stdlib/frontend compatibility debt cannot hide inside
    a broad marker count.
    """
    findings: list[Finding] = []
    compatibility_proved = not compatibility_errors.projection_errors(root)
    for path in _iter_source_files(root, (".py",)):
        if (
            _is_generated(path, root)
            and path.relative_to(root).as_posix()
            not in compatibility_errors.projections()
        ):
            continue
        try:
            text = _source_text(path)
        except OSError:
            continue
        projection = _compatibility_projection_inventory(
            path, root, compatibility_proved
        )
        inventory = (
            projection
            if projection is not None
            else compatibility_errors.python_inventory(text, compatibility_proved)
        )
        findings.extend(_compatibility_findings(path, root, inventory))
        # Exact canonical emitters are classified, never skipped by a generated
        # filename/header exemption. Invalid projection/calls consume stub debt.
        hits = (
            []
            if projection is not None and compatibility_proved
            else _python_stub_surface_hits(path, text)
        )
        hits.extend(
            ImplementationGapHit(hit.line, "unproved compatibility: " + hit.outcome)
            for hit in inventory
            if not hit.proved
        )
        if not hits:
            continue
        rel = path.relative_to(root).as_posix()
        count = len(hits)
        examples = ", ".join(f"L{hit.line}:{hit.marker}" for hit in hits[:5])
        findings.append(
            Finding(
                probe="python_stub_surface",
                severity="medium" if count >= 3 else "low",
                title=f"{count} Python stub/NotImplemented surface(s)",
                location=f"{rel}:{hits[0].line}",
                detail=examples,
                suggested_action=(
                    "replace the stub with a real intrinsic/runtime/compiler "
                    "primitive or delete the surface if it is outside the Molt "
                    "AOT contract"
                ),
                class_retired="python-executable-stub-surface",
                metric=count,
            )
        )
    return findings


def _rust_test_source_paths(root: Path) -> set[Path]:
    # Ownership must see declarations outside a --path diagnostic selection.
    def compute() -> set[Path]:
        paths = [
            path
            for sub in _SOURCE_ROOTS
            if (root / sub).is_dir()
            for path in _iter_pruned_files(root / sub, root, (".rs",))
        ]
        return rust_test_only_source_files(paths)

    return _run_cached(("rust_test_paths", root), compute)


def _rust_line_is_comment_only(line: str) -> bool:
    stripped = line.lstrip()
    return (
        stripped.startswith("//")
        or stripped.startswith("/*")
        or stripped.startswith("*")
    )


# Rust Pattern_White_Space, including bidi marks absent from Python's \s.
_RUST_WS_CHARS = "\t\n\v\f\r \x85\u200e\u200f\u2028\u2029"
_RUST_WS = r"[\t\n\v\f\r \x85\u200e\u200f\u2028\u2029]"


_RUST_NOTIMPLEMENTED_STUB_CONTEXT_RE = re.compile(
    r"\b(?:raise_exception|set_exception|raise_py_exception)\b"
)


def _rust_line_raises_notimplemented(lines: list[str], index: int) -> bool:
    """Return true when a NotImplementedError token is part of a live raise.

    Rust code also names ``NotImplementedError`` in exception hierarchy tables
    and in fallback catch lists. Those are compatibility facts, not executable
    implementation gaps. Count the token only when it is adjacent to a runtime
    exception-emission primitive.
    """
    line = lines[index]
    if "NotImplementedError" not in line:
        return False
    start = max(0, index - 5)
    end = min(len(lines), index + 3)
    window = "\n".join(lines[start:end])
    return _RUST_NOTIMPLEMENTED_STUB_CONTEXT_RE.search(window) is not None


def _rust_stub_surface_hits(text: str) -> list[ImplementationGapHit]:
    hits: list[ImplementationGapHit] = []
    live_text = mask_rust_test_items(text)
    lines = live_text.split("\n")
    code_without_comments_or_strings = mask_rust_comments_and_strings(live_text)
    for match in _CODE_DEBT_RE.finditer(code_without_comments_or_strings):
        hits.append(
            ImplementationGapHit(
                line=_line_of_offset(code_without_comments_or_strings, match.start()),
                marker=match.group(1),
            )
        )
    for line_no, line in enumerate(lines, start=1):
        if _rust_line_is_comment_only(line):
            continue
        if "MOLT_STUB" in line:
            hits.append(ImplementationGapHit(line=line_no, marker="MOLT_STUB"))
        if _rust_line_raises_notimplemented(lines, line_no - 1):
            hits.append(
                ImplementationGapHit(line=line_no, marker="NotImplementedError")
            )
        if re.search(r"panic!\s*\([^)]*not implemented", line, re.IGNORECASE):
            hits.append(
                ImplementationGapHit(line=line_no, marker="panic!(not implemented)")
            )
    return sorted(
        {(hit.line, hit.marker): hit for hit in hits}.values(),
        key=lambda hit: (hit.line, hit.marker),
    )


@_audit_probe
def probe_rust_stub_surfaces(root: Path) -> list[Finding]:
    """Rust/backend/runtime implementation-gap surfaces.

    This catches generated-code stubs such as `MOLT_STUB` emission, Rust
    `todo!`/`unimplemented!`, and production runtime paths that raise
    `NotImplementedError`. Test fixtures are skipped so the ratchet tracks live
    implementation debt, not intentionally fake harness inputs.
    """
    findings: list[Finding] = []
    compatibility_proved = not compatibility_errors.projection_errors(root)
    test_paths = _rust_test_source_paths(root)
    for path in _iter_source_files(root, (".rs",)):
        if (
            _is_generated(path, root)
            and path.relative_to(root).as_posix()
            not in compatibility_errors.projections()
        ) or _resolved(path) in test_paths:
            continue
        try:
            text = _source_text(path)
        except OSError:
            continue
        projection = _compatibility_projection_inventory(
            path, root, compatibility_proved
        )
        inventory = (
            projection
            if projection is not None
            else compatibility_errors.rust_inventory(
                text,
                mask_rust_comments_and_strings(mask_rust_test_items(text)),
                compatibility_proved,
            )
        )
        findings.extend(_compatibility_findings(path, root, inventory))
        # Exact canonical emitters are classified, never skipped by a generated
        # filename/header exemption. Invalid projection/calls consume stub debt.
        hits = (
            []
            if projection is not None and compatibility_proved
            else _rust_stub_surface_hits(text)
        )
        hits.extend(
            ImplementationGapHit(hit.line, "unproved compatibility: " + hit.outcome)
            for hit in inventory
            if not hit.proved
        )
        if not hits:
            continue
        rel = path.relative_to(root).as_posix()
        count = len(hits)
        examples = ", ".join(f"L{hit.line}:{hit.marker}" for hit in hits[:5])
        findings.append(
            Finding(
                probe="rust_stub_surface",
                severity="medium" if count >= 3 else "low",
                title=f"{count} Rust stub/NotImplemented surface(s)",
                location=f"{rel}:{hits[0].line}",
                detail=examples,
                suggested_action=(
                    "replace emitted stubs or NotImplementedError paths with the "
                    "shared runtime/compiler primitive that owns the missing "
                    "semantics"
                ),
                class_retired="rust-executable-stub-surface",
                metric=count,
            )
        )
    return findings


def _rust_match_arm_text_before(
    lines: list[str], call_index: int, *, call_end_offset: int | None = None
) -> str:
    code = mask_rust_comments_and_strings("\n".join(lines))
    code_lines = code.split("\n")
    floor = max(0, call_index - 50)
    window_start = sum(len(line) + 1 for line in code_lines[:floor])
    if call_end_offset is None:
        call_end_offset = sum(len(line) + 1 for line in code_lines[: call_index + 1])
    anchors = list(
        re.finditer(
            rf"=>{_RUST_WS}*(?:\{{|self{_RUST_WS}*\.)",
            code[window_start:call_end_offset],
        )
    )
    arrow_index = (
        code.count("\n", 0, window_start + anchors[-1].start()) if anchors else None
    )
    if arrow_index is None:
        return lines[call_index].strip()
    start = arrow_index
    while start > 0:
        previous = lines[start - 1].strip(_RUST_WS_CHARS)
        # A completed neighboring arm is not part of this arm's pattern, even
        # when its first token is another opcode string literal.
        if "=>" in code_lines[start - 1]:
            break
        if not previous:
            start -= 1
            continue
        if previous.startswith('"') or previous.startswith('| "'):
            start -= 1
            continue
        break
    arrow_offset = window_start + anchors[-1].start()
    arrow_line_start = code.rfind("\n", 0, arrow_offset) + 1
    # Retain literal patterns, but never count a diagnostic/body literal as an op.
    final_pattern = lines[arrow_index][: arrow_offset - arrow_line_start + 2]
    return "\n".join(
        [*(line.strip() for line in lines[start:arrow_index]), final_pattern.strip()]
    )


def _rust_backend_lowering_gap_marker(arm_text: str) -> tuple[str, int]:
    names = re.findall(r'"([a-z][a-z0-9_]*)"', arm_text)
    if names:
        marker = ", ".join(names[:10])
        if len(names) > 10:
            marker += f", ... ({len(names)} total)"
        return marker, len(names)
    if re.search(r"\bother\s*=>", arm_text):
        return "catch-all Rust backend op", 1
    if re.search(r"_\s*=>", arm_text):
        return "method catch-all", 1
    return "unsupported lowering diagnostic", 1


def _rust_self_reference_matches(body: str):
    # Named self/Self references are lexical candidates, not target/type proofs.
    # Calls, method values and associated references share whitespace grammar.
    return re.finditer(
        rf"\b(?:(?P<instance>self){_RUST_WS}*\.{_RUST_WS}*|Self{_RUST_WS}*::{_RUST_WS}*)"
        rf"(?:r#)?(?P<name>[A-Za-z_]\w*)(?P<call>{_RUST_WS}*\()?",
        body,
    )


def _rust_self_reference_names(body: str) -> set[str]:
    return {match["name"] for match in _rust_self_reference_matches(body)}


def _rust_single_self_call(body: str) -> str | None:
    """Recognize a rejection-only forwarding body, never infer branch semantics."""
    match = next(_rust_self_reference_matches(body), None)
    if (
        match is None
        or body[: match.start()].strip(_RUST_WS_CHARS)
        or match["instance"] is None
        or match["call"] is None
    ):
        return None
    depth = 1
    cursor = match.end()
    while cursor < len(body) and depth:
        depth += (body[cursor] == "(") - (body[cursor] == ")")
        cursor += 1
    if depth or body[cursor:].strip(_RUST_WS_CHARS) not in {"", ";"}:
        return None
    return match["name"]


def _rust_terminal_refusal_push(body: str, fields: set[str]) -> bool:
    """Recognize the bounded straight-line refusal recorder, not any mutation.

    Extend/insert and pushes followed by clearing are only diagnostic producers.
    A deferred closure or control-flow body cannot grant a definite path proof.
    Receiver/type/macro resolution beyond the source-bound recorder is unproven.
    """
    if re.search(r"\b(?:if|match|while|for|loop|return|move)\b|\|", body):
        return False
    for field in fields:
        for match in re.finditer(
            rf"self{_RUST_WS}*\.{_RUST_WS}*{re.escape(field)}{_RUST_WS}*\.{_RUST_WS}*push{_RUST_WS}*\(",
            body,
        ):
            prefix = body[: match.start()]
            # The actual recorder's string normalization is the only preceding
            # statement currently proven; other prefixes remain proof debt.
            if not re.fullmatch(
                rf"{_RUST_WS}*(?:let{_RUST_WS}+([A-Za-z_]\w*){_RUST_WS}*={_RUST_WS}*\1{_RUST_WS}*\.{_RUST_WS}*into{_RUST_WS}*\({_RUST_WS}*\){_RUST_WS}*;{_RUST_WS}*)?",
                prefix,
            ):
                continue
            depth = 1
            for index in range(match.end(), len(body)):
                if body[index] == "(":
                    depth += 1
                elif body[index] == ")":
                    depth -= 1
                    if depth == 0:
                        if any(
                            macro != "format"
                            for macro in re.findall(
                                rf"\b([A-Za-z_]\w*){_RUST_WS}*!",
                                body[match.end() : index],
                            )
                        ):
                            break
                        if re.fullmatch(
                            rf"{_RUST_WS}*;?{_RUST_WS}*", body[index + 1 :]
                        ):
                            return True
                        break
    return False


def _rust_checked_refusal_guards(consumer: str) -> list[re.Match[str]]:
    """Only a nonempty field guard with immediate Err grants consumer identity."""
    matches = []
    for match in re.finditer(
        rf"if{_RUST_WS}+!{_RUST_WS}*self{_RUST_WS}*\.{_RUST_WS}*([A-Za-z_]\w*){_RUST_WS}*\.{_RUST_WS}*is_empty{_RUST_WS}*\({_RUST_WS}*\){_RUST_WS}*\{{",
        consumer,
    ):
        _, block = _balanced_block(consumer, match.end() - 1)
        if re.match(rf"\{{{_RUST_WS}*return{_RUST_WS}+Err{_RUST_WS}*\(", block):
            matches.append(match)
    return matches


def _rust_protocol_surface_admitted(
    code: str, root_code: str, reachable: set[str], unambiguous: set[str]
) -> bool:
    """Bound the publication/whole-state surface independently of body inventory.

    This deliberately fails closed on source shapes outside the recognized
    private assembly protocol; it is not alias, type, macro or Rust CFG proof.
    Module resolution (use aliases, #[path], include!, production tests modules),
    unsafe operations and dynamic/function-pointer dispatch remain unproven.
    Shared whole-family barriers also cover skipped/ambiguous method bodies.
    """
    if len(re.findall(r"\b(?:r#)?compile_checked\b", code)) != 1:
        return False
    visibility = rf"\bpub(?:{_RUST_WS}*\((?P<scope>[^)]*)\))?{_RUST_WS}+[^{{}};]*?\bfn{_RUST_WS}+(?:r#)?(?P<name>[A-Za-z_]\w*){_RUST_WS}*[<(]"
    for surface, root in ((code, False), (root_code, True)):
        for match in re.finditer(visibility, surface):
            exposed = root or (match["scope"] or "").strip(_RUST_WS_CHARS) != "super"
            if exposed and (
                match["name"] not in unambiguous
                or match["name"] in reachable - {"compile_checked"}
            ):
                return False
    # Only source-owned, no-argument constructors may create initial state.
    # Do not exempt arbitrary methods merely because their name is new/default.
    nonconstructor = list(code)
    for match in re.finditer(
        rf"\bfn{_RUST_WS}+(?:new|default){_RUST_WS}*\({_RUST_WS}*\){_RUST_WS}*->{_RUST_WS}*Self{_RUST_WS}*\{{",
        code,
    ):
        end, _ = _balanced_block(code, match.end() - 1)
        nonconstructor[match.end() - 1 : end] = " " * (end - match.end() + 1)
    surface = "".join(nonconstructor)
    constructor = (
        rf"\b(?:Self|RustBackend|Default){_RUST_WS}*::{_RUST_WS}*(?:r#)?(?:new|default)\b"
        rf"|<{_RUST_WS}*(?:Self|RustBackend)(?:{_RUST_WS}+as{_RUST_WS}+Default)?{_RUST_WS}*>{_RUST_WS}*::{_RUST_WS}*(?:r#)?(?:new|default)\b"
    )
    if re.search(constructor, surface):
        return False
    if re.search(
        rf"\*{_RUST_WS}*\(*{_RUST_WS}*self\b{_RUST_WS}*\)*{_RUST_WS}*=(?!=)", surface
    ):
        return False
    if re.search(
        rf"\blet{_RUST_WS}+(?:mut{_RUST_WS}+)?[A-Za-z_]\w*{_RUST_WS}*(?::{_RUST_WS}*[^;=]+)?={_RUST_WS}*(?:&{_RUST_WS}*(?:mut{_RUST_WS}*)?\*?{_RUST_WS}*)?self\b(?!{_RUST_WS}*\.)",
        surface,
    ):
        return False
    # Qualified std/core mem calls and imported free forms share one check;
    # .take() iterator methods are a distinct operation and remain admitted.
    mutation = rf"\b(?P<mem_path>(?:(?:std|core){_RUST_WS}*::{_RUST_WS}*)?mem{_RUST_WS}*::{_RUST_WS}*)?(?:take|replace|swap){_RUST_WS}*\("
    for match in re.finditer(mutation, surface):
        prefix = surface[: match.start()].rstrip(_RUST_WS_CHARS)
        if match["mem_path"] is None and prefix.endswith((".", ":")):
            continue
        if not re.match(
            rf"{_RUST_WS}*&{_RUST_WS}*mut{_RUST_WS}+self{_RUST_WS}*\.",
            surface[match.end() :],
        ):
            return False
    for match in re.finditer(
        rf"\b(?:Self|RustBackend){_RUST_WS}*::{_RUST_WS}*(?:r#)?([A-Za-z_]\w*){_RUST_WS}*\(",
        surface,
    ):
        if match[1] in reachable and not re.match(
            rf"{_RUST_WS}*self\b{_RUST_WS}*[,)]", surface[match.end() :]
        ):
            return False
    return True


def _rust_refusal_protocol_proven(
    methods: dict[str, tuple[str, int, str]],
    fields: set[str],
    recording: set[str],
    family_code: str,
    root_code: str,
    unambiguous: set[str],
) -> bool:
    """Bound the definite-path classification to the checked publication protocol.

    Unknown recovery/reset consumers are diagnostic debt, not a rejection proof.
    This source contract deliberately recognizes the existing backend boundary;
    it does not establish arbitrary Rust receiver, alias, macro or CFG semantics.
    """
    if len(fields) != 1:
        return False
    field = next(iter(fields))
    field_token = rf"\b(?:r#)?{re.escape(field)}\b"
    # Whole production text closes unknown signatures/UFCS and destructuring
    # escapes that a self-call/body inventory cannot safely resolve.
    if len(re.findall(field_token, family_code)) != 1 + sum(
        len(re.findall(field_token, body)) for _, _, body in methods.values()
    ):
        return False
    if len(re.findall(r"\b(?:r#)?emit_source\b", family_code)) != 2:
        return False
    if re.search(
        rf"\bpub(?:{_RUST_WS}*\([^)]*\))?{_RUST_WS}+(?:async{_RUST_WS}+)?fn{_RUST_WS}+(?:r#)?emit_source\b",
        family_code,
    ):
        return False
    declaration = re.search(
        rf"\bstruct{_RUST_WS}+RustBackend{_RUST_WS}*\{{", family_code
    )
    if not declaration:
        return False
    _, structure = _balanced_block(family_code, declaration.end() - 1)
    if not re.search(
        rf"(?m)^{_RUST_WS}*(?:r#)?{re.escape(field)}{_RUST_WS}*:{_RUST_WS}*Vec{_RUST_WS}*<{_RUST_WS}*String{_RUST_WS}*>{_RUST_WS}*,",
        structure,
    ):
        return False
    if any(
        re.search(
            rf"\basync{_RUST_WS}+fn{_RUST_WS}+(?:r#)?{re.escape(name)}\b", family_code
        )
        for name in recording
    ):
        return False
    reachable = set(recording)
    while True:
        added = {
            name
            for name, (_, _, body) in methods.items()
            if _rust_self_reference_names(body) & reachable
        } - reachable
        if not added:
            break
        reachable.update(added)
    if not _rust_protocol_surface_admitted(
        family_code, root_code, reachable, unambiguous
    ):
        return False
    access = rf"\bself{_RUST_WS}*\.{_RUST_WS}*(?:r#)?{re.escape(field)}\b"
    consumer = methods.get("compile_checked", ("", 0, ""))[2]
    emission = re.search(
        rf"let{_RUST_WS}+([A-Za-z_]\w*){_RUST_WS}*={_RUST_WS}*self{_RUST_WS}*\.{_RUST_WS}*emit_source{_RUST_WS}*\(",
        consumer,
    )
    guard = next(
        (
            match
            for match in _rust_checked_refusal_guards(consumer)
            if match[1] == field
        ),
        None,
    )
    if not emission or not guard or emission.start() >= guard.start():
        return False
    if re.search(r"\b(?:return|self)\b", consumer[: emission.start()]):
        return False
    depth = 1
    end = None
    for index in range(emission.end(), len(consumer)):
        if consumer[index] == "(":
            depth += 1
        elif consumer[index] == ")":
            depth -= 1
            if depth == 0:
                end = index + 1
                break
    if end is None or not re.fullmatch(
        rf"{_RUST_WS}*;{_RUST_WS}*", consumer[end : guard.start()]
    ):
        return False
    guard_end, block = _balanced_block(consumer, guard.end() - 1)
    if not re.match(rf"\{{{_RUST_WS}*return{_RUST_WS}+Err{_RUST_WS}*\(", block):
        return False
    if not re.fullmatch(
        rf"{_RUST_WS}*Ok{_RUST_WS}*\({_RUST_WS}*{re.escape(emission[1])}{_RUST_WS}*\){_RUST_WS}*",
        consumer[guard_end:],
    ):
        return False
    entry = methods.get("emit_source", ("", 0, ""))[2]
    reset = re.search(
        rf"{access}{_RUST_WS}*\.{_RUST_WS}*clear{_RUST_WS}*\({_RUST_WS}*\){_RUST_WS}*;",
        entry,
    )
    if not reset or not re.fullmatch(rf"{_RUST_WS}*", entry[: reset.start()]):
        return False
    if re.search(access, entry[reset.end() :]):
        return False
    for name, (_, _, body) in methods.items():
        tokens = len(re.findall(field_token, body))
        expected = (
            2
            if name == "compile_checked"
            else 1
            if name == "emit_source" or name in recording
            else tokens
            if name == "new"
            else 0
        )
        if tokens != expected or (name == "new" and tokens > 1):
            return False
        if name not in {"new", "default"}:
            if re.search(
                rf"\*{_RUST_WS}*self\b{_RUST_WS}*=(?!=)|\b(?:Self|RustBackend){_RUST_WS}*(?::{_RUST_WS}*:{_RUST_WS}*(?:r#)?(?:new|default){_RUST_WS}*\(|\{{)",
                body,
            ):
                return False
            if re.search(
                rf"\blet{_RUST_WS}+(?:mut{_RUST_WS}+)?[A-Za-z_]\w*{_RUST_WS}*={_RUST_WS}*(?:&{_RUST_WS}*(?:mut{_RUST_WS}*)?\*?{_RUST_WS}*)?self\b(?!{_RUST_WS}*\.)",
                body,
            ):
                return False
            for mutation in re.finditer(
                rf"\b(?:(?:std|core){_RUST_WS}*::{_RUST_WS}*)?mem{_RUST_WS}*::{_RUST_WS}*(?:take|replace|swap){_RUST_WS}*\(",
                body,
            ):
                if not re.match(
                    rf"{_RUST_WS}*&{_RUST_WS}*mut{_RUST_WS}+self{_RUST_WS}*\.",
                    body[mutation.end() :],
                ):
                    return False
        references = list(_rust_self_reference_matches(body))
        for call in re.finditer(
            rf"\.{_RUST_WS}*(?:r#)?([A-Za-z_]\w*){_RUST_WS}*\(", body
        ):
            if call[1] in reachable and not any(
                ref["instance"] is not None
                and ref["name"] == call[1]
                and ref.start() <= call.start() < ref.end()
                for ref in references
            ):
                return False
        # An alias/foreign receiver touching this accumulator spelling cannot
        # silently inherit self's ownership proof. Keep owner resolution unmet.
        member_accesses = re.findall(rf"\.{_RUST_WS}*(?:r#)?{re.escape(field)}\b", body)
        if len(member_accesses) != len(re.findall(access, body)):
            return False
        if name in {"emit_source", "compile_checked"}:
            continue
        if re.search(access, body):
            if name not in recording or not _rust_terminal_refusal_push(body, fields):
                return False
    return True


def _rust_rejection_family(
    root: Path,
) -> tuple[set[str], list[Finding], BranchProjection | None]:
    """Inventory production method rejections across sibling lowering modules.

    Only syntactically rejection-only bodies grant a definite dispatch-path
    classification. Mixed bodies remain visible applicability obligations: they
    may enforce valid-input invariants or reject an unsupported operand domain.
    This lexical inventory cannot certify that those guards cover valid Python.
    Macro expansion, qualified paths other than Self:: and dynamic targets remain unproven;
    lexical discovery is not a Rust grammar or semantic support authority.
    """
    family = root / "runtime/molt-backend-rust/src/rust"
    full_backend = (root / "runtime/molt-backend-rust/Cargo.toml").is_file()
    methods: dict[str, tuple[str, int, str]] = {}
    raw_bodies: dict[str, str] = {}
    body_starts: dict[str, int] = {}
    ambiguous: set[str] = set()
    referenced: set[str] = set()
    reference_locations: dict[str, str] = {}
    findings: list[Finding] = []
    family_sources: list[str] = []
    root_code = ""
    paths = [root / "runtime/molt-backend-rust/src/rust.rs", *family.rglob("*.rs")]
    test_paths = _rust_test_source_paths(root)
    for path in sorted(path for path in paths if path.is_file()):
        if _resolved(path) in test_paths:
            continue
        text = _source_text(path)
        code = mask_rust_comments_and_strings(mask_rust_test_items(text))
        family_sources.append(code)
        if path == root / "runtime/molt-backend-rust/src/rust.rs":
            root_code = code
        rel = path.relative_to(root).as_posix()
        for call in _rust_self_reference_matches(code):
            if call["instance"] is not None and call["call"] is not None:
                referenced.add(call["name"])
                line = code.count("\n", 0, call.start()) + 1
                reference_locations.setdefault(call["name"], f"{rel}:{line}")
        for match in re.finditer(
            rf"\bfn{_RUST_WS}+(?:r#)?([A-Za-z_]\w*){_RUST_WS}*(?:<[^{{}};]*>{_RUST_WS}*)?\(",
            code,
        ):
            line = code.count("\n", 0, match.start()) + 1
            opening = code.find("{", match.end())
            semicolon = code.find(";", match.end())
            if opening < 0 or 0 <= semicolon < opening:
                continue
            end, _ = _balanced_block(code, opening)
            body = code[opening + 1 : end - 1]
            name = match[1]
            raw_bodies[name] = text[opening + 1 : end - 1]
            body_starts[name] = opening + 1
            if name in methods:
                # Ambiguous names cannot establish a forwarding proof.
                ambiguous.add(name)
                methods[name] = (rel, line, "")
            else:
                methods[name] = (rel, line, body)
    for name in sorted(ambiguous & referenced):
        rel, line, _ = methods[name]
        findings.append(
            Finding(
                probe="rust_backend_rejection_applicability",
                severity="high",
                title="Ambiguous Rust rejection-family method identity",
                location=f"{rel}:{line}",
                detail=f"{name}: duplicate method definitions require owner/type resolution",
                suggested_action="resolve method ownership before accepting rejection-path coverage",
                class_retired="rust-backend-rejection-applicability",
                metric=0,
            )
        )
    for name in sorted(referenced - methods.keys()):
        if name == "emit_unsupported_op" and not full_backend:
            continue
        findings.append(
            Finding(
                probe="rust_backend_rejection_applicability",
                severity="medium",
                title="Unresolved Rust lowering helper target",
                location=reference_locations[name],
                detail=f"{name}: called target absent from lexical method inventory",
                suggested_action="resolve owner/signature/macro target before accepting lowering coverage",
                class_retired="rust-backend-rejection-applicability",
                metric=0,
            )
        )
    # Bind discovery to the refusal accumulator consumed by compile_checked,
    # rather than only the historical recording helper spelling. Partial lexical
    # fixtures retain the known primitive; real backend trees require evidence.
    consumer = methods.get("compile_checked", ("", 0, ""))[2]
    refusal_fields = {match[1] for match in _rust_checked_refusal_guards(consumer)}
    recording = set()
    for name, (rel, line, body) in methods.items():
        if any(
            re.search(
                rf"self{_RUST_WS}*\.{_RUST_WS}*{re.escape(field)}{_RUST_WS}*\.{_RUST_WS}*(?:push|extend|insert){_RUST_WS}*\(",
                body,
            )
            for field in refusal_fields
        ):
            recording.add(name)
            # A recording write is diagnostic evidence, not a proof that all
            # operands reject. This includes writes moved directly into emitters.
            if _rust_terminal_refusal_push(body, refusal_fields):
                continue
            findings.append(
                Finding(
                    probe="rust_backend_rejection_applicability",
                    severity="medium",
                    title="Rust refusal accumulator write needs applicability evidence",
                    location=f"{rel}:{line}",
                    detail=f"{name}: writes publication refusal state",
                    suggested_action="prove recording path admission and valid-input coverage",
                    class_retired="rust-backend-rejection-applicability",
                    metric=0,
                )
            )
    exposed = set()
    for name, (rel, line, body) in methods.items():
        if name in recording or name in {"compile_checked", "emit_source"}:
            continue
        if any(
            re.search(rf"\.{_RUST_WS}*{re.escape(field)}\b", body)
            for field in refusal_fields
        ):
            exposed.add(name)
            findings.append(
                Finding(
                    probe="rust_backend_rejection_applicability",
                    severity="medium",
                    title="Rust refusal accumulator access needs ownership evidence",
                    location=f"{rel}:{line}",
                    detail=f"{name}: alias/receiver/mutator not resolved",
                    suggested_action="resolve accumulator exposure and downstream rejection before accepting lowering coverage",
                    class_retired="rust-backend-rejection-applicability",
                    metric=0,
                )
            )
    protocol_proven = full_backend and _rust_refusal_protocol_proven(
        methods,
        refusal_fields,
        recording,
        "\n".join(family_sources),
        root_code,
        set(methods) - ambiguous,
    )
    if full_backend and (not refusal_fields or (not recording and not protocol_proven)):
        findings.append(
            Finding(
                probe="rust_backend_rejection_applicability",
                severity="high",
                title="Rust rejection protocol authority unresolved",
                location="runtime/molt-backend-rust/src/rust.rs:1",
                detail="publication refusal consumer or recording producer absent from lexical inventory",
                suggested_action="resolve rejection-state producer and publication consumer before acceptance",
                class_retired="rust-backend-rejection-applicability",
                metric=0,
            )
        )
    rejected = {
        name
        for name in recording
        if _rust_terminal_refusal_push(methods[name][2], refusal_fields)
    }
    if full_backend and not protocol_proven:
        rejected.clear()
        findings.append(
            Finding(
                probe="rust_backend_rejection_applicability",
                severity="high",
                title="Rust refusal publication protocol needs downstream evidence",
                location="runtime/molt-backend-rust/src/rust.rs:1",
                detail="recording is not proven to reach checked rejection without reset/recovery",
                suggested_action="prove accumulator ownership, reset ordering and checked Err publication boundary",
                class_retired="rust-backend-rejection-applicability",
                metric=0,
            )
        )
    if not full_backend and not recording:
        rejected = {"emit_unsupported_op"}
    while True:
        added = {
            name
            for name, (_, _, body) in methods.items()
            if _rust_single_self_call(body) in rejected
        } - rejected
        if not added:
            break
        rejected.update(added)
    domain = proven_admitted_wire_domain(root, consumer) if protocol_proven else None
    projection = (
        proven_branch_projection(root, raw_bodies, domain)
        if domain is not None
        else None
    )
    # One edge graph owns both reachability directions. An unreachable refusal
    # must not leak back into every wrapper after its forward edge was pruned.
    edges = {
        name: {
            reference["name"]
            for reference in _rust_self_reference_matches(body)
            if projection is None or not projection.excludes(name, reference.start())
        }
        for name, (_, _, body) in methods.items()
    }
    admitted_reachable: set[str] | None = None
    if projection is not None:
        admitted_reachable = set()
        pending = ["compile_checked"]
        while pending:
            caller = pending.pop()
            if caller in admitted_reachable:
                continue
            admitted_reachable.add(caller)
            pending.extend(edges.get(caller, ()))

    # Reachability grants only an applicability obligation, never a claim that
    # every path or valid operand is rejected. Keep upstream mixed callers visible
    # when a rejection moves laterally into a helper or through multiple helpers.
    reachable = set(rejected) | recording | exposed
    while True:
        added = {
            name for name, (_, _, body) in methods.items() if edges[name] & reachable
        } - reachable
        if not added:
            break
        reachable.update(added)
    for name, (rel, line, body) in sorted(methods.items()):
        if name in rejected or name not in reachable:
            continue
        if admitted_reachable is not None and name not in admitted_reachable:
            continue
        findings.append(
            Finding(
                probe="rust_backend_rejection_applicability",
                severity="medium",
                title="Rust rejection guard needs applicability evidence",
                location=f"{rel}:{line}",
                detail=f"{name}: may-reject lowering body; not a globally unsupported opcode claim",
                suggested_action="trace valid-input admission and alternate lowering; retain adversarial operand/arity/flow coverage",
                class_retired="rust-backend-rejection-applicability",
                metric=0,
            )
        )
    for name, (rel, line, body) in sorted(methods.items()):
        for callee in sorted(
            recording
            | (
                {"emit_unsupported_op"}
                if full_backend and "emit_unsupported_op" not in methods
                else set()
            )
        ):
            # Free/qualified recording calls cannot inherit an instance-method
            # proof. Keep them visible without pretending owner/type resolution.
            for match in re.finditer(
                rf"(?<![\w.]){re.escape(callee)}{_RUST_WS}*\(", body
            ):
                if any(
                    reference["name"] == callee
                    and reference.start() <= match.start() < reference.end()
                    for reference in _rust_self_reference_matches(body)
                ):
                    continue
                findings.append(
                    Finding(
                        probe="rust_backend_rejection_applicability",
                        severity="medium",
                        title="Rust recording call needs receiver evidence",
                        location=f"{rel}:{line}",
                        detail=f"{name}: non-instance recording call {callee}",
                        suggested_action="resolve receiver and recording-state ownership",
                        class_retired="rust-backend-rejection-applicability",
                        metric=0,
                    )
                )
    if projection is not None:
        offset = body_starts.get("emit_op", 0)
        projection.dispatch = [
            (start + offset, end + offset) for start, end in projection.dispatch
        ]
    return rejected, findings, projection


@_audit_probe
def probe_rust_backend_lowering_gaps(root: Path) -> list[Finding]:
    """Backend ops that fail closed because Rust lowering is not implemented.

    These are better than fake output, but still first-class compiler debt: an
    op has reached Rust source emission and lacks a real lowering primitive.
    """
    rel = Path("runtime/molt-backend-rust/src/rust/op_emitter.rs")
    path = root / rel
    if not path.is_file():
        relocated = Path("runtime/molt-backend-rust/src/rust/op_emitter/mod.rs")
        if (root / relocated).is_file():
            rel, path = relocated, root / relocated
        elif (root / "runtime/molt-backend-rust/Cargo.toml").is_file():
            return [
                Finding(
                    probe="rust_backend_rejection_applicability",
                    severity="high",
                    title="Rust dispatch source authority unresolved",
                    location=f"{rel.as_posix()}:1",
                    detail="neither file-module nor directory-module dispatcher is present",
                    suggested_action="resolve production dispatcher before accepting lowering coverage",
                    class_retired="rust-backend-rejection-applicability",
                    metric=0,
                )
            ]
    try:
        text = _source_text(path)
    except OSError:
        return []
    lines = mask_rust_comments_and_strings(text, preserve_literals=True).split("\n")
    code_lines = mask_rust_comments_and_strings(text).split("\n")
    rejected, applicability, projection = _rust_rejection_family(root)
    excluded = projection.dispatch if projection is not None else []
    findings: list[Finding] = []
    seen: set[tuple[str, str]] = set()
    code = "\n".join(code_lines)
    for call in _rust_self_reference_matches(code):
        if call["name"] not in rejected:
            continue
        if any(start <= call.start() < end for start, end in excluded):
            continue
        line_no = code.count("\n", 0, call.start()) + 1
        if call["instance"] is None or call["call"] is None:
            findings.append(
                Finding(
                    probe="rust_backend_rejection_applicability",
                    severity="medium",
                    title="Rust rejection reference needs target/applicability evidence",
                    location=f"{rel.as_posix()}:{line_no}",
                    detail=f"{call['name']}: named associated/method reference; not a proven invocation",
                    suggested_action="resolve alias/receiver/call target before accepting lowering coverage",
                    class_retired="rust-backend-rejection-applicability",
                    metric=0,
                )
            )
            continue
        call_block = "\n".join(lines[line_no - 1 : min(len(lines), line_no + 8)])
        if "unexpectedly produces output" in call_block:
            continue
        arm_text = _rust_match_arm_text_before(
            lines, line_no - 1, call_end_offset=call.end()
        )
        marker, count = _rust_backend_lowering_gap_marker(arm_text)
        key = (arm_text, marker)
        if key in seen:
            continue
        seen.add(key)
        findings.append(
            Finding(
                probe="rust_backend_lowering_gap",
                severity="high" if count >= 5 else "medium",
                title=f"{count} Rust dispatch-path lowering gap(s)",
                location=f"{rel.as_posix()}:{line_no}",
                detail=f"L{line_no}:{marker}",
                suggested_action=(
                    "lower these ops through real Rust backend/runtime primitives "
                    "or keep them fail-closed behind an explicit unsupported-backend "
                    "contract until the lowering is implemented"
                ),
                class_retired="rust-backend-lowering-gap",
                metric=count,
            )
        )
    return findings + applicability


_NATIVE_SCALAR_PLAN_SURFACE_REL = (
    "runtime/molt-backend-native/src/native_backend/function_compiler"
)
_NATIVE_SCALAR_PLAN_FORBIDDEN = {
    r"\bbool_primary_vars\b": "raw-bool membership cloned out of ScalarRepresentationPlan",
    r"\bfloat_primary_vars\b": "raw-f64 membership cloned out of ScalarRepresentationPlan",
    r"\bint_carriers_plan\b": "legacy plan alias beside ScalarRepresentationPlan",
    r"\bprimary_name_sets\s*\(": "native backend cloned primary-name sets instead of plan predicates",
    r"\bint_like_vars\b": "semantic int membership cloned out of ScalarRepresentationPlan",
    r"\bbool_like_vars\b": "semantic bool membership cloned out of ScalarRepresentationPlan",
    r"\bfloat_like_vars\b": "semantic float membership cloned out of ScalarRepresentationPlan",
    r"\bstr_like_vars\b": "semantic str membership cloned out of ScalarRepresentationPlan",
    r"\bnone_like_vars\b": "semantic None membership cloned out of ScalarRepresentationPlan",
}


@_audit_probe
def probe_native_scalar_plan_authority(root: Path) -> list[Finding]:
    """Native scalar lowering must consume ScalarRepresentationPlan directly.

    The native backend used to thread bool/float carrier BTreeSets, semantic
    scalar "like" sets, plus an int-carrier plan alias through every extracted
    handler. That split made raw scalar representation and semantic scalar
    classification a multi-authority contract. This probe keeps the hot lowering
    path optimized around one plan: handlers may ask plan predicates, but may
    not clone carrier or scalar-kind membership into local side sets.
    """
    if _source_scope(root) is not None:
        targets = []
        surface_prefix = f"{_NATIVE_SCALAR_PLAN_SURFACE_REL}/"
        for path in _iter_source_files(root, (".rs",)):
            rel = path.relative_to(root).as_posix()
            if (
                rel
                == "runtime/molt-backend-native/src/native_backend/function_compiler.rs"
                or rel.startswith(surface_prefix)
            ):
                targets.append(path)
    else:
        targets = [
            root
            / "runtime/molt-backend-native/src/native_backend/function_compiler.rs",
        ]
        base = root / _NATIVE_SCALAR_PLAN_SURFACE_REL
        if base.is_dir():
            targets.extend(_iter_pruned_files(base, root, (".rs",)))

    findings: list[Finding] = []
    for path in targets:
        if not path.is_file():
            continue
        try:
            text = _source_text(path)
        except OSError:
            continue
        rel = path.relative_to(root).as_posix()
        for pattern, detail in _NATIVE_SCALAR_PLAN_FORBIDDEN.items():
            hits = list(re.finditer(pattern, text))
            if not hits:
                continue
            first_line = _line_of_offset(text, hits[0].start())
            findings.append(
                Finding(
                    probe="native_scalar_plan_authority",
                    severity="high",
                    title=f"{len(hits)} forbidden native scalar-plan clone(s)",
                    location=f"{rel}:{first_line}",
                    detail=detail,
                    suggested_action=(
                        "route native scalar membership through "
                        "ScalarRepresentationPlan predicates such as "
                        "is_raw_int_carrier_name/is_bool_unboxed/is_float_unboxed "
                        "and name_is_* scalar-kind queries"
                    ),
                    class_retired="native-scalar-representation-drift",
                    metric=float(len(hits)),
                )
            )
    return findings


_REPR_NAME_SCALAR_AUTHORITY_REL = "runtime/molt-tir/src/representation_plan.rs"
_REPR_NAME_SCALAR_FORBIDDEN = {
    r"\bbool_primary_names\b": "raw-bool membership stored beside repr_by_name",
    r"\bfloat_primary_names\b": "raw-f64 membership stored beside repr_by_name",
}


@_audit_probe
def probe_repr_name_scalar_authority(root: Path) -> list[Finding]:
    """Name-keyed scalar carriers must have one representation-map authority.

    `ScalarRepresentationPlan::repr_by_name` owns the native name-keyed carrier
    lattice for int, bool, and f64. Raw-bool/raw-f64 candidate computation may
    still exist, but storing the results in side sets re-creates the drift lane
    this authority cut removed.
    """
    path = root / _REPR_NAME_SCALAR_AUTHORITY_REL
    if not path.is_file():
        return []
    try:
        text = _source_text(path)
    except OSError:
        return []

    findings: list[Finding] = []
    for pattern, detail in _REPR_NAME_SCALAR_FORBIDDEN.items():
        hits = list(re.finditer(pattern, text))
        if not hits:
            continue
        first_line = _line_of_offset(text, hits[0].start())
        findings.append(
            Finding(
                probe="repr_name_scalar_authority",
                severity="high",
                title=f"{len(hits)} forbidden repr-by-name scalar side store(s)",
                location=f"{_REPR_NAME_SCALAR_AUTHORITY_REL}:{first_line}",
                detail=detail,
                suggested_action=(
                    "store native bool/f64 carrier eligibility in repr_by_name "
                    "as Repr::Bool/Repr::FloatUnboxed and derive views from that map"
                ),
                class_retired="repr-by-name-scalar-representation-drift",
                metric=float(len(hits)),
            )
        )
    return findings


# Predicate-name shapes that classify a SPECIFIC opcode-semantic property.
# Deliberately narrow (no bare `escape`/`classify`) so string-escaping helpers
# and generic classifiers do not masquerade as duplicate opcode authorities.
_PREDICATE_RE = re.compile(
    r"\bfn\s+([a-z0-9_]*(?:may_throw|side_effect|is_pure|mints_fresh|"
    r"is_inert|no_heap_move|is_barrier|operand_consume|consumes_operand|"
    r"is_inlinab|may_alias|may_escape|opcode_escapes|is_leaf_call)[a-z0-9_]*)\s*\("
)
# A predicate only counts as an OPCODE authority if its body actually inspects an
# opcode/kind — guards against same-named-but-unrelated functions.
_OPCODE_CONTEXT_RE = re.compile(r"OpCode::|\bopcode\b|\.kind\b|_original_kind")


@_audit_probe
def probe_duplicate_authorities(root: Path) -> list[Finding]:
    """Council Q1: the same opcode-semantic property decided in more than one
    file. Groups opcode-classifying predicate functions (whose body inspects an
    opcode/kind) by property and flags properties spread across ≥2 files."""
    by_keyword: dict[str, list[str]] = {}
    keyword_map = {
        "may_throw": "may_throw",
        "side_effect": "side_effecting",
        "is_pure": "purity",
        "mints_fresh": "fresh_value_ownership",
        "is_inert": "inert_marker",
        "no_heap_move": "no_heap_move",
        "is_barrier": "barrier",
        "operand_consume": "operand_consume",
        "consumes_operand": "operand_consume",
        "is_inlinab": "inlinability",
        "may_alias": "aliasing",
        "may_escape": "escape_analysis",
        "opcode_escapes": "escape_analysis",
        "is_leaf_call": "leaf",
    }
    test_paths = _rust_test_source_paths(root)
    for path in _iter_source_files(root, (".rs",)):
        if _is_generated(path, root) or _resolved(path) in test_paths:
            continue
        rel_path = path.relative_to(root)
        try:
            text = _source_text(path)
        except OSError:
            continue
        rel = rel_path.as_posix()
        text = mask_rust_comments_and_strings(mask_rust_test_items(text))
        for m in _PREDICATE_RE.finditer(text):
            fn = m.group(1)
            # require opcode/kind context in the function body window
            window = text[m.end() : m.end() + 800]
            if not _OPCODE_CONTEXT_RE.search(window):
                continue
            # A duplicate AUTHORITY hand-classifies with literals (`matches!(...)`
            # or `OpCode::Variant` arms). A predicate that merely DELEGATES to the
            # single generated authority (reads a `*_table`, calls another
            # predicate) is a CONSUMER, not a second authority — counting it would
            # report drift that does not exist (the op_kinds.toml registry remains
            # the sole source of truth). Discovery may be heuristic; this keeps it
            # from manufacturing false positives the ratchet would then enshrine.
            if "matches!(" not in window and "OpCode::" not in window:
                continue
            for needle, prop in keyword_map.items():
                if needle in fn:
                    line = text.count("\n", 0, m.start()) + 1
                    by_keyword.setdefault(prop, []).append(f"{rel}:{line} ({fn})")
                    break
    findings: list[Finding] = []
    for prop, sites in sorted(by_keyword.items()):
        files = {s.split(":")[0] for s in sites}
        if len(files) < 2:
            continue
        findings.append(
            Finding(
                probe="duplicate_authority",
                severity="medium" if len(files) >= 3 else "low",
                title=f"property `{prop}` classified in {len(files)} files",
                location="; ".join(sorted(files)),
                detail="sites: " + " | ".join(sites[:6]),
                suggested_action=f"make op_kinds.toml the sole authority for `{prop}` "
                "and have every site read the generated predicate",
                class_retired="duplicate-semantic-authority-drift",
                metric=len(files),
            )
        )
    return findings


def _count_enum_variants(rust_text: str, enum_name: str) -> set[str]:
    """Robustly extract variant identifiers from `pub enum <name> { .. }`.

    Splits the enum body on TOP-LEVEL commas (depth 0 within the body, so commas
    inside `Variant(a, b)` or `Variant { x, y }` payloads do not split), then
    takes the leading CamelCase identifier of each segment after stripping
    attributes/doc-comments. Robust to tuple/struct variants and `= discriminant`.
    """
    code = mask_rust_comments_and_strings(rust_text)
    m = re.search(rf"\benum\s+{re.escape(enum_name)}\s*(?:<[^{{}};]*>)?\s*\{{", code)
    if not m:
        return set()
    _, block = _balanced_block(rust_text, m.end() - 1)
    body = mask_rust_comments_and_strings(block)[1:-1]  # drop the outer { }
    segments: list[str] = []
    depth = 0
    seg_start = 0
    for i, c in enumerate(body):
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        elif c == "," and depth == 0:
            segments.append(body[seg_start:i])
            seg_start = i + 1
    segments.append(body[seg_start:])
    variants: set[str] = set()
    for seg in segments:
        s = re.sub(r"#\[[^\]]*\]", "", seg).strip()  # strip attributes
        vm = re.match(r"([A-Z][A-Za-z0-9_]*)", s)
        if vm:
            variants.add(vm.group(1))
    return variants


@_audit_probe
def probe_registry_reconciliation(root: Path) -> list[Finding]:
    """Confidence (INFO) check: the [[opcode]] effect-oracle table is rendered as
    an EXHAUSTIVE rustc match, so coverage is compiler-enforced — this only
    reports parser-agreement so a drift in the *parser* (not the data) surfaces."""
    ops_rs = root / "runtime/molt-ir/src/tir/ops.rs"
    toml_path = root / "runtime/molt-ir/src/tir/op_kinds.toml"
    findings: list[Finding] = []
    if not ops_rs.is_file() or not toml_path.is_file():
        return findings
    variants = _count_enum_variants(
        ops_rs.read_text(errors="replace", encoding="utf-8"), "OpCode"
    )
    toml_text = _source_text(toml_path)
    opcode_rows = set(
        re.findall(r'^\s*opcode\s*=\s*"([A-Za-z0-9_]+)"', toml_text, re.MULTILINE)
    )
    # Fallback: rows may key by [[opcode]] then name field; also accept name=.
    if not opcode_rows:
        opcode_rows = set(
            re.findall(r'^\s*name\s*=\s*"([A-Za-z0-9_]+)"', toml_text, re.MULTILINE)
        )
    findings.append(
        Finding(
            probe="registry_reconciliation",
            severity="info",
            title=f"OpCode variants={len(variants)} · [[opcode]] rows≈{len(opcode_rows)}",
            location="runtime/molt-ir/src/tir/{ops.rs,op_kinds.toml}",
            detail="effect oracle is an exhaustive (no-wildcard) match — coverage is "
            "rustc-enforced; this line is parser confidence only, not a gate",
            suggested_action="no action unless a NEW non-exhaustive opcode classifier "
            "appears (probe semantic_fallthrough catches those)",
            class_retired="",
            metric=0,
        )
    )
    return findings


# --- process-wide stdlib patches in tests (HF-43) -------------------------

# Shared stdlib modules whose attributes every caller in a pytest process reads,
# the suite-lease and sentinel threads among them.
_SHARED_STDLIB_MODULES = frozenset(
    {"os", "sys", "platform", "subprocess", "hashlib", "shutil", "threading", "time"}
)
# Process-wide by nature: the interpreter itself reads these, so a test that
# changes them means to change them for the process.
_PROCESS_WIDE_ATTRIBUTES = frozenset(
    {
        ("sys", "argv"),
        ("sys", "path"),
        ("sys", "stdin"),
        ("sys", "stdout"),
        ("sys", "stderr"),
        ("sys", "modules"),
        ("os", "environ"),
    }
)
_PROCESS_WIDE_PATCH_PREFILTER = re.compile(
    r"setattr\(\s*(?:\"[\w.]*\b(?:"
    + "|".join(sorted(_SHARED_STDLIB_MODULES))
    + r")\.|[\w.]*\b(?:"
    + "|".join(sorted(_SHARED_STDLIB_MODULES))
    + r")\s*,)"
)


def _process_wide_patch_lines(tree: ast.AST) -> list[int]:
    """Lines of ``monkeypatch.setattr`` calls that rebind a shared stdlib module.

    A patch of ``module.os`` after the same function installed a module view
    for ``module`` and ``os`` rebinds that view, not the process-wide module.
    """
    lines: list[int] = []
    for function in ast.walk(tree):
        if not isinstance(function, (ast.FunctionDef, ast.AsyncFunctionDef)):
            continue
        views: set[tuple[str, str]] = set()
        calls = sorted(
            (node for node in ast.walk(function) if isinstance(node, ast.Call)),
            key=lambda node: (node.lineno, node.col_offset),
        )
        for call in calls:
            func = call.func
            name = func.id if isinstance(func, ast.Name) else None
            if name == "install_module_view" and len(call.args) >= 4:
                stdlib = call.args[1]
                if isinstance(stdlib, ast.Constant) and isinstance(stdlib.value, str):
                    views.update(
                        (ast.unparse(module), stdlib.value) for module in call.args[3:]
                    )
                continue
            if name == "install_module_os_view":
                views.update((ast.unparse(module), "os") for module in call.args[1:])
                continue
            if not (
                isinstance(func, ast.Attribute)
                and func.attr == "setattr"
                and isinstance(func.value, ast.Name)
                and func.value.id == "monkeypatch"
                and call.args
            ):
                continue
            target = call.args[0]
            if isinstance(target, ast.Constant) and isinstance(target.value, str):
                parts = target.value.split(".")
                if len(parts) >= 2 and parts[-2] in _SHARED_STDLIB_MODULES:
                    if (parts[-2], parts[-1]) not in _PROCESS_WIDE_ATTRIBUTES:
                        lines.append(call.lineno)
                continue
            if len(call.args) < 2 or not (
                isinstance(call.args[1], ast.Constant)
                and isinstance(call.args[1].value, str)
            ):
                continue
            attribute = call.args[1].value
            if isinstance(target, ast.Name) and target.id in _SHARED_STDLIB_MODULES:
                stdlib_name = target.id
                owner = None
            elif (
                isinstance(target, ast.Attribute)
                and target.attr in _SHARED_STDLIB_MODULES
            ):
                stdlib_name = target.attr
                owner = ast.unparse(target.value)
            else:
                continue
            if (stdlib_name, attribute) in _PROCESS_WIDE_ATTRIBUTES:
                continue
            if owner is not None and (owner, stdlib_name) in views:
                continue
            lines.append(call.lineno)
    return sorted(set(lines))


def _iter_test_files(root: Path) -> list[Path]:
    base = root / "tests"
    scope = _source_scope(root)
    if scope is not None:
        return [
            root / rel
            for rel in sorted(scope)
            if rel.startswith("tests/")
            and rel.endswith(".py")
            and (root / rel).is_file()
            and not _is_excluded(root / rel, root)
        ]
    if not base.is_dir():
        return []
    return sorted(
        _iter_pruned_files(base, root, (".py",)),
        key=lambda path: path.relative_to(root).as_posix(),
    )


@_audit_probe
def probe_process_wide_test_patches(root: Path) -> list[Finding]:
    """Tests that fake a shared stdlib attribute for the whole process.

    ``monkeypatch.setattr(module.os, "getpid", fake)`` changes ``os.getpid``
    for every thread of the pytest process while the test runs. A module view
    (``tests/process_guard_common.install_module_view``) confines the fake to
    the modules under test. Reported per file, ratcheted in aggregate.
    """
    findings: list[Finding] = []
    for path in _iter_test_files(root):
        try:
            text = _source_text(path)
        except OSError:
            continue
        if not _PROCESS_WIDE_PATCH_PREFILTER.search(text):
            continue
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue
        lines = _process_wide_patch_lines(tree)
        if not lines:
            continue
        rel = path.relative_to(root).as_posix()
        findings.append(
            Finding(
                probe="process_wide_test_patch",
                severity="medium" if len(lines) >= 10 else "low",
                title=f"{len(lines)} process-wide stdlib patches",
                location=f"{rel}:{lines[0]}",
                detail=", ".join(f"L{line}" for line in lines[:8]),
                suggested_action="install a module view over every module that "
                "reads the attribute (tests/process_guard_common.install_module_view)",
                class_retired="process-wide-test-fake",
                metric=len(lines),
            )
        )
    return findings


_BUILD_SKIP_PREFILTER = re.compile(r"\.skip\(")
# A skip message that reports a failed build, as opposed to a missing tool
# ("cargo is required for backend compilation").
_BUILD_FAILURE_SKIP_MESSAGE = re.compile(
    r"\b(?:builds?|compil\w*)\b.*\b(?:fail\w*|error)\b|killed during compilation",
    re.IGNORECASE,
)


def _skip_message_text(node: ast.expr) -> str | None:
    """The literal text of a skip message, with f-string fields dropped."""
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value
    if isinstance(node, ast.JoinedStr):
        return "".join(
            part.value
            for part in node.values
            if isinstance(part, ast.Constant) and isinstance(part.value, str)
        )
    return None


def _build_failure_skip_lines(tree: ast.AST) -> list[int]:
    lines: list[int] = []
    for node in ast.walk(tree):
        if not (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and node.func.attr == "skip"
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == "pytest"
            and node.args
        ):
            continue
        text = _skip_message_text(node.args[0])
        if text is not None and _BUILD_FAILURE_SKIP_MESSAGE.search(text):
            lines.append(node.lineno)
    return sorted(lines)


@_audit_probe
def probe_build_failure_test_skips(root: Path) -> list[Finding]:
    """Tests that turn a failed build into a skip.

    ``pytest.skip(f"Compilation failed: ...")`` makes a compiler regression
    read as green. A missing tool is a capability skip and does not count.
    Reported per file, ratcheted in aggregate.
    """
    findings: list[Finding] = []
    for path in _iter_test_files(root):
        try:
            text = _source_text(path)
        except OSError:
            continue
        if not _BUILD_SKIP_PREFILTER.search(text):
            continue
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue
        lines = _build_failure_skip_lines(tree)
        if not lines:
            continue
        rel = path.relative_to(root).as_posix()
        findings.append(
            Finding(
                probe="build_failure_test_skip",
                severity="medium",
                title=f"{len(lines)} skips on a failed build",
                location=f"{rel}:{lines[0]}",
                detail=", ".join(f"L{line}" for line in lines[:8]),
                suggested_action="fail on a build error; name a program that is "
                "unsupported on purpose in an expected-failure list",
                class_retired="fail-open-build-skip",
                metric=len(lines),
            )
        )
    return findings


def _is_type_checking_guard(test: ast.expr) -> bool:
    return (isinstance(test, ast.Name) and test.id == "TYPE_CHECKING") or (
        isinstance(test, ast.Attribute) and test.attr == "TYPE_CHECKING"
    )


def _module_scope_statements(body: list[ast.stmt]) -> Iterator[ast.stmt]:
    """Statements that run at module scope, through guards but not into defs."""

    for node in body:
        yield node
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            continue
        if isinstance(node, ast.If) and _is_type_checking_guard(node.test):
            # Type-checking declarations never run.
            yield from _module_scope_statements(node.orelse)
            continue
        for field in ("body", "orelse", "finalbody"):
            nested = getattr(node, field, None)
            if isinstance(nested, list):
                yield from _module_scope_statements(nested)
        for handler in getattr(node, "handlers", ()):
            yield from _module_scope_statements(handler.body)
        for case in getattr(node, "cases", ()):
            yield from _module_scope_statements(case.body)


def _binds_raw_intrinsic_name(node: ast.stmt) -> bool:
    if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
        return node.name.startswith("molt_")
    if isinstance(node, (ast.Import, ast.ImportFrom)):
        return any(
            (alias.asname or alias.name.split(".")[0]).startswith("molt_")
            for alias in node.names
        )
    if isinstance(node, ast.Assign):
        targets: list[ast.expr] = list(node.targets)
    elif isinstance(node, (ast.AnnAssign, ast.AugAssign)):
        targets = [node.target]
    elif isinstance(node, ast.Expr) and isinstance(node.value, ast.Call):
        # ``require_intrinsic("molt_x", globals())`` binds the raw name.
        call = node.value
        return (
            len(call.args) >= 2
            and isinstance(call.args[0], ast.Constant)
            and str(call.args[0].value).startswith("molt_")
            and isinstance(call.args[1], ast.Call)
            and getattr(call.args[1].func, "id", None) == "globals"
        )
    else:
        return False
    return any(
        isinstance(name, ast.Name) and name.id.startswith("molt_")
        for target in targets
        for name in ast.walk(target)
    )


def probe_stdlib_raw_intrinsic_names(root: Path) -> list[Finding]:
    """Stdlib modules that leave a raw ``molt_*`` intrinsic name bound.

    A name bound at module scope is a public attribute of the module, which
    CPython's module does not have. Bind intrinsics to private ``_MOLT_*``
    names and release the resolver helper. Reported per file, ratcheted in
    aggregate.
    """
    findings: list[Finding] = []
    stdlib = root / "src" / "molt" / "stdlib"
    for path in _iter_source_files(root, (".py",)):
        if not path.is_relative_to(stdlib):
            continue
        try:
            text = _source_text(path)
        except OSError:
            continue
        if "molt_" not in text:
            continue
        try:
            tree = ast.parse(text)
        except SyntaxError:
            continue
        lines = [
            node.lineno
            for node in _module_scope_statements(tree.body)
            if _binds_raw_intrinsic_name(node)
        ]
        if not lines:
            continue
        rel = path.relative_to(root).as_posix()
        findings.append(
            Finding(
                probe="stdlib_raw_intrinsic_name",
                severity="medium" if len(lines) >= 10 else "low",
                title=f"{len(lines)} raw intrinsic names bound at module scope",
                location=f"{rel}:{lines[0]}",
                detail=", ".join(f"L{line}" for line in lines[:8]),
                suggested_action="bind each intrinsic to a private _MOLT_* name",
                class_retired="stdlib-namespace-leak",
                metric=len(lines),
            )
        )
    return findings


PROBES = (
    probe_semantic_fallthroughs,
    probe_large_source_files,
    probe_kitchen_sink_files,
    probe_undecomposed_god_files,
    probe_debt_markers,
    probe_python_stub_surfaces,
    probe_rust_stub_surfaces,
    probe_rust_backend_lowering_gaps,
    probe_native_scalar_plan_authority,
    probe_repr_name_scalar_authority,
    probe_duplicate_authorities,
    probe_registry_reconciliation,
    probe_process_wide_test_patches,
    probe_build_failure_test_skips,
    probe_stdlib_raw_intrinsic_names,
)


# A worker costs about 0.2 s to start; a file costs a few ms to project. At 128
# files a worker, start-up stays under a third of its work.
_PROJECTION_FILES_PER_WORKER = 128
# One worker holds a slice of the Rust sources and their projections.
_PROJECTION_BYTES_PER_WORKER = 256 * 1024 * 1024
_PROJECTION_MEMORY_HEADROOM_BYTES = 1024 * 1024 * 1024


def _prewarm_rust_projections(root: Path) -> None:
    """Project every Rust source in parallel before the probes read them.

    The probes project every Rust file under the source roots (test ownership
    ignores a --path selection), so the whole set is warmed once.
    """
    from molt.dx import _memory_bounded_worker_count

    paths = [
        path
        for sub in _SOURCE_ROOTS
        if (root / sub).is_dir()
        for path in _iter_pruned_files(root / sub, root, (".rs",))
    ]
    workers = min(
        len(paths) // _PROJECTION_FILES_PER_WORKER,
        _memory_bounded_worker_count(
            bytes_per_worker=_PROJECTION_BYTES_PER_WORKER,
            headroom_bytes=_PROJECTION_MEMORY_HEADROOM_BYTES,
        ),
    )
    if workers > 1:
        prewarm_rust_item_projections(
            (_source_text(path) for path in paths), workers=workers
        )


def run_all(root: Path, path_scope: frozenset[str] | None = None) -> list[Finding]:
    findings: list[Finding] = []
    with audit_operation(root, path_scope):
        _prewarm_rust_projections(root)
        for probe in PROBES:
            if path_scope is not None and probe is probe_registry_reconciliation:
                continue
            findings.extend(probe(root))
    findings.sort(key=lambda f: f.sort_key())
    return findings


# --- ratchet metrics (the --check gate) -----------------------------------


def _large_region_count_from_title(title: str) -> int:
    m = re.match(r"(\d+)\s+large top-level regions", title)
    return int(m.group(1)) if m else 0


def ratchet_metrics(findings: list[Finding]) -> dict[str, float]:
    """Aggregate scalars that may only improve (decrease). CI fails on regress.

    Lowering paths and unresolved applicability are separate metrics. The latter
    counts missing target/applicability proof, not unsupported Python operations;
    moving rejection behind a mixed body cannot silently turn the gate green."""
    sem = [f for f in findings if f.probe == "semantic_fallthrough"]
    match_cls = [f for f in sem if f.title.startswith("hand-classified")]
    handsets = [f for f in sem if f.title.startswith("`matches!`")]
    debt = [f for f in findings if f.probe == "debt_marker"]
    python_stubs = [f for f in findings if f.probe == "python_stub_surface"]
    rust_stubs = [f for f in findings if f.probe == "rust_stub_surface"]
    rust_backend_lowering_gaps = [
        f for f in findings if f.probe == "rust_backend_lowering_gap"
    ]
    kitchen_sink = [f for f in findings if f.probe == "kitchen_sink_file"]
    undecomposed = [f for f in findings if f.probe == "undecomposed_god_file"]
    native_scalar_plan = [
        f for f in findings if f.probe == "native_scalar_plan_authority"
    ]
    repr_name_scalar = [f for f in findings if f.probe == "repr_name_scalar_authority"]
    dup = [f for f in findings if f.probe == "duplicate_authority"]
    process_wide_patches = [f for f in findings if f.probe == "process_wide_test_patch"]
    build_failure_skips = [f for f in findings if f.probe == "build_failure_test_skip"]
    raw_intrinsic_names = [
        f for f in findings if f.probe == "stdlib_raw_intrinsic_name"
    ]
    kitchen_sink_files = float(len(kitchen_sink))
    max_kitchen_sink_structural_score = float(
        max((f.metric for f in kitchen_sink), default=0)
    )
    kitchen_sink_large_regions = float(
        sum(_large_region_count_from_title(f.title) for f in kitchen_sink)
    )
    undecomposed_god_files = float(len(undecomposed))
    max_undecomposed_file_lines = float(
        max((f.metric for f in undecomposed), default=0)
    )
    metrics = {
        # the hand-maintained-opcode-fact surface (match classifiers w/ silent default)
        "hand_classified_matches": float(len(match_cls)),
        # the high-priority subset: critical file AND large (≥6-opcode) hand-list
        "critical_hand_classifications": float(
            sum(1 for f in match_cls if f.severity == "high")
        ),
        # hand-maintained opcode SETS via matches! (≥3 opcodes) in any file
        "handset_classifications": float(len(handsets)),
        "debt_markers_total": float(sum(int(f.metric) for f in debt)),
        "python_stub_surfaces_total": float(sum(int(f.metric) for f in python_stubs)),
        "rust_stub_surfaces_total": float(sum(int(f.metric) for f in rust_stubs)),
        # Unresolved target/applicability proof debt is independently gated.
        # Moving a definite rejection into a mixed body cannot make --check green.
        "rust_backend_rejection_applicability_total": float(
            sum(f.probe == "rust_backend_rejection_applicability" for f in findings)
        ),
        "rust_backend_lowering_gaps_total": float(
            sum(int(f.metric) for f in rust_backend_lowering_gaps)
        ),
        "kitchen_sink_files": kitchen_sink_files,
        "max_kitchen_sink_structural_score": max_kitchen_sink_structural_score,
        "kitchen_sink_large_regions": kitchen_sink_large_regions,
        "undecomposed_god_files": undecomposed_god_files,
        "max_undecomposed_file_lines": max_undecomposed_file_lines,
        "native_scalar_plan_authority_violations": float(
            sum(int(f.metric) for f in native_scalar_plan)
        ),
        "repr_name_scalar_authority_violations": float(
            sum(int(f.metric) for f in repr_name_scalar)
        ),
        "duplicate_authorities": float(len(dup)),
        "process_wide_test_patches": float(
            sum(int(f.metric) for f in process_wide_patches)
        ),
        "build_failure_test_skips": float(
            sum(int(f.metric) for f in build_failure_skips)
        ),
        "stdlib_raw_intrinsic_bindings": float(
            sum(int(f.metric) for f in raw_intrinsic_names)
        ),
    }

    if set(metrics) != release_receipt.STRUCTURAL_AUDIT_METRICS:
        raise ValueError(
            "structural metric computation disagrees with canonical receipt schema"
        )
    return metrics


# Metrics where a HIGHER value is worse (the ratchet direction is "down").
_RATCHET_DOWN = set(release_receipt.STRUCTURAL_AUDIT_METRICS)


# Replacement authority + equivalence gate per deletion-candidate class — so a
# deletion is never "just delete it" but "delete it, route to THIS authority,
# gated by THIS check". (council: deletion candidates need a replacement + gate.)
_DELETION_PLAYBOOK = {
    "duplicate_authority": (
        "op_kinds.toml generated predicate (op_kinds_generated.rs)",
        "tools/gen_op_kinds.py --check + tests/test_gen_op_kinds.py",
    ),
    "semantic_fallthrough": (
        "op_kinds.toml [[opcode]] row / classifier set (read generated predicate)",
        "tools/gen_op_kinds.py --check + cargo test -p molt-backend (byte-diff)",
    ),
}


def _tooling_gaps(root: Path) -> list[tuple[str, str]]:
    """Return audit limitations from the current tree, not stale prose."""

    call_fact_built = _repo_file_exists(root, "tools/call_fact_coverage.py")
    causality_built = _repo_file_exists(root, "tools/perf_causality.py")
    pass_delta_built = _repo_file_exists(root, "tools/pass_delta_dashboard.py")
    fact_graph_built = _repo_file_exists(
        root, "runtime/molt-passes/src/tir/fact_graph.rs"
    )
    fact_dump_built = _repo_file_exists(root, "tools/fact_graph_dump.py")

    gaps = [
        (
            "RULE: discovery may be heuristic; authority may not",
            "this tool's regex discovery RANKS candidates only; it asserts no semantic "
            "correctness. The authoritative gate stays tools/gen_op_kinds.py --check "
            "(consumes the generated registry). A future version should parse the Rust "
            "AST / consume compiler-emitted facts for any claim that gates behavior.",
        )
    ]

    if call_fact_built and causality_built and not pass_delta_built:
        gaps.append(
            (
                "PARTIAL: fact-by-benchmark attribution",
                "MISSING-FACT-by-benchmark impact has tools/call_fact_coverage.py "
                "(representation census) and tools/perf_causality.py (#76 cycle-profile "
                "attribution plus taxonomy fallback). The missing closure is the "
                "census/pass-delta join and pass-delta dashboard.",
            )
        )
    elif call_fact_built and causality_built:
        gaps.append(
            (
                "BUILT: fact-by-benchmark attribution substrate",
                "tools/call_fact_coverage.py, tools/perf_causality.py, and "
                "tools/pass_delta_dashboard.py are present; keep their gates wired so "
                "attribution stays derived from evidence.",
            )
        )
    else:
        missing = [
            rel
            for rel, built in (
                ("tools/call_fact_coverage.py", call_fact_built),
                ("tools/perf_causality.py", causality_built),
            )
            if not built
        ]
        gaps.append(
            (
                "MISSING: fact-by-benchmark attribution",
                "MISSING-FACT-by-benchmark impact needs "
                + " + ".join(missing)
                + " joined to #76 hot profiles.",
            )
        )

    if not pass_delta_built:
        gaps.append(
            (
                "MISSING: pass-delta ledger",
                "tools/pass_delta_dashboard.py (not built) — which pass loses Repr / "
                "adds boxing / increases generic calls / RC events. Needed to "
                "attribute drift.",
            )
        )

    if fact_graph_built and fact_dump_built:
        gaps.append(
            (
                "BUILT: fact graph substrate",
                "runtime/molt-passes/src/tir/fact_graph.rs derives per-value "
                "producer/consumer/fact provenance from live TIR and "
                "tools/fact_graph_dump.py validates compiler-emitted graph JSON.",
            )
        )
    else:
        gaps.append(
            (
                "MISSING: fact graph",
                "runtime/molt-passes/src/tir/fact_graph.rs + tools/fact_graph_dump.py "
                "(not both built) — per-value provenance "
                "(producer/consumer/invalidator) to explain 'why is this boxed?'.",
            )
        )

    return gaps


def _repo_file_exists(root: Path, rel: str) -> bool:
    return (root / rel).is_file()


def _deletion_candidates(findings: list[Finding]) -> list[tuple[str, str, str, str]]:
    """(location, what, replacement authority, equivalence gate), ranked."""
    out = []
    for f in findings:
        if f.probe not in _DELETION_PLAYBOOK:
            continue
        if f.severity not in ("high", "medium"):
            continue
        repl, gate = _DELETION_PLAYBOOK[f.probe]
        out.append((f.location, f.title, repl, gate))
    out.sort(key=lambda t: 0 if "duplicate" in t[1] else 1)
    return out


def format_board(
    findings: list[Finding], metrics: dict[str, float], *, root: Path = ROOT_DEFAULT
) -> str:
    lines = [
        "<!-- @generated by tools/structural_audit.py --write-board. DO NOT EDIT. -->",
        "# Structural audit board",
        "",
        "Product board for the molt structural sweep — the first instrument of the "
        "Molt Semantic Control Plane (docs/design/foundation/46_semantic_control_plane.md). "
        "Generated by `tools/structural_audit.py`; the `--check` ratchet (CI) fails "
        "if any metric below regresses. It answers council questions #1 (duplicate "
        "semantic authorities), #2 (backend-local semantic guesses), #8 (legacy "
        "deletable once a generated fact covers them).",
        "",
        "> **Discovery-vs-authority rule (binding):** this tool uses heuristic "
        "regex DISCOVERY to *rank candidates*; it asserts no semantic correctness. "
        "Any output that GATES behavior must consume generated facts or typed AST. "
        "The authoritative op-semantics gate remains `tools/gen_op_kinds.py --check`.",
        "",
        "## Ratchet metrics (may only go DOWN)",
        "",
        "| metric | value |",
        "| --- | --- |",
    ]
    for k, v in metrics.items():
        lines.append(f"| {k} | {int(v) if v == int(v) else v} |")
    lines.append("")

    # TOP STRUCTURAL RISKS — highest-ranked findings across all probes.
    lines.append("## TOP STRUCTURAL RISKS (ranked)")
    lines.append("")
    lines.append("| sev | risk class | where | what |")
    lines.append("| --- | --- | --- | --- |")
    for f in findings[:15]:
        where = f.location if len(f.location) < 60 else f.location[:57] + "…"
        lines.append(f"| {f.severity} | {f.probe} | `{where}` | {f.title[:60]} |")
    lines.append("")

    # TOP DELETION CANDIDATES — with replacement authority + equivalence gate.
    dels = _deletion_candidates(findings)
    lines.append(
        f"## TOP DELETION CANDIDATES ({len(dels)}) — replace, don't just delete"
    )
    lines.append("")
    lines.append("| where | what | replacement authority | equivalence gate |")
    lines.append("| --- | --- | --- | --- |")
    for loc, what, repl, gate in dels[:20]:
        where = loc if len(loc) < 55 else loc[:52] + "…"
        lines.append(f"| `{where}` | {what[:42]} | {repl[:48]} | {gate[:42]} |")
    if len(dels) > 20:
        lines.append(f"| … | _{len(dels) - 20} more_ | | |")
    lines.append("")

    # TOP TOOLING GAPS — the tool's own limits + missing instruments.
    lines.append("## TOP TOOLING GAPS")
    lines.append("")
    for title, detail in _tooling_gaps(root):
        lines.append(f"- **{title}** — {detail}")
    lines.append("")
    lines.append(
        "> MISSING-FACT-by-benchmark board lives in "
        "`call_fact_coverage.py` (representation census) + doc 46 — "
        "structural_audit does not have benchmark profiles, so it does "
        "not claim that board (no overclaiming)."
    )
    lines.append("")

    # Full ranked findings by probe (the raw detail).
    lines.append("## Full findings by probe")
    lines.append("")
    by_probe: dict[str, list[Finding]] = {}
    for f in findings:
        by_probe.setdefault(f.probe, []).append(f)
    for probe, items in by_probe.items():
        lines.append(f"### {probe} ({len(items)})")
        lines.append("")
        lines.append("| sev | what | where | action |")
        lines.append("| --- | --- | --- | --- |")
        for f in items[:40]:
            where = f.location if len(f.location) < 70 else f.location[:67] + "…"
            lines.append(
                f"| {f.severity} | {f.title} | `{where}` | {f.suggested_action[:80]} |"
            )
        if len(items) > 40:
            lines.append(
                f"| … | _{len(items) - 40} more_ | | run `--json` for full list |"
            )
        lines.append("")
    return "\n".join(lines).rstrip("\n")


def format_path_scope_report(
    findings: list[Finding], metrics: dict[str, float], path_scope: frozenset[str]
) -> str:
    lines = [
        "Path-scoped structural audit (diagnostic only; CI --check remains whole-tree)",
        "",
        "## Scope",
    ]
    for rel in sorted(path_scope):
        lines.append(f"- `{rel}`")

    nonzero_metrics = {key: value for key, value in metrics.items() if value}
    lines.append("")
    lines.append("## Ratchet Metrics In Scope")
    if nonzero_metrics:
        for key in sorted(nonzero_metrics):
            lines.append(f"- `{key}`: {nonzero_metrics[key]:g}")
    else:
        lines.append("- none")

    lines.append("")
    lines.append("## Findings")
    if not findings:
        lines.append("- none")
        return "\n".join(lines)

    for finding in findings:
        lines.append(
            f"- `{finding.probe}` {finding.severity}: {finding.title} at "
            f"`{finding.location}` ({finding.detail})"
        )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    raw_argv = list(sys.argv[1:] if argv is None else argv)
    ap = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    ap.add_argument(
        "--root",
        type=Path,
        default=ROOT_DEFAULT,
        help="repo root to audit (default: this tool's repo)",
    )
    ap.add_argument(
        "--json", action="store_true", help="emit machine-readable findings"
    )
    ap.add_argument(
        "--path",
        action="append",
        type=Path,
        default=[],
        help=(
            "diagnostic scope: report findings/metrics for a project-owned file "
            "or directory under runtime/, src/, or tools/; repeatable"
        ),
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if any ratchet metric regressed vs baseline",
    )
    ap.add_argument(
        "--update-baseline",
        action="store_true",
        help="re-pin tools/structural_audit_baseline.json to current metrics",
    )
    ap.add_argument(
        "--write-board",
        action="store_true",
        help="regenerate docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md",
    )
    release_receipt.add_receipt_arguments(ap)
    args = ap.parse_args(raw_argv)

    root: Path = args.root.resolve()
    if args.receipt is not None and (
        args.path or args.update_baseline or args.write_board
    ):
        ap.error(
            "--receipt cannot be combined with --path, --update-baseline, "
            "or --write-board"
        )
    try:
        receipt_destination = release_receipt.prepare_receipt_destination(
            repo_root=root,
            receipt_path=args.receipt,
            observe_audit_engine=True,
            source_sha=args.source_sha,
        )
    except ValueError as exc:
        ap.error(str(exc))
    path_scope: frozenset[str] | None = None
    if args.path:
        if args.check or args.update_baseline or args.write_board:
            print(
                "ERROR: --path is diagnostic-only and cannot be combined with "
                "--check, --update-baseline, or --write-board",
                file=sys.stderr,
            )
            return 2
        try:
            path_scope = resolve_path_scope(root, args.path)
        except ValueError as exc:
            print(f"ERROR: {exc}", file=sys.stderr)
            return 2

    findings = run_all(root, path_scope=path_scope)
    metrics = ratchet_metrics(findings)
    baseline_path = root / BASELINE_PATH_REL

    wrote_artifact = False

    if args.update_baseline:
        baseline_path.parent.mkdir(parents=True, exist_ok=True)
        baseline_path.write_text(
            json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"baseline updated: {baseline_path}")
        wrote_artifact = True

    if args.write_board:
        board_path = root / BOARD_PATH_REL
        board_path.parent.mkdir(parents=True, exist_ok=True)
        board_path.write_text(
            format_board(findings, metrics, root=root) + "\n", encoding="utf-8"
        )
        print(f"board written: {board_path}")
        wrote_artifact = True

    if wrote_artifact and not args.json and not args.check:
        return 0

    baseline: dict[str, float] | None = None
    regressions: list[tuple[str, float, float]] = []
    improved: list[str] = []
    if args.check or receipt_destination is not None:
        if not baseline_path.is_file():
            print(
                f"ERROR: no baseline at {baseline_path}; run --update-baseline",
                file=sys.stderr,
            )
            return 2
        try:
            raw_baseline = release_receipt.loads_exact(
                baseline_path.read_text(encoding="utf-8")
            )
        except (
            OSError,
            UnicodeError,
            json.JSONDecodeError,
            release_receipt.ExactJsonError,
        ) as exc:
            print(f"ERROR: invalid baseline at {baseline_path}: {exc}", file=sys.stderr)
            return 2
        if not isinstance(raw_baseline, dict):
            print(
                f"ERROR: baseline root is not an object: {baseline_path}",
                file=sys.stderr,
            )
            return 2
        if set(raw_baseline) != set(release_receipt.STRUCTURAL_AUDIT_METRICS):
            print(
                f"ERROR: baseline metric keys differ from canonical authority: {baseline_path}",
                file=sys.stderr,
            )
            return 2
        if not all(release_receipt._metric(value) for value in raw_baseline.values()):
            print(
                f"ERROR: baseline values must be finite non-negative numbers: {baseline_path}",
                file=sys.stderr,
            )
            return 2
        baseline = raw_baseline
        for key in _RATCHET_DOWN:
            cur = metrics.get(key, 0.0)
            base = baseline.get(key, 0.0)
            if cur > base:
                regressions.append((key, base, cur))
        regressions.sort(key=lambda item: item[0])
        improved = sorted(
            key for key in _RATCHET_DOWN if metrics.get(key, 0) < baseline.get(key, 0)
        )

    if receipt_destination is not None:
        assert baseline is not None
        status = (
            release_receipt.STATUS_PASS
            if not regressions
            else release_receipt.STATUS_FAIL
        )
        try:
            receipt = release_receipt.build_receipt(
                kind=release_receipt.KIND_STRUCTURAL_AUDIT,
                source_sha=receipt_destination.source_sha,
                audit_engine=receipt_destination.audit_engine,
                status=status,
                argv=raw_argv,
                tool_path=Path(__file__),
                facts={
                    "baseline_metrics": baseline,
                    "baseline_path": BASELINE_PATH_REL,
                    "findings_count": len(findings),
                    "improved_metrics": improved,
                    "metrics": metrics,
                    "regressed_metrics": [key for key, _base, _cur in regressions],
                },
                input_paths=[baseline_path],
                repo_root=root,
            )
            release_receipt.write_receipt(receipt, receipt_destination)
        except ValueError as exc:
            print(f"structural audit receipt: ERROR: {exc}", file=sys.stderr)
            return 2

    if args.json:
        payload = {
            "metrics": metrics,
            "findings": [asdict(f) for f in findings],
        }
        if path_scope is not None:
            payload["path_scope"] = sorted(path_scope)
        print(json.dumps(payload, indent=2))
        return 1 if regressions else 0

    if args.check or receipt_destination is not None:
        if regressions:
            print(
                "STRUCTURAL RATCHET REGRESSED — new structural debt added:",
                file=sys.stderr,
            )
            for key, base, cur in regressions:
                print(f"  {key}: {base} -> {cur}  (must not increase)", file=sys.stderr)
            print(
                "Resolve the debt, or if intentional, justify and "
                "re-pin with --update-baseline.",
                file=sys.stderr,
            )
            return 1
        print(
            f"structural ratchet OK ({len(findings)} findings; "
            f"{len(improved)} metric(s) improved)"
        )
        if receipt_destination is not None:
            print(
                f"structural audit receipt written: {receipt_destination.output_path}"
            )
        return 0

    if path_scope is not None:
        print(format_path_scope_report(findings, metrics, path_scope))
        return 0

    # default: human board to stdout
    print(format_board(findings, metrics, root=root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
