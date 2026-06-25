#!/usr/bin/env python3
"""Single authority boundary for what is - and is NOT - a citable perf number.

molt has exactly ONE canonical performance source of truth:
``tools/perf_scoreboard.py`` run over the native+LLVM release-fast core board
with cold+warm samples, repeat-CI classification, the quiescence guard, and a
git-ancestry/dirty-tree ``authoritative`` provenance check. Every OTHER lane
that emits wall-clock numbers - ``tools/bench.py`` (daemon batch builder) and
``bench/harness.py`` (the dev/correctness differential harness) - is
NON-CANONICAL and must SELF-IDENTIFY as such so a design agent never cites it.

This module is that shared boundary. It owns three primitives that all the
non-canonical lanes route through, instead of each re-implementing them:

  1. :func:`non_canonical_provenance` - the stamp every non-canonical JSON
     carries: ``authoritative=False``, ``source=non-canonical``, the ACTUAL
     profile, and a pointer to the canonical gate. It reuses the field
     vocabulary of ``perf_scoreboard.gather_provenance`` so a reader sees the
     same keys (``authoritative`` / ``authoritative_reason``) on every board.
  2. :func:`safe_speedup` - the ONE place a ``cpython_time / molt_time`` ratio
     is computed. A missing/None/non-positive molt time (the build-failure /
     daemon-crash / runaway shape) yields ``None``, NEVER a finite ratio. This
     is the mechanical kill for "a BUILD_FAILED cell rendered as ~0.01x".
  3. freshness checks (:func:`git_rev_is_ancestor_of_origin`, :func:`doc_age_days`,
     :func:`STALE_BANNER`) used by freshness consumers to flag any perf doc whose
     ``git_rev`` is not on origin/main or that is older than N days.

The native+LLVM release-fast core scoreboard command is the daily contract; it
is the only lane permitted to emit ``authoritative=true``. See
``tools/PERF_AUTHORITY.md`` for the consumer rule.
"""

from __future__ import annotations

import datetime as dt
import math
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# The one canonical gate. Cited in every non-canonical stamp + every stale
# banner so a reader is always pointed back at the live truth.
CANONICAL_GATE = (
    "tools/perf_scoreboard.py --set core --backend native --backend llvm "
    "--profile release-fast --samples 5 --warmup 2 --repeat 5 --classify "
    "--require-quiescent"
)

# The release-fast cargo profile is the daily perf contract. A board is only
# *eligible* for authoritative=true when it is the canonical gate at this
# profile; the non-canonical lanes are never authoritative regardless of
# profile, but we record the actual profile so the stamp is honest.
CONTRACT_PROFILE = "release-fast"

# Default staleness horizon for perf docs (days). A markdown perf snapshot
# older than this - OR whose git_rev is not an ancestor of origin/main - is
# stale. 30 days is generous: the canonical board is regenerated per-release; a
# month-old hand-written table is lore, not data.
DEFAULT_STALE_DAYS = 30

# The banner stamped at the top of every stale perf markdown. Self-identifies
# the doc as non-authoritative and points at the live gate.
STALE_BANNER_MARK = "<!-- PERF-AUTHORITY:stale -->"


def STALE_BANNER(*, generated_at: str, git_rev: str | None) -> str:
    """Return the freshness banner block to prepend to a stale perf markdown."""
    rev = git_rev or "unknown"
    return (
        f"{STALE_BANNER_MARK}\n"
        "> **STALE PERF SNAPSHOT - NOT AUTHORITATIVE.**\n"
        ">\n"
        f"> The ONLY citable perf source of truth is `{CANONICAL_GATE}`\n"
        "> (release-fast, cold+warm, quiescent, with a git-ancestry provenance\n"
        "> check). This file is a point-in-time snapshot kept for historical\n"
        "> context only; its numbers may reflect a different profile, a stale\n"
        "> tree, or an already-fixed regression. Do NOT rank or cite it.\n"
        f">\n"
        f"> - generated_at: `{generated_at}`\n"
        f"> - git_rev: `{rev}`\n"
    )


def _git_output(args: list[str]) -> str | None:
    """Run a bounded read-only git probe; None on any failure (no raise)."""
    try:
        res = subprocess.run(
            ["git", *args],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode != 0:
        return None
    out = res.stdout.strip()
    return out or None


def safe_speedup(cpython_time: float | None, molt_time: float | None) -> float | None:
    """Speedup = cpython_time / molt_time, or None when it is not measurable.

    Returns ``None`` - never a finite number - whenever either time is missing
    (None) or non-positive. This is the single guard that makes it STRUCTURALLY
    IMPOSSIBLE for a build failure / daemon crash / runaway (which leave
    ``molt_time = None``) to render as a finite ratio such as ``~0.01x``.

    Direction matches the canonical board: ``> 1.0`` means molt is faster.
    """
    if cpython_time is None or molt_time is None:
        return None
    # Reject non-finite / non-positive denominators and numerators. A zero or
    # negative molt_time is not a real measurement; it must not become a ratio.
    try:
        cpy = float(cpython_time)
        mlt = float(molt_time)
    except (TypeError, ValueError):
        return None
    # Reject any non-finite (NaN/inf) time outright - those are not real
    # measurements. Combined with the >0 check this makes it impossible for a
    # degenerate time to produce a finite OR infinite ratio.
    if not (math.isfinite(cpy) and math.isfinite(mlt)):
        return None
    if not (cpy > 0.0 and mlt > 0.0):
        return None
    return cpy / mlt


def non_canonical_provenance(
    *,
    profile: str,
    source: str,
    git_rev: str | None = None,
) -> dict[str, object]:
    """Provenance stamp marking a perf JSON as NON-CANONICAL (never citable).

    Every lane that is not ``perf_scoreboard.py --profile release-fast`` emits
    this block so the numbers self-identify. ``authoritative`` is always False;
    ``authoritative_reason`` names why; ``source`` and ``profile`` let a reader
    see exactly which non-canonical lane and profile produced the file.

    Reuses the ``authoritative`` / ``authoritative_reason`` field names from
    ``perf_scoreboard.gather_provenance`` so boards share one vocabulary.
    """
    return {
        "authoritative": False,
        "authoritative_reason": (
            f"non-canonical lane ({source}); the only citable perf source is "
            f"`{CANONICAL_GATE}`"
        ),
        "source": "non-canonical",
        "lane": source,
        "profile": profile,
        "canonical_gate": CANONICAL_GATE,
        "git_rev": git_rev
        if git_rev is not None
        else (_git_output(["rev-parse", "HEAD"]) or "unknown"),
    }


def git_rev_is_ancestor_of_origin(git_rev: str | None) -> bool | None:
    """Is ``git_rev`` an ancestor of (or equal to) ``origin/main``?

    Returns True/False, or None when it cannot be determined (unknown rev,
    origin/main ref absent, or git unavailable). A perf doc whose recorded
    ``git_rev`` is NOT an ancestor of origin/main was measured on a tree that
    is not the shipped contract - freshness consumers flag it.
    """
    if not git_rev or git_rev == "unknown":
        return None
    origin = _git_output(["rev-parse", "origin/main"])
    if origin is None:
        return None
    # `git merge-base --is-ancestor A B` exits 0 iff A is an ancestor of B
    # (or A == B). Run it directly so we get the exit code, not stdout.
    try:
        res = subprocess.run(
            ["git", "merge-base", "--is-ancestor", git_rev, origin],
            cwd=str(REPO_ROOT),
            capture_output=True,
            text=True,
            check=False,
            timeout=30,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    if res.returncode == 0:
        return True
    if res.returncode == 1:
        return False
    # rc 128 etc. => unknown commit / bad ref: undeterminable, not "fresh".
    return None


def doc_age_days(
    generated_at: str | None, *, now: dt.datetime | None = None
) -> float | None:
    """Age in days of an ISO-8601 ``generated_at`` timestamp, or None if unparseable."""
    if not generated_at:
        return None
    text = generated_at.strip()
    # Accept a trailing 'Z' (UTC) which datetime.fromisoformat rejects before 3.11
    # in some forms; normalize to +00:00.
    if text.endswith("Z"):
        text = text[:-1] + "+00:00"
    try:
        ts = dt.datetime.fromisoformat(text)
    except ValueError:
        # Date-only form (e.g. "2026-03-25") used by some hand-written docs.
        try:
            ts = dt.datetime.strptime(text, "%Y-%m-%d")
        except ValueError:
            return None
    if ts.tzinfo is None:
        ts = ts.replace(tzinfo=dt.timezone.utc)
    current = now or dt.datetime.now(dt.timezone.utc)
    if current.tzinfo is None:
        current = current.replace(tzinfo=dt.timezone.utc)
    return (current - ts).total_seconds() / 86400.0
