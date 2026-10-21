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
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

from molt.process_environment import ProcessEnvironment
from tools.git_identity import require_git_object_id
from tools.molt_dev_common import _COMMANDS, DriverError, _say, _warn
from tools.release.git_source_snapshot import immutable_git_environment
from tools.venv_exec import venv_python

SCHEMA = "molt.integration-controller.v1"
# The driver's repository modules, and the project, lock and interpreter
# version that define its environment.
CONTROLLER_PATHS = ("tools", "src/molt", "pyproject.toml", "uv.lock", ".python-version")
DRIVER = ("tools", "molt_dev.py")
RECORD_NAME = ".molt-controller.json"
ENVIRONMENT_NAME = ".venv"
STAGING_PREFIX = ".staging-"
# Retention: the newest KEEP controllers stay; an older one retires only after
# IDLE_S without use, which outlasts any landing that could still run it.
KEEP = 4
IDLE_S = 24 * 3600
_ARCHIVE_TIMEOUT_S = 300.0
_SYNC_TIMEOUT_S = 900.0


@dataclass(frozen=True)
class Controller:
    key: str
    root: Path

    @property
    def driver(self) -> Path:
        return self.root.joinpath(*DRIVER)


def controllers_root(common_dir: Path) -> Path:
    return common_dir / "molt" / "controllers"


def _git_environment() -> ProcessEnvironment:
    env = immutable_git_environment(os.environ)
    env["GIT_TERMINAL_PROMPT"] = "0"
    return ProcessEnvironment(env)


def _git_bytes(repo: Path, *args: str) -> bytes:
    # Bounded plumbing, run directly as _run_fast_captured_command does.
    proc = _COMMANDS.run(
        ["git", "-C", str(repo), *args],
        cwd=repo,
        env=_git_environment(),
        capture_output=True,
        timeout=_ARCHIVE_TIMEOUT_S,
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
            try:
                shutil.rmtree(entry)
            except OSError as exc:
                # Retention never decides a landing.
                _warn(f"controller retention left {entry}: {exc}")


def pinned_controller(repo: Path, common_dir: Path, commit: str) -> Controller:
    """The controller for main's commit, extracted once per tree set."""

    require_git_object_id(commit, label="controller commit")
    key = controller_key(repo, commit)
    root = controllers_root(common_dir) / key
    if not (root / RECORD_NAME).is_file():
        _extract(repo, commit, root, key)
    _sync_environment(root)
    now = time.time()
    # The record's mtime is the controller's last use.
    os.utime(root / RECORD_NAME, (now, now))
    _retire_idle(root.parent, current=root, now=now)
    return Controller(key=key, root=root)


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
        timeout=_SYNC_TIMEOUT_S,
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


def run_controller(controller: Controller, argv: Sequence[str], *, repo: Path) -> int:
    """Hand this landing to the controller; never returns on POSIX."""

    interpreter = str(venv_python(controller.root / ENVIRONMENT_NAME))
    command = [
        interpreter,
        "-I",
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
        os.execv(interpreter, command)
    # Windows has no exec: the driver waits on the controller, as a launcher does.
    return _COMMANDS.run(command, cwd=Path.cwd(), env=os.environ).returncode
