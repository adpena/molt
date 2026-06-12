#!/usr/bin/env python3
"""safe_run.py — run a command (especially a molt-compiled binary) under HARD
wall-time AND resident-memory (RSS) caps, killing the whole process group
cleanly *before* a runaway loop can OOM the host or hang the session.

This exists because raw compiled binaries carry no memory guard of their own,
and the harness memory guard only wraps `molt run`/`molt test`/`molt build` —
NOT bare `./binary` execution. Any time you bisect, profile, or smoke-test a
compiled binary directly (or run anything that might infinite-loop / allocate
unboundedly), route it through this wrapper.

Usage:
    python3 tools/safe_run.py [options] -- CMD [ARGS...]
    python3 tools/safe_run.py [options] CMD [ARGS...]     # `--` optional

Options:
    --rss-mb N      kill if the process-group RSS exceeds N MiB   (default 2048)
    --timeout S     kill if wall-clock exceeds S seconds          (default 30)
    --poll S        RSS/timeout poll interval in seconds          (default 0.2)
    --label TEXT    label for the SAFE_RUN status line            (default cmd[0])
    --quiet         suppress the SAFE_RUN status line on success
    --json          emit a one-line JSON status to stderr (for tooling)

Behaviour:
    * The command runs in its own session/process-group; on a violation the
      ENTIRE group gets SIGKILL (so children/daemons die too).
    * stdout/stderr are inherited (live), so partial output is preserved up to
      the kill — exactly what bisecting needs.
    * A status line is written to STDERR (never stdout) so piping the command's
      stdout stays clean.

Exit codes:
    0..127   the child's own exit code (forwarded) when it exits on its own
    124      TIMEOUT  — killed for exceeding --timeout
    137      OOM      — killed for exceeding --rss-mb (128 + SIGKILL)
    125      could not start the command
"""

from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import time

EXIT_TIMEOUT = 124
EXIT_OOM = 137
EXIT_SPAWN = 125


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


def _group_rss_kib(pgid: int) -> int:
    """Sum RSS (KiB) of every process in the given process group via `ps`.

    Dependency-free and works on macOS + Linux. Returns 0 if `ps` reports
    nothing (e.g. the group already exited)."""
    try:
        out = subprocess.run(
            ["ps", "-o", "rss=", "-g", str(pgid)],
            capture_output=True,
            text=True,
            timeout=5,
        ).stdout
    except Exception:
        return 0
    total = 0
    for line in out.split():
        try:
            total += int(line)
        except ValueError:
            pass
    return total


def _kill_group(pgid: int) -> None:
    for sig in (signal.SIGTERM, signal.SIGKILL):
        try:
            os.killpg(pgid, sig)
        except ProcessLookupError:
            return
        except Exception:
            pass
        time.sleep(0.15)


def main(argv: list[str]) -> int:
    ns, cmd = _parse_args(argv)
    if not cmd:
        print("safe_run.py: no command given (use `-- CMD ARGS`)", file=sys.stderr)
        return EXIT_SPAWN

    label = ns.label or os.path.basename(cmd[0])
    rss_limit_kib = ns.rss_mb * 1024
    start = time.monotonic()

    try:
        # start_new_session=True -> child is a session+group leader (pgid == pid),
        # so we can SIGKILL the whole tree and not just the immediate child.
        proc = subprocess.Popen(cmd, start_new_session=True)
    except FileNotFoundError:
        print(f"safe_run.py: command not found: {cmd[0]}", file=sys.stderr)
        return EXIT_SPAWN
    except Exception as exc:  # noqa: BLE001 - report any spawn failure cleanly
        print(f"safe_run.py: failed to start {cmd[0]}: {exc}", file=sys.stderr)
        return EXIT_SPAWN

    pgid = proc.pid  # equals the new session/group id
    peak_kib = 0
    status = "ok"
    rc = 0

    try:
        while True:
            rc = proc.poll()
            if rc is not None:
                break
            elapsed = time.monotonic() - start
            if elapsed > ns.timeout:
                status = "timeout"
                _kill_group(pgid)
                proc.wait()
                rc = EXIT_TIMEOUT
                break
            rss = _group_rss_kib(pgid)
            if rss > peak_kib:
                peak_kib = rss
            if rss > rss_limit_kib:
                status = "oom"
                _kill_group(pgid)
                proc.wait()
                rc = EXIT_OOM
                break
            time.sleep(ns.poll)
    except KeyboardInterrupt:
        status = "interrupted"
        _kill_group(pgid)
        proc.wait()
        rc = 130

    if rc is None:
        rc = proc.returncode if proc.returncode is not None else 0

    elapsed = time.monotonic() - start
    peak_mib = peak_kib / 1024.0
    detail = (
        f"status={status} exit={rc} peak_rss={peak_mib:.0f}MiB "
        f"elapsed={elapsed:.2f}s limit_rss={ns.rss_mb}MiB limit_t={ns.timeout:g}s"
    )
    if ns.json:
        print(
            "SAFE_RUN "
            + json.dumps(
                {
                    "label": label,
                    "status": status,
                    "exit": rc,
                    "peak_rss_mib": round(peak_mib),
                    "elapsed_s": round(elapsed, 3),
                    "rss_limit_mib": ns.rss_mb,
                    "timeout_s": ns.timeout,
                }
            ),
            file=sys.stderr,
        )
    elif not (ns.quiet and status == "ok"):
        print(f"SAFE_RUN [{label}] {detail}", file=sys.stderr)

    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
