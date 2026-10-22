"""Capability-gated pathlib implementation using the shared path and stream owners."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic
import os as _os
import io as _io
import sys as _sys
from itertools import chain as _chain
import warnings as _warnings

_MOLT_SYS_VERSION_INFO = _require_intrinsic("molt_sys_version_info")
_RUNTIME_VERSION = _MOLT_SYS_VERSION_INFO()
_PYTHON_313 = _RUNTIME_VERSION >= (3, 13)
_PYTHON_314 = _RUNTIME_VERSION >= (3, 14)

_WINDOWS_UTF16_ROOT = _os.name == "nt" and _PYTHON_313

_MOLT_CAPABILITIES_REQUIRE = _require_intrinsic("molt_capabilities_require")


class _CapabilitiesProxy:
    __slots__ = ()

    def require(self, name: str) -> None:
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
_MOLT_PATH_ABSPATH = _require_intrinsic("molt_path_abspath")
_MOLT_PATH_MATCH = _require_intrinsic("molt_path_match")
_MOLT_PATH_GLOB = _require_intrinsic("molt_path_glob")
_MOLT_PATH_EXISTS = _require_intrinsic("molt_path_exists")
_MOLT_PATH_ISDIR = _require_intrinsic("molt_path_isdir")
_MOLT_PATH_ISFILE = _require_intrinsic("molt_path_isfile")
_MOLT_PATH_ISLINK = _require_intrinsic("molt_path_islink")
_MOLT_PATH_READLINK = _require_intrinsic("molt_path_readlink")
_MOLT_PATH_SYMLINK = _require_intrinsic("molt_path_symlink")
_MOLT_PATH_MKDIR = _require_intrinsic("molt_path_mkdir")
_MOLT_PATH_UNLINK = _require_intrinsic("molt_path_unlink")
_MOLT_PATH_RMDIR = _require_intrinsic("molt_path_rmdir")
_MOLT_PATH_MAKEDIRS = _require_intrinsic("molt_path_makedirs")
_MOLT_FILE_OPEN_EX = _require_intrinsic("molt_file_open_ex")
_MOLT_PATH_AS_URI = _require_intrinsic("molt_path_as_uri")
_MOLT_PATH_CHMOD = _require_intrinsic("molt_path_chmod")
_MOLT_OS_STAT = _require_intrinsic("molt_os_stat")
_MOLT_OS_LSTAT = _require_intrinsic("molt_os_lstat")
_MOLT_OS_RENAME = _require_intrinsic("molt_os_rename")
_MOLT_OS_REPLACE = _require_intrinsic("molt_os_replace")

# Native parsed-path and filesystem providers.
_MOLT_PATHLIB_SPLITROOT = _require_intrinsic("molt_path_splitroot")
_MOLT_PATHLIB_PARTS = _require_intrinsic("molt_pathlib_parts")
_MOLT_PATHLIB_SAMEFILE = _require_intrinsic("molt_pathlib_samefile")
_MOLT_PATHLIB_OWNER = _require_intrinsic("molt_pathlib_owner")
_MOLT_PATHLIB_GROUP = _require_intrinsic("molt_pathlib_group")
_MOLT_PATHLIB_IS_MOUNT = _require_intrinsic("molt_pathlib_is_mount")
_MOLT_PATHLIB_HARDLINK_TO = _require_intrinsic("molt_pathlib_hardlink_to")
_MOLT_PATHLIB_READ_BYTES = _require_intrinsic("molt_pathlib_read_bytes")
_MOLT_PATHLIB_WRITE_BYTES = _require_intrinsic("molt_pathlib_write_bytes")
_MOLT_PATHLIB_TOUCH = _require_intrinsic("molt_pathlib_touch")
_MOLT_PATHLIB_CWD = _require_intrinsic("molt_pathlib_cwd")
_MOLT_PATHLIB_ITERDIR = _require_intrinsic("molt_pathlib_iterdir")
_MOLT_PATHLIB_RESOLVE = _require_intrinsic("molt_pathlib_resolve")

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


def _parse_path_parts(value, posix):
    """Preserve legacy subclass-produced roots and normalized tail components."""
    separator = "/" if posix else "\\"
    drive, root, relative = _os._path_splitroot(value, posix)
    if not root and drive.startswith(separator) and not drive.endswith(separator):
        drive_parts = drive.split(separator)
        if len(drive_parts) == 4 and drive_parts[2] not in "?.":
            root = separator
        elif len(drive_parts) == 6:
            root = separator
    parts = [_sys.intern(str(part)) for part in relative.split(separator) if part and part != "."]
    return drive, root, parts


class _PathParents:
    __slots__ = ("_path", "_drive", "_root", "_tail")

    def __init__(self, path):
        self._path = path
        self._drive, self._root, self._tail = path._parsed_parts()

    def __len__(self):
        return len(self._tail)

    def __getitem__(self, index):
        if isinstance(index, slice):
            return tuple(self[i] for i in range(*index.indices(len(self))))
        if index >= len(self) or index < -len(self):
            raise IndexError(index)
        if index < 0:
            index += len(self)
        return self._path._from_parsed_parts(
            self._drive, self._root, self._tail[:-index - 1]
        )


class Path:
    __slots__ = ("_raw_paths", "_parsed", "_str")
    _posix_flavor = _os.name != "nt"

    def __init__(self, *args) -> None:
        paths = []
        for arg in args:
            if isinstance(arg, Path):
                if arg._posix_flavor != self._posix_flavor:
                    paths.append(arg.as_posix())
                else:
                    paths.extend(arg._raw_paths)
            else:
                try:
                    path = _os.fspath(arg)
                except TypeError:
                    path = arg
                if not isinstance(path, str):
                    raise TypeError(
                        "argument should be a str or an os.PathLike "
                        "object where __fspath__ returns a str, "
                        f"not {type(path).__name__!r}"
                    )
                paths.append(path)
        self._raw_paths = paths

    def with_segments(self, *pathsegments):
        return type(self)(*pathsegments)

    @classmethod
    def _parse_path(cls, path):
        if not path:
            return "", "", []
        posix = cls._posix_flavor
        if not posix:
            path = path.replace("/", "\\")
        if type(path) is str:
            wide = _WINDOWS_UTF16_ROOT and not posix
            drive, root, _ = _MOLT_PATHLIB_SPLITROOT(path, posix, wide, True)
            parts = _MOLT_PATHLIB_PARTS(path, posix, wide)
            tail = list(parts[1:] if drive or root else parts)
            return drive, root, tail
        return _parse_path_parts(path, posix)

    @property
    def _raw_path(self):
        paths = self._raw_paths
        if not paths:
            return ""
        if len(paths) == 1:
            return paths[0]
        return _os._path_join(paths[0], tuple(paths[1:]), self._posix_flavor)

    def _parsed_parts(self):
        try:
            return self._parsed
        except AttributeError:
            self._parsed = self._parse_path(self._raw_path)
            return self._parsed

    @classmethod
    def _format_parsed_parts(cls, drive, root, tail):
        sep = "/" if cls._posix_flavor else "\\"
        if drive or root:
            return drive + root + sep.join(tail)
        if tail and not cls._posix_flavor and _os._path_splitroot(tail[0], False)[0]:
            tail = ["."] + tail
        return sep.join(tail)

    @property
    def _path(self):
        try:
            return self._str
        except AttributeError:
            self._str = self._format_parsed_parts(*self._parsed_parts()) or "."
            return self._str

    def _from_parsed_string(self, path_str):
        path = self.with_segments(path_str)
        path._str = path_str or "."
        return path

    def _from_parsed_parts(self, drive, root, tail):
        path_str = self._format_parsed_parts(drive, root, tail)
        if not _PYTHON_313:
            path = self.with_segments(path_str)
            path._str = path_str or "."
        else:
            path = self._from_parsed_string(path_str)
        path._parsed = (drive, root, tail)
        return path

    @classmethod
    def cwd(cls) -> Path:
        if not _PYTHON_313:
            return cls().absolute()
        if not _PYTHON_314:
            return cls("").absolute()
        cwd = _MOLT_PATHLIB_CWD()
        path = cls(cwd)
        path._str = cwd
        return path

    @classmethod
    def home(cls) -> Path:
        return cls("~").expanduser()

    def _coerce_part(self, value) -> str:
        return _coerce_fspath(value)

    def __fspath__(self) -> str:
        return str(self)

    def __str__(self) -> str:
        return self._path

    def __bytes__(self) -> bytes:
        return _os.fsencode(self)

    def __repr__(self) -> str:
        return f"{type(self).__name__}({self.as_posix()!r})"

    def _comparison_key(self):
        path = str(self)
        return path if self._posix_flavor else path.lower()

    def __hash__(self) -> int:
        return hash(self._comparison_key())

    def as_posix(self) -> str:
        path = str(self)
        return path if self._posix_flavor else path.replace("\\", "/")

    def as_uri(self) -> str:
        uri = _MOLT_PATH_AS_URI(self._path)
        if not isinstance(uri, str):
            raise RuntimeError("path as_uri intrinsic returned invalid value")
        return uri

    def is_absolute(self) -> bool:
        drive, root, _ = self._parsed_parts()
        return bool(root) if self._posix_flavor else bool(drive and root)

    def absolute(self) -> Path:
        if self.is_absolute():
            return self
        drive, root, tail = self._parsed_parts()
        if _PYTHON_313 and root:
            cwd_drive = _os._path_splitroot(_MOLT_PATHLIB_CWD(), self._posix_flavor)[0]
            return self._from_parsed_parts(cwd_drive, root, tail)
        cwd = _MOLT_PATH_ABSPATH(drive) if drive else _MOLT_PATHLIB_CWD()
        if not _PYTHON_313:
            if not drive and not root and not tail:
                path = self.with_segments(cwd)
                path._str = cwd
                return path
            return self.with_segments(cwd, self)
        if not tail:
            return self._from_parsed_string(cwd)
        cwd_drive, cwd_root, cwd_tail = self._parse_path(cwd)
        return self._from_parsed_parts(cwd_drive, cwd_root, cwd_tail + tail)

    def expanduser(self) -> Path:
        drive, root, tail = self._parsed_parts()
        if not (drive or root) and tail and tail[0][:1] == "~":
            homedir = _os.path.expanduser(tail[0])
            if homedir[:1] == "~":
                raise RuntimeError("Could not determine home directory.")
            drive, root, home_tail = self._parse_path(homedir)
            return self._from_parsed_parts(drive, root, home_tail + tail[1:])
        return self

    def resolve(self, strict: bool = False) -> Path:
        return self.with_segments(_MOLT_PATHLIB_RESOLVE(self))

    def _parts(self) -> list[str]:
        drive, root, tail = self._parsed_parts()
        return [drive + root, *tail] if drive or root else list(tail)

    def _splitroot(self) -> tuple[str, str, str]:
        drive, root, tail = self._parsed_parts()
        sep = "/" if self._posix_flavor else "\\"
        return drive, root, sep.join(tail)

    @property
    def parts(self) -> tuple[str, ...]:
        return tuple(self._parts())

    @property
    def drive(self) -> str:
        return self._parsed_parts()[0]

    @property
    def root(self) -> str:
        return self._parsed_parts()[1]

    @property
    def anchor(self) -> str:
        drive, root, _ = self._parsed_parts()
        return drive + root

    def joinpath(self, *others: str) -> Path:
        return self.with_segments(self, *others)

    def __truediv__(self, key) -> Path:
        try:
            if not _PYTHON_313:
                return self.joinpath(key)
            return self.with_segments(self, key)
        except TypeError:
            return NotImplemented

    def __rtruediv__(self, key) -> Path:
        try:
            return self.with_segments(key, self)
        except TypeError:
            return NotImplemented

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
        if "b" not in mode:
            encoding = _io.text_encoding(encoding)
        return _MOLT_FILE_OPEN_EX(
            self,
            mode,
            buffering,
            encoding,
            errors,
            newline,
            closefd,
            opener,
        )

    if _PYTHON_313:

        def read_text(self, encoding=None, errors=None, newline=None) -> str:
            encoding = _io.text_encoding(encoding)
            with self.open(
                mode="r", encoding=encoding, errors=errors, newline=newline
            ) as stream:
                return stream.read()
    else:

        def read_text(self, encoding=None, errors=None) -> str:
            encoding = _io.text_encoding(encoding)
            with self.open(mode="r", encoding=encoding, errors=errors) as stream:
                return stream.read()

    def read_bytes(self) -> bytes:
        capabilities.require("fs.read")
        return _MOLT_PATHLIB_READ_BYTES(self)

    def write_text(
        self,
        data: str,
        encoding=None,
        errors=None,
        newline=None,
    ) -> int:
        if _PYTHON_313:
            encoding = _io.text_encoding(encoding)
        if not isinstance(data, str):
            raise TypeError("data must be str, not %s" % data.__class__.__name__)
        if not _PYTHON_313:
            encoding = _io.text_encoding(encoding)
        with self.open(
            mode="w", encoding=encoding, errors=errors, newline=newline
        ) as stream:
            return stream.write(data)

    def write_bytes(self, data: bytes) -> int:
        capabilities.require("fs.write")
        view = memoryview(data)
        return int(_MOLT_PATHLIB_WRITE_BYTES(self, view))

    def exists(self) -> bool:
        return bool(_MOLT_PATH_EXISTS(self))

    def is_dir(self) -> bool:
        return bool(_MOLT_PATH_ISDIR(self))

    def is_file(self) -> bool:
        return bool(_MOLT_PATH_ISFILE(self))

    def is_symlink(self) -> bool:
        return bool(_MOLT_PATH_ISLINK(self))

    def is_mount(self) -> bool:
        capabilities.require("fs.read")
        return bool(_MOLT_PATHLIB_IS_MOUNT(self))

    def readlink(self) -> Path:
        value = _MOLT_PATH_READLINK(self)
        if not isinstance(value, str):
            raise RuntimeError("path readlink intrinsic returned invalid value")
        return self.with_segments(value)

    def symlink_to(self, target, target_is_directory: bool = False) -> None:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_PATH_SYMLINK(target_path, self._path, bool(target_is_directory))

    def hardlink_to(self, target) -> None:
        capabilities.require("fs.write")
        target_path = self._coerce_part(target)
        _MOLT_PATHLIB_HARDLINK_TO(self, target_path)

    def unlink(self, missing_ok: bool = False) -> None:
        capabilities.require("fs.write")
        try:
            _MOLT_PATH_UNLINK(self)
        except FileNotFoundError:
            if not missing_ok:
                raise

    def iterdir(self) -> Iterator[Path]:
        capabilities.require("fs.read")
        if not _PYTHON_313:
            names = _MOLT_PATHLIB_ITERDIR(self)
        else:
            root_dir = str(self)
            names = _MOLT_PATHLIB_ITERDIR(root_dir)
        if not isinstance(names, list):
            raise RuntimeError("path iterdir intrinsic returned invalid value")
        for name in names:
            if isinstance(name, str):
                if not _PYTHON_313:
                    name = _os.path.basename(name)
                    path_str = str(self)
                    drive, root, tail = self._parsed_parts()
                    separator = "/" if self._posix_flavor else "\\"
                    if tail:
                        path_str = path_str + separator + name
                    elif path_str != ".":
                        path_str += name
                    else:
                        path_str = name
                    path = self.with_segments(path_str)
                    path._str = path_str
                    path._parsed = (drive, root, tail + [name])
                    yield path
                else:
                    if root_dir == ".":
                        name = name[2:]
                    yield self._from_parsed_string(name)

    def glob(self, pattern: str) -> Iterator[Path]:
        capabilities.require("fs.read")
        legacy_parsed = None
        if not _PYTHON_313:
            if not pattern:
                raise ValueError("Unacceptable pattern: {!r}".format(pattern))
            # _parse_path normalizes Windows separators before splitroot/fspath.
            value = ""
            if pattern:
                parsed = pattern.replace("/", "\\") if _os.name == "nt" else pattern
                value = _os.fspath(parsed)
                if type(value) not in (str, bytes):
                    legacy_parsed = _parse_path_parts(value, _os.name != "nt")
            original = pattern
        else:
            if not _PYTHON_314:
                if not isinstance(pattern, Path):
                    pattern = self.with_segments(pattern)
                original = pattern._raw_path
            else:
                original = _os.fspath(pattern)
            value = original
            if not _PYTHON_314:
                # PurePath._parse_path observes raw str truth on both platforms.
                if not value:
                    value = ""
                elif _os.name == "nt":
                    value = _os.fspath(value.replace("/", "\\"))
        # 3.12 observes the caller object; 3.13 observes the converted raw str.
        # Both tail callbacks follow anchor admission; 3.14 uses native parts.
        names = _MOLT_PATH_GLOB(self, value, False, original, legacy_parsed)
        for name in names:
            if not name and not _PYTHON_313:
                yield self
            else:
                drive, root, tail = self._parsed_parts()
                separator = "/" if self._posix_flavor else "\\"
                parts = tail + (name.split(separator) if name else [])
                if not _PYTHON_313:
                    yield self._from_parsed_parts(drive, root, parts)
                else:
                    yield self._from_parsed_string(
                        self._format_parsed_parts(drive, root, parts)
                    )

    if _PYTHON_313:

        def rglob(self, pattern: str) -> Iterator[Path]:
            # Admit before fspath callbacks; glob still owns modern dispatch.
            capabilities.require("fs.read")
            # Newer CPython versions intentionally dispatch through glob.
            if not _PYTHON_314:
                if not isinstance(pattern, Path):
                    pattern = self.with_segments(pattern)
                return self.glob("**" / pattern)
            return self.glob(_os._path_join("**", (pattern,), self._posix_flavor))
    else:

        def rglob(self, pattern: str) -> Iterator[Path]:
            capabilities.require("fs.read")
            value = ""
            legacy_parsed = None
            if pattern:
                parsed = pattern.replace("/", "\\") if _os.name == "nt" else pattern
                value = _os.fspath(parsed)
                if type(value) not in (str, bytes):
                    legacy_parsed = _parse_path_parts(value, _os.name != "nt")
            names = _MOLT_PATH_GLOB(self, value, True, pattern, legacy_parsed)
            for name in names:
                if not name:
                    yield self
                else:
                    drive, root, tail = self._parsed_parts()
                    separator = "/" if self._posix_flavor else "\\"
                    yield self._from_parsed_parts(drive, root, tail + name.split(separator))

    def mkdir(
        self,
        mode: int = 0o777,
        parents: bool = False,
        exist_ok: bool = False,
    ) -> None:
        if parents:
            _MOLT_PATH_MAKEDIRS(self, mode, bool(exist_ok))
            return
        try:
            _MOLT_PATH_MKDIR(self, mode)
        except FileExistsError:
            if exist_ok and bool(_MOLT_PATH_ISDIR(self)):
                return
            raise

    def rmdir(self) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_RMDIR(self)

    def stat(self) -> _os.stat_result:
        return _MOLT_OS_STAT(self)

    def lstat(self) -> _os.stat_result:
        return _MOLT_OS_LSTAT(self)

    def touch(self, mode: int = 0o666, exist_ok: bool = True) -> None:
        capabilities.require("fs.write")
        _MOLT_PATHLIB_TOUCH(self, exist_ok)

    def rename(self, target) -> Path:
        capabilities.require("fs.write")
        _MOLT_OS_RENAME(self, target)
        return self.with_segments(target)

    def replace(self, target) -> Path:
        capabilities.require("fs.write")
        _MOLT_OS_REPLACE(self, target)
        return self.with_segments(target)

    def chmod(self, mode: int) -> None:
        capabilities.require("fs.write")
        _MOLT_PATH_CHMOD(self, int(mode))

    def owner(self) -> str:
        capabilities.require("fs.read")
        return str(_MOLT_PATHLIB_OWNER(self))

    def group(self) -> str:
        capabilities.require("fs.read")
        return str(_MOLT_PATHLIB_GROUP(self))

    def samefile(self, other_path) -> bool:
        capabilities.require("fs.read")
        other = self._coerce_part(other_path)
        return bool(_MOLT_PATHLIB_SAMEFILE(self, other))

    @property
    def name(self) -> str:
        tail = self._parsed_parts()[2]
        return tail[-1] if tail else ""

    @property
    def suffix(self) -> str:
        name = self.name
        if _PYTHON_314:
            name = name.lstrip(".")
            index = name.rfind(".")
            return name[index:] if index >= 0 else ""
        index = name.rfind(".")
        return name[index:] if 0 < index < len(name) - 1 else ""

    @property
    def suffixes(self) -> list[str]:
        name = self.name
        if not _PYTHON_314 and name.endswith("."):
            return []
        return ["." + suffix for suffix in name.lstrip(".").split(".")[1:]]

    @property
    def stem(self) -> str:
        name = self.name
        index = name.rfind(".")
        if _PYTHON_314:
            if index >= 0 and name[:index].lstrip("."):
                return name[:index]
        elif 0 < index < len(name) - 1:
            return name[:index]
        return name

    @property
    def parent(self) -> Path:
        drive, root, tail = self._parsed_parts()
        if not tail:
            return self
        return self._from_parsed_parts(drive, root, tail[:-1])

    @property
    def parents(self):
        return _PathParents(self)

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, Path):
            return NotImplemented
        return (
            self._comparison_key() == other._comparison_key()
            and self._posix_flavor == other._posix_flavor
        )

    def _parts_normcase(self):
        separator = "/" if self._posix_flavor else "\\"
        return self._comparison_key().split(separator)

    def __lt__(self, other: object) -> bool:
        if not isinstance(other, Path) or self._posix_flavor != other._posix_flavor:
            return NotImplemented
        return self._parts_normcase() < other._parts_normcase()

    def __le__(self, other: object) -> bool:
        if not isinstance(other, Path) or self._posix_flavor != other._posix_flavor:
            return NotImplemented
        return self._parts_normcase() <= other._parts_normcase()

    def __gt__(self, other: object) -> bool:
        if not isinstance(other, Path) or self._posix_flavor != other._posix_flavor:
            return NotImplemented
        return self._parts_normcase() > other._parts_normcase()

    def __ge__(self, other: object) -> bool:
        if not isinstance(other, Path) or self._posix_flavor != other._posix_flavor:
            return NotImplemented
        return self._parts_normcase() >= other._parts_normcase()

    def relative_to(self, other, /, *_deprecated, walk_up=False) -> Path:
        if _deprecated:
            if _PYTHON_314:
                raise TypeError("relative_to() takes 2 positional arguments")
            _warnings.warn(
                "support for supplying more than one positional argument "
                "to pathlib.PurePath.relative_to() is deprecated and "
                "scheduled for removal in Python 3.14",
                DeprecationWarning, stacklevel=2,
            )
        if (
            not _PYTHON_313 or _deprecated
            or (not _PYTHON_314 and not isinstance(other, Path))
            or (_PYTHON_314 and not hasattr(other, "with_segments"))
        ):
            other = self.with_segments(other, *_deprecated)
        candidates = (
            [other] + list(other.parents) if not _PYTHON_313
            else _chain([other], other.parents)
        )
        for step, path in enumerate(candidates):
            contained = (
                self.is_relative_to(path) if not _PYTHON_313
                else path == self or path in self.parents
            )
            if contained:
                break
            if not walk_up:
                raise ValueError(f"{str(self)!r} is not in the subpath of {str(other)!r}")
            if path.name == "..":
                raise ValueError(f"'..' segment in {str(other)!r} cannot be walked")
        else:
            raise ValueError(f"{str(self)!r} and {str(other)!r} have different anchors")
        parts = [".."] * step + self._parsed_parts()[2][len(path._parsed_parts()[2]):]
        if not _PYTHON_313:
            return self.with_segments(*parts)
        return self._from_parsed_parts("", "", parts)

    def is_relative_to(self, other, /, *_deprecated) -> bool:
        if _deprecated:
            if _PYTHON_314:
                raise TypeError("is_relative_to() takes 2 positional arguments")
            _warnings.warn(
                "support for supplying more than one argument to "
                "pathlib.PurePath.is_relative_to() is deprecated and "
                "scheduled for removal in Python 3.14",
                DeprecationWarning, stacklevel=2,
            )
        if (
            not _PYTHON_313 or _deprecated
            or (not _PYTHON_314 and not isinstance(other, Path))
            or (_PYTHON_314 and not hasattr(other, "with_segments"))
        ):
            other = self.with_segments(other, *_deprecated)
        return other == self or other in self.parents

    def match(self, pattern: str) -> bool:
        return bool(_MOLT_PATH_MATCH(self._path, str(pattern)))

    def with_name(self, name: str) -> Path:
        if not _PYTHON_313 and not self.name:
            raise ValueError(f"{self!r} has an empty name")
        separator = "/" if self._posix_flavor else "\\"
        if (
            not name or separator in name
            or (not self._posix_flavor and "/" in name) or name == "."
        ):
            raise ValueError(f"Invalid name {name!r}")
        drive, root, tail = self._parsed_parts()
        if not tail:
            raise ValueError(f"{self!r} has an empty name")
        return self._from_parsed_parts(drive, root, tail[:-1] + [name])

    def with_suffix(self, suffix: str) -> Path:
        if _PYTHON_313:
            stem = self.stem
            if not stem:
                raise ValueError(f"{self!r} has an empty name")
            if suffix and (
                not suffix.startswith(".")
                or (not _PYTHON_314 and len(suffix) <= 1)
            ):
                raise ValueError(f"Invalid suffix {suffix!r}")
            return self.with_name(stem + suffix)
        separator = "/" if self._posix_flavor else "\\"
        if separator in suffix or (not self._posix_flavor and "/" in suffix):
            raise ValueError(f"Invalid suffix {suffix!r}")
        if suffix and not suffix.startswith(".") or suffix == ".":
            raise ValueError(f"Invalid suffix {suffix!r}")
        name = self.name
        if not name:
            raise ValueError(f"{self!r} has an empty name")
        previous = self.suffix
        name = name[:-len(previous)] + suffix if previous else name + suffix
        drive, root, tail = self._parsed_parts()
        return self._from_parsed_parts(drive, root, tail[:-1] + [name])

    def with_stem(self, stem: str) -> Path:
        suffix = self.suffix
        if not _PYTHON_313:
            return self.with_name(stem + suffix)
        if not suffix:
            return self.with_name(stem)
        if not stem:
            raise ValueError(f"{self!r} has a non-empty suffix")
        return self.with_name(stem + suffix)

    def walk(
        self,
        top_down: bool = True,
        on_error=None,
        follow_symlinks: bool = False,
    ) -> Iterator[tuple[Path, list[str], list[str]]]:
        capabilities.require("fs.read")
        root_dir = str(self)
        for dirpath, dirnames, filenames in _os.walk(
            root_dir,
            topdown=top_down,
            onerror=on_error,
            followlinks=follow_symlinks,
        ):
            if not _PYTHON_313 and dirpath == root_dir:
                path = self
            else:
                if root_dir == ".":
                    dirpath = dirpath[2:]
                path = self._from_parsed_string(dirpath)
            yield path, dirnames, filenames


class PurePosixPath(Path):
    __slots__ = ()
    _posix_flavor = True


PurePath = Path


class PureWindowsPath(Path):
    __slots__ = ()
    _posix_flavor = False


globals().pop("_require_intrinsic", None)
