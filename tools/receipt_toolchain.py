"""Observed interpreter identity for Python-only structural receipt producers.

This identifies the audit engine, never a guest compiler/runtime or oracle.
Source script and inspected inputs are bound separately by the receipt authority.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path
import platform
import sys
from typing import Any

from molt.portable_paths import portable_path_component
from molt.python_runtime_identity import (
    capture_current_python_runtime,
    validate_python_runtime_identity,
    runtime_explicit_file_content,
)
from molt.toolchain_identity import executable_content_identity


def observe_python_audit_engine() -> dict[str, Any]:
    return {
        "kind": "python-audit-engine-v1",
        "implementation": platform.python_implementation(),
        "version": platform.python_version(),
        "executable": executable_content_identity(
            Path(getattr(sys, "_base_executable", sys.executable)),
            label="release structural audit interpreter",
        ),
        "runtime_closure": capture_current_python_runtime(),
        "command_executable": executable_content_identity(
            Path(sys.executable), label="release structural audit command interpreter"
        ),
    }


def audit_engine_problems(value: object) -> tuple[str, ...]:
    if not isinstance(value, Mapping) or set(value) != {
        "kind",
        "implementation",
        "version",
        "executable",
        "command_executable",
        "runtime_closure",
    }:
        return (
            "audit engine must contain exactly kind, implementation, version, executable, command_executable, runtime_closure",
        )
    problems: list[str] = []
    if value.get("kind") != "python-audit-engine-v1":
        problems.append("audit engine kind must be python-audit-engine-v1")
    if value.get("implementation") != "CPython":
        problems.append("audit engine implementation must be CPython")
    version = value.get("version")
    if (
        not isinstance(version, str)
        or len(version.split(".")) != 3
        or not all(
            component.isascii() and component.isdigit()
            for component in version.split(".")
        )
    ):
        problems.append(
            "audit engine version must be an exact Python major.minor.micro"
        )
    try:
        runtime = validate_python_runtime_identity(value.get("runtime_closure"))
    except ValueError as exc:
        problems.append(f"audit engine runtime_closure is invalid: {exc}")
    else:
        if runtime.get("implementation") != str(
            value.get("implementation", "")
        ).lower() or runtime.get("version") != value.get("version"):
            problems.append(
                "audit engine identity differs from its observed runtime closure"
            )
        base_image = runtime_explicit_file_content(runtime, "base-executable")
        executable = value.get("executable")
        if (
            not isinstance(executable, Mapping)
            or base_image is None
            or any(
                executable.get(field) != base_image.get(field)
                for field in ("sha256", "size")
            )
            or executable.get("content_filename") != base_image.get("filename")
        ):
            problems.append(
                "audit engine executable differs from its observed runtime base image"
            )

    for role in ("executable", "command_executable"):
        problems.extend(_executable_identity_problems(value.get(role), role=role))
    return tuple(problems)


def _executable_identity_problems(identity: object, *, role: str) -> tuple[str, ...]:
    problems: list[str] = []
    if not isinstance(identity, Mapping) or set(identity) != {
        "entrypoint",
        "content_filename",
        "size",
        "sha256",
    }:
        problems.append(f"audit engine {role} content identity has invalid keys")
    else:
        for field in ("entrypoint", "content_filename"):
            try:
                portable_path_component(identity.get(field))
            except (TypeError, ValueError):
                problems.append(
                    f"audit engine {role}.{field} must be a portable filename"
                )
        size = identity.get("size")
        if not isinstance(size, int) or isinstance(size, bool) or size <= 0:
            problems.append(f"audit engine {role}.size must be a positive integer")
        digest = identity.get("sha256")
        if (
            not isinstance(digest, str)
            or len(digest) != 64
            or any(character not in "0123456789abcdef" for character in digest)
        ):
            problems.append(f"audit engine {role}.sha256 must be lowercase SHA-256")
    return tuple(problems)
