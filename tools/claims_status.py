#!/usr/bin/env python3
"""CLAIMS terminal-status vocabulary + live-vs-retired classifier (APPARATUS A11).

molt's ``docs/agent/CLAIMS.md`` is the git-atomic lane-custody ledger (M11): one
agent claims a SOLO lane, others back off. The gap pact already closed and molt
had not: a claim could only be freed by ``RELEASED`` / ``COMPLETE`` (a positive
completion), so a lane whose premise was DISPROVEN, whose implementation was
MEASURED and then retired, or that went quietly dead had no honest terminal state
-- it either sat CLAIMED forever (silently blocking a lane) or was reclaimed by a
race the staleness heuristic could get wrong. pact's answer is a self-retirement
vocabulary: a claim carries the seed of its OWN retirement, distinct from
RELEASED (the "Godel self-retirement" property, APPARATUS 1.9 / M64).

This module adds that vocabulary to molt and a classifier over it:

  LIVE statuses      CLAIMED / PROGRESS / RECLAIM     -- an agent owns the lane
  TERMINAL statuses  COMPLETE / RELEASED              -- positive completion / handoff
                     FALSIFIED                        -- the lane's PREMISE was disproven
                     MEASURED_IMPLEMENTATION_RETIRED  -- built + measured, then retired
                                                         (superseded by a better result)
                     STALE_ASSUMED_DEAD               -- no objective liveness; retired
                                                         with the liveness evidence cited
                     SUPERSEDED                       -- a different lane subsumed this one

A TERMINAL row RETIRES the lane (it no longer blocks a claimant). A LIVE row that
has had no fresh PROGRESS for > STALE_HOURS is flagged STALE (advisory only -- a
silent worktree may still be alive; see CLAIMS.md 6 for the objective-liveness
bar before an actual RECLAIM). So this tool never reclaims and never fails a
build: it REPORTS live-vs-retired counts and flags stale CLAIMED rows so stale
custody cannot silently block a lane.

Two modes:
  * default (warn-only, wired into ci_gate as ``claims-status-warn``): parse the
    real CLAIMS.md, print live / retired / stale counts + any stale CLAIMED rows.
    ALWAYS exits 0 -- a stale claim is flagged, never a hard CI failure.
  * ``--check`` (falsifiable self-test, wired into ci_gate tier-1 + a
    check_gate_liveness canary): feed the PURE classifier synthetic fixtures and
    fail (exit 1) if a FALSIFIED row is not RETIRED, a stale CLAIMED is not
    STALE, or a fresh CLAIMED is not LIVE -- the "a gate that cannot fire
    certifies nothing" meta-bug (M34/M42), pointed at this classifier.

Pure + stdlib-only; the classifier (``classify_status`` / ``summarize``) is
unit-tested without a crafted git HEAD. ASCII + UTF-8-explicit (M43).
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path

# UTF-8 backstop (M43): this tool relays CLAIMS.md notes that carry em-dashes and
# other non-cp1252 bytes; a stray byte must never abort the report on Windows.
try:  # pragma: no cover - trivial import shim
    from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
    try:
        from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
    except Exception:

        def _force_utf8_stdio(*, errors: str = "backslashreplace") -> None:
            for stream in (sys.stdout, sys.stderr):
                reconfigure = getattr(stream, "reconfigure", None)
                if reconfigure is None:
                    continue
                try:
                    reconfigure(encoding="utf-8", errors=errors)
                except (AttributeError, ValueError, OSError):
                    pass


ROOT = Path(__file__).resolve().parents[1]
CLAIMS_REL = "docs/agent/CLAIMS.md"
STALE_HOURS = 4.0

# The three LIVE statuses an agent posts while it owns a lane.
LIVE_STATUSES: frozenset[str] = frozenset({"CLAIMED", "PROGRESS", "RECLAIM"})

# The pre-existing positive terminals (a completion / a handoff).
POSITIVE_TERMINALS: frozenset[str] = frozenset({"COMPLETE", "RELEASED"})

# The APPARATUS A11 self-retirement vocabulary: a claim retires ITSELF with
# evidence, distinct from a positive COMPLETE/RELEASED.
SELF_RETIRE_TERMINALS: frozenset[str] = frozenset(
    {
        "FALSIFIED",
        "MEASURED_IMPLEMENTATION_RETIRED",
        "STALE_ASSUMED_DEAD",
        "SUPERSEDED",
    }
)

TERMINAL_STATUSES: frozenset[str] = POSITIVE_TERMINALS | SELF_RETIRE_TERMINALS
ALL_STATUSES: frozenset[str] = LIVE_STATUSES | TERMINAL_STATUSES

# Classification buckets returned by classify_status().
LIVE = "LIVE"
STALE = "STALE"
RETIRED = "RETIRED"

_TS_FMT = "%Y-%m-%dT%H:%M:%SZ"


@dataclass(frozen=True)
class Row:
    lane: str
    agent: str
    utc: str
    status: str
    note: str


@dataclass
class LaneState:
    lane: str
    row: Row
    klass: str  # LIVE | STALE | RETIRED
    age_hours: float | None = None


@dataclass
class Summary:
    live: list[LaneState] = field(default_factory=list)
    stale: list[LaneState] = field(default_factory=list)
    retired: list[LaneState] = field(default_factory=list)

    def as_dict(self) -> dict[str, object]:
        def _one(s: LaneState) -> dict[str, object]:
            return {
                "lane": s.lane,
                "agent": s.row.agent,
                "utc": s.row.utc,
                "status": s.row.status,
                "class": s.klass,
                "age_hours": (None if s.age_hours is None else round(s.age_hours, 2)),
                "note": s.row.note,
            }

        return {
            "counts": {
                "live": len(self.live),
                "stale": len(self.stale),
                "retired": len(self.retired),
            },
            "live": [_one(s) for s in self.live],
            "stale": [_one(s) for s in self.stale],
            "retired": [_one(s) for s in self.retired],
        }


# ------------------------------- pure parsing --------------------------------


def parse_rows(claims_text: str) -> list[Row]:
    """Parse every ``## Log`` table row (all lanes), oldest -> newest.

    Mirrors ``tools/claim_lane.py`` cell-splitting but keeps ALL lanes and rejoins
    any note cells that themselves contained a ``|`` so a pipe in the evidence
    note does not truncate it.
    """
    rows: list[Row] = []
    in_log = False
    for line in claims_text.splitlines():
        if line.startswith("## Log"):
            in_log = True
            continue
        if not in_log or not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 4:
            continue
        if cells[0].lower() in {"lane", "------"} or cells[0].startswith("-"):
            continue
        status = cells[3]
        if status not in ALL_STATUSES:  # header / placeholder / free-text row
            continue
        note = " | ".join(cells[4:]) if len(cells) > 4 else ""
        rows.append(
            Row(lane=cells[0], agent=cells[1], utc=cells[2], status=status, note=note)
        )
    return rows


def latest_by_lane(rows: list[Row]) -> dict[str, Row]:
    """The current (last-in-file) row per lane; append-only newest-at-bottom."""
    latest: dict[str, Row] = {}
    for r in rows:
        latest[r.lane] = r
    return latest


def _age_hours(utc: str, now: _dt.datetime) -> float | None:
    try:
        ts = _dt.datetime.strptime(utc, _TS_FMT).replace(tzinfo=_dt.timezone.utc)
    except ValueError:
        return None
    return (now - ts).total_seconds() / 3600.0


def classify_status(
    row: Row, now: _dt.datetime, stale_hours: float = STALE_HOURS
) -> tuple[str, float | None]:
    """Classify one lane's latest row into (LIVE | STALE | RETIRED, age_hours).

    * a TERMINAL status (COMPLETE / RELEASED / FALSIFIED /
      MEASURED_IMPLEMENTATION_RETIRED / STALE_ASSUMED_DEAD / SUPERSEDED) -> RETIRED
    * a LIVE status (CLAIMED / PROGRESS / RECLAIM) older than ``stale_hours`` ->
      STALE (advisory: flagged, never auto-reclaimed)
    * a LIVE status within the window (or with an unparseable timestamp -- we
      never assert stale on uncertainty) -> LIVE
    """
    if row.status in TERMINAL_STATUSES:
        return RETIRED, _age_hours(row.utc, now)
    age = _age_hours(row.utc, now)
    if age is not None and age > stale_hours:
        return STALE, age
    return LIVE, age


def summarize(
    rows: list[Row], now: _dt.datetime, stale_hours: float = STALE_HOURS
) -> Summary:
    """Bucket every lane's current state into live / stale / retired."""
    summary = Summary()
    for lane, row in sorted(latest_by_lane(rows).items()):
        klass, age = classify_status(row, now, stale_hours)
        state = LaneState(lane=lane, row=row, klass=klass, age_hours=age)
        if klass == LIVE:
            summary.live.append(state)
        elif klass == STALE:
            summary.stale.append(state)
        else:
            summary.retired.append(state)
    return summary


# --------------------------------- reporting ---------------------------------


def _read_claims(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except OSError as exc:
        print(f"claims_status: cannot read {path}: {exc}", file=sys.stderr)
        return ""


def _print_report(summary: Summary) -> None:
    c = summary.as_dict()["counts"]
    print(
        f"CLAIMS custody: {c['live']} live, {c['stale']} stale, "
        f"{c['retired']} retired ({c['live'] + c['stale'] + c['retired']} lanes)."
    )
    if summary.stale:
        print(
            "\nStale CLAIMED lanes (no fresh PROGRESS for > "
            f"{STALE_HOURS:g}h -- advisory, verify objective liveness per "
            "CLAIMS.md 6 before any RECLAIM):"
        )
        for s in summary.stale:
            age = "?" if s.age_hours is None else f"{s.age_hours:.1f}h"
            print(f"  [STALE {age}] {s.lane} -- {s.row.agent} ({s.row.status})")
    if summary.live:
        print("\nLive lanes:")
        for s in summary.live:
            age = "?" if s.age_hours is None else f"{s.age_hours:.1f}h"
            print(f"  [LIVE {age}] {s.lane} -- {s.row.agent} ({s.row.status})")


# ------------------------------- CI self-test --------------------------------
# ``--check`` feeds the PURE classifier known fixtures and fails (exit 1) if the
# terminal vocabulary or the staleness split ever stops working. A FALSIFIED /
# SUPERSEDED / MEASURED_IMPLEMENTATION_RETIRED / STALE_ASSUMED_DEAD row MUST be
# RETIRED (not live); a stale CLAIMED MUST be STALE; a fresh CLAIMED MUST be LIVE.

_NOW = _dt.datetime(2026, 7, 11, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _selftest_cases() -> list[tuple[str, Row, str]]:
    def row(status: str, hours_ago: float) -> Row:
        ts = (_NOW - _dt.timedelta(hours=hours_ago)).strftime(_TS_FMT)
        return Row(lane="L", agent="a", utc=ts, status=status, note="")

    return [
        ("fresh-claimed-is-live", row("CLAIMED", 0.5), LIVE),
        ("fresh-progress-is-live", row("PROGRESS", 1.0), LIVE),
        ("old-claimed-is-stale", row("CLAIMED", 9.0), STALE),
        ("old-progress-is-stale", row("PROGRESS", 12.0), STALE),
        ("complete-is-retired", row("COMPLETE", 100.0), RETIRED),
        ("released-is-retired", row("RELEASED", 100.0), RETIRED),
        ("falsified-is-retired", row("FALSIFIED", 0.1), RETIRED),
        (
            "measured-impl-retired-is-retired",
            row("MEASURED_IMPLEMENTATION_RETIRED", 0.1),
            RETIRED,
        ),
        ("stale-assumed-dead-is-retired", row("STALE_ASSUMED_DEAD", 0.1), RETIRED),
        ("superseded-is-retired", row("SUPERSEDED", 0.1), RETIRED),
    ]


def _run_selftest() -> tuple[int, list[str]]:
    failures: list[str] = []
    for name, r, expect in _selftest_cases():
        got, _ = classify_status(r, _NOW)
        if got != expect:
            failures.append(f"{name}: expected {expect}, got {got}")
    # A FALSIFIED row must not survive latest_by_lane->summarize as live/stale.
    rows = [
        Row(
            "X", "a", (_NOW - _dt.timedelta(hours=99)).strftime(_TS_FMT), "CLAIMED", ""
        ),
        Row(
            "X", "a", (_NOW - _dt.timedelta(hours=1)).strftime(_TS_FMT), "FALSIFIED", ""
        ),
    ]
    summ = summarize(rows, _NOW)
    if [s.lane for s in summ.retired] != ["X"] or summ.live or summ.stale:
        failures.append(
            "self-retire override: a later FALSIFIED row must retire an earlier "
            "CLAIMED lane (got "
            f"live={len(summ.live)} stale={len(summ.stale)} retired={len(summ.retired)})"
        )
    return (1 if failures else 0), failures


# ----------------------------------- main ------------------------------------


def main(argv: list[str] | None = None) -> int:
    _force_utf8_stdio()
    ap = argparse.ArgumentParser(
        prog="claims_status", description=__doc__.splitlines()[0]
    )
    ap.add_argument(
        "--check",
        action="store_true",
        help="falsifiable self-test: exit 1 if the terminal/stale classifier rots",
    )
    ap.add_argument(
        "--path",
        type=Path,
        default=ROOT / CLAIMS_REL,
        help="CLAIMS.md path (default: the repo copy)",
    )
    ap.add_argument("--json", action="store_true", help="emit machine-readable output")
    args = ap.parse_args(argv)

    if args.check:
        code, failures = _run_selftest()
        if failures:
            for f in failures:
                print(f"  [DEAD] claims_status self-test: {f}")
            print(
                f"\n{len(failures)} claims_status self-test(s) FAILED -- the terminal "
                "vocabulary / staleness classifier has silently rotted (M34/M42)."
            )
        else:
            print(f"All {len(_selftest_cases()) + 1} claims_status self-tests pass.")
        return code

    text = _read_claims(args.path)
    summary = summarize(parse_rows(text), _dt.datetime.now(_dt.timezone.utc))
    if args.json:
        print(json.dumps(summary.as_dict(), indent=2, sort_keys=True))
    else:
        _print_report(summary)
    # Warn-only: a stale claim is FLAGGED, never a hard failure.
    return 0


if __name__ == "__main__":
    sys.exit(main())
