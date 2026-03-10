"""Minimal tempfile shim for Molt."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import os as _os

_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join", globals())

__all__ = [
    "NamedTemporaryFile",
    "TemporaryDirectory",
    "gettempdir",
    "gettempdirb",
    "mkdtemp",
]

_TEMP_DIR: str | None = None
_TEMP_COUNTER = 0


def _candidate_tempdir_list() -> list[str]:
    dirlist: list[str] = []
    for key in ("TMPDIR", "TEMP", "TMP"):
        try:
            val = _os.getenv(key)
        except PermissionError:
            # Capability-gated env reads can be denied; keep scanning deterministic
            # OS fallbacks instead of hard-failing.
            continue
        if val:
            dirlist.append(val)

    if _os.name == "nt":
        dirlist.extend(
            [
                _os.path.expanduser(r"~\AppData\Local\Temp"),
                _os.path.expandvars(r"%SYSTEMROOT%\Temp"),
                r"c:\temp",
                r"c:\tmp",
                r"\temp",
                r"\tmp",
            ]
        )
    else:
        dirlist.extend(["/tmp", "/var/tmp", "/usr/tmp"])

    try:
        dirlist.append(_os.getcwd())
    except (AttributeError, OSError):
        dirlist.append(_os.curdir)

    seen: set[str] = set()
    ordered: list[str] = []
    for dirname in dirlist:
        if not dirname:
            continue
        normalized = str(dirname)
        if normalized in seen:
            continue
        seen.add(normalized)
        ordered.append(normalized)
    return ordered


def _dir_is_usable(dirname: str) -> bool:
    directory = dirname if dirname == _os.curdir else _os.path.abspath(dirname)
    try:
        if not _os.path.isdir(directory):
            return False
    except OSError:
        return False

    flags = _os.O_RDWR | _os.O_CREAT | _os.O_EXCL
    if hasattr(_os, "O_BINARY"):
        flags |= _os.O_BINARY

    for seq in range(64):
        probe_name = f".molt_tmp_probe_{_os.getpid()}_{seq}"
        probe_path = _MOLT_PATH_JOIN(directory, probe_name)
        try:
            fd = _os.open(probe_path, flags, 0o600)
        except FileExistsError:
            continue
        except PermissionError:
            if _os.name == "nt":
                try:
                    if _os.path.isdir(directory) and _os.access(directory, _os.W_OK):
                        continue
                except OSError:
                    pass
            return False
        except OSError:
            return False

        try:
            _os.write(fd, b"molt")
        finally:
            _os.close(fd)
        try:
            _os.unlink(probe_path)
        except OSError:
            pass
        return True

    return False


def _pick_tempdir() -> str:
    dirlist = _candidate_tempdir_list()
    for dirname in dirlist:
        if _dir_is_usable(dirname):
            return dirname if dirname == _os.curdir else _os.path.abspath(dirname)
    raise FileNotFoundError(
        "No usable temporary directory found in " + ", ".join(dirlist)
    )


def gettempdir() -> str:
    global _TEMP_DIR
    if _TEMP_DIR is None:
        _TEMP_DIR = _pick_tempdir()
    return _TEMP_DIR


def gettempdirb() -> bytes:
    return gettempdir().encode("utf-8")


def mkdtemp(suffix: str = "", prefix: str = "tmp", dir: str | None = None) -> str:
    global _TEMP_COUNTER
    base = dir or gettempdir()
    for _ in range(10000):
        name = f"{prefix}{_TEMP_COUNTER}"
        _TEMP_COUNTER += 1
        candidate = _MOLT_PATH_JOIN(base, f"{name}{suffix}")
        try:
            _os.makedirs(candidate)
            return candidate
        except FileExistsError:
            continue
    raise FileExistsError("No usable temporary directory name")


class _NamedTemporaryFile:
    def __init__(self, handle, name: str, delete: bool) -> None:
        self._handle = handle
        self.name = name
        self.delete = delete
        self._closed = False

    def __getattr__(self, name: str):
        return getattr(self._handle, name)

    def __enter__(self):
        return self

    def close(self) -> None:
        if self._closed:
            return
        try:
            self._handle.close()
        finally:
            if self.delete:
                try:
                    _os.unlink(self.name)
                except FileNotFoundError:
                    pass
            self._closed = True

    def __exit__(self, exc_type, exc, tb) -> None:
        self.close()


def NamedTemporaryFile(
    mode: str = "w+b",
    buffering: int = -1,
    encoding: str | None = None,
    newline: str | None = None,
    suffix: str = "",
    prefix: str = "tmp",
    dir: str | None = None,
    delete: bool = True,
):
    global _TEMP_COUNTER
    base = dir or gettempdir()
    open_mode = mode
    if "x" not in open_mode:
        if "w" in open_mode:
            open_mode = open_mode.replace("w", "x", 1)
        elif "a" in open_mode:
            open_mode = open_mode.replace("a", "x", 1)
        else:
            open_mode = "x" + open_mode
    for _ in range(10000):
        name = f"{prefix}{_TEMP_COUNTER}"
        _TEMP_COUNTER += 1
        path = _MOLT_PATH_JOIN(base, f"{name}{suffix}")
        try:
            handle = open(
                path, open_mode, buffering=buffering, encoding=encoding, newline=newline
            )
        except FileExistsError:
            continue
        return _NamedTemporaryFile(handle, path, delete)
    raise FileExistsError("No usable temporary file name")


def _rmtree(path: str) -> None:
    try:
        entries = _os.listdir(path)
    except Exception:
        entries = []
    for name in entries:
        entry = _MOLT_PATH_JOIN(path, name)
        try:
            if _os.path.isdir(entry):
                _rmtree(entry)
                _os.rmdir(entry)
            else:
                _os.unlink(entry)
        except Exception:
            pass
    try:
        _os.rmdir(path)
    except Exception:
        pass


class TemporaryDirectory:
    def __init__(
        self, suffix: str = "", prefix: str = "tmp", dir: str | None = None
    ) -> None:
        self.name = mkdtemp(suffix=suffix, prefix=prefix, dir=dir)
        self._closed = False

    def cleanup(self) -> None:
        if self._closed:
            return
        _rmtree(self.name)
        self._closed = True

    def __enter__(self) -> str:
        return self.name

    def __exit__(self, exc_type, exc, tb) -> None:
        self.cleanup()
