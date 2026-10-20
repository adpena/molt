from __future__ import annotations

from molt.compilation_target import Target, TargetKind, admit_target

import os
import tomllib
from pathlib import Path
from typing import Any, Literal, Mapping, cast, get_args

from molt.backend_executable_names import (
    CODEGEN_BACKENDS,
    DEFAULT_CODEGEN_BACKEND,
    CodegenBackend,
)
from molt.capability_policy import CapabilityInput


def _load_molt_config(project_root: Path) -> dict[str, Any]:
    """The project's `molt.toml` merged with `[tool.molt]` from
    `pyproject.toml`; unreadable or malformed files contribute nothing."""
    config: dict[str, Any] = {}
    molt_toml = project_root / "molt.toml"
    if molt_toml.exists():
        try:
            config.update(tomllib.loads(molt_toml.read_text(encoding="utf-8")))
        except (OSError, tomllib.TOMLDecodeError):
            pass
    pyproject = project_root / "pyproject.toml"
    if pyproject.exists():
        try:
            data = tomllib.loads(pyproject.read_text(encoding="utf-8"))
        except (OSError, tomllib.TOMLDecodeError):
            data = {}
        tool_cfg = data.get("tool", {}).get("molt", {})
        if tool_cfg:
            config.setdefault("tool", {})
            config["tool"].setdefault("molt", {})
            config["tool"]["molt"].update(tool_cfg)
    return config


ENTRY_OVERRIDE_ENV = "MOLT_ENTRY_MODULE"
STATIC_IMPORT_MODULES_ENV = "MOLT_STATIC_IMPORT_MODULES"

# --- stdlib_profile: the single config authority (doctrine D5, §4.4) ----------
#
# `stdlib_profile` is the user-facing runtime stdlib intent. The default
# "auto" means "choose the smallest concrete runtime tier that satisfies the
# reached-intrinsic feature set"; named tiers are explicit ceilings. This used
# to be resolved at four independent sites that each carried their own literal
# "micro" default (the build dispatcher, the `build()` API kwarg, the internal
# batch-server normalizer, and the module-graph closure reader). Those defaults
# could desync: the module-graph reader (`module_stdlib_policy`) reads
# `MOLT_STDLIB_PROFILE` to decide which modules enter the dependency closure,
# while the runtime-staticlib selector consumes the resolved kwarg to decide
# which prebuilt `.a`/`.lib` to link. When the two disagree (env-only `full`
# pulling `hashlib`/crypto modules into the closure while a `micro` staticlib is
# linked) the link fails on undefined full-profile intrinsics
# (`molt_pbkdf2_hmac`, `molt_scrypt`, ...).
#
# This module owns the closed profile types, their projected choices, the default,
# and the ONE precedence order. Every consumer routes through
# `resolve_stdlib_profile`. `build()` passes the resolved intent as a parameter
# to module-graph construction and to the runtime selector; the CLI never
# writes it back to the process environment. After backend IR reachability is
# known, `runtime_features` resolves that intent to one concrete runtime tier
# for artifact selection.
MOLT_STDLIB_PROFILE_ENV = "MOLT_STDLIB_PROFILE"
AUTO_STDLIB_PROFILE = "auto"
RuntimeStdlibProfile = Literal["micro", "edge", "standard", "server", "full"]
StdlibProfile = Literal["auto", RuntimeStdlibProfile]
RUNTIME_STDLIB_PROFILE_TIERS: tuple[RuntimeStdlibProfile, ...] = get_args(
    RuntimeStdlibProfile
)
DEFAULT_RUNTIME_STDLIB_PROFILE = RUNTIME_STDLIB_PROFILE_TIERS[0]
STDLIB_PROFILE_CHOICES: tuple[StdlibProfile, ...] = get_args(StdlibProfile)
DEFAULT_STDLIB_PROFILE = AUTO_STDLIB_PROFILE


def resolve_stdlib_profile(
    *,
    flag: str | None,
    build_cfg: Mapping[str, Any] | None = None,
    deploy_defaults: Mapping[str, Any] | None = None,
    env: Mapping[str, str] | None = None,
) -> tuple[StdlibProfile, str]:
    """Resolve the effective stdlib profile and report its provenance.

    Precedence (highest first):

    1. ``--stdlib-profile`` CLI flag.
    2. ``MOLT_STDLIB_PROFILE`` environment variable.
    3. ``[tool.molt.build].stdlib-profile`` / ``stdlib_profile`` toml config.
    4. The selected deploy-profile default.
    5. :data:`DEFAULT_STDLIB_PROFILE` (``"auto"``).

    The env var outranks toml/deploy/default. The resolved value then travels
    as a parameter to the module-graph closure and the runtime-staticlib
    selection, so both derive from the same value. Invalid values at any layer
    are ignored in favor of the next.

    Returns ``(profile, source)`` where ``source`` is one of ``"flag"``,
    ``"env"``, ``"config"``, ``"deploy-profile"``, or ``"default"``.
    """

    if isinstance(flag, str) and flag in STDLIB_PROFILE_CHOICES:
        return flag, "flag"

    env_map = os.environ if env is None else env
    env_value = env_map.get(MOLT_STDLIB_PROFILE_ENV)
    if env_value in STDLIB_PROFILE_CHOICES:
        return env_value, "env"

    if build_cfg is not None:
        cfg_value = build_cfg.get("stdlib_profile")
        if cfg_value is None:
            cfg_value = build_cfg.get("stdlib-profile")
        if isinstance(cfg_value, str) and cfg_value in STDLIB_PROFILE_CHOICES:
            return cast(StdlibProfile, cfg_value), "config"

    if deploy_defaults is not None:
        deploy_value = deploy_defaults.get("stdlib_profile")
        if isinstance(deploy_value, str) and deploy_value in STDLIB_PROFILE_CHOICES:
            return cast(StdlibProfile, deploy_value), "deploy-profile"

    return DEFAULT_STDLIB_PROFILE, "default"


def _config_value(config: Mapping[str, Any], path: list[str]) -> Any | None:
    current: Any = config
    for key in path:
        if not isinstance(current, Mapping) or key not in current:
            return None
        current = current[key]
    return current


def _coerce_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return default


def _resolve_command_config(config: Mapping[str, Any], command: str) -> dict[str, Any]:
    cmd_cfg: dict[str, Any] = {}
    direct = _config_value(config, [command])
    if isinstance(direct, Mapping):
        cmd_cfg.update(direct)
    tool_cfg = _config_value(config, ["tool", "molt", command])
    if isinstance(tool_cfg, Mapping):
        cmd_cfg.update(tool_cfg)
    return cmd_cfg


def _resolve_build_config(config: Mapping[str, Any]) -> dict[str, Any]:
    build_config = _resolve_command_config(config, "build")
    if "fallback" in build_config:
        raise ValueError(
            "build configuration 'fallback' was removed; unsupported constructs fail at compile time"
        )
    return build_config


def _resolve_capabilities_config(
    config: Mapping[str, Any],
) -> CapabilityInput | None:
    for path in (["capabilities"], ["tool", "molt", "capabilities"]):
        caps = _config_value(config, path)
        if isinstance(caps, (list, str, dict)):
            return caps
    return None


def _select_capability_input(
    *candidates: CapabilityInput | None,
) -> CapabilityInput | None:
    """Select the first present policy while preserving explicit deny-all values."""
    return next((candidate for candidate in candidates if candidate is not None), None)


def _select_codegen_backend(
    target: str, backend_choice: str
) -> tuple[Target, CodegenBackend, str | None]:
    """Canonicalize ``--target llvm``/``--backend``; return target and backend.

    `--target llvm` is an alias for "native binary, LLVM backend": the LLVM
    backend emits host-native objects, so the runtime staticlib and the entire
    native link path are identical to `--target native`; the only difference
    is the codegen backend. Canonicalize it to `TargetKind.NATIVE`, preserving
    the shared runtime triple, stdlib object and native link policies, and
    return the backend separately.
    Without this, "llvm" leaks into the cargo `--target` slot, which expects a
    rustc target triple, and the runtime build fails with "could not find
    specification for target \"llvm\"".

    "auto" defaults to cranelift for all builds. LLVM remains opt-in until its
    end-to-end parity and operational tooling are on the same footing as the
    default Cranelift lane. `build`, `factgraph`, the batch build server and
    `internal-backend-build` all select here, so a prewarm builds the backend
    feature lane the build dispatches. The caller passes the backend to
    `build()`; the backend process receives it in its own environment mapping
    (`CodegenSelection.environment`). This function never writes `os.environ`.
    """
    if target == "llvm":
        if backend_choice not in {"auto", "llvm"}:
            return (
                TargetKind.NATIVE,
                DEFAULT_CODEGEN_BACKEND,
                (
                    "`--target llvm` selects the LLVM backend; it conflicts "
                    f"with `--backend {backend_choice}`. Use `--target native "
                    "--backend llvm` to mix, or drop one flag."
                ),
            )
        backend_choice = "llvm"
        target = "native"
    if backend_choice == "auto":
        backend_choice = DEFAULT_CODEGEN_BACKEND
    if backend_choice not in CODEGEN_BACKENDS:
        choices = ", ".join(("auto", *CODEGEN_BACKENDS))
        return (
            TargetKind.NATIVE,
            DEFAULT_CODEGEN_BACKEND,
            (f"Unknown backend {backend_choice!r}; expected one of: {choices}."),
        )
    try:
        selected_target = admit_target(target)
    except ValueError as exc:
        return TargetKind.NATIVE, backend_choice, str(exc)
    return selected_target, backend_choice, None
