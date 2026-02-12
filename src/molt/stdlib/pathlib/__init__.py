"""Capability-gated pathlib implementation for Molt."""

# TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P2, status:planned):
# continue broadening pathlib parity (glob recursion corner cases, Windows
# drive/anchor flavor nuances, and symlink edge semantics) while keeping path
# shaping in runtime intrinsics.

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import os as _os

_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted", globals())
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require", globals())


class _CapabilitiesProxy:
    __slots__ = ()

    def trusted(self) -> bool:
        return bool(_MOLT_CAPABILITIES_TRUSTED())

    def require(self, name: str) -> None:
        if self.trusted():
            return
        _MOLT_CAPABILITIES_REQUIRE(name)


capabilities = _CapabilitiesProxy()

TYPE_CHECKING = False
if TYPE_CHECKING:
    from typing import Iterator
else:

    class _TypingAlias:
        __slots__ = ()

        def __getitem__(self, _item):
            return self

    Iterator = _TypingAlias()

_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join", globals())
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs", globals())
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname", globals())
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath", globals())
_MOLT_PATH_RESOLVE = _require_intrinsic("molt_path_resolve", globals())
_MOLT_PATH_PARTS = _require_intrinsic("molt_path_parts", globals())
_MOLT_PATH_SPLITROOT = _require_intrinsic("molt_path_splitroot", globals())
_MOLT_PATH_PARENTS = _require_intrinsic("molt_path_parents", globals())
_MOLT_PATH_COMPARE = _require_intrinsic("molt_path_compare", globals())
_MOLT_PATH_RELATIVE_TO = _require_intrinsic("molt_path_relative_to", globals())
_MOLT_PATH_WITH_NAME = _require_intrinsic("molt_path_with_name", globals())
_MOLT_PATH_WITH_SUFFIX = _require_intrinsic("molt_path_with_suffix", globals())
_MOLT_PATH_WITH_STEM = _require_intrinsic("molt_path_with_stem", globals())
_MOLT_PATH_IS_RELATIVE_TO = _require_intrinsic("molt_path_is_relative_to", globals())
_MOLT_PATH_EXPANDUSER = _require_intrinsic("molt_path_expanduser", globals())
_MOLT_PATH_MATCH = _require_intrinsic("molt_path_match", globals())
_MOLT_PATH_GLOB = _require_intrinsic("molt_path_glob", globals())
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists", globals())
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir", globals())
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile", globals())
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink", globals())
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink", globals())
_MOLT_PATH_SYMLINK = _require_intrinsic("molt_path_symlink", globals())
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir", globals())
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir", globals())
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink", globals())
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir", globals())
_MOLT_PATH_MAKEDIRS = _require_intrinsic("molt_path_makedirs", globals())
_MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex", globals())
_MOLT_PATH_JOIN_MANY = _require_intrinsic("molt_path_join_many", globals())
_MOLT_PATH_NAME = _require_intrinsic("molt_path_name", globals())
_MOLT_PATH_SUFFIX = _require_intrinsic("molt_path_suffix", globals())
_MOLT_PATH_STEM = _require_intrinsic("molt_path_stem", globals())
_MOLT_PATH_SUFFIXES = _require_intrinsic("molt_path_suffixes", globals())
_MOLT_PATH_AS_URI = _require_intrinsic("molt_path_as_uri", globals())
_MOLT_PATH_RELATIVE_TO_MANY = _require_intrinsic(
    "molt_path_relative_to_many", globals()
)


def _coerce_windows_text(path: str | Path) -> str:
    if isinstance(path, Path):
        return path._path
    text = _os.fspath(path)
    if isinstance(text, bytes):
        raise TypeError(
            "argument should be a str or an os.PathLike object "
            "where __fspath__ returns a str, not 'bytes'"
        )
    return text.replace("/", "\\")


def _parse_windows_parts(path: str) -> tuple[str, tuple[str, ...]]:
    text = _coerce_windows_text(path)
    if text.startswith("\\\\"):
        remainder = text[2:]
        parts = [item for item in remainder.split("\\") if item]
        if len(parts) >= 2:
            anchor = f"\\\\{parts[0]}\\{parts[1]}\\"
            return anchor, (anchor, *tuple(parts[2:]))
        if len(parts) == 1:
            anchor = f"\\\\{parts[0]}\\"
            return anchor, (anchor,)
        return "\\\\", ("\\\\",)

    if len(text) >= 2 and text[1] == ":":
        if len(text) >= 3 and text[2] == "\\":
            anchor = f"{text[:2]}\\"
            tail = tuple(item for item in text[3:].split("\\") if item)
            return anchor, (anchor, *tail)
        anchor = text[:2]
        tail = tuple(item for item in text[2:].split("\\") if item)
        return anchor, (anchor, *tail)

    if text.startswith("\\"):
        anchor = "\\"
        tail = tuple(item for item in text[1:].split("\\") if item)
        return anchor, (anchor, *tail)

    return "", tuple(item for item in text.split("\\") if item)


def _windows_drive(anchor: str) -> str:
    if not anchor:
        return ""
    if anchor.startswith("\\\\"):
        return anchor[:-1]
    return anchor[:-1] if anchor.endswith("\\") else anchor


def _windows_root(anchor: str) -> str:
    if not anchor:
        return ""
    if anchor.startswith("\\\\"):
        return "\\"
    return "\\" if anchor.endswith("\\") else ""


def _windows_name(parts: tuple[str, ...], anchor: str) -> str:
    if not parts:
        return ""
    if len(parts) == 1 and parts[0] == anchor:
        return ""
    return parts[-1]


def _join_windows_parts(parts: tuple[str, ...], is_unc: bool) -> str:
    if not parts:
        return "."
    if len(parts) == 1:
        return parts[0]
    normalized = list(parts)
    normalized[0] = normalized[0].rstrip("\\")
    return "\\".join(normalized)


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
        uri = _MOLT_PATH_AS_URI(self._path)
        if not isinstance(uri, str):
            raise RuntimeError("path as_uri intrinsic returned invalid value")
        return uri

    def is_absolute(self) -> bool:
        return bool(_MOLT_PATH_ISABS(self._path))

    def absolute(self) -> Path:
        return self._wrap(_MOLT_PATH_ABSPATH(self._path))

    def expanduser(self) -> Path:
        return self._wrap(_MOLT_PATH_EXPANDUSER(self._path))

    def resolve(self, strict: bool = False) -> Path:
        return self._wrap(_MOLT_PATH_RESOLVE(self._path, bool(strict)))

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
        parts = [self._coerce_part(part) for part in others]
        path = _MOLT_PATH_JOIN_MANY(self._path, tuple(parts))
        if not isinstance(path, str):
            raise RuntimeError("path join_many intrinsic returned invalid value")
        return self._wrap(path)

    def __truediv__(self, key: str) -> Path:
        key = self._coerce_part(key)
        path = _MOLT_PATH_JOIN(self._path, key)
        return self._wrap(path)

    def __rtruediv__(self, key: str) -> Path:
        key = self._coerce_part(key)
        path = _MOLT_PATH_JOIN(key, self._path)
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

    def is_symlink(self) -> bool:
        capabilities.require("fs.read")
        return bool(_MOLT_PATH_ISLINK(self._path))

    def readlink(self) -> Path:
        capabilities.require("fs.read")
        value = _MOLT_PATH_READLINK(self._path)
        if not isinstance(value, str):
            raise RuntimeError("path readlink intrinsic returned invalid value")
        return self._wrap(value)

    def symlink_to(self, target: str | Path, target_is_directory: bool = False) -> None:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_PATH_SYMLINK(target_path, self._path, bool(target_is_directory))

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
        del mode
        if parents:
            _MOLT_PATH_MAKEDIRS(self._path, bool(exist_ok))
            return
        try:
            _MOLT_PATH_MKDIR(self._path)
        except FileExistsError:
            if exist_ok and bool(_MOLT_PATH_ISDIR(self._path)):
                return
            raise

    def rmdir(self) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_RMDIR(self._path)

    @property
    def name(self) -> str:
        name = _MOLT_PATH_NAME(self._path)
        if not isinstance(name, str):
            raise RuntimeError("path name intrinsic returned invalid value")
        return name

    @property
    def suffix(self) -> str:
        suffix = _MOLT_PATH_SUFFIX(self._path)
        if not isinstance(suffix, str):
            raise RuntimeError("path suffix intrinsic returned invalid value")
        return suffix

    @property
    def suffixes(self) -> list[str]:
        suffixes = _MOLT_PATH_SUFFIXES(self._path)
        if not isinstance(suffixes, list):
            raise RuntimeError("path suffixes intrinsic returned invalid value")
        for suffix in suffixes:
            if not isinstance(suffix, str):
                raise RuntimeError("path suffixes intrinsic returned invalid value")
        return suffixes

    @property
    def stem(self) -> str:
        stem = _MOLT_PATH_STEM(self._path)
        if not isinstance(stem, str):
            raise RuntimeError("path stem intrinsic returned invalid value")
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
        return int(_MOLT_PATH_COMPARE(self._path, other._path)) == 0

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return int(_MOLT_PATH_COMPARE(self._path, other._path)) < 0

    def __le__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return int(_MOLT_PATH_COMPARE(self._path, other._path)) <= 0

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return int(_MOLT_PATH_COMPARE(self._path, other._path)) > 0

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return int(_MOLT_PATH_COMPARE(self._path, other._path)) >= 0

    def relative_to(self, *other: str) -> Path:
        if not other:
            raise TypeError(
                "relative_to() missing 1 required positional argument: 'other'"
            )
        base = self._coerce_part(other[0])
        if len(other) == 1:
            rel = _MOLT_PATH_RELATIVE_TO(self._path, base)
        else:
            parts = [self._coerce_part(part) for part in other[1:]]
            rel = _MOLT_PATH_RELATIVE_TO_MANY(self._path, base, tuple(parts))
        if not isinstance(rel, str):
            raise RuntimeError("path relative_to intrinsic returned invalid value")
        return Path(str(rel))

    def is_relative_to(self, *other: str) -> bool:
        if not other:
            raise TypeError(
                "is_relative_to() missing 1 required positional argument: 'other'"
            )
        base = self._coerce_part(other[0])
        if len(other) == 1:
            return bool(_MOLT_PATH_IS_RELATIVE_TO(self._path, base, None))
        parts = [self._coerce_part(part) for part in other[1:]]
        return bool(_MOLT_PATH_IS_RELATIVE_TO(self._path, base, tuple(parts)))

    def match(self, pattern: str) -> bool:
        return bool(_MOLT_PATH_MATCH(self._path, str(pattern)))

    def with_name(self, name: str) -> Path:
        return self._wrap(_MOLT_PATH_WITH_NAME(self._path, name))

    def with_suffix(self, suffix: str) -> Path:
        return self._wrap(_MOLT_PATH_WITH_SUFFIX(self._path, suffix))

    def with_stem(self, stem: str) -> Path:
        return self._wrap(_MOLT_PATH_WITH_STEM(self._path, stem))


PurePosixPath = Path
PurePath = Path


class _PureWindowsPath(Path):
    __slots__ = ()

    def _parse(self) -> tuple[str, tuple[str, ...]]:
        return _parse_windows_parts(self._path)

    @property
    def anchor(self) -> str:
        return self._parse()[0]

    @property
    def drive(self) -> str:
        return _windows_drive(self.anchor)

    @property
    def root(self) -> str:
        return _windows_root(self.anchor)

    @property
    def parts(self) -> tuple[str, ...]:
        return self._parse()[1]

    @property
    def name(self) -> str:
        anchor, parts = self._parse()
        return _windows_name(parts, anchor)

    @property
    def suffix(self) -> str:
        name = self.name
        if not name:
            return ""
        dot = name.rfind(".")
        if dot <= 0:
            return ""
        return name[dot:]

    @property
    def stem(self) -> str:
        name = self.name
        if not name:
            return ""
        dot = name.rfind(".")
        if dot <= 0:
            return name
        return name[:dot]

    @property
    def parent(self) -> Path:
        parts = self.parts
        if not parts:
            return self._wrap(".")
        if len(parts) == 1 and parts[0] == self.anchor:
            return self._wrap(parts[0])
        if len(parts) == 1:
            return self._wrap(".")
        return self._wrap(
            _join_windows_parts(parts[:-1], is_unc=self.anchor.startswith("\\"))
        )

    def with_suffix(self, suffix: str) -> Path:
        name = self.name
        if not name:
            return self._wrap(self._path)
        suffix_text = str(suffix)
        if not suffix_text:
            base = name
        else:
            dot = name.rfind(".")
            base = name if dot <= 0 else name[:dot]
            base += suffix_text
        new_parts = self.parts[:-1] + (base,)
        return self._wrap(
            _join_windows_parts(new_parts, is_unc=self.anchor.startswith("\\"))
        )

    def with_name(self, name: str) -> Path:
        if not name:
            raise ValueError("empty name")
        name_text = _coerce_windows_text(name)
        return self._wrap(
            _join_windows_parts(
                (self.parts[:-1] + (name_text,)),
                is_unc=self.anchor.startswith("\\"),
            )
        )

    def as_posix(self) -> str:
        return self._path.replace("\\", "/")

    def is_absolute(self) -> bool:
        return bool(self.anchor)


PureWindowsPath = _PureWindowsPath
