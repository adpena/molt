"""Immutable committed controller source publication and shared-use custody.

This is the one source authority for maintenance and the dev driver. Environment
provisioning, retention policy and command routing belong to their callers.
"""

from __future__ import annotations

import hashlib
import io
import json
import os
import re
import shutil
import tarfile
import tempfile
from collections.abc import Generator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

from molt.process_environment import ProcessEnvironment
from molt.file_locks import (
    _FileLockHandle,
    _acquire_file_lock,
    _file_lock_owned_operation,
    _release_file_lock,
)
from tools.command_execution import CommandExecutor
from tools.driver_errors import DriverError
from tools.git_identity import immutable_git_environment, require_git_object_id

_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_DEV")

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
STAGING_PREFIX = ".staging-"


@dataclass(frozen=True)
class Controller:
    key: str
    root: Path

    @property
    def driver(self) -> Path:
        return self.root.joinpath(*DRIVER)


def controllers_root(common_dir: Path) -> Path:
    return common_dir / "molt" / "controllers"


def source_lock_path(root: Path) -> Path:
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
        handle, expected_lock_path=source_lock_path(controller.root), shared=True
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
    lock_path = source_lock_path(controller.root)
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
        source_lock_path(root),
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
    # Local plumbing uses the existing shared executor without a wall-clock deadline.
    proc = _COMMANDS.run(
        ["git", "-C", str(repo), *args],
        cwd=repo,
        env=_git_environment(),
        capture_output=True,
        check=False,
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
