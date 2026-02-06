"""Capability-gated pathlib implementation for Molt."""

from __future__ import annotations

from collections.abc import Iterator

from _intrinsics import require_intrinsic as _require_intrinsic
from molt import capabilities
from molt.stdlib import os as _os

_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join", globals())
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs", globals())
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname", globals())
_MOLT_PATH_SPLITEXT = _require_intrinsic("molt_path_splitext", globals())
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath", globals())
_MOLT_PATH_PARTS = _require_intrinsic("molt_path_parts", globals())
_MOLT_PATH_SPLITROOT = _require_intrinsic("molt_path_splitroot", globals())
_MOLT_PATH_PARENTS = _require_intrinsic("molt_path_parents", globals())
_MOLT_PATH_RELATIVE_TO = _require_intrinsic("molt_path_relative_to", globals())
_MOLT_PATH_WITH_NAME = _require_intrinsic("molt_path_with_name", globals())
_MOLT_PATH_WITH_SUFFIX = _require_intrinsic("molt_path_with_suffix", globals())
_MOLT_PATH_EXPANDUSER = _require_intrinsic("molt_path_expanduser", globals())
_MOLT_PATH_MATCH = _require_intrinsic("molt_path_match", globals())
_MOLT_PATH_GLOB = _require_intrinsic("molt_path_glob", globals())
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists", globals())
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir", globals())
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile", globals())
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir", globals())
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir", globals())
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink", globals())
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir", globals())
_MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex", globals())


class Path:
    def __init__(self, path: str | Path | None = None) -> None:
        if path is None:
            self._path = "."
        elif isinstance(path, Path):
            self._path = path._path
        else:
            fspath = _os.fspath(path)
            if isinstance(fspath, bytes):
                raise TypeError(
                    "argument should be a str or an os.PathLike object "
                    "where __fspath__ returns a str, not 'bytes'"
                )
            self._path = fspath

    @classmethod
    def cwd(cls) -> Path:
        return cls(_os.getcwd())

    @classmethod
    def home(cls) -> Path:
        return cls("~").expanduser()

    def _coerce_part(self, value: str | Path) -> str:
        if isinstance(value, Path):
            return value._path
        fspath = _os.fspath(value)
        if isinstance(fspath, bytes):
            raise TypeError(
                "argument should be a str or an os.PathLike object "
                "where __fspath__ returns a str, not 'bytes'"
            )
        return fspath

    def __fspath__(self) -> str:
        return self._path

    def __str__(self) -> str:
        return self._path

    def __bytes__(self) -> bytes:
        return bytes(self._path, "utf-8")

    def __repr__(self) -> str:
        return f"Path({self._path!r})"

    def __hash__(self) -> int:
        return hash(tuple(self._parts()))

    def as_posix(self) -> str:
        return self._path.replace(_os.sep, "/")

    def as_uri(self) -> str:
        if not self.is_absolute():
            raise ValueError("relative path can't be expressed as a file URI")
        path = self.as_posix()
        if not path.startswith("/"):
            path = "/" + path
        return "file://" + path

    def is_absolute(self) -> bool:
        return bool(_MOLT_PATH_ISABS(self._path))

    def absolute(self) -> Path:
        return self._wrap(_MOLT_PATH_ABSPATH(self._path))

    def expanduser(self) -> Path:
        return self._wrap(_MOLT_PATH_EXPANDUSER(self._path))

    def resolve(self) -> Path:
        return self._wrap(_MOLT_PATH_ABSPATH(self._path))

    def _parts(self) -> list[str]:
        raw = _MOLT_PATH_PARTS(self._path)
        if not isinstance(raw, list):
            raise RuntimeError("path parts intrinsic returned invalid value")
        parts: list[str] = []
        for item in raw:
            if not isinstance(item, str):
                raise RuntimeError("path parts intrinsic returned invalid value")
            parts.append(item)
        return parts

    def _splitroot(self) -> tuple[str, str, str]:
        raw = _MOLT_PATH_SPLITROOT(self._path)
        if (
            not isinstance(raw, (tuple, list))
            or len(raw) != 3
            or not isinstance(raw[0], str)
            or not isinstance(raw[1], str)
            or not isinstance(raw[2], str)
        ):
            raise RuntimeError("path splitroot intrinsic returned invalid value")
        return str(raw[0]), str(raw[1]), str(raw[2])

    @property
    def parts(self) -> tuple[str, ...]:
        return tuple(self._parts())

    @property
    def drive(self) -> str:
        drive, _root, _tail = self._splitroot()
        return drive

    @property
    def root(self) -> str:
        _drive, root, _tail = self._splitroot()
        return root

    @property
    def anchor(self) -> str:
        drive, root, _tail = self._splitroot()
        return drive + root

    def _wrap(self, path: str) -> Path:
        return Path(path)

    def joinpath(self, *others: str) -> Path:
        path = self._path
        for part in others:
            part = self._coerce_part(part)
            path = _MOLT_PATH_JOIN(path, part)
        return self._wrap(path)

    def __truediv__(self, key: str) -> Path:
        key = self._coerce_part(key)
        path = _MOLT_PATH_JOIN(self._path, key)
        return self._wrap(path)

    def open(
        self,
        mode: str = "r",
        buffering: int = -1,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
        closefd: bool = True,
        opener: object | None = None,
    ):
        return _MOLT_FILE_OPEN_EX(
            self._path,
            mode,
            buffering,
            encoding,
            errors,
            newline,
            closefd,
            opener,
        )

    def read_text(self, encoding: str | None = None, errors: str | None = None) -> str:
        capabilities.require("fs.read")
        with self.open("r", encoding=encoding, errors=errors) as handle:
            return handle.read()

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        with self.open("rb") as handle:
            return handle.read()

    def write_text(
        self,
        data: str,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> int:
        capabilities.require("fs.write")
        with self.open(
            "w",
            encoding=encoding,
            errors=errors,
            newline=newline,
        ) as handle:
            return handle.write(data)

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        with self.open("wb") as handle:
            return handle.write(data)

    def exists(self) -> bool:
        capabilities.require("fs.read")
        result = _MOLT_PATH_EXISTS(self._path)
        if result is None:
            return False
        return bool(result)

    def is_dir(self) -> bool:
        capabilities.require("fs.read")
        return bool(_MOLT_PATH_ISDIR(self._path))

    def is_file(self) -> bool:
        capabilities.require("fs.read")
        return bool(_MOLT_PATH_ISFILE(self._path))

    def unlink(self) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_UNLINK(self._path)

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        names = _MOLT_PATH_LISTDIR(self._path)
        if not isinstance(names, list):
            raise RuntimeError("path listdir intrinsic returned invalid value")
        for name in names:
            if not isinstance(name, str):
                raise RuntimeError("path listdir intrinsic returned invalid value")
            yield self.joinpath(name)

    def glob(self, pattern: str) -> Iterator[Path]:
        capabilities.require("fs.read")
        names = _MOLT_PATH_GLOB(self._path, str(pattern))
        for name in names:
            yield self.joinpath(name)

    def rglob(self, pattern: str) -> Iterator[Path]:
        pat = str(pattern)
        if pat == "**" or pat.startswith("**" + _os.sep):
            full = pat
        else:
            full = "**" + _os.sep + pat
        yield from self.glob(full)

    def mkdir(
        self,
        mode: int = 0o777,
        parents: bool = False,
        exist_ok: bool = False,
    ) -> None:
        capabilities.require("fs.write")

        def _is_dir(path: str) -> bool:
            try:
                _MOLT_PATH_LISTDIR(path)
                return True
            except Exception:
                return False

        if parents:
            path = self._path
            if not path:
                return
            parts: list[str] = []
            for part in path.split(_os.sep):
                if not part:
                    if not parts:
                        parts.append(_os.sep)
                    continue
                parts.append(part)
                current = parts[0]
                for extra in parts[1:]:
                    current = _MOLT_PATH_JOIN(current, extra)
                if _MOLT_PATH_EXISTS(current):
                    continue
                try:
                    _MOLT_PATH_MKDIR(current)
                except FileExistsError:
                    if not exist_ok:
                        raise
            if not exist_ok and not _MOLT_PATH_EXISTS(path):
                raise FileNotFoundError(path)
            return
        try:
            _MOLT_PATH_MKDIR(self._path)
        except FileExistsError:
            if exist_ok and _is_dir(self._path):
                return
            raise

    def rmdir(self) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_RMDIR(self._path)

    @property
    def name(self) -> str:
        parts = self._parts()
        if not parts:
            return ""
        if parts == [_os.sep]:
            return ""
        return parts[-1]

    @property
    def suffix(self) -> str:
        result = _MOLT_PATH_SPLITEXT(self._path)
        if not isinstance(result, tuple) or len(result) != 2:
            raise RuntimeError("path splitext intrinsic returned invalid value")
        suffix = result[1]
        if not isinstance(suffix, str):
            raise RuntimeError("path splitext intrinsic returned invalid value")
        return suffix

    @property
    def suffixes(self) -> list[str]:
        name = self.name
        if not name or name == ".":
            return []
        suffixes: list[str] = []
        stem = name
        while True:
            result = _MOLT_PATH_SPLITEXT(stem)
            if not isinstance(result, tuple) or len(result) != 2:
                raise RuntimeError("path splitext intrinsic returned invalid value")
            stem, suffix = result
            if not isinstance(stem, str) or not isinstance(suffix, str):
                raise RuntimeError("path splitext intrinsic returned invalid value")
            if not suffix:
                break
            suffixes.insert(0, suffix)
        return suffixes

    @property
    def stem(self) -> str:
        name = self.name
        if not name or name == ".":
            return ""
        result = _MOLT_PATH_SPLITEXT(name)
        if not isinstance(result, tuple) or len(result) != 2:
            raise RuntimeError("path splitext intrinsic returned invalid value")
        stem = result[0]
        if not isinstance(stem, str):
            raise RuntimeError("path splitext intrinsic returned invalid value")
        return stem

    @property
    def parent(self) -> Path:
        parent = _MOLT_PATH_DIRNAME(self._path) or "."
        return self._wrap(parent)

    @property
    def parents(self) -> list[Path]:
        raw = _MOLT_PATH_PARENTS(self._path)
        if not isinstance(raw, list):
            raise RuntimeError("path parents intrinsic returned invalid value")
        out: list[Path] = []
        for item in raw:
            if not isinstance(item, str):
                raise RuntimeError("path parents intrinsic returned invalid value")
            out.append(self._wrap(item))
        return out

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return False
        return self._parts() == other._parts()

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return tuple(self._parts()) < tuple(other._parts())

    def __le__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return tuple(self._parts()) <= tuple(other._parts())

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return tuple(self._parts()) > tuple(other._parts())

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return tuple(self._parts()) >= tuple(other._parts())

    def relative_to(self, *other: str) -> Path:
        if not other:
            raise TypeError(
                "relative_to() missing 1 required positional argument: 'other'"
            )
        base = self._coerce_part(other[0])
        for part in other[1:]:
            part = self._coerce_part(part)
            if part.startswith(_os.sep):
                base = part
            else:
                if base and not base.endswith(_os.sep):
                    base += _os.sep
                base += part
        return self._wrap(_MOLT_PATH_RELATIVE_TO(self._path, base))

    def is_relative_to(self, *other: str) -> bool:
        try:
            self.relative_to(*other)
            return True
        except Exception:
            return False

    def match(self, pattern: str) -> bool:
        return bool(_MOLT_PATH_MATCH(self._path, str(pattern)))

    def with_name(self, name: str) -> Path:
        return self._wrap(_MOLT_PATH_WITH_NAME(self._path, name))

    def with_suffix(self, suffix: str) -> Path:
        return self._wrap(_MOLT_PATH_WITH_SUFFIX(self._path, suffix))

    def with_stem(self, stem: str) -> Path:
        stem = str(stem)
        if not stem:
            raise ValueError(f"{self._path!r} has an empty name")
        return self.with_name(stem + self.suffix)


PurePosixPath = Path
PureWindowsPath = Path
PurePath = Path
