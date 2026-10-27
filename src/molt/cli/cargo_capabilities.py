"""Admit Cargo capabilities using one custody-bound, cached version probe."""

from __future__ import annotations

import os
import re
import subprocess
from enum import Enum
from pathlib import Path
from typing import Mapping

from molt import process_guard
from molt.cli.cargo_resource_custody import CargoExecutableCustody
from molt.process_environment import ProcessEnvironment
from molt.toolchain_facts import (
    CARGO_VERSION_ENVIRONMENT,
    ProbeKey,
    ProbeTool,
    completed_probe,
    environment_inputs,
    host_toolchain_facts,
)


class CargoCapability(Enum):
    BUILD_DIRECTORY = ((1, 91, 0), "resource-keyed build directories")
    WARNINGS_DENY = ((1, 97, 0), "warning denial")

    def __init__(self, minimum_version: tuple[int, int, int], description: str) -> None:
        self.minimum_version = minimum_version
        self.description = description


def require_cargo_capability_version(version: str, capability: CargoCapability) -> None:
    """Admit a typed Cargo capability from its captured version banner."""
    banner = version.partition("\n")[0].rstrip("\r")
    match = re.fullmatch(r"cargo (\d+)\.(\d+)\.(\d+)(?:[-+ ][^\r\n]*)?", banner)
    if match is None or tuple(map(int, match.groups())) < capability.minimum_version:
        minimum = ".".join(map(str, capability.minimum_version)).removesuffix(".0")
        raise ValueError(
            f"runtime Cargo requires Cargo {minimum} or newer for {capability.description}"
        )


def require_cargo_capabilities(
    cargo: CargoExecutableCustody,
    *,
    root: Path,
    env: Mapping[str, str],
    capabilities: tuple[CargoCapability, ...] = (),
) -> None:
    # Older Cargo ignores unknown configuration keys instead of proving that
    # the selected policy is used. Admit all requirements from one cached probe.
    tool = ProbeTool.from_identity(cargo.entrypoint, cargo.identity)
    key = ProbeKey(
        "cargo-version",
        (tool,),
        ("--version",),
        os.fspath(root),
        environment_inputs(env, CARGO_VERSION_ENVIRONMENT),
        (),
    )

    def run() -> subprocess.CompletedProcess[str]:
        with tool.fenced(label="runtime Cargo version"):
            return process_guard.run_completed_command(
                [os.fspath(cargo.entrypoint), "--version"],
                cwd=root,
                env=ProcessEnvironment(env),
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=15,
                memory_guard_prefix=None,
                check=False,
            )

    result, _cached = completed_probe(host_toolchain_facts(), key, run)
    if result.returncode != 0:
        raise ValueError("runtime Cargo capability version probe failed")
    for capability in dict.fromkeys((CargoCapability.BUILD_DIRECTORY, *capabilities)):
        require_cargo_capability_version(result.stdout, capability)
