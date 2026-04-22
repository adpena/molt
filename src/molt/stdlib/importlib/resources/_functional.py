"""Simplified function-based API for importlib.resources."""

from _intrinsics import require_intrinsic as _require_intrinsic

import io as _io
import sys as _sys
import warnings

from . import as_file, files

_require_intrinsic("molt_stdlib_probe")
_MOLT_OPEN_BYTES_FROM_PACKAGE_PARTS = _require_intrinsic(
    "molt_importlib_resources_open_resource_bytes_from_package_parts"
)
_MOLT_READ_TEXT_FROM_PACKAGE_PARTS = _require_intrinsic(
    "molt_importlib_resources_read_text_from_package_parts"
)
_MOLT_CONTENTS_FROM_PACKAGE_PARTS = _require_intrinsic(
    "molt_importlib_resources_contents_from_package_parts"
)
_MOLT_IS_RESOURCE_FROM_PACKAGE_PARTS = _require_intrinsic(
    "molt_importlib_resources_is_resource_from_package_parts"
)
_MOLT_RESOURCE_PATH_FROM_PACKAGE_PARTS = _require_intrinsic(
    "molt_importlib_resources_resource_path_from_package_parts"
)

_MISSING = object()


def _search_paths() -> tuple[str, ...]:
    return tuple(_sys.path)


def _resources_module_file() -> str | None:
    module_file = globals().get("__file__")
    if isinstance(module_file, str) and module_file:
        return module_file
    return None


def _package_bytes(anchor: str, path_names: tuple[str, ...]) -> bytes:
    value = _MOLT_OPEN_BYTES_FROM_PACKAGE_PARTS(
        anchor, _search_paths(), _resources_module_file(), path_names
    )
    if isinstance(value, bytes):
        return value
    if isinstance(value, bytearray):
        return bytes(value)
    raise RuntimeError("invalid importlib resources open payload")


def _package_text(
    anchor: str, path_names: tuple[str, ...], *, encoding: str, errors: str
) -> str:
    value = _MOLT_READ_TEXT_FROM_PACKAGE_PARTS(
        anchor,
        _search_paths(),
        _resources_module_file(),
        path_names,
        encoding,
        errors,
    )
    if not isinstance(value, str):
        raise RuntimeError("invalid importlib resources text payload")
    return value


def _package_contents(anchor: str, path_names: tuple[str, ...]) -> list[str]:
    value = _MOLT_CONTENTS_FROM_PACKAGE_PARTS(
        anchor, _search_paths(), _resources_module_file(), path_names
    )
    if not isinstance(value, (list, tuple)) or not all(
        isinstance(entry, str) for entry in value
    ):
        raise RuntimeError("invalid importlib resources contents payload")
    return list(value)


def _get_encoding_arg(path_names: tuple[str, ...], encoding):
    # For compatibility with versions where *encoding* was positional, require
    # explicit encoding with multiple path components.
    if encoding is _MISSING:
        if len(path_names) > 1:
            raise TypeError("'encoding' argument required with multiple path names")
        return "utf-8"
    return encoding


def _get_resource(anchor, path_names: tuple[str, ...]):
    if anchor is None:
        raise TypeError("anchor must be module or string, got None")
    return files(anchor).joinpath(*path_names)


def open_binary(anchor, *path_names):
    """Open for binary reading the *resource* within *package*."""
    if isinstance(anchor, str):
        return _io.BytesIO(_package_bytes(anchor, path_names))
    return _get_resource(anchor, path_names).open("rb")


def open_text(anchor, *path_names, encoding=_MISSING, errors="strict"):
    """Open for text reading the *resource* within *package*."""
    encoding = _get_encoding_arg(path_names, encoding)
    if isinstance(anchor, str):
        return _io.StringIO(
            _package_text(anchor, path_names, encoding=encoding, errors=errors)
        )
    resource = _get_resource(anchor, path_names)
    return resource.open("r", encoding=encoding, errors=errors)


def read_binary(anchor, *path_names):
    """Read and return contents of *resource* within *package* as bytes."""
    if isinstance(anchor, str):
        return _package_bytes(anchor, path_names)
    return _get_resource(anchor, path_names).read_bytes()


def read_text(anchor, *path_names, encoding=_MISSING, errors="strict"):
    """Read and return contents of *resource* within *package* as str."""
    encoding = _get_encoding_arg(path_names, encoding)
    if isinstance(anchor, str):
        return _package_text(anchor, path_names, encoding=encoding, errors=errors)
    resource = _get_resource(anchor, path_names)
    return resource.read_text(encoding=encoding, errors=errors)


def path(anchor, *path_names):
    """Return the path to the *resource* as an actual file system path."""
    if isinstance(anchor, str):
        resource_path = _MOLT_RESOURCE_PATH_FROM_PACKAGE_PARTS(
            anchor, _search_paths(), _resources_module_file(), path_names
        )
        if isinstance(resource_path, str):
            return as_file(resource_path)
    return as_file(_get_resource(anchor, path_names))


def is_resource(anchor, *path_names):
    """Return ``True`` when *path_names* resolves to a file resource."""
    if isinstance(anchor, str):
        value = _MOLT_IS_RESOURCE_FROM_PACKAGE_PARTS(
            anchor, _search_paths(), _resources_module_file(), path_names
        )
        if not isinstance(value, bool):
            raise RuntimeError("invalid importlib resources is_resource payload")
        return value
    return _get_resource(anchor, path_names).is_file()


def contents(anchor, *path_names):
    """Return an iterable over the named resources within the package."""
    warnings.warn(
        "importlib.resources.contents is deprecated. "
        "Use files(anchor).iterdir() instead.",
        DeprecationWarning,
        stacklevel=1,
    )
    if isinstance(anchor, str):
        return (name for name in _package_contents(anchor, path_names))
    return (resource.name for resource in _get_resource(anchor, path_names).iterdir())


globals().pop("_require_intrinsic", None)
