"""Abstract classes and protocols for importlib.resources."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import abc
import io
import itertools
import os
import pathlib
from typing import Any, BinaryIO, Iterable, Iterator, NoReturn, Optional, Protocol
from typing import Union
from typing import runtime_checkable

StrPath = Union[str, os.PathLike]

__all__ = ["ResourceReader", "Traversable", "TraversableResources"]


class ResourceReader(metaclass=abc.ABCMeta):
    """Abstract base class for loaders to provide resource reading support."""

    @abc.abstractmethod
    def open_resource(self, resource: str) -> BinaryIO:
        raise FileNotFoundError

    @abc.abstractmethod
    def resource_path(self, resource: str) -> str:
        raise FileNotFoundError

    @abc.abstractmethod
    def is_resource(self, path: str) -> bool:
        raise FileNotFoundError

    @abc.abstractmethod
    def contents(self) -> Iterable[str]:
        raise FileNotFoundError


class TraversalError(Exception):
    pass


@runtime_checkable
class Traversable(Protocol):
    @abc.abstractmethod
    def iterdir(self) -> Iterator["Traversable"]:
        """Yield traversable children."""

    def read_bytes(self) -> bytes:
        with self.open("rb") as stream:
            return stream.read()

    def read_text(self, encoding: Optional[str] = None) -> str:
        with self.open(encoding=encoding) as stream:
            return stream.read()

    @abc.abstractmethod
    def is_dir(self) -> bool:
        """True when this entry is a directory."""

    @abc.abstractmethod
    def is_file(self) -> bool:
        """True when this entry is a file."""

    def joinpath(self, *descendants: StrPath) -> "Traversable":
        if not descendants:
            return self
        names = itertools.chain.from_iterable(
            path.parts for path in map(pathlib.PurePosixPath, descendants)
        )
        target = next(names)
        matches = (
            traversable for traversable in self.iterdir() if traversable.name == target
        )
        try:
            match = next(matches)
        except StopIteration:
            raise TraversalError(
                "Target not found during traversal.", target, list(names)
            )
        return match.joinpath(*names)

    def __truediv__(self, child: StrPath) -> "Traversable":
        return self.joinpath(child)

    @abc.abstractmethod
    def open(self, mode="r", *args, **kwargs):
        """Open this resource in text ('r') or binary ('rb') mode."""

    @property
    @abc.abstractmethod
    def name(self) -> str:
        """Base name of the traversable entry."""


class TraversableResources(ResourceReader):
    """ResourceReader variant that provides a traversable tree."""

    @abc.abstractmethod
    def files(self) -> "Traversable":
        """Return a Traversable object for the loaded package."""

    def open_resource(self, resource: StrPath) -> io.BufferedReader:
        return self.files().joinpath(resource).open("rb")

    def resource_path(self, resource: Any) -> NoReturn:
        raise FileNotFoundError(resource)

    def is_resource(self, path: StrPath) -> bool:
        return self.files().joinpath(path).is_file()

    def contents(self) -> Iterator[str]:
        return (item.name for item in self.files().iterdir())
