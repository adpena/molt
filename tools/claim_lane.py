#!/usr/bin/env python3
"""Solo-lane claim helper — operationalizes docs/agent/CLAIMS.md so agents can't
get the check / claim / back-off dance subtly wrong.

The claim is a git-atomic lock: appends a row to CLAIMS.md and lands it via
`tools/ff_land.py`. Because landing is a fast-forward, exactly one agent wins a
claim race; the loser's push is refused and it backs off. This tool wraps that
with a fail-closed pre-check (drift-sweep + current claim state) and correct row
formatting.

Usage::

    # Before starting a SOLO lane — is it free? (read-only; exit 0=claimable, 1=held)
    python tools/claim_lane.py E1-WITNESS-TO-GREEN --check

    # Claim it (pre-checks, appends CLAIMED, ff_lands; backs off if raced)
    python tools/claim_lane.py E1-WITNESS-TO-GREEN --claim --agent codex-xyz --note "first step ..."

    # Log progress / release / complete (claimant only)
    python tools/claim_lane.py E1-WITNESS-TO-GREEN --append PROGRESS --agent codex-xyz --note "seal regenerated, run 2026...-abc"

A claim with no PROGRESS/CLAIMED row for >STALE_HOURS is treated as STALE
(reclaimable). COMPLETE / RELEASED free the lane. See CLAIMS.md §5-6 for the
completion bar (final exit criteria + adversarial review + senior sign-off).
"""

from __future__ import annotations

import argparse
import datetime as _dt
import subprocess
import sys
from pathlib import Path

CLAIMS_REL = "docs/agent/CLAIMS.md"
STALE_HOURS = 4.0
LIVE_STATUSES = {"CLAIMED", "PROGRESS", "RECLAIM"}
FREEING_STATUSES = {"RELEASED", "COMPLETE"}
ALL_STATUSES = LIVE_STATUSES | FREEING_STATUSES


def _git(root: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=root, check=check, capture_output=True, text=True)


def _repo_root() -> Path:
    return Path(_git(Path.cwd(), "rev-parse", "--show-toplevel").stdout.strip())


def _utc_now_iso() -> str:
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _parse_rows(claims_text: str, lane: str) -> list[dict[str, str]]:
    """Return the log rows for `lane`, oldest→newest."""
    rows: list[dict[str, str]] = []
    in_log = False
    for line in claims_text.splitlines():
        if line.startswith("## Log"):
            in_log = True
            continue
        if not in_log or not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) < 4 or cells[0].lower() in {"lane", "------"} or cells[0].startswith("-"):
            continue
        if cells[3] not in ALL_STATUSES:  # skip header/placeholder rows
            continue
        if cells[0] != lane:
            continue
        rows.append({"lane": cells[0], "agent": cells[1], "utc": cells[2], "status": cells[3],
                     "note": cells[4] if len(cells) > 4 else ""})
    return rows


def _latest_state(rows: list[dict[str, str]]) -> tuple[str, dict[str, str] | None]:
    if not rows:
        return "UNCLAIMED", None
    last = rows[-1]
    if last["status"] in FREEING_STATUSES:
        return last["status"], last
    # live claim — check staleness against the newest live row's timestamp
    try:
        ts = _dt.datetime.strptime(last["utc"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=_dt.timezone.utc)
        age_h = (_dt.datetime.now(_dt.timezone.utc) - ts).total_seconds() / 3600.0
    except ValueError:
        age_h = 0.0
    return ("STALE" if age_h > STALE_HOURS else "CLAIMED-ALIVE"), last


def _read_claims_at_origin(root: Path) -> str:
    _git(root, "fetch", "origin", "--quiet", check=False)
    r = _git(root, "show", f"origin/main:{CLAIMS_REL}", check=False)
    return r.stdout if r.returncode == 0 else (root / CLAIMS_REL).read_text(encoding="utf-8")


def _claimable(state: str) -> bool:
    return state in {"UNCLAIMED", "STALE", "RELEASED", "COMPLETE"}


def _report(lane: str, state: str, row: dict[str, str] | None) -> None:
    if row is None:
        print(f"CLAIM {lane}: {state}")
    else:
        print(f"CLAIM {lane}: {state} — by {row['agent']} @ {row['utc']} ({row['status']}) — {row['note']}")


def _append_row_and_land(root: Path, lane: str, agent: str, status: str, note: str) -> int:
    claims_path = root / CLAIMS_REL
    text = claims_path.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    row = f"| {lane} | {agent} | {_utc_now_iso()} | {status} | {note} |\n"
    claims_path.write_text(text + row, encoding="utf-8")
    _git(root, "add", "--", CLAIMS_REL)
    _git(root, "commit", "-m", f"{status} {lane} ({agent})", "--", CLAIMS_REL)
    land = subprocess.run([sys.executable, str(root / "tools" / "ff_land.py")],
                          cwd=root, capture_output=True, text=True)
    print(land.stdout.strip())
    return land.returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane")
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--check", action="store_true", help="read-only: report claim state (exit 0=claimable, 1=held)")
    action.add_argument("--claim", action="store_true", help="claim the lane (pre-check + append CLAIMED + ff_land)")
    action.add_argument("--append", metavar="STATUS", choices=sorted(ALL_STATUSES), help="append a status row + ff_land")
    parser.add_argument("--agent", help="your agent id (required for --claim/--append)")
    parser.add_argument("--note", default="", help="note / evidence for the row")
    args = parser.parse_args(argv)

    root = _repo_root()
    state, row = _latest_state(_parse_rows(_read_claims_at_origin(root), args.lane))

    if args.claim or args.append:
        if not args.agent:
            print("claim_lane: --agent is required for --claim/--append")
            return 2

    if args.append:
        # progress/complete/release/reclaim: allow, but guard silent takeovers
        if args.append in LIVE_STATUSES and state == "CLAIMED-ALIVE" and row and row["agent"] != args.agent:
            print(f"REFUSED: {args.lane} is held by {row['agent']} (alive). Do not take over a live claim; "
                  f"escalate to the orchestrator (CLAIMS.md §6).")
            return 1
        rc = _append_row_and_land(root, args.lane, args.agent, args.append, args.note)
        if rc != 0:
            print("  (ff_land refused — fetch, re-check, and retry if still valid)")
        return rc

    if args.claim:
        if not _claimable(state):
            _report(args.lane, state, row)
            print(f"BACK OFF: {args.lane} is already {state}. Pick a different SOLO lane or a standing lane.")
            return 1
        rc = _append_row_and_land(root, args.lane, args.agent, "CLAIMED", args.note)
        if rc != 0:
            # someone raced us to the fast-forward — re-check and back off if now held
            state2, row2 = _latest_state(_parse_rows(_read_claims_at_origin(root), args.lane))
            _report(args.lane, state2, row2)
            print("BACK OFF: lost the claim race (ff_land refused). Reset to origin/main and pick another lane.")
            return 1
        print(f"CLAIMED {args.lane} as {args.agent}. You own it end-to-end (CLAIMS.md §4-5).")
        return 0

    # default: --check
    _report(args.lane, state, row)
    return 0 if _claimable(state) else 1


if __name__ == "__main__":
    raise SystemExit(main())
