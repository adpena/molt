from __future__ import annotations

import contextlib
from contextlib import contextmanager
import errno
import json
import os
import shutil
import stat
from pathlib import Path
from typing import Any, Generator, Mapping
import zipfile

from molt import file_publication
from molt.toolchain_identity import (
    StableRegularFileError,
    StableRegularFileIdentity,
    iter_stable_regular_file_chunks,
    open_stable_regular_file,
)


def _atomic_write_text(
    path: Path,
    text: str,
    *,
    durability: file_publication.PublicationDurability = file_publication.PublicationDurability.DURABLE,
    skip_if_unchanged: bool = False,
) -> None:
    data = text.encode("utf-8")
    if skip_if_unchanged:
        file_publication._require_durability(durability)
        destination = file_publication.canonical_file_leaf(path, create_parent=True)
        # Equality is a complete stable direct-file observation, not a decoded
        # text comparison. An unreadable or changing leaf still needs publication.
        try:
            matches = False
            with open_stable_regular_file(
                destination, label="unchanged publication destination"
            ) as opened:
                if opened.stat.st_size == len(data):
                    desired = memoryview(data)
                    offset = 0
                    for chunk in iter_stable_regular_file_chunks(opened):
                        if chunk != desired[offset : offset + len(chunk)]:
                            break
                        offset += len(chunk)
                        del chunk
                    else:
                        matches = True
            # The context's closing pathname/handle/change-time fences must run
            # before equality can suppress a publication.
            if matches:
                return
        except (OSError, StableRegularFileError):
            pass
    file_publication.atomic_write_bytes(path, data, durability=durability)


def _write_text_if_changed(path: Path, content: str) -> None:
    _atomic_write_text(path, content, skip_if_unchanged=True)


def _remove_file_or_tree(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    file_publication.atomic_write_bytes(path, data)


def _atomic_write_json(
    path: Path,
    payload: Any,
    *,
    indent: int | None = 2,
    sort_keys: bool = False,
    default: Any | None = None,
    durability: file_publication.PublicationDurability = file_publication.PublicationDurability.DURABLE,
    skip_if_unchanged: bool = False,
) -> None:
    _atomic_write_text(
        path,
        json.dumps(
            payload,
            indent=indent,
            sort_keys=sort_keys,
            default=default,
            allow_nan=False,
        )
        + "\n",
        durability=durability,
        skip_if_unchanged=skip_if_unchanged,
    )


def _write_json_sidecar(path: Path, payload: Mapping[str, Any]) -> None:
    _atomic_write_json(path, payload, indent=2, sort_keys=True)


def _codesign_atomic_copy_temp(path: Path, destination: Path) -> None:
    """Sign a private stage as the binary it will be published as."""
    from molt.cli.native_toolchain import _codesign_binary

    _codesign_binary(path, identifier=destination.name)


def _atomic_copy_file(
    src: Path,
    dst: Path,
    *,
    expected_sha256: str | None = None,
    observed: StableRegularFileIdentity | None = None,
    durability: file_publication.PublicationDurability = file_publication.PublicationDurability.DURABLE,
) -> None:
    file_publication._require_durability(durability)
    with _staged_copy_file(
        src,
        dst,
        expected_sha256=expected_sha256,
        observed=observed,
    ) as tmp_path:
        if durability is file_publication.PublicationDurability.DURABLE:
            file_publication.durable_replace(tmp_path, dst)
        else:
            file_publication.atomic_replace(tmp_path, dst, durability=durability)


@contextmanager
def _staged_copy_file(
    src: Path,
    dst: Path,
    *,
    codesign: bool = False,
    expected_sha256: str | None = None,
    observed: StableRegularFileIdentity | None = None,
) -> Generator[Path]:
    """Own the final byte/mode copy until its caller publishes or abandons it."""
    from molt import artifact_publication

    with artifact_publication.staged_copy_file(
        src,
        dst,
        prepare=(lambda stage: _codesign_atomic_copy_temp(stage, dst))
        if codesign
        else None,
        expected_sha256=expected_sha256,
        observed=observed,
    ) as candidate:
        yield candidate


# os.link is only an optimization over copying. When it fails because hard
# links are unavailable for this (src, dst) pair we must fall back to a byte
# copy: cross-device (EXDEV), permission (EPERM/EACCES), or a filesystem without
# hard-link support. exFAT/FAT volumes can reject
# os.link with ERROR_INVALID_FUNCTION (winerror 1 -> errno EINVAL 22), which is
# NONE of the classic POSIX link errnos, so it must be recognized explicitly or
# every freshly staged artifact is dropped on that volume.
_LINK_COPY_FALLBACK_ERRNOS = frozenset(
    {errno.EXDEV, errno.EPERM, errno.EACCES, errno.ENOTSUP, errno.EINVAL, errno.ENOENT}
)
# ERROR_INVALID_FUNCTION / ERROR_NOT_SUPPORTED / ERROR_INVALID_PARAMETER.
_LINK_COPY_FALLBACK_WINERRORS = frozenset({1, 50, 87})


def _link_failure_wants_copy(exc: OSError) -> bool:
    """True when an ``os.link`` OSError means "no hard links here — copy instead"."""
    if exc.errno in _LINK_COPY_FALLBACK_ERRNOS:
        return True
    return getattr(exc, "winerror", None) in _LINK_COPY_FALLBACK_WINERRORS


def _atomic_link_or_copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = file_publication.staged_file_path(dst, purpose="link")
    try:
        source_mode = src.stat().st_mode
        if source_mode & stat.S_IWRITE:
            try:
                os.link(src, tmp_path)
                file_publication.durable_replace(tmp_path, dst)
                return
            except OSError as exc:
                if not _link_failure_wants_copy(exc):
                    raise
        _atomic_copy_file(src, dst)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


@contextmanager
def _atomic_zip_file(path: Path) -> Generator[zipfile.ZipFile]:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = file_publication.staged_file_path(path, purpose="zip")
    try:
        with zipfile.ZipFile(tmp_path, "w") as zf:
            yield zf
        file_publication.durable_replace(tmp_path, path)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()
