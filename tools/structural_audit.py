#!/usr/bin/env python3
"""Whole-tree structural audit — the ranked cleanup board + a fail-loud ratchet.

The op-kind registry (``op_kinds.toml`` → ``tools/gen_op_kinds.py``) proved the
thesis: *repeated semantics belong in one generated table, not hand-maintained
across passes*. Its effect oracle is an EXHAUSTIVE Rust ``match`` (no wildcard),
so a new opcode that forgets a row fails to COMPILE — drift is impossible there.

This tool finds the places that have NOT yet reached that bar — where a semantic
property is still decided by a hand-written list with a silent default, where a
file has grown into a god-object, where workaround/debt markers accumulate, and
where two authorities classify the same thing. It answers the council's
structural-sweep questions #1 (duplicate semantic authorities), #2 (backend-local
semantic guesses), and #8 (legacy paths now coverable by generated facts) with a
RANKED BOARD, and — critically — a ``--check`` RATCHET so the numbers can only go
down: adding a new hand-maintained semantic fallthrough, growing a god-file past
its ceiling, or adding debt markers fails CI.

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
            wildcard = _WILDCARD_ARM_RE.search(block)
            if not wildcard:
                continue  # exhaustive → compiler-gated → safe, skip
            default_body = _default_arm_body(block, wildcard.start())
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

# Tooling gaps the tool knows about ITSELF and its siblings (council: the audit
# must name its own limitations + the missing instruments). This encodes the
# binding discovery-vs-authority rule.
_TOOLING_GAPS = [
    (
        "RULE: discovery may be heuristic; authority may not",
        "this tool's regex discovery RANKS candidates only; it asserts no semantic "
        "correctness. The authoritative gate stays tools/gen_op_kinds.py --check "
        "(consumes the generated registry). A future version should parse the Rust "
        "AST / consume compiler-emitted facts for any claim that gates behavior.",
    ),
    (
        "MISSING: fact-by-benchmark attribution",
        "MISSING-FACT-by-benchmark impact needs tools/call_fact_coverage.py (built) "
        "+ tools/perf_causality.py (not built) joined to #76 hot profiles.",
    ),
    (
        "MISSING: pass-delta ledger",
        "tools/pass_delta_dashboard.py (not built) — which pass loses Repr / adds "
        "boxing / increases generic calls / RC events. Needed to attribute drift.",
    ),
    (
        "MISSING: fact graph",
        "runtime/.../fact_graph.rs + tools/fact_graph_dump.py (not built) — per-value "
        "provenance (producer/consumer/invalidator) to explain 'why is this boxed?'.",
    ),
]


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


def format_board(findings: list[Finding], metrics: dict[str, float]) -> str:
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
    for title, detail in _TOOLING_GAPS:
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
            format_board(findings, metrics) + "\n", encoding="utf-8"
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
    print(format_board(findings, metrics))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
