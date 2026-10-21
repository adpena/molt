#!/usr/bin/env python3
"""Detached daemon commands for the canonical Molt dev driver."""

from __future__ import annotations

import argparse
import atexit
from contextlib import ExitStack, contextmanager
import json
import os
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import TextIO
from collections.abc import Generator

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.molt_dev_common import (  # noqa: E402
    EXIT_FAIL,
    EXIT_OK,
    EXIT_USAGE,
    DriverError,
    _fail,
    _ok,
    _say,
)
from tools.molt_dev_probe import probe_path, probe_pid  # noqa: E402

from tools.command_execution import (  # noqa: E402
    CommandExecutor,
    DetachedStateHandoff,
    detached_command_use,
    detached_worker_pid,
)

_COMMANDS = CommandExecutor.for_file(__file__)

# State root for detached daemons. Per-name dirs hold: pid, sid, cmd.json,
# run.log, rc. The rc file is the ONLY proof of orderly completion: a dead
# pid with no rc is the hazard-11 died-silent class, and detached-verify
# reports it as such.


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
    root = (
        Path(override).resolve()
        if override
        else Path(tempfile.gettempdir()) / "molt_dev_detached"
    )
    return root / name


def verify_command(name: str, state_dir: str | None, min_age_s: float) -> list[str]:
    """The ``detached-verify`` argv that finds the state ``detached-run`` wrote.

    A run under ``--state-dir`` is invisible to a verify without it: the
    default root is the system temp directory.
    """
    state_root = ["--state-dir", str(Path(state_dir).resolve())] if state_dir else []
    return [
        "python3",
        "tools/molt_dev.py",
        "detached-verify",
        "--name",
        name,
        *state_root,
        "--min-age-s",
        str(min_age_s),
    ]


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
    from molt.checkout_state import checkout_state_command_roots, checkout_state_use

    state_use = ExitStack()
    state_use.enter_context(
        checkout_state_use(checkout_state_command_roots(_REPO_ROOT, env))
    )
    # Keep the generator-backed hold durable if waiting fails and this frame
    # unwinds. The existing child wait runs first at process shutdown.
    atexit.register(state_use.close)
    proc = None
    try:
        proc = _COMMANDS.start_owned(
            command,
            env=env,
            creationflags=(
                _windows_owned_child_creationflags() if os.name == "nt" else 0
            ),
            stdout=log_file if log_file is not None else None,
            stderr=subprocess.STDOUT if log_file is not None else None,
        )
        atexit.register(proc.wait)
        returncode = proc.wait()
    except Exception as exc:  # noqa: BLE001 - retain every unresolved child owner
        if proc is not None and proc.poll() is None:
            setattr(exc, "owned_process", proc)
            exc.add_note("detached child wait unresolved; use owner retained")
            raise
        _write_exec_message(f"detached-run: exec failed: {exc}\n", log_file)
        returncode = 127 if isinstance(exc, FileNotFoundError) else 126
    finally:
        if proc is None or proc.poll() is not None:
            if proc is not None:
                atexit.unregister(proc.wait)
            atexit.unregister(state_use.close)
            state_use.close()
    return returncode if returncode >= 0 else 128 + abs(returncode)


def python_command(*arguments: str) -> list[str]:
    """This interpreter, with this process's isolation, running ``arguments``.

    The pinned controller runs as ``python -I``. A Python helper it starts
    without ``-I`` reads the caller's ``PYTHONPATH``; inside an integrate gate
    that names the candidate's import roots, so the helper imported the
    candidate's modules beside the controller's and died at startup.
    """
    return [sys.executable, *(["-I"] if sys.flags.isolated else []), *arguments]


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
    token = os.urandom(16).hex()
    _atomic_write_text(
        payload_path,
        json.dumps({"argv": command, "cwd": str(cwd), "use_token": token}, indent=2),
    )
    worker = python_command(
        str(Path(__file__).resolve()), "--detached-worker", str(payload_path)
    )
    with DetachedStateHandoff(
        _REPO_ROOT, env, cwd, state / "state-use-ready.json", token
    ) as handoff:
        handoff.process = _COMMANDS.start_owned(
            worker,
            cwd=str(cwd),
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            close_fds=True,
            creationflags=_windows_hidden_creationflags() if os.name == "nt" else 0,
            start_new_session=os.name != "nt",
        )
        try:
            handoff.wait_ready()
        except RuntimeError as error:
            if os.name == "posix" and handoff.process.returncode is not None:
                # No redirector exists: the observed process was the actual
                # worker, and no acknowledgement transferred this parent's use.
                _atomic_write_text(state / "pid", str(handoff.process.pid))
                _atomic_write_text(state / "rc", str(handoff.process.returncode or 126))
            raise DriverError(f"detached: {error}", code=EXIT_FAIL) from error
        pid = handoff.worker_pid
        if pid is None:
            raise RuntimeError("detached handoff has no acknowledged worker identity")
        sid = f"windows-process-group:{pid}" if os.name == "nt" else str(pid)
        _atomic_write_text(state / "sid", sid)
        _atomic_write_text(state / "pid", str(pid))
        return pid


def _detached_worker_main(payload_path: Path) -> int:
    payload = json.loads(payload_path.read_text(encoding="utf-8"))
    state = payload_path.parent
    command = list(payload["argv"])
    cwd = Path(payload["cwd"])
    try:
        env = os.environ.copy()
        with detached_command_use(
            _REPO_ROOT,
            env,
            cwd=cwd,
            ready_path=state / "state-use-ready.json",
            use_token=payload["use_token"],
        ):
            with (state / "run.log").open("w", encoding="utf-8", buffering=1) as log:
                os.chdir(cwd)
                rc = _exec_wait_rc(command, env, log)
    except Exception as exc:  # noqa: BLE001 - worker must record every death
        with (state / "run.log").open("a", encoding="utf-8", buffering=1) as log:
            log.write(f"detached-run: daemon crashed: {exc}\n")
        if getattr(exc, "owned_process", None) is not None:
            # No rc may claim completion while a child wait is unresolved.
            return 0
        rc = 126
    # The terminal record permits replacement of this state directory. Every
    # owned cleanup and crash-log write must finish before that publication;
    # this worker must never write into a later run's replacement directory.
    _atomic_write_text(state / "rc", str(rc))
    return 0


@contextmanager
def _detached_launch_admission(state: Path, *, replace: bool) -> Generator[None]:
    """Serialize this existing name's check/replace/start; never erase unknown use."""
    from molt.file_locks import _try_acquire_file_lock, _release_file_lock

    # Outside the replaceable state tree: replacing a dead run must not replace
    # the OS lock which excludes a competing launcher for the same name.
    lock = _try_acquire_file_lock(state.with_name(f".{state.name}.launch.lock"))
    if lock is None:
        raise DriverError(f"detached-run: launch admission is busy for {state.name!r}")
    try:
        if state.exists():
            rc_file = state / "rc"
            try:
                # Existing terminal authority: the worker writes rc only after
                # its owned command returns. A PID alone is not this proof.
                int(rc_file.read_text(encoding="utf-8").strip())
            except FileNotFoundError:
                try:
                    payload = json.loads(
                        (state / "worker.json").read_text(encoding="utf-8")
                    )
                    token = payload["use_token"]
                    if not isinstance(token, str):
                        raise ValueError("detached use token is not text")
                    pid = detached_worker_pid(state / "state-use-ready.json", token)
                    if pid is None:
                        raise ValueError("no acknowledged worker")
                    observation = probe_pid(pid)
                    if observation["alive"]:
                        raise DriverError(
                            f"detached-run: {state.name!r} is already RUNNING (pid {pid}). "
                            "This driver NEVER kills - wait or use a new --name."
                        )
                    if observation["alive"] is None:
                        raise ValueError("worker death is not established")
                except DriverError:
                    raise
                except (
                    OSError,
                    ValueError,
                    KeyError,
                    TypeError,
                    RuntimeError,
                ) as error:
                    raise DriverError(
                        f"detached-run: state for {state.name!r} is INCOMPLETE or "
                        "unresolved; no terminal result or acknowledged dead worker "
                        "permits replacement. Preserve this state and use a new --name."
                    ) from error
            except (OSError, ValueError) as error:
                raise DriverError(
                    f"detached-run: terminal state for {state.name!r} is unreadable; "
                    "preserve it and use a new --name."
                ) from error
            if not replace:
                raise DriverError(
                    f"detached-run: state for {state.name!r} already exists at "
                    f"{state} (finished or died). Pass --replace to clear DEAD "
                    "state and respawn."
                )
            shutil.rmtree(state)
        state.mkdir(parents=True, exist_ok=False)
        yield
    finally:
        _release_file_lock(lock)


def cmd_detached_run(args: argparse.Namespace) -> int:
    command = list(args.argv or [])
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        raise DriverError("detached-run: give the command after `--`", code=EXIT_USAGE)
    state = _detached_state_dir(args.name, args.state_dir)
    with _detached_launch_admission(state, replace=args.replace):
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
            "      "
            + shlex.join(
                verify_command(args.name, args.state_dir, args.verify_min_age_hint)
            )
        )
        if args.json:
            print(
                json.dumps(
                    {"name": args.name, "pid": daemon_pid, "state_dir": str(state)}
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
    if len(sys.argv) == 3 and sys.argv[1] == "--detached-worker":
        raise SystemExit(_detached_worker_main(Path(sys.argv[2])))
    raise SystemExit(
        "molt_dev_detached.py is an internal module; use tools/molt_dev.py"
    )
