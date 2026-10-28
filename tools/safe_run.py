#!/usr/bin/env python3
"""Measure an owned child through the shared guard; CLI output is diagnostic.

Python consumers use the typed result directly. Guest stdout/stderr never carry
measurement authority, and child launch-to-exit time excludes guard cleanup.
"""

from __future__ import annotations

import argparse
from collections.abc import Mapping
from contextlib import nullcontext
from dataclasses import dataclass, replace
import json
import math
import os
from pathlib import Path
import stat
import sys
from typing import BinaryIO

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))
from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from molt.file_publication import metadata_is_link_like  # noqa: E402
from tools import harness_memory_guard  # noqa: E402

EXIT_TIMEOUT = 124
EXIT_OOM = 137
EXIT_SPAWN = 125
ROOT = _REPO_ROOT


@dataclass
class RunOutcome:
    ok: bool
    elapsed_s: float | None
    peak_rss_mib: float | None
    status: str
    exit_code: int | None
    stdout: str | None = None
    stderr: str | None = None
    stdout_tail: str | None = None
    stderr_tail: str | None = None
    host_stderr: str | None = None
    guard_result: (
        harness_memory_guard.GuardedCompletedProcess[str]
        | harness_memory_guard.GuardedCompletedProcess[bytes]
        | None
    ) = None


def _tail_text(text: str | None, *, max_chars: int = 4096) -> str | None:
    return text[-max_chars:] if text else None


def _finite_nonnegative(value: object) -> bool:
    return (
        isinstance(value, (int, float))
        and not isinstance(value, bool)
        and math.isfinite(value)
        and value >= 0
    )


def _require_capture_file(
    path: Path, stream: BinaryIO, initial: os.stat_result
) -> None:
    observed = path.lstat()
    opened = os.fstat(stream.fileno())
    if (
        metadata_is_link_like(observed)
        or not stat.S_ISREG(observed.st_mode)
        or not stat.S_ISREG(opened.st_mode)
        or (initial.st_dev, initial.st_ino) != (opened.st_dev, opened.st_ino)
        or (observed.st_dev, observed.st_ino) != (opened.st_dev, opened.st_ino)
    ):
        raise ValueError("guest stderr capture is not its original regular file")


def _output_text(value: str | bytes | None) -> str | None:
    # Binary capture avoids the text stream's universal-newline translation.
    return (
        value.decode("utf-8", "surrogateescape") if isinstance(value, bytes) else value
    )


def observed_run_outcome(
    proc: harness_memory_guard.GuardedCompletedProcess[str]
    | harness_memory_guard.GuardedCompletedProcess[bytes],
    *,
    captured_stderr: str | bytes | None,
    capture_output: bool = True,
    host_stderr: str | bytes | None = None,
) -> RunOutcome:
    """Interpret one closed guard observation; never infer timing from its wrapper.

    Native capture supplies the guard's child stderr. Manifest-bound WASM
    supplies its owned semantic capture after closure, retaining host diagnostics
    separately. Every benchmark consumer uses this same completion policy.
    """
    if not isinstance(proc, harness_memory_guard.GuardedCompletedProcess):
        raise TypeError("measurement requires the shared guard's typed result")
    complete = (
        proc.descendants_closed is True
        and proc.infrastructure_failure is None
        and not proc.cancelled
        and proc.guard_signal is None
        and not proc.orphaned_process_groups
    )
    elapsed = proc.child_elapsed_s
    if not _finite_nonnegative(elapsed) or elapsed == 0:
        elapsed = None
    peaks = [
        v.rss_kb / 1024.0
        for v in (proc.peak, proc.peak_total)
        if v is not None and _finite_nonnegative(v.rss_kb)
    ]
    peak = max(peaks) if peaks else None
    if proc.infrastructure_failure is not None or not complete:
        status = "infrastructure_error"
    elif proc.timed_out:
        status = "timeout"
    elif proc.violation is not None:
        status = "oom"
    elif proc.returncode != 0 or proc.child_returncode != 0:
        status = "nonzero"
    elif (
        elapsed is None
        or peak is None
        or (capture_output and (proc.stdout is None or captured_stderr is None))
    ):
        status = "error"
    else:
        status = "ok"
    return RunOutcome(
        ok=status == "ok",
        elapsed_s=elapsed,
        peak_rss_mib=peak,
        status=status,
        exit_code=proc.returncode,
        stdout=_output_text(proc.stdout) if capture_output else None,
        stderr=_output_text(captured_stderr) if capture_output else None,
        stdout_tail=_tail_text(_output_text(proc.stdout)),
        stderr_tail=_tail_text(_output_text(proc.stderr)),
        host_stderr=_output_text(host_stderr),
        guard_result=proc,
    )


def _run_measured_command(
    cmd: list[str],
    *,
    env: Mapping[str, str],
    rss_mb: int,
    timeout_s: float,
    label: str,
    capture_output: bool = False,
    cwd: Path = ROOT,
    prefix: str = "MOLT_BENCH",
    poll_s: float = 0.01,
    guest_stderr_path: Path | None = None,
    limits: harness_memory_guard.HarnessMemoryLimits | None = None,
) -> RunOutcome:
    """One child launch and typed guard result, without a receipt subprocess.

    The optional guest capture is precreated inside the subject's owned output
    directory. Its original inode stays open through launch and closure; only
    the host's stderr is separated, never normalized or discarded.
    """
    if (
        isinstance(rss_mb, bool)
        or not isinstance(rss_mb, int)
        or rss_mb <= 0
        or not _finite_nonnegative(timeout_s)
        or timeout_s <= 0
        or not _finite_nonnegative(poll_s)
        or poll_s <= 0
    ):
        raise ValueError("measurement limits must be positive finite values")
    base = (
        limits
        if limits is not None
        else harness_memory_guard.limits_from_env(prefix, env)
    )
    cap = rss_mb / 1024.0
    limits = replace(
        base,
        max_process_rss_gb=min(base.max_process_rss_gb, cap),
        max_total_rss_gb=min(base.max_total_rss_gb, cap),
        poll_interval=poll_s,
        dynamic_process_rss=False,
        dynamic_total_rss=False,
    )
    capture = nullcontext(None)
    if guest_stderr_path is not None:
        initial = guest_stderr_path.lstat()
        if metadata_is_link_like(initial) or not stat.S_ISREG(initial.st_mode):
            raise ValueError("guest stderr capture requires a direct regular file")
        flags = (
            os.O_RDWR
            | getattr(os, "O_NOFOLLOW", 0)
            | getattr(os, "O_NONBLOCK", 0)
            | getattr(os, "O_BINARY", 0)
        )
        capture = os.fdopen(os.open(guest_stderr_path, flags), "r+b")
    with capture as stream:
        if stream is not None:
            assert guest_stderr_path is not None
            _require_capture_file(guest_stderr_path, stream, initial)
            stream.truncate(0)
        proc = harness_memory_guard.guarded_completed_process(
            cmd,
            prefix=prefix,
            env=env,
            cwd=cwd,
            capture_output=True,
            text=False,
            timeout=timeout_s,
            limits=limits,
            operation_role=label,
            encoding="utf-8",
            errors="surrogateescape",
        )
        if not isinstance(proc, harness_memory_guard.GuardedCompletedProcess):
            raise TypeError("measurement requires the shared guard's typed result")
        closed = proc.descendants_closed is True
        child_stderr = proc.child_stderr
        host_stderr = None
        if stream is not None:
            assert guest_stderr_path is not None
            _require_capture_file(guest_stderr_path, stream, initial)
            host_stderr = child_stderr
            child_stderr = None
            if closed:
                stream.seek(0)
                child_stderr = stream.read().decode("utf-8", "surrogateescape")
    return observed_run_outcome(
        proc,
        captured_stderr=child_stderr,
        capture_output=capture_output,
        host_stderr=host_stderr,
    )


def _parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    # Split on the first standalone "--" so the child command keeps its own flags.
    if "--" in argv:
        sep = argv.index("--")
        ours, cmd = argv[:sep], argv[sep + 1 :]
    else:
        # No explicit separator: take leading --opt[=val] / known flags for us,
        # the rest is the command. We only own a fixed, known flag set, so the
        # first token that is not one of ours starts the command.
        ours, cmd = [], []
        known_val = {"--rss-mb", "--timeout", "--poll", "--label"}
        known_flag = {"--quiet", "--json"}
        i = 0
        while i < len(argv):
            tok = argv[i]
            if tok in known_val:
                ours += [tok, argv[i + 1]] if i + 1 < len(argv) else [tok]
                i += 2
                continue
            if tok.split("=", 1)[0] in known_val and "=" in tok:
                ours.append(tok)
                i += 1
                continue
            if tok in known_flag:
                ours.append(tok)
                i += 1
                continue
            cmd = argv[i:]
            break
        else:
            cmd = []

    p = argparse.ArgumentParser(prog="safe_run.py", add_help=True)
    p.add_argument("--rss-mb", type=int, default=2048)
    p.add_argument("--timeout", type=float, default=30.0)
    p.add_argument("--poll", type=float, default=0.2)
    p.add_argument("--label", default=None)
    p.add_argument("--quiet", action="store_true")
    p.add_argument("--json", action="store_true")
    ns = p.parse_args(ours)
    return ns, cmd


def main(argv: list[str]) -> int:
    ns, cmd = _parse_args(argv)
    if not cmd:
        print("safe_run.py: no command given (use `-- CMD ARGS`)", file=sys.stderr)
        return EXIT_SPAWN
    label = ns.label or os.path.basename(cmd[0])
    try:
        outcome = _run_measured_command(
            cmd,
            env=dict(os.environ),
            rss_mb=ns.rss_mb,
            timeout_s=ns.timeout,
            poll_s=ns.poll,
            label=label,
            capture_output=True,
            cwd=Path.cwd(),
        )
    except (OSError, ValueError) as exc:
        print(f"safe_run.py: failed to measure {cmd[0]}: {exc}", file=sys.stderr)
        return EXIT_SPAWN
    if outcome.stdout:
        sys.stdout.write(outcome.stdout)
    proc = outcome.guard_result
    assert proc is not None
    if proc.stderr:
        sys.stderr.write(proc.stderr)
    if ns.json:
        print(
            "SAFE_RUN "
            + json.dumps(
                {
                    "label": label,
                    "status": outcome.status,
                    "exit": outcome.exit_code,
                    "child_returncode": proc.child_returncode,
                    "infrastructure_failure": (
                        None
                        if proc.infrastructure_failure is None
                        else proc.infrastructure_failure.json_payload()
                    ),
                    "peak_rss_mib": outcome.peak_rss_mib,
                    "elapsed_s": outcome.elapsed_s,
                    "guard_elapsed_s": proc.elapsed_s,
                    "cleanup_elapsed_s": proc.cleanup_elapsed_s,
                    "timing_scope": "child-launch-to-exit",
                    "rss_limit_mib": ns.rss_mb,
                    "timeout_s": ns.timeout,
                }
            ),
            file=sys.stderr,
        )
    elif not (ns.quiet and outcome.ok):
        print(
            f"SAFE_RUN [{label}] status={outcome.status} exit={outcome.exit_code} "
            f"peak_rss={outcome.peak_rss_mib}MiB elapsed={outcome.elapsed_s}s "
            f"limit_rss={ns.rss_mb}MiB limit_t={ns.timeout:g}s",
            file=sys.stderr,
        )
    return proc.returncode if outcome.ok or proc.returncode != 0 else EXIT_SPAWN


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
