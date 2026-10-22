"""The executable search PATH of a Molt-driven Cargo build (HF-385).

A build script that declares ``cargo:rerun-if-env-changed=PATH`` (such as
``runtime/molt-runtime-platform/build.rs``, for bare ``cargo build``) reruns,
and Cargo rebuilds every dependent unit, whenever the caller's PATH differs.
The Cargo plan already selects every tool by absolute path, so the caller's
PATH changes no product byte: ``uv run molt`` and ``.venv/bin/molt`` differ
only in PATH and published byte-identical runtime archives, each after a full
Cargo rebuild. Molt therefore gives Cargo a PATH derived only from the plan's
typed selections, as Bazel's ``--incompatible_strict_action_env`` gives every
action a fixed PATH so a user's PATH cannot split the action cache:

1. ``<MOLT_LLVM_PREFIX>/bin`` when the plan selects the managed LLVM SDK, first,
   as the managed toolchain projection orders it (HF-F238);
2. the directory of each identity-captured executable of the plan, in the
   plan's role order;
3. the platform's standard utility directories, POSIX ``confstr(_CS_PATH)``.

No caller PATH entry survives, so the value is a function of the plan alone
and verification recomputes it exactly. Windows keeps the caller's PATH: MSVC
tools resolve their companions through the developer environment's PATH, and
no Windows build has qualified a derived value (an unverified cell).
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping
import os
from pathlib import Path

LLVM_PREFIX_SELECTOR = "MOLT_LLVM_PREFIX"


def derives_cargo_search_path() -> bool:
    """Whether this host gives Cargo a derived PATH (POSIX) or the caller's."""
    return os.name != "nt"


def standard_utility_directories() -> tuple[Path, ...]:
    """POSIX's PATH value that finds every standard utility (``getconf PATH``)."""
    value = os.confstr("CS_PATH")
    if not value:
        raise ValueError("confstr(_CS_PATH) names no standard utility directories")
    return tuple(Path(entry) for entry in value.split(os.pathsep) if entry)


def cargo_search_path(
    environment: Mapping[str, str], executables: Iterable[Path]
) -> str:
    """The PATH for a Cargo plan whose captured executables are ``executables``.

    ``environment`` supplies only the typed toolchain selector
    ``MOLT_LLVM_PREFIX``; its PATH is never read.
    """
    directories: list[Path] = []
    llvm_prefix = environment.get(LLVM_PREFIX_SELECTOR, "").strip()
    if llvm_prefix:
        prefix = Path(llvm_prefix)
        if not prefix.is_absolute():
            raise ValueError(f"{LLVM_PREFIX_SELECTOR} must be absolute: {llvm_prefix}")
        directories.append(prefix / "bin")
    for executable in executables:
        if not executable.is_absolute():
            raise ValueError(f"Cargo plan executable is not absolute: {executable}")
        directories.append(executable.parent)
    directories.extend(standard_utility_directories())
    seen: set[str] = set()
    entries: list[str] = []
    for directory in directories:
        text = os.fspath(directory)
        if os.pathsep in text:
            raise ValueError(f"PATH directory contains {os.pathsep!r}: {text}")
        key = os.path.normcase(os.path.normpath(text))
        if key not in seen:
            seen.add(key)
            entries.append(text)
    return os.pathsep.join(entries)
