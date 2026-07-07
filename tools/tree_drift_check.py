#!/usr/bin/env python3
"""Tree-drift check: make a stale/masking working tree LOUD before you trust it.

The shared Molt checkout is a live swarm workspace that routinely lags
``origin/main`` by hundreds of commits with a heavily dirty tree. Running a
build, an audit, or a witness acceptance against it silently reports results
that reflect a *stale* or *dirty-masked* tree rather than current ``origin/main``
-- exactly the failure mode that hid the real witness frontier (a build that
"passed" the seal stage on the shared checkout while clean ``origin/main`` fails
closed). This tool turns that silent masking into a one-line, fail-closed verdict.

It is the instrument behind the drift-sweep cadence in
``docs/agent/ORCHESTRATOR_GOAL.md`` / ``docs/agent/ORCHESTRATION.md``: run it at
the start of an arc, before trusting a result, or before landing.

Usage::

    python tools/tree_drift_check.py [--fetch] [--base origin/main]
                                     [--files F ...] [--witness] [--quiet]

Exit code is 0 only when the tree is safe to trust for the requested scope:
non-zero when the tree is materially behind the base, or when any file in the
requested scope is STALE (its committed HEAD version differs from the base) or
DIRTY/UNTRACKED (its working-tree content differs from HEAD) -- both of which
mean "what you build here is not what is on ``origin/main``".
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

# Files whose staleness silently MASKS the witness/E1 frontier: if the shared
# checkout's version of any of these differs from origin/main, an acceptance run
# reports a different frontier than a clean-main build would. Keep this list in
# sync with the witness build + seal custody authority.
WITNESS_FILES: tuple[str, ...] = (
    "tools/wasm_link_edit.py",
    "tools/pact_witness_acceptance.py",
    "src/molt/cli/source_extensions.py",
    "src/molt/cli/external_native.py",
    "src/molt/cli/extension_seal.py",
    "src/molt/cli/module_graph.py",
    "src/molt/cli/models.py",
)

# Behind-count above which the tree is considered materially stale for a plain
# (no explicit file scope) trust decision.
DEFAULT_BEHIND_THRESHOLD = 1


def _git(args: list[str], cwd: Path) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _repo_root() -> Path:
    return Path(_git(["rev-parse", "--show-toplevel"], Path.cwd()))


def _porcelain(root: Path) -> dict[str, str]:
    """Map path -> 2-char porcelain status for every dirty/untracked entry.

    ``git status --porcelain`` is COLUMN-SENSITIVE (chars 0-1 are the status
    code, which may be a leading space for a worktree-only modification), so the
    output must NOT be stripped -- read it raw and split on newlines only.
    """
    out = subprocess.run(
        ["git", "status", "--porcelain"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    states: dict[str, str] = {}
    for line in out.split("\n"):
        if not line.strip():
            continue
        code, path = line[:2], line[3:]
        # Renames appear as "old -> new"; key on the new path.
        if " -> " in path:
            path = path.split(" -> ", 1)[1]
        states[path] = code
    return states


def _file_committed_differs(root: Path, base: str, path: str) -> bool | None:
    """True if HEAD:path != base:path (committed content is stale vs base).

    None when the path does not exist on one side (added/removed), which we
    surface distinctly rather than silently treating as "same".
    """
    try:
        head_blob = _git(["rev-parse", f"HEAD:{path}"], root)
    except subprocess.CalledProcessError:
        head_blob = None
    try:
        base_blob = _git(["rev-parse", f"{base}:{path}"], root)
    except subprocess.CalledProcessError:
        base_blob = None
    if head_blob is None or base_blob is None:
        return None
    return head_blob != base_blob


def _classify(root: Path, base: str, path: str, porcelain: dict[str, str]) -> str:
    code = porcelain.get(path)
    if code == "??":
        return "UNTRACKED"
    if code is not None and code.strip():
        # Working tree differs from HEAD (staged and/or unstaged).
        return "DIRTY"
    differs = _file_committed_differs(root, base, path)
    if differs is None:
        return "ABSENT-ONE-SIDE"
    return "STALE" if differs else "CLEAN"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", default="origin/main", help="ref to compare against (default origin/main)")
    parser.add_argument("--fetch", action="store_true", help="git fetch origin before comparing")
    parser.add_argument("--files", nargs="*", default=None, help="explicit files to scope the trust decision to")
    parser.add_argument("--witness", action="store_true", help="scope to the witness/E1 frontier-masking file set")
    parser.add_argument("--behind-threshold", type=int, default=DEFAULT_BEHIND_THRESHOLD)
    parser.add_argument("--quiet", action="store_true", help="print only the one-line verdict")
    args = parser.parse_args(argv)

    root = _repo_root()
    if args.fetch:
        try:
            _git(["fetch", "origin", "--quiet"], root)
        except subprocess.CalledProcessError as exc:  # network/offline: LOUD, not silent
            print(f"DRIFT-CHECK ERROR: git fetch failed: {exc.stderr.strip() if exc.stderr else exc}")
            return 3

    try:
        head = _git(["rev-parse", "--short", "HEAD"], root)
        base_sha = _git(["rev-parse", "--short", args.base], root)
    except subprocess.CalledProcessError as exc:
        print(f"DRIFT-CHECK ERROR: cannot resolve {args.base}: {exc.stderr.strip() if exc.stderr else exc}")
        return 3

    behind = int(_git(["rev-list", "--count", f"HEAD..{args.base}"], root))
    ahead = int(_git(["rev-list", "--count", f"{args.base}..HEAD"], root))
    porcelain = _porcelain(root)
    dirty = len(porcelain)

    scope: list[str] = []
    if args.witness:
        scope.extend(WITNESS_FILES)
    if args.files:
        scope.extend(args.files)
    # de-dup, preserve order
    seen: set[str] = set()
    scope = [f for f in scope if not (f in seen or seen.add(f))]

    file_states: list[tuple[str, str]] = [(f, _classify(root, args.base, f, porcelain)) for f in scope]
    masking = [f for f, s in file_states if s in {"STALE", "DIRTY", "UNTRACKED", "ABSENT-ONE-SIDE"}]

    # A scoped request trusts the tree only if no scoped file is masking.
    # An unscoped request trusts the tree only if it is not materially behind.
    if scope:
        ok = not masking
    else:
        ok = behind < args.behind_threshold

    verdict = "OK" if ok else "DRIFT"
    summary = f"{verdict}: HEAD={head} vs {args.base}={base_sha} | behind={behind} ahead={ahead} dirty={dirty}"
    if scope:
        summary += f" | scoped {len(masking)}/{len(scope)} masking"
    print(summary)

    if not args.quiet and file_states:
        for f, s in file_states:
            mark = " " if s == "CLEAN" else "!"
            print(f"  {mark} {s:16s} {f}")
    if not args.quiet and not ok and not scope:
        print(f"  ! tree is {behind} commit(s) behind {args.base}; build/audit/acceptance here reflects a STALE tree, not current {args.base}.")

    return 0 if ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
