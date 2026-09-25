"""Commit-authoritative Git source snapshots for release build consumers."""

from __future__ import annotations

from dataclasses import dataclass
import hashlib
from pathlib import Path, PurePosixPath
import subprocess
import tempfile
from typing import Callable, Mapping, Sequence, TypeVar

from molt.file_publication import (
    durable_publish_directory_exclusive,
    durable_remove_path,
    resolve_owned_path,
)
from molt.portable_paths import portable_path_identity, portable_relative_path
from molt.compiler_distribution import verify_source_inventory
from tools.git_identity import require_git_object_id


GIT_SOURCE_SNAPSHOT_SCHEMA = "molt.git-source-snapshot.v1"
_REGULAR_GIT_MODES = frozenset({0o100644, 0o100755})
_OBJECT_FORMAT_LENGTHS = {"sha1": 40, "sha256": 64}
_CAPTURE_TIMEOUT_SECONDS = 120
_BlobResult = TypeVar("_BlobResult")


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

    def verify(
        self,
        root: Path,
        *,
        verify_modes: bool | None = None,
    ) -> Path:
        """Verify that *root* is the exact regular-file projection of this snapshot."""

        return verify_source_inventory(
            root,
            tuple(item.as_record() for item in self.files),
            verify_modes=verify_modes,
        )


def _run_git_text(
    git: Path,
    repo_root: Path,
    environment: Mapping[str, str],
    *arguments: str,
) -> str:
    try:
        completed = subprocess.run(
            [str(git), *arguments],
            cwd=repo_root,
            env=dict(environment),
            check=True,
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="strict",
            timeout=_CAPTURE_TIMEOUT_SECONDS,
        )
    except (OSError, UnicodeError, subprocess.SubprocessError) as exc:
        raise ValueError(f"Git source snapshot query failed: {arguments!r}") from exc
    return completed.stdout.strip()


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
    with tempfile.TemporaryFile() as output:
        subprocess.run(
            [str(git), "cat-file", "--batch"],
            cwd=repo_root,
            env=dict(environment),
            input=request,
            stdout=output,
            stderr=subprocess.PIPE,
            check=True,
            timeout=_CAPTURE_TIMEOUT_SECONDS,
        )
        output.seek(0)
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
    command = [
        str(git),
        "ls-tree",
        "-r",
        "-z",
        "-l",
        "--full-tree",
        source_sha,
    ]
    normalized_pathspecs = tuple(
        portable_relative_path(path).as_posix() for path in pathspecs
    )
    if normalized_pathspecs:
        command.extend(("--", *normalized_pathspecs))
    try:
        listing = subprocess.run(
            command,
            cwd=resolved_repo,
            env=dict(environment),
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
    files = _hash_git_blobs(git, resolved_repo, environment, rows)
    return GitSourceSnapshot(
        source_sha=source_sha,
        tree_sha=tree_sha,
        object_format=object_format,
        pathspecs=normalized_pathspecs,
        files=files,
    )


def materialize_git_source_snapshot(
    snapshot: GitSourceSnapshot,
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
