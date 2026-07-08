#!/usr/bin/env python3
"""Drift harvest + prune: land all worktree/branch signal, then prune the rest.

The shared repo accumulates worktrees + branches faster than they land. Left
alone it becomes hundreds of stale worktrees — bad OSS hygiene and a real
signal-loss risk. This tool makes consolidation SYSTEMATIC and REPEATABLE so it
can run regularly instead of erupting into a crisis.

Policy (patch-id aware, via ``git cherry`` — a rebased/reworded commit already on
main counts as landed):

  * SUPERSEDED  — branch has 0 commits not already on origin/main → safe to prune.
  * FRESH       — a worktree with uncommitted changes OR a commit within
                  ``--fresh-hours`` (default 24h) → KEEP (active WIP; never prune).
  * SIGNAL      — branch has unique commits and is not fresh → its commits are
                  BUNDLED (durable backup) before anything is pruned, and it is
                  listed for landing (cherry-pick onto main), never silently dropped.

Zero signal loss: every unique-commit branch is written to a git bundle on the
external volume BEFORE any prune. A prune only removes a worktree/branch whose
commits are (a) already on main, or (b) captured in that bundle.

Modes:
  drift_harvest.py --report                 classify every worktree/branch (no changes)
  drift_harvest.py --bundle <path>          bundle all unique-commit branches
  drift_harvest.py --prune [--fresh-hours N]  bundle + remove non-fresh SUPERSEDED
                                            worktrees/branches (keeps SIGNAL + FRESH)
  drift_harvest.py --prune --include-signal  ALSO prune SIGNAL worktrees (commits are
                                            bundled first; use after landing them)

Never touches: the OneDrive shared checkout, the CLI-source worktree, this
worktree, or locked worktrees.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Worktrees that must never be pruned regardless of freshness.
_PROTECTED_SUBSTRINGS = ("OneDrive", "recover-mainclean-20260707", "molt-cli")


def _git(args: list[str], cwd: Path | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args],
        cwd=str(cwd or REPO_ROOT),
        capture_output=True,
        text=True,
    )


def _worktrees() -> list[dict]:
    out = _git(["worktree", "list", "--porcelain"]).stdout
    rows: list[dict] = []
    cur: dict = {}
    for line in out.splitlines():
        if line.startswith("worktree "):
            cur = {"path": line[len("worktree ") :]}
        elif line.startswith("HEAD "):
            cur["head"] = line[len("HEAD ") :]
        elif line.startswith("branch "):
            cur["branch"] = line[len("branch ") :].removeprefix("refs/heads/")
        elif line.startswith("locked"):
            cur["locked"] = True
        elif line == "":
            if cur:
                rows.append(cur)
                cur = {}
    if cur:
        rows.append(cur)
    return rows


def _unique_commit_count(ref: str) -> int:
    # git cherry marks commits not present upstream (by patch-id) with '+'.
    out = _git(["cherry", "origin/main", ref]).stdout
    return sum(1 for ln in out.splitlines() if ln.startswith("+"))


def _last_commit_epoch(path: str) -> int:
    r = _git(["log", "-1", "--format=%ct", "HEAD"], cwd=Path(path))
    try:
        return int(r.stdout.strip())
    except ValueError:
        return 0


def _dirty_count(path: str) -> int:
    r = _git(["status", "--porcelain"], cwd=Path(path))
    return len([ln for ln in r.stdout.splitlines() if ln.strip()])


def _protected(path: str) -> bool:
    return any(s in path for s in _PROTECTED_SUBSTRINGS)


def _worktree_exists(path: str) -> bool:
    return Path(path).is_dir()


def classify(fresh_hours: float, now: float) -> list[dict]:
    cutoff = now - fresh_hours * 3600
    rows = []
    for wt in _worktrees():
        path = wt["path"]
        head = wt.get("head", "")
        ref = wt.get("branch") or head
        if not _worktree_exists(path):
            # Stale registration — the worktree dir is gone. `git worktree prune`
            # cleans these; never treat as fresh.
            rows.append(
                {
                    "path": path,
                    "branch": wt.get("branch"),
                    "uniq": 0,
                    "dirty": 0,
                    "state": "STALE",
                }
            )
            continue
        dirty = _dirty_count(path)
        last = _last_commit_epoch(path)
        uniq = _unique_commit_count(ref) if ref else 0
        fresh = dirty > 0 or last >= cutoff
        if _protected(path):
            state = "PROTECTED"
        elif wt.get("locked"):
            state = "LOCKED"
        elif fresh:
            state = "FRESH"
        elif uniq == 0:
            state = "SUPERSEDED"
        else:
            state = "SIGNAL"
        rows.append(
            {
                "path": path,
                "branch": wt.get("branch"),
                "head": head,
                "uniq": uniq,
                "dirty": dirty,
                "state": state,
            }
        )
    return rows


def bundle_signal(rows: list[dict], bundle_path: Path) -> set[str]:
    """Bundle every unique-commit worktree and return the set of worktree PATHS
    whose commits are now durably captured.

    A detached-HEAD worktree (no branch, but ``uniq > 0``) is captured under a
    synthetic ``refs/harvest/<sha>`` ref — which ALSO protects it from GC — so it
    can never be pruned without a backup. The returned set is the prune-safety
    predicate: a SIGNAL worktree is prunable only if it is in this set. This must
    stay in lockstep with the prune gate; keying bundle-safety on ``branch`` while
    keying prune-safety on ``state`` alone loses detached-HEAD commits (the exact
    zero-signal-loss violation this guards).
    """
    captured: set[str] = set()
    refs: list[str] = []
    for r in rows:
        if r["uniq"] <= 0:
            continue
        if r["branch"]:
            refs.append(r["branch"])
            captured.add(r["path"])
        elif r.get("head"):
            synthetic = f"refs/harvest/{r['head']}"
            if _git(["update-ref", synthetic, r["head"]]).returncode == 0:
                refs.append(synthetic)
                captured.add(r["path"])
    if not refs:
        return captured
    bundle_path.parent.mkdir(parents=True, exist_ok=True)
    res = _git(["bundle", "create", str(bundle_path), *sorted(set(refs))])
    if res.returncode != 0:
        raise SystemExit(f"bundle failed: {res.stderr}")
    return captured


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--report", action="store_true")
    ap.add_argument("--prune", action="store_true")
    ap.add_argument("--include-signal", action="store_true")
    ap.add_argument("--fresh-hours", type=float, default=24.0)
    ap.add_argument("--bundle", default=None, help="bundle output path")
    ap.add_argument("--now", type=float, default=None, help="override epoch (tests)")
    args = ap.parse_args()

    _git(["fetch", "origin", "--quiet"])
    now = args.now if args.now is not None else time.time()
    rows = classify(args.fresh_hours, now)

    counts: dict[str, int] = {}
    for r in rows:
        counts[r["state"]] = counts.get(r["state"], 0) + 1
    print("drift classification:", counts)
    for r in sorted(rows, key=lambda x: x["state"]):
        if r["state"] in ("SIGNAL", "SUPERSEDED"):
            print(
                f"  {r['state']:11s} uniq={r['uniq']} dirty={r['dirty']} "
                f"{r['branch'] or '(detached)'}  {r['path']}"
            )

    captured: set[str] = set()
    if args.bundle or args.prune:
        bundle_path = Path(
            args.bundle
            or (REPO_ROOT.parent / f"drift-harvest-{int(now)}.bundle")
        )
        captured = bundle_signal(rows, bundle_path)
        print(f"bundled {len(captured)} signal worktrees -> {bundle_path}")

    if not args.prune:
        return 0

    removed = 0
    deleted = 0
    for r in rows:
        if r["state"] == "SUPERSEDED":
            prunable = True  # commits already on origin/main
        elif args.include_signal and r["state"] == "SIGNAL":
            # NEVER prune SIGNAL unless its commits were durably captured this run
            # (branch OR detached-HEAD synthetic ref). Prune-safety == bundle-safety.
            prunable = r["path"] in captured
        else:
            prunable = False
        if not prunable:
            continue
        rm = _git(["worktree", "remove", r["path"]])
        if rm.returncode == 0:
            removed += 1
            if r["branch"]:
                # SUPERSEDED is on main; SIGNAL is bundled — force-delete is safe.
                if _git(["branch", "-D", r["branch"]]).returncode == 0:
                    deleted += 1
    _git(["worktree", "prune"])
    # Delete any remaining fully-merged branches (not checked out anywhere).
    merged = _git(["branch", "--merged", "origin/main", "--format=%(refname:short)"]).stdout
    for b in merged.splitlines():
        b = b.strip()
        if b and b != "main":
            if _git(["branch", "-d", b]).returncode == 0:
                deleted += 1
    print(f"pruned worktrees={removed} branches={deleted}")
    print(f"remaining worktrees: {len(_worktrees())}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
