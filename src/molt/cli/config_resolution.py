from __future__ import annotations

import os
from typing import Any, Mapping, cast

from molt.backend_executable_names import (
    CODEGEN_BACKENDS,
    DEFAULT_CODEGEN_BACKEND,
    CodegenBackend,
)
from molt.capability_policy import CapabilityInput

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
# This module is now the ONE place that knows the legal values, the ONE default,
# and the ONE precedence order. Every consumer routes through
# `resolve_stdlib_profile`. `build()` passes the resolved intent as a parameter
# to module-graph construction and to the runtime selector; the CLI never
# writes it back to the process environment. After backend IR reachability is
# known, `runtime_features` resolves that intent to one concrete runtime tier
# for artifact selection.
MOLT_STDLIB_PROFILE_ENV = "MOLT_STDLIB_PROFILE"
AUTO_STDLIB_PROFILE = "auto"
RUNTIME_STDLIB_PROFILE_TIERS: tuple[str, ...] = (
    "micro",
    "edge",
    "standard",
    "server",
    "full",
)
DEFAULT_RUNTIME_STDLIB_PROFILE = RUNTIME_STDLIB_PROFILE_TIERS[0]
STDLIB_PROFILE_CHOICES: tuple[str, ...] = (
    AUTO_STDLIB_PROFILE,
    *RUNTIME_STDLIB_PROFILE_TIERS,
)
DEFAULT_STDLIB_PROFILE = AUTO_STDLIB_PROFILE


def resolve_stdlib_profile(
    *,
    flag: str | None,
    build_cfg: Mapping[str, Any] | None = None,
    deploy_defaults: Mapping[str, Any] | None = None,
    env: Mapping[str, str] | None = None,
) -> tuple[str, str]:
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
            return cfg_value, "config"

    if deploy_defaults is not None:
        deploy_value = deploy_defaults.get("stdlib_profile")
        if isinstance(deploy_value, str) and deploy_value in STDLIB_PROFILE_CHOICES:
            return deploy_value, "deploy-profile"

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
    return _resolve_command_config(config, "build")


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
) -> tuple[str, CodegenBackend, str | None]:
    """Canonicalize ``--target llvm``/``--backend``; return target and backend.

    `--target llvm` is an alias for "native binary, LLVM backend": the LLVM
    backend emits host-native objects, so the runtime staticlib and the entire
    native link path are identical to `--target native`; the only difference
    is the codegen backend. Canonicalize it to the `native` target (so every
    downstream `target == "native"` branch - runtime triple, stdlib object
    split, native link driver - fires) and return the backend separately.
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
                target,
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
        return target, DEFAULT_CODEGEN_BACKEND, None
    if backend_choice not in CODEGEN_BACKENDS:
        choices = ", ".join(("auto", *CODEGEN_BACKENDS))
        return (
            target,
            DEFAULT_CODEGEN_BACKEND,
            (f"Unknown backend {backend_choice!r}; expected one of: {choices}."),
        )
    return target, cast(CodegenBackend, backend_choice), None
