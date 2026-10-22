"""Shared backend environment contract, also embedded by molt-ir.

Only backend_environment.json authors membership. Keep absent and empty values
distinct: several pass controls intentionally test presence, not truthiness.
"""

from __future__ import annotations

import json
import os
from dataclasses import dataclass
from pathlib import Path
from typing import Literal, Mapping, cast

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


@dataclass(frozen=True, slots=True)
class _EnvironmentGroup:
    request_scoped: bool
    semantic_target: Literal["native", "wasm", "both", "none"]
    native_worker: bool
    projection: Literal["raw", "presence"]
    diagnostic_bypass: bool
    request_reuse_only: bool
    keys: tuple[str, ...]


def _parse_catalog(catalog: object) -> dict[str, _EnvironmentGroup]:
    if (
        not isinstance(catalog, dict)
        or set(catalog) != {"schema", "groups"}
        or type(catalog["schema"]) is not int
        or catalog["schema"] != 2
        or not isinstance(catalog["groups"], dict)
        or not catalog["groups"]
    ):
        raise ValueError("unsupported backend environment catalog")
    groups: dict[str, _EnvironmentGroup] = {}
    seen: set[str] = set()
    for name, role in catalog["groups"].items():
        if (
            not isinstance(name, str)
            or not name
            or not name.isascii()
            or not all(
                part and part.isalpha() and part.islower() for part in name.split("_")
            )
            or not isinstance(role, dict)
            or set(role)
            != {
                "request_scoped",
                "semantic_target",
                "native_worker",
                "projection",
                "diagnostic_bypass",
                "request_reuse_only",
                "keys",
            }
            or any(
                type(role[field]) is not bool
                for field in (
                    "request_scoped",
                    "native_worker",
                    "diagnostic_bypass",
                    "request_reuse_only",
                )
            )
            or role["semantic_target"] not in ("native", "wasm", "both", "none")
            or role["projection"] not in ("raw", "presence")
            or (
                role["request_reuse_only"]
                and (
                    not role["request_scoped"]
                    or role["native_worker"]
                    or role["semantic_target"] == "none"
                )
            )
            or (
                role["native_worker"]
                and role["semantic_target"] not in ("native", "both")
            )
            or not isinstance(role["keys"], list)
            or not role["keys"]
        ):
            raise ValueError("malformed backend environment catalog group")
        for key in role["keys"]:
            if (
                not isinstance(key, str)
                or not key
                or not key.isascii()
                or not all(
                    char.isupper() or char.isdigit() or char == "_" for char in key
                )
                or key in seen
            ):
                raise ValueError(
                    "malformed or duplicate backend environment catalog entry"
                )
            seen.add(key)
        groups[name] = _EnvironmentGroup(
            request_scoped=role["request_scoped"],
            semantic_target=cast(
                Literal["native", "wasm", "both", "none"], role["semantic_target"]
            ),
            native_worker=role["native_worker"],
            projection=cast(Literal["raw", "presence"], role["projection"]),
            diagnostic_bypass=role["diagnostic_bypass"],
            request_reuse_only=role["request_reuse_only"],
            keys=tuple(role["keys"]),
        )
    return groups


_CATALOG = _parse_catalog(
    json.loads(Path(__file__).with_suffix(".json").read_text(encoding="utf-8"))
)
_REQUEST_KEYS = tuple(
    key for role in _CATALOG.values() if role.request_scoped for key in role.keys
)
_DIAGNOSTIC_KEYS = tuple(
    key for role in _CATALOG.values() if role.diagnostic_bypass for key in role.keys
)


def environment_keys(*groups: str) -> tuple[str, ...]:
    return tuple(name for group in groups for name in _CATALOG[group].keys)


def request_environment_keys() -> tuple[str, ...]:
    """Raw settings the catalog permits a daemon request to carry."""
    return _REQUEST_KEYS


def request_environment_inputs(env: Mapping[str, str]) -> dict[str, str]:
    return {name: env[name] for name in _REQUEST_KEYS if name in env}


def compilation_diagnostics_requested(env: Mapping[str, str] | None = None) -> bool:
    """Requested compilation diagnostics require execution, including empty values."""
    if env is None:
        env = os.environ
    return any(name in env for name in _DIAGNOSTIC_KEYS)


def _role_inputs(
    env: Mapping[str, str], roles: list[_EnvironmentGroup]
) -> dict[str, str | bool]:
    inputs: dict[str, str | bool] = {}
    for role in roles:
        for name in role.keys:
            if name in env:
                inputs[name] = True if role.projection == "presence" else env[name]
    return dict(sorted(inputs.items()))


def codegen_environment_inputs(
    *, is_wasm: bool, env: Mapping[str, str]
) -> dict[str, str | bool]:
    """Project actual operation settings, excluding transport and observation."""
    target = "wasm" if is_wasm else "native"
    return _role_inputs(
        env,
        [
            role
            for role in _CATALOG.values()
            if not role.request_reuse_only and role.semantic_target in (target, "both")
        ],
    )


def daemon_process_environment_inputs(env: Mapping[str, str]) -> dict[str, str | bool]:
    """Project the settings that fix one backend daemon process.

    The daemon strips every request-scoped setting from its environment when
    it starts (``serve_requests``) and reads each request's settings from that
    request, so one daemon serves any mix of programs and codegen settings.
    Only process-scoped roles may name it: a request-scoped value in its
    identity starts one daemon per value, such as one per entry module.
    """
    return _role_inputs(
        env, [role for role in _CATALOG.values() if not role.request_scoped]
    )


def native_object_environment_inputs(env: Mapping[str, str]) -> dict[str, str | bool]:
    """Project the settings one self-contained native object job reads.

    These are the native-worker roles, the values molt-ir's
    ``NativeCodegenEnvironment`` captures into every batch job. The program's
    context (entry module, stdlib module set) only chooses which functions
    form each job; it never reaches the worker. Objects shared across programs
    bind this projection, so a new import cannot invalidate their machine code.
    """
    return _role_inputs(env, [role for role in _CATALOG.values() if role.native_worker])


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
    and projects it into the backend's request environment, and into every
    cache key that binds that environment, through ``environment``. These
    settings are request-scoped, so they never name a daemon process
    (``daemon_process_environment_inputs``). An unset optional field leaves
    the caller's value of that variable in place: ``MOLT_PORTABLE=0`` still
    opts in to host-CPU code.
    The projection always names the backend's debug artifact directory
    (`backend_debug_artifact_dir`); the backend never derives one itself.
    """

    backend: CodegenBackend = DEFAULT_CODEGEN_BACKEND
    portable: bool = False
    wasm_profile: str | None = None

    def __post_init__(self) -> None:
        if self.backend not in CODEGEN_BACKENDS:
            raise ValueError(f"unknown codegen backend: {self.backend!r}")
        if self.wasm_profile is not None and not self.wasm_profile:
            raise ValueError("an empty wasm profile selects nothing; use None")

    def environment(
        self, base: Mapping[str, str], *, profile: str | None = None
    ) -> dict[str, str]:
        """Return a copy of ``base`` with this selection applied."""
        if base.get("MOLT_GUARD_SCRATCH_ROOT"):
            from molt.temporary_artifacts import guard_scratch

            # Admission validates inherited scratch; the actual one-shot guard
            # may replace it with its own allocation at child launch. Suite
            # daemons remove it before acquiring their longer-lived temp root.
            guard_scratch(base)
        env = dict(base)
        if profile in ("release-size", "wasm-release"):
            env.setdefault("MOLT_BACKEND_OPT_LEVEL", "speed_and_size")
        env["MOLT_BACKEND"] = self.backend
        env[DEBUG_ARTIFACT_DIR_ENV] = str(backend_debug_artifact_dir(base))
        if self.portable:
            env["MOLT_PORTABLE"] = "1"
        if self.wasm_profile is not None:
            env["MOLT_WASM_PROFILE"] = self.wasm_profile
        return env
