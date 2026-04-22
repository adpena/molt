"""Shared helpers for importlib.resources."""

from _intrinsics import require_intrinsic as _require_intrinsic

_require_intrinsic("molt_stdlib_probe")
_MOLT_IMPORTLIB_IMPORT_REQUIRED = _require_intrinsic("molt_importlib_import_required")

import functools
import importlib  # noqa: F401
import inspect
import itertools
import contextlib  # noqa: F401
import os  # noqa: F401
import pathlib  # noqa: F401
import tempfile  # noqa: F401
import types
import warnings
from typing import Optional, Union, cast

from .abc import ResourceReader, Traversable
from ._adapters import wrap_spec  # noqa: F401
from . import as_file as _resources_as_file
from . import files as _resources_files

Package = Union[types.ModuleType, str]
Anchor = Package


def package_to_anchor(func):
    undefined = object()

    @functools.wraps(func)
    def wrapper(anchor=undefined, package=undefined):
        if package is not undefined:
            if anchor is not undefined:
                return func(anchor, package)
            warnings.warn(
                "First parameter to files is renamed to 'anchor'",
                DeprecationWarning,
                stacklevel=2,
            )
            return func(package)
        if anchor is undefined:
            return func()
        return func(anchor)

    return wrapper


@package_to_anchor
def files(anchor: Optional[Anchor] = None) -> Traversable:
    return from_package(resolve(anchor))


def get_resource_reader(package: types.ModuleType) -> Optional[ResourceReader]:
    spec = package.__spec__
    reader = getattr(spec.loader, "get_resource_reader", None)  # type: ignore[attr-defined]
    if reader is None:
        return None
    return reader(spec.name)  # type: ignore[misc]


def resolve(cand: Optional[Anchor]) -> types.ModuleType:
    if isinstance(cand, str):
        return cast(types.ModuleType, _MOLT_IMPORTLIB_IMPORT_REQUIRED(cand))
    if cand is None:
        return resolve(_infer_caller().f_globals["__name__"])
    return cast(types.ModuleType, cand)


def _infer_caller():
    def is_this_file(frame_info):
        return frame_info.filename == stack[0].filename

    def is_wrapper(frame_info):
        return frame_info.function == "wrapper"

    stack = inspect.stack()
    not_this_file = itertools.filterfalse(is_this_file, stack)
    callers = itertools.filterfalse(is_wrapper, not_this_file)
    return next(callers).frame


def from_package(package: types.ModuleType):
    return _resources_files(package)


def as_file(path):
    return _resources_as_file(path)


globals().pop("_require_intrinsic", None)
