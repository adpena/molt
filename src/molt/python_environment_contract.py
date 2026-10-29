"""Lightweight schema and policy facts shared by image producers and readers."""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import re
from typing import Final, TypedDict


class SourceBuildEnvironmentError(ValueError):
    pass


SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION: Final = 9
IMAGE_BYTECODE_POLICY: Final = "cpython-source-only.v1"
SOURCE_BUILD_ENVIRONMENT_MANIFEST: Final = "molt-source-build-environment.json"
DEVELOPMENT_ENVIRONMENT_SELECTION: Final = "molt-development-selection.json"
DEVELOPMENT_ENVIRONMENT_SELECTION_SCHEMA: Final = "molt.development-selection-record.v1"
ENVIRONMENT_BYTECODE_DIRECTORY: Final = "bytecode"

PYTHON_ENVIRONMENT_IDENTITY_SCHEMA: Final = "molt.python-environment-closure.v7"
PYTHON_ENVIRONMENT_LOCATION_SCHEMA: Final = "molt.python-environment-location.v2"
PYTHON_RUNTIME_IDENTITY_SCHEMA: Final = "molt.python-runtime-closure.v8"
UV_LOCK_GROUP_CLOSURE_SCHEMA: Final = "molt.uv-lock-group-closure.v4"
PYTHON_WHEEL_TARGET_SCHEMA: Final = "molt.python-wheel-target.v1"


class SourceBuildEnvironmentContract(TypedDict):
    source_build: int
    lock_closure: str
    wheel_target: str
    runtime: str
    environment: str
    location: str
    bytecode: str
    selection: str


def source_build_environment_contract() -> SourceBuildEnvironmentContract:
    """One producer identity, without importing capture or package machinery."""
    return {
        "source_build": SOURCE_BUILD_ENVIRONMENT_SCHEMA_VERSION,
        "lock_closure": UV_LOCK_GROUP_CLOSURE_SCHEMA,
        "wheel_target": PYTHON_WHEEL_TARGET_SCHEMA,
        "runtime": PYTHON_RUNTIME_IDENTITY_SCHEMA,
        "environment": PYTHON_ENVIRONMENT_IDENTITY_SCHEMA,
        "location": PYTHON_ENVIRONMENT_LOCATION_SCHEMA,
        "bytecode": IMAGE_BYTECODE_POLICY,
        "selection": DEVELOPMENT_ENVIRONMENT_SELECTION_SCHEMA,
    }


def source_build_environment_contract_matches(value: object) -> bool:
    expected = source_build_environment_contract()
    return (
        isinstance(value, dict)
        and value.keys() == expected.keys()
        and all(
            type(value[key]) is type(item) and value[key] == item
            for key, item in expected.items()
        )
    )


@dataclass(frozen=True)
class DevelopmentEnvironmentRequest:
    """Checkout/Python/group intent, independent of selection or proof role."""

    repo_root: Path
    python: str | None
    groups: tuple[str, ...]

    def intent_arguments(self) -> tuple[str, ...]:
        return (
            "--development-checkout",
            str(self.repo_root),
            *(() if self.python is None else ("--development-python", self.python)),
            *(
                value
                for group in self.groups
                for value in ("--development-group", group)
            ),
        )


@dataclass(frozen=True, slots=True)
class CanonicalPythonRuntimeVersion:
    text: str
    release: tuple[int, int, int]
    is_prerelease: bool


def parse_canonical_python_runtime_version(
    value: object,
) -> CanonicalPythonRuntimeVersion:
    """Parse the runtime closure's existing CPython version grammar once.

    Retain prerelease identity even when an unconstrained image selection is
    allowed; a requested final release must not accidentally admit it.
    """
    if not isinstance(value, str):
        raise ValueError("invalid canonical CPython runtime version")
    match = re.fullmatch(
        r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
        r"(?:(a|b|rc)(0|[1-9][0-9]*))?",
        value,
    )
    if match is None:
        raise ValueError("invalid canonical CPython runtime version")
    return CanonicalPythonRuntimeVersion(
        value,
        (int(match[1]), int(match[2]), int(match[3])),
        match[4] is not None,
    )
