from __future__ import annotations

import os
from pathlib import Path
import re
import subprocess

from molt.cli.file_hashing import _sha256_file
from molt.cli.llvm_wasi_tools import (
    _tool_version,
    llvm_named_tool_candidates,
    llvm_tool_candidates,
)
from molt.cli.native_link_plan import NativeLinkPlan, NativeObjectFormat
from molt import process_guard


def _resolve_native_link_executable(
    command: str, *, sibling: Path | None = None
) -> Path | None:
    candidate = Path(command).expanduser()
    if candidate.is_file():
        return candidate.resolve()
    if sibling is not None:
        adjacent = sibling / command
        if os.name == "nt" and not adjacent.suffix:
            adjacent = adjacent.with_suffix(".exe")
        if adjacent.is_file():
            return adjacent.resolve()
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
    return (
        _resolve_native_link_executable(raw, sibling=driver.parent) if raw else None
    )


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
    if traced_linker is not None:
        linker = traced_linker
    elif plan.linker_hint == "lld":
        linker_name = (
            "lld-link"
            if plan.target.object_format is NativeObjectFormat.COFF
            else "ld.lld"
        )
        linker = _resolve_native_link_executable(linker_name, sibling=sibling)
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

    return [
        {
            "role": role,
            "resolved": path is not None,
            "path": str(path) if path is not None else None,
            "version": _tool_version(path) if path is not None else None,
            "sha256": _sha256_file(path) if path is not None else None,
        }
        for role, path in candidates
    ]
