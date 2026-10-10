"""Shared backend environment contract, also embedded by molt-ir.

Only backend_environment.json authors membership. Keep absent and empty values
distinct: several pass controls intentionally test presence, not truthiness.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Mapping

from molt.backend_executable_names import (
    CODEGEN_BACKENDS,
    DEFAULT_CODEGEN_BACKEND,
    CodegenBackend,
)
from molt.dx import scratch_dir
from molt.source_root import compiler_source_root

# The one input that places backend debug artifacts and scratch objects;
# runtime/molt-ir/src/debug_artifacts.rs reads only this (HF-111).
DEBUG_ARTIFACT_DIR_ENV = "MOLT_DEBUG_ARTIFACT_DIR"
DEBUG_ARTIFACT_SCRATCH_PURPOSE = "molt-backend"

_CATALOG = json.loads(Path(__file__).with_suffix(".json").read_text(encoding="utf-8"))
_GROUPS = {
    "common",
    "native",
    "wasm",
    "diagnostic",
    "observation",
    "resource",
    "transport",
    "runtime_identity",
    "build_identity",
}
if (
    not isinstance(_CATALOG, dict)
    or _CATALOG.get("schema") != 1
    or set(_CATALOG) != _GROUPS | {"schema"}
):
    raise ValueError("unsupported backend environment catalog")
_CATALOG.pop("schema")
if any(
    not isinstance(names, list)
    or not all(
        isinstance(name, str)
        and name
        and name.isascii()
        and name.replace("_", "").isalnum()
        and name == name.upper()
        for name in names
    )
    for names in _CATALOG.values()
):
    raise ValueError("malformed backend environment catalog group")
_ALL = [name for names in _CATALOG.values() for name in names]
if len(_ALL) != len(set(_ALL)):
    raise ValueError("duplicate backend environment catalog entry")


def environment_keys(*groups: str) -> tuple[str, ...]:
    return tuple(name for group in groups for name in _CATALOG[group])


def compilation_diagnostics_requested(env: Mapping[str, str] | None = None) -> bool:
    """Requested compilation diagnostics require execution, including empty values."""
    if env is None:
        env = os.environ
    return any(name in env for name in environment_keys("diagnostic"))


def codegen_environment_inputs(
    *, is_wasm: bool, env: Mapping[str, str]
) -> dict[str, str]:
    keys = environment_keys(
        "common",
        "native",
        "runtime_identity",
        "build_identity",
        "diagnostic",
        "observation",
    )
    if is_wasm:
        keys += environment_keys("wasm")
    return {name: env[name] for name in sorted(keys) if name in env}


def backend_debug_artifact_dir(env: Mapping[str, str]) -> Path:
    """Return the directory the backend writes debug artifacts to.

    An explicit ``MOLT_DEBUG_ARTIFACT_DIR`` wins, made absolute so a backend
    daemon with another working directory agrees. Otherwise it is the
    compiler's run scratch, ``molt.dx.scratch_dir(<compiler source>,
    "molt-backend")``: never inside a checkout, and one directory per compiler
    and artifact root, so the cache keys that bind it stay stable.
    """
    raw = env.get(DEBUG_ARTIFACT_DIR_ENV, "").strip()
    if raw:
        return Path(raw).expanduser().absolute()
    return scratch_dir(compiler_source_root(), DEBUG_ARTIFACT_SCRATCH_PURPOSE, env)


@dataclass(frozen=True, slots=True)
class CodegenSelection:
    """Backend-process inputs that molt resolves from flags and configuration.

    The CLI never writes these to ``os.environ``. A build carries one selection
    and projects it into the backend process environment, and into every cache
    key and daemon identity that binds that environment, through
    ``environment``. An unset optional field leaves the caller's value of that
    variable in place: ``MOLT_PORTABLE=0`` still opts in to host-CPU code.
    The projection always names the backend's debug artifact directory
    (`backend_debug_artifact_dir`); the backend never derives one itself.
    """

    backend: CodegenBackend = DEFAULT_CODEGEN_BACKEND
    portable: bool = False
    wasm_profile: str | None = None
    type_gate: bool = False

    def __post_init__(self) -> None:
        if self.backend not in CODEGEN_BACKENDS:
            raise ValueError(f"unknown codegen backend: {self.backend!r}")
        if self.wasm_profile is not None and not self.wasm_profile:
            raise ValueError("an empty wasm profile selects nothing; use None")

    def environment(self, base: Mapping[str, str]) -> dict[str, str]:
        """Return a copy of ``base`` with this selection applied."""
        env = dict(base)
        env["MOLT_BACKEND"] = self.backend
        env[DEBUG_ARTIFACT_DIR_ENV] = str(backend_debug_artifact_dir(base))
        if self.portable:
            env["MOLT_PORTABLE"] = "1"
        if self.wasm_profile is not None:
            env["MOLT_WASM_PROFILE"] = self.wasm_profile
        if self.type_gate:
            env["MOLT_TYPE_GATE"] = "1"
        return env
