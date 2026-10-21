#!/usr/bin/env python3
"""Shared primitives for the canonical Molt dev driver."""

from __future__ import annotations

from collections.abc import Callable
import subprocess
import os
import re
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Protocol

from tools import harness_memory_guard
from molt.cargo_execution_policy import normalize_cargo_environment
from molt.process_environment import ProcessEnvironment

from tools.command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

DEFAULT_REPO = Path(__file__).resolve().parents[1]

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2

# The trailer that keeps one change's identity across rebases and amends
# (Gerrit's commit-msg hook; docs/design/landing_queue.md, "Stable change ids").
CHANGE_ID_TRAILER = "Change-Id"
CHANGE_ID = re.compile(r"I[0-9a-f]{40}")


def _say(msg: str) -> None:
    """A status line -> stderr (so stdout stays clean for machine consumers)."""
    print(msg, file=sys.stderr, flush=True)


def _step(name: str) -> None:
    _say(f"==> {name}")


def _ok(msg: str) -> None:
    _say(f"    OK: {msg}")


def _fail(msg: str) -> None:
    _say(f"    FAIL: {msg}")


def _warn(msg: str) -> None:
    _say(f"    WARN: {msg}")


class DriverError(Exception):
    """A loud, fatal driver condition. Carries an exit code (default FAIL)."""

    def __init__(self, message: str, code: int = EXIT_FAIL):
        super().__init__(message)
        self.code = code


# The landing queue attributes a failure to a candidate only when the
# candidate caused it; these name the outcomes it must tell apart.
class CandidateFailure(DriverError):
    """The candidate failed a check that its own commits decide."""


class GateFailure(CandidateFailure):
    """A selected gate failed on the candidate."""


class RebaseConflict(DriverError):
    """The candidate does not apply to the current upstream."""


class PublicationRace(DriverError):
    """Upstream moved between the gates and the push confirmation."""


def _run_driver_command(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: ProcessEnvironment | None = None,
    input_text: str | None = None,
    timeout: float | None = 60.0,
    prefix: str = "MOLT_DEV",
) -> subprocess.CompletedProcess[str]:
    """Run one bounded captured driver command through the shared guard."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd or DEFAULT_REPO,
        env=env,
        input=input_text,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _run_driver_command_bytes(
    cmd: list[str],
    *,
    cwd: Path,
    env: ProcessEnvironment,
    timeout: float,
    prefix: str,
) -> subprocess.CompletedProcess[bytes]:
    """Run one bounded captured driver command with byte-exact output custody."""
    return harness_memory_guard.guarded_completed_process(
        cmd,
        prefix=prefix,
        cwd=cwd,
        env=env,
        capture_output=True,
        text=False,
        timeout=timeout,
    )


def _run_fast_captured_command(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: ProcessEnvironment | None = None,
    input_text: str | None = None,
    timeout: float | None = 60.0,
) -> subprocess.CompletedProcess[str]:
    """Run cheap non-shell plumbing directly, with a bounded fail-loud timeout."""
    try:
        return _COMMANDS.run(
            cmd,
            cwd=cwd or DEFAULT_REPO,
            env=env,
            input=input_text,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
            encoding="utf-8",
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else ""
        stderr = exc.stderr if isinstance(exc.stderr, str) else ""
        if timeout is None:
            timeout_msg = "command timed out"
        else:
            timeout_msg = f"command timed out after {timeout:.1f}s"
        if stderr:
            stderr = f"{stderr.rstrip()}\n{timeout_msg}\n"
        else:
            stderr = f"{timeout_msg}\n"
        return subprocess.CompletedProcess(cmd, 124, stdout, stderr)


class GitPlumbing(Protocol):
    """The driver's fail-loud `git -C <repo>` runner (`molt_dev.Git`)."""

    def _run(
        self, args: list[str], *, check: bool = True, input_text: str | None = None
    ) -> subprocess.CompletedProcess[str]: ...


@dataclass(frozen=True, slots=True)
class LandingRepresentation:
    """Which of a branch's commits an upstream already carries."""

    merge_base: str | None
    commits: tuple[str, ...]
    unrepresented: tuple[str, ...]

    @property
    def landed(self) -> bool:
        return not self.unrepresented


def landing_representation(
    git: GitPlumbing, tip: str, upstream: str
) -> LandingRepresentation:
    """The one landed predicate: every commit of `upstream..tip` is on upstream.

    A queue replays a lane onto main and may amend its last commit, so a
    landed tip need not be an ancestor of upstream. A commit is represented
    when its ``Change-Id:`` trailer appears on an upstream commit (Gerrit's
    rule), or when its patch-id matches one (``git cherry``'s rule). Both
    searches cover only upstream's commits since the merge base. Merge commits
    carry no patch of their own and are skipped, as ``git cherry`` skips them;
    an empty commit needs a matching Change-Id or an empty upstream commit.

    Cost: one ``log`` of ``upstream..tip``, which is empty when the tip is on
    upstream. Otherwise also one ``merge-base``, one ``log`` of upstream since
    the merge base when a lane commit carries a Change-Id, and one ``cherry``
    when a commit has no matching Change-Id. A Change-Id is ``I`` and 40 hex
    digits, the managed commit-msg hook's format.
    """
    listing = git._run(
        [
            "log",
            "-z",
            "--no-merges",
            f"--format=%H%x1f%(trailers:key={CHANGE_ID_TRAILER},valueonly,separator=%x1e)",
            f"{upstream}..{tip}",
        ]
    ).stdout
    commits: list[tuple[str, frozenset[str]]] = []
    for entry in listing.split("\0"):
        entry = entry.strip("\n")
        if not entry:
            continue
        sha, _, trailers = entry.partition("\x1f")
        ids = frozenset(
            value.strip()
            for value in trailers.split("\x1e")
            if CHANGE_ID.fullmatch(value.strip())
        )
        commits.append((sha.strip(), ids))
    if not commits:
        return LandingRepresentation(None, (), ())
    shas = tuple(sha for sha, _ids in commits)
    base = git._run(["merge-base", upstream, tip], check=False)
    if base.returncode == 1:
        return LandingRepresentation(None, shas, shas)  # Unrelated histories.
    if base.returncode != 0:
        raise DriverError(
            f"git merge-base {upstream} {tip} failed (exit {base.returncode}): "
            f"{base.stderr.strip()}"
        )
    merge_base = base.stdout.strip()
    upstream_ids: frozenset[str] = frozenset()
    if any(ids for _sha, ids in commits):
        upstream_listing = git._run(
            [
                "log",
                "-z",
                f"--format=%(trailers:key={CHANGE_ID_TRAILER},valueonly,separator=%x1e)",
                f"{merge_base}..{upstream}",
            ]
        ).stdout
        upstream_ids = frozenset(
            value.strip()
            for entry in upstream_listing.split("\0")
            for value in entry.split("\x1e")
            if CHANGE_ID.fullmatch(value.strip())
        )
    pending = [sha for sha, ids in commits if not ids & upstream_ids]
    if pending:
        cherry = git._run(["cherry", upstream, tip]).stdout
        on_upstream = {
            line[2:].strip() for line in cherry.splitlines() if line.startswith("- ")
        }
        pending = [sha for sha in pending if sha not in on_upstream]
    return LandingRepresentation(merge_base, shas, tuple(pending))


def _run_live_gate(
    cmd: str | tuple[str, ...], *, repo: Path, env: ProcessEnvironment
) -> int:
    """Run shell syntax or structured argv live through memory custody."""


def _gate_launch(
    cmd: str | tuple[str, ...], env: ProcessEnvironment
) -> tuple[list[str], ProcessEnvironment]:
    # A compound shell gate can launch Cargo in any arm. Its opaque shell argv
    # cannot trigger direct-Cargo admission, so normalize the inherited wrapper
    # environment before launching it, without parsing shell syntax.
    env, _policies = normalize_cargo_environment(env)
    if not isinstance(cmd, str):
        return list(cmd), env
    if os.name == "nt":
        shell = env.get("COMSPEC") or "cmd.exe"
        return [shell, "/d", "/s", "/c", cmd], env
    return ["/bin/sh", "-c", cmd], env


def _run_live_gate(
    cmd: str | tuple[str, ...], *, repo: Path, env: ProcessEnvironment
) -> int:
    """Run shell syntax or structured argv live through memory custody."""
    argv, env = _gate_launch(cmd, env)
    proc = harness_memory_guard.guarded_completed_process(
        argv,
        prefix="MOLT_TEST_SUITE",
        cwd=repo,
        env=env,
        capture_output=False,
        text=True,
        timeout=None,
    )
    return proc.returncode


def _run_captured_gate(
    cmd: str | tuple[str, ...],
    *,
    repo: Path,
    env: ProcessEnvironment,
    cancelled: Callable[[], bool],
) -> tuple[int, str, str, int | None]:
    """Run one gate beside others: capture its streams, honor cancellation.

    The guard owns the child tree; ``cancelled`` lets the scheduler stop a gate
    whose outcome no longer matters.
    """
    argv, env = _gate_launch(cmd, env)
    proc = harness_memory_guard.guarded_completed_process(
        argv,
        prefix="MOLT_TEST_SUITE",
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=None,
        cancellation_requested=cancelled,
    )
    peak = proc.peak_total or proc.peak
    peak_bytes = None if peak is None else peak.rss_kb * 1024
    return proc.returncode, proc.stdout or "", proc.stderr or "", peak_bytes
