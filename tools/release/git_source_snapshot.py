"""Exact immutable Git source snapshots for release and staged consumers."""

from __future__ import annotations

from molt.temporary_artifacts import OwnedTemporaryDirectory

from dataclasses import dataclass
from contextlib import contextmanager
import hashlib
import os
from pathlib import Path, PurePosixPath
import subprocess
import tempfile
from typing import Callable, Iterator, Mapping, Sequence, TypeVar

from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_remove_path,
    canonical_file_leaf,
    resolve_owned_path,
)
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.compiler_distribution import verify_source_inventory
from molt.toolchain_identity import capture_stable_regular_file
from tools.git_identity import require_git_object_id
from tools.command_execution import CommandExecutor


GIT_SOURCE_SNAPSHOT_SCHEMA = "molt.git-source-snapshot.v1"
_REGULAR_GIT_MODES = frozenset({0o100644, 0o100755})
_OBJECT_FORMAT_LENGTHS = {"sha1": 40, "sha256": 64}
_CAPTURE_TIMEOUT_SECONDS = 120
_BlobResult = TypeVar("_BlobResult")
_COMMANDS = CommandExecutor.for_file(__file__)


def immutable_git_environment(environment: Mapping[str, str]) -> dict[str, str]:
    """A recorded object ID always denotes its original immutable Git object."""
    return {
        **{
            key: value
            for key, value in environment.items()
            if key.upper() != "GIT_NO_REPLACE_OBJECTS"
        },
        "GIT_NO_REPLACE_OBJECTS": "1",
    }


@dataclass(frozen=True, slots=True)
class GitSourceFile:
    """One regular Git blob in a portable source closure."""

    relative: PurePosixPath
    mode: int
    blob_oid: str
    size: int
    sha256: str

    @property
    def archive_mode(self) -> int:
        return 0o755 if self.mode == 0o100755 else 0o644

    def as_record(self) -> dict[str, object]:
        return {
            "path": self.relative.as_posix(),
            "mode": self.mode,
            "blob_oid": self.blob_oid,
            "size": self.size,
            "sha256": self.sha256,
        }


@dataclass(frozen=True, slots=True)
class GitSourceSnapshot:
    """An exact, path-independent view of selected blobs from one commit."""

    source_sha: str
    tree_sha: str
    object_format: str
    pathspecs: tuple[str, ...]
    files: tuple[GitSourceFile, ...]

    @property
    def total_bytes(self) -> int:
        return sum(item.size for item in self.files)

    @property
    def mode_map(self) -> dict[str, int]:
        return {item.relative.as_posix(): item.archive_mode for item in self.files}

    def as_record(self) -> dict[str, object]:
        return {
            "schema": GIT_SOURCE_SNAPSHOT_SCHEMA,
            "object_format": self.object_format,
            "source_sha": self.source_sha,
            "tree_sha": self.tree_sha,
            "files": [item.as_record() for item in self.files],
        }

    def verify(self, root: Path) -> Path:
        """Verify that *root* is the exact regular-file projection of this snapshot."""

        return verify_source_inventory(
            root,
            tuple(item.as_record() for item in self.files),
        )


@dataclass(frozen=True, slots=True)
class GitIndexSourceSnapshot:
    """Git's semantic staged tree, without inventing a source commit."""

    tree_sha: str
    object_format: str
    index_path: Path
    files: tuple[GitSourceFile, ...]

    def verify(self, root: Path) -> Path:
        return verify_source_inventory(
            root,
            tuple(item.as_record() for item in self.files),
        )

    def verify_index(
        self, *, repo_root: Path, git: Path, environment: Mapping[str, str]
    ) -> None:
        if (
            _captured_index_tree(
                repo_root,
                git=git,
                environment=environment,
                expected_index=self.index_path,
            )
            != self.tree_sha
        ):
            raise ValueError("Git staged source changed during projection")


def _run_git_text(
    git: Path,
    repo_root: Path,
    environment: Mapping[str, str],
    *arguments: str,
) -> str:
    try:
        completed = _COMMANDS.run(
            [str(git), *arguments],
            cwd=repo_root,
            env=immutable_git_environment(environment),
            check=True,
            capture_output=True,
            timeout=_CAPTURE_TIMEOUT_SECONDS,
        )
    except (OSError, UnicodeError, subprocess.SubprocessError) as exc:
        raise ValueError(f"Git source snapshot query failed: {arguments!r}") from exc
    # Git terminates these queries with exactly one LF. Filesystem paths may
    # themselves end in whitespace or contain CR/LF; text mode and strip() would
    # silently select a different index or repository.
    output = completed.stdout
    if not output.endswith(b"\n"):
        raise ValueError("Git source snapshot query has invalid framing")
    return output[:-1].decode("utf-8", errors="strict")


def _map_git_blobs(
    git: Path,
    repo_root: Path,
    environment: Mapping[str, str],
    rows: Sequence[tuple[PurePosixPath, int, str, int]],
    consume: Callable[[PurePosixPath, int, str, bytes, str], _BlobResult],
) -> tuple[_BlobResult, ...]:
    # Spool the batch once: bounded subprocess lifetime, no pipe deadlock, and
    # no whole-source bytes retained in memory while constructing the inventory.
    request = "".join(f"{row[2]}\n" for row in rows).encode("ascii")
    with OwnedTemporaryDirectory(prefix="molt-git-blobs-") as temporary:
        spool = Path(temporary) / "blobs"
        _COMMANDS.run(
            [str(git), "cat-file", "--batch"],
            cwd=repo_root,
            env=immutable_git_environment(environment),
            input=request,
            capture_output=True,
            stdout_capture_path=spool,
            stderr_capture_path=Path(temporary) / "stderr",
            capture_tail_bytes=64 * 1024,
            check=True,
            timeout=_CAPTURE_TIMEOUT_SECONDS,
        )
        with spool.open("rb") as output:
            results: list[_BlobResult] = []
            for relative, mode, blob_oid, declared_size in rows:
                header = output.readline().rstrip(b"\n").split()
                if header != [
                    blob_oid.encode("ascii"),
                    b"blob",
                    str(declared_size).encode("ascii"),
                ]:
                    raise ValueError(
                        f"Git source snapshot blob header is invalid: {relative}"
                    )
                data = output.read(declared_size)
                if len(data) != declared_size or output.read(1) != b"\n":
                    raise ValueError(
                        f"Git source snapshot blob framing is invalid: {relative}"
                    )
                algorithm = {40: "sha1", 64: "sha256"}.get(len(blob_oid))
                if algorithm is None:
                    raise ValueError(
                        f"Git source snapshot blob ID is invalid: {relative}"
                    )
                object_hash = hashlib.new(algorithm)
                object_hash.update(f"blob {declared_size}\0".encode("ascii"))
                object_hash.update(data)
                if object_hash.hexdigest() != blob_oid:
                    raise ValueError(
                        f"Git source snapshot blob content does not match its object ID: {relative}"
                    )
                results.append(
                    consume(
                        relative, mode, blob_oid, data, hashlib.sha256(data).hexdigest()
                    )
                )
            if output.read(1):
                raise ValueError("Git source snapshot has trailing blob data")
            return tuple(results)


def _hash_git_blobs(
    git: Path,
    repo_root: Path,
    environment: Mapping[str, str],
    rows: Sequence[tuple[PurePosixPath, int, str, int]],
) -> tuple[GitSourceFile, ...]:
    return _map_git_blobs(
        git,
        repo_root,
        environment,
        rows,
        lambda relative, mode, blob_oid, data, digest: GitSourceFile(
            relative=relative,
            mode=mode,
            blob_oid=blob_oid,
            size=len(data),
            sha256=digest,
        ),
    )


def read_git_source_file(
    snapshot: GitSourceSnapshot | GitIndexSourceSnapshot,
    relative: str,
    *,
    repo_root: Path,
    git: Path,
    environment: Mapping[str, str],
    max_bytes: int,
) -> bytes:
    """Read one bounded original blob and rebind it to the captured inventory."""
    entry = next(
        (item for item in snapshot.files if item.relative.as_posix() == relative), None
    )
    if entry is None or entry.size > max_bytes:
        raise ValueError(
            f"Git source snapshot file is absent or exceeds limit: {relative}"
        )

    def admit(
        path: PurePosixPath, mode: int, oid: str, data: bytes, digest: str
    ) -> bytes:
        if (path, mode, oid, len(data), digest) != (
            entry.relative,
            entry.mode,
            entry.blob_oid,
            entry.size,
            entry.sha256,
        ):
            raise ValueError(f"Git source snapshot file changed: {relative}")
        return data

    return _map_git_blobs(
        git,
        repo_root,
        environment,
        ((entry.relative, entry.mode, entry.blob_oid, entry.size),),
        admit,
    )[0]


def capture_git_source_snapshot(
    repo_root: Path,
    source_sha: str,
    *,
    git: Path,
    environment: Mapping[str, str],
    pathspecs: Sequence[str] = (),
    required_markers: frozenset[str] = frozenset(),
    max_files: int,
    max_bytes: int,
) -> GitSourceSnapshot:
    """Capture selected blob identities from exactly one full commit ID."""

    source_sha = require_git_object_id(source_sha, label="Git source snapshot commit")
    resolved_repo = repo_root.resolve(strict=True)
    if not resolved_repo.is_dir():
        raise ValueError(f"Git source snapshot repository is invalid: {repo_root}")
    object_format = _run_git_text(
        git,
        resolved_repo,
        environment,
        "rev-parse",
        "--show-object-format",
    )
    expected_length = _OBJECT_FORMAT_LENGTHS.get(object_format)
    if expected_length is None or len(source_sha) != expected_length:
        raise ValueError("Git source snapshot commit does not match object format")
    resolved_commit = _run_git_text(
        git,
        resolved_repo,
        environment,
        "rev-parse",
        "--verify",
        f"{source_sha}^{{commit}}",
    )
    if resolved_commit != source_sha:
        raise ValueError("Git source snapshot commit identity is not exact")
    tree_sha = require_git_object_id(
        _run_git_text(
            git,
            resolved_repo,
            environment,
            "rev-parse",
            f"{source_sha}^{{tree}}",
        ),
        label="Git source snapshot tree",
    )
    normalized_pathspecs = tuple(
        portable_relative_path(path).as_posix() for path in pathspecs
    )
    files = _capture_git_tree_files(
        resolved_repo,
        tree_sha,
        object_format=object_format,
        git=git,
        environment=environment,
        pathspecs=normalized_pathspecs,
        required_markers=required_markers,
        max_files=max_files,
        max_bytes=max_bytes,
    )
    return GitSourceSnapshot(
        source_sha=source_sha,
        tree_sha=tree_sha,
        object_format=object_format,
        pathspecs=normalized_pathspecs,
        files=files,
    )


def _capture_git_tree_files(
    repo_root: Path,
    tree_sha: str,
    *,
    object_format: str,
    git: Path,
    environment: Mapping[str, str],
    pathspecs: tuple[str, ...],
    required_markers: frozenset[str],
    max_files: int,
    max_bytes: int,
) -> tuple[GitSourceFile, ...]:
    expected_length = _OBJECT_FORMAT_LENGTHS[object_format]
    command = [
        str(git),
        "ls-tree",
        "-r",
        "-z",
        "-l",
        "--full-tree",
        tree_sha,
    ]
    if pathspecs:
        command.extend(("--", *pathspecs))
    try:
        listing = _COMMANDS.run(
            command,
            cwd=repo_root,
            env=immutable_git_environment(environment),
            check=True,
            capture_output=True,
            timeout=_CAPTURE_TIMEOUT_SECONDS,
        ).stdout
    except (OSError, subprocess.SubprocessError) as exc:
        raise ValueError("Git source snapshot tree query failed") from exc
    rows: list[tuple[PurePosixPath, int, str, int]] = []
    identities: set[str] = set()
    total_bytes = 0
    for raw_record in listing.split(b"\0"):
        if not raw_record:
            continue
        try:
            raw_metadata, raw_path = raw_record.split(b"\t", 1)
            raw_mode, raw_kind, raw_oid, raw_size = raw_metadata.split()
            relative_text = raw_path.decode("utf-8")
            relative = portable_relative_path(relative_text)
            mode = int(raw_mode, 8)
            kind = raw_kind.decode("ascii")
            blob_oid = require_git_object_id(
                raw_oid.decode("ascii"),
                label=f"Git source snapshot blob {relative_text}",
            )
            size = int(raw_size)
        except (UnicodeError, ValueError) as exc:
            raise ValueError("Git source snapshot tree listing is malformed") from exc
        if kind != "blob" or mode not in _REGULAR_GIT_MODES:
            raise ValueError(
                "Git source snapshot must contain only regular files: "
                f"{relative_text} mode={mode:o} type={kind}"
            )
        if len(blob_oid) != expected_length:
            raise ValueError(
                f"Git source snapshot blob has the wrong object format: {relative_text}"
            )
        identity = portable_path_identity(relative_text)
        if identity in identities:
            raise ValueError(
                f"Git source snapshot path identity collides: {relative_text}"
            )
        identities.add(identity)
        total_bytes += size
        if len(rows) >= max_files or total_bytes > max_bytes:
            raise ValueError("Git source snapshot exceeds its bounded closure policy")
        rows.append((relative, mode, blob_oid, size))
    if not rows:
        raise ValueError("Git source snapshot is empty")
    rows.sort(key=lambda item: item[0].as_posix())
    missing = required_markers.difference(item[0].as_posix() for item in rows)
    if missing:
        raise ValueError(
            "Git source snapshot is missing required markers: "
            + ", ".join(sorted(missing))
        )
    return _hash_git_blobs(git, repo_root, environment, rows)


def _selected_index_path(
    repo_root: Path, *, git: Path, environment: Mapping[str, str]
) -> Path:
    resolved_repo = repo_root.resolve(strict=True)
    actual_root = Path(
        _run_git_text(git, resolved_repo, environment, "rev-parse", "--show-toplevel")
    )
    if actual_root.resolve(strict=True) != resolved_repo:
        raise ValueError("Git staged source requires the repository root")
    index_path = Path(
        _run_git_text(
            git, resolved_repo, environment, "rev-parse", "--git-path", "index"
        )
    )
    if not index_path.is_absolute():
        index_path = resolved_repo / index_path
    return canonical_file_leaf(index_path, create_parent=False, role="Git staged index")


@contextmanager
def fenced_git_index(
    snapshot: GitIndexSourceSnapshot,
    *,
    repo_root: Path,
    git: Path,
    environment: Mapping[str, str],
) -> Iterator[None]:
    """Hold Git's selected index lock during validated projection publication.

    Cooperative Git writers cannot change the staged generation during this
    scope. The caller's index and any pre-existing lock remain untouched.
    """
    index = _selected_index_path(repo_root, git=git, environment=environment)
    if index != snapshot.index_path:
        raise ValueError("Git staged index selection changed during projection")
    lock = index.with_name(index.name + ".lock")
    # Windows cannot unlink a file through a name while a handle without
    # delete sharing is open, so there the lock is opened delete-on-close:
    # closing the descriptor removes it, even if this process dies.
    delete_on_close = getattr(os, "O_TEMPORARY", 0)
    descriptor = os.open(
        lock,
        os.O_WRONLY
        | os.O_CREAT
        | os.O_EXCL
        | getattr(os, "O_NOFOLLOW", 0)
        | delete_on_close,
        0o600,
    )
    held = os.fstat(descriptor)
    primary: BaseException | None = None
    try:
        snapshot.verify_index(repo_root=repo_root, git=git, environment=environment)
        yield
    except BaseException as exc:
        primary = exc
        raise
    finally:
        try:
            named = lock.lstat()
            if (named.st_dev, named.st_ino) != (held.st_dev, held.st_ino):
                raise ValueError(f"Git staged index lock changed ownership: {lock}")
            if not delete_on_close:
                lock.unlink()
        except BaseException as cleanup:
            if primary is None:
                raise
            BaseException.add_note(primary, f"Git index lock cleanup failed: {cleanup}")
        finally:
            # Keep the opened inode alive through the ownership check/removal;
            # closing first permits inode reuse before the named-leaf fence.
            os.close(descriptor)


def _captured_index_tree(
    repo_root: Path,
    *,
    git: Path,
    environment: Mapping[str, str],
    expected_index: Path | None = None,
) -> str:
    """Let Git interpret a private exact copy of its selected index.

    Git owns intent-to-add, split/sparse index and conflict semantics. Writing
    a tree from the copied index can create immutable objects and update the
    private cache-tree extension. Private plumbing admits no repository hooks
    or filesystem-monitor commands and does not change the caller's index/refs.
    """
    resolved_repo = repo_root.resolve(strict=True)
    index_path = _selected_index_path(resolved_repo, git=git, environment=environment)
    if expected_index is not None and index_path != expected_index:
        raise ValueError("Git staged index selection changed during projection")
    _identity, index_bytes = capture_stable_regular_file(
        index_path,
        label="Git staged index",
        max_bytes=64 * 1024 * 1024,
    )
    with OwnedTemporaryDirectory(prefix="molt-index-") as temporary:
        copied_index = Path(temporary) / "index"
        with copied_index.open("xb") as output:
            output.write(index_bytes)
        empty_hooks = Path(temporary) / "hooks"
        empty_hooks.mkdir()
        selected_environment = {
            **{
                key: value
                for key, value in environment.items()
                if key.upper() != "GIT_INDEX_FILE"
            },
            "GIT_INDEX_FILE": str(copied_index),
        }
        return require_git_object_id(
            _run_git_text(
                git,
                resolved_repo,
                selected_environment,
                "-c",
                f"core.hooksPath={empty_hooks}",
                "-c",
                "core.fsmonitor=",
                "write-tree",
            ),
            label="Git staged source tree",
        )


def capture_git_index_source_snapshot(
    repo_root: Path,
    *,
    git: Path,
    environment: Mapping[str, str],
    required_markers: frozenset[str] = frozenset(),
    max_files: int,
    max_bytes: int,
) -> GitIndexSourceSnapshot:
    index_path = _selected_index_path(repo_root, git=git, environment=environment)
    tree_sha = _captured_index_tree(
        repo_root, git=git, environment=environment, expected_index=index_path
    )
    object_format = _run_git_text(
        git, repo_root, environment, "rev-parse", "--show-object-format"
    )
    expected_length = _OBJECT_FORMAT_LENGTHS.get(object_format)
    if expected_length is None or len(tree_sha) != expected_length:
        raise ValueError("Git staged source tree does not match object format")
    files = _capture_git_tree_files(
        repo_root.resolve(strict=True),
        tree_sha,
        object_format=object_format,
        git=git,
        environment=environment,
        pathspecs=(),
        required_markers=required_markers,
        max_files=max_files,
        max_bytes=max_bytes,
    )
    return GitIndexSourceSnapshot(
        tree_sha=tree_sha,
        object_format=object_format,
        index_path=index_path,
        files=files,
    )


def materialize_git_source_snapshot(
    snapshot: GitSourceSnapshot | GitIndexSourceSnapshot,
    destination: Path,
    *,
    repo_root: Path,
    git: Path,
    environment: Mapping[str, str],
) -> Path:
    """Publish exact blobs without a second archive/extraction implementation."""
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination = resolve_owned_path(destination)
    if destination.exists():
        raise ValueError("Git source snapshot destination already exists")
    staging = Path(tempfile.mkdtemp(prefix=".source-", dir=destination.parent))
    owned = staging.stat()
    try:
        expected = {item.relative: item for item in snapshot.files}
        rows = tuple(
            (item.relative, item.mode, item.blob_oid, item.size)
            for item in snapshot.files
        )

        def write_blob(
            relative: PurePosixPath, mode: int, oid: str, data: bytes, digest: str
        ) -> None:
            item = expected[relative]
            if (mode, oid, len(data), digest) != (
                item.mode,
                item.blob_oid,
                item.size,
                item.sha256,
            ):
                raise ValueError(f"Git source snapshot blob changed: {relative}")
            path = staging.joinpath(*relative.parts)
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as handle:
                handle.write(data)
            path.chmod(item.archive_mode)

        _map_git_blobs(git, repo_root, environment, rows, write_blob)
        snapshot.verify(staging)
        durable_publish_directory_exclusive(staging, destination)
        return destination
    finally:
        if staging.exists():
            current = staging.lstat()
            if (current.st_dev, current.st_ino) != (owned.st_dev, owned.st_ino):
                raise ValueError(
                    f"source staging identity changed; preserved {staging}"
                )
            durable_remove_path(staging, retirement_scope="release-source")
