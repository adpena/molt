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

import hashlib
import io
import json
import os
import shutil
import sys
import tarfile
import tempfile
import time
import argparse
import atexit
import re
from contextlib import ExitStack, contextmanager
from collections.abc import Generator, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType

from molt.process_environment import ProcessEnvironment
from molt import process_guard
from molt.file_locks import (
    _FileLockHandle,
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
    _try_acquire_file_lock,
)
from tools.git_identity import require_git_object_id
from tools import molt_dev_common
from tools.molt_dev_common import _COMMANDS, EXIT_USAGE, DriverError, _say, _warn
from tools.git_identity import immutable_git_environment
from tools.venv_exec import venv_python

SCHEMA = "molt.integration-controller.v1"
# The driver's repository modules, the data they read from the checkout they
# run from (``config``: tool release pins and policy tables, 1.9 MB), and the
# Python startup source, project, lock and interpreter version that define its
# environment. A lane start provisions through molt.dx, which reads config/tool_releases.toml
# from the controller (HF-468 review v2, B-1).
CONTROLLER_PATHS = (
    "tools",
    "src/molt",
    "src/sitecustomize.py",
    "config",
    "pyproject.toml",
    "uv.lock",
    ".python-version",
)
DRIVER = ("tools", "molt_dev.py")
RECORD_NAME = ".molt-controller.json"
ENVIRONMENT_NAME = ".venv"
STAGING_PREFIX = ".staging-"
# Retention: the newest KEEP controllers stay; an older one retires only after
# IDLE_S without use and no live shared source owner.
KEEP = 4
IDLE_S = 24 * 3600


@dataclass(frozen=True)
class Controller:
    key: str
    root: Path

    @property
    def driver(self) -> Path:
        return self.root.joinpath(*DRIVER)


_running_source_use: tuple[int, Controller, ExitStack] | None = None


def controllers_root(common_dir: Path) -> Path:
    return common_dir / "molt" / "controllers"


def _source_lock_path(root: Path) -> Path:
    # Outside the replaceable source tree and the controller inventory.
    key = root.name
    if key.startswith(STAGING_PREFIX):
        key = key[len(STAGING_PREFIX) :].split("-", 1)[0]
    return root.parent.parent / "controller-use" / f"{key}.lock"


def controller_for_source(root: Path) -> Controller | None:
    """Admit an existing physical controller; ordinary checkouts return None."""
    root = root.resolve()
    marker = root / RECORD_NAME
    if not marker.is_file():
        return None
    try:
        record = json.loads(marker.read_text(encoding="utf-8"))
        if (
            not isinstance(record, dict)
            or set(record) != {"schema", "key", "commit"}
            or record["schema"] != SCHEMA
            or record["key"] != root.name
            or re.fullmatch(r"[0-9a-f]{32}", root.name) is None
        ):
            raise ValueError("invalid controller source record")
        require_git_object_id(record["commit"], label="controller source commit")
    except (OSError, ValueError, TypeError) as error:
        raise DriverError(f"cannot admit controller source {root}: {error}") from error
    return Controller(key=root.name, root=root)


def _admit_source_use(controller: Controller, handle: _FileLockHandle) -> None:
    with _file_lock_owned_operation(
        handle, expected_lock_path=_source_lock_path(controller.root), shared=True
    ):
        if controller_for_source(controller.root) != controller:
            raise DriverError(
                f"controller source is absent or changed: {controller.root}"
            )
        os.utime(controller.root / RECORD_NAME, None)


@contextmanager
def controller_source_use(
    controller: Controller,
) -> Generator[_FileLockHandle, None, None]:
    """Hold one immutable controller through use, independently of its age."""
    lock_path = _source_lock_path(controller.root)
    handle = _acquire_file_lock(
        lock_path,
        shared=True,
        timeout_s=None,
        timeout_message=f"controller source stayed in retirement: {controller.root}",
    )
    try:
        _admit_source_use(controller, handle)
        yield handle
    finally:
        _release_file_lock(handle)


@contextmanager
def controller_source(
    repo: Path, common_dir: Path, commit: str
) -> Generator[Controller, None, None]:
    """Publish/reuse exact committed source, without provisioning or retention.

    The caller selects the admitted commit. This never fetches or interprets a
    dirty working tree as the published policy.
    """
    require_git_object_id(commit, label="controller commit")
    key = controller_key(repo, commit)
    root = controllers_root(common_dir).resolve() / key
    handle = _acquire_file_lock(
        _source_lock_path(root),
        shared=True,
        timeout_s=None,
        timeout_message=f"controller source stayed in retirement: {root}",
    )
    try:
        if not (root / RECORD_NAME).is_file():
            _extract(repo, commit, root, key)
        controller = Controller(key=key, root=root)
        _admit_source_use(controller, handle)
        yield controller
    finally:
        _release_file_lock(handle)


def _git_environment() -> ProcessEnvironment:
    env = immutable_git_environment(os.environ)
    env["GIT_TERMINAL_PROMPT"] = "0"
    return ProcessEnvironment(env)


def _git_bytes(repo: Path, *args: str) -> bytes:
    # Local plumbing: no wall-clock deadline, through the driver's one runner.
    proc = molt_dev_common._run_fast_captured_bytes(
        ["git", "-C", str(repo), *args], cwd=repo, env=_git_environment()
    )
    if proc.returncode != 0:
        detail = os.fsdecode(proc.stderr).strip()
        raise DriverError(
            f"git {' '.join(args)} failed (exit {proc.returncode}): {detail}"
        )
    return proc.stdout


def controller_key(repo: Path, commit: str) -> str:
    """Name a controller by the trees it runs, so an unrelated landing reuses it."""

    output = _git_bytes(
        repo, "rev-parse", *(f"{commit}:{path}" for path in CONTROLLER_PATHS)
    )
    ids = [
        require_git_object_id(line, label="controller tree")
        for line in os.fsdecode(output).splitlines()
    ]
    if len(ids) != len(CONTROLLER_PATHS):
        raise DriverError(f"controller trees of {commit} did not resolve: {ids}")
    digest = hashlib.sha256(SCHEMA.encode("ascii"))
    for path, object_id in zip(CONTROLLER_PATHS, ids, strict=True):
        digest.update(b"\0" + path.encode("ascii") + b"\0" + object_id.encode("ascii"))
    return digest.hexdigest()[:32]


def _extract(repo: Path, commit: str, root: Path, key: str) -> None:
    archive = _git_bytes(
        repo, "archive", "--format=tar", commit, "--", *CONTROLLER_PATHS
    )
    root.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=f"{STAGING_PREFIX}{key}-", dir=root.parent))
    try:
        with tarfile.open(fileobj=io.BytesIO(archive), mode="r:") as members:
            members.extractall(staging, filter="data")
        record = {"schema": SCHEMA, "key": key, "commit": commit}
        (staging / RECORD_NAME).write_text(json.dumps(record) + "\n", encoding="utf-8")
        try:
            os.rename(staging, root)
        except OSError:
            # A concurrent landing published the same trees first.
            if not (root / RECORD_NAME).is_file():
                raise DriverError(
                    f"incomplete controller at {root}; it was not written by this "
                    "driver. Move it aside, then retry."
                ) from None
    finally:
        if staging.exists():
            shutil.rmtree(staging)


def _retire_idle(directory: Path, *, current: Path, now: float) -> None:
    """Keep the newest KEEP controllers; retire older ones idle for IDLE_S."""

    entries = []
    for entry in directory.iterdir():
        if entry == current:
            continue
        marker = entry / RECORD_NAME
        try:
            last_use = (marker if marker.is_file() else entry).stat().st_mtime
        except FileNotFoundError:
            continue
        entries.append((last_use, entry))
    entries.sort(reverse=True)
    for last_use, entry in entries[KEEP - 1 :]:
        if now - last_use >= IDLE_S:
            handle = _try_acquire_file_lock(_source_lock_path(entry))
            if handle is None:
                continue
            try:
                with _file_lock_owned_operation(
                    handle, expected_lock_path=_source_lock_path(entry)
                ):
                    marker = entry / RECORD_NAME
                    fresh = (marker if marker.is_file() else entry).stat().st_mtime
                    if now - fresh >= IDLE_S:
                        shutil.rmtree(entry)
            except OSError as exc:
                # Retention never decides a landing.
                _warn(f"controller retention left {entry}: {exc}")
            finally:
                _release_file_lock(handle)


def pinned_controller(repo: Path, common_dir: Path, commit: str) -> Controller:
    """The controller for main's commit, extracted once per tree set."""

    with controller_source(repo, common_dir, commit) as controller:
        _sync_environment(controller.root)
        _retire_idle(controller.root.parent, current=controller.root, now=time.time())
        return controller


def _uv_executable() -> str:
    # `uv run` names itself in UV; a direct launch finds uv on PATH.
    uv = os.environ.get("UV") or shutil.which("uv")
    if not uv:
        raise DriverError("the pinned controller needs uv to build its environment")
    return uv


def _sync_environment(root: Path) -> None:
    """Build or confirm main's locked environment; uv makes a current one a no-op."""

    env = dict(os.environ)
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


def verify_pinned_controller(key: str, *, driver_file: str, common_dir: Path) -> None:
    """Refuse a --pinned-controller claim that this process does not satisfy."""

    expected = controllers_root(common_dir) / key
    if not (expected / RECORD_NAME).is_file():
        raise DriverError(f"pinned controller {key} has no record at {expected}")
    if Path(driver_file).resolve() != expected.joinpath(*DRIVER).resolve():
        raise DriverError(
            f"--pinned-controller {key} names {expected}, but this driver runs "
            f"from {driver_file}"
        )
    # The actual controller owns its source as well as its launching parent.
    # This closes the Windows redirector/startup handoff and survives the
    # parent's later exit. Repeated validation does not accumulate handles.
    global _running_source_use
    controller = Controller(key=key, root=expected.resolve())
    if _running_source_use is not None and _running_source_use[0] == os.getpid():
        if _running_source_use[1] != controller:
            raise DriverError("this process already owns a different controller source")
        return
    source_use = ExitStack()
    source_use.enter_context(controller_source_use(controller))
    atexit.register(source_use.close)
    _running_source_use = (os.getpid(), controller, source_use)


def add_pinned_controller_arguments(parser: argparse.ArgumentParser) -> None:
    """One private startup contract shared by integrate and queue-run."""
    parser.add_argument("--pinned-controller", help=argparse.SUPPRESS)
    parser.add_argument("--controller-cwd", help=argparse.SUPPRESS)


def resume_controller_cwd(args: argparse.Namespace, *, driver_file: str) -> None:
    """Consume the pinned worker's original context before command selection.

    The venv redirector and waiting Windows launcher stay outside a checkout
    that this worker may retire. The worker resumes its complete original cwd,
    preserving every relative environment/tool selection without a second
    environment-path policy. Only its existing physical controller may do so.
    """
    raw = getattr(args, "controller_cwd", None)
    key = getattr(args, "pinned_controller", None)
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
                key, driver_file=driver_file, common_dir=driver.parents[4]
            )
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
    common_dir = driver.parents[4]
    verify_pinned_controller(key, driver_file=driver_file, common_dir=common_dir)
    try:
        os.chdir(original)
    except OSError as error:
        raise DriverError(
            f"cannot resume controller cwd {original}: {error}", code=EXIT_USAGE
        ) from error
    args.controller_cwd = None  # One consumption; queue-to-integrate never replays it.


def run_controller(controller: Controller, argv: Sequence[str], *, repo: Path) -> int:
    """Hand this landing to the controller; never returns on POSIX."""

    source_use = ExitStack()
    handle = source_use.enter_context(controller_source_use(controller))
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
    controller: Controller,
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
            command, env=os.environ, retained_shared_handles=(source_handle,)
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
