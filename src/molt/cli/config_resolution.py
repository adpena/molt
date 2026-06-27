from __future__ import annotations

import os
from typing import Any, Mapping

from molt.cli.capability_spec import CapabilityInput

ENTRY_OVERRIDE_ENV = "MOLT_ENTRY_MODULE"
STATIC_IMPORT_MODULES_ENV = "MOLT_STATIC_IMPORT_MODULES"

# --- stdlib_profile: the single config authority (doctrine D5, §4.4) ----------
#
# `stdlib_profile` selects which runtime stdlib closure is compiled into the
# binary: "micro" (core only, smallest binary) or "full" (all modules). It used
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
# `resolve_stdlib_profile` (resolution) or `MOLT_STDLIB_PROFILE_ENV` +
# `DEFAULT_STDLIB_PROFILE` (the env transport read by the module-graph closure).
# The resolved value is re-exported to `MOLT_STDLIB_PROFILE` before module-graph
# construction, so the closure reader and the staticlib selector can never
# observe different values.
MOLT_STDLIB_PROFILE_ENV = "MOLT_STDLIB_PROFILE"
STDLIB_PROFILE_CHOICES: tuple[str, ...] = ("micro", "full")
DEFAULT_STDLIB_PROFILE = "micro"


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
    4. The selected deploy-profile default (e.g. ``--profile wasi`` -> ``full``).
    5. :data:`DEFAULT_STDLIB_PROFILE`.

    The env var deliberately outranks toml/deploy/default: it is the transport
    signal the in-process module-graph closure reads directly, so honoring it
    here keeps the closure and the runtime-staticlib selection derived from the
    same value. Invalid values at any layer are ignored in favor of the next.

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


def _config_value(config: dict[str, Any], path: list[str]) -> Any | None:
    current: Any = config
    for key in path:
        if not isinstance(current, dict) or key not in current:
            return None
        current = current[key]
    return current


def _coerce_bool(value: Any, default: bool) -> bool:
    if isinstance(value, bool):
        return value
    if isinstance(value, str):
        return value.strip().lower() in {"1", "true", "yes", "on"}
    return default


def _resolve_command_config(config: dict[str, Any], command: str) -> dict[str, Any]:
    cmd_cfg: dict[str, Any] = {}
    direct = _config_value(config, [command])
    if isinstance(direct, dict):
        cmd_cfg.update(direct)
    tool_cfg = _config_value(config, ["tool", "molt", command])
    if isinstance(tool_cfg, dict):
        cmd_cfg.update(tool_cfg)
    return cmd_cfg


def _resolve_build_config(config: dict[str, Any]) -> dict[str, Any]:
    return _resolve_command_config(config, "build")


def _resolve_capabilities_config(config: dict[str, Any]) -> CapabilityInput | None:
    for path in (["capabilities"], ["tool", "molt", "capabilities"]):
        caps = _config_value(config, path)
        if isinstance(caps, (list, str, dict)):
            return caps
    return None
