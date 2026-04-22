"""Reader implementations for importlib.resources."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_RESOURCES_ONLY = _require_intrinsic("molt_importlib_resources_only")

import collections
import itertools
import operator
import pathlib
import zipfile

from . import abc


def only(iterable, default=None, too_long=None):
    return _MOLT_IMPORTLIB_RESOURCES_ONLY(iterable, default, too_long)


def remove_duplicates(items):
    return iter(collections.OrderedDict.fromkeys(items))


class FileReader(abc.TraversableResources):
    def __init__(self, loader):
        self.path = pathlib.Path(loader.path).parent

    def resource_path(self, resource):
        return str(self.path.joinpath(resource))

    def files(self):
        return self.path


class ZipReader(abc.TraversableResources):
    def __init__(self, loader, module):
        self.prefix = loader.prefix.replace("\\", "/")
        if loader.is_package(module):
            _, _, name = module.rpartition(".")
            self.prefix += name + "/"
        self.archive = loader.archive

    def open_resource(self, resource):
        try:
            return super().open_resource(resource)
        except KeyError as exc:
            raise FileNotFoundError(exc.args[0])

    def is_resource(self, path):
        target = self.files().joinpath(path)
        return target.is_file() and target.exists()

    def files(self):
        return zipfile.Path(self.archive, self.prefix)


class MultiplexedPath(abc.Traversable):
    def __init__(self, *paths):
        self._paths = list(map(pathlib.Path, remove_duplicates(paths)))
        if not self._paths:
            raise FileNotFoundError("MultiplexedPath must contain at least one path")
        if not all(path.is_dir() for path in self._paths):
            raise NotADirectoryError("MultiplexedPath only supports directories")

    def iterdir(self):
        children = (child for path in self._paths for child in path.iterdir())
        by_name = operator.attrgetter("name")
        groups = itertools.groupby(sorted(children, key=by_name), key=by_name)
        return map(self._follow, (locations for _, locations in groups))

    def read_bytes(self):
        raise FileNotFoundError(f"{self} is not a file")

    def read_text(self, *args, **kwargs):
        raise FileNotFoundError(f"{self} is not a file")

    def is_dir(self):
        return True

    def is_file(self):
        return False

    def joinpath(self, *descendants):
        try:
            return super().joinpath(*descendants)
        except abc.TraversalError:
            return self._paths[0].joinpath(*descendants)

    @classmethod
    def _follow(cls, children):
        subdirs, one_dir, one_file = itertools.tee(children, 3)
        try:
            return _MOLT_IMPORTLIB_RESOURCES_ONLY(one_dir)
        except ValueError:
            try:
                return cls(*subdirs)
            except NotADirectoryError:
                return next(one_file)

    def open(self, *args, **kwargs):
        raise FileNotFoundError(f"{self} is not a file")

    @property
    def name(self):
        return self._paths[0].name

    def __repr__(self):
        paths = ", ".join(f"{path!r}" for path in self._paths)
        return f"MultiplexedPath({paths})"


class NamespaceReader(abc.TraversableResources):
    def __init__(self, namespace_path):
        if "NamespacePath" not in str(namespace_path):
            raise ValueError("Invalid path")
        self.path = MultiplexedPath(*list(namespace_path))

    def resource_path(self, resource):
        return str(self.path.joinpath(resource))

    def files(self):
        return self.path


globals().pop("_require_intrinsic", None)
