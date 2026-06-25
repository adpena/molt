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
  structural_audit.py --write-board    regenerate docs/design/foundation/STRUCTURAL_AUDIT_BOARD.md
  structural_audit.py --check          fail (exit 1) if any ratchet metric regressed vs baseline
  structural_audit.py --update-baseline  re-pin tools/structural_audit_baseline.json

Wire into .github/workflows/ci.yml next to gen_op_kinds.py --check.
"""

from __future__ import annotations

import argparse
import ast
import json
import re
import sys
from dataclasses import dataclass, asdict
from pathlib import Path

ROOT_DEFAULT = Path(__file__).resolve().parents[1]
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


def _is_generated(path: Path) -> bool:
    name = path.name
    if name.endswith("_generated.rs") or name.endswith("_generated.py"):
        return True
    if path.as_posix().endswith("intrinsics/generated.rs"):
        return True
    try:
        head = path.read_text(errors="replace")[:400]
    except OSError:
        return False
    return "@generated" in head or "DO NOT EDIT" in head.upper()


def _iter_source_files(root: Path, suffixes: tuple[str, ...]) -> list[Path]:
    out: list[Path] = []
    for sub in _SOURCE_ROOTS:
        base = root / sub
        if not base.is_dir():
            continue
        for path in base.rglob("*"):
            if not path.is_file() or path.suffix not in suffixes:
                continue
            if _is_excluded(path, root):
                continue
            out.append(path)
    return out


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

    def sort_key(self) -> tuple[int, float]:
        return (_SEV_ORDER.get(self.severity, 9), -self.metric)


@dataclass(frozen=True)
class SourceRegion:
    kind: str
    name: str
    start_line: int
    end_line: int

    @property
    def span(self) -> int:
        return max(1, self.end_line - self.start_line + 1)


_LARGE_SOURCE_REGION_LINES = 250
_STRUCTURAL_GOD_MIN_LARGE_REGIONS = 3
_STRUCTURAL_GOD_MIXED_KIND_MIN_SCORE = 500


# --- robust Rust scanning -------------------------------------------------

_COMMENT_RE = re.compile(r"//.*?$", re.MULTILINE)


def _strip_line_comments(text: str) -> str:
    # Good enough for arm/brace counting: drop // comments (not string-aware,
    # but match arms in this codebase do not embed `//` inside string literals
    # on the arm lines we inspect). Block comments are rare in match bodies.
    return _COMMENT_RE.sub("", text)


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
_WILDCARD_ARM_RE = re.compile(r"(^|\n)\s*_\s*(=>|if\b)")
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
        if c in "([{":
            depth += 1
            line_start = False
            i += 1
            continue
        if c in ")]}":
            depth = max(depth - 1, 0)
            line_start = False
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
            if block.startswith("=>", j) or block.startswith("if", j):
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
    r"(?m)^\s*"
    r"(?:pub(?:\([^)]*\))?\s+)?"
    r"(?:async\s+|unsafe\s+|extern\s+\"[^\"]+\"\s+)*"
    r"(?P<kind>fn|impl|trait|struct|enum|mod)\b"
)


def _rust_top_level_regions(text: str) -> list[SourceRegion]:
    depths = _line_start_depths(text)
    regions: list[SourceRegion] = []
    for m in _RUST_TOP_LEVEL_ITEM_RE.finditer(text):
        line_start = _line_start_for_offset(text, m.start())
        if depths.get(line_start, 0) != 0:
            continue
        kind = m.group("kind")
        name = _rust_region_name(text, m.end(), kind)
        if _is_cfg_test_module(text, m.start(), kind, name):
            continue
        end_offset = _rust_region_end(text, m.end())
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


def _is_cfg_test_module(text: str, start: int, kind: str, name: str) -> bool:
    if kind != "mod" or name != "tests":
        return False
    return "#[cfg(test)]" in text[max(0, start - 300) : start]


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
        regions.append(SourceRegion(kind=kind, name=name, start_line=start, end_line=end))
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
        and _structural_god_score(large_regions)
        >= _STRUCTURAL_GOD_MIXED_KIND_MIN_SCORE
    )


def _region_summary(regions: list[SourceRegion], limit: int = 6) -> str:
    ranked = sorted(regions, key=lambda region: (-region.span, region.start_line))
    parts = [
        f"{region.kind} {region.name} {region.span} lines"
        for region in ranked[:limit]
    ]
    if len(ranked) > limit:
        parts.append(f"{len(ranked) - limit} more")
    return "; ".join(parts)


def probe_semantic_fallthroughs(root: Path) -> list[Finding]:
    """Hand-maintained semantic classifications over OpCode/kind that drift
    silently: `match {.. _ => default}` and `matches!(x, OpCode::A | B | ..)`.

    Each is a row the op-semantics ladder (op_kinds.toml) could absorb, deleting
    a drift point. EXHAUSTIVE matches (no wildcard) are rustc-gated and SKIPPED —
    they cannot drift, so flagging them would be noise."""
    findings: list[Finding] = []
    for path in _iter_source_files(root, (".rs",)):
        if _is_generated(path):
            continue
        try:
            raw = path.read_text(errors="replace")
        except OSError:
            continue
        if "OpCode::" not in raw:
            continue
        text = _strip_line_comments(raw)
        rel = path.relative_to(root).as_posix()
        critical = _file_is_critical(path)

        # (a) match blocks with a wildcard default over opcode-like scrutinee.
        for m in _MATCH_HEAD_RE.finditer(text):
            scrutinee = m.group(1)
            brace_idx = m.end() - 1
            _, block = _balanced_block(text, brace_idx)
            opcode_arms = len(set(_OPCODE_ARM_RE.findall(block)))
            if opcode_arms < 2:
                continue
            if not (_KIND_SCRUTINEE_RE.search(scrutinee) or opcode_arms >= 3):
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


def probe_god_files(root: Path, ceiling: int = 4000) -> list[Finding]:
    """Non-generated source files large enough to be decomposition candidates."""
    findings: list[Finding] = []
    for suffix, lang_ceiling in ((".rs", ceiling), (".py", 2500)):
        for path in _iter_source_files(root, (suffix,)):
            if _is_generated(path):
                continue
            try:
                n = path.read_text(errors="replace").count("\n") + 1
            except OSError:
                continue
            if n < lang_ceiling:
                continue
            rel = path.relative_to(root).as_posix()
            sev = (
                "high"
                if n >= lang_ceiling * 3
                else "medium"
                if n >= lang_ceiling * 1.5
                else "low"
            )
            findings.append(
                Finding(
                    probe="god_file",
                    severity=sev,
                    title=f"{n} lines (ceiling {lang_ceiling})",
                    location=rel,
                    detail=f"{n} lines — {n // lang_ceiling}× the {suffix} decomposition ceiling",
                    suggested_action="extract cohesive submodules along legible seams "
                    "(Lattner: one responsibility per file)",
                    class_retired="god-object-extension-fear",
                    metric=n,
                )
            )
    return findings


def probe_structural_god_files(
    root: Path,
    ceiling: int = 4000,
    py_ceiling: int = 2500,
) -> list[Finding]:
    """Oversized files with multiple large top-level extraction regions."""
    findings: list[Finding] = []
    for suffix, lang_ceiling in ((".rs", ceiling), (".py", py_ceiling)):
        for path in _iter_source_files(root, (suffix,)):
            if _is_generated(path):
                continue
            try:
                text = path.read_text(errors="replace")
            except OSError:
                continue
            line_count = _line_count(text)
            if line_count < lang_ceiling:
                continue
            large_regions = _large_source_regions(_top_level_regions(path, text))
            if not _is_structural_god_region_set(large_regions):
                continue
            score = _structural_god_score(large_regions)
            rel = path.relative_to(root).as_posix()
            large_region_count = len(large_regions)
            sev = (
                "high"
                if score >= lang_ceiling and large_region_count >= 4
                else "medium"
                if score >= lang_ceiling // 2 or large_region_count >= 4
                else "low"
            )
            findings.append(
                Finding(
                    probe="structural_god_file",
                    severity=sev,
                    title=(
                        f"{large_region_count} large top-level regions "
                        f"({score} excess lines)"
                    ),
                    location=rel,
                    detail=(
                        f"{line_count} lines; "
                        f"large_regions={_region_summary(large_regions)}"
                    ),
                    suggested_action=(
                        "extract the large top-level regions into cohesive modules; "
                        "do not add more authority to this file"
                    ),
                    class_retired="multi-region-god-file",
                    metric=float(score),
                )
            )
    return findings


_DEBT_RE = re.compile(
    r"\b(TODO|FIXME|HACK|XXX|WORKAROUND|KLUDGE)\b|"
    r"\b(unimplemented!|todo!)\s*\(|"
    r"for now\b|temporar(y|ily)\b",
    re.IGNORECASE,
)


def probe_debt_markers(root: Path) -> list[Finding]:
    """Workaround/debt markers — the CLAUDE.md zero-workaround policy made
    machine-checkable. Reported per file (ranked), ratcheted in aggregate."""
    findings: list[Finding] = []
    for path in _iter_source_files(root, (".rs", ".py")):
        if _is_generated(path):
            continue
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        hits = _DEBT_RE.findall(text)
        count = len(hits)
        if count == 0:
            continue
        rel = path.relative_to(root).as_posix()
        sev = "medium" if count >= 15 else "low"
        findings.append(
            Finding(
                probe="debt_marker",
                severity=sev,
                title=f"{count} debt/workaround markers",
                location=rel,
                detail="TODO/FIXME/HACK/XXX/WORKAROUND/unimplemented!/'for now'",
                suggested_action="resolve in place (zero-workaround policy) or convert "
                "to a tracked task with a structural fix",
                class_retired="accumulating-technical-debt",
                metric=count,
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
    for path in _iter_source_files(root, (".rs",)):
        if _is_generated(path):
            continue
        try:
            text = path.read_text(errors="replace")
        except OSError:
            continue
        rel = path.relative_to(root).as_posix()
        # Test code is not a semantic authority: predicates inside the file's test
        # module (`#[cfg(test)]` / `mod tests`) are regression fixtures whose names
        # happen to match a property keyword (e.g. `side_effecting_ops_preserved`).
        test_offsets = [text.find("#[cfg(test)]"), text.find("mod tests")]
        test_boundary = min((o for o in test_offsets if o >= 0), default=len(text))
        for m in _PREDICATE_RE.finditer(text):
            if m.start() >= test_boundary:
                continue  # test-module fixture, not a classifier
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
    m = re.search(rf"\benum\s+{re.escape(enum_name)}\s*\{{", rust_text)
    if not m:
        return set()
    _, block = _balanced_block(rust_text, m.end() - 1)
    body = _strip_line_comments(block)[1:-1]  # drop the outer { }
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


def probe_registry_reconciliation(root: Path) -> list[Finding]:
    """Confidence (INFO) check: the [[opcode]] effect-oracle table is rendered as
    an EXHAUSTIVE rustc match, so coverage is compiler-enforced — this only
    reports parser-agreement so a drift in the *parser* (not the data) surfaces."""
    ops_rs = root / "runtime/molt-tir/src/tir/ops.rs"
    toml_path = root / "runtime/molt-tir/src/tir/op_kinds.toml"
    findings: list[Finding] = []
    if not ops_rs.is_file() or not toml_path.is_file():
        return findings
    variants = _count_enum_variants(ops_rs.read_text(errors="replace"), "OpCode")
    toml_text = toml_path.read_text(errors="replace")
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
            location="runtime/molt-tir/src/tir/{ops.rs,op_kinds.toml}",
            detail="effect oracle is an exhaustive (no-wildcard) match — coverage is "
            "rustc-enforced; this line is parser confidence only, not a gate",
            suggested_action="no action unless a NEW non-exhaustive opcode classifier "
            "appears (probe semantic_fallthrough catches those)",
            class_retired="",
            metric=0,
        )
    )
    return findings


PROBES = (
    probe_semantic_fallthroughs,
    probe_god_files,
    probe_structural_god_files,
    probe_debt_markers,
    probe_duplicate_authorities,
    probe_registry_reconciliation,
)


def run_all(root: Path) -> list[Finding]:
    findings: list[Finding] = []
    for probe in PROBES:
        findings.extend(probe(root))
    findings.sort(key=lambda f: f.sort_key())
    return findings


# --- ratchet metrics (the --check gate) -----------------------------------


def _large_region_count_from_title(title: str) -> int:
    m = re.match(r"(\d+)\s+large top-level regions", title)
    return int(m.group(1)) if m else 0


def ratchet_metrics(findings: list[Finding]) -> dict[str, float]:
    """Aggregate scalars that may only improve (decrease). CI fails on regress.

    These are deliberately PRECISE (fail-loud dispatch switchboards and emitter
    routes already excluded by the probe), so the ratchet fires on real new
    hand-maintained semantic surface, not on legitimate new dispatch arms."""
    sem = [f for f in findings if f.probe == "semantic_fallthrough"]
    match_cls = [f for f in sem if f.title.startswith("hand-classified")]
    handsets = [f for f in sem if f.title.startswith("`matches!`")]
    debt = [f for f in findings if f.probe == "debt_marker"]
    god = [f for f in findings if f.probe == "god_file"]
    structural_god = [f for f in findings if f.probe == "structural_god_file"]
    dup = [f for f in findings if f.probe == "duplicate_authority"]
    return {
        # the hand-maintained-opcode-fact surface (match classifiers w/ silent default)
        "hand_classified_matches": float(len(match_cls)),
        # the high-priority subset: critical file AND large (≥6-opcode) hand-list
        "critical_hand_classifications": float(
            sum(1 for f in match_cls if f.severity == "high")
        ),
        # hand-maintained opcode SETS via matches! (≥3 opcodes) in any file
        "handset_classifications": float(len(handsets)),
        "debt_markers_total": float(sum(int(f.metric) for f in debt)),
        "god_files": float(len(god)),
        "max_god_file_lines": float(max((f.metric for f in god), default=0)),
        "structural_god_files": float(len(structural_god)),
        "max_god_file_structural_score": float(
            max((f.metric for f in structural_god), default=0)
        ),
        "god_file_large_regions": float(
            sum(_large_region_count_from_title(f.title) for f in structural_god)
        ),
        "duplicate_authorities": float(len(dup)),
    }


# Metrics where a HIGHER value is worse (the ratchet direction is "down").
_RATCHET_DOWN = set(ratchet_metrics([]).keys())


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
    fact_graph_built = _repo_file_exists(root, "runtime/molt-tir/src/tir/fact_graph.rs")
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
                "runtime/molt-tir/src/tir/fact_graph.rs derives per-value "
                "producer/consumer/fact provenance from live TIR and "
                "tools/fact_graph_dump.py validates compiler-emitted graph JSON.",
            )
        )
    else:
        gaps.append(
            (
                "MISSING: fact graph",
                "runtime/molt-tir/src/tir/fact_graph.rs + tools/fact_graph_dump.py "
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


def main(argv: list[str] | None = None) -> int:
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
    args = ap.parse_args(argv)

    root: Path = args.root.resolve()
    findings = run_all(root)
    metrics = ratchet_metrics(findings)
    baseline_path = root / BASELINE_PATH_REL

    if args.update_baseline:
        baseline_path.write_text(
            json.dumps(metrics, indent=2, sort_keys=True) + "\n", encoding="utf-8"
        )
        print(f"baseline updated: {baseline_path}")
        return 0

    if args.write_board:
        board_path = root / BOARD_PATH_REL
        board_path.write_text(
            format_board(findings, metrics, root=root) + "\n", encoding="utf-8"
        )
        print(f"board written: {board_path}")
        return 0

    if args.json:
        print(
            json.dumps(
                {
                    "metrics": metrics,
                    "findings": [asdict(f) for f in findings],
                },
                indent=2,
            )
        )
        return 0

    if args.check:
        if not baseline_path.is_file():
            print(
                f"ERROR: no baseline at {baseline_path}; run --update-baseline",
                file=sys.stderr,
            )
            return 2
        baseline = json.loads(baseline_path.read_text())
        regressions = []
        for key in _RATCHET_DOWN:
            cur = metrics.get(key, 0.0)
            base = baseline.get(key, 0.0)
            if cur > base:
                regressions.append((key, base, cur))
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
        improved = [k for k in _RATCHET_DOWN if metrics.get(k, 0) < baseline.get(k, 0)]
        print(
            f"structural ratchet OK ({len(findings)} findings; "
            f"{len(improved)} metric(s) improved)"
        )
        return 0

    # default: human board to stdout
    print(format_board(findings, metrics, root=root))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
