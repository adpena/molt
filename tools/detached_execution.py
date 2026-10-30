#!/usr/bin/env python3
"""Canonical detached execution, startup acknowledgment and lifetime custody."""

from __future__ import annotations

import atexit
from contextlib import ExitStack, contextmanager
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path
from typing import TYPE_CHECKING, TextIO
from collections.abc import Generator, Mapping, Sequence

if TYPE_CHECKING:
    from tools.controller_source import Controller

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.driver_errors import EXIT_FAIL, EXIT_USAGE, DriverError  # noqa: E402
from tools.molt_dev_probe import probe_pid  # noqa: E402
from tools.command_execution import (  # noqa: E402
    CommandExecutor,
    DetachedStateHandoff,
    detached_command_use,
    detached_worker_pid,
)

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")

# Per-state pid/sid/command/log records are observations until the supervised
# command and its use close. The rc file is the sole orderly terminal record.


def _windows_hidden_creationflags() -> int:
    flags = 0
    for name in ("CREATE_NEW_PROCESS_GROUP", "CREATE_NO_WINDOW", "DETACHED_PROCESS"):
        flags |= getattr(subprocess, name, 0)
    return flags


def _windows_owned_child_creationflags() -> int:
    # The worker is already detached. The command it supervises must stay owned
    # by that worker so rc/log custody has exactly one process boundary.
    return getattr(subprocess, "CREATE_NO_WINDOW", 0)


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
        checkout_state_use(checkout_state_command_roots(_REPO_ROOT, env, command))
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
    """This interpreter's control-helper startup policy, running ``arguments``.

    The pinned controller runs as ``python -I``. A Python helper it starts
    without ``-I`` reads the caller's ``PYTHONPATH``; inside an integrate gate
    that names the candidate's import roots, so the helper imported the
    candidate's modules beside the controller's and died at startup.
    Isolation also ignores Python environment variables, so carry the selected
    bytecode read namespace and UTF-8 mode explicitly, and disable bytecode
    writes before startup imports. An empty prefix preserves a selected None.
    """
    return [
        sys.executable,
        *(["-I"] if sys.flags.isolated else []),
        "-B",
        "-X",
        f"pycache_prefix={sys.pycache_prefix or ''}",
        "-X",
        f"utf8={sys.flags.utf8_mode}",
        *arguments,
    ]


def _detached_daemonize(
    state: Path,
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    *,
    controller: Controller | None = None,
) -> int:
    """Spawn one isolated supervisor without forking the live Python process.

    The caller can be multi-threaded (the proof memory guard is), so POSIX
    ``fork()`` is not a safe control-plane primitive.  A fresh interpreter is
    the single cross-platform supervisor authority: it owns the session/process
    group, command log, child lifetime, and terminal rc record.
    """
    from tools.controller_source import controller_for_source, controller_source_use

    controller = controller or controller_for_source(_REPO_ROOT)
    source_use = ExitStack()
    if controller is not None:
        source_use.enter_context(controller_source_use(controller))
    # Failed handoff retains source until the existing process owner finishes
    # waiting at exit. Its later registration executes before this callback.
    atexit.register(source_use.close)
    try:
        pid = _start_detached_worker(state, command, cwd, env, controller=controller)
    except BaseException as error:
        if getattr(error, "owned_process", None) is None:
            atexit.unregister(source_use.close)
            source_use.close()
        raise
    atexit.unregister(source_use.close)
    source_use.close()
    return pid


def _start_detached_worker(
    state: Path,
    command: list[str],
    cwd: Path,
    env: dict[str, str],
    *,
    controller: Controller | None,
) -> int:
    payload_path = state / "worker.json"
    token = os.urandom(16).hex()
    payload: dict[str, object] = {"argv": command, "cwd": str(cwd), "use_token": token}
    if controller is not None:
        payload["controller"] = {"key": controller.key, "root": str(controller.root)}
    _atomic_write_text(
        payload_path,
        json.dumps(payload, indent=2),
    )
    source = _REPO_ROOT if controller is None else controller.root
    worker = python_command(
        str(source / "tools" / "detached_execution.py"),
        "--detached-worker" if controller is None else "--detached-controller-worker",
        str(payload_path),
    )
    if controller is not None and "-I" not in worker:
        worker.insert(1, "-I")
    if controller is not None:
        # The supervisor is stdlib-only. An editable image's .pth/sitecustomize
        # can import the caller's Molt before this immutable source binds, even
        # under -I. Skip site initialization rather than replacing those modules.
        worker.insert(1, "-S")
    with DetachedStateHandoff(
        _REPO_ROOT, env, cwd, worker, state / "state-use-ready.json", token
    ) as handoff:
        # Own diagnostics before Python imports or worker entry can fail. The
        # worker appends to this same log; a successful entry never erases its
        # startup output. Launch admission already owns the fresh state tree.
        with (state / "run.log").open("a", encoding="utf-8", buffering=1) as log:
            handoff.process = _COMMANDS.start_owned(
                worker,
                cwd=str(cwd),
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=log,
                stderr=subprocess.STDOUT,
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


def _detached_worker_main(
    payload_path: Path, *, controller_required: bool = False
) -> int:
    payload = json.loads(payload_path.read_text(encoding="utf-8"))
    state = payload_path.parent
    command = list(payload["argv"])
    cwd = Path(payload["cwd"])
    source_use = ExitStack()
    atexit.register(source_use.close)
    try:
        from tools.controller_source import (
            controller_for_source,
            controller_source_use,
        )

        controller = controller_for_source(_REPO_ROOT)
        descriptor = payload.get("controller")
        if controller_required or descriptor is not None:
            if controller is None or descriptor != {
                "key": controller.key,
                "root": str(controller.root),
            }:
                raise ValueError("detached controller source does not match its worker")
        if controller is not None:
            source_use.enter_context(controller_source_use(controller))
        env = os.environ.copy()
        with detached_command_use(
            _REPO_ROOT,
            env,
            cwd=cwd,
            ready_path=state / "state-use-ready.json",
            use_token=payload["use_token"],
        ):
            with (state / "run.log").open("a", encoding="utf-8", buffering=1) as log:
                os.chdir(cwd)
                rc = _exec_wait_rc(command, env, log)
    except Exception as exc:  # noqa: BLE001 - worker must record every death
        with (state / "run.log").open("a", encoding="utf-8", buffering=1) as log:
            log.write(f"detached-run: daemon crashed: {exc}\n")
        if getattr(exc, "owned_process", None) is not None:
            # No rc may claim completion while a child wait is unresolved.
            return 0
        rc = 126
    # The terminal record permits replacement of this state directory. Child
    # and managed-state cleanup and every crash-log write precede publication.
    # Source use survives the write, then releases without touching this state.
    _atomic_write_text(state / "rc", str(rc))
    atexit.unregister(source_use.close)
    source_use.close()
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


def launch_detached(
    state: Path,
    command: Sequence[str],
    *,
    cwd: Path,
    environment: Mapping[str, str],
    controller: Controller | None = None,
    replace: bool = False,
    environment_overrides: Sequence[str] = (),
) -> int:
    """Launch the existing supervisor with explicit environment and source custody."""
    if isinstance(command, (str, bytes)) or not command:
        raise DriverError("detached-run: give the command after `--`", code=EXIT_USAGE)
    state = state.absolute()
    cwd = cwd.resolve()
    from tools.controller_source import controller_for_source

    controller = controller or controller_for_source(_REPO_ROOT)
    source = _REPO_ROOT if controller is None else controller.root
    worker_entry = source / "tools" / "detached_execution.py"
    if not worker_entry.is_file():
        raise DriverError(
            f"detached-run: selected source has no detached execution worker: "
            f"{worker_entry}"
        )
    with _detached_launch_admission(state, replace=replace):
        if not cwd.is_dir():
            raise DriverError(
                f"detached-run: --cwd {cwd} is not a directory", code=EXIT_USAGE
            )
        env = dict(environment)
        # Unbuffered IO so a group-kill cannot eat block-buffered progress (the
        # empty-log signature that made hazard 11 undiagnosable).
        env["PYTHONUNBUFFERED"] = "1"
        _atomic_write_text(
            state / "cmd.json",
            json.dumps(
                {
                    "argv": list(command),
                    "cwd": str(cwd),
                    "start_unix": time.time(),
                    "env_overrides": list(environment_overrides),
                    "controller": (
                        None
                        if controller is None
                        else {"key": controller.key, "root": str(controller.root)}
                    ),
                },
                indent=2,
            ),
        )
        return _detached_daemonize(
            state, list(command), cwd, env, controller=controller
        )


if __name__ == "__main__":
    if len(sys.argv) == 3 and sys.argv[1] == "--detached-worker":
        raise SystemExit(_detached_worker_main(Path(sys.argv[2])))
    if len(sys.argv) == 3 and sys.argv[1] == "--detached-controller-worker":
        raise SystemExit(
            _detached_worker_main(Path(sys.argv[2]), controller_required=True)
        )
    raise SystemExit(
        "detached_execution.py is an internal worker; use tools/molt_dev.py"
    )
