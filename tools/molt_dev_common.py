#!/usr/bin/env python3
"""Shared primitives for the canonical Molt dev driver."""

from __future__ import annotations

from collections.abc import Callable
import subprocess
import os
import sys
from pathlib import Path

from tools import harness_memory_guard
from molt.cargo_execution_policy import normalize_cargo_environment
from molt.process_environment import ProcessEnvironment

from tools.command_execution import CommandExecutor  # type: ignore

_COMMANDS = CommandExecutor.for_file(__file__)

DEFAULT_REPO = Path(__file__).resolve().parents[1]

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_USAGE = 2


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

    def __init__(
        self,
        message: str,
        code: int = EXIT_FAIL,
        *,
        maintenance_selection: tuple[Path, ProcessEnvironment] | None = None,
    ):
        super().__init__(message)
        self.code = code
        # Only a completed command attaches its captured selection. The CLI
        # publishes this failure before scheduling its independent maintenance.
        self.maintenance_selection = maintenance_selection


# The landing queue attributes a failure to a candidate only when the
# candidate caused it; these name the outcomes it must tell apart.
class CandidateFailure(DriverError):
    """The candidate failed a check that its own commits decide."""


class GateFailure(CandidateFailure):
    """A selected gate failed on the candidate."""


class RebaseConflict(DriverError):
    """The candidate does not apply to the current upstream."""


class PublicationRace(DriverError):
    """Upstream moved after the candidate's upstream was selected."""


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
    timeout: float | None = None,
) -> subprocess.CompletedProcess[str]:
    """Run cheap non-shell plumbing directly; it has no wall-clock deadline.

    A deadline on local plumbing measures the host, not the command. On
    2026-10-21 the host stalled for 38 minutes and two 5 ms Git reads in a
    landing gate passed the old 60 s bound, which failed a 47-minute batch.
    Local plumbing now waits out a stall. A network transfer is bounded by its
    progress instead (tools/git_transfer.py, applied by CommandExecutor). A
    caller that must bound a command passes ``timeout`` and receives exit 124
    when it expires.
    """
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


def _run_fast_captured_bytes(
    cmd: list[str], *, cwd: Path, env: ProcessEnvironment
) -> subprocess.CompletedProcess[bytes]:
    """``_run_fast_captured_command`` with byte-exact output (paths, archives)."""
    return _COMMANDS.run(cmd, cwd=cwd, env=env, capture_output=True, check=False)


def _gate_launch(
    cmd: str | tuple[str, ...], env: ProcessEnvironment, *, cwd: Path
) -> tuple[list[str], ProcessEnvironment]:
    # A compound shell gate can launch Cargo in any arm. Its opaque shell argv
    # cannot trigger direct-Cargo admission, so normalize the inherited wrapper
    # environment before launching it, without parsing shell syntax.
    env, _policies = normalize_cargo_environment(env, cwd=cwd)
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
    argv, env = _gate_launch(cmd, env, cwd=repo)
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


#: The bounded end of each stream a gate returns when its streams go to files.
GATE_STREAM_TAIL_BYTES = 64 * 1024


def _run_captured_gate(
    cmd: str | tuple[str, ...],
    *,
    repo: Path,
    env: ProcessEnvironment,
    cancelled: Callable[[], bool],
    streams: tuple[Path, Path] | None = None,
    label: str | None = None,
) -> harness_memory_guard.GuardedCompletedProcess[str]:
    """Run one gate: capture its streams, honor cancellation.

    The guard owns the child tree; ``cancelled`` lets the scheduler stop a gate
    whose outcome no longer matters. With ``streams`` the child writes stdout
    and stderr to those files while it runs (a killed runner leaves them), and
    the outcome holds only each stream's bounded end. ``label`` keeps the
    guard's keepalive line, which a captured run otherwise drops.
    """
    argv, env = _gate_launch(cmd, env, cwd=repo)
    files: dict[str, object] = {}
    if streams is not None:
        files = {
            "stdout_capture_path": streams[0],
            "stderr_capture_path": streams[1],
            "capture_tail_bytes": GATE_STREAM_TAIL_BYTES,
        }
    proc = harness_memory_guard.guarded_completed_process(
        argv,
        prefix="MOLT_TEST_SUITE",
        cwd=repo,
        env=env,
        capture_output=True,
        text=True,
        timeout=None,
        cancellation_requested=cancelled,
        progress_label=label,
        **files,
    )
    if not isinstance(proc, harness_memory_guard.GuardedCompletedProcess) or tuple(
        proc.args
    ) != tuple(argv):
        raise DriverError("gate guard returned an unavailable or mismatched outcome")
    return proc
