#!/usr/bin/env python3
"""Git safety guard for the shared Molt checkout.

Structural defense against a recurring META-BUG: destructive working-tree git
operations (`git reset --hard`, `git checkout -- <path>`, `git clean -f`,
`git stash drop/clear/pop`, ...) run against the shared OneDrive checkout
silently destroy other agents' UNCOMMITTED working-tree WIP. This has caused
real signal loss more than once (incident 2026-07-03: a `git reset --hard HEAD`
in a cleanup one-liner discarded a concurrent lane's unstaged `function.rs`
refactor; earlier: a shared-checkout `reset: moving to main` wiped WIP).

Knowing the rule ("never destructive git on the shared checkout") was NOT enough
-- the knowledge existed in agent memory yet the op still ran. This tool turns
the rule into MECHANISM:

  * `run`      -- run git through the guard: destructive ops on the shared
                  checkout are REFUSED (with the safe alternative) unless
                  MOLT_GIT_GUARD_OVERRIDE=1, and even then a snapshot is taken
                  first. Non-destructive ops and ops in isolated worktrees /
                  plumbing-index mode pass straight through (fail-open).
  * `check`    -- classify only; exit 3 if the op would be blocked.
  * `snapshot` -- capture WT+index to refs/wip-guard/<ts> (never mutates the WT).
  * `watch`    -- always-on recovery net: snapshot every N seconds so nothing
                  uncommitted is ever more than N seconds from recovery,
                  regardless of who runs what.
  * `list`     -- show recovery snapshots.

Safe alternatives the guard points you to:
  * Need a clean tree for a build or cherry-pick trial? Use an ISOLATED
    worktree (`git worktree add`), never the shared checkout.
  * Need to stage/commit without touching the WT? Use a temp GIT_INDEX_FILE
    (the plumbing-landing pattern: read-tree origin/main -> update-index ->
    write-tree -> commit-tree -> push).
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
import time
try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

# git <subcommand> -> predicate(rest_args) -> True if this invocation discards
# working-tree / index / ref state that is not otherwise recoverable.
def _has_flag(a: list[str], short_chars: str = "", long_flags: tuple[str, ...] = ()) -> bool:
    """Detect a flag whether written standalone (`-f`), CLUSTERED (`-fd`,
    `-fdx`), or long (`--force`). Cluster handling is essential: `git clean -fd`
    is the common destructive form and must not slip through an exact match."""
    for x in a:
        if x in long_flags:
            return True
        if x.startswith("-") and not x.startswith("--"):
            if any(c in x[1:] for c in short_chars):
                return True
    return False


def _reset_destructive(a: list[str]) -> bool:
    return any(f in a for f in ("--hard", "--merge", "--keep"))


def _checkout_destructive(a: list[str]) -> bool:
    # `checkout -f`, `checkout -- <path>`, `checkout <ref> -- <path>` overwrite
    # the working tree. Plain branch switches (no `--`, no `-f`) are allowed.
    if _has_flag(a, "f", ("--force",)):
        return True
    return "--" in a


def _switch_destructive(a: list[str]) -> bool:
    return _has_flag(a, "f", ("--force", "--discard-changes"))


def _clean_destructive(a: list[str]) -> bool:
    return _has_flag(a, "fdxX", ("--force",))


def _stash_destructive(a: list[str]) -> bool:
    return bool(a) and a[0] in ("drop", "clear", "pop")


def _branch_destructive(a: list[str]) -> bool:
    # `-D` (force-delete, incl. clustered `-Df`) or `--delete --force`.
    return _has_flag(a, "D") or ("--delete" in a and _has_flag(a, "f", ("--force",)))


def _gc_destructive(a: list[str]) -> bool:
    return any(x in ("--prune=now", "--prune=all") for x in a)


def _updateref_destructive(a: list[str]) -> bool:
    return "-d" in a


def _reflog_destructive(a: list[str]) -> bool:
    return "expire" in a and any("--expire=now" in x or "--expire=all" in x for x in a)


DESTRUCTIVE = {
    "reset": _reset_destructive,
    "checkout": _checkout_destructive,
    "restore": lambda a: True,  # `restore` overwrites WT/index by design
    "switch": _switch_destructive,
    "clean": _clean_destructive,
    "stash": _stash_destructive,
    "branch": _branch_destructive,
    "gc": _gc_destructive,
    "update-ref": _updateref_destructive,
    "reflog": _reflog_destructive,
}


def _git(args: list[str], **kw) -> subprocess.CompletedProcess:
    return _COMMANDS.run(["git", *args], capture_output=True, text=True, **kw)


def in_plumbing_mode() -> bool:
    """A temp GIT_INDEX_FILE means we are doing plumbing that never touches the
    shared working tree -- always safe."""
    return bool(os.environ.get("GIT_INDEX_FILE"))


def is_linked_worktree() -> bool:
    gd = _git(["rev-parse", "--git-dir"]).stdout.strip()
    cd = _git(["rev-parse", "--git-common-dir"]).stdout.strip()
    return bool(gd) and bool(cd) and os.path.abspath(gd) != os.path.abspath(cd)


def is_shared_checkout() -> bool:
    """The shared checkout is the MAIN worktree, operated on live-index mode.
    Isolated (linked) worktrees and plumbing-index mode are always safe."""
    if in_plumbing_mode():
        return False
    if is_linked_worktree():
        return False
    return True


def classify(argv: list[str]) -> str | None:
    """Return the destructive subcommand name, or None if the op is safe."""
    if not argv:
        return None
    sub = argv[0]
    pred = DESTRUCTIVE.get(sub)
    if pred and pred(argv[1:]):
        return sub
    return None


def snapshot(label: str = "auto") -> tuple[str, str] | None:
    """Capture WT+index into a recovery ref WITHOUT mutating the working tree.
    `git stash create` builds stash commit objects but leaves WT/index/HEAD
    untouched, so it is safe to run at any time and against a live index."""
    r = _git(["stash", "create", f"git-guard {label}"])
    sha = r.stdout.strip()
    if not sha:
        return None  # clean tree -- nothing to snapshot
    ref = f"refs/wip-guard/{int(time.time())}-{label}"
    _git(["update-ref", ref, sha])
    return ref, sha


SAFE_ALTERNATIVE = (
    "SAFE ALTERNATIVES:\n"
    "  * clean tree for a build/cherry-pick trial -> use an ISOLATED worktree:\n"
    "      git worktree add <path> <ref>\n"
    "  * stage/commit without touching the WT -> temp-index plumbing:\n"
    "      GIT_INDEX_FILE=<tmp> git read-tree origin/main; git update-index ...;\n"
    "      tree=$(git write-tree); commit-tree; push\n"
    "  * a recovery snapshot of the current WT exists via: git_guard.py snapshot\n"
)


def cmd_run(git_args: list[str], override: bool) -> int:
    danger = classify(git_args)
    if danger and is_shared_checkout():
        snap = snapshot(label=f"pre-{danger}")
        snaptxt = f"recovery snapshot: {snap[0]}" if snap else "(working tree already clean)"
        if not override:
            sys.stderr.write(
                f"\n[git-guard] BLOCKED: `git {' '.join(git_args)}` is a DESTRUCTIVE\n"
                f"working-tree operation on the SHARED checkout. This class has\n"
                f"destroyed other agents' uncommitted WIP before. Refusing.\n"
                f"{snaptxt}\n\n{SAFE_ALTERNATIVE}\n"
                f"If you have truly verified this is safe, re-run with\n"
                f"MOLT_GIT_GUARD_OVERRIDE=1 (a snapshot is taken automatically).\n"
            )
            return 3
        sys.stderr.write(
            f"[git-guard] OVERRIDE: proceeding with `git {' '.join(git_args)}`; "
            f"{snaptxt}\n"
        )
    proc = _COMMANDS.run(["git", *git_args])
    return proc.returncode


def cmd_check(git_args: list[str]) -> int:
    danger = classify(git_args)
    if danger and is_shared_checkout():
        print(f"BLOCK {danger}")
        return 3
    print("ALLOW")
    return 0


def cmd_snapshot(label: str) -> int:
    snap = snapshot(label=label or "manual")
    if snap:
        print(f"{snap[0]} -> {snap[1]}")
    else:
        print("(clean tree -- nothing to snapshot)")
    return 0


def cmd_list() -> int:
    r = _git(["for-each-ref", "--sort=-refname", "--format=%(refname) %(objectname:short)", "refs/wip-guard/"])
    sys.stdout.write(r.stdout or "(no recovery snapshots)\n")
    return 0


def cmd_watch(interval: int) -> int:
    sys.stderr.write(f"[git-guard] recovery watch every {interval}s -> refs/wip-guard/*\n")
    last = None
    while True:
        try:
            snap = snapshot(label="watch")
            if snap and snap[1] != last:
                last = snap[1]
                sys.stderr.write(f"[git-guard] snapshot {snap[0]}\n")
                sys.stderr.flush()
        except Exception as exc:  # never let the net die on a transient git error
            sys.stderr.write(f"[git-guard] watch tick error (continuing): {exc}\n")
        time.sleep(interval)


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(prog="git_guard", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)
    pr = sub.add_parser("run", help="run git through the guard")
    pr.add_argument("git_args", nargs=argparse.REMAINDER)
    pc = sub.add_parser("check", help="classify only (exit 3 if blocked)")
    pc.add_argument("git_args", nargs=argparse.REMAINDER)
    ps = sub.add_parser("snapshot", help="save WT+index -> refs/wip-guard/<ts>")
    ps.add_argument("--label", default="manual")
    sub.add_parser("list", help="list recovery snapshots")
    pw = sub.add_parser("watch", help="always-on recovery snapshots")
    pw.add_argument("--interval", type=int, default=90)
    args = p.parse_args(argv)

    def _strip_dashdash(xs: list[str]) -> list[str]:
        return xs[1:] if xs and xs[0] == "--" else xs

    if args.cmd == "run":
        return cmd_run(_strip_dashdash(args.git_args), override=os.environ.get("MOLT_GIT_GUARD_OVERRIDE") == "1")
    if args.cmd == "check":
        return cmd_check(_strip_dashdash(args.git_args))
    if args.cmd == "snapshot":
        return cmd_snapshot(args.label)
    if args.cmd == "list":
        return cmd_list()
    if args.cmd == "watch":
        return cmd_watch(args.interval)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
