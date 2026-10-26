"""Exact pinned Cargo configuration projection."""

from __future__ import annotations

import json
import os
from pathlib import Path
from typing import Mapping, Sequence


def cargo_environment_force_overrides(
    forced_environment: Sequence[str],
) -> tuple[str, ...]:
    """Keep resolved environment values from being replaced again by Cargo."""
    return tuple(
        "env." + json.dumps(name) + ".force=false" for name in forced_environment
    )


def _pinned_tool_configurations(
    tools: Mapping[str, Path],
    wrappers: Mapping[str, Path],
    target: str,
    forced_environment: tuple[str, ...] = (),
    build_directory: Path | None = None,
    target_directory: Path | None = None,
) -> tuple[tuple[str, str], ...]:
    selectors = [("build.rustc", os.fspath(tools["rustc"]), "tool/rustc")]
    selectors.extend(
        (
            "build." + key,
            os.fspath(wrappers[name]) if name in wrappers else "",
            "wrapper/" + name if name in wrappers else "disabled",
        )
        for name, key in (
            ("RUSTC_WRAPPER", "rustc-wrapper"),
            ("RUSTC_WORKSPACE_WRAPPER", "rustc-workspace-wrapper"),
        )
    )
    if "linker" in tools:
        selectors.append(
            (
                "target." + json.dumps(target) + ".linker",
                os.fspath(tools["linker"]),
                "tool/linker",
            )
        )
    pinned = tuple(
        (key + "=" + json.dumps(path), key + "=" + json.dumps(logical))
        for key, path, logical in selectors
    )
    # The resolved environment already contains the selected value. Disable
    # child-time force overrides so they cannot replace pinned executable paths.
    env_pins = cargo_environment_force_overrides(forced_environment)
    output = (
        ()
        if build_directory is None
        else (
            (
                "build.build-dir=" + json.dumps(os.fspath(build_directory)),
                'build.build-dir="${cargo-build-directory}"',
            ),
        )
    )
    target_output = (
        ()
        if target_directory is None
        else (
            (
                "build.target-dir=" + json.dumps(os.fspath(target_directory)),
                'build.target-dir="${cargo-target-directory}"',
            ),
        )
    )
    return (*pinned, *((value, value) for value in env_pins), *output, *target_output)


def _pin_cargo_command(
    command: Sequence[str],
    tools: Mapping[str, Path],
    wrappers: Mapping[str, Path],
    target: str,
    forced_environment: tuple[str, ...] = (),
    build_directory: Path | None = None,
    target_directory: Path | None = None,
) -> tuple[str, ...]:
    pinned = tuple(
        token
        for raw, _logical in _pinned_tool_configurations(
            tools,
            wrappers,
            target,
            forced_environment,
            build_directory,
            target_directory,
        )
        for token in ("--config", raw)
    )
    separator = command.index("--") if "--" in command else len(command)
    # The compiler's runtime dependency graph is immutable for every guest
    # profile and determinism setting. Own this once for native and WASM,
    # including manifest refreshes, before the rustc passthrough boundary.
    locked = () if "--locked" in command[1:separator] else ("--locked",)
    return (
        os.fspath(tools["cargo"]),
        *command[1:separator],
        *locked,
        *pinned,
        *command[separator:],
    )
