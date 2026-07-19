#!/usr/bin/env python3
"""Premise-verification preflight: has a SISTER lane already landed this? (A11).

The duplicate-work class this session hit repeatedly (M22/M24): a spawned agent
starts writing on a surface a sibling lane already LANDED (wasted work, then a
mismerge of a stale diff), or two lanes silently edit the same files at once
(colliding hunks, trampled landings). pact kills this bidirectionally with a
preflight every subagent runs BEFORE its first Write
(``check_sister_files_recently_landed``: exit 8 = STAND_DOWN_DUPLICATE, 9 =
WAIT_AND_REASSESS -- ``docs/canonical_subagent_pre_flight_checklist.md``). This is
the molt port.

Given the lane's target files (``--files``) and/or topic (``--topic``), it asks
two questions against ``docs/agent/CLAIMS.md`` and ``git log``:

  * Did a sister lane already LAND this surface? -- a RETIRED CLAIMS row
    (COMPLETE / RELEASED / FALSIFIED / SUPERSEDED / ...) whose evidence note
    references a target file or the topic, OR a recent ``git`` commit touching a
    target file. -> exit 8 STAND_DOWN_DUPLICATE. "Already landed" is the strongest
    stand-down, so it outranks question two.
  * Is a sister lane MID-FLIGHT on this surface? -- a non-terminal CLAIMS row
    (CLAIMED / PROGRESS / RECLAIM, incl. a STALE one -- a silent worktree may
    still be alive, CLAIMS.md 6) referencing a target file or the topic.
    -> exit 9 WAIT_AND_REASSESS.

  * Otherwise -> exit 0, proceed.

The verdict is ADVISORY (an exit code the spawning agent + the A7
``subagent_contract`` PREMISE_VERIFICATION clause read): 8/9 mean "re-read
CLAIMS + the recent landings and reassess", not a hard abort. Fail-open: any
git/parse hiccup yields no matches -> exit 0 (a preflight must never block a lane
because it could not read the ledger).

``--check`` is the falsifiable self-test (wired into ci_gate tier-1 + a
check_gate_liveness canary): a synthetic already-landed CLAIMS row for the target
files MUST return 8, a mid-flight CLAIMED row MUST return 9, an unrelated surface
MUST return 0. Pure classifier + pure matcher; ASCII + UTF-8-explicit (M43).
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
import sys
from dataclasses import dataclass, field
from pathlib import Path
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

try:  # pragma: no cover - trivial import shim
    from tools import claims_status as cs
    from tools._io_utf8 import force_utf8_stdio as _force_utf8_stdio
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.abspath(__file__))))
    from tools import claims_status as cs

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

PROCEED = 0
STAND_DOWN_DUPLICATE = 8
WAIT_AND_REASSESS = 9
USAGE_ERROR = 2

DEFAULT_WINDOW_HOURS = 24.0


@dataclass
class Verdict:
    code: int
    label: str
    reason: str
    landed: list[str] = field(default_factory=list)
    inflight: list[str] = field(default_factory=list)

    def as_dict(self) -> dict[str, object]:
        return {
            "code": self.code,
            "label": self.label,
            "reason": self.reason,
            "landed": self.landed,
            "inflight": self.inflight,
        }


# ------------------------------ pure matching --------------------------------


def _norm(path: str) -> str:
    return str(path or "").replace("\\", "/").strip().strip("/")


def row_matches_target(
    note: str, lane: str, files: list[str], topic: str | None
) -> bool:
    """True if a CLAIMS row plausibly concerns the target files or topic.

    A file matches if its normalized path OR its basename appears in the row note
    (evidence notes cite the paths + commit shas of what landed). The topic
    matches if the keyword appears (case-insensitive) in the lane name or note.
    Both are substring tests on purpose -- a preflight favors recall (catch the
    duplicate) over precision (a spurious 8/9 only costs a re-read, M22/M24).
    """
    hay = f"{lane}\n{note}".lower()
    for f in files:
        nf = _norm(f).lower()
        if not nf:
            continue
        if nf in hay or nf.rsplit("/", 1)[-1] in hay:
            return True
    if topic:
        t = topic.strip().lower()
        if t and t in hay:
            return True
    return False


def classify(landed: list[str], inflight: list[str]) -> Verdict:
    """PURE verdict: landed outranks inflight outranks proceed.

    * landed non-empty  -> 8 STAND_DOWN_DUPLICATE (a sister already landed it)
    * inflight non-empty -> 9 WAIT_AND_REASSESS   (a sister is mid-flight)
    * else               -> 0 PROCEED
    """
    if landed:
        return Verdict(
            STAND_DOWN_DUPLICATE,
            "STAND_DOWN_DUPLICATE",
            "a sister lane already LANDED this surface -- do not duplicate. Re-read "
            "the landing (git log / the CLAIMS row) and pick a different lane or a "
            "genuinely remaining slice (M22/M24).",
            landed=list(landed),
            inflight=list(inflight),
        )
    if inflight:
        return Verdict(
            WAIT_AND_REASSESS,
            "WAIT_AND_REASSESS",
            "a sister lane is MID-FLIGHT on this surface -- do not start on top of "
            "it. Back off, wait, and reassess (or claim a different lane); colliding "
            "edits trample landings and mask frontiers (M22/M24, CLAIMS.md).",
            landed=list(landed),
            inflight=list(inflight),
        )
    return Verdict(
        PROCEED,
        "PROCEED",
        "no sister lane has landed or is mid-flight on this surface -- proceed.",
    )


# --------------------------- fact gathering (impure) --------------------------


def claims_matches(
    claims_text: str,
    files: list[str],
    topic: str | None,
    now: _dt.datetime,
) -> tuple[list[str], list[str]]:
    """Split matching CLAIMS lanes into (landed, inflight) by terminal-vs-not.

    A RETIRED (terminal) matching lane is a LANDED signal; a non-terminal
    (LIVE or STALE) matching lane is an INFLIGHT signal -- a stale claim may still
    be a silently-working sister, so a preflight waits rather than barges
    (CLAIMS.md 6). Uses the A11 terminal vocabulary via claims_status.
    """
    landed: list[str] = []
    inflight: list[str] = []
    rows = cs.parse_rows(claims_text)
    for lane, row in sorted(cs.latest_by_lane(rows).items()):
        if not row_matches_target(row.note, row.lane, files, topic):
            continue
        klass, _ = cs.classify_status(row, now)
        tag = f"{lane} ({row.status}, {row.agent})"
        if klass == cs.RETIRED:
            landed.append(tag)
        else:  # LIVE or STALE -> a possibly-live sister
            inflight.append(tag)
    return landed, inflight


def git_recent_file_matches(root: Path, files: list[str], hours: float) -> list[str]:
    """Target files touched by a commit in the last ``hours`` (a LANDED signal).

    A commit already on the branch that touched a target file means a sister
    landed there. New files the lane will CREATE return nothing (no false match).
    Best-effort: any git failure yields no matches (fail-open).
    """
    paths = [p for p in ((_norm(f) for f in files)) if p]
    if not paths:
        return []
    # An ABSOLUTE UTC cutoff, not "<N> hours ago": git's approxidate parser
    # rejects a fractional "24.0 hours ago", and an ISO cutoff is unambiguous.
    cutoff = (
        _dt.datetime.now(_dt.timezone.utc) - _dt.timedelta(hours=hours)
    ).isoformat()
    try:
        r = _COMMANDS.run(
            [
                "git",
                "log",
                f"--since={cutoff}",
                "--name-only",
                "--pretty=format:%H %s",
                "--",
                *paths,
            ],
            cwd=str(root),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
            timeout=8.0,
        )
    except Exception:
        return []
    if r.returncode != 0 or not r.stdout.strip():
        return []
    touched: list[str] = []
    wanted = {p.lower() for p in paths}
    wanted_bases = {p.rsplit("/", 1)[-1].lower() for p in paths}
    for line in r.stdout.splitlines():
        s = line.strip()
        if not s:
            continue
        head = s.split(" ", 1)[0]
        if len(head) == 40 and all(c in "0123456789abcdef" for c in head.lower()):
            continue  # a "<sha> <subject>" header line -- only name-only paths count
        nl = _norm(s).lower()
        if nl in wanted or nl.rsplit("/", 1)[-1] in wanted_bases:
            touched.append(_norm(s))
    return sorted(dict.fromkeys(touched))


def evaluate(
    root: Path,
    files: list[str],
    topic: str | None,
    hours: float,
    claims_path: Path | None = None,
    now: _dt.datetime | None = None,
) -> Verdict:
    """Gather CLAIMS + git facts and return the advisory verdict. Fail-open."""
    now = now or _dt.datetime.now(_dt.timezone.utc)
    cpath = claims_path or (root / CLAIMS_REL)
    try:
        claims_text = cpath.read_text(encoding="utf-8")
    except OSError:
        claims_text = ""
    landed, inflight = claims_matches(claims_text, files, topic, now)
    landed = list(landed) + [
        f"git: {p} touched in last {hours:g}h"
        for p in git_recent_file_matches(root, files, hours)
    ]
    return classify(landed, inflight)


# ------------------------------- CI self-test --------------------------------

_NOW = _dt.datetime(2026, 7, 11, 12, 0, 0, tzinfo=_dt.timezone.utc)


def _fixture_claims() -> str:
    fresh = (_NOW - _dt.timedelta(hours=1)).strftime(cs._TS_FMT)
    old = (_NOW - _dt.timedelta(hours=200)).strftime(cs._TS_FMT)
    return (
        "## Log\n"
        "| lane | agent-id | UTC (ISO) | status | note / evidence |\n"
        "|------|----------|-----------|--------|-----------------|\n"
        f"| LANE-DONE | codex/x | {old} | COMPLETE | Landed abc123 in "
        "runtime/molt-backend/src/landed_surface.rs; teeth green. |\n"
        f"| LANE-LIVE | codex/y | {fresh} | CLAIMED | working src/molt/frontend/inflight_surface.py |\n"
    )


def _selftest_cases() -> list[tuple[str, list[str], str | None, int]]:
    return [
        (
            "already-landed-file-stands-down",
            ["runtime/molt-backend/src/landed_surface.rs"],
            None,
            STAND_DOWN_DUPLICATE,
        ),
        (
            "mid-flight-file-waits",
            ["src/molt/frontend/inflight_surface.py"],
            None,
            WAIT_AND_REASSESS,
        ),
        (
            "unrelated-file-proceeds",
            ["src/molt/frontend/brand_new_surface.py"],
            None,
            PROCEED,
        ),
        (
            "landed-topic-stands-down",
            [],
            "landed_surface",
            STAND_DOWN_DUPLICATE,
        ),
        (
            "landed-outranks-inflight",
            [
                "runtime/molt-backend/src/landed_surface.rs",
                "src/molt/frontend/inflight_surface.py",
            ],
            None,
            STAND_DOWN_DUPLICATE,
        ),
    ]


def _run_selftest() -> tuple[int, list[str]]:
    failures: list[str] = []
    text = _fixture_claims()
    for name, files, topic, expect in _selftest_cases():
        landed, inflight = claims_matches(text, files, topic, _NOW)
        got = classify(landed, inflight).code
        if got != expect:
            failures.append(f"{name}: expected exit {expect}, got {got}")
    return (1 if failures else 0), failures


# ----------------------------------- main ------------------------------------


def main(argv: list[str] | None = None) -> int:
    _force_utf8_stdio()
    ap = argparse.ArgumentParser(
        prog="check_sister_landed", description=__doc__.splitlines()[0]
    )
    ap.add_argument(
        "--files",
        nargs="*",
        default=[],
        help="repo-relative target files this lane intends to write",
    )
    ap.add_argument("--topic", default=None, help="lane topic / keyword to match")
    ap.add_argument(
        "--hours",
        type=float,
        default=DEFAULT_WINDOW_HOURS,
        help=f"recent-landing git window in hours (default: {DEFAULT_WINDOW_HOURS:g})",
    )
    ap.add_argument("--claims", type=Path, default=None, help="CLAIMS.md path override")
    ap.add_argument("--root", type=Path, default=ROOT, help="repo root for git queries")
    ap.add_argument("--json", action="store_true", help="emit machine-readable output")
    ap.add_argument(
        "--check",
        action="store_true",
        help="falsifiable self-test: exit 1 if the 8/9/0 classifier rots",
    )
    args = ap.parse_args(argv)

    if args.check:
        code, failures = _run_selftest()
        if failures:
            for f in failures:
                print(f"  [DEAD] check_sister_landed self-test: {f}")
            print(
                f"\n{len(failures)} check_sister_landed self-test(s) FAILED -- the "
                "duplicate/mid-flight classifier has silently rotted (M34/M42)."
            )
        else:
            print(f"All {len(_selftest_cases())} check_sister_landed self-tests pass.")
        return code

    if not args.files and not args.topic:
        print(
            "check_sister_landed: pass --files and/or --topic (the surface to check)",
            file=sys.stderr,
        )
        return USAGE_ERROR

    verdict = evaluate(
        args.root.resolve(),
        args.files,
        args.topic,
        args.hours,
        claims_path=args.claims,
    )
    if args.json:
        print(json.dumps(verdict.as_dict(), indent=2, sort_keys=True))
    else:
        print(f"[{verdict.label}] {verdict.reason}")
        for m in verdict.landed:
            print(f"  landed:   {m}")
        for m in verdict.inflight:
            print(f"  inflight: {m}")
    return verdict.code


if __name__ == "__main__":
    sys.exit(main())
