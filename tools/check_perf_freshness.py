#!/usr/bin/env python3
"""Fail-closed PERF-FRESHNESS gate - no stale perf number masquerades as current.

molt has exactly one citable perf source of truth: the canonical scoreboard
command exported by ``tools/perf_authority.py``. Every other perf artifact -
dated result JSONs, hand-written markdown tables, triage snapshots - is a
point-in-time record that goes stale. A stale record is dangerous precisely
because it still reads as a confident table of ratios; a design agent handed a
3-month-old ``0.01x`` row can rank it #1 and chase a regression that no longer
exists (the exact forensic mislead this gate exists to kill).

This checker flags any perf doc/store that is BOTH (a) presents perf numbers and
(b) is stale - older than the staleness horizon OR measured on a tree whose
``git_rev`` is not an ancestor of ``origin/main`` - UNLESS it has been explicitly
stamped with the stale banner (``perf_authority.STALE_BANNER_MARK``). A stamped
doc is an acknowledged historical record and passes; an UNSTAMPED stale doc is a
hazard and fails the gate.

The canonical board itself is exempt: it carries its own ``authoritative`` /
``FAIL_STALE`` provenance machinery (``perf_scoreboard.gather_provenance``) and is
the live truth, so freshness is enforced there, not here.

Usage::

    python3 tools/check_perf_freshness.py            # gate (exit 1 on hazard)
    python3 tools/check_perf_freshness.py --json      # machine-readable report
    python3 tools/check_perf_freshness.py --max-age-days 14
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
TOOLS_ROOT = REPO_ROOT / "tools"
if str(TOOLS_ROOT) not in sys.path:
    sys.path.insert(0, str(TOOLS_ROOT))

import perf_authority as pa  # noqa: E402

# Directories scanned for perf artifacts.
SCAN_DIRS = [
    REPO_ROOT / "bench",
    REPO_ROOT / "bench" / "results",
]

# The canonical board's own directory is governed by perf_scoreboard's
# provenance gate, not this freshness checker.
EXEMPT_DIRS = {
    REPO_ROOT / "bench" / "scoreboard",
}

# A doc "presents CITABLE perf numbers" if it contains a measured ratio token:
# a numeric speedup like ``1.12x`` / ``0.01x`` (the shape a design agent ranks),
# or one of the scoreboard ratio field names emitted into a results table. A
# bare prose mention of the WORD "speedup" (e.g. "expected 20-40% speedup" in a
# root-cause note) is deliberately NOT matched - that is a design projection,
# not a stale benchmark result. Only genuine citable numbers trip the gate.
_RATIO_TOKEN_RE = re.compile(
    r"\d+\.\d+\s*x\b"  # measured ratio, e.g. 1.12x / 0.01x
    r"|molt_cpython_ratio|molt_speedup|warm_speedup|cold_speedup"  # result fields
    r"|\bspeedup\b\s*[:|=]",  # a "Speedup:" / "Speedup |" column header/value
    re.IGNORECASE,
)

# Date embedded in a filename, e.g. *_20260320.* or *-20260320T*.
_FILENAME_DATE_RE = re.compile(r"(20\d{2})(\d{2})(\d{2})")

# git_rev embedded in a markdown line, e.g. "Git rev: <40-hex>".
_DOC_GIT_REV_RE = re.compile(r"git[\s_]*rev[:\s`]*([0-9a-f]{7,40})", re.IGNORECASE)
# generated_at / Date embedded in a markdown line.
_DOC_DATE_RE = re.compile(
    r"(?:generated_at|date)[:\s`*]*([0-9]{4}-[0-9]{2}-[0-9]{2}[T0-9:+\-Z.]*)",
    re.IGNORECASE,
)


def _read_text(path: Path) -> str:
    # Perf docs are UTF-8; tolerate the recurring Windows cp1252 hazard by
    # decoding with errors="replace" rather than crashing the gate.
    try:
        return path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return path.read_bytes().decode("utf-8", errors="replace")


def _iter_perf_docs() -> list[Path]:
    out: list[Path] = []
    seen: set[Path] = set()
    for base in SCAN_DIRS:
        if not base.exists():
            continue
        for path in sorted(base.glob("*")):
            if path.is_dir():
                continue
            if path.suffix.lower() not in (".md", ".txt"):
                continue
            if path.parent in EXEMPT_DIRS:
                continue
            rp = path.resolve()
            if rp in seen:
                continue
            seen.add(rp)
            out.append(path)
    return out


def _doc_git_rev(text: str) -> str | None:
    m = _DOC_GIT_REV_RE.search(text)
    if not m:
        return None
    rev = m.group(1)
    return rev if rev not in ("unknown",) else None


def _doc_generated_at(text: str, path: Path) -> str | None:
    m = _DOC_DATE_RE.search(text)
    if m:
        return m.group(1)
    fn = _FILENAME_DATE_RE.search(path.name)
    if fn:
        return f"{fn.group(1)}-{fn.group(2)}-{fn.group(3)}"
    return None


def evaluate_doc(path: Path, *, max_age_days: float, now: dt.datetime) -> dict:
    """Classify one perf doc. Returns a record with verdict + reasons."""
    text = _read_text(path)
    try:
        rel = str(path.resolve().relative_to(REPO_ROOT)).replace("\\", "/")
    except ValueError:
        # Path outside the repo (e.g. a unit-test tmp dir): fall back to the
        # file name. The classification logic is path-independent.
        rel = path.name
    has_numbers = bool(_RATIO_TOKEN_RE.search(text))
    stamped = pa.STALE_BANNER_MARK in text
    git_rev = _doc_git_rev(text)
    generated_at = _doc_generated_at(text, path)
    age = pa.doc_age_days(generated_at, now=now)
    ancestor = pa.git_rev_is_ancestor_of_origin(git_rev)

    reasons: list[str] = []
    if age is not None and age > max_age_days:
        reasons.append(
            f"generated_at {generated_at} is {age:.0f}d old (> {max_age_days:.0f}d)"
        )
    if ancestor is False:
        reasons.append(f"git_rev {git_rev} is NOT an ancestor of origin/main")
    # FAIL CLOSED: a doc that presents perf numbers but proves freshness through
    # NEITHER a parseable generated_at NOR an ancestor-verified git_rev is
    # UNDATEABLE - it cannot demonstrate it reflects the current tree, so it is
    # treated as stale. (A stale doc with all-zero / unknown rev and no date is
    # the classic abandoned snapshot.) Stamping it clears the hazard.
    fresh_proof = (age is not None and age <= max_age_days) or (ancestor is True)
    if has_numbers and not fresh_proof and not reasons:
        reasons.append(
            "cannot prove freshness: no parseable generated_at and no "
            "ancestor-verified git_rev (undateable perf snapshot)"
        )

    is_stale = bool(reasons)
    # A doc is a HAZARD iff it presents perf numbers, is stale/undateable, and
    # is NOT stamped as an acknowledged historical record.
    hazard = has_numbers and is_stale and not stamped

    if not has_numbers:
        verdict = "no-perf-numbers"
    elif not is_stale:
        verdict = "fresh"
    elif stamped:
        verdict = "stale-stamped"
    else:
        verdict = "stale-hazard"

    return {
        "path": rel,
        "verdict": verdict,
        "hazard": hazard,
        "has_perf_numbers": has_numbers,
        "stamped": stamped,
        "git_rev": git_rev,
        "generated_at": generated_at,
        "age_days": round(age, 1) if age is not None else None,
        "git_rev_ancestor_of_origin": ancestor,
        "reasons": reasons,
    }


def run(max_age_days: float) -> dict:
    now = dt.datetime.now(dt.timezone.utc)
    records = [
        evaluate_doc(p, max_age_days=max_age_days, now=now) for p in _iter_perf_docs()
    ]
    hazards = [r for r in records if r["hazard"]]
    return {
        "kind": "perf_freshness",
        "max_age_days": max_age_days,
        "canonical_gate": pa.CANONICAL_GATE,
        "docs_scanned": len(records),
        "hazards": len(hazards),
        "records": records,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--json", action="store_true", help="machine-readable report")
    parser.add_argument(
        "--max-age-days",
        type=float,
        default=pa.DEFAULT_STALE_DAYS,
        help=f"staleness horizon in days (default: {pa.DEFAULT_STALE_DAYS})",
    )
    ns = parser.parse_args(argv)
    report = run(ns.max_age_days)

    if ns.json:
        print(json.dumps(report, indent=2, sort_keys=True))
    else:
        print(
            f"perf-freshness: scanned {report['docs_scanned']} perf doc(s); "
            f"{report['hazards']} unstamped-stale hazard(s)."
        )
        for rec in report["records"]:
            if rec["hazard"]:
                why = "; ".join(rec["reasons"])
                print(f"  HAZARD  {rec['path']}: {why}")
                print(
                    f"          -> stamp it with the stale banner "
                    f"(perf_authority.STALE_BANNER) or delete it. "
                    f"Cite only `{report['canonical_gate']}`."
                )
        if report["hazards"] == 0:
            print("  OK: every perf doc presenting numbers is fresh or stamped stale.")

    return 1 if report["hazards"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
