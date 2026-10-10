"""The image Windows CreateProcessW runs for a Python child launch.

CPython's ``subprocess.Popen`` passes ``executable`` to CreateProcessW as
lpApplicationName and the ``list2cmdline`` string as lpCommandLine. Windows
resolves the image in the calling process: the child's ``env`` cannot change
the PATH it searches, and the child's ``cwd`` is not the directory it searches
(CPython's subprocess documentation; gh-101283 in ``Lib/subprocess.py``). The
proof broker admits a Python launch on Windows only through this module.

Documented rules (CreateProcessW, NeedCurrentDirectoryForExePathW):

* lpApplicationName names the image. Windows completes a partial name from the
  current directory and does not search.
* Otherwise the module name is the quoted first token of lpCommandLine, or each
  whitespace-delimited prefix in turn until one names an image.
* A module name without an extension gets ``.exe``.
* Windows searches for a bare name in: the caller's image directory; its
  current directory, when NeedCurrentDirectoryForExePathW allows it; the system
  directory; the 16-bit system directory; the Windows directory; then the
  caller's PATH.

Where the documentation contradicts itself or is silent, the resolver evaluates
each plausible reading. It admits an image only when no reading runs a
different image:

* a path-qualified name without an extension (the text says Windows does not
  append ``.exe``; its own example appends it), and lpApplicationName without
  an extension;
* a relative name with a separator (searched, or completed from the current
  directory);
* a PATH entry that is empty, quoted or not fully qualified;
* a search hit that is not a regular file.

Names it cannot model (device and drive-relative paths, streams, wildcards,
parent traversal, trailing dots or spaces) fail closed.
"""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
import ntpath
import os
import stat
import sys


class LaunchUndeterminable(ValueError):
    """Windows could run more than one image, or one this module cannot name."""


@dataclass(frozen=True)
class CallerSearch:
    """The calling-process facts that CreateProcessW searches with."""

    image_directory: str
    current_directory: str
    path: str | None
    # NeedCurrentDirectoryForExePathW for a name without a backslash; a name
    # with a backslash always searches the current directory.
    searches_current_directory: bool
    system_directory: str
    windows_directory: str


_EXECUTABLE_EXTENSION = ".exe"
_SEPARATORS = "\\/"
_WHITESPACE = " \t"
_FORBIDDEN_CHARACTERS = frozenset('"*<>?|')


def _entry_kind(location: str) -> str | None:
    """Classify a location as CreateProcessW would open it."""
    try:
        mode = os.stat(location).st_mode
    except (FileNotFoundError, NotADirectoryError):
        return None
    except OSError as exc:
        raise LaunchUndeterminable(f"cannot inspect {location!r}: {exc}") from exc
    return "file" if stat.S_ISREG(mode) else "other"


def _final_component(name: str) -> str:
    return name[max(name.rfind("\\"), name.rfind("/")) + 1 :]


def _has_extension(name: str) -> bool:
    return "." in _final_component(name)


def _fully_qualified(name: str) -> bool:
    if len(name) >= 3 and name[0].isalpha() and name[1] == ":":
        return name[2] in _SEPARATORS
    return len(name) >= 2 and name[0] in _SEPARATORS and name[1] in _SEPARATORS


def _checked_name(name: str) -> str:
    if any(ord(char) < 32 or char in _FORBIDDEN_CHARACTERS for char in name):
        raise LaunchUndeterminable(f"module name {name!r} has a reserved character")
    if len(name) >= 4 and name[0] in _SEPARATORS and name[1] in _SEPARATORS:
        if name[2] in ".?" and name[3] in _SEPARATORS:
            raise LaunchUndeterminable(f"module name {name!r} is a device path")
    if ":" in name and not _fully_qualified(name):
        raise LaunchUndeterminable(
            f"module name {name!r} is drive-relative or names a stream"
        )
    if ":" in name[2:]:
        raise LaunchUndeterminable(f"module name {name!r} names a stream")
    components = name.replace("/", "\\").split("\\")
    if ".." in components:
        raise LaunchUndeterminable(
            f"module name {name!r} uses parent traversal, which custody refuses"
        )
    return name


def _location(name: str, current_directory: str) -> str:
    """Complete a name from the current directory, as Win32 path parsing does."""
    if _fully_qualified(name):
        location = ntpath.normpath(name)
        if name[0] in _SEPARATORS and not ntpath.splitdrive(location)[1].strip("\\"):
            raise LaunchUndeterminable(f"module name {name!r} names no file")
        return location
    # A rooted name takes the current drive; any other name is relative.
    return ntpath.normpath(ntpath.join(current_directory, name))


def _located(location: str) -> str | None:
    if _final_component(location).endswith((".", " ")):
        # Win32 drops a trailing dot or space, so the name is not the file.
        raise LaunchUndeterminable(f"{location!r} ends in a dot or space")
    return _entry_kind(location)


def _direct_images(location: str) -> set[str | None]:
    """Images the readings of an exact location run; None means launch fails."""
    if _has_extension(location):
        return {location if _located(location) == "file" else None}
    appended = location + _EXECUTABLE_EXTENSION
    return {
        location if _located(location) == "file" else None,
        appended if _located(appended) == "file" else None,
    }


def _search_directories(name: str, search: CallerSearch) -> list[tuple[str, bool]]:
    """Return (directory, definite) pairs in CreateProcessW search order."""
    directories = [(search.image_directory, True)]
    if "\\" in name or search.searches_current_directory:
        directories.append((search.current_directory, True))
    directories += [
        (search.system_directory, True),
        (ntpath.join(search.windows_directory, "System"), True),
        (search.windows_directory, True),
    ]
    if search.path is not None:
        for entry in search.path.split(";"):
            if entry and '"' not in entry and _fully_qualified(entry):
                directories.append((entry, True))
            else:
                # Windows does not document empty, quoted or relative entries.
                # Locate the plausible reading; a hit there is undeterminable.
                interpreted = entry.replace('"', "")
                directories.append(
                    (ntpath.join(search.current_directory, interpreted), False)
                )
    return directories


def _searched_image(name: str, search: CallerSearch) -> str | None:
    filename = name if _has_extension(name) else name + _EXECUTABLE_EXTENSION
    for directory, definite in _search_directories(name, search):
        location = ntpath.normpath(ntpath.join(directory, filename))
        kind = _located(location)
        if kind is None:
            continue
        if not definite:
            raise LaunchUndeterminable(
                f"{location!r} sits in an undocumented PATH entry {directory!r}"
            )
        if kind != "file":
            raise LaunchUndeterminable(f"{location!r} shadows the search")
        return location
    return None


def _module_images(name: str, search: CallerSearch) -> set[str | None]:
    if not name:
        return {None}
    _checked_name(name)
    if not any(separator in name for separator in _SEPARATORS):
        return {_searched_image(name, search)}
    images = _direct_images(_location(name, search.current_directory))
    if not _fully_qualified(name) and name[0] not in _SEPARATORS:
        images.add(_searched_image(name, search))
    return images


def module_names(command_line: str) -> list[str]:
    """Return the module names CreateProcessW tries, in order."""
    if not command_line or command_line[0] in _WHITESPACE:
        raise LaunchUndeterminable("command line does not start with a module name")
    if command_line[0] == '"':
        end = command_line.find('"', 1)
        if end < 0:
            raise LaunchUndeterminable("command line has an unterminated quote")
        return [command_line[1:end]]
    names = [
        command_line[:index]
        for index, char in enumerate(command_line)
        if char in _WHITESPACE
    ]
    return [*names, command_line]


def _distinct_images(images: set[str | None]) -> dict[str, str]:
    # Windows names are case-insensitive: one file has one key.
    return {ntpath.normcase(image): image for image in images if image is not None}


def _single_image(images: set[str | None], what: str) -> str | None:
    found = _distinct_images(images)
    if len(found) > 1:
        raise LaunchUndeterminable(f"{what} can run {sorted(found.values())}")
    return next(iter(found.values()), None)


def createprocess_image(
    application_name: str | None, command_line: str | None, search: CallerSearch
) -> str | None:
    """Return the image CreateProcessW runs, or None when the launch fails."""
    if application_name is not None:
        _checked_name(application_name)
        location = _location(application_name, search.current_directory)
        return _single_image(_direct_images(location), repr(application_name))
    if command_line is None:
        raise LaunchUndeterminable("launch names neither an application nor a command")
    chosen: str | None = None
    for name in module_names(command_line):
        images = _module_images(name, search)
        if chosen is None:
            chosen = _single_image(images, repr(name))
            if chosen is not None and None not in images:
                return chosen
        elif _distinct_images(images).keys() - {ntpath.normcase(chosen)}:
            # A reading that failed an earlier prefix reaches a different image.
            raise LaunchUndeterminable(f"{command_line!r} can run more than one image")
        elif None not in images:
            return chosen
    return chosen


def requested_module(intent: Mapping[str, object]) -> str:
    """Name the requested module for diagnostics, without resolving it."""
    application_name = intent.get("application_name")
    if isinstance(application_name, str):
        return application_name
    command_line = intent.get("command_line")
    if not isinstance(command_line, str):
        return repr(command_line)
    try:
        return module_names(command_line)[0]
    except LaunchUndeterminable:
        return command_line


def _windows_directories() -> tuple[str, str]:
    if sys.platform != "win32":
        raise OSError("CreateProcessW search directories require Windows")
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    directories = []
    for function in (kernel32.GetSystemDirectoryW, kernel32.GetWindowsDirectoryW):
        function.argtypes = [wintypes.LPWSTR, wintypes.UINT]
        function.restype = wintypes.UINT
        size = 260
        while True:
            buffer = ctypes.create_unicode_buffer(size)
            length = function(buffer, size)
            if length == 0:
                raise ctypes.WinError(ctypes.get_last_error())
            if length < size:
                directories.append(buffer.value)
                break
            size = length
    return directories[0], directories[1]


def caller_search(intent: Mapping[str, object]) -> CallerSearch:
    """Read the calling-process facts a Python hook reports for one launch."""
    image = intent.get("caller_image")
    current_directory = intent.get("caller_cwd")
    path = intent.get("caller_path")
    searches_current_directory = intent.get("caller_searches_current_directory")
    if not isinstance(image, str) or not _fully_qualified(image):
        raise LaunchUndeterminable("launch intent has no absolute caller image")
    if not isinstance(current_directory, str) or not _fully_qualified(
        current_directory
    ):
        raise LaunchUndeterminable("launch intent has no absolute caller directory")
    if path is not None and not isinstance(path, str):
        raise LaunchUndeterminable("launch intent has a malformed caller PATH")
    if not isinstance(searches_current_directory, bool):
        raise LaunchUndeterminable("launch intent has no current-directory rule")
    system_directory, windows_directory = _windows_directories()
    return CallerSearch(
        image_directory=ntpath.dirname(image),
        current_directory=current_directory,
        path=path,
        searches_current_directory=searches_current_directory,
        system_directory=system_directory,
        windows_directory=windows_directory,
    )


def intent_image(intent: Mapping[str, object]) -> str | None:
    """Return the image a Python hook's Windows launch intent runs."""
    application_name = intent.get("application_name")
    command_line = intent.get("command_line")
    if application_name is not None and not isinstance(application_name, str):
        raise LaunchUndeterminable("launch intent has a malformed application name")
    if command_line is not None and not isinstance(command_line, str):
        raise LaunchUndeterminable("launch intent has a malformed command line")
    return createprocess_image(application_name, command_line, caller_search(intent))
