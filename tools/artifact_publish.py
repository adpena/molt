from __future__ import annotations

import contextlib
import json
import os
import shutil
import uuid
from collections.abc import Callable
from typing import Any
from pathlib import Path


def staged_output_path(final: Path, *, root: Path | None = None) -> Path:
    """Return a same-directory hidden staging path for a final artifact."""
    stage_root = final.parent if root is None else root
    stage_root.mkdir(parents=True, exist_ok=True)
    return stage_root / f".{final.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp"


def fsync_parent(path: Path) -> None:
    if os.name != "posix":
        return
    with contextlib.suppress(OSError):
        dir_fd = os.open(path.parent, os.O_RDONLY)
        try:
            os.fsync(dir_fd)
        finally:
            os.close(dir_fd)


def fsync_file(path: Path) -> None:
    if os.name != "posix":
        return
    with contextlib.suppress(OSError), path.open("rb") as handle:
        os.fsync(handle.fileno())


def atomic_copy_file(src: Path, dst: Path) -> None:
    tmp_path = staged_output_path(dst)
    try:
        shutil.copyfile(src, tmp_path)
        with contextlib.suppress(OSError):
            shutil.copymode(src, tmp_path)
        fsync_file(tmp_path)
        publish_validated_outputs([(tmp_path, dst)])
    finally:
        with contextlib.suppress(OSError):
            tmp_path.unlink()


def atomic_write_bytes(path: Path, data: bytes) -> None:
    tmp_path = staged_output_path(path)
    try:
        with tmp_path.open("wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        publish_validated_outputs([(tmp_path, path)])
    finally:
        with contextlib.suppress(OSError):
            tmp_path.unlink()


def atomic_write_text(path: Path, text: str, *, encoding: str = "utf-8") -> None:
    atomic_write_bytes(path, text.encode(encoding))


def atomic_write_json(
    path: Path,
    payload: Any,
    *,
    indent: int | None = 2,
    sort_keys: bool = False,
    default: Callable[[Any], Any] | None = None,
) -> None:
    atomic_write_text(
        path,
        json.dumps(
            payload,
            indent=indent,
            sort_keys=sort_keys,
            default=default,
        )
        + "\n",
    )


def publish_validated_outputs(pairs: list[tuple[Path, Path]]) -> None:
    """Replace final artifacts as one rollback-protected publication set.

    Callers must validate every staged source before calling this function.
    Each staged source should live on the same filesystem as its final
    destination so every replace is atomic at the file level.
    """
    normalized = [(Path(staged), Path(final)) for staged, final in pairs]
    seen_finals: set[Path] = set()
    for staged, final in normalized:
        if final in seen_finals:
            raise ValueError(f"duplicate final artifact in publication set: {final}")
        seen_finals.add(final)
        if staged == final:
            raise ValueError(f"staged and final artifact paths match: {final}")
        if not staged.exists():
            raise FileNotFoundError(f"staged artifact missing: {staged}")

    backups: list[tuple[Path, Path]] = []
    replaced: list[Path] = []
    try:
        for staged, final in normalized:
            final.parent.mkdir(parents=True, exist_ok=True)
            backup: Path | None = None
            if final.exists() or final.is_symlink():
                backup = final.with_name(
                    f".{final.name}.{os.getpid()}.{uuid.uuid4().hex}.old"
                )
                os.replace(final, backup)
                backups.append((final, backup))
            try:
                os.replace(staged, final)
            except BaseException:
                if backup is not None and backup.exists() and not final.exists():
                    os.replace(backup, final)
                    backups.remove((final, backup))
                raise
            replaced.append(final)
            fsync_parent(final)
        for final, backup in backups:
            with contextlib.suppress(OSError):
                backup.unlink()
            fsync_parent(final)
    except BaseException:
        for final in reversed(replaced):
            with contextlib.suppress(OSError):
                final.unlink()
        for final, backup in reversed(backups):
            if backup.exists():
                with contextlib.suppress(OSError):
                    if final.exists() or final.is_symlink():
                        final.unlink()
                os.replace(backup, final)
                fsync_parent(final)
        raise
