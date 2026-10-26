"""Runtime executable and host/target C-tool selection before custody capture."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Mapping

from molt.cargo_execution_policy import (
    cargo_c_tool_environment_names,
    cargo_c_tool_words,
)
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)
from molt.toolchain_identity import find_executable, resolve_executable


def runtime_tool_path(
    value: object, *, root: Path, env: Mapping[str, str], role: str
) -> Path:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"runtime {role} executable is not selected")
    command = value.strip()
    if role in {
        "cc",
        "cxx",
        "ar",
        "ranlib",
        "host_cc",
        "host_cxx",
        "host_ar",
        "host_ranlib",
    }:
        words = cargo_c_tool_words(
            value, cwd=root, windows=ProcessEnvironment(env).rule is NameRule.WINDOWS
        )
        if len(words) != 1:
            raise ValueError(
                f"runtime {role} compound C commands require an attested executable dependency plan"
            )
        command = words[0]
        if not Path(command).is_absolute() and (root / command).exists():
            command = os.fspath(root / command)
    candidate = Path(command)
    if not candidate.is_absolute() and any(part in command for part in ("/", "\\")):
        candidate = root / candidate
        command = os.fspath(candidate)
    # Cargo executable selectors are paths, not shell commands. Compound C
    # wrapper commands require a separate attested executable dependency plan.
    return resolve_executable(
        command, environment=env, label=f"runtime {role}", cwd=root
    )


def runtime_c_tool_selection(
    role: str,
    *,
    root: Path,
    env: Mapping[str, str],
    target: str,
    host_target: str,
    allow_default: bool = True,
) -> Path | None:
    """One cc-rs selector precedence for Cargo and its development proof."""
    names = cargo_c_tool_environment_names(role, target=target, host_target=host_target)
    value = next((env[name] for name in names if env.get(name)), None)
    if value is None and allow_default and target == host_target:
        value = next(
            (
                name
                for name in target_tool_defaults(target)[role]
                if find_executable(name, environment=env, cwd=root) is not None
            ),
            None,
        )
    return (
        None
        if value is None
        else runtime_tool_path(value, root=root, env=env, role=role)
    )


def select_host_c_tools(
    tools: dict[str, Path],
    environment: ProcessEnvironmentBuilder,
    *,
    root: Path,
    target: str,
    host_target: str,
) -> None:
    """Pin explicit host C selections for cross-target build dependencies.

    Native builds already select the host lane as their target lane. A cross
    build cannot use its target compiler receipt to attest HOST_CC et al.
    Implicit host compiler SDK/resource discovery remains a separate admission
    boundary; do not pretend the target compiler's sysroot supplies it.
    """
    if target == host_target:
        return
    for role in ("cc", "cxx", "ar", "ranlib"):
        names = cargo_c_tool_environment_names(
            role, target=host_target, host_target=host_target
        )
        path = runtime_c_tool_selection(
            role,
            root=root,
            env=environment,
            target=host_target,
            host_target=host_target,
            allow_default=False,
        )
        if path is None:
            continue
        tools["host_" + role] = path
        for name in names[:2]:
            environment[name] = os.fspath(path)


def target_tool_defaults(target: str) -> Mapping[str, tuple[str, ...]]:
    if target.endswith("-pc-windows-msvc"):
        return {
            "cc": ("cl",),
            "cxx": ("cl",),
            "ar": ("lib",),
            "ranlib": ("lib",),
            "linker": ("link",),
        }
    if "windows-gnu" in target or "windows-gnullvm" in target:
        return {
            "cc": ("gcc", "clang"),
            "cxx": ("g++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("gcc", "clang"),
        }
    if "-apple-" in target or target.endswith("-darwin"):
        return {
            "cc": ("cc", "clang"),
            "cxx": ("c++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("cc", "clang"),
        }
    if "-linux-" in target:
        return {
            "cc": ("cc", "gcc", "clang"),
            "cxx": ("c++", "g++", "clang++"),
            "ar": ("ar", "llvm-ar"),
            "ranlib": ("ranlib", "llvm-ranlib"),
            "linker": ("cc", "gcc", "clang"),
        }
    # WebAssembly C tools are never host guesses: the manifest-pinned WASI SDK
    # owns them and `llvm_toolchain.apply_provisioned_wasm_toolchain` selects
    # them, like every other target without a host default.
    return {role: () for role in ("cc", "cxx", "ar", "ranlib", "linker")}
