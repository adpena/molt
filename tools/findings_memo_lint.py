#!/usr/bin/env python3
"""APPARATUS A4 — the memo->registry lint (the pact Catalog-#344 port).

A ``docs/agent/*.md`` or ``memory/*.md`` line that STATES a measured finding —
a speedup ratio, a residual, a bench number, a parity result — but does NOT
reference a ``finding_id`` in the registry is prose that will silently rot: the
next session reads a stale number with no queryable, recalibratable record
behind it. This lint makes that line carry a reference or an explicit waiver.

A line is a VIOLATION when it matches the measured-finding vocabulary AND does
NOT carry either:

  * a ``finding_id`` reference (the literal token ``finding_id`` OR a registered
    ``snake_case_vN`` finding id appearing on the line), OR
  * a same-line waiver ``# FORMALIZATION_PENDING:<rationale>`` whose rationale is
    a substantive non-placeholder string (>= 4 real chars).

Measured-finding vocabulary (per APPARATUS A4):
    \\d+(\\.\\d+)?x  |  speedup  |  ns/  |  residual  |  bit-exact  |
    parity PASS  |  measured

LIFECYCLE — this gate lands WARN-ONLY (per the A9 discipline). molt has a real
existing backlog of un-formalized measured lines; a fresh gate must not flip the
whole tier-1 red. Default mode reports the backlog count and exits 0. ``--strict``
exits non-zero when the backlog is non-empty. The NAMED strict-flip condition is
recorded in the ``tools/molt_dev_gates.toml`` ``[[gate_flip]]`` registry
(``name = "findings_memo_lint"``, ``strict_when = "live_count == 0"``,
``count_cmd = "python3 tools/findings_memo_lint.py --count"``) and read by
``tools/check_gate_flips.py``: once the backlog is burned to zero, set
``state = "strict"`` and flip the ci_gate check to ``--strict``.

Run::

    python tools/findings_memo_lint.py                # warn-only, exit 0
    python tools/findings_memo_lint.py --strict       # exit 1 if backlog > 0
    python tools/findings_memo_lint.py --count        # integer backlog only
    python tools/findings_memo_lint.py --json
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass
from pathlib import Path

_THIS_FILE = Path(__file__).resolve()
REPO_ROOT = _THIS_FILE.parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))

from tools._io_utf8 import force_utf8_stdio  # noqa: E402

force_utf8_stdio()

# Default scan surface: the two doc tiers that hold operator-facing findings.
DEFAULT_SCAN_GLOBS = (
    "docs/agent/*.md",
    "memory/*.md",
)

# Measured-finding vocabulary. The ``x``-multiplier is bounded by ``\b`` on BOTH
# sides so a hex literal like ``0x1f`` (x followed by a hex word char, no
# boundary) does NOT match while a real ``1.65x`` / ``2.3x`` (x followed by a
# boundary) does.
MEASURED_VOCAB_RE = re.compile(
    r"\b\d+(?:\.\d+)?x\b"
    r"|speedup"
    # ``ns/`` is the nanoseconds-per unit — "ns" must be a standalone token, not
    # the tail of a longer word: ``(?<![a-zA-Z])`` excludes ``sessions/``,
    # ``plans/``, ``DNS/`` while still matching ``100ns/op`` / `` ns/iter``.
    r"|(?<![a-zA-Z])ns/"
    r"|residual"
    r"|bit-exact"
    r"|parity PASS"
    r"|measured",
    re.IGNORECASE,
)

# A same-line finding_id reference: the literal token, or an inline citation.
_FINDING_ID_TOKEN_RE = re.compile(r"\bfinding_id\b")

# A registered finding id is snake_case + trailing _vN.
_FINDING_ID_VALUE_RE = re.compile(r"\b[a-z][a-z0-9_]*_v\d+\b")

# The same-line waiver. Rationale must be substantive (>= 4 real chars, not a
# placeholder) so a bare ``# FORMALIZATION_PENDING:`` cannot self-waive.
_FORMALIZATION_PENDING_RE = re.compile(r"#\s*FORMALIZATION_PENDING:\s*(.+?)\s*$")
_PLACEHOLDER_RATIONALES = frozenset(
    {"<rationale>", "<reason>", "<why>", "todo", "tbd", "xxxx", "...."}
)

# File-level opt-out: a line ``<!-- findings-lint: skip-file <rationale> -->``
# suppresses the whole file (same substantive-rationale rule). For research/
# synthesis docs that DESCRIBE measured findings from elsewhere rather than
# asserting molt's own.
_SKIP_FILE_RE = re.compile(r"<!--\s*findings-lint:\s*skip-file\s+(.+?)\s*-->")


@dataclass(frozen=True)
class MemoViolation:
    file: str
    line: int
    text: str

    def __str__(self) -> str:
        return f"{self.file}:{self.line}: {self.text.strip()}"


def _rationale_is_substantive(rationale: str) -> bool:
    cleaned = (rationale or "").strip()
    if len(cleaned) < 4:
        return False
    if cleaned.lower() in _PLACEHOLDER_RATIONALES:
        return False
    return True


def line_is_measured(line: str) -> bool:
    return bool(MEASURED_VOCAB_RE.search(line))


def line_is_satisfied(line: str, registered_ids: frozenset[str] | None = None) -> bool:
    """A measured line is satisfied by a finding_id reference or a valid waiver."""
    # Same-line FORMALIZATION_PENDING waiver with a substantive rationale.
    m = _FORMALIZATION_PENDING_RE.search(line)
    if m and _rationale_is_substantive(m.group(1)):
        return True
    # The literal ``finding_id`` token (inline citation).
    if _FINDING_ID_TOKEN_RE.search(line):
        return True
    # A registered finding id value appearing on the line.
    if registered_ids:
        for tok in _FINDING_ID_VALUE_RE.findall(line):
            if tok in registered_ids:
                return True
    return False


def _load_registered_ids() -> frozenset[str]:
    """Best-effort: the set of registered finding ids (for inline-id citations)."""
    try:
        from tools.findings_registry import query_findings

        return frozenset(f.finding_id for f in query_findings())
    except Exception:  # noqa: BLE001 — lint must never hard-fail on registry issues
        return frozenset()


def scan_text(
    text: str, *, registered_ids: frozenset[str] | None = None, rel: str = "<text>"
) -> list[MemoViolation]:
    """Return the unformalized measured-finding lines in ``text``."""
    lines = text.splitlines()
    # Whole-file opt-out (with substantive rationale).
    for line in lines:
        sm = _SKIP_FILE_RE.search(line)
        if sm and _rationale_is_substantive(sm.group(1)):
            return []
    violations: list[MemoViolation] = []
    in_fence = False
    for i, line in enumerate(lines, start=1):
        stripped = line.lstrip()
        if stripped.startswith("```") or stripped.startswith("~~~"):
            in_fence = not in_fence
            continue
        if in_fence:
            # Code blocks are examples/output, not asserted findings.
            continue
        if not line_is_measured(line):
            continue
        if line_is_satisfied(line, registered_ids):
            continue
        violations.append(MemoViolation(rel, i, line))
    return violations


def scan_file(
    path: Path, *, root: Path, registered_ids: frozenset[str] | None = None
) -> list[MemoViolation]:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    try:
        rel = path.relative_to(root).as_posix()
    except ValueError:
        rel = path.as_posix()
    return scan_text(text, registered_ids=registered_ids, rel=rel)


def _iter_target_files(root: Path, globs: tuple[str, ...]) -> list[Path]:
    out: list[Path] = []
    seen: set[Path] = set()
    for pattern in globs:
        for path in sorted(root.glob(pattern)):
            if path.is_file() and path not in seen:
                seen.add(path)
                out.append(path)
    return out


def scan_tree(
    root: Path | None = None,
    *,
    globs: tuple[str, ...] = DEFAULT_SCAN_GLOBS,
    registered_ids: frozenset[str] | None = None,
) -> list[MemoViolation]:
    root = root or REPO_ROOT
    if registered_ids is None:
        registered_ids = _load_registered_ids()
    violations: list[MemoViolation] = []
    for path in _iter_target_files(root, globs):
        violations.extend(scan_file(path, root=root, registered_ids=registered_ids))
    return violations


def main(argv: list[str] | None = None) -> int:
    argv = sys.argv[1:] if argv is None else argv
    parser = argparse.ArgumentParser(description="memo->registry lint (APPARATUS A4).")
    parser.add_argument(
        "--strict",
        action="store_true",
        help="exit 1 when the backlog is non-empty (post-flip mode; strict_when="
        "'live_count == 0' [[gate_flip]] in molt_dev_gates.toml)",
    )
    parser.add_argument(
        "--check", action="store_true", help="alias for the default warn-only run"
    )
    parser.add_argument(
        "--count",
        action="store_true",
        help="print ONLY the integer backlog to stdout (the [[gate_flip]] "
        "count_cmd source that feeds live_count in check_gate_flips.py)",
    )
    parser.add_argument("--json", action="store_true")
    parser.add_argument(
        "paths", nargs="*", help="explicit files to scan (default: the doc tiers)"
    )
    args = parser.parse_args(argv)

    registered_ids = _load_registered_ids()
    if args.paths:
        violations: list[MemoViolation] = []
        for p in args.paths:
            violations.extend(
                scan_file(
                    Path(p).resolve(), root=REPO_ROOT, registered_ids=registered_ids
                )
            )
    else:
        violations = scan_tree(registered_ids=registered_ids)

    backlog = len(violations)
    if args.count:
        # Integer-only stdout for check_gate_flips.py count_cmd (live_count).
        print(backlog)
        return 0
    if args.json:
        print(
            json.dumps(
                {
                    "backlog_count": backlog,
                    "strict": args.strict,
                    "violations": [
                        {"file": v.file, "line": v.line, "text": v.text.strip()}
                        for v in violations
                    ],
                },
                indent=2,
            )
        )
    else:
        if backlog == 0:
            print("findings-memo-lint: OK (0 unformalized measured-finding lines)")
        else:
            mode = "STRICT" if args.strict else "WARN-ONLY"
            print(
                f"findings-memo-lint: {mode} — {backlog} measured-finding line(s) "
                "state a number with no finding_id reference. Formalize each via "
                "tools/findings_registry.py and cite its finding_id, or mark the "
                "line '# FORMALIZATION_PENDING:<rationale>'.",
                file=sys.stderr,
            )
            for v in violations[:50]:
                print(f"  {v}", file=sys.stderr)
            if backlog > 50:
                print(f"  ... and {backlog - 50} more", file=sys.stderr)

    if args.strict and backlog > 0:
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
