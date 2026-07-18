#!/usr/bin/env python3
"""Detached daemon commands for the canonical Molt dev driver."""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import TextIO

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


def _windows_hidden_creationflags() -> int:
    flags = 0
    for name in ("CREATE_NEW_PROCESS_GROUP", "CREATE_NO_WINDOW", "DETACHED_PROCESS"):
        flags |= getattr(subprocess, name, 0)
    return flags


def _windows_owned_child_creationflags() -> int:
    # The worker is already detached. The command it supervises must stay owned
    # by that worker so rc/log custody has exactly one process boundary.
    return getattr(subprocess, "CREATE_NO_WINDOW", 0)


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


def _write_exec_message(message: str, log_file: TextIO | None) -> None:
    if log_file is not None:
        log_file.write(message)
        log_file.flush()
    else:
        os.write(1, message.encode())


def _exec_wait_rc(
    command: list[str], env: dict[str, str], log_file: TextIO | None = None
) -> int:
    """Run ``command`` in the detached supervisor and return shell-style status."""
    if not command:
        _write_exec_message("detached-run: empty command\n", log_file)
        return 127
    try:
        proc = subprocess.run(
            command,
            env=env,
            check=False,
            creationflags=(
                _windows_owned_child_creationflags() if os.name == "nt" else 0
            ),
            stdout=log_file if log_file is not None else None,
            stderr=subprocess.STDOUT if log_file is not None else None,
        )
        return proc.returncode if proc.returncode >= 0 else 128 + abs(proc.returncode)
    except FileNotFoundError as exc:
        _write_exec_message(f"detached-run: exec failed: {exc}\n", log_file)
        return 127
    except Exception as exc:  # noqa: BLE001 - supervisor must report every exec death
        _write_exec_message(f"detached-run: exec crashed: {exc}\n", log_file)
        return 126


def _detached_daemonize(
    state: Path, command: list[str], cwd: Path, env: dict[str, str]
) -> int:
    """Spawn one isolated supervisor without forking the live Python process.

    The caller can be multi-threaded (the proof memory guard is), so POSIX
    ``fork()`` is not a safe control-plane primitive.  A fresh interpreter is
    the single cross-platform supervisor authority: it owns the session/process
    group, command log, child lifetime, and terminal rc record.
    """
    payload_path = state / "worker.json"
    _atomic_write_text(
        payload_path,
        json.dumps({"argv": command, "cwd": str(cwd)}, indent=2),
    )
    worker = [
        sys.executable,
        str(Path(__file__).resolve()),
        "--detached-worker",
        str(payload_path),
    ]
    if os.name == "nt":
        proc = subprocess.Popen(
            worker,
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=_windows_hidden_creationflags(),
        )
    else:
        proc = subprocess.Popen(
            worker,
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            start_new_session=True,
        )
    if os.name == "nt":
        _atomic_write_text(state / "sid", f"windows-process-group:{proc.pid}")
        _atomic_write_text(state / "pid", str(proc.pid))
        return proc.pid

    pid_f = state / "pid"
    deadline = time.monotonic() + 5.0
    while time.monotonic() < deadline:
        if pid_f.exists():
            raw_pid = pid_f.read_text(encoding="utf-8").strip()
            if raw_pid:
                return int(raw_pid)
        if proc.poll() is not None:
            break
        time.sleep(0.05)
    raise DriverError(f"detached-run: supervisor never wrote {pid_f} within 5s")


def _detached_worker_main(payload_path: Path) -> int:
    payload = json.loads(payload_path.read_text(encoding="utf-8"))
    state = payload_path.parent
    command = list(payload["argv"])
    cwd = Path(payload["cwd"])
    try:
        with (state / "run.log").open("w", encoding="utf-8", buffering=1) as log:
            if os.name != "nt":
                _atomic_write_text(state / "sid", str(os.getsid(0)))
                _atomic_write_text(state / "pid", str(os.getpid()))
            # The launcher owns pid/sid publication on Windows. Rewriting
            # those identity files from the worker races readers and can turn
            # a child exit into a false daemon-crash rc.
            os.chdir(cwd)
            rc = _exec_wait_rc(command, os.environ.copy(), log)
            _atomic_write_text(state / "rc", str(rc))
        return 0
    except Exception as exc:  # noqa: BLE001 - worker must record every death
        with (state / "run.log").open("a", encoding="utf-8", buffering=1) as log:
            log.write(f"detached-run: daemon crashed: {exc}\n")
        _atomic_write_text(state / "rc", "126")
        return 0


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


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--detached-worker":
        raise SystemExit(_detached_worker_main(Path(sys.argv[2])))
    raise SystemExit("molt_dev_detached.py is an internal module; use tools/molt_dev.py")
