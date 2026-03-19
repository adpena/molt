"""Legacy importlib.resources functional API."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_RESOURCES_NORMALIZE_PATH = _require_intrinsic(
    "molt_importlib_resources_normalize_path"
)

import functools  # noqa: F401
import os  # noqa: F401
import pathlib
import types
import warnings  # noqa: F401
from typing import Any, BinaryIO, ContextManager, Iterable, TextIO, Union

from . import contents as _resources_contents
from . import is_resource as _resources_is_resource
from . import open_binary as _resources_open_binary
from . import open_text as _resources_open_text
from . import path as _resources_path
from . import read_binary as _resources_read_binary
from . import read_text as _resources_read_text

Package = Union[types.ModuleType, str]
Resource = str


def normalize_path(path: Any) -> str:
    return _MOLT_IMPORTLIB_RESOURCES_NORMALIZE_PATH(path)


def open_binary(package: Package, resource: Resource) -> BinaryIO:
    return _resources_open_binary(package, normalize_path(resource))


def read_binary(package: Package, resource: Resource) -> bytes:
    return _resources_read_binary(package, normalize_path(resource))


def open_text(
    package: Package,
    resource: Resource,
    encoding: str = "utf-8",
    errors: str = "strict",
) -> TextIO:
    return _resources_open_text(
        package, normalize_path(resource), encoding=encoding, errors=errors
    )


def read_text(
    package: Package,
    resource: Resource,
    encoding: str = "utf-8",
    errors: str = "strict",
) -> str:
    return _resources_read_text(
        package, normalize_path(resource), encoding=encoding, errors=errors
    )


def contents(package: Package) -> Iterable[str]:
    return _resources_contents(package)


def is_resource(package: Package, name: str) -> bool:
    return _resources_is_resource(package, normalize_path(name))


def path(package: Package, resource: Resource) -> ContextManager[pathlib.Path]:
    return _resources_path(package, normalize_path(resource))
