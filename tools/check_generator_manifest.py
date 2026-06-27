#!/usr/bin/env python3
"""The generator-manifest meta-gate + closed-domain exhaustiveness auditor.

This is the doc 59 (semantic fact plane) Phase 1+3 enforcement tool. It makes
three classes of structural failure machine-checkable, reading the single
declarative authority ``tools/generator_manifest.toml``:

  1. ORPHAN GENERATED FILE — a ``@generated`` / DO-NOT-EDIT file under the owned
     source roots whose producer is NOT a registered generator and is not on the
     orphan allowlist. (A generated file with no committed, gated generator is
     structural debt by the project's own definition, gen_protocol.py:25.)

  2. UNGATED GENERATOR — a registered authority that does not support ``--check``
     (``check_mode = false``), or a CI-checkable generator with no ``--check``
     step in ``.github/workflows/ci.yml``. Closes the §4 gating holes so "a
     generated file with no committed, gated generator" becomes unexpressible.

  3. NON-EXHAUSTIVE CLOSED-DOMAIN MATCH — a hand-written ``match`` / ``matches!``
     over a CLOSED enum domain (``OpCode``, ``Terminator``, …) that silently
     defaults or misses an arm. THIS is the structural defense against the
     dispatch-handler-mirror-hazard bug class (a match over a closed op_kinds
     domain that silently defaults or misses an arm = a loud panic or a silent
     wrong-dispatch). The class has bitten main (an arm not in HANDLED_KINDS;
     copy-kind / get_attr canonical-default sites). For each declared closed
     domain the auditor parses the LIVE enum and proves every hand-written match
     over it either (a) covers all live variants, or (b) carries a top-level
     default that is fail-loud (panic/Err/unreachable — the correct fail-CLOSED
     dispatch pattern) or is explicitly listed in ``audited_defaults``.

The discovery-vs-authority firewall (doc 46 rule #1, doc 59 §3 F2) is honored:
the Rust enum/match parser is DISCOVERY (it ranks "is this match total?"). The
AUTHORITY that the generated tables are correct stays the rustc exhaustive match
(rustc refuses an unclassified variant at compile time). A parser miss can only
yield a false "looks total"; it can never manufacture a passing gate that rustc
would fail. The Rust-scanning primitives are imported from
``tools/structural_audit.py`` so there is exactly ONE Rust-match parser authority
in the tree (no duplicate parser).

Usage::

    python3 tools/check_generator_manifest.py            # human report
    python3 tools/check_generator_manifest.py --check     # exit 1 on any violation
    python3 tools/check_generator_manifest.py --check --check-idempotence
        # additionally run each CI-checkable generator's --check (proves the
        # committed output equals a fresh render in THIS environment)
    python3 tools/check_generator_manifest.py --json      # machine-readable

Wired into ``tools/ci_gate.py`` (tier 1) and ``.github/workflows/ci.yml``.
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import subprocess
import sys
import tomllib
from dataclasses import dataclass, field
from pathlib import Path

ROOT_DEFAULT = Path(__file__).resolve().parents[1]
MANIFEST_REL = "tools/generator_manifest.toml"
CI_WORKFLOW_REL = ".github/workflows/ci.yml"


# ---------------------------------------------------------------------------
# Shared Rust-scanning primitives — imported from structural_audit.py so there
# is exactly ONE Rust-match parser authority in the tree (doc 46 rule #1).
# ---------------------------------------------------------------------------


def _load_structural_audit(root: Path):
    """Import tools/structural_audit.py as a module (it is a script, not a
    package member). Registered in sys.modules so its @dataclass resolves."""
    tool = root / "tools" / "structural_audit.py"
    spec = importlib.util.spec_from_file_location(
        "molt_structural_audit_for_manifest", tool
    )
    if spec is None or spec.loader is None:  # pragma: no cover - defensive
        raise RuntimeError(f"cannot load {tool}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


# ---------------------------------------------------------------------------
# Findings
# ---------------------------------------------------------------------------

_SEV_ORDER = {"critical": 0, "high": 1, "medium": 2, "low": 3, "info": 4}


@dataclass(frozen=True)
class Violation:
    kind: str  # "orphan" | "ungated" | "closed_domain" | "manifest" | "idempotence"
    severity: str
    location: str
    detail: str

    def sort_key(self) -> tuple:
        return (_SEV_ORDER.get(self.severity, 9), self.kind, self.location, self.detail)


@dataclass
class Manifest:
    schema_version: int
    generated_scan_roots: list[str]
    generators: list[dict]
    closed_domains: list[dict]
    orphan_generated: list[dict]
    raw: dict = field(default_factory=dict)


class ManifestError(RuntimeError):
    """A structural defect in generator_manifest.toml itself (fail-loud)."""


# ---------------------------------------------------------------------------
# Manifest loading + self-validation
# ---------------------------------------------------------------------------

_REQUIRED_GENERATOR_FIELDS = ("tool", "outputs", "source")


def load_manifest(root: Path) -> Manifest:
    path = root / MANIFEST_REL
    if not path.is_file():
        raise ManifestError(f"missing manifest: {MANIFEST_REL}")
    data = tomllib.loads(path.read_text(encoding="utf-8"))

    schema_version = data.get("schema_version")
    if schema_version != 1:
        raise ManifestError(
            f"unsupported schema_version {schema_version!r} (expected 1)"
        )
    scan_roots = data.get("generated_scan_roots")
    if not isinstance(scan_roots, list) or not all(
        isinstance(s, str) for s in scan_roots
    ):
        raise ManifestError("generated_scan_roots must be a list of strings")

    generators = data.get("generator", [])
    if not isinstance(generators, list) or not generators:
        raise ManifestError("manifest has no [[generator]] rows")
    seen_tools: set[str] = set()
    for row in generators:
        for fld in _REQUIRED_GENERATOR_FIELDS:
            if fld not in row:
                raise ManifestError(
                    f"[[generator]] row missing {fld!r}: {row.get('tool', row)}"
                )
        tool = row["tool"]
        if tool in seen_tools:
            raise ManifestError(f"duplicate [[generator]] tool: {tool}")
        seen_tools.add(tool)
        if not isinstance(row["outputs"], list) or not all(
            isinstance(o, str) for o in row["outputs"]
        ):
            raise ManifestError(f"{tool}: outputs must be a list of strings")
        # A non-discovery authority that is CI-checkable must justify any skip.
        if not row.get("discovery_only", False):
            if not row.get("ci_checkable", True) and not row.get("ci_skip_reason"):
                raise ManifestError(
                    f"{tool}: ci_checkable = false requires a non-empty ci_skip_reason"
                )
            # Either a sync_test or a sync_test_reason must be recorded.
            if not row.get("sync_test") and not row.get("sync_test_reason"):
                raise ManifestError(
                    f"{tool}: must declare either sync_test or sync_test_reason"
                )
            # A declared sync_test must reference a real file (no phantom test —
            # the manifest must not claim coverage that does not exist).
            sync_test = row.get("sync_test")
            if sync_test and not (root / sync_test).is_file():
                raise ManifestError(
                    f"{tool}: sync_test {sync_test!r} does not exist (declare an "
                    "existing test or use sync_test_reason)"
                )

    closed_domains = data.get("closed_domain", [])
    if not isinstance(closed_domains, list):
        raise ManifestError("[[closed_domain]] must be a list")
    for cd in closed_domains:
        for fld in ("name", "enum_file", "enum_name", "owned_by"):
            if fld not in cd:
                raise ManifestError(
                    f"[[closed_domain]] row missing {fld!r}: {cd.get('name', cd)}"
                )
        # The owning generator must declare this domain in its closed_domains[].
        owner = next((g for g in generators if g["tool"] == cd["owned_by"]), None)
        if owner is None:
            raise ManifestError(
                f"closed_domain {cd['name']}: owned_by {cd['owned_by']} is not a "
                "registered generator"
            )
        if cd["name"] not in owner.get("closed_domains", []):
            raise ManifestError(
                f"closed_domain {cd['name']} not listed in {cd['owned_by']}'s "
                "closed_domains[] (the two declarations must agree)"
            )

    orphan_generated = data.get("orphan_generated", [])
    if not isinstance(orphan_generated, list):
        raise ManifestError("[[orphan_generated]] must be a list")
    for og in orphan_generated:
        for fld in ("path", "producer", "reason"):
            if fld not in og:
                raise ManifestError(
                    f"[[orphan_generated]] row missing {fld!r}: {og.get('path', og)}"
                )

    return Manifest(
        schema_version=schema_version,
        generated_scan_roots=scan_roots,
        generators=generators,
        closed_domains=closed_domains,
        orphan_generated=orphan_generated,
        raw=data,
    )


# ---------------------------------------------------------------------------
# (1) Orphan generated-file detection
# ---------------------------------------------------------------------------

# Generated-file families produced by a generator that writes a large, dynamic
# fan-out of files (so they are not enumerated one-by-one in outputs[]). Each
# fragment maps to the generator that owns it; the orphan scan credits a file
# under such a path to that generator. Keep this list MINIMAL and justified.
_GENERATED_FAMILY_OWNERS = {
    # gen_intrinsics writes per-module resolver files under these stems.
    "/intrinsics_generated/": "tools/gen_intrinsics.py",
    "intrinsics_generated.rs": "tools/gen_intrinsics.py",
    "/intrinsics/generated_resolvers/": "tools/gen_intrinsics.py",
}


def _is_generated_file(sa, path: Path) -> bool:
    """A file is generated iff structural_audit's authoritative heuristic says so.
    We reuse that one predicate rather than re-deriving the marker scan."""
    return sa._is_generated(path)


def detect_orphans(root: Path, manifest: Manifest, sa) -> list[Violation]:
    declared_outputs: set[str] = set()
    declared_sources: set[str] = set()
    for g in manifest.generators:
        declared_outputs.update(g["outputs"])
        # A `source` may be a repo-relative file (a SEED table with an @generated
        # header) or a free-text description; only the former matters here.
        src = g.get("source", "")
        if src and (root / src).is_file():
            declared_sources.add(src)
    for cd in manifest.closed_domains:
        st = cd.get("source_table", "")
        if st and (root / st).is_file():
            declared_sources.add(st)
    allowlisted = {og["path"] for og in manifest.orphan_generated}
    # Fact-plane tooling DESCRIBES @generated in its prose; it is hand-maintained
    # authority, not a generated file. Exempt the manifest and its checker.
    tooling_exemptions = {MANIFEST_REL, "tools/check_generator_manifest.py"}

    violations: list[Violation] = []
    for sub in manifest.generated_scan_roots:
        base = root / sub
        if not base.is_dir():
            continue
        for path in base.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix not in (
                ".rs",
                ".py",
                ".pyi",
                ".inc",
                ".txt",
                ".md",
                ".toml",
            ):
                continue
            if sa._is_excluded(path, root):
                continue
            if not _is_generated_file(sa, path):
                continue
            rel = path.relative_to(root).as_posix()
            if rel in declared_outputs or rel in allowlisted:
                continue
            # A declared `source`/seed table is the INPUT to a generator (it may
            # carry an @generated-SEED header), not a generated output.
            if rel in declared_sources:
                continue
            # Credit a recognized dynamic family to its generator.
            if any(frag in rel for frag in _GENERATED_FAMILY_OWNERS):
                continue
            # The fact-plane tooling describes @generated; it is not generated.
            if rel in tooling_exemptions:
                continue
            violations.append(
                Violation(
                    kind="orphan",
                    severity="high",
                    location=rel,
                    detail=(
                        "@generated/DO-NOT-EDIT file with no registered generator. "
                        "Add its generator's outputs[] row to generator_manifest.toml, "
                        "or add an [[orphan_generated]] allowlist row naming the real "
                        "producer (e.g. a Rust build-side generator)."
                    ),
                )
            )
    return violations


# ---------------------------------------------------------------------------
# (2) check_mode / CI-gating enforcement
# ---------------------------------------------------------------------------


def check_gating(root: Path, manifest: Manifest) -> list[Violation]:
    violations: list[Violation] = []
    ci_path = root / CI_WORKFLOW_REL
    ci_text = ci_path.read_text(encoding="utf-8") if ci_path.is_file() else ""

    for g in manifest.generators:
        tool = g["tool"]
        if g.get("discovery_only", False):
            continue
        # The generator file must exist.
        if not (root / tool).is_file():
            violations.append(
                Violation(
                    kind="ungated",
                    severity="high",
                    location=tool,
                    detail="registered generator file does not exist",
                )
            )
            continue
        # A non-discovery authority MUST support --check.
        if not g.get("check_mode", False):
            violations.append(
                Violation(
                    kind="ungated",
                    severity="high",
                    location=tool,
                    detail=(
                        "check_mode = false: a generated authority must support "
                        "`--check` so its output cannot drift silently. Add a "
                        "--check flag to the generator."
                    ),
                )
            )
        # A CI-checkable generator MUST have a --check step in ci.yml.
        if g.get("ci_checkable", True):
            needle = f"{tool} --check"
            if needle not in ci_text:
                violations.append(
                    Violation(
                        kind="ungated",
                        severity="high",
                        location=tool,
                        detail=(
                            f"no `{needle}` step in {CI_WORKFLOW_REL}. Either wire "
                            "the CI --check step, or set ci_checkable = false with a "
                            "ci_skip_reason if its source is not reproducible in CI."
                        ),
                    )
                )
    return violations


# ---------------------------------------------------------------------------
# (3) The closed-domain exhaustiveness auditor (the priority deliverable)
# ---------------------------------------------------------------------------


def _blank_rust_comments_and_strings(text: str) -> str:
    """Return `text` with the *contents* of `//` line comments, `/* */` block
    comments, and "…"/'…' string/char literals replaced by spaces, preserving
    every character offset and newline (so line numbers stay valid).

    This is essential: the word `match` and stray `{`/`}` appear constantly in
    Rust comments and string literals ("matches the pattern", a doc-comment, a
    format string with `{}`). Searching the RAW text for the `match` keyword would
    land on those and balance the wrong brace — exactly the false-positive/missed-
    detection class that makes a gate untrustworthy. We blank them once up front
    so keyword search, brace balancing, and arm scanning all operate on code only.
    """
    out = list(text)
    n = len(text)
    i = 0
    LINE, BLOCK, STR, CHAR = 1, 2, 3, 4
    state = 0
    while i < n:
        c = text[i]
        if state == 0:
            if c == "/" and i + 1 < n and text[i + 1] == "/":
                state = LINE
                i += 2
                continue
            if c == "/" and i + 1 < n and text[i + 1] == "*":
                out[i] = " "
                out[i + 1] = " "
                state = BLOCK
                i += 2
                continue
            if c == '"':
                state = STR
                i += 1
                continue
            if c == "'":
                # Could be a char literal or a lifetime (`'a`). Treat only a real
                # char literal (closing quote within a few chars) as a string; a
                # lifetime has no closing quote and is harmless to leave as-is.
                j = i + 1
                if j < n and text[j] == "\\":
                    j += 2
                else:
                    j += 1
                if j < n and text[j] == "'":
                    state = CHAR
                    i += 1
                    continue
                i += 1
                continue
            i += 1
            continue
        if state == LINE:
            if c == "\n":
                state = 0
            else:
                out[i] = " "
            i += 1
            continue
        if state == BLOCK:
            if c == "*" and i + 1 < n and text[i + 1] == "/":
                out[i] = " "
                out[i + 1] = " "
                state = 0
                i += 2
                continue
            if c != "\n":
                out[i] = " "
            i += 1
            continue
        if state == STR:
            if c == "\\":
                out[i] = " "
                if i + 1 < n and text[i + 1] != "\n":
                    out[i + 1] = " "
                i += 2
                continue
            if c == '"':
                state = 0
                i += 1
                continue
            if c != "\n":
                out[i] = " "
            i += 1
            continue
        if state == CHAR:
            if c == "\\":
                out[i] = " "
                if i + 1 < n and text[i + 1] != "\n":
                    out[i + 1] = " "
                i += 2
                continue
            if c == "'":
                state = 0
                i += 1
                continue
            if c != "\n":
                out[i] = " "
            i += 1
            continue
    return "".join(out)


def _scan_closed_domain_matches(
    sa, text: str, rel: str, enum_name: str, variants: set[str], audited: set[str]
) -> list[Violation]:
    """Find every hand-written `match` over `enum_name` in `text` and prove each
    is exhaustive-or-audited over the live `variants` set.

    Reuses structural_audit's depth-aware brace/arm primitives so this is the
    same parser the debt ratchet trusts (no second Rust parser). A `match` is
    "over the closed domain" iff at least two distinct `EnumName::Variant`
    patterns appear among its top-level arms (a single mention is a guard/equality
    check, not a classifier). All scanning runs on COMMENT/STRING-BLANKED text so
    the word `match` or a stray brace in prose can never corrupt detection (offsets
    are preserved, so reported line numbers map back to the original source).
    """
    violations: list[Violation] = []
    code = _blank_rust_comments_and_strings(text)
    arm_pat = enum_name + "::"
    i = 0
    n = len(code)
    while True:
        idx = code.find("match", i)
        if idx < 0:
            break
        i = idx + 5
        # Require a word boundary before/after `match`.
        if idx > 0 and (code[idx - 1].isalnum() or code[idx - 1] == "_"):
            continue
        if idx + 5 < n and (code[idx + 5].isalnum() or code[idx + 5] == "_"):
            continue
        brace = code.find("{", idx)
        if brace < 0:
            break
        # Balance braces over the COMMENT/STRING-BLANKED code so a `{`/`}` in prose
        # cannot desync the block. Offsets match the original text exactly.
        end, block = sa._balanced_block(code, brace)
        i = end
        # Collect the variants this match's TOP-LEVEL arms name (block is already
        # blanked, so no nested-comment `Enum::` token can leak in).
        named = _top_level_named_variants(sa, block, arm_pat, enum_name)
        if len(named) < 2:
            continue  # not a classifier over this domain
        # If this match consumes a generated *_table() result, it is a generated
        # role consumer, not a hand-maintained authority over the raw enum.
        head = code[idx:brace]
        if "_table(" in head or "_table (" in head:
            continue
        wildcard = sa._top_level_wildcard_arm_start(block)
        if wildcard is not None:
            body = sa._default_arm_body(block, wildcard)
            # A fail-loud default (panic/Err/unreachable) is the CORRECT
            # fail-closed dispatch pattern — never silent miscompile. Allowed.
            if sa._FAILLOUD_RE.search(body):
                continue
            # An explicitly-audited default for THIS file is allowed.
            if rel in audited:
                continue
            line = sa._line_of_offset(text, idx)
            violations.append(
                Violation(
                    kind="closed_domain",
                    severity="critical" if sa._file_is_critical(Path(rel)) else "high",
                    location=f"{rel}:{line}",
                    detail=(
                        f"hand-written `match` over closed domain {enum_name} with a "
                        f"SILENT default arm (covers {len(named)}/{len(variants)} "
                        "variants then `_ => <non-fail-loud>`). A new enum variant "
                        "would silently take the default — the dispatch-handler-"
                        "mirror-hazard class. Either cover every variant (delete the "
                        "wildcard so rustc enforces exhaustiveness), make the default "
                        "fail-loud (panic!/unreachable!/Err), route through a generated "
                        f"*_table() predicate, or add {rel!r} to the domain's "
                        "audited_defaults with justification."
                    ),
                )
            )
            continue
        # No wildcard: rustc already enforces exhaustiveness over the enum. But a
        # match that names a strict SUBSET of variants with no wildcard would not
        # compile, so if we see no wildcard we trust rustc. (Discovery firewall:
        # the parser cannot manufacture a pass rustc would fail.)
    return violations


def _top_level_named_variants(sa, block: str, arm_pat: str, enum_name: str) -> set[str]:
    """Return the set of `EnumName::Variant` names used as ARM PATTERNS at the top
    level (depth 1) of a match block — i.e. the variants this match DISPATCHES on.

    A variant counts only when it appears in the pattern region of an arm (left of
    that arm's top-level `=>`), never in an arm BODY. This is the crux distinction
    between `match x { OpCode::A => …, _ => … }` (dispatches on OpCode — a closed-
    domain classifier) and `match cond { _ => OpCode::Lt }` (merely PRODUCES an
    OpCode value — not a classifier). Without this, a match that constructs enum
    values in its arm bodies would be mis-flagged.

    The variant region resets to "pattern" at the start of the block and after
    every top-level arm boundary (a top-level `,` separating arms, or the end of a
    top-level `{…}` arm body). Within an arm it flips to "body" at the top-level
    `=>`. Nested braces/brackets/parens (struct-variant fields, closures, nested
    matches) are skipped via depth tracking and never contribute patterns.
    """
    named: set[str] = set()
    depth = 0
    in_pattern = True  # we open inside the match body, before the first arm's =>
    i = 0
    n = len(block)
    plen = len(arm_pat)
    while i < n:
        c = block[i]
        if c in "([{":
            depth += 1
            i += 1
            continue
        if c in ")]}":
            depth = max(depth - 1, 0)
            # Closing a top-level brace ends a block-bodied arm → next is a pattern.
            if depth == 1 and c == "}":
                in_pattern = True
            i += 1
            continue
        if depth == 1:
            if in_pattern and block.startswith("=>", i):
                in_pattern = False
                i += 2
                continue
            if not in_pattern and c == ",":
                # End of an expression-bodied arm → next is a pattern.
                in_pattern = True
                i += 1
                continue
            if in_pattern and block.startswith(arm_pat, i):
                j = i + plen
                m_end = j
                while m_end < n and (block[m_end].isalnum() or block[m_end] == "_"):
                    m_end += 1
                named.add(block[j:m_end])
                i = m_end
                continue
        i += 1
    return named


def audit_closed_domains(root: Path, manifest: Manifest, sa) -> list[Violation]:
    violations: list[Violation] = []

    # Resolve every declared domain's live variant set ONCE up front.
    domains: list[
        tuple[str, str, set[str], set[str]]
    ] = []  # (name, marker, variants, audited)
    for cd in manifest.closed_domains:
        enum_file = root / cd["enum_file"]
        enum_name = cd["enum_name"]
        if not enum_file.is_file():
            violations.append(
                Violation(
                    kind="closed_domain_structural",
                    severity="high",
                    location=cd["enum_file"],
                    detail=f"closed_domain {cd['name']}: enum_file does not exist",
                )
            )
            continue
        variants = sa._count_enum_variants(
            enum_file.read_text(errors="replace"), enum_name
        )
        if not variants:
            violations.append(
                Violation(
                    kind="closed_domain_structural",
                    severity="high",
                    location=f"{cd['enum_file']}::{enum_name}",
                    detail=(
                        f"closed_domain {cd['name']}: could not parse any variants of "
                        f"`enum {enum_name}` (the discovery parser found nothing — the "
                        "enum may have been renamed/moved; fix enum_file/enum_name)."
                    ),
                )
            )
            continue
        domains.append(
            (enum_name, enum_name + "::", variants, set(cd.get("audited_defaults", [])))
        )

    if not domains:
        return violations

    # Single file-read pass: scan each .rs source file once for every domain whose
    # `Enum::` marker it contains (the Comprehensive Analysis Spine — one pass, not
    # one pass per domain).
    for path in sa._iter_source_files(root, (".rs",)):
        if sa._is_generated(path):
            continue
        text = path.read_text(errors="replace")
        rel = None
        for enum_name, marker, variants, audited in domains:
            if marker not in text:
                continue
            if rel is None:
                rel = path.relative_to(root).as_posix()
            violations.extend(
                _scan_closed_domain_matches(sa, text, rel, enum_name, variants, audited)
            )
    return violations


# ---------------------------------------------------------------------------
# (4) Idempotence / freshness (opt-in: runs the generators)
# ---------------------------------------------------------------------------


def check_idempotence(root: Path, manifest: Manifest) -> list[Violation]:
    """Run each CI-checkable generator's `--check`. A pass proves the committed
    output equals a fresh render in THIS environment (the byte-stable/idempotent
    contract). A non-idempotent generator (e.g. nondeterministic ordering) is a
    REAL generator bug surfaced here, never silenced.

    Opt-in (--check-idempotence), NOT part of the default CI `--check`: the real
    CI freshness gate is the per-generator `--check` step in ci.yml, which runs in
    the Linux/LF CI environment. On a Windows checkout with autocrlf, a generator
    that byte-compares against an LF render can report a spurious staleness here
    (working-copy CRLF vs rendered LF); that is a host line-ending artifact, not a
    generator bug — re-confirm on Linux/CI before treating it as real drift."""
    violations: list[Violation] = []
    for g in manifest.generators:
        if g.get("discovery_only", False) or not g.get("ci_checkable", True):
            continue
        cmd = g.get("check_command")
        if not cmd:
            continue
        argv = [sys.executable, *cmd.split()]
        # Resolve the script path relative to root.
        argv[1] = str(root / cmd.split()[0])
        argv[2:] = cmd.split()[1:]
        try:
            proc = subprocess.run(
                argv,
                cwd=str(root),
                capture_output=True,
                text=True,
                timeout=180,
            )
        except subprocess.TimeoutExpired:
            violations.append(
                Violation(
                    kind="idempotence",
                    severity="high",
                    location=g["tool"],
                    detail="--check timed out after 180s",
                )
            )
            continue
        if proc.returncode != 0:
            tail = (proc.stderr or proc.stdout or "").strip().splitlines()[-4:]
            violations.append(
                Violation(
                    kind="idempotence",
                    severity="high",
                    location=g["tool"],
                    detail=(
                        "`--check` failed (committed output is stale or the "
                        "generator is non-idempotent): " + " | ".join(tail)
                    ),
                )
            )
    return violations


# ---------------------------------------------------------------------------
# Ratchet baseline (the closed-domain backlog, down-only)
# ---------------------------------------------------------------------------
#
# Two enforcement regimes, deliberately distinct (doc 59 §0/§8):
#   * HARD (must be 0 NOW): orphan generated files, ungated authorities, manifest
#     defects, idempotence failures. These are the Phase 1+2 institution — a new
#     violation is a build error, period.
#   * RATCHETED (down-only): closed-domain silent-default consumer matches. The
#     OpCode classifiers were migrated to generated predicates (count 0); the
#     Terminator consumer matches are the not-yet-migrated backlog. The gate
#     fails on a REGRESSION (a NEW silent-default match — the dispatch-handler-
#     mirror-hazard re-appearing), exactly the proven structural_audit.py
#     ratchet. A site is retired by covering all variants, making the default
#     fail-loud, routing through a generated *_table(), or (per-site, justified)
#     adding its file to the domain's audited_defaults.

BASELINE_REL = "tools/generator_manifest_baseline.json"


def _closed_domain_counts(violations: list[Violation]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for v in violations:
        if v.kind != "closed_domain":
            continue
        # The enum name is embedded as "closed domain <Name> ".
        marker = "closed domain "
        idx = v.detail.find(marker)
        if idx < 0:
            name = "_unknown"
        else:
            tail = v.detail[idx + len(marker) :]
            name = tail.split(" ", 1)[0].split(":", 1)[0]
        counts[name] = counts.get(name, 0) + 1
    return counts


def load_baseline(root: Path) -> dict[str, int]:
    path = root / BASELINE_REL
    if not path.is_file():
        return {}
    data = json.loads(path.read_text(encoding="utf-8"))
    return dict(data.get("closed_domain_silent_defaults", {}))


def _baseline_payload(counts: dict[str, int], sites: dict[str, list[str]]) -> dict:
    return {
        "_comment": (
            "Down-only ratchet for closed-domain silent-default consumer matches "
            "(tools/check_generator_manifest.py). Counts may only DECREASE: a new "
            "silent default over a closed enum domain is the dispatch-handler-"
            "mirror-hazard class and fails CI. Retire a site by covering all "
            "variants, making the default fail-loud, routing through a generated "
            "*_table() predicate, or adding its file to the domain's "
            "audited_defaults in generator_manifest.toml. Never re-pin UP."
        ),
        "closed_domain_silent_defaults": dict(sorted(counts.items())),
        "backlog_sites": {k: sorted(v) for k, v in sorted(sites.items())},
    }


def run_all(
    root: Path, *, with_idempotence: bool = False
) -> tuple[list[Violation], dict]:
    sa = _load_structural_audit(root)
    manifest = load_manifest(root)
    hard: list[Violation] = []
    hard.extend(detect_orphans(root, manifest, sa))
    hard.extend(check_gating(root, manifest))
    if with_idempotence:
        hard.extend(check_idempotence(root, manifest))
    domain_findings = audit_closed_domains(root, manifest, sa)
    # Structural domain errors (enum file missing / unparseable) are HARD; the
    # per-site silent-default consumer matches are the down-only ratchet backlog.
    hard.extend(v for v in domain_findings if v.kind == "closed_domain_structural")
    ratcheted = [v for v in domain_findings if v.kind == "closed_domain"]

    counts = _closed_domain_counts(ratcheted)
    baseline = load_baseline(root)
    # A regression: a domain's live count exceeds its baseline (or a brand-new
    # domain with no baseline entry has any silent defaults).
    regressions: list[Violation] = []
    for name, live in sorted(counts.items()):
        base = baseline.get(name)
        if base is None or live > base:
            shown = "no baseline" if base is None else str(base)
            regressions.append(
                Violation(
                    kind="closed_domain_regression",
                    severity="critical",
                    location=f"closed_domain:{name}",
                    detail=(
                        f"closed-domain {name} silent-default consumer matches "
                        f"regressed: live={live} > baseline={shown}. A NEW hand-written "
                        f"`match` over {name} with a silent `_ =>` default appeared — the "
                        "dispatch-handler-mirror-hazard class. Make it exhaustive-or-"
                        "fail-loud, route it through a generated *_table(), or justify it "
                        "in audited_defaults. (Do not re-pin the baseline up.)"
                    ),
                )
            )

    all_for_report = hard + regressions + ratcheted
    all_for_report.sort(key=lambda v: v.sort_key())
    # Hard violations + regressions are gate-failing; the raw ratcheted backlog
    # is reported but does not fail the gate when at/under baseline.
    gating = hard + regressions
    summary = {
        "generators": len(manifest.generators),
        "closed_domains": [cd["name"] for cd in manifest.closed_domains],
        "orphan_allowlist": len(manifest.orphan_generated),
        "hard_violations": len(hard),
        "closed_domain_counts": dict(sorted(counts.items())),
        "closed_domain_baseline": dict(sorted(baseline.items())),
        "regressions": len(regressions),
        "gating_violations": len(gating),
        "by_kind": {
            kind: sum(1 for v in hard if v.kind == kind)
            for kind in ("orphan", "ungated", "manifest", "idempotence")
        },
    }
    return all_for_report, summary, gating


def collect_backlog_sites(root: Path) -> tuple[dict[str, int], dict[str, list[str]]]:
    """Return (per-domain counts, per-domain site locations) for baseline writing."""
    sa = _load_structural_audit(root)
    manifest = load_manifest(root)
    ratcheted = audit_closed_domains(root, manifest, sa)
    counts = _closed_domain_counts(ratcheted)
    sites: dict[str, list[str]] = {}
    for v in ratcheted:
        marker = "closed domain "
        idx = v.detail.find(marker)
        name = (
            v.detail[idx + len(marker) :].split(" ", 1)[0].split(":", 1)[0]
            if idx >= 0
            else "_unknown"
        )
        sites.setdefault(name, []).append(v.location)
    # Ensure every declared domain has an entry, even at 0 (so a future regression
    # from 0 is caught — a new silent default in a currently-clean domain).
    for cd in manifest.closed_domains:
        counts.setdefault(cd["name"], 0)
        sites.setdefault(cd["name"], [])
    return counts, sites


def _format_report(violations: list[Violation], summary: dict, gating: list) -> str:
    lines: list[str] = []
    lines.append("Generator manifest meta-gate")
    lines.append(
        f"  generators={summary['generators']} "
        f"closed_domains={summary['closed_domains']} "
        f"orphan_allowlist={summary['orphan_allowlist']}"
    )
    lines.append(
        f"  closed-domain silent-default counts (down-only ratchet): "
        f"live={summary['closed_domain_counts']} "
        f"baseline={summary['closed_domain_baseline']}"
    )
    gate_ids = {id(v) for v in gating}
    if not gating:
        lines.append(
            "  GATE OK — no orphan generated files, every authority --check-gated, "
            "no closed-domain ratchet regression."
        )
    else:
        lines.append(f"  {len(gating)} GATE-FAILING VIOLATION(S):")
    for v in violations:
        tag = "GATE" if id(v) in gate_ids else "backlog"
        lines.append(f"  [{v.severity.upper()}] ({v.kind}) [{tag}] {v.location}")
        for chunk in _wrap(v.detail, 88):
            lines.append(f"        {chunk}")
    return "\n".join(lines)


def _wrap(text: str, width: int) -> list[str]:
    words = text.split()
    out: list[str] = []
    cur = ""
    for w in words:
        if cur and len(cur) + 1 + len(w) > width:
            out.append(cur)
            cur = w
        else:
            cur = f"{cur} {w}" if cur else w
    if cur:
        out.append(cur)
    return out


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=ROOT_DEFAULT)
    parser.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if any violation is found (CI mode)",
    )
    parser.add_argument(
        "--check-idempotence",
        action="store_true",
        help="additionally run each CI-checkable generator's --check (slower)",
    )
    parser.add_argument("--json", action="store_true", help="machine-readable output")
    parser.add_argument(
        "--update-baseline",
        action="store_true",
        help=(
            "rewrite tools/generator_manifest_baseline.json from the live "
            "closed-domain counts. Use ONLY when retiring sites (the count goes "
            "DOWN) — never to absorb a regression."
        ),
    )
    parser.add_argument(
        "--accept-correction",
        metavar="REASON",
        default=None,
        help=(
            "with --update-baseline, permit an UPWARD count move ONLY as an "
            "auditable measurement correction (e.g. the scanner was fixed and now "
            "finds previously-hidden real sites). REASON is recorded in the "
            "baseline file. This is the doc-52 §A.4 correction path, never a "
            "relaxation to hide a real new regression."
        ),
    )
    args = parser.parse_args(argv)

    root = args.root.resolve()

    if args.update_baseline:
        try:
            counts, sites = collect_backlog_sites(root)
        except ManifestError as exc:
            print(f"generator_manifest.toml is malformed: {exc}", file=sys.stderr)
            return 2
        prior = load_baseline(root)
        regressed = {
            k: (counts[k], prior[k])
            for k in counts
            if k in prior and counts[k] > prior[k]
        }
        if regressed and not args.accept_correction:
            print(
                "REFUSING to update baseline — these domains REGRESSED (count went "
                f"UP): {regressed}. Fix the new silent-default match(es) instead of "
                "re-pinning the ratchet up. (If this is a genuine MEASUREMENT "
                "correction — e.g. the scanner was fixed — re-run with "
                "--accept-correction 'reason'.)",
                file=sys.stderr,
            )
            return 2
        payload = _baseline_payload(counts, sites)
        if regressed and args.accept_correction:
            payload["correction"] = {
                "reason": args.accept_correction,
                "raised": {k: list(v) for k, v in regressed.items()},
            }
            print(
                f"ACCEPTED measurement correction ({args.accept_correction}); "
                f"raised: {regressed}",
                file=sys.stderr,
            )
        (root / BASELINE_REL).write_text(
            json.dumps(payload, indent=2) + "\n", encoding="utf-8"
        )
        print(f"wrote {BASELINE_REL}: {dict(sorted(counts.items()))}")
        return 0

    try:
        violations, summary, gating = run_all(
            root, with_idempotence=args.check_idempotence
        )
    except ManifestError as exc:
        print(f"generator_manifest.toml is malformed: {exc}", file=sys.stderr)
        return 2

    if args.json:
        print(
            json.dumps(
                {
                    "summary": summary,
                    "gating_violation_count": len(gating),
                    "violations": [
                        {
                            "kind": v.kind,
                            "severity": v.severity,
                            "location": v.location,
                            "detail": v.detail,
                            "gating": id(v) in {id(g) for g in gating},
                        }
                        for v in violations
                    ],
                },
                indent=2,
            )
        )
    else:
        print(_format_report(violations, summary, gating))

    if args.check:
        return 1 if gating else 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
