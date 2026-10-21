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
  * OBSERVE-ONLY while this guard's ``[[gate_flip]]`` state is ``warn`` (owner,
    2026-10-21): a would-block decision is appended to
    ``.molt/state/bash_guard_shadow.jsonl`` and the command is allowed. The
    flip to strict waits for a quote-, heredoc- and directory-aware command
    model proven by a replay of recorded session commands.
  * in strict state a BLOCK is emitted as exit code 2 with the reason on stderr -- the most
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
from collections.abc import Callable
from dataclasses import dataclass, field

from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.hooks import _common  # noqa: E402
from tools.hooks.waivers import record_waiver  # noqa: E402
from tools.check_gate_flips import FlipState  # noqa: E402


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
_HEAVY_CARGO_SUBS = {
    "build",
    "test",
    "check",
    "clippy",
    "run",
    "bench",
    "rustc",
    "doc",
    "rustdoc",
    "install",
}

_SEGMENT_SPLIT = re.compile(r"&&|\|\||;|\||\n")


@dataclass
class Segment:
    envs: dict[str, str] = field(default_factory=dict)
    exe: str = ""
    args: list[str] = field(default_factory=list)


class GuardFacts:
    """The host facts the rules may need, each read at most once and only when
    a rule reaches a command that needs it.

    The rules decide which facts to read, so no text predicate outside them
    can skip a rule (review of HF-426: a `"molt build"` substring test skipped
    the queue rule for `python -m molt.cli build`).
    """

    def __init__(
        self,
        *,
        in_shared_checkout: Callable[[], bool],
        queue_live: Callable[[], bool],
        origin_is_https: Callable[[], bool],
    ) -> None:
        self._readers = {
            "in_shared_checkout": in_shared_checkout,
            "queue_live": queue_live,
            "origin_is_https": origin_is_https,
        }
        self._values: dict[str, bool] = {}

    @classmethod
    def fixed(
        cls, *, shared: bool = False, queue: bool = False, https: bool = False
    ) -> "GuardFacts":
        return cls(
            in_shared_checkout=lambda: shared,
            queue_live=lambda: queue,
            origin_is_https=lambda: https,
        )

    def _read(self, name: str) -> bool:
        if name not in self._values:
            self._values[name] = bool(self._readers[name]())
        return self._values[name]

    def in_shared_checkout(self) -> bool:
        return self._read("in_shared_checkout")

    def queue_live(self) -> bool:
        return self._read("queue_live")

    def origin_is_https(self) -> bool:
        return self._read("origin_is_https")


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
    remaining = seg.lstrip()
    assignment = re.compile(
        r"^([A-Za-z_][A-Za-z0-9_]*)=((?:[^\s\"'\\]|\\.|'[^']*'|\"(?:[^\"\\]|\\.)*\")*)(?=\s|$)"
    )
    while match := assignment.match(remaining):
        words = shlex.split(match.group(0), posix=True)
        if len(words) != 1:
            return None
        name, _, value = words[0].partition("=")
        out.envs[name] = value
        remaining = remaining[match.end() :].lstrip()
    try:
        tokens = shlex.split(remaining, posix=True)
    except ValueError:
        tokens = remaining.split()
    if not tokens:
        return out
    out.exe = tokens[0]
    out.args = tokens[1:]
    return out


def _exe_base(exe: str) -> str:
    # Strip any path prefix so ".../cargo" or "cargo.exe" -> "cargo". Casefold:
    # on a case-insensitive filesystem (the macOS and Windows defaults) `GIT`
    # runs git, and the guard over-approximates rather than miss it.
    base = re.split(r"[\\/]", exe)[-1].casefold()
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


def _override_active(segment: Segment | None, env) -> bool:
    # An inline assignment applies only to its own execution segment and wins
    # over inherited environment. Quoted token/argument text is not a grant.
    if segment is not None and OVERRIDE_TOKEN in segment.envs:
        return segment.envs[OVERRIDE_TOKEN] == "1"
    return bool(env and str(env.get(OVERRIDE_TOKEN, "")) == "1")


# --- the pure decision surface ---------------------------------------------


def _heavy_payload(segment: Segment) -> tuple[str, str]:
    """Inspect executable payloads, never queue-looking text in other segments."""
    from tools.proof_queue_pkg import command_admission

    command = [segment.exe, *segment.args]
    modeled = command_admission._command_wrapper(command)
    if modeled is not None:
        command = modeled[1]
        if command_admission._command_wrapper(command) is not None:
            raise ValueError("command wrappers are limited to one typed layer")
    if command_admission._basename(command[0]) in {"uv", "uv.exe"}:
        _prefix, command = command_admission._uv_prefix_and_payload(command)
    entrypoint = command_admission._command_entrypoint(command)
    if entrypoint == ("python-cli-command", "molt.cli:build"):
        return "molt", "build"
    base = _exe_base(command[0])
    if base == "cargo":
        invocation = command_admission.parse_cargo_invocation(command)
        return base, invocation.subcommand or ""
    return base, command[1] if len(command) > 1 else ""


def decide(command: str, *, facts: GuardFacts, env=None) -> Decision:
    """Pure: classify a Bash command against the four guarded classes.

    Host facts are INJECTED (``facts``) and read only when a rule reaches a
    command that needs one, so this function is fully unit-testable. Returns a
    ``Decision``; ``block`` is True only for a refused command with no active
    override.
    """
    if not command or not command.strip():
        return Decision()

    override = _override_active(None, env)

    segments = [_parse_segment(s) for s in _split_segments(command)]
    segments = [s for s in segments if s is not None]

    git_segs = [s for s in segments if _exe_base(s.exe) == "git"]

    # (a) destructive working-tree git on the shared checkout
    for s in git_segs:
        override = _override_active(s, env)
        sub, rest = _git_subcommand(s.args)
        pred = DESTRUCTIVE_GIT.get(sub)
        if pred and pred(rest) and facts.in_shared_checkout():
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
        override = _override_active(s, env)
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

    # (c) Heavy builds are classified per execution segment. A queue command
    # elsewhere, a filename substring or an environment wrapper grants no route.
    for segment in segments:
        override = _override_active(segment, env)
        try:
            base, sub = _heavy_payload(segment)
        except ValueError as exc:
            if facts.queue_live():
                return _mk(override, "build-bypasses-queue", str(exc))
            continue
        if (
            (base == "cargo" and sub in _HEAVY_CARGO_SUBS)
            or (base == "molt" and sub == "build")
        ) and facts.queue_live():
            return _mk(
                override,
                "build-bypasses-queue",
                f"`{base} {sub}` is a heavy build launched while a proof_queue is LIVE. "
                "Route builds through the queue (M27): "
                "`uv run --python 3.12 python tools/proof_queue.py ...`.",
            )

    # (d) https push to origin (M19)
    for s in git_segs:
        override = _override_active(s, env)
        sub, rest = _git_subcommand(s.args)
        if sub == "push":
            pushes_https_url = any(
                a.lower().startswith("https://") or a.lower().startswith("http://")
                for a in rest
            )
            pushes_origin = "origin" in rest or not any(
                not a.startswith("-") for a in rest
            )
            if pushes_https_url or (pushes_origin and facts.origin_is_https()):
                return _mk(
                    override,
                    "https-push",
                    "Pushing to origin over HTTPS is refused (M19): the HTTPS OAuth cred lacks "
                    "the `workflow` scope and rejects any .github/workflows change. Use SSH "
                    "(git@github.com:adpena/molt.git).",
                )

    return Decision(override=_override_active(None, env))


def _mk(override: bool, rule: str, reason: str) -> Decision:
    return Decision(block=not override, rule=rule, reason=reason, override=override)


# --- fail-open wrapper -----------------------------------------------------


def _queue_live(root) -> bool:
    """Is a proof_queue run active? Uncertain -> False (never block a build)."""
    cnt = _common.proof_queue_active_count(root)
    return bool(cnt) if cnt is not None else False


SHADOW_NAME = "bash_guard_shadow.jsonl"

# Every rule needs one of these, in any case, in the command: git (rules a, b,
# d), cargo (rule c) or the `build` of `molt build` (rule c). The one source:
# tools/hooks/run.sh applies a projection generated from this tuple to the raw
# payload before it starts Python (tools/hooks/prefilter.generated.sh).
PREFILTER_TOKENS = ("git", "cargo", "build")


def passes_prefilter(text: str) -> bool:
    """Could any rule apply? A shell escape can still hide a token (HF-423)."""
    folded = text.casefold()
    return any(token in folded for token in PREFILTER_TOKENS)


def enforcement_state() -> FlipState:
    """This guard's ``[[gate_flip]]`` state, read through the one reader.

    ``strict`` blocks; ``warn``, no entry, or an unreadable plan only records
    the decision. Read only for a would-block decision, so an allowed command
    never pays for parsing the plan.
    """
    from tools.check_gate_flips import flip_state

    try:
        return flip_state("bash_guard") or "warn"
    except (OSError, ValueError):  # tomllib.TOMLDecodeError is a ValueError
        return "warn"


def run() -> int:
    data = _common.read_hook_input()
    tool_name = data.get("tool_name", "")
    tool_input = data.get("tool_input") or {}
    command = tool_input.get("command", "") if isinstance(tool_input, dict) else ""
    if tool_name and tool_name != "Bash":
        return 0
    if not command or not passes_prefilter(command):
        return 0

    cwd = data.get("cwd")
    root = _common.repo_root(cwd)
    facts = GuardFacts(
        # Shared checkout = the MAIN worktree (not a linked worktree/plumbing).
        in_shared_checkout=lambda: not _common.is_linked_worktree(root),
        queue_live=lambda: _queue_live(root),
        origin_is_https=lambda: _common.origin_is_https(root),
    )
    decision = decide(command, facts=facts, env=dict(os.environ))

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

    if decision.block and enforcement_state() != "strict":
        # Observe-only: record what strict mode would refuse; allow the call.
        _common.record_shadow(
            root,
            SHADOW_NAME,
            {
                "session_id": str(data.get("session_id", "")),
                "rule": decision.rule,
                "reason": decision.reason,
                "command": command,
                "cwd": str(cwd or ""),
            },
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
