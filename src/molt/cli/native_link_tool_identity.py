from __future__ import annotations

import functools
import os
from pathlib import Path
import re
import subprocess

from molt.file_hashing import _sha256_file
from molt.cli.llvm_wasi_tools import (
    _tool_version,
    llvm_linker_candidates,
    llvm_named_tool_candidates,
    llvm_tool_candidates,
)
from molt.cli.native_link_plan import NativeLinkPlan
from molt.llvm_linker_roles import (
    executable_selects_linker_role,
    lexical_executable_path,
    llvm_linker_role_for_object_format,
)
from molt import process_guard


@functools.lru_cache(maxsize=128)
def _cached_native_link_tool_content_fact(
    physical_path: str,
    stat_identity: tuple[int, int, int, int, int],
) -> tuple[str | None, str]:
    path = Path(physical_path)
    version = _tool_version(path)
    sha256 = _sha256_file(path)
    stat = path.stat()
    if (
        stat.st_size,
        stat.st_mtime_ns,
        stat.st_ctime_ns,
        stat.st_dev,
        stat.st_ino,
    ) != stat_identity:
        raise OSError(f"native link tool changed while hashing: {path}")
    return version, sha256


def _native_link_tool_content_fact(path: Path) -> tuple[str | None, str]:
    stat = path.stat()
    return _cached_native_link_tool_content_fact(
        os.path.realpath(path),
        (
            stat.st_size,
            stat.st_mtime_ns,
            stat.st_ctime_ns,
            stat.st_dev,
            stat.st_ino,
        ),
    )


def _native_link_tool_stat_fact(role: str, path: Path | None) -> dict[str, object]:
    if path is None:
        return {"role": role, "resolved": False, "path": None}
    stat = path.stat()
    return {
        "role": role,
        "resolved": True,
        "path": str(path),
        "size": stat.st_size,
        "mtime_ns": stat.st_mtime_ns,
        "ctime_ns": stat.st_ctime_ns,
        "device": stat.st_dev,
        "inode": stat.st_ino,
    }


def _resolve_native_link_executable(
    command: str, *, sibling: Path | None = None
) -> Path | None:
    candidate = Path(command).expanduser()
    if candidate.is_file():
        return lexical_executable_path(candidate)
    if sibling is not None:
        adjacent = sibling / command
        if os.name == "nt" and not adjacent.suffix:
            adjacent = adjacent.with_suffix(".exe")
        if adjacent.is_file():
            return lexical_executable_path(adjacent)
    if "/" in command or "\\" in command:
        return None
    candidates = llvm_named_tool_candidates(
        command,
        sibling_directories=(() if sibling is None else (sibling,)),
    )
    return candidates[0] if candidates else None


def _system_linker_from_driver(driver: Path) -> Path | None:
    try:
        result = process_guard.run_completed_command(
            [str(driver), "-print-prog-name=ld"],
            cwd=driver.parent,
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    raw = result.stdout.strip()
    return _resolve_native_link_executable(raw, sibling=driver.parent) if raw else None


def _linker_from_driver_trace(plan: NativeLinkPlan, driver: Path) -> Path | None:
    command = list(plan.command)
    command[0] = str(driver)
    insert_at = 2 if driver.stem.lower() == "zig" and command[1:2] == ["cc"] else 1
    command.insert(insert_at, "-###")
    try:
        result = process_guard.run_completed_command(
            command,
            cwd=driver.parent,
            capture_output=True,
            text=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    paths = re.findall(r'"([^"\r\n]+)"', result.stderr)
    linker_names = {
        "ld",
        "ld.exe",
        "ld.lld",
        "ld.lld.exe",
        "ld64.lld",
        "ld64.lld.exe",
        "lld-link",
        "lld-link.exe",
        "link",
        "link.exe",
        "mold",
        "mold.exe",
    }
    for raw in reversed(paths):
        candidate = Path(raw)
        if candidate.name.lower() in linker_names:
            return _resolve_native_link_executable(raw, sibling=driver.parent)
    return None


def native_link_tool_facts(plan: NativeLinkPlan) -> list[dict[str, object]]:
    """Resolve and content-identify every tool participating in a native link.

    This is shared production authority so diagnostics and profilers cannot
    independently guess the linker selected by the canonical driver plan.
    """
    driver = _resolve_native_link_executable(plan.command[0])
    candidates: list[tuple[str, Path | None]] = [("driver", driver)]
    sibling = driver.parent if driver is not None else None
    traced_linker = (
        _linker_from_driver_trace(plan, driver) if driver is not None else None
    )
    if plan.linker_hint == "lld":
        linker_role = llvm_linker_role_for_object_format(
            plan.target.object_format.value
        )
        if traced_linker is not None:
            if not executable_selects_linker_role(traced_linker, linker_role):
                raise RuntimeError(
                    "Native LLVM linker trace crossed object-format roles: "
                    f"expected {linker_role}, found {traced_linker}. The traced "
                    "generic or cross-role driver cannot be attested by a "
                    "different fallback executable."
                )
            linker = traced_linker
        else:
            role_candidates = llvm_linker_candidates(
                linker_role,
                sibling_directories=(() if sibling is None else (sibling,)),
            )
            linker = role_candidates[0] if role_candidates else None
    elif traced_linker is not None:
        linker = traced_linker
    elif plan.linker_hint == "mold":
        linker = _resolve_native_link_executable("mold", sibling=sibling)
    else:
        linker = _system_linker_from_driver(driver) if driver is not None else None
    candidates.append(("linker", linker))
    if plan.policy.strip_after_link or plan.policy.bolt_requested:
        strips = llvm_tool_candidates(
            "strip", sibling_directories=(() if sibling is None else (sibling,))
        )
        candidates.append(("strip", strips[0] if strips else None))
    readobjs = llvm_named_tool_candidates(
        "llvm-readobj", sibling_directories=(() if sibling is None else (sibling,))
    )
    candidates.append(("inspector", readobjs[0] if readobjs else None))
    if plan.policy.bolt_requested:
        candidates.extend(
            (
                (
                    "llvm-bolt",
                    _resolve_native_link_executable("llvm-bolt", sibling=sibling),
                ),
                (
                    "merge-fdata",
                    _resolve_native_link_executable("merge-fdata", sibling=sibling),
                ),
                ("bash", _resolve_native_link_executable("bash")),
            )
        )

    facts: list[dict[str, object]] = []
    for role, path in candidates:
        version, sha256 = (
            _native_link_tool_content_fact(path) if path is not None else (None, None)
        )
        facts.append(
            {
                "role": role,
                "resolved": path is not None,
                "path": str(path) if path is not None else None,
                "version": version,
                "sha256": sha256,
            }
        )
    return facts


def native_link_cache_tool_facts(plan: NativeLinkPlan) -> list[dict[str, object]]:
    """Return cheap, role-specific identities for native link cache custody.

    Full benchmark facts deliberately trace the driver and hash every tool. The
    incremental link cache runs on every build, so it uses lexical path plus the
    filesystem mutation identity instead. LLD resolution remains exact by
    object-format role and never substitutes the generic driver.
    """

    driver = _resolve_native_link_executable(plan.command[0])
    sibling = driver.parent if driver is not None else None
    linker: Path | None
    if plan.linker_hint == "lld":
        linker_role = llvm_linker_role_for_object_format(
            plan.target.object_format.value
        )
        explicit_linkers = tuple(
            lexical_executable_path(Path(arg.split("=", 1)[1].strip()))
            for arg in plan.command
            if arg.startswith("-fuse-ld=")
            and arg.split("=", 1)[1].strip().lower() != "lld"
            and executable_selects_linker_role(
                Path(arg.split("=", 1)[1].strip()), linker_role
            )
        )
        linker = next((path for path in explicit_linkers if path.is_file()), None)
        if linker is None:
            candidates = llvm_linker_candidates(
                linker_role,
                sibling_directories=(() if sibling is None else (sibling,)),
            )
            linker = candidates[0] if candidates else None
        if linker is None:
            raise RuntimeError(
                f"Native link plan requires {linker_role}, but no exact-role "
                "entrypoint can be attested for the incremental link cache."
            )
    elif plan.linker_hint == "mold":
        linker = _resolve_native_link_executable("mold", sibling=sibling)
    else:
        linker = _system_linker_from_driver(driver) if driver is not None else None

    tools: list[tuple[str, Path | None]] = [("driver", driver), ("linker", linker)]
    if plan.policy.strip_after_link or plan.policy.bolt_requested:
        strips = llvm_tool_candidates(
            "strip", sibling_directories=(() if sibling is None else (sibling,))
        )
        tools.append(("strip", strips[0] if strips else None))
    if plan.policy.bolt_requested:
        tools.extend(
            (
                (
                    "llvm-bolt",
                    _resolve_native_link_executable("llvm-bolt", sibling=sibling),
                ),
                (
                    "merge-fdata",
                    _resolve_native_link_executable("merge-fdata", sibling=sibling),
                ),
                ("bash", _resolve_native_link_executable("bash")),
            )
        )
    return [_native_link_tool_stat_fact(role, path) for role, path in tools]
