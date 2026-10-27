"""Fixture repositories whose main carries this checkout's driver.

Every landing runs main's pinned driver (tools/molt_dev_controller.py), so a
fixture that lands through the real ``integrate`` needs that driver on its
main. The copy comes from the working tree, tracked and untracked alike, so a
test runs the code under test rather than the last commit.

The driver is host tooling. ``src/molt/stdlib`` is the compiled program's
library: the host never imports it (tests/conftest.py refuses it on
``sys.path``), so the fixture leaves it out; it was 950 of the 2,071 files
every fixture repository copied, committed, checked out and deleted.
``tools/browser_asset_graph`` is the Acorn parser's package set: the driver
never runs the generator that needs it, but a checkout that carries its lock
declares the ``browser-acorn`` input, and every fixture sandbox then paid an
``npm ci`` of its own.

The seed retains the real project dependencies and lock, but declares no DX
development or benchmark groups. Driver-contract tests still exercise the real
publisher; unrelated optional package workloads belong to their owning tests.
Private seed declarations are preserved when cloning an existing seed.

Fixture trees are copied many times per session, on whatever volume holds the
session's temporary files. ``clone_tree`` shares file blocks where the
filesystem can (APFS, btrfs, XFS) and copies elsewhere, and
``shared_session_tree`` builds an immutable seed once per session for every
xdist worker instead of once per worker.
``driver_history_seed`` also shares the committed driver history between
integration and claim tests. Prepared repository pairs retain private mutable
copies of source, Git objects, refs, indexes and config; only file blocks share.
"""

from __future__ import annotations

import ctypes
import errno
import os
import shutil
import sys
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

import pytest

from molt.file_locks import _acquire_file_lock, _release_file_lock
from tools.molt_dev_common import _run_fast_captured_command
from tools.molt_dev_controller import CONTROLLER_PATHS

REPO_ROOT = Path(__file__).resolve().parents[1]
# Under the controller paths but not part of the driver (module docstring).
EXCLUDED_DRIVER_TREES = ("src/molt/stdlib", "tools/browser_asset_graph")


@pytest.fixture
def isolated_checkout_artifacts(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    developer_host_context: None,
) -> Path:
    """Synthetic checkouts own state; tool caches and guard custody stay shared."""
    from molt.dx import ARTIFACT_ROOT_ENV, MOLT_ROOT_ENV_KEYS, RunContext

    # These explicit selectors can outlive the derived run context cleared by
    # developer_host_context and otherwise address another checkout's state.
    for key in ("MOLT_BUILD_STATE_DIR", "MOLT_BACKEND_DAEMON_SOCKET_DIR"):
        monkeypatch.delenv(key, raising=False)
    artifacts = tmp_path / "artifacts"
    artifacts.mkdir(exist_ok=True)
    monkeypatch.setenv(ARTIFACT_ROOT_ENV, str(artifacts))
    # Clearing inherited roots alone would let generic gates choose work/target
    # and create control locks in the synthetic source. Bind every derived root
    # through DX, without reselecting tool caches or process-guard custody.
    selected = RunContext(tmp_path).root_env()
    for key in MOLT_ROOT_ENV_KEYS:
        monkeypatch.setenv(key, selected[key])
    return artifacts


def driver_tree_files() -> list[str]:
    """The driver's files in this working tree, relative to the checkout."""
    listed = _run_fast_captured_command(
        [
            "git",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
            *CONTROLLER_PATHS,
            # The fixture is a checkout that provisions itself (uv sync
            # writes src/molt.egg-info): it keeps the repository's ignores.
            ".gitignore",
            *(f":(exclude){tree}" for tree in EXCLUDED_DRIVER_TREES),
        ],
        cwd=REPO_ROOT,
    )
    if listed.returncode != 0:
        raise RuntimeError(f"cannot list the driver trees: {listed.stderr}")
    return [relative for relative in listed.stdout.split("\0") if relative]


def copy_driver_trees(target: Path, *, seed: Path | None = None) -> None:
    """Copy the controller's trees from this working tree into ``target``.

    With ``seed`` (the ``driver_seed`` fixture) the files come from that
    session copy, which shares the target's volume and so clones instead of
    copying across volumes.
    """
    if seed is not None:
        shutil.copytree(
            seed, target, symlinks=True, copy_function=clone_file, dirs_exist_ok=True
        )
        return
    for relative in driver_tree_files():
        source = REPO_ROOT / relative
        if source.is_file():
            destination = target / relative
            destination.parent.mkdir(parents=True, exist_ok=True)
            clone_file(source, destination)
    from tools.pin_freshness import TableEdits, rewrite_toml_fields

    project = target / "pyproject.toml"
    edits: TableEdits = {"tool.molt.dx": {"dependency_groups": []}}
    project.write_text(
        rewrite_toml_fields(project.read_text(encoding="utf-8"), edits),
        encoding="utf-8",
    )


def _darwin_clonefile() -> Callable[[bytes, bytes, int], int] | None:
    if sys.platform != "darwin":
        return None
    libc = ctypes.CDLL(None, use_errno=True)
    clonefile = libc.clonefile
    clonefile.argtypes = [ctypes.c_char_p, ctypes.c_char_p, ctypes.c_int]
    clonefile.restype = ctypes.c_int
    return clonefile


_CLONEFILE = _darwin_clonefile()
_FICLONE = 0x40049409  # linux/fs.h: share the source's blocks (btrfs, XFS)


def clone_file(
    source: str | os.PathLike[str], destination: str | os.PathLike[str]
) -> None:
    """Copy one file, sharing its blocks where the filesystem can.

    A clone is an independent copy (copy-on-write) that costs no data I/O.
    It needs both paths on one filesystem that supports it; anything else
    (another volume, Windows, ext4) falls back to a plain copy.
    """
    if _CLONEFILE is not None:
        if _CLONEFILE(os.fsencode(source), os.fsencode(destination), 0) == 0:
            return
        if ctypes.get_errno() not in (errno.EXDEV, errno.ENOTSUP, errno.EEXIST):
            raise OSError(ctypes.get_errno(), os.strerror(ctypes.get_errno()), source)
    elif sys.platform == "linux":
        import fcntl

        try:
            with open(source, "rb") as src, open(destination, "wb") as dst:
                fcntl.ioctl(dst.fileno(), _FICLONE, src.fileno())
            shutil.copystat(source, destination)
            return
        except OSError as exc:
            if exc.errno not in (errno.EXDEV, errno.EOPNOTSUPP, errno.EINVAL):
                raise
    shutil.copy2(source, destination)


def clone_tree(source: Path, destination: Path) -> None:
    """Copy a directory tree the way ``clone_file`` copies one file."""
    shutil.copytree(source, destination, symlinks=True, copy_function=clone_file)


@dataclass(frozen=True, slots=True)
class GitFixture:
    """A private origin and its prepared main checkout; neither uses alternates."""

    origin: Path
    work: Path

    @property
    def remote_url(self) -> str:
        """A sibling-relative Git path that survives atomic seed publication."""
        return Path(os.path.relpath(self.origin, start=self.work)).as_posix()


def _fixture_git(repo: Path, *arguments: str) -> None:
    result = _run_fast_captured_command(["git", "-C", str(repo), *arguments])
    if result.returncode:
        raise RuntimeError(f"fixture git {arguments!r}: {result.stderr}")


def publish_git_fixture(fixture: GitFixture) -> None:
    """Publish committed history and prepare tracking once, before seed sharing."""
    _fixture_git(
        fixture.work, "clone", "--bare", "-q", str(fixture.work), str(fixture.origin)
    )
    _fixture_git(fixture.origin, "config", "--remove-section", "remote.origin")
    # A relative remote survives shared_session_tree's atomic staging rename.
    _fixture_git(fixture.work, "remote", "add", "origin", fixture.remote_url)
    _fixture_git(fixture.work, "fetch", "-q", "origin")
    _fixture_git(fixture.work, "branch", "-q", "--set-upstream-to=origin/main", "main")
    _fixture_git(
        fixture.work,
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    )


def copy_git_fixture(seed: GitFixture, destination: GitFixture) -> None:
    """Copy prepared history and files independently, then bind the private remote.

    Git's local clone can hardlink objects. Our CoW/copy authority instead keeps
    direct object writes, indexes, refs, config and source mutations private.
    Tracking refs already describe the immutable seed's published main; each
    subject's real fetch/rebase/push still runs in its test.
    """
    clone_tree(seed.origin, destination.origin)
    clone_tree(seed.work, destination.work)
    _fixture_git(
        destination.work, "remote", "set-url", "origin", destination.remote_url
    )


def build_driver_history(root: Path, driver: Path) -> None:
    """Commit the complete driver once for every real-controller test family."""
    fixture = GitFixture(root / "origin.git", root / "work")
    copy_driver_trees(fixture.work, seed=driver)
    _fixture_git(fixture.work, "init", "-q", "-b", "main")
    _fixture_git(fixture.work, "config", "user.email", "dev@molt.test")
    _fixture_git(fixture.work, "config", "user.name", "Molt Dev")
    _fixture_git(fixture.work, "config", "commit.gpgsign", "false")
    (fixture.work / "tools/proof_plan.toml").write_text(
        'always = ["echo integration-policy"]\n', encoding="utf-8"
    )
    (fixture.work / "README.md").write_text("seed\n", encoding="utf-8")
    _fixture_git(fixture.work, "add", "-A", "--", ".")
    _fixture_git(fixture.work, "commit", "-q", "--no-verify", "-m", "seed: main driver")
    publish_git_fixture(fixture)


def shared_session_tree(
    tmp_path_factory: pytest.TempPathFactory,
    worker_id: str,
    name: str,
    build: Callable[[Path], None],
) -> Path:
    """Build an immutable tree once per session, shared by every xdist worker.

    A session fixture runs once per xdist worker, so an expensive seed built
    in one costs the session N times. The workers share their basetemp's
    parent: the first to take the lock builds the tree there, the rest find
    it ready. Callers clone from the result and never write into it.
    """
    base = tmp_path_factory.getbasetemp()
    shared = base if worker_id == "master" else base.parent
    target = shared / name
    ready = shared / f"{name}.ready"
    if ready.is_file():
        return target
    # The builder holds the lock until the tree is ready; its exit releases
    # it. A wall-clock bound here failed waiters while a loaded host built.
    handle = _acquire_file_lock(
        shared / f"{name}.lock", timeout_s=None, timeout_message=""
    )
    try:
        if not ready.is_file():
            staging = shared / f"{name}.staging"
            if staging.exists():
                shutil.rmtree(staging)
            staging.mkdir()
            build(staging)
            staging.rename(target)
            ready.touch()
    finally:
        _release_file_lock(handle)
    return target
