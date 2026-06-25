from __future__ import annotations

import contextlib
from contextlib import contextmanager
import errno
import json
import os
import shutil
import uuid
from pathlib import Path
from typing import Any, Iterator, Mapping
import zipfile


def _atomic_write_text(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with tmp_path.open("w", encoding="utf-8") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_path, path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(path.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def _write_text_if_changed(path: Path, content: str) -> None:
    try:
        existing = path.read_text()
    except OSError:
        existing = None
    if existing == content:
        return
    _atomic_write_text(path, content)


def _remove_file_or_tree(path: Path) -> None:
    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()


def _atomic_write_bytes(path: Path, data: bytes) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with tmp_path.open("wb") as handle:
            handle.write(data)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(tmp_path, path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(path.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def _atomic_write_json(
    path: Path,
    payload: Any,
    *,
    indent: int | None = 2,
    sort_keys: bool = False,
    default: Any | None = None,
) -> None:
    _atomic_write_text(
        path,
        json.dumps(
            payload,
            indent=indent,
            sort_keys=sort_keys,
            default=default,
        )
        + "\n",
    )


def _write_json_sidecar(path: Path, payload: Mapping[str, Any]) -> None:
    _atomic_write_json(path, payload, indent=2, sort_keys=True)


def _codesign_atomic_copy_temp(path: Path) -> None:
    from molt.cli.native_toolchain import _codesign_binary

    _codesign_binary(path)


def _atomic_copy_file(src: Path, dst: Path, *, codesign: bool = False) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        shutil.copyfile(src, tmp_path)
        with contextlib.suppress(OSError):
            shutil.copymode(src, tmp_path)
        if codesign:
            _codesign_atomic_copy_temp(tmp_path)
        tmp_path.replace(dst)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(dst.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


def _atomic_link_or_copy_file(src: Path, dst: Path) -> None:
    dst.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = dst.with_name(f".{dst.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        try:
            os.link(src, tmp_path)
            try:
                tmp_path.replace(dst)
                return
            except OSError as exc:
                if exc.errno != errno.ENOENT:
                    raise
        except OSError as exc:
            if exc.errno not in {
                errno.EXDEV,
                errno.EPERM,
                errno.EACCES,
                errno.ENOTSUP,
                errno.ENOENT,
            }:
                raise
        shutil.copyfile(src, tmp_path)
        tmp_path.replace(dst)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()


@contextmanager
def _atomic_zip_file(path: Path) -> Iterator[zipfile.ZipFile]:
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp_path = path.with_name(f".{path.name}.{os.getpid()}.{uuid.uuid4().hex}.tmp")
    try:
        with zipfile.ZipFile(tmp_path, "w") as zf:
            yield zf
        os.replace(tmp_path, path)
        if os.name == "posix":
            with contextlib.suppress(OSError):
                dir_fd = os.open(path.parent, os.O_RDONLY)
                try:
                    os.fsync(dir_fd)
                finally:
                    os.close(dir_fd)
    finally:
        with contextlib.suppress(OSError):
            if tmp_path.exists():
                tmp_path.unlink()
