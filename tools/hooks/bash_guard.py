#!/usr/bin/env python3
"""PreToolUse(Bash) guard for molt sessions (APPARATUS Wave 1, A1).

Turns four standing directives from reminders into MECHANISM by inspecting the
Bash command BEFORE it runs and refusing the dangerous classes:

  (a) destructive working-tree git on the SHARED checkout -- ``reset --hard``,
      ``checkout -- <path>``, ``clean -fd``, ``stash drop`` ... (M17/M18). Lifts
      ``tools/git_guard.py``'s classifier from convention to mechanical. A
      linked worktree or temp-index plumbing session is provably NOT the shared
      root and passes.
  (b) bare ``git add``-then-commit sweeps -- ``git add -A``/``.``, ``git commit
      -a``, or ``git add X && git commit`` without a ``-- <pathspec>`` (M20:
      those forms sweep other agents' staged WIP).
  (c) a raw heavy ``cargo build``/``molt build`` that bypasses ``proof_queue``
      while a queue is live (M27's <=1-2 builds; the pact launch-guard class).
  (d) a push to origin over HTTPS (M19: origin push MUST use SSH).

INVARIANTS (this runs on the operator's OWN sessions):
  * ``decide()`` is PURE and exhaustively unit-tested.
  * the wrapper is FAIL-OPEN: any exception -> ALLOW (``tools/hooks/_common``).
  * a BLOCK is emitted as exit code 2 with the reason on stderr -- the most
    portable, version-independent PreToolUse block (the tool call is refused and
    Claude is shown the reason). ALLOW is exit 0 with no output.
  * override token ``MOLT_GUARD_OK=1`` (in-command env-assignment or process
    env) turns a block into an audited allow, logged to
    ``.molt/state/waivers.jsonl``.

Design note on the block mechanism: Claude Code also supports blocking a
PreToolUse via ``{"hookSpecificOutput":{"permissionDecision":"deny",...}}`` on
exit 0. We deliberately use exit-2-with-stderr because it is the longest-lived,
most portable block contract across CLI versions and cannot be silently
ignored -- reliability of the refusal outranks UI polish for a safety guard.
Verified against the primary hooks reference (code.claude.com/docs/en/hooks).
"""

from __future__ import annotations

import os
import re
import shlex
import sys
from dataclasses import dataclass, field

try:
    from tools.hooks import _common
    from tools.hooks.waivers import record_waiver
except Exception:  # pragma: no cover - path-invocation fallback
    import os as _os

    sys.path.insert(0, _os.path.dirname(_os.path.dirname(_os.path.dirname(__file__))))
    from tools.hooks import _common
    from tools.hooks.waivers import record_waiver


def _ensure_disk_headroom_for_build() -> None:
    """Preemptive, AGENT-SAFE reclaim before a heavy build. Never raises/blocks.

    Fires the decoupled disk guard (tools/disk_guard.py) so a build never starts
    on a near-full disk -- the fix for the C:-hits-0-bytes class. The guard
    reclaims ONLY stale build-artifact dirs and NEVER touches a process; it is a
    fast no-op when free space is already above the high-water mark, so calling
    it before every build is cheap. Fail-open: any error is swallowed (logged by
    the guard) and the build proceeds.
    """
    if any(
        os.environ.get(k)
        for k in ("PYTEST_CURRENT_TEST", "PYTEST_VERSION")
    ):
        return  # never reclaim real artifacts during a test run
    try:
        from tools import disk_guard
    except Exception:
        return
    try:
        disk_guard.ensure_free_fail_open()
    except Exception:
        pass


OVERRIDE_TOKEN = "MOLT_GUARD_OK"


# --- pure classification helpers (lifted from tools/git_guard.py) ----------


def _has_flag(
    a: list[str], short_chars: str = "", long_flags: tuple[str, ...] = ()
) -> bool:
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
    return _has_flag(a, "D") or ("--delete" in a and _has_flag(a, "f", ("--force",)))


def _gc_destructive(a: list[str]) -> bool:
    return any(x in ("--prune=now", "--prune=all") for x in a)


def _updateref_destructive(a: list[str]) -> bool:
    return "-d" in a


def _reflog_destructive(a: list[str]) -> bool:
    return "expire" in a and any("--expire=now" in x or "--expire=all" in x for x in a)


DESTRUCTIVE_GIT = {
    "reset": _reset_destructive,
    "checkout": _checkout_destructive,
    "restore": lambda a: True,
    "switch": _switch_destructive,
    "clean": _clean_destructive,
    "stash": _stash_destructive,
    "branch": _branch_destructive,
    "gc": _gc_destructive,
    "update-ref": _updateref_destructive,
    "reflog": _reflog_destructive,
}

_ADD_SWEEP_FLAGS = {"-A", "--all", "-u", "--update"}
_HEAVY_CARGO_SUBS = {"build", "test", "check", "clippy", "run", "bench"}
# Substrings that mean "this build IS routed through the governed queue path".
_QUEUE_ROUTED_MARKERS = ("proof_queue", "molt_dev.py", "molt_dev ", "venv_exec.py")

_SEGMENT_SPLIT = re.compile(r"&&|\|\||;|\||\n")


@dataclass
class Segment:
    envs: dict[str, str] = field(default_factory=dict)
    exe: str = ""
    args: list[str] = field(default_factory=list)


@dataclass
class Decision:
    block: bool = False
    rule: str = ""
    reason: str = ""
    override: bool = False


def _split_segments(command: str) -> list[str]:
    return [s.strip() for s in _SEGMENT_SPLIT.split(command) if s.strip()]


def _parse_segment(seg: str) -> Segment | None:
    try:
        tokens = shlex.split(seg, posix=True)
    except ValueError:
        tokens = seg.split()
    if not tokens:
        return None
    out = Segment()
    i = 0
    while i < len(tokens) and re.match(r"^[A-Za-z_][A-Za-z0-9_]*=", tokens[i]):
        name, _, val = tokens[i].partition("=")
        out.envs[name] = val
        i += 1
    if i >= len(tokens):
        return out  # env-only segment
    out.exe = tokens[i]
    out.args = tokens[i + 1 :]
    return out


def _exe_base(exe: str) -> str:
    # Strip any path prefix so ".../cargo" or "cargo.exe" -> "cargo".
    base = re.split(r"[\\/]", exe)[-1]
    if base.endswith(".exe"):
        base = base[:-4]
    return base


def _git_subcommand(args: list[str]) -> tuple[str, list[str]]:
    """Return (subcommand, rest) skipping leading ``-c k=v`` / ``-C dir`` / ``--`` global opts."""
    i = 0
    while i < len(args):
        tok = args[i]
        if tok in ("-C", "-c", "--git-dir", "--work-tree", "--namespace"):
            i += 2
            continue
        if tok.startswith("-"):
            i += 1
            continue
        return tok, args[i + 1 :]
    return "", []


def _override_active(command: str, env) -> bool:
    if env and str(env.get(OVERRIDE_TOKEN, "")) == "1":
        return True
    # in-command env-assignment form: MOLT_GUARD_OK=1 git reset --hard ...
    return bool(re.search(rf"\b{OVERRIDE_TOKEN}=1\b", command))


# --- the pure decision surface ---------------------------------------------


def decide(
    command: str,
    *,
    in_shared_checkout: bool,
    queue_live: bool,
    origin_is_https: bool,
    env=None,
) -> Decision:
    """Pure: classify a Bash command against the four guarded classes.

    All environment-dependent facts are INJECTED so this function is fully
    unit-testable. Returns a ``Decision``; ``block`` is True only for a refused
    command with no active override.
    """
    if not command or not command.strip():
        return Decision()

    override = _override_active(command, env)

    segments = [_parse_segment(s) for s in _split_segments(command)]
    segments = [s for s in segments if s is not None]

    git_segs = [s for s in segments if _exe_base(s.exe) == "git"]

    # (a) destructive working-tree git on the shared checkout
    if in_shared_checkout:
        for s in git_segs:
            sub, rest = _git_subcommand(s.args)
            pred = DESTRUCTIVE_GIT.get(sub)
            if pred and pred(rest):
                cmd_txt = f"git {sub} {' '.join(rest)}".rstrip()
                return _mk(
                    override,
                    "destructive-git-shared-checkout",
                    f"`{cmd_txt}` is a DESTRUCTIVE working-tree op on the SHARED checkout. "
                    f"This class has destroyed other agents' uncommitted WIP (M17/M18). Use "
                    f"an ISOLATED `git worktree add`, or temp-index plumbing. Recovery "
                    f"snapshots: tools/git_guard.py snapshot.",
                )

    # (b) bare git add / commit sweeps (M20)
    has_add = False
    has_scoped_commit = False
    has_unscoped_commit = False
    for s in git_segs:
        sub, rest = _git_subcommand(s.args)
        if sub == "add":
            has_add = True
            if any(f in rest for f in _ADD_SWEEP_FLAGS) or "." in rest or ":/" in rest:
                add_txt = f"git add {' '.join(rest)}".rstrip()
                return _mk(
                    override,
                    "git-add-sweep",
                    f"`{add_txt}` stages a sweep of the whole tree/index and can absorb other "
                    f"agents' WIP (M20). Stage explicit paths: `git add -- <path> ...`, or "
                    f"commit with `git commit -- <pathspec>`.",
                )
        elif sub == "commit":
            if _has_flag(rest, "a", ("--all",)):
                return _mk(
                    override,
                    "git-commit-all-sweep",
                    "`git commit -a/--all` commits ALL tracked modifications, sweeping other "
                    "agents' WIP (M20). Commit explicit paths: `git commit -- <pathspec>`.",
                )
            if "--" in rest:
                has_scoped_commit = True
            elif not _has_flag(rest, "", ("--amend",)):
                has_unscoped_commit = True
    if has_add and has_unscoped_commit and not has_scoped_commit:
        return _mk(
            override,
            "git-add-commit-sweep",
            "`git add ... && git commit` without a `-- <pathspec>` sweeps whatever is staged, "
            "including other agents' WIP (M20). Scope the commit: `git commit -- <pathspec>`.",
        )

    # (c) heavy build bypassing the live proof_queue (M27)
    if queue_live and OVERRIDE_TOKEN not in command:
        routed = any(m in command for m in _QUEUE_ROUTED_MARKERS)
        if not routed:
            for s in segments:
                base = _exe_base(s.exe)
                sub = s.args[0] if s.args else ""
                if base == "cargo" and sub in _HEAVY_CARGO_SUBS:
                    return _mk(
                        override,
                        "build-bypasses-queue",
                        f"`cargo {sub}` is a heavy build launched while a proof_queue is LIVE. "
                        f"Route builds through the queue (M27's <=1-2 builds): "
                        f"`python tools/proof_queue.py ...`. Bypassing risks the OOM/contention class.",
                    )
                if base == "molt" and sub == "build":
                    return _mk(
                        override,
                        "build-bypasses-queue",
                        "`molt build` launched while a proof_queue is LIVE. Route builds through "
                        "the queue (M27): `python tools/proof_queue.py ...`.",
                    )

    # (d) https push to origin (M19)
    for s in git_segs:
        sub, rest = _git_subcommand(s.args)
        if sub == "push":
            pushes_https_url = any(
                a.lower().startswith("https://") or a.lower().startswith("http://")
                for a in rest
            )
            pushes_origin = "origin" in rest or not any(
                not a.startswith("-") for a in rest
            )
            if pushes_https_url or (pushes_origin and origin_is_https):
                return _mk(
                    override,
                    "https-push",
                    "Pushing to origin over HTTPS is refused (M19): the HTTPS OAuth cred lacks "
                    "the `workflow` scope and rejects any .github/workflows change. Use SSH "
                    "(git@github.com:adpena/molt.git).",
                )

    return Decision(override=override)


def _mk(override: bool, rule: str, reason: str) -> Decision:
    return Decision(block=not override, rule=rule, reason=reason, override=override)


# --- fail-open wrapper -----------------------------------------------------


def _maybe_destructive_git(command: str) -> bool:
    return "git" in command and any(
        k in command
        for k in (
            "reset",
            "checkout",
            "restore",
            "switch",
            "clean",
            "stash",
            "branch",
            "gc",
            "update-ref",
            "reflog",
        )
    )


def _maybe_heavy_build(command: str) -> bool:
    return ("cargo" in command) or ("molt build" in command)


def _queue_live(root) -> bool:
    """Is a proof_queue run active? Uncertain -> False (never block a build)."""
    cnt = _common.proof_queue_active_count(root, timeout=4.0)
    return bool(cnt) if cnt is not None else False


def run() -> int:
    data = _common.read_hook_input()
    tool_name = data.get("tool_name", "")
    tool_input = data.get("tool_input") or {}
    command = tool_input.get("command", "") if isinstance(tool_input, dict) else ""
    if tool_name and tool_name != "Bash":
        return 0
    if not command:
        return 0
    # Fast path: nothing we guard is mentioned -> allow without any git calls.
    if not any(tok in command for tok in ("git", "cargo", "molt")):
        return 0

    cwd = data.get("cwd")
    root = _common.repo_root(cwd)

    in_shared = False
    if _maybe_destructive_git(command):
        # Shared checkout = the MAIN worktree (not a linked worktree / plumbing).
        in_shared = not _common.is_linked_worktree(root)
    heavy_build = _maybe_heavy_build(command)
    if heavy_build:
        # Preemptive, agent-safe disk reclamation BEFORE the build starts, so a
        # near-full C: can never fail the build mid-compile (fail-open).
        _ensure_disk_headroom_for_build()
    queue_live = _queue_live(root) if heavy_build else False
    origin_https = _common.origin_is_https(root) if "push" in command else False

    decision = decide(
        command,
        in_shared_checkout=in_shared,
        queue_live=queue_live,
        origin_is_https=origin_https,
        env=dict(os.environ),
    )

    if decision.override and decision.rule:
        # An override neutralized a would-be block; audit it.
        record_waiver(
            "bash_guard",
            f"MOLT_GUARD_OK override of {decision.rule}",
            source="override-token",
            context=command[:400],
            root=root,
        )
        return 0

    if decision.block:
        sys.stderr.write(
            f"[molt bash_guard] BLOCKED ({decision.rule}): {decision.reason}\n"
            f"Override (audited): prefix the command with MOLT_GUARD_OK=1 once you have "
            f"verified it is safe.\n"
        )
        return 2
    return 0


def main() -> None:
    _common.run_fail_open("bash_guard", run)


if __name__ == "__main__":
    main()
