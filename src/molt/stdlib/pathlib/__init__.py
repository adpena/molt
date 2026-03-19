"""Capability-gated pathlib implementation for Molt -- fully intrinsic-backed."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import os as _os

_MOLT_CAPABILITIES_TRUSTED = _require_intrinsic("molt_capabilities_trusted")
_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require")


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

# --- existing intrinsics ---
_MOLT_PATH_JOIN = _require_intrinsic("molt_path_join")
_MOLT_PATH_ISABS = _require_intrinsic("molt_path_isabs")
_MOLT_PATH_DIRNAME = _require_intrinsic("molt_path_dirname")
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath")
_MOLT_PATH_RESOLVE = _require_intrinsic("molt_path_resolve")
_MOLT_PATH_PARTS = _require_intrinsic("molt_path_parts")
_MOLT_PATH_SPLITROOT = _require_intrinsic("molt_path_splitroot")
_MOLT_PATH_PARENTS = _require_intrinsic("molt_path_parents")
_MOLT_PATH_COMPARE = _require_intrinsic("molt_path_compare")
_MOLT_PATH_RELATIVE_TO = _require_intrinsic("molt_path_relative_to")
_MOLT_PATH_WITH_NAME = _require_intrinsic("molt_path_with_name")
_MOLT_PATH_WITH_SUFFIX = _require_intrinsic("molt_path_with_suffix")
_MOLT_PATH_WITH_STEM = _require_intrinsic("molt_path_with_stem")
_MOLT_PATH_IS_RELATIVE_TO = _require_intrinsic("molt_path_is_relative_to")
_MOLT_PATH_EXPANDUSER = _require_intrinsic("molt_path_expanduser")
_MOLT_PATH_MATCH = _require_intrinsic("molt_path_match")
_MOLT_PATH_GLOB = _require_intrinsic("molt_path_glob")
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile")
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink")
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink")
_MOLT_PATH_SYMLINK = _require_intrinsic("molt_path_symlink")
_MOLT_PATH_LISTDIR = _require_intrinsic("molt_path_listdir")
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir")
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink")
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir")
_MOLT_PATH_MAKEDIRS = _require_intrinsic("molt_path_makedirs")
_MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex")
_MOLT_PATH_JOIN_MANY = _require_intrinsic("molt_path_join_many")
_MOLT_PATH_NAME = _require_intrinsic("molt_path_name")
_MOLT_PATH_SUFFIX = _require_intrinsic("molt_path_suffix")
_MOLT_PATH_STEM = _require_intrinsic("molt_path_stem")
_MOLT_PATH_SUFFIXES = _require_intrinsic("molt_path_suffixes")
_MOLT_PATH_AS_URI = _require_intrinsic("molt_path_as_uri")
_MOLT_PATH_RELATIVE_TO_MANY = _require_intrinsic(
    "molt_path_relative_to_many")
_MOLT_PATH_CHMOD = _require_intrinsic("molt_path_chmod")
_MOLT_OS_STAT = _require_intrinsic("molt_os_stat")
_MOLT_OS_LSTAT = _require_intrinsic("molt_os_lstat")
_MOLT_OS_RENAME = _require_intrinsic("molt_os_rename")
_MOLT_OS_REPLACE = _require_intrinsic("molt_os_replace")

# --- new pathlib intrinsics (fully backing __init__, __str__, __hash__, etc.) ---
_MOLT_PATHLIB_STR = _require_intrinsic("molt_pathlib_str")
_MOLT_PATHLIB_HASH = _require_intrinsic("molt_pathlib_hash")
_MOLT_PATHLIB_EQ = _require_intrinsic("molt_pathlib_eq")
_MOLT_PATHLIB_LT = _require_intrinsic("molt_pathlib_lt")
_MOLT_PATHLIB_AS_POSIX = _require_intrinsic("molt_pathlib_as_posix")
_MOLT_PATHLIB_SEP = _require_intrinsic("molt_pathlib_sep")
_MOLT_PATHLIB_SPLITROOT = _require_intrinsic("molt_pathlib_splitroot")
_MOLT_PATHLIB_PARTS = _require_intrinsic("molt_pathlib_parts")
_MOLT_PATHLIB_SAMEFILE = _require_intrinsic("molt_pathlib_samefile")
_MOLT_PATHLIB_OWNER = _require_intrinsic("molt_pathlib_owner")
_MOLT_PATHLIB_GROUP = _require_intrinsic("molt_pathlib_group")
_MOLT_PATHLIB_IS_MOUNT = _require_intrinsic("molt_pathlib_is_mount")
_MOLT_PATHLIB_HARDLINK_TO = _require_intrinsic("molt_pathlib_hardlink_to")
_MOLT_PATHLIB_READ_TEXT = _require_intrinsic("molt_pathlib_read_text")
_MOLT_PATHLIB_READ_BYTES = _require_intrinsic("molt_pathlib_read_bytes")
_MOLT_PATHLIB_WRITE_TEXT = _require_intrinsic("molt_pathlib_write_text")
_MOLT_PATHLIB_WRITE_BYTES = _require_intrinsic("molt_pathlib_write_bytes")
_MOLT_PATHLIB_TOUCH = _require_intrinsic("molt_pathlib_touch")
_MOLT_PATHLIB_CWD = _require_intrinsic("molt_pathlib_cwd")
_MOLT_PATHLIB_HOME = _require_intrinsic("molt_pathlib_home")
_MOLT_PATHLIB_RGLOB = _require_intrinsic("molt_pathlib_rglob")
_MOLT_PATHLIB_ITERDIR = _require_intrinsic("molt_pathlib_iterdir")
_MOLT_PATHLIB_RESOLVE = _require_intrinsic("molt_pathlib_resolve")
_MOLT_PATHLIB_EXPANDUSER = _require_intrinsic("molt_pathlib_expanduser")

# Windows path intrinsics (reuse splitroot with posix=False)
_MOLT_PATHLIB_WIN_SPLITROOT = _require_intrinsic("molt_pathlib_splitroot")


def _coerce_fspath(path) -> str:
    """Coerce a path-like or str to str via os.fspath, backed by intrinsic."""
    if isinstance(path, Path):
        return path._path
    text = _os.fspath(path)
    if isinstance(text, bytes):
        raise TypeError(
            "argument should be a str or an os.PathLike object "
            "where __fspath__ returns a str, not 'bytes'"
        )
    return text


class Path:
    __slots__ = ("_path",)

    def __init__(self, path=None) -> None:
        if path is None:
            self._path = "."
        elif isinstance(path, Path):
            self._path = path._path
        else:
            self._path = _coerce_fspath(path)

    @classmethod
    def cwd(cls) -> Path:
        return cls(_MOLT_PATHLIB_CWD())

    @classmethod
    def home(cls) -> Path:
        return cls(_MOLT_PATHLIB_HOME())

    def _coerce_part(self, value) -> str:
        if isinstance(value, Path):
            return value._path
        return _coerce_fspath(value)

    def __fspath__(self) -> str:
        return self._path

    def __str__(self) -> str:
        return str(_MOLT_PATHLIB_STR(self._path))

    def __bytes__(self) -> bytes:
        return bytes(self.__str__(), "utf-8")

    def __repr__(self) -> str:
        return f"Path({self._path!r})"

    def __hash__(self) -> int:
        return int(_MOLT_PATHLIB_HASH(self._path))

    def as_posix(self) -> str:
        return str(_MOLT_PATHLIB_AS_POSIX(self._path))

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
        return self._wrap(str(_MOLT_PATHLIB_EXPANDUSER(self._path)))

    def resolve(self, strict: bool = False) -> Path:
        return self._wrap(str(_MOLT_PATHLIB_RESOLVE(self._path)))

    def _parts(self) -> list[str]:
        raw = _MOLT_PATHLIB_PARTS(self._path, True)
        if isinstance(raw, tuple):
            return list(raw)
        if isinstance(raw, list):
            return raw
        raise RuntimeError("path parts intrinsic returned invalid value")

    def _splitroot(self) -> tuple[str, str, str]:
        raw = _MOLT_PATHLIB_SPLITROOT(self._path, True)
        if (
            not isinstance(raw, (tuple, list))
            or len(raw) != 3
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

    def __truediv__(self, key) -> Path:
        key = self._coerce_part(key)
        path = _MOLT_PATH_JOIN(self._path, key)
        return self._wrap(path)

    def __rtruediv__(self, key) -> Path:
        key = self._coerce_part(key)
        path = _MOLT_PATH_JOIN(key, self._path)
        return self._wrap(path)

    def open(
        self,
        mode: str = "r",
        buffering: int = -1,
        encoding=None,
        errors=None,
        newline=None,
        closefd: bool = True,
        opener=None,
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

    def read_text(self, encoding=None, errors=None) -> str:
        capabilities.require("fs.read")
        return str(_MOLT_PATHLIB_READ_TEXT(self._path))

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        return _MOLT_PATHLIB_READ_BYTES(self._path)

    def write_text(
        self,
        data: str,
        encoding=None,
        errors=None,
        newline=None,
    ) -> int:
        capabilities.require("fs.write")
        return int(_MOLT_PATHLIB_WRITE_TEXT(self._path, data))

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        return int(_MOLT_PATHLIB_WRITE_BYTES(self._path, data))

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

    def is_mount(self) -> bool:
        capabilities.require("fs.read")
        return bool(_MOLT_PATHLIB_IS_MOUNT(self._path))

    def readlink(self) -> Path:
        capabilities.require("fs.read")
        value = _MOLT_PATH_READLINK(self._path)
        if not isinstance(value, str):
            raise RuntimeError("path readlink intrinsic returned invalid value")
        return self._wrap(value)

    def symlink_to(self, target, target_is_directory: bool = False) -> None:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_PATH_SYMLINK(target_path, self._path, bool(target_is_directory))

    def hardlink_to(self, target) -> None:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_PATHLIB_HARDLINK_TO(self._path, target_path)

    def unlink(self, missing_ok: bool = False) -> None:
        capabilities.require("fs.write")
        try:
            _MOLT_PATH_UNLINK(self._path)
        except FileNotFoundError:
            if not missing_ok:
                raise

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        names = _MOLT_PATHLIB_ITERDIR(self._path)
        if not isinstance(names, list):
            raise RuntimeError("path iterdir intrinsic returned invalid value")
        for name in names:
            if isinstance(name, str):
                yield Path(name)

    def glob(self, pattern: str) -> Iterator[Path]:
        capabilities.require("fs.read")
        names = _MOLT_PATH_GLOB(self._path, str(pattern))
        for name in names:
            yield self.joinpath(name)

    def rglob(self, pattern: str) -> Iterator[Path]:
        capabilities.require("fs.read")
        results = _MOLT_PATHLIB_RGLOB(self._path, str(pattern))
        if not isinstance(results, list):
            raise RuntimeError("path rglob intrinsic returned invalid value")
        for name in results:
            if isinstance(name, str):
                yield Path(name)

    def mkdir(
        self,
        mode: int = 0o777,
        parents: bool = False,
        exist_ok: bool = False,
    ) -> None:
        capabilities.require("fs.write")
        if parents:
            _MOLT_PATH_MAKEDIRS(self._path, mode, bool(exist_ok))
            return
        try:
            _MOLT_PATH_MKDIR(self._path, mode)
        except FileExistsError:
            if exist_ok and bool(_MOLT_PATH_ISDIR(self._path)):
                return
            raise

    def rmdir(self) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_RMDIR(self._path)

    def stat(self) -> _os.stat_result:
        capabilities.require("fs.read")
        raw = _MOLT_OS_STAT(self._path)
        return _os.stat_result(raw)

    def lstat(self) -> _os.stat_result:
        capabilities.require("fs.read")
        raw = _MOLT_OS_LSTAT(self._path)
        return _os.stat_result(raw)

    def touch(self, mode: int = 0o666, exist_ok: bool = True) -> None:
        capabilities.require("fs.write")
        _MOLT_PATHLIB_TOUCH(self._path, exist_ok)

    def rename(self, target) -> Path:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_OS_RENAME(self._path, target_path)
        return Path(target_path)

    def replace(self, target) -> Path:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_OS_REPLACE(self._path, target_path)
        return Path(target_path)

    def chmod(self, mode: int) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_CHMOD(self._path, int(mode))

    def owner(self) -> str:
        capabilities.require("fs.read")
        return str(_MOLT_PATHLIB_OWNER(self._path))

    def group(self) -> str:
        capabilities.require("fs.read")
        return str(_MOLT_PATHLIB_GROUP(self._path))

    def samefile(self, other_path) -> bool:
        capabilities.require("fs.read")
        other = self._coerce_part(other_path)
        return bool(_MOLT_PATHLIB_SAMEFILE(self._path, other))

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
        return bool(_MOLT_PATHLIB_EQ(self._path, other._path))

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented  # type: ignore[return-value]
        return bool(_MOLT_PATHLIB_LT(self._path, other._path))

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

    def walk(
        self,
        top_down: bool = True,
        on_error=None,
        follow_symlinks: bool = False,
    ) -> Iterator[tuple[Path, list[str], list[str]]]:
        capabilities.require("fs.read")
        for dirpath, dirnames, filenames in _os.walk(
            str(self._path),
            topdown=top_down,
            onerror=on_error,
            followlinks=follow_symlinks,
        ):
            yield Path(dirpath), dirnames, filenames


PurePosixPath = Path
PurePath = Path


class _PureWindowsPath(Path):
    """Windows path flavor -- all parsing delegated to intrinsic splitroot."""
    __slots__ = ()

    def _splitroot_win(self) -> tuple[str, str, str]:
        raw = _MOLT_PATHLIB_WIN_SPLITROOT(self._path, False)
        if not isinstance(raw, (tuple, list)) or len(raw) != 3:
            raise RuntimeError("path splitroot intrinsic returned invalid value")
        return str(raw[0]), str(raw[1]), str(raw[2])

    def _parts_win(self) -> tuple[str, ...]:
        raw = _MOLT_PATHLIB_PARTS(self._path, False)
        if isinstance(raw, tuple):
            return raw
        if isinstance(raw, list):
            return tuple(raw)
        raise RuntimeError("path parts intrinsic returned invalid value")

    @property
    def anchor(self) -> str:
        drive, root, _tail = self._splitroot_win()
        return drive + root

    @property
    def drive(self) -> str:
        drive, _root, _tail = self._splitroot_win()
        return drive

    @property
    def root(self) -> str:
        _drive, root, _tail = self._splitroot_win()
        return root

    @property
    def parts(self) -> tuple[str, ...]:
        return self._parts_win()

    @property
    def name(self) -> str:
        parts = self._parts_win()
        if not parts:
            return ""
        anchor = self.anchor
        if len(parts) == 1 and parts[0] == anchor:
            return ""
        return parts[-1]

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
        parts = self._parts_win()
        anchor = self.anchor
        if not parts:
            return self._wrap(".")
        if len(parts) == 1 and parts[0] == anchor:
            return self._wrap(parts[0])
        if len(parts) == 1:
            return self._wrap(".")
        parent_parts = parts[:-1]
        return self._wrap("\\".join(parent_parts))

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
        parts = self._parts_win()
        new_parts = parts[:-1] + (base,)
        return self._wrap("\\".join(new_parts))

    def with_name(self, name: str) -> Path:
        if not name:
            raise ValueError("empty name")
        parts = self._parts_win()
        new_parts = parts[:-1] + (name,)
        return self._wrap("\\".join(new_parts))

    def as_posix(self) -> str:
        return str(_MOLT_PATHLIB_AS_POSIX(self._path))

    def is_absolute(self) -> bool:
        drive, root, _tail = self._splitroot_win()
        return bool(root and drive)


PureWindowsPath = _PureWindowsPath

globals().pop("_require_intrinsic", None)
