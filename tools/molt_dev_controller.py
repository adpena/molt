#!/usr/bin/env python3
"""The pinned integration controller: main's driver runs every landing.

rustc builds stage 1 with the compiler that ``src/stage0`` pins, and Zuul
applies a change to a trusted config project only after it merges. A landing
follows the same rule. ``integrate`` fetches, then runs ``tools/molt_dev.py``
from main's tip, extracted once per tree set into Git's common directory, in
an environment that ``uv sync`` builds from main's own lock, as rustc's stage0
brings its own libraries. The controller runs as ``python -I``: no module, no
dependency version and no ``PYTHON*`` setting comes from the candidate. The
candidate still supplies its gate policy and its tests.
See docs/design/landing_queue.md, "A pinned controller".
"""

from __future__ import annotations

import os
import shutil
import sys
import time
import argparse
import atexit
from contextlib import ExitStack
from collections.abc import Sequence, Mapping
from pathlib import Path
from types import ModuleType

from molt import process_guard
from molt.custody_layout import Regenerable, storage_rule
from molt.process_spawn import ExecStateHandoff
from molt.file_locks import (
    _FileLockHandle,
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)
from tools import controller_source
from tools.driver_errors import EXIT_USAGE, DriverError  # noqa: E402
from tools.molt_dev_common import _COMMANDS, _say, _warn  # noqa: E402
from tools.venv_exec import venv_python

ENVIRONMENT_NAME = ".venv"
# Retention: the newest KEEP controllers stay; an older one retires only after
# IDLE_S without use and no live shared source owner.
_RETENTION = storage_rule("controllers").retention
assert isinstance(_RETENTION, Regenerable)
KEEP = _RETENTION.keep_recent
IDLE_S = _RETENTION.max_idle_s


_running_source_use: (
    tuple[int, controller_source.Controller, ExitStack, _FileLockHandle] | None
) = None


def _retire_idle(
    directory: Path, *, current: Path, now: float, apply: bool = True
) -> list[dict[str, object]]:
    """Keep current/recent versions; lock and recheck older idle generations."""
    entries = []
    rows = []
    for entry in directory.iterdir():
        if entry == current:
            rows.append(
                {"class": "controllers", "path": str(entry), "action": "current"}
            )
            continue
        try:
            admitted = controller_source.controller_for_source(entry, allow_staged=True)
        except DriverError:
            admitted = None
        if admitted is None:
            rows.append(
                {
                    "class": "controllers",
                    "path": str(entry),
                    "action": "unproven-preimage",
                }
            )
            continue
        marker = entry / controller_source.RECORD_NAME
        try:
            entries.append((marker.stat().st_mtime, entry, entry.name == admitted.key))
        except FileNotFoundError:
            continue
    entries.sort(reverse=True)
    recent = set(
        [entry for _, entry, published in entries if published][: max(0, KEEP - 1)]
    )
    for last_use, entry, _published in entries:
        row = {
            "class": "controllers",
            "path": str(entry),
            "last_use": last_use,
            "action": "recent",
        }
        rows.append(row)
        if entry in recent or now - last_use < IDLE_S:
            continue
        lock_path = controller_source.source_lock_path(entry)
        try:
            handle = _try_acquire_file_lock(lock_path, create=apply)
        except FileNotFoundError:
            handle = None
        if handle is None:
            row["action"] = (
                "state-in-use"
                if lock_path.exists()
                else "unavailable-lock"
                if apply
                else "retire"
            )
            continue
        try:
            with _file_lock_owned_operation(handle, expected_lock_path=lock_path):
                fresh = (entry / controller_source.RECORD_NAME).stat().st_mtime
                if now - fresh < IDLE_S:
                    row["action"] = "use-changed"
                else:
                    row["action"] = "retire"
                    if apply:
                        shutil.rmtree(entry)
        except OSError as exc:
            row.update(action="failed", error=str(exc))
            _warn(f"controller retention left {entry}: {exc}")
        finally:
            _release_file_lock(handle)
    return rows


def pinned_controller(
    repo: Path, common_dir: Path, commit: str
) -> controller_source.Controller:
    """The controller for main's commit, extracted once per tree set."""

    with controller_source.controller_source(repo, common_dir, commit) as controller:
        from molt.dx import RunContext

        _sync_environment(
            controller.root, environment=RunContext(repo).canonical_env(os.environ)
        )
        _retire_idle(controller.root.parent, current=controller.root, now=time.time())
        return controller


def _uv_executable() -> str:
    # `uv run` names itself in UV; a direct launch finds uv on PATH.
    uv = os.environ.get("UV") or shutil.which("uv")
    if not uv:
        raise DriverError("the pinned controller needs uv to build its environment")
    return uv


def _sync_environment(root: Path, *, environment: Mapping[str, str]) -> None:
    """Build or confirm main's locked environment; uv makes a current one a no-op."""

    env = dict(environment)
    env["UV_PROJECT_ENVIRONMENT"] = str(root / ENVIRONMENT_NAME)
    env.pop("VIRTUAL_ENV", None)
    proc = _COMMANDS.run(
        [
            _uv_executable(),
            "sync",
            "--frozen",
            "--no-dev",
            "--no-install-project",
            "--quiet",
            "--project",
            str(root),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
        # No wall-clock deadline: uv bounds its downloads by read inactivity
        # (UV_HTTP_TIMEOUT); installing from its cache is local work.
        encoding="utf-8",
    )
    if proc.returncode != 0:
        raise DriverError(
            f"uv sync of the pinned controller at {root} failed "
            f"(exit {proc.returncode}): {proc.stderr.strip()}"
        )


def verify_pinned_controller(
    key: str,
    *,
    driver_file: str,
    common_dir: Path,
    exec_handoff: ExecStateHandoff | None = None,
) -> None:
    """Refuse a --pinned-controller claim that this process does not satisfy."""

    physical_driver = Path(driver_file).resolve()
    admitted = controller_source.controller_for_source(physical_driver.parent.parent)
    if admitted is None:
        raise DriverError(
            f"pinned controller {key} has no record for this driver source: {driver_file}"
        )
    expected = admitted.root
    if (
        admitted.key != key
        or controller_source.controller_common_dir(expected) != common_dir.resolve()
        or physical_driver != admitted.driver
    ):
        raise DriverError(
            f"--pinned-controller {key} names {expected}, but this driver runs "
            f"from {driver_file}"
        )
    # The actual controller owns its source as well as its launching parent.
    # This closes the Windows redirector/startup handoff and survives the
    # parent's later exit. Repeated validation does not accumulate handles.
    global _running_source_use
    controller = controller_source.Controller(key=key, root=expected.resolve())
    if _running_source_use is not None and _running_source_use[0] == os.getpid():
        if _running_source_use[1] != controller:
            raise DriverError("this process already owns a different controller source")
        if exec_handoff is not None:
            exec_handoff.receive(_running_source_use[3])
        return
    source_use = ExitStack()
    atexit.register(source_use.close)
    source_handle = source_use.enter_context(
        controller_source.controller_source_use(controller)
    )
    _running_source_use = (os.getpid(), controller, source_use, source_handle)
    if exec_handoff is not None:
        exec_handoff.receive(source_handle)


def add_pinned_controller_arguments(parser: argparse.ArgumentParser) -> None:
    """One private startup contract shared by integrate and queue-run."""
    parser.add_argument("--pinned-controller", help=argparse.SUPPRESS)
    parser.add_argument("--controller-cwd", help=argparse.SUPPRESS)
    parser.add_argument(
        "--controller-state-handoff",
        type=ExecStateHandoff.from_json,
        help=argparse.SUPPRESS,
    )


def resume_controller_cwd(args: argparse.Namespace, *, driver_file: str) -> None:
    """Consume the pinned worker's original context before command selection.

    The venv redirector and waiting Windows launcher stay outside a checkout
    that this worker may retire. The worker resumes its complete original cwd,
    preserving every relative environment/tool selection without a second
    environment-path policy. Only its existing physical controller may do so.
    """
    raw = getattr(args, "controller_cwd", None)
    key = getattr(args, "pinned_controller", None)
    exec_handoff = getattr(args, "controller_state_handoff", None)
    if exec_handoff is not None and (key is None or os.name != "posix"):
        raise DriverError(
            "exec state handoff requires a pinned POSIX controller", code=EXIT_USAGE
        )
    if raw is None:
        if os.name == "nt" and key is not None:
            raise DriverError(
                "pinned Windows controller has no original cwd", code=EXIT_USAGE
            )
        if key is not None:
            driver = Path(driver_file).resolve()
            if len(driver.parents) < 5:
                raise DriverError(
                    "controller driver has no pinned cache identity", code=EXIT_USAGE
                )
            verify_pinned_controller(
                key,
                driver_file=driver_file,
                common_dir=controller_source.controller_common_dir(
                    driver.parent.parent
                ),
                exec_handoff=exec_handoff,
            )
            args.controller_state_handoff = None
        return
    if key is None:
        raise DriverError(
            "controller cwd requires a pinned controller", code=EXIT_USAGE
        )
    original = Path(raw)
    if not original.is_absolute() or not original.is_dir():
        raise DriverError(
            "controller cwd must name an existing absolute directory", code=EXIT_USAGE
        )
    if not Path(args.repo).is_absolute():
        raise DriverError(
            "controller cwd requires an absolute repository", code=EXIT_USAGE
        )
    # This layout is the existing cache producer's physical identity. Inspect
    # it before chdir without selecting Git through the still-unrestored PATH;
    # integration_controller later verifies the repository's actual common dir.
    driver = Path(driver_file).resolve()
    if len(driver.parents) < 5:
        raise DriverError(
            "controller driver has no pinned cache identity", code=EXIT_USAGE
        )
    common_dir = controller_source.controller_common_dir(driver.parent.parent)
    verify_pinned_controller(
        key,
        driver_file=driver_file,
        common_dir=common_dir,
        exec_handoff=exec_handoff,
    )
    args.controller_state_handoff = None
    try:
        os.chdir(original)
    except OSError as error:
        raise DriverError(
            f"cannot resume controller cwd {original}: {error}", code=EXIT_USAGE
        ) from error
    args.controller_cwd = None  # One consumption; queue-to-integrate never replays it.


def run_controller(
    controller: controller_source.Controller, argv: Sequence[str], *, repo: Path
) -> int:
    """Hand this landing to the controller; never returns on POSIX."""

    source_use = ExitStack()
    handle = source_use.enter_context(
        controller_source.controller_source_use(controller)
    )
    # A Windows guard can fail with unresolved child custody. Keep source until
    # its registered child cleanup, which runs before this earlier callback.
    atexit.register(source_use.close)
    try:
        result = _run_controller(controller, argv, repo=repo, source_handle=handle)
    except BaseException:
        if os.name == "posix":  # failed exec created no child
            atexit.unregister(source_use.close)
            source_use.close()
        raise
    atexit.unregister(source_use.close)
    source_use.close()
    return result


def _run_controller(
    controller: controller_source.Controller,
    argv: Sequence[str],
    *,
    repo: Path,
    source_handle: _FileLockHandle,
) -> int:
    repo = repo.resolve()
    interpreter = str(venv_python(controller.root / ENVIRONMENT_NAME))
    command = [
        interpreter,
        "-I",
        "-B",
        str(controller.driver),
        *argv,
        "--repo",
        str(repo),
        "--pinned-controller",
        controller.key,
    ]
    _say(f"==> pinned controller {controller.key} ({controller.root})")
    for stream in (sys.stdout, sys.stderr):
        stream.flush()
    if os.name == "posix":
        # One process: signals, the pid and the exit status stay the landing's own.
        from molt.process_spawn import forward_command

        return forward_command(
            command,
            env=os.environ,
            retained_shared_handles=(source_handle,),
            handoff_argument="--controller-state-handoff",
        )
    # Windows launchers retain their startup directory. Keep both this
    # waiting process and the selected venv redirector in the surviving cache;
    # only the actual pinned worker resumes the captured logical cwd.
    original = Path.cwd()
    # Bind the existing source and descendant owner before leaving the caller's
    # directory; MOLT_SOURCE_ROOT may itself be relative to that directory.
    guard = process_guard.load_harness_memory_guard(
        None, guard_source_root=_COMMANDS.repo_root
    )
    limits = guard.limits_from_env(_COMMANDS.prefix, os.environ)
    command.extend(["--controller-cwd", str(original)])
    os.chdir(controller.root)
    # Keep the existing guarded descendant lifecycle, but carry the original
    # environment directly. A RunContext here would rebase relative selections
    # before the worker can restore their cwd. The guard uses that same logical
    # directory for managed-state admission while spawning from the safe cache.
    result = guard.guarded_completed_process(
        command,
        prefix=_COMMANDS.prefix,
        cwd=controller.root,
        logical_cwd=original,
        env=os.environ,
        limits=limits,
        capture_output=False,
        text=False,
    )
    process_guard.raise_for_guard_outcome(command, result, timeout=None)
    return result.returncode


def _driver() -> ModuleType:
    """The canonical driver; imported late because it imports this module."""
    from tools import molt_dev

    return molt_dev


def integration_controller(args: argparse.Namespace) -> str:
    """Which driver runs this landing; a pinned claim must match this process."""
    drv = _driver()
    if args.pinned_controller is None:
        return "candidate"
    common_dir = drv._git_common_directory(Path(args.repo).resolve())
    if common_dir is None:
        raise DriverError("cannot resolve the Git common directory for integration")
    driver_file = drv.__file__
    if driver_file is None:
        raise DriverError("integration driver has no source file")
    verify_pinned_controller(
        args.pinned_controller, driver_file=driver_file, common_dir=common_dir
    )
    return f"pinned:{args.pinned_controller}"


def run_from_pinned_controller(args: argparse.Namespace, argv: list[str]) -> int:
    """Fetch, then hand the command to main's driver (docs/design/landing_queue.md)."""
    drv = _driver()
    repo = Path(args.repo).resolve()
    common_dir = drv._git_common_directory(repo)
    if common_dir is None:
        raise DriverError("cannot resolve the Git common directory for integration")
    git = drv.Git(repo)
    git.fetch(args.remote, args.branch)
    tip = drv.require_git_object_id(
        git.rev_parse(f"{args.remote}/{args.branch}"), label="controller commit"
    )
    controller = pinned_controller(repo, common_dir, tip)
    return run_controller(controller, argv, repo=repo)


def runs_from_pinned_controller(args: argparse.Namespace) -> bool:
    """A landing, a queued landing, the queue runner, a withdrawal, and a
    lane's start and retirement run main's driver.

    ``land`` and ``queue withdraw`` write queue state and start the runner, so
    a candidate's stale driver would decide both. ``lane start`` provisions a
    lane at main and ``lane retire`` judges whether a lane's ignored trees are
    current; a driver at another commit reads their manifests with another
    schema (HF-468: a lane's driver failed to provision a new lane, and an
    old main's driver judged a landed lane's tree stale).
    """
    pinned = args.command in {"integrate", "land"} and not getattr(
        args, "dry_run", False
    )
    pinned = pinned or (
        args.command == "queue" and args.queue_command in {"run", "withdraw"}
    )
    pinned = pinned or (
        args.command == "lane" and args.lane_command in {"start", "retire"}
    )
    return pinned and not (args.candidate_controller or args.pinned_controller)
