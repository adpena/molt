"""Shared exact-value primitives for Python custody authorities."""

from __future__ import annotations

import re
import unicodedata
from collections.abc import Callable
from functools import wraps
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath
from typing import ParamSpec, TypeVar

from molt.portable_paths import portable_path_component

_HEX = frozenset("0123456789abcdef")
_P = ParamSpec("_P")
_R = TypeVar("_R")


class PythonEnvironmentIdentityError(ValueError):
    """The selected interpreter or environment has no exact content identity."""


def canonical_absolute_path(value: object) -> PurePath:
    """Parse host-path custody without borrowing the validating host's grammar."""
    if not isinstance(value, str) or "\x00" in value:
        raise PythonEnvironmentIdentityError("Python custody path is invalid")
    path = (
        PureWindowsPath(value) if PureWindowsPath(value).drive else PurePosixPath(value)
    )
    if not path.is_absolute() or str(path) != value or ".." in path.parts:
        raise PythonEnvironmentIdentityError(
            f"Python custody path must be canonical absolute: {value!r}"
        )
    if isinstance(path, PureWindowsPath):
        try:
            if value.startswith(("\\\\?\\", "\\\\.\\")):
                raise ValueError("device namespaces are not canonical paths")
            for part in path.parts[1:]:
                portable_path_component(part)
        except ValueError as exc:
            raise PythonEnvironmentIdentityError(
                f"Python custody path uses an unsupported Windows namespace or alias: {value!r}"
            ) from exc
    return path


def identity_validator(
    label: str,
) -> Callable[[Callable[_P, _R]], Callable[_P, _R]]:
    """Keep malformed external receipts in one explicit rejection error domain."""

    def decorate(
        validate: Callable[_P, _R],
    ) -> Callable[_P, _R]:
        @wraps(validate)
        def checked(*args: _P.args, **kwargs: _P.kwargs) -> _R:
            try:
                return validate(*args, **kwargs)
            except PythonEnvironmentIdentityError:
                raise
            except (
                TypeError,
                KeyError,
                ValueError,
                OverflowError,
                RecursionError,
                AttributeError,
            ) as exc:
                raise PythonEnvironmentIdentityError(
                    f"{label} contains malformed values: {exc}"
                ) from exc

        return checked

    return decorate


def _canonicalize_name(value: str) -> str:
    return re.sub(r"[-_.]+", "-", value).casefold()


def _valid_sha256(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == 64
        and all(character in _HEX for character in value)
    )


def _valid_relative_payload_path(value: object) -> bool:
    if not isinstance(value, str) or not value or "\\" in value or "\x00" in value:
        return False
    path = Path(value)
    return (
        value == unicodedata.normalize("NFC", value)
        and not path.is_absolute()
        and not value.startswith("/")
        and not re.match(r"^[A-Za-z]:", value)
        and all(part not in {"", ".", ".."} for part in value.split("/"))
    )
