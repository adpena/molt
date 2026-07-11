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
import os
import subprocess
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Worktrees that must never be pruned regardless of freshness.
_PROTECTED_SUBSTRINGS = ("OneDrive", "recover-mainclean-20260707", "molt-cli")


def _contains_path(parent: Path, child: Path) -> bool:
    try:
        child.resolve().relative_to(parent.resolve())
    except ValueError:
        return False
    return True


def _leave_prune_targets(targets: list[Path]) -> Path:
    current = Path.cwd().resolve()
    if not any(_contains_path(target, current) for target in targets):
        return current
    destination = REPO_ROOT.parent.resolve()
    if any(_contains_path(target, destination) for target in targets):
        raise RuntimeError(
            "cannot prune a worktree containing the process cwd; rerun from "
            f"outside the targets, for example: Set-Location {REPO_ROOT.parent}"
        )
    os.chdir(destination)
    return destination


def _sweep_empty_orphan_worktree_dirs() -> list[Path]:
    registered = {Path(row["path"]).resolve() for row in _worktrees()}
    removed: list[Path] = []
    canonical_root = REPO_ROOT.parent.resolve()
    roots = (canonical_root, (canonical_root / "worktrees").resolve())
    for root in roots:
        if not root.is_dir():
            continue
        for candidate in root.iterdir():
            if not candidate.is_dir() or candidate.resolve() in registered:
                continue
            if root == canonical_root and not candidate.name.startswith("wt-"):
                continue
            try:
                candidate.rmdir()
            except OSError:
                continue
            removed.append(candidate)
    return removed


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
        fresh = dirty > 0 or last >= cutoff
        # ``git cherry`` (patch-id) is the expensive per-worktree call. PROTECTED,
        # LOCKED, and FRESH worktrees never consume ``uniq`` (they are always kept),
        # so compute it lazily only for the SUPERSEDED-vs-SIGNAL decision. This keeps
        # the pre-push gate fast on a checkout of mostly-fresh worktrees without
        # changing any state assignment or prune/bundle behaviour.
        if _protected(path):
            state, uniq = "PROTECTED", 0
        elif wt.get("locked"):
            state, uniq = "LOCKED", 0
        elif fresh:
            state, uniq = "FRESH", 0
        else:
            uniq = _unique_commit_count(ref) if ref else 0
            state = "SUPERSEDED" if uniq == 0 else "SIGNAL"
        rows.append(
            {
                "path": path,
                "branch": wt.get("branch"),
                "head": head,
                "uniq": uniq,
                "dirty": dirty,
                "last": last,
                "state": state,
            }
        )
    return rows


def gate(rows: list[dict], now: float, *, max_worktrees: int, max_signal_age_hours: float) -> list[str]:
    """Return a list of drift violations (empty == clean).

    The gate makes worktree/branch drift BLOCKING instead of a silent slow
    accumulation. Two failure classes, matching the exact 100-worktree /
    165-branch incident this tool exists to prevent:

      * SPRAWL   — more than ``max_worktrees`` live worktrees. Long-lived
                   worktrees are the drift substrate; agents must work in
                   short-lived worktrees off the canonical checkout and prune
                   after landing.
      * STALE-SIGNAL — a SIGNAL worktree (unique unlanded commits) whose tip is
                   older than ``max_signal_age_hours``. Unlanded signal that
                   ages is drift: harvest it (cherry-pick/reconcile onto
                   origin/main) and prune, or it rots into a 130-worktree mess.

    Fresh SIGNAL (recent WIP) never trips the gate — only aged, unlanded work.
    """
    violations: list[str] = []
    live = [r for r in rows if r["state"] not in ("STALE",)]
    if len(live) > max_worktrees:
        violations.append(
            f"SPRAWL: {len(live)} live worktrees > max {max_worktrees}. "
            f"Prune landed/superseded worktrees: drift_harvest.py --prune"
        )
    cutoff = now - max_signal_age_hours * 3600
    for r in rows:
        if r["state"] == "SIGNAL" and r.get("last", 0) and r["last"] < cutoff:
            age_h = (now - r["last"]) / 3600
            violations.append(
                f"STALE-SIGNAL ({age_h:.0f}h, uniq={r['uniq']}): "
                f"{r['branch'] or '(detached)'} {r['path']} — harvest onto "
                f"origin/main + prune, or record an evidence-backed blocker"
            )
    return violations


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
    ap.add_argument(
        "--gate",
        action="store_true",
        help="fail (exit 1) on drift: worktree sprawl or aged unlanded SIGNAL",
    )
    ap.add_argument("--max-worktrees", type=int, default=24)
    ap.add_argument("--max-signal-age-hours", type=float, default=72.0)
    ap.add_argument(
        "--no-fetch",
        action="store_true",
        help="skip 'git fetch origin' (for pre-push hooks: the push already "
        "contacts origin, and a stale origin/main only risks a benign false "
        "positive — never a missed prune)",
    )
    args = ap.parse_args()

    if not args.no_fetch:
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

    if args.gate:
        violations = gate(
            rows,
            now,
            max_worktrees=args.max_worktrees,
            max_signal_age_hours=args.max_signal_age_hours,
        )
        if violations:
            print(f"\nDRIFT GATE FAILED ({len(violations)} violation(s)):")
            for v in violations:
                print(f"  ✗ {v}")
            return 1
        print("\nDRIFT GATE PASSED (no sprawl, no aged unlanded signal)")
        return 0

    captured: set[str] = set()
    if args.bundle or args.prune:
        # Default the bundle to a stable `drift-bundles/` dir under the artifact
        # root (never a worktree being pruned, and off the repo working tree). An
        # explicit --bundle always wins.
        default_root = Path(os.environ.get("MOLT_EXT_ROOT") or REPO_ROOT.parent)
        bundle_path = Path(
            args.bundle
            or (default_root / "drift-bundles" / f"drift-harvest-{int(now)}.bundle")
        )
        captured = bundle_signal(rows, bundle_path)
        print(f"bundled {len(captured)} signal worktrees -> {bundle_path}")

    if not args.prune:
        return 0

    prune_targets = [
        Path(r["path"])
        for r in rows
        if r["state"] == "SUPERSEDED"
        or (args.include_signal and r["state"] == "SIGNAL" and r["path"] in captured)
    ]
    cleanup_cwd = _leave_prune_targets(prune_targets)
    print(f"cleanup cwd: {cleanup_cwd}")

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
                # Force-delete the branch ONLY if we can PROVE its work is safe: the
                # tip is an ancestor of origin/main (truly landed — immune to a
                # git-cherry merge-commit miscount), OR its commits were captured in
                # this run's bundle. Otherwise fall back to `-d` (refuses unmerged).
                on_main = (
                    _git(["merge-base", "--is-ancestor", r["branch"], "origin/main"]).returncode == 0
                )
                flag = "-D" if (on_main or r["path"] in captured) else "-d"
                if _git(["branch", flag, r["branch"]]).returncode == 0:
                    deleted += 1
    _git(["worktree", "prune"])
    swept = _sweep_empty_orphan_worktree_dirs()
    # Delete any remaining fully-merged branches (not checked out anywhere).
    merged = _git(["branch", "--merged", "origin/main", "--format=%(refname:short)"]).stdout
    for b in merged.splitlines():
        b = b.strip()
        if b and b != "main":
            if _git(["branch", "-d", b]).returncode == 0:
                deleted += 1
    print(f"pruned worktrees={removed} branches={deleted}")
    print(f"swept empty orphan worktree dirs={len(swept)}")
    print(f"remaining worktrees: {len(_worktrees())}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
