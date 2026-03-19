"""Path-like interface for ``zipfile`` archives."""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic

import contextlib
import importlib as _importlib
import io
import pathlib
import posixpath
import re
import sys as _sys

from . import glob

_require_intrinsic("molt_capabilities_has")
_MOLT_ZIPFILE_PATH_IMPLIED_DIRS = _require_intrinsic(
    "molt_zipfile_path_implied_dirs"
)
_MOLT_ZIPFILE_PATH_RESOLVE_DIR = _require_intrinsic(
    "molt_zipfile_path_resolve_dir"
)
_MOLT_ZIPFILE_PATH_IS_CHILD = _require_intrinsic(
    "molt_zipfile_path_is_child"
)


if _sys.version_info >= (3, 13):
    Translator = glob.Translator
    import stat as _stat

    sys = _sys

    def _is_symlink_mode(mode: int) -> bool:
        return _stat.S_ISLNK(mode)
else:
    translate = glob.translate


_PARENT_PACKAGE = __name__.rpartition(".")[0]
zipfile = _importlib.import_module(_PARENT_PACKAGE)

__all__ = ["Path"]


class InitializedState:
    def __init__(self, *args, **kwargs):
        self.__args = args
        self.__kwargs = kwargs
        super().__init__(*args, **kwargs)

    def __getstate__(self):
        return self.__args, self.__kwargs

    def __setstate__(self, state):
        args, kwargs = state
        super().__init__(*args, **kwargs)


class CompleteDirs(InitializedState, zipfile.ZipFile):
    @staticmethod
    def _implied_dirs(names):
        return list(_MOLT_ZIPFILE_PATH_IMPLIED_DIRS(names))

    def namelist(self):
        names = super().namelist()
        return names + list(self._implied_dirs(names))

    def _name_set(self):
        return set(self.namelist())

    def resolve_dir(self, name: str) -> str:
        return _MOLT_ZIPFILE_PATH_RESOLVE_DIR(name, self._name_set())

    def getinfo(self, name: str):
        try:
            return super().getinfo(name)
        except (AttributeError, KeyError):
            if not name.endswith("/") or name not in self._name_set():
                raise
            return zipfile.ZipInfo(filename=name)

    @classmethod
    def make(cls, source):
        if isinstance(source, CompleteDirs):
            return source

        if not isinstance(source, zipfile.ZipFile):
            return cls(source)

        if "r" not in source.mode:
            cls = CompleteDirs

        source.__class__ = cls
        return source

    if _sys.version_info >= (3, 13):

        @classmethod
        def inject(cls, zf):
            for name in cls._implied_dirs(zf.namelist()):
                zf.writestr(name, b"")
            return zf


class FastLookup(CompleteDirs):
    def namelist(self):
        with contextlib.suppress(AttributeError):
            return self.__names
        self.__names = super().namelist()
        return self.__names

    def _name_set(self):
        with contextlib.suppress(AttributeError):
            return self.__lookup
        self.__lookup = super()._name_set()
        return self.__lookup


def _extract_text_encoding(encoding=None, *args, **kwargs):
    if _sys.version_info >= (3, 13):
        is_pypy = _sys.implementation.name == "pypy"
        is_old_pypy = is_pypy and _sys.pypy_version_info < (7, 3, 19)
        stack_level = 3 + is_old_pypy
        return io.text_encoding(encoding, stack_level), args, kwargs
    return io.text_encoding(encoding, 3), args, kwargs


class Path:
    __repr = "{self.__class__.__name__}({self.root.filename!r}, {self.at!r})"

    def __init__(self, root, at: str = ""):
        self.root = FastLookup.make(root)
        self.at = at

    def __eq__(self, other):
        if self.__class__ is not other.__class__:
            return NotImplemented
        return (self.root, self.at) == (other.root, other.at)

    def __hash__(self):
        return hash((self.root, self.at))

    def open(self, mode: str = "r", *args, pwd=None, **kwargs):
        if self.is_dir():
            raise IsADirectoryError(self)

        zip_mode = mode[0]
        if zip_mode == "r" and not self.exists():
            raise FileNotFoundError(self)

        if hasattr(self.root, "open"):
            stream = self.root.open(self.at, zip_mode, pwd=pwd)
        elif zip_mode == "r":
            stream = io.BytesIO(self.root.read(self.at))
        else:
            raise NotImplementedError("zipfile.Path write mode requires zipfile.open")

        if "b" in mode:
            if args or kwargs:
                raise ValueError("encoding args invalid for binary operation")
            return stream

        encoding, args, kwargs = _extract_text_encoding(*args, **kwargs)
        return io.TextIOWrapper(stream, encoding, *args, **kwargs)

    def _base(self):
        if _sys.version_info >= (3, 13):
            return pathlib.PurePosixPath(self.at) if self.at else self.filename
        return pathlib.PurePosixPath(self.at or self.root.filename)

    @property
    def name(self):
        return self._base().name

    @property
    def suffix(self):
        return self._base().suffix

    @property
    def suffixes(self):
        return self._base().suffixes

    @property
    def stem(self):
        return self._base().stem

    @property
    def filename(self):
        return pathlib.Path(self.root.filename).joinpath(self.at)

    def read_text(self, *args, **kwargs):
        encoding, args, kwargs = _extract_text_encoding(*args, **kwargs)
        with self.open("r", encoding, *args, **kwargs) as stream:
            return stream.read()

    def read_bytes(self):
        with self.open("rb") as stream:
            return stream.read()

    def _is_child(self, path) -> bool:
        return bool(_MOLT_ZIPFILE_PATH_IS_CHILD(path.at, self.at))

    def _next(self, at: str):
        return self.__class__(self.root, at)

    def is_dir(self) -> bool:
        return not self.at or self.at.endswith("/")

    def is_file(self) -> bool:
        return self.exists() and not self.is_dir()

    def exists(self) -> bool:
        return self.at in self.root._name_set()

    def iterdir(self):
        if not self.is_dir():
            raise ValueError("Can't listdir a file")
        subs = map(self._next, self.root.namelist())
        return filter(self._is_child, subs)

    def match(self, path_pattern: str) -> bool:
        return pathlib.PurePosixPath(self.at).match(path_pattern)

    def is_symlink(self) -> bool:
        if _sys.version_info < (3, 13):
            return False
        try:
            info = self.root.getinfo(self.at)
        except (AttributeError, KeyError):
            return False
        mode = info.external_attr >> 16
        return _is_symlink_mode(mode)

    def glob(self, pattern: str):
        if not pattern:
            raise ValueError(f"Unacceptable pattern: {pattern!r}")

        prefix = re.escape(self.at)
        if _sys.version_info >= (3, 13):
            translator = Translator(seps="/")
            matcher = re.compile(prefix + translator.translate(pattern)).fullmatch
        else:
            matcher = re.compile(prefix + translate(pattern)).fullmatch
        return map(self._next, filter(matcher, self.root.namelist()))

    def rglob(self, pattern: str):
        return self.glob(f"**/{pattern}")

    def relative_to(self, other, *extra):
        return posixpath.relpath(str(self), str(other.joinpath(*extra)))

    def __str__(self):
        return posixpath.join(self.root.filename, self.at)

    def __repr__(self):
        return self.__repr.format(self=self)

    def joinpath(self, *other):
        next_path = posixpath.join(self.at, *other)
        return self._next(self.root.resolve_dir(next_path))

    __truediv__ = joinpath

    @property
    def parent(self):
        if not self.at:
            return self.filename.parent
        parent_at = posixpath.dirname(self.at.rstrip("/"))
        if parent_at:
            parent_at += "/"
        return self._next(parent_at)
