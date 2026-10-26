"""Native process launch facts shared by lifetime retention and strict custody.

These facts describe finite search resources. They do not resolve an executable,
admit identity, rewrite a command, or impose proof-custody refusals on a launcher.
This module stays stdlib-only so isolated proof bootstrap can load its exact file.
"""

from __future__ import annotations

from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
import ntpath
import os
from pathlib import Path
import sys
from typing import Literal, TypedDict


_SEPARATORS = "\\/"


class WindowsCallerFacts(TypedDict):
    caller_image: str
    caller_cwd: str
    caller_path: str | None
    caller_searches_current_directory: bool


@dataclass(frozen=True)
class PosixSearch:
    path: tuple[str, ...]
    cwd: str


@dataclass(frozen=True)
class NativeLaunchResource:
    path: Path
    kind: Literal["entrypoint", "directory"]


def posix_search(child_env: object, child_cwd: object) -> PosixSearch:
    """Project CPython exec search without opening or choosing any executable."""
    path = tuple(
        os.get_exec_path(child_env if isinstance(child_env, Mapping) else None)
    )
    if isinstance(child_cwd, bytes):
        child_cwd = os.fsdecode(child_cwd)
    effective_cwd = (
        os.path.abspath(child_cwd)
        if isinstance(child_cwd, str) and child_cwd
        else os.getcwd()
    )
    return PosixSearch(path, effective_cwd)


def posix_launch(
    token: object, child_env: object, child_cwd: object, argv: object = None
) -> dict[str, object]:
    """The existing child-custody wire projection of native POSIX launch facts."""
    search = posix_search(child_env, child_cwd)
    return {
        "requested": os.fsdecode(token) if isinstance(token, bytes) else str(token),
        "path": os.pathsep.join(search.path),
        "cwd": search.cwd,
        "argv": (
            [os.fsdecode(arg) if isinstance(arg, bytes) else arg for arg in argv]
            if isinstance(argv, (list, tuple))
            else None
        ),
    }


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


def native_environment_reader() -> Callable[[str], str | None]:
    """Read the real inherited block, including os.putenv-only changes."""
    import ctypes

    if os.name != "nt":
        getenv = ctypes.CDLL(None).getenv
        getenv.argtypes = [ctypes.c_char_p]
        getenv.restype = ctypes.c_char_p

        def variable(name: str) -> str | None:
            value = getenv(os.fsencode(name))
            return None if value is None else os.fsdecode(value)

        return variable

    from ctypes import wintypes

    get_variable = ctypes.WinDLL(
        "kernel32", use_last_error=True
    ).GetEnvironmentVariableW
    get_variable.argtypes = [wintypes.LPCWSTR, wintypes.LPWSTR, wintypes.DWORD]
    get_variable.restype = wintypes.DWORD
    create_buffer = ctypes.create_unicode_buffer
    get_last_error = ctypes.get_last_error
    set_last_error = ctypes.set_last_error
    error_envvar_not_found = 203

    def variable(name: str) -> str | None:
        size = 1024
        while True:
            buffer = create_buffer(size)
            set_last_error(0)
            length = get_variable(name, buffer, size)
            if length == 0:
                error = get_last_error()
                if error == error_envvar_not_found:
                    return None
                if error:
                    raise OSError(error, f"cannot read the process {name}")
                return ""
            if length < size:
                return buffer.value
            size = length

    return variable


def windows_caller_facts(
    variable: Callable[[str], str | None],
) -> Callable[[], WindowsCallerFacts]:
    """Bind CreateProcessW search to the actual calling process environment."""
    if sys.platform != "win32":
        raise OSError("Windows caller facts require Windows")
    import ctypes
    from ctypes import wintypes

    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    get_module_file_name = kernel32.GetModuleFileNameW
    get_module_file_name.argtypes = [wintypes.HMODULE, wintypes.LPWSTR, wintypes.DWORD]
    get_module_file_name.restype = wintypes.DWORD
    need_current_directory = kernel32.NeedCurrentDirectoryForExePathW
    need_current_directory.argtypes = [wintypes.LPCWSTR]
    need_current_directory.restype = wintypes.BOOL
    create_buffer = ctypes.create_unicode_buffer
    get_last_error = ctypes.get_last_error
    set_last_error = ctypes.set_last_error
    win_error = ctypes.WinError
    current_directory = os.getcwd

    def module_file_name() -> str:
        size = 260
        # The extended Windows path bound includes its terminating UTF-16 NUL.
        maximum_units = 32768
        while True:
            buffer = create_buffer(size)
            set_last_error(0)
            length = get_module_file_name(0, buffer, size)
            if length == 0:
                raise win_error(get_last_error())
            if length < size:
                return buffer.value
            # GetModuleFileNameW returns nSize on truncation, rather than the
            # required capacity. Never return a truncated caller identity.
            if size == maximum_units:
                raise win_error(122)  # ERROR_INSUFFICIENT_BUFFER
            size = min(size * 2, maximum_units)

    def caller_facts() -> WindowsCallerFacts:
        return {
            "caller_image": module_file_name(),
            "caller_cwd": current_directory(),
            "caller_path": variable("PATH"),
            # A name without a backslash follows NoDefaultCurrentDirectoryInExePath.
            "caller_searches_current_directory": bool(need_current_directory("molt")),
        }

    return caller_facts


def windows_directories() -> tuple[str, str]:
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


def windows_fully_qualified(name: str) -> bool:
    if len(name) >= 3 and name[0].isalpha() and name[1] == ":":
        return name[2] in _SEPARATORS
    return len(name) >= 2 and name[0] in _SEPARATORS and name[1] in _SEPARATORS


def windows_search_directories(
    name: str, search: CallerSearch
) -> list[tuple[str, bool]]:
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
            if entry and '"' not in entry and windows_fully_qualified(entry):
                directories.append((entry, True))
            else:
                # Windows does not document empty, quoted or relative entries.
                # Locate the plausible reading; a hit there is undeterminable.
                interpreted = entry.replace('"', "")
                directories.append(
                    (ntpath.join(search.current_directory, interpreted), False)
                )
    return directories


def windows_search(facts: WindowsCallerFacts) -> CallerSearch:
    """Complete native caller facts; strict wire validation belongs to custody."""
    system, windows = windows_directories()
    return CallerSearch(
        image_directory=ntpath.dirname(facts["caller_image"]),
        current_directory=facts["caller_cwd"],
        path=facts["caller_path"],
        searches_current_directory=facts["caller_searches_current_directory"],
        system_directory=system,
        windows_directory=windows,
    )


def command_resources(
    command: Sequence[str], environment: Mapping[str, str], *, cwd: Path
) -> tuple[NativeLaunchResource, ...]:
    """Conservative finite launch resources, with native launch behavior intact.

    Direct lexical entrypoints keep their enclosing environment even when their
    final file is a symlink to a base interpreter. A bare name retains possible
    search directories, not an alleged executable identity. No file is probed.
    """
    if isinstance(command, (str, bytes)) or not command:
        raise TypeError("command resource selection requires nonempty typed argv")
    requested = command[0]
    if os.name != "nt":
        candidate = Path(requested)
        if candidate.is_absolute() or "/" in requested:
            return (
                NativeLaunchResource(
                    candidate if candidate.is_absolute() else cwd / candidate,
                    "entrypoint",
                ),
            )
        search = posix_search(environment, str(cwd))
        directories = []
        for raw in search.path:
            path = Path(raw)
            directories.append(
                NativeLaunchResource(
                    path if path.is_absolute() else Path(search.cwd) / path, "directory"
                )
            )
        return tuple(dict.fromkeys(directories))
    candidate = Path(requested)
    if candidate.is_absolute():
        return (NativeLaunchResource(candidate, "entrypoint"),)
    facts = windows_caller_facts(native_environment_reader())()
    paths = [Path(facts["caller_cwd"]) / candidate]
    # Relative Windows names with separators have documented ambiguous search
    # readings. Retain the complete finite search superset; never resolve or
    # reject a launch that the native API would decide for itself.
    search = windows_search(facts)
    paths.extend(
        Path(directory) / candidate
        for directory, _definite in windows_search_directories(requested, search)
    )
    return tuple(
        NativeLaunchResource(path, "entrypoint") for path in dict.fromkeys(paths)
    )
