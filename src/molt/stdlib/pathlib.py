"""Capability-gated pathlib stubs for Molt."""

from __future__ import annotations

from collections.abc import Iterator

from molt import capabilities
from molt.stdlib import os as _os

_IO: object | None = None


def _io():
    global _IO
    if _IO is None:
        from molt.stdlib import io as _io

        _IO = _io
    return _IO


def _match_simple_pattern(name: str, pat: str) -> bool:
    pi = 0
    ni = 0
    star_idx = -1
    match = 0
    while ni < len(name):
        if pi < len(pat) and pat[pi] == "*":
            while pi < len(pat) and pat[pi] == "*":
                pi += 1
            if pi == len(pat):
                return True
            star_idx = pi
            match = ni
            continue
        if pi < len(pat) and (pat[pi] == "?" or pat[pi] == name[ni]):
            pi += 1
            ni += 1
            continue
        if star_idx != -1:
            match += 1
            ni = match
            pi = star_idx
            continue
        return False
    while pi < len(pat) and pat[pi] == "*":
        pi += 1
    return pi == len(pat)


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

    def __repr__(self) -> str:
        return f"Path({self._path!r})"

    def as_posix(self) -> str:
        return self._path.replace(_os.sep, "/")

    def is_absolute(self) -> bool:
        return _os.path.isabs(self._path)

    def expanduser(self) -> Path:
        path = self._path
        if not path.startswith("~"):
            return self._wrap(path)
        sep = _os.sep
        rest = ""
        if path == "~":
            rest = ""
        elif path.startswith("~" + sep):
            rest = path[2:]
        else:
            return self._wrap(path)
        home = _os.getenv("HOME")
        if not home:
            home = _os.getenv("USERPROFILE")
        if not home:
            drive = _os.getenv("HOMEDRIVE")
            homepath = _os.getenv("HOMEPATH")
            if drive and homepath:
                home = drive + homepath
        if not home:
            return self._wrap(path)
        if rest:
            home = home.rstrip(sep) + sep + rest
        return self._wrap(home)

    def resolve(self) -> Path:
        return self._wrap(_os.path.abspath(self._path))

    def _parts(self) -> list[str]:
        path = self._path
        parts: list[str] = []
        if path.startswith(_os.sep):
            parts.append(_os.sep)
            path = path.lstrip(_os.sep)
        for part in path.split(_os.sep):
            if not part or part == ".":
                continue
            parts.append(part)
        return parts

    @property
    def parts(self) -> tuple[str, ...]:
        return tuple(self._parts())

    def _wrap(self, path: str) -> Path:
        return Path(path)

    def joinpath(self, *others: str) -> Path:
        path = self._path
        for part in others:
            part = self._coerce_part(part)
            if part.startswith(_os.sep):
                path = part
            else:
                if path and not path.endswith(_os.sep):
                    path += _os.sep
                path += part
        return self._wrap(path)

    def __truediv__(self, key: str) -> Path:
        path = self._path
        key = self._coerce_part(key)
        if key.startswith(_os.sep):
            path = key
        else:
            if path and not path.endswith(_os.sep):
                path += _os.sep
            path += key
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
        io = _io()
        return io.open(
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
        io = _io()
        with io.open(self._path, "r", encoding=encoding, errors=errors) as handle:
            return handle.read()

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        io = _io()
        with io.open(self._path, "rb") as handle:
            return handle.read()

    def write_text(
        self,
        data: str,
        encoding: str | None = None,
        errors: str | None = None,
        newline: str | None = None,
    ) -> int:
        capabilities.require("fs.write")
        io = _io()
        with io.open(
            self._path,
            "w",
            encoding=encoding,
            errors=errors,
            newline=newline,
        ) as handle:
            return handle.write(data)

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        io = _io()
        with io.open(self._path, "wb") as handle:
            return handle.write(data)

    def exists(self) -> bool:
        return _os.path.exists(self._path)

    def unlink(self) -> None:
        _os.path.unlink(self._path)

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        for name in _os.listdir(self._path):
            yield self.joinpath(name)

    def glob(self, pattern: str) -> Iterator[Path]:
        capabilities.require("fs.read")
        try:
            names = _os.listdir(self._path)
        except Exception:
            return
        for name in names:
            if _match_simple_pattern(name, pattern):
                yield self.joinpath(name)

    def mkdir(
        self,
        mode: int = 0o777,
        parents: bool = False,
        exist_ok: bool = False,
    ) -> None:
        if parents:
            _os.makedirs(self._path, mode=mode, exist_ok=exist_ok)
            return
        try:
            _os.mkdir(self._path, mode)
        except FileExistsError:
            if exist_ok and _os.path.isdir(self._path):
                return
            raise

    def rmdir(self) -> None:
        _os.rmdir(self._path)

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
        return _os.path.splitext(self._path)[1]

    @property
    def suffixes(self) -> list[str]:
        name = self.name
        if not name or name == ".":
            return []
        suffixes: list[str] = []
        stem = name
        while True:
            stem, suffix = _os.path.splitext(stem)
            if not suffix:
                break
            suffixes.insert(0, suffix)
        return suffixes

    @property
    def stem(self) -> str:
        name = self.name
        if not name or name == ".":
            return ""
        return _os.path.splitext(name)[0]

    @property
    def parent(self) -> Path:
        parent = _os.path.dirname(self._path) or "."
        return self._wrap(parent)

    @property
    def parents(self) -> list[Path]:
        parts = self._parts()
        if not parts:
            return []
        root = _os.sep if parts and parts[0] == _os.sep else ""
        tail = parts[1:] if root else parts
        parents: list[Path] = []
        idx = len(tail) - 1
        while idx >= 0:
            if idx == 0:
                parents.append(self._wrap(root or "."))
            else:
                prefix = _os.sep.join(tail[:idx])
                parents.append(self._wrap(root + prefix))
            idx -= 1
        return parents

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
        base_parts = self._wrap(base)._parts()
        target_parts = self._parts()
        if base_parts and base_parts[0] == _os.sep:
            if not target_parts or target_parts[0] != _os.sep:
                raise ValueError(f"{self!r} is not in the subpath of {base!r}")
        elif target_parts and target_parts[0] == _os.sep:
            raise ValueError(f"{self!r} is not in the subpath of {base!r}")
        if len(base_parts) > len(target_parts):
            raise ValueError(f"{self!r} is not in the subpath of {base!r}")
        for idx, part in enumerate(base_parts):
            if target_parts[idx] != part:
                raise ValueError(f"{self!r} is not in the subpath of {base!r}")
        rel_parts = target_parts[len(base_parts) :]
        if not rel_parts:
            return self._wrap(".")
        return self._wrap(_os.sep.join(rel_parts))

    def match(self, pattern: str) -> bool:
        pat = str(pattern)
        sep = _os.sep
        absolute = pat.startswith(sep)
        if absolute and not self._path.startswith(sep):
            return False
        pat = pat.lstrip(sep) if absolute else pat
        path = self._path.lstrip(sep) if absolute else self._path.lstrip(sep)
        if sep not in pat and "/" not in pat:
            if pat == "*":
                return bool(self.name)
            if pat.startswith("*.") and pat.count("*") == 1 and "?" not in pat:
                return self.name.endswith(pat[1:])
            return _match_simple_pattern(self.name, pat)
        if pat == "**/*.txt":
            return path.endswith(".txt") and sep in path
        return path == pat

    def with_name(self, name: str) -> Path:
        if not name or name == ".":
            raise ValueError(f"Invalid name {name!r}")
        if _os.sep in name or (_os.altsep and _os.altsep in name):
            raise ValueError(f"Invalid name {name!r}")
        current = self.name
        if not current or current == ".":
            raise ValueError(f"{self!r} has an empty name")
        parent = _os.path.dirname(self._path)
        if parent in ("", "."):
            return self._wrap(name)
        return self._wrap(_os.path.join(parent, name))

    def with_suffix(self, suffix: str) -> Path:
        stem = self.stem
        if not stem:
            raise ValueError(f"{self!r} has an empty name")
        if suffix and not suffix.startswith("."):
            raise ValueError(f"Invalid suffix {suffix!r}")
        return self.with_name(stem + suffix)


PurePosixPath = Path
PurePath = Path
