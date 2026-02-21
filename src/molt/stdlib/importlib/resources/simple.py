"""Interface adapters for low-level resource readers."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe", globals())

import abc
import io
import itertools
from typing import BinaryIO, List

from .abc import Traversable, TraversableResources


class SimpleReader(abc.ABC):
    @property
    @abc.abstractmethod
    def package(self) -> str:
        """Package name for this reader."""

    @abc.abstractmethod
    def children(self) -> List["SimpleReader"]:
        """Child resource containers."""

    @abc.abstractmethod
    def resources(self) -> List[str]:
        """Resource names available at this container."""

    @abc.abstractmethod
    def open_binary(self, resource: str) -> BinaryIO:
        """Return an opened binary handle for the resource."""

    @property
    def name(self):
        return self.package.split(".")[-1]


class ResourceContainer(Traversable):
    def __init__(self, reader: SimpleReader):
        self.reader = reader

    def is_dir(self):
        return True

    def is_file(self):
        return False

    def iterdir(self):
        files = (ResourceHandle(self, name) for name in self.reader.resources())
        directories = map(ResourceContainer, self.reader.children())
        return itertools.chain(files, directories)

    def open(self, *args, **kwargs):
        raise IsADirectoryError()


class ResourceHandle(Traversable):
    def __init__(self, parent: ResourceContainer, name: str):
        self.parent = parent
        self.name = name  # type: ignore[assignment]

    def is_file(self):
        return True

    def is_dir(self):
        return False

    def open(self, mode="r", *args, **kwargs):
        stream = self.parent.reader.open_binary(self.name)
        if "b" not in mode:
            stream = io.TextIOWrapper(stream, *args, **kwargs)
        return stream

    def joinpath(self, name):
        raise RuntimeError("Cannot traverse into a resource")


class TraversableReader(TraversableResources, SimpleReader):
    def files(self):
        return ResourceContainer(self)
