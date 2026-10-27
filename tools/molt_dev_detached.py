#!/usr/bin/env python3
"""Detached command arguments, verification and reports for the Molt dev driver."""

from __future__ import annotations

import argparse
import json
import os
import re
import shlex
import sys
import tempfile
import time
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import detached_execution  # noqa: E402
from tools.driver_errors import (  # noqa: E402
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    DriverError,
)
from tools.molt_dev_common import _fail, _ok, _say  # noqa: E402
from tools.molt_dev_probe import probe_path, probe_pid  # noqa: E402

_DETACHED_NAME_RE = re.compile(r"[A-Za-z0-9][A-Za-z0-9_.-]*")


def _detached_state_dir(name: str, override: str | None) -> Path:
    if not _DETACHED_NAME_RE.fullmatch(name):
        raise DriverError(
            f"detached: name {name!r} must match {_DETACHED_NAME_RE.pattern}",
            code=EXIT_USAGE,
        )
    root = (
        Path(override).resolve()
        if override
        else Path(tempfile.gettempdir()) / "molt_dev_detached"
    )
    return root / name


def verify_command(name: str, state_root: Path, min_age_s: float) -> list[str]:
    """Carry the admitted state root into the later verification process.

    Even a default root must be explicit: the later process can have a
    different temporary environment. The caller passes the selected state.parent.
    """
    return [
        # This interpreter and the driver by absolute path: a bare `python3`
        # may be one that cannot run the driver (macOS ships 3.9).
        *detached_execution.python_command(
            str(Path(__file__).resolve().with_name("molt_dev.py"))
        ),
        "detached-verify",
        "--name",
        name,
        "--state-dir",
        str(state_root),
        "--min-age-s",
        str(min_age_s),
    ]


def cmd_detached_run(args: argparse.Namespace) -> int:
    command = list(args.argv or [])
    if command and command[0] == "--":
        command = command[1:]
    state = _detached_state_dir(args.name, args.state_dir)
    env = dict(os.environ)
    for kv in args.env or []:
        key, sep, value = kv.partition("=")
        if not sep:
            raise DriverError(
                f"detached-run: --env needs K=V, got {kv!r}", code=EXIT_USAGE
            )
        env[key] = value
    daemon_pid = detached_execution.launch_detached(
        state,
        command,
        cwd=Path(args.cwd) if args.cwd else Path.cwd(),
        environment=env,
        replace=args.replace,
        environment_overrides=args.env or (),
    )
    verify_argv = verify_command(args.name, state.parent, args.verify_min_age_hint)
    _ok(f"detached {args.name!r} spawned: pid {daemon_pid}")
    _say(f"    state: {state}")
    _say(f"    log:   {state / 'run.log'}")
    _say("    REQUIRED next step, in a LATER tool call (teardown of THIS call")
    _say("    is exactly what hazard 11 is about):")
    if os.name == "nt":
        _say("    verify_argv (JSON argument array; launch without a shell):")
        _say("      " + json.dumps(verify_argv))
    else:
        _say("    POSIX shell command:")
        _say("      " + shlex.join(verify_argv))
    if args.json:
        print(
            json.dumps(
                {
                    "name": args.name,
                    "pid": daemon_pid,
                    "state_dir": str(state),
                    "verify_argv": verify_argv,
                }
            )
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
    observation = probe_pid(pid)
    if observation["alive"] is None:
        result["status"], result["detail"] = "unknown", observation["detail"]
        if args.json:
            print(json.dumps(result))
        _fail(f"detached {args.name!r}: UNKNOWN - {observation['detail']}")
        return EXIT_FAIL
    if observation["alive"]:
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
    raise SystemExit(
        "molt_dev_detached.py is an internal module; use tools/molt_dev.py"
    )
