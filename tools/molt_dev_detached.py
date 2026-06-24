#!/usr/bin/env python3
"""Detached daemon commands for the canonical Molt dev driver."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import tempfile
import time
from pathlib import Path

from molt_dev_common import (
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    DriverError,
    _fail,
    _ok,
    _say,
)
from molt_dev_probe import probe_path, probe_pid

# State root for detached daemons. Per-name dirs hold: pid, sid, cmd.json,
# run.log, rc. The rc file is the ONLY proof of orderly completion: a dead
# pid with no rc is the hazard-11 died-silent class, and detached-verify
# reports it as such.
DETACHED_STATE_ROOT = Path(tempfile.gettempdir()) / "molt_dev_detached"

_DETACHED_NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")


def _detached_state_dir(name: str, override: str | None) -> Path:
    if not _DETACHED_NAME_RE.fullmatch(name):
        raise DriverError(
            f"detached: name {name!r} must match {_DETACHED_NAME_RE.pattern}",
            code=EXIT_USAGE,
        )
    root = Path(override).resolve() if override else DETACHED_STATE_ROOT
    return root / name


def _atomic_write_text(path: Path, text: str) -> None:
    tmp = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def _exec_wait_rc(command: list[str], env: dict[str, str]) -> int:
    """Exec ``command`` in a child and return shell-style exit status."""
    if not command:
        os.write(1, b"detached-run: empty command\n")
        return 127
    child = os.fork()
    if child == 0:
        try:
            os.execvpe(command[0], command, env)
        except FileNotFoundError as exc:
            os.write(1, f"detached-run: exec failed: {exc}\n".encode())
            os._exit(127)
        except Exception as exc:  # noqa: BLE001 - child must report every exec death
            os.write(1, f"detached-run: exec crashed: {exc}\n".encode())
            os._exit(126)
    while True:
        try:
            _, status = os.waitpid(child, 0)
            break
        except InterruptedError:
            continue
    if os.WIFEXITED(status):
        return os.WEXITSTATUS(status)
    if os.WIFSIGNALED(status):
        return 128 + os.WTERMSIG(status)
    return 126


def _detached_daemonize(
    state: Path, command: list[str], cwd: Path, env: dict[str, str]
) -> int:
    """Double-fork + setsid; the grandchild runs `command` and writes rc.

    Returns (in the ORIGINAL process) the daemon pid read back from the state
    dir. The grandchild NEVER returns: it forks/execs the command, records the
    exit status (127 exec-failure / 126 crash sentinels included), and
    `os._exit`s, so no parent atexit/exception machinery runs twice.
    """
    pid_f = state / "pid"
    first = os.fork()
    if first == 0:
        # First child: new session, then fork the real daemon and exit so the
        # daemon is reparented to init (no controlling terminal, no harness
        # process-group membership).
        os.setsid()
        second = os.fork()
        if second > 0:
            os._exit(0)
        # The daemon (grandchild).
        try:
            fd = os.open(state / "run.log", os.O_WRONLY | os.O_CREAT | os.O_TRUNC)
            os.dup2(fd, 1)
            os.dup2(fd, 2)
            null = os.open(os.devnull, os.O_RDONLY)
            os.dup2(null, 0)
            _atomic_write_text(state / "sid", str(os.getsid(0)))
            _atomic_write_text(pid_f, str(os.getpid()))
            os.chdir(cwd)
            try:
                rc = _exec_wait_rc(command, env)
            except Exception as exc:  # noqa: BLE001 - daemon must record ANY death
                os.write(1, f"detached-run: daemon crashed: {exc}\n".encode())
                rc = 126
            _atomic_write_text(state / "rc", str(rc))
        finally:
            os._exit(0)
    # Original process: reap the first child (exits immediately post-fork) and
    # wait, bounded, for the daemon's pid file to appear.
    os.waitpid(first, 0)
    deadline = time.monotonic() + 5.0
    daemon_pid: int | None = None
    while time.monotonic() < deadline:
        if pid_f.exists():
            raw_pid = pid_f.read_text(encoding="utf-8").strip()
            if raw_pid:
                daemon_pid = int(raw_pid)
                break
        time.sleep(0.05)
    if daemon_pid is None:
        raise DriverError(f"detached-run: daemon never wrote {pid_f} within 5s")
    return daemon_pid


def cmd_detached_run(args: argparse.Namespace) -> int:
    command = list(args.command or [])
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise DriverError("detached-run: give the command after `--`", code=EXIT_USAGE)
    state = _detached_state_dir(args.name, args.state_dir)
    pid_f, rc_f = state / "pid", state / "rc"
    if pid_f.exists():
        old_pid = int(pid_f.read_text(encoding="utf-8").strip() or "0")
        if old_pid and probe_pid(old_pid)["alive"] and not rc_f.exists():
            raise DriverError(
                f"detached-run: {args.name!r} is already RUNNING (pid {old_pid}). "
                "This driver NEVER kills - wait, detached-verify it, or use a "
                "new --name."
            )
        if not args.replace:
            raise DriverError(
                f"detached-run: state for {args.name!r} already exists at "
                f"{state} (finished or died). Pass --replace to clear DEAD "
                "state and respawn."
            )
        shutil.rmtree(state)
    state.mkdir(parents=True, exist_ok=True)
    cwd = Path(args.cwd).resolve() if args.cwd else Path.cwd()
    if not cwd.is_dir():
        raise DriverError(
            f"detached-run: --cwd {cwd} is not a directory", code=EXIT_USAGE
        )
    env = dict(os.environ)
    # Unbuffered IO so a group-kill cannot eat block-buffered progress (the
    # empty-log signature that made hazard 11 undiagnosable).
    env["PYTHONUNBUFFERED"] = "1"
    for kv in args.env or []:
        key, sep, value = kv.partition("=")
        if not sep:
            raise DriverError(
                f"detached-run: --env needs K=V, got {kv!r}", code=EXIT_USAGE
            )
        env[key] = value
    _atomic_write_text(
        state / "cmd.json",
        json.dumps(
            {
                "argv": command,
                "cwd": str(cwd),
                "start_unix": time.time(),
                "env_overrides": list(args.env or []),
            },
            indent=2,
        ),
    )
    daemon_pid = _detached_daemonize(state, command, cwd, env)
    _ok(f"detached {args.name!r} spawned: pid {daemon_pid}")
    _say(f"    state: {state}")
    _say(f"    log:   {state / 'run.log'}")
    _say("    REQUIRED next step, in a LATER tool call (teardown of THIS call")
    _say("    is exactly what hazard 11 is about):")
    _say(
        f"      python3 tools/molt_dev.py detached-verify --name {args.name}"
        f" --min-age-s {args.verify_min_age_hint}"
    )
    if args.json:
        print(
            json.dumps({"name": args.name, "pid": daemon_pid, "state_dir": str(state)})
        )
    return EXIT_OK


def cmd_detached_verify(args: argparse.Namespace) -> int:
    state = _detached_state_dir(args.name, args.state_dir)
    pid_f, rc_f, log_f = state / "pid", state / "rc", state / "run.log"
    if not pid_f.exists():
        raise DriverError(
            f"detached-verify: no state for {args.name!r} at {state} "
            "(was detached-run ever invoked?)"
        )
    pid = int(pid_f.read_text(encoding="utf-8").strip())
    log_probe = probe_path(log_f)
    age_s = round(time.time() - pid_f.stat().st_mtime, 1)
    result: dict = {
        "name": args.name,
        "pid": pid,
        "age_s": age_s,
        "log_size": log_probe.get("size", 0) if log_probe.get("exists") else 0,
        "state_dir": str(state),
    }
    if rc_f.exists():
        rc = int(rc_f.read_text(encoding="utf-8").strip())
        result["status"], result["rc"] = "done", rc
        if args.json:
            print(json.dumps(result))
        if rc == 0:
            _ok(f"detached {args.name!r}: DONE rc=0 (log {result['log_size']}B)")
            return EXIT_OK
        _fail(f"detached {args.name!r}: DONE rc={rc} (log {result['log_size']}B)")
        return EXIT_FAIL
    if probe_pid(pid)["alive"]:
        if age_s < args.min_age_s:
            result["status"] = "too-young"
            if args.json:
                print(json.dumps(result))
            _fail(
                f"detached {args.name!r}: alive but only {age_s}s old "
                f"(< --min-age-s {args.min_age_s}); the spawning call's "
                "teardown window may still reap it - re-verify later."
            )
            return EXIT_FAIL
        result["status"] = "running"
        if args.json:
            print(json.dumps(result))
        _ok(
            f"detached {args.name!r}: RUNNING (pid {pid}, {age_s}s, "
            f"log {result['log_size']}B)"
        )
        return EXIT_OK
    result["status"] = "died-silent"
    if args.json:
        print(json.dumps(result))
    _fail(
        f"detached {args.name!r}: DIED-SILENT - pid {pid} is gone and no rc "
        f"was written (hazard-11 group-kill class). Log may be truncated by "
        f"lost buffers: {log_f} ({result['log_size']}B)"
    )
    return EXIT_FAIL
