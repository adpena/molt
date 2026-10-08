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

A claim with no PROGRESS/CLAIMED row for >claims_status.STALE_HOURS is STALE
(reclaimable). Any TERMINAL status frees the lane. tools/claims_status.py owns
the status vocabulary, the log parser and the live/stale/retired classifier;
this tool only applies them to one lane. See CLAIMS.md §5-7 for the completion
bar and the meaning of each status.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import subprocess
import sys
from pathlib import Path

try:
    from tools import claims_status as cs
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    import claims_status as cs  # type: ignore
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

CLAIMS_REL = cs.CLAIMS_REL


def _git(
    root: Path, *args: str, check: bool = True
) -> subprocess.CompletedProcess[str]:
    return _COMMANDS.run(
        ["git", *args],
        cwd=root,
        check=check,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )


def _repo_root() -> Path:
    return Path(_git(Path.cwd(), "rev-parse", "--show-toplevel").stdout.strip())


def _utc_now_iso() -> str:
    return _dt.datetime.now(_dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _lane_state(claims_text: str, lane: str) -> tuple[str, cs.Row | None]:
    """The lane's state: UNCLAIMED, STALE, CLAIMED-ALIVE, or its terminal status."""
    last = cs.latest_by_lane(cs.parse_rows(claims_text)).get(lane)
    if last is None:
        return "UNCLAIMED", None
    klass, _ = cs.classify_status(last, _dt.datetime.now(_dt.timezone.utc))
    if klass == cs.RETIRED:
        return last.status, last
    return ("STALE" if klass == cs.STALE else "CLAIMED-ALIVE"), last


def _read_claims_at_origin(root: Path) -> str:
    _git(root, "fetch", "origin", "--quiet", check=False)
    r = _git(root, "show", f"origin/main:{CLAIMS_REL}", check=False)
    return (
        r.stdout
        if r.returncode == 0
        else (root / CLAIMS_REL).read_text(encoding="utf-8")
    )


def _claimable(state: str) -> bool:
    return state in ({"UNCLAIMED", "STALE"} | cs.TERMINAL_STATUSES)


def _report(lane: str, state: str, row: cs.Row | None) -> None:
    if row is None:
        print(f"CLAIM {lane}: {state}")
    else:
        print(
            f"CLAIM {lane}: {state} — by {row.agent} @ {row.utc} ({row.status}) — {row.note}"
        )


def _append_row_and_land(
    root: Path, lane: str, agent: str, status: str, note: str
) -> int:
    claims_path = root / CLAIMS_REL
    text = claims_path.read_text(encoding="utf-8")
    if not text.endswith("\n"):
        text += "\n"
    row = f"| {lane} | {agent} | {_utc_now_iso()} | {status} | {note} |\n"
    claims_path.write_text(text + row, encoding="utf-8")
    _git(root, "add", "--", CLAIMS_REL)
    _git(root, "commit", "-m", f"{status} {lane} ({agent})", "--", CLAIMS_REL)
    # Land with the ff_land that ships beside this tool; `root` (from cwd) is
    # only the repository being landed, as `tests/tools/test_ff_land.py` runs it.
    land = _COMMANDS.run(
        [sys.executable, str(Path(__file__).with_name("ff_land.py"))],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    print(land.stdout.strip())
    if land.stderr.strip():
        # A crash also exits nonzero; never let it pass as a lost claim race.
        print(land.stderr.strip(), file=sys.stderr)
    return land.returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("lane")
    action = parser.add_mutually_exclusive_group()
    action.add_argument(
        "--check",
        action="store_true",
        help="read-only: report claim state (exit 0=claimable, 1=held)",
    )
    action.add_argument(
        "--claim",
        action="store_true",
        help="claim the lane (pre-check + append CLAIMED + ff_land)",
    )
    action.add_argument(
        "--append",
        metavar="STATUS",
        choices=sorted(cs.ALL_STATUSES),
        help="append a status row + ff_land",
    )
    parser.add_argument("--agent", help="your agent id (required for --claim/--append)")
    parser.add_argument("--note", default="", help="note / evidence for the row")
    args = parser.parse_args(argv)

    root = _repo_root()
    try:
        state, row = _lane_state(_read_claims_at_origin(root), args.lane)
    except cs.ClaimsLogError as exc:
        print(f"claim_lane: {CLAIMS_REL} at origin/main: {exc}", file=sys.stderr)
        return 2

    if args.claim or args.append:
        if not args.agent:
            print("claim_lane: --agent is required for --claim/--append")
            return 2
        cells = {"lane": args.lane, "--agent": args.agent, "--note": args.note}
        broken = [
            name for name, value in cells.items() if "|" in value or "\n" in value
        ]
        if broken:
            print(
                f"claim_lane: {', '.join(broken)} must not contain '|' or a newline; "
                "either one breaks the log table"
            )
            return 2

    if args.append:
        # progress/complete/release/reclaim: allow, but guard silent takeovers
        if (
            args.append in cs.LIVE_STATUSES
            and state == "CLAIMED-ALIVE"
            and row
            and row.agent != args.agent
        ):
            print(
                f"REFUSED: {args.lane} is held by {row.agent} (alive). Do not take over a live claim; "
                f"escalate to the orchestrator (CLAIMS.md §6)."
            )
            return 1
        rc = _append_row_and_land(root, args.lane, args.agent, args.append, args.note)
        if rc != 0:
            print("  (ff_land refused — fetch, re-check, and retry if still valid)")
        return rc

    if args.claim:
        if not _claimable(state):
            _report(args.lane, state, row)
            print(
                f"BACK OFF: {args.lane} is already {state}. Pick a different SOLO lane or a standing lane."
            )
            return 1
        rc = _append_row_and_land(root, args.lane, args.agent, "CLAIMED", args.note)
        if rc != 0:
            # someone raced us to the fast-forward — re-check and back off if now held
            state2, row2 = _lane_state(_read_claims_at_origin(root), args.lane)
            _report(args.lane, state2, row2)
            print(
                "BACK OFF: lost the claim race (ff_land refused). Reset to origin/main and pick another lane."
            )
            return 1
        print(
            f"CLAIMED {args.lane} as {args.agent}. You own it end-to-end (CLAIMS.md §4-5)."
        )
        return 0

    # default: --check
    _report(args.lane, state, row)
    return 0 if _claimable(state) else 1


if __name__ == "__main__":
    raise SystemExit(main())
