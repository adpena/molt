"""One cross-platform relative-path grammar and collision identity."""

from __future__ import annotations

from pathlib import PurePosixPath, PureWindowsPath
import unicodedata


_WINDOWS_FORBIDDEN_CHARACTERS = frozenset('<>:"/\\|?*')
_WINDOWS_RESERVED_STEMS = frozenset(
    {
        "AUX",
        "CON",
        "CONIN$",
        "CONOUT$",
        "NUL",
        "PRN",
        *(f"COM{number}" for number in range(1, 10)),
        *(f"LPT{number}" for number in range(1, 10)),
        "COM¹",
        "COM²",
        "COM³",
        "LPT¹",
        "LPT²",
        "LPT³",
    }
)


def portable_path_component(value: object) -> str:
    """Validate a native Windows or portable archive component without rewriting it."""
    if not isinstance(value, str) or not value:
        raise ValueError("path component must be a nonempty portable name")
    stem = value.split(".", 1)[0].rstrip(" ").upper()
    if (
        value.endswith((" ", "."))
        or stem in _WINDOWS_RESERVED_STEMS
        or any(
            character in _WINDOWS_FORBIDDEN_CHARACTERS or ord(character) < 32
            for character in value
        )
    ):
        raise ValueError("path component must be a portable name")
    return value


def portable_relative_path(value: object) -> PurePosixPath:
    """Parse the sole receipt/archive path dialect shared by every platform."""

    if not isinstance(value, str) or not value or "\\" in value or "\x00" in value:
        raise ValueError("path must be a portable relative POSIX path")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or PureWindowsPath(value).drive
        or path.as_posix() != value
        or any(part in {"", ".", ".."} for part in path.parts)
    ):
        raise ValueError("path must be a portable relative POSIX path")
    for index, part in enumerate(path.parts):
        try:
            portable_path_component(part)
        except ValueError as exc:
            raise ValueError(
                f"path must be a portable relative POSIX path: component {index} is not portable"
            ) from exc
    return path


def portable_path_identity(value: object) -> str:
    """Return the NFC, case-folded identity used for collision admission."""

    path = portable_relative_path(value)
    normalized = unicodedata.normalize("NFC", path.as_posix())
    return unicodedata.normalize("NFC", normalized.casefold())
